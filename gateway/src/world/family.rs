//! Protocol Family handlers share session state and return replies to the World Session.

use super::*;

pub(crate) trait ProtocolFamily<St: ?Sized> {
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply>;
}

pub(crate) enum ProtocolRequest {
    Message(ClientOpcodeMessage),
    AuctionBrowse(AuctionBrowseRequest),
    FactionAtWar(Vec<u8>),
    WatchedFaction(Vec<u8>),
}

impl From<ClientOpcodeMessage> for ProtocolRequest {
    fn from(message: ClientOpcodeMessage) -> Self {
        Self::Message(message)
    }
}

impl ProtocolRequest {
    pub(crate) fn message(self) -> Result<ClientOpcodeMessage> {
        match self {
            Self::Message(message) => Ok(message),
            _ => Err(anyhow!("raw request routed to a typed Protocol Family")),
        }
    }
}

#[derive(Default)]
pub(crate) struct ProtocolReply {
    pub(crate) outbound: Vec<Outbound>,
    pub(crate) after_queue: Option<WorldSessionAction>,
}

impl From<Vec<Outbound>> for ProtocolReply {
    fn from(outbound: Vec<Outbound>) -> Self {
        Self {
            outbound,
            after_queue: None,
        }
    }
}

impl ProtocolReply {
    /// Queue all replies before running the next World Session operation on the reader thread.
    pub(super) fn complete(
        self,
        tx: &SessionTx,
        execute: impl FnOnce(WorldSessionAction) -> Result<()>,
    ) -> Result<()> {
        for message in self.outbound {
            send(tx, message)?;
        }
        match self.after_queue {
            Some(action) => execute(action),
            None => Ok(()),
        }
    }
}

pub(crate) enum WorldSessionAction {
    Login(Actor),
    WorldPortAck,
    Logout,
    ArmTaxi(Actor),
    RedriveMail(Actor),
}

/// Protocol state shared by handlers. Crypto, routing and Account Claims stay on WorldConn.
pub struct ProtocolSession {
    pub(crate) account_id: u64,
    pub(crate) account_name: String,
    pub(crate) state: WorldState,
    pub(crate) gossip_menu: Option<GossipMenuSnapshot>,
    pub(crate) who_throttled_until: Option<Instant>,
    pub(crate) group_broadcast_cooldowns: party::GroupBroadcastCooldowns,
}

impl ProtocolSession {
    pub(crate) fn new(account_id: u64, account_name: String) -> Self {
        Self {
            account_id,
            account_name,
            state: WorldState::CharSelect,
            gossip_menu: None,
            who_throttled_until: None,
            group_broadcast_cooldowns: Default::default(),
        }
    }

    pub(crate) fn actor(&self) -> Option<Actor> {
        self.self_guid().and_then(Actor::new)
    }

    pub(crate) fn self_guid(&self) -> Option<u64> {
        match &self.state {
            WorldState::CharSelect => None,
            WorldState::InWorld(world) => Some(world.self_guid),
        }
    }

    /// Admit one `CMSG_WHO`, or refuse it because the last one this session sent was inside
    /// [`WHO_THROTTLE`]. Advances the cooldown on every admitted request, including a malformed
    /// one that answers nothing — the request itself is what cost the scan.
    pub(super) fn admit_who(&mut self) -> bool {
        let now = Instant::now();
        if self.who_throttled_until.is_some_and(|until| now < until) {
            return false;
        }
        self.who_throttled_until = Some(now + WHO_THROTTLE);
        true
    }

    /// [`party::GroupBroadcastCooldowns::admit_at`] now.
    pub(super) fn admit_group_broadcast(&mut self, op: party::Op) -> bool {
        self.group_broadcast_cooldowns.admit_at(op, Instant::now())
    }

    #[cfg(test)]
    pub(crate) fn in_world(account_id: u64, self_guid: u64) -> Self {
        Self {
            state: WorldState::InWorld(InWorld {
                self_guid,
                subs: PlayerSubscriptions::empty(),
                attacking_target: None,
                open_loot: OpenLootState::default(),
                ranged_repeat: false,
            }),
            ..Self::new(account_id, "TESTER".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taxi_reply() -> ProtocolReply {
        ProtocolReply {
            outbound: vec![Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(
                codec::build_activate_taxi_reply(codec::TaxiActivationResult {
                    result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
                }),
            ))],
            after_queue: Some(WorldSessionAction::ArmTaxi(Actor::new(42).unwrap())),
        }
    }

    #[test]
    fn taxi_reply_is_queued_before_arming() {
        let (tx, rx) = SessionTx::with_depth(0);
        let mut armed = false;
        taxi_reply()
            .complete(&tx, |action| {
                assert!(matches!(action, WorldSessionAction::ArmTaxi(actor) if actor.guid() == 42));
                assert!(matches!(
                    rx.try_recv().unwrap(),
                    Outbound::One(ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(_))
                ));
                armed = true;
                Ok(())
            })
            .unwrap();
        assert!(armed);
    }

    #[test]
    fn failed_reply_queue_does_not_arm() {
        let (tx, rx) = SessionTx::with_depth(0);
        drop(rx);
        let mut armed = false;
        assert!(taxi_reply()
            .complete(&tx, |_| {
                armed = true;
                Ok(())
            })
            .is_err());
        assert!(!armed);
    }

    #[test]
    fn a_reply_without_a_following_operation_only_queues_messages() {
        let (tx, rx) = SessionTx::with_depth(0);
        let mut reply = taxi_reply();
        reply.after_queue = None;
        reply
            .complete(&tx, |_| panic!("a refused activation cannot arm a flight"))
            .unwrap();
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn arming_failure_propagates_after_the_reply() {
        let (tx, rx) = SessionTx::with_depth(0);
        let error = taxi_reply()
            .complete(&tx, |_| Err(anyhow!("flight could not be armed")))
            .unwrap_err();
        assert_eq!(error.to_string(), "flight could not be armed");
        assert!(rx.try_recv().is_ok());
    }
}
