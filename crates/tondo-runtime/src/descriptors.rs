//! The descriptors and the mappings of `std.os.unix` (standard library
//! 14.45): a native object each, which closes its descriptor, or unmaps its
//! memory, when its last holder goes, unless the program closed it first.

use super::native::{self, NativeObject};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

/// A descriptor, which it closes when it goes if it owns it: the standard
/// ones are never closed.
struct Descriptor {
    /// The number, or -1 once closed.
    number: AtomicI64,
    owned: bool,
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        let number = *self.number.get_mut();
        if self.owned && number >= 0 {
            // An error here has no one to tell: `close` gives it.
            unsafe { libc::close(number as libc::c_int) };
        }
    }
}

/// The number of the system's last error on this thread.
fn errno() -> i64 {
    i64::from(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
}

/// `descriptor`: the descriptor `number`, which it closes when `owned`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_os_descriptor(number: i64, owned: bool) -> *mut NativeObject {
    native::new(Descriptor { number: AtomicI64::new(number), owned })
}

/// `descriptorNumber`: its number, or -1 once closed.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_os_descriptor_number(object: *mut NativeObject) -> i64 {
    native::get::<Descriptor>(object).map_or(-1, |d| d.number.load(Ordering::Acquire))
}

/// `descriptorClose`: closes it now; 0, or the system's error number. A
/// standard one is let go without being closed.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_os_descriptor_close(object: *mut NativeObject) -> i64 {
    let Some(descriptor) = native::get::<Descriptor>(object) else { return 0 };
    let number = descriptor.number.swap(-1, Ordering::AcqRel);
    if number < 0 || !descriptor.owned {
        return 0;
    }
    match unsafe { libc::close(number as libc::c_int) } {
        0 => 0,
        _ => errno(),
    }
}

/// Memory mapped into the program, which it unmaps when it goes.
struct Mapping {
    /// Its address, or 0 once unmapped.
    address: AtomicUsize,
    length: usize,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let address = *self.address.get_mut();
        if address != 0 {
            unsafe { libc::munmap(address as *mut libc::c_void, self.length) };
        }
    }
}

/// `mapping`: the memory `mmap` gave at `address`, of `length` bytes.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_os_mapping(address: i64, length: i64) -> *mut NativeObject {
    native::new(Mapping { address: AtomicUsize::new(address as usize), length: length as usize })
}

/// `mappingAddress`: its address, or 0 once unmapped.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_os_mapping_address(object: *mut NativeObject) -> i64 {
    native::get::<Mapping>(object).map_or(0, |m| m.address.load(Ordering::Acquire) as i64)
}

/// `mappingLength`: its length in bytes.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_os_mapping_length(object: *mut NativeObject) -> i64 {
    native::get::<Mapping>(object).map_or(0, |m| m.length as i64)
}

/// `mappingClose`: unmaps it now; 0, or the system's error number.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_os_mapping_close(object: *mut NativeObject) -> i64 {
    let Some(mapping) = native::get::<Mapping>(object) else { return 0 };
    let address = mapping.address.swap(0, Ordering::AcqRel);
    if address == 0 {
        return 0;
    }
    match unsafe { libc::munmap(address as *mut libc::c_void, mapping.length) } {
        0 => 0,
        _ => errno(),
    }
}
