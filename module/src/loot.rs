//! Money + item loot from a creature corpse. The lootable copper is rolled onto the corpse's
//! `WorldEntity` (with `UNIT_DYNFLAG_LOOTABLE`) on the killing blow in `combat/`; `loot_money` moves
//! it into the looter's purse and clears the flag. Item drops are rolled into `game_corpse_loot` rows
//! on the same killing blow; the gateway reads them to populate the loot window, and taking one
//! (`CMSG_AUTOSTORE_LOOT_ITEM`) is `crate::items::take_loot` → `apply_take_loot`. [server]/[entity]
//!
//! Loot-family completeness (work-item 210) — two design decisions recorded here because they shape
//! every table/reducer below:
//!
//! 1. **FIXED 2026-07-07 (work-item 187 slice 0): quest-only drops roll UNCONDITIONALLY; visibility
//!    and takability are decided PER VIEWER / PER TAKER, not at roll time.** cmangos marks a
//!    quest-drop row with a negative `ChanceOrQuestChance`; the loot table still rolls ONCE per
//!    corpse at the killing blow and the drop-CHANCE (`chance_bp`/`group_pick`) still applies exactly
//!    as before — what changed is that the row is no longer gated on any player's quest state before
//!    it rolls. Every rolled row now carries a `quest_only` marker (`CorpseLoot.quest_only`) plus a
//!    `reserved_for` guid (`0` = shared/unclaimed by any specific character). Two consumers do the
//!    ACTUAL gating now: the gateway's `corpse_loot` read (`gateway/src/stdb/reads.rs`) filters
//!    quest_only rows PER VIEWER at loot-open time — a viewer sees an unreserved quest_only row only
//!    if they currently need the item, and always sees a row already reserved for THEM; `apply_take_loot`
//!    (`items/ops.rs`) re-validates the TAKER's own need server-side and, on the FIRST take of a
//!    still-shared quest_only row, clones it once per OTHER group member who still needs the item
//!    (each clone `reserved_for` that member, at a fresh slot) before deleting the shared original —
//!    so every grouped quest-holder gets their own independently-lootable copy, matching vanilla,
//!    instead of the item vanishing for everyone else the instant one member takes it.
//!    ORIGINAL (pre-187) DECISION, superseded: quest-only drops gated at ROLL TIME against the
//!    CREDITED KILLER only (the `Option<u64>` threaded through `kill_creature` → `roll_creature_loot`,
//!    via `killer_needs_item`/`quest_gate_rows`) — a grouped quest-holder who didn't land kill credit
//!    missed the drop entirely, and the credited killer's own take deleted the shared row for
//!    everyone else. `killer_needs_item`/`needs_item_pure` are KEPT (still the need-check primitive,
//!    now applied to the TAKER/viewer instead of the killer); `quest_gate_rows` (the old pre-roll
//!    table filter) is REMOVED — nothing calls it anymore.
//! 2. **Reference pools (`reference_loot_template`) flatten RECURSIVELY AT IMPORT, with a cycle guard.**
//!    A pool can itself reference another pool (`mincountOrRef < 0` inside `reference_loot_template`);
//!    the importer's `resolve_ref_pool` walks that chain to full depth (not one level) and writes only
//!    FLAT item rows into the SQL-loaded tables — so the module's roll (`roll_loot_rows`) stays the same
//!    one-pass independent-rows-then-groups algorithm it always was; nesting is entirely an import-time
//!    concern, invisible at roll time.

use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::character::game_character; // credit_purse's offline-recipient fallback (work-item 221)
use crate::game_creature_template;
use crate::game_group_member; // clone_quest_loot_for_group's group-roster read (work-item 187 slice 0)
use crate::game_item_instance; // the items table accessor (disenchant deletes a row; enchant updates one)
use crate::game_item_template; // the enchant/disenchant gear-class gate (require_gear) reads tmpl.class
use crate::game_player_skill; // Skinning skill-gate reads the skill row (accessor trait)
use crate::game_world_entity;
use crate::helpers::entity_by_owner;
use crate::quest::objective_kind;
use crate::{game_area, game_character_quest, game_quest_objective}; // killer_needs_item + fishing's zone resolve (209 idiom)

/// One item available to loot on a creature corpse. Born on the killing blow (combat/),
/// read by the gateway for `SMSG_LOOT_RESPONSE`, reaped when the corpse decays (creatures/). Public
/// + no RLS: loot is shown to whoever opens the corpse (the take is gated by the loot reducer, like
/// money). `item_entry` → `game_item_template` (the gateway joins it for the display id). [entity]
#[table(accessor = game_corpse_loot, public, index(accessor = by_corpse, btree(columns = [corpse_guid])))]
pub struct CorpseLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub corpse_guid: u64,
    pub slot: u8,        // loot-window slot index (0-based)
    pub item_entry: u32, // -> game_item_template.entry
    pub count: u32,
    // END-APPENDED (work-item 187 slice 0, fixing 210's recorded divergence — module doc decision #1):
    // does this row carry a QUEST-only item (rolled from a `quest_only` creature/pickpocket/
    // gameobject-loot row)? Quest rows now roll UNCONDITIONALLY (the drop-chance still applies; only
    // the pre-roll killer gate is gone) — visibility (gateway) and takability (`apply_take_loot`) are
    // decided per-viewer/per-taker instead. `#[default(false)]` so every pre-187 row (and every
    // non-quest drop) keeps behaving exactly as before — additive auto-migration.
    #[default(false)]
    pub quest_only: bool,
    // END-APPENDED (work-item 187 slice 0): `0` = the SHARED, not-yet-split row — any currently-needing
    // character may claim it, which triggers the per-member clone (`clone_quest_loot_for_group`);
    // nonzero = a character guid — this specific row is a per-member CLONE reserved for exactly that
    // character (minted the moment the shared row is first taken). Meaningless when `quest_only` is
    // false UNLESS it's a group-loot WINNER-LOCKED row (below) — GENERALIZED (work-item 187 slices
    // 2-4): nonzero now also marks "this row belongs EXCLUSIVELY to this guid" for a NEED/GREED roll
    // winner whose bag was full at grant time (`resolve_roll`'s inventory-full fallback — the item
    // sits here until they free space and re-loot). Same predicate on both the gateway's visibility
    // read and `apply_take_loot`'s gate either way: nonzero `reserved_for` always means "only this
    // guid, unconditionally". `#[default(0)]` — additive.
    #[default(0u64)]
    pub reserved_for: u64,
    // END-APPENDED (work-item 187 slices 2-4, group loot methods): three columns the module stamps
    // at KILL TIME (`apply_group_loot_rules`, called from `combat::kill_creature` — never at
    // loot-open; see the module doc below for why). All three are meaningless (stay at their
    // defaults) for an ungrouped kill, an FFA-method group, or any `quest_only` row (the two systems
    // are kept ORTHOGONAL — `apply_group_loot_rules` never touches a `quest_only` row). `#[default]`
    // on all three — additive, byte-identical for every pre-187 row.
    /// `0` = no round-robin/master restriction (FFA, or a solo/ungrouped kill); nonzero = ONLY this
    /// character guid may see/take the row via the plain loot path (ROUND_ROBIN/GROUP-below-
    /// threshold: the corpse's picked-once designee; MASTER-above-threshold: the master looter).
    #[default(0u64)]
    pub designated_looter_guid: u64,
    /// Above-threshold under the MASTER method: takable ONLY via `loot_master_give` (never the plain
    /// autostore path, not even by the master themselves — matches vanilla's explicit-assign model).
    #[default(false)]
    pub master_only: bool,
    /// A live NEED/GREED/NBG roll is in progress on this row — WITHHELD from EVERY viewer's loot
    /// window (including the eventual winner) until `resolve_roll` clears it (by deleting the row on
    /// a successful grant, or flipping `reserved_for` to the winner on an inventory-full fallback).
    #[default(false)]
    pub withheld: bool,
}

/// A creature's loot-table entry: when an `creature_entry` creature dies, its rows roll for drops.
/// `chance_bp` is BASIS POINTS (0..=10000; 10000 = always, 0 = never) so sub-1% odds are representable
/// (real loot tables are full of fractional chances). `group_id` gives vanilla "pick-one" pools:
/// `0` = an independent roll (this row drops on its own chance), `>0` = a group where AT MOST ONE
/// member drops per kill (the members' `chance_bp` are weights walked as a cumulative band — so a
/// shared loot pool of N items drops one of them, not N independent items). Multiple rows per
/// `creature_entry` form the full table. Loaded by the importer (cmangos `*_loot_template`) or seeded;
/// public + read-only, no Timestamp → SQL-loadable. A creature with NO rows drops no items. [static]
#[table(accessor = game_creature_loot, public, index(accessor = by_creature, btree(columns = [creature_entry])))]
pub struct CreatureLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub item_entry: u32,
    pub chance_bp: u32, // drop chance in basis points (0..=10000); a group member's weight if group_id>0
    pub count: u32,
    pub group_id: u32, // 0 = independent roll; >0 = pick-one group (≤1 member drops per kill)
    // END-APPENDED (work-item 210): cmangos `ChanceOrQuestChance < 0` — the item is a QUEST-drop, only
    // ever visible to a player who currently needs it (an active matching COLLECT_ITEM objective — see
    // `killer_needs_item`). Gated at ROLL TIME against the CREDITED KILLER (module doc, decision #1),
    // never re-evaluated later. `#[default(false)]` so existing rows (imported pre-210, or seeded) keep
    // rolling exactly as before (never quest-gated) — additive auto-migration.
    #[default(false)]
    pub quest_only: bool,
}

/// Independent drop test (`group_id == 0`): does an item with `chance_bp` (0..=10000) drop on a
/// `roll` in 0..10000? Half-open (`roll < chance`) so 10000 always drops and 0 never does. Pure/testable.
pub fn loot_drops(chance_bp: u32, roll_0_9999: u32) -> bool {
    roll_0_9999 < chance_bp
}

/// Pick-one group roll: given member weights in basis points and a `roll` in 0..10000, return the index
/// of the member that drops, or `None` if the roll lands past the members' cumulative weight (the
/// group's "nothing" band — only reachable when the weights sum below 10000). This is the vanilla
/// group-loot semantics: at most one item from a shared pool, weighted by chance. Pure/testable.
pub fn group_pick(member_bp: &[u32], roll_0_9999: u32) -> Option<usize> {
    let mut cum = 0u32;
    for (i, &bp) in member_bp.iter().enumerate() {
        cum = cum.saturating_add(bp);
        if roll_0_9999 < cum {
            return Some(i);
        }
    }
    None
}

/// Per-1000 copper-loot multiplier for a creature's classification `rank` — an elite/rare/boss is
/// worth more money than a normal mob of the same level. cmangos `creature_template.rank`: `0`=normal,
/// `1`=elite, `2`=rare-elite, `3`=boss, `4`=rare. Bumps: normal 1.0×, elite 1.5×, rare-elite 2.0×,
/// boss 3.0×, rare 2.0×. PERMILLE (×1000) for exact integer math; rank 0 (`1000`) is the identity so a
/// normal kill's purse is BYTE-IDENTICAL to before. Unknown rank → normal (never amplify). Pure —
/// unit-tested. (HP/damage rank scaling live in `creatures/`/`combat/`; this is the loot twin.)
pub fn rank_money_multiplier_permille(rank: u8) -> u32 {
    match rank {
        0 => 1000, // normal — identity (baseline)
        1 => 1500, // elite
        2 => 2000, // rare-elite
        3 => 3000, // boss
        4 => 2000, // rare
        _ => 1000, // unknown rank → treat as normal
    }
}

/// Apply the rank money multiplier to a rolled copper amount (integer permille math, saturating so a
/// large roll × a high multiplier can't wrap a `u32`). Shared by the killing-blow money roll
/// (combat/) and the unit test so the live scaling and the asserted numbers never drift. Rank 0 →
/// the amount unchanged. Pure.
pub fn scale_money_for_rank(money: u32, rank: u8) -> u32 {
    let permille = rank_money_multiplier_permille(rank);
    ((money as u64 * permille as u64) / 1000) as u32
}

/// Roll ONE generic loot table (the shape shared by every family — creature/pickpocket/gameobject/
/// skinning/fishing): independent rows (`group_id == 0`) each test their own `chance_bp`; grouped rows
/// (`group_id > 0`) are collected and rolled ONCE per group (pick-one, weighted by `chance_bp`).
/// Returns the winners as `(item_entry, count)`, in table order for independents then group order —
/// callers decide what a winner MEANS (a `game_corpse_loot` row, a direct `grant_item`, …). This is the
/// ONE roll body every family's wrapper below calls (module doc, decision #2's flip side: nesting is an
/// import-time concern, so at roll time every family looks like this same flat shape). Draws from the
/// module RNG (`ctx.random`), so — like `roll_creature_loot` before it — it has no direct unit test;
/// its pure primitives (`loot_drops`/`group_pick`) are tested below.
pub(crate) fn roll_loot_rows(
    ctx: &ReducerContext,
    rows: Vec<(u32, u32, u32, u32)>,
) -> Vec<(u32, u32)> {
    let mut winners = Vec::new();
    let mut grouped: std::collections::BTreeMap<u32, Vec<(u32, u32, u32)>> =
        std::collections::BTreeMap::new();
    for (item_entry, chance_bp, count, group_id) in rows {
        if group_id == 0 {
            if loot_drops(chance_bp, ctx.random::<u32>() % 10000) {
                winners.push((item_entry, count.max(1)));
            }
        } else {
            grouped
                .entry(group_id)
                .or_default()
                .push((item_entry, chance_bp, count));
        }
    }
    for (_gid, members) in grouped {
        let weights: Vec<u32> = members.iter().map(|(_, bp, _)| *bp).collect();
        if let Some(i) = group_pick(&weights, ctx.random::<u32>() % 10000) {
            let (item_entry, _bp, count) = members[i];
            winners.push((item_entry, count.max(1)));
        }
    }
    winners
}

/// Roll ONE generic loot table the SAME way `roll_loot_rows` does (independent rows test their own
/// `chance_bp`; grouped rows are collected and rolled ONCE per group, pick-one weighted), but carries
/// each winning row's `quest_only` flag through instead of discarding it (work-item 187 slice 0): quest
/// rows now roll into `game_corpse_loot` UNCONDITIONALLY — the drop-CHANCE still applies; only the old
/// pre-roll `killer_needs_item` GATE is gone (module doc, decision #1's fix) — so the caller needs to
/// know which winners were quest-only to stamp the new `quest_only` column. Shared by the three
/// families that carry the flag (creature/pickpocket/gameobject); skinning/fishing have no `quest_only`
/// column and keep calling the original `roll_loot_rows`. Draws from `ctx.random`, like `roll_loot_rows`.
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

/// Pure gate (work-item 187 slice 0): may `taker_guid` claim a `quest_only` row whose current
/// reservation is `reserved_for`, given `taker_needs_item` (already resolved by the caller — mirrors
/// `needs_item_pure`'s ctx/pure split)? An UNRESERVED row (`reserved_for == 0` — the shared row nobody
/// has split yet) is claimable by anyone who currently needs it; a RESERVED row is claimable by its
/// reserved owner UNCONDITIONALLY — the reservation itself is the grant (need was checked when the
/// clone was minted). Re-checking need here would contradict the gateway's display filter
/// (`quest_row_visible_to_viewer`, `gateway/src/stdb/reads.rs`), which always shows a reservee their
/// row: a member whose need lapsed between clone and take (capped the count off another corpse, say)
/// would face a visible-but-untakeable row stuck until corpse decay. Same predicate shape on both
/// sides, so a row a viewer's window shows is always a row their take can actually succeed on. Pure.
pub(crate) fn quest_take_allowed(
    reserved_for: u64,
    taker_guid: u64,
    taker_needs_item: bool,
) -> bool {
    reserved_for == taker_guid || (reserved_for == 0 && taker_needs_item)
}

