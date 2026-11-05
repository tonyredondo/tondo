//! `tondo api --diff <rev>` (toolchain 7.3): what the project's public API
//! lost or changed since a commit, which breaks who uses it, and what it
//! gained, which does not.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// A public declaration: its text, and where it is.
struct Public {
    declaration: String,
    file: String,
    line: usize,
}

/// The public declarations of the project at `dir`, by module, kind and name.
fn public(dir: &Path) -> Result<BTreeMap<(String, String, String), Public>, String> {
    let packages = crate::deps::resolve(dir, None)?;
    let package = packages.first().ok_or("the project has no package")?;
    let (files, _) = crate::project::sources(&package.dir, &package.name, Some(crate::driver::system()))?;
    let mut found = BTreeMap::new();
    for file in files {
        let text =
            std::fs::read_to_string(&file.path).map_err(|e| format!("cannot read {}: {e}", file.path.display()))?;
        let module = file.module.join(".");
        let path = file.path.strip_prefix(&package.dir).unwrap_or(&file.path).display().to_string();
        for declared in crate::api::declarations(&crate::api::render(&text, false)) {
            let text_of = |key: &str| declared[key].as_str().unwrap_or_default().to_string();
            let declaration = text_of("declaration");
            let first = declaration.lines().next().unwrap_or_default();
            let line = text
                .lines()
                .position(|line| line.trim() == first.trim() || line.contains(first.trim_end_matches(" {")));
            let place = (module.clone(), text_of("kind"), text_of("name"));
            found.insert(place, Public { declaration, file: path.clone(), line: line.map_or(1, |line| line + 1) });
        }
    }
    Ok(found)
}

/// Says what changed; gives whether anything that breaks did.
pub fn diff(root: &Path, rev: &str, json: bool) -> Result<bool, String> {
    let now = public(root)?;
    let (worktree, project) = crate::bisect::Worktree::at(root, rev)?;
    let before = public(&project);
    drop(worktree);
    let before = before?;
    let item = |(module, _, name): &(String, String, String)| format!("{module}.{name}");
    let (mut removed, mut changed, mut added) = (Vec::new(), Vec::new(), Vec::new());
    for (key, old) in &before {
        match now.get(key) {
            None => removed.push(json!({ "name": item(key), "was": old.declaration })),
            Some(new) if new.declaration != old.declaration => changed.push(json!({
                "name": item(key), "was": old.declaration, "is": new.declaration, "file": new.file, "line": new.line,
            })),
            Some(_) => {}
        }
    }
    for (key, new) in now.iter().filter(|(key, _)| !before.contains_key(key)) {
        added.push(json!({ "name": item(key), "is": new.declaration, "file": new.file, "line": new.line }));
    }
    let breaks = !removed.is_empty() || !changed.is_empty();
    if json {
        let report = json!({ "breaks": breaks, "removed": removed, "changed": changed, "added": added });
        crate::cli::print_json("api-diff", true, report);
        return Ok(breaks);
    }
    let text = |value: &Value, key: &str| value[key].as_str().unwrap_or_default().to_string();
    for gone in &removed {
        println!("removed {}\n    was {}", text(gone, "name"), text(gone, "was"));
    }
    for one in &changed {
        println!("changed {} at {}:{}", text(one, "name"), text(one, "file"), one["line"]);
        println!("    was {}\n    is  {}", text(one, "was"), text(one, "is"));
    }
    for new in &added {
        println!("added {}", text(new, "name"));
    }
    if !breaks && added.is_empty() {
        println!("the public API is as it was at {rev}");
    }
    Ok(breaks)
}
