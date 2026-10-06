//! Lumen — a graphical UEFI boot picker.

#![no_std]
#![no_main]

extern crate alloc;

mod clock;
mod config;
mod discover;
mod gfx;
mod icons;
mod launch;
mod mouse;
mod os;
mod text;
mod ui;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use config::{Config, Resolution};
use core::time::Duration;
use discover::{Entry, SelfImage};
use gfx::Canvas;
use uefi::boot::{self, EventType, ScopedProtocol, TimerTrigger, Tpl};
use uefi::fs::{FileSystem, PathBuf};
use uefi::prelude::*;
use uefi::proto::console::gop::{BltOp, BltPixel, BltRegion, GraphicsOutput};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::{system, CString16};

// Entry point for the x86_64 build (targets/x86_64-lumen-uefi.json), which
// uses SSE2 for floating point. The UEFI spec requires firmware to enable
// SSE on x64, but some firmware forgets; a single SSE instruction would then
// crash. So before any Rust code runs: if CR4.OSFXSR is clear, enable SSE
// (CR4.OSFXSR|OSXMMEXCPT, CR0.EM off, CR0.MP on, default MXCSR), then jump
// to the normal entry with the firmware's arguments (RCX, RDX) untouched.
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".globl lumen_entry",
    "lumen_entry:",
    "    mov rax, cr4",
    "    test eax, 0x200",
    "    jnz 2f",
    "    or rax, 0x600",
    "    mov cr4, rax",
    "    mov rax, cr0",
    "    and rax, -5",
    "    or rax, 2",
    "    mov cr0, rax",
    "    sub rsp, 8",
    "    mov dword ptr [rsp], 0x1f80",
    "    ldmxcsr [rsp]",
    "    add rsp, 8",
    "2:",
    "    jmp efi_main",
);

/// Secure Boot Advanced Targeting metadata. shim refuses to start a second
/// stage without it; the generation number lets a vulnerable Lumen release be
/// revoked without revoking the signing key.
#[used]
#[unsafe(link_section = ".sbat")]
static SBAT: [u8; include_bytes!("../sbat.csv").len()] = *include_bytes!("../sbat.csv");

/// Referenced by the toolchain for UCS-2 strings on UEFI but not provided
/// by `compiler_builtins` for this target.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn wcslen(s: *const u16) -> usize {
    let mut n = 0;
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// Never strand the machine: on a crash, show the message briefly and exit
/// back to the firmware, which then tries the next entry in its boot order
/// (normally Windows Boot Manager or GRUB).
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Lumen never exits boot services, so they are always available here.
    system::with_stdout(|o| {
        let _ = o.clear();
        let _ = core::fmt::Write::write_fmt(o, format_args!("Lumen hit an error and is handing back to your firmware.\r\n\r\n{info}\r\n"));
    });
    boot::stall(Duration::from_secs(4));
    let _ = unsafe { boot::exit(boot::image_handle(), Status::ABORTED, 0, core::ptr::null_mut()) };
    loop {
        core::hint::spin_loop();
    }
}

struct Display {
    gop: ScopedProtocol<GraphicsOutput>,
    canvas: Canvas,
    backdrop: Vec<u32>,
    /// What's currently on screen, so only changed areas are sent to the
    /// graphics card (framebuffer writes are slow on real hardware). Empty
    /// means unknown: the next present sends the whole frame.
    shown: Vec<u32>,
}

impl Display {
    fn new(res: &Resolution) -> Option<Self> {
        let handle = boot::get_handle_for_protocol::<GraphicsOutput>().ok()?;
        let mut gop = discover::open::<GraphicsOutput>(handle)?;
        let (cw, ch) = gop.current_mode_info().resolution();
        let want = match res {
            Resolution::Keep if cw >= 1024 => None,
            // Low default modes look awful; pick the best up to 1080p.
            Resolution::Keep => gop
                .modes()
                .filter(|m| {
                    let (w, h) = m.info().resolution();
                    w <= 1920 && h <= 1200
                })
                .max_by_key(|m| m.info().resolution().0 * m.info().resolution().1),
            Resolution::Max => gop.modes().max_by_key(|m| m.info().resolution().0 * m.info().resolution().1),
            Resolution::Exact(w, h) => gop.modes().find(|m| m.info().resolution() == (*w, *h)),
        };
        if let Some(mode) = want {
            if mode.info().resolution() != (cw, ch) {
                let _ = gop.set_mode(&mode);
            }
        }
        let mut d = Self { gop, canvas: Canvas::new(1, 1), backdrop: Vec::new(), shown: Vec::new() };
        d.sync_mode();
        Some(d)
    }

