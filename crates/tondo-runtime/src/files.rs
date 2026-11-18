//! The rest of `std.fs`, which `std/files.to` is written on (standard
//! library 10.5): walks, temporary entries, permissions, links and watching.
//! Each operation runs on a thread of its own while the task waits, and
//! gives a `Done`: its texts and numbers, or the code of its `FsError`.

use super::native::{self, NativeObject};
use super::{ArrayHeader, Str};
use crate::http::strings;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn text<'a>(s: &'a *const Str) -> &'a str {
    unsafe { crate::str_text(s) }
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

/// The code of an error's `FsError`, as `fs.rs` gives it; for `Host`, the
/// system's number of the error above its low byte.
fn failure(error: &std::io::Error) -> i64 {
    if !matches!(
        error.kind(),
        ErrorKind::NotFound
            | ErrorKind::PermissionDenied
            | ErrorKind::AlreadyExists
            | ErrorKind::NotADirectory
            | ErrorKind::IsADirectory
            | ErrorKind::DirectoryNotEmpty
            | ErrorKind::InvalidData
            | ErrorKind::InvalidInput
            | ErrorKind::InvalidFilename
    ) {
        return 8 | (error.raw_os_error().unwrap_or(0) as i64) << 8;
    }
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
    native::get::<Done>(done).map_or(8, |d| d.failure & 0xff)
}

