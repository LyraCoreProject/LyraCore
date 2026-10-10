//! Durable Requests on `Coordinator` that no Store trait owns: the relays' intent claims, logon,
//! provisioning and the Gateway heartbeat. A family's reducer calls live in `store/<family>.rs`,
//! the Realm index calls in `realm_db.rs`.

use anyhow::Result;
use spacetimedb_sdk::Identity;
use std::time::Duration;

use super::bindings::*;
use super::connection::{call_reducer, Coordinator};

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
}
