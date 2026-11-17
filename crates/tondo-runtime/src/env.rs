//! `std.env` (standard library 14.3.5): the program's arguments and its own
//! copy of the environment, which processes it starts inherit. The host's
//! `setenv` is not safe while other threads run, so changes reach the host
//! only through `syncNative`.

use super::{ArrayHeader, MapGlue, MapObject, Str, str_bytes};
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

/// `bytes` as UTF-8 reads them, in `region` when it is not null.
fn text(region: *mut crate::regions::Region, bytes: &[u8]) -> *mut Str {
    crate::strings::str_in(region, String::from_utf8_lossy(bytes).as_bytes())
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
pub extern "C" fn tondo_rt_env_args(region: *mut crate::regions::Region) -> *mut ArrayHeader {
    crate::arrays::texts_array_in(region, || arguments().iter().skip(1).map(|arg| arg.as_bytes()))
}

/// The program's arguments, its name first, as `main` was given them:
/// Rust's own reach a program only through its `main`, which a static
/// executable that the toolchain links does not run. A library, which C
/// runs, has the ones its process has.
static ARGUMENTS: std::sync::OnceLock<Vec<OsString>> = std::sync::OnceLock::new();

/// Keeps the arguments the program's `main` was given.
#[cfg(tondo_staticlib)]
pub(crate) unsafe fn keep_arguments(argc: i32, argv: *const *const u8) {
    let kept = (0..argc.max(0) as usize)
        .map(|index| unsafe { *argv.add(index) })
        .take_while(|arg| !arg.is_null())
        .map(|arg| OsString::from_vec(unsafe { std::ffi::CStr::from_ptr(arg.cast()) }.to_bytes().to_vec()))
        .collect();
    let _ = ARGUMENTS.set(kept);
}

pub(crate) fn arguments() -> &'static [OsString] {
    ARGUMENTS.get_or_init(|| std::env::args_os().collect())
}

/// `env.get(name)`: null for none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_get(name: *const Str, region: *mut crate::regions::Region) -> *mut Str {
    let name = unsafe { str_bytes(name) };
    table().as_ref().unwrap().get(name).map_or(std::ptr::null_mut(), |value| text(region, value))
}

/// `env.vars()`, by name, as a `Map[String, String]` with `glue`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_vars(glue: *const MapGlue) -> *mut MapObject {
    let mut map = super::tondo_rt_map_new(16, 8);
    let mut old = [0u8; 16];
    for (name, value) in table().as_ref().unwrap() {
        let (key, value) = (text(std::ptr::null_mut(), name), text(std::ptr::null_mut(), value));
        unsafe {
            super::tondo_rt_map_insert(
                &mut map,
                &key as *const *mut Str as *const u8,
                &value as *const *mut Str as *const u8,
                old.as_mut_ptr(),
                glue,
                std::ptr::null_mut(),
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
pub extern "C" fn tondo_rt_env_current_directory(region: *mut crate::regions::Region) -> *mut Str {
    std::env::current_dir().map_or_else(|_| text(region, b""), |path| text(region, path.as_os_str().as_bytes()))
}

/// `env.hostname()`: the machine's name, as the system gives it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_hostname(region: *mut crate::regions::Region) -> *mut Str {
    let mut name = [0u8; 256];
    let length = match unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } {
        0 => name.iter().position(|&b| b == 0).unwrap_or(name.len()),
        _ => 0,
    };
    text(region, &name[..length])
}

/// `env.processId()`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_process_id() -> i64 {
    std::process::id() as i64
}

/// `env.executable()`: the absolute path of the program's executable, its
/// links resolved.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_executable(region: *mut crate::regions::Region) -> *mut Str {
    let path = std::env::current_exe().and_then(|path| path.canonicalize());
    path.map_or_else(|_| text(region, b""), |path| text(region, path.as_os_str().as_bytes()))
}

/// `env.invokedAs()`: the name the program was run by, `argv[0]`, as
/// given: a link's own name, not its target's.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_invoked_as(region: *mut crate::regions::Region) -> *mut Str {
    let invoked = arguments().first().cloned().unwrap_or_default();
    text(region, invoked.as_bytes())
}

/// `env.processors()`: the processors the program may run on, as its
/// affinity and its cgroup's quota allow.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_processors() -> i64 {
    std::thread::available_parallelism().map_or(1, |count| count.get() as i64)
}

/// The cores of the machine, each once however many threads it runs; what
/// the program may run on when the system does not say.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_cores() -> i64 {
    #[cfg(target_os = "macos")]
    {
        let mut count: i32 = 0;
        let mut size = std::mem::size_of::<i32>();
        let asked = unsafe {
            libc::sysctlbyname(c"hw.physicalcpu".as_ptr(), (&raw mut count).cast(), &mut size, std::ptr::null_mut(), 0)
        };
        if asked == 0 && count > 0 {
            return count as i64;
        }
    }
    #[cfg(target_os = "linux")]
    {
        // A core is a pair of package and core numbers, which its threads
        // share.
        let mut cores = std::collections::HashSet::new();
        for entry in std::fs::read_dir("/sys/devices/system/cpu").into_iter().flatten().flatten() {
            let name = entry.file_name();
            let numbered = name
                .to_str()
                .and_then(|n| n.strip_prefix("cpu"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if !numbered {
                continue;
            }
            let topology = entry.path().join("topology");
            let read = |file: &str| std::fs::read_to_string(topology.join(file)).ok().map(|t| t.trim().to_string());
            if let (Some(package), Some(core)) = (read("physical_package_id"), read("core_id")) {
                cores.insert((package, core));
            }
        }
        if !cores.is_empty() {
            return cores.len() as i64;
        }
    }
    tondo_rt_env_processors()
}

/// The user the program runs as: its id, its group's, its name and its
/// home, as the system's accounts say; empty texts for what they lack.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_user(region: *mut crate::regions::Region) -> *mut ArrayHeader {
    let (id, group) = unsafe { (libc::getuid(), libc::getgid()) };
    let mut account: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    unsafe { libc::getpwuid_r(id, &mut account, buffer.as_mut_ptr(), buffer.len(), &mut found) };
    let field = |text: *const libc::c_char| match text.is_null() {
        true => String::new(),
        false => unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy().into_owned(),
    };
    let (name, home) = match found.is_null() {
        true => (String::new(), String::new()),
        false => (field(account.pw_name), field(account.pw_dir)),
    };
    crate::arrays::string_array_in(region, &[id.to_string(), group.to_string(), name, home])
}

/// The system the program runs on, as `uname` says: its name, release and
/// machine.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_system(region: *mut crate::regions::Region) -> *mut ArrayHeader {
    let mut named: libc::utsname = unsafe { std::mem::zeroed() };
    unsafe { libc::uname(&mut named) };
    let field =
        |text: &[libc::c_char]| unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }.to_string_lossy().into_owned();
    crate::arrays::string_array_in(region, &[field(&named.sysname), field(&named.release), field(&named.machine)])
}

/// The target the program was built for, as tondo names it:
/// `x86_64-linux-gnu`, `aarch64-linux-musl`, `aarch64-macos`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_target(region: *mut crate::regions::Region) -> *mut Str {
    let system = match (cfg!(target_os = "macos"), cfg!(target_env = "musl")) {
        (true, _) => "macos",
        (false, true) => "linux-musl",
        (false, false) => "linux-gnu",
    };
    crate::strings::str_in(region, format!("{}-{system}", std::env::consts::ARCH).as_bytes())
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
