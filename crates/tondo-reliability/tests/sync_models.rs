use std::fs;

use tondo_compiler::driver::ResourceLimits;
use tondo_reliability::harness::{decode_hex, run};
use tondo_reliability::sync_model::{
    BarrierModel, BarrierPoll, BarrierRole, ConditionModel, ConditionPoll, LockPoll,
    MAX_FUZZ_STEPS, MemoryOrder, MutexModel, OnceModel, OnceResolution, OnceStart, PermitToken,
    PublicationOutcome, SemaphoreModel, SemaphorePoll, SyncModelError, publication_outcome_allowed,
    run_fuzz_case,
};
use tondo_reliability::workspace_root;

#[test]
fn memory_order_matrix_rejects_invalid_operations_and_models_publication() {
    assert!(MemoryOrder::Relaxed.valid_load());
    assert!(MemoryOrder::Acquire.valid_load());
    assert!(!MemoryOrder::Release.valid_load());
    assert!(!MemoryOrder::AcqRel.valid_load());
    assert!(MemoryOrder::SeqCst.valid_load());
    assert!(MemoryOrder::Relaxed.valid_store());
    assert!(!MemoryOrder::Acquire.valid_store());
    assert!(MemoryOrder::Release.valid_store());
    assert!(!MemoryOrder::AcqRel.valid_store());
    assert!(MemoryOrder::SeqCst.valid_store());
    assert!(MemoryOrder::Relaxed.valid_failure());
    assert!(MemoryOrder::Acquire.valid_failure());
    assert!(!MemoryOrder::Release.valid_failure());
    assert!(!MemoryOrder::AcqRel.valid_failure());
    assert!(MemoryOrder::SeqCst.valid_failure());

    for success in MemoryOrder::ALL {
        for failure in MemoryOrder::ALL {
            let expected = match success {
                MemoryOrder::Relaxed => failure == MemoryOrder::Relaxed,
                MemoryOrder::Acquire | MemoryOrder::AcqRel => {
                    matches!(failure, MemoryOrder::Relaxed | MemoryOrder::Acquire)
                }
                MemoryOrder::Release => failure == MemoryOrder::Relaxed,
                MemoryOrder::SeqCst => {
                    matches!(
                        failure,
                        MemoryOrder::Relaxed | MemoryOrder::Acquire | MemoryOrder::SeqCst
                    )
                }
            };
            assert_eq!(
                MemoryOrder::valid_compare_exchange(success, failure),
                expected
            );
        }
    }
    assert!(MemoryOrder::synchronizes_with(
        MemoryOrder::Release,
        MemoryOrder::Acquire
    ));
    assert!(!MemoryOrder::synchronizes_with(
        MemoryOrder::Relaxed,
        MemoryOrder::Acquire
    ));
    assert!(publication_outcome_allowed(
        MemoryOrder::Release,
        MemoryOrder::Acquire,
        PublicationOutcome::Published
    ));
    assert!(!publication_outcome_allowed(
        MemoryOrder::Release,
        MemoryOrder::Acquire,
        PublicationOutcome::StaleAfterFlag
    ));
    assert!(publication_outcome_allowed(
        MemoryOrder::Relaxed,
        MemoryOrder::Relaxed,
        PublicationOutcome::StaleAfterFlag
    ));
    assert!(MemoryOrder::synchronizes_with(
        MemoryOrder::AcqRel,
        MemoryOrder::SeqCst
    ));
    assert!(MemoryOrder::synchronizes_with(
        MemoryOrder::SeqCst,
        MemoryOrder::SeqCst
    ));
}

