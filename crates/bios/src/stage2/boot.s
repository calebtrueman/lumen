// Lumen on BIOS PCs: the real-mode parts.
//
// Stage 1 is the MBR code (at most 432 bytes plus the 8-byte location of
// Lumen's area, which the installer patches in; the disk signature and
// partition table that follow are the disk's own). It loads stage 2 from
// the gap between the MBR and the first partition. If anything is wrong,
// or Shift is held, it starts the disk's original MBR instead (kept by the
// installer), so the PC always boots as it did before.
//
// Stage 2 enables A20, switches to 32-bit protected mode and calls Rust.
// It stays in memory: `bios_int` drops back to real mode to make one BIOS
// call, `chainload` leaves for good to a boot sector at 0x7C00.
//
// Low memory:
//   0x0500  BootInfo (see common.rs)        0x0800  Lumen header sector
//   0x0600  stage 1 after relocating         0x0C00  BIOS call registers
//   0x0E00  mouse packet ring                0x7C00  real-mode stack top /
//   0x8000  stage 2                                   boot sector address
//   0x20000 64 KiB bounce buffer for disk and VBE calls

.set HEADER, 0x0800
.set REGS, 0x0C00
.set MOUSE_RING, 0x0E00
.set RM_STACK, 0x7BF0

// ---------------------------------------------------------------- stage 1
.section .mbr, "ax"
.code16
.global stage1_start
stage1_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    mov si, 0x7C00
    mov di, 0x0600
    mov cx, 256
    cld
    rep movsw
    ljmp 0, offset s1_relocated
s1_relocated:
    sti
    mov [s1_drive], dl
    // LBA disk access (INT 13h extensions) is required.
    mov ah, 0x41
    mov bx, 0x55AA
    int 0x13
    jc s1_dead
    cmp bx, 0xAA55
    jne s1_dead
    // Shift held: start the PC's previous boot loader.
    mov ah, 0x02
    int 0x16
    test al, 0x03
    jnz s1_original
    // Header (Lumen area + 0) -> 0x0800.
    mov eax, [s1_base]
    mov edx, [s1_base + 4]
    mov word ptr [s1_dap + 2], 1
    mov word ptr [s1_dap + 4], HEADER
    mov word ptr [s1_dap + 6], 0
    call s1_read
    jc s1_original
    mov si, offset s1_magic
    mov di, HEADER
    mov cx, 8
    repe cmpsb
    jne s1_original
    // Stage 2 (Lumen area + 3, header says how long) -> 0x8000.
    mov ax, [HEADER + 24]
    test ax, ax
    jz s1_original
    cmp ax, 127
    ja s1_original
    mov [s1_dap + 2], ax
    mov word ptr [s1_dap + 4], 0
    mov word ptr [s1_dap + 6], 0x0800
    mov eax, [s1_base]
    mov edx, [s1_base + 4]
    add eax, 3
    adc edx, 0
    call s1_read
    jc s1_original
    mov dl, [s1_drive]
    ljmp 0, 0x8000

// The original MBR (Lumen area + 1) -> 0x7C00, and run it.
s1_original:
    mov eax, [s1_base]
    mov edx, [s1_base + 4]
    add eax, 1
    adc edx, 0
    mov word ptr [s1_dap + 2], 1
    mov word ptr [s1_dap + 4], 0x7C00
    mov word ptr [s1_dap + 6], 0
    call s1_read
    jc s1_dead
    cmp word ptr [0x7DFE], 0xAA55
    jne s1_dead
    mov dl, [s1_drive]
    ljmp 0, 0x7C00

s1_dead:
    mov si, offset s1_msg
s1_print:
    lodsb
    test al, al
    jz s1_halt
    mov ah, 0x0E
    mov bx, 7
    int 0x10
    jmp s1_print
s1_halt:
    int 0x18
    hlt
    jmp s1_halt

// Read [s1_dap+2] sectors at LBA edx:eax into the DAP's buffer.
s1_read:
    mov [s1_dap + 8], eax
    mov [s1_dap + 12], edx
    mov si, offset s1_dap
    mov dl, [s1_drive]
    mov ah, 0x42
    int 0x13
    ret

s1_magic: .ascii "LUMENBIO"
s1_msg: .asciz "Lumen: can't read the disk. "
s1_drive: .byte 0x80
.balign 4
s1_dap: .byte 16, 0
    .word 0, 0, 0
    .quad 0
// The installer writes the LBA of Lumen's area here.
.org 0x1B0
s1_base: .quad 0

// ---------------------------------------------------------------- stage 2
.section .stage2.entry, "ax"
.code16
.global stage2_start
stage2_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    mov [0x0504], dl                 // BootInfo.drive
    call enable_a20
    lgdt [gdt_desc]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    ljmp 0x10, offset pm_entry

// A20: ask the BIOS, then the "fast A20" port; check by wraparound.
enable_a20:
    call a20_on
    je 1f
    mov ax, 0x2401
    int 0x15
    call a20_on
    je 1f
    in al, 0x92
    test al, 2
    jnz 2f
    or al, 2
    and al, 0xFE
    out 0x92, al
2:  call a20_on
    je 1f
    // Keyboard controller method.
    call kbc_wait
    mov al, 0xD1
    out 0x64, al
    call kbc_wait
    mov al, 0xDF
    out 0x60, al
    call kbc_wait
1:  ret
kbc_wait:
    in al, 0x64
    test al, 2
    jnz kbc_wait
    ret
