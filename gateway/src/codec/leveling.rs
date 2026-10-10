//! XP / level-up wire mapping: the `SMSG_LOG_XPGAIN` "+N experience" text and the
//! `SMSG_LEVELUP_INFO` ding popup (Tier 5 / XP). The matching descriptor VALUES updates
//! (`build_player_xp_values` / `build_levelup_values`) live in `values.rs`. Pure code-motion.

use super::*;

/// Build `SMSG_LOG_XPGAIN` (Tier 5 / XP) — the "+N experience" combat-log text for a kill.
/// `killed_guid` is the slain creature.
/// SMSG_EXPLORATION_EXPERIENCE (opcode 0x01F8) — the "Discovered: <area>" popup, sent once per fresh
/// subzone. Body is `area_id: u32 LE` + `experience: u32 LE` (8 bytes). Hand-rolled raw rather than the
/// gtker typed message so an `Area` enum that lacks an imported area id can't reject the send (the fog +
/// XP still ship; only the text would be skipped otherwise). Returns `(opcode, body)` for `Outbound::Raw`.
pub const SMSG_EXPLORATION_EXPERIENCE_OPCODE: u16 = 0x01F8;
pub fn build_exploration_experience_raw(area_id: u32, experience: u32) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(8);
    body.extend_from_slice(&area_id.to_le_bytes());
    body.extend_from_slice(&experience.to_le_bytes());
    (SMSG_EXPLORATION_EXPERIENCE_OPCODE, body)
}

/// Build 5875 XP feedback. Only a kill carries base XP and the group multiplier.
/// The typed dependency attaches that tail to the opposite award type.
pub const SMSG_LOG_XPGAIN: u16 = 0x01d0;

pub fn build_log_xpgain_raw(
    killed_guid: u64,
    total_exp: u32,
    is_kill: bool,
    rested_bonus: u32,
) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(if is_kill { 21 } else { 13 });
    body.extend_from_slice(&killed_guid.to_le_bytes());
    body.extend_from_slice(&total_exp.to_le_bytes());
    body.push(u8::from(!is_kill));
    if is_kill {
        body.extend_from_slice(&total_exp.saturating_sub(rested_bonus).to_le_bytes());
        body.extend_from_slice(&1.0f32.to_le_bytes());
    }
    (SMSG_LOG_XPGAIN, body)
}

/// Build `SMSG_LEVELUP_INFO` (Tier 5 / XP) — the "You have reached level N" ding popup, carrying the
/// health/mana gains and the five per-stat deltas the module's ding loop computed. rage/focus/
/// energy/happiness stay 0 — those pools never grow per level in vanilla.
#[allow(clippy::too_many_arguments)]
pub fn build_levelup_info(
    new_level: u32,
    health_gained: u32,
    mana_gained: u32,
    strength_gained: u32,
    agility_gained: u32,
    stamina_gained: u32,
    intellect_gained: u32,
    spirit_gained: u32,
) -> SMSG_LEVELUP_INFO {
    SMSG_LEVELUP_INFO {
        new_level: Level::new(new_level as u8),
        health: health_gained,
        mana: mana_gained,
        rage: 0,
        focus: 0,
        energy: 0,
        happiness: 0,
        strength: strength_gained,
        agility: agility_gained,
        stamina: stamina_gained,
        intellect: intellect_gained,
        spirit: spirit_gained,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benilla_decodes_kill_rested_and_nonkill_xp_without_a_tail() {
        use benilla_protocol::messages::{parse_server_with_tail, ServerPacket};
        for (guid, total, kill, rested, base) in [
            (0xF130_0000_0000_0001, 45, true, 0, 45),
            (0xF130_0000_0000_0001, 90, true, 45, 45),
            (0, 90, false, 0, 90),
        ] {
            let (opcode, body) = build_log_xpgain_raw(guid, total, kill, rested);
            let (ServerPacket::XpGain(xp), tail) = parse_server_with_tail(opcode, &body).unwrap()
            else {
                panic!("expected XP gain");
            };
            assert_eq!(tail, 0);
            assert_eq!(
                (xp.victim, xp.total, xp.kill, xp.base),
                (guid, total, kill, base)
            );
            if kill {
                assert_eq!(&body[17..], &1.0f32.to_le_bytes());
            }
        }
    }

    #[test]
    fn levelup_info_carries_the_ding_and_all_five_stat_deltas() {
        let msg = build_levelup_info(11, 20, 15, 1, 0, 2, 0, 1);
        assert_eq!(msg.new_level, Level::new(11));
        assert_eq!(msg.health, 20);
        assert_eq!(msg.mana, 15);
        assert_eq!(msg.strength, 1);
        assert_eq!(msg.agility, 0);
        assert_eq!(msg.stamina, 2);
        assert_eq!(msg.intellect, 0);
        assert_eq!(msg.spirit, 1);
        // rage/focus/energy/happiness never grow per level in vanilla — always 0.
        assert_eq!(msg.rage, 0);
        assert_eq!(msg.focus, 0);
        assert_eq!(msg.energy, 0);
        assert_eq!(msg.happiness, 0);
    }
}
