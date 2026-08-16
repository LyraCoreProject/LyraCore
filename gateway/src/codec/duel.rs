//! Duel-specific sparse player descriptor updates.

use super::*;

/// Set only the Duel descriptor fields that changed. `arbiter` occupies two u32 slots; `team` is
/// one. Neither update carries `OBJECT_FIELD_TYPE`, preserving the vanilla partial-VALUES rule.
pub fn build_duel_player_values(
    guid: u64,
    arbiter: Option<u64>,
    team: Option<u32>,
) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    if let Some(arbiter) = arbiter {
        mask.set_u64(update_mask::idx::PLAYER_DUEL_ARBITER, arbiter);
    }
    if let Some(team) = team {
        mask.set_u32(update_mask::idx::PLAYER_DUEL_TEAM, team);
    }
    build_values_update_raw(guid, &mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duel_values_are_sparse_and_each_transition_changes_the_wire() {
        let arbiter = build_duel_player_values(1, Some(99), None);
        let team = build_duel_player_values(1, None, Some(1));
        let clear = build_duel_player_values(1, Some(0), Some(0));
        assert_eq!(arbiter.0, 0x00A9);
        assert_eq!(team.0, 0x00A9);
        assert_ne!(arbiter.1, team.1);
        assert_ne!(team.1, clear.1);
    }
}
