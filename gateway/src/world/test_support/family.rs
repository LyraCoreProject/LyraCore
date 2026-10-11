//! Pieces the handler-level family tests share: a session connection to run a handler against, and
//! typed reply flattening. A family test owns its Fake and its `run` helper.

use super::*;

/// A connection for `account_id` in the world as `self_guid`. The session's routed Store is
/// never read here: every handler under test takes its family Fake as an argument.
pub(crate) fn in_world_conn(account_id: u64, self_guid: u64) -> WorldConn {
    let username = NormalizedString::new("TESTER").unwrap();
    let server_seed = ProofSeed::new();
    let client_seed = ProofSeed::new();
    let client_seed_value = client_seed.seed();
    let (client_proof, _client_crypto) =
        client_seed.into_client_header_crypto(&username, K, server_seed.seed());
    let (_encrypt, decrypt) = server_seed
        .into_server_header_crypto(&username, K, client_proof, client_seed_value)
        .map(|crypto| crypto.split())
        .unwrap_or_else(|_| panic!("the proof was made for the same key"));
    WorldConn {
        session_claim: None,
        protocol: ProtocolSession::in_world(account_id, self_guid),
        decrypt,
        move_coalesce: Default::default(),
        unavailable_notices: Default::default(),
        store: RoutedStore::new(Arc::new(WorldFake::default())),
        session_key: None,
        guild_signed_on: None,
        move_desync_drops: 0,
        chat_flood: Default::default(),
    }
}

/// Flatten typed replies while preserving their protocol order.
pub(crate) fn outbound_messages(outbound: Vec<Outbound>) -> Vec<ServerOpcodeMessage> {
    let mut sent = Vec::new();
    for out in outbound {
        match out {
            Outbound::One(message) => sent.push(message),
            Outbound::Batch(messages) => sent.extend(messages),
            Outbound::Raw { opcode, .. } => panic!("unexpected raw packet {opcode:#06x}"),
            Outbound::Job(_) => panic!("unexpected deferred relay job"),
        }
    }
    sent
}

/// `NpcStore` for a Fake that only needs the Npc to refuse or accept an interaction. Every other
/// method panics, so a handler that starts to read more fails the test loudly.
macro_rules! npc_store_refusing_by {
    ($fake:ty, $refuses:ident) => {
        impl NpcStore for $fake {
            fn npc_refuses_interaction(&self, _npc_guid: u64, _player_guid: u64) -> Result<bool> {
                Ok(self.$refuses)
            }

            fn creature_template(&self, _entry: u32) -> Result<Option<codec::CreatureView>> {
                unimplemented!("creature_template")
            }

            fn pet_name(
                &self,
                _requester: Actor,
                _pet_number: u32,
                _pet_guid: u64,
            ) -> Result<Option<codec::PetNameView>> {
                unimplemented!("pet_name")
            }

            fn gameobject_template(
                &self,
                _entry: u32,
            ) -> Result<Option<codec::GameObjectTemplateView>> {
                unimplemented!("gameobject_template")
            }

            fn gameobject_type(&self, _go_guid: u64) -> Result<Option<u8>> {
                unimplemented!("gameobject_type")
            }

            fn enter_areatrigger(&self, _actor: Actor, _trigger_id: u32) -> Result<()> {
                unimplemented!("enter_areatrigger")
            }

            fn bind_home(&self, _actor: Actor, _innkeeper_guid: u64) -> Result<InteractionOutcome> {
                unimplemented!("bind_home")
            }

            fn npc_is_innkeeper(&self, _guid: u64) -> Result<bool> {
                unimplemented!("npc_is_innkeeper")
            }

            fn npc_gossip_text_id(&self, _npc_guid: u64) -> u32 {
                unimplemented!("npc_gossip_text_id")
            }

            fn npc_text_for_id(&self, _text_id: u32) -> Option<codec::NpcTextView> {
                unimplemented!("npc_text_for_id")
            }

            fn gossip_options(&self, _npc_guid: u64) -> Result<Vec<codec::GossipOptionView>> {
                unimplemented!("gossip_options")
            }

            fn inspect(&self, _actor: Actor, _target_guid: u64) -> Result<()> {
                unimplemented!("inspect")
            }

            fn gossip_select(
                &self,
                _actor: Actor,
                _npc_guid: u64,
                _option_id: u32,
                _option_row_id: u32,
            ) -> Result<()> {
                unimplemented!("gossip_select")
            }
        }
    };
}
pub(crate) use npc_store_refusing_by;
