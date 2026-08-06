//! Character-select wire mapping: `CharacterView` + the char-enum / char-create / name-query
//! builders and the logout ack. Pure code-motion out of `mod.rs` (behind the `pub use` facade).

use super::*;

/// A character row as the gateway reads it from its per-player subscription (`game_character`),
/// flattened to the plain ints/strings the codec needs to build `SMSG_CHAR_ENUM`. Decoupled
/// from the module's table type; the typed enum conversions happen in [`build_char_enum`].
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
    /// Equipment display models for the character-select screen.  Indexed by equipment slot
    /// (0..=18, matching `game_item_instance.slot`).  Default is all-zero (naked/grey).
    /// Populated by `Coordinator::characters` from `game_item_instance` + `game_item_template`.
    pub equipment: [CharacterGear; 19],
    /// Accrued played-time total in whole seconds, NOT counting the current live session (work-item
    /// 029, `/played`). Folded with `session_start_micros` at the `CMSG_PLAYED_TIME` reply site so an
    /// online player's total keeps ticking without a periodic write.
    pub played_total_secs: u32,
    /// Unix-epoch micros the current live session began (0 = offline / no live session).
    pub session_start_micros: u64,
}

/// Build the `SMSG_CHAR_ENUM` reply for the character-select screen (Phase 3, gateway
/// translation §4). Each [`CharacterView`] becomes a `wow_world_messages` `Character` block;
/// The five appearance bytes a player picks at character creation, bundled so `create_character`
/// (dispatch → trait → coordinator) passes one value instead of five positional `u8`s that are
/// trivial to transpose. (race/class/gender are validated separately via typed `try_from`.)
#[derive(Clone, Copy, Debug, Default)]
pub struct Appearance {
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

/// Outcome of a `CMSG_CHAR_CREATE` from the server's perspective (mapped to a `WorldResult`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharCreateOutcome {
    Success,
    NameInUse,
    /// Account already holds the per-realm character cap (10) — the 11th is refused (work-item 105).
    ServerLimit,
    Failed,
}

/// Build the `SMSG_CHAR_CREATE` reply. `CharCreateSuccess` makes the client re-send
/// `CMSG_CHAR_ENUM` (so the new character appears); the others raise a popup.
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

/// Outcome of a `CMSG_CHAR_DELETE` from the server's perspective (mapped to a `WorldResult`).
/// [081]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharDeleteOutcome {
    Success,
    Failed,
}

/// Build the `SMSG_CHAR_DELETE` reply. Per the wire doc this updates the character-select screen
/// directly — the client does NOT need to re-send `CMSG_CHAR_ENUM` (unlike char-create). [081]
pub fn build_char_delete_response(outcome: CharDeleteOutcome) -> SMSG_CHAR_DELETE {
    SMSG_CHAR_DELETE {
        result: match outcome {
            CharDeleteOutcome::Success => WorldResult::CharDeleteSuccess,
            CharDeleteOutcome::Failed => WorldResult::CharDeleteFailed,
        },
    }
}

/// the count and the `first_bag_*` trailers are written by the codec, not set here.
///
/// Equipment is all-zero ("naked but valid") for the slice. `race`/`class`/`gender`/`map` are
/// load-bearing for what the client renders, so an out-of-range raw value is a hard error;
/// `area`/`zone` is cosmetic on this screen, so an unknown zone degrades to `Area::None`
/// rather than failing the whole enumeration.
pub fn build_char_enum(chars: &[CharacterView]) -> Result<SMSG_CHAR_ENUM> {
    let mut characters = Vec::with_capacity(chars.len());
    for c in chars {
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
            guild_id: 0,
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

/// Build the `SMSG_NAME_QUERY_RESPONSE` the client needs to render a (peer or self) character's
/// name + race/class/gender, so it shows the real name instead of "Unknown". `realm_name` is empty
/// (same-realm). race/class/gender are load-bearing for the name plate, so an out-of-range raw
/// value is a hard error.
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

/// Build the `SMSG_INSPECT` reply to `CMSG_INSPECT`: just an echo of the validated target guid — the
/// client opens the paperdoll and renders it from the target's OWN visible-item fields (already synced
/// via the entity's `PLAYER_VISIBLE_ITEM_*` update-mask fields), no extra payload needed. The gateway
/// only sends this after the `inspect` reducer's range+friendly gate passes (work-item 137).
pub fn build_inspect_response(target_guid: u64) -> SMSG_INSPECT {
    SMSG_INSPECT {
        guid: Guid::new(target_guid),
    }
}

/// The reply to `CMSG_LOGOUT_REQUEST` (the client's Logout/Exit button): acknowledge an
/// **instant** successful logout, then immediately complete it. Without this the client hangs on
/// the logout/exit screen waiting for `SMSG_LOGOUT_COMPLETE`. Only called when the player is NOT
/// in combat.
pub fn logout_sequence() -> Vec<ServerOpcodeMessage> {
    vec![
        ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(SMSG_LOGOUT_RESPONSE {
            result: LogoutResult::Success,
            speed: LogoutSpeed::Instant,
        }),
        ServerOpcodeMessage::SMSG_LOGOUT_COMPLETE,
    ]
}

/// Denial reply to `CMSG_LOGOUT_REQUEST` when the player is in combat. Vanilla uses
/// `FAILURE_IN_COMBAT` (result=1); the client shows an error message and does NOT begin a
/// logout countdown. `speed` is irrelevant for non-Success results but must be set.
pub fn logout_denied_in_combat() -> SMSG_LOGOUT_RESPONSE {
    SMSG_LOGOUT_RESPONSE {
        result: LogoutResult::FailureInCombat,
        speed: LogoutSpeed::Instant,
    }
}

/// The reply to `CMSG_PLAYED_TIME` (`/played`, work-item 029): `total_played_time` is the durable
/// `played_total_secs` plus this session's live elapsed span (so an online player's total keeps
/// ticking without a periodic write); `level_played_time` is not tracked per-level in this slice, so
/// it mirrors the total (matching vanilla's own `/played` fallback shape when level-time is unset).
/// `now_micros` is the caller's current wall-clock reading (unix-epoch micros) so this stays pure/
/// testable rather than reaching for `SystemTime::now()` internally.
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
        // Offline (no live session): total is exactly the durable value, no fold.
        let offline = build_played_time(3600, 0, 999_999);
        assert_eq!(offline.total_played_time, 3600);
        assert_eq!(offline.level_played_time, 3600);
        // Online: total = durable + elapsed seconds since session_start_micros.
        let now = 10_000_000_000u64; // an arbitrary "current" wall-clock reading, in micros
        let session_start = now - 5_000_000; // session began 5 real seconds ago
        let online = build_played_time(3600, session_start, now);
        assert_eq!(online.total_played_time, 3605);
    }

    #[test]
    fn played_time_never_underflows_a_clock_that_moved_backward() {
        // A `now_micros` at or before `session_start_micros` (a clock adjustment / bad caller) must
        // saturate the elapsed span at 0 rather than wrap a u64 subtraction into a huge total.
        let msg = build_played_time(100, 5_000_000, 1_000_000);
        assert_eq!(
            msg.total_played_time, 100,
            "backward clock must not inflate the total"
        );
    }
}