/// Pure (work-item 187 slice 0): given `(character_guid, currently_needs_item)` pairs for every OTHER
/// group member (the taker already excluded by the caller), which should receive a fresh per-member
/// clone when the shared `quest_only` row is first taken? Only the still-needing ones — a member who's
/// already capped on the item (or no longer holds the quest) gets nothing, matching vanilla (the item
/// simply isn't offered to them). Pure.
pub(crate) fn clone_targets(other_members: &[(u64, bool)]) -> Vec<u64> {
    other_members
        .iter()
        .filter(|(_, needs)| *needs)
        .map(|(guid, _)| *guid)
        .collect()
}

/// Pure (work-item 187 slice 0): `count` fresh, ascending loot-window slot indices past whatever is
/// already used on a corpse (`used_slots`), so per-member quest clones never collide with the
/// just-deleted shared row's slot or any sibling drop. Saturating (a corpse loot window is
/// realistically far under 255 rows — the gateway's RAW `SMSG_LOOT_RESPONSE` builder already caps the
/// wire count at `u8::MAX`). Pure.
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

/// Pure decision behind `killer_needs_item`: does ANY of `objectives` (flattened `(quest_entry, kind,
/// target_entry)` triples for the killer's ACTIVE — unrewarded — quests) want `item` as a
/// `COLLECT_ITEM` target on a quest actually in `active_quests`? Split out so the decision is
/// unit-testable without a live `ReducerContext` (mirrors `can_skin`/`can_gather`'s pure-gate split).
pub(crate) fn needs_item_pure(
    active_quests: &[u32],
    objectives: &[(u32, u8, u32, u32)], // (quest, kind, target, required_count)
    item: u32,
    held: u32,
) -> bool {
    objectives
        .iter()
        .any(|&(quest_entry, kind, target_entry, required_count)| {
            kind == objective_kind::COLLECT_ITEM
            && target_entry == item
            && active_quests.contains(&quest_entry)
            // Capped at the requirement: a killer already holding 10/10 stops seeing the drop
            // (vanilla's behavior; the quest.rs completion check's same rule — review catch).
            && held < required_count.max(1)
        })
}

/// Does `killer` currently need `item` for an ACTIVE (unrewarded) quest's `COLLECT_ITEM` objective?
/// `killer = None` (a debug/environmental kill with no credited player) never needs anything — a
/// quest-only row is simply invisible there, matching vanilla (no quest, no drop). Fetches the killer's
/// live quest log + objectives, then defers the actual decision to the pure `needs_item_pure`.
pub(crate) fn killer_needs_item(ctx: &ReducerContext, killer: Option<u64>, item: u32) -> bool {
    let Some(killer_guid) = killer else {
        return false;
    };
    let active: Vec<u32> = ctx
        .db
        .game_character_quest()
        .by_character()
        .filter(&killer_guid)
        .filter(|cq| !cq.rewarded)
        .map(|cq| cq.quest_entry)
        .collect();
    if active.is_empty() {
        return false;
    }
    let objs: Vec<(u32, u8, u32, u32)> = active
        .iter()
        .flat_map(|q| {
            ctx.db
                .game_quest_objective()
                .by_quest()
                .filter(q)
                .map(|o| (o.quest_entry, o.kind, o.target_entry, o.required_count))
        })
        .collect();
    let held = crate::items::item_count(ctx, killer_guid, item);
    needs_item_pure(&active, &objs, item, held)
}

/// Roll a creature's loot table into `game_corpse_loot` rows on its corpse; returns whether anything
/// dropped (so the caller sets `UNIT_DYNFLAG_LOOTABLE`). Data-driven: reads `game_creature_loot` for
/// `creature_entry`, then rolls independent rows + one weighted pick per group with the
/// `roll_loot_rows_quest_aware` core (work-item 187 slice 0: `quest_only` rows roll UNCONDITIONALLY
/// now — no killer gate — the flag just rides along onto the inserted row for the gateway/take-path to
/// gate per-viewer/per-taker instead), inserting the winners at sequential loot slots with a fresh
/// `reserved_for = 0` (unclaimed). No table rows → nothing drops (no universal placeholder). Shared by
/// the real killing blow (combat/) and the debug kill (debug.rs) so both produce identical loot.
/// [server]
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
    let dropped = !winners.is_empty();
    for (slot, (item_entry, count, quest_only)) in winners.into_iter().enumerate() {
        ctx.db.game_corpse_loot().insert(CorpseLoot {
            id: 0,
            corpse_guid,
            slot: slot as u8,
            item_entry,
            count,
            quest_only,
            reserved_for: 0,
            // Group-loot stamping (work-item 187 slices 2-4) happens AFTER this fn returns, in
            // `apply_group_loot_rules` (combat::kill_creature) — this insert always starts FFA.
            designated_looter_guid: 0,
            master_only: false,
            withheld: false,
        });
    }
    dropped
}

// ===========================================================================================
//  LOOT-FAMILY COMPLETENESS (work-item 210) — the four families `game_creature_loot` never covered:
//  pickpocket / gameobject-chest / skinning / fishing. All four share the SAME row shape as
//  `CreatureLoot` (item/chance_bp/count/group_id, some also carrying `quest_only`); the importer maps
//  cmangos's `*_loot_template` family onto them (see importer's `parse_loot_family`). Public + no
//  Timestamp → SQL-loadable, exactly like `CreatureLoot`. [static]
// ===========================================================================================

/// `pickpocketing_loot_template` — a creature's PICKPOCKET table, keyed directly on
/// `creature_entry` (the importer collapses cmangos's `PickpocketLootId` indirection the same way it
/// already does for `CreatureLoot`'s `LootId`). Rolled by `roll_pickpocket_loot` (E_PICKPOCKET,
/// spell/cast.rs) into `game_corpse_loot` keyed on the LIVE creature's guid — pickpocketing doesn't
/// kill it, so the shared corpse-loot table just serves as "this guid has a loot window" (like the
/// GO-chest reuse below). [static]
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

/// `gameobject_loot_template` — a lootable gameobject's (CHEST) table, keyed on `loot_id`
/// (`GameObjectTemplate.data1` — the REAL cmangos `gameobject_template.Data1`, read verbatim by the
/// importer). Rolled by `apply_use_gameobject`'s CHEST arm (gameobject.rs) into `game_corpse_loot`
/// keyed on the GO's guid, multi-slot (unlike the old single-`data0`-item shortcut it replaces). A
/// `loot_id` of 0 (unimported / the seed/demo chest) has no rows here — the module falls back to the
/// legacy single-item `data0` drop, so existing seed/demo data + tests stay byte-identical. [static]
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

/// `skinning_loot_template` — a level-banded leather table, keyed on `skin_loot_id`
/// (`CreatureTemplate.skin_loot_id` — cmangos's `SkinLootId`; several creatures of the same level band
/// share ONE `skin_loot_id`, so this is NOT collapsed onto `creature_entry` the way pickpocket is).
/// Rolled by `skin_corpse` (this file) in place of the flat `LEATHER_ENTRY` shortcut; a `skin_loot_id`
/// of 0 (unimported, or a seeded/test beast) falls back to the flat leather, byte-identical to before.
/// No `quest_only` — no cmangos skinning row is ever quest-gated. [static]
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

/// `fishing_loot_template` — a ZONE-keyed catch table (`zone_id` is a cmangos/AreaTable zone id, NOT a
/// creature/loot-id indirection — cmangos's `fishing_loot_template.entry` IS the zone). Rolled by
/// `apply_fish` (this file) once the caster's zone is resolved (`fishing_zone_id`, the work-item 209
/// terrain-area one-hop-parent-walk idiom); an unresolved zone OR a zone with no rows (or a roll that
/// lands on nothing) falls back to the flat `FISH_POOL`, byte-identical to the pre-210 alpha. No
/// `quest_only` — fishing junk isn't quest-gated in the family this slice imports. [static]
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

/// Roll a creature's PICKPOCKET table into `game_corpse_loot` rows keyed on the LIVE target's guid, then
/// refresh `UNIT_DYNFLAG_LOOTABLE` (`refresh_lootable`, work-item 186) so a still-alive-but-pickpocketed
/// creature shows the loot cursor if anything rolled. Quest-only rows roll UNCONDITIONALLY now
/// (work-item 187 slice 0 — no rogue gate; the flag rides onto the row for per-viewer/per-taker gating
/// downstream). A no-row table (or an all-miss roll) inserts nothing and never touches the flag — the
/// existing money-only path (spell/cast.rs) is unaffected either way. Called from `E_PICKPOCKET`
/// (spell/cast.rs), inside the SAME `!target.pickpocketed` once-gate the copper roll already uses, so
/// items can never be drained twice per life either. [server]
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
    for (slot, (item_entry, count, quest_only)) in winners.into_iter().enumerate() {
        ctx.db.game_corpse_loot().insert(CorpseLoot {
            id: 0,
            corpse_guid: target_guid,
            slot: slot as u8,
            item_entry,
            count,
            quest_only,
            reserved_for: 0,
            // Pickpocketing is never group-loot-gated (out of scope — a single player's action).
            designated_looter_guid: 0,
            master_only: false,
            withheld: false,
        });
    }
    refresh_lootable(ctx, target_guid);
}

/// On the FIRST take of a shared `quest_only` row (work-item 187 slice 0, `apply_take_loot`'s caller):
/// mint one independent, per-member `reserved_for` clone for every OTHER member of `taker_guid`'s group
/// who currently still needs `item_entry` — so a grouped quest-holder's copy doesn't vanish the instant
/// a fellow member loots theirs. A solo `taker_guid` (`group_of` returns `None`) touches nothing —
/// solo behavior is unchanged (design requirement #4). Clones land at FRESH slots past whatever is
/// already on the corpse (`next_free_slots`), never colliding with the just-deleted original's slot or
/// any sibling drop. Naturally bounded by [`crate::group::GROUP_MAX_MEMBERS`] (a group can't exceed 5,
/// so at most 4 clones per take). [server]
pub(crate) fn clone_quest_loot_for_group(
    ctx: &ReducerContext,
    taker_guid: u64,
    corpse_guid: u64,
    item_entry: u32,
    count: u32,
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
        });
    }
}

/// Max distance to loot a corpse: (10 yd)². Generous — the vanilla client walks into interaction
/// range itself before sending `CMSG_LOOT_MONEY`, so this only rejects clearly-out-of-range abuse.
pub(crate) const LOOT_RANGE_SQ: f32 = 100.0;

/// Take the money from a creature corpse (`CMSG_LOOT_MONEY`, routed by the gateway with the corpse
/// the player currently has open). Validates the corpse is dead, has money, is on the same map, and
/// is in range; then zeroes the corpse money and clears `UNIT_DYNFLAG_LOOTABLE` (relayed so the loot
/// sparkle disappears). SOLO (no `game_corpse_loot_eligible` snapshot for this corpse): the whole
/// amount credits the looter's purse, unchanged from before work-item 221, and NO event fires — the
/// client prints its own local "You loot X copper" line. GROUPED (>=2 recipients snapshotted at KILL
/// time): the copper splits evenly across the snapshot, remainder to the looter, each purse credited
/// exactly once (`crate::loot::split_money`/`credit_purse`), and a `MONEY_SHARE` event fires per
/// recipient (`SMSG_LOOT_MONEY_NOTIFY(share)` on relay) — see work-item 221. Authorized via
/// `ctx.sender` like all player ops. A second call finds `money == 0` and errors (no dupe).
#[reducer]
pub fn loot_money(ctx: &ReducerContext, target_guid: u64) -> Result<(), String> {
    let looter =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "looter not in world".to_string())?;
    apply_loot_money(ctx, looter.guid, target_guid)
}

/// Shared core: take corpse money by explicit looter guid — the body behind the `loot_money`
/// reducer and `actor::loot_money` (147/149). Pure code motion; every gate byte-identical.
pub(crate) fn apply_loot_money(
    ctx: &ReducerContext,
    looter_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut looter = entities
        .guid()
        .find(looter_guid)
        .ok_or_else(|| "looter not in world".to_string())?;
    // A dead looter (killed with a loot window still open) can't loot — gate on the same `dead` flag
    // every other action checks, so death stays server-authoritative.
    if looter.dead {
        return Err("dead players cannot loot".to_string());
    }
    let mut corpse = entities
        .guid()
        .find(target_guid)
        .ok_or_else(|| "no such corpse".to_string())?;

    // Only CREATURE corpses are lootable. A dead/ghost PLAYER is also `dead` with a non-zero purse
    // (loaded from character.money), so without this guard a second player in range could drain the
    // victim's coinage (PvP purse theft). Money loot is creature-only until real player-loot rules.
    if corpse.is_player() {
        return Err("cannot loot a player".to_string());
    }
    if !corpse.dead {
        return Err("target is not a corpse".to_string());
    }
    if corpse.money == 0 {
        return Err("nothing to loot".to_string());
    }
    // Map + instance gated (190 slice 2): a creature corpse is a `game_world_entity` row, so its
    // `instance_id` came free with slice 1 — a looter can never reach across an instance wall.
    if corpse.map_id != looter.map_id {
        return Err("corpse on another map".to_string());
    }
    if corpse.instance_id != looter.instance_id {
        return Err("corpse in another instance".to_string());
    }
    let (dx, dy, dz) = (
        corpse.x - looter.x,
        corpse.y - looter.y,
        corpse.z - looter.z,
    );
    if dx * dx + dy * dy + dz * dz > LOOT_RANGE_SQ {
        return Err("corpse out of range".to_string());
    }

    let amount = corpse.money;
    corpse.money = 0;
    entities.guid().update(corpse);

    // Work-item 221: split across the KILL-TIME eligibility snapshot (`game_corpse_loot_eligible`,
    // work-item 187) instead of recomputing group membership/range at LOOT time — so money and item
    // eligibility can never drift apart (`apply_group_loot_rules` populates the very same rows). No
    // snapshot rows for this corpse means the kill never produced a >=2-recipient split
    // (`apply_group_loot_rules` early-outs below 2 recipients) — that IS the solo case: the whole
    // amount to the looter, byte-identical to the pre-221 path, and NO `MONEY_SHARE` event (the
    // looter's own client prints its local "You loot X copper" line; the gateway sends no notify).
    let recipients: Vec<u64> = ctx
        .db
        .game_corpse_loot_eligible()
        .by_corpse()
        .filter(&target_guid)
        .map(|e| e.eligible_guid)
        .collect();
    let grouped = !recipients.is_empty();
    let shares = split_money(amount, &recipients, looter_guid);

    // Credit the looter's own share in-place (avoid a stale-copy clobber: `looter` is updated ONCE,
    // after the loop, never re-fetched mid-function) and every other recipient's purse via
    // `credit_purse` (live entity if online, else the durable Character row — TRAP: a member who was
    // online at KILL time (the snapshot only ever contains callers of `kill_reward_recipients` who
    // were online then) may have logged off by LOOT time; vanilla still pays their share [V], so this
    // never silently drops their copper).
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
    entities.guid().update(looter);

    // Only a real GROUPED split pushes `MONEY_SHARE` — a solo credit above never reaches here with
    // `grouped == true`. Every recipient gets its own notify, including the looter (vanilla shows the
    // looter the same "Your share of the loot is X" line as the rest of the party when grouped).
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

    // Money may not be the last thing on the corpse — the flag follows the RULE, not this path.
    refresh_lootable(ctx, target_guid);
    Ok(())
}

