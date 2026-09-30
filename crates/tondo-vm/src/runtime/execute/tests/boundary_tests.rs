//! Runtime rejection and failure-atomicity contracts for corrupted descriptors.

use super::*;
use crate::bytecode::{BytecodeConstantId, BytecodeProjection, BytecodeProjectionKind};

fn rejects<T>(result: Result<T, VmError>, expected: &str) {
    match result {
        Err(VmError::Invariant(message)) => assert!(message.contains(expected), "{message}"),
        Err(error) => panic!("expected invariant containing {expected:?}, got {error}"),
        Ok(_) => panic!("expected invariant containing {expected:?}, got success"),
    }
}

fn frame() -> Frame {
    Frame {
        memory: None,
        function: BytecodeFunctionId::new(0),
        block: BytecodeBlockId::new(0),
        instruction: 0,
        slots: vec![SlotState::Value(Value::Integer(0))],
        loans: Vec::new(),
        task_scopes: Vec::new(),
        cleanups: Vec::new(),
        continuation: None,
        select: None,
    }
}

#[test]
fn malformed_inline_literals_and_call_associations_leave_no_temporary_roots() {
    let (mut program, types) = executor_program();
    let byte = BytecodeTypeId::new(program.types.len() as u32);
    program.types.push(BytecodeType {
        name: "Byte".into(),
        kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Byte),
    });
    let float = BytecodeTypeId::new(program.types.len() as u32);
    program.types.push(BytecodeType {
        name: "Float".into(),
        kind: BytecodeTypeKind::Scalar(BytecodeScalarType::Float),
    });
    let mut host = RejectingHost;
    let mut engine = executor_engine(&program, &mut host);
    for (ty, constant, message) in [
        (
            types.int,
            BytecodeConstant::Integer("invalid".into()),
            "integer literal",
        ),
        (
            byte,
            BytecodeConstant::Integer("256".into()),
            "Byte literal",
        ),
        (
            float,
            BytecodeConstant::Float("invalid".into()),
            "float literal",
        ),
        (
            types.int,
            BytecodeConstant::Char("'ab'".into()),
            "character literal",
        ),
        (
            types.string,
            BytecodeConstant::String("\"unterminated".into()),
            "string literal",
        ),
        (
            types.int,
            BytecodeConstant::Named(BytecodeConstantId::new(u32::MAX)),
            "constant index",
        ),
    ] {
        rejects(engine.inline_constant(ty, &constant), message);
    }
    for callee in [
        Value::Unit,
        Value::Function {
            callable: BytecodeCallableId::new(u32::MAX),
            arguments: vec![],
        },
    ] {
        let message = if matches!(callee, Value::Unit) {
            "callee"
        } else {
            "metadata index"
        };
        rejects(engine.prepare_evaluated_call(callee, Vec::new()), message);
        assert!(engine.temporary_roots.is_empty());
    }
    rejects(
        engine.prepare_evaluated_call(
            Value::Function {
                callable: BytecodeCallableId::new(0),
                arguments: vec![],
            },
            Vec::new(),
        ),
        "kind differs",
    );
    let free_function = program
        .callables
        .iter()
        .position(|callable| callable.closure.is_none() && callable.parameters.is_empty())
        .unwrap();
    let callable = Value::Function {
        callable: BytecodeCallableId::new(free_function as u32),
        arguments: vec![],
    };
    for (target, message) in [
        (
            BytecodeCallArgumentTarget::Receiver,
            "receiver to a free function",
        ),
        (BytecodeCallArgumentTarget::Fixed(u32::MAX), "target index"),
        (
            BytecodeCallArgumentTarget::VariadicSpread,
            "spread is not Array",
        ),
    ] {
        rejects(
            engine.prepare_evaluated_call(callable.clone(), vec![(target, Value::Integer(1))]),
            message,
        );
        assert!(engine.temporary_roots.is_empty());
    }
}

