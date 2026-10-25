use super::*;
use wow_world_base::shared::quest_party_message_vanilla_tbc::QuestPartyMessage;

pub const ROLE_START: u8 = 0;
pub const ROLE_END: u8 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct QuestRewardView {
    pub item_entry: u32,
    pub count: u32,
    pub display_id: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuestDetailView {
    pub quest_id: u32,
    pub quest_level: u32,
    pub zone_or_sort: i32,
    pub title: String,
    pub details: String,
    pub objectives_text: String,
    pub offer_reward_text: String,
    pub request_items_text: String,
    pub money_reward: u32,
    pub reward_xp: u32,
    pub next_quest_id: u32,
    pub max_level_money_reward: u32,
    pub rewards: Vec<QuestRewardView>,
    pub choice_rewards: Vec<QuestRewardView>,
    /// Gameobject objectives encode the negated entry id as a u32.
    pub objectives: Vec<(u32, u32, u32, u32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GiverQuestEval {
    pub quest_id: u32,
    pub title: String,
    pub level: u32,
    pub role: u8,
    pub startable: bool,
    pub active: bool,
    pub complete: bool,
}

pub fn eval_status(e: &GiverQuestEval) -> QuestGiverStatus {
    if e.role == ROLE_END && e.active {
        if e.complete {
            QuestGiverStatus::Reward2
        } else {
            QuestGiverStatus::Incomplete
        }
    } else if e.startable {
        QuestGiverStatus::Available
    } else {
        QuestGiverStatus::None
    }
}

fn status_rank(s: QuestGiverStatus) -> u8 {
    match s {
        QuestGiverStatus::Reward2 | QuestGiverStatus::RewardRep | QuestGiverStatus::RewardOld => 3,
        QuestGiverStatus::Available => 2,
        QuestGiverStatus::Incomplete => 1,
        _ => 0,
    }
}

pub fn quest_giver_status(evals: &[GiverQuestEval]) -> QuestGiverStatus {
    evals.iter().fold(QuestGiverStatus::None, |best, e| {
        let s = eval_status(e);
        if status_rank(s) > status_rank(best) {
            s
        } else {
            best
        }
    })
}

pub fn build_questgiver_status(
    giver_guid: u64,
    status: QuestGiverStatus,
) -> SMSG_QUESTGIVER_STATUS {
    SMSG_QUESTGIVER_STATUS {
        guid: Guid::new(giver_guid),
        status,
    }
}

pub fn build_questupdate_add_kill(
    quest_id: u32,
    creature_id: u32,
    kill_count: u32,
    required: u32,
) -> SMSG_QUESTUPDATE_ADD_KILL {
    SMSG_QUESTUPDATE_ADD_KILL {
        quest_id,
        creature_id,
        kill_count,
        required_kill_count: required,
        guid: Guid::new(0),
    }
}

pub fn kill_progress_add_kills(
    quest_entry: u32,
    old_counts: &[u32],
    new_counts: &[u32],
    objectives: &[(u8, u8, u32, u32)],
) -> Vec<SMSG_QUESTUPDATE_ADD_KILL> {
    objectives
        .iter()
        .filter_map(|&(kind, obj_index, target_entry, required)| {
            if kind != 0 {
                return None;
            }
            let i = obj_index as usize;
            let new_c = new_counts.get(i).copied().unwrap_or(0);
            (new_c > old_counts.get(i).copied().unwrap_or(0))
                .then(|| build_questupdate_add_kill(quest_entry, target_entry, new_c, required))
        })
        .collect()
}

#[cfg(test)]
mod add_kill_tests {
    use super::*;

    #[test]
    fn emits_one_per_incremented_kill_objective() {
        let objs = [(0u8, 0u8, 257u32, 10u32), (1, 1, 999, 5)];
        let out = kill_progress_add_kills(15, &[2, 4], &[3, 5], &objs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].quest_id, 15);
        assert_eq!(out[0].creature_id, 257);
        assert_eq!(out[0].kill_count, 3);
        assert_eq!(out[0].required_kill_count, 10);
        assert!(kill_progress_add_kills(15, &[3, 5], &[3, 5], &objs).is_empty());
    }
}

pub fn build_questupdate_failedtimer(quest_id: u32) -> SMSG_QUESTUPDATE_FAILEDTIMER {
    SMSG_QUESTUPDATE_FAILEDTIMER { quest_id }
}

/// `SMSG_QUESTGIVER_QUEST_INVALID` (0x018F): reason u32.
pub fn build_questgiver_quest_invalid(
    reason: wow_world_messages::vanilla::QuestFailedReason,
) -> wow_world_messages::vanilla::SMSG_QUESTGIVER_QUEST_INVALID {
    wow_world_messages::vanilla::SMSG_QUESTGIVER_QUEST_INVALID { msg: reason }
}

/// `SMSG_QUESTGIVER_QUEST_FAILED` (0x0192): quest id u32, reason u32.
pub fn build_questgiver_quest_failed(
    quest_id: u32,
    reason: wow_world_messages::vanilla::QuestFailedReason,
) -> wow_world_messages::vanilla::SMSG_QUESTGIVER_QUEST_FAILED {
    wow_world_messages::vanilla::SMSG_QUESTGIVER_QUEST_FAILED { quest_id, reason }
}

/// `SMSG_QUESTUPDATE_COMPLETE` (0x0198): quest id u32.
pub fn build_questupdate_complete(
    quest_id: u32,
) -> wow_world_messages::vanilla::SMSG_QUESTUPDATE_COMPLETE {
    wow_world_messages::vanilla::SMSG_QUESTUPDATE_COMPLETE { quest_id }
}

pub fn build_quest_push_result(member_guid: u64, code: u8) -> MSG_QUEST_PUSH_RESULT {
    MSG_QUEST_PUSH_RESULT {
        guid: Guid::new(member_guid),
        message: QuestPartyMessage::try_from(code).unwrap_or(QuestPartyMessage::SharingQuest),
    }
}

pub fn quest_menu_items(evals: &[GiverQuestEval]) -> Vec<QuestItem> {
    evals
        .iter()
        .filter_map(|e| {
            let status = menu_icon(e);
            (status_rank(status) > 0).then(|| QuestItem {
                quest_id: e.quest_id,
                quest_icon: status.as_int() as u32,
                level: Level::new(e.level as u8),
                title: e.title.clone(),
            })
        })
        .collect()
}

/// Completed quests use RewardRep, 4, in menus and Reward, 6, above the giver.
fn menu_icon(e: &GiverQuestEval) -> QuestGiverStatus {
    if e.role == ROLE_END && e.active {
        if e.complete {
            QuestGiverStatus::RewardRep
        } else {
            QuestGiverStatus::Incomplete
        }
    } else if e.startable {
        QuestGiverStatus::Available
    } else {
        QuestGiverStatus::None
    }
}

fn reward_items(rewards: &[QuestRewardView]) -> Vec<QuestItemReward> {
    rewards
        .iter()
        .map(|r| QuestItemReward {
            item: r.item_entry,
            item_count: r.count,
        })
        .collect()
}

fn reward_items_with_icon(rewards: &[QuestRewardView]) -> Vec<QuestItemRequirement> {
    rewards
        .iter()
        .map(|r| QuestItemRequirement {
            item: r.item_entry,
            item_count: r.count,
            item_display_id: r.display_id,
        })
        .collect()
}

pub fn build_quest_list(
    giver_guid: u64,
    greeting: &str,
    evals: &[GiverQuestEval],
) -> SMSG_QUESTGIVER_QUEST_LIST {
    SMSG_QUESTGIVER_QUEST_LIST {
        npc: Guid::new(giver_guid),
        title: greeting.to_string(),
        emote_delay: 0,
        emote: 0,
        quest_items: quest_menu_items(evals),
    }
}

/// Build 5875 needs item/count/display triples and four emote/delay pairs; gtker omits them.
pub fn build_quest_details_raw(giver_guid: u64, d: &QuestDetailView) -> (u16, Vec<u8>) {
    const SMSG_QUESTGIVER_QUEST_DETAILS: u16 = 0x0188;
    const DETAILS_EMOTE_COUNT: u32 = 4;

    let mut body = Vec::with_capacity(
        12 + d.title.len()
            + d.details.len()
            + d.objectives_text.len()
            + 1
            + 1
            + 1
            + 4
            + 4
            + (d.choice_rewards.len() + d.rewards.len()) * 12
            + 4
            + 4
            + 4
            + (DETAILS_EMOTE_COUNT as usize) * 8,
    );
    body.extend_from_slice(&giver_guid.to_le_bytes());
    body.extend_from_slice(&d.quest_id.to_le_bytes());
    for text in [&d.title, &d.details, &d.objectives_text] {
        body.extend_from_slice(text.as_bytes());
        body.push(0);
    }
    body.extend_from_slice(&1u32.to_le_bytes());
    for rewards in [&d.choice_rewards, &d.rewards] {
        body.extend_from_slice(&(rewards.len() as u32).to_le_bytes());
        for reward in rewards.iter() {
            body.extend_from_slice(&reward.item_entry.to_le_bytes());
            body.extend_from_slice(&reward.count.to_le_bytes());
            body.extend_from_slice(&reward.display_id.to_le_bytes());
        }
    }
    body.extend_from_slice(&d.money_reward.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes());
    body.extend_from_slice(&DETAILS_EMOTE_COUNT.to_le_bytes());
    body.resize(body.len() + DETAILS_EMOTE_COUNT as usize * 8, 0);
    (SMSG_QUESTGIVER_QUEST_DETAILS, body)
}

pub fn build_offer_reward(giver_guid: u64, d: &QuestDetailView) -> SMSG_QUESTGIVER_OFFER_REWARD {
    SMSG_QUESTGIVER_OFFER_REWARD {
        npc: Guid::new(giver_guid),
        quest_id: d.quest_id,
        title: d.title.clone(),
        offer_reward_text: d.offer_reward_text.clone(),
        auto_finish: true,
        emotes: Vec::<NpcTextUpdateEmote>::new(),
        choice_item_rewards: reward_items_with_icon(&d.choice_rewards),
        item_rewards: reward_items_with_icon(&d.rewards),
        money_reward: Gold::new(d.money_reward),
        reward_spell: 0,
        reward_spell_cast: 0,
    }
}

pub fn build_request_items(
    giver_guid: u64,
    d: &QuestDetailView,
    completable: bool,
) -> SMSG_QUESTGIVER_REQUEST_ITEMS {
    SMSG_QUESTGIVER_REQUEST_ITEMS {
        npc: Guid::new(giver_guid),
        quest_id: d.quest_id,
        title: d.title.clone(),
        request_items_text: d.request_items_text.clone(),
        emote_delay: 0,
        emote: 0,
        auto_finish: false,
        required_money: Gold::new(0),
        required_items: Vec::new(),
        unknown1: 0x02,
        completable: if completable {
            QuestCompletable::Completeable
        } else {
            QuestCompletable::NotCompletable
        },
        flags2: 0x04,
        flags3: 0x08,
    }
}

// gtker writes u16 reputation-faction fields; build 5875 reads u32.
const SMSG_QUEST_QUERY_RESPONSE_OPCODE: u16 = 0x005D;

fn push_cstr(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

pub fn build_quest_query_response_raw(d: &QuestDetailView) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(256);
    let u32le = |body: &mut Vec<u8>, v: u32| body.extend_from_slice(&v.to_le_bytes());
    u32le(&mut body, d.quest_id);
    u32le(&mut body, 2);
    u32le(&mut body, d.quest_level);
    body.extend_from_slice(&d.zone_or_sort.to_le_bytes());
    u32le(&mut body, 0);
    u32le(&mut body, 0);
    u32le(&mut body, 0);
    u32le(&mut body, 0);
    u32le(&mut body, 0);
    u32le(&mut body, d.next_quest_id);
    u32le(&mut body, d.money_reward);
    u32le(&mut body, d.max_level_money_reward);
    u32le(&mut body, 0);
    u32le(&mut body, 0);
    u32le(&mut body, 0);
    for i in 0..4 {
        let (item, count) = d
            .rewards
            .get(i)
            .map(|r| (r.item_entry, r.count))
            .unwrap_or((0, 0));
        u32le(&mut body, item);
        u32le(&mut body, count);
    }
    for i in 0..6 {
        let (item, count) = d
            .choice_rewards
            .get(i)
            .map(|r| (r.item_entry, r.count))
            .unwrap_or((0, 0));
        u32le(&mut body, item);
        u32le(&mut body, count);
    }
    u32le(&mut body, 0);
    body.extend_from_slice(&0f32.to_le_bytes());
    body.extend_from_slice(&0f32.to_le_bytes());
    u32le(&mut body, 0);
    push_cstr(&mut body, &d.title);
    push_cstr(&mut body, &d.objectives_text);
    push_cstr(&mut body, &d.details);
    push_cstr(&mut body, "");
    for i in 0..4 {
        let (creature_or_go, count, req_item, req_item_count) =
            d.objectives.get(i).copied().unwrap_or((0, 0, 0, 0));
        u32le(&mut body, creature_or_go);
        u32le(&mut body, count);
        u32le(&mut body, req_item);
        u32le(&mut body, req_item_count);
    }
    for _ in 0..4 {
        push_cstr(&mut body, "");
    }
    (SMSG_QUEST_QUERY_RESPONSE_OPCODE, body)
}

pub fn build_quest_complete(d: &QuestDetailView) -> SMSG_QUESTGIVER_QUEST_COMPLETE {
    SMSG_QUESTGIVER_QUEST_COMPLETE {
        quest_id: d.quest_id,
        unknown: 0x03,
        experience_reward: d.reward_xp,
        money_reward: Gold::new(d.money_reward),
        item_rewards: reward_items(&d.rewards),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(role: u8, startable: bool, active: bool, complete: bool) -> GiverQuestEval {
        GiverQuestEval {
            quest_id: 7,
            title: "Q".into(),
            level: 2,
            role,
            startable,
            active,
            complete,
        }
    }

    #[test]
    fn status_picks_most_actionable() {
        assert_eq!(
            quest_giver_status(&[ev(ROLE_START, true, false, false)]),
            QuestGiverStatus::Available
        );
        assert_eq!(
            quest_giver_status(&[
                ev(ROLE_START, true, false, false),
                ev(ROLE_END, false, true, true)
            ]),
            QuestGiverStatus::Reward2
        );
        assert_eq!(
            quest_giver_status(&[
                ev(ROLE_END, false, true, false),
                ev(ROLE_START, true, false, false)
            ]),
            QuestGiverStatus::Available
        );
        assert_eq!(
            quest_giver_status(&[ev(ROLE_END, false, true, false)]),
            QuestGiverStatus::Incomplete
        );
        assert_eq!(
            quest_giver_status(&[ev(ROLE_START, false, false, false)]),
            QuestGiverStatus::None
        );
        assert_eq!(quest_giver_status(&[]), QuestGiverStatus::None);
    }

    #[test]
    fn questgiver_status_echoes_the_giver_and_its_overhead_icon() {
        let msg = build_questgiver_status(0xF130_0000_0000_0001, QuestGiverStatus::Available);
        assert_eq!(msg.guid.guid(), 0xF130_0000_0000_0001);
        assert_eq!(msg.status, QuestGiverStatus::Available);
    }

    #[test]
    fn quest_list_filters_to_actionable_and_tags_icons() {
        let evals = [
            ev(ROLE_START, true, false, false),
            ev(ROLE_END, false, true, true),
            ev(ROLE_START, false, false, false),
        ];
        let list = build_quest_list(100, "Greetings", &evals);
        assert_eq!(list.quest_items.len(), 2);
        assert_eq!(
            list.quest_items[0].quest_icon,
            QuestGiverStatus::Available.as_int() as u32
        );
        assert_eq!(
            list.quest_items[1].quest_icon,
            QuestGiverStatus::RewardRep.as_int() as u32
        );
    }

    #[test]
    fn quest_query_response_raw_is_well_formed() {
        let d = QuestDetailView {
            quest_id: 783,
            quest_level: 1,
            zone_or_sort: 12,
            title: "A Threat Within".into(),
            details: "D".into(),
            objectives_text: "O".into(),
            offer_reward_text: String::new(),
            request_items_text: String::new(),
            money_reward: 0,
            reward_xp: 0,
            next_quest_id: 0,
            max_level_money_reward: 0,
            rewards: vec![],
            choice_rewards: vec![],
            objectives: vec![],
        };
        let (opcode, body) = build_quest_query_response_raw(&d);
        assert_eq!(opcode, 0x005D);
        assert_eq!(&body[0..4], &783u32.to_le_bytes());
        assert_eq!(&body[12..16], &12i32.to_le_bytes());
        assert_eq!(&body[156..156 + 15], b"A Threat Within");
        let title_pos = body
            .windows(15)
            .position(|w| w == b"A Threat Within")
            .expect("full title present");
        assert_eq!(title_pos, 156);
        assert_eq!(body[title_pos + 15], 0);
    }

    #[test]
    fn quest_query_response_raw_threads_next_quest_id_and_max_level_money_reward() {
        let d = QuestDetailView {
            next_quest_id: 15,
            money_reward: 500,
            max_level_money_reward: 4200,
            ..detail_view()
        };
        let (_, body) = build_quest_query_response_raw(&d);
        assert_eq!(&body[36..40], &15u32.to_le_bytes(), "next_quest_in_chain");
        assert_eq!(
            &body[40..44],
            &500u32.to_le_bytes(),
            "money_reward (unaffected sibling field)"
        );
        assert_eq!(
            &body[44..48],
            &4200u32.to_le_bytes(),
            "max_level_money_reward"
        );
    }

    #[test]
    fn offer_reward_and_details_arm_auto_finish() {
        let mut d = detail_view();
        d.title = "T".into();

        // Guid, quest id and two CStrings put auto_finish at byte 15.
        let mut framed = Vec::new();
        ServerOpcodeMessage::SMSG_QUESTGIVER_OFFER_REWARD(Box::new(build_offer_reward(100, &d)))
            .write_unencrypted_server(&mut framed)
            .unwrap();
        assert_eq!(
            u16::from_le_bytes([framed[2], framed[3]]),
            0x018D,
            "opcode == SMSG_QUESTGIVER_OFFER_REWARD"
        );
        let body = &framed[4..];
        assert_eq!(
            &body[15..19],
            &1u32.to_le_bytes(),
            "OFFER_REWARD auto_finish (EnableNext) must be 1"
        );

        // Guid, quest id and three CStrings put auto_finish at byte 16.
        let (opcode, body) = build_quest_details_raw(100, &d);
        assert_eq!(opcode, 0x0188, "opcode == SMSG_QUESTGIVER_QUEST_DETAILS");
        assert_eq!(
            &body[16..20],
            &1u32.to_le_bytes(),
            "DETAILS auto_finish (ActivateAccept) must be 1"
        );
    }

    #[test]
    fn quest_details_raw_uses_display_triples_and_four_emotes() {
        let d = QuestDetailView {
            title: "T".into(),
            details: String::new(),
            objectives_text: String::new(),
            choice_rewards: vec![QuestRewardView {
                item_entry: 1234,
                count: 2,
                display_id: 55,
            }],
            rewards: vec![QuestRewardView {
                item_entry: 5678,
                count: 3,
                display_id: 66,
            }],
            ..detail_view()
        };
        let (opcode, body) = build_quest_details_raw(100, &d);

        assert_eq!(opcode, 0x0188);
        assert_eq!(&body[16..20], &1u32.to_le_bytes());
        assert_eq!(&body[20..24], &1u32.to_le_bytes());
        assert_eq!(&body[24..36], &[210, 4, 0, 0, 2, 0, 0, 0, 55, 0, 0, 0]);
        assert_eq!(&body[36..40], &1u32.to_le_bytes());
        assert_eq!(&body[40..52], &[46, 22, 0, 0, 3, 0, 0, 0, 66, 0, 0, 0]);
        assert_eq!(&body[60..64], &4u32.to_le_bytes());
        assert_eq!(
            body.len(),
            96,
            "four zeroed emote/delay pairs follow the count"
        );
    }

    fn detail_view() -> QuestDetailView {
        QuestDetailView {
            quest_id: 7,
            quest_level: 1,
            zone_or_sort: 12,
            title: String::new(),
            details: String::new(),
            objectives_text: String::new(),
            offer_reward_text: String::new(),
            request_items_text: String::new(),
            money_reward: 0,
            reward_xp: 0,
            next_quest_id: 0,
            max_level_money_reward: 0,
            rewards: Vec::new(),
            choice_rewards: Vec::new(),
            objectives: Vec::new(),
        }
    }

    #[test]
    fn failedtimer_wire_pin() {
        let msg = build_questupdate_failedtimer(783);
        assert_eq!(msg.quest_id, 783);
        let mut framed = Vec::new();
        ServerOpcodeMessage::SMSG_QUESTUPDATE_FAILEDTIMER(msg)
            .write_unencrypted_server(&mut framed)
            .unwrap();
        assert_eq!(u16::from_le_bytes([framed[2], framed[3]]), 0x0197);
        let body = &framed[4..];
        assert_eq!(&body[0..4], &783u32.to_le_bytes());
    }

    #[test]
    fn quest_push_result_wire_pin() {
        use lyracore_shared::quest::share_result;
        let msg = build_quest_push_result(0xF130_0000_0000_0042, share_result::HAVE_QUEST);
        assert_eq!(msg.guid.guid(), 0xF130_0000_0000_0042);
        assert_eq!(msg.message, QuestPartyMessage::HaveQuest);
        let mut framed = Vec::new();
        ServerOpcodeMessage::MSG_QUEST_PUSH_RESULT(Box::new(msg))
            .write_unencrypted_server(&mut framed)
            .unwrap();
        assert_eq!(u16::from_le_bytes([framed[2], framed[3]]), 0x0276);
        let body = &framed[4..];
        assert_eq!(&body[0..8], &0xF130_0000_0000_0042u64.to_le_bytes());
        assert_eq!(body[8], share_result::HAVE_QUEST);
    }

    #[test]
    fn quest_push_result_out_of_range_code_falls_back_to_sharing() {
        let msg = build_quest_push_result(1, 255);
        assert_eq!(msg.message, QuestPartyMessage::SharingQuest);
    }

    #[test]
    fn complete_popup_carries_resolved_rewards() {
        let d = QuestDetailView {
            quest_id: 7,
            quest_level: 2,
            zone_or_sort: 12,
            title: "Kobold Camp Cleanup".into(),
            details: "The kobolds infest the mine.".into(),
            objectives_text: "Kobold Vermin slain: 0/10".into(),
            offer_reward_text: "Well done.".into(),
            request_items_text: String::new(),
            money_reward: 25,
            reward_xp: 130,
            next_quest_id: 0,
            max_level_money_reward: 0,
            rewards: vec![QuestRewardView {
                item_entry: 6078,
                count: 1,
                display_id: 42,
            }],
            choice_rewards: vec![
                QuestRewardView {
                    item_entry: 1234,
                    count: 1,
                    display_id: 11,
                },
                QuestRewardView {
                    item_entry: 5678,
                    count: 2,
                    display_id: 22,
                },
            ],
            objectives: vec![(6, 10, 0, 0)],
        };
        let c = build_quest_complete(&d);
        assert_eq!(c.experience_reward, 130);
        assert_eq!(c.money_reward, Gold::new(25));
        assert_eq!(c.item_rewards.len(), 1);
        assert_eq!(c.item_rewards[0].item, 6078);
        let o = build_offer_reward(100, &d);
        assert_eq!(o.item_rewards[0].item_display_id, 42);
    }

    #[test]
    fn turn_in_screens_pack_choice_rewards() {
        let d = QuestDetailView {
            choice_rewards: vec![
                QuestRewardView {
                    item_entry: 1234,
                    count: 1,
                    display_id: 11,
                },
                QuestRewardView {
                    item_entry: 5678,
                    count: 3,
                    display_id: 22,
                },
            ],
            rewards: vec![QuestRewardView {
                item_entry: 6078,
                count: 1,
                display_id: 42,
            }],
            ..detail_view()
        };

        let (_, det) = build_quest_details_raw(100, &d);
        let choice_start = 12 + d.title.len() + d.details.len() + d.objectives_text.len() + 3 + 8;
        assert_eq!(&det[choice_start..choice_start + 4], &1234u32.to_le_bytes());
        assert_eq!(
            &det[choice_start + 12..choice_start + 16],
            &5678u32.to_le_bytes()
        );
        assert_eq!(
            &det[choice_start + 24..choice_start + 28],
            &1u32.to_le_bytes()
        );
        assert_eq!(
            &det[choice_start + 28..choice_start + 32],
            &6078u32.to_le_bytes()
        );

        let off = build_offer_reward(100, &d);
        assert_eq!(off.choice_item_rewards.len(), 2);
        assert_eq!(off.choice_item_rewards[0].item, 1234);
        assert_eq!(off.choice_item_rewards[0].item_display_id, 11);
        assert_eq!(off.choice_item_rewards[1].item_display_id, 22);
        assert_eq!(off.item_rewards.len(), 1);

        let comp = build_quest_complete(&d);
        assert_eq!(comp.item_rewards.len(), 1);
    }
}

#[cfg(test)]
mod refusal_message_tests {
    use super::*;
    use wow_world_messages::vanilla::{QuestFailedReason, ServerMessage};

    fn frame(message: impl ServerMessage) -> Vec<u8> {
        let mut bytes = Vec::new();
        message.write_unencrypted_server(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn quest_invalid_is_opcode_018f_with_one_reason_word() {
        let bytes = frame(build_questgiver_quest_invalid(
            QuestFailedReason::QuestFailedLowLevel,
        ));
        assert_eq!(bytes, [0x00, 0x06, 0x8F, 0x01, 0x01, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn quest_failed_is_opcode_0192_with_quest_then_reason() {
        let bytes = frame(build_questgiver_quest_failed(
            0x0102,
            QuestFailedReason::QuestFailedInventoryFull,
        ));
        assert_eq!(
            bytes,
            [0x00, 0x0A, 0x92, 0x01, 0x02, 0x01, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn quest_update_complete_is_opcode_0198_with_the_quest_id() {
        let bytes = frame(build_questupdate_complete(0x0102));
        assert_eq!(bytes, [0x00, 0x06, 0x98, 0x01, 0x02, 0x01, 0x00, 0x00]);
    }
}
