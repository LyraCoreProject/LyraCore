use super::tables::{COMBAT_BACKSTOP_SQ, LEASH_RADIUS_SQ, PURSUIT_WINDOW_MS};

pub(crate) fn pursuit_deadline_ms(now_ms: u32) -> u32 {
    now_ms.wrapping_add(PURSUIT_WINDOW_MS).max(1)
}

pub(crate) fn beyond_combat_backstop(dist_sq: f32) -> bool {
    dist_sq > COMBAT_BACKSTOP_SQ
}

pub(crate) fn should_evade(now_ms: u32, pursuit_ends_ms: u32, target_dist_sq: f32) -> bool {
    pursuit_ends_ms != 0 && now_ms >= pursuit_ends_ms && target_dist_sq > LEASH_RADIUS_SQ
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_leash_constants_hold_their_invariants() {
        const {
            assert!(LEASH_RADIUS_SQ < COMBAT_BACKSTOP_SQ);
            assert!(crate::combat::RANGED_RANGE_SQ < COMBAT_BACKSTOP_SQ);
        }
    }

    #[test]
    fn a_pursuit_deadline_never_stamps_the_never_refreshed_sentinel() {
        assert_eq!(pursuit_deadline_ms(50_000), 50_000 + PURSUIT_WINDOW_MS);
        assert_eq!(pursuit_deadline_ms(u32::MAX - PURSUIT_WINDOW_MS + 1), 1);
        assert!(!should_evade(
            50_000,
            pursuit_deadline_ms(50_000),
            f32::INFINITY
        ));
    }
}
