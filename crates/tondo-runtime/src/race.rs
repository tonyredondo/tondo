//! Updates that tasks lose, under `tondo test` (toolchain 8): safe code has no
//! data races, but a task that reads a cell of `std.sync` and writes it later in
//! the same function, after another task wrote it, may overwrite that change
//! with a value made from the old one. The cost is where the tests take locks;
//! anywhere else it is a load.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Seen {
    /// Each cell: how many times it was written, and the last time.
    cells: HashMap<usize, Cell>,
    /// What each task read last of a cell: the cell's writes then, and where.
    reads: HashMap<(u64, usize), (u64, Vec<usize>)>,
    said: HashSet<String>,
}

#[derive(Default)]
struct Cell {
    writes: u64,
    task: u64,
    at: Vec<usize>,
}

static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

/// How many callers of an operation it keeps, to find the program's code.
const FRAMES: usize = 12;

pub(crate) fn start() {
    ON.store(true, Ordering::Relaxed);
}

#[inline]
pub(crate) fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

fn here() -> Vec<usize> {
    let bounds = super::tasks::stack_bounds().unwrap_or((0, 0));
    super::trace::walk(super::trace::frame_pointer(), bounds).take(FRAMES).collect()
}

/// The code of the program a list of callers comes to, as `file:line in
/// function`.
fn place(frames: &[usize]) -> Option<String> {
    frames.iter().find_map(|&address| Some(super::trace::scopes_at(address).first()?.1.clone()))
}

fn seen<T>(with: impl FnOnce(&mut Seen) -> T) -> T {
    with(SEEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Seen::default))
}

/// The running task read the cell.
pub(crate) fn read(cell: usize) {
    let (task, at) = (super::tasks::current_id(), here());
    seen(|seen| {
        let writes = seen.cells.get(&cell).map_or(0, |cell| cell.writes);
        seen.reads.insert((task, cell), (writes, at));
    });
}

/// The running task replaced the cell's value, or changed it in a way that
/// sees the value it changes, which loses nothing (`checked`), as `add` and
/// `compareExchange` do.
pub(crate) fn wrote(cell: usize, checked: bool) {
    let (task, at) = (super::tasks::current_id(), here());
    let lost = seen(|seen| {
        let read = seen.reads.remove(&(task, cell)).filter(|_| !checked);
        let written = seen.cells.entry(cell).or_default();
        let stale = read.filter(|(writes, _)| *writes != written.writes && written.task != task);
        let lost = stale.map(|(_, read)| (read, written.task, written.at.clone()));
        written.writes += 1;
        written.task = task;
        written.at.clone_from(&at);
        lost
    });
    let Some((read, other, theirs)) = lost else { return };
    let (Some(read), Some(wrote), Some(theirs)) = (place(&read), place(&at), place(&theirs)) else { return };
    // Far apart, the read and the write are not one change of the value.
    if read.rsplit(" in ").next() != wrote.rsplit(" in ").next() {
        return;
    }
    let message = format!(
        "task {task} read a value at {read} and wrote it at {wrote}, and task {other} had written it at {theirs} \
         in between: if the new value comes from the old one, that write is lost. Read and write it in one \
         `update`, or with `compareExchange`"
    );
    let key = format!("{read}\0{wrote}");
    if seen(|seen| seen.said.insert(key)) {
        super::testing::report("race", message.as_bytes());
    }
}

/// A cell goes away, and another may come where it was.
pub(crate) fn forget(cell: usize) {
    seen(|seen| {
        seen.cells.remove(&cell);
        seen.reads.retain(|&(_, read), _| read != cell);
    });
}
