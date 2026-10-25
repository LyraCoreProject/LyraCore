use anyhow::{anyhow, Result};
use lyracore_protocol::{
    codec,
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    world::Packet,
};

fn message(value: ServerOpcodeMessage) -> Packet {
    lyracore_protocol::world::encode(value)
        .expect("bounded unavailable-service response fits a world frame")
}

fn word(opcode: u16, value: u32) -> Packet {
    Packet {
        opcode,
        body: value.to_le_bytes().to_vec(),
    }
}

pub fn responses(msg: &ClientOpcodeMessage, notices: &mut u8) -> Option<Vec<Packet>> {
    let notice = match msg {
        ClientOpcodeMessage::CMSG_SET_ACTIONBAR_TOGGLES(_) => {
            Some((1, "Action bar visibility preferences"))
        }
        ClientOpcodeMessage::CMSG_TUTORIAL_FLAG(_)
        | ClientOpcodeMessage::CMSG_TUTORIAL_CLEAR
        | ClientOpcodeMessage::CMSG_TUTORIAL_RESET => Some((2, "Tutorial preferences")),
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
        ClientOpcodeMessage::CMSG_GMTICKET_SYSTEMSTATUS => word(0x021b, 0),
        ClientOpcodeMessage::CMSG_GMTICKET_GETTICKET => word(0x0212, 10),
        ClientOpcodeMessage::CMSG_GMTICKET_DELETETICKET => word(0x0218, 9),
        ClientOpcodeMessage::MSG_LIST_STABLED_PETS(_)
        | ClientOpcodeMessage::CMSG_STABLE_PET(_)
        | ClientOpcodeMessage::CMSG_UNSTABLE_PET(_)
        | ClientOpcodeMessage::CMSG_BUY_STABLE_SLOT(_)
        | ClientOpcodeMessage::CMSG_STABLE_SWAP_PET(_) => Packet {
            opcode: 0x0273,
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
                opcode: 0x023d,
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
            opcode: 0x02e0,
            body: vec![0; 5],
        },
        ClientOpcodeMessage::MSG_BATTLEGROUND_PLAYER_POSITIONS => Packet {
            opcode: 0x02e9,
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
                    opcode: 0x0138,
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
        ClientOpcodeMessage::CMSG_RESET_INSTANCES => unavailable("Instance reset requests"),
        ClientOpcodeMessage::CMSG_REQUEST_RAID_INFO => unavailable("Raid lockout queries"),
        ClientOpcodeMessage::MSG_INSPECT_HONOR_STATS(_) => unavailable("Honor statistics"),
        ClientOpcodeMessage::CMSG_BUY_ITEM_IN_SLOT(_) => {
            message(ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(
                Box::new(codec::build_inventory_refusal(
                    lyracore_shared::item::ItemRefusal::NotRightNow,
                    0,
                    0,
                )),
            ))
        }
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
                opcode: 0x02d4,
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

pub fn raw_responses(opcode: u32, body: &[u8]) -> Result<Option<Vec<Packet>>> {
    match opcode {
        0x0205 => {
            anyhow::ensure!(body.len() >= 19, "short CMSG_GMTICKET_CREATE body");
            let tail = after_cstring(after_cstring(&body[17..])?)?;
            anyhow::ensure!(
                tail.is_empty() || (body[0] == 2 && tail.len() >= 8),
                "invalid CMSG_GMTICKET_CREATE transcript"
            );
            Ok(Some(vec![word(0x0206, 3)]))
        }
        0x0207 => {
            anyhow::ensure!(body.len() >= 2, "short CMSG_GMTICKET_UPDATETEXT body");
            anyhow::ensure!(
                after_cstring(&body[1..])?.is_empty(),
                "invalid CMSG_GMTICKET_UPDATETEXT body"
            );
            Ok(Some(vec![word(0x0208, 5)]))
        }
        0x023e => {
            anyhow::ensure!(
                body.len() == 9 && body[8] <= 1,
                "invalid CMSG_BATTLEFIELD_JOIN body"
            );
            Ok(Some(battlefield_refusal()))
        }
        0x00f9 => {
            anyhow::ensure!(body.is_empty(), "invalid CMSG_OPENING_CINEMATIC body");
            Ok(Some(vec![unavailable("Opening cinematics")]))
        }
        0x0200 => {
            anyhow::ensure!(
                body.len() >= 13
                    && body.last() == Some(&0)
                    && !body[12..body.len() - 1].contains(&0),
                "invalid CMSG_SET_LOOKING_FOR_GROUP body"
            );
            std::str::from_utf8(&body[12..body.len() - 1])?;
            Ok(Some(vec![unavailable("Looking-for-group preferences")]))
        }
        0x0312 => {
            anyhow::ensure!(body.len() >= 16, "short CMSG_ACTIVATETAXIEXPRESS body");
            let count = u32::from_le_bytes(body[12..16].try_into()?);
            anyhow::ensure!(
                u64::from(count) * 4 + 16 == body.len() as u64,
                "invalid CMSG_ACTIVATETAXIEXPRESS node count"
            );
            Ok(Some(vec![word(0x01ae, 1)]))
        }
        0x0317 => {
            anyhow::ensure!(
                body.len() == 5 && body[4] <= 1,
                "invalid CMSG_SET_FACTION_INACTIVE body"
            );
            Ok(Some(vec![unavailable("Reputation display preferences")]))
        }
        // Unhandled in vmangos and cmangos: ground-item and loot-slot stores, skill-point purchases, lottery.
        0x0107 | 0x0109 | 0x021f | 0x0220 | 0x0336 => Ok(Some(vec![])),
        0x005a => {
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
        opcode: 0x005b,
        body,
    }
}

pub fn is_control_receipt(msg: &ClientOpcodeMessage) -> bool {
    matches!(
        msg,
        ClientOpcodeMessage::MSG_MOVE_TELEPORT_ACK(_)
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
    )
}
