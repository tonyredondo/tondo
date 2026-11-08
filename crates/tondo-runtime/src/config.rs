//! What the environment tells the runtime (docs/refactor.md, phase 3): each
//! variable it reads, read once as the program starts, before anything else
//! runs. The table of the toolchain's specification (7.17) lists them, from
//! the comments here, which a test holds together. Those a person sets come
//! first; then those `tondo` sets for the programs it runs, which speak
//! between the two. The runtime takes some of them out of the environment
//! as it starts, where it reads them now, for its children not to inherit
//! them; `TZ`, `HOME` and `XDG_CACHE_HOME` are the system's, and mean what
//! they mean to every program.

use std::ffi::OsString;
use std::sync::OnceLock;

pub(crate) struct Config {
    /// `TONDO_DIAGNOSTICS`: `1` listens for the tools on a socket and counts
    /// for them; `0` does not, even in a live build.
    pub(crate) diagnostics: Option<OsString>,
    /// `TONDO_DUMP`: the directory a dump goes to, and with it a dump at
    /// `kill -QUIT` and the events a dump keeps; `off` or `0`, no dump of a
    /// crash; unset, a crash's dump in the user's cache.
    pub(crate) dump: Option<OsString>,
    /// `TONDO_DUMP_WHEN`: with `TONDO_DUMP`, when the program dumps itself
    /// and goes on, as `every=5m` or `memory=90%`.
    pub(crate) dump_when: Option<String>,
    /// `TONDO_DUMP_VALUES`: `off`, or anything but `on`, leaves the values
    /// out of a dump.
    pub(crate) dump_values: Option<OsString>,
    /// `TONDO_DUMP_COMMAND`: a command to run with each dump written, which
    /// finds it in `TONDO_DUMP_FILE`.
    pub(crate) dump_command: Option<OsString>,
    /// `TONDO_PANIC_FORMAT`: `json` reports a failure as JSON on stderr.
    pub(crate) panic_format: Option<OsString>,
    /// `TONDO_HEAP_RATE`: the bytes between two samples of the heap.
    pub(crate) heap_rate: Option<String>,
    /// `TONDO_LOCK_ORDER`: whether the program looks at the order in which
    /// tasks take locks: `fail` fails where a cycle closes, `off` does not
    /// look, anything else says it on stderr.
    pub(crate) lock_order: Option<String>,
    /// `TONDO_ZONES`: `builtin` uses the time zones built into the program,
    /// not the system's.
    pub(crate) zones: Option<String>,
    /// `TONDO_DEBUG`, set by `tondo debug`, the editor, and a program
    /// debugged for the children it follows: `1` waits for the debugger
    /// before `main`.
    pub(crate) debug: Option<OsString>,
    /// `TONDO_TRACE`, set by `tondo trace`: `1` traces from the start, `cpu`
    /// samples the processor without counting the memory.
    pub(crate) trace: Option<OsString>,
    /// `TONDO_RACE`, set by `tondo test` and `tondo debug` of a test: `1`
    /// looks for lost updates.
    pub(crate) race: Option<OsString>,
    /// `TONDO_LIVE_HOLD`, set by `tondo run --watch` and the tools' sessions:
    /// `1` holds a panic until the code changes.
    pub(crate) live_hold: Option<OsString>,
    /// `TONDO_RT_CHECK_LEAKS`, set by the tests of the toolchain: counts the
    /// objects alive as the program ends.
    pub(crate) check_leaks: Option<OsString>,
    /// `TONDO_RT_RECORD`, set by `tondo run --record` and `tondo test
    /// --record`: the file to record the run to.
    pub(crate) record: Option<OsString>,
    /// `TONDO_RT_RECORD_CHILDREN`: the directory of the recording of a
    /// parent, among which a child records its own.
    pub(crate) record_children: Option<OsString>,
    /// `TONDO_RT_RECORD_LAST`: the milliseconds of the run a recording keeps,
    /// the last ones.
    pub(crate) record_last: Option<String>,
    /// `TONDO_RT_RECORD_HOLD`: how long a recorded run waits for a debugger.
    pub(crate) record_hold: Option<String>,
    /// `TONDO_RT_REPLAY`, set by `tondo debug --replay`: the recording to
    /// replay.
    pub(crate) replay: Option<OsString>,
    /// `TONDO_RT_SHOWN_FROM`: the probe from which a replay shows what the
    /// program writes.
    pub(crate) shown_from: Option<String>,
    /// `TONDO_RT_SCHEDULE`, set by `tondo test --schedules`, `tondo run
    /// --record` and its replay: the seed of the order in which tasks run,
    /// in hexadecimal.
    pub(crate) schedule: Option<String>,
    /// `TONDO_RT_ORDER`, set by `tondo test --schedules all`: the choices of
    /// the run.
    pub(crate) order: Option<String>,
    /// `TONDO_RT_ORDER_OUT`: where the run says what it did, for the next.
    pub(crate) order_out: Option<OsString>,
    /// `TONDO_TEST_PLAN`, set by `tondo test`: the file of the tests to run.
    pub(crate) test_plan: Option<OsString>,
    /// `TONDO_TEST_NONCE`: what marks the lines of the test's report.
    pub(crate) test_nonce: Option<String>,
    /// `TONDO_TEST_SNAPSHOTS`: the file of the snapshots.
    pub(crate) test_snapshots: Option<OsString>,
    /// `TONDO_TEST_UPDATE`: records the snapshots again.
    pub(crate) test_update: Option<OsString>,
    /// `TONDO_TEST_COVERAGE`: the directory the counts of coverage go to.
    pub(crate) test_coverage: Option<OsString>,
    /// `TONDO_TEST_BENCH`: the seconds each benchmark measures.
    pub(crate) test_bench: Option<String>,
    /// `TONDO_TEST_PROFILE`: `cpu:` or `memory:` and the directory of the
    /// tests' profiles.
    pub(crate) test_profile: Option<String>,
    /// `TONDO_TEST_FUZZ`: the file of the inputs the fuzz tests try.
    pub(crate) test_fuzz: Option<OsString>,
    /// `TONDO_FUZZ`, set by `tondo test --fuzz`: the fuzz test to search and
    /// its limits.
    pub(crate) fuzz: Option<String>,
    /// `TONDO_FUZZ_REPLAY`: the inputs to try as a failing one is reduced.
    pub(crate) fuzz_replay: Option<String>,
    /// `TONDO_META_STEPS`, set for a program that runs at build time: its
    /// steps.
    pub(crate) meta_steps: Option<String>,
    /// `TONDO_META_MEMORY`: its bytes.
    #[cfg_attr(not(tondo_staticlib), allow(dead_code))]
    pub(crate) meta_memory: Option<String>,
    /// `TONDO_META_REQUEST`: the file of its request.
    pub(crate) meta_request: Option<OsString>,
    /// `TONDO_META_RESPONSE`: the file of its response.
    pub(crate) meta_response: Option<OsString>,
}

