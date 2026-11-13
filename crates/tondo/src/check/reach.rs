//! The bodies of the standard library a program reaches, which are all of
//! it that is checked (docs/speed.md, E): every other body is checked, and
//! then each of the standard library's that a checked one could lead
//! monomorphization to, as `mono.rs` would, a trait's methods conservatively.

use super::*;

/// The declared bodies checked, and those reached and not yet checked.
pub(super) struct Reach {
    pub(super) functions: Vec<Option<hir::Function>>,
    reached: Vec<bool>,
    order: Vec<FnId>,
    next: usize,
    /// How many closures were walked.
    closures: usize,
    /// The methods reached, by trait, method and the head of the receiver
    /// (none for any), with how many implementations there were then.
    methods: HashMap<(TraitId, u32, Option<Head>), usize>,
    /// The records and enums whose parts' `Display` was reached.
    shown_parts: HashSet<Ty>,
}

impl Reach {
    /// For `count` declared bodies, the standard library's left until reached.
    pub(super) fn new(count: usize) -> Reach {
        Reach {
            functions: (0..count).map(|_| None).collect(),
            reached: vec![false; count],
            order: Vec::new(),
            next: 0,
            closures: 0,
            methods: HashMap::new(),
            shown_parts: HashSet::new(),
        }
    }

    pub(super) fn add(&mut self, id: FnId) {
        // Closures, a script's `main` and the tests are checked as they are made.
        if let Some(reached) = self.reached.get_mut(id.0 as usize)
            && !*reached
        {
            *reached = true;
            self.order.push(id);
        }
    }
}

impl<'a> Checker<'a> {
    /// Whether a body is the standard library's, which is checked once reached.
    pub(super) fn reached_later(&self, f: &ast::FnDecl) -> bool {
        !self.every_body && self.sources.file_of(f.span.start).logical.starts_with("<std>/")
    }

