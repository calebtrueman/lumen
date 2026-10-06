//! Lumen for BIOS PCs (and UEFI PCs in legacy/CSM mode): the same menu as
//! the UEFI build, drawn with VESA graphics, on top of the PC's BIOS.

#![no_std]
#![no_main]

extern crate alloc;

mod bios;
mod boot;
mod clock;
mod common;
mod discover;
mod disk;
mod input;
mod mem;
mod power;
mod state;
mod trap;
mod video;

pub use lumen_core::{gfx, icons, os, text, ui};

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

core::arch::global_asm!(
    ".section .text.entry, \"ax\"",
    ".global lumen_start",
    "lumen_start:",
    "    mov esp, offset __stack_top",
    "    mov edi, offset __bss_start",
    "    mov ecx, offset __bss_end",
    "    sub ecx, edi",
    "    xor eax, eax",
    "    cld",
    "    rep stosb",
    "    call lumen_main",
    "2:  hlt",
    "    jmp 2b",
    ".section .bss.stack, \"aw\", @nobits",
    ".balign 16",
    "    .skip 0x100000",
    "__stack_top:",
);

#[unsafe(no_mangle)]
extern "C" fn lumen_main() -> ! {
    trap::init();
    let map = mem::e820();
    if !mem::init(&map) {
        fatal_static("not enough memory");
    }
    clock::init();
    dbg!("lumen: {} memory regions, heap ends at {:#x}", map.count, mem::heap_end());

    let Some(mut display) = video::Display::new() else {
        fatal("no VESA graphics mode");
    };
    dbg!("lumen: video {}x{} {}bpp", display.mode.width, display.mode.height, display.mode.bpp);
    display.canvas.px.copy_from_slice(&display.backdrop);
    display.present();

    let drives = disk::drives();
    for d in &drives {
        dbg!("lumen: drive {:#x} {} sectors of {} [{}]", d.number, d.sectors, d.sector_size, d.interface);
    }
    let mut entries = discover::scan(&drives);
    let mut saved = state::load();
    let sel = entries.iter().position(|e| e.card.id == saved.last).unwrap_or(0);

    // A saved Auto-start of 0 means "start at once" (unless a key is held).
    let mut text = text::Text::new();
    let mut ui = ui::Ui::new(cards(&entries), sel, false, clock::now());
    ui.wall_clock = clock::wall_clock;
    ui.set_auto_start(if saved.auto_start > 0 { saved.auto_start } else { -1 });
    if saved.auto_start > 0 && !entries.is_empty() {
        ui.start_countdown(clock::now(), saved.auto_start as f64);
    }
    let mut mouse = input::Mouse::new(display.canvas.w, display.canvas.h);
    dbg!("lumen: {} entries, mouse {}", entries.len(), mouse.present);

    let mut last = clock::now();
    let mut last_full = last;
    loop {
        let frame_start = clock::now();
        let now = frame_start;
        let dt = ((now - last) as f32).min(0.1);
        last = now;
        let mut command = ui::Command::None;
        while let Some(k) = input::key() {
            command = ui.key(k);
            if !matches!(command, ui::Command::None) {
                break;
            }
        }
        if matches!(command, ui::Command::None) {
            let ev = mouse.poll(display.canvas.w, display.canvas.h, ui::Ui::scale(display.canvas.w, display.canvas.h));
            if ev.clicked {
                command = ui.pointer_clicked(mouse.x, mouse.y);
            } else if ev.moved {
                ui.pointer_moved(mouse.x, mouse.y);
            }
        }
        if ui.countdown_done(now) {
            command = ui::Command::Boot(ui.selected());
        }
        match command {
            ui::Command::None => {}
            ui::Command::Boot(i) => {
                saved.last = entries[i].card.id.clone();
                state::save(&saved);
                let why = start(&entries[i].target, &drives, &display, &mut mouse);
                ui.toast(format!("Couldn't start {} — {why}", entries[i].card.title), clock::now());
                mouse = input::Mouse::new(display.canvas.w, display.canvas.h);
            }
            ui::Command::Act(ui::Action::AutoStart) => {
                let secs = ui::next_auto_start(ui.auto_start);
                saved.auto_start = secs;
                state::save(&saved);
                ui.set_auto_start(secs);
                let msg = if secs < 0 {
                    String::from("Lumen will wait until you choose")
                } else {
                    format!("Lumen will start the last-used system after {secs} s")
                };
                ui.notify(msg, clock::now());
            }
            ui::Command::Act(ui::Action::Restart) => {
                mouse.disable();
                power::restart();
            }
            ui::Command::Act(ui::Action::Shutdown) => {
                mouse.disable();
                power::shutdown();
                ui.toast(String::from("This PC couldn't be switched off by software"), clock::now());
            }
            ui::Command::Act(_) => {}
            ui::Command::Rescan => {
                let drives_now = disk::drives();
                entries = discover::scan(&drives_now);
                let fresh = ui.set_entries(cards(&entries), clock::now());
                match fresh.as_slice() {
                    [] => {}
                    [one] => ui.notify(format!("Found {one}"), clock::now()),
                    many => ui.notify(format!("Found {} new entries", many.len()), clock::now()),
                }
            }
        }
        // Repaint fully now and then, like the UEFI build.
        let refresh = now - last_full >= 2.0;
        if refresh {
            last_full = now;
            display.invalidate();
        }
        if ui.update(dt, now) || refresh {
            ui.draw(&mut display.canvas, &display.backdrop, &mut text, now, dt);
            display.present();
        }
        while clock::now() - frame_start < 1.0 / 120.0 {
            core::hint::spin_loop();
        }
    }
}

