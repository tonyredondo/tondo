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
    /// Whether the file is production code, a unit test companion or an
    /// integration test root (testing 5.1).
    pub class: SourceClass,
    /// After linking: the path of each module, by its number.
    pub module_paths: Vec<Vec<String>>,
    /// In a test build, the name of the package under test.
    pub test_package: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum SourceClass {
    #[default]
    Production,
    Unit,
    Integration,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub path: Vec<Ident>,
    pub alias: Option<Ident>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Item {
    Const(ConstDecl),
    Fn(FnDecl),
    Type(TypeDecl),
    Alias(AliasDecl),
    Enum(EnumDecl),
    Trait(TraitDecl),
    Impl(ImplDecl),
    Test(TestNode),
    Derive(DeriveDecl),
}

/// `derive[T] module.Trait + other.Trait for Type[T]`: asks each trait's
/// provider for an `impl` for a type of this module (27.2).
#[derive(Clone, Debug)]
pub struct DeriveDecl {
    pub generics: Vec<GenericParam>,
    pub traits: Vec<TypeExpr>,
    pub target: TypeExpr,
    pub span: Span,
}

/// A `test` or a `suite` (testing 3).
#[derive(Clone, Debug)]
pub enum TestNode {
    Test(TestDecl),
    Suite(SuiteDecl),
}

impl TestNode {
    pub fn name(&self) -> &Ident {
        match self {
            TestNode::Test(t) => &t.name,
            TestNode::Suite(s) => &s.name,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            TestNode::Test(t) => t.span,
            TestNode::Suite(s) => s.span,
        }
    }
}

/// `test name { body }`: a leaf the runner runs.
#[derive(Clone, Debug)]
pub struct TestDecl {
    pub name: Ident,
    pub body: Block,
    pub span: Span,
    /// The module and the class of the file that declares it, set when
    /// modules are linked.
    pub module: u32,
    pub class: SourceClass,
}

/// `suite name { setup members }`: its setup runs before its members and
/// its `defer`s after them.
#[derive(Clone, Debug)]
pub struct SuiteDecl {
    pub name: Ident,
    pub setup: Vec<Stmt>,
    pub members: Vec<TestNode>,
    pub span: Span,
    pub module: u32,
    pub class: SourceClass,
}

/// `trait Name[Params] { fn ... }`: required methods have no body (12.3).
#[derive(Clone, Debug)]
pub struct TraitDecl {
    pub public: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub methods: Vec<FnDecl>,
    pub span: Span,
    /// The module that declares it, numbered when modules are linked; the
    /// root module is 0.
    pub module: u32,
}

/// `impl[Binders] Trait[Args] for Target { fn ... }` (12.4).
#[derive(Clone, Debug)]
pub struct ImplDecl {
    pub generics: Vec<GenericParam>,
    pub trait_ref: TypeExpr,
    pub target: TypeExpr,
    pub methods: Vec<FnDecl>,
    pub span: Span,
    /// The module that declares it, numbered when modules are linked; the
    /// root module is 0.
    pub module: u32,
}

/// `T` or `T: Bound + Bound` in a generic declaration (12.1).
#[derive(Debug, Clone)]
pub struct GenericParam {
    pub name: Ident,
    pub bounds: Vec<TypeExpr>,
}

#[derive(Clone, Debug)]
pub struct EnumDecl {
    pub public: bool,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub variants: Vec<VariantDecl>,
    pub span: Span,
    /// The module that declares it, numbered when modules are linked; the
    /// root module is 0.
    pub module: u32,
}

#[derive(Clone, Debug)]
pub struct VariantDecl {
    pub name: Ident,
    pub payload: VariantPayload,
}

#[derive(Clone, Debug)]
pub enum VariantPayload {
    Unit,
    Tuple(Vec<TypeExpr>),
    Record(Vec<FieldDecl>),
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum PatternKind {
    Wildcard,
    Binding(Ident),
    /// `ref name`, `mut name` or `var name`: a loan of the matched part
    /// instead of a copy (14.3).
    Loan(Mode, Ident),
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
    /// that long; `rest` is `Some(None)` for a bare `..`, and its mode is
    /// `Ref` for `..ref rest`.
    Array {
        items: Vec<Pattern>,
        rest: Option<Option<(Mode, Ident)>>,
    },
    /// `Array[Int](pattern)`: a union member given by a generic type (9.6).
    Member {
        ty: TypeExpr,
        inner: Box<Pattern>,
    },
}

#[derive(Clone, Debug)]
pub struct FieldPattern {
    pub name: Ident,
    /// `None` for the shorthand `name`, which binds `name`.
    pub pattern: Option<Pattern>,
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: ArmBody,
    pub span: Span,
}

/// An arm of `select` (11.14): `let pattern = operation => body`,
/// `operation => body` or `else => body`.
#[derive(Clone, Debug)]
pub struct SelectArm {
    pub pattern: Option<Pattern>,
    /// The operation it waits for; `None` for `else`.
    pub operation: Option<Expr>,
    pub body: ArmBody,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ArmBody {
    Expr(Expr),
    /// A statement written directly after `=>`: `return`, `fail`, `break`,
    /// `continue`, an assignment or `_ = value`.
    Statement(Box<Stmt>),
}

/// `type Name = { fields }` (record) or `type Name = T` (newtype); with
/// `extern`, a C struct or, without a body, an opaque C type.
#[derive(Clone, Debug)]
pub struct TypeDecl {
    pub public: bool,
    /// `extern type`: laid out as C lays out the struct.
    pub foreign: bool,
    /// `@packed` and `@align(n)` before the declaration.
    pub annotations: Vec<Annotation>,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub body: TypeBody,
    pub span: Span,
    /// The module that declares it, numbered when modules are linked; the
    /// root module is 0.
    pub module: u32,
}

#[derive(Clone, Debug)]
pub enum TypeBody {
    Record(Vec<FieldDecl>),
    Newtype(TypeExpr),
    /// `extern type Name`: a C type known only by pointer.
    Opaque,
}

#[derive(Clone, Debug)]
pub struct FieldDecl {
    pub private: bool,
    pub name: Ident,
    pub ty: TypeExpr,
    /// `@name("wire")` and `@ignore`, which serialization reads (8.6),
    /// or `@count(n)` in an `extern type`.
    pub annotations: Vec<Annotation>,
}

/// `@name`, `@name("text")` or `@name(16)` before a declaration or a
/// record field.
#[derive(Debug, Clone)]
pub struct Annotation {
    pub name: Ident,
    pub argument: Option<AnnotationArg>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationArg {
    Text(String),
    Int(u64),
}

/// `const Name: T = value`: a value evaluated in compilation (6.5).
#[derive(Clone, Debug)]
pub struct ConstDecl {
    pub public: bool,
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
    /// The module that declares it, numbered when modules are linked; the
    /// root module is 0.
    pub module: u32,
}

/// `alias Name = T`: a transparent name for `T`.
#[derive(Clone, Debug)]
pub struct AliasDecl {
    pub public: bool,
    pub name: Ident,
    pub target: TypeExpr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub public: bool,
    /// `unsafe fn`: the caller guarantees preconditions the compiler cannot
    /// check (16.12).
    pub is_unsafe: bool,
    /// `extern fn`: a C function, which has no body.
    pub foreign: Option<Foreign>,
    /// `@symbol("name")` before the declaration.
    pub annotations: Vec<Annotation>,
    pub owner: Option<Ident>,
    /// The binders of a generic owner: `fn Pair[A, B].swapped(self)`.
    pub owner_generics: Vec<GenericParam>,
    pub name: Ident,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub result: Option<TypeExpr>,
    /// `None` for a required method of a trait.
    pub body: Option<Block>,
    pub span: Span,
    /// The module that declares it, numbered when modules are linked; the
    /// root module is 0.
    pub module: u32,
}

/// What an `extern fn` calls: the C symbol, its declared name unless
/// `@symbol` gives another, and whether it ends with C's `...`.
#[derive(Debug, Clone)]
pub struct Foreign {
    pub symbol: String,
    pub variadic: bool,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: Ident,
    /// `None` for a receiver: `self`, `mut self` or `var self`.
    pub ty: Option<TypeExpr>,
    pub mode: Mode,
    /// `name: ...T`
    pub variadic: bool,
    /// `name: T = value`: the value a call that omits the argument passes.
    pub default: Option<Expr>,
    pub span: Span,
}

/// How a parameter or argument is passed: by value, or as a `ref`, `mut` or
/// `var` loan (11.3, 16.3). A plain `self` receiver is a `ref` loan.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
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
    /// `~Copy` in a list of bounds: drops the implicit `Copy` (12.1).
    NotCopy,
    /// `fn(A, mut B): R`, the uniform function type (11.7), with how each
    /// parameter is passed.
    Fn {
        params: Vec<TypeExpr>,
        modes: Vec<Mode>,
        variadic: bool,
        result: Option<Box<TypeExpr>>,
        /// `unsafe fn(...)`
        is_unsafe: bool,
        /// `extern fn(...)`: the address of a C function.
        is_extern: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
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
    /// `defer call(args)` or `defer { block }` (13.7).
    Defer(Defer),
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub enum Defer {
    Call(Expr),
    Block(Block),
}

#[derive(Clone, Debug)]
pub enum ForHeader {
    Infinite,
    Condition(Expr),
    In { pattern: Pattern, source: Expr },
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    /// `unsafe { ... }`: a region where raw operations are allowed (16.12).
    Unsafe(Block),
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
    /// `select { ... }` (11.14).
    Select(Vec<SelectArm>),
    /// `sync.Map["one": 1]` and the other literals of shared collections
    /// (5.11): the qualified type path and the literal of its contents.
    SyncLiteral(Vec<Ident>, Box<Expr>),
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
    /// `spawn call(arguments)`, or `spawn detached call(arguments)` when
    /// true (11.11).
    Spawn(Box<Expr>, bool),
    /// Placeholder after a reported syntax error.
    Error,
}

#[derive(Clone, Debug)]
pub struct Closure {
    /// `unsafe (...) { ... }`
    pub is_unsafe: bool,
    pub params: Vec<ClosureParam>,
    pub result: Option<TypeExpr>,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub struct ClosureParam {
    pub name: Ident,
    /// May be omitted, with its mode, when an expected function type gives
    /// them.
    pub ty: Option<TypeExpr>,
    pub mode: Mode,
    pub variadic: bool,
}

#[derive(Clone, Debug)]
pub struct FieldInit {
    pub name: Ident,
    /// `None` for the shorthand `name`, which reads the binding `name`.
    pub value: Option<Expr>,
}

#[derive(Clone, Debug)]
pub enum StrSegment {
    Text(String),
    Expr(Expr),
}

#[derive(Clone, Debug)]
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
