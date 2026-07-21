use crate::source::TextRange;

use super::{Token, TokenKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u32);

impl NodeId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenId(u32);

impl TokenId {
    pub fn index(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntaxElement {
    Node(NodeId),
    Token(TokenId),
}

#[derive(Debug, Clone, Copy)]
pub struct SyntaxNodeRef<'a> {
    cst: &'a Cst,
    id: NodeId,
}

impl PartialEq for SyntaxNodeRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.cst, other.cst) && self.id == other.id
    }
}

impl Eq for SyntaxNodeRef<'_> {}

impl<'a> SyntaxNodeRef<'a> {
    pub fn id(self) -> NodeId {
        self.id
    }

    pub fn kind(self) -> SyntaxKind {
        self.cst.node(self.id).kind()
    }

    pub fn range(self) -> TextRange {
        self.cst.node(self.id).range()
    }

    pub fn elements(self) -> &'a [SyntaxElement] {
        self.cst.node(self.id).children()
    }

    pub fn child_nodes(self) -> impl Iterator<Item = SyntaxNodeRef<'a>> + 'a {
        let cst = self.cst;
        self.elements()
            .iter()
            .filter_map(move |element| match *element {
                SyntaxElement::Node(id) => Some(SyntaxNodeRef { cst, id }),
                SyntaxElement::Token(_) => None,
            })
    }

    pub fn child_tokens(self) -> impl Iterator<Item = SyntaxTokenRef<'a>> + 'a {
        let cst = self.cst;
        self.elements()
            .iter()
            .filter_map(move |element| match *element {
                SyntaxElement::Token(id) => Some(SyntaxTokenRef { cst, id }),
                SyntaxElement::Node(_) => None,
            })
    }

    pub fn descendant_tokens(self) -> DescendantTokens<'a> {
        DescendantTokens {
            cst: self.cst,
            stack: vec![(self.id, 0)],
        }
    }

    pub fn cst(self) -> &'a Cst {
        self.cst
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SyntaxTokenRef<'a> {
    cst: &'a Cst,
    id: TokenId,
}

impl PartialEq for SyntaxTokenRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.cst, other.cst) && self.id == other.id
    }
}

impl Eq for SyntaxTokenRef<'_> {}

impl<'a> SyntaxTokenRef<'a> {
    pub fn id(self) -> TokenId {
        self.id
    }

    pub fn token(self) -> &'a Token {
        self.cst.token(self.id)
    }

    pub fn kind(self) -> TokenKind {
        self.token().kind()
    }

    pub fn range(self) -> TextRange {
        self.token().range()
    }

    pub fn is_synthetic(self) -> bool {
        self.token().is_synthetic()
    }

    pub fn cst(self) -> &'a Cst {
        self.cst
    }
}

#[derive(Debug)]
pub struct DescendantTokens<'a> {
    cst: &'a Cst,
    stack: Vec<(NodeId, usize)>,
}

impl<'a> Iterator for DescendantTokens<'a> {
    type Item = SyntaxTokenRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (node, child_index) = self.stack.last_mut()?;
            let children = self.cst.node(*node).children();
            let Some(child) = children.get(*child_index).copied() else {
                self.stack.pop();
                continue;
            };
            *child_index += 1;
            match child {
                SyntaxElement::Node(child) => self.stack.push((child, 0)),
                SyntaxElement::Token(id) => return Some(SyntaxTokenRef { cst: self.cst, id }),
            }
        }
    }
}

