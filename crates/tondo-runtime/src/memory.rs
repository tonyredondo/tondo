//! `std.internal.memory`, on which `std.ffi` is written: memory from C's
//! allocator, C strings, and the calls of C functions that take `...`.

use super::io::{array_bytes, byte_array};
use super::{ArrayHeader, Str, str_bytes, str_new};

unsafe extern "C" {
    fn calloc(count: usize, size: usize) -> *mut u8;
    fn free(block: *mut u8);
}

/// Zeroed memory from C's allocator, so that C can free it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_memory_allocate(size: i64) -> *mut u8 {
    let block = unsafe { calloc(1, size.max(1) as usize) };
    if block.is_null() {
        super::runtime_fault("out of memory");
    }
    block
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_memory_release(block: *mut u8, _size: i64) {
    unsafe { free(block) }
}

/// A copy of the text that ends with a zero byte, from C's allocator.
/// Text with a zero byte inside has no C string (`P0016`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_c_string(text: *const Str, location: *const Str) -> *mut u8 {
    let bytes = unsafe { str_bytes(text) };
    if let Some(at) = bytes.iter().position(|&b| b == 0) {
        super::raise(16, &format!("the text has a zero byte at {at}, so C would see it end there"), location);
        return std::ptr::null_mut();
    }
    let copy = tondo_rt_memory_allocate(bytes.len() as i64 + 1);
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), copy, bytes.len()) };
    copy
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_c_string_free(copy: *mut u8) {
    unsafe { free(copy) }
}

/// `memory.cString(text)`: the copy `ffi.toCString` gives.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_memory_c_string(text: *const Str) -> *mut u8 {
    // The location only exists to report a panic.
    let location = if unsafe { str_bytes(text) }.contains(&0) { str_new(b"ffi.toCString") } else { text };
    unsafe { tondo_rt_c_string(text, location) }
}

/// The text of a C string, with bytes that are not UTF-8 replaced.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_memory_from_c_string(text: *const u8) -> *mut Str {
    let bytes = unsafe { std::ffi::CStr::from_ptr(text.cast()) }.to_bytes();
    str_new(String::from_utf8_lossy(bytes).as_bytes())
}

/// The text of a C array of characters, up to its first zero byte.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_memory_from_c_chars(chars: *mut ArrayHeader) -> *mut Str {
    let bytes = unsafe { array_bytes(chars) };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    str_new(String::from_utf8_lossy(&bytes[..end]).as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_memory_bytes(from: *const u8, count: i64) -> *mut ArrayHeader {
    byte_array(unsafe { std::slice::from_raw_parts(from, count.max(0) as usize) })
}

/// Calls the C function at `target` with the arguments of `frame`: six
/// integer registers, eight vector registers, the number of stack words
/// and of vector registers used, then the stack words. The results come
/// back where the first registers were. A function with `...` learns from
/// `al` how many vector registers carry arguments, which compiled code
/// cannot set.
#[cfg(target_arch = "x86_64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_c_variadic(target: *const u8, frame: *mut u64) {
    std::arch::naked_asm!(
        "push rbp",
        "mov rbp, rsp",
        "push rbx",
        "push r12",
        "mov r12, rdi",
        "mov rbx, rsi",
        "mov rcx, [rbx + 112]",
        "lea rax, [rcx * 8 + 15]",
        "and rax, -16",
        "sub rsp, rax",
        "xor rdx, rdx",
        "2:",
        "cmp rdx, rcx",
        "jae 3f",
        "mov rax, [rbx + rdx * 8 + 128]",
        "mov [rsp + rdx * 8], rax",
        "inc rdx",
        "jmp 2b",
        "3:",
        "movsd xmm0, [rbx + 48]",
        "movsd xmm1, [rbx + 56]",
        "movsd xmm2, [rbx + 64]",
        "movsd xmm3, [rbx + 72]",
        "movsd xmm4, [rbx + 80]",
        "movsd xmm5, [rbx + 88]",
        "movsd xmm6, [rbx + 96]",
        "movsd xmm7, [rbx + 104]",
        "mov rdi, [rbx]",
        "mov rsi, [rbx + 8]",
        "mov rdx, [rbx + 16]",
        "mov rcx, [rbx + 24]",
        "mov r8, [rbx + 32]",
        "mov r9, [rbx + 40]",
        "mov rax, [rbx + 120]",
        "call r12",
        "mov [rbx], rax",
        "mov [rbx + 8], rdx",
        "movsd [rbx + 48], xmm0",
        "movsd [rbx + 56], xmm1",
        "lea rsp, [rbp - 16]",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

#[cfg(not(target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_c_variadic(_target: *const u8, _frame: *mut u64) {
    super::runtime_fault("calls of C functions with `...` are not supported on this target yet");
}
