//! The vectors of `std.simd` (standard library 14.46): their types, the
//! functions that make them, their methods and their operators, each an
//! intrinsic of the compiler.

use super::*;
use crate::simd::{LANE_TYPES, Lanes, SimdOp};
use crate::types::Width;

/// A vector or mask type: its width, its lanes' type, their kind, and
/// whether it is a mask.
#[derive(Copy, Clone)]
pub(super) struct Vectored {
    pub width: Width,
    pub lane: Ty,
    pub lanes: Lanes,
    pub mask: bool,
}

const WIDTHS: [Width; 4] = [Width::W128, Width::W256, Width::W512, Width::Widest];

impl<'a> Checker<'a> {
    /// A field is no place for a `Vector[T]` or a `Mask[T]` (18.11).
    pub(super) fn vector_field(&mut self, name: &str, ty: Ty, span: Span) {
        if let Some(vector) = self.types.reaches_widest(ty) {
            let vector = self.types.display(vector);
            let what = match self.types.display(ty) {
                shown if shown == vector => format!("the field `{name}` is a `{vector}`"),
                shown => format!("the field `{name}`, a `{shown}`, keeps a `{vector}`"),
            };
            self.diags.error("E1120", span, crate::simd::kept(&what, &vector));
        }
    }

    /// The first place of each function that keeps a `Vector[T]` or a
    /// `Mask[T]` where it may not be: only locals, parameters and results
    /// hold one (18.11).
    pub(super) fn vector_places(&mut self, functions: &[hir::Function]) {
        let types = &self.types;
        let mut holds: HashMap<Ty, Option<Ty>> = HashMap::new();
        let mut found = Vec::new();
        for function in functions {
            let mut first: Option<(Span, Ty, Ty)> = None;
            let mut note = |ty: Ty, span: Span| {
                if span == Span::default() || first.is_some_and(|(at, ..)| at.start <= span.start) {
                    return;
                }
                if let Some(vector) = *holds.entry(ty).or_insert_with(|| types.holds_widest(ty)) {
                    first = Some((span, ty, vector));
                }
            };
            // A closure's body holds the closure: where it is made says it.
            for local in function.locals.iter().filter(|local| !types.is_closure(local.ty)) {
                note(local.ty, local.span);
            }
            super::reach::each_expr(&function.body, &mut |e| {
                if !matches!(e.kind, hir::ExprKind::Local(_)) || !types.is_closure(e.ty) {
                    note(e.ty, e.span)
                }
            });
            found.extend(first);
        }
        let mut reported = HashSet::new();
        for (span, ty, vector) in found {
            if reported.insert(span) {
                let vector = self.types.display(vector);
                let what = match self.types.is_closure(ty) {
                    true => format!("this closure captures a `{vector}`"),
                    false => format!("`{}` keeps a `{vector}`", self.types.display(ty)),
                };
                self.diags.error("E1120", span, crate::simd::kept(&what, &vector));
            }
        }
    }
}

/// A parameter of an operation: its name, type, mode and default.
type Param<'s> = (&'s str, Ty, ast::Mode, Option<hir::Expr>);

impl<'a> Checker<'a> {
    /// `simd.Vector128[T]` and the other types of `std.simd`.
    pub(super) fn simd_type(&mut self, name: &ast::Ident, args: &[Ty], span: Span) -> Ty {
        let found = WIDTHS.into_iter().find_map(|width| match width.names() {
            (vector, _) if vector == name.name => Some((width, false)),
            (_, mask) if mask == name.name => Some((width, true)),
            _ => None,
        });
        let Some((width, mask)) = found else {
            let message = format!(
                "module `simd` has no type `{}`: its types are `Vector128`, `Vector256`, `Vector512` and `Vector`, and their masks, `Mask128` and the others",
                name.name
            );
            self.diags.error("E1001", name.span, message);
            return Ty::ERROR;
        };
        let [lane] = args else {
            let message = format!("`simd.{}` takes one type argument, its lanes' type: one of {LANE_TYPES}", name.name);
            self.diags.error("E1104", span, message);
            return Ty::ERROR;
        };
        if self.types.has_error(*lane) {
            return Ty::ERROR;
        }
        if Lanes::of(&self.types, *lane).is_none() {
            let message = format!("the lanes of a vector are {LANE_TYPES}, not {}", self.display(*lane));
            self.diags.error("E1102", span, message);
            return Ty::ERROR;
        }
        let kind = if mask { TyKind::Mask(width, *lane) } else { TyKind::Vector(width, *lane) };
        self.types.intern(kind)
    }

