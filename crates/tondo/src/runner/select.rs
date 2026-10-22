//! Which tests run and in what order (testing 8.2 to 8.4): a selector, a
//! shard of the selection, and the execution plan.

use super::Tree;
use crate::hash::sha256;

pub enum Selector {
    All,
    /// A substring of a test's identity.
    Filter(String),
    /// A glob over the identity of a test or a suite.
    Glob(Glob),
    /// The identity of a test or a suite.
    Exact(String),
}

/// A portable glob (8.2): components split by `::`, where `*` matches any
/// text inside a component, `?` one character, and a component `**` any
/// number of whole components.
pub struct Glob(Vec<Part>, String);

enum Part {
    Any,
    Component(Vec<char>),
}

impl Glob {
    pub fn parse(pattern: &str) -> Result<Glob, String> {
        let invalid = |why: &str| Err(format!("invalid --glob {pattern:?}: {why}"));
        if pattern.is_empty() {
            return invalid("it is empty");
        }
        let mut parts = Vec::new();
        for component in pattern.split("::") {
            if component.is_empty() {
                return invalid("it has an empty component");
            }
            if component.contains(':') {
                return invalid("a `:` stands alone; components are separated by `::`");
            }
            if component == "**" {
                if matches!(parts.last(), Some(Part::Any)) {
                    return invalid("two `**` components follow each other");
                }
                parts.push(Part::Any);
            } else if component.contains("**") {
                return invalid("`**` is a component on its own; inside one, write `*`");
            } else {
                parts.push(Part::Component(component.chars().collect()));
            }
        }
        Ok(Glob(parts, pattern.to_string()))
    }

    /// The pattern as written.
    pub fn text(&self) -> &str {
        &self.1
    }

    /// Whether the whole identity matches.
    pub fn matches(&self, id: &str) -> bool {
        let components: Vec<Vec<char>> = id.split("::").map(|c| c.chars().collect()).collect();
        // `reach[j]`: the parts so far match the first `j` components.
        let mut reach = vec![false; components.len() + 1];
        reach[0] = true;
        for part in &self.0 {
            let mut next = vec![false; components.len() + 1];
            for j in 0..=components.len() {
                if !reach[j] {
                    continue;
                }
                match part {
                    Part::Any => next[j..].iter_mut().for_each(|n| *n = true),
                    Part::Component(pattern) => {
                        if j < components.len() && component_matches(pattern, &components[j]) {
                            next[j + 1] = true;
                        }
                    }
                }
            }
            reach = next;
        }
        reach[components.len()]
    }
}

fn component_matches(pattern: &[char], text: &[char]) -> bool {
    // `reach[j]`: the pattern so far matches the first `j` characters.
    let mut reach = vec![false; text.len() + 1];
    reach[0] = true;
    for &p in pattern {
        let mut next = vec![false; text.len() + 1];
        for j in 0..=text.len() {
            if !reach[j] {
                continue;
            }
            match p {
                '*' => next[j..].iter_mut().for_each(|n| *n = true),
                '?' if j < text.len() => next[j + 1] = true,
                c if j < text.len() && text[j] == c => next[j + 1] = true,
                _ => {}
            }
        }
        reach = next;
    }
    reach[text.len()]
}

/// The tests a selector selects, in canonical order.
pub fn select(tree: &Tree, selector: &Selector) -> Vec<u32> {
    let matches = |node: u32| match selector {
        Selector::All => true,
        Selector::Filter(text) => tree.id(node).contains(text.as_str()),
        Selector::Glob(glob) => glob.matches(tree.id(node)),
        Selector::Exact(id) => tree.id(node) == id,
    };
    let mut selected: Vec<u32> = (1..tree.nodes.len() as u32)
        .filter(|&test| !tree.nodes[test as usize].suite)
        .filter(|&test| {
            let mut node = test;
            // A test matches, or a suite around it does (but not for --filter).
            loop {
                if matches(node) {
                    return true;
                }
                node = tree.nodes[node as usize].parent;
                if node == 0 || matches!(selector, Selector::Filter(_)) {
                    return false;
                }
            }
        })
        .collect();
    selected.sort_by(|&a, &b| tree.id(a).cmp(tree.id(b)));
    selected
}

/// The shard, from 1 to `count`, a test belongs to (8.3, `sha256-mod-v1`).
pub fn shard_of(id: &str, count: u64) -> u64 {
    let digest = sha256(&[b"tondo-test-shard-v1\0".as_slice(), id.as_bytes()].concat());
    let remainder = digest.iter().fold(0u128, |r, &b| (r * 256 + b as u128) % count as u128);
    1 + remainder as u64
}

pub enum Order {
    Canonical,
    /// With its seed, and whether `tondo test` chose it.
    Random(u64, bool),
}

/// The tests in the order they are dispatched (8.4): by identity, or with
/// the members of each node shuffled by their digest under the seed; a
/// suite's tests stay together either way.
pub fn plan(tree: &Tree, selected: &[u32], order: &Order) -> Vec<u32> {
    let Order::Random(seed, _) = order else { return selected.to_vec() };
    let mut plan = Vec::new();
    visit(tree, selected, *seed, 0, &mut plan);
    plan
}

fn visit(tree: &Tree, selected: &[u32], seed: u64, node: u32, plan: &mut Vec<u32>) {
    let mut members: Vec<(u32, [u8; 32])> = (1..tree.nodes.len() as u32)
        .filter(|&m| tree.nodes[m as usize].parent == node && selected.iter().any(|&t| tree.within(t, m)))
        .map(|m| (m, order_digest(seed, tree.id(node), tree.id(m))))
        .collect();
    members.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| tree.id(a.0).cmp(tree.id(b.0))));
    for (member, _) in members {
        if tree.nodes[member as usize].suite {
            visit(tree, selected, seed, member, plan);
        } else {
            plan.push(member);
        }
    }
}

/// `sha256-tree-v1`: the digest a member is ordered by under its parent.
pub fn order_digest(seed: u64, parent: &str, child: &str) -> [u8; 32] {
    let mut bytes = b"tondo-test-order-v1\0".to_vec();
    bytes.extend_from_slice(&seed.to_be_bytes());
    bytes.push(0);
    bytes.extend_from_slice(parent.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(child.as_bytes());
    sha256(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_whole_components() {
        let glob = Glob::parse("app::integration::**::creates*").unwrap();
        assert!(glob.matches("app::integration::users::createsUser"));
        assert!(glob.matches("app::integration::createsUser"));
        assert!(glob.matches("app::integration::a::b::creates"));
        assert!(!glob.matches("app::unit::users::createsUser"));
        assert!(!glob.matches("app::integration::users::createsUser::more"));
        assert!(Glob::parse("a::?b").unwrap().matches("a::xb"));
        for invalid in ["", "a::::b", "a:b", "a::**::**", "a**", "a::b**c"] {
            assert!(Glob::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn shards_and_orders_match_the_spec_vectors() {
        assert_eq!(shard_of("application::unit::math::arithmetic::addReturnsSum", 8), 8);
        let parent = "application::unit::math::arithmetic";
        let hex = |d: [u8; 32]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(
            hex(order_digest(0x5eed, parent, &format!("{parent}::subtractReturnsDifference"))),
            "00c637b1e275874ed716704cd93b6b8928b23d378fd457c41a38060684094a68"
        );
        assert_eq!(
            hex(order_digest(0x5eed, parent, &format!("{parent}::addReturnsSum"))),
            "9bd8be59d09c20c60e7549466be069ce52f98d8cff6d608878d194029e9650cf"
        );
    }
}
