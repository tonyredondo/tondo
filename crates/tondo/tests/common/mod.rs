//! What the tests share. `cargo test -p tondo --test waits` holds them to
//! waiting through `until` rather than sleeping.

#![allow(dead_code)]

use std::time::{Duration, Instant};

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
