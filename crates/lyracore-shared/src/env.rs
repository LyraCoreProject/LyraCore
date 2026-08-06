//! Environmental damage (058): fall damage from the client-reported airborne time. Shared by the
//! module (applies the damage in `movement_update`) and the gateway (sends the matching
//! `SMSG_ENVIRONMENTAL_DAMAGE_LOG` flavor line) so the two can never drift.

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
    const SWIMMING: u32 = 0x0020_0000;
    let flags = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?);
    if flags & ON_TRANSPORT != 0 {
        return None;
    }
    let mut off = 4 + 4 + 16; // flags + time + x/y/z/o
    if flags & SWIMMING != 0 {
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
        let mut swim = 0x0020_0000u32.to_le_bytes().to_vec();
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
}
