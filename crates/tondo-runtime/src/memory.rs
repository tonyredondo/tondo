//! `std.internal.memory`: raw memory for tests of `unsafe` code (16.12).
//! Blocks are zeroed and aligned for any value.

const ALIGN: usize = 16;

fn layout(size: i64) -> std::alloc::Layout {
    std::alloc::Layout::from_size_align(size.max(1) as usize, ALIGN).expect("a valid size")
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_memory_allocate(size: i64) -> *mut u8 {
    let block = unsafe { std::alloc::alloc_zeroed(layout(size)) };
    if block.is_null() {
        super::runtime_fault("out of memory");
    }
    block
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_memory_release(block: *mut u8, size: i64) {
    unsafe { std::alloc::dealloc(block, layout(size)) }
}