/// Stable inventory of concrete node shapes produced by the Tondo 0.1 parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SyntaxKind {
    Module,
    Script,
    Fragment,
    SyntaxSequence,
    StandaloneBlock,
    Error,

    ImportDecl,
    ConstDecl,
    TypeDecl,
    AliasDecl,
    EnumDecl,
    EnumVariant,
    TraitDecl,
    TraitMethod,
    ImplDecl,
    ImplementationMethod,
    FunctionDecl,
    FunctionSignature,
    FunctionHead,
    MethodOwner,
    Visibility,
    GenericParams,
    GenericParam,
    GenericBound,
    GenericArgs,
    ModulePath,
    TypePath,
    ValuePath,
    OutcomeAnnotation,
    OpaqueOutcome,
    RecordBody,
    RecordField,
    TuplePayload,
    ParameterList,
    Parameter,

    TypeExpr,
    UnionType,
    ResultType,
    OptionalType,
    PathType,
    TupleType,
    GroupType,
    FunctionType,
    FunctionTypeList,
    FunctionTypeItem,

    Block,
    BindingDecl,
    Assignment,
    TupleAssignmentPattern,
    Lvalue,
    ReturnStmt,
    FailStmt,
    BreakStmt,
    ContinueStmt,
    DeferStmt,
    ForStmt,
    ForHeader,
    ExpressionStmt,
    TailExpression,

    IfExpr,
    MatchExpr,
    MatchArm,
    ClosureExpr,
    ClosureParameterList,
    ClosureParameter,
    BinaryExpr,
    PrefixExpr,
    PostfixExpr,
    AwaitExpr,
    SpawnExpr,
    LiteralExpr,
    StringLiteralExpr,
    Interpolation,
    SelfExpr,
    PathExpr,
    TupleExpr,
    GroupExpr,
    BracketLiteralExpr,
    SetLiteralExpr,
    RecordLikeExpr,
    RecordInitializer,
    RecordUpdateBody,
    RecordUpdate,
    OptionResultConstructor,
    ScopeExpr,
    UnsafeExpr,
    CallSuffix,
    CallArgument,
    BracketPostfix,
    BracketItem,
    SliceSpec,
    MemberSuffix,
    PropagateSuffix,

    WildcardPattern,
    UnitPattern,
    LiteralPattern,
    OptionResultPattern,
    TuplePattern,
    ArrayPattern,
    ArrayRestPattern,
    ConstructorPattern,
    RecordPattern,
    RecordPatternField,
    RecordRestPattern,
    QualifiedValuePattern,
    BorrowBindingPattern,
    BindingPattern,
}

#[derive(Debug)]
pub struct SyntaxNode {
    kind: SyntaxKind,
    range: TextRange,
    children: Box<[SyntaxElement]>,
    has_physical_source: bool,
}

impl SyntaxNode {
    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn children(&self) -> &[SyntaxElement] {
        &self.children
    }
}

/// Immutable lossless concrete syntax tree.
#[derive(Debug)]
pub struct Cst {
    nodes: Vec<SyntaxNode>,
    tokens: Vec<Token>,
    root: NodeId,
    original_token_count: usize,
}

impl Cst {
    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn node(&self, id: NodeId) -> &SyntaxNode {
        &self.nodes[id.0 as usize]
    }

    pub fn root_node(&self) -> SyntaxNodeRef<'_> {
        SyntaxNodeRef {
            cst: self,
            id: self.root,
        }
    }

    pub fn node_ref(&self, id: NodeId) -> SyntaxNodeRef<'_> {
        SyntaxNodeRef { cst: self, id }
    }

    pub fn token_ref(&self, id: TokenId) -> SyntaxTokenRef<'_> {
        SyntaxTokenRef { cst: self, id }
    }

    pub fn token(&self, id: TokenId) -> &Token {
        &self.tokens[id.0 as usize]
    }

    pub fn nodes(&self) -> &[SyntaxNode] {
        &self.nodes
    }

    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    pub fn original_token_count(&self) -> usize {
        self.original_token_count
    }

    pub fn reconstruct(&self, source: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(source.len());
        self.walk_physical_tokens(self.root, &mut |token| {
            let range = token.range();
            output.extend_from_slice(&source[range.start() as usize..range.end() as usize]);
        });
        output
    }

    pub fn has_exact_physical_partition(&self, source_length: u32) -> bool {
        let mut cursor = 0_u32;
        let mut valid = true;
        self.walk_tokens(self.root, &mut |token| {
            if token.is_synthetic() {
                valid &= token.range().start() == token.range().end();
            } else {
                valid &= token.range().start() == cursor && token.range().end() >= cursor;
                cursor = token.range().end();
            }
        });
        valid && cursor == source_length
    }

    pub fn token_kinds_in_tree_order(&self) -> Vec<TokenKind> {
        let mut kinds = Vec::new();
        self.walk_tokens(self.root, &mut |token| kinds.push(token.kind()));
        kinds
    }

    fn walk_physical_tokens(&self, node: NodeId, visitor: &mut impl FnMut(&Token)) {
        self.walk_tokens(node, &mut |token| {
            if !token.is_synthetic() {
                visitor(token);
            }
        });
    }

    fn walk_tokens(&self, node: NodeId, visitor: &mut impl FnMut(&Token)) {
        for child in self.node(node).children() {
            match *child {
                SyntaxElement::Node(child) => self.walk_tokens(child, visitor),
                SyntaxElement::Token(token) => visitor(self.token(token)),
            }
        }
    }
}