fn cards(entries: &[discover::Entry]) -> Vec<ui::Card> {
    entries.iter().map(|e| e.card.clone()).collect()
}

/// Start an entry. Returns why it couldn't be started.
fn start(target: &discover::Target, drives: &[disk::Drive], display: &video::Display, mouse: &mut input::Mouse) -> String {
    use discover::Target;
    match target {
        Target::Mbr { drive, original } => {
            mouse.disable();
            let saved = if *original { discover::original_mbr() } else { None };
            if *original && saved.is_none() {
                return String::from("Lumen's copy of the original boot code is missing");
            }
            boot::chain_mbr(*drive, saved.as_ref())
        }
        Target::Partition { drive, part } => {
            mouse.disable();
            boot::chain_partition(&drives[*drive], part)
        }
        Target::Linux { drive, part, kernel, initrds, cmdline, fallback } => {
            let d = &drives[*drive];
            let why = match lumen_core::fs::open(disk::volume(d, part)) {
                Some(mut fsys) => {
                    let m = &display.mode;
                    let fb = boot::Framebuffer {
                        base: m.lfb as u32,
                        width: m.width as u16,
                        height: m.height as u16,
                        depth: m.bpp as u16,
                        pitch: m.pitch as u16,
                        rgb: m.rgb,
                    };
                    let mut before = || {
                        mouse.disable();
                        video::clear(&display.mode);
                    };
                    boot::linux(fsys.as_mut(), kernel, initrds, cmdline, Some(fb), &mut before)
                }
                None => String::from("its file system can no longer be read"),
            };
            dbg!("lumen: direct start failed: {why}");
            match fallback {
                Some(f) => start(f, drives, display, mouse),
                None => why,
            }
        }
    }
}

/// Can't run the menu: say why and start the PC's previous boot loader.
fn fatal(why: &str) -> ! {
    video::Display::text_mode();
    let msg = format!("Lumen: {why} - starting the previous boot loader.\r\n");
    for b in msg.bytes() {
        bios::call(0x10, bios::R { eax: 0x0E00 | b as u32, ebx: 7, ..Default::default() });
    }
    clock::sleep(2.0);
    boot_original()
}

/// Like `fatal`, without allocating.
fn fatal_static(why: &str) -> ! {
    video::Display::text_mode();
    for b in "Lumen: ".bytes().chain(why.bytes()).chain(" - starting the previous boot loader.\r\n".bytes()) {
        bios::call(0x10, bios::R { eax: 0x0E00 | b as u32, ebx: 7, ..Default::default() });
    }
    clock::sleep(3.0);
    boot_original()
}

/// Start the PC's original MBR (saved in Lumen's area).
fn boot_original() -> ! {
    let info = bios::info();
    let header = unsafe { core::slice::from_raw_parts(common::HEADER as *const u8, 512) };
    let base = u64::from_le_bytes(header[16..24].try_into().unwrap());
    if common::read_sectors(info.bios_int, info.drive, base + common::header::ORIGINAL_MBR, 1) {
        unsafe { core::ptr::copy_nonoverlapping(bios::BOUNCE as *const u8, 0x7C00 as *mut u8, 512) };
        (info.chainload)(info.drive as u32, 0);
    }
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

#[panic_handler]
fn panic(p: &core::panic::PanicInfo) -> ! {
    bios::debug(&format!("lumen: panic: {p}"));
    fatal("internal error")
}