/// The settings, read the first time, as the program starts.
pub(crate) fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let os = |name: &str| std::env::var_os(name);
        let text = |name: &str| std::env::var(name).ok();
        Config {
            diagnostics: os("TONDO_DIAGNOSTICS"),
            dump: os("TONDO_DUMP"),
            dump_when: text("TONDO_DUMP_WHEN"),
            dump_values: os("TONDO_DUMP_VALUES"),
            dump_command: os("TONDO_DUMP_COMMAND"),
            panic_format: os("TONDO_PANIC_FORMAT"),
            heap_rate: text("TONDO_HEAP_RATE"),
            lock_order: text("TONDO_LOCK_ORDER"),
            zones: text("TONDO_ZONES"),
            debug: os("TONDO_DEBUG"),
            trace: os("TONDO_TRACE"),
            race: os("TONDO_RACE"),
            live_hold: os("TONDO_LIVE_HOLD"),
            check_leaks: os("TONDO_RT_CHECK_LEAKS"),
            record: os("TONDO_RT_RECORD"),
            record_children: os("TONDO_RT_RECORD_CHILDREN"),
            record_last: text("TONDO_RT_RECORD_LAST"),
            record_hold: text("TONDO_RT_RECORD_HOLD"),
            replay: os("TONDO_RT_REPLAY"),
            shown_from: text("TONDO_RT_SHOWN_FROM"),
            schedule: text("TONDO_RT_SCHEDULE"),
            order: text("TONDO_RT_ORDER"),
            order_out: os("TONDO_RT_ORDER_OUT"),
            test_plan: os("TONDO_TEST_PLAN"),
            test_nonce: text("TONDO_TEST_NONCE"),
            test_snapshots: os("TONDO_TEST_SNAPSHOTS"),
            test_update: os("TONDO_TEST_UPDATE"),
            test_coverage: os("TONDO_TEST_COVERAGE"),
            test_bench: text("TONDO_TEST_BENCH"),
            test_profile: text("TONDO_TEST_PROFILE"),
            test_fuzz: os("TONDO_TEST_FUZZ"),
            fuzz: text("TONDO_FUZZ"),
            fuzz_replay: text("TONDO_FUZZ_REPLAY"),
            meta_steps: text("TONDO_META_STEPS"),
            meta_memory: text("TONDO_META_MEMORY"),
            meta_request: os("TONDO_META_REQUEST"),
            meta_response: os("TONDO_META_RESPONSE"),
        }
    })
}

/// Whether a variable read as it started was `1`.
pub(crate) fn on(value: &Option<OsString>) -> bool {
    value.as_ref().is_some_and(|value| value == "1")
}
