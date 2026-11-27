use crate::race_class::{mask_admits, ZeroMask};

// game_group_event kind 9 is retired; quest sharing uses 10 and 11.
pub mod share_event_kind {
    pub const QUEST_SHARE: u8 = 10;
    pub const QUEST_PUSH_RESULT: u8 = 11;
}

pub mod share_result {
    pub const SHARING_QUEST: u8 = 0;
    pub const CANT_TAKE_QUEST: u8 = 1;
    pub const ACCEPT_QUEST: u8 = 2;
    pub const DECLINE_QUEST: u8 = 3;
    pub const TOO_FAR: u8 = 4;
    pub const BUSY: u8 = 5;
    pub const LOG_FULL: u8 = 6;
    pub const HAVE_QUEST: u8 = 7;
    pub const FINISH_QUEST: u8 = 8;
}

/// vmangos `QUEST_FLAGS_SHARABLE`.
pub const QUEST_FLAG_SHARABLE: u32 = 0x8;

/// vmangos `QUEST_FLAGS_UNK2`: a rewarded repeatable quest with it shows no blue mark.
pub const QUEST_FLAG_DELIVER_MORE: u32 = 0x100;

pub fn shareable(quest_flags: u32) -> bool {
    quest_flags & QUEST_FLAG_SHARABLE != 0
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ShareCandidate {
    pub same_partition: bool,
    pub in_range: bool,
    pub on_quest: bool,
    pub finished: bool,
    pub log_full: bool,
    pub gates_ok: bool,
    pub holds_quest_share: bool,
}

pub fn share_result(member: ShareCandidate) -> u8 {
    if !member.same_partition || !member.in_range {
        share_result::TOO_FAR
    } else if member.on_quest {
        share_result::HAVE_QUEST
    } else if member.finished {
        share_result::FINISH_QUEST
    } else if member.log_full {
        share_result::LOG_FULL
    } else if !member.gates_ok {
        share_result::CANT_TAKE_QUEST
    } else if member.holds_quest_share {
        share_result::BUSY
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
    fn a_quest_cost_is_offered_as_negative_money() {
        assert_eq!(offered_money(150, 0), 150);
        assert_eq!(offered_money(0, 250), -250);
        assert_eq!(offered_money(0, 0), 0);
    }

    #[test]
    fn quest_xp_is_full_up_to_five_levels_above_the_quest() {
        assert_eq!(quest_xp_for_level(110, 10, 1), 110);
        assert_eq!(quest_xp_for_level(110, 10, 15), 110);
    }

    #[test]
    fn quest_xp_falls_to_a_tenth_by_the_vmangos_steps() {
        let steps = [(16, 88), (17, 66), (18, 44), (19, 22), (20, 11), (60, 11)];
        for (player_level, expected) in steps {
            assert_eq!(quest_xp_for_level(110, 10, player_level), expected);
        }
    }

    #[test]
    fn quest_xp_rounds_up_in_single_precision_like_the_reference_server() {
        assert_eq!(quest_xp_for_level(100, 10, 16), 80);
        assert_eq!(quest_xp_for_level(100, 10, 17), 61);
        assert_eq!(quest_xp_for_level(1, 10, 60), 1);
    }

    #[test]
    fn a_quest_without_xp_grants_none_at_any_level() {
        assert_eq!(quest_xp_for_level(0, 10, 10), 0);
        assert_eq!(quest_xp_for_level(0, 10, 40), 0);
    }

    #[test]
    fn share_result_priority_order() {
        let ready = ShareCandidate {
            same_partition: true,
            in_range: true,
            gates_ok: true,
            ..ShareCandidate::default()
        };
        let blocked = ShareCandidate {
            on_quest: true,
            finished: true,
            log_full: true,
            gates_ok: false,
            holds_quest_share: true,
            ..ready
        };
        assert_eq!(share_result(ready), share_result::SHARING_QUEST);
        for far in [
            ShareCandidate {
                same_partition: false,
                ..blocked
            },
            ShareCandidate {
                same_partition: false,
                ..ready
            },
            ShareCandidate {
                in_range: false,
                ..ready
            },
        ] {
            assert_eq!(share_result(far), share_result::TOO_FAR);
        }
        assert_eq!(share_result(blocked), share_result::HAVE_QUEST);
        let blocked = ShareCandidate {
            on_quest: false,
            ..blocked
        };
        assert_eq!(share_result(blocked), share_result::FINISH_QUEST);
        let blocked = ShareCandidate {
            finished: false,
            ..blocked
        };
        assert_eq!(share_result(blocked), share_result::LOG_FULL);
        let blocked = ShareCandidate {
            log_full: false,
            ..blocked
        };
        assert_eq!(share_result(blocked), share_result::CANT_TAKE_QUEST);
        let blocked = ShareCandidate {
            gates_ok: true,
            ..blocked
        };
        assert_eq!(share_result(blocked), share_result::BUSY);
    }

    #[test]
    fn only_the_sharable_flag_makes_a_quest_shareable() {
        assert!(!shareable(0));
        assert!(!shareable(0x2 | 0x4));
        assert!(shareable(0x8));
        assert!(shareable(0x8 | 0x2));
    }
}

pub mod quest_role {
    pub const START: u8 = 0;
    pub const END: u8 = 1;
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

#[derive(Clone, Debug, Default)]
pub struct AcceptanceRequirements {
    pub quest_entry: u32,
    pub min_level: u32,
    /// Zero admits every level.
    pub max_level: u32,
    /// Zero stands for the Character's own level.
    pub quest_level: u32,
    pub required_races: u32,
    pub required_classes: u32,
    pub repeatable: bool,
    /// Positive: only one quest of the group. Negative: a successor needs every quest of the group.
    pub exclusive_group: i32,
    pub next_in_chain: u32,
    /// Quests whose `NextQuestInChain` names this quest; one in the log closes it.
    pub prev_chain: Vec<u32>,
    pub quest_flags: u32,
    /// The first one met decides; vmangos's order is unspecified.
    pub prerequisites: Vec<Prerequisite>,
    pub required_skill: Option<SkillRequirement>,
    /// Met at this reputation value or above.
    pub min_reputation: Option<ReputationRequirement>,
    /// Met below this reputation value.
    pub max_reputation: Option<ReputationRequirement>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillRequirement {
    pub skill_line: u32,
    /// Met at this skill value or above.
    pub value: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReputationRequirement {
    pub faction_id: u32,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prerequisite {
    pub quest_entry: u32,
    /// Met while the quest is in the log, instead of once it is rewarded.
    pub must_be_active: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuestStanding {
    pub active: bool,
    pub rewarded: bool,
    pub repeatable: bool,
}

impl QuestStanding {
    // vmangos QUEST_STATUS_INCOMPLETE or _COMPLETE; a rewarded repeatable quest returns to NONE.
    fn taken_or_done(self) -> bool {
        self.active || (self.rewarded && !self.repeatable)
    }
}

pub trait QuestHistory {
    fn standing(&self, quest_entry: u32) -> QuestStanding;
    fn exclusive_group(&self, quest_entry: u32) -> i32;
    fn exclusive_group_members(&self, group: i32) -> Vec<u32>;
    /// Stored standing, else the Character's Faction.dbc base standing.
    fn reputation(&self, faction_id: u32) -> i32;
    /// vmangos `GetSkillValue`: with temporary and permanent bonuses; zero when unlearned.
    fn skill_value(&self, skill_line: u32) -> u32;
}

#[derive(Clone, Copy, Debug)]
pub struct Applicant {
    pub level: u32,
    pub race: u8,
    pub class: u8,
}

#[derive(Clone, Copy)]
pub struct Attempt {
    pub rewarded: bool,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AcceptanceRefusal {
    AboveMaxLevel,
    Level,
    Race,
    Class,
    Prerequisite,
    AlreadyTaken,
    Skill,
    Reputation,
}

/// vmangos `Player::CanTakeQuest`, in its check order.
pub fn acceptance(
    requirements: &AcceptanceRequirements,
    applicant: Applicant,
    existing: Option<Attempt>,
    history: &impl QuestHistory,
) -> Result<(), AcceptanceRefusal> {
    if requirements.max_level != 0 && requirements.max_level < applicant.level {
        return Err(AcceptanceRefusal::AboveMaxLevel);
    }
    if !untaken(requirements, existing) {
        return Err(AcceptanceRefusal::AlreadyTaken);
    }
    if !exclusive_group_open(requirements, history) {
        return Err(AcceptanceRefusal::Prerequisite);
    }
    if !mask_admits(
        requirements.required_classes,
        applicant.class,
        ZeroMask::AdmitsAll,
    ) {
        return Err(AcceptanceRefusal::Class);
    }
    if !mask_admits(
        requirements.required_races,
        applicant.race,
        ZeroMask::AdmitsAll,
    ) {
        return Err(AcceptanceRefusal::Race);
    }
    if applicant.level < requirements.min_level {
        return Err(AcceptanceRefusal::Level);
    }
    if !skill_met(requirements, history) {
        return Err(AcceptanceRefusal::Skill);
    }
    if !reputation_met(requirements, history) {
        return Err(AcceptanceRefusal::Reputation);
    }
    if !prerequisites_met(requirements, history) || !chain_open(requirements, history) {
        return Err(AcceptanceRefusal::Prerequisite);
    }
    Ok(())
}

pub const HIGH_LEVEL_HIDE_DIFF: u32 = 7;
pub const LOW_LEVEL_HIDE_DIFF: u32 = 4;

/// How a quest giver marks one quest it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartSighting {
    Hidden,
    /// Within `HIGH_LEVEL_HIDE_DIFF` levels below the quest's minimum level.
    Upcoming,
    Offered,
    /// More than `LOW_LEVEL_HIDE_DIFF` levels below the Character.
    Trivial,
    /// A repeatable quest the Character was rewarded for and can take again.
    RewardedRepeatable,
}

/// vmangos `CanSeeStartQuest` and the giver half of `GetDialogStatus`; no maximum level check.
pub fn start_sighting(
    requirements: &AcceptanceRequirements,
    applicant: Applicant,
    existing: Option<Attempt>,
    history: &impl QuestHistory,
) -> StartSighting {
    let seen = untaken(requirements, existing)
        && mask_admits(
            requirements.required_classes,
            applicant.class,
            ZeroMask::AdmitsAll,
        )
        && mask_admits(
            requirements.required_races,
            applicant.race,
            ZeroMask::AdmitsAll,
        )
        && skill_met(requirements, history)
        && exclusive_group_open(requirements, history)
        && reputation_met(requirements, history)
        && prerequisites_met(requirements, history)
        && chain_open(requirements, history);
    let quest_level = match requirements.quest_level {
        0 => applicant.level,
        level => level,
    };
    if !seen || applicant.level.saturating_add(HIGH_LEVEL_HIDE_DIFF) < requirements.min_level {
        StartSighting::Hidden
    } else if applicant.level < requirements.min_level {
        StartSighting::Upcoming
    } else if rewarded_repeatable(requirements, existing) {
        StartSighting::RewardedRepeatable
    } else if applicant.level > quest_level.saturating_add(LOW_LEVEL_HIDE_DIFF) {
        StartSighting::Trivial
    } else {
        StartSighting::Offered
    }
}

fn rewarded_repeatable(requirements: &AcceptanceRequirements, existing: Option<Attempt>) -> bool {
    requirements.repeatable
        && requirements.quest_flags & QUEST_FLAG_DELIVER_MORE == 0
        && existing.is_some_and(|attempt| attempt.rewarded)
}

fn untaken(requirements: &AcceptanceRequirements, existing: Option<Attempt>) -> bool {
    existing.is_none_or(|attempt| attempt.failed || (requirements.repeatable && attempt.rewarded))
}

fn exclusive_group_open(
    requirements: &AcceptanceRequirements,
    history: &impl QuestHistory,
) -> bool {
    requirements.exclusive_group <= 0
        || history
            .exclusive_group_members(requirements.exclusive_group)
            .into_iter()
            .all(|member| {
                member == requirements.quest_entry || !history.standing(member).taken_or_done()
            })
}

fn prerequisites_met(requirements: &AcceptanceRequirements, history: &impl QuestHistory) -> bool {
    if requirements.prerequisites.is_empty() {
        return true;
    }
    let met = |quest_entry, must_be_active| {
        let standing = history.standing(quest_entry);
        if must_be_active {
            standing.active
        } else {
            standing.rewarded
        }
    };
    let Some(first) = requirements
        .prerequisites
        .iter()
        .find(|prerequisite| met(prerequisite.quest_entry, prerequisite.must_be_active))
    else {
        return false;
    };
    let group = history.exclusive_group(first.quest_entry);
    group >= 0
        || history
            .exclusive_group_members(group)
            .into_iter()
            .all(|member| member == first.quest_entry || met(member, first.must_be_active))
}

fn skill_met(requirements: &AcceptanceRequirements, history: &impl QuestHistory) -> bool {
    requirements
        .required_skill
        .is_none_or(|skill| history.skill_value(skill.skill_line) >= skill.value)
}

fn reputation_met(requirements: &AcceptanceRequirements, history: &impl QuestHistory) -> bool {
    requirements
        .min_reputation
        .is_none_or(|min| history.reputation(min.faction_id) >= min.value)
        && requirements
            .max_reputation
            .is_none_or(|max| history.reputation(max.faction_id) < max.value)
}

/// vmangos `SatisfyQuestNextChain` and `SatisfyQuestPrevChain`.
fn chain_open(requirements: &AcceptanceRequirements, history: &impl QuestHistory) -> bool {
    (requirements.next_in_chain == 0
        || !history.standing(requirements.next_in_chain).taken_or_done())
        && !requirements
            .prev_chain
            .iter()
            .any(|&earlier| history.standing(earlier).active)
}

impl AcceptanceRefusal {
    pub fn text(self, min_level: u32) -> String {
        match self {
            Self::AboveMaxLevel => "your level is too high for that quest".to_string(),
            Self::Level => format!("requires level {min_level}"),
            Self::Race => "quest not available to your race".to_string(),
            Self::Class => "quest not available to your class".to_string(),
            Self::Prerequisite => "must complete the prerequisite quest first".to_string(),
            Self::AlreadyTaken => "already on or completed that quest".to_string(),
            Self::Skill => "your skill is too low for that quest".to_string(),
            Self::Reputation => "your reputation does not allow that quest".to_string(),
        }
    }
}

/// `Quest::XPValue`: full XP up to five levels above the quest, then 80%, 60%, 40%, 20% and 10%.
pub fn quest_xp_for_level(reward_xp: u32, quest_level: u32, player_level: u32) -> u32 {
    let full = reward_xp as f32;
    let factor = match player_level.saturating_sub(quest_level) {
        0..=5 => return reward_xp,
        6 => 0.8f32,
        7 => 0.6,
        8 => 0.4,
        9 => 0.2,
        _ => 0.1,
    };
    (full * factor).ceil() as u32
}

pub const QUEST_LOG_FULL_TEXT: &str = "quest log is full";

/// The signed `RewOrReqMoney` wire value: a quest cost shows as a negative reward.
pub fn offered_money(reward_money: u32, required_money: u32) -> i32 {
    reward_money.wrapping_sub(required_money) as i32
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

    #[derive(Default)]
    struct History {
        standings: std::collections::BTreeMap<u32, QuestStanding>,
        groups: std::collections::BTreeMap<u32, i32>,
        reputations: std::collections::BTreeMap<u32, i32>,
        skills: std::collections::BTreeMap<u32, u32>,
    }

    impl History {
        fn with(mut self, quest_entry: u32, standing: QuestStanding) -> Self {
            self.standings.insert(quest_entry, standing);
            self
        }

        fn grouped(mut self, group: i32, members: &[u32]) -> Self {
            for member in members {
                self.groups.insert(*member, group);
            }
            self
        }

        fn reputation(mut self, faction_id: u32, value: i32) -> Self {
            self.reputations.insert(faction_id, value);
            self
        }

        fn skill(mut self, skill_line: u32, value: u32) -> Self {
            self.skills.insert(skill_line, value);
            self
        }
    }

    impl QuestHistory for History {
        fn standing(&self, quest_entry: u32) -> QuestStanding {
            self.standings
                .get(&quest_entry)
                .copied()
                .unwrap_or_default()
        }

        fn exclusive_group(&self, quest_entry: u32) -> i32 {
            self.groups.get(&quest_entry).copied().unwrap_or(0)
        }

        fn exclusive_group_members(&self, group: i32) -> Vec<u32> {
            self.groups
                .iter()
                .filter(|(_, member_group)| **member_group == group)
                .map(|(member, _)| *member)
                .collect()
        }

        fn reputation(&self, faction_id: u32) -> i32 {
            self.reputations.get(&faction_id).copied().unwrap_or(0)
        }

        fn skill_value(&self, skill_line: u32) -> u32 {
            self.skills.get(&skill_line).copied().unwrap_or(0)
        }
    }

    const ACTIVE: QuestStanding = QuestStanding {
        active: true,
        rewarded: false,
        repeatable: false,
    };
    const REWARDED: QuestStanding = QuestStanding {
        active: false,
        rewarded: true,
        repeatable: false,
    };
    const QUEST: u32 = 100;

    fn requirements(repeatable: bool) -> AcceptanceRequirements {
        AcceptanceRequirements {
            quest_entry: QUEST,
            min_level: 10,
            quest_level: 12,
            required_races: 1,
            required_classes: 2,
            repeatable,
            ..AcceptanceRequirements::default()
        }
    }

    fn applicant(level: u32, race: u8, class: u8) -> Applicant {
        Applicant { level, race, class }
    }

    fn eligible() -> Applicant {
        applicant(10, 1, 2)
    }

    fn after(prerequisites: &[(u32, bool)]) -> AcceptanceRequirements {
        AcceptanceRequirements {
            prerequisites: prerequisites
                .iter()
                .map(|&(quest_entry, must_be_active)| Prerequisite {
                    quest_entry,
                    must_be_active,
                })
                .collect(),
            ..requirements(false)
        }
    }

    fn accept(
        requirements: &AcceptanceRequirements,
        history: &History,
    ) -> Result<(), AcceptanceRefusal> {
        acceptance(requirements, eligible(), None, history)
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
        let history = History::default();
        assert_eq!(
            acceptance(&requirements(false), eligible(), failed, &history),
            Ok(())
        );
        assert_eq!(
            acceptance(&requirements(false), eligible(), rewarded, &history),
            Err(AcceptanceRefusal::AlreadyTaken)
        );
        assert_eq!(
            acceptance(&requirements(true), eligible(), rewarded, &history),
            Ok(())
        );
        assert_eq!(
            acceptance(
                &requirements(true),
                eligible(),
                Some(Attempt {
                    failed: false,
                    rewarded: false
                }),
                &history
            ),
            Err(AcceptanceRefusal::AlreadyTaken)
        );
    }

    #[test]
    fn checks_refuse_in_the_vmangos_can_take_quest_order() {
        let taken = Some(Attempt {
            failed: false,
            rewarded: false,
        });
        let capped = AcceptanceRequirements {
            max_level: 9,
            exclusive_group: 5,
            prerequisites: vec![Prerequisite {
                quest_entry: 7,
                must_be_active: false,
            }],
            ..requirements(false)
        };
        let history = History::default().grouped(5, &[QUEST, 6]).with(6, ACTIVE);
        let wrong = applicant(10, 2, 1);
        assert_eq!(
            acceptance(&capped, wrong, taken, &history),
            Err(AcceptanceRefusal::AboveMaxLevel)
        );
        let uncapped = AcceptanceRequirements {
            max_level: 0,
            ..capped
        };
        assert_eq!(
            acceptance(&uncapped, applicant(9, 2, 1), taken, &history),
            Err(AcceptanceRefusal::AlreadyTaken)
        );
        assert_eq!(
            acceptance(&uncapped, applicant(9, 2, 1), None, &history),
            Err(AcceptanceRefusal::Prerequisite)
        );
        let open_group = History::default();
        assert_eq!(
            acceptance(&uncapped, applicant(9, 2, 1), None, &open_group),
            Err(AcceptanceRefusal::Class)
        );
        assert_eq!(
            acceptance(&uncapped, applicant(9, 2, 2), None, &open_group),
            Err(AcceptanceRefusal::Race)
        );
        assert_eq!(
            acceptance(&uncapped, applicant(9, 1, 2), None, &open_group),
            Err(AcceptanceRefusal::Level)
        );
        assert_eq!(
            acceptance(&uncapped, eligible(), None, &open_group),
            Err(AcceptanceRefusal::Prerequisite)
        );
        let gated = AcceptanceRequirements {
            required_skill: Some(SkillRequirement {
                skill_line: SKILL_LINE,
                value: 50,
            }),
            min_reputation: Some(ReputationRequirement {
                faction_id: FACTION,
                value: FRIENDLY,
            }),
            ..uncapped
        };
        assert_eq!(
            acceptance(&gated, applicant(9, 1, 2), None, &open_group),
            Err(AcceptanceRefusal::Level)
        );
        assert_eq!(
            acceptance(&gated, eligible(), None, &open_group),
            Err(AcceptanceRefusal::Skill)
        );
        let skilled = History::default().skill(SKILL_LINE, 50);
        assert_eq!(
            acceptance(&gated, eligible(), None, &skilled),
            Err(AcceptanceRefusal::Reputation)
        );
        let skilled_and_friendly = History::default()
            .skill(SKILL_LINE, 50)
            .reputation(FACTION, FRIENDLY);
        assert_eq!(
            acceptance(&gated, eligible(), None, &skilled_and_friendly),
            Err(AcceptanceRefusal::Prerequisite)
        );
    }

    #[test]
    fn a_maximum_level_refuses_only_characters_above_it() {
        let capped = AcceptanceRequirements {
            max_level: 10,
            ..requirements(false)
        };
        let history = History::default();
        assert_eq!(acceptance(&capped, eligible(), None, &history), Ok(()));
        assert_eq!(
            acceptance(&capped, applicant(11, 1, 2), None, &history),
            Err(AcceptanceRefusal::AboveMaxLevel)
        );
    }

    #[test]
    fn any_rewarded_or_active_prerequisite_unlocks_the_quest() {
        let quest = after(&[(1, false), (2, true)]);
        assert_eq!(
            accept(&quest, &History::default()),
            Err(AcceptanceRefusal::Prerequisite)
        );
        assert_eq!(
            accept(&quest, &History::default().with(1, REWARDED)),
            Ok(())
        );
        assert_eq!(accept(&quest, &History::default().with(2, ACTIVE)), Ok(()));
        assert_eq!(
            accept(
                &quest,
                &History::default().with(1, ACTIVE).with(2, REWARDED)
            ),
            Err(AcceptanceRefusal::Prerequisite),
            "a rewarded prerequisite needs a reward and an active one needs the log"
        );
    }

    #[test]
    fn a_negative_group_prerequisite_needs_every_quest_of_its_group() {
        let quest = after(&[(1, false)]);
        let partial = History::default().grouped(-3, &[1, 2]).with(1, REWARDED);
        assert_eq!(
            accept(&quest, &partial),
            Err(AcceptanceRefusal::Prerequisite)
        );
        assert_eq!(accept(&quest, &partial.with(2, REWARDED)), Ok(()));

        let active = after(&[(1, true)]);
        let partial = History::default().grouped(-3, &[1, 2]).with(1, ACTIVE);
        assert_eq!(
            accept(&active, &partial),
            Err(AcceptanceRefusal::Prerequisite)
        );
        assert_eq!(accept(&active, &partial.with(2, ACTIVE)), Ok(()));
    }

    #[test]
    fn a_positive_group_prerequisite_needs_only_itself() {
        let quest = after(&[(1, false)]);
        let history = History::default().grouped(3, &[1, 2]).with(1, REWARDED);
        assert_eq!(accept(&quest, &history), Ok(()));
    }

    #[test]
    fn a_positive_exclusive_group_admits_one_taken_or_done_quest() {
        let quest = AcceptanceRequirements {
            exclusive_group: 4,
            ..requirements(false)
        };
        let group = || History::default().grouped(4, &[QUEST, 2]);
        assert_eq!(accept(&quest, &group()), Ok(()));
        assert_eq!(
            accept(&quest, &group().with(2, ACTIVE)),
            Err(AcceptanceRefusal::Prerequisite)
        );
        assert_eq!(
            accept(&quest, &group().with(2, REWARDED)),
            Err(AcceptanceRefusal::Prerequisite)
        );
        let repeated = QuestStanding {
            repeatable: true,
            ..REWARDED
        };
        assert_eq!(
            accept(&quest, &group().with(2, repeated)),
            Ok(()),
            "a rewarded repeatable quest no longer counts as taken"
        );
        assert_eq!(
            accept(&quest, &group().with(QUEST, ACTIVE)),
            Ok(()),
            "the quest's own standing is the attempt's concern"
        );
    }

    #[test]
    fn a_negative_exclusive_group_does_not_limit_its_own_quests() {
        let quest = AcceptanceRequirements {
            exclusive_group: -4,
            ..requirements(false)
        };
        let history = History::default().grouped(-4, &[QUEST, 2]).with(2, ACTIVE);
        assert_eq!(accept(&quest, &history), Ok(()));
    }

    #[test]
    fn a_taken_or_done_next_quest_in_chain_closes_the_quest() {
        let quest = AcceptanceRequirements {
            next_in_chain: 2,
            ..requirements(false)
        };
        assert_eq!(accept(&quest, &History::default()), Ok(()));
        assert_eq!(
            accept(&quest, &History::default().with(2, ACTIVE)),
            Err(AcceptanceRefusal::Prerequisite)
        );
        assert_eq!(
            accept(&quest, &History::default().with(2, REWARDED)),
            Err(AcceptanceRefusal::Prerequisite)
        );
    }

    #[test]
    fn an_earlier_quest_of_the_chain_in_the_log_closes_the_quest() {
        let quest = AcceptanceRequirements {
            prev_chain: vec![2, 3],
            ..requirements(false)
        };
        let failed = QuestStanding {
            active: false,
            ..ACTIVE
        };
        assert_eq!(accept(&quest, &History::default()), Ok(()));
        assert_eq!(
            accept(&quest, &History::default().with(2, REWARDED)),
            Ok(())
        );
        assert_eq!(accept(&quest, &History::default().with(3, failed)), Ok(()));
        for earlier in [2, 3] {
            let history = History::default().with(earlier, ACTIVE);
            assert_eq!(
                accept(&quest, &history),
                Err(AcceptanceRefusal::Prerequisite)
            );
            assert_eq!(
                start_sighting(&quest, eligible(), None, &history),
                StartSighting::Hidden
            );
        }
    }

    fn rewarded_attempt() -> Option<Attempt> {
        Some(Attempt {
            failed: false,
            rewarded: true,
        })
    }

    #[test]
    fn a_rewarded_repeatable_quest_shows_the_repeat_sighting_from_its_minimum_level() {
        let quest = requirements(true);
        let sighted = |level| {
            start_sighting(
                &quest,
                applicant(level, 1, 2),
                rewarded_attempt(),
                &History::default(),
            )
        };
        assert_eq!(sighted(2), StartSighting::Hidden);
        assert_eq!(sighted(9), StartSighting::Upcoming);
        assert_eq!(sighted(10), StartSighting::RewardedRepeatable);
        assert_eq!(sighted(60), StartSighting::RewardedRepeatable);
    }

    #[test]
    fn a_repeatable_quest_without_a_reward_or_with_the_deliver_more_flag_shows_no_repeat_sighting()
    {
        let sighted = |quest: &AcceptanceRequirements, existing| {
            start_sighting(quest, eligible(), existing, &History::default())
        };
        assert_eq!(sighted(&requirements(true), None), StartSighting::Offered);
        let deliver_more = AcceptanceRequirements {
            quest_flags: QUEST_FLAG_DELIVER_MORE,
            ..requirements(true)
        };
        assert_eq!(
            sighted(&deliver_more, rewarded_attempt()),
            StartSighting::Offered
        );
        assert_eq!(
            sighted(&requirements(false), rewarded_attempt()),
            StartSighting::Hidden
        );
    }

    const FACTION: u32 = 72;
    const FRIENDLY: i32 = 3000;
    const HONORED: i32 = 9000;
    const SKILL_LINE: u32 = 185;

    #[test]
    fn a_minimum_reputation_admits_its_value_and_above() {
        let quest = AcceptanceRequirements {
            min_reputation: Some(ReputationRequirement {
                faction_id: FACTION,
                value: FRIENDLY,
            }),
            ..requirements(false)
        };
        let at = |value| History::default().reputation(FACTION, value);
        assert_eq!(
            accept(&quest, &at(FRIENDLY - 1)),
            Err(AcceptanceRefusal::Reputation)
        );
        assert_eq!(accept(&quest, &at(FRIENDLY)), Ok(()));
        assert_eq!(
            start_sighting(&quest, eligible(), None, &at(FRIENDLY - 1)),
            StartSighting::Hidden
        );
        assert_eq!(
            start_sighting(&quest, eligible(), None, &at(FRIENDLY)),
            StartSighting::Offered
        );
    }

    #[test]
    fn a_maximum_reputation_admits_only_values_below_it() {
        let quest = AcceptanceRequirements {
            max_reputation: Some(ReputationRequirement {
                faction_id: FACTION,
                value: HONORED,
            }),
            ..requirements(false)
        };
        let at = |value| History::default().reputation(FACTION, value);
        assert_eq!(accept(&quest, &at(HONORED - 1)), Ok(()));
        assert_eq!(
            accept(&quest, &at(HONORED)),
            Err(AcceptanceRefusal::Reputation)
        );
        assert_eq!(
            start_sighting(&quest, eligible(), None, &at(HONORED)),
            StartSighting::Hidden
        );
    }

    #[test]
    fn a_required_skill_admits_its_value_and_above() {
        let quest = AcceptanceRequirements {
            required_skill: Some(SkillRequirement {
                skill_line: SKILL_LINE,
                value: 50,
            }),
            ..requirements(false)
        };
        let at = |value| History::default().skill(SKILL_LINE, value);
        assert_eq!(accept(&quest, &at(49)), Err(AcceptanceRefusal::Skill));
        assert_eq!(accept(&quest, &at(50)), Ok(()));
        assert_eq!(
            start_sighting(&quest, eligible(), None, &at(49)),
            StartSighting::Hidden
        );
    }

    fn sighting(level: u32, quest: &AcceptanceRequirements) -> StartSighting {
        start_sighting(quest, applicant(level, 1, 2), None, &History::default())
    }

    #[test]
    fn a_quest_up_to_seven_levels_ahead_is_upcoming() {
        let quest = requirements(false);
        assert_eq!(sighting(2, &quest), StartSighting::Hidden);
        assert_eq!(sighting(3, &quest), StartSighting::Upcoming);
        assert_eq!(sighting(9, &quest), StartSighting::Upcoming);
        assert_eq!(sighting(10, &quest), StartSighting::Offered);
    }

    #[test]
    fn a_quest_more_than_four_levels_below_the_character_is_trivial() {
        let quest = requirements(false);
        assert_eq!(sighting(16, &quest), StartSighting::Offered);
        assert_eq!(sighting(17, &quest), StartSighting::Trivial);
        let own_level = AcceptanceRequirements {
            quest_level: 0,
            ..requirements(false)
        };
        assert_eq!(sighting(60, &own_level), StartSighting::Offered);
    }

    #[test]
    fn the_sighting_hides_what_acceptance_refuses_except_the_level_limits() {
        let capped = AcceptanceRequirements {
            max_level: 11,
            ..requirements(false)
        };
        assert_eq!(sighting(12, &capped), StartSighting::Offered);
        assert_eq!(
            start_sighting(
                &requirements(false),
                applicant(10, 2, 2),
                None,
                &History::default()
            ),
            StartSighting::Hidden
        );
        assert_eq!(
            start_sighting(
                &requirements(false),
                applicant(10, 1, 1),
                None,
                &History::default()
            ),
            StartSighting::Hidden
        );
        assert_eq!(
            start_sighting(
                &after(&[(1, false)]),
                applicant(9, 1, 2),
                None,
                &History::default()
            ),
            StartSighting::Hidden
        );
        let grouped = AcceptanceRequirements {
            exclusive_group: 4,
            ..requirements(false)
        };
        let taken = History::default().grouped(4, &[QUEST, 2]).with(2, ACTIVE);
        assert_eq!(
            start_sighting(&grouped, eligible(), None, &taken),
            StartSighting::Hidden
        );
        let chained = AcceptanceRequirements {
            next_in_chain: 2,
            ..requirements(false)
        };
        assert_eq!(
            start_sighting(
                &chained,
                eligible(),
                None,
                &History::default().with(2, REWARDED)
            ),
            StartSighting::Hidden
        );
        let active = Some(Attempt {
            failed: false,
            rewarded: false,
        });
        assert_eq!(
            start_sighting(
                &requirements(false),
                eligible(),
                active,
                &History::default()
            ),
            StartSighting::Hidden
        );
    }
}
