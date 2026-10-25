//! Live builds (toolchain 7.10): debug builds whose code changes while
//! they run. Every call of one of the program's functions goes through a
//! slot of a table, and the image keeps tables of what a patch may use of
//! it: the runtime's functions, the functions of C it calls and the
//! identity of each type. A patch is the program compiled again; the
//! runtime maps it and points the slots at its functions, and the old
//! code stays for the frames still in it.

mod migrate;
mod osr;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// What codegen needs of a live build, and what it tells of it.
pub struct Build {
    /// The build's identity, which its program reports.
    pub id: String,
    /// The slot of each function by its identity: its name, its
    /// parameters and its result.
    pub slots: HashMap<String, u32>,
    pub capacity: u32,
    /// The runtime's functions, in the order of the image's table.
    pub runtime: Vec<String>,
    /// The functions of C the image calls, and the types whose identity it
    /// holds, in the order of their tables.
    pub c: Vec<String>,
    pub data: Vec<String>,
    /// A hash of each function's code, to tell what a patch changes.
    pub code: HashMap<String, u64>,
    /// The debugger's sites of the build.
    pub sites: u32,
    /// For a patch: what the running program has.
    pub patch: Option<Base>,
    /// The nominal types a frame of each function may keep in values with
    /// identity, which a patch does not convert, by the function's name.
    pub holds: HashMap<String, std::collections::BTreeSet<String>>,
    /// Each function's lines and points, by its identity: what moves frames
    /// to new code; and what each type their states hold is made of, by its
    /// key.
    pub points: serde_json::Map<String, serde_json::Value>,
    pub structures: serde_json::Map<String, serde_json::Value>,
    /// For a patch: the functions that convert values of the running
    /// program's types to the new ones, in the order of the patch's table.
    pub migrations: Vec<crate::hir::FnId>,
}

