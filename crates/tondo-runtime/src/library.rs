//! Shared libraries loaded while the program runs (standard library 19.3):
//! `dlopen`, `dlsym` and `dlclose`. Each library is a native object, which
//! its last holder closes when the library may be unloaded.

use super::native::{self, NativeObject};
use super::{Str, str_bytes, str_new};
use std::ffi::{CStr, CString, c_void};
use std::sync::Mutex;

/// A library that opened, or why it did not.
enum Library {
    Open(Opened),
    Failed(String),
}

struct Opened {
    /// The handle `dlopen` gave, until the library is closed.
    handle: Mutex<Option<usize>>,
    path: String,
    /// Closed when its last holder goes (rule 14).
    unloadable: bool,
    /// The program itself, which is never unloaded.
    program: bool,
    /// What ends the runtime of a library that Tondo built, which is
    /// unloadable (rule 21).
    stop: Option<extern "C" fn()>,
}

impl Drop for Library {
    fn drop(&mut self) {
        if let Library::Open(opened) = self
            && opened.unloadable
            && let Some(handle) = opened.handle.get_mut().unwrap_or_else(|e| e.into_inner()).take()
        {
            if let Some(stop) = opened.stop {
                stop();
            }
            // An error here has no one to tell: the library stays loaded.
            unsafe { libc::dlclose(handle as *mut c_void) };
        }
    }
}

/// The entry that ends the runtime of a library that Tondo built, which
/// every one publishes under this name (toolchain 7.7): its own, and not
/// that of a library of Tondo it depends on, which `dlsym` finds too.
#[cfg(target_os = "linux")]
fn tondo_stop(handle: usize, _: &CStr) -> Option<extern "C" fn()> {
    unsafe extern "C" {
        fn dlinfo(handle: *mut c_void, request: i32, info: *mut c_void) -> i32;
    }
    const RTLD_DI_LINKMAP: i32 = 2;
    unsafe {
        let address = libc::dlsym(handle as *mut c_void, c"tondo_library_stop".as_ptr());
        let mut map: *const [usize; 2] = std::ptr::null();
        let mut found: libc::Dl_info = std::mem::zeroed();
        if address.is_null()
            || dlinfo(handle as *mut c_void, RTLD_DI_LINKMAP, (&raw mut map).cast()) != 0
            || libc::dladdr(address, &mut found) == 0
            // `l_name`, after `l_addr`, which `dladdr` gives as the file.
            || map.is_null()
            || found.dli_fname as usize != (*map)[1]
        {
            return None;
        }
        Some(std::mem::transmute::<*mut c_void, extern "C" fn()>(address))
    }
}

#[cfg(target_os = "macos")]
fn tondo_stop(_: usize, path: &CStr) -> Option<extern "C" fn()> {
    unsafe {
        // The image alone, not those it depends on.
        let own = libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD | libc::RTLD_FIRST);
        if own.is_null() {
            return None;
        }
        let address = libc::dlsym(own, c"tondo_library_stop".as_ptr());
        libc::dlclose(own);
        (!address.is_null()).then(|| std::mem::transmute::<*mut c_void, extern "C" fn()>(address))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn tondo_stop(_: usize, _: &CStr) -> Option<extern "C" fn()> {
    None
}

/// What the system said of the last thing that failed on this thread.
fn said() -> String {
    let error = unsafe { libc::dlerror() };
    match error.is_null() {
        true => "the system gave no reason".into(),
        false => unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned(),
    }
}

fn text(value: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(value) }).into_owned()
}

/// `open`: the library at `path`, with its symbols bound now, and offered
/// to the libraries loaded after it when `global`. A path without `/` is
/// searched for as the system searches.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_library_open(path: *const Str, global: bool, unloadable: bool) -> *mut NativeObject {
    let path = text(path);
    let Ok(name) = CString::new(path.clone()) else {
        return native::new(Library::Failed("a path holds no NUL character".into()));
    };
    let scope = if global { libc::RTLD_GLOBAL } else { libc::RTLD_LOCAL };
    let opened = crate::io::host(true, move || {
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | scope) };
        if handle.is_null() { Err(said()) } else { Ok((handle as usize, tondo_stop(handle as usize, &name))) }
    });
    native::new(match opened {
        Ok((handle, stop)) => {
            let unloadable = unloadable || stop.is_some();
            Library::Open(Opened { handle: Mutex::new(Some(handle)), path, unloadable, program: false, stop })
        }
        Err(why) => Library::Failed(why),
    })
}

