//! Syntax tree produced by the parser.

use crate::source::Span;

#[derive(Clone, Debug)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct Module {
    pub imports: Vec<Import>,
    pub items: Vec<Item>,
    /// Top-level statements of a root script (spec 20.3).
    pub statements: Vec<Stmt>,
}

#[derive(Debug)]
pub struct Import {
    pub path: Vec<Ident>,
    pub alias: Option<Ident>,
    pub span: Span,
}

#[derive(Debug)]
pub enum Item {
    Fn(FnDecl),
    Type(TypeDecl),
    Alias(AliasDecl),
    Enum(EnumDecl),
    Trait(TraitDecl),
    Impl(ImplDecl),
}

/// `trait Name[Params] { fn ... }`: required methods have no body (12.3).
#[derive(Debug)]
pub struct TraitDecl {
    pub public: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

/// `impl[Binders] Trait[Args] for Target { fn ... }` (12.4).
#[derive(Debug)]
pub struct ImplDecl {
    pub generics: Vec<GenericParam>,
    pub trait_ref: TypeExpr,
    pub target: TypeExpr,
    pub methods: Vec<FnDecl>,
    pub span: Span,
}

/// `T` or `T: Bound + Bound` in a generic declaration (12.1).
#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: Ident,
    pub bounds: Vec<TypeExpr>,
}

#[derive(Debug)]
pub struct EnumDecl {
    pub public: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub variants: Vec<VariantDecl>,
    pub span: Span,
}

#[derive(Debug)]
pub struct VariantDecl {
    pub name: Ident,
    pub payload: VariantPayload,
}

#[derive(Debug)]
pub enum VariantPayload {
    Unit,
    Tuple(Vec<TypeExpr>),
    Record(Vec<FieldDecl>),
}

#[derive(Debug)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum PatternKind {
    Wildcard,
    Binding(Ident),
    /// An integer, boolean, string or `()` literal, possibly negated.
    Literal(Expr),
    /// `none`
    None,
    Tuple(Vec<Pattern>),
    /// `Enum.Variant(...)`, `some(...)`, `ok(...)`, `err(...)` or `Newtype(...)`.
    Constructor {
        path: Vec<Ident>,
        args: Vec<Pattern>,
    },
    /// `Type { field, field: pattern, .. }`
    Record {
        path: Vec<Ident>,
        fields: Vec<FieldPattern>,
        rest: bool,
    },
    /// A qualified payload-free variant: `Enum.Variant`.
    Path(Vec<Ident>),
    /// `[a, b]`, or `[a, b, ..]` and `[a, b, ..rest]` for arrays at least
    /// that long; `rest` is `Some(None)` for a bare `..`.
    Array {
        items: Vec<Pattern>,
        rest: Option<Option<Ident>>,
    },
    /// `Array[Int](pattern)`: a union member given by a generic type (9.6).
    Member {
        ty: TypeExpr,
        inner: Box<Pattern>,
    },
}

#[derive(Debug)]
pub struct FieldPattern {
    pub name: Ident,
    /// `None` for the shorthand `name`, which binds `name`.
    pub pattern: Option<Pattern>,
}

#[derive(Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: ArmBody,
    pub span: Span,
}

#[derive(Debug)]
pub enum ArmBody {
    Expr(Expr),
    /// `return`, `fail`, `break` or `continue` written directly after `=>`.
    Transfer(Box<Stmt>),
}

/// `type Name = { fields }` (record) or `type Name = T` (newtype).
#[derive(Debug)]
pub struct TypeDecl {
    pub public: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub body: TypeBody,
    pub span: Span,
}

#[derive(Debug)]
pub enum TypeBody {
    Record(Vec<FieldDecl>),
    Newtype(TypeExpr),
}

#[derive(Debug)]
pub struct FieldDecl {
    pub private: bool,
    pub name: Ident,
    pub ty: TypeExpr,
}

/// `alias Name = T`: a transparent name for `T`.
#[derive(Debug)]
pub struct AliasDecl {
    pub public: bool,
    pub name: Ident,
    pub target: TypeExpr,
    pub span: Span,
}

#[derive(Debug)]
pub struct FnDecl {
    pub public: bool,
    pub owner: Option<Ident>,
    /// The binders of a generic owner: `fn Pair[A, B].swapped(self)`.
    pub owner_generics: Vec<GenericParam>,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub result: Option<TypeExpr>,
    pub suspends: bool,
    /// `None` for a required method of a trait.
    pub body: Option<Block>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Param {
    pub name: Ident,
    /// `None` for a receiver: `self`, `mut self` or `var self`.
    pub ty: Option<TypeExpr>,
    pub mode: Mode,
    /// `name: ...T`
    pub variadic: bool,
    pub span: Span,
}

/// How a parameter or argument is passed: by value, or as a `ref`, `mut` or
/// `var` loan (11.3, 16.3). A plain `self` receiver is a `ref` loan.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Value,
    Ref,
    Mut,
    Var,
}