    /// Rebuilds buffers if the mode changed (e.g. a loader we started
    /// switched resolution and then returned to us).
    fn sync_mode(&mut self) {
        let (w, h) = self.gop.current_mode_info().resolution();
        if (w, h) != (self.canvas.w, self.canvas.h) || self.backdrop.is_empty() {
            self.canvas = Canvas::new(w, h);
            gfx::render_backdrop(&mut self.canvas);
            self.backdrop = self.canvas.px.clone();
        }
        // Something else (a loader that returned, a mode change) may have
        // drawn on the screen since.
        self.shown.clear();
    }

    /// Sends the canvas to the screen. Only the rows that changed since the
    /// last present are sent, grouped into bands, each trimmed to the
    /// columns that changed.
    /// Returns how many pixels were sent.
    fn present(&mut self) -> usize {
        let (w, h) = (self.canvas.w, self.canvas.h);
        if self.shown.len() != w * h {
            self.blit(0, 0, w, h);
            self.shown = self.canvas.px.clone();
            return w * h;
        }
        let mut sent = 0;
        let mut y = 0;
        while y < h {
            let row = |y: usize| y * w..(y + 1) * w;
            if self.canvas.px[row(y)] == self.shown[row(y)] {
                y += 1;
                continue;
            }
            // A band of consecutive changed rows and its changed columns.
            let (mut x0, mut x1, y0) = (w, 0, y);
            while y < h && self.canvas.px[row(y)] != self.shown[row(y)] {
                let (new, old) = (&self.canvas.px[row(y)], &self.shown[row(y)]);
                if let Some(first) = new.iter().zip(old).position(|(a, b)| a != b) {
                    let last = w - 1 - new.iter().rev().zip(old.iter().rev()).position(|(a, b)| a != b).unwrap_or(0);
                    x0 = x0.min(first);
                    x1 = x1.max(last + 1);
                }
                y += 1;
            }
            self.blit(x0, y0, x1 - x0, y - y0);
            sent += (x1 - x0) * (y - y0);
            for yy in y0..y {
                self.shown[yy * w + x0..yy * w + x1].copy_from_slice(&self.canvas.px[yy * w + x0..yy * w + x1]);
            }
        }
        sent
    }

    fn blit(&mut self, x: usize, y: usize, bw: usize, bh: usize) {
        if bw == 0 || bh == 0 {
            return;
        }
        let w = self.canvas.w;
        // 0x00RRGGBB little-endian is byte-for-byte a BltPixel (B, G, R, x).
        let buf = unsafe { core::slice::from_raw_parts(self.canvas.px.as_ptr().cast::<BltPixel>(), self.canvas.px.len()) };
        let _ = self.gop.blt(BltOp::BufferToVideo {
            buffer: buf,
            src: BltRegion::SubRectangle { coords: (x, y), px_stride: w },
            dest: (x, y),
            dims: (bw, bh),
        });
    }

    /// Fade the current frame to black.
    fn fade_out(&mut self) {
        let frame = self.canvas.px.clone();
        let t0 = clock::now();
        loop {
            let t = ((clock::now() - t0) / 0.28) as f32;
            self.canvas.px.copy_from_slice(&frame);
            self.canvas.fade(1.0 - t.min(1.0));
            let _ = self.present();
            if t >= 1.0 {
                break;
            }
        }
    }

    fn fade_in(&mut self, dur: f64) {
        let frame = self.canvas.px.clone();
        let t0 = clock::now();
        loop {
            let t = ((clock::now() - t0) / dur) as f32;
            self.canvas.px.copy_from_slice(&frame);
            self.canvas.fade(t.min(1.0));
            let _ = self.present();
            if t >= 1.0 {
                break;
            }
        }
    }
}

fn load_config(me: &SelfImage) -> Config {
    let Some(sfs) = me.device.and_then(discover::open::<SimpleFileSystem>) else { return Config::default() };
    let mut fs = FileSystem::new(sfs);
    let path = format!("{}\\lumen.conf", me.dir());
    CString16::try_from(path.as_str())
        .ok()
        .and_then(|p| fs.read_to_string(PathBuf::from(p)).ok())
        .map(|s| Config::parse(&s))
        .unwrap_or_default()
}

fn default_index(entries: &[Entry], cfg: &Config) -> usize {
    let want = cfg.default.to_lowercase();
    if want == "last" {
        if let Some(last) = launch::last_choice() {
            if let Some(i) = entries.iter().position(|e| e.id == last) {
                return i;
            }
        }
        0
    } else {
        entries.iter().position(|e| e.title.to_lowercase().contains(&want)).unwrap_or(0)
    }
}

