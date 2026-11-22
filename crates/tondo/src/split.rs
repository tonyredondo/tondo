//! Records, tuples and enums kept as a local for each of their fields,
//! which the code generator keeps in registers, rather than as values in
//! the frame's memory. A lazy chain, inlined, made `some(item)` in each
//! adapter's `next` and took it apart in its caller: every element was
//! copied whole through the frame three or four times, and released at
//! each step, where the copies are now moves between registers.
//!
//! A local is split when the function never lends it, nor reads it whole
//! but to copy it: a record read whole would be put together again in
//! memory, costing what splitting saves, and an enum would need its tag to
//! say how; a copy into a place that is not split writes each field. Its
//! fields that are records or tuples are split in turn; an enum inside one
//! stays a field. A local split from an enum keeps the tag as a local of
//! its own, and every field of the variants it does not hold is zero, so
//! releasing the enum releases each field. Parameters and the result stay
//! as they are.
//!
//! A binding of the program is split too: each field's local takes its
//! name, `order.id`, and the table of a Release build's dumps names the
//! records it was (`mir::Split`), from which a debugger, a breakpoint's
//! condition and a dump show it whole, its secrets kept. One that is an
//! enum stays whole: nothing shows an enum from its tag.
//!
//! A local the split leaves zero, as the field an enum's value moved out
//! of, needs no release where every path to the release left it so: those
//! go.

use crate::mir::*;
use crate::source::Span;
use crate::types::{Ty, TyKind, Types};
use std::collections::HashMap;

/// The most locals one is split into.
const LEAVES: usize = 32;

pub fn split(bodies: &mut [Body], types: &Types, sources: &crate::source::SourceMap) {
    // Which parameters of each function are loans: the caller's place is
    // lent, not copied.
    let loans: Vec<Vec<bool>> = bodies.iter().map(|body| body.loans.clone()).collect();
    crate::parallel::each_mut(bodies, |body| {
        // The program's own bindings, which a dump shows: an enum among them
        // stays whole, as a dump shows a record from its fields but not an
        // enum from its tag.
        let std = |span: Span| sources.file_of(span.start).logical.starts_with("<std>/");
        let mut bindings = Vec::new();
        if !std(body.span) {
            bindings.extend(body.names.iter().enumerate().filter(|(_, n)| n.is_some()).map(|(i, _)| Local(i as u32)));
        }
        for (local, _, call) in &body.scoped {
            if body.inlined.get(*call as usize).is_some_and(|inlined| !std(inlined.span)) {
                bindings.push(*local);
            }
        }
        split_in(body, types, &loans, &bindings)
    });
}

/// What a local of a type is split into.
#[derive(Clone)]
enum Shape {
    Leaf(Ty),
    Record(Ty, Vec<Shape>),
    /// The tag's type, and each variant's fields.
    Enum(Ty, Vec<Vec<Shape>>),
}

impl Shape {
    fn of(types: &Types, ty: Ty, top: bool) -> Shape {
        let repr = types.repr(ty);
        // C's layout, a union's fields sharing their memory, and a
        // closure's captures, which are only read and gain nothing in
        // registers: as they are.
        if let TyKind::Adt(id) = types.kind(repr)
            && (types.foreign.contains_key(id) || types.is_closure(repr))
        {
            return Shape::Leaf(ty);
        }
        let boxed = |parts: &[Ty]| parts.iter().any(|&part| types.is_boxed(repr, part));
        if let Some(items) = types.components(repr)
            && !boxed(&items)
        {
            return Shape::Record(ty, items.iter().map(|&item| Shape::of(types, item, false)).collect());
        }
        if top
            && !matches!(types.kind(repr), TyKind::Union(_))
            && let Some(variants) = types.variants(repr)
            && !boxed(&variants.concat())
        {
            let variants: Vec<Vec<Shape>> =
                variants.iter().map(|fields| fields.iter().map(|&f| Shape::of(types, f, false)).collect()).collect();
            // Every field of every variant is a local of its own, which
            // a copy copies and making another variant zeroes: worth it
            // only where each is a word in a register.
            if variants.iter().flatten().all(|shape| shape.in_registers(types)) {
                return Shape::Enum(types.tag_ty(repr), variants);
            }
        }
        Shape::Leaf(ty)
    }

    /// Whether every local it is split into holds a number or a pointer.
    fn in_registers(&self, types: &Types) -> bool {
        match self {
            Shape::Leaf(ty) => matches!(
                types.kind(types.repr(*ty)),
                TyKind::Int(_)
                    | TyKind::Float(_)
                    | TyKind::Bool
                    | TyKind::Char
                    | TyKind::Byte
                    | TyKind::Unit
                    | TyKind::String
                    | TyKind::Array(_)
                    | TyKind::Deque(_)
                    | TyKind::Map(..)
                    | TyKind::Set(_)
                    | TyKind::Fn(..)
                    | TyKind::CFn(..)
                    | TyKind::Pointer(_)
                    | TyKind::Handle(..)
            ),
            Shape::Record(_, fields) => fields.iter().all(|field| field.in_registers(types)),
            Shape::Enum(..) => false,
        }
    }