impl Mode {
    pub fn keyword(self) -> &'static str {
        match self {
            Mode::Value => "",
            Mode::Ref => "ref",
            Mode::Mut => "mut",
            Mode::Var => "var",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypeExpr {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    Path {
        segments: Vec<Ident>,
        args: Vec<TypeExpr>,
    },
    Optional(Box<TypeExpr>),
    /// `T ! E`; `ok` is `None` for the `!E` shorthand.
    Result {
        ok: Option<Box<TypeExpr>>,
        err: Box<TypeExpr>,
    },
    Union(Vec<TypeExpr>),
    Tuple(Vec<TypeExpr>),
    /// `impl A + B`: an opaque function result (12.8).
    Opaque(Vec<TypeExpr>),
    /// `fn(A, B): R`, the uniform function type (11.7).
    Fn {
        params: Vec<TypeExpr>,
        variadic: bool,
        result: Option<Box<TypeExpr>>,
    },
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum StmtKind {
    Let {
        mutable: bool,
        pattern: Pattern,
        ty: Option<TypeExpr>,
        value: Option<Expr>,
    },
    /// `target op= value`; `op` is `None` for plain `=`.
    Assign {
        target: Expr,
        op: Option<BinOp>,
        value: Expr,
    },
    /// `_ = value`
    Discard(Expr),
    Return(Option<Expr>),
    Fail(Expr),
    Break,
    Continue,
    For {
        header: ForHeader,
        body: Block,
    },
    Expr(Expr),
}

#[derive(Debug)]
pub enum ForHeader {
    Infinite,
    Condition(Expr),
    In { pattern: Pattern, source: Expr },
}

#[derive(Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum ExprKind {
    Int {
        value: u64,
        suffix: Option<&'static str>,
    },
    Float {
        text: String,
        suffix: Option<&'static str>,
    },
    Bool(bool),
    Char(char),
    Str(Vec<StrSegment>),
    Unit,
    /// `a.b.c` before resolution decides between module members and fields.
    Path(Vec<Ident>),
    Field(Box<Expr>, Ident),
    TupleSlot(Box<Expr>, u32),
    /// `Path { field: value, shorthand }` or `Path[Args] { ... }`
    Record {
        path: Vec<Ident>,
        type_args: Vec<TypeExpr>,
        fields: Vec<FieldInit>,
    },
    /// `base with { field: value }`
    With {
        base: Box<Expr>,
        updates: Vec<FieldInit>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
    },
    Unary(UnaryOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    If {
        condition: Box<Expr>,
        then: Block,
        otherwise: Option<Box<Expr>>,
    },
    Block(Block),
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Tuple(Vec<Expr>),
    /// `[a, b, c]`
    Array(Vec<Expr>),
    /// `[key: value, ...]` or `[:]`
    Map(Vec<(Expr, Expr)>),
    /// `Set[a, b]`
    Set(Vec<Expr>),
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    /// `base[start:end:step]`, each part optional.
    Slice {
        base: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    /// Explicit type arguments of a call: `Array.new[Int]`.
    Generic {
        base: Box<Expr>,
        args: Vec<TypeExpr>,
    },
    Try(Box<Expr>),
    /// `(params): Result { body }` (11.8).
    Closure(Box<Closure>),
    /// `none`
    None,
    /// `some(value)`, `ok(value)` or `err(value)`: the constructor's name.
    Construct(&'static str, Box<Expr>),
    /// Placeholder after a reported syntax error.
    Error,
}

#[derive(Debug)]
pub struct Closure {
    pub params: Vec<ClosureParam>,
    pub result: Option<TypeExpr>,
    pub body: Block,
}

#[derive(Debug)]
pub struct ClosureParam {
    pub name: Ident,
    /// May be omitted when an expected function type gives it.
    pub ty: Option<TypeExpr>,
    pub variadic: bool,
}

#[derive(Debug)]
pub struct FieldInit {
    pub name: Ident,
    /// `None` for the shorthand `name`, which reads the binding `name`.
    pub value: Option<Expr>,
}

#[derive(Debug)]
pub enum StrSegment {
    Text(String),
    Expr(Expr),
}

#[derive(Debug)]
pub struct Arg {
    pub name: Option<Ident>,
    pub value: Expr,
    /// `ref value`, `mut value` or `var value`
    pub mode: Mode,
    /// `...values`
    pub spread: bool,
    pub span: Span,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Range,
    RangeInclusive,
    In,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::Range => "..",
            BinOp::RangeInclusive => "..=",
            BinOp::In => "in",
        }
    }
}
