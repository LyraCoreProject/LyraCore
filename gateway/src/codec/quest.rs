//! Quest-giver dialog codec (gateway wire). Maps the gateway's quest row-views to the gtker-typed
//! `SMSG_QUESTGIVER_*` messages — the `!`/`?` overhead status, the right-click quest menu, the quest
//! details + accept screen, and the turn-in offer/complete screens. Unlike loot/vendor (raw-encoded
//! past gtker version gaps), every quest packet here is a vanilla-complete gtker typed message, so
//! these are thin view→message mappings except for raw DETAILS reward triples, which gtker omits.
//!
//! The gameplay-meaningful piece is the PURE status logic ([`quest_giver_status`] / [`eval_status`]):
//! given the giver's quests evaluated against the player's log, pick the vanilla overhead icon
//! (turn-in > available > incomplete > none). It's unit-tested here; the coordinator read just gathers
//! the [`GiverQuestEval`]s and the SMSG send is mechanical.
//!
//! NOT here: the quest-LOG window (`PLAYER_QUEST_LOG_*` descriptor fields) + `CMSG_QUEST_QUERY` text —
//! those ride the UpdateMask wall (the hand-rolled `update_mask` encoder lacks the quest-log indices)
//! and are the deferred Phase-2 sub-slice.

use super::*;
use wow_world_base::shared::quest_party_message_vanilla_tbc::QuestPartyMessage;

/// `CreatureQuest.role` mirror (module `quest::quest_role`): which end of the quest a giver is.
pub const ROLE_START: u8 = 0;
pub const ROLE_END: u8 = 1;

/// A reward item line for a quest, joined with its template `display_id` (needed by the offer-reward
/// screen, which carries the icon). `display_id` 0 if the template isn't loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestRewardView {
    pub item_entry: u32,
    pub count: u32,
    pub display_id: u32,
}

/// Everything the gateway needs to render a quest's details / offer-reward / completion dialogs. The
/// text bodies come from `game_quest_text` (cmangos Details/Objectives/OfferReward/RequestItems);
/// `objectives_text` falls back to a synthesized "Creature slain: 0/N" line when the imported objectives
/// text is empty. `reward_xp` is the RESOLVED award (explicit or computed via the shared formula) so the
/// completion popup matches the turn-in grant.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestDetailView {
    pub quest_id: u32,
    pub quest_level: u32,
    /// cmangos ZoneOrSort — the quest-log category header (>0 zone, <0 sort; 0 → "Missing header!").
    pub zone_or_sort: i32,
    pub title: String,
    pub details: String,
    pub objectives_text: String,
    pub offer_reward_text: String,
    pub request_items_text: String,
    pub money_reward: u32,
    pub reward_xp: u32,
    /// cmangos NextQuestId/NextQuestInChain: the successor quest auto-offered on
    /// turn-in (0 = no successor). Threaded into `SMSG_QUEST_QUERY_RESPONSE`'s `next_quest_in_chain`
    /// field (`build_quest_query_response_raw`) — previously a hardcoded 0 stub.
    pub next_quest_id: u32,
    /// The level-cap (60) money conversion of this quest's XP reward
    /// (`lyracore_shared::quest::max_level_money_reward`) — the SAME number a level-60 turn-in actually
    /// pays. Threaded into `SMSG_QUEST_QUERY_RESPONSE`'s `max_level_money_reward` field (previously a
    /// hardcoded 0 stub); the client only surfaces it in the log preview when the VIEWER is capped, so
    /// it's always populated regardless of who's asking.
    pub max_level_money_reward: u32,
    pub rewards: Vec<QuestRewardView>,
    /// CHOICE reward items (pick-1-of-N), ordered by `choice_index` so the wire index == the index the
    /// client sends back as `CMSG_QUESTGIVER_CHOOSE_REWARD.reward`. Empty for a guaranteed-only quest.
    pub choice_rewards: Vec<QuestRewardView>,
    /// Objectives in wire order for `SMSG_QUEST_QUERY_RESPONSE`'s objectives[4] array: each slot is
    /// `(creature_or_go, count, req_item, req_item_count)`. Kill = `(entry, n, 0, 0)`, collect =
    /// `(0, 0, item, n)`, GO-use = `((-(go_entry as i32)) as u32, n, 0, 0)`.
    pub objectives: Vec<(u32, u32, u32, u32)>,
}

