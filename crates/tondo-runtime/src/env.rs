//! `std.env` (standard library 14.3.5): the program's arguments and its own
//! copy of the environment, which processes it starts inherit. The host's
//! `setenv` is not safe while other threads run, so changes reach the host
//! only through `syncNative`.

use super::{ArrayHeader, MapGlue, MapObject, Str, str_bytes};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::sync::{Arc, Mutex, MutexGuard};

type Table = BTreeMap<Vec<u8>, Vec<u8>>;
pub(crate) type EnvList = Vec<(OsString, OsString)>;

/// The program's copy of the environment, as children are given it, and
/// the system's environment while it is known to be that copy.
struct State {
    table: Table,
    /// The copy as a child is given it, made at the first start and again
    /// only after `set`, `setAll` or `remove`.
    prepared: Option<Arc<EnvList>>,
    /// The system's environment, entry by entry in its order, as it was when
    /// it last equalled the copy: when the copy was read from it, and after
    /// `syncNative`. `None` once the program changed its copy or the system's
    /// moved from it, until `syncNative` makes them equal again.
    system: Option<Vec<Vec<u8>>>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn state() -> MutexGuard<'static, Option<State>> {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert_with(|| {
        let system = system_entries();
        let table: Table = system.iter().filter_map(|entry| variable(entry)).collect();
        let system = same(system, &table);
        State { table, prepared: None, system }
    });
    guard
}

/// The system's entries when they are the copy, one entry for each of its
/// variables: an entry with no `=`, or a name given twice, which the copy
/// reads as its last, would give a child what the copy has not.
fn same(system: Vec<Vec<u8>>, table: &Table) -> Option<Vec<Vec<u8>>> {
    let each = system.len() == table.len()
        && system.iter().all(|entry| {
            variable(entry).is_some_and(|(name, value)| table.get(&name).is_some_and(|kept| *kept == value))
        });
    each.then_some(system)
}

/// What a child started now is given of the program's environment: the
/// copy, and whether the system's environment is that copy byte for byte,
/// so that the child may inherit it as it is, with nothing copied (standard
/// library 14.3.5: children inherit the program's copy). Both come from
/// one hold of the lock, so a start beside a `set` gets the whole of the
/// environment from before it or the whole of it from after.
pub(crate) struct ForChild {
    pub(crate) list: Arc<EnvList>,
    pub(crate) inherit: bool,
}

pub(crate) fn for_child() -> ForChild {
    let mut guard = state();
    let state = guard.as_mut().unwrap();
    // Compared by bytes, not by where they are: a `setenv` may put another
    // value where the old one was, or C code may write over a string it gave
    // `putenv`. Once the two differ, the copy goes to every child until
    // `syncNative` makes them equal again.
    let inherit = match &state.system {
        Some(entries) if unsafe { system_is(entries) } => true,
        Some(_) => {
            state.system = None;
            false
        }
        None => false,
    };
    let list = state.prepared.get_or_insert_with(|| {
        Arc::new(
            state.table.iter().map(|(k, v)| (OsString::from_vec(k.clone()), OsString::from_vec(v.clone()))).collect(),
        )
    });
    ForChild { list: Arc::clone(list), inherit }
}

/// The program's copy, as `env.vars()` gives it.
#[cfg(test)]
pub(crate) fn copy() -> Table {
    state().as_ref().unwrap().table.clone()
}

/// The string of the system's environment that holds `name`, for a test to
/// write over in place as C code may.
#[cfg(test)]
pub(crate) unsafe fn system_entry(name: &[u8]) -> *mut libc::c_char {
    unsafe {
        let mut at = environ();
        while !at.is_null() && !(*at).is_null() {
            let entry = std::ffi::CStr::from_ptr(*at).to_bytes();
            if entry.len() > name.len() && entry.starts_with(name) && entry[name.len()] == b'=' {
                return *at as *mut libc::c_char;
            }
            at = at.add(1);
        }
    }
    std::ptr::null_mut()
}

/// The program's `PATH`, from its copy.
pub(crate) fn path_var() -> Option<OsString> {
    state().as_ref().unwrap().table.get(b"PATH".as_slice()).map(|v| OsString::from_vec(v.clone()))
}

/// A change of the program's copy: what a child is given is made again,
/// and the system's environment is no longer the copy.
fn changed(state: &mut State) {
    state.prepared = None;
    state.system = None;
}

