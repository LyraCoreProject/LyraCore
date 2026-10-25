#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LootRefusal {
    LootTagIneligible,
    OutOfRange,
    NoLootSource,
    LooterUnavailable,
    NothingToLoot,
    RollUnavailable,
    NotMasterLooter,
    RecipientUnavailable,
    RecipientInventoryFull,
}

impl LootRefusal {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::LootTagIneligible => "loot:loot_tag_ineligible",
            Self::OutOfRange => "loot:out_of_range",
            Self::NoLootSource => "loot:no_loot_source",
            Self::LooterUnavailable => "loot:looter_unavailable",
            Self::NothingToLoot => "loot:nothing_to_loot",
            Self::RollUnavailable => "loot:roll_unavailable",
            Self::NotMasterLooter => "loot:not_master_looter",
            Self::RecipientUnavailable => "loot:recipient_unavailable",
            Self::RecipientInventoryFull => "loot:recipient_inventory_full",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LootBoundaryFailure {
    OperatorRejected,
    MissingActor,
}

impl LootBoundaryFailure {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::OperatorRejected => "loot:boundary_operator_rejected",
            Self::MissingActor => "loot:boundary_missing_actor",
        }
    }
}

pub fn quest_take_allowed(reserved_for: u64, taker_guid: u64, taker_needs_item: bool) -> bool {
    reserved_for == taker_guid || (reserved_for == 0 && taker_needs_item)
}

pub fn group_loot_take_allowed(
    withheld: bool,
    reserved_for: u64,
    master_only: bool,
    designated_looter_guid: u64,
    taker_guid: u64,
) -> bool {
    if withheld {
        return false;
    }
    if reserved_for != 0 {
        return reserved_for == taker_guid;
    }
    !master_only && (designated_looter_guid == 0 || designated_looter_guid == taker_guid)
}

#[derive(Clone, Copy)]
pub struct QuestItemObjective {
    pub quest_entry: u32,
    pub quest_type: u32,
    pub kind: u8,
    pub target_entry: u32,
    pub required_count: u32,
}

pub fn needs_quest_item(
    in_raid: bool,
    active_quests: &[u32],
    objectives: impl IntoIterator<Item = QuestItemObjective>,
    item: u32,
    held: u32,
) -> bool {
    objectives.into_iter().any(|objective| {
        objective.kind == crate::quest::objective_kind::COLLECT_ITEM
            && active_quests.contains(&objective.quest_entry)
            && objective.target_entry == item
            && held < objective.required_count.max(1)
            && crate::quest::quest_progresses_for_raid(in_raid, objective.quest_type)
    })
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    #[test]
    fn withheld_loot_stays_owned_by_the_roll_even_with_a_reservation() {
        assert!(!group_loot_take_allowed(true, 7, false, 0, 7));
        assert!(group_loot_take_allowed(false, 7, true, 8, 7));
        assert!(!group_loot_take_allowed(false, 7, false, 0, 8));
    }

    #[test]
    fn a_quest_reservation_survives_the_characters_need() {
        assert!(quest_take_allowed(7, 7, false));
        assert!(!quest_take_allowed(7, 8, true));
        assert!(quest_take_allowed(0, 8, true));
        assert!(!quest_take_allowed(0, 8, false));
    }

    #[test]
    fn collect_need_respects_raid_type_quantity_and_the_minimum_requirement() {
        let collect = |quest_type, target_entry, required_count| QuestItemObjective {
            quest_entry: 100,
            quest_type,
            kind: 1,
            target_entry,
            required_count,
        };
        assert!(needs_quest_item(false, &[100], [collect(0, 55, 10)], 55, 9));
        assert!(!needs_quest_item(
            false,
            &[100],
            [collect(0, 55, 10)],
            55,
            10
        ));
        assert!(!needs_quest_item(true, &[100], [collect(0, 55, 10)], 55, 0));
        assert!(needs_quest_item(true, &[100], [collect(62, 55, 10)], 55, 0));
        assert!(needs_quest_item(false, &[100], [collect(0, 55, 0)], 55, 0));
        assert!(!needs_quest_item(false, &[100], [collect(0, 55, 0)], 55, 1));
        assert!(!needs_quest_item(
            false,
            &[100],
            [collect(0, 56, 10)],
            55,
            0
        ));
        assert!(!needs_quest_item(false, &[], [collect(0, 55, 10)], 55, 0));
        let kill = QuestItemObjective {
            kind: 0,
            ..collect(0, 55, 10)
        };
        assert!(!needs_quest_item(false, &[100], [kill], 55, 0));
    }
}
