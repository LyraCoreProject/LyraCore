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

    /// Enter the world: call the `player_login` reducer on the coordinator connection
    /// (so `ctx.sender` is the player's bound identity), then read the resulting
    /// `game_world_entity` row back through the privileged cache as an `EntityView`.
    fn player_login(
        &self,
        account_id: u64,
        character: Actor,
        entry: codec::WorldEntry,
    ) -> Result<codec::EntityView> {
        // Login rides `gw_player_login` on the COORDINATOR connection (module half: delegates to
        // apply_player_login with the account's bound identity as row owner, binds entity→lease,
        // fail-closed on either missing) — no per-player connection exists anywhere. A world-port
        // rides its twin `gw_player_world_port`, which keeps the Away Status.
        let coord = self.0.call_pipe();
        let actor = self.session_actor(character);
        match entry {
            codec::WorldEntry::FreshLogin => call_reducer!(
                coord.conn.reducers,
                "gw_player_login",
                gw_player_login_then(account_id, actor)
            )?,
            codec::WorldEntry::WorldPort => call_reducer!(
                coord.conn.reducers,
                "gw_player_world_port",
                gw_player_world_port_then(account_id, actor)
            )?,
        }

        // The reducer committed; the row propagates to the owner cache asynchronously. Poll
        // briefly until it appears (home_* ride along from the game_character row, and its
        // zone_id is the fallback for a live row the Module could not resolve a zone for).
        let character_guid = character.guid();
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
                let Some(character) = self
                    .0
                    .coord()
                    .conn
                    .db
                    .game_character()
                    .guid()
                    .find(&character_guid)
                else {
                    std::thread::sleep(Duration::from_millis(15));
                    continue;
                };
                let mut view = entity_view(e, character.zone_id);
                view.home_map = character.home_map;
                view.home_zone = character.home_zone;
                view.home_x = character.home_x;
                view.home_y = character.home_y;
                view.home_z = character.home_z;
                view.watched_faction_index = Some(character.watched_faction_index);
                return Ok(view);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        Err(anyhow!(
            "player_login committed but Character or game_world_entity {character_guid} not visible in the \
             coordinator cache within 15s"
        ))
    }

    fn movement_update(&self, actor: Actor, opcode: u32, info: &MovementInfo) -> Result<()> {
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

    /// Forward an addon-bridge command to the module's `client_command` dispatch.
    fn client_command(&self, actor: Actor, cmd: String, payload: String) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_client_command",
            gw_client_command_then(self.session_actor(actor), cmd, payload)
        )
    }

    /// Undelivered private System Messages addressed to `self_guid`, oldest-first, from the
    /// shard's `game_system_message_event` cache. Rows live until the module's event GC reaps
    /// them, so a message emitted inside `player_login` is still here at world entry.
    fn pending_system_messages(&self, self_guid: u64) -> Vec<String> {
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

    fn entity_in_world(&self, guid: u64) -> bool {
        self.0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
            .is_some()
    }

    /// Read from the privileged cache; the module applies the authoritative fall damage itself.
    fn entity_max_health(&self, guid: u64) -> u32 {
        self.0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
            .map_or(0, |e| e.max_health)
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
        self.0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&player_guid)
            .map_or(0, |e| e.combat_until_ms)
    }
}
