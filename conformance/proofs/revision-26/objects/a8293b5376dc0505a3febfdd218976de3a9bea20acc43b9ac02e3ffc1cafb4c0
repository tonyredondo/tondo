use tondo_conformance::manifest::SemanticQuery;
use tondo_conformance::protocol::{
    AdapterAction, AdapterRequest, AdapterResult, CompilationState, TargetSelection, WireOperation,
    WireSemanticAction, WireSource, WireSourceAction, WireSourceForm,
};
use tondo_reference_adapter::ReferenceAdapter;

const SOURCE: &str = "\
pub enum Payload[T] {
    Empty
    Tuple(T)
    Record { item: T }
}

fn pass_option(optional: Int?): Int? {
    optional
}

fn pass_union(choice: Int | String): Int | String {
    choice
}

fn pass_payload(payload: Payload[Int]): Payload[Int] {
    payload
}

fn pass_generic[T](generic: T): T {
    generic
}
";

const EDGE_SOURCE: &str = "\
pub type UserId = Int

pub type State = {
    count: Int
    values: Array[Int]
}

pub enum Shape {
    Point
    Circle(Int)
    Rectangle { width: Int, height: Int }
}

fn lift(value: Int?): Int? {
    let item = value?
    some(item)
}

fn make(id: UserId, name: String): (State, Shape, Shape, Shape) {
    _ = id
    _ = name
    (
        State { count: 0, values: [1, 2, 3, 4] },
        Shape.Point,
        Shape.Circle(2),
        Shape.Rectangle { width: 3, height: 4 },
    )
}

fn inspect(value: ref Int): Int {
    value
}

fn update_both(left: mut Int, right: mut Int) {
    left += 1
    right += 1
}

