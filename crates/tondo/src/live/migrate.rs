//! Data migration (toolchain 7.10): a patch that changes what a type is
//! made of converts the values of the old shape that a frame holds as the
//! frame moves to the new code. Values have no identity (4.1 of the
//! language), so a converted copy is the value. The patch rebuilds each old
//! type from the running build's manifest, as a type of its own of the same
//! name, and a function that converts a value of it, or gives `none`.

use crate::hir::{
    Arm, Block, Callee, Expr, ExprKind, FnId, Function, Local, LocalId, Mutation, PatKind, Pattern, Place, Program,
    Stmt,
};
use crate::source::Span;
use crate::types::{AdtKind, Field, Payload, Ty, TyKind, Types};
use serde_json::Value;
use std::collections::HashMap;

/// What a patch does with values of the old types frames hold: the
/// function that converts those of each old type, by its key, with the key
/// of the new type; why values of others do not convert; and what
/// converting does to them.
#[derive(Default)]
pub(super) struct Migrations {
    pub conversions: HashMap<String, (FnId, String)>,
    pub refused: HashMap<String, String>,
    pub notes: Vec<String>,
}

/// Adds to the program the conversions of the old types `wanted`, by their
/// keys, which `structures` describes.
pub(super) fn add(program: &mut Program, structures: &serde_json::Map<String, Value>, wanted: &[String]) -> Migrations {
    let mut migrations = Migrations::default();
    let mut rebuild = Rebuild::new(&mut program.types, structures);
    let mut pairs = Vec::new();
    for key in wanted {
        match (rebuild.old(key), rebuild.now(key)) {
            (Some(old), Some(new)) if old != new => pairs.push((key.clone(), old, new)),
            (Some(_), Some(_)) => {}
            _ => {
                let shown = key.split(" @").next().unwrap_or(key);
                migrations.refused.insert(key.clone(), format!("`{shown}` is not a type of the new code"));
            }
        }
    }
    let mut plan = Plan { types: &program.types, checked: HashMap::new(), notes: Vec::new() };
    let pairs: Vec<(String, Ty, Ty)> = pairs
        .into_iter()
        .filter(|(key, old, new)| match plan.check(*old, *new) {
            Ok(()) => true,
            Err(why) => {
                migrations.refused.insert(key.clone(), why);
                false
            }
        })
        .collect();
    let (checked, notes) = (plan.checked, plan.notes);
    migrations.notes = notes;
    let mut generate = Generate { program, checked, memo: HashMap::new() };
    for (key, old, new) in pairs {
        let function = generate.function(old, new);
        let new = super::type_key(&generate.program.types, new);
        migrations.conversions.insert(key, (function, new));
    }
    migrations
}

/// The types of the running program, rebuilt from its manifest: those that
/// are the same now are the new ones; the others, types of their own.
struct Rebuild<'a> {
    types: &'a mut Types,
    structures: &'a serde_json::Map<String, Value>,
    /// The new program's types by how they show, and its nominal types by
    /// name.
    shown: HashMap<String, Ty>,
    named: HashMap<String, Ty>,
    ghosts: HashMap<String, Ty>,
}