/// The system's number of a `Host` failure.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_host_error(done: *mut NativeObject) -> i64 {
    native::get::<Done>(done).map_or(0, |d| d.failure >> 8)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_texts(
    done: *mut NativeObject,
    region: *mut crate::regions::Region,
) -> *mut ArrayHeader {
    crate::arrays::string_array_in(region, native::get::<Done>(done).map_or(&[][..], |d| &d.texts))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_numbers(
    done: *mut NativeObject,
    region: *mut crate::regions::Region,
) -> *mut ArrayHeader {
    crate::arrays::int_array_in(region, native::get::<Done>(done).map_or(&[][..], |d| &d.numbers))
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
    let path = text(&path).to_string();
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
/// The entries under `root`, `depth` levels deep or all; with `skip`, a
/// directory below it that cannot be read is left out rather than failing.
fn walk(root: &str, depth: Option<usize>, skip: bool, out: &mut Vec<(String, i64)>) -> Result<(), i64> {
    fn visit(
        root: &str,
        relative: &str,
        depth: Option<usize>,
        skip: bool,
        out: &mut Vec<(String, i64)>,
    ) -> Result<(), i64> {
        let directory = if relative.is_empty() { root.to_string() } else { below(root, relative) };
        let mut entries = Vec::new();
        let listed = std::fs::read_dir(if directory.is_empty() { "." } else { &directory });
        if listed.is_err() && skip && !relative.is_empty() {
            return Ok(());
        }
        for entry in io(listed)? {
            let entry = io(entry)?;
            let name = entry.file_name().into_string().map_err(|_| INVALID_NAME)?;
            entries.push((name, kind(io(entry.file_type())?)));
        }
        entries.sort();
        for (name, kind) in entries {
            let path = if relative.is_empty() { name } else { format!("{relative}/{name}") };
            out.push((path.clone(), kind));
            if kind == 1 && depth.is_none_or(|d| d > 1) {
                visit(root, &path, depth.map(|d| d - 1), skip, out)?;
            }
        }
        Ok(())
    }
    visit(root, "", depth, skip, out)
}

/// `fs.walk`: the paths and the kinds, `depth` levels deep, or all of them
/// when it is negative.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_walk(root: *const Str, depth: i64, skip: u8) -> *mut NativeObject {
    let root = text(&root).to_string();
    done(move || {
        let mut entries = Vec::new();
        walk(&root, usize::try_from(depth).ok(), skip != 0, &mut entries)?;
        let (texts, numbers) = entries.into_iter().map(|(path, kind)| (below(&root, &path), kind)).unzip();
        Ok(Done { texts, numbers, ..Done::default() })
    })
}

/// `fs.createTemporaryDirectory` and `fs.createTemporaryFile`: a new entry
/// in `place`, or the system's temporary directory, named by `pattern` with
/// its last `*`, or its end, made random so that no other has it, which only
/// its user can read.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_temporary(
    directory: u8,
    place: *const Str,
    pattern: *const Str,
) -> *mut NativeObject {
    let (place, pattern) = (text(&place).to_string(), text(&pattern).to_string());
    done(move || {
        let base = if place.is_empty() { std::env::temp_dir() } else { PathBuf::from(place) };
        let pattern = if pattern.is_empty() { "tondo-*".to_string() } else { pattern };
        if pattern.contains('/') {
            return Err(INVALID_NAME);
        }
        let (before, after) = pattern.rsplit_once('*').unwrap_or((&pattern, ""));
        loop {
            let random = super::codec::entropy(8);
            let name: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let path = base.join(format!("{before}{name}{after}"));
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
    let path = text(&path).to_string();
    done(move || {
        io(std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode as u32 & 0o7777)))?;
        Ok(Done::default())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_symlink(target: *const Str, link: *const Str) -> *mut NativeObject {
    let (target, link) = (text(&target).to_string(), text(&link).to_string());
    done(move || {
        io(std::os::unix::fs::symlink(target, link))?;
        Ok(Done::default())
    })
}

/// `fs.createLink`: a hard link.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_link(target: *const Str, link: *const Str) -> *mut NativeObject {
    let (target, link) = (text(&target).to_string(), text(&link).to_string());
    done(move || {
        io(std::fs::hard_link(target, link))?;
        Ok(Done::default())
    })
}

/// `fs.setModified`: the nanoseconds since 1970 of the last change of the
/// content.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_set_modified(path: *const Str, nanos: i64) -> *mut NativeObject {
    let path = text(&path).to_string();
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
    let path = text(&path).to_string();
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
    let path = text(&path).to_string();
    done(move || Ok(texts(vec![utf8(io(std::fs::read_link(path))?)?])))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_real_path(path: *const Str) -> *mut NativeObject {
    let path = text(&path).to_string();
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
    watcher: Option<Watcher>,
    /// Out while a task waits for the next batch, and after the watch ends.
    changes: Option<tokio::sync::mpsc::UnboundedReceiver<Seen>>,
    roots: Vec<(String, PathBuf)>,
    settle: std::time::Duration,
    existing: HashMap<PathBuf, Stamp>,
    /// Whether the system says each file that was written, as inotify does.
    told: bool,
    /// When it does not, the content of each file whose stamp may not show
    /// its next write, as `content` gives it.
    racy: HashMap<PathBuf, Option<u64>>,
}

/// A path the system says changed, and whether it said the file itself was
/// written, which inotify tells and kqueue does not.
type Seen = (PathBuf, bool);

/// What tells that an entry changed: its kind, last change, size and file.
#[derive(Copy, Clone, PartialEq)]
struct Stamp {
    directory: bool,
    regular: bool,
    modified: i64,
    size: u64,
    file: u64,
}

/// An entry's stamp, without following a link; `None` when it is gone.
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let modified = metadata.mtime().saturating_mul(1_000_000_000).saturating_add(metadata.mtime_nsec());
    let (directory, regular) = (metadata.is_dir(), metadata.is_file());
    Some(Stamp { directory, regular, modified, size: metadata.len(), file: metadata.ino() })
}

/// How soon after a file's last change its stamp is racy: a write of the
/// same size within the same tick of the file system's clock leaves the
/// stamp as it was, and the coarsest clock a watch meets, FAT's, ticks
/// every two seconds.
const RACY: i64 = 2_000_000_000;

/// The largest file whose content a racy stamp keeps; a larger one whose
/// racy stamp stays the same counts as modified, without reading it.
const HASHED: u64 = 1 << 20;

/// The time, as stamps give it.
fn now() -> i64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    i64::try_from(since.as_nanos()).unwrap_or(i64::MAX)
}

/// Whether a regular file's stamp, read after `now`, may not show its next
/// write.
fn racy(now: i64, stamp: &Stamp) -> bool {
    stamp.regular && now.saturating_sub(stamp.modified) < RACY
}

/// A hash of what a file of `size` holds, for a racy stamp; `None` past
/// `HASHED` or when it cannot be read, and then the file counts as modified.
fn content(path: &Path, size: u64) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    use std::io::Read;
    if size > HASHED {
        return None;
    }
    let mut held = Vec::new();
    std::fs::File::open(path).ok()?.take(HASHED + 1).read_to_end(&mut held).ok()?;
    if held.len() as u64 > HASHED {
        return None;
    }
    let mut hasher = std::hash::DefaultHasher::new();
    held.hash(&mut hasher);
    Some(hasher.finish())
}

