//! How a patch moves frames to the new code (toolchain 7.10): a frame of
//! a function whose code changes goes, at the next statement it starts,
//! to the same statement of the new code, the one on the same line once
//! the lines that stay the same are matched, when what the new code needs
//! there comes from what the old has.

use serde_json::Value;
use std::collections::HashMap;

/// The moves a patch ends with, as the runtime reads them, how many, and
/// where frames stay on the old code and why.
pub(super) struct Moves {
    pub bytes: Vec<u8>,
    pub count: usize,
    pub stays: Vec<String>,
}

/// The moves from the points of `old` to those of `new`, by identity, for
/// the functions `changed`, whose slots are those of `slots`.
pub(super) fn moves(old: &Value, new: &Value, changed: &[String], slots: &HashMap<String, u32>) -> Moves {
    let mut entries = Vec::new();
    let mut stays = Vec::new();
    for identity in changed {
        let (Some(before), Some(after), Some(&slot)) = (old.get(identity), new.get(identity), slots.get(identity))
        else {
            continue;
        };
        let lines = line_map(&hashes(before), &hashes(after));
        let (from, to) = (list(before, "points"), list(after, "points"));
        // Each point by its line, what it is and its place among those of
        // its kind on that line.
        let place = |list: &[Value]| -> Vec<(u64, String, usize)> {
            let mut seen: HashMap<(u64, String), usize> = HashMap::new();
            list.iter()
                .map(|p| {
                    let key = (p["line"].as_u64().unwrap_or(0), p["kind"].as_str().unwrap_or_default().to_string());
                    let count = seen.entry(key.clone()).or_insert(0);
                    *count += 1;
                    (key.0, key.1, *count - 1)
                })
                .collect()
        };
        let (old_places, new_places) = (place(from), place(to));
        let at: HashMap<&(u64, String, usize), usize> = new_places.iter().enumerate().map(|(i, p)| (p, i)).collect();
        let pairs: Vec<Option<usize>> = old_places
            .iter()
            .map(|(line, kind, ordinal)| {
                let line = *lines.get(&(*line as usize))? as u64;
                at.get(&(line, kind.clone(), *ordinal)).copied()
            })
            .collect();
        // The statements of the old code by those of the new, which name
        // the values the compiler makes for them.
        let mut paths: HashMap<&str, &str> = HashMap::from([("", "")]);
        for (index, pair) in pairs.iter().enumerate() {
            if let Some(pair) = pair {
                paths.insert(
                    from[index]["path"].as_str().unwrap_or_default(),
                    to[*pair]["path"].as_str().unwrap_or_default(),
                );
            }
        }
        let mut reasons: Vec<(String, Vec<u64>)> = Vec::new();
        for (index, point) in from.iter().enumerate() {
            let (Some(site), Some(pair)) = (point["site"].as_i64(), pairs[index]) else { continue };
            let target = &to[pair];
            match copies(point, target, &paths) {
                Ok(copies) => {
                    let word = |key: &str| target[key].as_u64().unwrap_or(0) as u32;
                    entries.push((site, slot, word("point"), word("size"), copies));
                }
                Err(reason) => {
                    let line = after["top"].as_u64().unwrap_or(0) + target["line"].as_u64().unwrap_or(0);
                    match reasons.iter_mut().find(|(r, _)| *r == reason) {
                        Some((_, lines)) => lines.push(line),
                        None => reasons.push((reason, vec![line])),
                    }
                }
            }
        }
        let name = shown(identity);
        let file = after["file"].as_str().unwrap_or_default();
        for (reason, mut lines) in reasons {
            lines.sort();
            lines.dedup();
            let plural = if lines.len() == 1 { "line" } else { "lines" };
            stays.push(format!(
                "a frame of `{name}` stays on the old code at {plural} {} of {file}: {reason}",
                ranges(&lines)
            ));
        }
    }
    // A function whose parameters or result change is another: frames of
    // the old one finish in its code.
    let names: Vec<&str> = new.as_object().into_iter().flatten().map(|(identity, _)| shown(identity)).collect();
    for (identity, entry) in old.as_object().into_iter().flatten() {
        let sited = entry["points"].as_array().is_some_and(|p| p.iter().any(|p| p["site"].is_i64()));
        if sited && new.get(identity).is_none() && names.contains(&shown(identity)) {
            let why = match identity.starts_with("closure in ") {
                true => "its function's closures or its signature changed",
                false => "its parameters or result changed",
            };
            stays.push(format!("frames of `{}` finish on the old code: {why}", shown(identity)));
        }
    }
    stays.sort();
    stays.dedup();
    let count = entries.len();
    let mut bytes = Vec::new();
    if count > 0 {
        bytes.extend(b"OSR1");
        bytes.extend((count as u32).to_le_bytes());
        for (site, slot, point, size, copies) in entries {
            bytes.extend(site.to_le_bytes());
            bytes.extend([slot, point, size, copies.len() as u32].into_iter().flat_map(u32::to_le_bytes));
            for (from, to, size) in copies {
                bytes.extend([from, to, size].into_iter().flat_map(u32::to_le_bytes));
            }
        }
    }
    Moves { bytes, count, stays }
}

/// A function as a report names it: without its parameters, result and
/// place among its function's closures.
fn shown(identity: &str) -> &str {
    let name = identity.split('(').next().unwrap_or(identity);
    match name.rsplit_once(' ') {
        Some((closure, place)) if place.contains('/') && closure.starts_with("closure in ") => closure,
        _ => name,
    }
}

fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map_or(&[], Vec::as_slice)
}

fn hashes(entry: &Value) -> Vec<u64> {
    entry["lines"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect()
}

/// Which lines of the new text each line of the old one is: those that
/// stay the same, by their longest common sequence, and those that change
/// in place, one for one where the change starts.
fn line_map(old: &[u64], new: &[u64]) -> HashMap<usize, usize> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..].iter().rev().zip(new[prefix..].iter().rev()).take_while(|(a, b)| a == b).count();
    let (a, b) = (&old[prefix..old.len() - suffix], &new[prefix..new.len() - suffix]);
    let width = b.len() + 1;
    let mut table = vec![0u32; (a.len() + 1) * width];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i * width + j] = match a[i] == b[j] {
                true => table[(i + 1) * width + j + 1] + 1,
                false => table[(i + 1) * width + j].max(table[i * width + j + 1]),
            };
        }
    }
    let mut map: HashMap<usize, usize> = (0..prefix).map(|i| (i, i)).collect();
    for k in 0..suffix {
        map.insert(old.len() - 1 - k, new.len() - 1 - k);
    }
    let (mut i, mut j, mut hunk) = (0, 0, (0, 0));
    let changed = |map: &mut HashMap<usize, usize>, (i0, j0): (usize, usize), i: usize, j: usize| {
        for k in 0..(i - i0).min(j - j0) {
            map.insert(prefix + i0 + k, prefix + j0 + k);
        }
    };
    loop {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            changed(&mut map, hunk, i, j);
            map.insert(prefix + i, prefix + j);
            (i, j) = (i + 1, j + 1);
            hunk = (i, j);
        } else if i < a.len() && (j == b.len() || table[(i + 1) * width + j] >= table[i * width + j + 1]) {
            i += 1;
        } else if j < b.len() {
            j += 1;
        } else {
            changed(&mut map, hunk, i, j);
            return map;
        }
    }
}

/// Where each part of the state of the old point goes in that of the new,
/// or why the frame cannot move there.
fn copies(old: &Value, new: &Value, paths: &HashMap<&str, &str>) -> Result<Vec<(u32, u32, u32)>, String> {
    if old["defers"] != new["defers"] {
        return Err("the `defer`s registered there changed".into());
    }
    // The old state by the names of the new code.
    let mut given: Vec<(String, &Value)> = list(old, "state")
        .iter()
        .filter_map(|item| {
            let key = item["key"].as_str()?;
            let key = match key.strip_prefix('#') {
                Some(made) => {
                    let (path, ordinal) = made.rsplit_once('/')?;
                    format!("#{}/{ordinal}", paths.get(path)?)
                }
                None => key.to_string(),
            };
            Some((key, item))
        })
        .collect();
    let named = |key: &str| key.rsplit_once('/').map_or(key, |(name, _)| name).to_string();
    let mut copies = Vec::new();
    for item in list(new, "state") {
        let key = item["key"].as_str().unwrap_or_default();
        let made = key.starts_with('#');
        let Some(index) = given.iter().position(|(k, _)| k == key) else {
            return Err(match made {
                true => "the statement around it is new".into(),
                false => format!("`{}` is new there", named(key)),
            });
        };
        let (_, from) = given.swap_remove(index);
        if from["type"] != item["type"] || from["size"] != item["size"] {
            return Err(match made {
                true => "a value of the statement around it has another type".into(),
                false => format!("`{}` has another type", named(key)),
            });
        }
        if from["lent"].as_bool() == Some(true) || item["lent"].as_bool() == Some(true) {
            return Err(match made {
                true => "it is inside a `lock` or borrows a value there".into(),
                false => format!("`{}` borrows a value there", named(key)),
            });
        }
        let word = |v: &Value, key: &str| v[key].as_u64().unwrap_or(0) as u32;
        copies.push((word(from, "offset"), word(item, "offset"), word(item, "size")));
    }
    // What the old code owns there and the new does not would never be
    // released.
    if let Some((key, _)) = given.iter().find(|(_, item)| item["owned"].as_bool() == Some(true)) {
        return Err(match key.starts_with('#') {
            true => "a value the old code holds there would not be released".into(),
            false => format!("`{}` would not be released", named(key)),
        });
    }
    Ok(copies)
}

/// Lines as a short list: `3, 5–8`.
fn ranges(lines: &[u64]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let start = lines[index];
        while index + 1 < lines.len() && lines[index + 1] == lines[index] + 1 {
            index += 1;
        }
        parts.push(match lines[index] == start {
            true => start.to_string(),
            false => format!("{start}–{}", lines[index]),
        });
        index += 1;
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_match_where_they_stay_and_change_in_place() {
        // a b c d → a x c y d: b becomes x, y is new.
        let map = line_map(&[1, 2, 3, 4], &[1, 9, 3, 8, 4]);
        assert_eq!((map[&0], map[&1], map[&2], map[&3]), (0, 1, 2, 4));
        // A line removed has no place.
        let map = line_map(&[1, 2, 3], &[1, 3]);
        assert_eq!((map[&0], map.get(&1), map[&2]), (0, None, 1));
        assert_eq!(ranges(&[3, 5, 6, 7, 9]), "3, 5–7, 9");
    }
}