/// Credit `share` copper to `recipient_guid`'s purse: the live `WorldEntity.money` if currently
/// online (relayed to their own connection as `PLAYER_FIELD_COINAGE`), else the durable
/// `Character.money` row directly (work-item 221 — an OFFLINE grouped recipient still gets paid;
/// `build_player_entity` loads `character.money` back into the entity at their next login). Saturating,
/// mirroring the looter's own transfer in [`apply_loot_money`]. No-op if the guid resolves to neither
/// (a deleted character mid-flight) — never panics on a stale snapshot row.
///
/// DEFER verdict for the in-transit case (issue #30). This is the one audited by-guid path a REFUSAL
/// gets wrong: the recipient is a PARTY MEMBER collecting their share of someone else's kill, so
/// dropping the write would silently short a third party who is not transferring and cannot know
/// why. `transfer::defer_money_delta` folds the copper into the escrowed export blob instead, so it
/// travels with the character. `begin_transfer` deleted the live entity, so the entity branch above
/// has already missed — the fold happens before the durable-row write, which stays as the
/// same-database payer of record (both partitions share the one `game_character` row).
fn credit_purse(ctx: &ReducerContext, recipient_guid: u64, share: u32) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(recipient_guid) {
        e.money = e.money.saturating_add(share);
        entities.guid().update(e);
        return;
    }
    crate::transfer::defer_money_delta(ctx, recipient_guid, share);
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(recipient_guid) {
        c.money = c.money.saturating_add(share);
        chars.guid().update(c);
    }
}

/// Split `total` copper evenly across `recipients` (the `game_corpse_loot_eligible` KILL-TIME
/// snapshot), remainder to `looter_guid` (cmangos convention [V] — no live client to confirm the
/// exact vanilla remainder rule, but this matches the reference emulator's `Group::GetLootMoneyShare`
/// odd-copper handling). `recipients` empty ⇒ solo/ungrouped: the whole amount to `looter_guid` in a
/// single entry — byte-identical to the pre-221 unconditional-credit path. If `looter_guid` is NOT
/// itself among `recipients` (possible only if the looter joined the group strictly after the kill was
/// snapshotted, or is looting a corpse whose kill credited someone else entirely — [V], unconfirmed
/// against a live client), the remainder is folded into an EXTRA entry for the looter so no copper is
/// ever silently dropped and opening the corpse always pays out at least the remainder. Pure.
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

/// Re-derive `UNIT_DYNFLAG_LOOTABLE` for a corpse from what is actually left on it:
/// lootable ⇔ any `game_corpse_loot` row remains OR `money > 0`. The ONE clear/set point both
/// loot paths call after consuming — item-only corpses go dark when the last row is taken, and
/// taking the money early no longer strips the cursor while items remain. Writes only on a real
/// change (no spurious on_update relay); a despawned/non-corpse guid is a no-op.
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
        entities.guid().update(corpse);
    }
}

// ===========================================================================================
//  GROUP LOOT METHODS (work-item 187 slices 1-4) — round-robin / need-greed rolls / master looter.
//
//  Timing decision (diverges from the work item's own draft, documented here): ALL group-loot
//  stamping — round-robin/master designation AND spawning need/greed rolls — happens at KILL TIME
//  (`apply_group_loot_rules`, called once from `combat::kill_creature` right after
//  `roll_creature_loot`), not lazily "at first loot-open" as the work item's flow draft describes.
//  Folding it into the kill path (already this slice's territory) avoids a NEW gateway reducer +
//  CMSG_LOOT dispatch change just to detect "first viewer" — and the observable result is
//  equivalent: eligible members get `SMSG_LOOT_START_ROLL` immediately after the kill instead of
//  exactly-when-someone-opens-the-corpse. Round-robin designation was ALREADY spec'd for
//  KILL-time creation in the work item's own design ("corpse gets `designated_looter_guid`
//  stamped AT CREATION"), so this only extends that timing to the roll-spawn half too.
//
//  Relay-pattern decision: rolls/master-list notifications reuse the EXISTING `game_group_event`
//  per-recipient relay (`crate::group::push_event`) instead of a new gateway-subscribed table — see
//  `lyracore_shared::loot_roll`'s module doc for the full rationale. The actual roll STATE
//  (`LootRoll`/`LootRollVote` below) is never read by the gateway for the CLIENT's sake.
//
//  **Where the roll DECISION lives (issue #50).** `LootRoll`/`LootRollVote` are authoritative on
//  REALM-CORE, alongside `game_group`/`game_group_member` (#22) — a roll's audience is who is in the
//  group, not where anyone stands, exactly like the membership it is snapshotted from. Kill-time
//  creation (`start_roll`, below) still runs on the corpse's own WORLD SHARD unconditionally — combat
//  resolution has no path to another database — so in a sharded deployment this write is a TRANSIENT
//  staging copy: the gateway's loot-roll relay (`gateway/src/world/loot.rs::relay_tick`) promotes it
//  onto realm-core (`realm_loot_op`'s `loot_op::START`, calling `insert_roll_rows` again there WITHOUT
//  re-pushing `ROLL_START` — the popup already fired locally) and clears the shard's copy
//  (`clear_promoted_loot_roll`). Voting then routes to realm-core too (`loot_op::VOTE` →
//  `cast_vote_on(Plane::RealmCore, ..)`), so `remove_member`'s disband branch — unchanged, still
//  `crate::loot::force_resolve_rolls_for_disband` — resolves a live roll in the SAME transaction as
//  the membership change, on the SAME database, with no mirror and no round trip: whichever database
//  `remove_member` executes on is also the one its `game_loot_roll` rows are the truth on. Unsharded,
//  none of this promotion/routing ever runs (`WorldStore::realm_store()` answers `None`), so the roll
//  lives and dies on one database exactly as it did before this issue — byte-identical.
//
//  **What did NOT move.** Only the roll — the group-scoped decision — travels. `game_corpse_loot`
//  (which corpse, which slot, item quality) and the actual item GRANT stay on the world shard with
//  their existing escrow guarantees (`items::grant_item`); a winner decided on realm-core is handed
//  their item by `settle_loot_roll`, an operator reducer the relay calls on the corpse's OWN shard
//  after observing the `ROLL_WON` event. `game_corpse_loot_eligible` (who is in range to roll) is
//  computed spatially at kill time and stays world-shard-local too — it is `apply_group_loot_rules`'s
//  own `recipients` snapshot, not part of the roll's realm-core state.
//
//  **The new case (issue #50).** A roll's PARTICIPANTS could not previously be on different shards —
//  `kill_reward_recipients` only ever names members who are physically near the corpse, i.e. on its
//  own shard, at the moment the roll starts. What #50 makes possible is a participant LEAVING that
//  shard mid-roll (a portal, a dungeon entry) during the 60s window. Pre-#50 that voter's row was
//  simply unreachable from wherever they went — their vote auto-passed at the deadline, same as being
//  offline. Post-#50 it is reachable: voting is realm-core state, and the gateway routes a vote
//  through whichever shard the player is CURRENTLY on, so a mid-roll shard-hopper can still vote from
//  their new location. No special-casing was needed for this — it falls out of routing votes to
//  realm-core rather than to "the shard that created the roll".
// ===========================================================================================

use crate::game_group; // Group row: loot_method/loot_threshold/rr_cursor/master_looter_guid
use crate::group::Plane;
use lyracore_shared::loot_roll::{event_kind as roll_event_kind, vote_kind};

/// A member of a kill's loot-eligible set, snapshotted at KILL TIME from `kill_reward_recipients`
/// (work-item 187 design: "Eligibility set = `kill_reward_recipients` at corpse creation,
/// SNAPSHOTTED into vote rows at roll start"). One row per `(corpse_guid, recipient)`. Private, no
/// RLS — only the module reads it (roll-vote snapshotting, `loot_master_give`'s recipient gate).
/// `eligible_guid` (not `character_guid`) deliberately dodges the crate's `character_owned` sweep-
/// marker tripwire (`lib.rs`): these rows are transient, corpse-lifetime data reaped alongside
/// `game_corpse_loot` on decay (`creatures::tick::pass_decay`) — never durable per-character state, so
/// they need no delete/restamp sweep on character delete/relog (a stale row just never matches any
/// live roll again). [entity]
#[table(accessor = game_corpse_loot_eligible, index(accessor = by_corpse, btree(columns = [corpse_guid])))]
pub struct CorpseLootEligible {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub corpse_guid: u64,
    pub eligible_guid: u64,
}

/// A live NEED/GREED/NBG roll on one `game_corpse_loot` row. Private — the gateway never reads this
/// (relayed via `game_group_event` instead, see the module doc above). `resolved` is the exactly-once
/// resolution guard (work-item 187 trap): set the moment `resolve_roll` decides an outcome, checked
/// FIRST on every entry point (the deadline sweep AND a landing vote can both reach the same roll).
/// [entity]
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
}

/// One eligible member's vote on a [`LootRoll`] — snapshotted (one row per eligible guid) the moment
/// the roll starts, so a member joining the group later never joins a live roll. `voted=false` rows
/// auto-pass at resolution (deadline OR all-voted). `rolled` is assigned THE MOMENT a NEED/GREED vote
/// lands (mirrors real vanilla: `SMSG_LOOT_ROLL`'s roll number is generated at vote time, not
/// deferred to resolution) — `0` for a pass (real or auto-) and never read for one. Private, no RLS
/// (the module resolves rolls server-side; the gateway never reads vote state directly). `voter_guid`
/// (not `character_guid`) dodges the `character_owned` tripwire — same transient-data rationale as
/// [`CorpseLootEligible`]. [entity]
#[table(accessor = game_loot_roll_vote, index(accessor = by_roll, btree(columns = [roll_id])))]
pub struct LootRollVote {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub roll_id: u64,
    pub voter_guid: u64,
    pub voted: bool,
    pub vote: u8,   // vote_kind::* — meaningful only if `voted`
    pub rolled: u8, // 1-100 once a NEED/GREED vote lands; 0 otherwise
}

/// Roll-window countdown (vanilla: 60s).
pub(crate) const ROLL_WINDOW_MICROS: i64 = 60_000_000;

/// Which handling applies to ONE `game_corpse_loot` row under the group's current `method`/
/// `threshold`, given the row's item `quality`? Pure — the single decision table every group-loot
/// stamping choice below reduces to; unit-tested as the "threshold split" matrix. An unrecognized
/// `method` (shouldn't happen — `group_loot_method` validates it) degrades to `Ffa` (never invents a
/// restriction from a bad value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupLootDecision {
    /// No restriction — behaves exactly as before this feature (FFA method, or a bad/unknown value).
    Ffa,
    /// Round-robin/below-threshold-GROUP: stamp the corpse's one pre-picked designee.
    Designate,
    /// Above-threshold under GROUP or any row under NEED_BEFORE_GREED: spawn a need/greed/pass roll.
    Roll,
    /// Above-threshold under MASTER: stamp the master + `master_only`.
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

/// Round-robin cursor advance (work-item 187 trap: "skipping offline members"): given a STABLE
/// member ordering's online bitmap and the group's current `cursor`, return the chosen member's
/// INDEX into that same ordering plus the cursor value to persist for NEXT time (wrapping). `None`
/// if nobody is online (a fully-offline group — degrades to FFA, never panics/blocks looting). Pure.
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

/// Given every ELIGIBLE (voted-or-auto-passed) member's vote kind, which tier contends for the item —
/// NEED beats GREED, an empty result means everyone passed (the row unlocks FFA-in-group)? Pure.
/// Returns the CONTENDING members' indices into `votes` (order-preserving).
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

/// Pick the winner among a contending tier's already-rolled 1-100 values: the unique max, or `None`
/// on a TIE (multiple share the max — cmangos re-rolls internally and re-decides; the caller loops
/// this with fresh rolls for the tied members until it resolves, then announces ONCE). Pure.
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
        None // a second index shares the max — a tie
    } else {
        Some(first)
    }
}

/// May `taker_guid` take a NON-quest `game_corpse_loot` row via the plain autostore path, given its
/// `withheld` bit (a live NEED/GREED roll owns the row), `reserved_for` (generalized — see the
/// struct doc: nonzero now ALSO covers a NEED/GREED winner's inventory-full fallback row),
/// `master_only`, and `designated_looter_guid`? Quest rows are gated separately by
/// [`quest_take_allowed`] (unchanged) — `apply_take_loot` calls this one only when
/// `!row.quest_only`. `withheld` is checked FIRST and unconditionally: the per-viewer display
/// filter hides these rows, but display filtering is NOT enforcement — without this gate a crafted
/// `take_loot` on the hidden slot steals the item mid-roll, and the eventual `resolve_roll` grant
/// silently no-ops while ROLL_WON still fires (187 review finding #1). Pure.
pub(crate) fn group_loot_take_allowed(
    withheld: bool,
    reserved_for: u64,
    master_only: bool,
    designated_looter_guid: u64,
    taker_guid: u64,
) -> bool {
    if withheld {
        return false; // a live roll owns this row — nobody autostores it; the winner gets reserved_for
    }
    if reserved_for != 0 {
        return reserved_for == taker_guid;
    }
    if master_only {
        return false; // master-loot rows are taken ONLY via `loot_master_give`, never autostore
    }
    designated_looter_guid == 0 || designated_looter_guid == taker_guid
}

/// KILL-TIME group-loot stamping (module doc above explains the timing choice): called once from
/// `combat::kill_creature` right after `roll_creature_loot`, for a GROUPED kill only (an ungrouped/
/// solo `killer_guid` — `group_of` returns `None` — leaves every row exactly as `roll_creature_loot`
/// produced it: the pre-187 FFA baseline, byte-identical). `recipients` is the SAME
/// `kill_reward_recipients` slice the XP/quest-credit split already computed — reused, never
/// recomputed, so eligibility can't drift between the XP split and the loot roll.
///
/// Work-item 187 trap ("solo player with method GROUP set: threshold rows must NOT roll"): a solo
/// `recipients` (`len() < 2`, e.g. every other member out of XP range or dead) skips ALL group-loot
/// handling too — vanilla's "party size 1 -> direct loot" applies to loot exactly like it does to
/// the XP split, so the same recipient-count gate covers both.
pub(crate) fn apply_group_loot_rules(
    ctx: &ReducerContext,
    corpse_guid: u64,
    killer_guid: u64,
    recipients: &[u64],
) {
    let Some(m) = crate::group::group_of(ctx, killer_guid) else {
        return;
    };
    if recipients.len() < 2 {
        return;
    }
    let Some(mut group) = ctx.db.game_group().group_id().find(m.group_id) else {
        return;
    };

    // Snapshot eligibility BEFORE the FFA early-return (221 review catch): vanilla splits CORPSE
    // COIN across the group under EVERY loot method (FFA only affects items), and
    // `apply_loot_money` reads this snapshot as its recipient set — snapshotting only for non-FFA
    // methods would silently turn an FFA party's money loot into a solo whole-amount grab. The
    // rows also feed roll-vote membership + `loot_master_give`'s recipient gate for the methods
    // that use them; an FFA kill just never reads them for items.
    let eligible = ctx.db.game_corpse_loot_eligible();
    for &guid in recipients {
        eligible.insert(CorpseLootEligible {
            id: 0,
            corpse_guid,
            eligible_guid: guid,
        });
    }

    if group.loot_method == crate::group::loot_method::FFA {
        return;
    }

    // One STABLE member ordering (sorted by character_guid — deterministic regardless of join
    // order) so the rr_cursor index means the same thing kill after kill.
    let mut members: Vec<u64> = crate::group::members_of(ctx, m.group_id)
        .into_iter()
        .map(|gm| gm.character_guid)
        .collect();
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
    let master_guid = if group.master_looter_guid != 0 {
        group.master_looter_guid
    } else {
        group.leader_guid
    };

    let templates = ctx.db.game_item_template();
    let rows: Vec<CorpseLoot> = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&corpse_guid)
        .collect();
    let mut master_rows: Vec<u8> = Vec::new(); // slots stamped master_only, for the MASTER_LIST push
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    for mut row in rows {
        if row.quest_only {
            continue; // orthogonal systems — never group-gate a quest drop (module doc invariant)
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
                start_roll(ctx, corpse_guid, row.slot, row.item_entry, recipients, now);
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
            lyracore_shared::loot_roll::encode_master_list(corpse_guid, recipients),
        );
    }
}