#[test]
fn sync_models_cover_public_accessors_and_negative_transitions() {
    let mut mutex = MutexModel::default();
    assert_eq!(mutex.wakeups(), 0);
    assert_eq!(mutex.try_lock(1), Ok(true));
    assert_eq!(mutex.try_lock(2), Ok(false));
    assert_eq!(mutex.lock(2), Ok(LockPoll::Waiting));
    assert_eq!(mutex.lock(2), Err(SyncModelError::AlreadyHeld));
    assert_eq!(mutex.unlock(99), Err(SyncModelError::NotOwner));
    assert_eq!(mutex.unlock(1), Ok(Some(2)));
    assert_eq!(mutex.wakeups(), 1);
    assert_eq!(mutex.unlock(2), Ok(None));
    mutex.cleanup_owner().unwrap();

    let mut condition = ConditionModel::default();
    assert_eq!(condition.wakeups(), 0);
    assert_eq!(condition.wait(1), Err(SyncModelError::NotOwner));
    condition.lock_for(1).unwrap();
    assert_eq!(condition.lock_for(2), Err(SyncModelError::AlreadyHeld));
    condition.unlock(1).unwrap();
    condition.lock_for(1).unwrap();
    condition.wait(1).unwrap();
    assert_eq!(condition.lock_for(1), Err(SyncModelError::AlreadyHeld));
    condition.lock_for(2).unwrap();
    condition.wait(2).unwrap();
    assert_eq!(condition.notify_all(), 2);
    assert_eq!(condition.wakeups(), 2);
    assert_eq!(condition.notify_one(), None);
    condition.lock_for(3).unwrap();
    assert_eq!(condition.reacquire(1), Ok(ConditionPoll::Waiting));
    condition.unlock(3).unwrap();
    assert_eq!(condition.reacquire(1), Ok(ConditionPoll::Reacquired));
    assert_eq!(condition.reacquire(2), Ok(ConditionPoll::Waiting));
    condition.unlock(1).unwrap();
    assert_eq!(condition.reacquire(2), Ok(ConditionPoll::Reacquired));
    condition.unlock(2).unwrap();
    assert_eq!(condition.unlock(99), Err(SyncModelError::NotOwner));
    assert_eq!(condition.spurious_wake(99), Err(SyncModelError::NotWaiting));

    let mut semaphore = SemaphoreModel::new(1).unwrap();
    let token = match semaphore.acquire(1).unwrap() {
        SemaphorePoll::Acquired(token) => token,
        SemaphorePoll::Waiting => panic!("first permit must be immediate"),
    };
    assert_eq!(semaphore.acquire(2), Ok(SemaphorePoll::Waiting));
    assert_eq!(semaphore.acquire(2), Err(SyncModelError::AlreadyHeld));
    assert_eq!(
        semaphore.release(PermitToken {
            id: token.id,
            owner: 99,
        }),
        Err(SyncModelError::NotOwner)
    );
    semaphore.cancel_wait(2).unwrap();
    semaphore.cleanup_all().unwrap();
    assert_eq!(semaphore.permits(), semaphore.capacity());

    let mut once = OnceModel::<i32, ()>::default();
    assert_eq!(once.waiting(), 0);
    assert_eq!(once.wakeups(), 0);
    assert_eq!(once.get(), None);
    assert_eq!(once.start(1), OnceStart::Initializer);
    assert_eq!(once.start(2), OnceStart::Waiting);
    assert_eq!(once.start(2), OnceStart::Waiting);
    assert_eq!(once.waiting(), 1);
    assert_eq!(
        once.finish(99, OnceResolution::Success(1)),
        Err(SyncModelError::NotOwner)
    );
    assert_eq!(once.finish(1, OnceResolution::Error(())), Ok(vec![2]));
    assert_eq!(once.wakeups(), 1);
    assert_eq!(once.start(2), OnceStart::Initializer);
    once.finish(2, OnceResolution::Success(7)).unwrap();
    assert_eq!(once.get(), Some(7));
    assert_eq!(
        once.finish(2, OnceResolution::Success(8)),
        Err(SyncModelError::NotOwner)
    );

    assert_eq!(BarrierModel::new(65), Err(SyncModelError::InvalidParties));
    let mut barrier = BarrierModel::new(2).unwrap();
    assert_eq!(barrier.parties(), 2);
    assert_eq!(barrier.waiting(), 0);
    assert_eq!(
        barrier.arrive(1),
        Ok(BarrierPoll::Waiting { generation: 0 })
    );
    assert_eq!(barrier.waiting(), 1);
    assert_eq!(barrier.cancel(99), Err(SyncModelError::NotWaiting));

    for argument in 1..=3 {
        let mut input = vec![6, 1, 7, 0];
        input[3] = argument;
        run_fuzz_case(&input).unwrap();
    }
    run_fuzz_case(&[8, 9, 10, 1, 8]).unwrap();
    run_fuzz_case(&[]).unwrap();
}

