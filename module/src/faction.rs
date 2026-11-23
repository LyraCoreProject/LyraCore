use spacetimedb::{table, ReducerContext};

#[table(accessor = game_faction_template, public)]
pub struct FactionTemplate {
    #[primary_key]
    pub id: u32,
    pub faction: u32,
    pub faction_group: u32,
    pub friend_group: u32,
    pub enemy_group: u32,
    pub enemy_0: u32,
    pub enemy_1: u32,
    pub enemy_2: u32,
    pub enemy_3: u32,
    pub friend_0: u32,
    pub friend_1: u32,
    pub friend_2: u32,
    pub friend_3: u32,
    /// `FactionTemplate.dbc` `Flags`.
    #[default(0)]
    pub flags: u32,
}

// vmangos DBCEnums.h FactionTemplateFlags.
const RESPOND_TO_CALL_FOR_HELP: u32 = 0x1;
const FLEE_FROM_CALL_FOR_HELP: u32 = 0x400;

impl FactionTemplate {
    /// vmangos `RESPOND_TO_CALL_FOR_HELP` without `FLEE_FROM_CALL_FOR_HELP`.
    pub(crate) fn answers_call_for_help(&self) -> bool {
        self.flags & RESPOND_TO_CALL_FOR_HELP != 0 && self.flags & FLEE_FROM_CALL_FOR_HELP == 0
    }
    /// vmangos `RESPOND_TO_CALL_FOR_HELP` or `FLEE_FROM_CALL_FOR_HELP`: a call reaches the creature.
    pub(crate) fn reacts_to_call_for_help(&self) -> bool {
        self.flags & (RESPOND_TO_CALL_FOR_HELP | FLEE_FROM_CALL_FOR_HELP) != 0
    }
    pub(crate) fn flees_from_call_for_help(&self) -> bool {
        self.flags & FLEE_FROM_CALL_FOR_HELP != 0
    }
    fn enemies(&self) -> [u32; 4] {
        [self.enemy_0, self.enemy_1, self.enemy_2, self.enemy_3]
    }
    fn friends(&self) -> [u32; 4] {
        [self.friend_0, self.friend_1, self.friend_2, self.friend_3]
    }
}

#[table(accessor = game_faction, public)]
pub struct Faction {
    #[primary_key]
    pub faction_id: u32,
    pub reputation_index: i32,
    /// Faction.dbc `BaseRepValue[0]`; the `_2` to `_4` columns hold the other three slots.
    pub base_standing: i32,
    #[default(0)]
    pub base_standing_2: i32,
    #[default(0)]
    pub base_standing_3: i32,
    #[default(0)]
    pub base_standing_4: i32,
    #[default(0)]
    pub base_race_mask_1: u32,
    #[default(0)]
    pub base_race_mask_2: u32,
    #[default(0)]
    pub base_race_mask_3: u32,
    #[default(0)]
    pub base_race_mask_4: u32,
    #[default(0)]
    pub base_class_mask_1: u32,
    #[default(0)]
    pub base_class_mask_2: u32,
    #[default(0)]
    pub base_class_mask_3: u32,
    #[default(0)]
    pub base_class_mask_4: u32,
}

impl Faction {
    pub(crate) fn base_standing_for(&self, race: u8, class: u8) -> i32 {
        lyracore_shared::faction::base_standing(
            [
                (
                    self.base_race_mask_1,
                    self.base_class_mask_1,
                    self.base_standing,
                ),
                (
                    self.base_race_mask_2,
                    self.base_class_mask_2,
                    self.base_standing_2,
                ),
                (
                    self.base_race_mask_3,
                    self.base_class_mask_3,
                    self.base_standing_3,
                ),
                (
                    self.base_race_mask_4,
                    self.base_class_mask_4,
                    self.base_standing_4,
                ),
            ],
            race,
            class,
        )
    }
}

fn relation_holds(
    asserting: &[u32; 4],
    vetoing: &[u32; 4],
    mask: u32,
    other: &FactionTemplate,
) -> bool {
    lyracore_shared::faction::relation_holds(
        asserting,
        vetoing,
        mask,
        other.faction,
        other.faction_group,
    )
}

pub(crate) fn compute_hostile(a: &FactionTemplate, b: &FactionTemplate) -> bool {
    relation_holds(&a.enemies(), &a.friends(), a.enemy_group, b)
}

pub(crate) fn compute_friendly(a: &FactionTemplate, b: &FactionTemplate) -> bool {
    relation_holds(&a.friends(), &a.enemies(), a.friend_group, b)
}

pub(crate) fn is_hostile(ctx: &ReducerContext, attacker_faction: u32, target_faction: u32) -> bool {
    let ft = ctx.db.game_faction_template();
    match (ft.id().find(attacker_faction), ft.id().find(target_faction)) {
        (Some(a), Some(b)) => compute_hostile(&a, &b),
        _ => false,
    }
}

