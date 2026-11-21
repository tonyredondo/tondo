//! What the scheduler promises, checked as a program runs
//! (`TONDO_CHECK_SCHEDULER`, language 16.14): a program that hands work
//! between tasks, sleeps on timers and wakes a task from a computation
//! breaks none of its rules, whatever the number of workers, and so never
//! ends with `P0020`. That a rule broken ends a program with `P0020` and the
//! scheduler's state is the runtime's own test (tasks/checker.rs), as only a
//! broken runtime can show it.

use std::path::Path;
use std::process::Command;

const PROGRAM: &str = "import std.console
import std.sync
import std.time

fn echo(from: Receiver[Int], back: Sender[Int], rounds: Int) {
    for _ in 0..rounds {
        back.send(from.receive() ?? 0)
    }
}

/// Two tasks that hand a number to each other, through the next of a worker.
fn pingPong(rounds: Int): Int {
    let (to, from) = channel[Int]()
    let (back, again) = channel[Int]()
    let other = spawn echo(from, back, rounds)
    var total = 0
    for round in 0..rounds {
        to.send(round)
        total += again.receive() ?? 0
    }
    other.wait()
    total
}

fn nap(millis: Int): Int {
    time.sleep(time.milliseconds(millis))
    millis
}

/// Computes until `stop` is set, once it has woken the task that waits.
fn compute(began: Sender[Unit], stop: sync.Atomic[Bool]) {
    began.send(())
    for not stop.load() {}
}

fn main() {
    console.println(\"handed: {pingPong(20000)}\")
    let naps = for millis in 0..40 { spawn nap(millis % 7) }
    console.println(\"slept: {naps.wait().sum()}\")
    let (began, beginning) = channel[Unit]()
    let stop = sync.Atomic(false)
    let job = spawn compute(began, stop)
    _ = beginning.receive()
    stop.store(true)
    job.wait()
    console.println(\"woken beside a computation\")
}
";

#[test]
fn a_program_breaks_none_of_the_schedulers_rules() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("scheduler-rules");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rules.to"), PROGRAM).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built =
        Command::new(tondo).args(["build", "rules.to", "--output", "rules"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let expected = "handed: 199990000\nslept: 115\nwoken beside a computation\n";
    for workers in ["1", "2", "4"] {
        let run = Command::new(dir.join("rules"))
            .env("TONDO_CHECK_SCHEDULER", "1")
            .env("TONDO_WORKERS", workers)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(!stderr.contains("P0020"), "with {workers} workers a rule broke:\n{stderr}");
        assert!(run.status.success(), "with {workers} workers: {stderr}");
        assert_eq!(String::from_utf8_lossy(&run.stdout), expected, "with {workers} workers");
    }
}