/// One giver↔quest relation evaluated against the player — the input to the status icon + the menu.
/// The coordinator computes the booleans (it has the player's log + the objectives); the codec only
/// maps them to vanilla statuses.
#[derive(Clone, Debug, PartialEq)]
pub struct GiverQuestEval {
    pub quest_id: u32,
    pub title: String,
    pub level: u32,
    pub role: u8,        // ROLE_START / ROLE_END
    pub startable: bool, // role START && player qualifies (level) && not already in the log
    pub active: bool,    // role END && player has it && !rewarded
    pub complete: bool,  // active && every objective met
}

/// The vanilla overhead status for ONE evaluated giver↔quest relation. An END relation the player is
/// on shows turn-in-ready (`Reward2`, yellow `?`) when complete else `Incomplete`; a startable START
/// relation shows `Available` (the `!`); anything else contributes nothing (`None`). Pure.
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

/// Priority rank for picking the single overhead icon when a giver has several quests (vanilla shows
/// the most actionable: turn-in > available > incomplete > none).
fn status_rank(s: QuestGiverStatus) -> u8 {
    match s {
        QuestGiverStatus::Reward2 | QuestGiverStatus::RewardRep | QuestGiverStatus::RewardOld => 3,
        QuestGiverStatus::Available => 2,
        QuestGiverStatus::Incomplete => 1,
        _ => 0,
    }
}

/// The single overhead status for a giver, folding every evaluated relation by priority. Pure/testable.
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

/// `SMSG_QUESTGIVER_STATUS` — the overhead `!`/`?` icon for `giver_guid`.
pub fn build_questgiver_status(
    giver_guid: u64,
    status: QuestGiverStatus,
) -> SMSG_QUESTGIVER_STATUS {
    SMSG_QUESTGIVER_STATUS {
        guid: Guid::new(giver_guid),
        status,
    }
}

/// `SMSG_QUESTUPDATE_ADD_KILL` — the "Creature slain: n/N" kill-progress toast. `creature_id` is the
/// objective's target creature entry (the client looks up its name); `guid` is sent as 0 — the quest-update
/// relay knows the entry + counts but not the specific corpse, and the client renders the toast from
/// `creature_id` regardless.
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

/// Pure kill-progress diff: given a quest's objectives and its old→new progress counts, return one
/// `SMSG_QUESTUPDATE_ADD_KILL` per KILL objective (kind 0) whose count INCREASED. The gateway's
/// `on_quest_update` relay is just this + the send — extracted so the diff is unit-testable without a live
/// client. `objectives` are `(kind, obj_index, target_entry, required_count)`.
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
                return None; // KILL_CREATURE only (collect/explore have no per-kill counter)
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
        // obj0 kills creature 257 (×10); obj1 is a COLLECT (kind 1) that must be ignored.
        let objs = [(0u8, 0u8, 257u32, 10u32), (1, 1, 999, 5)];
        // obj0 went 2→3; obj1's count also changed but it's not a KILL objective.
        let out = kill_progress_add_kills(15, &[2, 4], &[3, 5], &objs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].quest_id, 15);
        assert_eq!(out[0].creature_id, 257);
        assert_eq!(out[0].kill_count, 3);
        assert_eq!(out[0].required_kill_count, 10);
        // No count change → no message.
        assert!(kill_progress_add_kills(15, &[3, 5], &[3, 5], &objs).is_empty());
    }
}

/// `SMSG_QUESTUPDATE_FAILEDTIMER` (0x0197) — a timed quest's log entry flips to FAILED
/// the instant `quest_timer_pass` fails it (`gateway/src/stdb/subscriptions.rs`'s `on_quest_update`
/// diffs `!old.failed && row.failed`). Pure builder: not boxed (unlike `SMSG_QUESTUPDATE_ADD_KILL`) —
/// gtker's enum wraps this variant directly.
pub fn build_questupdate_failedtimer(quest_id: u32) -> SMSG_QUESTUPDATE_FAILEDTIMER {
    SMSG_QUESTUPDATE_FAILEDTIMER { quest_id }
}