    fn leaves(&self) -> usize {
        match self {
            Shape::Leaf(_) => 1,
            Shape::Record(_, fields) => fields.iter().map(Shape::leaves).sum(),
            Shape::Enum(_, variants) => 1 + variants.iter().flatten().map(Shape::leaves).sum::<usize>(),
        }
    }

    /// The part of it a path of fields and variants reaches, and the rest
    /// of the path, which only a part that is not split may have.
    fn at<'s, 'p>(&'s self, path: &'p [Proj]) -> Option<(&'s Shape, &'p [Proj])> {
        let mut shape = self;
        for (index, proj) in path.iter().enumerate() {
            shape = match (shape, proj) {
                (Shape::Leaf(_), _) => return Some((shape, &path[index..])),
                (Shape::Record(_, fields), Proj::Field(field)) => fields.get(*field as usize)?,
                (Shape::Enum(_, variants), Proj::Variant(variant, field)) => {
                    variants.get(*variant as usize)?.get(*field as usize)?
                }
                _ => return None,
            };
        }
        Some((shape, &[]))
    }
}

/// The locals a split local is made of, shaped as its type.
#[derive(Clone)]
enum Node {
    Leaf(Local),
    Record(Ty, Vec<Node>),
    Enum(Local, Ty, Vec<Vec<Node>>),
}

impl Node {
    fn make(shape: &Shape, locals: &mut Vec<Ty>) -> Node {
        let mut new = |ty: Ty| {
            locals.push(ty);
            Local(locals.len() as u32 - 1)
        };
        match shape {
            Shape::Leaf(ty) => Node::Leaf(new(*ty)),
            Shape::Record(ty, fields) => {
                Node::Record(*ty, fields.iter().map(|field| Node::make(field, locals)).collect())
            }
            Shape::Enum(tag, variants) => {
                let tag_local = new(*tag);
                let variants =
                    variants.iter().map(|fields| fields.iter().map(|f| Node::make(f, locals)).collect()).collect();
                Node::Enum(tag_local, *tag, variants)
            }
        }
    }

    fn at<'n, 'p>(&'n self, path: &'p [Proj]) -> (&'n Node, &'p [Proj]) {
        let mut node = self;
        for (index, proj) in path.iter().enumerate() {
            node = match (node, proj) {
                (Node::Leaf(_), _) => return (node, &path[index..]),
                (Node::Record(_, fields), Proj::Field(field)) => &fields[*field as usize],
                (Node::Enum(_, _, variants), Proj::Variant(variant, field)) => {
                    &variants[*variant as usize][*field as usize]
                }
                _ => unreachable!("a path the shape checked"),
            };
        }
        (node, &[])
    }

    /// Each local it is made of, with its path from it; a tag's is none.
    fn locals(&self, path: &mut Vec<Proj>, out: &mut Vec<(Option<Vec<Proj>>, Local)>) {
        match self {
            Node::Leaf(local) => out.push((Some(path.clone()), *local)),
            Node::Record(_, fields) => {
                for (index, field) in fields.iter().enumerate() {
                    path.push(Proj::Field(index as u32));
                    field.locals(path, out);
                    path.pop();
                }
            }
            Node::Enum(tag, _, variants) => {
                out.push((None, *tag));
                for (variant, fields) in variants.iter().enumerate() {
                    for (index, field) in fields.iter().enumerate() {
                        path.push(Proj::Variant(variant as u32, index as u32));
                        field.locals(path, out);
                        path.pop();
                    }
                }
            }
        }
    }

    fn all(&self) -> Vec<(Option<Vec<Proj>>, Local)> {
        let mut out = Vec::new();
        self.locals(&mut Vec::new(), &mut out);
        out
    }

    /// The locals of it that hold counted values.
    fn counted(&self, locals: &[Ty], types: &Types) -> Vec<Local> {
        self.all()
            .into_iter()
            .filter(|(path, local)| path.is_some() && types.needs_drop(locals[local.0 as usize]))
            .map(|(_, local)| local)
            .collect()
    }
}

/// The fields of a split record named by their paths from `name`, but
/// those `@secret` or of a secret type; an enum's are not named.
fn leaf_names(types: &Types, node: &Node, name: &str) -> Vec<(Local, String)> {
    match node {
        Node::Leaf(local) => vec![(*local, name.to_string())],
        Node::Enum(..) => Vec::new(),
        Node::Record(ty, fields) => {
            let names: Vec<Option<String>> = match types.kind(types.repr(*ty)) {
                TyKind::Adt(id) => match &types.adt(*id).kind {
                    _ if types.adt(*id).secret => return Vec::new(),
                    crate::types::AdtKind::Record(declared) => {
                        declared.iter().map(|field| (!field.secret).then(|| field.name.clone())).collect()
                    }
                    _ => return Vec::new(),
                },
                _ => (0..fields.len()).map(|index| Some(index.to_string())).collect(),
            };
            fields
                .iter()
                .zip(names)
                .filter_map(|(field, part)| Some(leaf_names(types, field, &format!("{name}.{}", part?))))
                .flatten()
                .collect()
        }
    }
}

