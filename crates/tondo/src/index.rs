//! What an editor needs to know of a checked program: every declaration,
//! and every place a name refers to one. The checker fills it only when the
//! language server asks for it.

use crate::source::Span;
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DefKind {
    Function,
    Method,
    Type,
    Enum,
    Trait,
    Field,
    Variant,
    Const,
    Local,
    Parameter,
    Module,
}

/// A declaration: its name as written, the identifier and the whole
/// declaration, and for a binding the type it has.
#[derive(Clone, Debug)]
pub struct Def {
    pub name: String,
    pub kind: DefKind,
    pub name_span: Span,
    pub decl_span: Span,
    pub ty: Option<String>,
}

/// How the checker finds a declaration again: by the names it uses.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Key {
    Type(String),
    Value(String),
    Fn(u32),
    Param(u32, String),
    Field(String, String),
    Variant(String, String),
    Trait(u32),
    TraitMethod(u32, String),
    Module(String),
}

/// A field or method a value has, for completion: its declaration when
/// the program has one, or the intrinsic type that documents it.
#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub method: bool,
    /// A field's type.
    pub ty: Option<String>,
    pub def: Option<usize>,
    /// `Array`, `String`: the intrinsic type whose documentation has it.
    pub builtin: Option<String>,
}

/// A type the program's expressions have, and its members.
#[derive(Clone, Debug)]
pub struct Shape {
    pub name: String,
    pub members: Vec<Member>,
}

#[derive(Default)]
pub struct Index {
    pub defs: Vec<Def>,
    /// Each name that refers to a declaration, its own included.
    pub uses: Vec<(Span, usize)>,
    /// Fields written by the name of a binding, `User { name }`: renaming
    /// either one writes both.
    pub shorthands: Vec<Span>,
    /// The type of each expression of the program's own files.
    pub types: Vec<(Span, usize)>,
    pub shapes: Vec<Shape>,
    /// Each `impl`: the declaration of its trait, when the program has it,
    /// where the `impl` is, and the name of the type it is for.
    pub impls: Vec<(Option<usize>, Span, String)>,
    /// Each method of an `impl`: the trait method it implements, and its
    /// name.
    pub implemented: Vec<(usize, Span)>,
    keys: HashMap<Key, usize>,
}

impl Index {
    /// Records a declaration, found again by `key` when given.
    pub fn define(&mut self, key: Option<Key>, def: Def) -> usize {
        let id = self.defs.len();
        if let Some(key) = key {
            if let Some(&known) = self.keys.get(&key) {
                return known;
            }
            self.keys.insert(key, id);
        }
        if def.name_span != Span::default() {
            self.uses.push((def.name_span, id));
        }
        self.defs.push(def);
        id
    }

    /// Finds `def` under `key` too, unless something is there already.
    pub fn alias(&mut self, key: Key, def: usize) {
        self.keys.entry(key).or_insert(def);
    }

    pub fn of(&self, key: &Key) -> Option<usize> {
        self.keys.get(key).copied()
    }

    pub fn refer(&mut self, span: Span, def: usize) {
        if span != Span::default() {
            self.uses.push((span, def));
        }
    }

    /// The declaration a name at `offset` refers to.
    pub fn at(&self, offset: u32) -> Option<usize> {
        self.uses.iter().filter(|(span, _)| span.start <= offset && offset <= span.end).map(|&(_, def)| def).next()
    }

    /// Every name that refers to `def`, its declaration included, once each.
    pub fn references(&self, def: usize) -> Vec<Span> {
        let mut spans: Vec<Span> = self.uses.iter().filter(|(_, d)| *d == def).map(|&(span, _)| span).collect();
        spans.sort_by_key(|span| (span.start, span.end));
        spans.dedup();
        spans
    }
}