impl<'a> Rebuild<'a> {
    fn new(types: &'a mut Types, structures: &'a serde_json::Map<String, Value>) -> Rebuild<'a> {
        let mut shown = HashMap::new();
        let mut named = HashMap::new();
        for ty in types.all() {
            if let TyKind::Adt(id) = types.kind(ty) {
                named.entry(super::adt_name(types, *id)).or_insert(ty);
            }
            if !matches!(types.kind(ty), TyKind::Param(_) | TyKind::Opaque(..) | TyKind::Error) {
                shown.entry(types.display(ty)).or_insert(ty);
            }
        }
        Rebuild { types, structures, shown, named, ghosts: HashMap::new() }
    }

    /// The old type of a key: a type of the new program when it is the
    /// same, or one rebuilt as it was.
    fn old(&mut self, key: &str) -> Option<Ty> {
        if let Some(&ghost) = self.ghosts.get(key) {
            return Some(ghost);
        }
        let structure = self.structures.get(key)?.clone();
        if let Some(name) = structure["adt"].as_str() {
            if let Some(&now) = self.named.get(name)
                && super::type_key(self.types, now) == key
            {
                return Some(now);
            }
            let (id, ghost) = self.types.declare_adt(name.to_string());
            self.ghosts.insert(key.to_string(), ghost);
            let kind = self.adt_kind(&structure, &mut |r: &mut Self, key| r.old(key))?;
            self.types.define_adt(id, kind);
            return Some(ghost);
        }
        self.composite(&structure, key, &mut |r: &mut Self, key| r.old(key))
    }

    /// The type of the new program a value of the old type of a key becomes:
    /// the nominal types of the same names.
    fn now(&mut self, key: &str) -> Option<Ty> {
        let structure = self.structures.get(key)?.clone();
        if let Some(name) = structure["adt"].as_str() {
            return self.named.get(name).copied();
        }
        self.composite(&structure, key, &mut |r: &mut Self, key| r.now(key))
    }

    fn composite(
        &mut self,
        structure: &Value,
        key: &str,
        each: &mut dyn FnMut(&mut Self, &str) -> Option<Ty>,
    ) -> Option<Ty> {
        let parts = |r: &mut Self, name: &str, each: &mut dyn FnMut(&mut Self, &str) -> Option<Ty>| {
            let keys = structure[name].as_array()?.clone();
            keys.iter().map(|k| each(r, k.as_str()?)).collect::<Option<Vec<Ty>>>()
        };
        let kind = if let Some(items) = parts(self, "option", each) {
            TyKind::Option(items[0])
        } else if let Some(items) = parts(self, "array", each) {
            TyKind::Array(items[0])
        } else if let Some(items) = parts(self, "deque", each) {
            TyKind::Deque(items[0])
        } else if let Some(items) = parts(self, "set", each) {
            TyKind::Set(items[0])
        } else if let Some(items) = parts(self, "map", each) {
            TyKind::Map(items[0], items[1])
        } else if let Some(items) = parts(self, "result", each) {
            TyKind::Result(items[0], items[1])
        } else if let Some(items) = parts(self, "tuple", each) {
            TyKind::Tuple(items)
        } else {
            // A type that does not change shape is the new program's.
            return self.shown.get(key).copied();
        };
        Some(self.types.intern(kind))
    }

    fn adt_kind(&mut self, structure: &Value, each: &mut dyn FnMut(&mut Self, &str) -> Option<Ty>) -> Option<AdtKind> {
        let fields = |r: &mut Self, list: &Value, each: &mut dyn FnMut(&mut Self, &str) -> Option<Ty>| {
            list.as_array()?
                .iter()
                .map(|field| {
                    let ty = each(r, field[1].as_str()?)?;
                    Some(Field { name: field[0].as_str()?.to_string(), ty, private: field[2].as_bool()? })
                })
                .collect::<Option<Vec<Field>>>()
        };
        if structure.get("record").is_some() {
            return Some(AdtKind::Record(fields(self, &structure["record"], each)?));
        }
        if let Some(inner) = structure["newtype"].as_str() {
            return Some(AdtKind::Newtype(each(self, inner)?));
        }
        let variants = structure["enum"].as_array()?.clone();
        let mut out = Vec::new();
        for variant in &variants {
            let payload = if let Some(items) = variant["tuple"].as_array() {
                Payload::Tuple(items.iter().map(|k| each(self, k.as_str()?)).collect::<Option<Vec<Ty>>>()?)
            } else if variant.get("record").is_some() {
                Payload::Record(fields(self, &variant["record"], each)?)
            } else {
                Payload::Unit
            };
            out.push(crate::types::Variant { name: variant["name"].as_str()?.to_string(), payload });
        }
        Some(AdtKind::Enum(out))
    }
}

/// Which old types convert to the new ones, and what converting does.
struct Plan<'a> {
    types: &'a Types,
    checked: HashMap<(Ty, Ty), Result<(), String>>,
    notes: Vec<String>,
}

