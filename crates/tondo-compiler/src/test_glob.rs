//! Portable, full-match glob selection for visible suite/test identities.
//!
//! The grammar is intentionally smaller than a shell glob. `::` separates
//! components, `*` and `?` operate only inside one component, and `**` is a
//! component-level globstar. Matching uses bounded dynamic programming and
//! never consults the shell, filesystem, locale, or Unicode normalization.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

pub const TEST_GLOB_FORMAT: &str = "tondo-test-glob-draft/1";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ComponentToken {
    Literal(char),
    Star,
    Question,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Component {
    Tokens(Vec<ComponentToken>),
    GlobStar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlobError {
    EmptyPattern,
    EmptyComponent { index: usize },
    IsolatedColon { index: usize },
    ConsecutiveStars { index: usize },
    EmbeddedGlobStar { index: usize },
    AdjacentGlobStars { index: usize },
    EmptyNodeId,
    DuplicateNode(String),
    UnknownParent { child: String, parent: String },
    LeafParent { child: String, parent: String },
    Cycle(String),
}

impl fmt::Display for GlobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPattern => formatter.write_str("glob pattern cannot be empty"),
            Self::EmptyComponent { index } => write!(formatter, "glob component {index} is empty"),
            Self::IsolatedColon { index } => {
                write!(formatter, "glob contains an isolated `:` at scalar {index}")
            }
            Self::ConsecutiveStars { index } => {
                write!(formatter, "glob contains consecutive `*` at scalar {index}")
            }
            Self::EmbeddedGlobStar { index } => write!(
                formatter,
                "globstar must be a complete component at scalar {index}"
            ),
            Self::AdjacentGlobStars { index } => write!(
                formatter,
                "globstar components cannot be adjacent at component {index}"
            ),
            Self::EmptyNodeId => formatter.write_str("glob tree node identity cannot be empty"),
            Self::DuplicateNode(id) => write!(formatter, "glob tree node `{id}` is duplicated"),
            Self::UnknownParent { child, parent } => write!(
                formatter,
                "node `{child}` refers to unknown parent `{parent}`"
            ),
            Self::LeafParent { child, parent } => {
                write!(formatter, "leaf `{parent}` cannot contain `{child}`")
            }
            Self::Cycle(id) => write!(formatter, "glob tree contains a cycle at `{id}`"),
        }
    }
}

impl Error for GlobError {}

/// Parsed and canonical glob pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobPattern {
    source: String,
    components: Vec<Component>,
}

impl GlobPattern {
    pub fn parse(pattern: impl Into<String>) -> Result<Self, GlobError> {
        let source = pattern.into();
        if source.is_empty() {
            return Err(GlobError::EmptyPattern);
        }
        let raw = source.split("::").collect::<Vec<_>>();
        let mut components = Vec::with_capacity(raw.len());
        for (index, component) in raw.iter().enumerate() {
            if component.is_empty() {
                return Err(GlobError::EmptyComponent { index });
            }
            if *component == "**" {
                components.push(Component::GlobStar);
                continue;
            }
            if component.contains("**") {
                return Err(GlobError::EmbeddedGlobStar { index });
            }
            let mut tokens = Vec::new();
            let mut chars = component.chars().peekable();
            let mut scalar_index = source
                .split("::")
                .take(index)
                .map(|part| part.chars().count() + 2)
                .sum::<usize>();
            while let Some(ch) = chars.next() {
                if ch == ':' {
                    return Err(GlobError::IsolatedColon {
                        index: scalar_index,
                    });
                }
                if ch == '*' {
                    if chars.peek() == Some(&'*') {
                        return Err(GlobError::ConsecutiveStars {
                            index: scalar_index,
                        });
                    }
                    tokens.push(ComponentToken::Star);
                } else if ch == '?' {
                    tokens.push(ComponentToken::Question);
                } else {
                    tokens.push(ComponentToken::Literal(ch));
                }
                scalar_index += 1;
            }
            components.push(Component::Tokens(tokens));
        }
        for index in 1..components.len() {
            if matches!(components[index - 1], Component::GlobStar)
                && matches!(components[index], Component::GlobStar)
            {
                return Err(GlobError::AdjacentGlobStars { index });
            }
        }
        Ok(Self { source, components })
    }

    pub fn source(&self) -> &str {
        &self.source
    }
    pub fn component_count(&self) -> usize {
        self.components.len()
    }

    /// Full match over visible ID components. The component DP has
    /// `O(pattern_scalars * id_scalars)` complexity and the outer DP has
    /// `O(pattern_components * id_components)` complexity.
    pub fn matches(&self, id: &str) -> bool {
        let id_components = id.split("::").collect::<Vec<_>>();
        if id.is_empty() || id_components.iter().any(|component| component.is_empty()) {
            return false;
        }
        let rows = self.components.len() + 1;
        let cols = id_components.len() + 1;
        let mut table = vec![false; rows * cols];
        table[self.components.len() * cols + id_components.len()] = true;
        for pattern_index in (0..self.components.len()).rev() {
            for id_index in (0..=id_components.len()).rev() {
                let value = match &self.components[pattern_index] {
                    Component::GlobStar => {
                        table[(pattern_index + 1) * cols + id_index]
                            || (id_index < id_components.len()
                                && table[pattern_index * cols + id_index + 1])
                    }
                    Component::Tokens(tokens) => {
                        id_index < id_components.len()
                            && component_matches(tokens, id_components[id_index])
                            && table[(pattern_index + 1) * cols + id_index + 1]
                    }
                };
                table[pattern_index * cols + id_index] = value;
            }
        }
        table[0]
    }

