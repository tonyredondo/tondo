//! What the scheduler promises, checked as a program runs
//! (`TONDO_CHECK_SCHEDULER`, language 16.14): a program that hands work
//! between tasks, sleeps on timers and wakes a task from a computation
//! breaks none of its rules, whatever the number of workers, and so never
//! ends with `P0020`. That a rule broken ends a program with `P0020` and the
//! scheduler's state is the runtime's own test (tasks/checker.rs), as only a
//! broken runtime can show it. With the checker's stops (`TONDO_RT_STALLS`),
//! a job of the host that waited for a process and a deadline race for the
//! task's wait in both orders, and so do the job's reaping of the stages a
//! deadline ended and the task's look at it, and none leaves anything
//! behind.

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

/// Rounds of a started process's output under a deadline its job beats,
/// but for a stop of the seed's between the job's end and its claim of the
/// task's wait (`stalls::Point::Give`): the deadline then ends the wait,
/// the call gives back nothing and the job drops what it read. Then rounds
/// of a deadline that ends a stage: the task waits until the job reaped
/// it, which a stop between the kill and its look (`stalls::Point::Kill`)
/// lets happen first, and comes back at once in either order.
const RACE: &str = "import std.console
import std.process
import std.time

fn rounds(): String {
    var gave = 0
    var dropped = 0
    for _ in 0..12 {
        match process.command(\"printf\", \"x\").stdout(process.Output.Capture).start() {
            ok(running) => {
                // It has ended: its job's wait is short.
                time.sleep(time.milliseconds(30))
                match within time.milliseconds(20) { running.output() } {
                    ok(read) => {
                        let text = read.text() ?? \"not a text\"
                        assert(text == \"x\", text)
                        gave += 1
                    }
                    err(_) => {
                        dropped += 1
                    }
                }
            }
            err(error) => panic(\"{error}\")
        }
    }
    var ended = 0
    for _ in 0..12 {
        let since = time.now()
        let outcome = within time.milliseconds(20) { process.command(\"sleep\", \"5\").output() }
        if not outcome.isOk() and since.elapsed().seconds() < 2 {
            ended += 1
        }
    }
    \"{gave} {dropped} {ended}\"
}

fn main() {
    // In a task, which starts the workers, and the checker with its stops.
    let job = spawn rounds()
    console.println(job.wait())
}
";

#[test]
fn a_host_job_and_a_deadline_race_in_both_orders_and_leave_nothing() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("scheduler-race");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("race.to"), RACE).unwrap();
    let tondo = env!("CARGO_BIN_EXE_tondo");
    let built = Command::new(tondo).args(["build", "race.to", "--output", "race"]).current_dir(&dir).output().unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let (mut gave, mut dropped) = (0, 0);
    for seed in 1..=8 {
        let run = Command::new(dir.join("race"))
            .env("TONDO_CHECK_SCHEDULER", "1")
            .env("TONDO_RT_STALLS", seed.to_string())
            .env("TONDO_RT_CHECK_LEAKS", "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert!(run.status.success(), "with the seed {seed}: {stderr}");
        let stdout = String::from_utf8_lossy(&run.stdout);
        let counts: Vec<u32> = stdout.split_whitespace().map(|count| count.parse().unwrap()).collect();
        assert_eq!(counts.len(), 3, "with the seed {seed}: {stdout}");
        assert_eq!(counts[0] + counts[1], 12, "with the seed {seed}: {stdout}");
        assert_eq!(counts[2], 12, "with the seed {seed}, a round that ended a stage was late: {stdout}");
        gave += counts[0];
        dropped += counts[1];
    }
    assert!(gave > 0, "no job claimed the wait before the deadline, over eight seeds");
    assert!(dropped > 0, "no deadline ended the wait before the job claimed it, over eight seeds");
}
