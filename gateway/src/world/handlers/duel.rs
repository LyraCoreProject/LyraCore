//! Narrow Duel accept/cancel dispatcher. All client output returns on the Duel event relay.

use super::super::*;

pub(crate) trait DuelActionStore: Send + Sync {
    fn duel_accept(&self, account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()>;
    fn duel_cancel(&self, account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()>;
}

impl DuelActionStore for crate::stdb::Coordinator {
    fn duel_accept(&self, account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::duel_accept(self, account_id, actor_guid, flag_guid)
    }

    fn duel_cancel(&self, account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::duel_cancel(self, account_id, actor_guid, flag_guid)
    }
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
    let Some(actor_guid) = player.self_guid else {
        return Ok(DuelActionOutcome::Handled {
            outbound: Vec::new(),
        });
    };
    let result = if accept {
        store.duel_accept(player.account_id, actor_guid, flag_guid)
    } else {
        store.duel_cancel(player.account_id, actor_guid, flag_guid)
    };
    if let Err(error) = result {
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

    #[derive(Default)]
    struct InMemoryDuelStore {
        calls: Mutex<Vec<(&'static str, u64, u64)>>,
    }

    impl DuelActionStore for InMemoryDuelStore {
        fn duel_accept(&self, _account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(("accept", actor_guid, flag_guid));
            Ok(())
        }

        fn duel_cancel(&self, _account_id: u64, actor_guid: u64, flag_guid: u64) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(("cancel", actor_guid, flag_guid));
            Ok(())
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
