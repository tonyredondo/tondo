//! Checked typed views over the lossless concrete syntax tree.
//!
//! AST values contain only a [`SyntaxNodeRef`]. They borrow the original CST,
//! never own source text, and never duplicate trivia, tokens, or byte ranges.

use std::fmt;
use std::marker::PhantomData;
use std::slice;

use super::{Cst, SyntaxElement, SyntaxKind, SyntaxNodeRef, SyntaxTokenRef, TokenKind};

pub trait AstNode<'a>: Copy + Sized {
    fn can_cast(kind: SyntaxKind) -> bool;
    fn cast(syntax: SyntaxNodeRef<'a>) -> Option<Self>;
    fn syntax(self) -> SyntaxNodeRef<'a>;

    fn child<N: AstNode<'a>>(self) -> Option<N> {
        self.children().next()
    }

    fn children<N: AstNode<'a>>(self) -> AstChildren<'a, N> {
        AstChildren::new(self.syntax())
    }

    fn direct_token(self, kind: TokenKind) -> Option<SyntaxTokenRef<'a>> {
        self.syntax()
            .child_tokens()
            .find(|token| token.kind() == kind)
    }

    fn first_token(self, kind: TokenKind) -> Option<SyntaxTokenRef<'a>> {
        self.syntax()
            .descendant_tokens()
            .find(|token| token.kind() == kind)
    }
}

pub struct AstChildren<'a, N> {
    cst: &'a Cst,
    elements: slice::Iter<'a, SyntaxElement>,
    marker: PhantomData<N>,
}

impl<'a, N> AstChildren<'a, N> {
    fn new(parent: SyntaxNodeRef<'a>) -> Self {
        Self {
            cst: parent.cst(),
            elements: parent.elements().iter(),
            marker: PhantomData,
        }
    }
}

impl<'a, N: AstNode<'a>> Iterator for AstChildren<'a, N> {
    type Item = N;

    fn next(&mut self) -> Option<Self::Item> {
        for element in self.elements.by_ref() {
            if let SyntaxElement::Node(id) = *element
                && let Some(node) = N::cast(self.cst.node_ref(id))
            {
                return Some(node);
            }
        }
        None
    }
}

macro_rules! define_ast_nodes {
    ($($name:ident => $kind:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Copy, PartialEq, Eq)]
            pub struct $name<'a>(SyntaxNodeRef<'a>);

            impl<'a> $name<'a> {
                pub const KIND: SyntaxKind = SyntaxKind::$kind;

                pub fn cast(syntax: SyntaxNodeRef<'a>) -> Option<Self> {
                    <Self as AstNode<'a>>::cast(syntax)
                }

                pub fn syntax(self) -> SyntaxNodeRef<'a> {
                    self.0
                }
            }

            impl<'a> AstNode<'a> for $name<'a> {
                fn can_cast(kind: SyntaxKind) -> bool {
                    kind == Self::KIND
                }

                fn cast(syntax: SyntaxNodeRef<'a>) -> Option<Self> {
                    Self::can_cast(syntax.kind()).then_some(Self(syntax))
                }

                fn syntax(self) -> SyntaxNodeRef<'a> {
                    self.0
                }
            }

            impl fmt::Debug for $name<'_> {
                fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter
                        .debug_struct(stringify!($name))
                        .field("id", &self.0.id())
                        .field("range", &self.0.range())
                        .finish()
                }
            }
        )+
    };
}

