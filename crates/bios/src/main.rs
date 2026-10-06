//! Lumen for BIOS PCs (and UEFI PCs in legacy/CSM mode): the same menu as
//! the UEFI build, drawn with VESA graphics, on top of the PC's BIOS.

#![no_std]
#![no_main]

extern crate alloc;

mod bios;
mod clock;
mod common;
mod input;
mod mem;
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

    let cards: Vec<ui::Card> = ["Windows", "Ubuntu", "Fedora"]
        .iter()
        .map(|n| {
            let o = os::by_name(n).unwrap();
            ui::Card { id: String::from(*n), title: String::from(*n), location: String::from("Disk 1"), detail: String::new(), icon: o.icon }
        })
        .collect();
    let mut text = text::Text::new();
    let mut ui = ui::Ui::new(cards, 0, false, clock::now());
    ui.wall_clock = clock::wall_clock;
    let mut mouse = input::Mouse::new(display.canvas.w, display.canvas.h);
    dbg!("lumen: mouse {}", mouse.present);

    let mut last = clock::now();
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
        match command {
            ui::Command::Boot(i) => ui.toast(format!("Would start entry {}", i + 1), clock::now()),
            ui::Command::Act(_) => ui.notify(String::from("Action"), clock::now()),
            _ => {}
        }
        if ui.update(dt, now) {
            ui.draw(&mut display.canvas, &display.backdrop, &mut text, now, dt);
            display.present();
        }
        // ~120 frames a second at most.
        while clock::now() - frame_start < 1.0 / 120.0 {
            core::hint::spin_loop();
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
