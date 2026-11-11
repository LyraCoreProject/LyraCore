use anyhow::{anyhow, Result};
use lyracore_protocol::{
    codec::{self, *},
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    world::Packet,
};
use spacetimedb::{ConnectionId, ReducerContext};

use super::bounded as message;

/// Ignores an obsolete control or answers an unavailable service from the raw opcode, before any
/// decode. Returns whether it did.
pub(super) fn intercept(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    opcode: u32,
    body: &[u8],
) -> Result<bool, String> {
    if is_obsolete(opcode) {
        return Ok(true);
    }
    let Some(packets) = raw_responses(opcode, body).map_err(|error| error.to_string())? else {
        return Ok(false);
    };
    for packet in packets {
        super::send_raw(ctx, connection_id, packet.opcode, packet.body, false);
    }
    Ok(true)
}

// Obsolete controls are discarded before readers inspect untrusted sizes, compressed data or enums.
fn is_obsolete(opcode: u32) -> bool {
    matches!(
        opcode,
        CMSG_WORLD_TELEPORT_OPCODE
            | CMSG_TELEPORT_TO_UNIT_OPCODE
            | CMSG_ZONE_MAP_OPCODE
            | CMSG_UNDRESSPLAYER_OPCODE
            | MSG_MOVE_TELEPORT_CHEAT_OPCODE
            | MSG_MOVE_TOGGLE_COLLISION_CHEAT_OPCODE
            | CMSG_MOVE_SET_RAW_POSITION_OPCODE
            | CMSG_ZONEUPDATE_OPCODE
            | CMSG_UPDATE_ACCOUNT_DATA_OPCODE
            | CMSG_GM_REQUEST_PLAYER_INFO_OPCODE
            | MSG_MOVE_TOGGLE_GRAVITY_CHEAT_OPCODE
            | CMSG_MOVE_FALL_RESET_OPCODE
            | CMSG_MINIGAME_MOVE_OPCODE
            | CMSG_CHAT_FILTERED_OPCODE
            | CMSG_LOTTERY_QUERY_OBSOLETE_OPCODE
    )
}

fn word(opcode: u16, value: u32) -> Packet {
    Packet {
        opcode,
        body: value.to_le_bytes().to_vec(),
    }
}