define_ast_nodes! {
    Module => Module,
    Script => Script,
    Fragment => Fragment,
    SyntaxSequence => SyntaxSequence,
    StandaloneBlock => StandaloneBlock,
    ErrorNode => Error,

    ImportDecl => ImportDecl,
    ConstDecl => ConstDecl,
    TypeDecl => TypeDecl,
    AliasDecl => AliasDecl,
    EnumDecl => EnumDecl,
    EnumVariant => EnumVariant,
    TraitDecl => TraitDecl,
    TraitMethod => TraitMethod,
    ImplDecl => ImplDecl,
    ImplementationMethod => ImplementationMethod,
    FunctionDecl => FunctionDecl,
    FunctionSignature => FunctionSignature,
    FunctionHead => FunctionHead,
    MethodOwner => MethodOwner,
    Visibility => Visibility,
    GenericParams => GenericParams,
    GenericParam => GenericParam,
    GenericBound => GenericBound,
    GenericArgs => GenericArgs,
    ModulePath => ModulePath,
    TypePath => TypePath,
    ValuePath => ValuePath,
    OutcomeAnnotation => OutcomeAnnotation,
    OpaqueOutcome => OpaqueOutcome,
    RecordBody => RecordBody,
    RecordField => RecordField,
    TuplePayload => TuplePayload,
    ParameterList => ParameterList,
    Parameter => Parameter,

    TypeExpr => TypeExpr,
    UnionType => UnionType,
    ResultType => ResultType,
    OptionalType => OptionalType,
    PathType => PathType,
    TupleType => TupleType,
    GroupType => GroupType,
    FunctionType => FunctionType,
    FunctionTypeList => FunctionTypeList,
    FunctionTypeItem => FunctionTypeItem,

    Block => Block,
    BindingDecl => BindingDecl,
    Assignment => Assignment,
    TupleAssignmentPattern => TupleAssignmentPattern,
    Lvalue => Lvalue,
    ReturnStmt => ReturnStmt,
    FailStmt => FailStmt,
    BreakStmt => BreakStmt,
    ContinueStmt => ContinueStmt,
    DeferStmt => DeferStmt,
    ForStmt => ForStmt,
    ForHeader => ForHeader,
    ExpressionStmt => ExpressionStmt,
    TailExpression => TailExpression,

    IfExpr => IfExpr,
    MatchExpr => MatchExpr,
    MatchArm => MatchArm,
    ClosureExpr => ClosureExpr,
    ClosureParameterList => ClosureParameterList,
    ClosureParameter => ClosureParameter,
    BinaryExpr => BinaryExpr,
    PrefixExpr => PrefixExpr,
    PostfixExpr => PostfixExpr,
    AwaitExpr => AwaitExpr,
    SpawnExpr => SpawnExpr,
    LiteralExpr => LiteralExpr,
    StringLiteralExpr => StringLiteralExpr,
    Interpolation => Interpolation,
    SelfExpr => SelfExpr,
    PathExpr => PathExpr,
    TupleExpr => TupleExpr,
    GroupExpr => GroupExpr,
    BracketLiteralExpr => BracketLiteralExpr,
    SetLiteralExpr => SetLiteralExpr,
    RecordLikeExpr => RecordLikeExpr,
    RecordInitializer => RecordInitializer,
    RecordUpdateBody => RecordUpdateBody,
    RecordUpdate => RecordUpdate,
    OptionResultConstructor => OptionResultConstructor,
    ScopeExpr => ScopeExpr,
    UnsafeExpr => UnsafeExpr,
    CallSuffix => CallSuffix,
    CallArgument => CallArgument,
    BracketPostfix => BracketPostfix,
    BracketItem => BracketItem,
    SliceSpec => SliceSpec,
    MemberSuffix => MemberSuffix,
    PropagateSuffix => PropagateSuffix,

    WildcardPattern => WildcardPattern,
    UnitPattern => UnitPattern,
    LiteralPattern => LiteralPattern,
    OptionResultPattern => OptionResultPattern,
    TuplePattern => TuplePattern,
    ArrayPattern => ArrayPattern,
    ArrayRestPattern => ArrayRestPattern,
    ConstructorPattern => ConstructorPattern,
    RecordPattern => RecordPattern,
    RecordPatternField => RecordPatternField,
    RecordRestPattern => RecordRestPattern,
    QualifiedValuePattern => QualifiedValuePattern,
    BorrowBindingPattern => BorrowBindingPattern,
    BindingPattern => BindingPattern,
}

macro_rules! define_ast_sum {
    ($name:ident { $($variant:ident($node:ident)),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name<'a> {
            $($variant($node<'a>)),+
        }

        impl<'a> AstNode<'a> for $name<'a> {
            fn can_cast(kind: SyntaxKind) -> bool {
                $($node::can_cast(kind))||+
            }

            fn cast(syntax: SyntaxNodeRef<'a>) -> Option<Self> {
                $(
                    if let Some(node) = $node::cast(syntax) {
                        return Some(Self::$variant(node));
                    }
                )+
                None
            }

            fn syntax(self) -> SyntaxNodeRef<'a> {
                match self {
                    $(Self::$variant(node) => node.syntax()),+
                }
            }
        }

        impl<'a> $name<'a> {
            pub fn cast(syntax: SyntaxNodeRef<'a>) -> Option<Self> {
                <Self as AstNode<'a>>::cast(syntax)
            }

            pub fn syntax(self) -> SyntaxNodeRef<'a> {
                <Self as AstNode<'a>>::syntax(self)
            }
        }
    };
}

