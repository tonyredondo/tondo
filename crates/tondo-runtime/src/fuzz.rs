//! `testing.fuzz(name, seeds, body)` (testing 10.6). The standard library's
//! loop (`std/fuzz.to`) gives the runtime each seed as JSON, then asks it
//! for the inputs to run: the seeds, in order, then the inputs of the fuzz
//! store that `tondo test` gives the worker in `TONDO_TEST_FUZZ`, by name.
//! Each call ends with a `fuzz` frame to `tondo test`: its name, how many
//! inputs it ran, and the one it failed with, if any; a panic in the body
//! sends it first.

use crate::Str;
use std::collections::HashMap;
use std::sync::Mutex;

/// Where an input comes from: a seed, by index, or a file of the store,
/// from the package's directory.
enum Origin {
    Seed(usize),
    File(String),
}

struct Call {
    name: String,
    seeds: Vec<String>,
    stored: Vec<(String, String)>,
    /// The inputs given so far; the last is the one that runs.
    given: usize,
    current: Option<(String, Origin)>,
}

/// The fuzz store, by test and directory of the call: each input's file
/// and JSON, in the order of their names.
type Store = HashMap<(String, String), Vec<(String, String)>>;

struct State {
    call: Option<Call>,
    /// The calls of each test that ran here, by node and name.
    named: Vec<(u32, String)>,
    store: Option<Store>,
}

static STATE: Mutex<State> = Mutex::new(State { call: None, named: Vec::new(), store: None });

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Panics as the call of `testing.fuzz` that runs, which the runtime
/// reports at the program's call.
fn fail(code: u32, message: String) -> ! {
    let message = crate::str_new(message.as_bytes());
    let location = crate::str_new(b"<std>/fuzz.to");
    unsafe { crate::tondo_rt_panic(code, message, location) }
}

/// The directory of a call under its test's: its name, with what is not a
/// letter, a digit, `.`, `_` or `-` as `_` (testing 15.14).
pub(crate) fn directory(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect()
}

/// The fuzz store `tondo test` wrote for the worker: records of four
/// lengths on a line, then the test's id, the call's directory, the file
/// and the JSON.
fn read_store() -> Store {
    let bytes = std::env::var_os("TONDO_TEST_FUZZ").and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
    let mut store = Store::new();
    let mut rest = &bytes[..];
    while let Some(end) = rest.iter().position(|&b| b == b'\n') {
        let lengths: Vec<usize> =
            String::from_utf8_lossy(&rest[..end]).split(' ').filter_map(|n| n.parse().ok()).collect();
        let [id, dir, file, json] = lengths[..] else { break };
        let body = &rest[end + 1..];
        if body.len() < id + dir + file + json {
            break;
        }
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let (key, entry) = (
            (text(&body[..id]), text(&body[id..id + dir])),
            (text(&body[id + dir..id + dir + file]), text(&body[id + dir + file..id + dir + file + json])),
        );
        store.entry(key).or_default().push(entry);
        rest = &body[id + dir + file + json..];
    }
    for inputs in store.values_mut() {
        inputs.sort();
    }
    store
}

/// Starts the call `name` of the test that runs: a name that is empty, has
/// a control character or that the test already used is `P2010`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fuzz_begin(name: *const Str) {
    let name = String::from_utf8_lossy(unsafe { crate::str_bytes(name) }).into_owned();
    if name.is_empty() || name.chars().any(char::is_control) {
        fail(2010, format!("{name:?} cannot name a call of `testing.fuzz`: it is empty or has a control character"));
    }
    let node = crate::testing::current();
    let id = crate::testing::id(node);
    let mut state = state();
    if state.named.iter().any(|(n, named)| *n == node && *named == name) {
        drop(state);
        fail(2010, format!("this test already calls `testing.fuzz` with the name {name:?}: give each its own name"));
    }
    state.named.push((node, name.clone()));
    let store = state.store.get_or_insert_with(read_store);
    let stored = store.get(&(id, directory(&name))).cloned().unwrap_or_default();
    state.call = Some(Call { name, seeds: Vec::new(), stored, given: 0, current: None });
}

/// A seed, as the JSON `json.encode` wrote for it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fuzz_seed(json: *const Str) {
    let json = String::from_utf8_lossy(unsafe { crate::str_bytes(json) }).into_owned();
    if let Some(call) = state().call.as_mut() {
        call.seeds.push(json);
    }
}

/// Moves to the next input; false when there is none.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_fuzz_next() -> u8 {
    let mut state = state();
    let Some(call) = state.call.as_mut() else { return 0 };
    let index = call.given;
    call.current = if index < call.seeds.len() {
        Some((call.seeds[index].clone(), Origin::Seed(index)))
    } else {
        let (file, json) = match call.stored.get(index - call.seeds.len()) {
            Some(entry) => entry.clone(),
            None => return 0,
        };
        Some((json, Origin::File(file)))
    };
    call.given += 1;
    1
}

/// The input that runs, as JSON.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_fuzz_input() -> *mut Str {
    let json = state().call.as_ref().and_then(|c| c.current.as_ref().map(|(json, _)| json.clone()));
    crate::str_new(json.unwrap_or_default().as_bytes())
}

/// The input that runs does not read as the call's type: `P2011`, naming
/// where it is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_fuzz_invalid(message: *const Str) {
    let message = String::from_utf8_lossy(unsafe { crate::str_bytes(message) }).into_owned();
    let (name, origin) = {
        let state = state();
        let Some(call) = state.call.as_ref() else { return };
        let origin = match call.current.as_ref().map(|(_, origin)| origin) {
            Some(Origin::File(file)) => format!("{file} does not read as the input of `{}`", call.name),
            Some(Origin::Seed(index)) => format!("seed {index} of `{}` does not read back as its type", call.name),
            None => return,
        };
        (call.name.clone(), origin)
    };
    let fix = match origin.starts_with("seed") {
        true => "its `Encode` and `Decode` do not agree".to_string(),
        false => format!("fix the file to be an input of `{name}`, or delete it"),
    };
    fail(2011, format!("{origin}: {message}; {fix}"));
}

/// The body failed with the input that runs: the call ends with it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_fuzz_failed() {
    let call = state().call.take();
    if let Some(call) = call {
        report(&call, true);
    }
}

/// The call ran every input.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_fuzz_end() {
    let call = state().call.take();
    if let Some(call) = call {
        report(&call, false);
    }
}

/// A panic ends the worker in the call that runs: it ends with the input
/// that ran.
pub(crate) fn interrupted() {
    let call = state().call.take();
    if let Some(call) = call {
        report(&call, true);
    }
}

/// Tells `tondo test` what a call did: its name, the inputs it ran, and
/// the one it failed with, as JSON, file and seed, separated by NUL.
fn report(call: &Call, failed: bool) {
    let (json, file, seed) = match call.current.as_ref().filter(|_| failed) {
        Some((json, Origin::File(file))) => (json.as_str(), file.as_str(), String::new()),
        Some((json, Origin::Seed(index))) => (json.as_str(), "", index.to_string()),
        None => ("", "", String::new()),
    };
    let payload = format!("{}\0{}\0{}\0{file}\0{seed}\0{}", call.name, call.given, json, failed as u8);
    crate::testing::report("fuzz", payload.as_bytes());
}
