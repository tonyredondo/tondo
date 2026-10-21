//! Exhaustiveness and reachability of patterns (spec 14.2 and 14.5).
//!
//! This is the usefulness algorithm of Maranget, "Warnings for pattern
//! matching" (2007). A row is useful against a matrix when some value matches
//! it and no earlier row; an arm that is not useful is unreachable, and a
//! wildcard that is still useful after every arm is a value the match misses.
//! The witness of usefulness is reported to the programmer.
//!
//! Arrays have a constructor per length. A column of array patterns only
//! distinguishes lengths up to the longest exact pattern and the longest
//! prefix before `..`; every longer array behaves alike, so the lengths are
//! `0..L` plus "at least `L`".

use crate::hir::{Literal, PatKind, Pattern};
use crate::types::{AdtKind, Payload, Ty, TyKind, Types};

#[derive(Clone, Debug, PartialEq)]
enum Ctor {
    /// The only constructor of tuples, records, newtypes and `Unit`.
    Single,
    Variant(u32),
    Bool(bool),
    /// A value of an open domain (integers, strings).
    Value(Literal),
    /// An array of exactly this length.
    Len(usize),
    /// Arrays of at least this length.
    LenAtLeast(usize),
}

#[derive(Clone, Debug)]
enum Pat {
    Wild,
    Ctor(Ctor, Vec<Pat>),
    /// `[p, q, ..]`: arrays of at least as many elements as the prefix.
    Slice(Vec<Pat>),
}

impl Pat {
    /// Whether every value built with `ctor` starts with this pattern's
    /// constructor; a wildcard covers everything.
    fn covers(&self, ctor: &Ctor) -> bool {
        match (self, ctor) {
            (Pat::Wild, _) => true,
            (Pat::Ctor(own, _), _) => own == ctor,
            (Pat::Slice(prefix), Ctor::Len(n) | Ctor::LenAtLeast(n)) => *n >= prefix.len(),
            (Pat::Slice(_), _) => false,
        }
    }
}

fn simplify(pattern: &Pattern) -> Pat {
    match &pattern.kind {
        PatKind::Wildcard | PatKind::Binding(_) => Pat::Wild,
        PatKind::Literal(Literal::Bool(b)) => Pat::Ctor(Ctor::Bool(*b), Vec::new()),
        PatKind::Literal(Literal::Unit) => Pat::Ctor(Ctor::Single, Vec::new()),
        PatKind::Literal(literal) => Pat::Ctor(Ctor::Value(literal.clone()), Vec::new()),
        PatKind::Fields(fields) => Pat::Ctor(Ctor::Single, fields.iter().map(simplify).collect()),
        PatKind::Variant(variant, fields) => Pat::Ctor(Ctor::Variant(*variant), fields.iter().map(simplify).collect()),
        PatKind::Newtype(inner) => Pat::Ctor(Ctor::Single, vec![simplify(inner)]),
        PatKind::Array { items, rest: None } => Pat::Ctor(Ctor::Len(items.len()), items.iter().map(simplify).collect()),
        PatKind::Array { items, rest: Some(_) } => Pat::Slice(items.iter().map(simplify).collect()),
    }
}

/// Component types of `ctor` for a value of type `ty`.
fn fields(types: &Types, ty: Ty, ctor: &Ctor) -> Vec<Ty> {
    match ctor {
        Ctor::Single => match types.kind(ty) {
            TyKind::Adt(id) => match &types.adt(*id).kind {
                AdtKind::Newtype(inner) => vec![*inner],
                _ => types.components(ty).unwrap_or_default(),
            },
            _ => types.components(ty).unwrap_or_default(),
        },
        Ctor::Variant(v) => types.variants(ty).map(|v_| v_[*v as usize].clone()).unwrap_or_default(),
        Ctor::Len(n) | Ctor::LenAtLeast(n) => match types.kind(types.repr(ty)) {
            TyKind::Array(element) => vec![*element; *n],
            _ => Vec::new(),
        },
        Ctor::Bool(_) | Ctor::Value(_) => Vec::new(),
    }
}

