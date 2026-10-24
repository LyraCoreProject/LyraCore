use super::*;
use wow_world_messages::vanilla::{
    UnitStandState, SMSG_CHARACTER_LOGIN_FAILED, SMSG_CHAR_RENAME, SMSG_STANDSTATE_UPDATE,
};

#[derive(Clone, Debug, Default)]
pub struct CharacterView {
    pub guid: u64,
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
    pub level: u8,
    pub map_id: u32,
    pub zone_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub first_login: bool,
    pub equipment: [CharacterGear; 19],
    /// Whole seconds excluding the current World Session.
    pub played_total_secs: u32,
    /// Unix-epoch microseconds; zero means offline.
    pub session_start_micros: u64,
    pub guild_id: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Appearance {
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharCreateOutcome {
    Success,
    NameInUse,
    ServerLimit,
    Failed,
}

pub fn build_char_create_response(outcome: CharCreateOutcome) -> SMSG_CHAR_CREATE {
    SMSG_CHAR_CREATE {
        result: match outcome {
            CharCreateOutcome::Success => WorldResult::CharCreateSuccess,
            CharCreateOutcome::NameInUse => WorldResult::CharCreateNameInUse,
            CharCreateOutcome::ServerLimit => WorldResult::CharCreateServerLimit,
            CharCreateOutcome::Failed => WorldResult::CharCreateError,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharDeleteOutcome {
    Success,
    Failed,
}

pub fn build_char_delete_response(outcome: CharDeleteOutcome) -> SMSG_CHAR_DELETE {
    SMSG_CHAR_DELETE {
        result: match outcome {
            CharDeleteOutcome::Success => WorldResult::CharDeleteSuccess,
            CharDeleteOutcome::Failed => WorldResult::CharDeleteFailed,
        },
    }
}

pub fn build_char_enum(chars: &[CharacterView]) -> Result<SMSG_CHAR_ENUM> {
    let mut ordered: Vec<&CharacterView> = chars.iter().collect();
    ordered.sort_unstable_by_key(|c| c.guid);
    let mut characters = Vec::with_capacity(ordered.len());
    for c in ordered {
        characters.push(Character {
            guid: Guid::new(c.guid),
            name: c.name.clone(),
            race: Race::try_from(c.race).map_err(|_| anyhow!("invalid race {}", c.race))?,
            class: Class::try_from(c.class).map_err(|_| anyhow!("invalid class {}", c.class))?,
            gender: Gender::try_from(c.gender)
                .map_err(|_| anyhow!("invalid gender {}", c.gender))?,
            skin: c.skin,
            face: c.face,
            hair_style: c.hair_style,
            hair_color: c.hair_color,
            facial_hair: c.facial_hair,
            level: Level::new(c.level),
            area: Area::try_from(c.zone_id).unwrap_or_default(),
            map: Map::try_from(c.map_id).map_err(|_| anyhow!("invalid map {}", c.map_id))?,
            position: Vector3d {
                x: c.x,
                y: c.y,
                z: c.z,
            },
            guild_id: c.guild_id,
            flags: CharacterFlags::empty(),
            first_login: c.first_login,
            pet_display_id: 0,
            pet_level: Level::new(0),
            pet_family: CreatureFamily::default(),
            equipment: c.equipment,
        });
    }
    Ok(SMSG_CHAR_ENUM { characters })
}

pub fn build_name_query_response(c: &CharacterView) -> Result<SMSG_NAME_QUERY_RESPONSE> {
    Ok(SMSG_NAME_QUERY_RESPONSE {
        guid: Guid::new(c.guid),
        character_name: c.name.clone(),
        realm_name: String::new(),
        race: Race::try_from(c.race).map_err(|_| anyhow!("invalid race {}", c.race))?,
        gender: Gender::try_from(c.gender).map_err(|_| anyhow!("invalid gender {}", c.gender))?,
        class: Class::try_from(c.class).map_err(|_| anyhow!("invalid class {}", c.class))?,
    })
}

/// The client renders inspection from the target's existing visible-item fields.
pub fn build_inspect_response(target_guid: u64) -> SMSG_INSPECT {
    SMSG_INSPECT {
        guid: Guid::new(target_guid),
    }
}

pub fn logout_sequence() -> Vec<ServerOpcodeMessage> {
    vec![
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(SMSG_LOGOUT_RESPONSE {
            result: LogoutResult::Success,
            speed: LogoutSpeed::Instant,
        }),
        ServerOpcodeMessage::SMSG_LOGOUT_COMPLETE,
    ]
}

pub fn logout_denied_in_combat() -> SMSG_LOGOUT_RESPONSE {
    SMSG_LOGOUT_RESPONSE {
        result: LogoutResult::FailureInCombat,
        speed: LogoutSpeed::Instant,
    }
}

/// The client shows CHAR_RENAME_FAILED for any result except 0 and 0x31.
pub fn build_char_rename_refusal() -> SMSG_CHAR_RENAME {
    SMSG_CHAR_RENAME::CharCreateError
}

pub fn build_character_login_failed() -> SMSG_CHARACTER_LOGIN_FAILED {
    SMSG_CHARACTER_LOGIN_FAILED {
        result: WorldResult::ResponseFailure,
    }
}

/// u32 section, u32 size 0, no zlib stream. `None` past the client's eight sections.
pub fn build_empty_account_data(section: u32) -> Option<(u16, Vec<u8>)> {
    const SECTIONS: u32 = 8;
    (section < SECTIONS).then(|| {
        let mut body = section.to_le_bytes().to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        (0x020c, body)
    })
}

/// ChrRaces.dbc CinematicSequenceID (column 16), build 5875.
pub fn opening_cinematic(race: u8) -> Option<u32> {
    match race {
        1 => Some(81),  // Human
        2 => Some(21),  // Orc
        3 => Some(41),  // Dwarf
        4 => Some(61),  // Night Elf
        5 => Some(2),   // Undead
        6 => Some(141), // Tauren
        7 => Some(101), // Gnome
        8 => Some(121), // Troll
        _ => None,
    }
}

pub fn build_trigger_cinematic(sequence: u32) -> (u16, Vec<u8>) {
    (0x00fa, sequence.to_le_bytes().to_vec())
}

/// u32 map, u32 zone, u16 count 0: the client clears its world-state table.
pub fn build_init_world_states(map_id: u32, zone_id: u32) -> (u16, Vec<u8>) {
    let mut body = map_id.to_le_bytes().to_vec();
    body.extend_from_slice(&zone_id.to_le_bytes());
    body.extend_from_slice(&0u16.to_le_bytes());
    (0x02c2, body)
}

pub fn build_stand_state_update(state: UnitStandState) -> SMSG_STANDSTATE_UPDATE {
    SMSG_STANDSTATE_UPDATE { state }
}

/// `Some` when byte 0 of UNIT_FIELD_BYTES_1, the stand state, changed.
pub fn stand_state_changed(
    old_unit_bytes_1: u32,
    new_unit_bytes_1: u32,
) -> Option<SMSG_STANDSTATE_UPDATE> {
    let state = (new_unit_bytes_1 & 0xFF) as u8;
    if state == (old_unit_bytes_1 & 0xFF) as u8 {
        return None;
    }
    UnitStandState::try_from(state)
        .ok()
        .map(build_stand_state_update)
}

pub fn build_player_bound(binder_guid: u64, area_id: u32) -> (u16, Vec<u8>) {
    let mut body = binder_guid.to_le_bytes().to_vec();
    body.extend_from_slice(&area_id.to_le_bytes());
    (0x0158, body)
}

pub fn build_played_time(
    played_total_secs: u32,
    session_start_micros: u64,
    now_micros: u64,
) -> SMSG_PLAYED_TIME {
    let live_secs = if session_start_micros == 0 {
        0
    } else {
        now_micros.saturating_sub(session_start_micros) / 1_000_000
    };
    let total = played_total_secs.saturating_add(live_secs as u32);
    SMSG_PLAYED_TIME {
        total_played_time: total,
        level_played_time: total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use wow_world_messages::vanilla::ServerMessage;

    /// `[size:u16 BE][opcode:u16 LE][body]`, the plaintext frame the World Session writes.
    fn typed_frame(message: &impl ServerMessage) -> Vec<u8> {
        let mut framed = Vec::new();
        message.write_unencrypted_server(&mut framed).unwrap();
        framed
    }

    fn raw_frame((opcode, body): (u16, Vec<u8>)) -> Vec<u8> {
        let mut framed = u16::try_from(body.len() + 2)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        framed.extend_from_slice(&opcode.to_le_bytes());
        framed.extend(body);
        framed
    }

    #[test]
    fn rename_refusal_is_the_char_create_error_byte() {
        assert_eq!(
            typed_frame(&build_char_rename_refusal()),
            [0x00, 0x03, 0xc8, 0x02, 0x2f]
        );
    }

    #[test]
    fn login_failure_is_response_failure() {
        assert_eq!(
            typed_frame(&build_character_login_failed()),
            [0x00, 0x03, 0x41, 0x00, 0x01]
        );
    }

    #[test]
    fn empty_account_data_is_the_section_and_a_zero_size() {
        assert_eq!(
            raw_frame(build_empty_account_data(7).unwrap()),
            [0x00, 0x0a, 0x0c, 0x02, 7, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(build_empty_account_data(8), None);
    }

    #[test]
    fn each_playable_race_has_its_opening_cinematic() {
        let ids: Vec<_> = (1..=8).map(opening_cinematic).collect();
        assert_eq!(ids, [81, 21, 41, 61, 2, 141, 101, 121].map(Some).to_vec());
        assert_eq!(opening_cinematic(0), None);
        assert_eq!(opening_cinematic(9), None);
        assert_eq!(
            raw_frame(build_trigger_cinematic(141)),
            [0x00, 0x06, 0xfa, 0x00, 141, 0, 0, 0]
        );
    }

    #[test]
    fn init_world_states_names_the_map_and_zone_with_no_states() {
        assert_eq!(
            raw_frame(build_init_world_states(0, 12)),
            [0x00, 0x0c, 0xc2, 0x02, 0, 0, 0, 0, 12, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn stand_state_update_is_the_state_byte() {
        assert_eq!(
            typed_frame(&build_stand_state_update(UnitStandState::Sit)),
            [0x00, 0x03, 0x9d, 0x02, 0x01]
        );
    }

    #[test]
    fn only_a_changed_stand_byte_tells_the_character() {
        let ghost = 0x0100_0000;
        assert_eq!(
            stand_state_changed(ghost | 1, ghost),
            Some(build_stand_state_update(UnitStandState::Stand))
        );
        assert_eq!(
            stand_state_changed(0, 3),
            Some(build_stand_state_update(UnitStandState::Sleep))
        );
        assert_eq!(stand_state_changed(1, ghost | 1), None);
    }

    #[test]
    fn player_bound_is_the_innkeeper_guid_then_the_area() {
        assert_eq!(
            raw_frame(build_player_bound(0xF130_0000_0000_1234, 87)),
            [0x00, 0x0e, 0x58, 0x01, 0x34, 0x12, 0, 0, 0, 0, 0x30, 0xf1, 87, 0, 0, 0]
        );
    }

    #[test]
    fn char_create_response_maps_every_outcome() {
        assert_eq!(
            build_char_create_response(CharCreateOutcome::Success).result,
            WorldResult::CharCreateSuccess
        );
        assert_eq!(
            build_char_create_response(CharCreateOutcome::NameInUse).result,
            WorldResult::CharCreateNameInUse
        );
        assert_eq!(
            build_char_create_response(CharCreateOutcome::ServerLimit).result,
            WorldResult::CharCreateServerLimit
        );
        assert_eq!(
            build_char_create_response(CharCreateOutcome::Failed).result,
            WorldResult::CharCreateError
        );
    }

    #[test]
    fn char_delete_response_maps_both_outcomes() {
        assert_eq!(
            build_char_delete_response(CharDeleteOutcome::Success).result,
            WorldResult::CharDeleteSuccess
        );
        assert_eq!(
            build_char_delete_response(CharDeleteOutcome::Failed).result,
            WorldResult::CharDeleteFailed
        );
    }

    #[test]
    fn logout_sequence_acks_then_completes_instantly() {
        let seq = logout_sequence();
        assert_eq!(seq.len(), 2);
        match &seq[0] {
            ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(r) => {
                assert_eq!(r.result, LogoutResult::Success);
                assert_eq!(r.speed, LogoutSpeed::Instant);
            }
            other => panic!("expected SMSG_LOGOUT_RESPONSE, got {other}"),
        }
        assert!(matches!(seq[1], ServerOpcodeMessage::SMSG_LOGOUT_COMPLETE));
    }

    #[test]
    fn logout_denied_in_combat_carries_the_failure_result() {
        let msg = logout_denied_in_combat();
        assert_eq!(msg.result, LogoutResult::FailureInCombat);
    }

    #[test]
    fn played_time_folds_the_live_session_span_into_the_durable_total() {
        let offline = build_played_time(3600, 0, 999_999);
        assert_eq!(offline.total_played_time, 3600);
        assert_eq!(offline.level_played_time, 3600);
        let now = 10_000_000_000u64;
        let session_start = now - 5_000_000;
        let online = build_played_time(3600, session_start, now);
        assert_eq!(online.total_played_time, 3605);
    }

    #[test]
    fn played_time_never_underflows_a_clock_that_moved_backward() {
        let msg = build_played_time(100, 5_000_000, 1_000_000);
        assert_eq!(
            msg.total_played_time, 100,
            "backward clock must not inflate the total"
        );
    }
}
