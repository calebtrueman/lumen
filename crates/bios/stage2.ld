/* Stage 1 (the MBR code, run at 0x0600 after relocating itself) and
 * stage 2 (real-mode services + the payload unpacker, at 0x8000) in one
 * flat image: the first 512 bytes are the MBR, stage 2 follows. */
ENTRY(stage1_start)
SECTIONS {
    .mbr 0x0600 : AT(0) { KEEP(*(.mbr)) }
    .stage2 0x8000 : AT(0x200) {
        KEEP(*(.stage2.entry))
        *(.stage2*)
        *(.text .text.*)
        *(.rodata .rodata.*)
        *(.data .data.*)
        . = ALIGN(16);
    }
    .bss (NOLOAD) : {
        __bss_start = .;
        *(.bss .bss.*) *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }
    __stage2_end = .;
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
