//! Narrow Duel accept/cancel dispatcher. All client output returns on the Duel event relay.

use super::super::*;

pub(crate) trait DuelActionStore: Send + Sync {
    fn duel_accept(&self, actor: Actor, flag_guid: u64) -> Result<()>;
    fn duel_cancel(&self, actor: Actor, flag_guid: u64) -> Result<()>;
}

#[derive(Clone, Copy)]
pub(crate) struct DuelActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum DuelActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

pub(crate) fn dispatch_duel_action<St: DuelActionStore + ?Sized>(
    store: &St,
    player: DuelActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<DuelActionOutcome> {
    let (accept, flag_guid) = match msg {
        ClientOpcodeMessage::CMSG_DUEL_ACCEPTED(request) => (true, request.guid.guid()),
        ClientOpcodeMessage::CMSG_DUEL_CANCELLED(request) => (false, request.guid.guid()),
        other => return Ok(DuelActionOutcome::PassThrough(other)),
    };
    let Some(actor) = player.self_guid.and_then(Actor::new) else {
        return Ok(DuelActionOutcome::Handled {
            outbound: Vec::new(),
        });
    };
    let result = if accept {
        store.duel_accept(actor, flag_guid)
    } else {
        store.duel_cancel(actor, flag_guid)
    };
    if let Err(error) = result {
        if matches!(
            crate::stdb::classify(&error),
            crate::stdb::DurableFailure::TransportLoss
        ) {
            return Err(error);
        }
        log::debug!(
            "world: duel action ignored (account {}): {error}",
            player.account_id
        );
    }
    Ok(DuelActionOutcome::Handled {
        outbound: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{CMSG_DUEL_ACCEPTED, CMSG_DUEL_CANCELLED, CMSG_PING};
    use wow_world_messages::Guid;

    use crate::stdb::ReducerCallError;

    #[derive(Default)]
    struct InMemoryDuelStore {
        calls: Mutex<Vec<(&'static str, u64, u64)>>,
        failure: Mutex<Option<ReducerCallError>>,
    }

    impl InMemoryDuelStore {
        fn failing(failure: ReducerCallError) -> Self {
            Self {
                failure: Mutex::new(Some(failure)),
                ..Self::default()
            }
        }

        fn record(&self, call: &'static str, actor: Actor, flag_guid: u64) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((call, actor.guid(), flag_guid));
            match self.failure.lock().unwrap().take() {
                Some(failure) => Err(failure.into()),
                None => Ok(()),
            }
        }
    }

    impl DuelActionStore for InMemoryDuelStore {
        fn duel_accept(&self, actor: Actor, flag_guid: u64) -> Result<()> {
            self.record("accept", actor, flag_guid)
        }

        fn duel_cancel(&self, actor: Actor, flag_guid: u64) -> Result<()> {
            self.record("cancel", actor, flag_guid)
        }
    }

    fn player() -> DuelActionPlayer {
        DuelActionPlayer {
            account_id: 7,
            self_guid: Some(42),
        }
    }

    #[test]
    fn accept_and_cancel_forward_the_wire_arbiter_as_reducer_intents() {
        let store = InMemoryDuelStore::default();
        let accepted = dispatch_duel_action(
            &store,
            player(),
            ClientOpcodeMessage::CMSG_DUEL_ACCEPTED(CMSG_DUEL_ACCEPTED {
                guid: Guid::new(99),
            }),
        )
        .unwrap();
        let cancelled = dispatch_duel_action(
            &store,
            player(),
            ClientOpcodeMessage::CMSG_DUEL_CANCELLED(CMSG_DUEL_CANCELLED {
                guid: Guid::new(100),
            }),
        )
        .unwrap();
        assert!(matches!(accepted, DuelActionOutcome::Handled { outbound } if outbound.is_empty()));
        assert!(
            matches!(cancelled, DuelActionOutcome::Handled { outbound } if outbound.is_empty())
        );
        assert_eq!(
            store.calls.lock().unwrap().as_slice(),
            &[("accept", 42, 99), ("cancel", 42, 100)]
        );
    }

    #[test]
    fn no_in_world_actor_is_consumed_without_a_forged_reducer_identity() {
        let store = InMemoryDuelStore::default();
        let outcome = dispatch_duel_action(
            &store,
            DuelActionPlayer {
                account_id: 7,
                self_guid: None,
            },
            ClientOpcodeMessage::CMSG_DUEL_ACCEPTED(CMSG_DUEL_ACCEPTED {
                guid: Guid::new(99),
            }),
        )
        .unwrap();
        assert!(matches!(outcome, DuelActionOutcome::Handled { .. }));
        assert!(store.calls.lock().unwrap().is_empty());
    }

    fn accept_message() -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_DUEL_ACCEPTED(CMSG_DUEL_ACCEPTED {
            guid: Guid::new(99),
        })
    }

    #[test]
    fn transport_loss_ends_the_world_session() {
        let store = InMemoryDuelStore::failing(ReducerCallError::transport_lost("gw_duel_accept"));
        assert!(dispatch_duel_action(&store, player(), accept_message()).is_err());
    }

    #[test]
    fn refusal_is_ignored_and_the_session_continues() {
        let store = InMemoryDuelStore::failing(ReducerCallError::refused(
            "gw_duel_accept",
            "duel_not_pending",
        ));
        let outcome = dispatch_duel_action(&store, player(), accept_message()).unwrap();
        assert!(matches!(outcome, DuelActionOutcome::Handled { outbound } if outbound.is_empty()));
    }

    #[test]
    fn unrelated_opcode_passes_through() {
        let store = InMemoryDuelStore::default();
        let outcome = dispatch_duel_action(
            &store,
            player(),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            DuelActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }
}
