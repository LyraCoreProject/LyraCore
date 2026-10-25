use spacetimedb::{table, ReducerContext, Table};

use crate::actor::{ActionRefusal, ActionRefusalKind};
use crate::character::game_character;
use crate::game_group_member;
use crate::game_world_entity;
use crate::{game_character_quest, game_quest_objective};
use lyracore_shared::loot::LootRefusal;
use lyracore_shared::loot_roll::event_kind as roll_event_kind;

mod rolls;
pub use rolls::{
    game_corpse_loot_eligible, game_loot_roll, game_loot_roll_promotion_receipt,
    game_loot_roll_vote, CorpseLootEligible, LootRoll, LootRollPromotionReceipt, LootRollVote,
};

pub(crate) use rolls::{
    apply_group_loot_rules, apply_master_give, cast_vote_on, force_resolve_rolls_for_disband,
    sweep_loot_rolls,
};

pub(crate) mod tag;
pub(crate) use tag::{
    corpse_access, corpse_access_gate, corpse_eligible_for_access, corpse_eligible_recipients,
    death_entitlement,
};
pub use tag::{
    game_creature_loot_tag_group, game_creature_quest_tap, game_creature_quest_tap_member,
    CreatureLootTagGroup, CreatureQuestTap, CreatureQuestTapMember,
};

#[allow(unused_imports)]
pub(crate) use crate::professions::{
    apply_disenchant, apply_enchant_item, apply_fish, entry_is_beast, skin_corpse,
};

#[table(accessor = game_corpse_loot, public, index(accessor = by_corpse, btree(columns = [corpse_guid])))]
pub struct CorpseLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub corpse_guid: u64,
    pub slot: u8,
    pub item_entry: u32,
    pub count: u32,
    #[default(false)]
    pub quest_only: bool,
    // Nonzero reserves the row for one Character, including a Loot Roll winner with full bags.
    #[default(0u64)]
    pub reserved_for: u64,
    #[default(0u64)]
    pub designated_looter_guid: u64,
    // Even the master must use the explicit assignment path.
    #[default(false)]
    pub master_only: bool,
    // Withheld rows may belong to a Loot Roll promoted to Realm-core.
    #[default(false)]
    pub withheld: bool,
    #[default(0)]
    pub random_property_id: u32,
}

#[table(accessor = game_creature_loot, public, index(accessor = by_creature, btree(columns = [creature_entry])))]
pub struct CreatureLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub item_entry: u32,
    pub chance_bp: u32, // Basis points; group_id > 0 uses this as a weight.
    pub count: u32,
    pub group_id: u32, // Zero is independent; nonzero selects a pool with at most one drop.
    #[default(false)]
    pub quest_only: bool,
}

pub(crate) fn loot_drops(chance_bp: u32, roll_0_9999: u32) -> bool {
    roll_0_9999 < chance_bp
}

pub(crate) fn group_pick(member_bp: &[u32], roll_0_9999: u32) -> Option<usize> {
    let mut cum = 0u32;
    for (i, &bp) in member_bp.iter().enumerate() {
        cum = cum.saturating_add(bp);
        if roll_0_9999 < cum {
            return Some(i);
        }
    }
    None
}

pub(crate) fn rank_money_multiplier_permille(rank: u8) -> u32 {
    match rank {
        0 => 1000, // Normal.
        1 => 1500, // Elite.
        2 => 2000, // Rare elite.
        3 => 3000, // Boss.
        4 => 2000, // Rare.
        _ => 1000,
    }
}

pub(crate) fn scale_money_for_rank(money: u32, rank: u8) -> u32 {
    let permille = rank_money_multiplier_permille(rank);
    ((money as u64 * permille as u64) / 1000) as u32
}

