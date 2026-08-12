//! Reducer-call wrapper methods on `Coordinator`: each fires a `gw_*` module reducer over the
//! shared coordinator call pipe (the module attributes the caller by the `actor_guid` argument)
//! and blocks on its completion via the `call_reducer!` macro. Cache reads live in `reads.rs`.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Identity;
use std::time::Duration;

use super::bindings::*;
use super::connection::{call_reducer, recv_reducer, Coordinator};
use super::views::entity_view;

impl Coordinator {
    /// Enter the world (Phase 4): call the `player_login` reducer on the coordinator connection
    /// (so `ctx.sender` is the player's bound identity), then read the resulting
    /// `game_world_entity` row back through the privileged cache as an `EntityView`.
    pub fn player_login(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<crate::codec::EntityView> {
        // Login rides `gw_player_login` on the COORDINATOR connection (module half: delegates to
        // apply_player_login with the account's bound identity as row owner, binds entity→lease,
        // fail-closed on either missing) — no per-player connection exists anywhere.
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_player_login",
            gw_player_login_then(account_id, character_guid)
        )?;

        // The reducer committed; the row propagates to the owner cache asynchronously. Poll
        // briefly until it appears (zone_id and home_* ride along from the game_character row).
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

    /// Persist + relay an inbound movement (Phases 5-6): `gw_movement_update` on the coordinator
    /// connection with the mover named by `actor_guid` (0 = caller doesn't know the guid, never
    /// true in-world → error). `movement_info` is the raw body to relay verbatim to observers
    /// (empty until the inbound raw bytes are threaded through — harmless while no peers are in
    /// range).
    #[allow(clippy::too_many_arguments)]
    pub fn movement_update(
        &self,
        _account_id: u64,
        actor_guid: u64,
        opcode: u16,
        movement_info: &[u8],
        x: f32,
        y: f32,
        z: f32,
        o: f32,
        move_time_ms: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("movement_update: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_movement_update",
            gw_movement_update_then(
                actor_guid,
                opcode,
                movement_info.to_vec(),
                x,
                y,
                z,
                o,
                move_time_ms
            )
        )
    }

    /// [`movement_update`](Self::movement_update) without waiting on the completion channel.
    /// `on_done` receives the module's outcome immediately (the batch flush task owns delivery) —
    /// movement runs on the session's socket-reader thread and must never block on a round-trip.
    #[allow(clippy::too_many_arguments)]
    pub fn movement_update_nowait(
        &self,
        _account_id: u64,
        actor_guid: u64,
        opcode: u16,
        movement_info: &[u8],
        x: f32,
        y: f32,
        z: f32,
        o: f32,
        move_time_ms: u32,
        on_done: impl Fn(std::result::Result<(), String>) + Send + Sync + 'static,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("movement_update_nowait: actor_guid unresolved"));
        }
        // Push onto the shard's batch — ONE gw_movement_batch transaction per 40ms
        // tick carries the whole realm's heartbeats instead of one transaction each (the
        // measured 92%-writer wall). Per-move rejection logging moved module-side (the batch
        // reducer logs and skips), so completion is immediate here.
        self.0.motion_batch.lock().unwrap().push(GwMove {
            actor_guid,
            opcode,
            movement_info: movement_info.to_vec(),
            x,
            y,
            z,
            o,
            move_time_ms,
        });
        on_done(Ok(()));
        Ok(())
    }