/// `MSG_QUEST_PUSH_RESULT` (0x0276) — the SENDER's per-member feedback line
/// after `push_quest_to_party` ("So-and-so already has that quest", "is too far away", etc). `code` is
/// the module's `lyracore_shared::quest::share_result` wire byte, which mirrors gtker's
/// `QuestPartyMessage::as_int()` 1:1 by construction — decode is a straight `try_into()`; an
/// out-of-range byte (never actually produced by the module) falls back to `SharingQuest` rather than
/// dropping the packet.
pub fn build_quest_push_result(member_guid: u64, code: u8) -> MSG_QUEST_PUSH_RESULT {
    MSG_QUEST_PUSH_RESULT {
        guid: Guid::new(member_guid),
        message: QuestPartyMessage::try_from(code).unwrap_or(QuestPartyMessage::SharingQuest),
    }
}

/// The actionable quests of a giver as gtker `QuestItem`s (one menu line each, tagged with the
/// per-quest icon `!`/`?`/grey). Quests contributing no action (`None`) are filtered out. Shared by
/// the pure-questgiver menu ([`build_quest_list`]) AND the gossip window's quest section
/// (`build_gossip_message`), since a gossip-flagged questgiver delivers its quests through
/// `SMSG_GOSSIP_MESSAGE` rather than `SMSG_QUESTGIVER_QUEST_LIST`.
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

/// The MENU/quest-list icon for a giver↔quest line — distinct from [`eval_status`] (the overhead `!`/`?`
/// icon) in ONE case: a COMPLETE turn-in shows as `RewardRep` (4), the value the 1.12 client treats as
/// turn-in-able in a gossip/quest list; the overhead icon uses `Reward2` (7, the yellow minimap dot).
/// Reusing the overhead value here left the gossip line un-clickable for turn-in.
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

/// The guaranteed reward items of a quest as gtker `QuestItemReward`s (item + count). Used by the quest
/// details + completion popups (which don't show the item icon).
fn reward_items(rewards: &[QuestRewardView]) -> Vec<QuestItemReward> {
    rewards
        .iter()
        .map(|r| QuestItemReward {
            item: r.item_entry,
            item_count: r.count,
        })
        .collect()
}

/// The reward items as gtker `QuestItemRequirement`s (item + count + display_id) — the offer-reward
/// screen variant, which DOES show the item icon (`item_display_id`).
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

/// `SMSG_QUESTGIVER_QUEST_LIST` — the right-click menu for a PURE questgiver (no gossip flag): every
/// quest it offers/completes for the player, each tagged with its per-quest icon (`!` vs `?` per line).
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

/// `SMSG_QUESTGIVER_QUEST_DETAILS` (0x0188) — the quest text + Accept screen.
///
/// This must be raw-encoded: gtker models its reward records as `(item, count)`, but the 1.12 wire
/// contains `(item, count, display_id)` triples and always appends four `(emote, delay)` pairs. The
/// truncated typed form desynchronizes clients as soon as a quest has an item reward.
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
    body.extend_from_slice(&1u32.to_le_bytes()); // ActivateAccept
    for rewards in [&d.choice_rewards, &d.rewards] {
        body.extend_from_slice(&(rewards.len() as u32).to_le_bytes());
        for reward in rewards.iter() {
            body.extend_from_slice(&reward.item_entry.to_le_bytes());
            body.extend_from_slice(&reward.count.to_le_bytes());
            body.extend_from_slice(&reward.display_id.to_le_bytes());
        }
    }
    body.extend_from_slice(&d.money_reward.to_le_bytes());
    body.extend_from_slice(&0u32.to_le_bytes()); // reward spell
    body.extend_from_slice(&DETAILS_EMOTE_COUNT.to_le_bytes());
    body.resize(body.len() + DETAILS_EMOTE_COUNT as usize * 8, 0);
    (SMSG_QUESTGIVER_QUEST_DETAILS, body)
}