pub(crate) fn roll_loot_rows_quest_aware(
    ctx: &ReducerContext,
    rows: Vec<(u32, u32, u32, u32, bool)>, // (item_entry, chance_bp, count, group_id, quest_only)
) -> Vec<(u32, u32, bool)> {
    // (item_entry, count, quest_only)
    let mut winners = Vec::new();
    let mut grouped: std::collections::BTreeMap<u32, Vec<(u32, u32, u32, bool)>> =
        std::collections::BTreeMap::new();
    for (item_entry, chance_bp, count, group_id, quest_only) in rows {
        if group_id == 0 {
            if loot_drops(chance_bp, ctx.random::<u32>() % 10000) {
                winners.push((item_entry, count.max(1), quest_only));
            }
        } else {
            grouped
                .entry(group_id)
                .or_default()
                .push((item_entry, chance_bp, count, quest_only));
        }
    }
    for (_gid, members) in grouped {
        let weights: Vec<u32> = members.iter().map(|(_, bp, _, _)| *bp).collect();
        if let Some(i) = group_pick(&weights, ctx.random::<u32>() % 10000) {
            let (item_entry, _bp, count, quest_only) = members[i];
            winners.push((item_entry, count.max(1), quest_only));
        }
    }
    winners
}

pub(crate) fn roll_loot_rows(
    ctx: &ReducerContext,
    rows: Vec<(u32, u32, u32, u32)>,
) -> Vec<(u32, u32)> {
    let tagged: Vec<(u32, u32, u32, u32, bool)> = rows
        .into_iter()
        .map(|(item_entry, chance_bp, count, group_id)| {
            (item_entry, chance_bp, count, group_id, false)
        })
        .collect();
    roll_loot_rows_quest_aware(ctx, tagged)
        .into_iter()
        .map(|(item_entry, count, _quest_only)| (item_entry, count))
        .collect()
}

pub(crate) fn quest_take_allowed(
    reserved_for: u64,
    taker_guid: u64,
    taker_needs_item: bool,
) -> bool {
    lyracore_shared::loot::quest_take_allowed(reserved_for, taker_guid, taker_needs_item)
}

pub(crate) fn group_loot_take_allowed(
    withheld: bool,
    reserved_for: u64,
    master_only: bool,
    designated_looter_guid: u64,
    taker_guid: u64,
) -> bool {
    lyracore_shared::loot::group_loot_take_allowed(
        withheld,
        reserved_for,
        master_only,
        designated_looter_guid,
        taker_guid,
    )
}

pub(crate) fn clone_targets(other_members: &[(u64, bool)]) -> Vec<u64> {
    other_members
        .iter()
        .filter(|(_, needs)| *needs)
        .map(|(guid, _)| *guid)
        .collect()
}

pub(crate) fn next_free_slots(used_slots: &[u8], count: usize) -> Vec<u8> {
    let mut next = used_slots
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m.saturating_add(1));
    (0..count)
        .map(|_| {
            let s = next;
            next = next.saturating_add(1);
            s
        })
        .collect()
}

pub(crate) fn killer_needs_item(ctx: &ReducerContext, killer: Option<u64>, item: u32) -> bool {
    let Some(killer_guid) = killer else {
        return false;
    };
    let in_raid = crate::group::in_raid(ctx, killer_guid);
    let active: Vec<u32> = ctx
        .db
        .game_character_quest()
        .by_character()
        .filter(&killer_guid)
        .filter(|quest| !quest.rewarded)
        .map(|quest| quest.quest_entry)
        .collect();
    let objectives = active.iter().flat_map(|quest_entry| {
        let quest_type = crate::quest::quest_type_of(ctx, *quest_entry);
        ctx.db
            .game_quest_objective()
            .by_quest()
            .filter(quest_entry)
            .map(move |objective| lyracore_shared::loot::QuestItemObjective {
                quest_entry: objective.quest_entry,
                quest_type,
                kind: objective.kind,
                target_entry: objective.target_entry,
                required_count: objective.required_count,
            })
    });
    lyracore_shared::loot::needs_quest_item(
        in_raid,
        &active,
        objectives,
        item,
        crate::items::item_count(ctx, killer_guid, item),
    )
}