    /// Heartbeat this gateway's `game_gateway_lease` row every 15 s on EVERY connected
    /// database's coordinator connection, forever. Every shard, not just the default:
    /// `gw_player_login` fail-closes on the lease of the database it runs ON, and a
    /// cross-database login (an instance entry resuming a transfer) runs on that destination
    /// shard — a default-only lease made every instance login die with "no lease for this
    /// gateway". Fire-and-forget per beat — a missed beat is harmless (the TTL tolerates
    /// several) and the loop must never stall on a slow call. Spawned from `main`.
    pub fn spawn_gateway_heartbeat(&self) {
        let coord = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(15));
            loop {
                tick.tick().await;
                // `world_shards()` is default-first; realm-core is appended when it is a
                // distinct database (unconfigured, it aliases the default handle).
                let mut shards = coord.world_shards();
                if let Ok(rc) = coord.realm_core() {
                    if !shards.iter().any(|(n, _)| n == rc.shard_name()) {
                        shards.push((rc.shard_name().to_string(), rc));
                    }
                }
                for (shard_name, shard) in shards {
                    let guard = shard.0.coord();
                    if let Err(e) = guard.conn.reducers.gw_heartbeat() {
                        log::warn!(
                            "gateway heartbeat send failed on {shard_name} (will retry next beat): {e}"
                        );
                    }
                }
            }
        });
    }

    /// Provision SRP6 credentials computed by the gateway (Phase 0 bring-up).
    pub fn provision_account(&self, username: &str, salt: &[u8], verifier: &[u8]) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "provision_account",
            provision_account_then(username.to_string(), salt.to_vec(), verifier.to_vec())
        )
    }

    /// Create a character via the `create_character` reducer (owner connection), mapping the
    /// reducer result to a game outcome. A distinguished `NAME_IN_USE` error → `NameInUse`; any
    /// other reducer/transport error → `Failed` (never propagated as a hard error, so a bad
    /// creation can't drop the world session).
    pub fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: crate::codec::Appearance,
    ) -> Result<crate::codec::CharCreateOutcome> {
        use crate::codec::CharCreateOutcome;
        // The SpacetimeDB-generated reducer binding takes the five appearance bytes positionally;
        // unbundle `Appearance` here, at the single generated-boundary call.
        let result = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "create_character",
            create_character_then(
                account_id,
                name.to_string(),
                race,
                class,
                gender,
                appearance.skin,
                appearance.face,
                appearance.hair_style,
                appearance.hair_color,
                appearance.facial_hair
            )
        );
        Ok(match result {
            Ok(()) => CharCreateOutcome::Success,
            Err(e) if e.to_string().contains("NAME_IN_USE") => CharCreateOutcome::NameInUse,
            Err(e) if e.to_string().contains("SERVER_LIMIT") => CharCreateOutcome::ServerLimit,
            // The 5875 client has no code for "this database may not mint guids", so the outcome is
            // the generic failure — but the REASON must not be swallowed: the whole point of
            // guid-range licensing is that an unlicensed shard fails loudly instead of minting
            // into someone else's range.
            Err(e) => {
                log::warn!("create_character on {} failed: {e:#}", self.shard_name());
                CharCreateOutcome::Failed
            }
        })
    }

    /// Delete a character via the `delete_character` reducer (owner connection — the reducer is
    /// operator-gated, mirroring `create_character`). Ownership is enforced module-side (`NOT_OWNER`
    /// if `character_guid` isn't `account_id`'s), so a malicious/buggy client can't delete another
    /// account's character. Maps to a game outcome the same way `create_character` does: never
    /// propagated as a hard error, so a bad delete can't drop the world session.
    pub fn delete_character(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<crate::codec::CharDeleteOutcome> {
        use crate::codec::CharDeleteOutcome;
        let result = call_reducer!(
            self.0.call_pipe().conn.reducers,
            "delete_character",
            delete_character_then(account_id, character_guid)
        );
        Ok(match result {
            Ok(()) => CharDeleteOutcome::Success,
            Err(_) => CharDeleteOutcome::Failed,
        })
    }

    /// Logon writes K + the bound per-account identity (Phase 1).
    pub fn establish_session(
        &self,
        account_id: u64,
        session_key: &[u8; 40],
        bound_identity: [u8; 32],
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "establish_session",
            establish_session_then(
                account_id,
                session_key.to_vec(),
                Identity::from_byte_array(bound_identity)
            )
        )
    }

    /// Publish `character_guid`'s location into this handle's character→shard index. Call it
    /// on the REALM-CORE handle: on a world shard the index is already maintained transactionally by
    /// `finish_transfer`. Operator-gated module-side (the index is a routing input).
    pub fn set_character_shard(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "set_character_shard",
            set_character_shard_then(character_guid, map_id, instance_id)
        )
    }

    /// Set the player's current target (`CMSG_SET_SELECTION`, Tier 2 / N3) over the coordinator
    /// connection so the module attributes it to the caller. `target_guid` 0 clears it.
    pub fn set_target(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_target: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_target",
            gw_set_target_then(actor_guid, target_guid)
        )
    }

    /// Validate a `CMSG_INSPECT` request (target is a real in-world player, on the caller's map, in
    /// range, friendly) over the coordinator connection so the module resolves the caller from
    /// `ctx.sender`. `Err` (out of range / hostile / no such target) → the caller ignores it.
    pub fn inspect(&self, _account_id: u64,
        actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("inspect: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_inspect",
            gw_inspect_then(actor_guid, target_guid)
        )
    }

    /// Use a gameobject (`CMSG_GAMEOBJ_USE`) — a chest rolls its loot into the corpse-loot table keyed
    /// on the GO guid, a quest-use object grants quest credit. The module gates range + type.
    /// Rides the coordinator connection as `gw_use_gameobject`.
    pub fn use_gameobject(&self, _account_id: u64, actor_guid: u64, go_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("use_gameobject: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_use_gameobject",
            gw_use_gameobject_then(actor_guid, go_guid)
        )
    }

    /// Enter an area trigger (`CMSG_AREATRIGGER`) — credit any active explore quest tied to `trigger_id`.
    pub fn enter_areatrigger(&self, _account_id: u64,
        actor_guid: u64, trigger_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("enter_areatrigger: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_enter_areatrigger",
            gw_enter_areatrigger_then(actor_guid, trigger_id)
        )
    }

    /// Forward an addon-bridge command to the module's `client_command` dispatch.
    pub fn client_command(
        &self,
        _account_id: u64,
        actor_guid: u64,
        cmd: String,
        payload: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("client_command: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_client_command",
            gw_client_command_then(actor_guid, cmd, payload)
        )
    }

    /// Start the player's melee auto-attack on `target_guid` (`CMSG_ATTACKSWING`, combat C1) over
    /// the coordinator connection so the module attributes the swing to the caller.
    pub fn start_attack(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("start_attack: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_attack",
            gw_attack_then(actor_guid, target_guid)
        )
    }

    /// Relay a pet command-bar action (`CMSG_PET_ACTION`) over the coordinator connection so the module
    /// attributes it to the pet's owner. `data` is the raw packed action (flag<<24 | id); the module
    /// decodes stay/follow/attack/dismiss + passive/defensive/aggressive.
    pub fn pet_command(&self, _account_id: u64,
        actor_guid: u64, data: u32, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("pet_command: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_pet_command",
            gw_pet_command_then(actor_guid, data, target_guid)
        )
    }

    /// Start the player's RANGED auto-attack on `target_guid` with `spell_id` (75 Auto Shot / 5019 Shoot)
    /// over the coordinator connection so the module attributes the shot to the caller.
    /// Rides the coordinator connection as `gw_ranged_attack`.
    pub fn start_ranged_attack(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("start_ranged_attack: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_ranged_attack",
            gw_ranged_attack_then(actor_guid, target_guid, spell_id)
        )
    }

    /// Stop the player's melee auto-attack (`CMSG_ATTACKSTOP`, combat C1).
    pub fn stop_attack(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("stop_attack: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_stop_attack",
            gw_stop_attack_then(actor_guid)
        )
    }

    /// Draw or stow the player's weapons (`CMSG_SETSHEATHED`). [#101]
    pub fn set_sheathed(&self, _account_id: u64, actor_guid: u64, state: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_sheathed: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_sheathed",
            gw_set_sheathed_then(actor_guid, state)
        )
    }

    /// Cast a spell (`CMSG_CAST_SPELL`, aura tracer) over the coordinator connection so the module
    /// attributes the cast to the caller. `target_guid` is the client's selected unit (0 = none/self →
    /// the module substitutes the caster), threaded so target-keyed effects see the real target.
    pub fn cast_spell(&self, _account_id: u64,
        actor_guid: u64, spell_id: u32, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cast_spell: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cast_spell",
            gw_cast_spell_then(actor_guid, spell_id, target_guid)
        )
    }

    /// Cast a GROUND-TARGETED spell at a clicked world point (`CMSG_CAST_SPELL` with a DEST_LOCATION —
    /// Flamestrike/Blizzard/Rain of Fire). Same per-account attribution as `cast_spell`; the `(x,y,z)` is
    /// the ground click so the module anchors the AoE/patch there.
    pub fn cast_spell_at(
        &self,
        _account_id: u64,
        actor_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cast_spell_at: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cast_spell_at",
            gw_cast_spell_at_then(actor_guid, spell_id, target_guid, x, y, z)
        )
    }

    /// Cancel one of the caller's own auras by spell id (`CMSG_CANCEL_AURA`) over the coordinator
    /// connection so the module attributes the removal to the caller.
    pub fn cancel_aura(&self, _account_id: u64,
        actor_guid: u64, spell_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cancel_aura: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_aura",
            gw_cancel_aura_then(actor_guid, spell_id)
        )
    }

    /// Cancel the caller's in-progress cast (`CMSG_CANCEL_CAST`) over the coordinator connection so the
    /// module clears the caller's pending cast — no phantom completion GO.
    pub fn cancel_cast(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("cancel_cast: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_cancel_cast",
            gw_cancel_cast_then(actor_guid)
        )
    }

    pub fn send_chat(
        &self,
        _account_id: u64,
        actor_guid: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_chat: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_chat",
            gw_send_chat_then(actor_guid, chat_type, language, message)
        )
    }

    /// Join a chat channel (CMSG_JOIN_CHANNEL — the client auto-sends on zone-in).
    pub fn join_channel(&self, _account_id: u64,
        actor_guid: u64, channel: String) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("join_channel: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_join_channel",
            gw_join_channel_then(actor_guid, channel)
        )
    }

    /// Leave a chat channel (CMSG_LEAVE_CHANNEL).
    pub fn leave_channel(&self, _account_id: u64,
        actor_guid: u64, channel: String) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("leave_channel: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_leave_channel",
            gw_leave_channel_then(actor_guid, channel)
        )
    }

    /// Speak into a channel (the CMSG_MESSAGECHAT Channel arm).
    pub fn send_channel_message(
        &self,
        _account_id: u64,
        actor_guid: u64,
        channel: String,
        message: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_channel_message: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_channel_message",
            gw_send_channel_message_then(actor_guid, channel, message)
        )
    }

    pub fn send_emote(
        &self,
        _account_id: u64,
        actor_guid: u64,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_emote: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_emote",
            gw_send_emote_then(actor_guid, text_emote, emote_anim, target_guid)
        )
    }

    pub fn send_roll(&self, _account_id: u64,
        actor_guid: u64, min_roll: u32, max_roll: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_roll: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_roll",
            gw_send_roll_then(actor_guid, min_roll, max_roll)
        )
    }

    pub fn send_whisper(
        &self,
        _account_id: u64,
        actor_guid: u64,
        target_player: String,
        message: String,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("send_whisper: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_send_whisper",
            gw_send_whisper_then(actor_guid, target_player, message)
        )
    }

    /// `CMSG_MESSAGECHAT` Party (`/p`) — over the coordinator connection so the module
    /// attributes the line (and its group-membership check) to the caller.
    pub fn party_chat(&self, _account_id: u64,
        actor_guid: u64, message: String) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("party_chat: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_party_chat",
            gw_party_chat_then(actor_guid, message)
        )
    }

    /// GM playtest dot-command: `CMSG_MESSAGECHAT` Say text starting with `.`, over the
    /// coordinator connection so the module attributes it (and its `gm_level` gate) to the caller.
    /// Deliberately does NOT use the `call_reducer!` macro: that macro wraps a module `Err` as
    /// `"{what} reducer failed: {e}"` (fine when a caller only substring-matches it, like `party_chat`'s
    /// `NOT_IN_GROUP` check), but the Say handler relays this `Err`'s text VERBATIM to the sender as a
    /// system chat line — a raw `"permission denied"` / `"unknown command: .foo"` must reach the client
    /// with no wrapper prefix.
    pub fn gm_command(&self, _account_id: u64, actor_guid: u64, text: String) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("gm_command: actor_guid unresolved"));
        }
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
        // Raw-module-message plumbing: the GM console renders the module's own rejection text
        // ("permission denied", parse errors) verbatim, no "reducer failed" wrapper.
        let coord = self.0.call_pipe();
        coord
            .conn
            .reducers
            .gw_gm_command_then(actor_guid, text, move |_ctx, status| {
                let _ = tx.send(match status {
                    Ok(inner) => inner,
                    Err(e) => Err(format!("{e:?}")),
                });
            })
            .map_err(|e| anyhow!("send gw_gm_command: {e}"))?;
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(anyhow!("{e}")), // the RAW module message, no "reducer failed" wrapper
            Err(_) => Err(anyhow!("gm_command timed out after 10s")),
        }
    }

    /// `CMSG_PUSHQUESTTOPARTY` — over the coordinator connection so the module
    /// attributes the sender + its grouped/on-quest gates to the caller.
    pub fn push_quest(&self, _account_id: u64, actor_guid: u64, quest_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("push_quest: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_push_quest_to_party",
            gw_push_quest_to_party_then(actor_guid, quest_id)
        )
    }

    /// `CMSG_GROUP_INVITE` — `target_guid` is already resolved by the gateway.
    pub fn group_invite(&self, _account_id: u64,
        actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_invite: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_invite",
            gw_group_invite_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_GROUP_ACCEPT`. Rides the coordinator connection as
    /// `gw_accept_group_invite`.
    pub fn group_accept(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_accept: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_accept_group_invite",
            gw_accept_group_invite_then(actor_guid)
        )
    }

    /// `CMSG_GROUP_DECLINE`.
    pub fn group_decline(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_decline: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_decline",
            gw_group_decline_then(actor_guid)
        )
    }

    /// `CMSG_GROUP_DISBAND` — leave the caller's group.
    pub fn group_leave(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_leave: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_leave",
            gw_group_leave_then(actor_guid)
        )
    }

    /// `CMSG_GROUP_UNINVITE` — the leader kicks `target_guid`.
    pub fn group_uninvite(&self, _account_id: u64,
        actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_uninvite: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_uninvite",
            gw_group_uninvite_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_LOOT_METHOD` — the leader sets the party's loot method/
    /// threshold/master. Echoed to every member via the existing `SMSG_GROUP_LIST` relay (the
    /// module's `group_loot_method` reducer re-renders the roster payload); no separate ack packet
    /// (vanilla sends none for this opcode either).
    pub fn group_loot_method(
        &self,
        _account_id: u64,
        actor_guid: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("group_loot_method: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_group_loot_method",
            gw_group_loot_method_then(actor_guid, loot_setting, master_guid, loot_threshold)
        )
    }

    /// `CMSG_GOSSIP_SELECT_OPTION` — the NOTIFY-ONLY module chokepoint. Fired
    /// best-effort BEFORE the gateway's own gossip behavior; a failure never blocks the reply.
    pub fn gossip_select(
        &self,
        _account_id: u64,
        actor_guid: u64,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("gossip_select: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_gossip_select",
            gw_gossip_select_then(actor_guid, npc_guid, option_id, option_row_id)
        )
    }

    /// `CMSG_ADD_FRIEND` — `target_guid` is already resolved by the gateway.
    pub fn add_friend(&self, _account_id: u64,
        actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("add_friend: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_add_friend",
            gw_add_friend_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_DEL_FRIEND`.
    pub fn del_friend(&self, _account_id: u64,
        actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("del_friend: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_del_friend",
            gw_del_friend_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_ADD_IGNORE` — `target_guid` is already resolved by the gateway.
    pub fn add_ignore(&self, _account_id: u64,
        actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("add_ignore: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_add_ignore",
            gw_add_ignore_then(actor_guid, target_guid)
        )
    }

    /// `CMSG_DEL_IGNORE`.
    pub fn del_ignore(&self, _account_id: u64,
        actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("del_ignore: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_del_ignore",
            gw_del_ignore_then(actor_guid, target_guid)
        )
    }

    /// Take the money from a corpse (`CMSG_LOOT_MONEY`, slice 3) over the coordinator connection so
    /// the module attributes the loot to the caller (as `gw_loot_money`).
    pub fn loot_money(&self, _account_id: u64, actor_guid: u64, target_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("loot_money: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_loot_money",
            gw_loot_money_then(actor_guid, target_guid)
        )
    }

    /// Take one item from the open corpse into the backpack (`CMSG_AUTOSTORE_LOOT_ITEM`, slice 4) over
    /// the coordinator connection so the module attributes the loot to the caller. The module moves the
    /// item into a free slot + deletes the corpse-loot row (the inventory relay then shows it in the bag).
    /// Rides the coordinator connection as `gw_take_loot`.
    pub fn take_loot(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("take_loot: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_take_loot",
            gw_take_loot_then(actor_guid, corpse_guid, loot_slot)
        )
    }

    pub fn skin_corpse(&self, _account_id: u64, actor_guid: u64, corpse_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("skin_corpse: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_skin",
            gw_skin_then(actor_guid, corpse_guid)
        )
    }

    /// `CMSG_LOOT_ROLL` — record the caller's need/greed/pass vote on a
    /// live roll. Live votes/roll numbers relay to every eligible member via the `game_group_event`
    /// roll-kind rows (`stdb/subscriptions.rs`).
    pub fn loot_roll(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("loot_roll: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_loot_roll",
            gw_loot_roll_then(actor_guid, corpse_guid, loot_slot, vote)
        )
    }

    /// `CMSG_LOOT_MASTER_GIVE` — the master looter assigns an above-
    /// threshold row to `target_guid`.
    pub fn loot_master_give(
        &self,
        _account_id: u64,
        actor_guid: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("loot_master_give: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_loot_master_give",
            gw_loot_master_give_then(actor_guid, corpse_guid, loot_slot, target_guid)
        )
    }

    pub fn disenchant_item(&self, _account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("disenchant_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_disenchant",
            gw_disenchant_then(actor_guid, slot)
        )
    }

    pub fn enchant_item_on_slot(
        &self,
        _account_id: u64,
        actor_guid: u64,
        slot: u8,
        enchant_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("enchant_item_on_slot: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_enchant_item",
            gw_enchant_item_then(actor_guid, slot, enchant_id)
        )
    }

    /// Buy `count` of `item_entry` from the vendor `vendor_guid` (`CMSG_BUY_ITEM`, Tier 2) over the
    /// coordinator connection so the module attributes the purchase to the caller. The module gates
    /// it on the vendor (stock + NPC flags + range) and debits the buyer's copper.
    pub fn buy_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("buy_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buy_item",
            gw_buy_item_then(actor_guid, vendor_guid, item_entry, count)
        )
    }

    /// Learn `spell_id` from trainer `trainer_guid` (`CMSG_TRAINER_BUY_SPELL`) over the coordinator
    /// connection. The module gates it (range / level / cost / not-already-known) and charges copper;
    /// the `Err` message carries the module's `[N]` gtker failure-reason tag for the dispatch to forward.
    /// Rides the coordinator connection as `gw_trainer_buy`.
    pub fn buy_trainer_spell(
        &self,
        _account_id: u64,
        actor_guid: u64,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("buy_trainer_spell: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_trainer_buy",
            gw_trainer_buy_then(actor_guid, trainer_guid, spell_id)
        )
    }

    /// Buy the next bank bag slot from `banker_guid` (`CMSG_BUY_BANK_SLOT`) over the coordinator
    /// connection. A refusal carries the module's `[N]` `SMSG_BUY_BANK_SLOT_RESULT` code tag.
    pub fn buy_bank_slot(&self, _account_id: u64, actor_guid: u64, banker_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("buy_bank_slot: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buy_bank_slot",
            gw_buy_bank_slot_then(actor_guid, banker_guid)
        )
    }

    pub fn learn_talent(&self, _account_id: u64,
        actor_guid: u64, talent_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("learn_talent: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_learn_talent",
            gw_learn_talent_then(actor_guid, talent_id)
        )
    }

    /// Respec at a trainer (the "I wish to unlearn my talents." gossip option, #516) — clears every
    /// learned talent for the calling player's escalating gold cost. Rides the coordinator
    /// connection as `gw_reset_talents` (#483 deleted the per-player sender path).
    pub fn reset_talents(&self, _account_id: u64, actor_guid: u64, trainer_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("reset_talents: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_reset_talents",
            gw_reset_talents_then(actor_guid, trainer_guid)
        )
    }

    /// Fishing cast: instant-resolve catch — the module's lenient alpha gate auto-learns the
    /// skill and grants the fish straight to the bag. Caller resolved via ctx.sender.
    pub fn fish(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("fish: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_fish",
            gw_fish_then(actor_guid)
        )
    }

    /// Pick Lock: unlock the locked GameObject `go_guid` over the coordinator connection (so the
    /// module attributes the pick to the caller via ctx.sender). The module gates range / lock
    /// requirement / Lockpicking skill; on success it records the GO unlocked + climbs the skill.
    pub fn pick_lock(&self, _account_id: u64,
        actor_guid: u64, go_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("pick_lock: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_pick_lock",
            gw_pick_lock_then(actor_guid, go_guid)
        )
    }

    /// Persist one action-bar button (`CMSG_SET_ACTION_BUTTON`): upsert by (character, button);
    /// action 0 clears. Without this every bar drag was lost on relog (only creation seeds survived).
    pub fn set_action_button(
        &self,
        _account_id: u64,
        actor_guid: u64,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_action_button: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_action_button",
            gw_set_action_button_then(actor_guid, button, action, action_type)
        )
    }

    /// Persist the rep pane's At-War checkbox (`CMSG_SET_FACTION_ATWAR`, 195 slice B): the wire's
    /// u16 is the client's 0..63 rep-array slot (ReputationListID — the gtker `Faction` field name
    /// lies, same as SET_FACTION_STANDING); the module reverse-resolves the faction and upserts.
    pub fn set_faction_at_war(
        &self,
        _account_id: u64,
        actor_guid: u64,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("set_faction_at_war: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_set_faction_at_war",
            gw_set_faction_at_war_then(actor_guid, reputation_index, at_war)
        )
    }

    /// Sell the item in inventory `slot` back to a vendor (`CMSG_SELL_ITEM`, Tier 2) over the
    /// coordinator connection. The gateway resolves the client's item-INSTANCE guid to the owning
    /// slot before calling (the reducer takes the slot); the module credits the seller's copper.
    /// Rides the coordinator connection as `gw_sell_item`.
    pub fn sell_item(
        &self,
        _account_id: u64,
        actor_guid: u64,
        vendor_guid: u64,
        slot: u8,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("sell_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_sell_item",
            gw_sell_item_then(actor_guid, vendor_guid, slot)
        )
    }

    pub fn buyback_item(&self, _account_id: u64,
        actor_guid: u64, vendor_guid: u64, slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("buyback_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_buyback_item",
            gw_buyback_item_then(actor_guid, vendor_guid, slot)
        )
    }

    /// Repair the item in inventory `slot` at REPAIR-NPC `npc_guid` (`CMSG_REPAIR_ITEM`) over the
    /// coordinator connection. The module gates the NPC + charges copper; the player's item +
    /// purse replicate back via subscription.
    pub fn repair_item(&self, _account_id: u64,
        actor_guid: u64, npc_guid: u64, slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("repair_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_repair_item",
            gw_repair_item_then(actor_guid, npc_guid, slot)
        )
    }

    /// Equip the item in main-inventory `from_slot` (`CMSG_AUTOEQUIP_ITEM`) over the coordinator
    /// connection. The module resolves the matching equipment slot and gates the required level.
    /// Rides the coordinator connection as `gw_equip_item`.
    pub fn equip_item(&self, _account_id: u64, actor_guid: u64, from_slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("equip_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_equip_item",
            gw_equip_item_then(actor_guid, from_slot)
        )
    }

    /// Unequip the item in equipment `from_slot` to a free backpack slot (`CMSG_AUTOSTORE_BAG_ITEM`)
    /// over the coordinator connection. The module gates "is equipped" + "backpack has room".
    pub fn unequip_item(&self, _account_id: u64,
        actor_guid: u64, from_slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("unequip_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_unequip_item",
            gw_unequip_item_then(actor_guid, from_slot)
        )
    }

    /// Use the consumable in main-inventory `slot` (`CMSG_USE_ITEM`) over the coordinator connection —
    /// eat/drink/potion/bandage. The module applies the on-use effect (flat heal for slice food) and
    /// decrements the stack; a gameplay `Err` (no item / not usable) is per-action.
    pub fn use_item(&self, _account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("use_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_use_item",
            gw_use_item_then(actor_guid, slot)
        )
    }

    /// Bind the caller's hearthstone home to their current position (`CMSG_GOSSIP_SELECT_OPTION` on an
    /// innkeeper's "Make this inn your home.") over the coordinator connection so the module attributes
    /// it to the caller's entity. No args — `bind_home` resolves the caller via `ctx.sender`.
    pub fn bind_home(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("bind_home: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_bind_home",
            gw_bind_home_then(actor_guid)
        )
    }

    /// Move (or swap) main-inventory `from_slot` → `to_slot` (`CMSG_SWAP_INV_ITEM`/`CMSG_SWAP_ITEM`)
    /// over the coordinator connection. The module's move primitive validates equip-slot transitions.
    pub fn move_item(&self, _account_id: u64,
        actor_guid: u64, from_slot: u8, to_slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("move_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_move_item",
            gw_move_item_then(actor_guid, from_slot, to_slot)
        )
    }

    /// Auto-bank/auto-store-bank the item in `slot` (`CMSG_AUTOBANK_ITEM`/`CMSG_AUTOSTORE_BANK_ITEM`)
    /// over the coordinator connection. The module infers deposit vs. withdraw from `slot` and
    /// resolves the receiving free slot itself.
    pub fn auto_bank_item(&self, _account_id: u64, actor_guid: u64, slot: u8) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("auto_bank_item: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_auto_bank_item",
            gw_auto_bank_item_then(actor_guid, slot)
        )
    }

    /// Accept quest `quest_id` from giver `giver_guid` (`CMSG_QUESTGIVER_ACCEPT_QUEST`) over the
    /// coordinator connection so the module attributes it to the caller. The module gates the accept
    /// (giver relation + range + level + not-already-held); a gameplay `Err` is per-action, not fatal.
    /// Rides the coordinator connection as `gw_accept_quest`.
    pub fn accept_quest(
        &self,
        _account_id: u64,
        actor_guid: u64,
        giver_guid: u64,
        quest_id: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("accept_quest: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_accept_quest",
            gw_accept_quest_then(actor_guid, giver_guid, quest_id)
        )
    }

    /// Turn quest `quest_id` in to giver `giver_guid` (`CMSG_QUESTGIVER_CHOOSE_REWARD`) over the
    /// coordinator connection. The module validates completion + grants the rewards (money/XP/items).
    /// `reward_index` is the player's pick-1-of-N choice slot; ignored when the quest has no choices.
    /// Rides the coordinator connection as `gw_turn_in_quest`.
    pub fn turn_in_quest(
        &self,
        _account_id: u64,
        actor_guid: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("turn_in_quest: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_turn_in_quest",
            gw_turn_in_quest_then(actor_guid, giver_guid, quest_id, reward_index)
        )
    }

    /// Abandon quest `quest_id` (`CMSG_QUESTLOG_REMOVE_QUEST`) over the coordinator connection. The
    /// module deletes the player's quest-log row; the quest-log relay then clears the slot.
    pub fn abandon_quest(&self, _account_id: u64,
        actor_guid: u64, quest_id: u32) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("abandon_quest: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_abandon_quest",
            gw_abandon_quest_then(actor_guid, quest_id)
        )
    }

    /// Revive the caller after death (`CMSG_REPOP_REQUEST`, slice 4) over the coordinator connection.
    /// Rides the coordinator connection as `gw_repop`.
    pub fn repop(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("repop: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(coord.conn.reducers, "gw_repop", gw_repop_then(actor_guid))
    }

    /// Reclaim the caller's corpse (`CMSG_RECLAIM_CORPSE`, slice 5) over the coordinator connection.
    pub fn reclaim_corpse(&self, _account_id: u64,
        actor_guid: u64, corpse_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("reclaim_corpse: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_reclaim_corpse",
            gw_reclaim_corpse_then(actor_guid, corpse_guid)
        )
    }

    /// Answer a pending resurrect offer (`CMSG_RESURRECT_RESPONSE`) over the coordinator connection.
    /// Rides the coordinator connection as `gw_respond_resurrect`.
    pub fn resurrect_response(&self, _account_id: u64, actor_guid: u64, accept: bool) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("resurrect_response: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_respond_resurrect",
            gw_respond_resurrect_then(actor_guid, accept)
        )
    }

    /// Spirit-Healer resurrect (`CMSG_SPIRIT_HEALER_ACTIVATE`) over the coordinator connection: the
    /// module res's the caller in place at 50% + applies Resurrection Sickness if it's a ghost.
    /// `gw_spirit_res` takes no `healer_guid`.
    pub fn spirit_healer_res(
        &self,
        _account_id: u64,
        actor_guid: u64,
        _healer_guid: u64,
    ) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("spirit_healer_res: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_spirit_res",
            gw_spirit_res_then(actor_guid)
        )
    }

    /// Explicit logout (Phase 7): call the `logout` reducer over the coordinator connection so the
    /// module removes the live `game_world_entity` row. That delete fires every in-range observer's
    /// `game_world_entity` on_delete → `SMSG_DESTROY_OBJECT`, so the peer vanishes. Required because
    /// the player's SDK connection is cached/reused and does NOT drop when the game client's TCP
    /// socket closes (so the module's `on_disconnect` would not otherwise fire).
    pub fn logout(&self, _account_id: u64, actor_guid: u64) -> Result<()> {
        if actor_guid == 0 {
            return Err(anyhow!("logout: actor_guid unresolved"));
        }
        let coord = self.0.call_pipe();
        call_reducer!(
            coord.conn.reducers,
            "gw_leave_world",
            gw_leave_world_then(actor_guid)
        )
    }

    // -------------------------------------------------------------------------------------
    // Cross-database transfer. ALL of these are operator-gated orchestration (`require_operator`),
    // and the
    // destination shard has no bound player identity until the character has arrived on it — which
    // is precisely what they exist to make happen.
    // -------------------------------------------------------------------------------------

    /// `begin_transfer` — freeze the character, serialize it (row + every manifest table's rows),
    /// and delete its live entity, in ONE transaction. Idempotent on `transfer_id`.
    pub fn begin_transfer(&self, plan: &crate::world::transfer::TransferPlan) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "begin_transfer",
            begin_transfer_then(
                plan.transfer_id,
                plan.character_guid,
                plan.dest_map_id,
                plan.dest_instance_id,
                plan.dest_x,
                plan.dest_y,
                plan.dest_z,
                plan.dest_o,
                true, // cross_database — this wrapper only ever drives a two-database move
            )
        )
    }

    /// `import_character_blob` — materialise the arrival copy at the destination from the blob the
    /// gateway carried. Idempotent on `transfer_id`.
    pub fn import_character_blob(&self, transfer_id: u64, blob: &[u8]) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "import_character_blob",
            import_character_blob_then(transfer_id, blob.to_vec())
        )
    }

    /// `confirm_import` — attest ON THE SOURCE that the destination copy committed. Called only
    /// after `import_character_blob` returned Ok; see `world::transfer::run_transfer`.
    pub fn confirm_import(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "confirm_import",
            confirm_import_then(transfer_id)
        )
    }

    /// `finish_transfer` — delete-last: destroy the source copy and clear the escrow.
    pub fn finish_transfer(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_transfer",
            finish_transfer_then(transfer_id)
        )
    }

    /// `release_transfer` — drop the arrival copy's fence at the destination.
    pub fn release_transfer(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "release_transfer",
            release_transfer_then(transfer_id)
        )
    }

    /// `ensure_instance` — mirror an instance id onto this shard (idempotent), spawning its
    /// population the first time.
    pub fn ensure_instance(&self, instance_id: u64, map_id: u32, party_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "ensure_instance",
            ensure_instance_then(instance_id, map_id, party_id)
        )
    }

    /// `evict_instance_population` — stop this shard ticking an instance whose run moved elsewhere.
    pub fn evict_instance_population(&self, instance_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "evict_instance_population",
            evict_instance_population_then(instance_id)
        )
    }

    /// `record_shard_load` — fired against THIS handle's connection. Callers hold the
    /// **realm-core** handle: `game_shard_load` is only ever read from there.
    pub fn record_shard_load(
        &self,
        shard: &str,
        writer_occupancy_pct: f32,
        sessions: u32,
        gateway_key: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "record_shard_load",
            record_shard_load_then(
                shard.to_string(),
                writer_occupancy_pct,
                sessions,
                gateway_key
            )
        )
    }

    /// `realm_group_op` — one party op against the database THIS handle points at. The gateway
    /// calls it on the **realm-core** handle, where membership is authoritative.
    ///
    /// Through the COORDINATOR connection, not the player's: the reducer is operator-gated because
    /// it takes the acting character's guid as an argument (realm-core has no live entity to derive
    /// one from), so only the token that holds the operator identity may call it. The guid passed is
    /// the one this socket authenticated into the world with — see `world::party`.
    pub fn realm_group_op(
        &self,
        op: u8,
        actor_guid: u64,
        target_guid: u64,
        arg_a: u8,
        arg_b: u8,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_group_op",
            realm_group_op_then(op, actor_guid, target_guid, arg_a, arg_b)
        )
    }

    /// `realm_whisper` — deliver one whisper against the database THIS handle points at. The
    /// gateway calls it on the **realm-core** handle, the only database that can
    /// address both parties of a cross-shard whisper (a guid is realm-wide; an identity is not).
    ///
    /// Through the COORDINATOR connection, not the player's: the reducer is operator-gated because it
    /// takes the SENDING character's guid as an argument (realm-core has no live entity to derive one
    /// from), so only the token that holds the operator identity may call it. The guid passed is the
    /// one this socket authenticated into the world with — see `world::whisper`. `sender_is_ignored`
    /// is the target's ignore-list verdict, read from the shard that holds the target's contact rows.
    pub fn realm_whisper(
        &self,
        sender_guid: u64,
        target_guid: u64,
        message: String,
        sender_is_ignored: bool,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_whisper",
            realm_whisper_then(sender_guid, target_guid, message, sender_is_ignored)
        )
    }

    /// `sync_group_mirror` — replace THIS shard's mirror of one party with realm-core's roster.
    /// Operator-gated, coordinator connection, same reasoning as above; called
    /// on each WORLD shard after a party op and at world entry.
    pub fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "sync_group_mirror",
            sync_group_mirror_then(
                roster.group_id,
                roster.leader_guid,
                roster.loot_method,
                roster.loot_threshold,
                roster.master_looter_guid,
                roster.members.clone(),
            )
        )
    }

    /// `realm_loot_op` — one loot-roll op against the database THIS handle points at. The
    /// gateway calls it on the **realm-core** handle: START promotes a world shard's staging roll,
    /// VOTE casts `CMSG_LOOT_ROLL`'s vote.
    ///
    /// Through the COORDINATOR connection, not the player's: the reducer is operator-gated because
    /// it acts on realm-core, which has no live entity to derive an actor from. VOTE's `actor_guid`
    /// is the guid this socket authenticated into the world with (`InWorld::self_guid`), never a
    /// literal a client supplies; START's `recipients` are the spatial snapshot a world shard already
    /// computed at kill time.
    #[allow(clippy::too_many_arguments)]
    pub fn realm_loot_op(
        &self,
        op: u8,
        corpse_guid: u64,
        slot: u8,
        item_entry: u32,
        actor_guid: u64,
        vote: u8,
        deadline_micros: i64,
        recipients: Vec<u64>,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "realm_loot_op",
            realm_loot_op_then(
                op,
                corpse_guid,
                slot,
                item_entry,
                actor_guid,
                vote,
                deadline_micros,
                recipients
            )
        )
    }

    /// `settle_loot_roll` — grant a resolved roll's item on THIS world shard, if it holds the
    /// matching corpse row. Operator-gated, coordinator connection; the loot-roll relay calls
    /// it on every connected world shard after observing realm-core's `ROLL_WON` event — the
    /// module's own `withheld` guard makes a wrong-shard call a harmless no-op.
    pub fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "settle_loot_roll",
            settle_loot_roll_then(corpse_guid, slot, winner_guid)
        )
    }

    /// `clear_promoted_loot_roll` — delete a staging roll's rows on THIS world shard, once the
    /// loot-roll relay has promoted it onto realm-core. Operator-gated, coordinator connection.
    pub fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "clear_promoted_loot_roll",
            clear_promoted_loot_roll_then(roll_id)
        )
    }
}
