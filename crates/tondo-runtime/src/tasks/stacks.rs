//! Stacks: switching from one to another, the stacks tasks run on, each with a
//! guard below it, and those kept for reuse (docs/refactor.md, phase 2).

use super::*;

/// Saves the callee-saved registers on the current stack, stores its pointer
/// in `*from`, and resumes the stack `to` saved the same way.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
pub(super) unsafe extern "C" fn switch(from: *mut *mut u8, to: *mut u8) {
    std::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, rsi",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
pub(super) unsafe extern "C" fn switch(from: *mut *mut u8, to: *mut u8) {
    std::arch::naked_asm!(
        "sub sp, sp, #176",
        "stp x19, x20, [sp, #0]",
        "stp x21, x22, [sp, #16]",
        "stp x23, x24, [sp, #32]",
        "stp x25, x26, [sp, #48]",
        "stp x27, x28, [sp, #64]",
        "stp x29, x30, [sp, #80]",
        "stp d8, d9, [sp, #96]",
        "stp d10, d11, [sp, #112]",
        "stp d12, d13, [sp, #128]",
        "stp d14, d15, [sp, #144]",
        "mov x9, sp",
        "str x9, [x0]",
        "mov sp, x1",
        "ldp x19, x20, [sp, #0]",
        "ldp x21, x22, [sp, #16]",
        "ldp x23, x24, [sp, #32]",
        "ldp x25, x26, [sp, #48]",
        "ldp x27, x28, [sp, #64]",
        "ldp x29, x30, [sp, #80]",
        "ldp d8, d9, [sp, #96]",
        "ldp d10, d11, [sp, #112]",
        "ldp d12, d13, [sp, #128]",
        "ldp d14, d15, [sp, #144]",
        "add sp, sp, #176",
        "ret",
    )
}

/// The stack pointer `switch` resumes to start a task in `trampoline`.
pub(super) unsafe fn initial_stack(top: *mut u8) -> *mut u8 {
    let top = (top as usize & !15) as *mut usize;
    unsafe {
        #[cfg(target_arch = "x86_64")]
        {
            // `ret` pops the trampoline's address and leaves the stack as a
            // call would: 8 bytes past a 16-byte boundary.
            let ret = top.sub(2);
            ret.write(trampoline as *const () as usize);
            let saved = ret.sub(6);
            for i in 0..6 {
                saved.add(i).write(0);
            }
            saved as *mut u8
        }
        #[cfg(target_arch = "aarch64")]
        {
            let saved = top.sub(22);
            for i in 0..22 {
                saved.add(i).write(0);
            }
            // x30, the link register, sits after x29 at offset 88.
            saved.add(11).write(trampoline as *const () as usize);
            saved as *mut u8
        }
    }
}

// ---------------------------------------------------------------------------
// Stacks
// ---------------------------------------------------------------------------

unsafe extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
}

pub(super) const PROT_NONE: i32 = 0;
pub(super) const PROT_READ_WRITE: i32 = 3;
#[cfg(target_os = "linux")]
pub(super) const MAP_FLAGS: i32 = 0x02 | 0x20 | 0x4000; // PRIVATE | ANONYMOUS | NORESERVE
#[cfg(not(target_os = "linux"))]
pub(super) const MAP_FLAGS: i32 = 0x02 | 0x1000; // PRIVATE | ANON
/// Covers the largest page size of the supported targets.
pub(crate) const GUARD: usize = 64 * 1024;
#[cfg_attr(not(tondo_staticlib), allow(dead_code))]
pub(super) const MAIN_STACK: usize = 64 * 1024 * 1024;
pub(super) const TASK_STACK: usize = 8 * 1024 * 1024;

/// A stack mapped on demand, with an inaccessible guard region below it so
/// that an overflow faults instead of corrupting memory.
pub(super) struct Stack {
    pub(super) base: *mut u8,
    pub(super) size: usize,
}

impl Stack {
    pub(super) fn new(size: usize) -> Stack {
        unsafe {
            let base = mmap(std::ptr::null_mut(), size, PROT_READ_WRITE, MAP_FLAGS, -1, 0);
            if base as isize == -1 {
                runtime_fault("cannot map a task stack");
            }
            mprotect(base, GUARD, PROT_NONE);
            // A task uses a few pages of its stack: a huge page for the top
            // would hold 2 MiB.
            #[cfg(target_os = "linux")]
            libc::madvise(base.cast(), size, libc::MADV_NOHUGEPAGE);
            Stack { base, size }
        }
    }

    pub(super) fn top(&self) -> *mut u8 {
        unsafe { self.base.add(self.size) }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        unsafe { munmap(self.base, self.size) };
    }
}

unsafe impl Send for Stack {}

/// Task stacks kept for reuse: mapping and unmapping one per task costs
/// more than the task itself. Each thread keeps a few, and trades them
/// with the shared ones a batch at a time, since a task often ends on
/// another thread than the one that started it.
pub(super) static SPARE_STACKS: Mutex<Vec<Stack>> = Mutex::new(Vec::new());
pub(super) const SPARE_LIMIT: usize = 256;
pub(super) const BATCH: usize = 16;

/// This worker's spare stacks, made when it first keeps one. They are not a
/// field that drops with the thread, which would make every read of `Worker`
/// check whether the thread ends: what the runtime keeps for the thread
/// gives them back then (`Kept`).
pub(super) fn own_stacks() -> &'static mut Vec<Stack> {
    let mut stacks = worker(|w| w.stacks.get());
    if stacks.is_null() {
        stacks = Box::into_raw(Box::new(Vec::new()));
        kept().stacks.set(stacks);
        worker(|w| w.stacks.set(stacks));
    }
    // Neither caller is entered again while it holds them.
    unsafe { &mut *stacks }
}

/// A thread's spare stacks, as it ends, to the shared ones.
pub(super) fn give_back_stacks(stacks: *mut Vec<Stack>) {
    if stacks.is_null() {
        return;
    }
    let stacks = unsafe { Box::from_raw(stacks) };
    let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
    let room = SPARE_LIMIT.saturating_sub(spare.len());
    spare.extend(stacks.into_iter().take(room));
}

pub(super) fn task_stack() -> Stack {
    let own = own_stacks();
    if own.is_empty() {
        let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
        let at = spare.len().saturating_sub(BATCH);
        own.extend(spare.drain(at..));
    }
    own.pop().unwrap_or_else(|| Stack::new(TASK_STACK))
}

pub(super) fn recycle(stack: Stack) {
    if stack.size != TASK_STACK {
        return;
    }
    let own = own_stacks();
    own.push(stack);
    if own.len() > 2 * BATCH {
        let mut spare = SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner());
        let room = SPARE_LIMIT.saturating_sub(spare.len()).min(BATCH);
        let at = own.len() - BATCH;
        spare.extend(own.drain(at..).take(room));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A thread keeps two batches of spare stacks, and with one more gives
    /// a batch to those every thread shares.
    #[test]
    fn a_thread_keeps_two_batches_of_stacks_and_shares_the_rest() {
        // A thread of its own, whose stacks no other test takes.
        std::thread::spawn(|| {
            let shared = || SPARE_STACKS.lock().unwrap_or_else(|e| e.into_inner()).len();
            let before = shared();
            for _ in 0..2 * BATCH {
                recycle(Stack::new(TASK_STACK));
            }
            assert_eq!((own_stacks().len(), shared()), (2 * BATCH, before));
            recycle(Stack::new(TASK_STACK));
            assert_eq!((own_stacks().len(), shared()), (BATCH + 1, before + BATCH));
        })
        .join()
        .unwrap();
    }
}