/// The content of each entry whose stamp, read after `now`, is racy.
fn racy_contents(existing: &HashMap<PathBuf, Stamp>, now: i64) -> HashMap<PathBuf, Option<u64>> {
    let racy = existing.iter().filter(|(_, stamp)| racy(now, stamp));
    racy.map(|(path, stamp)| (path.clone(), content(path, stamp.size))).collect()
}

/// The entries under a directory, `depth` levels deep or all of them, with
/// their stamps, into `out`; a directory below it that cannot be read is
/// not watched, and the rest is.
fn stamps(directory: &Path, depth: Option<usize>, out: &mut HashMap<PathBuf, Stamp>) {
    let mut entries = Vec::new();
    if walk(&directory.to_string_lossy(), depth, true, &mut entries).is_ok() {
        for (entry, _) in entries {
            let path = directory.join(entry);
            if let Some(found) = stamp(&path) {
                out.insert(path, found);
            }
        }
    }
}

// The system's watcher, which sends each path it says changed: inotify's
// or kqueue's, whose events a task of the runtime reads, so that a recorded
// run makes its calls on the program's thread.
#[cfg(target_os = "linux")]
use inotify::{Watcher, watch, watcher};
#[cfg(target_os = "macos")]
use kqueue::{Watcher, watch, watcher};