impl Plan<'_> {
    fn check(&mut self, old: Ty, new: Ty) -> Result<(), String> {
        if old == new {
            return Ok(());
        }
        if let Some(done) = self.checked.get(&(old, new)) {
            return done.clone();
        }
        // A recursive type converts if the rest of it does.
        self.checked.insert((old, new), Ok(()));
        let done = self.check_kinds(old, new);
        self.checked.insert((old, new), done.clone());
        done
    }

    /// Whether a value of `old` becomes one of `new` itself, or as its `some`.
    fn fits(&mut self, old: Ty, new: Ty) -> bool {
        self.check(old, new).is_ok()
            || matches!(*self.types.kind(new), TyKind::Option(inner) if self.check(old, inner).is_ok())
    }

    fn check_kinds(&mut self, old: Ty, new: Ty) -> Result<(), String> {
        let types = self.types;
        match (types.kind(old).clone(), types.kind(new).clone()) {
            (TyKind::Adt(a), TyKind::Adt(b)) => {
                let name = types.display(new);
                match (types.adt(a).kind.clone(), types.adt(b).kind.clone()) {
                    (AdtKind::Record(from), AdtKind::Record(to)) => self.fields(&name, &from, &to),
                    (AdtKind::Newtype(from), AdtKind::Newtype(to)) => match self.fits(from, to) {
                        true => Ok(()),
                        false => Err(format!("`{name}` wraps another type now")),
                    },
                    (AdtKind::Enum(from), AdtKind::Enum(to)) => {
                        for variant in &from {
                            let stays = "a frame holding one stays on the old code";
                            match to.iter().find(|v| v.name == variant.name) {
                                None => self.notes.push(format!("`{name}.{}` is gone: {stays}", variant.name)),
                                Some(target) if !self.payload(&name, &variant.payload, &target.payload) => self
                                    .notes
                                    .push(format!("`{name}.{}` carries something else: {stays}", variant.name)),
                                Some(_) => {}
                            }
                        }
                        Ok(())
                    }
                    _ => Err(format!("`{name}` is another kind of type now")),
                }
            }
            (TyKind::Option(a), TyKind::Option(b))
            | (TyKind::Array(a), TyKind::Array(b))
            | (TyKind::Deque(a), TyKind::Deque(b))
            | (TyKind::Set(a), TyKind::Set(b)) => self.check(a, b),
            (TyKind::Map(k, v), TyKind::Map(j, w)) | (TyKind::Result(k, v), TyKind::Result(j, w)) => {
                self.check(k, j)?;
                self.check(v, w)
            }
            (TyKind::Tuple(from), TyKind::Tuple(to)) if from.len() == to.len() => {
                from.iter().zip(&to).try_for_each(|(&a, &b)| self.check(a, b))
            }
            _ => Err(format!("a `{}` does not convert to a `{}`", types.display(old), types.display(new))),
        }
    }

    /// A record's fields, by name: those of the same type or of a type that
    /// converts keep their values, and the new ones, and those of another
    /// type, get the empty value of their type.
    fn fields(&mut self, name: &str, from: &[Field], to: &[Field]) -> Result<(), String> {
        for field in to {
            let empty = empty_text(self.types, field.ty);
            match from.iter().find(|f| f.name == field.name) {
                Some(old) if self.fits(old.ty, field.ty) => {}
                Some(_) => match empty {
                    Some(empty) => {
                        self.notes.push(format!("`{name}.{}` has another type: values alive get {empty}", field.name))
                    }
                    None => {
                        return Err(format!(
                            "`{name}.{}` has another type, which has no empty value to start with",
                            field.name
                        ));
                    }
                },
                None => match empty {
                    Some(empty) => self.notes.push(format!("`{name}.{}` is new: values alive get {empty}", field.name)),
                    None => {
                        let shown = self.types.display(field.ty);
                        return Err(format!(
                            "`{name}.{}` is new and `{shown}` has no empty value to start with: make it `{shown}?`",
                            field.name
                        ));
                    }
                },
            }
        }
        for field in from.iter().filter(|f| !to.iter().any(|t| t.name == f.name)) {
            self.notes.push(format!("`{name}.{}` is gone", field.name));
        }
        Ok(())
    }

    /// Whether a variant's payload converts.
    fn payload(&mut self, name: &str, from: &Payload, to: &Payload) -> bool {
        match (from, to) {
            (Payload::Unit, Payload::Unit) => true,
            (Payload::Tuple(a), Payload::Tuple(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| self.fits(x, y))
            }
            (Payload::Record(a), Payload::Record(b)) => self.fields(name, a, b).is_ok(),
            _ => false,
        }
    }
}

