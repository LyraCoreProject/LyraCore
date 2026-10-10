//! `Coordinator`'s [`SessionStore`] adapter.

use std::time::Duration;

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;
use wow_world_messages::vanilla::MovementInfo;

use crate::codec;
use crate::stdb::bindings::{GwMove, *};
use crate::stdb::connection::call_reducer;
use crate::stdb::views::entity_view;
use crate::stdb::{Coordinator, PlayerSubscriptions};
use crate::world::{Actor, SessionStore, SessionTx, WorldSession, WorldStore, MOVE_SUBMITTED};

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

impl Coordinator {
    /// The EFFECTIVE armor for `guid` for the character-sheet CREATE (`UNIT_FIELD_RESISTANCES[0]`),
    /// Presence check for the WORLDPORT_ACK gate: is the guid's live entity in the world?
    pub fn entity_in_world(&self, guid: u64) -> bool {
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
            .is_some()
    }

    /// The live entity's `max_health` from the privileged cache — 0 if not in world. Feeds the
    /// fall-damage flavor line; the module applies the authoritative damage itself.
    pub fn entity_max_health(&self, guid: u64) -> u32 {
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
            .map(|e| e.max_health)
            .unwrap_or(0)
    }

    /// Return the `combat_until_ms` value for `player_guid`'s entity row (0 if the entity is not
    /// found or was never in combat). Read from the privileged coordinator cache.
    pub fn player_combat_until_ms(&self, player_guid: u64) -> u64 {
        let guard = self.0.coord();
        guard
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&player_guid)
            .map_or(0, |e| e.combat_until_ms)
    }

    /// Undelivered private System Messages addressed to `self_guid`, oldest-first, from the
    /// shard's `game_system_message_event` cache. Rows live until the module's event GC reaps
    /// them, so a message emitted inside `player_login` is still here at world entry.
    pub fn system_messages_for(&self, self_guid: u64) -> Vec<String> {
        let mut rows: Vec<(u64, String)> = self
            .0
            .coord()
            .conn
            .db
            .game_system_message_event()
            .iter()
            .filter(|row| row.recipient_guid == self_guid)
            .map(|row| (row.id, row.message))
            .collect();
        rows.sort_unstable_by_key(|(id, _)| *id);
        rows.into_iter().map(|(_, message)| message).collect()
    }

    /// Enter the world: call the `player_login` reducer on the coordinator connection
    /// (so `ctx.sender` is the player's bound identity), then read the resulting
    /// `game_world_entity` row back through the privileged cache as an `EntityView`.
    pub fn player_login(
        &self,
        account_id: u64,
        character_guid: u64,
        entry: crate::codec::WorldEntry,
    ) -> Result<crate::codec::EntityView> {
        // Login rides `gw_player_login` on the COORDINATOR connection (module half: delegates to
        // apply_player_login with the account's bound identity as row owner, binds entity→lease,
        // fail-closed on either missing) — no per-player connection exists anywhere. A world-port
        // rides its twin `gw_player_world_port`, which keeps the Away Status.
        let coord = self.0.call_pipe();
        let actor = self.actor_or_owner(character_guid);
        match entry {
            crate::codec::WorldEntry::FreshLogin => call_reducer!(
                coord.conn.reducers,
                "gw_player_login",
                gw_player_login_then(account_id, actor)
            )?,
            crate::codec::WorldEntry::WorldPort => call_reducer!(
                coord.conn.reducers,
                "gw_player_world_port",
                gw_player_world_port_then(account_id, actor)
            )?,
        }

        // The reducer committed; the row propagates to the owner cache asynchronously. Poll
        // briefly until it appears (home_* ride along from the game_character row, and its
        // zone_id is the fallback for a live row the Module could not resolve a zone for).
        let char_row = self
            .0
            .coord()
            .conn
            .db
            .game_character()
            .guid()
            .find(&character_guid);
        let zone_id = char_row.as_ref().map(|c| c.zone_id).unwrap_or(0);
        let home_map = char_row.as_ref().map(|c| c.home_map).unwrap_or(0);
        let home_zone = char_row.as_ref().map(|c| c.home_zone).unwrap_or(0);
        let home_x = char_row.as_ref().map(|c| c.home_x).unwrap_or(0.0);
        let home_y = char_row.as_ref().map(|c| c.home_y).unwrap_or(0.0);
        let home_z = char_row.as_ref().map(|c| c.home_z).unwrap_or(0.0);
        // 15 s cap, 15 ms steps. Was 3 s — the cold-1000 measurement showed the reducer
        // COMMITTING while the coordinator stream lagged the login-burst tail past 3 s (writer at
        // 34.5%, so pure propagation, not CPU): 67/1000 logins died here with the entity already
        // live. The poll exits on first sight, so the longer cap costs nothing outside a burst.
        for _ in 0..1000 {
            if let Some(e) = self
                .0
                .coord()
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&character_guid)
            {
                let mut view = entity_view(e, zone_id);
                view.home_map = home_map;
                view.home_zone = home_zone;
                view.home_x = home_x;
                view.home_y = home_y;
                view.home_z = home_z;
                return Ok(view);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Err(anyhow!(
            "player_login committed but game_world_entity {character_guid} not visible in the \
             coordinator cache within 15s"
        ))
    }

    /// Forward an addon-bridge command to the module's `client_command` dispatch.
    pub fn client_command(
        &self,
        _account_id: u64,
        actor_guid: u64,
        cmd: String,
        payload: String,
    ) -> Result<()> {
        let actor = Actor::new(actor_guid)
            .ok_or_else(|| anyhow!("client_command: actor_guid unresolved"))?;
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_client_command",
            gw_client_command_then(self.session_actor(actor), cmd, payload)
        )
    }
}