/// `SMSG_QUESTGIVER_OFFER_REWARD` — the turn-in reward screen (shown when the player clicks a complete
/// `?`). Reward items carry their display id here (the screen shows icons).
///
/// `auto_finish` is the `Bool32` mangos-zero names `EnableNext` — the real 5875 server hardcodes it
/// `true` when it sends this packet (`SendQuestGiverOfferReward(pQuest, npcGUID, true)`,
/// `mz_gossip.cpp:922` → `:811`), NOT `false`. It enables the "Complete Quest" button + automatic
/// progression to the next quest in a chain (783→7→15→21→54, exactly McBride's quests). Sending `0` left
/// the turn-in button in a non-armed state for a gossip-questgiver chain; this was the only field on the
/// turn-in packets that diverged from the authoritative server (the two trailing `reward_spell`/
/// `reward_spell_cast` u32s and every other field are byte-identical to mangos-zero). Match it: `true`.
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

/// `SMSG_QUESTGIVER_REQUEST_ITEMS` — shown when the player opens a turn-in that ISN'T finished yet
/// (the "you don't have everything" screen). `completable` drives the Complete button's enabled state.
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
        unknown1: 0x02, // all emulators set this constant
        completable: if completable {
            QuestCompletable::Completeable
        } else {
            QuestCompletable::NotCompletable
        },
        flags2: 0x04,
        flags3: 0x08,
    }
}

/// `SMSG_QUEST_QUERY_RESPONSE` opcode (0x005D) — RAW-encoded (the 3rd vanilla wire gap after the
/// update-mask wall + the loot-item codec). gtker's typed `SMSG_QUEST_QUERY_RESPONSE` writes the two
/// rep-objective `Faction` fields as u16, which is 4 bytes short before `title` → the 5875 client reads
/// the title 4 bytes late ("A Threat Within" → "reat Within"). We hand-roll the exact 5875 layout (every
/// numeric field a u32, the two position values f32, the rest CStrings) to fix it.
const SMSG_QUEST_QUERY_RESPONSE_OPCODE: u16 = 0x005D;

/// Push a NUL-terminated CString.
fn push_cstr(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

/// Build the RAW `SMSG_QUEST_QUERY_RESPONSE`: the full quest definition the client needs to DISPLAY a
/// quest in its log. The client sends `CMSG_QUEST_QUERY` the moment it sees a quest id in a
/// `PLAYER_QUEST_LOG` slot it has no data for; until this reply lands it won't show/count the quest. The
/// load-bearing fields are zone_or_sort (the log category header), title, objective text, details, money,
/// reward items, and the kill objectives; the rest are 0 for our slice. See the field table in
/// docs (the u16→u32 Faction fix is the 4-byte shift that corrupted the title).
pub fn build_quest_query_response_raw(d: &QuestDetailView) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(256);
    let u32le = |body: &mut Vec<u8>, v: u32| body.extend_from_slice(&v.to_le_bytes());
    u32le(&mut body, d.quest_id);
    u32le(&mut body, 2); // quest_method
    u32le(&mut body, d.quest_level);
    body.extend_from_slice(&d.zone_or_sort.to_le_bytes()); // i32 bits (header category)
    u32le(&mut body, 0); // quest_type
    u32le(&mut body, 0); // reputation_objective_faction (u32 in 5875 — gtker wrote u16)
    u32le(&mut body, 0); // reputation_objective_value
    u32le(&mut body, 0); // required_opposite_faction (u32 in 5875 — gtker wrote u16)
    u32le(&mut body, 0); // required_opposite_reputation_value
    u32le(&mut body, d.next_quest_id); // next_quest_in_chain (was a hardcoded 0 stub)
    u32le(&mut body, d.money_reward);
    u32le(&mut body, d.max_level_money_reward); // was a hardcoded 0 stub
    u32le(&mut body, 0); // reward_spell
    u32le(&mut body, 0); // source_item_id
    u32le(&mut body, 0); // quest_flags
                         // rewards[4]: (item, count), padded with zeros.
    for i in 0..4 {
        let (item, count) = d
            .rewards
            .get(i)
            .map(|r| (r.item_entry, r.count))
            .unwrap_or((0, 0));
        u32le(&mut body, item);
        u32le(&mut body, count);
    }
    // choice_rewards[6]: (item, count), padded with zeros — the pick-1-of-N choices for the quest log.
    for i in 0..6 {
        let (item, count) = d
            .choice_rewards
            .get(i)
            .map(|r| (r.item_entry, r.count))
            .unwrap_or((0, 0));
        u32le(&mut body, item);
        u32le(&mut body, count);
    }
    u32le(&mut body, 0); // point_map_id
    body.extend_from_slice(&0f32.to_le_bytes()); // point_x
    body.extend_from_slice(&0f32.to_le_bytes()); // point_y
    u32le(&mut body, 0); // point_opt
    push_cstr(&mut body, &d.title);
    push_cstr(&mut body, &d.objectives_text);
    push_cstr(&mut body, &d.details);
    push_cstr(&mut body, ""); // end_text
                              // objectives[4]: (creature_or_go, count, req_item, req_item_count). Kill = positive creature entry;
                              // GO-use = negative go_entry cast to u32; collect = creature_or_go/count=0, req_item/count filled.
    for i in 0..4 {
        let (creature_or_go, count, req_item, req_item_count) =
            d.objectives.get(i).copied().unwrap_or((0, 0, 0, 0));
        u32le(&mut body, creature_or_go);
        u32le(&mut body, count);
        u32le(&mut body, req_item);
        u32le(&mut body, req_item_count);
    }
    for _ in 0..4 {
        push_cstr(&mut body, ""); // objective_texts[4]
    }
    (SMSG_QUEST_QUERY_RESPONSE_OPCODE, body)
}

