//! `std.env` (standard library 14.3.5): the program's arguments and its own
//! copy of the environment, which processes it starts inherit. The host's
//! `setenv` is not safe while other threads run, so changes reach the host
//! only through `syncNative`.

use super::{ArrayHeader, MapGlue, MapObject, Str, element, str_bytes, str_new, tondo_rt_array_new};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::{Mutex, MutexGuard};

type Table = BTreeMap<Vec<u8>, Vec<u8>>;

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

fn table() -> MutexGuard<'static, Option<Table>> {
    let mut table = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    table.get_or_insert_with(|| std::env::vars_os().map(|(k, v)| (k.into_vec(), v.into_vec())).collect());
    table
}

/// The environment processes started now inherit.
pub(crate) fn environment() -> Vec<(OsString, OsString)> {
    let table = table();
    let table = table.as_ref().unwrap();
    table.iter().map(|(k, v)| (OsString::from_vec(k.clone()), OsString::from_vec(v.clone()))).collect()
}

fn text(bytes: &[u8]) -> *mut Str {
    str_new(String::from_utf8_lossy(bytes).as_bytes())
}

/// What is wrong with a variable, or nothing (`P0015`).
fn invalid(name: &[u8], value: &[u8]) -> Option<String> {
    let shown = String::from_utf8_lossy(name);
    if name.is_empty() {
        Some("an environment variable needs a name, and this one is empty".into())
    } else if name.contains(&b'=') {
        Some(format!("the environment variable name `{shown}` has a `=`, which ends a name"))
    } else if name.contains(&0) {
        Some(format!("the environment variable name `{shown}` has a zero byte"))
    } else if value.contains(&0) {
        Some(format!("the value of the environment variable `{shown}` has a zero byte"))
    } else {
        None
    }
}

/// Whether a name and value may go in the environment: otherwise a panic,
/// and false when a `recover` stops it.
fn check(name: &[u8], value: &[u8], location: *const Str) -> bool {
    match invalid(name, value) {
        Some(message) => {
            super::raise(15, &message, location);
            false
        }
        None => true,
    }
}

/// `env.args()`: without the program's name.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_args() -> *mut ArrayHeader {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let array = tondo_rt_array_new(8, args.len());
    for (index, arg) in args.iter().enumerate() {
        unsafe { *(element(array, 8, index) as *mut *mut Str) = text(arg.as_bytes()) };
    }
    array
}

/// `env.get(name)`: null for none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_get(name: *const Str) -> *mut Str {
    let name = unsafe { str_bytes(name) };
    table().as_ref().unwrap().get(name).map_or(std::ptr::null_mut(), |value| text(value))
}

/// `env.vars()`, by name, as a `Map[String, String]` with `glue`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_vars(glue: *const MapGlue) -> *mut MapObject {
    let mut map = super::tondo_rt_map_new(16, 8);
    let mut old = [0u8; 16];
    for (name, value) in table().as_ref().unwrap() {
        let (key, value) = (text(name), text(value));
        unsafe {
            super::tondo_rt_map_insert(
                &mut map,
                &key as *const *mut Str as *const u8,
                &value as *const *mut Str as *const u8,
                old.as_mut_ptr(),
                glue,
            )
        };
    }
    map
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_set(name: *const Str, value: *const Str, location: *const Str) {
    let (name, value) = unsafe { (str_bytes(name), str_bytes(value)) };
    if check(name, value, location) {
        table().as_mut().unwrap().insert(name.to_vec(), value.to_vec());
    }
}

/// `env.setAll(values)`: a `Map[String, String]`, whose entries are two
/// string pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_set_all(values: *mut MapObject, location: *const Str) {
    let entries: Vec<(Vec<u8>, Vec<u8>)> = (0..unsafe { super::tondo_rt_map_len(values) })
        .map(|index| unsafe {
            let entry = super::tondo_rt_map_entry(values, index) as *const *const Str;
            (str_bytes(*entry).to_vec(), str_bytes(*entry.add(1)).to_vec())
        })
        .collect();
    if entries.iter().all(|(name, value)| check(name, value, location)) {
        table().as_mut().unwrap().extend(entries);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_remove(name: *const Str, location: *const Str) {
    let name = unsafe { str_bytes(name) };
    if check(name, b"", location) {
        table().as_mut().unwrap().remove(name);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_current_directory() -> *mut Str {
    std::env::current_dir().map_or_else(|_| str_new(b""), |path| text(path.as_os_str().as_bytes()))
}

/// `env.hostname()`: the machine's name, as the system gives it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_hostname() -> *mut Str {
    let mut name = [0u8; 256];
    let length = match unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } {
        0 => name.iter().position(|&b| b == 0).unwrap_or(name.len()),
        _ => 0,
    };
    text(&name[..length])
}

/// `env.processId()`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_process_id() -> i64 {
    std::process::id() as i64
}

/// `env.executable()`: the absolute path of the program's executable, its
/// links resolved.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_executable() -> *mut Str {
    let path = std::env::current_exe().and_then(|path| path.canonicalize());
    path.map_or_else(|_| str_new(b""), |path| text(path.as_os_str().as_bytes()))
}

/// `env.processors()`: the processors the program may run on, as its
/// affinity and its cgroup's quota allow.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_processors() -> i64 {
    std::thread::available_parallelism().map_or(1, |count| count.get() as i64)
}

/// The target the program was built for, as tondo names it:
/// `x86_64-linux-gnu`, `aarch64-linux-musl`, `aarch64-macos`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_target() -> *mut Str {
    let system = match (cfg!(target_os = "macos"), cfg!(target_env = "musl")) {
        (true, _) => "macos",
        (false, true) => "linux-musl",
        (false, false) => "linux-gnu",
    };
    text(format!("{}-{system}", std::env::consts::ARCH).as_bytes())
}

/// `env.setCurrentDirectory(path)`: 0, or 1 plus the `EnvError` variant.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_set_current_directory(path: *const Str) -> i64 {
    let path = std::ffi::OsStr::from_bytes(unsafe { str_bytes(path) }).to_owned();
    let changed = super::io::host(false, move || std::env::set_current_dir(path));
    match changed {
        Ok(()) => 0,
        Err(error) => match error.kind() {
            std::io::ErrorKind::NotFound => 1,
            std::io::ErrorKind::NotADirectory => 2,
            std::io::ErrorKind::PermissionDenied => 3,
            _ => 4,
        },
    }
}

/// `env.syncNative()`: makes the host's environment the program's.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_sync_native() {
    let table = table();
    let table = table.as_ref().unwrap();
    for (name, _) in std::env::vars_os() {
        if !table.contains_key(name.as_bytes()) {
            unsafe { std::env::remove_var(name) };
        }
    }
    for (name, value) in table {
        unsafe { std::env::set_var(std::ffi::OsStr::from_bytes(name), std::ffi::OsStr::from_bytes(value)) };
    }
}
