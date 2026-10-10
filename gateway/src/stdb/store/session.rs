//! `Coordinator`'s [`SessionStore`] adapter.

use anyhow::{anyhow, Result};
use wow_world_messages::vanilla::MovementInfo;

use crate::codec;
use crate::world::{Actor, SessionStore, SessionTx, WorldSession, WorldStore, MOVE_SUBMITTED};

use crate::stdb::bindings::GwMove;
use crate::stdb::Coordinator;
use crate::stdb::PlayerSubscriptions;

impl SessionStore for Coordinator {
    fn lookup_session(&self, account_name: &str) -> Result<Option<WorldSession>> {
        crate::realm_core::lookup_session(self, account_name)
    }

    fn player_login(
        &self,
        account_id: u64,
        character_guid: u64,
        entry: codec::WorldEntry,
    ) -> Result<codec::EntityView> {
        self.player_login(account_id, character_guid, entry)
    }

    fn movement_update(
        &self,
        _account_id: u64,
        self_guid: u64,
        opcode: u32,
        info: &MovementInfo,
    ) -> Result<()> {
        let actor = Actor::new(self_guid)
            .ok_or_else(|| anyhow!("movement_update: actor_guid unresolved"))?;
        // Carry the MovementInfo verbatim so the module can relay it to in-range observers under
        // the same opcode. Queueing is the only completion observable on the shared batch;
        // individual reducer outcomes are intentionally unavailable.
        let body = codec::movement_info_to_bytes(info)?;
        self.0.motion_batch.push(GwMove {
            actor: self.session_actor(actor),
            opcode: opcode as u16,
            movement_info: body,
            x: info.position.x,
            y: info.position.y,
            z: info.position.z,
            o: info.orientation,
            move_time_ms: info.timestamp,
        });
        MOVE_SUBMITTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        arrival: &crate::codec::EntityView,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions> {
        self.subscribe_player_events(account_id, self_guid, arrival, tx)
    }

    fn client_command(
        &self,
        account_id: u64,
        self_guid: u64,
        cmd: String,
        payload: String,
    ) -> Result<()> {
        self.client_command(account_id, self_guid, cmd, payload)
    }

    fn pending_system_messages(&self, self_guid: u64) -> Vec<String> {
        self.system_messages_for(self_guid)
    }

    fn entity_in_world(&self, guid: u64) -> bool {
        self.entity_in_world(guid)
    }

    fn entity_max_health(&self, guid: u64) -> u32 {
        self.entity_max_health(guid)
    }

    fn claim_session(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<crate::world::WorldSessionToken> {
        self.claim_session(account_id, character_guid)
    }

    fn bind_session(
        &self,
        token: crate::world::WorldSessionToken,
    ) -> Result<Option<std::sync::Arc<dyn WorldStore>>> {
        Ok(Some(std::sync::Arc::new(self.bind_session(token)?)))
    }

    fn release_session(&self, token: crate::world::WorldSessionToken) -> Result<()> {
        self.release_session(token)
    }

    fn player_combat_until_ms(&self, player_guid: u64) -> u64 {
        self.player_combat_until_ms(player_guid)
    }
}