pub(crate) fn is_friendly(
    ctx: &ReducerContext,
    attacker_faction: u32,
    target_faction: u32,
) -> bool {
    let ft = ctx.db.game_faction_template();
    match (ft.id().find(attacker_faction), ft.id().find(target_faction)) {
        (Some(a), Some(b)) => compute_friendly(&a, &b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ft(
        id: u32,
        faction: u32,
        fg: u32,
        frg: u32,
        eg: u32,
        en: [u32; 4],
        fr: [u32; 4],
    ) -> FactionTemplate {
        FactionTemplate {
            id,
            faction,
            faction_group: fg,
            friend_group: frg,
            enemy_group: eg,
            enemy_0: en[0],
            enemy_1: en[1],
            enemy_2: en[2],
            enemy_3: en[3],
            friend_0: fr[0],
            friend_1: fr[1],
            friend_2: fr[2],
            friend_3: fr[3],
            flags: 0,
        }
    }

    #[test]
    fn can_attack_neutral_and_hostile_but_not_friendly() {
        let player = ft(1, 1, 0x3, 0x2, 0xC, [0; 4], [0; 4]);
        let guard = ft(11, 72, 0x3, 0x2, 0xC, [0; 4], [0; 4]);
        let wolf = ft(32, 29, 0x0, 0x0, 0x0, [0; 4], [0; 4]);
        let kobold = ft(25, 25, 0x8, 0x0, 0x0, [0; 4], [0; 4]);
        assert!(
            compute_friendly(&player, &guard),
            "player is friendly to an Alliance guard"
        );
        assert!(
            !compute_friendly(&player, &wolf),
            "a neutral wolf is NOT friendly → attackable"
        );
        assert!(
            !compute_friendly(&player, &kobold),
            "a Monster kobold is NOT friendly → attackable"
        );
    }

    #[test]
    fn compute_friendly_explicit_lists_override_group_masks() {
        let a = ft(1, 0, 0, 0x0, 0, [0; 4], [55, 0, 0, 0]);
        let b = ft(2, 55, 0x4, 0, 0, [0; 4], [0; 4]);
        assert!(
            compute_friendly(&a, &b),
            "explicit friend list wins even with a zero/mismatched friend_group mask"
        );

        let c = ft(3, 0, 0, 0x4, 0, [77, 0, 0, 0], [0; 4]);
        let d = ft(4, 77, 0x4, 0, 0, [0; 4], [0; 4]);
        assert!(
            !compute_friendly(&c, &d),
            "explicit enemy list beats a matching friend-group mask"
        );
    }

    #[test]
    fn explicit_enemy_faction_is_hostile() {
        let a = ft(1, 0, 0x2, 0, 0, [59, 0, 0, 0], [0; 4]);
        let b = ft(2, 59, 0x1, 0, 0, [0; 4], [0; 4]);
        assert!(compute_hostile(&a, &b));
    }

    #[test]
    fn explicit_friend_overrides_hostile_group_mask() {
        let a = ft(1, 0, 0, 0, 0xFFFF_FFFF, [0; 4], [77, 0, 0, 0]);
        let b = ft(2, 77, 0x1, 0, 0, [0; 4], [0; 4]);
        assert!(
            !compute_hostile(&a, &b),
            "explicit friend must beat the hostile group mask"
        );
    }

    #[test]
    fn group_mask_decides_with_no_explicit_entry() {
        let a = ft(1, 0, 0, 0, 0x4, [0; 4], [0; 4]);
        let hostile_b = ft(2, 0, 0x4, 0, 0, [0; 4], [0; 4]);
        let neutral_b = ft(3, 0, 0x2, 0, 0, [0; 4], [0; 4]);
        assert!(compute_hostile(&a, &hostile_b));
        assert!(!compute_hostile(&a, &neutral_b));
    }

    #[test]
    fn only_a_responding_template_that_does_not_flee_answers_a_call_for_help() {
        for (flags, answers) in [(0x0, false), (0x1, true), (0x401, false), (0x400, false)] {
            let template = FactionTemplate {
                flags,
                ..ft(1, 0, 0, 0, 0, [0; 4], [0; 4])
            };
            assert_eq!(
                template.answers_call_for_help(),
                answers,
                "flags {flags:#x}"
            );
        }
    }

    #[test]
    fn a_template_reacts_to_a_call_for_help_when_it_responds_or_flees() {
        for (flags, reacts, flees) in [
            (0x0, false, false),
            (0x1, true, false),
            (0x401, true, true),
            (0x400, true, true),
        ] {
            let template = FactionTemplate {
                flags,
                ..ft(1, 0, 0, 0, 0, [0; 4], [0; 4])
            };
            assert_eq!(
                template.reacts_to_call_for_help(),
                reacts,
                "flags {flags:#x}"
            );
            assert_eq!(
                template.flees_from_call_for_help(),
                flees,
                "flags {flags:#x}"
            );
        }
    }

    #[test]
    fn zero_faction_skips_explicit_lists() {
        let a = ft(1, 0, 0, 0, 0, [99, 0, 0, 0], [0; 4]);
        let b = ft(2, 0, 0x1, 0, 0, [0; 4], [0; 4]);
        assert!(!compute_hostile(&a, &b));
    }
}