/// Every constructor of `ty`, or `None` for open domains. Array lengths
/// depend on the patterns of the column, given as `heads`.
fn all_ctors<'a>(types: &Types, ty: Ty, heads: impl Iterator<Item = &'a Pat>) -> Option<Vec<Ctor>> {
    if let TyKind::Array(_) = types.kind(types.repr(ty)) {
        let longest = heads
            .map(|head| match head {
                Pat::Ctor(Ctor::Len(n), _) => n + 1,
                Pat::Slice(prefix) => prefix.len(),
                _ => 0,
            })
            .max()
            .unwrap_or(0);
        let mut ctors: Vec<Ctor> = (0..longest).map(Ctor::Len).collect();
        ctors.push(Ctor::LenAtLeast(longest));
        return Some(ctors);
    }
    // A newtype is matched through its own constructor, whatever it wraps.
    if let TyKind::Adt(id) = types.kind(ty)
        && let AdtKind::Newtype(_) = types.adt(*id).kind
    {
        return Some(vec![Ctor::Single]);
    }
    match types.kind(types.repr(ty)) {
        TyKind::Bool => Some(vec![Ctor::Bool(false), Ctor::Bool(true)]),
        TyKind::Never => Some(Vec::new()),
        TyKind::Int(_) | TyKind::String => None,
        _ => match types.variants(ty) {
            Some(variants) => Some((0..variants.len() as u32).map(Ctor::Variant).collect()),
            None => Some(vec![Ctor::Single]),
        },
    }
}

fn specialize(rows: &[Vec<Pat>], ctor: &Ctor, arity: usize) -> Vec<Vec<Pat>> {
    rows.iter()
        .filter_map(|row| {
            let mut out = match &row[0] {
                Pat::Wild => vec![Pat::Wild; arity],
                Pat::Ctor(c, args) if c == ctor => args.clone(),
                Pat::Slice(prefix) if row[0].covers(ctor) => {
                    let mut items = prefix.clone();
                    items.resize(arity, Pat::Wild);
                    items
                }
                Pat::Ctor(..) | Pat::Slice(_) => return None,
            };
            out.extend_from_slice(&row[1..]);
            Some(out)
        })
        .collect()
}

/// A value that matches `row` and none of `rows`, if any. Each entry of
/// `tys` is the type of the corresponding column.
fn useful(types: &Types, rows: &[Vec<Pat>], row: &[Pat], tys: &[Ty]) -> Option<Vec<Pat>> {
    let Some(first) = row.first() else {
        return rows.is_empty().then(Vec::new);
    };
    let ty = tys[0];
    let rebuild = |ctor: Ctor, arity: usize, mut witness: Vec<Pat>| {
        let rest = witness.split_off(arity);
        let mut out = vec![Pat::Ctor(ctor, witness)];
        out.extend(rest);
        out
    };
    let try_ctor = |ctor: &Ctor, head: Vec<Pat>| {
        let sub = fields(types, ty, ctor);
        let arity = sub.len();
        let mut next_row = head;
        next_row.extend_from_slice(&row[1..]);
        let mut next_tys = sub;
        next_tys.extend_from_slice(&tys[1..]);
        useful(types, &specialize(rows, ctor, arity), &next_row, &next_tys).map(|w| rebuild(ctor.clone(), arity, w))
    };
    let heads = || rows.iter().map(|r| &r[0]).chain(std::iter::once(first));
    // Whether a row other than a wildcard starts with `ctor`.
    let used = |ctor: &Ctor| rows.iter().any(|r| !matches!(r[0], Pat::Wild) && r[0].covers(ctor));
    match first {
        Pat::Ctor(ctor, args) => try_ctor(ctor, args.clone()),
        Pat::Slice(prefix) => {
            let all = all_ctors(types, ty, heads()).unwrap_or_default();
            all.iter().filter(|c| first.covers(c)).find_map(|ctor| {
                let mut head = prefix.clone();
                head.resize(fields(types, ty, ctor).len(), Pat::Wild);
                try_ctor(ctor, head)
            })
        }
        Pat::Wild => {
            let all = all_ctors(types, ty, heads());
            let complete = all.as_ref().is_some_and(|all| all.iter().all(used));
            if complete {
                all.unwrap().iter().find_map(|ctor| {
                    let arity = fields(types, ty, ctor).len();
                    try_ctor(ctor, vec![Pat::Wild; arity])
                })
            } else {
                let default: Vec<Vec<Pat>> =
                    rows.iter().filter(|r| matches!(r[0], Pat::Wild)).map(|r| r[1..].to_vec()).collect();
                let missing = all.and_then(|all| all.into_iter().find(|c| !used(c)));
                useful(types, &default, &row[1..], &tys[1..]).map(|witness| {
                    let head = match missing {
                        Some(ctor) => {
                            let arity = fields(types, ty, &ctor).len();
                            Pat::Ctor(ctor, vec![Pat::Wild; arity])
                        }
                        None => Pat::Wild,
                    };
                    let mut out = vec![head];
                    out.extend(witness);
                    out
                })
            }
        }
    }
}