/// `SMSG_QUESTGIVER_QUEST_COMPLETE` — the "Quest Complete" popup after choosing the reward, showing
/// the XP + money + items actually granted (the XP matches the module's turn-in via the shared formula).
pub fn build_quest_complete(d: &QuestDetailView) -> SMSG_QUESTGIVER_QUEST_COMPLETE {
    SMSG_QUESTGIVER_QUEST_COMPLETE {
        quest_id: d.quest_id,
        unknown: 0x03, // all emulators set this constant
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
        // A startable START quest → the `!` (Available).
        assert_eq!(
            quest_giver_status(&[ev(ROLE_START, true, false, false)]),
            QuestGiverStatus::Available
        );
        // An active+complete END quest → the yellow `?` (Reward2), even alongside an available one.
        assert_eq!(
            quest_giver_status(&[
                ev(ROLE_START, true, false, false),
                ev(ROLE_END, false, true, true)
            ]),
            QuestGiverStatus::Reward2
        );
        // Active but NOT complete → Incomplete (a grey `?`), ranked below an available `!`.
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
        // Nothing actionable (already done / under-level) → None.
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
            ev(ROLE_START, true, false, false), // available → in the list as `!`
            ev(ROLE_END, false, true, true),    // complete → in the list as `?`
            ev(ROLE_START, false, false, false), // none → filtered out
        ];
        let list = build_quest_list(100, "Greetings", &evals);
        assert_eq!(list.quest_items.len(), 2);
        assert_eq!(
            list.quest_items[0].quest_icon,
            QuestGiverStatus::Available.as_int() as u32
        );
        // Complete turn-in line uses the MENU icon RewardRep(4) — the value the client treats as
        // turn-in-able in a quest list (the overhead `?` uses Reward2(7); see menu_icon vs eval_status).
        assert_eq!(
            list.quest_items[1].quest_icon,
            QuestGiverStatus::RewardRep.as_int() as u32
        );
    }

    #[test]
    fn quest_query_response_raw_is_well_formed() {
        // The title must be FIRST byte of the title CString (no 4-byte shift) — the bug this fixes.
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
        // quest_id at byte 0, zone_or_sort at byte 12 (3 u32s in), full title present uncut.
        assert_eq!(&body[0..4], &783u32.to_le_bytes());
        assert_eq!(&body[12..16], &12i32.to_le_bytes());
        // PIN THE FIX: the title CString starts at EXACTLY byte 156 — the offset produced when the two
        // rep-objective Faction fields are u32 (the 5875 layout). gtker's u16 bug makes the fixed header
        // 4 bytes shorter, shifting the title to byte 152 ("reat Within"). This absolute-offset assert
        // fails loudly if anyone reverts those fields to u16; the windows()-search below would NOT (it
        // would still find the title at the wrong offset and pass).
        assert_eq!(&body[156..156 + 15], b"A Threat Within");
        let title_pos = body
            .windows(15)
            .position(|w| w == b"A Threat Within")
            .expect("full title present");
        assert_eq!(title_pos, 156);
        // The title's NUL terminator follows immediately (the title isn't truncated mid-string).
        assert_eq!(body[title_pos + 15], 0);
    }

    /// `next_quest_in_chain` and `max_level_money_reward` are THREADED from the view,
    /// not the old hardcoded-0 stubs — pinned at their exact byte offsets (9 leading u32s, then
    /// `next_quest_in_chain` at byte 36; `money_reward` at 40, then `max_level_money_reward` at 44).
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
        // PIN the 5875 turn-in fix: `auto_finish` (Bool32) must serialize as 1 — mangos-zero hardcodes
        // it `true` on both screens (SendQuestGiverOfferReward(.., true) / SendQuestGiverQuestDetails(..,
        // true)). It is the FIRST u32 after the two leading CStrings, so its byte offset is computable
        // from the title + body-text lengths. We use a known title ("T") + empty body text so the offset
        // is fixed: a regression to `false` (0) fails here loudly.
        let mut d = detail_view();
        d.title = "T".into(); // 1 byte + NUL

        // OFFER_REWARD body: npc(8) + quest_id(4) + title("T\0"=2) + offer_reward_text("\0"=1) = 15, then
        // auto_finish (Bool32) at byte 15.
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

        // DETAILS body: npc(8) + quest_id(4) + title("T\0"=2) + details("\0"=1) + objectives("\0"=1) = 16,
        // then auto_finish (Bool32) at byte 16.
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
        // Guid + id + three CStrings take 16 bytes. DETAILS has display triples, unlike
        // gtker's incomplete pair-only definition.
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

    /// A minimal detail view (all zero/empty) for the build_* codecs — the title is overwritten per-test.
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

    /// `SMSG_QUESTUPDATE_FAILEDTIMER` (0x0197) carries the quest id at byte 0 of its
    /// body — pinned by encoding the real gtker message (not just constructing the struct), so a
    /// gtker version bump that changes the layout fails here loudly.
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

    /// `MSG_QUEST_PUSH_RESULT` (0x0276) carries the member guid then the `QuestPartyMessage`
    /// byte — pinned via a real gtker encode. `share_result::HAVE_QUEST` (7) round-trips to
    /// `QuestPartyMessage::HaveQuest` (also 7 per gtker's own `as_int()`), proving the 1:1 mirror.
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

    /// An out-of-range code (never produced by the module — `share_result` only ever returns 0/1/4/6/7/8)
    /// fails closed to `SharingQuest` rather than panicking.
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
        // The complete popup carries ONLY the guaranteed items (no choices), matching mangos.
        // The offer-reward screen carries the item display id (icon).
        let o = build_offer_reward(100, &d);
        assert_eq!(o.item_rewards[0].item_display_id, 42);
    }

    /// Both turn-in screens pack the CHOICE list (pick-1-of-N) in `choice_index` order, alongside the
    /// unchanged guaranteed `item_rewards`: DETAILS without icons, OFFER with the display id (icon).
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

        // DETAILS: choice_item_rewards carries item+count (no icon field), ordered as given.
        let (_, det) = build_quest_details_raw(100, &d);
        // DETAILS uses 12-byte item/display triples and preserves choice order.
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

        // OFFER: choice_item_rewards carries the display id (icon) too.
        let off = build_offer_reward(100, &d);
        assert_eq!(off.choice_item_rewards.len(), 2);
        assert_eq!(off.choice_item_rewards[0].item, 1234);
        assert_eq!(off.choice_item_rewards[0].item_display_id, 11);
        assert_eq!(off.choice_item_rewards[1].item_display_id, 22);
        assert_eq!(off.item_rewards.len(), 1);

        // The "Quest Complete" popup is guaranteed-only — no choices.
        let comp = build_quest_complete(&d);
        assert_eq!(comp.item_rewards.len(), 1);
    }
}
