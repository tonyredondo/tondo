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