define_ast_sum! {
    SourceFile {
        Module(Module),
        Script(Script),
        Fragment(Fragment),
    }
}

define_ast_sum! {
    Declaration {
        Const(ConstDecl),
        Type(TypeDecl),
        Alias(AliasDecl),
        Enum(EnumDecl),
        Trait(TraitDecl),
        Impl(ImplDecl),
        Function(FunctionDecl),
    }
}

define_ast_sum! {
    Statement {
        Binding(BindingDecl),
        Assignment(Assignment),
        Return(ReturnStmt),
        Fail(FailStmt),
        Break(BreakStmt),
        Continue(ContinueStmt),
        Defer(DeferStmt),
        For(ForStmt),
        Expression(ExpressionStmt),
    }
}

define_ast_sum! {
    BlockItem {
        Binding(BindingDecl),
        Assignment(Assignment),
        Return(ReturnStmt),
        Fail(FailStmt),
        Break(BreakStmt),
        Continue(ContinueStmt),
        Defer(DeferStmt),
        For(ForStmt),
        Expression(ExpressionStmt),
        Tail(TailExpression),
    }
}

define_ast_sum! {
    Expression {
        If(IfExpr),
        Match(MatchExpr),
        Closure(ClosureExpr),
        Binary(BinaryExpr),
        Prefix(PrefixExpr),
        Postfix(PostfixExpr),
        Await(AwaitExpr),
        Spawn(SpawnExpr),
        Literal(LiteralExpr),
        String(StringLiteralExpr),
        SelfValue(SelfExpr),
        Path(PathExpr),
        Tuple(TupleExpr),
        Group(GroupExpr),
        BracketLiteral(BracketLiteralExpr),
        SetLiteral(SetLiteralExpr),
        RecordLike(RecordLikeExpr),
        OptionResult(OptionResultConstructor),
        Scope(ScopeExpr),
        Unsafe(UnsafeExpr),
        Block(Block),
    }
}

define_ast_sum! {
    Pattern {
        Wildcard(WildcardPattern),
        Unit(UnitPattern),
        Literal(LiteralPattern),
        OptionResult(OptionResultPattern),
        Tuple(TuplePattern),
        Array(ArrayPattern),
        Constructor(ConstructorPattern),
        Record(RecordPattern),
        QualifiedValue(QualifiedValuePattern),
        BorrowBinding(BorrowBindingPattern),
        Binding(BindingPattern),
    }
}

define_ast_sum! {
    PrimaryType {
        Path(PathType),
        Tuple(TupleType),
        Group(GroupType),
        Function(FunctionType),
    }
}

impl<'a> SourceFile<'a> {
    pub fn root(cst: &'a Cst) -> Option<Self> {
        Self::cast(cst.root_node())
    }

    pub fn imports(self) -> AstChildren<'a, ImportDecl<'a>> {
        self.children()
    }

    pub fn declarations(self) -> AstChildren<'a, Declaration<'a>> {
        self.children()
    }

    pub fn statements(self) -> AstChildren<'a, Statement<'a>> {
        self.children()
    }
}

macro_rules! impl_named {
    ($($node:ident),+ $(,)?) => {
        $(
            impl<'a> $node<'a> {
                pub fn name_token(self) -> Option<SyntaxTokenRef<'a>> {
                    <Self as AstNode<'a>>::first_token(self, TokenKind::Identifier)
                }
            }
        )+
    };
}

impl_named! {
    ConstDecl,
    TypeDecl,
    AliasDecl,
    EnumDecl,
    EnumVariant,
    TraitDecl,
    TraitMethod,
    ImplementationMethod,
    GenericParam,
    Parameter,
    ClosureParameter,
}