/// Insert one [`LootRoll`] row + one [`LootRollVote`] per recipient (`voted=false`), with NO event
/// push. The shared core of [`start_roll`] (kill time — this database keeps the roll unsharded, or
/// stages it for promotion sharded) and `realm_loot_op`'s `loot_op::START` arm (issue #50 — this
/// database IS realm-core, RE-CREATING a roll a world shard already announced locally; pushing
/// `ROLL_START` again here would double the popup). Returns the new roll's id.
fn insert_roll_rows(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    recipients: &[u64],
    deadline_micros: i64,
) -> u64 {
    let roll = ctx.db.game_loot_roll().insert(LootRoll {
        id: 0,
        corpse_guid,
        slot,
        item_entry,
        deadline_micros,
        resolved: false,
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

/// Spawn a NEED/GREED/NBG roll at KILL TIME: [`insert_roll_rows`] + `ROLL_START` to every recipient.
///
/// Issue #50: this ALWAYS runs on the corpse's own WORLD SHARD — kill-time combat resolution has no
/// path to another database — so in a SHARDED deployment the row it creates is a TRANSIENT staging
/// copy: the gateway's loot-roll relay (`gateway/src/world/loot.rs::relay_tick`) promotes it onto
/// realm-core and clears this copy (`clear_promoted_loot_roll`) so voting/resolution never runs
/// twice. Unsharded, nothing promotes or clears it — it just IS the roll, byte-identical to before
/// this issue.
fn start_roll(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    recipients: &[u64],
    now_micros: i64,
) {
    insert_roll_rows(
        ctx,
        corpse_guid,
        slot,
        item_entry,
        recipients,
        now_micros + ROLL_WINDOW_MICROS,
    );
    let payload = lyracore_shared::loot_roll::encode_start(
        corpse_guid,
        slot,
        item_entry,
        (ROLL_WINDOW_MICROS / 1000) as u32,
    );
    for &guid in recipients {
        crate::group::push_event(ctx, guid, roll_event_kind::ROLL_START, 0, payload.clone());
    }
}

/// `CMSG_LOOT_ROLL`: record the caller's vote on the roll open for `(corpse_guid, loot_slot)`. A
/// NEED/GREED vote rolls 1-100 IMMEDIATELY (mirrors real vanilla — `SMSG_LOOT_ROLL`'s number is
/// generated at vote time, not deferred; see `LootRollVote` doc). Pushes `ROLL_VOTE` to every
/// eligible member, then resolves NOW if this was the last outstanding vote (deadline-driven
/// resolution is the separate `sweep_loot_rolls` path for an AFK/disconnected voter). Idempotent
/// guard: a second vote from the same guid on the same roll is rejected (no re-vote/flip-flop).
#[reducer]
pub fn loot_roll(
    ctx: &ReducerContext,
    corpse_guid: u64,
    loot_slot: u32,
    vote: u8,
) -> Result<(), String> {
    let voter = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "not in world".to_string())?;
    cast_vote_on(
        ctx,
        Plane::Shard,
        corpse_guid,
        loot_slot as u8,
        voter.guid,
        vote,
    )
}

/// The identity-free vote core (issue #50, mirrors `group.rs`'s `*_on` cores): shared by the player
/// reducer above (`Plane::Shard`, the world shard the voter is physically standing on) and
/// `realm_loot_op`'s VOTE arm (`Plane::RealmCore`, the plane the roll is authoritative on once
/// promoted). Same gates, same resolution, in the same order — `loot_roll` is byte-identical for
/// clients. `plane` is accepted for symmetry with `group.rs`'s cores and not read: unlike a group op,
/// nothing about a vote's RULES differs by plane, only which database's `game_loot_roll` `ctx.db`
/// resolves to, which the caller already picked by calling this on the right connection.
fn cast_vote_on(
    ctx: &ReducerContext,
    _plane: Plane,
    corpse_guid: u64,
    slot: u8,
    voter_guid: u64,
    vote: u8,
) -> Result<(), String> {
    if !matches!(vote, vote_kind::PASS | vote_kind::NEED | vote_kind::GREED) {
        return Err("invalid vote".to_string());
    }
    let roll = ctx
        .db
        .game_loot_roll()
        .by_corpse()
        .filter(&corpse_guid)
        .find(|r| r.slot == slot && !r.resolved)
        .ok_or_else(|| "no roll open on that item".to_string())?;
    let votes = ctx.db.game_loot_roll_vote();
    let mut my_vote = votes
        .by_roll()
        .filter(&roll.id)
        .find(|v| v.voter_guid == voter_guid)
        .ok_or_else(|| "not eligible for this roll".to_string())?;
    if my_vote.voted {
        return Err("already voted".to_string());
    }
    my_vote.voted = true;
    my_vote.vote = vote;
    my_vote.rolled = if vote == vote_kind::PASS {
        0
    } else {
        (ctx.random::<u32>() % 100 + 1) as u8
    };
    let my_rolled = my_vote.rolled; // captured before the move (LootRollVote has no Clone)
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

/// Grant a resolved roll's item to its winner, on WHICHEVER database holds the corpse (issue #50).
///
/// Called inline by [`resolve_roll`] / [`force_resolve_rolls_for_disband`], wherever they execute —
/// a single database, or REALM-CORE in a sharded one — and by the [`settle_loot_roll`] operator
/// reducer, which the gateway's loot-roll relay calls on the corpse's OWN world shard after observing
/// the `ROLL_WON` event realm-core just pushed. All three call sites are safe to call
/// unconditionally: there is nothing to settle where no row matches at all (REALM-CORE never has
/// `game_corpse_loot` rows — no kills happen there — so the two inline callers no-op for free there),
/// and nothing to settle a second time (the row is gone after the first successful grant).
///
/// `withheld` is checked FIRST — the roll's own lock on the row, the same gate
/// [`group_loot_take_allowed`] uses — so a `(corpse_guid, slot)` pair that merely COINCIDES with an
/// unrelated row on another shard (`corpse_guid` is a per-shard `game_world_entity` sequence, not
/// realm-unique — the tail risk this issue's cross-shard-routing note accepts, same class as
/// character-name collisions elsewhere in this codebase) can never be mistaken for the row that just
/// resolved: a row that is not mid-roll is never touched by this function, however its numbers land.
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
        // #99 AC#4 sweep. This return is the one that ATE A WINNER'S ITEM (#50's review): a slow
        // voter had `CORPSE_DECAY_MICROS == ROLL_WINDOW_MICROS` reap the corpse out from under the
        // roll, settlement found no row, and it returned with no grant, no error and no log.
        //
        // It cannot be an error, because the gateway fans `settle_loot_roll` out to every shard and
        // "no row here" is the normal answer on all but one. So it is logged at INFO and worded so
        // the two cases are TELLABLE APART BY CONTEXT — exactly one shard should have held this
        // corpse, so if NO shard logs a grant for a settled roll, this line is where the item went.
        spacetimedb::log::info!(
            "settle_roll_grant: corpse {corpse_guid} slot {slot} — no loot row on this database \
             (expected on every shard but the corpse's owner; if NO shard granted, the corpse was \
             reaped before the roll settled and the winner got nothing)"
        );
        return;
    };
    if !row.withheld {
        // Not mid-roll: this row belongs to some other corpse/slot pairing on this shard, or the
        // roll already settled here. Distinct from the branch above — the row EXISTS.
        spacetimedb::log::info!(
            "settle_roll_grant: corpse {corpse_guid} slot {slot} — row is not withheld, so no roll \
             is open on it here; nothing granted"
        );
        return; // not a live roll's row on THIS database — see the doc above
    }
    match crate::items::grant_item(ctx, winner_guid, row.item_entry, row.count.max(1)) {
        Ok(()) => {
            loot.id().delete(row.id);
            refresh_lootable(ctx, corpse_guid);
        }
        Err(_) => {
            // Inventory-full fallback: leave the row, exclusively reserved for the winner.
            row.withheld = false;
            row.reserved_for = winner_guid;
            row.designated_looter_guid = 0;
            row.master_only = false;
            loot.id().update(row);
        }
    }
}

/// Resolve exactly once (the `resolved` flag guard — work-item 187 trap): any NEED beats any GREED;
/// a TIE within the winning tier re-rolls JUST the tied members internally (bounded — 1-100 rolls
/// collide vanishingly rarely, `MAX_TIE_REROLLS` is generous headroom) and announces once; all-pass
/// unlocks the row FFA-in-group (no `ROLL_WON` — vanilla shows no "won" line either). The winner is
/// granted via `items::grant_item`; on `Err` (inventory full) the row is LEFT, `reserved_for` stamped
/// to the winner (documented winner-locked fallback — 068's mail delivery is the eventual fix).
/// `votes`/`recipients` are the FULL snapshot (already auto-passed by the caller where needed).
fn resolve_roll(ctx: &ReducerContext, roll: &LootRoll, votes: &[LootRollVote], recipients: &[u64]) {
    if roll.resolved {
        return; // exactly-once guard
    }
    ctx.db.game_loot_roll().id().update(LootRoll {
        id: roll.id,
        corpse_guid: roll.corpse_guid,
        slot: roll.slot,
        item_entry: roll.item_entry,
        deadline_micros: roll.deadline_micros,
        resolved: true,
    }); // no Clone on LootRoll — rebuilt field-by-field (all Copy) instead

    let kinds: Vec<u8> = votes.iter().map(|v| v.vote).collect();
    let tier = contending_tier(&kinds);
    if tier.is_empty() {
        unlock_row_ffa(ctx, roll.corpse_guid, roll.slot);
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
        // Astronomically unlikely (20 straight all-tied re-rolls); fail safe to the first tied
        // member rather than looping forever or panicking.
        unlock_row_ffa(ctx, roll.corpse_guid, roll.slot);
        cleanup_roll(ctx, roll.id);
        return;
    };
    let winning_roll = rolls[w];
    let winner_guid = votes[tier[w]].voter_guid;
    let winning_vote = votes[tier[w]].vote; // NEED or GREED — the tier that actually won (finding #3)

    // Grant, then finalize the CorpseLoot row per the outcome (module doc: 068 mail is deferred).
    // Issue #50: on a single database this runs inline, right here, exactly as before. On REALM-CORE
    // (sharded) `game_corpse_loot` is always empty — no kills happen there — so this no-ops for free,
    // and the ACTUAL grant happens on the corpse's own world shard via `settle_loot_roll`, which the
    // gateway's loot-roll relay calls after observing the `ROLL_WON` event pushed below.
    settle_roll_grant(ctx, roll.corpse_guid, roll.slot, winner_guid);
    let payload = lyracore_shared::loot_roll::encode_won(
        roll.corpse_guid,
        roll.slot,
        roll.item_entry,
        winning_roll,
        winning_vote,
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

/// All-pass unlock: clear `withheld` so the row becomes an ordinary FFA-in-group row (any current
/// group member may loot it via the plain path — `designated_looter_guid`/`master_only` stay at
/// their defaults, so there is no further restriction).
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

/// Delete a resolved roll's vote rows + the roll row itself — nothing left to sweep/orphan.
fn cleanup_roll(ctx: &ReducerContext, roll_id: u64) {
    let votes = ctx.db.game_loot_roll_vote();
    let stale: Vec<u64> = votes.by_roll().filter(&roll_id).map(|v| v.id).collect();
    for id in stale {
        votes.id().delete(id);
    }
    ctx.db.game_loot_roll().id().delete(roll_id);
}

/// Deadline sweep (work-item 187 trap: "disconnected member mid-roll: their vote auto-passes at
/// deadline"): scans every UNRESOLVED roll whose deadline has elapsed, auto-passes any still-`!voted`
/// member (in place — never blocks), then resolves. `pub(crate)` — exposed for the orchestrator's
/// scheduled-GC tick (`gc.rs`, NOT edited here per instructions): wire one call
/// `crate::loot::sweep_loot_rolls(ctx);` into `reap_movement_events` (it already runs every
/// `EVENT_TTL_MICROS` = 1s, ample granularity for a 60s roll window).
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
                // No Clone on LootRollVote — rebuild the update value field-by-field (all Copy).
                votes_tab.id().update(LootRollVote {
                    id: v.id,
                    roll_id: v.roll_id,
                    voter_guid: v.voter_guid,
                    voted: v.voted,
                    vote: v.vote,
                    rolled: v.rolled,
                });
            }
        }
        let recipients: Vec<u64> = all.iter().map(|v| v.voter_guid).collect();
        resolve_roll(ctx, &roll, &all, &recipients);
    }
}

