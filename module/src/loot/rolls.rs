use spacetimedb::{reducer, table, Identity, ReducerContext, Table};

use crate::game_corpse_loot;
use crate::game_group;
use crate::game_item_template;
use crate::game_world_entity;
use lyracore_shared::loot::LootRefusal;
use lyracore_shared::loot_roll::{event_kind as roll_event_kind, vote_kind};

use super::{refused, CorpseLoot};

// Corpse lifetime owns these rows; they are not Character-owned state.
#[table(accessor = game_corpse_loot_eligible, index(accessor = by_corpse, btree(columns = [corpse_guid])))]
pub struct CorpseLootEligible {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub corpse_guid: u64,
    pub eligible_guid: u64,
}

#[derive(Clone)]
#[table(accessor = game_loot_roll, index(accessor = by_corpse, btree(columns = [corpse_guid])))]
pub struct LootRoll {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub corpse_guid: u64,
    pub slot: u8,
    pub item_entry: u32,
    pub deadline_micros: i64,
    pub resolved: bool,
    #[default(0)]
    pub random_property_id: u32,
    #[default(Identity::ZERO)]
    pub promotion_source: Identity,
}

#[derive(Clone)]
#[table(accessor = game_loot_roll_vote, index(accessor = by_roll, btree(columns = [roll_id])))]
pub struct LootRollVote {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub roll_id: u64,
    pub voter_guid: u64,
    pub voted: bool,
    pub vote: u8,   // Meaningful only when voted is true.
    pub rolled: u8, // 1..=100 for Need or Greed; zero otherwise.
}

// Retired Loot Roll promotion table keeps its declaration; see docs/danger-zones.md.
#[table(accessor = game_loot_roll_promotion_receipt, index(accessor = by_source_slot, btree(columns = [promotion_source, corpse_guid, slot])))]
pub struct LootRollPromotionReceipt {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub promotion_source: Identity,
    pub corpse_guid: u64,
    pub slot: u8,
    pub source_roll_id: u64,
}

pub(crate) const ROLL_WINDOW_MICROS: i64 = 60_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupLootDecision {
    Ffa,
    Designate,
    Roll,
    Master,
}

pub(crate) fn group_loot_decision_for_row(
    method: u8,
    quality: u8,
    threshold: u8,
) -> GroupLootDecision {
    use crate::group::loot_method::*;
    match method {
        FFA => GroupLootDecision::Ffa,
        ROUND_ROBIN => GroupLootDecision::Designate,
        MASTER => {
            if quality >= threshold {
                GroupLootDecision::Master
            } else {
                GroupLootDecision::Designate
            }
        }
        GROUP => {
            if quality >= threshold {
                GroupLootDecision::Roll
            } else {
                GroupLootDecision::Designate
            }
        }
        NEED_BEFORE_GREED => GroupLootDecision::Roll,
        _ => GroupLootDecision::Ffa,
    }
}

pub(crate) fn advance_rr_cursor(online: &[bool], cursor: u32) -> Option<(usize, u32)> {
    let n = online.len();
    if n == 0 {
        return None;
    }
    let start = (cursor as usize) % n;
    for step in 0..n {
        let idx = (start + step) % n;
        if online[idx] {
            return Some((idx, (idx as u32 + 1) % n as u32));
        }
    }
    None
}

pub(crate) fn contending_tier(votes: &[u8]) -> Vec<usize> {
    let need: Vec<usize> = votes
        .iter()
        .enumerate()
        .filter(|(_, &v)| v == vote_kind::NEED)
        .map(|(i, _)| i)
        .collect();
    if !need.is_empty() {
        return need;
    }
    votes
        .iter()
        .enumerate()
        .filter(|(_, &v)| v == vote_kind::GREED)
        .map(|(i, _)| i)
        .collect()
}

