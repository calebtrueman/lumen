/* The menu payload, linked at 32 MiB (below it: room for a Linux kernel
 * at 1 MiB; above it: the heap, then initrds near the top of memory). */
ENTRY(lumen_start)
SECTIONS {
    . = 0x2000000;
    .text : { KEEP(*(.text.entry)) *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    .bss (NOLOAD) : {
        . = ALIGN(16);
        __bss_start = .;
        *(.bss .bss.*) *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }
    __payload_end = .;
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
}
