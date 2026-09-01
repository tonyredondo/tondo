use tondo_reliability::sync_collection_model::{
    CollectionAction, CollectionKind, CollectionModelError, CollectionResult, CollectionSeed,
    HistoryOperation, MAX_COLLECTION_FUZZ_STEPS, MAX_LINEARIZABILITY_OPS, ReferenceCollection,
    SharedCollectionModel, is_linearizable, run_collection_fuzz_case,
};

fn apply(model: &mut ReferenceCollection, action: CollectionAction) -> CollectionResult {
    model.apply(&action).unwrap()
}

#[test]
fn reference_collections_preserve_order_cas_and_snapshot_contracts() {
    let mut array = ReferenceCollection::from_seed(CollectionSeed::Array(vec![1, 2, 3])).unwrap();
    assert_eq!(
        apply(&mut array, CollectionAction::ArrayGet { index: 9 }),
        CollectionResult::Optional(None)
    );
    assert_eq!(
        apply(
            &mut array,
            CollectionAction::ArraySet { index: 1, value: 7 },
        ),
        CollectionResult::Optional(Some(2))
    );
    assert_eq!(
        apply(
            &mut array,
            CollectionAction::ArrayCompareExchange {
                index: 1,
                expected: 2,
                desired: 8,
            },
        ),
        CollectionResult::CompareExchange {
            exchanged: false,
            observed: Some(7),
        }
    );
    assert_eq!(
        apply(&mut array, CollectionAction::ArraySnapshot),
        CollectionResult::Values(vec![1, 7, 3])
    );
    assert_eq!(
        array.apply(&CollectionAction::ArraySet { index: 8, value: 0 }),
        Err(CollectionModelError::InvalidIndex)
    );

    let mut map =
        ReferenceCollection::from_seed(CollectionSeed::Map(vec![(1, 10), (2, 20)])).unwrap();
    assert_eq!(
        apply(&mut map, CollectionAction::MapInsert { key: 1, value: 11 },),
        CollectionResult::Optional(Some(10))
    );
    assert_eq!(
        apply(&mut map, CollectionAction::MapRemove { key: 1 }),
        CollectionResult::Optional(Some(11))
    );
    assert_eq!(
        apply(&mut map, CollectionAction::MapInsert { key: 1, value: 12 },),
        CollectionResult::Optional(None)
    );
    assert_eq!(
        apply(&mut map, CollectionAction::MapSnapshot),
        CollectionResult::Entries(vec![(2, 20), (1, 12)])
    );
    assert_eq!(
        apply(
            &mut map,
            CollectionAction::MapCompareExchange {
                key: 2,
                expected: Some(99),
                desired: Some(21),
            },
        ),
        CollectionResult::CompareExchange {
            exchanged: false,
            observed: Some(20),
        }
    );

    let mut set = ReferenceCollection::from_seed(CollectionSeed::Set(vec![4, 4, 5])).unwrap();
    assert_eq!(set.len(), 2);
    assert_eq!(
        apply(&mut set, CollectionAction::SetInsert { value: 4 }),
        CollectionResult::Bool(false)
    );
    assert_eq!(
        apply(&mut set, CollectionAction::SetRemove { value: 4 }),
        CollectionResult::Bool(true)
    );
    assert_eq!(
        apply(&mut set, CollectionAction::SetInsert { value: 6 }),
        CollectionResult::Bool(true)
    );
    assert_eq!(
        apply(&mut set, CollectionAction::SetSnapshot),
        CollectionResult::Values(vec![5, 6])
    );

    let mut stack = ReferenceCollection::from_seed(CollectionSeed::Stack(vec![1, 2])).unwrap();
    assert_eq!(
        apply(&mut stack, CollectionAction::StackPush { value: 3 }),
        CollectionResult::Unit
    );
    assert_eq!(
        apply(&mut stack, CollectionAction::StackPeek),
        CollectionResult::Optional(Some(3))
    );
    assert_eq!(
        apply(&mut stack, CollectionAction::StackPop),
        CollectionResult::Optional(Some(3))
    );
    assert_eq!(
        apply(&mut stack, CollectionAction::StackSnapshot),
        CollectionResult::Values(vec![2, 1])
    );

    let mut queue = ReferenceCollection::from_seed(CollectionSeed::Queue(vec![1, 2])).unwrap();
    assert_eq!(
        apply(&mut queue, CollectionAction::QueueEnqueue { value: 3 }),
        CollectionResult::Unit
    );
    assert_eq!(
        apply(&mut queue, CollectionAction::QueuePeek),
        CollectionResult::Optional(Some(1))
    );
    assert_eq!(
        apply(&mut queue, CollectionAction::QueueDequeue),
        CollectionResult::Optional(Some(1))
    );
    assert_eq!(
        apply(&mut queue, CollectionAction::QueueSnapshot),
        CollectionResult::Values(vec![2, 3])
    );
    for model in [array, map, set, stack, queue] {
        model.assert_invariants().unwrap();
    }
}

