//! Independent bounded networking models and deterministic stream replay.

#[path = "net_model/admission.rs"]
pub mod admission;
#[path = "net_model/stream.rs"]
pub mod stream;
#[path = "net_model/tls.rs"]
pub mod tls;

use admission::Limits;
use stream::{Completion, Snapshot, Stream};

pub const MAX_INPUT: usize = 4096;
pub const MAX_STEPS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub steps: usize,
    pub cancellations: usize,
    pub rollbacks: usize,
    pub reads: usize,
    pub outside_domain: usize,
    pub terminal: Snapshot,
}

pub fn replay(input: &[u8]) -> Replay {
    let input = &input[..input.len().min(MAX_INPUT)];
    let capacity = 1 + input.first().copied().unwrap_or(0) as usize % 128;
    let steps = (input.len() / 4).clamp(1, MAX_STEPS);
    let mut stream = Stream::new(capacity).unwrap();
    let limits = Limits::create(128, 128, 32).unwrap();
    let mut reservation = None;
    let mut result = Replay {
        steps,
        cancellations: 0,
        rollbacks: 0,
        reads: 0,
        outside_domain: 0,
        terminal: stream.snapshot(),
    };
    for step in 0..steps {
        let byte = |offset| input.get(step * 4 + offset).copied().unwrap_or(0);
        match byte(0) % 9 {
            0 => {
                let before = stream.snapshot();
                match stream.feed(&[byte(1), byte(2)]) {
                    Ok(_) => {}
                    Err(admission::Error::OutsideDomain) => {
                        result.outside_domain += 1;
                        assert_eq!(stream.snapshot(), before);
                    }
                    other => panic!("unexpected feed result: {other:?}"),
                }
            }
            1 if reservation.is_none() => {
                reservation = Some(
                    stream
                        .prepare(1 + byte(1) as i128 % 128, limits, None, 7, 0)
                        .unwrap(),
                );
            }
            2 if reservation.is_some() => {
                let before = stream.snapshot();
                let _ = stream.ready(reservation.unwrap()).unwrap();
                assert_eq!(stream.snapshot(), before);
            }
            3 if reservation.is_some() => {
                match stream.finish(reservation.unwrap(), true, false, 0).unwrap() {
                    Completion::Pending => {}
                    Completion::Committed(_) => {
                        reservation = None;
                        result.reads += 1;
                    }
                    Completion::RolledBack => unreachable!(),
                }
            }
            4 if reservation.is_some() => {
                assert_eq!(
                    stream.finish(reservation.take().unwrap(), false, false, 0),
                    Ok(Completion::RolledBack)
                );
                result.rollbacks += 1;
            }
            5 if reservation.is_some() => {
                let before = stream.snapshot();
                assert_eq!(
                    stream.finish(reservation.take().unwrap(), true, true, 0),
                    Err(admission::Error::Cancelled)
                );
                assert_eq!(stream.snapshot().queued, before.queued);
                result.cancellations += 1;
            }
            6 => {
                let before = stream.snapshot();
                match stream.write(&[byte(1), byte(2)], byte(3) as usize % 3) {
                    Ok(_) => {}
                    Err(admission::Error::OutsideDomain) => {
                        result.outside_domain += 1;
                        assert_eq!(stream.snapshot(), before);
                    }
                    other => panic!("unexpected write result: {other:?}"),
                }
            }
            7 => stream.peer_fin(),
            8 => stream.shutdown_writer().unwrap(),
            _ => {}
        }
        assert!(stream.consistent());
    }
    stream.close_scope();
    assert!(stream.consistent());
    result.terminal = stream.snapshot();
    assert!(!result.terminal.transport_live && !result.terminal.prepared);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_is_bounded_deterministic_and_ignores_the_external_tail() {
        for length in [0, 1, 3, 4, 9, 2048, 4096, 5000] {
            let input = (0..length)
                .map(|index| (index * 73) as u8)
                .collect::<Vec<_>>();
            let first = replay(&input);
            assert_eq!(first, replay(&input));
            assert!((1..=MAX_STEPS).contains(&first.steps));
            assert_eq!(first, replay(&input[..input.len().min(MAX_INPUT)]));
            assert_eq!(
                first.terminal.fed,
                first.terminal.observed + first.terminal.discarded
            );
            assert!(first.terminal.queued.is_empty());
        }
        for seed in 0..4096u64 {
            let mut cursor = seed;
            let input = (0..64)
                .map(|_| {
                    cursor = cursor.wrapping_mul(6364136223846793005).wrapping_add(1);
                    (cursor >> 32) as u8
                })
                .collect::<Vec<_>>();
            assert_eq!(replay(&input), replay(&input), "seed {seed}");
        }
    }
}