#[cfg(target_os = "linux")]
mod inotify {
    use super::failure;
    use std::collections::HashMap;
    use std::ffi::OsStr;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard};
    use tokio::sync::mpsc::UnboundedSender;

    const MASK: u32 = libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_MODIFY
        | libc::IN_CLOSE_WRITE
        | libc::IN_ATTRIB
        | libc::IN_MOVED_FROM
        | libc::IN_MOVED_TO
        | libc::IN_DELETE_SELF
        | libc::IN_MOVE_SELF;

    /// A watch, and the task that reads its events, which ends with it.
    pub(super) struct Watcher {
        watches: Arc<Watches>,
        reading: tokio::task::AbortHandle,
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            self.reading.abort();
        }
    }

    /// inotify's descriptor, and the path of each of its watches.
    struct Watches {
        fd: OwnedFd,
        paths: Mutex<HashMap<i32, PathBuf>>,
    }

    pub(super) fn watcher(send: UnboundedSender<super::Seen>) -> Result<Watcher, i64> {
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(failure(&std::io::Error::last_os_error()));
        }
        let watches = Arc::new(Watches { fd: unsafe { OwnedFd::from_raw_fd(fd) }, paths: Mutex::default() });
        let reading = crate::http::runtime().spawn(read(watches.clone(), send)).abort_handle();
        Ok(Watcher { watches, reading })
    }

    pub(super) fn watch(watcher: &mut Watcher, path: &Path) -> Result<(), i64> {
        watcher.watches.add(path).map_err(|e| failure(&e))
    }

    impl Watches {
        fn paths(&self) -> MutexGuard<'_, HashMap<i32, PathBuf>> {
            self.paths.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// Watches `path`, and each directory below it.
        fn add(&self, path: &Path) -> std::io::Result<()> {
            let name = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            let watch = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), name.as_ptr(), MASK) };
            if watch < 0 {
                return Err(std::io::Error::last_os_error());
            }
            self.paths().insert(watch, path.to_path_buf());
            for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    let _ = self.add(&entry.path());
                }
            }
            Ok(())
        }

        /// The paths an event says changed, watching a directory that
        /// came and forgetting one that went.
        fn changed(&self, event: &libc::inotify_event, name: &[u8]) -> Vec<PathBuf> {
            // Events were lost: any watched directory may have changed.
            if event.mask & libc::IN_Q_OVERFLOW != 0 {
                return self.paths().values().cloned().collect();
            }
            if event.mask & libc::IN_IGNORED != 0 {
                self.paths().remove(&event.wd);
                return Vec::new();
            }
            let Some(directory) = self.paths().get(&event.wd).cloned() else { return Vec::new() };
            let path = if name.is_empty() { directory } else { directory.join(OsStr::from_bytes(name)) };
            if event.mask & libc::IN_ISDIR != 0 {
                if event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
                    let _ = self.add(&path);
                }
                if event.mask & libc::IN_MOVED_FROM != 0 {
                    let fd = self.fd.as_raw_fd();
                    self.paths().retain(|&watch, at| {
                        let kept = !at.starts_with(&path);
                        if !kept {
                            unsafe { libc::inotify_rm_watch(fd, watch) };
                        }
                        kept
                    });
                }
            }
            vec![path]
        }
    }

    /// Sends the paths of each event, until the watch or its receiver ends.
    async fn read(watches: Arc<Watches>, send: UnboundedSender<super::Seen>) {
        let fd = watches.fd.as_raw_fd();
        // The descriptor is the watch's, open while this task holds it.
        let ready = unsafe { tokio::io::unix::AsyncFd::register_with_interest(fd, tokio::io::Interest::READABLE) };
        let Ok(ready) = ready else { return };
        let head = std::mem::size_of::<libc::inotify_event>();
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            let Ok(mut guard) = ready.readable().await else { return };
            let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if read < 0 {
                if std::io::Error::last_os_error().kind() != std::io::ErrorKind::WouldBlock {
                    return;
                }
                guard.clear_ready();
                continue;
            }
            let (read, mut at) = (read as usize, 0);
            while at + head <= read {
                let event = unsafe { buffer.as_ptr().add(at).cast::<libc::inotify_event>().read_unaligned() };
                let name = &buffer[at + head..(at + head + event.len as usize).min(read)];
                at += head + event.len as usize;
                let name = &name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())];
                // The file itself written: so it changed, whatever its stamp.
                let written =
                    event.mask & (libc::IN_MODIFY | libc::IN_CLOSE_WRITE) != 0 && event.mask & libc::IN_ISDIR == 0;
                for path in watches.changed(&event, name) {
                    if send.send((path, written)).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod kqueue {
    use super::failure;
    use std::collections::{BTreeMap, HashMap};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard};
    use tokio::sync::mpsc::UnboundedSender;

    const NOTES: u32 = libc::NOTE_WRITE
        | libc::NOTE_EXTEND
        | libc::NOTE_ATTRIB
        | libc::NOTE_DELETE
        | libc::NOTE_RENAME
        | libc::NOTE_REVOKE;

    /// A watch, and the task that reads its events, which ends with it.
    pub(super) struct Watcher {
        watches: Arc<Watches>,
        reading: tokio::task::AbortHandle,
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            self.reading.abort();
        }
    }

    /// The kqueue, and what it watches: kqueue watches open files, each
    /// entry by itself.
    struct Watches {
        fd: OwnedFd,
        open: Mutex<Open>,
    }

    #[derive(Default)]
    struct Open {
        files: BTreeMap<PathBuf, OwnedFd>,
        paths: HashMap<RawFd, PathBuf>,
    }

    pub(super) fn watcher(send: UnboundedSender<super::Seen>) -> Result<Watcher, i64> {
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return Err(failure(&std::io::Error::last_os_error()));
        }
        let watches = Arc::new(Watches { fd: unsafe { OwnedFd::from_raw_fd(fd) }, open: Mutex::default() });
        let reading = crate::http::runtime().spawn(read(watches.clone(), send)).abort_handle();
        Ok(Watcher { watches, reading })
    }

    pub(super) fn watch(watcher: &mut Watcher, path: &Path) -> Result<(), i64> {
        watcher.watches.add(path).map_err(|e| failure(&e))
    }

    impl Watches {
        fn open(&self) -> MutexGuard<'_, Open> {
            self.open.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// Watches `path` and what it holds, unless it does already.
        fn add(&self, path: &Path) -> std::io::Result<()> {
            if !self.open().files.contains_key(path) {
                let name = std::ffi::CString::new(path.as_os_str().as_bytes())?;
                let fd = unsafe { libc::open(name.as_ptr(), libc::O_EVTONLY | libc::O_SYMLINK | libc::O_CLOEXEC) };
                if fd < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let file = unsafe { OwnedFd::from_raw_fd(fd) };
                let mut change: libc::kevent = unsafe { std::mem::zeroed() };
                change.ident = fd as usize;
                change.filter = libc::EVFILT_VNODE;
                change.flags = libc::EV_ADD | libc::EV_CLEAR;
                change.fflags = NOTES;
                let none = std::ptr::null_mut();
                if unsafe { libc::kevent(self.fd.as_raw_fd(), &change, 1, none, 0, std::ptr::null()) } < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let mut open = self.open();
                open.files.insert(path.to_path_buf(), file);
                open.paths.insert(fd, path.to_path_buf());
            }
            self.entries(path);
            Ok(())
        }

        /// Watches the entries of `path`, when it is a directory.
        fn entries(&self, path: &Path) {
            for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                let _ = self.add(&entry.path());
            }
        }

        /// The path an event says changed, watching what a directory
        /// gained, and what is at a path now instead of what went from it.
        fn changed(&self, event: &libc::kevent) -> Option<PathBuf> {
            let path = self.open().paths.get(&(event.ident as RawFd)).cloned()?;
            if event.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE) != 0 {
                let mut open = self.open();
                let gone: Vec<PathBuf> = open.files.keys().filter(|p| p.starts_with(&path)).cloned().collect();
                for at in gone {
                    if let Some(file) = open.files.remove(&at) {
                        open.paths.remove(&file.as_raw_fd());
                    }
                }
                drop(open);
                let _ = self.add(&path);
            } else if event.fflags & libc::NOTE_WRITE != 0 {
                self.entries(&path);
            }
            Some(path)
        }
    }

    /// Sends the path of each event, until the watch or its receiver ends.
    async fn read(watches: Arc<Watches>, send: UnboundedSender<super::Seen>) {
        let fd = watches.fd.as_raw_fd();
        // The descriptor is the watch's, open while this task holds it.
        let ready = unsafe { tokio::io::unix::AsyncFd::register_with_interest(fd, tokio::io::Interest::READABLE) };
        let Ok(ready) = ready else { return };
        loop {
            let Ok(mut guard) = ready.readable().await else { return };
            let mut events: [libc::kevent; 64] = unsafe { std::mem::zeroed() };
            let now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            let count = unsafe { libc::kevent(fd, std::ptr::null(), 0, events.as_mut_ptr(), 64, &now) };
            if count < 0 {
                return;
            }
            if count == 0 {
                guard.clear_ready();
            }
            for event in &events[..count as usize] {
                if let Some(path) = watches.changed(event)
                    && send.send((path, false)).is_err()
                {
                    return;
                }
            }
        }
    }
}