fn projections(
    state: mut State,
    pair: var (Int, String),
    values: var Array[Int],
    replacement: Array[Int],
    entries: var Map[String, Int],
) {
    state.count = 1
    state.values[0] = 2
    pair.0 = 3
    values[0] = pair.0
    values[1:3] = replacement
    values[::2] += 10
    entries[\"answer\"] = 42
    update_both(mut values[0], mut values[1])
    let item: Int = values[0]
    let view: Array[Int] = values[:]
    let found: Int? = entries[\"answer\"]
    _ = item
    _ = view
    _ = found
}

fn closure_facts(): Int {
    let operation = (value: ref Int) {
        value
    }
    let item = 1
    operation(ref item)
}

fn pattern_facts(shape: Shape, optional: Int?, outcome: Int ! String): Int ! String {
    let first = match shape {
        Shape.Point => 0
        Shape.Circle(radius) => radius
        Shape.Rectangle { width, height } => width + height
    }
    let second = match optional {
        some(value) => value
        none => 0
    }
    let third = match outcome {
        ok(value) => value
        err(error) => fail error
    }
    first + second + third
}

fn array_pattern(values: Array[Int]): Int {
    match values {
        [first, ..remaining] => first
        _ => 0
    }
}

fn ref_facts(reference: Ref[Int]): Int {
    _ = inspect(ref reference.value)
    reference.value
}

unsafe fn pointer_facts(pointer: Pointer[Int], address: UInt64): UInt64 {
    let value = pointer.read()
    pointer.write(value)
    let advanced = pointer.offset(1)
    let bytes = advanced.cast[Byte]()
    let reconstructed = address.toPointer[Int]()
    _ = reconstructed
    _ = bytes.address()
    Pointer.address(bytes)
}
";

const ADVANCED_SOURCE: &str = "\
pub type Label = {
    text: String
}

pub trait Summary {
    fn summarize(self): String
}

impl Summary for Int {
    fn summarize(self): String {
        \"integer\"
    }
}

impl Display for Label {
    fn display(self): String {
        self.text
    }
}

fn hidden(value: Int): impl Summary + Discard {
    value
}

fn consume[T: Discard](value: T) {
    _ = value
}

fn deferred(value: Int) {
    let owner = hidden(value)
    defer consume(owner)
}

fn collect(prefix: String, parts: ...String): Array[String] {
    _ = prefix
    parts
}

fn fallible(flag: Bool): Int ! (Bool | String) {
    if flag {
        1
    } else {
        fail \"bad\"
    }
}

fn propagate(flag: Bool): Int ! (Bool | String) {
    fallible(flag)?
}

fn collections(
    label: Label,
    key: String,
    values: var Array[Int],
    groups: var Array[Array[Int]],
    entries: var Map[Int, Int],
): Int {
    let dynamic: Map[String, Int] = [key: 1, key: 2]
    let repeated = values.repeat(2)
    let combined = values.concat(repeated)
    let range = 0..=3
    var total = 0
    for ref value in values {
        total += value
    }
    for mut value in values {
        value += 1
    }
    for var group in groups {
        group = [9]
    }
    for (ref entryKey, mut entryValue) in entries {
        entryValue += entryKey
    }
    _ = entries.remove(1)
    _ = 1 in range
    _ = 1 in combined
    _ = dynamic[key]
    let rendered = \"{label}:{total}\"
    let updated = label with { text: rendered }
    _ = updated
    total
}

async fn load(value: Int): Int {
    value
}

async fn concurrent(value: Int): Int {
    let direct = await load(value)
    scope {
        let task = spawn load(direct)
        let transferred = task
        await transferred
    }
}
";

#[test]
fn semantic_queries_expose_option_union_enum_and_generic_shapes() {
    let action = WireSourceAction {
        operation: WireOperation::Check,
        form: WireSourceForm::Module,
        root: "case.to".into(),
        sources: vec![WireSource {
            source_id: "test:semantic-shapes".into(),
            module: "main".into(),
            logical_path: "case.to".into(),
            contents_hex: tondo_conformance::encode_hex(SOURCE.as_bytes()),
        }],
        warning_profiles: Vec::new(),
        arguments: Vec::new(),
        gc_threshold: None,
    };
    let (optional_start, optional_end) = occurrence_span(SOURCE, "optional", 1);
    let (choice_start, choice_end) = occurrence_span(SOURCE, "choice", 1);
    let (payload_start, payload_end) = occurrence_span(SOURCE, "payload", 2);
    let (generic_start, generic_end) = occurrence_span(SOURCE, "generic", 2);
    let request = AdapterRequest::new(
        1,
        "semantic-shapes",
        TargetSelection {
            name: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capabilities: vec!["console".into(), "process".into()],
        },
        AdapterAction::Semantic(WireSemanticAction {
            source: action,
            queries: vec![
                SemanticQuery::TypeFacts {
                    file: "case.to".into(),
                    start: optional_start,
                    end: optional_end,
                },
                SemanticQuery::TypeMembers {
                    file: "case.to".into(),
                    start: choice_start,
                    end: choice_end,
                },
                SemanticQuery::TypeMembers {
                    file: "case.to".into(),
                    start: payload_start,
                    end: payload_end,
                },
                SemanticQuery::TypeFacts {
                    file: "case.to".into(),
                    start: generic_start,
                    end: generic_end,
                },
            ],
        }),
    );

    let AdapterResult::Ok { observation } = ReferenceAdapter.handle(&request).result else {
        panic!("semantic shape request must succeed");
    };
    assert!(observation.diagnostics.is_empty());
    let queries = observation.data["queries"]
        .as_array()
        .expect("semantic queries must be an array");

    assert_eq!(queries[0]["facts"]["shape"]["kind"], "option");
    assert_eq!(queries[1]["members"]["kind"], "union");
    assert_eq!(
        queries[1]["members"]["members"]
            .as_array()
            .expect("union members must be an array")
            .len(),
        2
    );
    assert_eq!(queries[2]["members"]["kind"], "enum");
    let variants = queries[2]["members"]["variants"]
        .as_array()
        .expect("enum variants must be an array");
    assert_eq!(variants.len(), 3);
    assert_eq!(variants[0]["payload"]["kind"], "unit");
    assert_eq!(variants[1]["payload"]["kind"], "tuple");
    assert_eq!(variants[2]["payload"]["kind"], "record");
    assert_eq!(queries[3]["facts"]["shape"]["kind"], "generic-parameter");
}

#[test]
fn semantic_snapshot_exposes_unsafe_borrow_projection_and_pattern_contracts() {
    let action = WireSourceAction {
        operation: WireOperation::Check,
        form: WireSourceForm::Module,
        root: "case.to".into(),
        sources: vec![WireSource {
            source_id: "test:semantic-edges".into(),
            module: "main".into(),
            logical_path: "case.to".into(),
            contents_hex: tondo_conformance::encode_hex(EDGE_SOURCE.as_bytes()),
        }],
        warning_profiles: Vec::new(),
        arguments: Vec::new(),
        gc_threshold: None,
    };
    let compile_request = AdapterRequest::new(
        0,
        "semantic-edges-check",
        TargetSelection {
            name: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capabilities: vec!["console".into(), "process".into()],
        },
        AdapterAction::Source(action.clone()),
    );
    let compile_response = ReferenceAdapter.handle(&compile_request);
    let AdapterResult::Ok {
        observation: compile_observation,
    } = compile_response.result
    else {
        panic!("semantic edge check must return an observation: {compile_response:#?}");
    };
    assert_eq!(
        compile_observation.compilation,
        CompilationState::Success,
        "{:#?}",
        compile_observation.diagnostics
    );
    assert!(
        compile_observation.diagnostics.is_empty(),
        "{:#?}",
        compile_observation.diagnostics
    );
    let request = AdapterRequest::new(
        1,
        "semantic-edges",
        TargetSelection {
            name: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capabilities: vec!["console".into(), "process".into()],
        },
        AdapterAction::Semantic(WireSemanticAction {
            source: action,
            queries: vec![SemanticQuery::SemanticSnapshot {
                file: "case.to".into(),
            }],
        }),
    );

    let response = ReferenceAdapter.handle(&request);
    let AdapterResult::Ok { observation } = response.result else {
        panic!("semantic edge request must succeed: {response:#?}");
    };
    assert!(
        observation.diagnostics.is_empty(),
        "{:#?}",
        observation.diagnostics
    );
    let snapshot = &observation.data["queries"][0];
    assert_eq!(snapshot["schema"], "tondo-semantic-snapshot-0.1/1");
    assert!(snapshot["unsafe"]["operations"].as_array().unwrap().len() >= 6);
    assert!(
        snapshot["borrow_bindings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|binding| binding["kind"] == "closure-parameter")
    );
    assert!(
        snapshot["public_types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|contract| contract["shape"] == "newtype")
    );
    assert!(
        snapshot["expressions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|expression| expression["semantic"]["sugar"]["kind"] == "question")
    );
    let ownership = snapshot["ownership"]["functions"].as_array().unwrap();
    assert!(
        ownership
            .iter()
            .flat_map(|function| function["dynamic_checks"].as_array().unwrap())
            .any(|check| check["kind"] == "index-overlap" || check["kind"] == "place-overlap")
    );
    assert!(
        ownership
            .iter()
            .flat_map(|function| function["loans"].as_array().unwrap())
            .flat_map(|loan| loan["origin"]["projections"].as_array().unwrap())
            .any(|projection| matches!(
                projection["kind"].as_str(),
                Some("field" | "index" | "slice" | "ref-value")
            ))
    );
}

#[test]
fn semantic_queries_and_snapshot_cover_advanced_language_contracts_end_to_end() {
    let action = WireSourceAction {
        operation: WireOperation::Check,
        form: WireSourceForm::Module,
        root: "advanced.to".into(),
        sources: vec![WireSource {
            source_id: "test:semantic-advanced".into(),
            module: "main".into(),
            logical_path: "advanced.to".into(),
            contents_hex: tondo_conformance::encode_hex(ADVANCED_SOURCE.as_bytes()),
        }],
        warning_profiles: Vec::new(),
        arguments: Vec::new(),
        gc_threshold: None,
    };
    let hidden_declaration = occurrence_span(ADVANCED_SOURCE, "hidden", 0);
    let hidden_call = occurrence_span(ADVANCED_SOURCE, "hidden(value)", 0);
    let collect_declaration = occurrence_span(ADVANCED_SOURCE, "collect", 0);
    let fallible_call = occurrence_span(ADVANCED_SOURCE, "fallible(flag)", 0);
    let union_annotation = occurrence_span(ADVANCED_SOURCE, "Bool | String", 0);
    let dynamic_map = occurrence_span(ADVANCED_SOURCE, "[key: 1, key: 2]", 0);
    let request = AdapterRequest::new(
        2,
        "semantic-advanced",
        TargetSelection {
            name: "tondo-vm-hosted".into(),
            profile: "hosted".into(),
            capabilities: vec!["console".into(), "process".into()],
        },
        AdapterAction::Semantic(WireSemanticAction {
            source: action,
            queries: vec![
                SemanticQuery::FormattedAst,
                SemanticQuery::ExpressionType {
                    file: "advanced.to".into(),
                    start: hidden_call.0,
                    end: hidden_call.1,
                },
                SemanticQuery::Entities {
                    file: "advanced.to".into(),
                    start: hidden_declaration.0,
                    end: hidden_declaration.1,
                },
                SemanticQuery::References {
                    file: "advanced.to".into(),
                    start: hidden_declaration.0,
                    end: hidden_declaration.1,
                },
                SemanticQuery::Signature {
                    file: "advanced.to".into(),
                    start: hidden_declaration.0,
                    end: hidden_declaration.1,
                },
                SemanticQuery::Signature {
                    file: "advanced.to".into(),
                    start: collect_declaration.0,
                    end: collect_declaration.1,
                },
                SemanticQuery::TypeMembers {
                    file: "advanced.to".into(),
                    start: union_annotation.0,
                    end: union_annotation.1,
                },
                SemanticQuery::ClosedCallErrors {
                    file: "advanced.to".into(),
                    start: fallible_call.0,
                    end: fallible_call.1,
                },
                SemanticQuery::TypeFacts {
                    file: "advanced.to".into(),
                    start: hidden_call.0,
                    end: hidden_call.1,
                },
                SemanticQuery::ExpressionFacts {
                    file: "advanced.to".into(),
                    start: dynamic_map.0,
                    end: dynamic_map.1,
                },
                SemanticQuery::SemanticSnapshot {
                    file: "advanced.to".into(),
                },
            ],
        }),
    );

    let response = ReferenceAdapter.handle(&request);
    let AdapterResult::Ok { observation } = response.result else {
        panic!("advanced semantic request must succeed: {response:#?}");
    };
    assert_eq!(
        observation.compilation,
        CompilationState::Success,
        "{:#?}",
        observation.diagnostics
    );
    assert!(
        observation.diagnostics.is_empty(),
        "{:#?}",
        observation.diagnostics
    );
    assert!(
        observation.data["expression_check_complete"]
            .as_bool()
            .unwrap()
    );
    let queries = observation.data["queries"].as_array().unwrap();
    assert_eq!(queries.len(), 11);
    assert_eq!(queries[0]["query"], "formatted-ast");
    assert_eq!(queries[0]["encoding"], "utf-8");
    assert!(
        queries[1]["type"]["canonical"]
            .as_str()
            .unwrap()
            .ends_with("hidden#result")
    );
    assert_eq!(queries[1]["type"]["shape"]["kind"], "opaque-result");
    assert!(
        queries[2]["entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entity| entity["kind"] == "symbol" && entity["name"] == "hidden")
    );
    assert!(!queries[3]["references"].as_array().unwrap().is_empty());
    assert_eq!(
        queries[4]["signature"]["opaque_result"]["bounds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        queries[5]["signature"]["parameters"][1]["variadic_element"],
        "String"
    );
    assert_eq!(queries[6]["members"]["kind"], "union");
    assert!(!queries[7]["errors"].as_array().unwrap().is_empty());
    assert_eq!(queries[8]["facts"]["shape"]["kind"], "opaque-result");
    assert_eq!(
        queries[9]["facts"]["semantic"]["dynamic_duplicate_check"]["status"],
        "elided"
    );
    assert_eq!(
        queries[9]["facts"]["semantic"]["dynamic_duplicate_check"]["proof"],
        "value-type-satisfies-Discard"
    );

    let snapshot = &queries[10];
    assert_eq!(snapshot["schema"], "tondo-semantic-snapshot-0.1/1");
    assert_eq!(snapshot["opaque_results"].as_array().unwrap().len(), 1);
    assert!(
        snapshot["iterators"]
            .as_array()
            .unwrap()
            .iter()
            .any(|iterator| iterator["cursor"]
                .as_str()
                .unwrap()
                .starts_with("cursor[mut,"))
    );
    assert!(
        snapshot["ownership"]["functions"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|function| function["joins"].as_array().unwrap())
            .any(|join| join["state"] == "consumed"
                && !join["transfers"].as_array().unwrap().is_empty())
    );
    assert!(
        snapshot["ownership"]["functions"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|function| function["affine_values"].as_array().unwrap())
            .flat_map(|value| value["events"].as_array().unwrap())
            .any(|event| event["cause"] == "defer-registration")
    );
}

fn occurrence_span(source: &str, needle: &str, occurrence: usize) -> (u32, u32) {
    let start = source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(start, _)| start)
        .expect("the requested occurrence must exist");
    (
        u32::try_from(start).expect("test source must fit u32"),
        u32::try_from(start + needle.len()).expect("test source must fit u32"),
    )
}