// Purge pickpocket residue before death loot reuses the same guid and slot numbers.
pub(crate) fn purge_corpse_residue(ctx: &ReducerContext, guid: u64) {
    let loot = ctx.db.game_corpse_loot();
    let stale_loot: Vec<u64> = loot.by_corpse().filter(&guid).map(|l| l.id).collect();
    for id in stale_loot {
        loot.id().delete(id);
    }
    let eligible = ctx.db.game_corpse_loot_eligible();
    let stale_eligible: Vec<u64> = eligible.by_corpse().filter(&guid).map(|e| e.id).collect();
    for id in stale_eligible {
        eligible.id().delete(id);
    }
}

// Preserve withheld rows: their live Loot Roll may exist only on Realm-core.
pub(crate) fn reap_corpse_loot_family(ctx: &ReducerContext, corpse_guid: u64) {
    let loot = ctx.db.game_corpse_loot();
    let stale: Vec<u64> = loot
        .by_corpse()
        .filter(&corpse_guid)
        .filter(|l| !l.withheld)
        .map(|l| l.id)
        .collect();
    for id in stale {
        loot.id().delete(id);
    }
    let eligible = ctx.db.game_corpse_loot_eligible();
    let stale: Vec<u64> = eligible
        .by_corpse()
        .filter(&corpse_guid)
        .map(|e| e.id)
        .collect();
    for id in stale {
        eligible.id().delete(id);
    }
    let rolls = ctx.db.game_loot_roll();
    let votes = ctx.db.game_loot_roll_vote();
    let stale_rolls: Vec<u64> = rolls
        .by_corpse()
        .filter(&corpse_guid)
        .map(|r| r.id)
        .collect();
    for roll_id in stale_rolls {
        let stale_votes: Vec<u64> = votes.by_roll().filter(&roll_id).map(|v| v.id).collect();
        for id in stale_votes {
            votes.id().delete(id);
        }
        rolls.id().delete(roll_id);
    }
}

pub(crate) fn insert_corpse_rows(
    ctx: &ReducerContext,
    corpse_guid: u64,
    winners: Vec<(u32, u32, bool)>,
) -> bool {
    let mut inserted = 0;
    for (item_entry, count, quest_only) in winners {
        let random_property_id = match crate::items::select_loot_property(ctx, item_entry) {
            Ok(id) => id,
            Err(reason) => {
                spacetimedb::log::error!("loot item {item_entry}: {reason}");
                continue;
            }
        };
        ctx.db.game_corpse_loot().insert(CorpseLoot {
            id: 0,
            corpse_guid,
            slot: inserted as u8,
            item_entry,
            count,
            quest_only,
            reserved_for: 0,
            designated_looter_guid: 0,
            master_only: false,
            withheld: false,
            random_property_id,
        });
        inserted += 1;
    }
    inserted != 0
}

pub(crate) fn roll_creature_loot(
    ctx: &ReducerContext,
    creature_entry: u32,
    corpse_guid: u64,
) -> bool {
    let raw: Vec<(u32, u32, u32, u32, bool)> = ctx
        .db
        .game_creature_loot()
        .by_creature()
        .filter(&creature_entry)
        .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id, r.quest_only))
        .collect();
    let winners = roll_loot_rows_quest_aware(ctx, raw);
    insert_corpse_rows(ctx, corpse_guid, winners)
}

// The importer resolves PickpocketLootId to creature_entry before storage.
#[table(accessor = game_pickpocket_loot, public, index(accessor = by_creature, btree(columns = [creature_entry])))]
pub struct GamePickpocketLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    pub quest_only: bool,
}

#[table(accessor = game_gameobject_loot, public, index(accessor = by_loot, btree(columns = [loot_id])))]
pub struct GameObjectLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub loot_id: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    pub quest_only: bool,
}