    /// What a vector or mask type is, or `None` for any other.
    pub(super) fn vectored(&self, ty: Ty) -> Option<Vectored> {
        let (width, lane, mask) = match *self.types.kind(self.types.repr(ty)) {
            TyKind::Vector(width, lane) => (width, lane, false),
            TyKind::Mask(width, lane) => (width, lane, true),
            _ => return None,
        };
        Some(Vectored { width, lane, lanes: Lanes::of(&self.types, lane)?, mask })
    }

    fn vector_of(&mut self, width: Width, lane: Ty) -> Ty {
        self.types.intern(TyKind::Vector(width, lane))
    }

    fn mask_of(&mut self, width: Width, lane: Ty) -> Ty {
        self.types.intern(TyKind::Mask(width, lane))
    }

    fn simd_call(op: SimdOp, values: Vec<hir::Expr>, ty: Ty, span: Span) -> hir::Expr {
        Self::mk(hir::ExprKind::Call(Callee::Std(StdFn::Simd(op)), Vec::new(), values), ty, span)
    }

    /// The signature of an operation of `std.simd`: its receiver, if it has
    /// one, then each parameter with its name, mode and default.
    fn simd_signature(&mut self, name: String, receiver: Option<Ty>, params: Vec<Param>, result: Ty) -> Signature {
        let mut signature = Signature::simple(Vec::new(), result);
        signature.name = name;
        if let Some(receiver) = receiver {
            signature.params.push(receiver);
            signature.names.push("self".into());
            signature.modes.push(ast::Mode::Value);
            signature.defaults.push(None);
            signature.receiver = Some(ast::Mode::Value);
        }
        for (name, ty, mode, default) in params {
            signature.params.push(ty);
            signature.names.push(name.into());
            signature.modes.push(mode);
            signature.defaults.push(default);
        }
        signature
    }

    /// The vector type a function of `simd.Vector128` and the others is
    /// of: the one written, or the one expected.
    pub(super) fn simd_owner(&mut self, width: Width, mask: bool, expected: Option<Ty>) -> Option<Ty> {
        let expected = expected?;
        let found = self.vectored(expected)?;
        (found.width == width && found.mask == mask).then_some(expected)
    }

