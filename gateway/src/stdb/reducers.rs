//! Reducer-call wrapper methods on `Coordinator`: each fires a module reducer (over the privileged
//! owner connection or a per-account player connection) and blocks on its completion via the
//! `call_reducer!` macro. Cache reads live in `reads.rs`.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Identity;
use std::time::Duration;

use super::bindings::*;
use super::connection::{call_reducer, call_reducer_nowait, recv_reducer, Coordinator};
use super::views::entity_view;

impl Coordinator {
    /// Enter the world (Phase 4): call the `player_login` reducer on the per-account connection
    /// (so `ctx.sender` is the player's bound identity), then read the resulting
    /// `game_world_entity` row back through the privileged cache as an `EntityView`.
    pub fn player_login(
        &self,
        account_id: u64,
        character_guid: u64,
    ) -> Result<crate::codec::EntityView> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "player_login",
            player_login_then(character_guid)
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
        for _ in 0..200 {
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
             coordinator cache within 3s"
        ))
    }

    /// Persist + relay an inbound movement (Phases 5-6): call the `movement_update` reducer on the
    /// per-account connection so the module attributes it to the right `game_world_entity`.
    /// `movement_info` is the raw body to relay verbatim to observers (empty until the inbound
    /// raw bytes are threaded through — harmless while no peers are in range).
    #[allow(clippy::too_many_arguments)]
    pub fn movement_update(
        &self,
        account_id: u64,
        opcode: u16,
        movement_info: &[u8],
        x: f32,
        y: f32,
        z: f32,
        o: f32,
        move_time_ms: u32,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "movement_update",
            movement_update_then(opcode, movement_info.to_vec(), x, y, z, o, move_time_ms)
        )
    }

    /// [`movement_update`](Self::movement_update) without waiting on the completion channel
    /// (perf catalog 1.13, #110). `on_done` receives the module's outcome on the SDK callback
    /// thread. See `call_reducer_nowait!` for why movement specifically must not block.
    #[allow(clippy::too_many_arguments)]
    pub fn movement_update_nowait(
        &self,
        account_id: u64,
        opcode: u16,
        movement_info: &[u8],
        x: f32,
        y: f32,
        z: f32,
        o: f32,
        move_time_ms: u32,
        on_done: impl Fn(std::result::Result<(), String>) + Send + Sync + 'static,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer_nowait!(
            player.conn.reducers,
            "movement_update",
            movement_update_then(opcode, movement_info.to_vec(), x, y, z, o, move_time_ms),
            on_done
        )
    }

    /// Provision SRP6 credentials computed by the gateway (Phase 0 bring-up).
    pub fn provision_account(&self, username: &str, salt: &[u8], verifier: &[u8]) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
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
            self.0.coord().conn.reducers,
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
            // the generic failure — but the REASON must not be swallowed: #108's whole point is that
            // an unlicensed shard fails loudly instead of minting into someone else's range.
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
            self.0.coord().conn.reducers,
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
            self.0.coord().conn.reducers,
            "establish_session",
            establish_session_then(
                account_id,
                session_key.to_vec(),
                Identity::from_byte_array(bound_identity)
            )
        )
    }

    /// Publish `character_guid`'s location into this handle's character→shard index (#20). Call it
    /// on the REALM-CORE handle: on a world shard the index is already maintained transactionally by
    /// `finish_transfer`. Operator-gated module-side (the index is a routing input).
    pub fn set_character_shard(
        &self,
        character_guid: u64,
        map_id: u32,
        instance_id: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "set_character_shard",
            set_character_shard_then(character_guid, map_id, instance_id)
        )
    }

    /// Set the player's current target (`CMSG_SET_SELECTION`, Tier 2 / N3) over the per-account
    /// connection so the module attributes it to the caller. `target_guid` 0 clears it.
    pub fn set_target(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "set_target",
            set_target_then(target_guid)
        )
    }

    /// Validate a `CMSG_INSPECT` request (target is a real in-world player, on the caller's map, in
    /// range, friendly) over the per-account connection so the module resolves the caller from
    /// `ctx.sender`. `Err` (out of range / hostile / no such target) → the caller ignores it.
    pub fn inspect(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "inspect", inspect_then(target_guid))
    }

    /// Use a gameobject (`CMSG_GAMEOBJ_USE`) — a chest rolls its loot into the corpse-loot table keyed
    /// on the GO guid, a quest-use object grants quest credit. The module gates range + type.
    pub fn use_gameobject(&self, account_id: u64, go_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "use_gameobject",
            use_gameobject_then(go_guid)
        )
    }

    /// Enter an area trigger (`CMSG_AREATRIGGER`) — credit any active explore quest tied to `trigger_id`.
    pub fn enter_areatrigger(&self, account_id: u64, trigger_id: u32) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "enter_areatrigger",
            enter_areatrigger_then(trigger_id)
        )
    }

    /// Forward an addon-bridge command (184) to the module's `client_command` dispatch.
    pub fn client_command(&self, account_id: u64, cmd: String, payload: String) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "client_command",
            client_command_then(cmd, payload)
        )
    }

    /// Start the player's melee auto-attack on `target_guid` (`CMSG_ATTACKSWING`, combat C1) over
    /// the per-account connection so the module attributes the swing to the caller.
    pub fn start_attack(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "start_attack",
            start_attack_then(target_guid)
        )
    }

    /// Relay a pet command-bar action (`CMSG_PET_ACTION`) over the per-account connection so the module
    /// attributes it to the pet's owner. `data` is the raw packed action (flag<<24 | id); the module
    /// decodes stay/follow/attack/dismiss + passive/defensive/aggressive.
    pub fn pet_command(&self, account_id: u64, data: u32, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "pet_command",
            pet_command_then(data, target_guid)
        )
    }

    /// Start the player's RANGED auto-attack on `target_guid` with `spell_id` (75 Auto Shot / 5019 Shoot,
    /// #10) over the per-account connection so the module attributes the shot to the caller.
    pub fn start_ranged_attack(
        &self,
        account_id: u64,
        target_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "start_ranged_attack",
            start_ranged_attack_then(target_guid, spell_id)
        )
    }

    /// Stop the player's melee auto-attack (`CMSG_ATTACKSTOP`, combat C1).
    pub fn stop_attack(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "stop_attack", stop_attack_then())
    }

    /// Cast a spell (`CMSG_CAST_SPELL`, aura tracer) over the per-account connection so the module
    /// attributes the cast to the caller. `target_guid` is the client's selected unit (0 = none/self →
    /// the module substitutes the caster), threaded so target-keyed effects see the real target.
    pub fn cast_spell(&self, account_id: u64, spell_id: u32, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "cast_spell",
            cast_spell_then(spell_id, target_guid)
        )
    }

    /// Cast a GROUND-TARGETED spell at a clicked world point (`CMSG_CAST_SPELL` with a DEST_LOCATION —
    /// Flamestrike/Blizzard/Rain of Fire). Same per-account attribution as `cast_spell`; the `(x,y,z)` is
    /// the ground click so the module anchors the AoE/patch there (118 phase 2).
    pub fn cast_spell_at(
        &self,
        account_id: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "cast_spell_at",
            cast_spell_at_then(spell_id, target_guid, x, y, z)
        )
    }

    /// Cancel one of the caller's own auras by spell id (`CMSG_CANCEL_AURA`) over the per-account
    /// connection so the module attributes the removal to the caller.
    pub fn cancel_aura(&self, account_id: u64, spell_id: u32) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "cancel_aura",
            cancel_aura_then(spell_id)
        )
    }

    /// Cancel the caller's in-progress cast (`CMSG_CANCEL_CAST`) over the per-account connection so the
    /// module clears the caller's pending cast — no phantom completion GO. [083]
    pub fn cancel_cast(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "cancel_cast", cancel_cast_then())
    }

    pub fn send_chat(
        &self,
        account_id: u64,
        chat_type: u8,
        language: u8,
        message: String,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "send_chat",
            send_chat_then(chat_type, language, message)
        )
    }

    /// Join a chat channel (065, CMSG_JOIN_CHANNEL — the client auto-sends on zone-in).
    pub fn join_channel(&self, account_id: u64, channel: String) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "join_channel",
            join_channel_then(channel)
        )
    }

    /// Leave a chat channel (065, CMSG_LEAVE_CHANNEL).
    pub fn leave_channel(&self, account_id: u64, channel: String) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "leave_channel",
            leave_channel_then(channel)
        )
    }

    /// Speak into a channel (065, the CMSG_MESSAGECHAT Channel arm).
    pub fn send_channel_message(
        &self,
        account_id: u64,
        channel: String,
        message: String,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "send_channel_message",
            send_channel_message_then(channel, message)
        )
    }

    pub fn send_emote(
        &self,
        account_id: u64,
        text_emote: u32,
        emote_anim: u32,
        target_guid: u64,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "send_emote",
            send_emote_then(text_emote, emote_anim, target_guid)
        )
    }

    pub fn send_roll(&self, account_id: u64, min_roll: u32, max_roll: u32) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "send_roll",
            send_roll_then(min_roll, max_roll)
        )
    }

    pub fn send_whisper(
        &self,
        account_id: u64,
        target_player: String,
        message: String,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "send_whisper",
            send_whisper_then(target_player, message)
        )
    }

    /// `CMSG_MESSAGECHAT` Party (`/p`, work-item 199) — over the per-account connection so the module
    /// attributes the line (and its group-membership check) to the caller.
    pub fn party_chat(&self, account_id: u64, message: String) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "party_chat", party_chat_then(message))
    }

    /// GM playtest dot-command (work-item 223): `CMSG_MESSAGECHAT` Say text starting with `.`, over the
    /// per-account connection so the module attributes it (and its `gm_level` gate) to the caller.
    /// Deliberately does NOT use the `call_reducer!` macro: that macro wraps a module `Err` as
    /// `"{what} reducer failed: {e}"` (fine when a caller only substring-matches it, like `party_chat`'s
    /// `NOT_IN_GROUP` check), but the Say handler relays this `Err`'s text VERBATIM to the sender as a
    /// system chat line — a raw `"permission denied"` / `"unknown command: .foo"` must reach the client
    /// with no wrapper prefix.
    pub fn gm_command(&self, account_id: u64, text: String) -> Result<()> {
        let player = self.player_conn(account_id)?;
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
        player
            .conn
            .reducers
            .gm_command_then(text, move |_ctx, status| {
                let _ = tx.send(match status {
                    Ok(inner) => inner,
                    Err(e) => Err(format!("{e:?}")),
                });
            })
            .map_err(|e| anyhow!("send gm_command: {e}"))?;
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(anyhow!("{e}")), // the RAW module message, no "reducer failed" wrapper
            Err(_) => Err(anyhow!("gm_command timed out after 10s")),
        }
    }

    /// `CMSG_PUSHQUESTTOPARTY` (work-item 194) — over the per-account connection so the module
    /// attributes the sender + its grouped/on-quest gates to the caller.
    pub fn push_quest(&self, account_id: u64, quest_id: u32) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "push_quest_to_party",
            push_quest_to_party_then(quest_id)
        )
    }

    /// `CMSG_GROUP_INVITE` (work-item 066) — `target_guid` is already resolved by the gateway.
    pub fn group_invite(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "group_invite",
            group_invite_then(target_guid)
        )
    }

    /// `CMSG_GROUP_ACCEPT`.
    pub fn group_accept(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "group_accept", group_accept_then())
    }

    /// `CMSG_GROUP_DECLINE`.
    pub fn group_decline(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "group_decline", group_decline_then())
    }

    /// `CMSG_GROUP_DISBAND` — leave the caller's group.
    pub fn group_leave(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "group_leave", group_leave_then())
    }

    /// `CMSG_GROUP_UNINVITE` — the leader kicks `target_guid`.
    pub fn group_uninvite(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "group_uninvite",
            group_uninvite_then(target_guid)
        )
    }

    /// `CMSG_LOOT_METHOD` (work-item 187 slice 1) — the leader sets the party's loot method/
    /// threshold/master. Echoed to every member via the existing `SMSG_GROUP_LIST` relay (the
    /// module's `group_loot_method` reducer re-renders the roster payload); no separate ack packet
    /// (vanilla sends none for this opcode either).
    pub fn group_loot_method(
        &self,
        account_id: u64,
        loot_setting: u8,
        master_guid: u64,
        loot_threshold: u8,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "group_loot_method",
            group_loot_method_then(loot_setting, master_guid, loot_threshold)
        )
    }

    /// `CMSG_GOSSIP_SELECT_OPTION` — the NOTIFY-ONLY module chokepoint (work-item 146). Fired
    /// best-effort BEFORE the gateway's own gossip behavior; a failure never blocks the reply.
    pub fn gossip_select(
        &self,
        account_id: u64,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "gossip_select",
            gossip_select_then(npc_guid, option_id, option_row_id)
        )
    }

    /// `CMSG_ADD_FRIEND` (work-item 130) — `target_guid` is already resolved by the gateway.
    pub fn add_friend(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "add_friend",
            add_friend_then(target_guid)
        )
    }

    /// `CMSG_DEL_FRIEND`.
    pub fn del_friend(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "del_friend",
            del_friend_then(target_guid)
        )
    }

    /// `CMSG_ADD_IGNORE` — `target_guid` is already resolved by the gateway.
    pub fn add_ignore(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "add_ignore",
            add_ignore_then(target_guid)
        )
    }

    /// `CMSG_DEL_IGNORE`.
    pub fn del_ignore(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "del_ignore",
            del_ignore_then(target_guid)
        )
    }

    /// Take the money from a corpse (`CMSG_LOOT_MONEY`, slice 3) over the per-account connection so
    /// the module attributes the loot to the caller.
    pub fn loot_money(&self, account_id: u64, target_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "loot_money",
            loot_money_then(target_guid)
        )
    }

    /// Take one item from the open corpse into the backpack (`CMSG_AUTOSTORE_LOOT_ITEM`, slice 4) over
    /// the per-account connection so the module attributes the loot to the caller. The module moves the
    /// item into a free slot + deletes the corpse-loot row (the inventory relay then shows it in the bag).
    pub fn take_loot(&self, account_id: u64, corpse_guid: u64, loot_slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "take_loot",
            take_loot_then(corpse_guid, loot_slot)
        )
    }

    pub fn skin_corpse(&self, account_id: u64, corpse_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "skin", skin_then(corpse_guid))
    }

    /// `CMSG_LOOT_ROLL` (work-item 187 slices 2-3) — record the caller's need/greed/pass vote on a
    /// live roll. Live votes/roll numbers relay to every eligible member via the `game_group_event`
    /// roll-kind rows (`stdb/subscriptions.rs`).
    pub fn loot_roll(
        &self,
        account_id: u64,
        corpse_guid: u64,
        loot_slot: u32,
        vote: u8,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "loot_roll",
            loot_roll_then(corpse_guid, loot_slot, vote)
        )
    }

    /// `CMSG_LOOT_MASTER_GIVE` (work-item 187 slice 4) — the master looter assigns an above-
    /// threshold row to `target_guid`.
    pub fn loot_master_give(
        &self,
        account_id: u64,
        corpse_guid: u64,
        loot_slot: u8,
        target_guid: u64,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "loot_master_give",
            loot_master_give_then(corpse_guid, loot_slot, target_guid)
        )
    }

    pub fn disenchant_item(&self, account_id: u64, slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "disenchant", disenchant_then(slot))
    }

    pub fn enchant_item_on_slot(&self, account_id: u64, slot: u8, enchant_id: u32) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "enchant_item",
            enchant_item_then(slot, enchant_id)
        )
    }

    /// Buy `count` of `item_entry` from the vendor `vendor_guid` (`CMSG_BUY_ITEM`, Tier 2) over the
    /// per-account connection so the module attributes the purchase to the caller. The module gates
    /// it on the vendor (stock + NPC flags + range) and debits the buyer's copper.
    pub fn buy_item(
        &self,
        account_id: u64,
        vendor_guid: u64,
        item_entry: u32,
        count: u32,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "buy_item",
            buy_item_then(vendor_guid, item_entry, count)
        )
    }

    /// Learn `spell_id` from trainer `trainer_guid` (`CMSG_TRAINER_BUY_SPELL`) over the per-account
    /// connection. The module gates it (range / level / cost / not-already-known) and charges copper;
    /// the `Err` message carries the module's `[N]` gtker failure-reason tag for the dispatch to forward.
    pub fn buy_trainer_spell(
        &self,
        account_id: u64,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "buy_trainer_spell",
            buy_trainer_spell_then(trainer_guid, spell_id)
        )
    }

    pub fn learn_talent(&self, account_id: u64, talent_id: u32) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "learn_talent",
            learn_talent_then(talent_id)
        )
    }

    /// Fishing cast (060): instant-resolve catch — the module's lenient alpha gate auto-learns the
    /// skill and grants the fish straight to the bag. Caller resolved via ctx.sender.
    pub fn fish(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "fish", fish_then())
    }

    /// Pick Lock (119): unlock the locked GameObject `go_guid` over the per-account connection (so the
    /// module attributes the pick to the caller via ctx.sender). The module gates range / lock
    /// requirement / Lockpicking skill; on success it records the GO unlocked + climbs the skill.
    pub fn pick_lock(&self, account_id: u64, go_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "pick_lock", pick_lock_then(go_guid))
    }

    /// Persist one action-bar button (`CMSG_SET_ACTION_BUTTON`): upsert by (character, button);
    /// action 0 clears. Without this every bar drag was lost on relog (only creation seeds survived).
    pub fn set_action_button(
        &self,
        account_id: u64,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "set_action_button",
            set_action_button_then(button, action, action_type)
        )
    }

    /// Persist the rep pane's At-War checkbox (`CMSG_SET_FACTION_ATWAR`, 195 slice B): the wire's
    /// u16 is the client's 0..63 rep-array slot (ReputationListID — the gtker `Faction` field name
    /// lies, same as SET_FACTION_STANDING); the module reverse-resolves the faction and upserts.
    pub fn set_faction_at_war(
        &self,
        account_id: u64,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "set_faction_at_war",
            set_faction_at_war_then(reputation_index, at_war)
        )
    }

    /// Sell the item in inventory `slot` back to a vendor (`CMSG_SELL_ITEM`, Tier 2) over the
    /// per-account connection. The gateway resolves the client's item-INSTANCE guid to the owning
    /// slot before calling (the reducer takes the slot); the module credits the seller's copper.
    pub fn sell_item(&self, account_id: u64, vendor_guid: u64, slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "sell_item",
            sell_item_then(vendor_guid, slot)
        )
    }

    pub fn buyback_item(&self, account_id: u64, vendor_guid: u64, slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "buyback_item",
            buyback_item_then(vendor_guid, slot)
        )
    }

    /// Repair the item in inventory `slot` at REPAIR-NPC `npc_guid` (`CMSG_REPAIR_ITEM`) over the
    /// per-account connection. The module gates the NPC + charges copper; the player's item +
    /// purse replicate back via subscription.
    pub fn repair_item(&self, account_id: u64, npc_guid: u64, slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "repair_item",
            repair_item_then(npc_guid, slot)
        )
    }

    /// Equip the item in main-inventory `from_slot` (`CMSG_AUTOEQUIP_ITEM`) over the per-account
    /// connection. The module resolves the matching equipment slot and gates the required level.
    pub fn equip_item(&self, account_id: u64, from_slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "equip_item",
            equip_item_then(from_slot)
        )
    }

    /// Unequip the item in equipment `from_slot` to a free backpack slot (`CMSG_AUTOSTORE_BAG_ITEM`)
    /// over the per-account connection. The module gates "is equipped" + "backpack has room".
    pub fn unequip_item(&self, account_id: u64, from_slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "unequip_item",
            unequip_item_then(from_slot)
        )
    }

    /// Use the consumable in main-inventory `slot` (`CMSG_USE_ITEM`) over the per-account connection —
    /// eat/drink/potion/bandage. The module applies the on-use effect (flat heal for slice food) and
    /// decrements the stack; a gameplay `Err` (no item / not usable) is per-action.
    pub fn use_item(&self, account_id: u64, slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "use_item", use_item_then(slot))
    }

    /// Bind the caller's hearthstone home to their current position (`CMSG_GOSSIP_SELECT_OPTION` on an
    /// innkeeper's "Make this inn your home.") over the per-account connection so the module attributes
    /// it to the caller's entity. No args — `bind_home` resolves the caller via `ctx.sender`.
    pub fn bind_home(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "bind_home", bind_home_then())
    }

    /// Move (or swap) main-inventory `from_slot` → `to_slot` (`CMSG_SWAP_INV_ITEM`/`CMSG_SWAP_ITEM`)
    /// over the per-account connection. The module's move primitive validates equip-slot transitions.
    pub fn move_item(&self, account_id: u64, from_slot: u8, to_slot: u8) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "move_item",
            move_item_then(from_slot, to_slot)
        )
    }

    /// Accept quest `quest_id` from giver `giver_guid` (`CMSG_QUESTGIVER_ACCEPT_QUEST`) over the
    /// per-account connection so the module attributes it to the caller. The module gates the accept
    /// (giver relation + range + level + not-already-held); a gameplay `Err` is per-action, not fatal.
    pub fn accept_quest(&self, account_id: u64, giver_guid: u64, quest_id: u32) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "accept_quest",
            accept_quest_then(giver_guid, quest_id)
        )
    }

    /// Turn quest `quest_id` in to giver `giver_guid` (`CMSG_QUESTGIVER_CHOOSE_REWARD`) over the
    /// per-account connection. The module validates completion + grants the rewards (money/XP/items).
    /// `reward_index` is the player's pick-1-of-N choice slot; ignored when the quest has no choices.
    pub fn turn_in_quest(
        &self,
        account_id: u64,
        giver_guid: u64,
        quest_id: u32,
        reward_index: u32,
    ) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "turn_in_quest",
            turn_in_quest_then(giver_guid, quest_id, reward_index)
        )
    }

    /// Abandon quest `quest_id` (`CMSG_QUESTLOG_REMOVE_QUEST`) over the per-account connection. The
    /// module deletes the player's quest-log row; the quest-log relay then clears the slot.
    pub fn abandon_quest(&self, account_id: u64, quest_id: u32) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "abandon_quest",
            abandon_quest_then(quest_id)
        )
    }

    /// Revive the caller after death (`CMSG_REPOP_REQUEST`, slice 4) over the per-account connection.
    pub fn repop(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "repop", repop_then())
    }

    /// Reclaim the caller's corpse (`CMSG_RECLAIM_CORPSE`, slice 5) over the per-account connection.
    pub fn reclaim_corpse(&self, account_id: u64, corpse_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "reclaim_corpse",
            reclaim_corpse_then(corpse_guid)
        )
    }

    /// Answer a pending resurrect offer (`CMSG_RESURRECT_RESPONSE`, #014) over the per-account connection.
    pub fn resurrect_response(&self, account_id: u64, accept: bool) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "resurrect_response",
            resurrect_response_then(accept)
        )
    }

    /// Spirit-Healer resurrect (`CMSG_SPIRIT_HEALER_ACTIVATE`) over the per-account connection: the
    /// module res's the caller in place at 50% + applies Resurrection Sickness if it's a ghost.
    pub fn spirit_healer_res(&self, account_id: u64, healer_guid: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(
            player.conn.reducers,
            "spirit_healer_res",
            spirit_healer_res_then(healer_guid)
        )
    }

    /// Explicit logout (Phase 7): call the `logout` reducer over the per-account connection so the
    /// module removes the live `game_world_entity` row. That delete fires every in-range observer's
    /// `game_world_entity` on_delete → `SMSG_DESTROY_OBJECT`, so the peer vanishes. Required because
    /// the player's SDK connection is cached/reused and does NOT drop when the game client's TCP
    /// socket closes (so the module's `on_disconnect` would not otherwise fire).
    pub fn logout(&self, account_id: u64) -> Result<()> {
        let player = self.player_conn(account_id)?;
        call_reducer!(player.conn.reducers, "logout", logout_then())
    }

    // -------------------------------------------------------------------------------------
    // Cross-database transfer (#19). ALL of these run over the COORDINATOR connection, not the
    // per-player one: they are operator-gated orchestration (`require_operator`), and the
    // destination shard has no bound player identity until the character has arrived on it — which
    // is precisely what they exist to make happen.
    // -------------------------------------------------------------------------------------

    /// `begin_transfer` — freeze the character, serialize it (row + every manifest table's rows),
    /// and delete its live entity, in ONE transaction. Idempotent on `transfer_id`.
    pub fn begin_transfer(&self, plan: &crate::world::transfer::TransferPlan) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
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
            self.0.coord().conn.reducers,
            "import_character_blob",
            import_character_blob_then(transfer_id, blob.to_vec())
        )
    }

    /// `confirm_import` — attest ON THE SOURCE that the destination copy committed. Called only
    /// after `import_character_blob` returned Ok; see `world::transfer::run_transfer`.
    pub fn confirm_import(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "confirm_import",
            confirm_import_then(transfer_id)
        )
    }

    /// `finish_transfer` — delete-last: destroy the source copy and clear the escrow.
    pub fn finish_transfer(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "finish_transfer",
            finish_transfer_then(transfer_id)
        )
    }

    /// `release_transfer` — drop the arrival copy's fence at the destination.
    pub fn release_transfer(&self, transfer_id: u64) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "release_transfer",
            release_transfer_then(transfer_id)
        )
    }

    /// `ensure_instance` — mirror an instance id onto this shard (idempotent), spawning its
    /// population the first time.
    pub fn ensure_instance(&self, instance_id: u64, map_id: u32, party_id: u64) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "ensure_instance",
            ensure_instance_then(instance_id, map_id, party_id)
        )
    }

    /// `evict_instance_population` — stop this shard ticking an instance whose run moved elsewhere.
    pub fn evict_instance_population(&self, instance_id: u64) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "evict_instance_population",
            evict_instance_population_then(instance_id)
        )
    }

    /// `record_shard_load` (#78) — fired against THIS handle's connection. Callers hold the
    /// **realm-core** handle: `game_shard_load` is only ever read from there
    /// (`docs/region-sharding.md`), the same convention `record_region_load`/
    /// `set_region_assignment` use for their own tables.
    pub fn record_shard_load(
        &self,
        shard: &str,
        writer_occupancy_pct: f32,
        sessions: u32,
    ) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "record_shard_load",
            record_shard_load_then(shard.to_string(), writer_occupancy_pct, sessions)
        )
    }

    /// `record_region_load` (#78) — same calling convention as [`Coordinator::record_shard_load`].
    pub fn record_region_load(&self, map_id: u32, region_id: u32, players: u32) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "record_region_load",
            record_region_load_then(map_id, region_id, players)
        )
    }

    /// `realm_group_op` — one party op against the database THIS handle points at (#22, group
    /// slice). The gateway calls it on the **realm-core** handle, where membership is authoritative.
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
            self.0.coord().conn.reducers,
            "realm_group_op",
            realm_group_op_then(op, actor_guid, target_guid, arg_a, arg_b)
        )
    }

    /// `realm_whisper` — deliver one whisper against the database THIS handle points at (#22,
    /// whisper slice). The gateway calls it on the **realm-core** handle, the only database that can
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
            self.0.coord().conn.reducers,
            "realm_whisper",
            realm_whisper_then(sender_guid, target_guid, message, sender_is_ignored)
        )
    }

    /// `sync_group_mirror` — replace THIS shard's mirror of one party with realm-core's roster
    /// (#22, group slice). Operator-gated, coordinator connection, same reasoning as above; called
    /// on each WORLD shard after a party op and at world entry.
    pub fn sync_group_mirror(&self, roster: &crate::world::party::GroupRoster) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
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

    /// `realm_loot_op` — one loot-roll op against the database THIS handle points at (#50). The
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
            self.0.coord().conn.reducers,
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
    /// matching corpse row (#50). Operator-gated, coordinator connection; the loot-roll relay calls
    /// it on every connected world shard after observing realm-core's `ROLL_WON` event — the
    /// module's own `withheld` guard makes a wrong-shard call a harmless no-op.
    pub fn settle_loot_roll(&self, corpse_guid: u64, slot: u8, winner_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "settle_loot_roll",
            settle_loot_roll_then(corpse_guid, slot, winner_guid)
        )
    }

    /// `clear_promoted_loot_roll` — delete a staging roll's rows on THIS world shard, once the
    /// loot-roll relay has promoted it onto realm-core (#50). Operator-gated, coordinator connection.
    pub fn clear_promoted_loot_roll(&self, roll_id: u64) -> Result<()> {
        call_reducer!(
            self.0.coord().conn.reducers,
            "clear_promoted_loot_roll",
            clear_promoted_loot_roll_then(roll_id)
        )
    }
}
