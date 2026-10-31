//! The rest of `std.fs`, which `std/files.to` is written on (standard
//! library 10.5): walks, temporary entries, permissions, links and watching.
//! Each operation runs on a thread of its own while the task waits, and
//! gives a `Done`: its texts and numbers, or the code of its `FsError`.

use super::native::{self, NativeObject};
use super::{ArrayHeader, Str, str_bytes};
use crate::http::{string_array, strings};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn text<'a>(s: *const Str) -> &'a str {
    std::str::from_utf8(unsafe { str_bytes(s) }).unwrap_or_default()
}

fn ints(values: &[i64]) -> *mut ArrayHeader {
    let array = super::tondo_rt_array_new(8, values.len());
    for (index, &value) in values.iter().enumerate() {
        unsafe { *(super::element(array, 8, index) as *mut i64) = value };
    }
    array
}

/// The shortest path that names the same lexically, as `path.clean`.
fn clean(path: &str) -> String {
    let rooted = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => match parts.last() {
                Some(&last) if last != ".." => {
                    parts.pop();
                }
                _ if rooted => {}
                _ => parts.push(".."),
            },
            part => parts.push(part),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

// ---------------------------------------------------------------------------
// The filesystem
// ---------------------------------------------------------------------------

/// What an operation of the filesystem gives: its texts and numbers, or
/// the code of its `FsError` (1 and on); and a watch it started.
#[derive(Default)]
struct Done {
    failure: i64,
    texts: Vec<String>,
    numbers: Vec<i64>,
    watch: Option<Mutex<Watch>>,
}

/// The code of an error's `FsError`, as `fs.rs` gives it.
fn failure(error: &std::io::Error) -> i64 {
    1 + match error.kind() {
        ErrorKind::NotFound => 0,
        ErrorKind::PermissionDenied => 1,
        ErrorKind::AlreadyExists => 2,
        ErrorKind::NotADirectory => 3,
        ErrorKind::IsADirectory | ErrorKind::DirectoryNotEmpty => 4,
        ErrorKind::InvalidData => 5,
        ErrorKind::InvalidInput | ErrorKind::InvalidFilename => 6,
        _ => 7,
    }
}

/// The code of `FsError.InvalidName`, for a name that is not UTF-8.
const INVALID_NAME: i64 = 7;

/// Runs `job` on a host thread and gives its outcome as a `Done`.
fn done(job: impl FnOnce() -> Result<Done, i64> + Send + 'static) -> *mut NativeObject {
    let outcome = super::io::host(false, job);
    native::new(outcome.unwrap_or_else(|failure| Done { failure, ..Done::default() }))
}

fn io<T>(result: std::io::Result<T>) -> Result<T, i64> {
    result.map_err(|e| failure(&e))
}

fn utf8(path: PathBuf) -> Result<String, i64> {
    path.into_os_string().into_string().map_err(|_| INVALID_NAME)
}

fn texts(texts: Vec<String>) -> Done {
    Done { texts, ..Done::default() }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_failure(done: *mut NativeObject) -> i64 {
    native::get::<Done>(done).map_or(8, |d| d.failure)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_texts(done: *mut NativeObject) -> *mut ArrayHeader {
    string_array(native::get::<Done>(done).map_or(&[][..], |d| &d.texts))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_numbers(done: *mut NativeObject) -> *mut ArrayHeader {
    ints(native::get::<Done>(done).map_or(&[][..], |d| &d.numbers))
}

/// The `FileKind` index of a type.
fn kind(file: std::fs::FileType) -> i64 {
    if file.is_file() {
        0
    } else if file.is_dir() {
        1
    } else if file.is_symlink() {
        2
    } else {
        3
    }
}

/// `fs.metadata`: the kind, size, read-only flag, the seconds and
/// nanoseconds of the last change since 1970, the permissions, the inode,
/// the hard links and the ids of the owner and the group.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_stat(path: *const Str) -> *mut NativeObject {
    let path = text(path).to_string();
    done(move || {
        let metadata = io(std::fs::symlink_metadata(path))?;
        let size = i64::try_from(metadata.len()).map_err(|_| 8)?;
        let modified = io(metadata.modified())?;
        let (seconds, nanos) = match modified.duration_since(std::time::UNIX_EPOCH) {
            Ok(after) => (after.as_secs() as i64, after.subsec_nanos() as i64),
            Err(before) => {
                let before = before.duration();
                let nanos = before.subsec_nanos() as i64;
                (-(before.as_secs() as i64) - (nanos > 0) as i64, (1_000_000_000 - nanos) % 1_000_000_000)
            }
        };
        let readonly = metadata.permissions().readonly() as i64;
        let mode = (metadata.permissions().mode() & 0o7777) as i64;
        let (inode, links) = (metadata.ino() as i64, metadata.nlink() as i64);
        let (owner, group) = (metadata.uid() as i64, metadata.gid() as i64);
        let numbers =
            vec![kind(metadata.file_type()), size, readonly, seconds, nanos, mode, inode, links, owner, group];
        Ok(Done { numbers, ..Done::default() })
    })
}

/// `root` joined with a path below it, clean.
fn below(root: &str, relative: &str) -> String {
    clean(&format!("{root}/{relative}"))
}

/// The entries under a directory, each directory before its content and
/// siblings sorted by bytes, without following links: their paths below
/// `root` and their kinds.
fn walk(root: &str, depth: Option<usize>, out: &mut Vec<(String, i64)>) -> Result<(), i64> {
    fn visit(root: &str, relative: &str, depth: Option<usize>, out: &mut Vec<(String, i64)>) -> Result<(), i64> {
        let directory = if relative.is_empty() { root.to_string() } else { below(root, relative) };
        let mut entries = Vec::new();
        for entry in io(std::fs::read_dir(if directory.is_empty() { "." } else { &directory }))? {
            let entry = io(entry)?;
            let name = entry.file_name().into_string().map_err(|_| INVALID_NAME)?;
            entries.push((name, kind(io(entry.file_type())?)));
        }
        entries.sort();
        for (name, kind) in entries {
            let path = if relative.is_empty() { name } else { format!("{relative}/{name}") };
            out.push((path.clone(), kind));
            if kind == 1 && depth.is_none_or(|d| d > 1) {
                visit(root, &path, depth.map(|d| d - 1), out)?;
            }
        }
        Ok(())
    }
    visit(root, "", depth, out)
}

/// `fs.walk`: the paths and the kinds, `depth` levels deep, or all of them
/// when it is negative.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_walk(root: *const Str, depth: i64) -> *mut NativeObject {
    let root = text(root).to_string();
    done(move || {
        let mut entries = Vec::new();
        walk(&root, usize::try_from(depth).ok(), &mut entries)?;
        let (texts, numbers) = entries.into_iter().map(|(path, kind)| (below(&root, &path), kind)).unzip();
        Ok(Done { texts, numbers, ..Done::default() })
    })
}

/// `fs.createTemporaryDirectory` and `fs.createTemporaryFile`: a new entry
/// with a name no other has, which only its user can read.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_temporary(directory: u8) -> *mut NativeObject {
    done(move || {
        let base = std::env::temp_dir();
        loop {
            let random = super::codec::entropy(8);
            let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let path = base.join(format!("tondo-{name}"));
            let made = if directory != 0 {
                std::fs::DirBuilder::new().mode(0o700).create(&path)
            } else {
                std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map(|_| ())
            };
            match made {
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                made => {
                    io(made)?;
                    return Ok(texts(vec![utf8(path)?]));
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_set_permissions(path: *const Str, mode: i64) -> *mut NativeObject {
    let path = text(path).to_string();
    done(move || {
        io(std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode as u32 & 0o7777)))?;
        Ok(Done::default())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_symlink(target: *const Str, link: *const Str) -> *mut NativeObject {
    let (target, link) = (text(target).to_string(), text(link).to_string());
    done(move || {
        io(std::os::unix::fs::symlink(target, link))?;
        Ok(Done::default())
    })
}

/// `fs.createLink`: a hard link.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_link(target: *const Str, link: *const Str) -> *mut NativeObject {
    let (target, link) = (text(target).to_string(), text(link).to_string());
    done(move || {
        io(std::fs::hard_link(target, link))?;
        Ok(Done::default())
    })
}

/// `fs.setModified`: the nanoseconds since 1970 of the last change of the
/// content.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_set_modified(path: *const Str, nanos: i64) -> *mut NativeObject {
    let path = text(path).to_string();
    done(move || {
        let since = std::time::Duration::from_nanos(nanos.unsigned_abs());
        let at = if nanos >= 0 { std::time::UNIX_EPOCH + since } else { std::time::UNIX_EPOCH - since };
        let file = io(std::fs::File::open(path))?;
        io(file.set_times(std::fs::FileTimes::new().set_modified(at)))?;
        Ok(Done::default())
    })
}

/// `fs.space`: the bytes of the file system of the path, those free and
/// those free that the program may use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_space(path: *const Str) -> *mut NativeObject {
    let path = text(path).to_string();
    done(move || {
        let named = std::ffi::CString::new(path).map_err(|_| INVALID_NAME)?;
        let mut found: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(named.as_ptr(), &mut found) } != 0 {
            return Err(failure(&std::io::Error::last_os_error()));
        }
        let unit = found.f_frsize as i64;
        let numbers = vec![found.f_blocks as i64 * unit, found.f_bfree as i64 * unit, found.f_bavail as i64 * unit];
        Ok(Done { numbers, ..Done::default() })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_read_symlink(path: *const Str) -> *mut NativeObject {
    let path = text(path).to_string();
    done(move || Ok(texts(vec![utf8(io(std::fs::read_link(path))?)?])))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_real_path(path: *const Str) -> *mut NativeObject {
    let path = text(path).to_string();
    done(move || Ok(texts(vec![utf8(io(std::fs::canonicalize(path))?)?])))
}

// ---------------------------------------------------------------------------
// Watching
// ---------------------------------------------------------------------------

/// A watch: the system's watcher, the paths it says changed, the paths it
/// watches, as given and as the system names them, and the entries under
/// them with their stamps. A batch decides what became of each path from
/// the stamps, not from the system's events, which kqueue gives for a
/// directory rather than what it holds, and sometimes twice.
struct Watch {
    watcher: Option<notify::RecommendedWatcher>,
    /// Out while a task waits for the next batch, and after the watch ends.
    changes: Option<tokio::sync::mpsc::UnboundedReceiver<PathBuf>>,
    roots: Vec<(String, PathBuf)>,
    settle: std::time::Duration,
    existing: HashMap<PathBuf, Stamp>,
}

/// What tells that an entry changed: its kind, last change, size and file.
#[derive(Copy, Clone, PartialEq)]
struct Stamp {
    directory: bool,
    modified: i64,
    size: u64,
    file: u64,
}

/// An entry's stamp, without following a link; `None` when it is gone.
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let modified = metadata.mtime().saturating_mul(1_000_000_000).saturating_add(metadata.mtime_nsec());
    Some(Stamp { directory: metadata.is_dir(), modified, size: metadata.len(), file: metadata.ino() })
}

/// The entries under a directory, `depth` levels deep or all of them, with
/// their stamps, into `out`.
fn stamps(directory: &Path, depth: Option<usize>, out: &mut HashMap<PathBuf, Stamp>) {
    let mut entries = Vec::new();
    if walk(&directory.to_string_lossy(), depth, &mut entries).is_ok() {
        for (entry, _) in entries {
            let path = directory.join(entry);
            if let Some(found) = stamp(&path) {
                out.insert(path, found);
            }
        }
    }
}

/// `fs.watch`: watches each path, with what its directories hold.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_watch(paths: *mut ArrayHeader, settle: i64) -> *mut NativeObject {
    use notify::Watcher;
    let paths = strings(paths);
    let settle = std::time::Duration::from_nanos(settle.max(0) as u64);
    done(move || {
        let (send, changes) = tokio::sync::mpsc::unbounded_channel();
        // An event says which paths changed; what became of each is read
        // when its batch ends.
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if !matches!(event.kind, notify::EventKind::Access(_)) {
                for path in event.paths {
                    let _ = send.send(path);
                }
            }
        })
        .map_err(|_| 8)?;
        let (mut roots, mut existing) = (Vec::new(), HashMap::new());
        for path in paths {
            let real = io(std::fs::canonicalize(&path))?;
            if let Some(found) = stamp(&real) {
                if found.directory {
                    stamps(&real, None, &mut existing);
                }
                existing.insert(real.clone(), found);
            }
            watcher.watch(&real, notify::RecursiveMode::Recursive).map_err(|e| match e.kind {
                notify::ErrorKind::Io(e) => failure(&e),
                notify::ErrorKind::PathNotFound => 1,
                _ => 8,
            })?;
            roots.push((path, real));
        }
        let watch = Watch { watcher: Some(watcher), changes: Some(changes), roots, settle, existing };
        Ok(Done { watch: Some(Mutex::new(watch)), ..Done::default() })
    })
}

/// The next batch of changes, once `settle` passes without another: each
/// path once, sorted, after the letter of its change (`C`, `M` or `R`).
/// One empty text when a second passes without a change, for the caller to
/// see whether anyone still receives; nothing when the watch ended or the
/// task is cancelled.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_watch_next(done: *mut NativeObject) -> *mut ArrayHeader {
    let Some(watch) = native::get::<Done>(done).and_then(|d| d.watch.as_ref()) else {
        return string_array(&[]);
    };
    let lock = || watch.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let (receiver, roots, settle) = {
            let mut watch = lock();
            let Some(receiver) = watch.changes.take() else { return string_array(&[]) };
            (receiver, watch.roots.clone(), watch.settle)
        };
        let batch = crate::http::wait(async move {
            let mut receiver = receiver;
            let first = tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv()).await;
            let mut changes = match first {
                Ok(change) => vec![change?],
                Err(_) => Vec::new(),
            };
            while !changes.is_empty()
                && let Ok(Some(change)) = tokio::time::timeout(settle, receiver.recv()).await
            {
                changes.push(change);
            }
            Some((changes, receiver))
        });
        let Some(Some((changes, receiver))) = batch else { return string_array(&[]) };
        let mut watch = lock();
        watch.changes = Some(receiver);
        if changes.is_empty() {
            return string_array(&[String::new()]);
        }
        let out = settled(&mut watch, &roots, changes);
        drop(watch);
        if !out.is_empty() {
            return string_array(&out);
        }
    }
}

/// What became of the paths the system says changed, from their stamps:
/// each with the letter of its change. A directory that changed is read
/// again for the entries it holds, one made is read whole, and one removed
/// takes what it held with it; a directory itself is only made or removed,
/// as what it holds changes on its own.
fn settled(watch: &mut Watch, roots: &[(String, PathBuf)], changes: Vec<PathBuf>) -> Vec<String> {
    let mut pending: Vec<PathBuf> = changes.into_iter().collect::<HashSet<_>>().into_iter().collect();
    for path in pending.clone() {
        let held = |watch: &Watch, deep: bool| -> Vec<PathBuf> {
            let known = watch.existing.keys();
            known.filter(|k| if deep { k.starts_with(&path) } else { k.parent() == Some(&path) }).cloned().collect()
        };
        match stamp(&path) {
            Some(found) if found.directory && watch.existing.contains_key(&path) => {
                let mut now = HashMap::new();
                stamps(&path, Some(1), &mut now);
                pending.extend(now.into_keys().chain(held(watch, false)));
            }
            None if watch.existing.get(&path).is_some_and(|s| s.directory) => pending.extend(held(watch, true)),
            _ => {}
        }
    }
    let mut states: BTreeMap<String, &str> = BTreeMap::new();
    while let Some(path) = pending.pop() {
        let after = stamp(&path);
        let before = match after {
            Some(found) => watch.existing.insert(path.clone(), found),
            None => watch.existing.remove(&path),
        };
        let letter = match (before, after) {
            (None, Some(made)) => {
                if made.directory {
                    let mut inner = HashMap::new();
                    stamps(&path, None, &mut inner);
                    pending.extend(inner.into_keys());
                }
                "C"
            }
            (Some(_), None) => "R",
            (Some(old), Some(new)) if old != new && !new.directory => "M",
            _ => continue,
        };
        let root = roots.iter().any(|(_, real)| *real == path);
        if let Some(shown) = shown(roots, &path)
            && !(root && after.is_some_and(|s| s.directory))
        {
            states.insert(shown, letter);
        }
    }
    states.into_iter().flat_map(|(path, letter)| [letter.to_string(), path]).collect()
}

/// A path the system reports, as its watched path names it.
fn shown(roots: &[(String, PathBuf)], path: &Path) -> Option<String> {
    roots.iter().find_map(|(given, real)| {
        let rest = path.strip_prefix(real).ok()?.to_str()?;
        Some(if rest.is_empty() { clean(given) } else { below(given, rest) })
    })
}

/// Stops a watch, which then gives no more batches.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_watch_stop(done: *mut NativeObject) {
    if let Some(watch) = native::get::<Done>(done).and_then(|d| d.watch.as_ref()) {
        let mut watch = watch.lock().unwrap_or_else(|e| e.into_inner());
        watch.watcher = None;
        watch.changes = None;
    }
}
