//! The labels of `profile.withLabels` (standard library 14.26): sets of
//! names and values a task carries, which each sample of where it runs
//! records, so that a profile divides the time by them, as by endpoint. A
//! sample keeps only the number of its set, which a signal handler may
//! read; the sets are kept in a table of fixed size, which a new set takes
//! a slot of in turn, so that labels made for each request, as a span's,
//! do not grow without end. A number is its slot and the generation of
//! that slot, so that a sample whose set has been replaced since has none.

use std::collections::HashMap;
use std::sync::Mutex;

const SLOTS: u32 = 4096;

pub(crate) type Set = Vec<(String, String)>;

struct Table {
    /// Each slot's generation and set.
    slots: Vec<(u32, Set)>,
    /// The slot the next new set takes, from 1: `0` is no labels.
    next: u32,
    /// The number of each set kept, to give a set again its number.
    known: HashMap<Set, u32>,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

/// The number of a set, which a new set takes from the oldest slot.
pub(crate) fn intern(set: Set) -> u32 {
    if set.is_empty() {
        return 0;
    }
    let mut table = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    let table = table.get_or_insert_with(|| Table {
        slots: vec![(0, Vec::new()); SLOTS as usize],
        next: 1,
        known: HashMap::new(),
    });
    if let Some(&number) = table.known.get(&set) {
        return number;
    }
    let slot = table.next;
    table.next = if slot + 1 == SLOTS { 1 } else { slot + 1 };
    let (generation, old) = &mut table.slots[slot as usize];
    let old = std::mem::replace(old, set.clone());
    table.known.remove(&old);
    *generation = generation.wrapping_add(1) & 0xf_ffff;
    let number = slot | *generation << 12;
    table.known.insert(set, number);
    number
}

/// The set a number names, unless its slot holds another now.
pub(crate) fn get(number: u32) -> Option<Set> {
    if number == 0 {
        return None;
    }
    let table = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    let (generation, set) = table.as_ref()?.slots.get((number & (SLOTS - 1)) as usize)?;
    (*generation == number >> 12).then(|| set.clone())
}

/// `profile.withLabels`, as it begins: the running task's labels and
/// these, which replace those of the same names; the labels it had, to
/// give it back as it ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_labels_push(keys: *mut crate::ArrayHeader, values: *mut crate::ArrayHeader) -> i64 {
    let previous = super::tasks::labels();
    let mut set = get(previous).unwrap_or_default();
    for (key, value) in crate::testing::strings(keys).into_iter().zip(crate::testing::strings(values)) {
        set.retain(|(name, _)| *name != key);
        set.push((key, value));
    }
    set.sort();
    super::tasks::set_labels(intern(set));
    previous as i64
}

/// `profile.withLabels`, as it ends: the labels the task had.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_labels_restore(previous: i64) {
    super::tasks::set_labels(previous as u32);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_sets_by_number() {
        let users = vec![("endpoint".to_string(), "/users".to_string())];
        let number = intern(users.clone());
        assert_eq!(intern(users.clone()), number);
        assert_eq!(get(number), Some(users));
        assert_eq!(get(0), None);
        // A slot taken again by other sets no longer gives the old one.
        for request in 0..SLOTS {
            intern(vec![("span".to_string(), request.to_string())]);
        }
        assert_eq!(get(number), None);
    }
}