/// The empty value of a type, as a report shows it, if it has one.
fn empty_text(types: &Types, ty: Ty) -> Option<String> {
    Some(match types.kind(ty) {
        TyKind::Int(_) | TyKind::Byte => "0".into(),
        TyKind::Float(_) => "0.0".into(),
        TyKind::Bool => "false".into(),
        TyKind::String => "\"\"".into(),
        TyKind::Unit => "()".into(),
        TyKind::Option(_) => "none".into(),
        TyKind::Array(_) | TyKind::Deque(_) => "[]".into(),
        TyKind::Set(_) => "Set[]".into(),
        TyKind::Map(..) => "[:]".into(),
        TyKind::Tuple(items) => {
            let items: Option<Vec<String>> = items.iter().map(|&t| empty_text(types, t)).collect();
            format!("({})", items?.join(", "))
        }
        TyKind::Adt(id) => match &types.adt(*id).kind {
            AdtKind::Record(fields) => {
                fields.iter().try_for_each(|f| empty_text(types, f.ty).map(|_| ()))?;
                format!("an empty `{}`", types.display(ty))
            }
            AdtKind::Newtype(inner) => empty_text(types, *inner)?,
            AdtKind::Enum(_) => return None,
        },
        _ => return None,
    })
}

/// The conversion functions, written as the checker writes a program.
struct Generate<'a> {
    program: &'a mut Program,
    checked: HashMap<(Ty, Ty), Result<(), String>>,
    memo: HashMap<(Ty, Ty), FnId>,
}

/// A conversion function being written: its locals and statements.
struct Body {
    locals: Vec<Local>,
    stmts: Vec<Stmt>,
}

impl Body {
    fn local(&mut self, name: &str, ty: Ty) -> LocalId {
        self.locals.push(Local { name: format!("<{name}>"), ty, span: Span::default() });
        LocalId(self.locals.len() as u32 - 1)
    }
}

fn expr(kind: ExprKind, ty: Ty) -> Expr {
    Expr { kind, ty, span: Span::default() }
}

fn pattern(kind: PatKind, ty: Ty) -> Pattern {
    Pattern { kind, ty, span: Span::default() }
}