/// `fs.watch`: watches each path, with what its directories hold.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_watch(paths: *mut ArrayHeader, settle: i64) -> *mut NativeObject {
    let paths = strings(paths);
    let settle = std::time::Duration::from_nanos(settle.max(0) as u64);
    done(move || {
        let (send, changes) = tokio::sync::mpsc::unbounded_channel();
        // An event says which paths changed; what became of each is read
        // when its batch ends.
        let mut watcher = watcher(send)?;
        let (mut roots, mut existing) = (Vec::new(), HashMap::new());
        let started = now();
        for path in paths {
            let real = io(std::fs::canonicalize(&path))?;
            if let Some(found) = stamp(&real) {
                if found.directory {
                    stamps(&real, None, &mut existing);
                }
                existing.insert(real.clone(), found);
            }
            watch(&mut watcher, &real)?;
            roots.push((path, real));
        }
        // kqueue tells that a directory changed, or a file, not how: a
        // file's stamp can stay as it was through a write, and its content
        // decides while it can.
        let told = cfg!(target_os = "linux");
        let racy = if told { HashMap::new() } else { racy_contents(&existing, started) };
        let watch = Watch { watcher: Some(watcher), changes: Some(changes), roots, settle, existing, told, racy };
        Ok(Done { watch: Some(Mutex::new(watch)), ..Done::default() })
    })
}

