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
        let flag_guid = 0xF110_2000_0000_0063;
        let arbiter = build_duel_player_values(1, Some(flag_guid), None);
        let team = build_duel_player_values(1, None, Some(1));
        let clear = build_duel_player_values(1, Some(0), Some(0));
        assert_eq!(arbiter.0, 0x00A9);
        assert_eq!(team.0, 0x00A9);
        assert_ne!(arbiter.1, team.1);
        assert_ne!(team.1, clear.1);

        let arbiter_values = lyracore_shared::values_mask::parse_values_updates(&arbiter.1);
        assert_eq!(
            arbiter_values[0].fields,
            vec![
                (update_mask::idx::PLAYER_DUEL_ARBITER, flag_guid as u32),
                (
                    update_mask::idx::PLAYER_DUEL_ARBITER + 1,
                    (flag_guid >> 32) as u32
                ),
            ]
        );
        let team_values = lyracore_shared::values_mask::parse_values_updates(&team.1);
        assert_eq!(
            team_values[0].fields,
            vec![(update_mask::idx::PLAYER_DUEL_TEAM, 1)]
        );
        let clear_values = lyracore_shared::values_mask::parse_values_updates(&clear.1);
        assert_eq!(
            clear_values[0].fields,
            vec![
                (update_mask::idx::PLAYER_DUEL_ARBITER, 0),
                (update_mask::idx::PLAYER_DUEL_ARBITER + 1, 0),
                (update_mask::idx::PLAYER_DUEL_TEAM, 0),
            ]
        );
        assert!(clear_values[0]
            .fields
            .iter()
            .all(|(index, _)| *index != update_mask::idx::OBJECT_TYPE));
    }
}