/// Work-item 187 trap ("disband mid-roll -> resolve to sole member"): called from
/// `group::remove_member`'s full-disband branch with the FULL former membership (`member_guids`,
/// leaver included) and the sole survivor if exactly one remains. Force-resolves every UNRESOLVED
/// roll whose ENTIRE voter set belonged to the disbanding group: with a sole survivor, grants them
/// the item directly (bypassing need/greed entirely — vanilla's behavior); with none (or more than
/// one — shouldn't happen, disband implies <2 remain), just unlocks FFA (nothing left to award to,
/// or the caller mis-invoked with a still-live group).
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
            continue; // not this group's roll
        }
        ctx.db.game_loot_roll().id().update(LootRoll {
            id: roll.id,
            corpse_guid: roll.corpse_guid,
            slot: roll.slot,
            item_entry: roll.item_entry,
            deadline_micros: roll.deadline_micros,
            resolved: true,
        }); // no Clone on LootRoll — rebuilt field-by-field (all Copy) instead
        match survivor.filter(|s| all.iter().any(|v| v.voter_guid == *s)) {
            Some(winner_guid) => {
                // Issue #50: inline on a single database (unchanged), a free no-op on REALM-CORE
                // (no local `game_corpse_loot` row there) — the real grant then settles onto the
                // corpse's own world shard via `settle_loot_roll`, driven by the gateway's loot-roll
                // relay off the `ROLL_WON` event pushed below. See `settle_roll_grant`'s own doc.
                settle_roll_grant(ctx, roll.corpse_guid, roll.slot, winner_guid);
                // Forced disband resolution: no contest happened — label it NEED (the sole
                // survivor takes by default; there is no honest greed/need distinction here).
                let payload = lyracore_shared::loot_roll::encode_won(
                    roll.corpse_guid,
                    roll.slot,
                    roll.item_entry,
                    0,
                    lyracore_shared::loot_roll::vote_kind::NEED,
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

// ===========================================================================================
//  REALM-CORE plane (issue #50) — mirrors `group.rs`'s realm-core section for the roll itself.
// ===========================================================================================

// `#[reducer]`: SpacetimeDB reducers take their arguments FLAT off the wire, so a parameter struct is not available.
#[allow(clippy::too_many_arguments)]
/// The realm-wide loot-roll ops (issue #50), as ONE operator-gated reducer keyed by
/// [`lyracore_shared::loot_roll::loot_op`] — the same one-reducer-not-several trade `realm_group_op`
/// made (`lyracore_shared::group::realm_op`'s doc), for the same reason: every gateway-callable reducer
/// needs a hand-maintained SDK binding (`docs/danger-zones.md` §1.2).
///
/// **Operator-gated, and it has to be**: both ops act on realm-core, which has no live entity to
/// derive an actor from. START's `recipients` are a snapshot the WORLD SHARD already computed
/// spatially at kill time (`kill_reward_recipients`); VOTE's `actor_guid` is the guid the gateway
/// authenticated for the calling socket (`InWorld::self_guid`), never a client-supplied literal.
#[reducer]
pub fn realm_loot_op(
    ctx: &ReducerContext,
    op: u8,
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    actor_guid: u64,
    vote: u8,
    deadline_micros: i64,
    recipients: Vec<u64>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    use lyracore_shared::loot_roll::loot_op;
    match op {
        // The gateway's loot-roll relay PROMOTING a roll a world shard already announced locally
        // (`gateway/src/world/loot.rs::relay_tick`) — `insert_roll_rows`, never `start_roll`: the
        // `ROLL_START` popup already fired on the shard that created it, so this must not re-push it.
        loot_op::START => {
            insert_roll_rows(
                ctx,
                corpse_guid,
                slot,
                item_entry,
                &recipients,
                deadline_micros,
            );
            Ok(())
        }
        loot_op::VOTE => cast_vote_on(ctx, Plane::RealmCore, corpse_guid, slot, actor_guid, vote),
        other => Err(format!("unknown realm loot op {other}")),
    }
}

/// Grant a resolved roll's item on the WORLD SHARD that actually holds the corpse (issue #50). The
/// gateway's loot-roll relay calls this on every connected world shard after observing realm-core's
/// `ROLL_WON` event — [`settle_roll_grant`]'s own guards make every wrong-shard call a harmless
/// no-op, so the relay never needs to know in advance WHICH shard holds the corpse.
#[reducer]
pub fn settle_loot_roll(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    winner_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    settle_roll_grant(ctx, corpse_guid, slot, winner_guid);
    Ok(())
}

/// Delete a STAGING roll's rows on the WORLD SHARD that created them, once the gateway's loot-roll
/// relay has promoted the roll onto realm-core (issue #50) — [`cleanup_roll`], the same delete a
/// resolved roll already used, called here for an UNRESOLVED one that just changed which database is
/// authoritative for it.
#[reducer]
pub fn clear_promoted_loot_roll(ctx: &ReducerContext, roll_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    cleanup_roll(ctx, roll_id);
    Ok(())
}

/// `CMSG_LOOT_MASTER_GIVE`: the group's master looter assigns a `master_only` row to `target_guid`
/// (any currently-eligible recipient of that corpse's kill — `game_corpse_loot_eligible`, not
/// necessarily the group's CURRENT roster, matching the roll-vote snapshot's semantics). Validates
/// the caller IS the row's stamped master (`designated_looter_guid`) and the row really is
/// `master_only`. Grants directly; `Err` (inventory full) rolls the whole call back (nothing
/// consumed) — the master can retry once the recipient frees space, matching every other
/// `grant_item`-gated reducer's rollback shape in this codebase.
#[reducer]
pub fn loot_master_give(
    ctx: &ReducerContext,
    corpse_guid: u64,
    loot_slot: u8,
    target_guid: u64,
) -> Result<(), String> {
    let master = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "not in world".to_string())?;
    let loot = ctx.db.game_corpse_loot();
    let row = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == loot_slot)
        .ok_or_else(|| "no loot in that slot".to_string())?;
    if !row.master_only || row.designated_looter_guid != master.guid {
        return Err("you are not the master looter for that item".to_string());
    }
    let is_eligible = ctx
        .db
        .game_corpse_loot_eligible()
        .by_corpse()
        .filter(&corpse_guid)
        .any(|e| e.eligible_guid == target_guid);
    if !is_eligible {
        return Err("that player is not eligible for this loot".to_string());
    }
    crate::items::grant_item(ctx, target_guid, row.item_entry, row.count.max(1))?;
    loot.id().delete(row.id);
    refresh_lootable(ctx, corpse_guid);
    Ok(())
}

// ===========================================================================================
//  SKINNING (professions slice 2) — a skill-gated corpse interaction, modeled on `loot_money`
// ===========================================================================================

/// The leather a skin yields — the REAL vanilla "Light Leather" `game_item_template` TRADE_GOODS entry
/// (2318); the slice mints a flat 1× per skin (a `game_skinning_loot`
/// table keyed on skill→tier is the DEFERRED real model — noted like the `RECIPES` map's
/// single-reagent-per-recipe shortcut).
pub(crate) const LEATHER_ENTRY: u32 = 2318; // "Light Leather" (real imported item)
/// Flat leather count per skin for the slice (DEFERRED: a skill→count table).
pub(crate) const LEATHER_COUNT: u32 = 1;

// Pure predicate over every gate a skinning attempt must pass, argument-shaped so it is unit-testable without a `ReducerContext`.
#[allow(clippy::too_many_arguments)]
/// The pure SKIN gate: may a `corpse` be skinned by a looter on `same_map`, in range? Decides on the
/// same primitives `loot_money` checks plus the beast/already-skinned markers — factored out (like
/// `loot_drops`/`group_pick`) so the gate is unit-testable without a live `ReducerContext`. Returns the
/// reject reason, or `Ok(())` to proceed. (Distance is the already-computed squared value vs `LOOT_RANGE_SQ`.)
///
/// `learned_skinning` is the character's current Skinning skill value (0 = not learned — no skill row),
/// mirroring `gameobject::can_gather`. `creature_level` is the corpse's level. Vanilla requires
/// Skinning trained AND skill >= (creature_level - 1) * 10, so level-1 beasts are always skinnable
/// with Apprentice Skinning (any skill ≥ 1).
pub(crate) fn can_skin(
    looter_dead: bool,
    corpse_is_player: bool,
    corpse_dead: bool,
    same_map: bool,
    dist_sq: f32,
    is_beast: bool,
    already_skinned: bool,
    learned_skinning: u32,
    creature_level: u32,
) -> Result<(), String> {
    if looter_dead {
        return Err("dead players cannot skin".to_string());
    }
    if corpse_is_player {
        return Err("cannot skin a player".to_string());
    }
    if !corpse_dead {
        return Err("target is not a corpse".to_string());
    }
    if !same_map {
        return Err("corpse on another map".to_string());
    }
    if dist_sq > LOOT_RANGE_SQ {
        return Err("corpse out of range".to_string());
    }
    if !is_beast {
        return Err("not a beast — cannot be skinned".to_string());
    }
    if already_skinned {
        return Err("already skinned".to_string());
    }
    // Skill gate: must have trained Skinning AND skill >= (creature_level - 1) * 10.
    // Level-1 beasts require 0 skill so a freshly-trained skinner (skill=1) can immediately skin
    // them, matching vanilla Apprentice Skinning.  Level-2 beasts require 10, level-3 require 20,
    // etc.  saturating_sub guards against a hypothetical level-0 creature (maps to 0 required).
    if learned_skinning == 0 {
        return Err("you have not learned Skinning".to_string());
    }
    let required = creature_level.saturating_sub(1) * 10;
    if learned_skinning < required {
        return Err(format!(
            "your Skinning skill ({learned_skinning}) is too low — need {required}"
        ));
    }
    Ok(())
}

/// Is `entry`'s creature a BEAST (creature_type == 1)? The skinnable gate — reads the imported
/// `game_creature_template.creature_type` (loaded verbatim by the importer). A missing template (no row)
/// is not a beast (false) — a corpse with no template can't be skinned, never panics.
pub(crate) fn entry_is_beast(ctx: &ReducerContext, entry: u32) -> bool {
    ctx.db
        .game_creature_template()
        .entry()
        .find(entry)
        .is_some_and(|t| t.creature_type == crate::spell::BEAST_TYPE)
}

/// SKIN a beast corpse: the core (resolved guids), modeled on `loot_money`. Requires the player to have
/// TRAINED Skinning at a trainer AND have skill >= corpse_level * 10 (vanilla floor), mirroring the
/// `can_gather` gate. Grants leather, climbs the Skinning line +1, and marks the corpse skinned so it
/// can't be re-skinned. Shared by the debug lever (`debug_skin_nearest`) and a future `CMSG`-routed
/// skin over the open corpse. Returns the reject reason on any gate failure, or `Ok` after the grant.
///
/// ROLLBACK: the leather grant (`grant_item`) is `?`-propagated — a full bag returns `Err` BEFORE the
/// `skinned` marker is set, so the reducer tx rolls back and the corpse stays skinnable (no leather is
/// minted on a full bag, and the player can retry), exactly like the COOKING reagent gate.
pub(crate) fn skin_corpse(
    ctx: &ReducerContext,
    looter_guid: u64,
    corpse_guid: u64,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let looter = entities
        .guid()
        .find(looter_guid)
        .ok_or_else(|| "skinner not in world".to_string())?;
    let mut corpse = entities
        .guid()
        .find(corpse_guid)
        .ok_or_else(|| "no such corpse".to_string())?;

    // All gates (the pure decision shared with the unit test): looter alive, a dead non-player BEAST
    // corpse, same map, in range, not already skinned, trained Skinning + skill >= level*10.
    let dist_sq = (corpse.x - looter.x).powi(2)
        + (corpse.y - looter.y).powi(2)
        + (corpse.z - looter.z).powi(2);
    // Learned Skinning skill (0 = not trained — no skill row), mirroring the `can_gather` gather gate.
    let learned_skinning = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(&looter_guid)
        .find(|s| s.skill_line == crate::skill::skill_line::SKINNING)
        .map(|s| s.current as u32)
        .unwrap_or(0);
    can_skin(
        looter.dead,
        corpse.is_player(),
        corpse.dead,
        // Map + instance (190 slice 2): `can_skin`'s `same_map` conjunct now carries the
        // instance-equality clause too — the corpse is a `game_world_entity` row, slice-1-tagged.
        corpse.map_id == looter.map_id && corpse.instance_id == looter.instance_id,
        dist_sq,
        entry_is_beast(ctx, corpse.entry),
        corpse.skinned,
        learned_skinning,
        corpse.level,
    )?;

    // Grant leather — DATA-DRIVEN (work-item 210): the corpse's creature template names a
    // `skin_loot_id` (cmangos `skinning_loot_template`, level-banded across many creatures sharing a
    // band); roll it and grant every winner. `skin_loot_id == 0` (unimported / a seeded/test beast) OR
    // an empty/no-win roll falls back to the flat `LEATHER_ENTRY`/`LEATHER_COUNT` — byte-identical to
    // the pre-210 alpha, so a skin never comes up empty-handed. `?`-rollback: ANY winner's `grant_item`
    // failing (full bag) rolls back the WHOLE skin BEFORE the corpse is marked skinned (the reducer tx
    // rolls the whole call back), so no partial skin — retry after freeing space, exactly the COOKING
    // reagent-gate rollback shape.
    let skin_loot_id = ctx
        .db
        .game_creature_template()
        .entry()
        .find(corpse.entry)
        .map(|t| t.skin_loot_id)
        .unwrap_or(0);
    let rows: Vec<(u32, u32, u32, u32)> = if skin_loot_id != 0 {
        ctx.db
            .game_skinning_loot()
            .by_skin()
            .filter(&skin_loot_id)
            .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id))
            .collect()
    } else {
        Vec::new()
    };
    let winners = roll_loot_rows(ctx, rows);
    if winners.is_empty() {
        crate::items::grant_item(ctx, looter_guid, LEATHER_ENTRY, LEATHER_COUNT)?;
    } else {
        for (item_entry, count) in winners {
            crate::items::grant_item(ctx, looter_guid, item_entry, count)?;
        }
    }

    // Skill-up — the COOKING skill-up hook reused verbatim (no-op if not learned / at the cap;
    // the gate above already confirmed learned_skinning > 0, so the row exists). The DEFAULT sentinel band
    // (orange=1, gray=0 ⇒ always +1) keeps skinning byte-identical to the pre-difficulty slice — a real
    // Skinning difficulty table (gray>0) is DEFERRED.
    crate::skill::gain_profession_skill(ctx, looter_guid, crate::skill::skill_line::SKINNING, 1, 0);

    // Mark skinned so a second skin is rejected (the gate above). Reset for free on respawn
    // (`build_creature_entity`). The corpse decays normally.
    corpse.skinned = true;
    entities.guid().update(corpse);
    Ok(())
}

// ===========================================================================================
//  FISHING (completing the 13) — a bounded new gather mechanic: an immediate-catch reducer modeled on
//  `skin_corpse` (auto-learn → grant a fish → climb the line). No bobber/cast/timer (DEFERRED), and a
//  LENIENT water gate (alpha: just alive) — the real near-water LiquidType check is a follow-up. [entity]
// ===========================================================================================

/// The small-fish FALLBACK pool — REAL low-level cmangos fish `game_item_template` entries (standard
/// trade-goods present in the imported box). Used when a cast's zone has NO `game_fishing_loot` rows
/// (unimported, or off-slice) — since work-item 210 the real per-zone table (`game_fishing_loot`) is
/// tried FIRST; this is the "never comes up empty-handed" floor beneath it (mirrors the skinning
/// `LEATHER_ENTRY` fallback). If a fish entry isn't in the imported `item_template`, `grant_item`
/// returns Err and the whole `fish` rolls back (no half-catch) — so verify these three are in-box.
pub(crate) const FISH_POOL: &[u32] = &[
    6291, // Raw Brilliant Smallfish
    6303, // Raw Slitherskin Mackerel
    6289, // Raw Longjaw Mud Snapper
];

/// Pure one-hop parent walk (work-item 209 idiom, duplicated from `world::graveyard::resolve_zone_id`
/// since that helper is private to `world.rs`'s `graveyard` submodule): given an `AreaTable.dbc` row's
/// own id and its `parent_area_id`, return the enclosing ZONE id — the parent when the area IS a
/// subzone, else the area's own id (already top-level). Split out so the walk itself is unit-testable
/// without a live `ReducerContext` (mirrors `pick_graveyard`'s pure/ctx split).
pub(crate) fn zone_of_area(area_id: u32, parent_area_id: u32) -> u32 {
    if parent_area_id != 0 {
        parent_area_id
    } else {
        area_id
    }
}

