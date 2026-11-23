//! What the tests share. `cargo test -p tondo --test waits` holds them to
//! waiting through `until` rather than sleeping.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The cache every program the tests run keeps its files in: kept builds,
/// the runtime's archives, the compilers `--compiler` builds and a crash's
/// dump. One for every test binary of the checkout, under the target's
/// scratch, never the user's own, where the newest dumps alone are kept and
/// a test's would prune theirs.
pub fn cache() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("tondo-cache")
}

/// A command the tests run, `tondo`, a program they built or a tool of the
/// system, that keeps its files in `cache()`: every one goes through here
/// (`the_tests_start_programs_only_through_common`). One that sets its own
/// `XDG_CACHE_HOME` after this keeps doing so.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command.env("XDG_CACHE_HOME", cache());
    command
}

/// How long `until` asks before it gives up: it only bounds a hang, as a run
/// that passes never comes near it.
const CAP: Duration = Duration::from_secs(120);

/// Asks `check` again and again until it gives a value, for what only
/// asking can see, as a port that starts to listen or a file a process
/// writes; `what` names it when it never comes.
pub fn until<T>(what: &str, check: impl FnMut() -> Option<T>) -> T {
    until_showing(what, check, String::new)
}

/// As `until`, and with what `shown` gives then, as a screen, when it
/// never comes.
pub fn until_showing<T>(what: &str, mut check: impl FnMut() -> Option<T>, shown: impl Fn() -> String) -> T {
    let started = Instant::now();
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(started.elapsed() < CAP, "{what} never came in {} s: {}", CAP.as_secs(), shown());
        // The pace of asking, not a wait for something to happen.
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The output of `child` once it ends, which a thread of its own waits for;
/// the cap only bounds a hang, which it ends.
pub fn ended(child: std::process::Child, what: &str) -> std::process::Output {
    let pid = child.id();
    let (done, output) = std::sync::mpsc::channel();
    std::thread::spawn(move || done.send(child.wait_with_output()));
    match output.recv_timeout(CAP) {
        Ok(output) => output.unwrap(),
        Err(_) => {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            panic!("{what} never ended in {} s", CAP.as_secs())
        }
    }
}

/// A program a test started that runs until the test ends it. Dropped, as
/// when an assertion fails before the end, it ends the program too, which
/// would otherwise outlive the test.
pub struct Started(Option<std::process::Child>);

impl Started {
    pub fn new(child: std::process::Child) -> Started {
        Started(Some(child))
    }

    /// The program, for the test to end itself.
    pub fn take(mut self) -> std::process::Child {
        self.0.take().expect("a started program")
    }
}

impl std::ops::Deref for Started {
    type Target = std::process::Child;

    fn deref(&self) -> &std::process::Child {
        self.0.as_ref().expect("a started program")
    }
}

impl std::ops::DerefMut for Started {
    fn deref_mut(&mut self) -> &mut std::process::Child {
        self.0.as_mut().expect("a started program")
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Makes `command`, and what it starts, run as on a system that refuses
/// `perf_event_open`, as the filter of system calls of a container does on
/// a runner: the side of a test where nothing counts the processor's
/// instructions, run on a machine that counts them too. Linux only, where a
/// program can count itself; elsewhere nothing counts, and the command is
/// left as it is.
///
/// It is a switch for a test, not a sandbox: the filter compares the call's
/// number for the architecture the test is built for, refuses that one call
/// and lets every other through. Where the filter cannot be set, as under an
/// emulator that refuses or ignores `PR_SET_SECCOMP`, the command runs as it
/// is, and the test's assertions decide: on a machine that counts, those of
/// the side that does not then fail, loudly; where nothing counts, they hold
/// anyway. So a filter that did not take is not an error here.
pub fn uncounted(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(target_os = "linux")]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            // Load the call's number; if it is `perf_event_open`'s, refuse
            // it as a system that does not allow it does; else let it be.
            let filter = [
                libc::sock_filter { code: 0x20, jt: 0, jf: 0, k: 0 },
                libc::sock_filter { code: 0x15, jt: 0, jf: 1, k: libc::SYS_perf_event_open as u32 },
                libc::sock_filter { code: 0x06, jt: 0, jf: 0, k: 0x0005_0000 | libc::EPERM as u32 },
                libc::sock_filter { code: 0x06, jt: 0, jf: 0, k: 0x7fff_0000 },
            ];
            let program = libc::sock_fprog { len: filter.len() as u16, filter: filter.as_ptr().cast_mut() };
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0 {
                libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program as *const libc::sock_fprog);
            }
            Ok(())
        });
    }
    command
}
