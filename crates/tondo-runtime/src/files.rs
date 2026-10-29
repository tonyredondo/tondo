//! The rest of `std.path` and `std.fs`, which `std/paths.to` and
//! `std/files.to` are written on (standard library 10.4, 10.5): paths made
//! clean or relative, glob patterns, walks, temporary entries, permissions,
//! links and watching. Each operation of the filesystem runs on a thread of
//! its own while the task waits, and gives a `Done`: its texts and numbers,
//! or the code of its `FsError`.

use super::native::{self, NativeObject};
use super::{ArrayHeader, Str, str_bytes, str_new};
use crate::http::{string_array, strings};
use std::collections::{BTreeMap, HashSet};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
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

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// The shortest path that names the same lexically, as Go's `path.Clean`.
pub(crate) fn clean(path: &str) -> String {
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

/// The path from `from` to `to`, as Go's `filepath.Rel`.
fn relative(from: &str, to: &str) -> Option<String> {
    let (from, to) = (clean(from), clean(to));
    if from == to {
        return Some(".".into());
    }
    if from.starts_with('/') != to.starts_with('/') {
        return None;
    }
    let parts = |path: &str| -> Vec<String> {
        path.split('/').filter(|p| !p.is_empty() && *p != ".").map(String::from).collect()
    };
    let (from, to) = (parts(&from), parts(&to));
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if from[common..].iter().any(|p| p == "..") {
        return None;
    }
    let mut out: Vec<&str> = vec![".."; from.len() - common];
    out.extend(to[common..].iter().map(String::as_str));
    Some(if out.is_empty() { ".".into() } else { out.join("/") })
}

/// `path.clean`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_path_clean(path: *const Str) -> *mut Str {
    str_new(clean(text(path)).as_bytes())
}

/// `path.relative`, or an empty text for `none`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_path_relative(from: *const Str, to: *const Str) -> *mut Str {
    str_new(relative(text(from), text(to)).unwrap_or_default().as_bytes())
}

/// `path.matches`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_path_matches(pattern: *const Str, path: *const Str) -> u8 {
    Glob::new(text(pattern)).matches(text(path)) as u8
}

// ---------------------------------------------------------------------------
// Glob patterns
// ---------------------------------------------------------------------------

/// A glob pattern: each alternative of its braces, by components.
struct Glob {
    alternatives: Vec<Vec<String>>,
}

impl Glob {
    fn new(pattern: &str) -> Self {
        let alternatives = expand(pattern).iter().map(|p| p.split('/').map(String::from).collect()).collect();
        Glob { alternatives }
    }

    fn matches(&self, path: &str) -> bool {
        let path: Vec<&str> = path.split('/').collect();
        self.alternatives.iter().any(|pattern| components(pattern, &path))
    }

    /// The directory to walk for each alternative, from its components
    /// before the first with a wildcard, and the components after them.
    fn bases(&self) -> Vec<(String, &[String])> {
        self.alternatives
            .iter()
            .map(|pattern| {
                let literal = pattern.iter().take_while(|p| !wild(p)).count();
                (unescape(&pattern[..literal].join("/")), &pattern[literal..])
            })
            .collect()
    }
}

fn wild(component: &str) -> bool {
    component.contains(['*', '?', '[', '{', '\\'])
}

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' { chars.next().unwrap_or('\\') } else { c });
    }
    out
}

/// The patterns a pattern's braces stand for: `a{b,c}` is `ab` and `ac`. A
/// brace without its closing is itself.
fn expand(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            '{' => {
                let (mut depth, mut end, mut commas) = (0, None, Vec::new());
                let mut at = index;
                while at < chars.len() {
                    match chars[at] {
                        '\\' => at += 1,
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(at);
                                break;
                            }
                        }
                        ',' if depth == 1 => commas.push(at),
                        _ => {}
                    }
                    at += 1;
                }
                let Some(end) = end else {
                    index += 1;
                    continue;
                };
                let head: String = chars[..index].iter().collect();
                let tail: String = chars[end + 1..].iter().collect();
                let mut bounds = vec![index];
                bounds.extend(&commas);
                bounds.push(end);
                let mut out = Vec::new();
                for pair in bounds.windows(2) {
                    let choice: String = chars[pair[0] + 1..pair[1]].iter().collect();
                    out.extend(expand(&format!("{head}{choice}{tail}")));
                }
                return out;
            }
            _ => index += 1,
        }
    }
    vec![pattern.to_string()]
}

/// Whether the components of a path match those of a pattern, where `**`
/// matches any number of them.
fn components(pattern: &[String], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((first, rest)) if first == "**" => (0..=path.len()).any(|skip| components(rest, &path[skip..])),
        Some((first, rest)) => {
            path.split_first().is_some_and(|(name, others)| component(first, name) && components(rest, others))
        }
    }
}

/// Whether a name matches one component of a pattern.
fn component(pattern: &str, name: &str) -> bool {
    let (pattern, name): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    fn at(pattern: &[char], name: &[char]) -> bool {
        match pattern.first() {
            None => name.is_empty(),
            Some('*') => (0..=name.len()).any(|skip| at(&pattern[1..], &name[skip..])),
            Some('?') => !name.is_empty() && at(&pattern[1..], &name[1..]),
            Some('[') => match class(pattern) {
                Some((set, negated, used)) => {
                    name.first().is_some_and(|&c| set(c) != negated) && at(&pattern[used..], &name[1..])
                }
                None => name.first() == Some(&'[') && at(&pattern[1..], &name[1..]),
            },
            Some('\\') if pattern.len() > 1 => name.first() == Some(&pattern[1]) && at(&pattern[2..], &name[1..]),
            Some(&c) => name.first() == Some(&c) && at(&pattern[1..], &name[1..]),
        }
    }
    at(&pattern, &name)
}

