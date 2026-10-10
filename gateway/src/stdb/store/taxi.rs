//! `Coordinator`'s [`TaxiActionStore`] adapter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};

use crate::codec;
use crate::stdb::bindings::*;
use crate::stdb::connection::call_reducer;
use crate::stdb::Coordinator;
use crate::world::{Actor, TaxiActionStore};

impl TaxiActionStore for crate::stdb::Coordinator {
    fn taxi_node_status(
        &self,
        character_guid: u64,
        npc_guid: u64,
    ) -> Result<Option<codec::TaxiNodeStatusView>> {
        crate::stdb::Coordinator::taxi_node_status(self, character_guid, npc_guid)
    }

    fn open_taxi(&self, character_guid: u64, npc_guid: u64) -> Result<Option<codec::TaxiMapView>> {
        crate::stdb::Coordinator::open_taxi(self, character_guid, npc_guid)
    }

    fn activate_taxi(
        &self,
        character_guid: u64,
        npc_guid: u64,
        source_client_node_id: u32,
        destination_client_node_id: u32,
    ) -> Result<codec::TaxiActivationResult> {
        crate::stdb::Coordinator::activate_taxi(
            self,
            character_guid,
            npc_guid,
            source_client_node_id,
            destination_client_node_id,
        )
    }

    fn arm_taxi_flight(&self, character_guid: u64) -> Result<()> {
        crate::stdb::Coordinator::arm_taxi_flight(self, character_guid)
    }
}

impl Coordinator {
    fn await_taxi_reply(
        &self,
        character_guid: u64,
        npc_guid: u64,
        request_id: u64,
        operation: u8,
    ) -> Result<Option<TaxiServiceReply>> {
        // Reducer completion and subscription propagation travel on the same SDK connection but
        // are separate events. Poll by the caller-chosen id so an older cached reply can never be
        // mistaken for this operation's result.
        for _ in 0..100 {
            let reply = self
                .0
                .coord()
                .conn
                .db
                .game_taxi_service_reply()
                .request_id()
                .find(&request_id);
            if let Some(reply) =
                reply.filter(|reply| taxi_reply_matches(reply, character_guid, npc_guid, operation))
            {
                // Copy the cache row before acknowledging it. The ack deletes only this request's
                // module row; a failed ack is infrastructure loss and remains session-fatal.
                call_reducer!(
                    self.0.call_pipe().conn.reducers,
                    "gw_ack_taxi_reply",
                    gw_ack_taxi_reply_then(self.actor_or_owner(character_guid), request_id)
                )?;
                if !reply.accepted {
                    log::debug!(
                        "taxi operation {operation} refused for character {character_guid}: {}",
                        reply.refusal
                    );
                }
                return Ok(Some(reply));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(anyhow!(
            "taxi operation {operation} committed but reply {request_id} was not visible within 1s"
        ))
    }

    /// Cohesive status request: module resolution + policy runs before this returns one known bit.
    pub fn taxi_node_status(
        &self,
        character_guid: u64,
        npc_guid: u64,
    ) -> Result<Option<crate::codec::TaxiNodeStatusView>> {
        let Some(actor) = Actor::new(character_guid) else {
            return Ok(None);
        };
        let request_id = next_taxi_request_id();
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_taxi_node_status",
            gw_taxi_node_status_then(self.session_actor(actor), npc_guid, request_id)
        )?;
        Ok(self
            .await_taxi_reply(
                character_guid,
                npc_guid,
                request_id,
                lyracore_shared::constants::taxi_protocol::REPLY_STATUS,
            )?
            .filter(|reply| reply.accepted)
            .map(|reply| crate::codec::TaxiNodeStatusView {
                npc_guid: reply.npc_guid,
                known: reply.known,
            }))
    }

    pub fn open_taxi(
        &self,
        character_guid: u64,
        npc_guid: u64,
    ) -> Result<Option<crate::codec::TaxiMapView>> {
        let Some(actor) = Actor::new(character_guid) else {
            return Ok(None);
        };
        let request_id = next_taxi_request_id();
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_open_taxi",
            gw_open_taxi_then(self.session_actor(actor), npc_guid, request_id)
        )?;
        Ok(self
            .await_taxi_reply(
                character_guid,
                npc_guid,
                request_id,
                lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
            )?
            .filter(|reply| reply.accepted)
            .map(|reply| crate::codec::TaxiMapView {
                npc_guid: reply.npc_guid,
                source_client_node_id: reply.source_client_node_id,
                available_client_node_ids: reply.available_client_node_ids,
            }))
    }

