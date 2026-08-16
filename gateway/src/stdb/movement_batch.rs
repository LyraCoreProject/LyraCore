use std::sync::Mutex;

use super::bindings::GwMove;

const MAX_MOVES_PER_CALL: usize = 128;

/// One shard's queued steady heartbeats. The scheduler decides when to drain it;
/// this type owns the deterministic queue and bounded submission behavior.
pub(crate) struct MovementBatch {
    queued: Mutex<Vec<GwMove>>,
}

pub(crate) struct SubmissionFailure<E> {
    pub(crate) dropped: usize,
    pub(crate) error: E,
}

impl MovementBatch {
    pub(crate) fn new() -> Self {
        Self {
            queued: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn push(&self, movement: GwMove) {
        self.queued
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(movement);
    }

    /// Removes the current window before submitting it, so movements queued by a
    /// submission callback belong to the next drain.
    pub(crate) fn drain<E>(
        &self,
        mut submit: impl FnMut(Vec<GwMove>) -> Result<(), E>,
    ) -> Vec<SubmissionFailure<E>> {
        let movements = {
            let mut queued = self
                .queued
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut *queued)
        };

        let mut failures = Vec::new();
        for chunk in movements.chunks(MAX_MOVES_PER_CALL) {
            let dropped = chunk.len();
            if let Err(error) = submit(chunk.to_vec()) {
                failures.push(SubmissionFailure { dropped, error });
            }
        }
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movement(actor_guid: u64) -> GwMove {
        GwMove {
            actor_guid,
            opcode: 0x00ee,
            movement_info: vec![actor_guid as u8],
            x: actor_guid as f32,
            y: 2.0,
            z: 3.0,
            o: 4.0,
            move_time_ms: actor_guid as u32,
        }
    }

    fn queued(count: u64) -> MovementBatch {
        let batch = MovementBatch::new();
        for actor_guid in 1..=count {
            batch.push(movement(actor_guid));
        }
        batch
    }

    #[test]
    fn empty_drain_does_not_submit() {
        let batch = MovementBatch::new();
        let mut calls = 0;

        let failures = batch.drain::<()>(|_| {
            calls += 1;
            Ok(())
        });

        assert_eq!(calls, 0);
        assert!(failures.is_empty());
    }

    #[test]
    fn one_window_is_submitted_in_fifo_order() {
        for count in [1, 128] {
            let batch = queued(count);
            let mut calls = Vec::new();

            batch.drain::<()>(|moves| {
                calls.push(moves);
                Ok(())
            });

            assert_eq!(calls.len(), 1);
            assert_eq!(
                calls[0].iter().map(|m| m.actor_guid).collect::<Vec<_>>(),
                (1..=count).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn drain_splits_only_at_the_call_bound() {
        for (count, expected_sizes) in [(129, vec![128, 1]), (256, vec![128, 128])] {
            let batch = queued(count);
            let mut calls = Vec::new();

            batch.drain::<()>(|moves| {
                calls.push(moves);
                Ok(())
            });

            assert_eq!(
                calls.iter().map(Vec::len).collect::<Vec<_>>(),
                expected_sizes
            );
            assert_eq!(
                calls
                    .into_iter()
                    .flatten()
                    .map(|m| m.actor_guid)
                    .collect::<Vec<_>>(),
                (1..=count).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn movements_queued_during_a_drain_wait_for_the_next_drain() {
        let batch = queued(1);
        let mut first = Vec::new();
        batch.drain::<()>(|moves| {
            first.push(moves);
            batch.push(movement(2));
            Ok(())
        });

        let mut second = Vec::new();
        batch.drain::<()>(|moves| {
            second.push(moves);
            Ok(())
        });

        assert_eq!(first[0][0].actor_guid, 1);
        assert_eq!(second[0][0].actor_guid, 2);
    }

    #[test]
    fn a_failed_chunk_is_counted_and_does_not_stop_later_chunks() {
        let batch = queued(257);
        let mut call_sizes = Vec::new();

        let failures = batch.drain(|moves| {
            call_sizes.push(moves.len());
            if call_sizes.len() == 1 {
                Err("transport unavailable")
            } else {
                Ok(())
            }
        });

        assert_eq!(call_sizes, [128, 128, 1]);
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].dropped, 128);
        assert_eq!(failures[0].error, "transport unavailable");
    }
}