#[test]
fn mutex_fifo_reentrancy_cancellation_and_exact_cleanup() {
    let mut mutex = MutexModel::new();
    assert_eq!(mutex.try_lock(1), Ok(true));
    assert_eq!(mutex.lock(1), Ok(LockPoll::Reentrant));
    assert_eq!(mutex.try_lock(1), Err(SyncModelError::Reentrant));
    assert_eq!(mutex.lock(2), Ok(LockPoll::Waiting));
    assert_eq!(mutex.lock(3), Ok(LockPoll::Waiting));
    assert_eq!(mutex.waiting(), vec![2, 3]);
    mutex.cancel_wait(2).unwrap();
    assert_eq!(mutex.waiting(), vec![3]);
    assert_eq!(mutex.unlock(1), Ok(Some(3)));
    assert_eq!(mutex.owner(), Some(3));
    assert_eq!(mutex.unlock(3), Ok(None));
    assert_eq!(mutex.cleanup_runs(1), 1);
    assert_eq!(mutex.cleanup_runs(3), 1);
    assert_eq!(mutex.unlock(3), Err(SyncModelError::NotOwner));
    assert!(matches!(
        mutex.cancel_wait(2),
        Err(SyncModelError::NotWaiting)
    ));
    assert!(mutex.try_lock(4).unwrap());
    mutex.cleanup_owner().unwrap();
    mutex.assert_invariants().unwrap();
}

#[test]
fn condition_release_register_notify_reacquire_and_cancel_are_atomic() {
    let mut condition = ConditionModel::new();
    condition.lock_for(1).unwrap();
    assert_eq!(condition.wait(1), Ok(ConditionPoll::Waiting));
    assert_eq!(condition.guard_owner(), None);
    assert_eq!(condition.waiting(), 1);
    assert_eq!(condition.spurious_wake(1), Ok(()));
    assert_eq!(condition.reacquire(1), Ok(ConditionPoll::Waiting));
    assert_eq!(condition.notify_one(), Some(1));
    assert_eq!(condition.reacquire(1), Ok(ConditionPoll::Reacquired));
    assert_eq!(condition.guard_owner(), Some(1));
    condition.unlock(1).unwrap();

    condition.lock_for(2).unwrap();
    condition.wait(2).unwrap();
    condition.lock_for(3).unwrap();
    assert_eq!(condition.cancel_wait(2), Ok(ConditionPoll::Waiting));
    condition.unlock(3).unwrap();
    assert_eq!(condition.cancel_wait(2), Ok(ConditionPoll::Cancelled));
    assert_eq!(condition.guard_owner(), Some(2));
    condition.unlock(2).unwrap();
    assert_eq!(condition.notify_one(), None);
    assert_eq!(condition.notify_all(), 0);
    condition.assert_invariants().unwrap();
}

#[test]
fn semaphore_fifo_handoff_capacity_and_double_release_are_explicit() {
    assert_eq!(SemaphoreModel::new(0), Err(SyncModelError::InvalidCapacity));
    assert_eq!(
        SemaphoreModel::new(65),
        Err(SyncModelError::InvalidCapacity)
    );
    let mut semaphore = SemaphoreModel::new(2).unwrap();
    let first = match semaphore.acquire(1).unwrap() {
        SemaphorePoll::Acquired(token) => token,
        SemaphorePoll::Waiting => panic!("first permit must be immediate"),
    };
    let second = match semaphore.try_acquire(2).unwrap() {
        Some(token) => token,
        None => panic!("second permit must be immediate"),
    };
    assert_eq!(semaphore.try_acquire(3), Ok(None));
    assert_eq!(semaphore.acquire(3), Ok(SemaphorePoll::Waiting));
    assert_eq!(semaphore.acquire(4), Ok(SemaphorePoll::Waiting));
    let replacement = semaphore.release(first).unwrap().expect("FIFO handoff");
    assert_eq!(replacement.owner, 3);
    assert_eq!(
        semaphore.release(second).unwrap().map(|token| token.owner),
        Some(4)
    );
    assert_eq!(semaphore.waiting(), 0);
    assert_eq!(
        semaphore.release(first),
        Err(SyncModelError::AlreadyReleased)
    );
    assert_eq!(semaphore.release(replacement).unwrap(), None);
    let last = semaphore.active_tokens().pop().expect("task 4 permit");
    semaphore.release(last).unwrap();
    assert_eq!(semaphore.permits(), semaphore.capacity());
    assert_eq!(semaphore.cleanup_runs(), 4);
    semaphore.cleanup_all().unwrap();
    semaphore.assert_invariants().unwrap();
}