/// Cheap fingerprint of attached storage and pointing devices, polled to
/// notice hot-plugged USB sticks and mice.
fn device_counts() -> (usize, usize, usize) {
    use uefi::proto::console::pointer::{AbsolutePointer, Pointer};
    use uefi::proto::media::block::BlockIO;
    let n = |r: uefi::Result<Vec<uefi::Handle>>| r.map(|v| v.len()).unwrap_or(0);
    (
        n(boot::find_handles::<BlockIO>()) + n(boot::find_handles::<SimpleFileSystem>()),
        n(boot::find_handles::<Pointer>()),
        n(boot::find_handles::<AbsolutePointer>()),
    )
}

fn describe(status: Status) -> &'static str {
    match status {
        Status::SECURITY_VIOLATION | Status::ACCESS_DENIED => "it was blocked by Secure Boot",
        Status::NOT_FOUND => "the loader file is missing",
        Status::LOAD_ERROR | Status::UNSUPPORTED => "it isn't a valid EFI program for this PC",
        Status::DEVICE_ERROR => "the disk couldn't be read",
        _ => "the loader reported an error",
    }
}

/// Boots an entry, falling back to the firmware's own boot entry if direct
/// chainloading fails. Returns an error message if nothing worked.
fn boot_entry(display: &mut Display, entry: &Entry, cfg: &Config) -> String {
    log::info!("starting {:?} ({})", entry.title, entry.file);
    launch::remember(entry);
    display.fade_out();

    let title = entry.title.to_lowercase();
    let prefer_firmware = entry.nvram_only || cfg.bootnext.iter().any(|b| title.contains(b.as_str()));
    if let (true, Some(num)) = (prefer_firmware, entry.boot_option) {
        let status = launch::boot_next(num);
        return format!("Couldn't start {}: firmware refused ({status:?})", entry.title);
    }

    if cfg.stay_default {
        launch::arm_return_to_lumen();
    }
    let _ = system::with_stdout(|o| o.clear());
    match launch::start(entry) {
        // The loader ran and came back (e.g. the user left the UEFI shell).
        launch::Outcome::Exited => String::new(),
        launch::Outcome::Refused(status) => {
            // Hand over to the firmware's own entry for this OS, which boots
            // it exactly the way the firmware would on its own.
            if let Some(num) = entry.boot_option {
                let _ = launch::boot_next(num);
            }
            format!("Couldn't start {} — {}", entry.title, describe(status))
        }
    }
}