/// Resolve the FISHING zone id at `(map_id, x, y)`: the MCNK area id (work-item 173) walked one hop up
/// to its enclosing zone via `zone_of_area`. `None` when `game_area` is empty (unimported) or the
/// position has no recorded/imported area — `apply_fish` falls back to the flat `FISH_POOL` rather than
/// guessing a zone (never a wrong-zone catch).
fn fishing_zone_id(ctx: &ReducerContext, map_id: u32, x: f32, y: f32) -> Option<u32> {
    if ctx.db.game_area().count() == 0 {
        return None;
    }
    let area_id = crate::terrain::area_id_at(ctx, map_id, x, y)?;
    let area = ctx.db.game_area().id().find(area_id)?;
    Some(zone_of_area(area.id, area.parent_area_id))
}

/// Pure choice of the CAUGHT fish: the zone table's `winner` if the roll produced one, else a
/// deterministic `FISH_POOL` pick via `pool_idx` (the caller passes an already-reduced
/// `ctx.random::<usize>()`, so this stays a pure index lookup). Covers BOTH fallback triggers — no zone
/// rows at all, or a zone table that rolled nothing this cast — with the SAME floor, so a cast never
/// comes up empty-handed. Split out so the fallback decision is unit-testable.
pub(crate) fn pick_caught_fish(winner: Option<u32>, pool_idx: usize) -> u32 {
    winner.unwrap_or_else(|| FISH_POOL[pool_idx % FISH_POOL.len()])
}

/// FISH at the player's current spot: the core (resolved guid), modeled on `skin_corpse`. LENIENT alpha
/// gate (alive only — the near-water LiquidType check is DEFERRED). Auto-learns Fishing on the first cast
/// (idempotent, born 1/75 like skinning), resolves the caster's ZONE (`fishing_zone_id`) and rolls
/// `game_fishing_loot` for it — falling back to the flat `FISH_POOL` when the zone is unresolved, has no
/// rows, or the roll lands on nothing (`pick_caught_fish`) — grants the caught fish, climbs the Fishing
/// line +1 (the default sentinel band ⇒ deterministic +1). Shared by the `fish` player reducer + the
/// `debug_fish` twin. ROLLBACK: the `grant_item` is `?`-propagated — a full bag returns Err BEFORE the
/// skill-up commits (the tx rolls back), so no fish is minted on a full bag and the skill doesn't climb.
pub(crate) fn apply_fish(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .ok_or_else(|| "fisher not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot fish".to_string());
    }
    // AUTO-LEARN Fishing on the first cast (idempotent — a re-cast never resets a climbed value), so the
    // skill-up hook has a row to climb. The very first cast is born 1/75; this cast's +1 takes it to 2/75.
    crate::skill::learn_profession(
        ctx,
        guid,
        player.owner_identity,
        crate::skill::skill_line::FISHING,
        crate::skill::APPRENTICE_CAP,
    );
    // Zone-keyed catch (work-item 210): resolve the caster's zone and roll its real table; an
    // unresolved zone (unimported terrain/AreaTable, or off-slice) rolls an empty Vec, which
    // `roll_loot_rows` correctly turns into zero winners — `pick_caught_fish` then floors to the flat
    // pool either way. `?`-rollback: a full bag fails here BEFORE the skill-up, so nothing is minted and
    // the skill doesn't climb (retry after freeing space).
    let rows: Vec<(u32, u32, u32, u32)> = fishing_zone_id(ctx, player.map_id, player.x, player.y)
        .map(|zone| {
            ctx.db
                .game_fishing_loot()
                .by_zone()
                .filter(&zone)
                .map(|r| (r.item_entry, r.chance_bp, r.count, r.group_id))
                .collect()
        })
        .unwrap_or_default();
    let winner = roll_loot_rows(ctx, rows)
        .into_iter()
        .next()
        .map(|(item, _)| item);
    let fish = pick_caught_fish(winner, ctx.random::<usize>());
    crate::items::grant_item(ctx, guid, fish, 1)?;
    // Skill-up — the same hook skinning/cooking reuse. DEFAULT sentinel band (orange=1, gray=0 ⇒ always +1)
    // keeps fishing on the byte-identical deterministic path; a real Fishing difficulty table is DEFERRED.
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::FISHING, 1, 0);
    Ok(())
}

/// FISH (`CMSG`-less player action for the alpha): cast at the player's spot, catch one fish, climb
/// Fishing. Authorized via `ctx.sender` like every other player action. The bobber/cast/timer + the
/// near-water gate are DEFERRED — this is the immediate-catch alpha mechanic.
#[reducer]
pub fn fish(ctx: &ReducerContext) -> Result<(), String> {
    let p = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "fisher not in world".to_string())?;
    apply_fish(ctx, p.guid)
}

/// Skin a beast corpse (`CMSG_LOOT` on an already-looted beast body). Grants Light Leather, climbs
/// Skinning, marks the corpse skinned. All gates live in `skin_corpse`; `Err` = client sees an empty
/// loot window (the leather was not granted), which is the correct safe fallback.
#[reducer]
pub fn skin(ctx: &ReducerContext, corpse_guid: u64) -> Result<(), String> {
    let p = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "skinner not in world".to_string())?;
    skin_corpse(ctx, p.guid, corpse_guid)
}

// ===========================================================================================
//  ENCHANTING (completing the 13) — two reducers over the per-instance enchant overlay (`enchant_id` on
//  ItemInstance + the `rules::ENCHANTS` stat table). `disenchant` consumes an equipped item → mats +
//  skill; `enchant_item` stamps an enchant id onto an equipped instance (server-real via the
//  effective-* pipeline). The client glow/green-text is DEFERRED (by-entry item-stat cache). [entity]
// ===========================================================================================

/// The mats a disenchant yields — REAL "Strange Dust" `game_item_template` entry (10940, the low-level
/// enchanting reagent, present in the imported box). The alpha mints a flat 1× per disenchant; a real
/// quality/ilvl→mats table is DEFERRED (the skinning `LEATHER_ENTRY` single-yield shortcut, again).
pub(crate) const DISENCHANT_MATS_ENTRY: u32 = 10940; // "Strange Dust"
pub(crate) const DISENCHANT_MATS_COUNT: u32 = 1;

/// Disenchant/enchant apply only to GEAR (weapon or armor class) — not trade goods, reagents, or stacks.
/// Gates the "disenchant a stack of 20 ore for 1 dust" foot-gun AND the "enchant the dust slot" stale-write
/// (with the target forced to gear, the mats-consume can never touch it). Deliberate
/// simplification: class gate only; the quality≥uncommon gate stays deferred (any weapon/armor
/// disenchants in the alpha).
fn require_gear(ctx: &ReducerContext, entry: u32) -> Result<(), String> {
    const ITEM_CLASS_WEAPON: u8 = 2;
    const ITEM_CLASS_ARMOR: u8 = 4;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(entry)
        .ok_or_else(|| format!("no template for item {entry}"))?;
    if tmpl.class != ITEM_CLASS_WEAPON && tmpl.class != ITEM_CLASS_ARMOR {
        return Err(format!(
            "item {entry} is not enchantable (weapon/armor only)"
        ));
    }
    Ok(())
}

/// DISENCHANT the item in `slot`: consume it (delete the instance), grant enchanting mats, climb
/// Enchanting +1. The core (resolved guid), shared by the `disenchant` reducer + `debug_disenchant` twin.
/// GATE: alive + a weapon/armor item present in `slot` (see `require_gear`). Auto-learns Enchanting on the
/// first disenchant (idempotent). ROLLBACK: `grant_item` is `?`-propagated BEFORE the instance is deleted,
/// so a full bag fails the whole tx and the item is NOT consumed (no item destroyed without delivering mats).
pub(crate) fn apply_disenchant(ctx: &ReducerContext, guid: u64, slot: u8) -> Result<(), String> {
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .ok_or_else(|| "enchanter not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot disenchant".to_string());
    }
    let inst = crate::items::item_in_slot(ctx, guid, slot)
        .ok_or_else(|| format!("no item in slot {slot}"))?;
    require_gear(ctx, inst.entry)?; // weapon/armor only — never destroy a trade-good stack for 1 dust
                                    // AUTO-LEARN Enchanting on the first disenchant (idempotent), giving the skill-up hook a row to climb.
    crate::skill::learn_profession(
        ctx,
        guid,
        player.owner_identity,
        crate::skill::skill_line::ENCHANTING,
        crate::skill::APPRENTICE_CAP,
    );
    // Grant mats FIRST (the `?`-rollback point): a full bag fails here BEFORE the item is consumed, so a
    // failed disenchant destroys nothing and grants nothing (the tx rolls back) — retry after freeing space.
    crate::items::grant_item(ctx, guid, DISENCHANT_MATS_ENTRY, DISENCHANT_MATS_COUNT)?;
    // Consume the disenchanted item (delete the instance) and climb the line. After the grant succeeded, so
    // the delete + skill-up only run on a delivered disenchant.
    ctx.db.game_item_instance().guid().delete(inst.guid);
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::ENCHANTING, 1, 0);
    // If the disenchanted item was EQUIPPED gear (slot 0..=18), its stats just left the body — re-derive
    // max HP/mana (a disenchanted +Sta piece shrinks the health bar). A bag item (>18) touches no equip slot.
    if slot <= crate::items::equip_slot::END {
        crate::spell::recompute_vitals(ctx, guid);
    }
    Ok(())
}

/// DISENCHANT the item in equipment `slot` for mats + Enchanting skill. Authorized via `ctx.sender`.
#[reducer]
pub fn disenchant(ctx: &ReducerContext, slot: u8) -> Result<(), String> {
    let p =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "enchanter not in world".to_string())?;
    apply_disenchant(ctx, p.guid, slot)
}

/// ENCHANT the item in `target_slot` with `enchant_id`: validate the id (it must be a known enchant in
/// `rules::ENCHANTS`), consume the enchanting mats, stamp `enchant_id` onto the instance, climb Enchanting.
/// The core (resolved guid), shared by the `enchant_item` reducer + `debug_enchant_item` twin. The enchant
/// is server-REAL: `equipped_stat_bonus` now folds `enchant_stat(enchant_id, ..)`, so an equipped enchanted
/// piece moves the effective-* readouts (swing/dodge/armor/crit/hit + max HP/mana via recompute_vitals).
/// The client glow/green-text is DEFERRED (the 5875 client caches item stats by ENTRY). ROLLBACK:
/// `remove_items` (the mats consume) is `?`-propagated BEFORE the instance is updated, so a missing-mats
/// enchant changes nothing.
pub(crate) fn apply_enchant_item(
    ctx: &ReducerContext,
    guid: u64,
    target_slot: u8,
    enchant_id: u32,
) -> Result<(), String> {
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .ok_or_else(|| "enchanter not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot enchant".to_string());
    }
    // The enchant id must be a known, applyable enchant — a client can't stamp an arbitrary id.
    if !crate::items::is_known_enchant(enchant_id) {
        return Err(format!("unknown enchant {enchant_id}"));
    }
    let mut inst = crate::items::item_in_slot(ctx, guid, target_slot)
        .ok_or_else(|| format!("no item in slot {target_slot}"))?;
    require_gear(ctx, inst.entry)?; // weapon/armor only — also keeps the target ≠ the mats (no stale write)
                                    // Consume the enchanting mats FIRST (the `?`-rollback point): not enough Strange Dust → Err here,
                                    // BEFORE the instance is stamped, so the item is unchanged and no skill is gained.
    crate::items::remove_items(ctx, guid, DISENCHANT_MATS_ENTRY, DISENCHANT_MATS_COUNT)?;
    // Stamp the enchant onto THIS instance (the per-instance overlay). The overlay folds through
    // `equipped_stat_bonus` for every effective-* consumer — server-real, no client display this slice.
    let instances = ctx.db.game_item_instance();
    inst.enchant_id = enchant_id;
    instances.guid().update(inst);
    crate::skill::gain_profession_skill(ctx, guid, crate::skill::skill_line::ENCHANTING, 1, 0);
    // If the enchanted item is EQUIPPED gear (slot 0..=18) and the enchant moves a pool stat (Stamina/
    // Intellect), re-derive max HP/mana so the bar grows. `recompute_vitals` is a no-op when the derived
    // max is unchanged (e.g. a +Strength weapon enchant), so this is safe to call unconditionally for gear.
    if target_slot <= crate::items::equip_slot::END {
        crate::spell::recompute_vitals(ctx, guid);
    }
    Ok(())
}