#[test]
fn bounded_histories_accept_linearizable_orders_and_reject_impossible_outcomes() {
    let linearizable = [
        HistoryOperation::new(
            0,
            0,
            4,
            CollectionAction::QueueEnqueue { value: 7 },
            CollectionResult::Unit,
        ),
        HistoryOperation::new(
            1,
            1,
            2,
            CollectionAction::QueueDequeue,
            CollectionResult::Optional(Some(7)),
        ),
    ];
    assert!(is_linearizable(CollectionSeed::Queue(Vec::new()), &linearizable).unwrap());

    let impossible = [
        HistoryOperation::new(
            0,
            0,
            3,
            CollectionAction::QueueDequeue,
            CollectionResult::Optional(Some(7)),
        ),
        HistoryOperation::new(
            1,
            1,
            2,
            CollectionAction::QueueDequeue,
            CollectionResult::Optional(Some(7)),
        ),
    ];
    assert!(!is_linearizable(CollectionSeed::Queue(vec![7]), &impossible).unwrap());

    let wrong_kind = [HistoryOperation::new(
        0,
        0,
        1,
        CollectionAction::MapGet { key: 1 },
        CollectionResult::Optional(None),
    )];
    assert_eq!(
        is_linearizable(CollectionSeed::Queue(Vec::new()), &wrong_kind),
        Err(CollectionModelError::WrongKind)
    );

    let too_long = (0..=MAX_LINEARIZABILITY_OPS)
        .map(|index| {
            HistoryOperation::new(
                0,
                index as u16,
                index as u16 + 1,
                CollectionAction::QueuePeek,
                CollectionResult::Optional(None),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        is_linearizable(CollectionSeed::Queue(Vec::new()), &too_long),
        Err(CollectionModelError::Limit)
    );
}

#[test]
fn direct_cursors_are_finite_generation_safe_and_non_destructive() {
    let mut map =
        ReferenceCollection::from_seed(CollectionSeed::Map(vec![(1, 10), (2, 20)])).unwrap();
    let mut map_cursor = map.start_cursor();
    apply(&mut map, CollectionAction::MapRemove { key: 1 });
    apply(&mut map, CollectionAction::MapInsert { key: 1, value: 30 });
    assert_eq!(map_cursor.next(&map).unwrap().unwrap().key, Some(2));
    assert_eq!(map_cursor.next(&map).unwrap(), None);
    assert!(map_cursor.exhausted());
    assert_eq!(map_cursor.seen_generations().len(), 1);

    let mut array = ReferenceCollection::from_seed(CollectionSeed::Array(vec![1, 2])).unwrap();
    let mut array_cursor = array.start_cursor();
    apply(
        &mut array,
        CollectionAction::ArraySet { index: 1, value: 9 },
    );
    assert_eq!(array_cursor.next(&array).unwrap().unwrap().value, 1);
    assert_eq!(array_cursor.next(&array).unwrap().unwrap().value, 9);
    assert_eq!(array_cursor.next(&array).unwrap(), None);

    let mut stack = ReferenceCollection::from_seed(CollectionSeed::Stack(vec![1, 2])).unwrap();
    let mut stack_cursor = stack.start_cursor();
    apply(&mut stack, CollectionAction::StackPush { value: 3 });
    assert_eq!(
        apply(&mut stack, CollectionAction::StackPop),
        CollectionResult::Optional(Some(3))
    );
    assert_eq!(stack_cursor.next(&stack).unwrap().unwrap().value, 2);
    assert_eq!(stack_cursor.next(&stack).unwrap().unwrap().value, 1);
    assert_eq!(stack_cursor.next(&stack).unwrap(), None);
    assert_eq!(
        apply(&mut stack, CollectionAction::StackSnapshot),
        CollectionResult::Values(vec![2, 1])
    );

    let mut queue = ReferenceCollection::from_seed(CollectionSeed::Queue(vec![1, 2])).unwrap();
    let mut queue_cursor = queue.start_cursor();
    apply(&mut queue, CollectionAction::QueueEnqueue { value: 3 });
    assert_eq!(queue_cursor.next(&queue).unwrap().unwrap().value, 1);
    assert_eq!(queue_cursor.next(&queue).unwrap().unwrap().value, 2);
    assert_eq!(queue_cursor.next(&queue).unwrap(), None);
    assert_eq!(queue.len(), 3);
}

#[test]
fn aliases_retain_cursor_sources_and_cleanup_exactly_once() {
    let mut model = SharedCollectionModel::new();
    let handle = model
        .create_seed(CollectionSeed::Map(vec![(1, 10)]))
        .unwrap();
    let alias = model.copy_handle(handle).unwrap();
    let cursor = model.start_cursor(alias).unwrap();
    model.discard_handle(handle).unwrap();
    model.discard_handle(alias).unwrap();
    assert_eq!(model.live_collections(), 1);
    assert_eq!(model.cursor_next(cursor).unwrap().unwrap().key, Some(1));
    assert_eq!(model.cursor_key(cursor).unwrap(), Some(1));
    assert_eq!(model.cursor_next(cursor).unwrap(), None);
    assert_eq!(model.live_collections(), 0);
    assert_eq!(model.cleanup_runs(), 1);
    assert_eq!(
        model.cursor_next(cursor),
        Err(CollectionModelError::InvalidCursor)
    );
    assert_eq!(
        model.discard_handle(handle),
        Err(CollectionModelError::StaleHandle)
    );
    model.assert_invariants().unwrap();
}

#[test]
fn collection_fuzz_is_bounded_replayable_and_leak_free() {
    for seed in 0..4_096_u64 {
        let bytes = seed.to_le_bytes();
        let first = run_collection_fuzz_case(&bytes).unwrap();
        let second = run_collection_fuzz_case(&bytes).unwrap();
        assert_eq!(first, second, "collection replay diverged for seed {seed}");
        assert!(first.steps <= MAX_COLLECTION_FUZZ_STEPS);
        assert_eq!(first.live_handles, 0);
        assert_eq!(first.live_cursors, 0);
        assert_eq!(first.live_collections, 0);
        assert!(first.cleanup_runs >= CollectionKind::ALL.len());
    }
}