    /// Select IDs with a full match, canonicalized by UTF-8 bytes.
    pub fn select<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut selected = ids
            .into_iter()
            .filter(|id| self.matches(id))
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        selected.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        selected
    }

    /// Match a validated tree. A matching suite contributes all selected leaf
    /// descendants; a matching leaf contributes only itself. The output is a
    /// deduplicated canonical leaf set, ready for sharding.
    pub fn select_tree(
        &self,
        nodes: impl IntoIterator<Item = GlobNode>,
    ) -> Result<GlobSelection, GlobError> {
        let mut map = BTreeMap::new();
        for node in nodes {
            if node.id.trim().is_empty() {
                return Err(GlobError::EmptyNodeId);
            }
            let id = node.id.clone();
            if map.insert(id.clone(), node).is_some() {
                return Err(GlobError::DuplicateNode(id));
            }
        }
        let mut children = BTreeMap::<String, Vec<String>>::new();
        for (id, node) in &map {
            let Some(parent) = node.parent.as_deref() else {
                continue;
            };
            let Some(parent_node) = map.get(parent) else {
                return Err(GlobError::UnknownParent {
                    child: id.clone(),
                    parent: parent.into(),
                });
            };
            if parent_node.kind == GlobNodeKind::Test {
                return Err(GlobError::LeafParent {
                    child: id.clone(),
                    parent: parent.into(),
                });
            }
            children.entry(parent.into()).or_default().push(id.clone());
        }
        for values in children.values_mut() {
            values.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        }
        for id in map.keys() {
            let mut seen = BTreeSet::new();
            let mut cursor = Some(id.as_str());
            while let Some(current) = cursor {
                if !seen.insert(current) {
                    return Err(GlobError::Cycle(id.clone()));
                }
                cursor = map[current].parent.as_deref();
            }
        }
        let matched_nodes = map
            .keys()
            .filter(|id| self.matches(id))
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut leaves = BTreeSet::new();
        for id in &matched_nodes {
            collect_leaves(id, &map, &children, &mut leaves);
        }
        let mut leaves = leaves.into_iter().collect::<Vec<_>>();
        leaves.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        Ok(GlobSelection {
            matched_nodes: matched_nodes.into_iter().collect(),
            leaves,
        })
    }
}

