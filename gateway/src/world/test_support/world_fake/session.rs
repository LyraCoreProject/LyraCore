use super::super::*;
use crate::stdb::ReducerCallError;

/// One recorded `movement_update`: (opcode, x, y, z, orientation, timestamp).
pub(crate) type MoveRecord = (u32, f32, f32, f32, f32, u32);

#[derive(Default)]
pub(crate) struct SessionState {
    pub(crate) movement_world:
        Option<std::sync::Arc<crate::world::tests::benilla_tests::MovementWorld>>,
    /// WORLDPORT_ACK gate: true = entity present -> a spurious ack is ignored;
    /// false (derive-Default) = absent -> a genuine transfer is pending.
    pub(crate) entity_in_world: bool,
    /// An in-session controllable cache answer for movement desync regression tests.
    pub(crate) entity_presence: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) entity_presence_checks: std::sync::atomic::AtomicUsize,
    pub(crate) username: String,
    pub(crate) session: Option<WorldSession>,
    pub(crate) login_entity: Option<codec::EntityView>,
    pub(crate) moves: std::sync::Mutex<Vec<MoveRecord>>,
    /// Recorded `client_command` calls: (actor guid, command, payload).
    pub(crate) client_commands: std::sync::Mutex<Vec<(u64, String, String)>>,
    /// When set, every `client_command` fails with the error this builds from the reducer name.
    pub(crate) client_command_error: Option<fn(&str) -> ReducerCallError>,
    /// Optional item row whose subscribed relay is queued during a successful turn-in. The paired
    /// sender is retained only for that test and consumed by `turn_in_quest`, so it cannot keep the
    /// session writer alive during teardown.
    pub(crate) turn_in_reward_item: Option<codec::ItemInstanceView>,
    pub(crate) turn_in_tx: std::sync::Mutex<Option<SessionTx>>,
    /// Override for `player_combat_until_ms`: 0 = out of combat (default), non-zero = in combat until
    /// this ms-epoch deadline (use u64::MAX for "always in combat" in tests).
    pub(crate) combat_until_ms: u64,
    /// Tracks whether `logout` was called (entity removal path taken).
    pub(crate) logout_called: std::sync::atomic::AtomicBool,
    /// When set, teardown cannot reach the database after a session-fatal Transport Loss.
    /// The world session must still close and relinquish its admission seat.
    pub(crate) logout_transport_lost: bool,
    /// The `WorldEntry` of every `player_login`, in order.
    pub(crate) login_entries: std::sync::Mutex<Vec<codec::WorldEntry>>,
    /// Parked private System Messages world entry replays (a Package `on_login` hook's output).
    pub(crate) pending_system_messages: Vec<String>,
    /// Recorded `player_login` call count — the WORLDPORT_ACK test distinguishes the
    /// initial `CMSG_PLAYER_LOGIN` call from a world-port RE-entry call, since both dispatch through
    /// this one trait method (`enter_world` is shared by both call sites).
    pub(crate) login_calls: std::sync::atomic::AtomicU32,
    /// When set, every `player_login` call AFTER the first returns THIS entity instead of
    /// `login_entity` — simulates the character row having moved to a new map (`teleport_player`'s
    /// durable write) between the initial login and the `MSG_MOVE_WORLDPORT_ACK`. `None` (default):
    /// every call keeps returning `login_entity`, byte-identical to before this field existed.
    pub(crate) worldport_entity: Option<codec::EntityView>,
    /// When set, every `player_login` AFTER the first FAILS with this message — the world entry
    /// behind a world-port that routed fine (the stranding guard, a refused re-login on the
    /// destination shard). The client is mid-loading-screen for it either way.
    pub(crate) worldport_login_error: Option<String>,
    /// Recorded `subscribe_player_events` calls: (self_guid, login_map, login_x, login_y) — the
    /// WORLDPORT_ACK test asserts this fires AGAIN (a fresh `created` dedup set) at the new
    /// map/position rather than reusing the old subscription.
    pub(crate) subscribed: std::sync::Mutex<Vec<(u64, u32, f32, f32)>>,
    /// The egress DEPTH counter of the live session `subscribe_player_events` was handed — so a test
    /// can read the real queue depth of a real `run_world_session` (the writer thread's decrement has
    /// no other reachable seam: it lives inside the spawned writer loop). Deliberately the depth
    /// `Arc` and NOT the `SessionTx` itself: holding a sender clone here would keep the writer's
    /// `rx.recv()` alive forever and hang every `enter_world` test's `server.join()`.
    pub(crate) session_depth:
        std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>>,
    /// Guids with a LIVE entity on this shard — the per-guid `entity_in_world` answer a
    /// realm-wide party frame's online flags are built from. Empty = the single `entity_in_world`
    /// flag above decides, as it did before.
    pub(crate) live_guids: Vec<u64>,
    /// When set, every `movement_update` fails with a Transport Loss.
    pub(crate) movement_transport_lost: bool,
    /// Optional real shared view used by world-port ordering tests.
    pub(crate) relay_view: Option<std::sync::Arc<crate::stdb::world_view::WorldView>>,
    /// With `relay_view`, the Member Stats a Relay tick delivered between viewer registration and
    /// the world-entry party frame.
    pub(crate) member_stats_before_party_frame: Option<u64>,
}

