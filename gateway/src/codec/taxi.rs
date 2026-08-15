//! Vanilla taxi query replies. Gameplay has already selected the source and available client node
//! ids; this layer only maps those facts to the fixed 256-bit build-5875 wire representation.

use super::*;
use wow_world_messages::vanilla::{
    ActivateTaxiReply, SMSG_ACTIVATETAXIREPLY, SMSG_SHOWTAXINODES, SMSG_TAXINODE_STATUS,
};

pub const TAXI_MASK_WORDS: usize =
    lyracore_shared::constants::taxi_protocol::NODE_MASK_WORDS as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaxiNodeStatusView {
    pub npc_guid: u64,
    pub known: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaxiMapView {
    pub npc_guid: u64,
    pub source_client_node_id: u32,
    pub available_client_node_ids: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaxiActivationResult {
    pub result_code: u8,
}

impl Default for TaxiActivationResult {
    fn default() -> Self {
        Self {
            result_code:
                lyracore_shared::constants::taxi_protocol::ACTIVATE_UNSPECIFIED_SERVER_ERROR,
        }
    }
}

/// Map the module's stable primitive result onto the closest vanilla 5875 reply. Unknown future
/// values fail closed as a server error; refusal prose is diagnostic only and never enters policy.
pub fn activate_taxi_reply(result_code: u8) -> ActivateTaxiReply {
    use lyracore_shared::constants::taxi_protocol as result;
    match result_code {
        result::ACTIVATE_OK => ActivateTaxiReply::Ok,
        result::ACTIVATE_NO_SUCH_PATH => ActivateTaxiReply::NoSuchPath,
        result::ACTIVATE_NOT_ENOUGH_MONEY => ActivateTaxiReply::NotEnoughMoney,
        result::ACTIVATE_TOO_FAR_AWAY => ActivateTaxiReply::TooFarAway,
        result::ACTIVATE_NO_VENDOR_NEARBY => ActivateTaxiReply::NoVendorNearby,
        result::ACTIVATE_NOT_VISITED => ActivateTaxiReply::NotVisited,
        result::ACTIVATE_PLAYER_BUSY => ActivateTaxiReply::PlayerBusy,
        result::ACTIVATE_PLAYER_ALREADY_MOUNTED => ActivateTaxiReply::PlayerAlreadyMounted,
        result::ACTIVATE_PLAYER_SHAPE_SHIFTED => ActivateTaxiReply::PlayerShapeShifted,
        result::ACTIVATE_PLAYER_MOVING => ActivateTaxiReply::PlayerMoving,
        result::ACTIVATE_SAME_NODE => ActivateTaxiReply::SameNode,
        result::ACTIVATE_NOT_STANDING => ActivateTaxiReply::NotStanding,
        _ => ActivateTaxiReply::UnspecifiedServerError,
    }
}

pub fn build_activate_taxi_reply(result: TaxiActivationResult) -> SMSG_ACTIVATETAXIREPLY {
    SMSG_ACTIVATETAXIREPLY {
        reply: activate_taxi_reply(result.result_code),
    }
}

/// Pack one-based client node ids into exactly eight little-endian u32 words. Invalid ids are
/// ignored defensively; imported data and module replies already enforce 1..=256.
pub fn build_taxi_mask(ids: impl IntoIterator<Item = u32>) -> [u32; TAXI_MASK_WORDS] {
    let mut words = [0u32; TAXI_MASK_WORDS];
    for id in ids {
        if !(lyracore_shared::constants::taxi_protocol::CLIENT_NODE_ID_MIN
            ..=lyracore_shared::constants::taxi_protocol::CLIENT_NODE_ID_MAX)
            .contains(&id)
        {
            continue;
        }
        let bit = id - 1;
        words[(bit / 32) as usize] |= 1u32 << (bit % 32);
    }
    words
}

pub fn build_taxi_node_status(view: TaxiNodeStatusView) -> SMSG_TAXINODE_STATUS {
    SMSG_TAXINODE_STATUS {
        guid: Guid::new(view.npc_guid),
        taxi_mask_node_known: view.known,
    }
}

pub fn build_show_taxi_nodes(view: &TaxiMapView) -> SMSG_SHOWTAXINODES {
    SMSG_SHOWTAXINODES {
        unknown1: 1,
        guid: Guid::new(view.npc_guid),
        nearest_node: view.source_client_node_id,
        nodes: build_taxi_mask(view.available_client_node_ids.iter().copied()).to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::ServerMessage;

    #[test]
    fn one_based_node_ids_cross_word_boundaries_correctly() {
        let mask = build_taxi_mask([0, 1, 32, 33, 255, 256, 257]);
        assert_eq!(mask.len(), 8);
        assert_eq!(mask[0], 0x8000_0001);
        assert_eq!(mask[1], 0x0000_0001);
        assert_eq!(mask[6], 0);
        assert_eq!(mask[7], 0xC000_0000);
    }

    #[test]
    fn status_reply_round_trips_npc_and_known_bit() {
        let message = build_taxi_node_status(TaxiNodeStatusView {
            npc_guid: 0xF130_0000_0000_0042,
            known: true,
        });
        let mut framed = Vec::new();
        message.write_unencrypted_server(&mut framed).unwrap();
        match ServerOpcodeMessage::read_unencrypted(&mut framed.as_slice()).unwrap() {
            ServerOpcodeMessage::SMSG_TAXINODE_STATUS(decoded) => assert_eq!(*decoded, message),
            other => panic!("expected SMSG_TAXINODE_STATUS, got {other}"),
        }
    }

    #[test]
    fn show_nodes_is_always_the_vanilla_eight_word_mask() {
        let message = build_show_taxi_nodes(&TaxiMapView {
            npc_guid: 99,
            source_client_node_id: 255,
            available_client_node_ids: vec![255, 256],
        });
        assert_eq!(message.unknown1, 1);
        assert_eq!(message.nearest_node, 255);
        assert_eq!(message.nodes.len(), 8);
        assert_eq!(message.nodes[7], 0xC000_0000);

        let mut framed = Vec::new();
        message.write_unencrypted_server(&mut framed).unwrap();
        match ServerOpcodeMessage::read_unencrypted(&mut framed.as_slice()).unwrap() {
            ServerOpcodeMessage::SMSG_SHOWTAXINODES(decoded) => assert_eq!(*decoded, message),
            other => panic!("expected SMSG_SHOWTAXINODES, got {other}"),
        }
    }

    #[test]
    fn every_stable_activation_result_maps_without_prose() {
        use lyracore_shared::constants::taxi_protocol as result;
        let cases = [
            (result::ACTIVATE_OK, ActivateTaxiReply::Ok),
            (
                result::ACTIVATE_UNSPECIFIED_SERVER_ERROR,
                ActivateTaxiReply::UnspecifiedServerError,
            ),
            (result::ACTIVATE_NO_SUCH_PATH, ActivateTaxiReply::NoSuchPath),
            (
                result::ACTIVATE_NOT_ENOUGH_MONEY,
                ActivateTaxiReply::NotEnoughMoney,
            ),
            (result::ACTIVATE_TOO_FAR_AWAY, ActivateTaxiReply::TooFarAway),
            (
                result::ACTIVATE_NO_VENDOR_NEARBY,
                ActivateTaxiReply::NoVendorNearby,
            ),
            (result::ACTIVATE_NOT_VISITED, ActivateTaxiReply::NotVisited),
            (result::ACTIVATE_PLAYER_BUSY, ActivateTaxiReply::PlayerBusy),
            (
                result::ACTIVATE_PLAYER_ALREADY_MOUNTED,
                ActivateTaxiReply::PlayerAlreadyMounted,
            ),
            (
                result::ACTIVATE_PLAYER_SHAPE_SHIFTED,
                ActivateTaxiReply::PlayerShapeShifted,
            ),
            (
                result::ACTIVATE_PLAYER_MOVING,
                ActivateTaxiReply::PlayerMoving,
            ),
            (result::ACTIVATE_SAME_NODE, ActivateTaxiReply::SameNode),
            (
                result::ACTIVATE_NOT_STANDING,
                ActivateTaxiReply::NotStanding,
            ),
        ];
        for (code, expected) in cases {
            assert_eq!(activate_taxi_reply(code), expected, "code {code}");
        }
        assert_eq!(
            activate_taxi_reply(u8::MAX),
            ActivateTaxiReply::UnspecifiedServerError
        );
    }

    #[test]
    fn activation_reply_round_trips_the_vanilla_result() {
        let message = build_activate_taxi_reply(TaxiActivationResult {
            result_code: lyracore_shared::constants::taxi_protocol::ACTIVATE_NOT_ENOUGH_MONEY,
        });
        let mut framed = Vec::new();
        message.write_unencrypted_server(&mut framed).unwrap();
        match ServerOpcodeMessage::read_unencrypted(&mut framed.as_slice()).unwrap() {
            ServerOpcodeMessage::SMSG_ACTIVATETAXIREPLY(decoded) => {
                assert_eq!(decoded, message);
                assert_eq!(decoded.reply, ActivateTaxiReply::NotEnoughMoney);
            }
            other => panic!("expected SMSG_ACTIVATETAXIREPLY, got {other}"),
        }
    }
}