#[table(accessor = game_skinning_loot, public, index(accessor = by_skin, btree(columns = [skin_loot_id])))]
pub struct GameSkinningLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub skin_loot_id: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
}

#[table(accessor = game_fishing_loot, public, index(accessor = by_zone, btree(columns = [zone_id])))]
pub struct GameFishingLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub zone_id: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
}

pub(crate) fn roll_pickpocket_loot(ctx: &ReducerContext, creature_entry: u32, target_guid: u64) {
    let raw: Vec<(u32, u32, u32, u32, bool)> = ctx
        .db
        .game_pickpocket_loot()
        .by_creature()
        .filter(&creature_entry)
        .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id, r.quest_only))
        .collect();
    let winners = roll_loot_rows_quest_aware(ctx, raw);
    if winners.is_empty() {
        return;
    }
    insert_corpse_rows(ctx, target_guid, winners);
    refresh_lootable(ctx, target_guid);
}

pub(crate) fn clone_quest_loot_for_group(
    ctx: &ReducerContext,
    taker_guid: u64,
    corpse_guid: u64,
    item_entry: u32,
    count: u32,
    random_property_id: u32,
) {
    let Some(m) = crate::group::group_of(ctx, taker_guid) else {
        return;
    };
    let others: Vec<(u64, bool)> = ctx
        .db
        .game_group_member()
        .by_group()
        .filter(&m.group_id)
        .filter(|gm| gm.character_guid != taker_guid)
        .map(|gm| {
            (
                gm.character_guid,
                killer_needs_item(ctx, Some(gm.character_guid), item_entry),
            )
        })
        .collect();
    let targets = clone_targets(&others);
    if targets.is_empty() {
        return;
    }
    let used_slots: Vec<u8> = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&corpse_guid)
        .map(|l| l.slot)
        .collect();
    let slots = next_free_slots(&used_slots, targets.len());
    for (target_guid, slot) in targets.into_iter().zip(slots) {
        ctx.db.game_corpse_loot().insert(CorpseLoot {
            id: 0,
            corpse_guid,
            slot,
            item_entry,
            count,
            quest_only: true,
            reserved_for: target_guid,
            designated_looter_guid: 0,
            master_only: false,
            withheld: false,
            random_property_id,
        });
    }
}

// Use death-time eligibility; a current roster would admit later joiners.
pub(crate) fn clone_quest_loot_for_eligible(
    ctx: &ReducerContext,
    taker_guid: u64,
    corpse_guid: u64,
    item_entry: u32,
    count: u32,
    random_property_id: u32,
) {
    let others: Vec<(u64, bool)> = corpse_eligible_recipients(ctx, corpse_guid)
        .into_iter()
        .filter(|eligible_guid| *eligible_guid != taker_guid)
        .map(|eligible_guid| {
            (
                eligible_guid,
                killer_needs_item(ctx, Some(eligible_guid), item_entry),
            )
        })
        .collect();
    let targets = clone_targets(&others);
    if targets.is_empty() {
        return;
    }
    let used_slots: Vec<u8> = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&corpse_guid)
        .map(|loot| loot.slot)
        .collect();
    let slots = next_free_slots(&used_slots, targets.len());
    for (target_guid, slot) in targets.into_iter().zip(slots) {
        ctx.db.game_corpse_loot().insert(CorpseLoot {
            id: 0,
            corpse_guid,
            slot,
            item_entry,
            count,
            quest_only: true,
            reserved_for: target_guid,
            designated_looter_guid: 0,
            master_only: false,
            withheld: false,
            random_property_id,
        });
    }
}

// Squared yards.
pub(crate) const LOOT_RANGE_SQ: f32 = 100.0;

