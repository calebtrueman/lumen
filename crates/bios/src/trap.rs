//! CPU exception handlers. Without them any fault in protected mode
//! would reset the PC; instead Lumen reports it and starts the previous
//! boot loader, so the PC still boots.

use core::arch::global_asm;

// One stub per exception vector 0..31: push a dummy error code where the
// CPU doesn't push one, then the vector number, then the common handler.
global_asm!(
    ".section .text",
    ".macro TRAP n, err",
    "trap_\\n:",
    "  .if \\err == 0",
    "  push 0",
    "  .endif",
    "  push \\n",
    "  jmp trap_common",
    ".endm",
    "TRAP 0,0", "TRAP 1,0", "TRAP 2,0", "TRAP 3,0", "TRAP 4,0", "TRAP 5,0", "TRAP 6,0", "TRAP 7,0",
    "TRAP 8,1", "TRAP 9,0", "TRAP 10,1", "TRAP 11,1", "TRAP 12,1", "TRAP 13,1", "TRAP 14,1", "TRAP 15,0",
    "TRAP 16,0", "TRAP 17,1", "TRAP 18,0", "TRAP 19,0", "TRAP 20,0", "TRAP 21,1", "TRAP 22,0", "TRAP 23,0",
    "TRAP 24,0", "TRAP 25,0", "TRAP 26,0", "TRAP 27,0", "TRAP 28,0", "TRAP 29,1", "TRAP 30,1", "TRAP 31,0",
    "trap_common:",
    "  mov eax, esp",
    "  and esp, 0xFFFFFFF0",
    "  sub esp, 12",
    "  push eax",
    "  call lumen_trap",
    "trap_table:",
    "  .long trap_0, trap_1, trap_2, trap_3, trap_4, trap_5, trap_6, trap_7",
    "  .long trap_8, trap_9, trap_10, trap_11, trap_12, trap_13, trap_14, trap_15",
    "  .long trap_16, trap_17, trap_18, trap_19, trap_20, trap_21, trap_22, trap_23",
    "  .long trap_24, trap_25, trap_26, trap_27, trap_28, trap_29, trap_30, trap_31",
    ".global trap_table",
);

unsafe extern "C" {
    static trap_table: [u32; 32];
}

#[repr(C, packed)]
struct Pointer {
    limit: u16,
    base: u32,
}

static mut IDT: [u64; 32] = [0; 32];

/// Load the protected-mode IDT. The real-mode trampoline loads the BIOS's
/// own vector table while in real mode and doesn't touch this one, which
/// stays in effect whenever Lumen's code runs.
pub fn init() {
    unsafe {
        let idt = &raw mut IDT;
        for (i, &h) in trap_table.iter().enumerate() {
            // 32-bit interrupt gate, present, DPL 0, code selector 0x10.
            (*idt)[i] = (h as u64 & 0xFFFF) | (0x10u64 << 16) | (0x8E00u64 << 32) | ((h as u64 >> 16) << 48);
        }
        let p = Pointer { limit: (32 * 8 - 1) as u16, base: idt as u32 };
        core::arch::asm!("lidt [{}]", in(reg) &p);
    }
}

/// Frame pushed by the stub and the CPU.
#[repr(C)]
pub struct Frame {
    pub vector: u32,
    pub error: u32,
    pub eip: u32,
    pub cs: u32,
    pub eflags: u32,
}

#[unsafe(no_mangle)]
extern "C" fn lumen_trap(frame: &Frame) -> ! {
    // No allocation here: the fault may be in the allocator itself.
    let mut buf = [0u8; 64];
    let mut n = 0;
    let mut put = |s: &[u8]| {
        for &c in s {
            if n < buf.len() {
                buf[n] = c;
                n += 1;
            }
        }
    };
    put(b"CPU exception ");
    put(&hex(frame.vector));
    put(b" at ");
    put(&hex(frame.eip));
    put(b" error ");
    put(&hex(frame.error));
    let msg = core::str::from_utf8(&buf[..n]).unwrap_or("CPU exception");
    crate::bios::debug(msg);
    // Words on the stack that look like code addresses (test builds).
    if cfg!(feature = "debugcon") {
        let sp = frame as *const Frame as *const u32;
        for i in 5..200 {
            let w = unsafe { sp.add(i).read() };
            if (0x0200_0000..0x0210_0000).contains(&w) {
                crate::bios::debug(core::str::from_utf8(&hex(w)).unwrap_or(""));
            }
        }
    }
    crate::fatal_static(msg)
}

fn hex(v: u32) -> [u8; 8] {
    let d = b"0123456789abcdef";
    core::array::from_fn(|i| d[(v >> (28 - i * 4)) as usize & 15])
}