#[derive(Debug)]
struct OpenNode {
    kind: SyntaxKind,
    insertion_offset: u32,
    children: Vec<SyntaxElement>,
}

#[derive(Debug)]
pub(crate) struct CstBuilder {
    nodes: Vec<SyntaxNode>,
    tokens: Vec<Token>,
    original_token_count: usize,
    stack: Vec<OpenNode>,
    root: Option<NodeId>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Checkpoint {
    stack_depth: usize,
    child_index: usize,
}

impl CstBuilder {
    pub(crate) fn new(tokens: Vec<Token>) -> Self {
        let original_token_count = tokens.len();
        Self {
            nodes: Vec::new(),
            tokens,
            original_token_count,
            stack: Vec::new(),
            root: None,
        }
    }

    pub(crate) fn start(&mut self, kind: SyntaxKind, insertion_offset: u32) {
        self.stack.push(OpenNode {
            kind,
            insertion_offset,
            children: Vec::new(),
        });
    }

    pub(crate) fn checkpoint(&self) -> Checkpoint {
        let current = self
            .stack
            .last()
            .expect("a checkpoint is created inside a CST node");
        Checkpoint {
            stack_depth: self.stack.len(),
            child_index: current.children.len(),
        }
    }

    pub(crate) fn start_at(
        &mut self,
        checkpoint: Checkpoint,
        kind: SyntaxKind,
        insertion_offset: u32,
    ) {
        assert_eq!(
            checkpoint.stack_depth,
            self.stack.len(),
            "a checkpoint belongs to the current CST parent"
        );
        let parent = self.stack.last_mut().expect("a CST parent is open");
        let children = parent.children.split_off(checkpoint.child_index);
        self.stack.push(OpenNode {
            kind,
            insertion_offset,
            children,
        });
    }

    pub(crate) fn token(&mut self, token: TokenId) {
        self.stack
            .last_mut()
            .expect("a token is always attached inside a CST node")
            .children
            .push(SyntaxElement::Token(token));
    }

    pub(crate) fn missing_token(&mut self, kind: TokenKind, offset: u32) -> TokenId {
        let id = TokenId(u32::try_from(self.tokens.len()).expect("token limits use u32 bounds"));
        self.tokens.push(Token::synthetic(kind, offset));
        self.token(id);
        id
    }

    pub(crate) fn finish(&mut self) -> NodeId {
        let open = self.stack.pop().expect("CST nodes are balanced");
        let (range, has_physical_source) = self.range_for(&open);
        let id = NodeId(u32::try_from(self.nodes.len()).expect("syntax limits use u32 bounds"));
        self.nodes.push(SyntaxNode {
            kind: open.kind,
            range,
            children: open.children.into_boxed_slice(),
            has_physical_source,
        });
        if let Some(parent) = self.stack.last_mut() {
            parent.children.push(SyntaxElement::Node(id));
        } else {
            assert!(
                self.root.replace(id).is_none(),
                "a CST has exactly one root"
            );
        }
        id
    }

    pub(crate) fn build(self) -> Cst {
        assert!(self.stack.is_empty(), "all CST nodes must be closed");
        Cst {
            nodes: self.nodes,
            tokens: self.tokens,
            root: self.root.expect("a CST root is required"),
            original_token_count: self.original_token_count,
        }
    }

    fn range_for(&self, open: &OpenNode) -> (TextRange, bool) {
        let mut start = None;
        let mut end = None;
        for child in &open.children {
            let bounds = match *child {
                SyntaxElement::Token(token) => {
                    let token = &self.tokens[token.0 as usize];
                    (!token.is_synthetic()).then_some(token.range())
                }
                SyntaxElement::Node(node) => {
                    let node = &self.nodes[node.0 as usize];
                    node.has_physical_source.then_some(node.range)
                }
            };
            if let Some(bounds) = bounds {
                start.get_or_insert(bounds.start());
                end = Some(bounds.end());
            }
        }
        match (start, end) {
            (Some(start), Some(end)) => (
                TextRange::new(start, end).expect("ordered CST children produce an ordered range"),
                true,
            ),
            _ => (TextRange::empty(open.insertion_offset), false),
        }
    }

    pub(crate) fn original_token(&self, index: usize) -> &Token {
        &self.tokens[index]
    }
}

impl TokenId {
    pub(crate) fn from_original_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("lexer token limits use u32 bounds"))
    }
}