    /// Checks the bodies reached and not checked, and what they reach, until
    /// none is left.
    pub(super) fn check_reached(&mut self, bodies: &[&'a ast::FnDecl], reach: &mut Reach) {
        loop {
            while reach.closures < self.closure_functions.len() {
                let closure = self.closure_functions[reach.closures].take();
                if let Some(closure) = &closure {
                    self.reach_from(closure, reach);
                }
                self.closure_functions[reach.closures] = closure;
                reach.closures += 1;
            }
            let Some(&id) = reach.order.get(reach.next) else { break };
            reach.next += 1;
            let function = self.function(bodies[id.0 as usize], id);
            self.reach_from(&function, reach);
            reach.functions[id.0 as usize] = Some(function);
        }
    }

    /// Adds to `reach` what a checked function may call.
    pub(super) fn reach_from(&mut self, function: &hir::Function, reach: &mut Reach) {
        each_expr(&function.body, &mut |e| self.reach_expr(e, reach));
    }

    fn reach_expr(&mut self, e: &hir::Expr, reach: &mut Reach) {
        match &e.kind {
            hir::ExprKind::Call(Callee::Fn(id), ..) | hir::ExprKind::FnValue(id, _) => {
                reach.add(*id);
                // `sum` of an array of floats is another function (18.5), as
                // `mono.rs` makes it.
                let name = &self.signatures[id.0 as usize].name;
                if let Some(&(_, wide, narrow)) = crate::simd::FLOAT_FORMS.iter().find(|(n, ..)| n == name) {
                    for form in [wide, narrow] {
                        if let Some(at) = self.signatures.iter().position(|s| s.name == form) {
                            reach.add(FnId(at as u32));
                        }
                    }
                }
            }
            hir::ExprKind::Call(Callee::Trait(trait_id, method), types, _) => {
                let receiver = types.first().copied().unwrap_or(Ty::ERROR);
                // What is not a nominal type displays itself, what it holds by theirs.
                if *trait_id == DISPLAY {
                    self.reach_display(receiver, reach);
                } else {
                    self.reach_method(*trait_id, *method, receiver, reach);
                }
            }
            // What monomorphization displays.
            hir::ExprKind::Interpolate(segments) => {
                for segment in segments {
                    if let hir::Segment::Value(value) = segment {
                        self.reach_display(value.ty, reach);
                    }
                }
            }
            hir::ExprKind::Reflect(hir::ReflectOp::Describe, ty, _) => self.reach_display(*ty, reach),
            hir::ExprKind::ArrayOp(hir::ArrayOp::Join, operands) => self.reach_display(operands[0].ty, reach),
            _ => {}
        }
    }

    /// The implementations of a trait's method that may be called on a
    /// value of type `receiver`: those of its head, or all for a type known
    /// later, and the trait's default.
    fn reach_method(&mut self, trait_id: TraitId, method: u32, receiver: Ty, reach: &mut Reach) {
        let later = matches!(self.types.kind(receiver), TyKind::Opaque(..) | TyKind::Union(_) | TyKind::Error);
        let head = if later { None } else { self.head(receiver) };
        // Once for each, unless implementations were added since.
        if reach.methods.insert((trait_id, method, head), self.impls.len()) == Some(self.impls.len()) {
            return;
        }
        let candidates: Vec<usize> = if later {
            (0..self.impls.len()).filter(|&at| self.impls[at].trait_id == trait_id).collect()
        } else {
            self.impl_candidates(trait_id, receiver)
        };
        for at in candidates {
            if let Some(id) = self.impls[at].methods.get(method as usize).copied().flatten() {
                reach.add(id);
            }
        }
        let methods = &self.traits[trait_id.0 as usize].methods;
        if let Some(default) = methods.get(method as usize).and_then(|m| m.default) {
            reach.add(default);
        }
        // A union decodes through these, which monomorphization calls by name.
        if self.traits[trait_id.0 as usize].name == "serialization.Decode" {
            for name in ["serialization.unionAttempt", "serialization.unionMember", "serialization.unionFailure"] {
                if let Some(&ModuleName::Fn(id)) = self.names.get(name) {
                    reach.add(id);
                }
            }
        }
    }

    /// The `Display` of a displayed type and of what it holds, as
    /// monomorphization finds them.
    pub(super) fn reach_display(&mut self, ty: Ty, reach: &mut Reach) {
        match self.types.kind(ty).clone() {
            TyKind::Array(element) | TyKind::Deque(element) | TyKind::Set(element) | TyKind::Option(element) => {
                self.reach_display(element, reach)
            }
            TyKind::Map(key, value) | TyKind::Result(key, value) => {
                self.reach_display(key, reach);
                self.reach_display(value, reach);
            }
            TyKind::Union(members) | TyKind::Tuple(members) => {
                members.into_iter().for_each(|member| self.reach_display(member, reach))
            }
            TyKind::Adt(_) => {
                self.reach_method(DISPLAY, 0, ty, reach);
                // A record or an enum that shows its parts shows them with
                // theirs (language 19.6).
                if self.impl_candidates(DISPLAY, ty).is_empty()
                    && reach.shown_parts.insert(ty)
                    && let Some(parts) = self.displayed_parts(ty)
                {
                    parts.into_iter().for_each(|(_, part, _)| self.reach_display(part, reach));
                }
            }
            TyKind::Param(_) | TyKind::Opaque(..) => self.reach_method(DISPLAY, 0, ty, reach),
            _ => {}
        }
    }
}

/// Calls `f` on every expression of `block`, however deep.
pub(super) fn each_expr(block: &hir::Block, f: &mut impl FnMut(&hir::Expr)) {
    for stmt in &block.stmts {
        match stmt {
            hir::Stmt::Let(_, value)
            | hir::Stmt::Destructure(_, value)
            | hir::Stmt::Expr(value)
            | hir::Stmt::Fail(value)
            | hir::Stmt::Return(Some(value))
            | hir::Stmt::ContextSet { value, .. }
            | hir::Stmt::ContextsActivate { activate: value, .. }
            | hir::Stmt::LockSync { handle: value, .. } => expr(value, f),
            hir::Stmt::Defer { closure, call, .. } => {
                expr(closure, f);
                expr(call, f);
            }
            hir::Stmt::Lend { place, .. } => steps(&place.path, f),
            hir::Stmt::Assign { path, value, .. } => {
                steps(path, f);
                expr(value, f);
            }
            hir::Stmt::AssignSlice { path, bounds, value, .. } => {
                steps(path, f);
                bounds.iter().flatten().for_each(|bound| expr(bound, f));
                expr(value, f);
            }
            hir::Stmt::Loop { condition, body } => {
                condition.iter().for_each(|condition| expr(condition, f));
                each_expr(body, f);
            }
            hir::Stmt::ForEach { source, body, .. } => {
                expr(source, f);
                each_expr(body, f);
            }
            hir::Stmt::ForRange { start, end, body, .. } => {
                expr(start, f);
                expr(end, f);
                each_expr(body, f);
            }
            hir::Stmt::AwaitChange(..) | hir::Stmt::Return(None) | hir::Stmt::Break | hir::Stmt::Continue => {}
        }
    }
    block.tail.iter().for_each(|tail| expr(tail, f));
}

fn steps(path: &[hir::PlaceStep], f: &mut impl FnMut(&hir::Expr)) {
    for step in path {
        match step {
            hir::PlaceStep::Index(index) => expr(index, f),
            hir::PlaceStep::Entry { key, default } => {
                expr(key, f);
                expr(default, f);
            }
            hir::PlaceStep::Field(_) => {}
        }
    }
}

fn expr(e: &hir::Expr, f: &mut impl FnMut(&hir::Expr)) {
    use hir::ExprKind as E;
    f(e);
    match &e.kind {
        E::Int(_)
        | E::Float(_)
        | E::Char(_)
        | E::Bool(_)
        | E::Str(_)
        | E::Bytes(_)
        | E::Unit
        | E::Local(_)
        | E::FnValue(..)
        | E::ExternFn(_)
        | E::Layout(..)
        | E::ContextRead(..)
        | E::Error => {}
        E::Aggregate(items)
        | E::Array(items)
        | E::Set(items)
        | E::ArrayOp(_, items)
        | E::StrOp(_, items)
        | E::Pointer(_, items)
        | E::MapOp(_, items)
        | E::Call(_, _, items)
        | E::Reflect(_, _, items)
        | E::TaskOp(_, items) => items.iter().for_each(|item| expr(item, f)),
        E::Record(values) | E::Variant(_, values) | E::TestMembers(values) => {
            values.iter().for_each(|(_, value)| expr(value, f))
        }
        E::Field(inner, _)
        | E::Retype(inner)
        | E::RefValue(inner)
        | E::Length(inner)
        | E::Try(inner)
        | E::Convert(inner)
        | E::Fixed(inner)
        | E::Erase(inner, _)
        | E::Callback(inner, _)
        | E::Spawn(inner)
        | E::Unary(_, inner)
        | E::Panic(inner, _) => expr(inner, f),
        E::With(base, values) => {
            expr(base, f);
            values.iter().for_each(|(_, value)| expr(value, f));
        }
        E::Match(scrutinee, arms) => {
            expr(scrutinee, f);
            for arm in arms {
                arm.guard.iter().for_each(|guard| expr(guard, f));
                expr(&arm.body, f);
            }
        }
        E::Index(left, right) | E::Binary(_, left, right) => {
            expr(left, f);
            expr(right, f);
        }
        E::Slice(base, bounds) => {
            expr(base, f);
            bounds.iter().flatten().for_each(|bound| expr(bound, f));
        }
        E::Loan(place) | E::CursorNext(place, _) | E::PlaceValue(place) => steps(&place.path, f),
        E::Region(place, bounds) => {
            steps(&place.path, f);
            bounds.iter().flatten().for_each(|bound| expr(bound, f));
        }
        E::Mutate(_, place, values) => {
            steps(&place.path, f);
            values.iter().for_each(|value| expr(value, f));
        }
        E::Map(entries) => entries.iter().for_each(|(key, value)| {
            expr(key, f);
            expr(value, f);
        }),
        E::CallValue(callee, values) => {
            expr(callee, f);
            values.iter().for_each(|value| expr(value, f));
        }
        E::Select(arms, otherwise) => {
            for arm in arms {
                expr(&arm.operation, f);
                expr(&arm.body, f);
            }
            otherwise.iter().for_each(|otherwise| expr(otherwise, f));
        }
        E::If(condition, then, otherwise) => {
            expr(condition, f);
            each_expr(then, f);
            otherwise.iter().for_each(|otherwise| expr(otherwise, f));
        }
        E::Block(block) => each_expr(block, f),
        E::Recover(block, caught) => {
            each_expr(block, f);
            expr(caught, f);
        }
        E::Within(limit, block, failure) => {
            expr(limit, f);
            each_expr(block, f);
            expr(failure, f);
        }
        E::Interpolate(segments) => segments.iter().for_each(|segment| {
            if let hir::Segment::Value(value) = segment {
                expr(value, f);
            }
        }),
        E::Assert { condition, parts, detail, .. } => {
            expr(condition, f);
            parts.iter().for_each(|part| expr(part, f));
            detail.iter().for_each(|detail| expr(detail, f));
        }
    }
}