impl Generate<'_> {
    fn converts(&self, old: Ty, new: Ty) -> bool {
        old == new || self.checked.get(&(old, new)).is_some_and(Result::is_ok)
    }

    /// The function that converts a value of `old` to `new`, or gives `none`.
    fn function(&mut self, old: Ty, new: Ty) -> FnId {
        if let Some(&id) = self.memo.get(&(old, new)) {
            return id;
        }
        let option = self.program.types.intern(TyKind::Option(new));
        let id = FnId(self.program.functions.len() as u32);
        let name = format!("migrate {}", self.program.types.display(new));
        let empty = Block { stmts: Vec::new(), tail: None, ty: option };
        self.program.functions.push(Function {
            name,
            generics: Vec::new(),
            params: vec![LocalId(0)],
            loans: vec![false],
            locals: Vec::new(),
            result: option,
            fallible: false,
            body: empty,
            moves: Default::default(),
            test: false,
        });
        self.memo.insert((old, new), id);
        let mut body = Body { locals: Vec::new(), stmts: Vec::new() };
        let param = body.local("old", old);
        let tail = self.convert(&mut body, expr(ExprKind::Local(param), old), old, new);
        let function = &mut self.program.functions[id.0 as usize];
        function.locals = body.locals;
        function.body = Block { stmts: body.stmts, tail: Some(Box::new(tail)), ty: option };
        id
    }

    fn some(&mut self, value: Expr) -> Expr {
        let ty = self.program.types.intern(TyKind::Option(value.ty));
        expr(ExprKind::Variant(1, vec![(0, value)]), ty)
    }

    /// `value` as a value of `new`: itself, converted, its `some`, or the
    /// empty value of `new`, as the plan says.
    fn value(&mut self, value: Expr, old: Ty, new: Ty) -> Expr {
        if old == new {
            return value;
        }
        if self.converts(old, new) {
            let function = self.function(old, new);
            let option = self.program.types.intern(TyKind::Option(new));
            let call = expr(ExprKind::Call(Callee::Fn(function), Vec::new(), vec![value]), option);
            return expr(ExprKind::Try(Box::new(call)), new);
        }
        if let TyKind::Option(inner) = *self.program.types.kind(new)
            && self.converts(old, inner)
        {
            let inner = self.value(value, old, inner);
            return self.some(inner);
        }
        self.empty(new)
    }

    fn empty(&mut self, ty: Ty) -> Expr {
        let types = &self.program.types;
        let kind = match types.kind(ty).clone() {
            TyKind::Int(_) | TyKind::Byte => ExprKind::Int(0),
            TyKind::Float(_) => ExprKind::Float(0.0),
            TyKind::Bool => ExprKind::Bool(false),
            TyKind::String => ExprKind::Str(String::new()),
            TyKind::Unit => ExprKind::Unit,
            TyKind::Option(_) => ExprKind::Variant(0, Vec::new()),
            TyKind::Array(_) | TyKind::Deque(_) => ExprKind::ArrayOp(crate::hir::ArrayOp::New, Vec::new()),
            TyKind::Set(_) => ExprKind::Set(Vec::new()),
            TyKind::Map(..) => ExprKind::Map(Vec::new()),
            TyKind::Tuple(items) => ExprKind::Aggregate(items.iter().map(|&t| self.empty(t)).collect()),
            TyKind::Adt(id) => match types.adt(id).kind.clone() {
                AdtKind::Record(fields) => {
                    ExprKind::Record(fields.iter().enumerate().map(|(i, f)| (i as u32, self.empty(f.ty))).collect())
                }
                AdtKind::Newtype(inner) => ExprKind::Retype(Box::new(self.empty(inner))),
                AdtKind::Enum(_) => unreachable!("the plan gives no enum an empty value"),
            },
            _ => unreachable!("the plan gives this type no empty value"),
        };
        expr(kind, ty)
    }

    /// The new record's fields from the old one's, by name.
    fn fields(&mut self, read: &dyn Fn(usize) -> Expr, from: &[Field], to: &[Field]) -> Vec<(u32, Expr)> {
        to.iter()
            .enumerate()
            .map(|(index, field)| {
                let value = match from.iter().position(|f| f.name == field.name) {
                    Some(old) => self.value(read(old), from[old].ty, field.ty),
                    None => self.empty(field.ty),
                };
                (index as u32, value)
            })
            .collect()
    }

    /// The body's value: `old` as `some` of a value of `new`, or `none`.
    fn convert(&mut self, body: &mut Body, old: Expr, from: Ty, to: Ty) -> Expr {
        let option = self.program.types.intern(TyKind::Option(to));
        let types = &self.program.types;
        let field = |base: &Expr, index: usize, ty: Ty| expr(ExprKind::Field(Box::new(base.clone()), index as u32), ty);
        let converted = match (types.kind(from).clone(), types.kind(to).clone()) {
            (TyKind::Adt(a), TyKind::Adt(b)) => match (types.adt(a).kind.clone(), types.adt(b).kind.clone()) {
                (AdtKind::Record(old_fields), AdtKind::Record(new_fields)) => {
                    let read = |index: usize| field(&old, index, old_fields[index].ty);
                    let fields = self.fields(&read, &old_fields, &new_fields);
                    expr(ExprKind::Record(fields), to)
                }
                (AdtKind::Newtype(inner), AdtKind::Newtype(target)) => {
                    let unwrapped = expr(ExprKind::Retype(Box::new(old)), inner);
                    let value = self.value(unwrapped, inner, target);
                    expr(ExprKind::Retype(Box::new(value)), to)
                }
                (AdtKind::Enum(old_variants), AdtKind::Enum(new_variants)) => {
                    return self.variants(body, old, from, to, &old_variants, &new_variants);
                }
                _ => unreachable!("the plan converts records, newtypes and enums"),
            },
            (TyKind::Option(a), TyKind::Option(b)) => {
                let x = body.local("some", a);
                let value = self.value(expr(ExprKind::Local(x), a), a, b);
                let arms = vec![
                    (pattern(PatKind::Variant(0, Vec::new()), from), expr(ExprKind::Variant(0, Vec::new()), to)),
                    (
                        pattern(PatKind::Variant(1, vec![pattern(PatKind::Binding(x), a)]), from),
                        expr(ExprKind::Variant(1, vec![(0, value)]), to),
                    ),
                ];
                return self.choose(old, arms, option);
            }
            (TyKind::Result(a, e), TyKind::Result(b, f)) => {
                let (x, y) = (body.local("ok", a), body.local("err", e));
                let ok = self.value(expr(ExprKind::Local(x), a), a, b);
                let err = self.value(expr(ExprKind::Local(y), e), e, f);
                let arms = vec![
                    (
                        pattern(PatKind::Variant(0, vec![pattern(PatKind::Binding(x), a)]), from),
                        expr(ExprKind::Variant(0, vec![(0, ok)]), to),
                    ),
                    (
                        pattern(PatKind::Variant(1, vec![pattern(PatKind::Binding(y), e)]), from),
                        expr(ExprKind::Variant(1, vec![(0, err)]), to),
                    ),
                ];
                return self.choose(old, arms, option);
            }
            (TyKind::Tuple(items), TyKind::Tuple(targets)) => {
                let values = items.iter().zip(&targets).enumerate().map(|(i, (&a, &b))| (i, a, b)).collect::<Vec<_>>();
                let values = values.into_iter().map(|(i, a, b)| self.value(field(&old, i, a), a, b)).collect();
                expr(ExprKind::Aggregate(values), to)
            }
            (TyKind::Array(a), TyKind::Array(b)) | (TyKind::Deque(a), TyKind::Deque(b)) => {
                let item = body.local("item", a);
                let value = self.value(expr(ExprKind::Local(item), a), a, b);
                self.collect(body, old, pattern(PatKind::Binding(item), a), to, Mutation::Push, vec![value], Ty::UNIT)
            }
            (TyKind::Set(a), TyKind::Set(b)) => {
                let item = body.local("item", a);
                let value = self.value(expr(ExprKind::Local(item), a), a, b);
                self.collect(body, old, pattern(PatKind::Binding(item), a), to, Mutation::Insert, vec![value], Ty::BOOL)
            }
            (TyKind::Map(k, v), TyKind::Map(j, w)) => {
                let (key, value) = (body.local("key", k), body.local("value", v));
                let entry = self.program.types.intern(TyKind::Tuple(vec![k, v]));
                let entry = pattern(
                    PatKind::Fields(vec![pattern(PatKind::Binding(key), k), pattern(PatKind::Binding(value), v)]),
                    entry,
                );
                let key = self.value(expr(ExprKind::Local(key), k), k, j);
                let value = self.value(expr(ExprKind::Local(value), v), v, w);
                let replaced = self.program.types.intern(TyKind::Option(w));
                self.collect(body, old, entry, to, Mutation::Insert, vec![key, value], replaced)
            }
            _ => unreachable!("the plan converts only these"),
        };
        self.some(converted)
    }

    /// A match of `old` with these arms, each giving its `some`.
    fn choose(&mut self, old: Expr, arms: Vec<(Pattern, Expr)>, option: Ty) -> Expr {
        let arms =
            arms.into_iter().map(|(pattern, value)| Arm { pattern, guard: None, body: self.some(value) }).collect();
        expr(ExprKind::Match(Box::new(old), arms), option)
    }

    /// An enum's variants, by name: a value of one the new type has not, or
    /// whose payload does not convert, gives `none`.
    fn variants(
        &mut self,
        body: &mut Body,
        old: Expr,
        from: Ty,
        to: Ty,
        old_variants: &[crate::types::Variant],
        new_variants: &[crate::types::Variant],
    ) -> Expr {
        let option = self.program.types.intern(TyKind::Option(to));
        let mut arms = Vec::new();
        for (index, variant) in old_variants.iter().enumerate() {
            let parts = variant.payload.types();
            let locals: Vec<LocalId> = parts.iter().map(|&t| body.local("payload", t)).collect();
            let patterns = locals.iter().zip(&parts).map(|(&l, &t)| pattern(PatKind::Binding(l), t)).collect();
            let pattern = pattern(PatKind::Variant(index as u32, patterns), from);
            let read = |i: usize| expr(ExprKind::Local(locals[i]), parts[i]);
            let target = new_variants.iter().position(|v| v.name == variant.name);
            let value = match target.map(|t| (t, &new_variants[t].payload)) {
                Some((t, Payload::Unit)) if matches!(variant.payload, Payload::Unit) => {
                    self.some(expr(ExprKind::Variant(t as u32, Vec::new()), to))
                }
                Some((t, Payload::Tuple(targets)))
                    if matches!(&variant.payload, Payload::Tuple(items) if items.len() == targets.len()
                        && items.iter().zip(targets).all(|(&a, &b)| self.fits(a, b))) =>
                {
                    let values =
                        parts.iter().zip(targets).enumerate().map(|(i, (&a, &b))| (i, a, b)).collect::<Vec<_>>();
                    let values = values.into_iter().map(|(i, a, b)| (i as u32, self.value(read(i), a, b))).collect();
                    self.some(expr(ExprKind::Variant(t as u32, values), to))
                }
                Some((t, Payload::Record(targets)))
                    if matches!(&variant.payload, Payload::Record(_))
                        && self.record_fits(&variant.payload, targets) =>
                {
                    let Payload::Record(fields) = &variant.payload else { unreachable!() };
                    let values = self.fields(&read, fields, targets);
                    self.some(expr(ExprKind::Variant(t as u32, values), to))
                }
                _ => expr(ExprKind::Variant(0, Vec::new()), option),
            };
            arms.push(Arm { pattern, guard: None, body: value });
        }
        expr(ExprKind::Match(Box::new(old), arms), option)
    }

    fn fits(&self, old: Ty, new: Ty) -> bool {
        self.converts(old, new)
            || matches!(*self.program.types.kind(new), TyKind::Option(inner) if self.converts(old, inner))
    }

    /// Whether a record payload converts: as a record does, its new fields
    /// and those of another type getting empty values.
    fn record_fits(&self, from: &Payload, to: &[Field]) -> bool {
        let Payload::Record(from) = from else { return false };
        to.iter().all(|field| match from.iter().find(|f| f.name == field.name) {
            Some(old) if self.fits(old.ty, field.ty) => true,
            _ => empty_text(&self.program.types, field.ty).is_some(),
        })
    }

    /// A new collection of the converted items of `old`.
    #[allow(clippy::too_many_arguments)]
    fn collect(
        &mut self,
        body: &mut Body,
        old: Expr,
        item: Pattern,
        to: Ty,
        mutation: Mutation,
        values: Vec<Expr>,
        result: Ty,
    ) -> Expr {
        let out = body.local("out", to);
        let empty = self.empty(to);
        body.stmts.push(Stmt::Let(out, empty));
        let place = Place { local: out, path: Vec::new() };
        let add = expr(ExprKind::Mutate(mutation, place, values), result);
        let each = Block { stmts: vec![Stmt::Expr(add)], tail: None, ty: Ty::UNIT };
        body.stmts.push(Stmt::ForEach { pattern: item, source: old, body: each });
        expr(ExprKind::Local(out), to)
    }
}