type Set = Box<dyn Fn(char) -> bool>;

/// A `[...]` at the start of `pattern`: what it matches, whether it is
/// negated and how many characters it takes; `None` without its `]`.
fn class(pattern: &[char]) -> Option<(Set, bool, usize)> {
    let mut at = 1;
    let negated = matches!(pattern.get(at), Some('!' | '^'));
    at += negated as usize;
    let mut ranges: Vec<(char, char)> = Vec::new();
    let mut first = true;
    loop {
        let mut c = *pattern.get(at)?;
        if c == ']' && !first {
            break;
        }
        if c == '\\' {
            at += 1;
            c = *pattern.get(at)?;
        }
        first = false;
        at += 1;
        if pattern.get(at) == Some(&'-') && pattern.get(at + 1).is_some_and(|&n| n != ']') {
            let mut high = pattern[at + 1];
            at += 2;
            if high == '\\' {
                high = *pattern.get(at)?;
                at += 1;
            }
            ranges.push((c, high));
        } else {
            ranges.push((c, c));
        }
    }
    let set: Set = Box::new(move |c| c != '/' && ranges.iter().any(|&(low, high)| low <= c && c <= high));
    Some((set, negated, at + 1))
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
/// nanoseconds of the last change since 1970 and the permissions.
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
        Ok(Done { numbers: vec![kind(metadata.file_type()), size, readonly, seconds, nanos, mode], ..Done::default() })
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

/// `fs.walk`: the paths and the kinds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_walk(root: *const Str) -> *mut NativeObject {
    let root = text(root).to_string();
    done(move || {
        let mut entries = Vec::new();
        walk(&root, None, &mut entries)?;
        let (texts, numbers) = entries.into_iter().map(|(path, kind)| (below(&root, &path), kind)).unzip();
        Ok(Done { texts, numbers, ..Done::default() })
    })
}

/// `fs.glob`: the paths that match, sorted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_files_glob(pattern: *const Str) -> *mut NativeObject {
    let pattern = text(pattern).to_string();
    done(move || {
        let glob = Glob::new(&pattern);
        let mut found = Vec::new();
        for (base, rest) in glob.bases() {
            if rest.is_empty() {
                if !base.is_empty() && std::fs::symlink_metadata(&base).is_ok() {
                    found.push(base);
                }
                continue;
            }
            // `/*` walks the root; only as deep as the pattern goes, unless
            // it has `**`.
            let directory = if base.is_empty() && glob.alternatives.iter().any(|a| a.len() > rest.len()) {
                "/".to_string()
            } else {
                base
            };
            let depth = (!rest.iter().any(|p| p == "**")).then_some(rest.len());
            let mut entries = Vec::new();
            match walk(&directory, depth, &mut entries) {
                Ok(()) => {}
                // No directory there: nothing matches.
                Err(1 | 4) => continue,
                Err(code) => return Err(code),
            }
            for (path, _) in entries {
                if components(rest, &path.split('/').collect::<Vec<_>>()) {
                    found.push(if directory.is_empty() { path } else { below(&directory, &path) });
                }
            }
        }
        found.sort();
        found.dedup();
        Ok(texts(found))
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

/// A watch: the system's watcher, the changes it reports, the paths it
/// watches, as given and as the system names them, and the paths that
/// exist under them, which tell a file replaced from one made.
struct Watch {
    watcher: Option<notify::RecommendedWatcher>,
    /// Out while a task waits for the next batch, and after the watch ends.
    changes: Option<tokio::sync::mpsc::UnboundedReceiver<PathBuf>>,
    roots: Vec<(String, PathBuf)>,
    settle: std::time::Duration,
    existing: HashSet<PathBuf>,
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
        let (mut roots, mut existing) = (Vec::new(), HashSet::new());
        for path in paths {
            let real = io(std::fs::canonicalize(&path))?;
            let mut entries = Vec::new();
            if std::fs::metadata(&real).is_ok_and(|m| m.is_dir()) {
                walk(&real.to_string_lossy(), None, &mut entries)?;
            }
            existing.extend(entries.into_iter().map(|(entry, _)| real.join(entry)));
            existing.insert(real.clone());
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
        // Whether each path existed before the batch and after it; a
        // watched directory itself changes with what it holds.
        let mut states: BTreeMap<String, (bool, bool)> = BTreeMap::new();
        for path in changes.into_iter().collect::<HashSet<_>>() {
            let after = std::fs::symlink_metadata(&path);
            if after.as_ref().is_ok_and(|m| m.is_dir()) && roots.iter().any(|(_, real)| *real == path) {
                continue;
            }
            let before = watch.existing.contains(&path);
            if after.is_ok() {
                watch.existing.insert(path.clone());
            } else {
                watch.existing.remove(&path);
            }
            let Some(shown) = shown(&roots, &path) else { continue };
            states.insert(shown, (before, after.is_ok()));
        }
        drop(watch);
        let mut out = Vec::new();
        for (path, state) in states {
            let letter = match state {
                (false, true) => "C",
                (true, true) => "M",
                (true, false) => "R",
                (false, false) => continue,
            };
            out.push(letter.to_string());
            out.push(path);
        }
        if !out.is_empty() {
            return string_array(&out);
        }
    }
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