/// The next batch of changes, once `settle` passes without another: each
/// path once, sorted, after the letter of its change (`C`, `M` or `R`).
/// One empty text when a second passes without a change, for the caller to
/// see whether anyone still receives; nothing when the watch ended or the
/// task is cancelled.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_files_watch_next(
    done: *mut NativeObject,
    region: *mut crate::regions::Region,
) -> *mut ArrayHeader {
    let Some(watch) = native::get::<Done>(done).and_then(|d| d.watch.as_ref()) else {
        return crate::arrays::string_array_in(region, &[]);
    };
    let lock = || watch.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let (receiver, roots, settle) = {
            let mut watch = lock();
            let Some(receiver) = watch.changes.take() else { return crate::arrays::string_array_in(region, &[]) };
            (receiver, watch.roots.clone(), watch.settle)
        };
        let batch = crate::http::wait(async move {
            let mut receiver = receiver;
            let changes = batch(&mut receiver, settle).await?;
            Some((changes, receiver))
        });
        let Some(Some((changes, receiver))) = batch else { return crate::arrays::string_array_in(region, &[]) };
        let mut watch = lock();
        watch.changes = Some(receiver);
        if changes.is_empty() {
            return crate::arrays::string_array_in(region, &[String::new()]);
        }
        let out = settled(&mut watch, &roots, changes);
        drop(watch);
        if !out.is_empty() {
            return crate::arrays::string_array_in(region, &out);
        }
    }
}

/// The paths of the next batch: the first change that comes within a
/// second, and each that comes within `settle` of the one before it; none
/// when a second passes without a change, and `None` once the watch ended.
async fn batch<T>(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<T>,
    settle: std::time::Duration,
) -> Option<Vec<T>> {
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
    Some(changes)
}

