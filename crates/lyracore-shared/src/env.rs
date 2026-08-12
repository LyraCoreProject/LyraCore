//! Environmental damage (058): fall damage from the client-reported airborne time. Shared by the
//! module (applies the damage in `movement_update`) and the gateway (sends the matching
//! `SMSG_ENVIRONMENTAL_DAMAGE_LOG` flavor line) so the two can never drift.

/// Vanilla's `MOVEMENTFLAG_SWIMMING` bit. It is also set while floating at the surface, so it
/// must be combined with the water-surface test in [`is_submerged`].
pub const MOVEMENT_FLAG_SWIMMING: u32 = 0x0020_0000;

/// vMaNGOS's default player collision height, used as the distance from the player's feet to its
/// head when deciding whether it is underwater. Keep this separately named so model-aware logic
/// can retune it in one place later.
pub const UNDERWATER_HEAD_HEIGHT: f32 = 2.0;

/// Whether a player is below a liquid surface deeply enough to be submerged. A swimming flag by
/// itself only establishes that the client is swimming; the head must also be below a terrain cell
/// that records liquid.
pub fn is_submerged(
    player_z: f32,
    liquid_level: f32,
    has_liquid: bool,
    movement_flags: u32,
) -> bool {
    has_liquid
        && movement_flags & MOVEMENT_FLAG_SWIMMING != 0
        && player_z + UNDERWATER_HEAD_HEIGHT < liquid_level
}

/// The breath bar's full duration and its surface refill time, pinned to vanilla's roughly
/// one-minute `Player::HandleDrowning` window and deliberately slower recovery.
pub const FULL_AIR_MICROS: i64 = 60_000_000;
pub const REFILL_MICROS: i64 = 10_000_000;
pub const DROWNING_INTERVAL_MICROS: i64 = 1_000_000;
pub const MAX_DROWNING_CATCHUP_TICKS: u32 = 5;

/// Pure state-machine result for a draining or refilling breath bar.
#[derive(Debug, PartialEq, Eq)]
pub struct BreathAdvance {
    pub remaining_air_micros: i64,
    pub drowning_ticks: u32,
}

/// Advance a breath bar by elapsed wall time. A scheduler stall catches up a bounded number of
/// one-second drowning hits, avoiding both one harmless hit and an unbounded damage burst.
pub fn advance_breath(
    remaining_air_micros: i64,
    draining: bool,
    elapsed_micros: i64,
) -> BreathAdvance {
    let elapsed = elapsed_micros.max(0);
    let remaining = remaining_air_micros.clamp(0, FULL_AIR_MICROS);
    if !draining {
        let restored = elapsed.saturating_mul(FULL_AIR_MICROS) / REFILL_MICROS;
        return BreathAdvance {
            remaining_air_micros: remaining.saturating_add(restored).min(FULL_AIR_MICROS),
            drowning_ticks: 0,
        };
    }
    if remaining >= elapsed {
        return BreathAdvance {
            remaining_air_micros: remaining - elapsed,
            drowning_ticks: 0,
        };
    }
    let after_empty = elapsed - remaining;
    let ticks = if remaining == 0 {
        after_empty / DROWNING_INTERVAL_MICROS
    } else {
        1 + after_empty / DROWNING_INTERVAL_MICROS
    };
    BreathAdvance {
        remaining_air_micros: 0,
        drowning_ticks: ticks.min(MAX_DROWNING_CATCHUP_TICKS as i64) as u32,
    }
}

/// Vanilla drowning's fixed-fraction base hit. The small level-scaled random addition is deferred;
/// the core gameplay invariant is one fifth of maximum health per tick.
pub fn drowning_damage(max_health: u32) -> u32 {
    max_health.saturating_mul(20).saturating_add(99) / 100
}

/// Fall damage from airborne time: WoW gravity ~19.29 yd/s^2 gives free-fall height h = 0.5*g*t^2,
/// folded into the mangos height curve `damage% = 1.8%*yd - 24.26%` (safe under ~13.5 yd, which is
/// ~1.18 s airborne). `fall_time_ms` is the client's MovementInfo fall time on MSG_MOVE_FALL_LAND —
/// it INCLUDES the jump ascent, so a flat-ground hop (~750 ms) lands at 0. Client-authoritative like
/// all movement (a hacked client can lie; the 255 anti-cheat item owns that). Capped at max_health.
pub fn fall_damage(fall_time_ms: u32, max_health: u32) -> u32 {
    let t = fall_time_ms as f32 / 1000.0;
    let height_yd = 0.5 * 19.29 * t * t;
    let perc = 0.018 * height_yd - 0.2426;
    if perc <= 0.0 {
        return 0;
    }
    (max_health as f32 * perc.min(1.0)) as u32
}