impl SessionStore for WorldFake {
    fn pending_system_messages(&self, _self_guid: u64) -> Vec<String> {
        self.session.pending_system_messages.clone()
    }

    fn lookup_session(&self, account_name: &str) -> Result<Option<WorldSession>> {
        Ok((account_name == self.session.username)
            .then(|| self.session.session.clone())
            .flatten())
    }

    fn player_login(
        &self,
        _account_id: u64,
        _character: Actor,
        entry: codec::WorldEntry,
    ) -> Result<codec::EntityView> {
        self.rec("player_login");
        self.session.login_entries.lock().unwrap().push(entry);
        let call = self
            .session
            .login_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call > 0 {
            if let Some(e) = &self.session.worldport_login_error {
                return Err(anyhow!("{e}"));
            }
            if let Some(e) = self.session.worldport_entity.clone() {
                return Ok(e);
            }
        }
        self.session
            .login_entity
            .clone()
            .ok_or_else(|| anyhow!("no login entity configured"))
    }

    fn movement_update(&self, actor: Actor, opcode: u32, info: &MovementInfo) -> Result<()> {
        self.rec("movement_update");
        if self.session.movement_transport_lost {
            return Err(ReducerCallError::transport_lost("gw_movement_batch").into());
        }
        self.session.moves.lock().unwrap().push((
            opcode,
            info.position.x,
            info.position.y,
            info.position.z,
            info.orientation,
            info.timestamp,
        ));
        if let Some(world) = &self.session.movement_world {
            world.update(actor.guid(), opcode, info)?;
        }
        Ok(())
    }

    fn subscribe_player_events(
        &self,
        _account_id: u64,
        self_guid: u64,
        arrival: &codec::EntityView,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions> {
        self.rec("subscribe_player_events");
        self.session.subscribed.lock().unwrap().push((
            self_guid,
            arrival.map_id,
            arrival.x,
            arrival.y,
        ));
        *self.session.session_depth.lock().unwrap() = Some(tx.depth_handle());
        if self.session.turn_in_reward_item.is_some() {
            *self.session.turn_in_tx.lock().unwrap() = Some(tx.clone());
        }
        let Some(view) = &self.session.relay_view else {
            return Ok(PlayerSubscriptions::empty());
        };
        let subs = PlayerSubscriptions::registered_for_test(view.clone(), self_guid, arrival, tx);
        if let (Some(mate), Some(record)) = (
            self.session.member_stats_before_party_frame,
            subs.member_stats_record(),
        ) {
            let delivered = codec::MemberStats::default();
            record
                .lock()
                .insert(mate, MemberSnapshot::Live(Box::new(delivered)));
        }
        Ok(subs)
    }

    fn client_command(&self, actor: Actor, cmd: String, payload: String) -> Result<()> {
        if let Some(error) = self.session.client_command_error {
            return Err(error("gw_client_command").into());
        }
        self.session
            .client_commands
            .lock()
            .unwrap()
            .push((actor.guid(), cmd, payload));
        Ok(())
    }

    fn entity_in_world(&self, guid: u64) -> bool {
        self.session
            .entity_presence_checks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(present) = &self.session.entity_presence {
            return present.load(std::sync::atomic::Ordering::SeqCst);
        }
        // `live_guids` is the per-guid answer the realm-wide party frame needs ("is this member
        // live on THIS shard"). Empty by default, so the single flag above is still the answer
        // every test written before realm-wide party routing set.
        self.session.entity_in_world || self.session.live_guids.contains(&guid)
    }

    fn entity_max_health(&self, _guid: u64) -> u32 {
        100
    }

    fn claim_session(&self, account_id: u64, _character_guid: u64) -> Result<WorldSessionToken> {
        Ok(WorldSessionToken {
            account_id,
            generation: 1,
            request_nonce: 1,
        })
    }

    fn bind_session(
        &self,
        _token: WorldSessionToken,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        Ok(None)
    }

    fn release_session(&self, _token: WorldSessionToken) -> Result<()> {
        self.rec("logout");
        self.session
            .logout_called
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if self.session.logout_transport_lost {
            return Err(ReducerCallError::transport_lost("logout").into());
        }
        Ok(())
    }

    fn player_combat_until_ms(&self, _player_guid: u64) -> u64 {
        self.session.combat_until_ms
    }
}