/// A `name=value` entry as std reads it: the name ends at the first `=`
/// after its first byte; an entry with none is no variable.
fn variable(entry: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let at = entry.iter().skip(1).position(|&b| b == b'=')? + 1;
    Some((entry[..at].to_vec(), entry[at + 1..].to_vec()))
}

/// The system's environment, entry by entry.
fn system_entries() -> Vec<Vec<u8>> {
    let mut entries = Vec::new();
    // SAFETY: `environ` ends with a null pointer, and each of its entries is
    // a C string. Another thread's `setenv` while this reads is the same
    // exposure std's own start of a child has.
    unsafe {
        let mut at = environ();
        while !at.is_null() && !(*at).is_null() {
            entries.push(std::ffi::CStr::from_ptr(*at).to_bytes().to_vec());
            at = at.add(1);
        }
    }
    entries
}

/// Whether the system's environment holds `entries`, in their order, and
/// nothing more, with nothing allocated.
///
/// # Safety
/// As `system_entries`.
unsafe fn system_is(entries: &[Vec<u8>]) -> bool {
    unsafe {
        let mut at = environ();
        for entry in entries {
            if at.is_null() || (*at).is_null() || std::ffi::CStr::from_ptr(*at).to_bytes() != entry.as_slice() {
                return false;
            }
            at = at.add(1);
        }
        at.is_null() || (*at).is_null()
    }
}

#[cfg(target_os = "macos")]
unsafe fn environ() -> *const *const libc::c_char {
    unsafe { *libc::_NSGetEnviron() as *const *const libc::c_char }
}

#[cfg(not(target_os = "macos"))]
unsafe fn environ() -> *const *const libc::c_char {
    unsafe extern "C" {
        static environ: *const *const libc::c_char;
    }
    unsafe { environ }
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
    let name = unsafe { str_bytes(&name) };
    state().as_ref().unwrap().table.get(name).map_or(std::ptr::null_mut(), |value| text(region, value))
}

/// `env.vars()`, by name, as a `Map[String, String]` with `glue`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_vars(glue: *const MapGlue) -> *mut MapObject {
    let mut map = super::tondo_rt_map_new(16, 8);
    let mut old = [0u8; 16];
    for (name, value) in &state().as_ref().unwrap().table {
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
    let (name, value) = unsafe { (str_bytes(&name), str_bytes(&value)) };
    if check(name, value, location) {
        let mut guard = state();
        let state = guard.as_mut().unwrap();
        state.table.insert(name.to_vec(), value.to_vec());
        changed(state);
    }
}

/// `env.setAll(values)`: a `Map[String, String]`, whose entries are two
/// string pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_set_all(values: *mut MapObject, location: *const Str) {
    let entries: Vec<(Vec<u8>, Vec<u8>)> = (0..unsafe { super::tondo_rt_map_len(values) })
        .map(|index| unsafe {
            let entry = super::tondo_rt_map_entry(values, index) as *const *const Str;
            (str_bytes(&*entry).to_vec(), str_bytes(&*entry.add(1)).to_vec())
        })
        .collect();
    if entries.iter().all(|(name, value)| check(name, value, location)) {
        let mut guard = state();
        let state = guard.as_mut().unwrap();
        state.table.extend(entries);
        changed(state);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_env_remove(name: *const Str, location: *const Str) {
    let name = unsafe { str_bytes(&name) };
    if check(name, b"", location) {
        let mut guard = state();
        let state = guard.as_mut().unwrap();
        state.table.remove(name);
        changed(state);
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

/// `env.workers()`: how many workers run the program's tasks at once,
/// `TONDO_WORKERS` or the processors.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_workers() -> i64 {
    crate::config::workers() as i64
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
    let path = std::ffi::OsStr::from_bytes(unsafe { str_bytes(&path) }).to_owned();
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

/// `env.syncNative()`: makes the host's environment the program's, which
/// children may then inherit as it is.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_env_sync_native() {
    let mut guard = state();
    let state = guard.as_mut().unwrap();
    for (name, _) in std::env::vars_os() {
        if !state.table.contains_key(name.as_bytes()) {
            unsafe { std::env::remove_var(name) };
        }
    }
    for (name, value) in &state.table {
        unsafe { std::env::set_var(std::ffi::OsStr::from_bytes(name), std::ffi::OsStr::from_bytes(value)) };
    }
    state.system = same(system_entries(), &state.table);
}