pub(super) fn responses(msg: &ClientOpcodeMessage, notices: &mut u8) -> Option<Vec<Packet>> {
    let notice = match msg {
        ClientOpcodeMessage::CMSG_SET_ACTIONBAR_TOGGLES(_) => {
            Some((1, "Action bar visibility preferences"))
        }
        ClientOpcodeMessage::CMSG_TUTORIAL_RESET => Some((2, "Tutorial resets")),
        _ => None,
    };
    if let Some((notice, service)) = notice {
        return Some(if *notices & notice == 0 {
            *notices |= notice;
            vec![unavailable(service)]
        } else {
            vec![]
        });
    }
    let response = match msg {
        // SMSG_TUTORIAL_FLAGS at login already marks every tutorial seen.
        ClientOpcodeMessage::CMSG_TUTORIAL_FLAG(_) | ClientOpcodeMessage::CMSG_TUTORIAL_CLEAR => {
            return Some(vec![])
        }
        ClientOpcodeMessage::CMSG_GMTICKET_SYSTEMSTATUS => {
            word(SMSG_GMTICKET_SYSTEMSTATUS_OPCODE, 0)
        }
        ClientOpcodeMessage::CMSG_GMTICKET_GETTICKET => word(SMSG_GMTICKET_GETTICKET_OPCODE, 10),
        ClientOpcodeMessage::CMSG_GMTICKET_DELETETICKET => {
            word(SMSG_GMTICKET_DELETETICKET_OPCODE, 9)
        }
        ClientOpcodeMessage::MSG_LIST_STABLED_PETS(_)
        | ClientOpcodeMessage::CMSG_STABLE_PET(_)
        | ClientOpcodeMessage::CMSG_UNSTABLE_PET(_)
        | ClientOpcodeMessage::CMSG_BUY_STABLE_SLOT(_)
        | ClientOpcodeMessage::CMSG_STABLE_SWAP_PET(_) => Packet {
            opcode: SMSG_STABLE_RESULT_OPCODE,
            body: vec![6],
        },
        ClientOpcodeMessage::CMSG_OPEN_ITEM(_) | ClientOpcodeMessage::CMSG_WRAP_ITEM(_) => {
            message(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                Box::new(codec::build_inventory_refusal(
                    lyracore_shared::item::ItemRefusal::NotRightNow,
                    0,
                    0,
                )),
            ))
        }
        ClientOpcodeMessage::CMSG_BATTLEFIELD_LIST(request) => {
            let mut body = 0u64.to_le_bytes().to_vec();
            body.extend_from_slice(&request.map.as_int().to_le_bytes());
            body.extend_from_slice(&[0; 5]);
            Packet {
                opcode: SMSG_BATTLEFIELD_LIST_OPCODE,
                body,
            }
        }
        ClientOpcodeMessage::CMSG_BATTLEFIELD_STATUS
        | ClientOpcodeMessage::CMSG_LEAVE_BATTLEFIELD(_) => return Some(empty_battlefield_status()),
        ClientOpcodeMessage::CMSG_BATTLEMASTER_JOIN(_)
        | ClientOpcodeMessage::CMSG_BATTLEFIELD_PORT(_) => return Some(battlefield_refusal()),
        ClientOpcodeMessage::CMSG_BATTLEMASTER_HELLO(_)
        | ClientOpcodeMessage::CMSG_AREA_SPIRIT_HEALER_QUERY(_)
        | ClientOpcodeMessage::CMSG_AREA_SPIRIT_HEALER_QUEUE(_) => unavailable("Battlegrounds"),
        ClientOpcodeMessage::MSG_PVP_LOG_DATA => Packet {
            opcode: MSG_PVP_LOG_DATA_OPCODE,
            body: vec![0; 5],
        },
        ClientOpcodeMessage::MSG_BATTLEGROUND_PLAYER_POSITIONS => Packet {
            opcode: MSG_BATTLEGROUND_PLAYER_POSITIONS_OPCODE,
            body: vec![0; 5],
        },
        ClientOpcodeMessage::CMSG_PET_SET_ACTION(_) => unavailable("Pet action bar changes"),
        ClientOpcodeMessage::CMSG_PET_ABANDON(_) => unavailable("Pet abandonment requests"),
        ClientOpcodeMessage::CMSG_PET_STOP_ATTACK(_) => unavailable("Pet stop-attack requests"),
        ClientOpcodeMessage::CMSG_PET_CANCEL_AURA(_) => unavailable("Pet aura cancellations"),
        ClientOpcodeMessage::CMSG_PET_UNLEARN(_) => unavailable("Pet talent resets"),
        ClientOpcodeMessage::CMSG_PET_SPELL_AUTOCAST(_) => unavailable("Pet autocast changes"),
        // SMSG_PET_CAST_FAILED: status 2 (failed), reason 0x17 (not reported).
        ClientOpcodeMessage::CMSG_PET_CAST_SPELL(request) => {
            let mut body = request.id.to_le_bytes().to_vec();
            body.extend_from_slice(&[0x02, 0x17]);
            return Some(vec![
                Packet {
                    opcode: SMSG_PET_CAST_FAILED_OPCODE,
                    body,
                },
                unavailable("Pet spell casts"),
            ]);
        }
        ClientOpcodeMessage::CMSG_UNLEARN_SKILL(_) => unavailable("Skill unlearning requests"),
        ClientOpcodeMessage::CMSG_SET_AMMO(_) => unavailable("Ammunition selections"),
        ClientOpcodeMessage::CMSG_MOUNTSPECIAL_ANIM => unavailable("Mount flourishes"),
        ClientOpcodeMessage::CMSG_TOGGLE_PVP(_) => unavailable("PvP flag changes"),
        ClientOpcodeMessage::CMSG_TOGGLE_HELM | ClientOpcodeMessage::CMSG_TOGGLE_CLOAK => {
            unavailable("Equipment visibility preferences")
        }
        ClientOpcodeMessage::CMSG_FAR_SIGHT(_) => unavailable("Remote camera requests"),
        ClientOpcodeMessage::CMSG_SUMMON_RESPONSE(_) => unavailable("Summon confirmations"),
        ClientOpcodeMessage::CMSG_QUEST_CONFIRM_ACCEPT(_) => {
            unavailable("Quest sharing confirmations")
        }
        ClientOpcodeMessage::MSG_QUEST_PUSH_RESULT(_) => unavailable("Quest sharing responses"),
        // Instance bindings end with their Instance, so no character holds a raid lockout.
        ClientOpcodeMessage::CMSG_REQUEST_RAID_INFO => {
            message(ServerOpcodeMessage::SMSG_RAID_INSTANCE_INFO(Box::default()))
        }
        ClientOpcodeMessage::MSG_INSPECT_HONOR_STATS(_) => unavailable("Honor statistics"),
        _ => return None,
    };
    Some(vec![response])
}

