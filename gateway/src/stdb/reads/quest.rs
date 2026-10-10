//! Quest-log / quest-giver cache-accessor methods — pure code-motion split of the former
//! `reads.rs`.

use spacetimedb_sdk::Table;

use super::super::bindings::*;
use super::player_item_count;

pub(in crate::stdb) fn quest_objectives_complete(
    db: &RemoteTables,
    quest_entry: u32,
    logrow: Option<&CharacterQuest>,
    owner_guid: u64,
) -> bool {
    const COLLECT_ITEM: u8 = 1; // == module objective_kind::COLLECT_ITEM
    db.game_quest_objective()
        .iter()
        .filter(|o| o.quest_entry == quest_entry)
        .all(|o| {
            let have = if o.kind == COLLECT_ITEM {
                player_item_count(db, owner_guid, o.target_entry)
            } else {
                logrow
                    .and_then(|q| q.counts.get(o.obj_index as usize).copied())
                    .unwrap_or(0)
            };
            have >= o.required_count
        })
}

/// Build the player's quest-log descriptor slots from raw quest + objective rows. Shared by the login
/// read ([`Coordinator::player_quest_log`], over the privileged coordinator cache) and the live relay
/// (`subscriptions.rs`, over the player connection's own RLS-scoped cache) so both produce identical
/// slot assignments. Active (un-rewarded) quests for `player_guid`, sorted by quest_entry → slot 0..,
/// capped at the 20 vanilla slots; `state` is 1 when every objective is met (INVENTORY-AWARE for
/// COLLECT_ITEM, via `quest_objectives_complete` — so the log shows "(Complete)" for a collect quest),
/// else 0. Reads `db` (game_quest_objective + game_item_instance); both callers' caches see this player.
pub(crate) fn build_quest_log_slots(
    db: &RemoteTables,
    quests: &[CharacterQuest],
    player_guid: u64,
) -> Vec<crate::codec::update_mask::QuestLogSlot> {
    let mut active: Vec<&CharacterQuest> = quests
        .iter()
        .filter(|q| q.character_guid == player_guid && !q.rewarded)
        .collect();
    active.sort_by_key(|q| q.quest_entry); // deterministic slot order
    active
        .into_iter()
        .take(crate::codec::update_mask::idx::QUEST_LOG_SLOTS as usize)
        .enumerate()
        .map(|(i, q)| {
            let complete = quest_objectives_complete(db, q.quest_entry, Some(q), player_guid);
            crate::codec::update_mask::QuestLogSlot {
                slot: i as u8,
                quest_id: q.quest_entry,
                counts: q.counts.clone(),
                state: u8::from(complete), // 1 = complete, 0 = incomplete
                timer: 0,
            }
        })
        .collect()
}