pub(crate) fn corpse_is_looted(ctx: &ReducerContext, corpse_guid: u64, money: u32) -> bool {
    money == 0
        && ctx
            .db
            .game_corpse_loot()
            .by_corpse()
            .filter(&corpse_guid)
            .next()
            .is_none()
}

pub(crate) fn refused(refusal: LootRefusal, detail: &str) -> String {
    refuse(refusal, detail).as_tag().to_string()
}

pub(crate) fn refuse(refusal: LootRefusal, detail: &str) -> LootRefusal {
    spacetimedb::log::info!("loot refused {}: {detail}", refusal.as_tag());
    refusal
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn request_open_creature_corpse(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
) -> Result<(), ActionRefusal> {
    corpse_open_gate(ctx, actor_guid, corpse_guid)
        .map_err(|(kind, refusal)| ActionRefusal::new(kind, refusal.as_tag()))
}

pub(crate) fn open_creature_corpse(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
) -> Result<(), LootRefusal> {
    corpse_open_gate(ctx, actor_guid, corpse_guid).map_err(|(_, refusal)| refusal)
}

fn corpse_open_gate(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
) -> Result<(), (ActionRefusalKind, LootRefusal)> {
    let actor = ctx
        .db
        .game_world_entity()
        .guid()
        .find(actor_guid)
        .ok_or_else(|| {
            (
                ActionRefusalKind::MissingActor,
                refuse(LootRefusal::LooterUnavailable, "looter not in world"),
            )
        })?;
    if actor.dead {
        return Err((
            ActionRefusalKind::DeadActor,
            refuse(
                LootRefusal::LooterUnavailable,
                "dead Characters cannot loot",
            ),
        ));
    }
    let corpse = ctx
        .db
        .game_world_entity()
        .guid()
        .find(corpse_guid)
        .ok_or_else(|| {
            (
                ActionRefusalKind::MissingTarget,
                refuse(LootRefusal::NoLootSource, "no such corpse"),
            )
        })?;
    if corpse.is_player() || !corpse.dead {
        return Err((
            ActionRefusalKind::MissingTarget,
            refuse(
                LootRefusal::NoLootSource,
                "Loot Source is not a creature corpse",
            ),
        ));
    }
    if corpse.map_id != actor.map_id || corpse.instance_id != actor.instance_id {
        return Err((
            ActionRefusalKind::OtherPartition,
            refuse(
                LootRefusal::OutOfRange,
                "Loot Source is on another map or instance",
            ),
        ));
    }
    let (dx, dy, dz) = (corpse.x - actor.x, corpse.y - actor.y, corpse.z - actor.z);
    if dx * dx + dy * dy + dz * dz > LOOT_RANGE_SQ {
        return Err((
            ActionRefusalKind::OutOfRange,
            refuse(LootRefusal::OutOfRange, "corpse is out of reach"),
        ));
    }
    corpse_access(ctx, actor_guid, corpse_guid).map_err(|refusal| {
        (
            ActionRefusalKind::CannotAct,
            refuse(
                refusal,
                &format!("actor_guid={actor_guid} corpse_guid={corpse_guid}"),
            ),
        )
    })
}

pub(crate) fn apply_loot_money(
    ctx: &ReducerContext,
    looter_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    claim_money(ctx, looter_guid, target_guid).map_err(|refusal| refusal.as_tag().into())
}

pub(crate) fn claim_money(
    ctx: &ReducerContext,
    looter_guid: u64,
    target_guid: u64,
) -> Result<(), LootRefusal> {
    let entities = ctx.db.game_world_entity();
    let mut looter = entities
        .guid()
        .find(looter_guid)
        .ok_or_else(|| refuse(LootRefusal::LooterUnavailable, "looter not in world"))?;
    if looter.dead {
        return Err(refuse(
            LootRefusal::LooterUnavailable,
            "dead players cannot loot",
        ));
    }
    let mut corpse = entities
        .guid()
        .find(target_guid)
        .ok_or_else(|| refuse(LootRefusal::NoLootSource, "no such corpse"))?;

    if corpse.is_player() {
        return Err(refuse(LootRefusal::NoLootSource, "cannot loot a player"));
    }
    if !corpse.dead {
        return Err(refuse(
            LootRefusal::NoLootSource,
            "Loot Source is not a corpse",
        ));
    }
    if corpse.map_id != looter.map_id {
        return Err(refuse(LootRefusal::OutOfRange, "corpse on another map"));
    }
    if corpse.instance_id != looter.instance_id {
        return Err(refuse(
            LootRefusal::OutOfRange,
            "corpse in another instance",
        ));
    }
    let (dx, dy, dz) = (
        corpse.x - looter.x,
        corpse.y - looter.y,
        corpse.z - looter.z,
    );
    if dx * dx + dy * dy + dz * dz > LOOT_RANGE_SQ {
        return Err(refuse(LootRefusal::OutOfRange, "corpse out of range"));
    }

    corpse_access(ctx, looter_guid, target_guid).map_err(|refusal| {
        refuse(
            refusal,
            &format!("actor_guid={looter_guid} corpse_guid={target_guid}"),
        )
    })?;
    if corpse.money == 0 {
        return Err(refuse(LootRefusal::NothingToLoot, "corpse purse is empty"));
    }

    let amount = corpse.money;
    corpse.money = 0;
    corpse.update(ctx);

    let recipients = corpse_eligible_recipients(ctx, target_guid);
    let grouped = money_is_grouped(&recipients);
    let shares = split_money(amount, &recipients, looter_guid);

    // Credit the looter in this local copy to avoid overwriting a separately updated purse.
    for &(recipient_guid, share) in &shares {
        if share == 0 {
            continue;
        }
        if recipient_guid == looter_guid {
            looter.money = looter.money.saturating_add(share);
        } else {
            credit_purse(ctx, recipient_guid, share);
        }
    }
    looter.update(ctx);

    if grouped {
        for &(recipient_guid, share) in &shares {
            if share == 0 {
                continue;
            }
            crate::group::push_event(
                ctx,
                recipient_guid,
                roll_event_kind::MONEY_SHARE,
                0,
                lyracore_shared::loot_roll::encode_money_share(share),
            );
        }
    }

    refresh_lootable(ctx, target_guid);
    Ok(())
}

// During Transfer, defer shares into Escrow so an in-flight recipient still gets paid.
fn credit_purse(ctx: &ReducerContext, recipient_guid: u64, share: u32) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(recipient_guid) {
        e.money = e.money.saturating_add(share);
        e.update(ctx);
        return;
    }
    crate::transfer::defer_money_delta(ctx, recipient_guid, share);
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(recipient_guid) {
        c.money = c.money.saturating_add(share);
        chars.guid().update(c);
    }
}