// ZF set if A20 is on: 0000:0500 and FFFF:0510 must differ.
a20_on:
    push ds
    push es
    xor ax, ax
    mov ds, ax
    not ax
    mov es, ax
    mov al, [0x0500]
    mov ah, al
    not ah
    mov es:[0x0510], ah
    cmp [0x0500], ah
    mov [0x0500], al
    pop es
    pop ds
    jne 3f
    or al, 1                         // clears ZF: A20 off
    ret
3:  xor ax, ax                       // sets ZF: A20 on
    ret

.code32
pm_entry:
    mov ax, 0x18
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, 0x7FFF0
    // x87 + SSE: CR0.EM off, CR0.MP on; CR4.OSFXSR and OSXMMEXCPT on.
    mov eax, cr0
    and eax, 0xFFFFFFFB
    or eax, 2
    mov cr0, eax
    mov eax, cr4
    or eax, 0x600
    mov cr4, eax
    fninit
    mov edi, offset __bss_start
    mov ecx, offset __bss_end
    sub ecx, edi
    xor eax, eax
    cld
    rep stosb
    call stage2_main
4:  hlt
    jmp 4b

// extern "C" fn bios_int(n: u32): one BIOS interrupt with the registers in
// the block at REGS (eax ebx ecx edx esi edi ebp ds es eflags), which
// receives the results. Interrupts are enabled while in real mode, so
// pending keyboard/timer/mouse interrupts are serviced there.
.global bios_int
bios_int:
    push ebp
    push ebx
    push esi
    push edi
    mov eax, [esp + 20]
    mov [int_no], al
    mov [saved_esp], esp
    ljmp 0x08, offset rm16
.code16
rm16:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov eax, cr0
    and eax, 0xFFFFFFFE
    mov cr0, eax
    ljmp 0, offset rm_real
rm_real:
    xor ax, ax
    mov ds, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    mov sp, RM_STACK
    lidt [rm_idt]
    mov ax, [REGS + 32]
    mov es, ax
    mov ebx, [REGS + 4]
    mov ecx, [REGS + 8]
    mov edx, [REGS + 12]
    mov esi, [REGS + 16]
    mov edi, [REGS + 20]
    mov ebp, [REGS + 24]
    push word ptr [REGS + 28]
    mov eax, [REGS + 0]
    pop ds
    sti
    .byte 0xCD                       // int imm8 (patched above)
int_no: .byte 0
    cli
    push ds
    push eax
    xor ax, ax
    mov ds, ax
    pop eax
    mov [REGS + 0], eax
    mov [REGS + 4], ebx
    mov [REGS + 8], ecx
    mov [REGS + 12], edx
    mov [REGS + 16], esi
    mov [REGS + 20], edi
    mov [REGS + 24], ebp
    pop ax
    mov [REGS + 28], ax
    mov ax, es
    mov [REGS + 32], ax
    pushfd
    pop eax
    mov [REGS + 36], eax
    lgdt [gdt_desc]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    ljmp 0x10, offset pm_back
.code32
pm_back:
    mov ax, 0x18
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, [saved_esp]
    pop edi
    pop esi
    pop ebx
    pop ebp
    ret

// extern "C" fn chainload(drive: u32, si: u32) -> !: run the boot sector
// already placed at 0x7C00, as the BIOS would (DL = drive; DS:SI = the
// partition table entry for a partition boot sector, if given).
.global chainload
chainload:
    cli
    mov eax, [esp + 4]
    mov [chain_drive], al
    mov eax, [esp + 8]
    mov [chain_si], ax
    ljmp 0x08, offset chain16
.code16
chain16:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov eax, cr0
    and eax, 0xFFFFFFFE
    mov cr0, eax
    ljmp 0, offset chain_real
chain_real:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov sp, 0x7C00
    lidt [rm_idt]
    mov dl, [chain_drive]
    mov si, [chain_si]
    xor ebx, ebx
    sti
    ljmp 0, 0x7C00

// INT 15h C207 mouse handler, called (far) by the BIOS with status, X, Y
// and Z words on the stack. Packets go into a ring Rust reads:
// u32 count, then 64 slots of [status, x, y, 0].
.global mouse_handler
mouse_handler:
    push bp
    mov bp, sp
    push ds
    push ax
    push bx
    xor ax, ax
    mov ds, ax
    mov bx, [MOUSE_RING]
    and bx, 63
    shl bx, 2
    mov al, [bp + 12]
    mov [MOUSE_RING + 4 + bx], al
    mov al, [bp + 10]
    mov [MOUSE_RING + 5 + bx], al
    mov al, [bp + 8]
    mov [MOUSE_RING + 6 + bx], al
    inc dword ptr [MOUSE_RING]
    pop bx
    pop ax
    pop ds
    pop bp
    retf

.code32
.balign 8
// 0x08: 16-bit code, 0x10: 32-bit flat code, 0x18: 32-bit flat data,
// 0x20: 16-bit data. 0x10/0x18 are also what Linux's 32-bit boot
// protocol expects (__BOOT_CS / __BOOT_DS).
gdt:
    .quad 0
    .quad 0x00009A000000FFFF
    .quad 0x00CF9A000000FFFF
    .quad 0x00CF92000000FFFF
    .quad 0x000092000000FFFF
gdt_end:
gdt_desc:
    .word gdt_end - gdt - 1
    .long gdt
rm_idt:
    .word 0x3FF
    .long 0
saved_esp: .long 0
chain_drive: .byte 0x80
.balign 2
chain_si: .word 0