fn component_matches(tokens: &[ComponentToken], value: &str) -> bool {
    let chars = value.chars().collect::<Vec<_>>();
    let cols = chars.len() + 1;
    let mut table = vec![false; (tokens.len() + 1) * cols];
    table[tokens.len() * cols + chars.len()] = true;
    for token_index in (0..tokens.len()).rev() {
        for char_index in (0..=chars.len()).rev() {
            table[token_index * cols + char_index] = match tokens[token_index] {
                ComponentToken::Literal(expected) => {
                    char_index < chars.len()
                        && chars[char_index] == expected
                        && table[(token_index + 1) * cols + char_index + 1]
                }
                ComponentToken::Question => {
                    char_index < chars.len() && table[(token_index + 1) * cols + char_index + 1]
                }
                ComponentToken::Star => {
                    table[(token_index + 1) * cols + char_index]
                        || (char_index < chars.len() && table[token_index * cols + char_index + 1])
                }
            };
        }
    }
    table[0]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobNodeKind {
    Suite,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobNode {
    id: String,
    parent: Option<String>,
    kind: GlobNodeKind,
}

impl GlobNode {
    pub fn suite(id: impl Into<String>, parent: Option<impl Into<String>>) -> Self {
        Self {
            id: id.into(),
            parent: parent.map(Into::into),
            kind: GlobNodeKind::Suite,
        }
    }

    pub fn test(id: impl Into<String>, parent: Option<impl Into<String>>) -> Self {
        Self {
            id: id.into(),
            parent: parent.map(Into::into),
            kind: GlobNodeKind::Test,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn parent(&self) -> Option<&str> {
        self.parent.as_deref()
    }
    pub const fn kind(&self) -> GlobNodeKind {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobSelection {
    matched_nodes: Vec<String>,
    leaves: Vec<String>,
}

impl GlobSelection {
    pub fn matched_nodes(&self) -> &[String] {
        &self.matched_nodes
    }
    pub fn leaves(&self) -> &[String] {
        &self.leaves
    }
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }
}

fn collect_leaves(
    id: &str,
    nodes: &BTreeMap<String, GlobNode>,
    children: &BTreeMap<String, Vec<String>>,
    leaves: &mut BTreeSet<String>,
) {
    if nodes[id].kind == GlobNodeKind::Test {
        leaves.insert(id.into());
        return;
    }
    for child in children.get(id).into_iter().flatten() {
        collect_leaves(child, nodes, children, leaves);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_match_supports_literal_star_question_and_unicode_scalars() {
        let pattern = GlobPattern::parse("application::unit::*::add?Returns*").unwrap();
        assert_eq!(pattern.source(), "application::unit::*::add?Returns*");
        assert_eq!(pattern.component_count(), 4);
        assert!(pattern.matches("application::unit::math::add1ReturnsSum"));
        assert!(!pattern.matches("application::unit::math::addReturnsSum::extra"));
        let unicode = GlobPattern::parse("café::?*").unwrap();
        assert!(unicode.matches("café::ñandú"));
        assert!(!unicode.matches("cafe::ñandú"));
    }

    #[test]
    fn globstar_matches_zero_or_many_complete_components() {
        let pattern = GlobPattern::parse("application::**::creates*").unwrap();
        assert!(pattern.matches("application::createsThing"));
        assert!(pattern.matches("application::integration::db::createsThing"));
        assert!(!pattern.matches("other::integration::db::createsThing"));
    }

    #[test]
    fn invalid_patterns_are_rejected_before_matching() {
        assert!(matches!(
            GlobPattern::parse(""),
            Err(GlobError::EmptyPattern)
        ));
        assert!(matches!(
            GlobPattern::parse("a::::b"),
            Err(GlobError::EmptyComponent { .. })
        ));
        assert!(matches!(
            GlobPattern::parse("a:b"),
            Err(GlobError::IsolatedColon { .. })
        ));
        assert!(matches!(
            GlobPattern::parse("a**b"),
            Err(GlobError::EmbeddedGlobStar { .. })
        ));
        assert!(matches!(
            GlobPattern::parse("a**::b"),
            Err(GlobError::EmbeddedGlobStar { .. })
        ));
        assert!(matches!(
            GlobPattern::parse("**::**"),
            Err(GlobError::AdjacentGlobStars { .. })
        ));
    }

    #[test]
    fn select_deduplicates_overlapping_ids_and_sorts_by_utf8_bytes() {
        let pattern = GlobPattern::parse("app::**").unwrap();
        let selected = pattern.select(["app::z", "app::a", "app::z", "other::x"]);
        assert_eq!(selected, ["app::a", "app::z"]);
    }

    #[test]
    fn suite_matches_select_all_descendant_leaves_and_union_is_deduplicated() {
        let pattern = GlobPattern::parse("app::integration").unwrap();
        let selection = pattern
            .select_tree([
                GlobNode::suite("app", None::<String>),
                GlobNode::suite("app::integration", Some("app")),
                GlobNode::test("app::integration::one", Some("app::integration")),
                GlobNode::test("app::integration::two", Some("app::integration")),
                GlobNode::test("app::unit::three", Some("app")),
            ])
            .unwrap();
        assert_eq!(selection.matched_nodes(), ["app::integration"]);
        assert_eq!(
            selection.leaves(),
            ["app::integration::one", "app::integration::two"]
        );
    }

    #[test]
    fn matching_a_leaf_selects_only_that_leaf() {
        let pattern = GlobPattern::parse("app::unit::one").unwrap();
        let suite = GlobNode::suite("app", None::<String>);
        assert_eq!(suite.id(), "app");
        assert_eq!(suite.parent(), None);
        assert_eq!(suite.kind(), GlobNodeKind::Suite);
        let child = GlobNode::test("app::unit::one", Some("app"));
        assert_eq!(child.id(), "app::unit::one");
        assert_eq!(child.parent(), Some("app"));
        assert_eq!(child.kind(), GlobNodeKind::Test);
        let selection = pattern
            .select_tree([
                suite,
                child,
                GlobNode::test("app::unit::two", Some("app")),
            ])
            .unwrap();
        assert_eq!(selection.leaves(), ["app::unit::one"]);
    }

    #[test]
    fn no_match_is_a_valid_empty_selection() {
        let selection = GlobPattern::parse("missing")
            .unwrap()
            .select_tree([GlobNode::test("present", None::<String>)])
            .unwrap();
        assert!(selection.is_empty());
    }

    #[test]
    fn malformed_trees_are_rejected() {
        let pattern = GlobPattern::parse("**").unwrap();
        assert!(matches!(
            pattern.select_tree([GlobNode::test("", None::<String>)]),
            Err(GlobError::EmptyNodeId)
        ));
        assert!(matches!(
            pattern.select_tree([
                GlobNode::test("x", None::<String>),
                GlobNode::test("x", None::<String>)
            ]),
            Err(GlobError::DuplicateNode(_))
        ));
        assert!(matches!(
            pattern.select_tree([GlobNode::test("x", Some("missing"))]),
            Err(GlobError::UnknownParent { .. })
        ));
        assert!(matches!(
            pattern.select_tree([
                GlobNode::test("parent", None::<String>),
                GlobNode::test("child", Some("parent"))
            ]),
            Err(GlobError::LeafParent { .. })
        ));
    }
}