/// The records of a split binding named `name`, the binding's and those
/// of its fields, for a dump to show them whole from their fields; none of
/// a secret type, nor of an enum.
fn records(types: &Types, node: &Node, name: &str, (local, scope): (Local, Option<u32>), out: &mut Vec<Split>) {
    let Node::Record(ty, fields) = node else { return };
    let secret = match types.kind(types.repr(*ty)) {
        TyKind::Adt(id) => types.adt(*id).secret,
        _ => false,
    };
    if secret {
        return;
    }
    out.push(Split { name: name.to_string(), ty: *ty, scope, local });
    let names: Vec<Option<String>> = match types.kind(types.repr(*ty)) {
        TyKind::Adt(id) => match &types.adt(*id).kind {
            crate::types::AdtKind::Record(declared) => {
                declared.iter().map(|field| (!field.secret).then(|| field.name.clone())).collect()
            }
            _ => Vec::new(),
        },
        _ => (0..fields.len()).map(|index| Some(index.to_string())).collect(),
    };
    for (field, part) in fields.iter().zip(names) {
        if let Some(part) = part {
            records(types, field, &format!("{name}.{part}"), (local, scope), out);
        }
    }
}

/// How a statement uses a place of a candidate.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    /// Written, released, retained, moved or copied whole: anything goes.
    Free,
    /// Read whole: a record is put together again; an enum cannot be.
    Whole,
    /// Lent, as the address of a place: only a part that is not split.
    Lent,
}

/// What a local of a type splits into, if it does.
fn candidate(types: &Types, ty: Ty) -> Option<Shape> {
    if crate::verify::nothing(types, ty) {
        return None;
    }
    let shape = Shape::of(types, ty, true);
    let leaves = shape.leaves();
    (!matches!(shape, Shape::Leaf(_)) && leaves > 0 && leaves <= LEAVES).then_some(shape)
}