/// `program`: the program itself and the libraries it started with.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_library_program() -> *mut NativeObject {
    let handle = unsafe { libc::dlopen(std::ptr::null(), libc::RTLD_NOW) };
    let path = std::env::current_exe().map(|path| path.display().to_string()).unwrap_or_default();
    let opened =
        Opened { handle: Mutex::new(Some(handle as usize)), path, unloadable: false, program: true, stop: None };
    native::new(Library::Open(opened))
}

/// Why the library did not open, or nothing.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_library_failure(object: *mut NativeObject) -> *mut Str {
    match native::get::<Library>(object) {
        Some(Library::Failed(why)) => str_new(why.as_bytes()),
        _ => str_new(b""),
    }
}

/// The handle of an open library, while it is open.
fn handle(object: *mut NativeObject) -> Option<usize> {
    match native::get::<Library>(object) {
        Some(Library::Open(opened)) => *opened.handle.lock().unwrap_or_else(|e| e.into_inner()),
        _ => None,
    }
}

/// Whether the library was closed.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_library_closed(object: *mut NativeObject) -> bool {
    handle(object).is_none()
}

/// The address of what the library publishes as `name`, or 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_library_symbol(object: *mut NativeObject, name: *const Str) -> i64 {
    let (Some(handle), Ok(name)) = (handle(object), CString::new(text(name))) else { return 0 };
    unsafe { libc::dlsym(handle as *mut c_void, name.as_ptr()) as i64 }
}

/// The file the library was loaded from: the executable for the program.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_library_path(object: *mut NativeObject) -> *mut Str {
    match native::get::<Library>(object) {
        Some(Library::Open(opened)) => str_new(opened.path.as_bytes()),
        _ => str_new(b""),
    }
}

/// `close`: unloads the library now; what the system said when it refused,
/// or nothing. The library stays open when it is refused.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_library_close(object: *mut NativeObject) -> *mut Str {
    let Some(Library::Open(opened)) = native::get::<Library>(object) else { return str_new(b"") };
    if opened.program {
        return str_new(b"the program itself is never unloaded");
    }
    let mut held = opened.handle.lock().unwrap_or_else(|e| e.into_inner());
    let Some(handle) = *held else { return str_new(b"") };
    let stop = opened.stop;
    let refused = crate::io::host(true, move || {
        if let Some(stop) = stop {
            stop();
        }
        let status = unsafe { libc::dlclose(handle as *mut c_void) };
        (status != 0).then(said)
    });
    match refused {
        Some(why) => str_new(why.as_bytes()),
        None => {
            *held = None;
            str_new(b"")
        }
    }
}

/// The file name of a library, as the system names it: `lib<name>.so`,
/// `lib<name>.so.<version>`; `lib<name>.dylib`, `lib<name>.<version>.dylib`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_library_file_name(name: *const Str, version: *const Str) -> *mut Str {
    let (name, version) = (text(name), text(version));
    let file = match (cfg!(target_os = "macos"), version.is_empty()) {
        (true, true) => format!("lib{name}.dylib"),
        (true, false) => format!("lib{name}.{version}.dylib"),
        (false, true) => format!("lib{name}.so"),
        (false, false) => format!("lib{name}.so.{version}"),
    };
    str_new(file.as_bytes())
}

/// The address of what a library that offers a plugin answers with (standard
/// library 14.48): each library has a runtime of its own, and so a slot of
/// its own, which its first `tondo_plugin_contract` fills.
static KEPT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Keeps `address` unless an address is kept already.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_plugin_keep(address: i64) {
    let _ =
        KEPT.compare_exchange(0, address, std::sync::atomic::Ordering::AcqRel, std::sync::atomic::Ordering::Acquire);
}

/// The address kept, or 0.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_plugin_kept() -> i64 {
    KEPT.load(std::sync::atomic::Ordering::Acquire)
}