pub(crate) fn pick_roll_winner(rolls: &[u8]) -> Option<usize> {
    if rolls.is_empty() {
        return None;
    }
    let max = *rolls.iter().max().unwrap();
    let mut winners = rolls
        .iter()
        .enumerate()
        .filter(|(_, &r)| r == max)
        .map(|(i, _)| i);
    let first = winners.next()?;
    if winners.next().is_some() {
        None
    } else {
        Some(first)
    }
}

pub(crate) fn apply_group_loot_rules(ctx: &ReducerContext, corpse_guid: u64, group_id: u64) {
    let recipients = super::tag::corpse_eligible_recipients(ctx, corpse_guid);
    if recipients.len() < 2 {
        return;
    }
    let Some(mut group) = ctx.db.game_group().group_id().find(group_id) else {
        return;
    };

    if group.loot_method == crate::group::loot_method::FFA {
        return;
    }

    // Stable GUID order keeps the round-robin cursor meaningful across kills.
    let mut members = recipients.clone();
    members.sort_unstable();
    let online: Vec<bool> = members
        .iter()
        .map(|g| ctx.db.game_world_entity().guid().find(g).is_some())
        .collect();
    let mut rr_cursor_dirty = false;
    let designee = advance_rr_cursor(&online, group.rr_cursor).map(|(idx, next_cursor)| {
        group.rr_cursor = next_cursor;
        rr_cursor_dirty = true;
        members[idx]
    });
    let preferred_master = if group.master_looter_guid != 0 {
        group.master_looter_guid
    } else {
        group.leader_guid
    };
    let master_guid = recipients
        .contains(&preferred_master)
        .then_some(preferred_master)
        .or_else(|| recipients.first().copied())
        .expect("two eligible recipients were checked above");

    let templates = ctx.db.game_item_template();
    let rows: Vec<CorpseLoot> = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&corpse_guid)
        .collect();
    let mut master_rows: Vec<u8> = Vec::new();
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    for mut row in rows {
        if row.quest_only {
            continue;
        }
        let quality = templates
            .entry()
            .find(row.item_entry)
            .map(|t| t.quality)
            .unwrap_or(0);
        match group_loot_decision_for_row(group.loot_method, quality, group.loot_threshold) {
            GroupLootDecision::Ffa => continue,
            GroupLootDecision::Designate => {
                if let Some(guid) = designee {
                    row.designated_looter_guid = guid;
                    ctx.db.game_corpse_loot().id().update(row);
                }
            }
            GroupLootDecision::Master => {
                row.designated_looter_guid = master_guid;
                row.master_only = true;
                master_rows.push(row.slot);
                ctx.db.game_corpse_loot().id().update(row);
            }
            GroupLootDecision::Roll => {
                start_roll(
                    ctx,
                    corpse_guid,
                    row.slot,
                    row.item_entry,
                    &recipients,
                    now,
                    row.random_property_id,
                );
                row.withheld = true;
                ctx.db.game_corpse_loot().id().update(row);
            }
        }
    }
    if rr_cursor_dirty {
        ctx.db.game_group().group_id().update(group);
    }
    if !master_rows.is_empty() {
        crate::group::push_event(
            ctx,
            master_guid,
            roll_event_kind::MASTER_LIST,
            0,
            lyracore_shared::loot_roll::encode_master_list(corpse_guid, &recipients),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_roll_rows(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    recipients: &[u64],
    deadline_micros: i64,
    random_property_id: u32,
) -> u64 {
    let rolls = ctx.db.game_loot_roll();
    if let Some(existing) = rolls
        .by_corpse()
        .filter(&corpse_guid)
        .find(|r| r.slot == slot)
    {
        return existing.id;
    }
    let roll = rolls.insert(LootRoll {
        id: 0,
        corpse_guid,
        slot,
        item_entry,
        deadline_micros,
        resolved: false,
        random_property_id,
        promotion_source: ctx.database_identity(),
    });
    let votes = ctx.db.game_loot_roll_vote();
    for &guid in recipients {
        votes.insert(LootRollVote {
            id: 0,
            roll_id: roll.id,
            voter_guid: guid,
            voted: false,
            vote: 0,
            rolled: 0,
        });
    }
    roll.id
}

fn start_roll(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    recipients: &[u64],
    now_micros: i64,
    random_property_id: u32,
) {
    insert_roll_rows(
        ctx,
        corpse_guid,
        slot,
        item_entry,
        recipients,
        now_micros + ROLL_WINDOW_MICROS,
        random_property_id,
    );
    let payload = lyracore_shared::loot_roll::encode_start(
        corpse_guid,
        slot,
        item_entry,
        (ROLL_WINDOW_MICROS / 1000) as u32,
        random_property_id,
    );
    for &guid in recipients {
        crate::group::push_event(ctx, guid, roll_event_kind::ROLL_START, 0, payload.clone());
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_stage_loot_roll_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    start_roll(
        ctx,
        5_090_001,
        0,
        5_090_002,
        &[5_090_003, 5_090_004, 5_090_005],
        ctx.timestamp.to_micros_since_unix_epoch(),
        117,
    );
    Ok(())
}

pub(crate) fn cast_vote_on(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    voter_guid: u64,
    vote: u8,
) -> Result<(), String> {
    if !matches!(vote, vote_kind::PASS | vote_kind::NEED | vote_kind::GREED) {
        return Err(refused(LootRefusal::RollUnavailable, "invalid vote"));
    }
    let roll = ctx
        .db
        .game_loot_roll()
        .by_corpse()
        .filter(&corpse_guid)
        .find(|r| r.slot == slot && !r.resolved)
        .ok_or_else(|| refused(LootRefusal::RollUnavailable, "no roll open on that item"))?;
    let votes = ctx.db.game_loot_roll_vote();
    let mut my_vote = votes
        .by_roll()
        .filter(&roll.id)
        .find(|v| v.voter_guid == voter_guid)
        .ok_or_else(|| refused(LootRefusal::RollUnavailable, "voter is not on this roll"))?;
    if my_vote.voted {
        return Err(refused(LootRefusal::RollUnavailable, "already voted"));
    }
    my_vote.voted = true;
    my_vote.vote = vote;
    my_vote.rolled = if vote == vote_kind::PASS {
        0
    } else {
        (ctx.random::<u32>() % 100 + 1) as u8
    };
    let my_rolled = my_vote.rolled;
    votes.id().update(my_vote);

    let all: Vec<LootRollVote> = votes.by_roll().filter(&roll.id).collect();
    let recipients: Vec<u64> = all.iter().map(|v| v.voter_guid).collect();
    let payload = lyracore_shared::loot_roll::encode_vote(
        corpse_guid,
        slot,
        roll.item_entry,
        my_rolled,
        vote,
        false,
        roll.random_property_id,
    );
    for &guid in &recipients {
        crate::group::push_event(
            ctx,
            guid,
            roll_event_kind::ROLL_VOTE,
            voter_guid,
            payload.clone(),
        );
    }
    if all.iter().all(|v| v.voted) {
        resolve_roll(ctx, &roll, &all, &recipients);
    }
    Ok(())
}

pub(crate) fn settle_roll_grant(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    winner_guid: u64,
) {
    let loot = ctx.db.game_corpse_loot();
    let Some(mut row) = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == slot)
    else {
        spacetimedb::log::info!(
            "settle_roll_grant: corpse {corpse_guid} slot {slot} — no loot row; the corpse was \
             reaped before the roll settled and the winner got nothing"
        );
        return;
    };
    if !row.withheld {
        spacetimedb::log::info!(
            "settle_roll_grant: corpse {corpse_guid} slot {slot} — row is not withheld, so no roll \
             is open on it here; nothing granted"
        );
        return;
    }
    match crate::items::grant_item_property(
        ctx,
        winner_guid,
        row.item_entry,
        row.count.max(1),
        Some(row.random_property_id),
    ) {
        Ok(()) => {
            loot.id().delete(row.id);
            super::refresh_lootable(ctx, corpse_guid);
        }
        Err(_) => {
            row.withheld = false;
            row.reserved_for = winner_guid;
            row.designated_looter_guid = 0;
            row.master_only = false;
            loot.id().update(row);
        }
    }
}

fn resolve_roll(ctx: &ReducerContext, roll: &LootRoll, votes: &[LootRollVote], recipients: &[u64]) {
    if roll.resolved {
        return;
    }
    ctx.db.game_loot_roll().id().update(LootRoll {
        resolved: true,
        ..roll.clone()
    });

    let kinds: Vec<u8> = votes.iter().map(|v| v.vote).collect();
    let tier = contending_tier(&kinds);
    if tier.is_empty() {
        unlock_row_ffa(ctx, roll.corpse_guid, roll.slot);
        let payload = lyracore_shared::loot_roll::encode_all_passed(
            roll.corpse_guid,
            roll.slot,
            roll.item_entry,
            roll.random_property_id,
        );
        for &guid in recipients {
            crate::group::push_event(ctx, guid, roll_event_kind::ALL_PASSED, 0, payload.clone());
        }
        cleanup_roll(ctx, roll.id);
        return;
    }
    let mut rolls: Vec<u8> = tier.iter().map(|&i| votes[i].rolled).collect();
    const MAX_TIE_REROLLS: u32 = 20;
    let mut winner_in_tier = pick_roll_winner(&rolls);
    let mut attempts = 0;
    while winner_in_tier.is_none() && attempts < MAX_TIE_REROLLS {
        for r in rolls.iter_mut() {
            *r = (ctx.random::<u32>() % 100 + 1) as u8;
        }
        winner_in_tier = pick_roll_winner(&rolls);
        attempts += 1;
    }
    let Some(w) = winner_in_tier else {
        unlock_row_ffa(ctx, roll.corpse_guid, roll.slot);
        cleanup_roll(ctx, roll.id);
        return;
    };
    let winning_roll = rolls[w];
    let winner_guid = votes[tier[w]].voter_guid;
    let winning_vote = votes[tier[w]].vote;

    settle_roll_grant(ctx, roll.corpse_guid, roll.slot, winner_guid);
    let payload = lyracore_shared::loot_roll::encode_won(
        roll.corpse_guid,
        roll.slot,
        roll.item_entry,
        winning_roll,
        winning_vote,
        roll.random_property_id,
    );
    for &guid in recipients {
        crate::group::push_event(
            ctx,
            guid,
            roll_event_kind::ROLL_WON,
            winner_guid,
            payload.clone(),
        );
    }
    cleanup_roll(ctx, roll.id);
}

fn unlock_row_ffa(ctx: &ReducerContext, corpse_guid: u64, slot: u8) {
    let loot = ctx.db.game_corpse_loot();
    if let Some(mut row) = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == slot)
    {
        row.withheld = false;
        loot.id().update(row);
    }
}

fn cleanup_roll(ctx: &ReducerContext, roll_id: u64) {
    let votes = ctx.db.game_loot_roll_vote();
    let stale: Vec<u64> = votes.by_roll().filter(&roll_id).map(|v| v.id).collect();
    for id in stale {
        votes.id().delete(id);
    }
    ctx.db.game_loot_roll().id().delete(roll_id);
}

pub(crate) fn sweep_loot_rolls(ctx: &ReducerContext) {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let due: Vec<LootRoll> = ctx
        .db
        .game_loot_roll()
        .iter()
        .filter(|r| !r.resolved && r.deadline_micros <= now)
        .collect();
    for roll in due {
        let votes_tab = ctx.db.game_loot_roll_vote();
        let mut all: Vec<LootRollVote> = votes_tab.by_roll().filter(&roll.id).collect();
        for v in all.iter_mut() {
            if !v.voted {
                v.voted = true;
                v.vote = vote_kind::PASS;
                v.rolled = 0;
                votes_tab.id().update(v.clone());
            }
        }
        let recipients: Vec<u64> = all.iter().map(|v| v.voter_guid).collect();
        resolve_roll(ctx, &roll, &all, &recipients);
    }
}

pub(crate) fn force_resolve_rolls_for_disband(
    ctx: &ReducerContext,
    member_guids: &[u64],
    survivor: Option<u64>,
) {
    let rolls: Vec<LootRoll> = ctx
        .db
        .game_loot_roll()
        .iter()
        .filter(|r| !r.resolved)
        .collect();
    for roll in rolls {
        let votes_tab = ctx.db.game_loot_roll_vote();
        let all: Vec<LootRollVote> = votes_tab.by_roll().filter(&roll.id).collect();
        if all.is_empty() || !all.iter().all(|v| member_guids.contains(&v.voter_guid)) {
            continue;
        }
        ctx.db.game_loot_roll().id().update(LootRoll {
            resolved: true,
            ..roll.clone()
        });
        match survivor.filter(|s| all.iter().any(|v| v.voter_guid == *s)) {
            Some(winner_guid) => {
                settle_roll_grant(ctx, roll.corpse_guid, roll.slot, winner_guid);
                // Forced disband has no contest; Need is the label sent for the sole survivor.
                let payload = lyracore_shared::loot_roll::encode_won(
                    roll.corpse_guid,
                    roll.slot,
                    roll.item_entry,
                    0,
                    lyracore_shared::loot_roll::vote_kind::NEED,
                    roll.random_property_id,
                );
                crate::group::push_event(
                    ctx,
                    winner_guid,
                    roll_event_kind::ROLL_WON,
                    winner_guid,
                    payload,
                );
            }
            None => unlock_row_ffa(ctx, roll.corpse_guid, roll.slot),
        }
        cleanup_roll(ctx, roll.id);
    }
}

pub(crate) fn apply_master_give(
    ctx: &ReducerContext,
    master_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
    target_guid: u64,
) -> Result<(), String> {
    let loot = ctx.db.game_corpse_loot();
    let row = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == loot_slot)
        .ok_or_else(|| refused(LootRefusal::NothingToLoot, "no loot in that slot"))?;
    if !row.master_only || row.designated_looter_guid != master_guid {
        return Err(refused(
            LootRefusal::NotMasterLooter,
            "Actor does not hold the master-looter right on that row",
        ));
    }
    let is_eligible = ctx
        .db
        .game_corpse_loot_eligible()
        .by_corpse()
        .filter(&corpse_guid)
        .any(|e| e.eligible_guid == target_guid);
    if !is_eligible {
        return Err(refused(
            LootRefusal::LootTagIneligible,
            &format!("master-give target {target_guid} is not eligible"),
        ));
    }
    crate::helpers::live_entity(ctx, target_guid).map_err(|_| {
        refused(
            LootRefusal::RecipientUnavailable,
            &format!("master-loot recipient {target_guid} is not in the world"),
        )
    })?;
    crate::items::grant_item_property(
        ctx,
        target_guid,
        row.item_entry,
        row.count.max(1),
        Some(row.random_property_id),
    )
    .map_err(master_delivery_error)?;
    loot.id().delete(row.id);
    super::refresh_lootable(ctx, corpse_guid);
    Ok(())
}

