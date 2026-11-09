//! `TONDO_COMPILE_TIMES`, internal: how long each part of a build takes,
//! on stderr, a line each as it ends, for the benchmarks of bench/compile
//! (docs/speed.md, A1). A part that runs once per analysis, as `tondo check`
//! makes one for each system and one for the tests, says so as often.

use std::sync::OnceLock;
use std::time::Instant;

/// A part of a build, timed until it is dropped.
pub struct Part {
    name: &'static str,
    started: Option<Instant>,
}

/// Starts timing `name`, when the switch is on.
pub fn part(name: &'static str) -> Part {
    static ON: OnceLock<bool> = OnceLock::new();
    let on = *ON.get_or_init(|| std::env::var_os("TONDO_COMPILE_TIMES").is_some());
    Part { name, started: on.then(Instant::now) }
}

impl Drop for Part {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            eprintln!("tondo: {} took {:.2} ms", self.name, started.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
