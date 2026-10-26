// Placeholder XP curve for templates without an explicit reward.
pub fn xp_reward(quest_level: u32) -> u32 {
    quest_level.saturating_mul(45).saturating_add(40)
}

pub fn race_allowed(required_races: u32, race: u8) -> bool {
    required_races == 0 || ((1..=32).contains(&race) && required_races & (1u32 << (race - 1)) != 0)
}

pub fn class_allowed(required_classes: u32, class: u8) -> bool {
    required_classes == 0
        || ((1..=32).contains(&class) && required_classes & (1u32 << (class - 1)) != 0)
}

// Placeholder conversion of six copper per XP at the level cap.
pub const MAX_LEVEL_XP_TO_COPPER: u32 = 6;

pub fn max_level_money_reward(xp: u32) -> u32 {
    xp.saturating_mul(MAX_LEVEL_XP_TO_COPPER)
}

// Vanilla XP is ceil(RewMoneyMaxLevel / 0.6).
pub fn quest_xp(reward_money_max_level: u32) -> u32 {
    if reward_money_max_level == 0 {
        return 0;
    }
    (((reward_money_max_level as u64) * 5).div_ceil(3)) as u32
}

// game_group_event kind 9 is retired; quest sharing uses 10 and 11.
pub mod share_event_kind {
    pub const QUEST_SHARE: u8 = 10;
    pub const QUEST_PUSH_RESULT: u8 = 11;
}

pub mod share_result {
    pub const SHARING_QUEST: u8 = 0;
    pub const CANT_TAKE_QUEST: u8 = 1;
    pub const TOO_FAR: u8 = 4;
    pub const LOG_FULL: u8 = 6;
    pub const HAVE_QUEST: u8 = 7;
    pub const FINISH_QUEST: u8 = 8;
}

#[allow(clippy::too_many_arguments)]
pub fn share_result(
    member_online: bool,
    same_map_instance: bool,
    in_range: bool,
    member_active: bool,
    member_finished: bool,
    log_full: bool,
    gates_ok: bool,
) -> u8 {
    if !member_online || !same_map_instance || !in_range {
        share_result::TOO_FAR
    } else if member_active {
        share_result::HAVE_QUEST
    } else if member_finished {
        share_result::FINISH_QUEST
    } else if log_full {
        share_result::LOG_FULL
    } else if !gates_ok {
        share_result::CANT_TAKE_QUEST
    } else {
        share_result::SHARING_QUEST
    }
}

pub const QUEST_TYPE_RAID: u32 = 62;

pub fn allowed_in_raid(quest_type: u32) -> bool {
    quest_type == QUEST_TYPE_RAID
}