/// What became of the paths the system says changed, from their stamps:
/// each with the letter of its change. A directory that changed is read
/// again for the entries it holds, one made is read whole, and one removed
/// takes what it held with it; a directory itself is only made or removed,
/// as what it holds changes on its own. A file whose stamp was racy and is
/// the same is compared by its content, where the system does not say that
/// it was written.
fn settled(watch: &mut Watch, roots: &[(String, PathBuf)], changes: Vec<Seen>) -> Vec<String> {
    let read = now();
    // A file the system said was written changed, though a write of the
    // same size in the same tick of its clock leaves its stamp as it was.
    let written: HashSet<PathBuf> =
        changes.iter().filter(|(_, written)| *written).map(|(path, _)| path.clone()).collect();
    let mut pending: Vec<PathBuf> =
        changes.into_iter().map(|(path, _)| path).collect::<HashSet<_>>().into_iter().collect();
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
        // A file `fs` writes whole appears or changes at once.
        if crate::fs::temporary(&path) {
            continue;
        }
        let after = stamp(&path);
        let before = match after {
            Some(found) => watch.existing.insert(path.clone(), found),
            None => watch.existing.remove(&path),
        };
        let kept = watch.racy.remove(&path);
        let racy_now = !watch.told && after.is_some_and(|found| racy(read, &found));
        let same = before.is_some() && before == after;
        let compared = racy_now || (same && kept.is_some_and(|kept| kept.is_some()));
        let held = after.filter(|_| compared).and_then(|found| content(&path, found.size));
        let rewritten = same && kept.is_some_and(|kept| kept.is_none() || kept != held);
        if racy_now {
            watch.racy.insert(path.clone(), held);
        }
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
            (Some(old), Some(new)) if (old != new || written.contains(&path) || rewritten) && !new.directory => "M",
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own for a test, with `files` in it.
    fn directory(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tondo-watch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            std::fs::write(dir.join(file), file).unwrap();
        }
        dir.canonicalize().unwrap()
    }

    /// A watch of `dir` as it starts, where the system says each file that
    /// was written, as inotify does, or not, as kqueue.
    fn watching(dir: &Path, told: bool) -> Watch {
        let started = now();
        let mut existing = HashMap::new();
        stamps(dir, None, &mut existing);
        let roots = vec![(dir.to_string_lossy().into_owned(), dir.to_path_buf())];
        let racy = if told { HashMap::new() } else { racy_contents(&existing, started) };
        Watch { watcher: None, changes: None, roots, settle: std::time::Duration::ZERO, existing, told, racy }
    }

    /// Gives `file` the stamp of a change at `at`, as if it changed then.
    fn changed_at(file: &Path, at: std::time::SystemTime) {
        std::fs::File::options().write(true).open(file).unwrap().set_modified(at).unwrap();
    }

    /// A batch says what became of each path, once, sorted, from what is
    /// there when it settles: a file made and removed within it is not
    /// there, nor the temporary file through which `fs` writes one whole.
    #[test]
    fn a_batch_says_what_became_of_each_path() {
        for told in [true, false] {
            a_batch_says_what_became_of_each_path_where(told);
        }
    }

    fn a_batch_says_what_became_of_each_path_where(told: bool) {
        let dir = directory("batch", &["old.txt", "gone.txt"]);
        let mut watch = watching(&dir, told);
        std::fs::write(dir.join("new.txt"), "new").unwrap();
        // Of another size: a write of the same size in the same tick of the
        // file system's clock, in the same file, leaves its stamp as it was.
        std::fs::write(dir.join("old.txt"), "changed, and longer").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::write(dir.join("brief.txt"), "brief").unwrap();
        std::fs::remove_file(dir.join("brief.txt")).unwrap();
        std::fs::write(dir.join(".draft.txt.tondo-1-2"), "as fs names them").unwrap();
        let changed = ["new.txt", "old.txt", "gone.txt", "brief.txt", ".draft.txt.tondo-1-2", "new.txt"];
        let roots = watch.roots.clone();
        let out = settled(&mut watch, &roots, changed.iter().map(|name| (dir.join(name), false)).collect());
        let at = |name: &str| dir.join(name).to_string_lossy().into_owned();
        assert_eq!(out, ["R", &at("gone.txt"), "C", &at("new.txt"), "M", &at("old.txt")]);
        // What it said is what the next batch starts from.
        assert!(settled(&mut watch, &roots, vec![(dir.join("new.txt"), false)]).is_empty());
        // A directory made with what it holds, which the system may tell
        // of as the directory alone.
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/inner.txt"), "inner").unwrap();
        let out = settled(&mut watch, &roots, vec![(dir.join("sub"), false)]);
        assert_eq!(out, ["C", &at("sub"), "C", &at("sub/inner.txt")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A file the system said was written is modified, though the write
    /// left its stamp as it was, as one of the same size in the same tick
    /// of the file system's clock does: here it is put back by hand.
    #[test]
    fn a_file_written_is_modified_whatever_its_stamp() {
        let dir = directory("written", &["same.txt"]);
        let mut watch = watching(&dir, true);
        let path = dir.join("same.txt");
        let stamped = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "SAME.TXT").unwrap();
        changed_at(&path, stamped);
        let roots = watch.roots.clone();
        // Seen only through its directory, as kqueue tells it: the stamp decides.
        assert!(settled(&mut watch, &roots, vec![(path.clone(), false)]).is_empty());
        let at = path.to_string_lossy().into_owned();
        assert_eq!(settled(&mut watch, &roots, vec![(path.clone(), true)]), ["M", &at]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Where the system does not say that a file was written, as kqueue, a
    /// stamp read within two seconds of the file's last change is racy, and
    /// while it stays the same the file's content decides: up to a
    /// mebibyte, and past it the file counts as modified. An older stamp
    /// decides alone, and nothing is read. The stamps here are put ahead of
    /// the clock or behind it, so that how long the test takes does not
    /// change what is racy.
    #[test]
    fn a_racy_stamp_is_compared_by_content() {
        let dir = directory("racy", &["same.txt"]);
        let (path, big) = (dir.join("same.txt"), dir.join("big.bin"));
        std::fs::write(&big, vec![0; HASHED as usize + 1]).unwrap();
        let ahead = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        changed_at(&path, ahead);
        changed_at(&big, ahead);
        let mut watch = watching(&dir, false);
        let roots = watch.roots.clone();
        let at = |path: &Path| path.to_string_lossy().into_owned();
        assert!(settled(&mut watch, &roots, vec![(path.clone(), false)]).is_empty());
        // Rewritten in place, as a write of the same size in the same tick
        // of the file system's clock leaves the stamp.
        std::fs::write(&path, "SAME.TXT").unwrap();
        changed_at(&path, ahead);
        assert_eq!(settled(&mut watch, &roots, vec![(path.clone(), false)]), ["M", &at(&path)]);
        assert!(settled(&mut watch, &roots, vec![(path.clone(), false)]).is_empty());
        assert_eq!(settled(&mut watch, &roots, vec![(big.clone(), false)]), ["M", &at(&big)]);
        // Stamps of ten seconds ago: changed, and then not racy.
        let behind = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        changed_at(&path, behind);
        changed_at(&big, behind);
        let both = vec![(path.clone(), false), (big.clone(), false)];
        assert_eq!(settled(&mut watch, &roots, both.clone()), ["M", &at(&big), "M", &at(&path)]);
        assert!(settled(&mut watch, &roots, both).is_empty());
        assert!(watch.racy.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The changes that come one after another are one batch; one that
    /// comes after the batch is the next one's.
    #[test]
    fn changes_that_come_together_are_one_batch() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        let (send, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        for name in ["a", "b", "c"] {
            send.send(PathBuf::from(name)).unwrap();
        }
        let settle = std::time::Duration::from_millis(10);
        let first = runtime.block_on(batch(&mut receiver, settle)).unwrap();
        assert_eq!(first, [PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]);
        send.send(PathBuf::from("d")).unwrap();
        assert_eq!(runtime.block_on(batch(&mut receiver, settle)).unwrap(), [PathBuf::from("d")]);
        drop(send);
        assert_eq!(runtime.block_on(batch(&mut receiver, settle)), None);
    }
}