#[entry]
fn main() -> Status {
    if uefi::helpers::init().is_err() {
        return Status::ABORTED;
    }
    // The logger writes to the text console, which would draw over the UI;
    // diagnostics are only wanted in debug-console test builds.
    if !cfg!(feature = "debugcon") {
        log::set_max_level(log::LevelFilter::Off);
    }
    // Firmware arms a 5-minute watchdog before running boot options.
    let _ = boot::set_watchdog_timer(0, 0x10000, None);
    clock::init();
    let _ = system::with_stdout(|o| o.enable_cursor(false));

    let me = SelfImage::get();
    let cfg = load_config(&me);
    let Some(mut display) = Display::new(&cfg.resolution) else {
        // No graphics: there's nothing to draw on, so boot the default.
        discover::connect_all();
        let entries = discover::scan(&me, &cfg);
        if let Some(e) = entries.get(default_index(&entries, &cfg)) {
            let _ = launch::start(e);
        }
        return Status::NOT_FOUND;
    };
    display.canvas.px.copy_from_slice(&display.backdrop);
    display.fade_in(0.25);
    // We can draw on this machine: record that, and only then claim (or
    // reclaim) first place in the firmware boot order.
    launch::mark_healthy();
    launch::heal_boot_order();

    discover::connect_all();
    let entries = discover::scan(&me, &cfg);
    let sel = default_index(&entries, &cfg);

    // Timeout 0: boot straight away, unless a key is already being held.
    let key_held = system::with_stdin(|i| i.read_key().ok().flatten()).is_some();
    // The delay chosen with Lumen's Auto-start button wins over lumen.conf.
    let timeout = launch::saved_auto_start().unwrap_or(cfg.timeout);
    if timeout == 0 && !key_held && !entries.is_empty() {
        boot_entry(&mut display, &entries[sel], &cfg);
        display.sync_mode();
    }

    let mut text = text::Text::new();
    let mut ui = ui::Ui::new(entries, sel, launch::firmware_setup_supported(), clock::now());
    ui.show_clock = cfg.clock;
    ui.set_auto_start(if timeout > 0 { timeout } else { -1 });
    if timeout > 0 && !key_held {
        ui.start_countdown(clock::now(), timeout as f64);
    }

    let mut mouse = mouse::Mouse::new(display.canvas.w, display.canvas.h);
    let mut devices = device_counts();
    let mut next_device_check = clock::now() + 1.5;

    let timer = unsafe { boot::create_event(EventType::TIMER, Tpl::CALLBACK, None, None) }.expect("timer event");
    let _ = boot::set_timer(&timer, TimerTrigger::Periodic(Duration::from_micros(8333)));
    let mut last = clock::now();
    let mut stats = (0.0f32, 0.0f32, 0.0f32); // draw ms, present ms, % sent
    let mut last_full = clock::now();

    loop {
        let _ = boot::wait_for_event(&[unsafe { timer.unsafe_clone() }]);
        let now = clock::now();
        let dt = ((now - last) as f32).min(0.1);
        last = now;

        let mut command = ui::Command::None;
        while let Some(key) = system::with_stdin(|i| i.read_key().ok().flatten()) {
            command = ui.key(key);
            if !matches!(command, ui::Command::None) {
                break;
            }
        }
        if matches!(command, ui::Command::None) {
            let ev = mouse.poll(display.canvas.w, display.canvas.h, ui::Ui::scale(display.canvas.w, display.canvas.h));
            if ev.scroll != 0 {
                ui.scroll(ev.scroll);
            }
            if ev.clicked {
                command = ui.pointer_clicked(mouse.x, mouse.y);
            } else if ev.moved {
                ui.pointer_moved(mouse.x, mouse.y);
            }
        }

        // Hot-plug: a USB stick or mouse plugged in while the menu is open.
        if now >= next_device_check {
            next_device_check = now + 1.5;
            let counts = device_counts();
            if counts != devices {
                devices = counts;
                command = ui::Command::Rescan;
            }
        }
        if ui.countdown_done(now) {
            command = ui::Command::Boot(ui.selected());
        }

        match command {
            ui::Command::None => {}
            ui::Command::Boot(i) => {
                let err = boot_entry(&mut display, &ui.entries[i], &cfg);
                display.sync_mode();
                let _ = system::with_stdout(|o| o.enable_cursor(false));
                if !err.is_empty() {
                    ui.toast(err, clock::now());
                }
                ui.draw(&mut display.canvas, &display.backdrop, &mut text, clock::now(), 0.0);
                display.fade_in(0.25);
            }
            ui::Command::Act(ui::Action::AutoStart) => {
                let secs = ui::next_auto_start(ui.auto_start);
                launch::save_auto_start(secs);
                ui.set_auto_start(secs);
                let msg = if secs < 0 {
                    String::from("Lumen will wait until you choose")
                } else {
                    format!("Lumen will start the last-used system after {secs} s")
                };
                ui.notify(msg, clock::now());
            }
            ui::Command::Act(action) => {
                display.fade_out();
                match action {
                    ui::Action::AutoStart => {}
                    ui::Action::Firmware => {
                        let s = launch::reboot_to_firmware();
                        ui.toast(format!("Couldn't open firmware settings ({s:?})"), clock::now());
                    }
                    ui::Action::Restart => launch::restart(),
                    ui::Action::Shutdown => launch::shutdown(),
                }
            }
            ui::Command::Rescan => {
                discover::connect_all();
                let entries = discover::scan(&me, &cfg);
                let fresh = ui.set_entries(entries, clock::now());
                match fresh.as_slice() {
                    [] => {}
                    [one] => ui.notify(format!("Found {one}"), clock::now()),
                    many => ui.notify(format!("Found {} new entries", many.len()), clock::now()),
                }
                mouse.reopen();
                devices = device_counts();
            }
        }

        // Every couple of seconds, repaint and resend the whole screen in case
        // something else (firmware text output, a driver) drew over it; only
        // changed areas are sent otherwise.
        let refresh = now - last_full >= 2.0;
        if refresh {
            last_full = now;
            display.shown.clear();
        }
        if ui.update(dt, now) || refresh {
            let t0 = clock::now();
            ui.draw(&mut display.canvas, &display.backdrop, &mut text, now, dt);
            let t1 = clock::now();
            if cfg.debug {
                // Timing readout (`debug on` in lumen.conf): last frame's draw
                // and present times, and how much of the screen was sent.
                let (cw, ch) = (display.canvas.w, display.canvas.h);
                let line = format!(
                    "{}x{}  draw {:.1} ms  present {:.1} ms  sent {:.0}%",
                    cw, ch, stats.0, stats.1, stats.2
                );
                let size = 14.0 * ui::Ui::scale(cw, ch);
                display.canvas.rrect(12.0, ch as f32 - size * 2.6, size * 26.0, size * 1.9, 6.0, gfx::rgb(0, 0, 0), 0.6);
                text.draw(&mut display.canvas, text::Face::Body, size, &line, 22.0, ch as f32 - size * 1.15, gfx::rgb(255, 255, 255), 0.95);
            }
            let sent = display.present();
            let t2 = clock::now();
            let total = (display.canvas.w * display.canvas.h).max(1);
            stats = (((t1 - t0) * 1000.0) as f32, ((t2 - t1) * 1000.0) as f32, sent as f32 * 100.0 / total as f32);
        }
    }
}
