use tondo_reliability::channel_model::{
    ChannelModel, ChannelModelError, MAX_CHANNEL_BUFFER, MAX_CHANNEL_FUZZ_STEPS,
    MAX_UNBOUNDED_QUEUE, ReceiveOutcome, SelectArm, SelectResult, SendOutcome, run_fuzz_case,
};

const _: () = assert!(MAX_UNBOUNDED_QUEUE <= MAX_CHANNEL_FUZZ_STEPS);

fn issue(model: &mut ChannelModel, value: u64) -> tondo_reliability::channel_model::Payload {
    model.issue(value).unwrap()
}

#[test]
fn constructors_and_endpoint_limits_are_explicit() {
    assert_eq!(
        ChannelModel::bounded(-1),
        Err(ChannelModelError::InvalidCapacity)
    );
    assert_eq!(
        ChannelModel::bounded((MAX_CHANNEL_BUFFER + 1) as i64),
        Err(ChannelModelError::Limit)
    );

    let mut model = ChannelModel::bounded(1).unwrap();
    for _ in 0..(128 / 2) {
        model.sender().unwrap();
        model.receiver().unwrap();
    }
    assert!(matches!(
        model.sender(),
        Err(ChannelModelError::ClosedEndpoint)
    ));
    model.assert_invariants().unwrap();
}

#[test]
fn multiple_producers_and_consumers_preserve_registration_fifo() {
    let mut model = ChannelModel::bounded(0).unwrap();
    let first_sender = model.sender().unwrap();
    let second_sender = model.fork_sender(first_sender).unwrap();
    let first_receiver = model.receiver().unwrap();
    let second_receiver = model.fork_receiver(first_receiver).unwrap();
    let first_payload = issue(&mut model, 10);
    let second_payload = issue(&mut model, 20);
    let first_send = model.register_send(first_sender, first_payload).unwrap();
    let second_send = model.register_send(second_sender, second_payload).unwrap();
    let first_receive = model.register_receive(first_receiver).unwrap();
    let second_receive = model.register_receive(second_receiver).unwrap();

    assert_eq!(model.progress().unwrap(), 2);
    assert_eq!(
        model.poll_receive(first_receive).unwrap(),
        ReceiveOutcome::Item(first_payload)
    );
    assert_eq!(
        model.poll_receive(second_receive).unwrap(),
        ReceiveOutcome::Item(second_payload)
    );
    assert_eq!(model.poll_send(first_send).unwrap(), SendOutcome::Committed);
    assert_eq!(
        model.poll_send(second_send).unwrap(),
        SendOutcome::Committed
    );
    assert_eq!(model.snapshot().wakeups, 4);
    model.cleanup().unwrap();
}

#[test]
fn select_ready_ties_rotate_and_else_keeps_state_unchanged() {
    let mut model = ChannelModel::bounded(2).unwrap();
    let sender = model.sender().unwrap();
    let first_receiver = model.receiver().unwrap();
    let second_receiver = model.fork_receiver(first_receiver).unwrap();
    let first = issue(&mut model, 1);
    let second = issue(&mut model, 2);
    assert_eq!(
        model.try_send(sender, first).unwrap(),
        SendOutcome::Committed
    );
    assert_eq!(
        model.try_send(sender, second).unwrap(),
        SendOutcome::Committed
    );

    let first_probe = model
        .prepare_select(
            &[
                SelectArm::Receive {
                    receiver: first_receiver,
                },
                SelectArm::Receive {
                    receiver: second_receiver,
                },
            ],
            false,
        )
        .unwrap();
    let before = model.snapshot();
    assert!(matches!(
        model.commit_select(first_probe).unwrap(),
        SelectResult::Receive(ReceiveOutcome::Item(_))
    ));
    let next = model
        .prepare_select(
            &[
                SelectArm::Receive {
                    receiver: first_receiver,
                },
                SelectArm::Receive {
                    receiver: second_receiver,
                },
            ],
            false,
        )
        .unwrap();
    assert!(matches!(
        model.commit_select(next).unwrap(),
        SelectResult::Receive(ReceiveOutcome::Item(_))
    ));
    assert_eq!(model.snapshot().committed_receives, 2);

    let empty = model
        .prepare_select(
            &[SelectArm::Receive {
                receiver: first_receiver,
            }],
            true,
        )
        .unwrap();
    let unchanged = model.snapshot();
    assert_eq!(model.commit_select(empty).unwrap(), SelectResult::Else);
    assert_eq!(model.snapshot(), unchanged);
    assert!(!before.queue.is_empty());
    model.cleanup().unwrap();
}

#[test]
fn cancellation_and_close_never_duplicate_or_drop_affine_payloads() {
    let mut model = ChannelModel::bounded(0).unwrap();
    let sender = model.sender().unwrap();
    let receiver = model.receiver().unwrap();
    let pending = issue(&mut model, 31);
    let waiter = model.register_send(sender, pending).unwrap();
    assert_eq!(
        model.cancel_send(waiter).unwrap(),
        SendOutcome::Cancelled(pending)
    );
    assert!(matches!(
        model.cancel_send(waiter),
        Err(ChannelModelError::UnknownWaiter)
    ));

    let receive_waiter = model.register_receive(receiver).unwrap();
    assert_eq!(
        model.cancel_receive(receive_waiter).unwrap(),
        ReceiveOutcome::Cancelled
    );
    assert!(matches!(
        model.cancel_receive(receive_waiter),
        Err(ChannelModelError::UnknownWaiter)
    ));
    assert_eq!(model.snapshot().wakeups, 2);
    model.cleanup().unwrap();
}

#[test]
fn last_receiver_drains_fifo_and_sender_close_ends_receive() {
    let mut model = ChannelModel::bounded(4).unwrap();
    let sender = model.sender().unwrap();
    let receiver = model.receiver().unwrap();
    let first = issue(&mut model, 41);
    let second = issue(&mut model, 42);
    assert_eq!(
        model.try_send(sender, first).unwrap(),
        SendOutcome::Committed
    );
    assert_eq!(
        model.try_send(sender, second).unwrap(),
        SendOutcome::Committed
    );
    model.close_sender(sender).unwrap();
    assert_eq!(
        model.try_receive(receiver).unwrap(),
        ReceiveOutcome::Item(first)
    );
    assert_eq!(
        model.try_receive(receiver).unwrap(),
        ReceiveOutcome::Item(second)
    );
    assert_eq!(model.try_receive(receiver).unwrap(), ReceiveOutcome::Closed);
    assert!(model.close_receiver(receiver).unwrap().is_empty());
    model.assert_invariants().unwrap();
}

#[test]
fn bounded_fuzz_replays_all_seed_cases_with_structured_cleanup() {
    for seed in 0..4_096_u64 {
        let bytes = seed.to_le_bytes();
        let first = run_fuzz_case(&bytes).unwrap();
        let second = run_fuzz_case(&bytes).unwrap();
        assert_eq!(first, second);
        assert!(first.steps <= MAX_CHANNEL_FUZZ_STEPS);
        assert_eq!(first.snapshot.sender_count, 0);
        assert_eq!(first.snapshot.receiver_count, 0);
        assert!(first.snapshot.queue.is_empty());
        assert!(first.snapshot.send_waiters.is_empty());
        assert!(first.snapshot.receive_waiters.is_empty());
        assert!(first.snapshot.send_results.is_empty());
        assert!(first.snapshot.receive_results.is_empty());
    }
}