fn unavailable(service: &str) -> Packet {
    message(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
        codec::build_gm_system_message(format!("{service} are not available on this realm.")),
    )))
}

fn empty_battlefield_status() -> Vec<Packet> {
    (0u32..3)
        .map(|slot| {
            let mut body = slot.to_le_bytes().to_vec();
            body.extend_from_slice(&0u32.to_le_bytes());
            Packet {
                opcode: SMSG_BATTLEFIELD_STATUS_OPCODE,
                body,
            }
        })
        .collect()
}

fn battlefield_refusal() -> Vec<Packet> {
    let mut replies = vec![unavailable("Battlegrounds")];
    replies.extend(empty_battlefield_status());
    replies
}

fn raw_responses(opcode: u32, body: &[u8]) -> Result<Option<Vec<Packet>>> {
    match opcode {
        CMSG_GMTICKET_CREATE_OPCODE => {
            anyhow::ensure!(body.len() >= 19, "short CMSG_GMTICKET_CREATE body");
            let tail = after_cstring(after_cstring(&body[17..])?)?;
            anyhow::ensure!(
                tail.is_empty() || (body[0] == 2 && tail.len() >= 8),
                "invalid CMSG_GMTICKET_CREATE transcript"
            );
            Ok(Some(vec![word(SMSG_GMTICKET_CREATE_OPCODE, 3)]))
        }
        CMSG_GMTICKET_UPDATETEXT_OPCODE => {
            anyhow::ensure!(body.len() >= 2, "short CMSG_GMTICKET_UPDATETEXT body");
            anyhow::ensure!(
                after_cstring(&body[1..])?.is_empty(),
                "invalid CMSG_GMTICKET_UPDATETEXT body"
            );
            Ok(Some(vec![word(SMSG_GMTICKET_UPDATETEXT_OPCODE, 5)]))
        }
        CMSG_BATTLEFIELD_JOIN_OPCODE => {
            anyhow::ensure!(
                body.len() == 9 && body[8] <= 1,
                "invalid CMSG_BATTLEFIELD_JOIN body"
            );
            Ok(Some(battlefield_refusal()))
        }
        CMSG_OPENING_CINEMATIC_OPCODE => {
            anyhow::ensure!(body.is_empty(), "invalid CMSG_OPENING_CINEMATIC body");
            Ok(Some(vec![unavailable("Opening cinematics")]))
        }
        CMSG_SET_LOOKING_FOR_GROUP_OPCODE => {
            anyhow::ensure!(
                body.len() >= 13
                    && body.last() == Some(&0)
                    && !body[12..body.len() - 1].contains(&0),
                "invalid CMSG_SET_LOOKING_FOR_GROUP body"
            );
            std::str::from_utf8(&body[12..body.len() - 1])?;
            Ok(Some(vec![unavailable("Looking-for-group preferences")]))
        }
        CMSG_ACTIVATETAXIEXPRESS_OPCODE => {
            anyhow::ensure!(body.len() >= 16, "short CMSG_ACTIVATETAXIEXPRESS body");
            let count = u32::from_le_bytes(body[12..16].try_into()?);
            anyhow::ensure!(
                u64::from(count) * 4 + 16 == body.len() as u64,
                "invalid CMSG_ACTIVATETAXIEXPRESS node count"
            );
            Ok(Some(vec![word(SMSG_ACTIVATETAXIREPLY_OPCODE, 1)]))
        }
        CMSG_SET_FACTION_INACTIVE_OPCODE => {
            anyhow::ensure!(
                body.len() == 5 && body[4] <= 1,
                "invalid CMSG_SET_FACTION_INACTIVE body"
            );
            Ok(Some(vec![unavailable("Reputation display preferences")]))
        }
        // Unhandled in vmangos and cmangos: ground-item and loot-slot stores, skill-point purchases, lottery.
        CMSG_AUTOSTORE_GROUND_ITEM_OPCODE
        | CMSG_STORE_LOOT_IN_SLOT_OPCODE
        | CMSG_SKILL_BUY_STEP_OPCODE
        | CMSG_SKILL_BUY_RANK_OPCODE
        | CMSG_BUY_LOTTERY_TICKET_OBSOLETE_OPCODE => Ok(Some(vec![])),
        CMSG_PAGE_TEXT_QUERY_OPCODE => {
            anyhow::ensure!(body.len() == 12, "invalid CMSG_PAGE_TEXT_QUERY body");
            Ok(Some(vec![page_unavailable(u32::from_le_bytes(
                body[..4].try_into()?,
            ))]))
        }
        _ => Ok(None),
    }
}

