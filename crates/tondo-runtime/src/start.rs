//! How an executable that `tondo` links itself starts (toolchain, H12):
//! `tondo_rt_start`, where the kernel enters, hands the arguments to the C
//! library's `__libc_start_main`, which calls `main`. A system linker
//! brings its own `_start` from the C toolchain instead, and drops these.

type Constructor = unsafe extern "C" fn(i32, *const *const u8, *const *const u8);

unsafe extern "C" {
    static __init_array_start: u8;
    static __init_array_end: u8;
}

/// What `__libc_start_main` gets as `init`: glibc calls it to run the
/// program's constructors, as the C toolchain's `__libc_csu_init` does;
/// musl runs them itself.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_init(argc: i32, argv: *const *const u8, envp: *const *const u8) {
    unsafe {
        let mut at = (&raw const __init_array_start).cast::<Constructor>();
        let end = (&raw const __init_array_end).cast::<Constructor>();
        while at < end {
            (*at)(argc, argv, envp);
            at = at.add(1);
        }
    }
}

#[cfg(target_arch = "x86_64")]
std::arch::global_asm!(
    ".globl tondo_rt_start",
    ".type tondo_rt_start, @function",
    "tondo_rt_start:",
    "xor ebp, ebp",
    // What the dynamic linker runs at exit, then argc and argv.
    "mov r9, rdx",
    "pop rsi",
    "mov rdx, rsp",
    "and rsp, -16",
    "push rax",
    // The end of the stack.
    "push rsp",
    "xor r8d, r8d",
    "lea rcx, [rip + {init}]",
    "lea rdi, [rip + {main}]",
    "call qword ptr [rip + __libc_start_main@GOTPCREL]",
    "hlt",
    init = sym tondo_rt_init,
    main = sym crate::main,
);

#[cfg(target_arch = "aarch64")]
std::arch::global_asm!(
    ".globl tondo_rt_start",
    ".type tondo_rt_start, %function",
    "tondo_rt_start:",
    "mov x29, #0",
    "mov x30, #0",
    // What the dynamic linker runs at exit, argc, argv and the end of the
    // stack.
    "mov x5, x0",
    "ldr x1, [sp]",
    "add x2, sp, #8",
    "mov x6, sp",
    "adrp x0, {main}",
    "add x0, x0, :lo12:{main}",
    "adrp x3, {init}",
    "add x3, x3, :lo12:{init}",
    "mov x4, #0",
    "bl __libc_start_main",
    "brk #0",
    init = sym tondo_rt_init,
    main = sym crate::main,
);