#[test]
fn invalid_projection_bounds_preserve_heap_payloads_and_release_temporary_roots() {
    let (mut program, types) = executor_program();
    let array_ty = BytecodeTypeId::new(program.types.len() as u32);
    program.types.push(BytecodeType {
        name: "Array[Int]".into(),
        kind: BytecodeTypeKind::Intrinsic {
            constructor: BytecodeIntrinsicType::Array,
            arguments: vec![types.int],
        },
    });
    let map_ty = BytecodeTypeId::new(program.types.len() as u32);
    program.types.push(BytecodeType {
        name: "Map[Int,Int]".into(),
        kind: BytecodeTypeKind::Intrinsic {
            constructor: BytecodeIntrinsicType::Map,
            arguments: vec![types.int, types.int],
        },
    });
    let mut host = RejectingHost;
    let mut engine = executor_engine(&program, &mut host);
    engine.frames.push(frame());
    let array = engine
        .allocate(
            array_ty,
            HeapObject::Array(vec![Some(Value::Integer(7))].into()),
            &[],
        )
        .unwrap();
    let map = engine
        .allocate(
            map_ty,
            HeapObject::Map(Vec::new().into()),
            std::slice::from_ref(&array),
        )
        .unwrap();
    let array_handle = match array {
        Value::Heap(handle) => handle,
        _ => unreachable!(),
    };
    for index in [-1, 1] {
        engine.frames[0].slots[0] = SlotState::Value(Value::Integer(index));
        let projection = BytecodeProjection {
            ty: types.int,
            kind: BytecodeProjectionKind::IteratorElement {
                index: BytecodeSlotId::new(0),
            },
        };
        rejects(
            engine.read_projection(0, array.clone(), &projection),
            if index < 0 {
                "position is negative"
            } else {
                "out of bounds"
            },
        );
        rejects(
            engine.write_projection(0, array.clone(), &projection, Value::Integer(99)),
            if index < 0 {
                "position is negative"
            } else {
                "out of bounds"
            },
        );
        rejects(
            engine.read_projection(0, map.clone(), &projection),
            if index < 0 {
                "position is negative"
            } else {
                "out of bounds"
            },
        );
        assert!(engine.temporary_roots.is_empty());
        assert!(
            matches!(engine.heap.get(array_handle).unwrap(), HeapObject::Array(values) if matches!(values[0], Some(Value::Integer(7))))
        );
    }
    for (start, suffix, message) in [(0, 2, "suffix exceeds"), (2, 0, "prefix exceeds")] {
        let projection = BytecodeProjection {
            ty: array_ty,
            kind: BytecodeProjectionKind::ArrayPatternRest { start, suffix },
        };
        rejects(
            engine.read_projection(0, array.clone(), &projection),
            message,
        );
        rejects(
            engine.take_projection(0, array.clone(), &projection),
            message,
        );
        assert!(engine.temporary_roots.is_empty());
    }
    engine.frames[0].slots[0] = SlotState::Value(Value::Integer(9));
    let missing_key = BytecodeProjection {
        ty: types.int,
        kind: BytecodeProjectionKind::Index {
            index: BytecodeSlotId::new(0),
            access: BytecodeIndexAccess::MapEntry,
        },
    };
    rejects(
        engine.read_projection(0, map.clone(), &missing_key),
        "map entry",
    );
    let invalid_array = BytecodeProjection {
        ty: types.int,
        kind: BytecodeProjectionKind::Index {
            index: BytecodeSlotId::new(0),
            access: BytecodeIndexAccess::Array,
        },
    };
    rejects(
        engine.read_projection(0, array.clone(), &invalid_array),
        "unvalidated array index",
    );
    rejects(
        engine.write_projection(0, array.clone(), &invalid_array, Value::Integer(99)),
        "unvalidated array write index",
    );
    assert!(engine.temporary_roots.is_empty());
    assert!(
        matches!(engine.heap.get(array_handle).unwrap(), HeapObject::Array(values) if matches!(values[0], Some(Value::Integer(7))))
    );
}

#[test]
fn task_scope_lookup_rejects_lost_closed_and_foreign_owner_states() {
    let (program, _) = executor_program();
    let mut host = RejectingHost;
    let mut engine = executor_engine(&program, &mut host);
    engine.frames.push(frame());
    let source = BytecodeScopeId::new(0);
    rejects(engine.active_task_scope(0, source), "no active task scope");
    assert_eq!(engine.select_task_scope(0).unwrap(), None);
    engine.frames[0].task_scopes.push(0);
    for state in [
        None,
        Some(RuntimeTaskScope {
            source,
            owner: 1,
            children: vec![],
            closed: false,
        }),
        Some(RuntimeTaskScope {
            source,
            owner: 0,
            children: vec![],
            closed: true,
        }),
    ] {
        engine.task_scopes = vec![state];
        rejects(
            engine.active_task_scope(0, source),
            if engine.task_scopes[0].is_none() {
                "state is missing"
            } else {
                "innermost task scope"
            },
        );
        rejects(
            engine.select_task_scope(0),
            if engine.task_scopes[0].is_none() {
                "state is missing"
            } else {
                "innermost task scope"
            },
        );
    }
    engine.task_scopes = vec![Some(RuntimeTaskScope {
        source,
        owner: 0,
        children: vec![],
        closed: false,
    })];
    assert_eq!(engine.active_task_scope(0, source).unwrap(), 0);
    assert_eq!(engine.select_task_scope(0).unwrap(), Some(0));
    rejects(
        engine.active_task_scope(0, BytecodeScopeId::new(1)),
        "innermost task scope",
    );
}