fn after_cstring(body: &[u8]) -> Result<&[u8]> {
    let end = body
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| anyhow!("unterminated ticket text"))?;
    std::str::from_utf8(&body[..end])?;
    Ok(&body[end + 1..])
}

fn page_unavailable(page_id: u32) -> Packet {
    let mut body = page_id.to_le_bytes().to_vec();
    body.extend_from_slice(b"Page text is not available on this realm.\0");
    body.extend_from_slice(&0u32.to_le_bytes());
    Packet {
        opcode: SMSG_PAGE_TEXT_QUERY_RESPONSE_OPCODE,
        body,
    }
}

/// In-world requests the Realm accepts without an answer.
pub(super) fn is_ignored(msg: &ClientOpcodeMessage) -> bool {
    matches!(
        msg,
        ClientOpcodeMessage::MSG_MOVE_TELEPORT_ACK(_)
            | ClientOpcodeMessage::CMSG_FORCE_RUN_SPEED_CHANGE_ACK(_)
            | ClientOpcodeMessage::CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK(_)
            | ClientOpcodeMessage::CMSG_FORCE_SWIM_SPEED_CHANGE_ACK(_)
            | ClientOpcodeMessage::CMSG_MOVE_KNOCK_BACK_ACK(_)
            | ClientOpcodeMessage::CMSG_MOVE_HOVER_ACK(_)
            | ClientOpcodeMessage::CMSG_MOVE_FEATHER_FALL_ACK(_)
            | ClientOpcodeMessage::CMSG_FORCE_WALK_SPEED_CHANGE_ACK(_)
            | ClientOpcodeMessage::CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK(_)
            | ClientOpcodeMessage::CMSG_FORCE_TURN_RATE_CHANGE_ACK(_)
            | ClientOpcodeMessage::CMSG_MOVE_SPLINE_DONE(_)
            | ClientOpcodeMessage::CMSG_MOVE_TIME_SKIPPED(_)
            | ClientOpcodeMessage::CMSG_MOVE_NOT_ACTIVE_MOVER(_)
            | ClientOpcodeMessage::CMSG_NEXT_CINEMATIC_CAMERA
            | ClientOpcodeMessage::CMSG_COMPLETE_CINEMATIC
            | ClientOpcodeMessage::CMSG_CANCEL_GROWTH_AURA
            | ClientOpcodeMessage::CMSG_REQUEST_PET_INFO
    )
}