#[test]
fn once_publishes_once_retries_failures_and_wakes_waiters() {
    let mut once = OnceModel::<i64, &'static str>::new();
    assert_eq!(once.start(1), OnceStart::Initializer);
    assert_eq!(once.start(1), OnceStart::Reentrant);
    assert_eq!(once.start(2), OnceStart::Waiting);
    assert_eq!(once.start(3), OnceStart::Waiting);
    assert_eq!(once.finish(1, OnceResolution::Error("bad")), Ok(vec![2, 3]));
    assert!(!once.is_ready());
    assert_eq!(once.cleanup_runs(), 1);
    assert_eq!(once.start(2), OnceStart::Initializer);
    assert_eq!(once.finish(2, OnceResolution::Success(41)), Ok(Vec::new()));
    assert_eq!(once.get(), Some(41));
    assert!(once.is_ready());
    assert_eq!(once.start(3), OnceStart::Ready(41));
    assert_eq!(once.start(4), OnceStart::Ready(41));
    once.assert_invariants().unwrap();

    let mut cancelled = OnceModel::<i64, &'static str>::new();
    assert_eq!(cancelled.start(7), OnceStart::Initializer);
    assert_eq!(cancelled.start(8), OnceStart::Waiting);
    assert_eq!(cancelled.cancel_initializer(7), Ok(vec![8]));
    assert!(!cancelled.is_ready());
    assert_eq!(cancelled.cleanup_runs(), 1);
    assert_eq!(cancelled.start(9), OnceStart::Initializer);
    assert_eq!(cancelled.finish(9, OnceResolution::Panic), Ok(Vec::new()));
    cancelled.assert_invariants().unwrap();
}

#[test]
fn barrier_generations_have_one_leader_and_break_on_cancellation() {
    assert_eq!(BarrierModel::new(0), Err(SyncModelError::InvalidParties));
    let mut barrier = BarrierModel::new(3).unwrap();
    assert_eq!(
        barrier.arrive(1),
        Ok(BarrierPoll::Waiting { generation: 0 })
    );
    assert_eq!(
        barrier.arrive(2),
        Ok(BarrierPoll::Waiting { generation: 0 })
    );
    assert_eq!(barrier.arrive(2), Err(SyncModelError::AlreadyHeld));
    assert_eq!(
        barrier.arrive(3),
        Ok(BarrierPoll::Complete {
            generation: 0,
            role: BarrierRole::Leader
        })
    );
    let released = barrier.take_released();
    assert_eq!(released.len(), 3);
    assert!(released.iter().any(|(_, poll)| matches!(
        poll,
        BarrierPoll::Complete {
            role: BarrierRole::Leader,
            ..
        }
    )));
    assert_eq!(barrier.generation(), 1);
    barrier.assert_invariants().unwrap();

    assert_eq!(
        barrier.arrive(4),
        Ok(BarrierPoll::Waiting { generation: 1 })
    );
    assert_eq!(barrier.cancel(4), Ok(1));
    assert!(barrier.is_broken());
    assert_eq!(barrier.take_released(), vec![(4, BarrierPoll::Broken)]);
    assert_eq!(
        barrier.arrive(5),
        Ok(BarrierPoll::Waiting { generation: 3 })
    );
    assert_eq!(
        barrier.arrive(6),
        Ok(BarrierPoll::Waiting { generation: 3 })
    );
    assert_eq!(
        barrier.arrive(7),
        Ok(BarrierPoll::Complete {
            generation: 3,
            role: BarrierRole::Leader
        })
    );
    barrier.assert_invariants().unwrap();
}

#[test]
fn adversarial_fuzz_is_bounded_replayable_and_leak_free() {
    for seed in 0..4_096_u64 {
        let bytes = seed.to_le_bytes();
        let first = run_fuzz_case(&bytes).unwrap();
        let second = run_fuzz_case(&bytes).unwrap();
        assert_eq!(first, second, "sync replay diverged for seed {seed}");
        assert!(first.steps <= MAX_FUZZ_STEPS);
        assert_eq!(first.pending_waiters, 0);
    }
}

#[test]
fn runtime_once_fixture_exercises_vm_continuation_and_memoization() {
    let root = workspace_root(&std::env::current_dir().unwrap()).unwrap();
    let source = fs::read_to_string(root.join("tests/runtime/m11-std-sync-test-001.to")).unwrap();
    let observation = run("std-sync-test-001", &source, ResourceLimits::default()).unwrap();
    assert!(observation.accepted, "{:?}", observation.diagnostic_codes);
    assert_eq!(observation.exit_code, 0);
    assert_eq!(
        decode_hex(&observation.stdout_hex).unwrap(),
        b"sync-test-ok\n"
    );
}