/// ENCHANT the item in `target_slot` with `enchant_id` (consumes mats, climbs Enchanting). Authorized via
/// `ctx.sender`.
#[reducer]
pub fn enchant_item(ctx: &ReducerContext, target_slot: u8, enchant_id: u32) -> Result<(), String> {
    let p =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "enchanter not in world".to_string())?;
    apply_enchant_item(ctx, p.guid, target_slot, enchant_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loot_chance_is_half_open() {
        // 10000 bp (100%) always drops (every 0..9999 roll is < 10000); 0 never drops.
        assert!(loot_drops(10000, 0));
        assert!(loot_drops(10000, 9999));
        assert!(!loot_drops(0, 0));
        // A 50% item (5000 bp) drops on the low half, not the high half.
        assert!(loot_drops(5000, 0));
        assert!(loot_drops(5000, 4999));
        assert!(!loot_drops(5000, 5000));
        assert!(!loot_drops(5000, 9999));
        // Basis points represent sub-1% odds the old integer percent couldn't: 39 bp = 0.39%.
        assert!(loot_drops(39, 38));
        assert!(!loot_drops(39, 39));
    }

    #[test]
    fn group_pick_is_one_weighted_band() {
        // A 3-item pool: bands [0,2000) [2000,5000) [5000,6000), total 6000 bp (60% to drop something).
        let w = [2000u32, 3000, 1000];
        assert_eq!(group_pick(&w, 0), Some(0)); // first band
        assert_eq!(group_pick(&w, 1999), Some(0));
        assert_eq!(group_pick(&w, 2000), Some(1)); // second band
        assert_eq!(group_pick(&w, 4999), Some(1));
        assert_eq!(group_pick(&w, 5000), Some(2)); // third band
        assert_eq!(group_pick(&w, 5999), Some(2));
        assert_eq!(group_pick(&w, 6000), None); // past the cumulative weight → nothing drops
        assert_eq!(group_pick(&w, 9999), None);
        // An equal-chance pool that sums to 10000 always yields exactly one member.
        let eq = [2500u32, 2500, 2500, 2500];
        assert!(group_pick(&eq, 9999).is_some());
        assert_eq!(group_pick(&eq, 2500), Some(1));
    }

    #[test]
    fn rank_money_scales_and_rank0_is_identity() {
        // Rank 0 (normal) is the IDENTITY — a normal kill's purse is unchanged for any rolled amount.
        for money in [0u32, 1, 17, 1000, 1_000_000, u32::MAX] {
            assert_eq!(
                scale_money_for_rank(money, 0),
                money,
                "rank-0 money must be identity for {money}"
            );
        }
        // Elite/rare/boss money bumps (permille): a 100-copper roll scales by the documented factors.
        assert_eq!(scale_money_for_rank(100, 1), 150); // elite ×1.5
        assert_eq!(scale_money_for_rank(100, 2), 200); // rare-elite ×2.0
        assert_eq!(scale_money_for_rank(100, 3), 300); // boss ×3.0
        assert_eq!(scale_money_for_rank(100, 4), 200); // rare ×2.0
                                                       // Unknown rank → normal (never amplify an unrecognized value).
        assert_eq!(scale_money_for_rank(100, 9), 100);
        // An empty roll (0 copper) stays 0 at any rank — a no-money creature drops nothing regardless.
        assert_eq!(scale_money_for_rank(0, 3), 0);
        // The permille table: rank 0 is the identity divisor; elite/boss exceed normal.
        assert_eq!(rank_money_multiplier_permille(0), 1000);
        assert!(rank_money_multiplier_permille(1) > rank_money_multiplier_permille(0));
        assert!(rank_money_multiplier_permille(3) > rank_money_multiplier_permille(1));
        // Saturating: a near-max roll × a >1 multiplier must not wrap.
        assert!(scale_money_for_rank(u32::MAX, 3) >= u32::MAX / 2);
    }

    // ---- SKINNING (professions slice 2) ----

    /// The SKIN gate: every reject condition fails with its reason, and ONLY an alive looter at a dead,
    /// in-range, same-map BEAST corpse that isn't already skinned proceeds (with sufficient Skinning skill).
    /// This is the pure decision the live `skin_corpse` keys all its rejects off (mirrors `loot_money`'s
    /// gate, plus beast + skinned + skill checks).
    #[test]
    fn skin_gate_admits_only_a_fresh_in_range_beast_corpse() {
        // Helpers: learned=1 (freshly trained), creature_level=1 (requires (1-1)*10=0) — the baseline pass.
        let learned: u32 = 1;
        let lvl: u32 = 1; // required = (1-1)*10 = 0 <= learned → passes

        // The all-pass baseline: alive looter, dead non-player beast, same map, in range, not skinned,
        // trained Skinning at sufficient skill.
        assert!(can_skin(false, false, true, true, 0.0, true, false, learned, lvl).is_ok());
        // In range at exactly the cap (10 yd)² is still OK (inclusive, matching loot_money's `>`).
        assert!(can_skin(
            false,
            false,
            true,
            true,
            LOOT_RANGE_SQ,
            true,
            false,
            learned,
            lvl
        )
        .is_ok());

        // Each reject, in the order the gate checks them:
        assert_eq!(
            can_skin(true, false, true, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "dead players cannot skin"
        );
        assert_eq!(
            can_skin(false, true, true, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "cannot skin a player"
        );
        assert_eq!(
            can_skin(false, false, false, true, 0.0, true, false, learned, lvl).unwrap_err(),
            "target is not a corpse"
        );
        assert_eq!(
            can_skin(false, false, true, false, 0.0, true, false, learned, lvl).unwrap_err(),
            "corpse on another map"
        );
        assert_eq!(
            can_skin(
                false,
                false,
                true,
                true,
                LOOT_RANGE_SQ + 1.0,
                true,
                false,
                learned,
                lvl
            )
            .unwrap_err(),
            "corpse out of range"
        );
        // NON-BEAST rejection (e.g. the demo Chicken, creature_type 8 → is_beast=false).
        assert_eq!(
            can_skin(false, false, true, true, 0.0, false, false, learned, lvl).unwrap_err(),
            "not a beast — cannot be skinned"
        );
        // RE-SKIN rejection (already_skinned=true) — the marker that stops a second skin of the same corpse.
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, true, learned, lvl).unwrap_err(),
            "already skinned"
        );
        // NOT TRAINED: learned=0 → "you have not learned Skinning".
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 0, lvl).unwrap_err(),
            "you have not learned Skinning"
        );
        // SKILL TOO LOW: learned=5, creature_level=2 (requires (2-1)*10=10) → rejected.
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 5, 2).unwrap_err(),
            "your Skinning skill (5) is too low — need 10"
        );
        // BOUNDARY: learned exactly equals the requirement → admitted (lvl=2 → required=10).
        assert!(can_skin(false, false, true, true, 0.0, true, false, 10, 2).is_ok());
        // Higher-level creature: creature_level=3 requires (3-1)*10=20; learned=19 rejected, 20 admitted.
        assert_eq!(
            can_skin(false, false, true, true, 0.0, true, false, 19, 3).unwrap_err(),
            "your Skinning skill (19) is too low — need 20"
        );
        assert!(can_skin(false, false, true, true, 0.0, true, false, 20, 3).is_ok());
    }

    /// Renamed (was `skinning_yields_one_light_leather_2318` — "yields" oversold a behavioral check when
    /// this only guards the constants): the leather product is the REAL "Light Leather" TRADE_GOODS entry
    /// (2318), minted a flat 1 per skin (the skill→count table is DEFERRED). A wire-facing drift guard.
    #[test]
    fn skin_leather_constants_are_the_real_light_leather_2318_flat_one() {
        assert_eq!(LEATHER_ENTRY, 2318);
        assert_eq!(LEATHER_COUNT, 1);
    }

    // ---- FISHING (completing the 13) ----

    /// Re-scoped (was `fish_grants_one_pooled_fish_and_climbs_fishing`): the "grants one" / "climbs
    /// fishing" narrative modeled `apply_fish`'s grant+skill-up with a local loop that never calls
    /// `apply_fish` (a ctx fn) — not exercised here. What's real and worth guarding: the pool itself,
    /// which `apply_fish` indexes with `ctx.random::<usize>() % FISH_POOL.len()` — a drift guard on its
    /// exact non-empty, real-entry contents (an empty pool would divide by zero at cast time).
    #[test]
    fn fish_pool_is_nonempty_and_lists_the_real_low_level_fish_entries() {
        assert!(
            !FISH_POOL.is_empty(),
            "fishing must have a pool to roll (an empty pool divides by zero)"
        );
        assert_eq!(
            FISH_POOL,
            &[6291, 6303, 6289],
            "the three real low-level fish entries"
        );
    }

    // ---- ENCHANTING: disenchant (completing the 13) ----

    /// Re-scoped (was `disenchant_consumes_item_and_grants_dust_and_skill`): the consume/grant/skill-up
    /// narrative modeled `apply_disenchant`'s effect with local counters that never call it (a ctx fn) —
    /// not exercised here. What's real and worth guarding: the mats entry/count `apply_disenchant` grants,
    /// a drift guard against the wire-facing item id ever moving.
    #[test]
    fn disenchant_mats_are_the_real_strange_dust_entry() {
        assert_eq!(
            DISENCHANT_MATS_ENTRY, 10940,
            "Strange Dust (real imported enchanting reagent)"
        );
        assert_eq!(
            DISENCHANT_MATS_COUNT, 1,
            "the alpha mints a flat 1x Strange Dust"
        );
    }

    // ---- LOOT-FAMILY COMPLETENESS (work-item 210) ----

    /// `needs_item_pure` — the pure decision behind `killer_needs_item`: a COLLECT_ITEM objective on an
    /// ACTIVE quest matching `item` says yes; a KILL_CREATURE objective (wrong kind), a different item
    /// (wrong target), or an objective whose quest ISN'T in `active_quests` (a stale/rewarded quest's
    /// leftover objective row) all say no.
    #[test]
    fn needs_item_pure_matches_only_an_active_quests_collect_item_objective() {
        let active = [100u32, 200];
        let objectives = [
            (100u32, objective_kind::COLLECT_ITEM, 55u32, 10u32), // matches — quest 100 is active, needs 10
            (100, objective_kind::KILL_CREATURE, 999, 5),         // wrong kind (same quest)
            (300, objective_kind::COLLECT_ITEM, 55, 10), // right item, but quest 300 is NOT active
        ];
        assert!(needs_item_pure(&active, &objectives, 55, 0));
        assert!(
            !needs_item_pure(&active, &objectives, 999, 0),
            "a KILL_CREATURE objective never satisfies an item need"
        );
        assert!(
            !needs_item_pure(&active, &objectives, 12345, 0),
            "no objective names this item at all"
        );
        assert!(
            !needs_item_pure(&[], &objectives, 55, 0),
            "no active quests at all → nothing is ever needed"
        );
        // The count cap: holding 9/10 still needs it; 10/10 (or more) stops the drop.
        assert!(needs_item_pure(&active, &objectives, 55, 9));
        assert!(
            !needs_item_pure(&active, &objectives, 55, 10),
            "requirement met → the quest drop stops"
        );
        assert!(!needs_item_pure(&active, &objectives, 55, 11));
    }

    /// `zone_of_area` — the pure one-hop parent walk `fishing_zone_id` shares with `world::graveyard`'s
    /// `resolve_zone_id`: a subzone (nonzero `parent_area_id`) resolves to its parent zone; a top-level
    /// zone (`parent_area_id == 0`) resolves to itself.
    #[test]
    fn zone_of_area_walks_one_hop_to_the_parent_or_stays_put() {
        assert_eq!(zone_of_area(38 /* Goldshire */, 12 /* Elwynn */), 12);
        assert_eq!(zone_of_area(12 /* Elwynn (top-level) */, 0), 12);
    }

    /// `pick_caught_fish` — the fallback floor: a real winner always wins; `None` (no zone rows, or a
    /// zone table that rolled nothing) floors to `FISH_POOL` at the given index, wrapping via `%` so an
    /// out-of-range `pool_idx` (a raw `ctx.random::<usize>()`) never panics.
    #[test]
    fn pick_caught_fish_prefers_the_winner_else_floors_to_the_pool() {
        assert_eq!(
            pick_caught_fish(Some(6291), 0),
            6291,
            "a real winner is never overridden by the pool"
        );
        assert_eq!(pick_caught_fish(None, 0), FISH_POOL[0]);
        assert_eq!(
            pick_caught_fish(None, FISH_POOL.len()),
            FISH_POOL[0],
            "wraps via %, never panics OOB"
        );
        assert_eq!(pick_caught_fish(None, 1), FISH_POOL[1]);
    }

    /// Schema round-trip guard: `CreatureLoot.quest_only` defaults to `false` (additive migration —
    /// every pre-210 row keeps rolling unconditionally), and the new tables' row shapes construct with
    /// named fields exactly like `CreatureLoot` (a compile-time guard against a silently reordered /
    /// renamed column — the importer's positional SQL INSERT depends on this order matching its
    /// column list verbatim). `CorpseLoot` now END-carries `quest_only`/`reserved_for` too (187 slice 0).
    #[test]
    fn new_loot_family_tables_construct_with_the_documented_shape() {
        let creature = CreatureLoot {
            id: 0,
            creature_entry: 1,
            item_entry: 2,
            chance_bp: 3,
            count: 4,
            group_id: 0,
            quest_only: false,
        };
        assert!(
            !creature.quest_only,
            "quest_only must default/construct false for an ordinary row"
        );

        let pickpocket = GamePickpocketLoot {
            id: 0,
            creature_entry: 1,
            item_entry: 2,
            chance_bp: 3,
            count: 4,
            group_id: 0,
            quest_only: true,
        };
        assert!(pickpocket.quest_only);

        let go_loot = GameObjectLoot {
            id: 0,
            loot_id: 1,
            item_entry: 2,
            chance_bp: 3,
            count: 4,
            group_id: 0,
            quest_only: false,
        };
        assert_eq!(go_loot.loot_id, 1);

        let skin = GameSkinningLoot {
            id: 0,
            skin_loot_id: 1,
            item_entry: 2,
            chance_bp: 3,
            count: 4,
            group_id: 0,
        };
        assert_eq!(skin.skin_loot_id, 1);

        let fish = GameFishingLoot {
            id: 0,
            zone_id: 12,
            item_entry: 6291,
            chance_bp: 3,
            count: 4,
            group_id: 0,
        };
        assert_eq!(fish.zone_id, 12);

        let corpse_row = CorpseLoot {
            id: 0,
            corpse_guid: 1,
            slot: 0,
            item_entry: 2,
            count: 1,
            quest_only: true,
            reserved_for: 42,
            designated_looter_guid: 0,
            master_only: false,
            withheld: false,
        };
        assert!(corpse_row.quest_only);
        assert_eq!(corpse_row.reserved_for, 42);
        let shared_row = CorpseLoot {
            id: 0,
            corpse_guid: 1,
            slot: 0,
            item_entry: 2,
            count: 1,
            quest_only: false,
            reserved_for: 0,
            designated_looter_guid: 7,
            master_only: true,
            withheld: true,
        };
        assert!(
            !shared_row.quest_only,
            "quest_only must default/construct false for an ordinary row"
        );
        assert_eq!(
            shared_row.reserved_for, 0,
            "reserved_for must default/construct 0 (unclaimed)"
        );
        assert_eq!(shared_row.designated_looter_guid, 7);
        assert!(shared_row.master_only);
        assert!(shared_row.withheld);
    }

    // ---- Group loot methods (work-item 187) slice 0: per-viewer quest drops ----

    // NOTE on `roll_loot_rows_quest_aware`: like `roll_loot_rows` before it, it draws from
    // `ctx.random` and has no direct unit test (the module crate has no `ReducerContext` test harness
    // by design — rule: never mock it, extract + test pure functions instead). Its pure primitives
    // (`loot_drops`/`group_pick`, exercised above) are UNCHANGED by this slice. The actual behavioral
    // change — "a quest_only row rolls regardless of any player's quest state" — is a STRUCTURAL fact
    // instead: the function's signature carries no "needs"/killer closure or parameter at all (compare
    // against the OLD `quest_gate_rows(rows, has_matching_quest)` this replaces, which took one), so
    // there is nothing left in the roll path capable of gating on it. Verified live via the wire
    // harness's group scenario (see the work-item's runbook note) rather than faked here.

    /// `quest_take_allowed` — the TAKE-time gate: an UNRESERVED row (`reserved_for == 0`, the shared
    /// row nobody split yet) is claimable by anyone who currently needs the item (a non-quest-holder
    /// can't snipe it); a RESERVED row is claimable by its reserved owner UNCONDITIONALLY — the
    /// reservation is the grant, need was checked at clone time. This MUST agree with the gateway's
    /// `quest_row_visible_to_viewer` truth table (reads.rs) or a viewer sees rows they can't take.
    #[test]
    fn quest_take_allowed_admits_the_reservee_unconditionally_and_needing_takers_on_shared_rows() {
        // Unreserved (shared) row: any needing taker is admitted; a non-needing one is rejected.
        assert!(quest_take_allowed(0, 7, true));
        assert!(
            !quest_take_allowed(0, 7, false),
            "no need → no take on the unreserved shared row"
        );
        // Reserved for guid 7: guid 7 admitted regardless of current need; guid 8 rejected even if
        // THEY also need it — the reservation is exclusive.
        assert!(quest_take_allowed(7, 7, true));
        assert!(
            !quest_take_allowed(7, 8, true),
            "reserved for someone else — not this taker's to claim"
        );
        assert!(
            quest_take_allowed(7, 7, false),
            "the reservation IS the grant — re-checking need here would strand a reservee whose need \
             lapsed post-clone with a visible-but-untakeable row (187 slice 0 review finding); mirrors \
             quest_row_visible_to_viewer's 'reservation is authoritative' arm"
        );
    }

    /// `clone_targets` — only the STILL-NEEDING other members get a clone; a member who capped out or
    /// dropped the quest between roll and take gets nothing (mirrors vanilla: the item simply isn't
    /// offered). Order-preserving (deterministic slot assignment downstream).
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

    /// `next_free_slots` — fresh ascending slots strictly past the corpse's current max, so a clone
    /// never collides with the just-deleted original's slot or a sibling drop. An empty corpse (no
    /// rows left) starts at slot 0, never panics.
    #[test]
    fn next_free_slots_never_collides_with_whats_already_on_the_corpse() {
        assert_eq!(next_free_slots(&[0, 1, 2], 2), vec![3, 4]);
        assert_eq!(
            next_free_slots(&[], 3),
            vec![0, 1, 2],
            "an emptied corpse starts back at slot 0"
        );
        assert_eq!(next_free_slots(&[5], 1), vec![6]);
        // Saturating at the u8 ceiling — never panics/wraps even from an already-maxed corpse.
        assert_eq!(next_free_slots(&[255], 2), vec![255, 255]);
    }

    // ---- Group loot methods (work-item 187 slices 1-4) ----

    /// `group_loot_decision_for_row` — the threshold-split matrix: FFA never restricts; ROUND_ROBIN
    /// always designates regardless of quality; GROUP splits on the threshold (below → designate,
    /// at/above → roll); NEED_BEFORE_GREED always rolls regardless of quality; MASTER splits on the
    /// threshold (below → designate, at/above → master-only). An unrecognized method degrades to Ffa.
    #[test]
    fn group_loot_decision_matches_the_threshold_split_matrix() {
        use crate::group::loot_method::*;
        use GroupLootDecision::*;

        // FFA: never restricts, at any quality.
        assert_eq!(group_loot_decision_for_row(FFA, 0, 2), Ffa);
        assert_eq!(group_loot_decision_for_row(FFA, 6, 2), Ffa);

        // ROUND_ROBIN: always designate, below AND at/above the threshold.
        assert_eq!(group_loot_decision_for_row(ROUND_ROBIN, 0, 2), Designate);
        assert_eq!(group_loot_decision_for_row(ROUND_ROBIN, 6, 2), Designate);

        // GROUP: the threshold split — below designates, AT (inclusive) and above rolls.
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

        // NEED_BEFORE_GREED: always rolls, ignoring the threshold entirely.
        assert_eq!(group_loot_decision_for_row(NEED_BEFORE_GREED, 0, 2), Roll);
        assert_eq!(group_loot_decision_for_row(NEED_BEFORE_GREED, 6, 6), Roll);

        // MASTER: the same threshold split as GROUP, but Master instead of Roll.
        assert_eq!(group_loot_decision_for_row(MASTER, 1, 2), Designate);
        assert_eq!(group_loot_decision_for_row(MASTER, 2, 2), Master);
        assert_eq!(group_loot_decision_for_row(MASTER, 6, 2), Master);

        // An unrecognized method (shouldn't happen post-validation) never invents a restriction.
        assert_eq!(group_loot_decision_for_row(255, 6, 2), Ffa);
    }

    /// `advance_rr_cursor` — round-robin trap: "skipping offline members". Wraps around a STABLE
    /// ordering, picks the next ONLINE member from the current cursor (inclusive), and returns
    /// `None` only when the whole group is offline (never panics/blocks looting).
    #[test]
    fn advance_rr_cursor_skips_offline_members_and_wraps() {
        // All online: cursor 0 picks index 0, advances to 1.
        assert_eq!(advance_rr_cursor(&[true, true, true], 0), Some((0, 1)));
        assert_eq!(advance_rr_cursor(&[true, true, true], 1), Some((1, 2)));
        // Wraps past the end back to 0.
        assert_eq!(advance_rr_cursor(&[true, true, true], 2), Some((2, 0)));
        // Member at the cursor is OFFLINE — skip forward to the next online member.
        assert_eq!(
            advance_rr_cursor(&[true, false, true], 1),
            Some((2, 0)),
            "index 1 is offline, skip to 2"
        );
        // The skip WRAPS if needed (cursor near the end, only an early member online).
        assert_eq!(
            advance_rr_cursor(&[true, false, false], 2),
            Some((0, 1)),
            "wraps around to the only online member"
        );
        // Nobody online at all → None (degrades to FFA at the call site, never panics).
        assert_eq!(advance_rr_cursor(&[false, false], 0), None);
        assert_eq!(
            advance_rr_cursor(&[], 0),
            None,
            "an empty roster never panics"
        );
    }

    /// `contending_tier` — NEED beats GREED unconditionally; an all-pass vote set contends nothing
    /// (the caller unlocks FFA-in-group). Order-preserving indices into the input slice.
    #[test]
    fn contending_tier_need_beats_greed_and_empty_means_all_passed() {
        use vote_kind::*;
        // A mix: NEED wins even though GREED/PASS are also present.
        assert_eq!(contending_tier(&[PASS, GREED, NEED, GREED]), vec![2]);
        // Multiple NEEDs all contend (the tie-break is a separate step, `pick_roll_winner`).
        assert_eq!(contending_tier(&[NEED, PASS, NEED]), vec![0, 2]);
        // No NEED at all → GREED contends.
        assert_eq!(contending_tier(&[PASS, GREED, GREED]), vec![1, 2]);
        // Everyone passed → nobody contends (empty).
        assert!(contending_tier(&[PASS, PASS, PASS]).is_empty());
        assert!(contending_tier(&[]).is_empty());
    }

    /// `pick_roll_winner` — the vote-resolution matrix's final step: the unique max roll wins; a
    /// TIE for the max (two+ share it) returns `None` so the caller re-rolls just the tied members
    /// (cmangos's internal re-roll) rather than picking arbitrarily.
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

    /// `group_loot_take_allowed` — the take-time gate for NON-quest rows: a nonzero `reserved_for`
    /// (winner-locked fallback) is exclusive and unconditional; a `master_only` row is NEVER takable
    /// via the plain path (even by the stamped master — they must use `loot_master_give`); a
    /// `designated_looter_guid` restricts to that guid; the zero/false baseline is plain FFA.
    #[test]
    fn group_loot_take_allowed_gates_withheld_reserved_master_and_designated_rows() {
        // WITHHELD (live roll): nobody autostores it — this is exactly the row shape a mid-roll
        // GROUP/NBG item has (withheld=true, everything else zero/false), the crafted-take exploit
        // shape the 187 review caught. Checked before every other flag.
        assert!(
            !group_loot_take_allowed(true, 0, false, 0, 7),
            "a live roll owns the row"
        );
        assert!(
            !group_loot_take_allowed(true, 7, false, 0, 7),
            "even the future reservee waits for resolution"
        );
        // Baseline: no restriction at all → anyone may take.
        assert!(group_loot_take_allowed(false, 0, false, 0, 7));
        // Reserved (winner-locked): only the reservee, regardless of the other flags.
        assert!(group_loot_take_allowed(false, 7, false, 0, 7));
        assert!(
            !group_loot_take_allowed(false, 7, false, 0, 8),
            "reserved for someone else"
        );
        assert!(
            !group_loot_take_allowed(false, 7, true, 99, 8),
            "reserved wins over master_only/designated too"
        );
        // Master-only: nobody takes it via the plain path, not even the stamped master.
        assert!(
            !group_loot_take_allowed(false, 0, true, 42, 42),
            "master_only blocks even the master's OWN take"
        );
        assert!(!group_loot_take_allowed(false, 0, true, 42, 8));
        // Designated (round-robin/below-threshold): only that guid.
        assert!(group_loot_take_allowed(false, 0, false, 42, 42));
        assert!(
            !group_loot_take_allowed(false, 0, false, 42, 8),
            "not the designated looter"
        );
    }

    // ---- Money-loot split (work-item 221) ----

    /// SOLO passthrough: an empty `recipients` slice (no `game_corpse_loot_eligible` snapshot) always
    /// credits the WHOLE amount to the looter in a single entry — byte-identical to the pre-221
    /// unconditional-credit path, regardless of amount.
    #[test]
    fn split_money_solo_passthrough_credits_the_whole_amount_to_the_looter() {
        assert_eq!(split_money(137, &[], 7), vec![(7, 137)]);
        assert_eq!(split_money(0, &[], 7), vec![(7, 0)]);
        assert_eq!(split_money(u32::MAX, &[], 7), vec![(7, u32::MAX)]);
    }

    /// GROUPED even split: divides evenly with NO remainder when it divides cleanly, in the exact
    /// recipient order given.
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

    /// GROUPED split with a remainder: the remainder (cmangos convention [V]) goes to whichever
    /// recipient IS the looter, wherever it sits in the list — never to the first/last member
    /// positionally.
    #[test]
    fn split_money_remainder_goes_to_the_looter() {
        // 100 / 3 = 33 each, remainder 1 → the looter (guid 2, the middle entry) gets 34.
        assert_eq!(
            split_money(100, &[1, 2, 3], 2),
            vec![(1, 33), (2, 34), (3, 33)]
        );
        // Looter is the FIRST entry.
        assert_eq!(
            split_money(100, &[1, 2, 3], 1),
            vec![(1, 34), (2, 33), (3, 33)]
        );
        // Looter is the LAST entry.
        assert_eq!(
            split_money(100, &[1, 2, 3], 3),
            vec![(1, 33), (2, 33), (3, 34)]
        );
        // 7 copper across 2 recipients: 3 each, remainder 1 to the looter.
        assert_eq!(split_money(7, &[5, 9], 9), vec![(5, 3), (9, 4)]);
    }

    /// If the looter is not itself in the snapshot (edge case — e.g. joined the group after the kill
    /// was snapshotted), the remainder is folded into an EXTRA entry for the looter instead of being
    /// silently dropped — every recipient still gets its even share.
    #[test]
    fn split_money_looter_outside_recipients_gets_an_extra_remainder_entry() {
        assert_eq!(
            split_money(100, &[10, 20, 30], 99),
            vec![(10, 33), (20, 33), (30, 33), (99, 1)]
        );
        // No remainder → no extra entry needed (nothing to fold in).
        assert_eq!(
            split_money(90, &[10, 20, 30], 99),
            vec![(10, 30), (20, 30), (30, 30)]
        );
    }

    /// A single-recipient "group" (shouldn't happen — `apply_group_loot_rules` never snapshots below
    /// 2 recipients — but the pure fn degrades sanely if it ever did): the whole amount to that one
    /// recipient, remainder folded in if they're the looter.
    #[test]
    fn split_money_single_recipient_degrades_to_whole_amount() {
        assert_eq!(split_money(50, &[7], 7), vec![(7, 50)]);
    }

    // ---- The realm-core loot-roll plane (issue #50) ----
    //
    // A reducer body needs a live `ReducerContext`, so none of these can be EXECUTED by a test in
    // this crate — exactly why they are scanned, the same technique `group.rs` uses for its own
    // realm-plane reducers (issue #22's review of PR #49).

    use crate::test_scan::code_of;

    /// **The operator gate is the entire authorization of the realm loot-roll plane.**
    ///
    /// All three reducers below take a corpse/roll identity or an actor guid as an ARGUMENT rather
    /// than deriving it from `ctx.sender()` — realm-core has no live entity to derive one from, and
    /// the two world-shard-only ones (`settle_loot_roll`/`clear_promoted_loot_roll`) grant an item or
    /// delete roll rows outright. The gate is the only thing between an arbitrary connection and
    /// forging a roll outcome or wiping another group's live roll.
    #[test]
    fn every_realm_loot_reducer_is_operator_gated() {
        for f in [
            "pub fn realm_loot_op(",
            "pub fn settle_loot_roll(",
            "pub fn clear_promoted_loot_roll(",
        ] {
            let body = code_of(include_str!("loot.rs"), f);
            let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{f}` no longer OPENS with the operator gate — a gate that is present but not the \
                 FIRST statement (wrapped in `if false`, `let _ =`, or preceded by an early return) \
                 is no gate. Body was:\n{body}"
            );
        }
    }

    /// The op byte is a wire value the gateway sends and `realm_loot_op` dispatches on —
    /// `lyracore_shared::loot_roll::loot_op` pins only the NUMBERS. This pins what each number DOES: a
    /// swapped arm would silently run VOTE for a START, corrupting a fresh roll's recipient snapshot.
    #[test]
    fn realm_loot_op_dispatches_start_and_vote_to_their_own_cores() {
        let body = code_of(include_str!("loot.rs"), "pub fn realm_loot_op(");
        for (op, core) in [
            ("loot_op::START =>", "{ insert_roll_rows( ctx, corpse_guid, slot, item_entry, &recipients, deadline_micros, ); Ok(()) }"),
            ("loot_op::VOTE =>", "cast_vote_on(ctx, Plane::RealmCore, corpse_guid, slot, actor_guid, vote)"),
        ] {
            let arm = body
                .split(op)
                .nth(1)
                .unwrap_or_else(|| panic!("`realm_loot_op` no longer dispatches `{op}`. Body was:\n{body}"));
            let arm: String = arm.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                arm.starts_with(core),
                "`{op}` no longer runs `{core}` — the op byte is a WIRE value the gateway sends and \
                 this match dispatches on; a swapped arm runs the wrong op for every roll, silently. \
                 Arm was:\n{}",
                &arm[..arm.len().min(140)]
            );
        }
    }

    /// [`settle_roll_grant`] must check `withheld` BEFORE granting anything — without it, a
    /// wrong-shard `settle_loot_roll` call (the gateway fans a `ROLL_WON` settle to EVERY connected
    /// shard, since it does not know in advance which one holds the corpse) could steal an ordinary,
    /// un-rolled `game_corpse_loot` row that merely happens to share a `(corpse_guid, slot)` number
    /// with the resolved roll on another shard.
    #[test]
    fn settle_roll_grant_checks_withheld_before_granting() {
        let body = code_of(include_str!("loot.rs"), "pub(crate) fn settle_roll_grant(");
        let withheld_at = body
            .find("if !row.withheld {")
            .expect("settle_roll_grant no longer gates on `row.withheld` — see this fn's own doc");
        let grant_at = body
            .find("crate::items::grant_item(")
            .expect("settle_roll_grant no longer calls grant_item at all");
        assert!(
            withheld_at < grant_at,
            "the `withheld` guard must run BEFORE `grant_item` — a guard added AFTER the grant \
             already happened is not a guard. Body was:\n{body}"
        );
    }

    /// Issue #50's own acceptance test: `group::remove_member`'s disband branch must call
    /// `force_resolve_rolls_for_disband` UNCONDITIONALLY and BEFORE it tears the group row down.
    /// That ordering — not a `Plane` flag, which does not exist on `remove_member` — is what makes a
    /// disbanding party's live rolls resolve on the SAME database, in the SAME transaction, as the
    /// membership change: whichever database `remove_member` executes on (a world shard, unsharded;
    /// REALM-CORE, sharded) is also the database its `game_loot_roll` rows are the truth on. Mutation-
    /// check: deleting the call, or moving it after the group-row delete, must turn this red.
    #[test]
    fn disband_resolves_live_rolls_before_it_tears_the_group_row_down() {
        let body = code_of(include_str!("group.rs"), "pub(crate) fn remove_member(");
        let call = "crate::loot::force_resolve_rolls_for_disband(ctx, &all_guids, survivor);";
        let call_at = body
            .find(call)
            .unwrap_or_else(|| panic!("`remove_member` no longer force-resolves live rolls on disband (issue #50). Body was:\n{body}"));
        let delete_at = body
            .find("ctx.db.game_group().group_id().delete(group_id);")
            .expect("`remove_member` no longer deletes the group row on disband");
        assert!(
            call_at < delete_at,
            "force_resolve_rolls_for_disband must run BEFORE the group row is torn down (issue #50) \
             — resolving after the delete would still be correct data-wise, but a reviewer relying on \
             \"same transaction, no mirror\" should not have to re-derive that from execution order \
             every time this function changes. Body was:\n{body}"
        );
    }
}