/// The result of checking a `match`.
pub struct MatchReport {
    /// Indices of arms that no value can reach.
    pub unreachable: Vec<usize>,
    /// A value no arm matches, rendered as a pattern.
    pub missing: Option<String>,
}

/// Checks arms in order; guarded arms never cover later ones (14.4).
pub fn check_match(types: &Types, scrutinee: Ty, arms: &[(&Pattern, bool)]) -> MatchReport {
    let mut rows: Vec<Vec<Pat>> = Vec::new();
    let mut unreachable = Vec::new();
    for (index, (pattern, guarded)) in arms.iter().enumerate() {
        let row = vec![simplify(pattern)];
        if useful(types, &rows, &row, &[scrutinee]).is_none() {
            unreachable.push(index);
        }
        if !guarded {
            rows.push(row);
        }
    }
    let missing = useful(types, &rows, &[Pat::Wild], &[scrutinee]).map(|w| render(types, scrutinee, &w[0]));
    MatchReport { unreachable, missing }
}

/// A value of `ty` the irrefutable `pattern` does not match, if any.
pub fn refutation(types: &Types, ty: Ty, pattern: &Pattern) -> Option<String> {
    useful(types, &[vec![simplify(pattern)]], &[Pat::Wild], &[ty]).map(|w| render(types, ty, &w[0]))
}

fn render(types: &Types, ty: Ty, pat: &Pat) -> String {
    let Pat::Ctor(ctor, args) = pat else { return "_".into() };
    let parts = |sub: Vec<Ty>| -> Vec<String> { sub.iter().zip(args).map(|(&t, p)| render(types, t, p)).collect() };
    match ctor {
        Ctor::Bool(b) => b.to_string(),
        Ctor::Value(_) => "_".into(),
        Ctor::Len(n) | Ctor::LenAtLeast(n) => {
            let mut items = parts(fields(types, ty, ctor));
            if matches!(ctor, Ctor::LenAtLeast(_)) {
                if *n == 0 {
                    return "_".into();
                }
                items.push("..".into());
            }
            format!("[{}]", items.join(", "))
        }
        Ctor::Variant(v) => {
            match (types.kind(types.repr(ty)), *v) {
                (TyKind::Option(_), 0) => return "none".into(),
                (TyKind::Option(inner), _) => return format!("some({})", parts(vec![*inner]).join("")),
                (TyKind::Result(ok, _), 0) => return format!("ok({})", parts(vec![*ok]).join("")),
                (TyKind::Result(_, err), _) => return format!("err({})", parts(vec![*err]).join("")),
                (TyKind::Union(members), v) => {
                    // A nominal member names itself; others are tested with
                    // `Member(pattern)` (9.6).
                    let member = members[v as usize];
                    let inner = parts(vec![member]).join("");
                    return match inner.as_str() {
                        "_" => format!("{}(_)", types.display(member)),
                        _ => inner,
                    };
                }
                _ => {}
            }
            let TyKind::Adt(id) = types.kind(types.repr(ty)) else { return "_".into() };
            let adt = types.adt(*id);
            let AdtKind::Enum(variants) = &adt.kind else { return "_".into() };
            let variant = &variants[*v as usize];
            let name = format!("{}.{}", types.pattern_name(*id), variant.name);
            match &variant.payload {
                Payload::Unit => name,
                Payload::Tuple(items) => format!("{name}({})", parts(items.clone()).join(", ")),
                Payload::Record(_) => format!("{name} {{ .. }}"),
            }
        }
        Ctor::Single => match types.kind(ty) {
            TyKind::Unit => "()".into(),
            TyKind::Tuple(items) => format!("({})", parts(items.clone()).join(", ")),
            TyKind::Adt(id) => match &types.adt(*id).kind {
                AdtKind::Newtype(inner) => format!("{}({})", types.pattern_name(*id), parts(vec![*inner]).join("")),
                AdtKind::Record(fields) => {
                    let names = fields.iter().map(|f| f.name.clone());
                    let values = parts(fields.iter().map(|f| f.ty).collect());
                    let shown: Vec<String> =
                        names.zip(values).filter(|(_, v)| v != "_").map(|(n, v)| format!("{n}: {v}")).collect();
                    if shown.is_empty() {
                        format!("{} {{ .. }}", types.pattern_name(*id))
                    } else {
                        format!("{} {{ {}, .. }}", types.pattern_name(*id), shown.join(", "))
                    }
                }
                AdtKind::Enum(_) => "_".into(),
            },
            _ => "_".into(),
        },
    }
}