    /// Cohesive direct-flight activation. The module commits every gameplay outcome as a stable
    /// result code; only reducer transport/timeout failures escape as `Err` and end the session.
    pub fn activate_taxi(
        &self,
        character_guid: u64,
        npc_guid: u64,
        source_client_node_id: u32,
        destination_client_node_id: u32,
    ) -> Result<crate::codec::TaxiActivationResult> {
        let Some(actor) = Actor::new(character_guid) else {
            return Ok(crate::codec::TaxiActivationResult {
                result_code:
                    lyracore_shared::constants::taxi_protocol::ACTIVATE_UNSPECIFIED_SERVER_ERROR,
            });
        };
        let request_id = next_taxi_request_id();
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_activate_taxi",
            gw_activate_taxi_then(
                self.session_actor(actor),
                npc_guid,
                source_client_node_id,
                destination_client_node_id,
                request_id
            )
        )?;
        let reply = self
            .await_taxi_reply(
                character_guid,
                npc_guid,
                request_id,
                lyracore_shared::constants::taxi_protocol::REPLY_ACTIVATE,
            )?
            .ok_or_else(|| anyhow!("taxi activation reply {request_id} disappeared"))?;
        Ok(crate::codec::TaxiActivationResult {
            result_code: reply.result_code,
        })
    }

    pub fn arm_taxi_flight(&self, character_guid: u64) -> Result<()> {
        call_reducer!(
            self.0.call_pipe().conn.reducers,
            "gw_arm_taxi_flight",
            gw_arm_taxi_flight_then(self.actor_or_owner(character_guid))
        )
    }
}

static NEXT_TAXI_REQUEST_ID: OnceLock<AtomicU64> = OnceLock::new();

fn next_taxi_request_id() -> u64 {
    let next = NEXT_TAXI_REQUEST_ID.get_or_init(|| {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        AtomicU64::new(seed.max(1))
    });
    let id = next.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        next.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    }
}

fn taxi_reply_matches(
    reply: &TaxiServiceReply,
    character_guid: u64,
    npc_guid: u64,
    operation: u8,
) -> bool {
    reply.character_guid == character_guid
        && reply.operation == operation
        && reply.npc_guid == npc_guid
}

#[cfg(test)]
mod taxi_reply_tests {
    use super::*;

    fn reply(
        request_id: u64,
        character_guid: u64,
        npc_guid: u64,
        operation: u8,
    ) -> TaxiServiceReply {
        TaxiServiceReply {
            request_id,
            character_guid,
            operation,
            npc_guid,
            accepted: true,
            known: false,
            source_client_node_id: 0,
            available_client_node_ids: Vec::new(),
            refusal: String::new(),
            created_micros: 0,
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_OK,
        }
    }

    #[test]
    fn reply_selection_rejects_stale_character_operation_and_npc_rows() {
        let current = reply(
            22,
            7,
            90,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        );
        assert!(taxi_reply_matches(
            &current,
            7,
            90,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        ));
        assert!(!taxi_reply_matches(
            &current,
            8,
            90,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        ));
        assert!(!taxi_reply_matches(
            &current,
            7,
            91,
            lyracore_shared::constants::taxi_protocol::REPLY_OPEN,
        ));
        assert!(!taxi_reply_matches(
            &current,
            7,
            90,
            lyracore_shared::constants::taxi_protocol::REPLY_STATUS,
        ));
    }
}