pub(crate) fn split_money(total: u32, recipients: &[u64], looter_guid: u64) -> Vec<(u64, u32)> {
    if recipients.is_empty() {
        return vec![(looter_guid, total)];
    }
    let n = recipients.len() as u32;
    let share = total / n;
    let remainder = total - share * n;
    let mut looter_found = false;
    let mut shares: Vec<(u64, u32)> = recipients
        .iter()
        .map(|&guid| {
            let mut amt = share;
            if guid == looter_guid {
                amt += remainder;
                looter_found = true;
            }
            (guid, amt)
        })
        .collect();
    if !looter_found && remainder > 0 {
        shares.push((looter_guid, remainder));
    }
    shares
}

pub(crate) fn money_is_grouped(recipients: &[u64]) -> bool {
    recipients.len() > 1
}

pub(crate) fn refresh_lootable(ctx: &ReducerContext, corpse_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut corpse) = entities.guid().find(corpse_guid) else {
        return;
    };
    let has_rows = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&corpse_guid)
        .next()
        .is_some();
    let flag = lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
    let want = if has_rows || corpse.money > 0 {
        flag
    } else {
        0
    };
    if corpse.dynamic_flags & flag != want {
        corpse.dynamic_flags = (corpse.dynamic_flags & !flag) | want;
        corpse.update(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loot_chance_is_half_open() {
        assert!(loot_drops(10000, 0));
        assert!(loot_drops(10000, 9999));
        assert!(!loot_drops(0, 0));
        assert!(loot_drops(5000, 0));
        assert!(loot_drops(5000, 4999));
        assert!(!loot_drops(5000, 5000));
        assert!(!loot_drops(5000, 9999));
        assert!(loot_drops(39, 38));
        assert!(!loot_drops(39, 39));
    }

    #[test]
    fn group_pick_is_one_weighted_band() {
        let w = [2000u32, 3000, 1000];
        assert_eq!(group_pick(&w, 0), Some(0));
        assert_eq!(group_pick(&w, 1999), Some(0));
        assert_eq!(group_pick(&w, 2000), Some(1));
        assert_eq!(group_pick(&w, 4999), Some(1));
        assert_eq!(group_pick(&w, 5000), Some(2));
        assert_eq!(group_pick(&w, 5999), Some(2));
        assert_eq!(group_pick(&w, 6000), None);
        assert_eq!(group_pick(&w, 9999), None);
        let eq = [2500u32, 2500, 2500, 2500];
        assert!(group_pick(&eq, 9999).is_some());
        assert_eq!(group_pick(&eq, 2500), Some(1));
    }

    #[test]
    fn rank_money_scales_and_rank0_is_identity() {
        for money in [0u32, 1, 17, 1000, 1_000_000, u32::MAX] {
            assert_eq!(
                scale_money_for_rank(money, 0),
                money,
                "rank-0 money must be identity for {money}"
            );
        }
        assert_eq!(scale_money_for_rank(100, 1), 150);
        assert_eq!(scale_money_for_rank(100, 2), 200);
        assert_eq!(scale_money_for_rank(100, 3), 300);
        assert_eq!(scale_money_for_rank(100, 4), 200);
        assert_eq!(scale_money_for_rank(100, 9), 100);
        assert_eq!(scale_money_for_rank(0, 3), 0);
        assert_eq!(rank_money_multiplier_permille(0), 1000);
        assert!(rank_money_multiplier_permille(1) > rank_money_multiplier_permille(0));
        assert!(rank_money_multiplier_permille(3) > rank_money_multiplier_permille(1));
        assert!(scale_money_for_rank(u32::MAX, 3) >= u32::MAX / 2);
    }

    #[test]
    fn quest_take_allowed_admits_the_reservee_unconditionally_and_needing_takers_on_shared_rows() {
        assert!(quest_take_allowed(0, 7, true));
        assert!(
            !quest_take_allowed(0, 7, false),
            "no need → no take on the unreserved shared row"
        );
        assert!(quest_take_allowed(7, 7, true));
        assert!(
            !quest_take_allowed(7, 8, true),
            "reserved for someone else — not this taker's to claim"
        );
        assert!(
            quest_take_allowed(7, 7, false),
            "the reservation IS the grant — re-checking need here would strand a reservee whose need \
             lapsed post-clone with a visible-but-untakeable row; mirrors \
             quest_row_visible_to_viewer's 'reservation is authoritative' arm"
        );
    }

    #[test]
    fn group_loot_take_allowed_gates_withheld_reserved_master_and_designated_rows() {
        assert!(
            !group_loot_take_allowed(true, 0, false, 0, 7),
            "a live roll owns the row"
        );
        assert!(
            !group_loot_take_allowed(true, 7, false, 0, 7),
            "even the future reservee waits for resolution"
        );
        assert!(group_loot_take_allowed(false, 0, false, 0, 7));
        assert!(group_loot_take_allowed(false, 7, false, 0, 7));
        assert!(
            !group_loot_take_allowed(false, 7, false, 0, 8),
            "reserved for someone else"
        );
        assert!(
            !group_loot_take_allowed(false, 7, true, 99, 8),
            "reserved wins over master_only/designated too"
        );
        assert!(
            !group_loot_take_allowed(false, 0, true, 42, 42),
            "master_only blocks even the master's OWN take"
        );
        assert!(!group_loot_take_allowed(false, 0, true, 42, 8));
        assert!(group_loot_take_allowed(false, 0, false, 42, 42));
        assert!(
            !group_loot_take_allowed(false, 0, false, 42, 8),
            "not the designated looter"
        );
    }

    #[test]
    fn clone_targets_admits_only_the_still_needing_members() {
        let others = [(10u64, true), (20u64, false), (30u64, true)];
        assert_eq!(
            clone_targets(&others),
            vec![10, 30],
            "the non-needing member 20 gets no clone"
        );
        assert!(
            clone_targets(&[]).is_empty(),
            "a solo taker (no other members) clones nothing"
        );
        assert!(
            clone_targets(&[(10, false)]).is_empty(),
            "no other member currently needs it → no clones"
        );
    }

    #[test]
    fn next_free_slots_never_collides_with_whats_already_on_the_corpse() {
        assert_eq!(next_free_slots(&[0, 1, 2], 2), vec![3, 4]);
        assert_eq!(
            next_free_slots(&[], 3),
            vec![0, 1, 2],
            "an emptied corpse starts back at slot 0"
        );
        assert_eq!(next_free_slots(&[5], 1), vec![6]);
        assert_eq!(next_free_slots(&[255], 2), vec![255, 255]);
    }

    #[test]
    fn split_money_solo_passthrough_credits_the_whole_amount_to_the_looter() {
        assert_eq!(split_money(137, &[], 7), vec![(7, 137)]);
        assert_eq!(split_money(0, &[], 7), vec![(7, 0)]);
        assert_eq!(split_money(u32::MAX, &[], 7), vec![(7, u32::MAX)]);
    }

    #[test]
    fn split_money_even_split_with_no_remainder() {
        assert_eq!(
            split_money(30, &[1, 2, 3], 1),
            vec![(1, 10), (2, 10), (3, 10)]
        );
        assert_eq!(
            split_money(100, &[10, 20, 30, 40], 10),
            vec![(10, 25), (20, 25), (30, 25), (40, 25)]
        );
    }

    #[test]
    fn split_money_remainder_goes_to_the_looter() {
        assert_eq!(
            split_money(100, &[1, 2, 3], 2),
            vec![(1, 33), (2, 34), (3, 33)]
        );
        assert_eq!(
            split_money(100, &[1, 2, 3], 1),
            vec![(1, 34), (2, 33), (3, 33)]
        );
        assert_eq!(
            split_money(100, &[1, 2, 3], 3),
            vec![(1, 33), (2, 33), (3, 34)]
        );
        assert_eq!(split_money(7, &[5, 9], 9), vec![(5, 3), (9, 4)]);
    }

    #[test]
    fn split_money_looter_outside_recipients_gets_an_extra_remainder_entry() {
        assert_eq!(
            split_money(100, &[10, 20, 30], 99),
            vec![(10, 33), (20, 33), (30, 33), (99, 1)]
        );
        assert_eq!(
            split_money(90, &[10, 20, 30], 99),
            vec![(10, 30), (20, 30), (30, 30)]
        );
    }

    #[test]
    fn split_money_single_recipient_degrades_to_whole_amount() {
        assert_eq!(split_money(50, &[7], 7), vec![(7, 50)]);
    }

    #[test]
    fn only_multiple_eligible_characters_share_money() {
        assert!(!money_is_grouped(&[]));
        assert!(!money_is_grouped(&[7]));
        assert!(money_is_grouped(&[7, 8]));
    }
}