fn split_in(body: &mut Body, types: &Types, loans: &[Vec<bool>], bindings: &[Local]) {
    // A live build, whose frames move between versions, is not optimized.
    if body.osr.is_some() {
        return;
    }
    let count = body.locals.len();
    // What holds nothing takes no room to split, and parameters and the
    // result stay as the caller passes them.
    let mut known: HashMap<Ty, Option<Shape>> = HashMap::new();
    let mut shapes: Vec<Option<Shape>> =
        body.locals.iter().map(|&ty| known.entry(ty).or_insert_with(|| candidate(types, ty)).clone()).collect();
    for &local in body.params.iter().chain(&body.ret) {
        shapes[local.0 as usize] = None;
    }
    for &local in bindings {
        if matches!(shapes[local.0 as usize], Some(Shape::Enum(..))) {
            shapes[local.0 as usize] = None;
        }
    }
    if shapes.iter().all(Option::is_none) {
        return;
    }
    // The statements that touch a candidate: most touch none, and the rest
    // of the pass leaves them as they are.
    let touched: Vec<Vec<bool>> = {
        let (mut used, mut defined) = (Vec::new(), Vec::new());
        let candidate = |local: &Local| shapes[local.0 as usize].is_some();
        body.blocks
            .iter()
            .map(|block| {
                block
                    .stmts
                    .iter()
                    .map(|statement| {
                        used.clear();
                        defined.clear();
                        crate::mir::statement_locals(statement, &mut used, &mut defined);
                        used.iter().chain(&defined).any(candidate)
                            || matches!(statement, Statement::Assign(place, ..) if candidate(&place.local))
                    })
                    .collect()
            })
            .collect()
    };
    // What disqualifies a candidate, until nothing more does: a copy of an
    // enum whole joins two locals, which are split together or not at all.
    let mut why: Vec<Option<&'static str>> = vec![None; count];
    while shapes.iter().any(Option::is_some) {
        let out = disqualified(body, &shapes, loans, types, &touched);
        let mut changed = false;
        for (local, reason) in out {
            if shapes[local.0 as usize].take().is_some() {
                why[local.0 as usize] = Some(reason);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    // One only read field by field costs the same in memory: splitting
    // pays where values are made or copied whole.
    let moving = moved_whole(body, &shapes, &touched);
    for (index, shape) in shapes.iter_mut().enumerate() {
        if shape.is_some() && !moving[index] {
            *shape = None;
            why[index] = Some("nothing makes or copies it whole, so registers would save nothing");
        }
    }
    note_whole(body, &why);
    if shapes.iter().all(Option::is_none) {
        return;
    }
    let noting = crate::mir_text::noting(&body.name);
    let mut nodes: Vec<Option<Node>> = vec![None; count];
    for (index, shape) in shapes.iter().enumerate() {
        let Some(shape) = shape else { continue };
        let node = Node::make(shape, &mut body.locals);
        if noting {
            let parts: Vec<String> = node.all().iter().map(|(_, l)| format!("l{}", l.0)).collect();
            body.notes.push(format!("split: l{index} into {}", parts.join(", ")));
        }
        nodes[index] = Some(node);
    }
    // A binding split is no longer a local a dump reads: the fields of a
    // record are, by their paths, as `sample.hits`, but those `@secret`.
    body.names.resize(body.locals.len(), None);
    let mut scoped = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        let Some(node) = node else { continue };
        let named = |name: &str| leaf_names(types, node, name);
        if let Some(name) = body.names[index].take() {
            for (leaf, name) in named(&name) {
                body.names[leaf.0 as usize] = Some(name);
            }
            records(types, node, &name, (Local(index as u32), None), &mut body.split);
        }
        for (_, name, call) in body.scoped.iter().filter(|(local, ..)| local.0 as usize == index) {
            scoped.extend(named(name).into_iter().map(|(leaf, name)| (leaf, name, *call)));
            records(types, node, name, (Local(index as u32), Some(*call)), &mut body.split);
        }
        body.locals[index] = Ty::UNIT;
    }
    body.scoped.retain(|(local, ..)| nodes[local.0 as usize].is_none());
    body.scoped.extend(scoped);
    body.arguments.retain(|local| nodes[local.0 as usize].is_none());
    let mut rewriter = Rewriter { locals: std::mem::take(&mut body.locals), nodes: &nodes, out: Vec::new(), types };
    for (block, touched) in body.blocks.iter_mut().zip(&touched) {
        for (statement, &touched) in std::mem::take(&mut block.stmts).into_iter().zip(touched) {
            match touched {
                true => rewriter.statement(statement),
                false => rewriter.out.push(statement),
            }
        }
        rewriter.terminator(&mut block.terminator);
        block.stmts = std::mem::take(&mut rewriter.out);
    }
    body.locals = rewriter.locals;
    body.names.resize(body.locals.len(), None);
    let counted: Vec<Local> = nodes.iter().flatten().flat_map(|node| node.counted(&body.locals, types)).collect();
    drop_zero_releases(body, &counted);
}

/// Notes, in a function whose MIR is to be written, why each local that
/// could have been split stays whole.
fn note_whole(body: &mut Body, why: &[Option<&'static str>]) {
    if !crate::mir_text::noting(&body.name) {
        return;
    }
    for (index, reason) in why.iter().enumerate() {
        if let Some(reason) = reason {
            body.notes.push(format!("split: l{index} stays whole: {reason}"));
        }
    }
}

/// Which candidates a statement makes, as a record or a variant, or copies
/// whole, to or from another place.
fn moved_whole(body: &Body, shapes: &[Option<Shape>], touched: &[Vec<bool>]) -> Vec<bool> {
    let mut moving = vec![false; shapes.len()];
    let whole = |place: &Place| {
        shapes[place.local.0 as usize]
            .as_ref()
            .is_some_and(|shape| !matches!(shape.at(&place.path), Some((Shape::Leaf(_), _))))
    };
    let statements = body.blocks.iter().zip(touched).flat_map(|(block, touched)| block.stmts.iter().zip(touched));
    for (statement, _) in statements.filter(|(_, touched)| **touched) {
        let Statement::Assign(place, rvalue, _) = statement else { continue };
        match &**rvalue {
            Rvalue::Use(Operand::Copy(from)) => {
                for side in [place, from] {
                    if whole(side) {
                        moving[side.local.0 as usize] = true;
                    }
                }
            }
            Rvalue::Aggregate(_) | Rvalue::Variant(..) if whole(place) => moving[place.local.0 as usize] = true,
            _ => {}
        }
    }
    moving
}

/// The candidates a use in the body rules out, and why.
fn disqualified(
    body: &Body,
    shapes: &[Option<Shape>],
    loans: &[Vec<bool>],
    types: &Types,
    touched: &[Vec<bool>],
) -> Vec<(Local, &'static str)> {
    const LENT: &str = "it is lent, as a `var` or `mut` argument";
    const WHOLE: &str = "it is read whole, as an argument or a part of another value";
    const JOINED: &str = "it is copied whole to or from a value that stays in memory";
    const UNWINDS: &str = "a statement that may panic gives it";
    let mut out: Vec<(Local, &'static str)> = Vec::new();
    let check = |place: &Place, how: Use, out: &mut Vec<(Local, &'static str)>| {
        let Some(shape) = &shapes[place.local.0 as usize] else { return };
        match shape.at(&place.path) {
            None => out.push((place.local, WHOLE)),
            Some((Shape::Leaf(_), _)) => {}
            Some((_, rest)) if !rest.is_empty() => out.push((place.local, WHOLE)),
            // Read whole, a record would be put together again in memory,
            // which costs what splitting saves; an enum cannot be.
            Some(_) => match how {
                Use::Free => {}
                Use::Whole => out.push((place.local, WHOLE)),
                Use::Lent => out.push((place.local, LENT)),
            },
        }
    };
    let whole_enum =
        |place: &Place| place.path.is_empty() && matches!(shapes[place.local.0 as usize], Some(Shape::Enum(..)));
    for (block, touched) in body.blocks.iter().zip(touched) {
        for (at, statement) in block.stmts.iter().enumerate() {
            if !touched[at] {
                continue;
            }
            match statement {
                Statement::Assign(place, rvalue, _) => {
                    check(place, Use::Free, &mut out);
                    // A statement that may panic is checked right after
                    // it, so it stays one statement: what it writes or
                    // copies whole is not split.
                    let unwinds = block.stmts.get(at + 1).is_some_and(|next| {
                        matches!(next, Statement::Assign(_, rvalue, _) if matches!(&**rvalue, Rvalue::Unwinding | Rvalue::UnwindingOwn))
                    });
                    let whole = |place: &Place| {
                        shapes[place.local.0 as usize]
                            .as_ref()
                            .is_some_and(|shape| !matches!(shape.at(&place.path), Some((Shape::Leaf(_), _))))
                    };
                    if unwinds {
                        if whole(place) {
                            out.push((place.local, UNWINDS));
                        }
                        if let Rvalue::Use(Operand::Copy(from)) = &**rvalue
                            && whole(from)
                        {
                            out.push((from.local, UNWINDS));
                        }
                    }
                    match &**rvalue {
                        // A copy of an enum whole joins both locals.
                        Rvalue::Use(Operand::Copy(from)) if whole_enum(from) || whole_enum(place) => {
                            if !(whole_enum(from) && whole_enum(place)) {
                                out.extend([(place.local, JOINED), (from.local, JOINED)]);
                            }
                        }
                        Rvalue::Variant(..) => {}
                        _ if whole_enum(place) => out.push((place.local, JOINED)),
                        _ => {}
                    }
                    // A call lends what a parameter of its takes as a loan,
                    // `var` or `mut`, directly or through a `fn` value.
                    let lent: Vec<bool> = match &**rvalue {
                        Rvalue::Call(crate::hir::Callee::Fn(callee), _) => {
                            loans.get(callee.0 as usize).cloned().unwrap_or_default()
                        }
                        Rvalue::CallIndirect(Operand::Copy(callee), _) => {
                            match types.kind(types.repr(body.place_ty(types, callee))) {
                                TyKind::Fn(_, _, _, modes, _) => modes
                                    .iter()
                                    .map(|mode| matches!(mode, crate::ast::Mode::Mut | crate::ast::Mode::Var))
                                    .collect(),
                                _ => Vec::new(),
                            }
                        }
                        _ => Vec::new(),
                    };
                    if let Rvalue::Call(_, args) | Rvalue::CallIndirect(_, args) = &**rvalue {
                        for (arg, _) in args.iter().zip(&lent).filter(|(_, loan)| **loan) {
                            if let Operand::Copy(place) = arg {
                                check(place, Use::Lent, &mut out);
                            }
                        }
                    }
                    // A copy, and the fields of a record or a variant made
                    // into a local that is split, are taken field by field.
                    let copying = match &**rvalue {
                        Rvalue::Use(Operand::Copy(_)) => true,
                        Rvalue::Aggregate(_) | Rvalue::Variant(..) => whole(place),
                        _ => false,
                    };
                    rvalue_places(rvalue, &mut |used, how| {
                        check(used, if copying && how == Use::Whole { Use::Free } else { how }, &mut out)
                    });
                    // An enum made a field of another value is a field that
                    // is not split, which it would be put together again for.
                    if let Rvalue::Aggregate(operands) | Rvalue::Variant(_, operands) = &**rvalue {
                        for operand in operands {
                            if let Operand::Copy(from) = operand
                                && whole_enum(from)
                            {
                                out.push((from.local, WHOLE));
                            }
                        }
                    }
                    if let Rvalue::Select(cases, ..) = &**rvalue {
                        out.extend(cases.iter().map(|case| (case.out, WHOLE)));
                    }
                }
                Statement::Retain(place) | Statement::Release(place) | Statement::Moved(place) => {
                    check(place, Use::Free, &mut out)
                }
                Statement::Clear(_) | Statement::DeferPop | Statement::OsrLoad(_) => {}
                Statement::Lend(alias, place) => {
                    out.push((*alias, LENT));
                    check(place, Use::Lent, &mut out);
                }
                Statement::LendAt(alias, operand) => {
                    out.push((*alias, LENT));
                    if let Operand::Copy(place) = operand {
                        check(place, Use::Lent, &mut out);
                    }
                }
                Statement::DeferPush(_, place) => check(place, Use::Lent, &mut out),
                Statement::Failing(Operand::Copy(place), ..) => check(place, Use::Whole, &mut out),
                Statement::Failing(..) => {}
            }
        }
        let operand = match &block.terminator {
            Terminator::Branch(operand, ..) | Terminator::Return(operand) => Some(operand),
            Terminator::Panic { message: Some(operand), .. } => Some(operand),
            _ => None,
        };
        if let Some(Operand::Copy(place)) = operand {
            check(place, Use::Whole, &mut out);
        }
    }
    out
}

/// Each place an rvalue reads or writes, and how.
fn rvalue_places(rvalue: &Rvalue, f: &mut impl FnMut(&Place, Use)) {
    match rvalue {
        Rvalue::Discriminant(place) => f(place, Use::Free),
        Rvalue::Mutate(_, place, args, ..) => {
            f(place, Use::Lent);
            for arg in args {
                if let Operand::Copy(place) = arg {
                    f(place, Use::Whole);
                }
            }
        }
        Rvalue::AssignSlice(place, bounds, operand, _) => {
            f(place, Use::Lent);
            for operand in bounds.iter().flatten().chain([operand]) {
                if let Operand::Copy(place) = operand {
                    f(place, Use::Whole);
                }
            }
        }
        _ => {
            let mut copy = rvalue.clone();
            crate::inline::visit_rvalue(&mut copy, &mut |place| f(place, Use::Whole));
        }
    }
}

struct Rewriter<'n> {
    locals: Vec<Ty>,
    nodes: &'n [Option<Node>],
    out: Vec<Statement>,
    types: &'n Types,
}

impl<'n> Rewriter<'n> {
    fn new_local(&mut self, ty: Ty) -> Local {
        self.locals.push(ty);
        Local(self.locals.len() as u32 - 1)
    }

    /// The node a place of a split local reaches, with the rest of its path.
    fn node<'p>(&self, place: &'p Place) -> Option<(&'n Node, &'p [Proj])> {
        let nodes: &'n [Option<Node>] = self.nodes;
        Some(nodes.get(place.local.0 as usize)?.as_ref()?.at(&place.path))
    }

    /// The place itself, or the part of a split local it reaches that is
    /// not split; none for a record or an enum of one.
    fn leaf_place(&self, place: &Place) -> Option<Place> {
        match self.node(place) {
            None => Some(place.clone()),
            Some((Node::Leaf(local), rest)) => Some(Place { local: *local, path: rest.to_vec() }),
            Some(_) => None,
        }
    }

    fn ty(&self, node: &Node) -> Ty {
        match node {
            Node::Leaf(local) => self.locals[local.0 as usize],
            Node::Record(ty, _) => *ty,
            Node::Enum(..) => unreachable!("an enum is never put together again"),
        }
    }

    /// An operand that reads a place: the part of a split local, or a
    /// record of one put together again first.
    fn operand(&mut self, operand: Operand, span: Span) -> Operand {
        let Operand::Copy(place) = operand else { return operand };
        if let Some(leaf) = self.leaf_place(&place) {
            return Operand::Copy(leaf);
        }
        let (node, _) = self.node(&place).expect("a split place");
        Operand::Copy(Place::local(self.assemble(node, span)))
    }

    /// A record of a split local put together again, in a new local.
    fn assemble(&mut self, node: &Node, span: Span) -> Local {
        let Node::Record(ty, fields) = node else { unreachable!("only a record is put together again") };
        let mut operands = Vec::new();
        for field in fields {
            operands.push(match field {
                Node::Leaf(local) => Operand::local(*local),
                record => Operand::local(self.assemble(record, span)),
            });
        }
        let local = self.new_local(*ty);
        self.out.push(Statement::Assign(Place::local(local), Box::new(Rvalue::Aggregate(operands)), span));
        local
    }

    /// Assigns each local of a node from the same path of a place.
    fn copy(&mut self, node: &Node, from: &Place, span: Span) {
        let from_node = self.node(from).map(|(node, _)| node);
        for (path, local) in node.all() {
            let value = match (path, &from_node) {
                (None, Some(Node::Enum(tag, ..))) => Operand::local(*tag),
                (None, _) => unreachable!("an enum is copied from a split one"),
                (Some(path), _) => {
                    let mut place = from.clone();
                    place.path.extend(path);
                    self.operand(Operand::Copy(place), span)
                }
            };
            self.out.push(Statement::Assign(Place::local(local), Box::new(Rvalue::Use(value)), span));
        }
    }

    /// Writes each field of a split record to the same path of a place
    /// that is not split; a local whole is assigned the record put together.
    fn store(&mut self, node: &Node, to: &Place, span: Span) {
        if to.path.is_empty() {
            let Node::Record(_, fields) = node else { unreachable!("a record") };
            let operands = fields
                .iter()
                .map(|field| match field {
                    Node::Leaf(local) => Operand::local(*local),
                    record => Operand::local(self.assemble(record, span)),
                })
                .collect();
            self.out.push(Statement::Assign(to.clone(), Box::new(Rvalue::Aggregate(operands)), span));
            return;
        }
        for (path, local) in node.all() {
            let mut place = to.clone();
            place.path.extend(path.expect("a record's fields"));
            self.out.push(Statement::Assign(place, Box::new(Rvalue::Use(Operand::local(local))), span));
        }
    }

    /// Assigns a node from an operand.
    fn assign(&mut self, node: &Node, operand: Operand, span: Span) {
        match (node, operand) {
            (Node::Leaf(local), operand) => {
                let operand = self.operand(operand, span);
                self.out.push(Statement::Assign(Place::local(*local), Box::new(Rvalue::Use(operand)), span));
            }
            (_, Operand::Copy(from)) => self.copy(node, &from, span),
            (_, constant) => {
                let local = self.new_local(self.ty(node));
                self.out.push(Statement::Assign(Place::local(local), Box::new(Rvalue::Use(constant)), span));
                self.copy(node, &Place::local(local), span);
            }
        }
    }

    fn statement(&mut self, statement: Statement) {
        let retain = matches!(statement, Statement::Retain(_));
        match statement {
            Statement::Assign(place, rvalue, span) => self.assignment(place, *rvalue, span),
            Statement::Retain(place) | Statement::Release(place)
                if self.node(&place).is_some_and(|(node, _)| !matches!(node, Node::Leaf(_))) =>
            {
                let (node, _) = self.node(&place).expect("split");
                for local in node.counted(&self.locals, self.types) {
                    let place = Place::local(local);
                    self.out.push(if retain { Statement::Retain(place) } else { Statement::Release(place) });
                }
            }
            Statement::Retain(place) => {
                let place = self.leaf_place(&place).expect("a part that is not split");
                self.out.push(Statement::Retain(place));
            }
            Statement::Release(place) => {
                let place = self.leaf_place(&place).expect("a part that is not split");
                self.out.push(Statement::Release(place));
            }
            Statement::Moved(place) => match self.node(&place) {
                None => self.out.push(Statement::Moved(place)),
                Some((Node::Leaf(local), rest)) if !rest.is_empty() => {
                    self.out.push(Statement::Moved(Place { local: *local, path: rest.to_vec() }))
                }
                // Zeroed whole, as the original's bytes are.
                Some((node, _)) => {
                    for (_, local) in node.all() {
                        self.out.push(Statement::Clear(local));
                    }
                }
            },
            Statement::Clear(local) => match &self.nodes[local.0 as usize] {
                Some(node) => {
                    for (_, local) in node.all() {
                        self.out.push(Statement::Clear(local));
                    }
                }
                None => self.out.push(Statement::Clear(local)),
            },
            Statement::Lend(alias, place) => {
                let place = self.leaf_place(&place).expect("a part that is not split");
                self.out.push(Statement::Lend(alias, place));
            }
            Statement::LendAt(alias, operand) => {
                let operand = match operand {
                    Operand::Copy(place) => Operand::Copy(self.leaf_place(&place).expect("not split")),
                    constant => constant,
                };
                self.out.push(Statement::LendAt(alias, operand));
            }
            Statement::DeferPush(function, place) => {
                let place = self.leaf_place(&place).expect("a part that is not split");
                self.out.push(Statement::DeferPush(function, place));
            }
            Statement::Failing(operand, span, from) => {
                let operand = self.operand(operand, span);
                self.out.push(Statement::Failing(operand, span, from));
            }
            other @ (Statement::DeferPop | Statement::OsrLoad(_)) => self.out.push(other),
        }
    }

    fn assignment(&mut self, place: Place, rvalue: Rvalue, span: Span) {
        // Into a split local, whole or a record or enum of it.
        if let Some((node, _)) = self.node(&place)
            && !matches!(node, Node::Leaf(_))
        {
            match rvalue {
                Rvalue::Use(operand) => self.assign(node, operand, span),
                Rvalue::Aggregate(operands) => {
                    let Node::Record(_, fields) = &node else { unreachable!("a record") };
                    for (field, operand) in fields.iter().zip(operands) {
                        self.assign(field, operand, span);
                    }
                }
                Rvalue::Variant(variant, operands) => {
                    let Node::Enum(tag, tag_ty, variants) = &node else { unreachable!("an enum") };
                    let value = Operand::Const(Const::Int(i128::from(variant), *tag_ty));
                    self.out.push(Statement::Assign(Place::local(*tag), Box::new(Rvalue::Use(value)), span));
                    for (index, fields) in variants.iter().enumerate() {
                        if index == variant as usize {
                            for (field, operand) in fields.iter().zip(operands.clone()) {
                                self.assign(field, operand, span);
                            }
                            continue;
                        }
                        // What the variant it holds does not have is zero.
                        for (_, local) in fields.iter().flat_map(Node::all) {
                            self.out.push(Statement::Clear(local));
                        }
                    }
                }
                rvalue => {
                    // A record from anything else, as a call's result:
                    // into memory, then taken apart.
                    let rvalue = self.rvalue(rvalue, span);
                    let local = self.new_local(self.ty(node));
                    self.out.push(Statement::Assign(Place::local(local), Box::new(rvalue), span));
                    self.copy(node, &Place::local(local), span);
                }
            }
            return;
        }
        // A split record copied whole into a place that is not split.
        if let Rvalue::Use(Operand::Copy(from)) = &rvalue
            && let Some((node @ Node::Record(..), _)) = self.node(from)
        {
            let to = self.leaf_place(&place).expect("a place that is not split");
            self.store(node, &to, span);
            return;
        }
        let rvalue = match rvalue {
            Rvalue::Discriminant(from) => match self.node(&from) {
                Some((Node::Enum(tag, ..), _)) => Rvalue::Use(Operand::local(*tag)),
                _ => Rvalue::Discriminant(self.leaf_place(&from).expect("a part that is not split")),
            },
            rvalue => self.rvalue(rvalue, span),
        };
        let place = self.leaf_place(&place).expect("a part that is not split");
        self.out.push(Statement::Assign(place, Box::new(rvalue), span));
    }

    /// An rvalue whose places of split locals are their parts, a record
    /// put together again where it is read whole.
    fn rvalue(&mut self, mut rvalue: Rvalue, span: Span) -> Rvalue {
        let mut places: Vec<Place> = Vec::new();
        crate::inline::visit_rvalue(&mut rvalue, &mut |place| places.push(place.clone()));
        let mut replaced = Vec::new();
        for place in places {
            let Operand::Copy(new) = self.operand(Operand::Copy(place), span) else { unreachable!("a place") };
            replaced.push(new);
        }
        let mut next = replaced.into_iter();
        crate::inline::visit_rvalue(&mut rvalue, &mut |place| *place = next.next().expect("the same places"));
        rvalue
    }

    fn terminator(&mut self, terminator: &mut Terminator) {
        let span = Span::default();
        let operand = match terminator {
            Terminator::Branch(operand, ..) | Terminator::Return(operand) => operand,
            Terminator::Panic { message: Some(operand), span, .. } => {
                let span = *span;
                *operand = self.operand(operand.clone(), span);
                return;
            }
            _ => return,
        };
        *operand = self.operand(operand.clone(), span);
    }
}

/// Drops a release or a retain of a local every path to it left zero.
fn drop_zero_releases(body: &mut Body, counted: &[Local]) {
    let mut tracked = vec![u32::MAX; body.locals.len()];
    for (at, local) in counted.iter().enumerate() {
        tracked[local.0 as usize] = at as u32;
    }
    let at = |local: Local| Some(tracked[local.0 as usize]).filter(|&at| at != u32::MAX).map(|at| at as usize);
    let dropped = |statement: &Statement| match statement {
        Statement::Retain(place) | Statement::Release(place) => place.path.is_empty() && at(place.local).is_some(),
        _ => false,
    };
    if !body.blocks.iter().flat_map(|block| &block.stmts).any(dropped) {
        return;
    }
    let words = counted.len().div_ceil(64);
    let set = |bits: &mut [u64], local: Local, on: bool| {
        if let Some(at) = at(local) {
            let (word, bit) = (at / 64, 1u64 << (at % 64));
            if on { bits[word] |= bit } else { bits[word] &= !bit }
        }
    };
    let has = |bits: &[u64], local: Local| at(local).is_some_and(|at| bits[at / 64] >> (at % 64) & 1 == 1);
    // What a statement does to what is known zero: a clear makes it zero,
    // and a write whole or through a path, as a push writes an array, not.
    let (mut used, mut defined) = (Vec::new(), Vec::new());
    let mut step = |statement: &Statement, bits: &mut [u64]| match statement {
        Statement::Clear(local) => set(bits, *local, true),
        Statement::Retain(_) | Statement::Release(_) => {}
        statement => {
            used.clear();
            defined.clear();
            crate::mir::statement_locals(statement, &mut used, &mut defined);
            for &local in &defined {
                set(bits, local, false);
            }
            match statement {
                Statement::Assign(place, rvalue, _) => {
                    set(bits, place.local, false);
                    if let Rvalue::Mutate(_, place, ..) | Rvalue::AssignSlice(place, ..) = &**rvalue {
                        set(bits, place.local, false);
                    }
                    if let Rvalue::Select(cases, ..) = &**rvalue {
                        cases.iter().for_each(|case| set(bits, case.out, false));
                    }
                }
                Statement::Lend(_, place) | Statement::Moved(place) | Statement::DeferPush(_, place) => {
                    set(bits, place.local, false)
                }
                Statement::LendAt(_, Operand::Copy(place)) => set(bits, place.local, false),
                _ => {}
            }
        }
    };
    // Each block's effect, as what it makes zero and what it makes not.
    let blocks = body.blocks.len();
    let mut made: Vec<(Vec<u64>, Vec<u64>)> = Vec::with_capacity(blocks);
    for block in &body.blocks {
        let (mut zero, mut all) = (vec![0u64; words], vec![u64::MAX; words]);
        for statement in &block.stmts {
            step(statement, &mut zero);
            step(statement, &mut all);
        }
        made.push((zero, all));
    }
    // A local starts as zeros; meeting ways keeps what all of them have.
    let mut entry: Vec<Option<Vec<u64>>> = vec![None; blocks];
    entry[0] = Some(vec![u64::MAX; words]);
    let mut changed = true;
    while changed {
        changed = false;
        for block in 0..blocks {
            let Some(start) = &entry[block] else { continue };
            // What every way out of the block knows: what it made zero, and
            // what was zero and it left so.
            let (zero, all) = &made[block];
            let end: Vec<u64> = start.iter().zip(zero).zip(all).map(|((s, z), a)| z | (s & a)).collect();
            for to in successors(&body.blocks[block].terminator) {
                let met = match &entry[to] {
                    None => end.clone(),
                    Some(old) => old.iter().zip(&end).map(|(a, b)| a & b).collect(),
                };
                if entry[to].as_ref() != Some(&met) {
                    entry[to] = Some(met);
                    changed = true;
                }
            }
        }
    }
    for (block, start) in entry.into_iter().enumerate() {
        let Some(mut bits) = start else { continue };
        if !body.blocks[block].stmts.iter().any(dropped) {
            continue;
        }
        let stmts = std::mem::take(&mut body.blocks[block].stmts);
        let mut kept = Vec::with_capacity(stmts.len());
        for statement in stmts {
            let zero = match &statement {
                Statement::Retain(place) | Statement::Release(place) if place.path.is_empty() => {
                    has(&bits, place.local)
                }
                _ => false,
            };
            step(&statement, &mut bits);
            if !zero {
                kept.push(statement);
            }
        }
        body.blocks[block].stmts = kept;
    }
}
