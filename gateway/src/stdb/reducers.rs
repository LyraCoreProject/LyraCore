//! Durable Requests on `Coordinator` that no single Store family owns: realm-core routing,
//! Transfer intents, provisioning and the helpers several families share. A family's own
//! reducer calls live in `store/<family>.rs`.

use anyhow::{anyhow, Result};
use spacetimedb_sdk::Identity;
use std::time::Duration;

use super::bindings::*;
use super::connection::{call_reducer, Coordinator};
use crate::world::Actor;

impl Coordinator {
    pub fn claim_bot_transfer_intent(
        &self,
        intent_id: u64,
        bot_guid: u64,
        controller_generation: u64,
        claim_token: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "claim_bot_transfer_intent",
            claim_bot_transfer_intent_then(intent_id, bot_guid, controller_generation, claim_token)
        )
    }

    pub fn complete_bot_transfer_intent(
        &self,
        intent_id: u64,
        bot_guid: u64,
        controller_generation: u64,
        claim_token: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "complete_bot_transfer_intent",
            complete_bot_transfer_intent_then(
                intent_id,
                bot_guid,
                controller_generation,
                claim_token
            )
        )
    }

    pub fn defer_party_command_intent(&self, intent_id: u64, claim_token: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "defer_party_command_intent",
            defer_party_command_intent_then(intent_id, claim_token)
        )
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

    /// Provision SRP6 credentials computed by the Gateway.
    pub fn provision_account(&self, username: &str, salt: &[u8], verifier: &[u8]) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "provision_account",
            provision_account_then(username.to_string(), salt.to_vec(), verifier.to_vec())
        )
    }

    /// Logon writes K + the bound per-account identity.
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
            set_character_shard_then(
                character_guid,
                map_id,
                instance_id,
                self.actor_or_owner(character_guid)
            )
        )
    }

    // The arguments mirror the Realm reducer's exact Transfer Gate.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_character_shard_transfer(
        &self,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
        source_module_identity: spacetimedb_sdk::Identity,
        intent_id: u64,
        controller_generation: u64,
        character_guid: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "begin_character_shard_transfer",
            begin_character_shard_transfer_then(
                character_guid,
                source_map,
                source_instance,
                source_revision,
                destination_map,
                destination_instance,
                source_module_identity,
                intent_id,
                controller_generation,
                self.actor_or_owner(character_guid)
            )
        )
    }

    pub fn finish_character_shard_transfer(
        &self,
        intent: &crate::world::transfer::BotTransferIntent,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_character_shard_transfer",
            finish_character_shard_transfer_then(
                intent.bot_guid,
                intent.source_map,
                intent.source_instance,
                intent.source_locator_revision,
                intent.destination_map,
                intent.destination_instance,
                intent.source_module_identity,
                intent.id,
                intent.controller_generation,
                self.actor_or_owner(intent.bot_guid)
            )
        )
    }

    pub fn finish_player_character_shard_transfer(
        &self,
        character_guid: u64,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_character_shard_transfer",
            finish_character_shard_transfer_then(
                character_guid,
                source_map,
                source_instance,
                source_revision,
                destination_map,
                destination_instance,
                spacetimedb_sdk::Identity::ZERO,
                0,
                0,
                self.actor_or_owner(character_guid)
            )
        )
    }

    // The arguments mirror the Realm reducer's exact Transfer Gate.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_pending_character_shard_transfer(
        &self,
        character_guid: u64,
        source_map: u32,
        source_instance: u64,
        source_revision: u64,
        destination_map: u32,
        destination_instance: u64,
        source_module_identity: spacetimedb_sdk::Identity,
        transfer_intent_id: u64,
        controller_generation: u64,
    ) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "finish_pending_character_shard_transfer",
            finish_pending_character_shard_transfer_then(
                character_guid,
                source_map,
                source_instance,
                source_revision,
                destination_map,
                destination_instance,
                source_module_identity,
                transfer_intent_id,
                controller_generation,
                self.actor_or_owner(character_guid)
            )
        )
    }

    /// Send one classified command request to this Home Shard. The Module combines the conveyed
    /// Account authority with its own Character GM level and remains the final Gate. Its Refusal
    /// reason is the text the GM sees.
    pub(crate) fn request_gm_command(
        &self,
        actor_guid: u64,
        alpha_test_tools: bool,
        text: String,
    ) -> Result<()> {
        let actor =
            Actor::new(actor_guid).ok_or_else(|| anyhow!("gm_command: actor_guid unresolved"))?;
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_gm_command",
            gw_gm_command_then(self.session_actor(actor), alpha_test_tools, text)
        )
    }

    // -------------------------------------------------------------------------------------
    // Cross-database transfer. ALL of these are operator-gated orchestration (`require_operator`),
    // and the
    // destination shard has no bound player identity until the character has arrived on it — which
    // is precisely what they exist to make happen.
    // -------------------------------------------------------------------------------------

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
}

/// Wait up to two seconds for a row a value-flow reducer just committed to reach this handle's
/// cache.
pub(super) fn wait_for_cache_row<T>(
    operation_id: u64,
    row_name: &str,
    mut read: impl FnMut() -> Option<T>,
) -> Result<T> {
    for _ in 0..100 {
        if let Some(row) = read() {
            return Ok(row);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(anyhow!(
        "{row_name} {operation_id} committed but is not visible in the coordinator cache"
    ))
}

/// A random nonzero operation id for one auction or guild fee value flow.
pub(super) fn next_operation_id() -> Result<u64> {
    loop {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow!("OS randomness unavailable: {error}"))?;
        let operation_id = u64::from_le_bytes(bytes);
        if operation_id != 0 {
            return Ok(operation_id);
        }
    }
}