    /// A function of a vector type: `simd.Vector128[Float32].zero()`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn simd_static(
        &mut self,
        cx: &mut FnCx,
        owner: Ty,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let Some(own) = self.vectored(owner) else { return Self::error_expr(span) };
        let shown = self.display(owner);
        if own.mask {
            let message =
                format!("{shown} has no functions of its own: a mask comes from comparing vectors, as `a.less(b)`");
            self.diags.error("E1001", method.span, message);
            return Self::error_expr(span);
        }
        let count = crate::simd::count(own.width, own.lanes);
        let int = |value: i128| Self::mk(hir::ExprKind::Int(value), Ty::INT, Span::default());
        let (op, params, result) = match method.name.as_str() {
            "lanes" | "accelerated" => {
                if !args.is_empty() {
                    return self.arity_error(span, 0, args.len());
                }
                return match method.name.as_str() {
                    // `Vector[T]`'s are known once the code's width is.
                    "lanes" if own.width == Width::Widest => Self::mk(hir::ExprKind::Lanes(owner), Ty::INT, span),
                    "lanes" => Self::mk(hir::ExprKind::Int(count as i128), Ty::INT, span),
                    _ => Self::simd_call(SimdOp::Accelerated(own.width), Vec::new(), Ty::BOOL, span),
                };
            }
            "zero" => (SimdOp::Zero, Vec::new(), owner),
            "all" => (SimdOp::Splat, vec![("value", own.lane, ast::Mode::Value, None)], owner),
            "of" if own.width == Width::Widest => {
                let (name, lane) = (self.types.display(owner), self.types.display(own.lane));
                let message = format!(
                    "{shown} has no function `of`: it can have more lanes in the version of a function for a \
                     processor with wider vectors; `{name}.all(value)` gives each lane one value, `{name}.load(ref \
                     values)` takes them from an array, and `simd.Vector128[{lane}].of(...)` takes as many as 128 bits hold"
                );
                self.diags.error("E1001", method.span, message);
                return Self::error_expr(span);
            }
            "of" => return self.simd_of(cx, owner, own, count, args, span),
            "load" => {
                let array = self.types.intern(TyKind::Array(own.lane));
                let params =
                    vec![("values", array, ast::Mode::Ref, None), ("at", Ty::INT, ast::Mode::Value, Some(int(0)))];
                (SimdOp::Load, params, owner)
            }
            "joined" if own.width != Width::W128 && own.width != Width::Widest => {
                let half = if own.width == Width::W512 { Width::W256 } else { Width::W128 };
                let half = self.vector_of(half, own.lane);
                let params = vec![("lower", half, ast::Mode::Value, None), ("upper", half, ast::Mode::Value, None)];
                (SimdOp::Joined, params, owner)
            }
            name => {
                let message = format!(
                    "{shown} has no function `{name}`: it has `zero`, `all`, {}`load`, `lanes` and `accelerated`{}",
                    if own.width == Width::Widest { "" } else { "`of`, " },
                    if matches!(own.width, Width::W256 | Width::W512) { ", and `joined`" } else { "" }
                );
                self.diags.error("E1001", method.span, message);
                return Self::error_expr(span);
            }
        };
        let signature = self.simd_signature(format!("{shown}.{}", method.name), None, params, result);
        let target = CallTarget::new(Callee::Std(StdFn::Simd(op)), signature, expected);
        self.call_signature(cx, target, None, args, span)
    }

    /// `of(values...)`: exactly as many values as lanes, written; or one
    /// array spread, whose length the program checks as it runs.
    fn simd_of(
        &mut self,
        cx: &mut FnCx,
        owner: Ty,
        own: Vectored,
        count: u32,
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        if let [arg] = args
            && arg.spread
        {
            let array = self.types.intern(TyKind::Array(own.lane));
            let value = self.expr(cx, &arg.value, Some(array));
            let value = self.coerce(value, array);
            return Self::simd_call(SimdOp::Of, vec![value], owner, span);
        }
        if args.len() != count as usize {
            let message = format!(
                "`of` takes {count} values, one for each lane of {}, and {} are given",
                self.display(owner),
                args.len()
            );
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        let mut values = Vec::new();
        for arg in args {
            let value = self.expr(cx, &arg.value, Some(own.lane));
            values.push(self.coerce(value, own.lane));
        }
        Self::simd_call(SimdOp::Of, values, owner, span)
    }

    /// A method of a vector or a mask.
    pub(super) fn simd_method(
        &mut self,
        cx: &mut FnCx,
        value: hir::Expr,
        method: &ast::Ident,
        args: &[ast::Arg],
        expected: Option<Ty>,
        span: Span,
    ) -> hir::Expr {
        let ty = value.ty;
        let own = self.vectored(ty).expect("a vector");
        let shown = self.display(ty);
        let lanes = own.lanes;
        let int = |value: i128| Self::mk(hir::ExprKind::Int(value), Ty::INT, Span::default());
        let value_param = |name: &'static str, ty: Ty| (name, ty, ast::Mode::Value, None);
        let mask = self.mask_of(own.width, own.lane);
        let float = lanes.is_float();
        let integer = matches!(lanes, Lanes::Signed(_) | Lanes::Unsigned(_));
        let wide = matches!(own.width, Width::W256 | Width::W512);
        // Each method: its operation, its parameters, its result, and
        // whether the lanes have it.
        let found: Option<(SimdOp, Vec<Param>, Ty, bool)> = match own.mask {
            true => match method.name.as_str() {
                "choose" => {
                    let vector = self.vector_of(own.width, own.lane);
                    let params = vec![value_param("whenTrue", vector), value_param("whenFalse", vector)];
                    Some((SimdOp::Choose, params, vector, true))
                }
                "any" => Some((SimdOp::Any, Vec::new(), Ty::BOOL, true)),
                "all" => Some((SimdOp::All, Vec::new(), Ty::BOOL, true)),
                "count" => Some((SimdOp::Count, Vec::new(), Ty::INT, true)),
                "bits" => Some((SimdOp::Bits, Vec::new(), Ty::int(IntTy::U64), true)),
                "lane" => Some((SimdOp::MaskLane, vec![value_param("index", Ty::INT)], Ty::BOOL, true)),
                _ => None,
            },
            false => match method.name.as_str() {
                "lane" => Some((SimdOp::Lane, vec![value_param("index", Ty::INT)], own.lane, true)),
                "withLane" => {
                    let params = vec![value_param("index", Ty::INT), value_param("value", own.lane)];
                    Some((SimdOp::WithLane, params, ty, true))
                }
                "toArray" => {
                    let array = self.types.intern(TyKind::Array(own.lane));
                    Some((SimdOp::ToArray, Vec::new(), array, true))
                }
                "store" => {
                    let array = self.types.intern(TyKind::Array(own.lane));
                    let params =
                        vec![("into", array, ast::Mode::Mut, None), ("at", Ty::INT, ast::Mode::Value, Some(int(0)))];
                    Some((SimdOp::Store, params, Ty::UNIT, true))
                }
                "min" => Some((SimdOp::Min, vec![value_param("other", ty)], ty, lanes.arithmetic())),
                "max" => Some((SimdOp::Max, vec![value_param("other", ty)], ty, lanes.arithmetic())),
                "abs" => Some((SimdOp::Abs, Vec::new(), ty, lanes.arithmetic())),
                "saturatingAdd" => Some((SimdOp::SaturatingAdd, vec![value_param("other", ty)], ty, integer)),
                "saturatingSub" => Some((SimdOp::SaturatingSub, vec![value_param("other", ty)], ty, integer)),
                "sqrt" => Some((SimdOp::Sqrt, Vec::new(), ty, float)),
                "floor" => Some((SimdOp::Floor, Vec::new(), ty, float)),
                "ceil" => Some((SimdOp::Ceil, Vec::new(), ty, float)),
                "round" => Some((SimdOp::Round, Vec::new(), ty, float)),
                "truncate" => Some((SimdOp::Truncate, Vec::new(), ty, float)),
                "fma" => {
                    let params = vec![value_param("factor", ty), value_param("addend", ty)];
                    Some((SimdOp::Fma, params, ty, float))
                }
                "equal" => Some((SimdOp::Equal, vec![value_param("other", ty)], mask, true)),
                "notEqual" => Some((SimdOp::NotEqual, vec![value_param("other", ty)], mask, true)),
                "less" => Some((SimdOp::Less, vec![value_param("other", ty)], mask, true)),
                "lessOrEqual" => Some((SimdOp::LessOrEqual, vec![value_param("other", ty)], mask, true)),
                "greater" => Some((SimdOp::Greater, vec![value_param("other", ty)], mask, true)),
                "greaterOrEqual" => Some((SimdOp::GreaterOrEqual, vec![value_param("other", ty)], mask, true)),
                "sum" => Some((SimdOp::Sum, Vec::new(), own.lane, lanes.arithmetic())),
                "minLane" => Some((SimdOp::MinLane, Vec::new(), own.lane, lanes.arithmetic())),
                "maxLane" => Some((SimdOp::MaxLane, Vec::new(), own.lane, lanes.arithmetic())),
                "reversed" => Some((SimdOp::Reversed, Vec::new(), ty, true)),
                "swizzle" => {
                    let bytes = own.width == Width::W128 && lanes.bits() == 8;
                    Some((SimdOp::Swizzle, vec![value_param("indexes", ty)], ty, bytes))
                }
                "lower" | "upper" => {
                    let half = if own.width == Width::W512 { Width::W256 } else { Width::W128 };
                    let half = self.vector_of(half, own.lane);
                    let op = if method.name == "lower" { SimdOp::Lower } else { SimdOp::Upper };
                    Some((op, Vec::new(), half, wide))
                }
                "to" | "cast" => {
                    let message = format!("`{}` takes the lanes' new type: `vector.{}[U]()`", method.name, method.name);
                    self.diags.error("E1104", span, message);
                    return Self::error_expr(span);
                }
                _ => None,
            },
        };
        let Some((op, params, result, has)) = found else {
            let message = format!("{shown} has no method `{}`: `tondo api std.simd` lists what it has", method.name);
            self.diags.error("E1001", method.span, message);
            return Self::error_expr(span);
        };
        if !has {
            let message = format!("{shown} has no `{}`, which {}", method.name, Self::simd_needs(op));
            self.diags.error("E1102", method.span, message);
            return Self::error_expr(span);
        }
        let signature = self.simd_signature(format!("{shown}.{}", method.name), Some(ty), params, result);
        self.dot_call(cx, Callee::Std(StdFn::Simd(op)), signature, HashMap::new(), value, method, args, expected, span)
    }

    /// What lanes an operation needs, as its error says.
    fn simd_needs(op: SimdOp) -> &'static str {
        match op {
            SimdOp::Sqrt | SimdOp::Floor | SimdOp::Ceil | SimdOp::Round | SimdOp::Truncate | SimdOp::Fma => {
                "vectors of `Float32` and `Float` have"
            }
            SimdOp::SaturatingAdd | SimdOp::SaturatingSub => "vectors of integers have",
            SimdOp::Swizzle => "a `Vector128` of `Byte`, `UInt8` or `Int8` has",
            SimdOp::Lower | SimdOp::Upper => "a `Vector256` and a `Vector512` have",
            _ => "a vector of `Byte` has not: it has `&`, `|`, `^`, `~` and the shifts, and no arithmetic",
        }
    }

    /// `vector.to[U]()` and `vector.cast[U]()`.
    pub(super) fn simd_converted(
        &mut self,
        value: hir::Expr,
        method: &ast::Ident,
        types: &[Ty],
        args: &[ast::Arg],
        span: Span,
    ) -> hir::Expr {
        let own = self.vectored(value.ty).expect("a vector");
        let shown = self.display(value.ty);
        if own.mask || !matches!(method.name.as_str(), "to" | "cast") {
            let message = format!("{shown} has no method `{}` with type parameters", method.name);
            self.diags.error("E1104", span, message);
            return Self::error_expr(span);
        }
        if !args.is_empty() {
            return self.arity_error(span, 0, args.len());
        }
        let [target] = types else {
            let message = format!("`{}` takes one type argument, the lanes' new type", method.name);
            self.diags.error("E1104", span, message);
            return Self::error_expr(span);
        };
        if self.types.has_error(*target) {
            return Self::error_expr(span);
        }
        let Some(to) = Lanes::of(&self.types, *target) else {
            let message = format!("the lanes of a vector are {LANE_TYPES}, not {}", self.display(*target));
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        };
        let op = if method.name == "to" { SimdOp::To } else { SimdOp::Cast };
        if op == SimdOp::To && (to.bits() != own.lanes.bits() || to == Lanes::Byte || own.lanes == Lanes::Byte) {
            let same: Vec<&str> = match own.lanes.bits() {
                32 => vec!["`Int32`", "`UInt32`", "`Float32`"],
                64 => vec!["`Int`", "`UInt`", "`Float`"],
                _ => Vec::new(),
            };
            let message = match same.is_empty() {
                true => format!(
                    "`to` converts lanes of 32 or 64 bits; {shown} has others: `cast[U]()` reads its bits as lanes of `U`"
                ),
                false => format!(
                    "`to` converts each lane to a type as wide: of {shown}, {}; `cast[U]()` reads the bits as lanes of any `U`",
                    same.join(", ")
                ),
            };
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        let result = self.vector_of(own.width, *target);
        Self::simd_call(op, vec![value], result, span)
    }

    /// `left op right` where one is a vector or a mask: lane by lane, with
    /// a `T` on either side standing for every lane (rule 8).
    pub(super) fn simd_binary(&mut self, op: BinOp, left: hir::Expr, right: hir::Expr, span: Span) -> hir::Expr {
        let (vector, other_is_left) = match (self.vectored(left.ty), self.vectored(right.ty)) {
            (Some(_), _) => (left.ty, false),
            (None, Some(_)) => (right.ty, true),
            (None, None) => unreachable!("one operand is a vector"),
        };
        let own = self.vectored(vector).expect("a vector");
        let shown = self.display(vector);
        if self.types.has_error(left.ty) || self.types.has_error(right.ty) {
            return Self::error_expr(span);
        }
        if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
            let method = match op {
                BinOp::Lt => "less",
                BinOp::Le => "lessOrEqual",
                BinOp::Gt => "greater",
                _ => "greaterOrEqual",
            };
            let message = format!(
                "`{}` gives one `Bool`, and vectors compare lane by lane: `a.{method}(b)` gives a mask",
                op.as_str()
            );
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        let shift = matches!(op, BinOp::Shl | BinOp::Shr);
        let (simd_op, allowed) = match op {
            BinOp::Add => (SimdOp::Add, !own.mask && own.lanes.arithmetic()),
            BinOp::Sub => (SimdOp::Sub, !own.mask && own.lanes.arithmetic()),
            BinOp::Mul => (SimdOp::Mul, !own.mask && own.lanes.arithmetic()),
            BinOp::Div => (SimdOp::Div, !own.mask && own.lanes.is_float()),
            BinOp::BitAnd => (SimdOp::And, own.mask || own.lanes.bitwise()),
            BinOp::BitOr => (SimdOp::Or, own.mask || own.lanes.bitwise()),
            BinOp::BitXor => (SimdOp::Xor, own.mask || own.lanes.bitwise()),
            BinOp::Shl => (SimdOp::Shl, !own.mask && own.lanes.bitwise() && !other_is_left),
            BinOp::Shr => (SimdOp::Shr, !own.mask && own.lanes.bitwise() && !other_is_left),
            BinOp::Eq | BinOp::Ne => (SimdOp::Same, true),
            _ => (SimdOp::Same, false),
        };
        if !allowed {
            let message =
                format!("operator `{}` cannot be applied to {shown}: {}", op.as_str(), Self::simd_operators(own));
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        if shift {
            let count = self.coerce(right, Ty::INT);
            return Self::simd_call(simd_op, vec![left, count], vector, span);
        }
        // A `T` stands for a vector with it in every lane.
        let widen = |this: &mut Self, side: hir::Expr| -> hir::Expr {
            if this.vectored(side.ty).is_some() {
                return this.coerce(side, vector);
            }
            let lane = this.coerce(side, own.lane);
            Self::simd_call(SimdOp::Splat, vec![lane], vector, span)
        };
        if op == BinOp::Eq || op == BinOp::Ne {
            if self.vectored(left.ty).is_none() || self.vectored(right.ty).is_none() {
                let message = format!("`{}` compares two values of {shown}", op.as_str());
                self.diags.error("E1102", span, message);
                return Self::error_expr(span);
            }
            let right = self.coerce(right, vector);
            let same = Self::simd_call(SimdOp::Same, vec![left, right], Ty::BOOL, span);
            return match op {
                BinOp::Eq => same,
                _ => Self::mk(hir::ExprKind::Unary(UnaryOp::Not, Box::new(same)), Ty::BOOL, span),
            };
        }
        let (left, right) = (widen(self, left), widen(self, right));
        Self::simd_call(simd_op, vec![left, right], vector, span)
    }

    /// The operators a vector or a mask has, as an error lists them.
    fn simd_operators(own: Vectored) -> &'static str {
        match (own.mask, own.lanes) {
            (true, _) => "a mask has `&`, `|`, `^` and `~`",
            (false, Lanes::Byte) => "a vector of `Byte` has `&`, `|`, `^`, `~` and the shifts, and no arithmetic",
            (false, Lanes::Float(_)) => "a vector of floats has `+`, `-`, `*`, `/` and unary `-`",
            (false, Lanes::Signed(_)) => {
                "a vector of integers has `+`, `-`, `*`, unary `-`, `&`, `|`, `^`, `~` and the shifts by an `Int`"
            }
            (false, Lanes::Unsigned(_)) => {
                "a vector of unsigned integers has `+`, `-`, `*`, `&`, `|`, `^`, `~` and the shifts by an `Int`"
            }
        }
    }

    /// `-vector` and `~vector`.
    pub(super) fn simd_unary(&mut self, op: UnaryOp, value: hir::Expr, span: Span) -> hir::Expr {
        let own = self.vectored(value.ty).expect("a vector");
        let ty = value.ty;
        let (simd_op, allowed) = match op {
            UnaryOp::Neg => (SimdOp::Neg, !own.mask && matches!(own.lanes, Lanes::Signed(_) | Lanes::Float(_))),
            UnaryOp::BitNot => (SimdOp::Not, own.mask || own.lanes.bitwise()),
            UnaryOp::Not => (SimdOp::Not, false),
        };
        if !allowed {
            let shown = self.display(ty);
            let written = match op {
                UnaryOp::Neg => "-",
                UnaryOp::BitNot => "~",
                UnaryOp::Not => "not",
            };
            let message = format!("operator `{written}` cannot be applied to {shown}: {}", Self::simd_operators(own));
            self.diags.error("E1102", span, message);
            return Self::error_expr(span);
        }
        Self::simd_call(simd_op, vec![value], ty, span)
    }

    /// `simd.Vector128`, `simd.Mask` and the others, written without their
    /// lanes' type: its width and whether it is a mask.
    pub(super) fn simd_named(&self, segments: &[ast::Ident]) -> Option<(Width, bool)> {
        let [module, name] = segments else { return None };
        if !matches!(self.names.get(&module.name), Some(ModuleName::Module(StdModule::Simd))) {
            return None;
        }
        WIDTHS.into_iter().find_map(|width| match width.names() {
            (vector, _) if vector == name.name => Some((width, false)),
            (_, mask) if mask == name.name => Some((width, true)),
            _ => None,
        })
    }
}
