//! Lumen for BIOS PCs: the menu (placeholder while the pieces come up).

#![no_std]
#![no_main]

mod common;

use common::*;

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
    let info = unsafe { &*(BOOT_INFO as *const BootInfo) };
    for b in b"Lumen BIOS payload running.\r\n" {
        let mut r = Regs { eax: 0x0E00 | *b as u32, ebx: 7, ..Default::default() };
        int(info.bios_int, 0x10, &mut r);
    }
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("hlt") };
    }
}