/// The types renamed, new names by old: of the types the old program had
/// and the new has not, each of those the new has and the old had not of the
/// same kind and mostly the same fields or variants, if it is the one.
fn renamed_types(
    old: &serde_json::Map<String, serde_json::Value>,
    new: &HashMap<String, String>,
) -> HashMap<String, String> {
    // A shape's kind and the names of its fields or variants, or what a
    // newtype wraps.
    let members = |shape: &str| -> (char, Vec<String>) {
        if let Some(inner) = shape.strip_prefix("= ") {
            return ('=', vec![inner.to_string()]);
        }
        let record = shape.starts_with('{');
        let (mut depth, mut word, mut names, mut naming) = (0, String::new(), Vec::new(), true);
        for c in shape.chars() {
            match c {
                '{' | '[' | '(' => depth += 1,
                '}' | ']' | ')' => depth -= 1,
                _ => {}
            }
            let top = if record { depth == 1 } else { depth == 0 };
            match c {
                ':' if record && top && naming => {
                    names.push(word.trim().to_string());
                    naming = false;
                }
                ',' if record && top => (word, naming) = (String::new(), true),
                '|' if !record && top => (word, naming) = (String::new(), true),
                c if !record && top && naming && (c == '(' || c == ' ' && !word.trim().is_empty()) => {
                    names.push(word.trim().to_string());
                    naming = false;
                }
                c if top && naming && c != '{' => word.push(c),
                _ => {}
            }
        }
        if !record && naming && !word.trim().is_empty() {
            names.push(word.trim().to_string());
        }
        (if record { '{' } else { '|' }, names)
    };
    let gone: Vec<(&String, &str)> =
        old.iter().filter(|(name, _)| !new.contains_key(*name)).filter_map(|(n, s)| Some((n, s.as_str()?))).collect();
    let fresh: Vec<(&String, &String)> = new.iter().filter(|(name, _)| !old.contains_key(*name)).collect();
    let mut renamed = HashMap::new();
    for (name, shape) in gone {
        let (kind, names) = members(shape);
        let mut scored: Vec<(f64, &String)> = fresh
            .iter()
            .filter_map(|(now, shape)| {
                let (other, members) = members(shape);
                let common = names.iter().filter(|n| members.contains(n)).count();
                let all = names.len() + members.len() - common;
                (other == kind && all > 0).then(|| (common as f64 / all as f64, *now))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        if let [(best, now), rest @ ..] = scored.as_slice()
            && *best >= 0.5
            && rest.first().is_none_or(|r| r.0 < *best)
        {
            renamed.insert(name.clone(), (*now).clone());
        }
    }
    renamed
}

/// A statement's place, as text.
pub(crate) fn path_text(path: &[u32]) -> String {
    path.iter().map(u32::to_string).collect::<Vec<_>>().join(".")
}

/// A line's hash, whatever its indentation.
pub(crate) fn line_hash(line: &str) -> u64 {
    u64::from_str_radix(&crate::driver::fingerprint(&[line.trim().as_bytes()]), 16).unwrap_or(0)
}

/// What a patch builds on: the types whose identity the running program
/// holds, and the number of its debugger's sites.
pub struct Base {
    pub data: Vec<String>,
    pub sites: u32,
    pub code: HashMap<String, u64>,
}

impl Build {
    pub fn new(runtime: Vec<String>) -> Build {
        Build {
            id: build_id(),
            slots: HashMap::new(),
            capacity: 0,
            runtime,
            c: Vec::new(),
            data: Vec::new(),
            code: HashMap::new(),
            sites: 0,
            patch: None,
            holds: HashMap::new(),
            points: serde_json::Map::new(),
            structures: serde_json::Map::new(),
            migrations: Vec::new(),
        }
    }
}

/// A new build's identity: when and where it was made.
fn build_id() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    crate::driver::fingerprint(&[&now.as_nanos().to_le_bytes(), &std::process::id().to_le_bytes()])
}

/// Room for the functions a patch adds.
pub fn capacity(functions: usize) -> u32 {
    (functions * 2 + 4096) as u32
}

/// Where the manifest of a build is kept.
pub fn manifest_path(id: &str) -> PathBuf {
    crate::driver::cache_dir().join("live").join(format!("{id}.json"))
}

/// How a live program was built, which a patch builds it again with.
pub struct Options {
    pub statically: bool,
    pub target: Option<crate::target::Target>,
    pub libraries: Vec<String>,
}

/// The object of a live build of the program at `path`, whose manifest it
/// keeps for the patches.
pub fn compile(
    analysis: &crate::driver::Analysis,
    path: &std::path::Path,
    options: &Options,
) -> Result<Vec<u8>, String> {
    let mut build = Build::new(crate::driver::runtime_functions(options.statically, options.target)?);
    let object = crate::driver::compile(analysis, true, options.target, Some(&mut build))?;
    let types = &analysis.program.as_ref().expect("a checked program").types;
    save(&build, &shapes(types), path, options, None)?;
    Ok(object)
}

/// A nominal type's name, as manifests key it.
fn adt_name(types: &crate::types::Types, id: crate::types::AdtId) -> String {
    let adt = types.adt(id);
    match &adt.instance {
        Some((_, args)) if !adt.name.contains('[') => {
            let args: Vec<String> = args.iter().map(|&arg| types.display(arg)).collect();
            format!("{}[{}]", adt.name, args.join(", "))
        }
        _ => adt.name.clone(),
    }
}

/// What a nominal type is made of: its fields, its variants, or what a
/// newtype wraps.
fn adt_shape(types: &crate::types::Types, id: crate::types::AdtId) -> String {
    use crate::types::{AdtKind, Payload};
    let fields = |fields: &[crate::types::Field]| {
        let fields: Vec<String> = fields.iter().map(|f| format!("{}: {}", f.name, types.display(f.ty))).collect();
        format!("{{ {} }}", fields.join(", "))
    };
    match &types.adt(id).kind {
        AdtKind::Record(record) => fields(record),
        AdtKind::Newtype(inner) => format!("= {}", types.display(*inner)),
        AdtKind::Enum(variants) => {
            let variants: Vec<String> = variants
                .iter()
                .map(|v| match &v.payload {
                    Payload::Unit => v.name.clone(),
                    Payload::Tuple(items) => {
                        let items: Vec<String> = items.iter().map(|&t| types.display(t)).collect();
                        format!("{}({})", v.name, items.join(", "))
                    }
                    Payload::Record(record) => format!("{} {}", v.name, fields(record)),
                })
                .collect();
            variants.join(" | ")
        }
    }
}

/// What each nominal type is made of. A patch may change one only when no
/// running frame may hold a value of the old one.
fn shapes(types: &crate::types::Types) -> HashMap<String, String> {
    (0..types.adt_count())
        .map(|index| crate::types::AdtId(index as u32))
        .map(|id| (adt_name(types, id), adt_shape(types, id)))
        .collect()
}

/// The nominal types a value of these types holds, through fields,
/// variants, elements and captures; with `signatures`, also those the
/// parameters and results of function types name.
pub(crate) fn reachable(
    types: &crate::types::Types,
    tys: &[crate::types::Ty],
    signatures: bool,
) -> std::collections::BTreeSet<u32> {
    use crate::types::{AdtKind, TyKind};
    fn walk(
        types: &crate::types::Types,
        ty: crate::types::Ty,
        signatures: bool,
        seen: &mut std::collections::BTreeSet<u32>,
    ) {
        let each = |tys: &[crate::types::Ty], seen: &mut std::collections::BTreeSet<u32>| {
            tys.iter().for_each(|&t| walk(types, t, signatures, seen))
        };
        match types.kind(ty) {
            TyKind::Adt(id) => {
                if seen.insert(id.0) {
                    match &types.adt(*id).kind {
                        AdtKind::Record(fields) => fields.iter().for_each(|f| walk(types, f.ty, signatures, seen)),
                        AdtKind::Newtype(inner) => walk(types, *inner, signatures, seen),
                        AdtKind::Enum(variants) => variants.iter().for_each(|v| each(&v.payload.types(), seen)),
                    }
                }
            }
            TyKind::Tuple(items) | TyKind::Union(items) | TyKind::Opaque(_, items) => each(items, seen),
            TyKind::Option(t)
            | TyKind::Array(t)
            | TyKind::Deque(t)
            | TyKind::Set(t)
            | TyKind::Pointer(t)
            | TyKind::CArray(t, _)
            | TyKind::Handle(_, t) => walk(types, *t, signatures, seen),
            TyKind::Result(a, b) | TyKind::Map(a, b) => each(&[*a, *b], seen),
            TyKind::Fn(params, result, ..) | TyKind::CFn(params, result, _) if signatures => {
                each(params, seen);
                walk(types, *result, signatures, seen);
            }
            _ => {}
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for &ty in tys {
        walk(types, ty, signatures, &mut seen);
    }
    seen
}

/// A digest of the shapes of the nominal types these types reach, which a
/// live build's identities carry: a function or type whose shapes change
/// is another, and the old code keeps the old one.
pub(crate) fn shape_digest(types: &crate::types::Types, tys: &[crate::types::Ty]) -> String {
    let adts = reachable(types, tys, true);
    if adts.is_empty() {
        return String::new();
    }
    let text: Vec<String> = adts
        .into_iter()
        .map(crate::types::AdtId)
        .map(|id| format!("{} {}", adt_name(types, id), adt_shape(types, id)))
        .collect();
    let text = text.join("\n");
    crate::driver::fingerprint(&[text.as_bytes()])[..8].to_string()
}

/// The names of the nominal types a frame of a function may keep in values
/// with identity, by the types of its locals: what `Ref`s, channels,
/// tasks and `std.sync` values hold.
pub(crate) fn held(types: &crate::types::Types, locals: &[crate::types::Ty]) -> Vec<String> {
    use crate::types::{AdtKind, TyKind};
    fn walk(
        types: &crate::types::Types,
        ty: crate::types::Ty,
        seen: &mut HashSet<u32>,
        out: &mut Vec<crate::types::Ty>,
    ) {
        let mut each =
            |tys: &[crate::types::Ty], seen: &mut HashSet<u32>| tys.iter().for_each(|&t| walk(types, t, seen, out));
        match types.kind(ty) {
            TyKind::Handle(_, state) => out.push(*state),
            TyKind::Adt(id) if seen.insert(id.0) => match &types.adt(*id).kind {
                AdtKind::Record(fields) => each(&fields.iter().map(|f| f.ty).collect::<Vec<_>>(), seen),
                AdtKind::Newtype(inner) => each(&[*inner], seen),
                AdtKind::Enum(variants) => variants.iter().for_each(|v| each(&v.payload.types(), seen)),
            },
            TyKind::Tuple(items) | TyKind::Union(items) => each(items, seen),
            TyKind::Option(t) | TyKind::Array(t) | TyKind::Deque(t) | TyKind::Set(t) => each(&[*t], seen),
            TyKind::Result(a, b) | TyKind::Map(a, b) => each(&[*a, *b], seen),
            _ => {}
        }
    }
    let (mut seen, mut inner) = (HashSet::new(), Vec::new());
    for &local in locals {
        walk(types, local, &mut seen, &mut inner);
    }
    reachable(types, &inner, false).into_iter().map(|id| adt_name(types, crate::types::AdtId(id))).collect()
}

/// How a live build's tables and manifests name a type: as it is shown,
/// and the digest of the shapes it reaches.
pub(crate) fn type_key(types: &crate::types::Types, ty: crate::types::Ty) -> String {
    let digest = shape_digest(types, &[ty]);
    match digest.is_empty() {
        true => types.display(ty),
        false => format!("{} @{digest}", types.display(ty)),
    }
}

/// What a type is made of, by the keys of the types it is made of, which
/// `out` gets too: what a patch rebuilds a type of an older shape from.
pub(crate) fn describe(
    types: &crate::types::Types,
    ty: crate::types::Ty,
    out: &mut serde_json::Map<String, serde_json::Value>,
) -> String {
    use crate::types::{AdtKind, Payload, TyKind};
    use serde_json::json;
    let key = type_key(types, ty);
    if out.contains_key(&key) {
        return key;
    }
    // Recursive types reach themselves.
    out.insert(key.clone(), serde_json::Value::Null);
    let keys = |tys: &[crate::types::Ty], out: &mut serde_json::Map<String, serde_json::Value>| -> Vec<String> {
        tys.iter().map(|&t| describe(types, t, out)).collect()
    };
    let fields = |fields: &[crate::types::Field], out: &mut serde_json::Map<String, serde_json::Value>| {
        let described: Vec<serde_json::Value> =
            fields.iter().map(|f| json!([f.name, describe(types, f.ty, out), f.private])).collect();
        described
    };
    let described = match types.kind(ty).clone() {
        TyKind::Adt(id) => {
            let name = adt_name(types, id);
            match types.adt(id).kind.clone() {
                AdtKind::Record(record) => json!({ "adt": name, "record": fields(&record, out) }),
                AdtKind::Newtype(inner) => json!({ "adt": name, "newtype": describe(types, inner, out) }),
                AdtKind::Enum(variants) => {
                    let variants: Vec<serde_json::Value> = variants
                        .iter()
                        .map(|v| match &v.payload {
                            Payload::Unit => json!({ "name": v.name }),
                            Payload::Tuple(items) => json!({ "name": v.name, "tuple": keys(items, out) }),
                            Payload::Record(record) => json!({ "name": v.name, "record": fields(record, out) }),
                        })
                        .collect();
                    json!({ "adt": name, "enum": variants })
                }
            }
        }
        TyKind::Option(t) => json!({ "option": keys(&[t], out) }),
        TyKind::Array(t) => json!({ "array": keys(&[t], out) }),
        TyKind::Deque(t) => json!({ "deque": keys(&[t], out) }),
        TyKind::Set(t) => json!({ "set": keys(&[t], out) }),
        TyKind::Map(k, v) => json!({ "map": keys(&[k, v], out) }),
        TyKind::Result(a, b) => json!({ "result": keys(&[a, b], out) }),
        TyKind::Tuple(items) => json!({ "tuple": keys(&items, out) }),
        // What a patch does not rebuild: scalars, which do not change, and
        // values with identity or code.
        _ => json!({ "other": types.display(ty) }),
    };
    out.insert(key.clone(), described);
    key
}

/// Writes a build's manifest: the build's own, or one process's after
/// its patches.
fn save(
    build: &Build,
    shapes: &HashMap<String, String>,
    path: &std::path::Path,
    options: &Options,
    pid: Option<u32>,
) -> Result<(), String> {
    // The path as it was given, and where from: the places in its code
    // name its files so, and a patch's must name them the same.
    let manifest = serde_json::json!({
        "build": build.id,
        "dir": std::env::current_dir().map_err(|e| e.to_string())?,
        "path": path,
        "static": options.statically,
        "target": options.target.map(crate::target::Target::name),
        "libraries": options.libraries,
        "capacity": build.capacity,
        "slots": build.slots,
        "runtime": build.runtime,
        "c": build.c,
        "data": build.data,
        "code": build.code,
        "sites": build.sites,
        "types": shapes,
        "holds": build.holds,
        "points": build.points,
        "structures": build.structures,
    });
    let file = match pid {
        Some(pid) => manifest_path(&format!("{}-{pid}", build.id)),
        None => manifest_path(&build.id),
    };
    std::fs::create_dir_all(file.parent().expect("a directory")).map_err(|e| e.to_string())?;
    std::fs::write(&file, manifest.to_string()).map_err(|e| format!("cannot write {}: {e}", file.display()))
}

/// What a reload did: the functions whose code changed and those it added;
/// how many places of the old code move frames to the new, and where they
/// stay on the old code and why.
pub struct Reloaded {
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub moves: usize,
    pub stays: Vec<String>,
    /// What converting values of types of another shape does to them.
    pub notes: Vec<String>,
}

/// Why a reload did not happen: the program does not compile, or the
/// change needs it started again.
pub enum Refused {
    Diagnostics(String),
    Restart(String),
    Failed(String),
}

impl From<String> for Refused {
    fn from(message: String) -> Refused {
        Refused::Failed(message)
    }
}

/// Builds the program a live process runs again from its sources, and
/// patches the process `pid` with it; `ask` sends the process a request
/// and gives its answer.
pub fn reload(pid: u32, ask: &mut dyn FnMut(&str) -> Result<serde_json::Value, String>) -> Result<Reloaded, Refused> {
    let live = ask("live")?;
    if let Some(error) = live["error"].as_str() {
        return Err(Refused::Failed(error.to_string()));
    }
    let id = live["build"].as_str().unwrap_or_default().to_string();
    let file = [manifest_path(&format!("{id}-{pid}")), manifest_path(&id)].into_iter().find(|f| f.exists());
    let file = file.ok_or_else(|| format!("tondo has no manifest of the build {id}: build it again with --live"))?;
    let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let strings = |key: &str| -> Vec<String> {
        manifest[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
    };
    let path = std::path::PathBuf::from(manifest["path"].as_str().unwrap_or_default());
    let options = Options {
        statically: manifest["static"].as_bool().unwrap_or(false),
        target: manifest["target"].as_str().and_then(|t| crate::target::Target::parse(t).ok()),
        libraries: strings("libraries"),
    };
    let dir = std::path::PathBuf::from(manifest["dir"].as_str().unwrap_or("."));
    let here = std::env::current_dir().map_err(|e| e.to_string())?;
    std::env::set_current_dir(&dir).map_err(|e| format!("cannot go to {}: {e}", dir.display()))?;
    let reloaded = patch(pid, ask, &manifest, &path, options, &live, id);
    let _ = std::env::set_current_dir(here);
    reloaded
}

/// The rest of a reload, from the directory the program was built in.
fn patch(
    pid: u32,
    ask: &mut dyn FnMut(&str) -> Result<serde_json::Value, String>,
    manifest: &serde_json::Value,
    path: &std::path::Path,
    options: Options,
    live: &serde_json::Value,
    id: String,
) -> Result<Reloaded, Refused> {
    let strings = |key: &str| -> Vec<String> {
        manifest[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
    };
    let mut analysis = crate::driver::analyze(path)?;
    if analysis.program.is_none() {
        return Err(Refused::Diagnostics(analysis.diags.render(&analysis.sources)));
    }
    let types = &analysis.program.as_ref().expect("a checked program").types;
    let shapes = shapes(types);
    // A type of another shape needs that no frame of the old code keeps a
    // value of it in a value with identity, which no conversion reaches:
    // the runtime looks before the patch goes in.
    let mut reshaped: Vec<String> = manifest["types"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, shape)| shapes.get(*name).is_some_and(|now| Some(now.as_str()) != shape.as_str()))
        .map(|(name, _)| name.clone())
        .collect();
    // A type renamed is another shape to the frames that keep the old one.
    let renamed = renamed_types(manifest["types"].as_object().unwrap_or(&serde_json::Map::new()), &shapes);
    reshaped.extend(renamed.keys().cloned());
    reshaped.sort();
    let guarded: Vec<String> = manifest["holds"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, held)| {
            held.as_array().into_iter().flatten().any(|t| t.as_str().is_some_and(|t| reshaped.iter().any(|r| r == t)))
        })
        .map(|(name, _)| name.clone())
        .collect();
    let known: HashMap<String, u64> = serde_json::from_value(manifest["code"].clone()).unwrap_or_default();
    let (runtime, c, data) = (strings("runtime"), strings("c"), strings("data"));
    let mut build = Build {
        id: id.clone(),
        slots: serde_json::from_value(manifest["slots"].clone()).unwrap_or_default(),
        capacity: manifest["capacity"].as_u64().unwrap_or(0) as u32,
        runtime: runtime.clone(),
        c: c.clone(),
        data: data.clone(),
        code: HashMap::new(),
        sites: 0,
        patch: Some(Base {
            data: data.clone(),
            sites: live["sites"].as_u64().unwrap_or(0) as u32,
            code: known.clone(),
        }),
        holds: HashMap::new(),
        points: serde_json::Map::new(),
        structures: serde_json::Map::new(),
        migrations: Vec::new(),
    };
    // The values of the old types frames hold convert as the frames move,
    // with the parameters of those that move to a function of another
    // identity.
    let functions = || manifest["points"].as_object().into_iter().flatten().map(|(_, function)| function);
    let held = functions()
        .flat_map(|function| function["points"].as_array().into_iter().flatten())
        .flat_map(|point| point["state"].as_array().into_iter().flatten());
    let passed = functions().flat_map(|function| function["params"].as_array().into_iter().flatten());
    let mut wanted: Vec<String> =
        held.chain(passed).filter_map(|item| item["type"].as_str().map(String::from)).collect();
    wanted.extend(functions().filter_map(|function| function["result"].as_str().map(String::from)));
    wanted.sort();
    wanted.dedup();
    let empty = serde_json::Map::new();
    let structures = manifest["structures"].as_object().unwrap_or(&empty);
    // A closure's captures, by the closure's function.
    let closures: Vec<(String, String)> = manifest["points"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(identity, _)| identity.starts_with("closure in "))
        .filter_map(|(identity, function)| {
            let captures = function["params"].as_array()?.first()?["type"].as_str()?.to_string();
            Some((captures, osr::base(identity).split('(').next()?.to_string()))
        })
        .collect();
    let program = analysis.program.as_mut().expect("a checked program");
    let latest = manifest["types"].as_object().unwrap_or(&empty);
    let migrated = migrate::add(program, structures, &wanted, &closures, &renamed, latest);
    let mut migrations = osr::Migrations { refused: migrated.refused, ..osr::Migrations::default() };
    for (keys, function) in migrated.conversions {
        migrations.conversions.insert(keys, build.migrations.len());
        build.migrations.push(function);
    }
    for (keys, function) in migrated.backs {
        migrations.backs.insert(keys, build.migrations.len());
        build.migrations.push(function);
    }
    let object = crate::driver::compile(&analysis, true, options.target, Some(&mut build)).map_err(Refused::Failed)?;
    use crate::link::patch::External;
    let resolve = |name: &str| -> Result<External, String> {
        let at = |list: &[String], table: u32| {
            list.iter().position(|n| n == name).map(|i| External { table, index: i as u32 })
        };
        if name == "tondo_live_slots" {
            return Ok(External { table: 0, index: 0 });
        }
        if let Some(index) = name.strip_prefix("tondo_live_data").and_then(|i| i.parse().ok()) {
            return Ok(External { table: 3, index });
        }
        at(&runtime, 1).or_else(|| at(&c, 2)).ok_or_else(|| {
            format!("the change calls `{name}`, which the running program does not link: start it again")
        })
    };
    let mut patch = crate::link::patch::link(&object, &resolve, "tondo_patch").map_err(Refused::Restart)?;
    // The functions whose code changes, which the patch sets, and how
    // their frames move to it.
    let set: Vec<String> = build
        .code
        .iter()
        .filter(|(identity, hash)| known.get(*identity) != Some(hash))
        .map(|(i, _)| i.clone())
        .collect();
    let points = serde_json::Value::Object(build.points.clone());
    let moves = osr::moves(&manifest["points"], &points, &set, &build.slots, &migrations, &renamed);
    patch.extend(&moves.bytes);
    let patch_file = crate::driver::cache_dir().join("live").join(format!("patch-{pid}.bin"));
    std::fs::write(&patch_file, &patch).map_err(|e| format!("cannot write {}: {e}", patch_file.display()))?;
    let request: Vec<String> = std::iter::once(format!("patch {}", patch_file.display())).chain(guarded).collect();
    let answer = ask(&request.join("\t"));
    let _ = std::fs::remove_file(&patch_file);
    let answer = answer?;
    if let Some(alive) = answer["alive"].as_array() {
        let shown: Vec<String> = reshaped.iter().map(|t| format!("`{t}`")).collect();
        let places: Vec<String> = alive
            .iter()
            .map(|frame| {
                let function = frame["function"].as_str().unwrap_or_default();
                match frame["at"].as_str().filter(|at| !at.is_empty()) {
                    Some(at) => format!("`{function}` ({at})"),
                    None => format!("`{function}`"),
                }
            })
            .collect();
        let mut places = places;
        places.dedup();
        return Err(Refused::Restart(format!(
            "{} changed what {} made of, and {} may keep values of the old shape in a `Ref`, a channel, a task or a value of `std.sync`, which a reload does not convert: start the program again",
            shown.join(", "),
            if shown.len() == 1 { "it is" } else { "they are" },
            places.join(", "),
        )));
    }
    if let Some(error) = answer["error"].as_str() {
        return Err(Refused::Restart(error.to_string()));
    }
    let mut changed = Vec::new();
    let mut added = Vec::new();
    for (identity, hash) in &build.code {
        // A closure without its place among its function's; conversions
        // are the patch's own.
        let name = identity.split('(').next().unwrap_or(identity);
        if name.starts_with("migrate ") {
            continue;
        }
        let name = match name.rsplit_once(' ') {
            Some((closure, place)) if place.contains('/') && closure.starts_with("closure in ") => closure,
            _ => name,
        }
        .to_string();
        // A function whose signature reaches a type of another shape, or
        // a closure numbered otherwise, is another identity, but the same.
        match known.get(identity) {
            Some(old) if old == hash => {}
            Some(_) => changed.push(name),
            None if known.keys().any(|k| osr::with_names(&osr::base(k), &renamed) == osr::base(identity)) => {
                changed.push(name)
            }
            None => added.push(name),
        }
    }
    let mut code = known;
    code.extend(build.code.drain());
    build.code = code;
    // The points of the code each function runs now, and what the types
    // of their states are made of.
    for (key, structure) in manifest["structures"].as_object().into_iter().flatten() {
        build.structures.entry(key.clone()).or_insert_with(|| structure.clone());
    }
    let mut points = manifest["points"].as_object().cloned().unwrap_or_default();
    for identity in &set {
        if let Some(entry) = build.points.get(identity) {
            points.insert(identity.clone(), entry.clone());
        }
    }
    build.points = points;
    save(&build, &shapes, path, &options, Some(pid))?;
    changed.sort();
    changed.dedup();
    added.sort();
    added.dedup();
    // What this reload's changes of shape do; conversions from older
    // versions, which frames may still hold, go without saying.
    let mut notes: Vec<String> = migrated
        .notes
        .into_iter()
        .filter(|note| {
            let name = note.trim_start_matches('`').split(['.', '`']).next().unwrap_or_default();
            reshaped.iter().any(|r| r == name || renamed.get(r).is_some_and(|now| now == name))
        })
        .collect();
    notes.extend(renamed.iter().map(|(old, new)| format!("`{old}` is `{new}` now")));
    notes.sort();
    notes.dedup();
    Ok(Reloaded { changed, added, moves: moves.count, stays: moves.stays, notes })
}

/// `tondo reload <pid|name>`: the running live program `name`.
pub fn reload_named(name: &str) -> Result<Reloaded, Refused> {
    let mut program = crate::observe::connect(name)?;
    let pid = program.pid;
    reload(pid, &mut |request| program.ask(request))
}

impl Refused {
    /// The refusal as one message.
    pub fn message(&self) -> String {
        match self {
            Refused::Diagnostics(text) => format!("{}\nnot reloaded: the program goes on as it was", text.trim_end()),
            Refused::Restart(message) | Refused::Failed(message) => format!("not reloaded: {message}"),
        }
    }
}

impl Reloaded {
    /// What a reload says it did, then where frames stay on the old code.
    pub fn summary(&self) -> String {
        let mut summary = self.changes();
        if self.moves > 0 {
            summary.push_str("; frames move to the new code at their next statement");
        }
        for line in self.notes.iter().chain(&self.stays) {
            summary.push('\n');
            summary.push_str(line);
        }
        summary
    }

    /// As JSON, for the debugger's and agents' protocols.
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "changed": self.changed,
            "added": self.added,
            "moves": self.moves,
            "stays": self.stays,
            "converted": self.notes,
        })
    }

    fn changes(&self) -> String {
        let list = |names: &[String]| names.join(", ");
        match (self.changed.is_empty(), self.added.is_empty()) {
            (true, true) => "reloaded: no function changed".into(),
            (false, true) => format!("reloaded: {} changed ({})", self.changed.len(), list(&self.changed)),
            (true, false) => format!("reloaded: {} added ({})", self.added.len(), list(&self.added)),
            (false, false) => format!(
                "reloaded: {} changed ({}), {} added ({})",
                self.changed.len(),
                list(&self.changed),
                self.added.len(),
                list(&self.added)
            ),
        }
    }
}