pub fn quest_progresses_for_raid(in_raid: bool, quest_type: u32) -> bool {
    !in_raid || allowed_in_raid(quest_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_in_raid_only_accepts_the_raid_quest_type() {
        assert!(!allowed_in_raid(0));
        assert!(allowed_in_raid(QUEST_TYPE_RAID));
        assert!(!allowed_in_raid(81));
    }

    #[test]
    fn quest_progresses_for_raid_gates_only_a_raid_members_non_raid_quest() {
        assert!(quest_progresses_for_raid(false, 0));
        assert!(quest_progresses_for_raid(false, QUEST_TYPE_RAID));
        assert!(!quest_progresses_for_raid(true, 0));
        assert!(quest_progresses_for_raid(true, QUEST_TYPE_RAID));
    }

    #[test]
    fn xp_reward_is_monotonic_and_nonzero() {
        assert_eq!(xp_reward(1), 85);
        assert_eq!(xp_reward(2), 130);
        assert_eq!(xp_reward(5), 265);
        assert!(xp_reward(10) > xp_reward(5));
        assert!(xp_reward(u32::MAX) > 0);
    }

    #[test]
    fn quest_xp_is_authentic_and_exact() {
        assert_eq!(quest_xp(102), 170);
        assert_eq!(quest_xp(150), 250);
        assert_eq!(quest_xp(270), 450);
        assert_eq!(quest_xp(0), 0);
        assert_eq!(quest_xp(3), 5);
        assert!(quest_xp(u32::MAX) > 0);
    }

    #[test]
    fn race_class_gates_respect_bitmask_and_zero_means_all() {
        assert!(race_allowed(0, 1));
        assert!(class_allowed(0, 1));
        assert!(race_allowed(77, 1));
        assert!(race_allowed(77, 3));
        assert!(!race_allowed(77, 2));
        assert!(!race_allowed(77, 5));
        assert!(class_allowed(1, 1));
        assert!(!class_allowed(1, 2));
        assert!(!race_allowed(77, 0));
        assert!(!class_allowed(1, 0));
    }

    #[test]
    fn race_class_gates_pin_the_bit31_bit33_range_boundary() {
        let mask_bit31: u32 = 1 << 31;
        assert!(race_allowed(mask_bit31, 32));
        assert!(class_allowed(mask_bit31, 32));
        assert!(!race_allowed(mask_bit31, 33));
        assert!(!class_allowed(mask_bit31, 33));
        assert!(!race_allowed(u32::MAX, u8::MAX));
        assert!(!class_allowed(u32::MAX, u8::MAX));
    }

    #[test]
    fn max_level_money_reward_uses_the_documented_v_coefficient() {
        assert_eq!(MAX_LEVEL_XP_TO_COPPER, 6);
        assert_eq!(max_level_money_reward(0), 0);
        assert_eq!(max_level_money_reward(100), 600);
        assert_eq!(max_level_money_reward(u32::MAX), u32::MAX);
    }

    #[test]
    fn share_event_kinds_are_distinct_and_skip_retired_kind_9() {
        use crate::group::event_kind as g;
        use crate::loot_roll::event_kind as roll;
        let mut kinds = vec![
            g::INVITE,
            g::LIST,
            g::DECLINE,
            g::DESTROYED,
            roll::ROLL_START,
            roll::ROLL_VOTE,
            roll::ROLL_WON,
            roll::MASTER_LIST,
            roll::MONEY_SHARE,
            share_event_kind::QUEST_SHARE,
            share_event_kind::QUEST_PUSH_RESULT,
        ];
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds, [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11]);
        assert_eq!(share_event_kind::QUEST_SHARE, 10);
        assert_eq!(
            share_event_kind::QUEST_PUSH_RESULT,
            share_event_kind::QUEST_SHARE + 1
        );
    }

    #[test]
    fn share_result_codes_mirror_the_gtker_quest_party_message_enum() {
        assert_eq!(share_result::SHARING_QUEST, 0);
        assert_eq!(share_result::CANT_TAKE_QUEST, 1);
        assert_eq!(share_result::TOO_FAR, 4);
        assert_eq!(share_result::LOG_FULL, 6);
        assert_eq!(share_result::HAVE_QUEST, 7);
        assert_eq!(share_result::FINISH_QUEST, 8);
    }

    #[test]
    fn share_result_priority_order() {
        assert_eq!(
            share_result(true, true, true, false, false, false, true),
            share_result::SHARING_QUEST
        );
        assert_eq!(
            share_result(false, true, true, true, true, true, false),
            share_result::TOO_FAR
        );
        assert_eq!(
            share_result(true, false, true, false, false, false, true),
            share_result::TOO_FAR
        );
        assert_eq!(
            share_result(true, true, false, false, false, false, true),
            share_result::TOO_FAR
        );
        assert_eq!(
            share_result(true, true, true, true, true, true, false),
            share_result::HAVE_QUEST
        );
        assert_eq!(
            share_result(true, true, true, false, true, true, false),
            share_result::FINISH_QUEST
        );
        assert_eq!(
            share_result(true, true, true, false, false, true, false),
            share_result::LOG_FULL
        );
        assert_eq!(
            share_result(true, true, true, false, false, false, false),
            share_result::CANT_TAKE_QUEST
        );
    }
}

pub mod objective_kind {
    pub const KILL_CREATURE: u8 = 0;
    pub const COLLECT_ITEM: u8 = 1;
    pub const USE_GAMEOBJECT: u8 = 2;
    pub const EXPLORE_AREATRIGGER: u8 = 3;
}

#[derive(Clone, Copy)]
pub struct Objective {
    pub kind: u8,
    pub index: u8,
    pub target_entry: u32,
    pub required_count: u32,
}

pub fn objectives_complete(
    objectives: impl IntoIterator<Item = Objective>,
    counts: &[u32],
    mut carried_item_count: impl FnMut(u32) -> u32,
    requires_event_credit: bool,
    has_event_credit: bool,
) -> bool {
    objectives.into_iter().all(|objective| {
        let have = if objective.kind == objective_kind::COLLECT_ITEM {
            carried_item_count(objective.target_entry)
        } else {
            counts.get(objective.index as usize).copied().unwrap_or(0)
        };
        have >= objective.required_count
    }) && (!requires_event_credit || has_event_credit)
}

#[derive(Clone, Copy)]
pub struct AcceptanceRequirements {
    pub min_level: u32,
    pub required_races: u32,
    pub required_classes: u32,
    pub repeatable: bool,
}