fn master_delivery_error(error: String) -> String {
    if error == lyracore_shared::mail::INVENTORY_FULL {
        refused(LootRefusal::RecipientInventoryFull, &error)
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_delivery_classifies_only_the_stable_capacity_refusal() {
        assert_eq!(
            master_delivery_error(lyracore_shared::mail::INVENTORY_FULL.to_string()),
            LootRefusal::RecipientInventoryFull.as_tag()
        );
        assert_eq!(
            master_delivery_error("no such item 123".to_string()),
            "no such item 123"
        );
    }

    #[test]
    fn group_loot_decision_matches_the_threshold_split_matrix() {
        use crate::group::loot_method::*;
        use GroupLootDecision::*;

        assert_eq!(group_loot_decision_for_row(FFA, 0, 2), Ffa);
        assert_eq!(group_loot_decision_for_row(FFA, 6, 2), Ffa);

        assert_eq!(group_loot_decision_for_row(ROUND_ROBIN, 0, 2), Designate);
        assert_eq!(group_loot_decision_for_row(ROUND_ROBIN, 6, 2), Designate);

        assert_eq!(
            group_loot_decision_for_row(GROUP, 1, 2),
            Designate,
            "Poor/Normal (1) is below Uncommon (2)"
        );
        assert_eq!(
            group_loot_decision_for_row(GROUP, 2, 2),
            Roll,
            "exactly at the threshold rolls (inclusive)"
        );
        assert_eq!(
            group_loot_decision_for_row(GROUP, 4, 2),
            Roll,
            "well above the threshold rolls"
        );

        assert_eq!(group_loot_decision_for_row(NEED_BEFORE_GREED, 0, 2), Roll);
        assert_eq!(group_loot_decision_for_row(NEED_BEFORE_GREED, 6, 6), Roll);

        assert_eq!(group_loot_decision_for_row(MASTER, 1, 2), Designate);
        assert_eq!(group_loot_decision_for_row(MASTER, 2, 2), Master);
        assert_eq!(group_loot_decision_for_row(MASTER, 6, 2), Master);

        assert_eq!(group_loot_decision_for_row(255, 6, 2), Ffa);
    }

    #[test]
    fn advance_rr_cursor_skips_offline_members_and_wraps() {
        assert_eq!(advance_rr_cursor(&[true, true, true], 0), Some((0, 1)));
        assert_eq!(advance_rr_cursor(&[true, true, true], 1), Some((1, 2)));
        assert_eq!(advance_rr_cursor(&[true, true, true], 2), Some((2, 0)));
        assert_eq!(
            advance_rr_cursor(&[true, false, true], 1),
            Some((2, 0)),
            "index 1 is offline, skip to 2"
        );
        assert_eq!(
            advance_rr_cursor(&[true, false, false], 2),
            Some((0, 1)),
            "wraps around to the only online member"
        );
        assert_eq!(advance_rr_cursor(&[false, false], 0), None);
        assert_eq!(
            advance_rr_cursor(&[], 0),
            None,
            "an empty roster never panics"
        );
    }

    #[test]
    fn contending_tier_need_beats_greed_and_empty_means_all_passed() {
        use vote_kind::*;
        assert_eq!(contending_tier(&[PASS, GREED, NEED, GREED]), vec![2]);
        assert_eq!(contending_tier(&[NEED, PASS, NEED]), vec![0, 2]);
        assert_eq!(contending_tier(&[PASS, GREED, GREED]), vec![1, 2]);
        assert!(contending_tier(&[PASS, PASS, PASS]).is_empty());
        assert!(contending_tier(&[]).is_empty());
    }

    #[test]
    fn pick_roll_winner_picks_the_unique_max_or_none_on_a_tie() {
        assert_eq!(pick_roll_winner(&[50, 99, 12]), Some(1));
        assert_eq!(pick_roll_winner(&[1]), Some(0));
        assert_eq!(
            pick_roll_winner(&[100, 100]),
            None,
            "a tie for the max must re-roll, never pick arbitrarily"
        );
        assert_eq!(
            pick_roll_winner(&[50, 90, 90, 10]),
            None,
            "a tie among a subset still blocks a decision"
        );
        assert!(pick_roll_winner(&[]).is_none());
    }
}