impl<'a> FunctionHead<'a> {
    pub fn name_token(self) -> Option<SyntaxTokenRef<'a>> {
        self.syntax()
            .child_tokens()
            .filter(|token| token.kind() == TokenKind::Identifier)
            .last()
    }
}

fn field_name_token<'a>(syntax: SyntaxNodeRef<'a>) -> Option<SyntaxTokenRef<'a>> {
    let mut tokens = syntax
        .child_tokens()
        .filter(|token| !token.kind().is_trivia());
    let first = tokens.next()?;
    if matches!(first.kind(), TokenKind::Priv | TokenKind::Ref) {
        let second = tokens.next();
        if second.is_some_and(|token| token.kind() != TokenKind::Colon) {
            return second;
        }
    }
    Some(first)
}

macro_rules! impl_field_name {
    ($($node:ident),+ $(,)?) => {
        $(
            impl<'a> $node<'a> {
                pub fn field_name_token(self) -> Option<SyntaxTokenRef<'a>> {
                    field_name_token(self.syntax())
                }
            }
        )+
    };
}

impl_field_name! {
    RecordField,
    RecordInitializer,
    RecordUpdate,
    RecordPatternField,
}

impl<'a> FunctionDecl<'a> {
    pub fn head(self) -> Option<FunctionHead<'a>> {
        self.child()
    }

    pub fn parameters(self) -> Option<ParameterList<'a>> {
        self.child()
    }

    pub fn outcome(self) -> Option<OutcomeAnnotation<'a>> {
        self.child()
    }

    pub fn body(self) -> Option<Block<'a>> {
        self.child()
    }
}

impl<'a> ParameterList<'a> {
    pub fn parameters(self) -> AstChildren<'a, Parameter<'a>> {
        self.children()
    }
}

impl<'a> Block<'a> {
    pub fn items(self) -> AstChildren<'a, BlockItem<'a>> {
        self.children()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::source::{LogicalPath, ModulePath, SourceDatabase, SourceId, SourceInput};
    use crate::syntax::{LexMode, ParseLimits, ParseMode, lex, parse};

    #[test]
    fn typed_views_cast_and_traverse_without_a_second_tree() {
        let source = br#"import std.io

type User = {
    fn: Int
    priv priv: Bool
}

fn User.compute(value: Int): Int {
    value + 1
}
"#;
        let mut sources = SourceDatabase::new();
        let file = sources
            .add(SourceInput::virtual_file(
                SourceId::new("root:ast-test").unwrap(),
                ModulePath::new("ast").unwrap(),
                LogicalPath::new("ast.to").unwrap(),
                Arc::<[u8]>::from(&source[..]),
            ))
            .unwrap();
        let lexed = lex(&sources, file, LexMode::Module).unwrap();
        let parsed = parse(
            &sources,
            file,
            lexed,
            ParseMode::Module,
            ParseLimits::default(),
        )
        .unwrap();
        let cst = parsed.cst();
        let root = SourceFile::root(cst).unwrap();
        assert!(matches!(root, SourceFile::Module(_)));
        assert_eq!(root.imports().count(), 1);

        let function = root
            .declarations()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) => Some(function),
                _ => None,
            })
            .unwrap();
        assert!(std::ptr::eq(function.syntax().cst(), cst));
        assert_eq!(
            function
                .head()
                .unwrap()
                .name_token()
                .unwrap()
                .token()
                .normalized_identifier(),
            Some("compute")
        );
        assert_eq!(function.parameters().unwrap().parameters().count(), 1);
        assert!(function.outcome().is_some());
        assert_eq!(function.body().unwrap().items().count(), 1);
        assert!(TypeDecl::cast(function.syntax()).is_none());

        let record = root
            .declarations()
            .find_map(|declaration| match declaration {
                Declaration::Type(declaration) => declaration.child::<RecordBody>(),
                _ => None,
            })
            .unwrap();
        let field_names = record
            .children::<RecordField>()
            .map(|field| field.field_name_token().unwrap().kind())
            .collect::<Vec<_>>();
        assert_eq!(field_names, [TokenKind::Fn, TokenKind::Priv]);

        let tree_tokens = root.syntax().descendant_tokens().count();
        assert_eq!(tree_tokens, cst.tokens().len());
    }
}