#[derive(Clone, Copy)]
pub struct Attempt {
    pub rewarded: bool,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptanceRefusal {
    Level,
    Race,
    Class,
    Prerequisite,
    AlreadyTaken,
}

pub fn acceptance(
    requirements: AcceptanceRequirements,
    level: u32,
    race: u8,
    class: u8,
    prerequisite_rewarded: bool,
    existing: Option<Attempt>,
) -> Result<(), AcceptanceRefusal> {
    if level < requirements.min_level {
        return Err(AcceptanceRefusal::Level);
    }
    if !race_allowed(requirements.required_races, race) {
        return Err(AcceptanceRefusal::Race);
    }
    if !class_allowed(requirements.required_classes, class) {
        return Err(AcceptanceRefusal::Class);
    }
    if !prerequisite_rewarded {
        return Err(AcceptanceRefusal::Prerequisite);
    }
    if existing
        .is_some_and(|attempt| !(attempt.failed || (requirements.repeatable && attempt.rewarded)))
    {
        return Err(AcceptanceRefusal::AlreadyTaken);
    }
    Ok(())
}

impl AcceptanceRefusal {
    pub fn text(self, min_level: u32) -> String {
        match self {
            Self::Level => format!("requires level {min_level}"),
            Self::Race => "quest not available to your race".to_string(),
            Self::Class => "quest not available to your class".to_string(),
            Self::Prerequisite => "must complete the prerequisite quest first".to_string(),
            Self::AlreadyTaken => "already on or completed that quest".to_string(),
        }
    }
}

pub const QUEST_LOG_FULL_TEXT: &str = "quest log is full";

pub fn base_reward_xp(explicit_xp: u32, reward_money_max_level: u32, quest_level: u32) -> u32 {
    if explicit_xp > 0 {
        explicit_xp
    } else if reward_money_max_level > 0 {
        quest_xp(reward_money_max_level)
    } else {
        xp_reward(quest_level)
    }
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    #[test]
    fn completion_reads_each_objective_kind_from_its_authority() {
        let objectives = [
            Objective {
                kind: 0,
                index: 0,
                target_entry: 80,
                required_count: 3,
            },
            Objective {
                kind: 1,
                index: 1,
                target_entry: 90,
                required_count: 2,
            },
            Objective {
                kind: 3,
                index: 2,
                target_entry: 100,
                required_count: 1,
            },
        ];
        assert!(objectives_complete(
            objectives,
            &[3, 0, 1],
            |_| 2,
            false,
            false
        ));
        assert!(!objectives_complete(
            objectives,
            &[3, 99, 1],
            |_| 1,
            false,
            false
        ));
        assert!(!objectives_complete(
            objectives,
            &[3, 99],
            |_| 2,
            false,
            false
        ));
    }

    #[test]
    fn event_credit_is_required_even_when_no_ordinary_objectives_exist() {
        assert!(!objectives_complete([], &[], |_| 0, true, false));
        assert!(objectives_complete([], &[], |_| 0, true, true));
        assert!(objectives_complete([], &[], |_| 0, false, false));
        let kill = Objective {
            kind: 0,
            index: 0,
            target_entry: 80,
            required_count: 3,
        };
        assert!(!objectives_complete([kill], &[2], |_| 0, true, true));
    }

    fn requirements(repeatable: bool) -> AcceptanceRequirements {
        AcceptanceRequirements {
            min_level: 10,
            required_races: 1,
            required_classes: 2,
            repeatable,
        }
    }

    #[test]
    fn failed_attempts_can_be_retried_without_making_completed_quests_repeatable() {
        let failed = Some(Attempt {
            failed: true,
            rewarded: false,
        });
        let rewarded = Some(Attempt {
            failed: false,
            rewarded: true,
        });
        assert_eq!(
            acceptance(requirements(false), 10, 1, 2, true, failed),
            Ok(())
        );
        assert_eq!(
            acceptance(requirements(false), 10, 1, 2, true, rewarded),
            Err(AcceptanceRefusal::AlreadyTaken)
        );
        assert_eq!(
            acceptance(requirements(true), 10, 1, 2, true, rewarded),
            Ok(())
        );
        assert_eq!(
            acceptance(
                requirements(true),
                10,
                1,
                2,
                true,
                Some(Attempt {
                    failed: false,
                    rewarded: false
                })
            ),
            Err(AcceptanceRefusal::AlreadyTaken)
        );
    }

    #[test]
    fn retry_still_applies_level_race_class_and_prerequisite_gates_in_order() {
        let failed = Some(Attempt {
            failed: true,
            rewarded: false,
        });
        assert_eq!(
            acceptance(requirements(false), 9, 2, 1, false, failed),
            Err(AcceptanceRefusal::Level)
        );
        assert_eq!(
            acceptance(requirements(false), 10, 2, 1, false, failed),
            Err(AcceptanceRefusal::Race)
        );
        assert_eq!(
            acceptance(requirements(false), 10, 1, 1, false, failed),
            Err(AcceptanceRefusal::Class)
        );
        assert_eq!(
            acceptance(requirements(false), 10, 1, 2, false, failed),
            Err(AcceptanceRefusal::Prerequisite)
        );
    }

    #[test]
    fn reward_priority_is_explicit_then_imported_then_placeholder() {
        assert_eq!(base_reward_xp(123, 270, 10), 123);
        assert_eq!(base_reward_xp(0, 270, 10), 450);
        assert_eq!(base_reward_xp(0, 0, 10), 490);
    }
}