/// `fall_time` (u32 ms) out of a raw vanilla MovementInfo body: flags u32 LE, time u32, x/y/z/o
/// 16 B, then [transport guid u64 + pos 16 B if flag 0x0200], [pitch f32 if SWIMMING 0x0020_0000],
/// then fall_time. `None` on a short body or an on-transport mover (no fall damage on transports).
/// NOTE the wire TYPE: cmangos reads fall time as u32 MILLISECONDS; gtker's MovementInfo types the
/// same 4 bytes f32 (a field-type lie, like its field-name ones) — this parser and any gtker-side
/// consumer must agree on the raw-u32 reading (`f32::to_bits` at the gtker boundary).
pub fn fall_time_from_movement_info(body: &[u8]) -> Option<u32> {
    const ON_TRANSPORT: u32 = 0x0200;
    let flags = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
    if flags & ON_TRANSPORT != 0 {
        return None;
    }
    let mut off = 4 + 4 + 16; // flags + time + x/y/z/o
    if flags & MOVEMENT_FLAG_SWIMMING != 0 {
        off += 4; // pitch
    }
    Some(u32::from_le_bytes(body.get(off..off + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_damage_curve() {
        assert_eq!(fall_damage(0, 100), 0);
        assert_eq!(fall_damage(800, 100), 0); // a flat-ground jump is safe
        assert_eq!(fall_damage(1_180, 100), 0); // ~13.4 yd — the safe threshold
        let d15 = fall_damage(1_500, 100); // ~21.7 yd → ~15%
        assert!((10..25).contains(&d15), "{d15}");
        assert_eq!(fall_damage(10_000, 100), 100); // terminal: capped at max_health
    }

    #[test]
    fn fall_time_parses_past_the_flag_dependent_blocks() {
        let mut body = vec![0u8; 24];
        body.extend_from_slice(&1500u32.to_le_bytes());
        assert_eq!(fall_time_from_movement_info(&body), Some(1500));
        // swimming: pitch shifts fall_time by 4
        let mut swim = MOVEMENT_FLAG_SWIMMING.to_le_bytes().to_vec();
        swim.extend_from_slice(&[0u8; 20]); // time + pos
        swim.extend_from_slice(&0f32.to_le_bytes()); // pitch
        swim.extend_from_slice(&2000u32.to_le_bytes());
        assert_eq!(fall_time_from_movement_info(&swim), Some(2000));
        // transport: no fall damage
        let mut tr = 0x0200u32.to_le_bytes().to_vec();
        tr.extend_from_slice(&[0u8; 60]);
        assert_eq!(fall_time_from_movement_info(&tr), None);
        // short body: None, never a panic
        assert_eq!(fall_time_from_movement_info(&[0u8; 10]), None);
    }

    #[test]
    fn submerged_requires_swimming_liquid_and_head_below_surface() {
        let surface = 10.0;
        let swimming = MOVEMENT_FLAG_SWIMMING;

        assert!(!is_submerged(8.0, surface, true, swimming)); // head exactly at surface
        assert!(is_submerged(7.99, surface, true, swimming));
        assert!(!is_submerged(8.01, surface, true, swimming));
        assert!(!is_submerged(0.0, surface, false, swimming)); // dry/off-slice cell
        assert!(!is_submerged(0.0, surface, true, 0));
    }

    #[test]
    fn breath_state_machine_drains_refills_and_bounds_catchup() {
        assert_eq!(
            advance_breath(FULL_AIR_MICROS, true, 12_000_000),
            BreathAdvance {
                remaining_air_micros: 48_000_000,
                drowning_ticks: 0
            }
        );
        assert_eq!(
            advance_breath(200_000, true, 300_000),
            BreathAdvance {
                remaining_air_micros: 0,
                drowning_ticks: 1
            }
        );
        assert_eq!(
            advance_breath(0, true, 20_000_000),
            BreathAdvance {
                remaining_air_micros: 0,
                drowning_ticks: MAX_DROWNING_CATCHUP_TICKS
            }
        );
        assert_eq!(
            advance_breath(54_000_000, false, 2_000_000),
            BreathAdvance {
                remaining_air_micros: FULL_AIR_MICROS,
                drowning_ticks: 0
            }
        );
        assert_eq!(drowning_damage(100).saturating_mul(10).min(100), 100);
    }
}
