//! OS-owned process containment for a compiled test participation.
//!
//! The coordinator admits a worker before sending its sealed input and closes
//! the whole containment group before draining inherited pipes or publishing.
//! A delegated root is explicit transport configuration, never report identity.

use std::io;
use std::path::Path;

pub const PROCESS_ISOLATION_MODEL: &str = "linux-cgroup-v2-kill-5000ms/1";

pub use platform::{ProcessGroup, ProcessRoot, validate_worker};

pub fn prepare(root: Option<&Path>, required: bool) -> io::Result<Option<ProcessRoot>> {
    if !required {
        return Ok(None);
    }
    let root = root.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "test target with `process` requires OS process isolation; provide --process-cgroup <delegated-root>",
        )
    })?;
    let provider = ProcessRoot::open(root)?;
    // Establish writable kernel controls before dispatching any participation.
    // This group contains no process and is never reused for a worker.
    provider.create()?.close()?;
    Ok(Some(provider))
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read, Seek, Write};
    use std::os::unix::fs::MetadataExt;
    use std::path::{Component, Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use nix::sys::statfs::{CGROUP2_SUPER_MAGIC, fstatfs};

    static NEXT_GROUP: AtomicU64 = AtomicU64::new(1);
    const CLEANUP_LIMIT: Duration = Duration::from_millis(5_000);

    #[derive(Clone)]
    pub struct ProcessRoot {
        path: PathBuf,
        directory: Arc<File>,
    }

    impl ProcessRoot {
        pub fn open(path: &Path) -> io::Result<Self> {
            Ok(Self {
                directory: Arc::new(open_directory(path)?),
                path: path.to_owned(),
            })
        }

        pub fn create(&self) -> io::Result<ProcessGroup> {
            verify_identity(&self.path, &self.directory)?;
            for _ in 0..64 {
                let nonce = NEXT_GROUP.fetch_add(1, Ordering::Relaxed);
                let path = self
                    .path
                    .join(format!("tondo-worker-{}-{nonce}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
                let opened = (|| {
                    let directory = open_directory(&path)?;
                    // Workers need no child cgroups. Keep the cleanup tree
                    // finite without requiring traversal or recursive removal.
                    fs::write(path.join("cgroup.max.descendants"), b"0")?;
                    Ok(ProcessGroup {
                        directory,
                        procs: OpenOptions::new()
                            .write(true)
                            .open(path.join("cgroup.procs"))?,
                        kill: OpenOptions::new()
                            .write(true)
                            .open(path.join("cgroup.kill"))?,
                        events: File::open(path.join("cgroup.events"))?,
                        path: path.clone(),
                        close_attempted: false,
                        closed: false,
                    })
                })();
                if opened.is_err() {
                    // No worker can have entered this freshly created group.
                    fs::remove_dir(&path).map_err(|error| {
                        io::Error::other(format!(
                            "cannot remove unused process group `{}`: {error}",
                            path.display()
                        ))
                    })?;
                }
                return opened;
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "process group nonce attempts exhausted",
            ))
        }
    }

    pub struct ProcessGroup {
        path: PathBuf,
        directory: File,
        procs: File,
        kill: File,
        events: File,
        close_attempted: bool,
        closed: bool,
    }

    impl ProcessGroup {
        pub fn path(&self) -> &Path {
            &self.path
        }

        /// Moving the worker's thread group precedes delivery of any bytecode.
        /// Every process it subsequently creates inherits this kernel group.
        pub fn admit(&mut self, pid: u32) -> io::Result<()> {
            if pid == 0 || self.close_attempted {
                return Err(io::Error::other("invalid process group admission"));
            }
            self.procs.rewind()?;
            self.procs.write_all(pid.to_string().as_bytes())
        }

        pub fn close(&mut self) -> io::Result<()> {
            if self.closed {
                return Ok(());
            }
            if self.close_attempted {
                return Err(io::Error::other(format!(
                    "failed process cleanup retains group `{}`",
                    self.path.display()
                )));
            }
            self.close_attempted = true;
            self.kill.rewind()?;
            self.kill.write_all(b"1")?;
            let deadline = Instant::now() + CLEANUP_LIMIT;
            loop {
                let events = read_control(&mut self.events)?;
                if events.lines().any(|line| line == "populated 0") {
                    verify_identity(&self.path, &self.directory)?;
                    fs::remove_dir(&self.path)?;
                    self.closed = true;
                    return Ok(());
                }
                if !events.lines().any(|line| line == "populated 1") {
                    return Err(io::Error::other(
                        "process group has invalid population state",
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "process group `{}` did not become empty",
                            self.path.display()
                        ),
                    ));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    impl Drop for ProcessGroup {
        fn drop(&mut self) {
            // Normal paths observe failures explicitly. Unwinding still owns
            // only this fresh group, never the delegated parent or a sibling.
            if !self.close_attempted {
                let _ = self.close();
            }
        }
    }

    pub fn validate_worker(path: &Path) -> io::Result<()> {
        let directory = open_directory(path)?;
        let mut procs = File::open(path.join("cgroup.procs"))?;
        let membership = read_control(&mut procs)?;
        let pid = std::process::id().to_string();
        if !membership.lines().any(|line| line == pid) {
            return Err(io::Error::other(
                "worker was not admitted to its process group",
            ));
        }
        verify_identity(path, &directory)
    }

    fn open_directory(path: &Path) -> io::Result<File> {
        if !path.is_absolute() {
            return Err(io::Error::other("process cgroup must be an absolute path"));
        }
        let mut prefix = PathBuf::new();
        for component in path.components() {
            if !matches!(component, Component::RootDir | Component::Normal(_)) {
                return Err(io::Error::other("process cgroup path must be canonical"));
            }
            prefix.push(component);
            if !fs::symlink_metadata(&prefix)?.file_type().is_dir() {
                return Err(io::Error::other("process cgroup cannot contain symlinks"));
            }
        }
        let directory = File::open(path)?;
        if fstatfs(&directory)
            .map_err(io::Error::from)?
            .filesystem_type()
            != CGROUP2_SUPER_MAGIC
        {
            return Err(io::Error::other(
                "process isolation requires a cgroup-v2 filesystem",
            ));
        }
        let mut kind = File::open(path.join("cgroup.type"))?;
        if read_control(&mut kind)?.trim_end() != "domain" {
            return Err(io::Error::other(
                "process isolation requires a domain cgroup",
            ));
        }
        Ok(directory)
    }

    fn verify_identity(path: &Path, directory: &File) -> io::Result<()> {
        let current = fs::symlink_metadata(path)?;
        let opened = directory.metadata()?;
        if !current.is_dir() || current.dev() != opened.dev() || current.ino() != opened.ino() {
            return Err(io::Error::other("process cgroup identity changed"));
        }
        Ok(())
    }

    fn read_control(file: &mut File) -> io::Result<String> {
        file.rewind()?;
        let mut text = String::new();
        file.take(4097).read_to_string(&mut text)?;
        if text.len() > 4096 {
            return Err(io::Error::other(
                "process cgroup control exceeds its byte limit",
            ));
        }
        Ok(text)
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use std::io;
    use std::path::Path;

    #[derive(Clone)]
    pub struct ProcessRoot;
    pub struct ProcessGroup;

    fn unsupported() -> io::Error {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "no OS process isolation provider is implemented for this host",
        )
    }

    impl ProcessRoot {
        pub fn open(_: &Path) -> io::Result<Self> {
            Err(unsupported())
        }
        pub fn create(&self) -> io::Result<ProcessGroup> {
            Err(unsupported())
        }
    }

    impl ProcessGroup {
        pub fn path(&self) -> &Path {
            unreachable!("unsupported provider cannot create groups")
        }
        pub fn admit(&mut self, _: u32) -> io::Result<()> {
            Err(unsupported())
        }
        pub fn close(&mut self) -> io::Result<()> {
            Err(unsupported())
        }
    }

    pub fn validate_worker(_: &Path) -> io::Result<()> {
        Err(unsupported())
    }
}
