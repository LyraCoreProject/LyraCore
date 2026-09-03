//! Money + item loot from a creature corpse. The lootable copper is rolled onto the corpse's
//! `WorldEntity` (with `UNIT_DYNFLAG_LOOTABLE`) on the killing blow in `combat/`; `loot_money` moves
//! it into the looter's purse and clears the flag. Item drops are rolled into `game_corpse_loot` rows
//! on the same killing blow; the gateway reads them to populate the loot window, and taking one
//! (`CMSG_AUTOSTORE_LOOT_ITEM`) is `crate::items::take_loot` → `apply_take_loot`. [server]/[entity]
//!
//! Split into three files that stay one Rust module (`crate::loot::*` paths are
//! unaffected — everything below re-exports through): this file (the roll core, the money split,
//! and the loot-family DATA tables — creature/pickpocket/gameobject/skinning/fishing), `rolls.rs`
//! (the NEED/GREED/round-robin/master group-loot machinery), and the sibling top-level
//! `crate::professions` (skinning/fishing/enchanting — feature reducers over that data, not loot-table
//! machinery themselves).
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

use spacetimedb::{table, ReducerContext, Table};

use crate::character::game_character; // credit_purse's offline-recipient fallback (work-item 221)
use crate::game_group_member; // clone_quest_loot_for_group's GameObject roster read
use crate::game_world_entity;
use crate::quest::objective_kind;
use crate::{game_character_quest, game_quest_objective}; // killer_needs_item (fishing's zone resolve now lives in terrain::zone_id_at)
use lyracore_shared::loot_roll::event_kind as roll_event_kind; // apply_loot_money's MONEY_SHARE push

mod rolls;
pub use rolls::*;
pub(crate) mod tag;
pub use tag::*;

// The profession reducers (skinning/fishing/enchanting) live in the sibling top-level
// `crate::professions` module — re-exported here so every existing
// `crate::loot::skin_corpse`/`apply_fish`/`apply_disenchant`/`apply_enchant_item`/`entry_is_beast`
// call site (debug.rs's twins) keeps compiling unchanged even though nothing in THIS file uses
// them directly.
#[allow(unused_imports)]
pub(crate) use crate::professions::{
    apply_disenchant, apply_enchant_item, apply_fish, entry_is_beast, skin_corpse,
};

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

/// Roll ONE generic loot table the SAME way independent rows (`group_id == 0`) test their own
/// `chance_bp`, and grouped rows (`group_id > 0`) are collected and rolled ONCE per group (pick-one,
/// weighted), but carries each winning row's `quest_only` flag through instead of discarding it
/// (work-item 187 slice 0): quest rows now roll into `game_corpse_loot` UNCONDITIONALLY — the
/// drop-CHANCE still applies; only the old pre-roll `killer_needs_item` GATE is gone (module doc,
/// decision #1's fix) — so the caller needs to know which winners were quest-only to stamp the new
/// `quest_only` column. This is the ONE roll body every family's wrapper calls (the "one
/// roll core" — `roll_loot_rows` below used to duplicate this whole algorithm minus the flag). Draws
/// from the module RNG (`ctx.random`), so it has no direct unit test; its pure primitives
/// (`loot_drops`/`group_pick`) are tested below.
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

/// Roll ONE generic loot table (the shape shared by every family — creature/pickpocket/gameobject/
/// skinning/fishing): independent rows (`group_id == 0`) each test their own `chance_bp`; grouped rows
/// (`group_id > 0`) are collected and rolled ONCE per group (pick-one, weighted by `chance_bp`).
/// Returns the winners as `(item_entry, count)`, in table order for independents then group order —
/// callers decide what a winner MEANS (a `game_corpse_loot` row, a direct `grant_item`, …).
///
/// A thin adapter over [`roll_loot_rows_quest_aware`] (the "one roll core"): tag every row
/// `quest_only = false`, delegate, then drop the flag off each winner. The RNG draw sequence is
/// IDENTICAL to the old standalone body this replaces (the same `ctx.random` calls in the same
/// order), so `professions::skin_corpse`/`professions::apply_fish` — the two callers with no
/// `quest_only` column to carry — stay byte-identical.
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

/// May `taker_guid` take a NON-quest `game_corpse_loot` row via the plain autostore path, given its
/// `withheld` bit (a live NEED/GREED roll owns the row — see `rolls.rs`), `reserved_for` (generalized
/// — see the struct doc: nonzero now ALSO covers a NEED/GREED winner's inventory-full fallback row),
/// `master_only`, and `designated_looter_guid`? Quest rows are gated separately by
/// [`quest_take_allowed`] (above) — `apply_take_loot` calls this one only when `!row.quest_only`.
/// `withheld` is checked FIRST and unconditionally: the per-viewer display filter hides these rows,
/// but display filtering is NOT enforcement — without this gate a crafted `take_loot` on the hidden
/// slot steals the item mid-roll, and the eventual `resolve_roll` grant silently no-ops while
/// ROLL_WON still fires (187 review finding #1). Pure.
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

/// Purge every `game_corpse_loot` / `game_corpse_loot_eligible` row still parked on `guid`, BEFORE a
/// fresh roll ever writes to it. Two residue sources land here: (a) work-item 267's corpse-guid REUSE
/// (a harness SQL teardown or `debug_spawn_at_feet` skipping the decay reaper leaves a departed kill's
/// `game_corpse_loot_eligible` snapshot behind for the next creature minted at that guid), and (b)
/// the pickpocket/kill collision — `roll_pickpocket_loot` inserts `game_corpse_loot` rows at
/// slots 0.. keyed on the LIVE creature's guid (vanilla lets a rogue crack a pocket before the kill);
/// if the mob dies before every row is taken, `roll_creature_loot` re-inserts kill drops starting at
/// slot 0 on the SAME guid, producing duplicate `(corpse_guid, slot)` pairs that every first-match
/// consumer (`items::apply_take_loot`, `unlock_row_ffa`, `settle_roll_grant`, `loot_master_give`)
/// addresses ambiguously. Vanilla-correct either way: the pickpocket window dies with the mob, so any
/// leftover pickpocket rows are stale the instant it does. Called from the ONE spot every creature
/// death routes through (`combat::kill_creature`), before the fresh loot roll / group snapshot — a
/// no-op for a guid with no residue (the common path). [server]
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

/// Reap every loot-family row keyed on `corpse_guid` (creature corpse OR chest-GO guid): item rows,
/// the group-loot eligibility snapshot, rolls + votes. This is the CANONICAL teardown sweep — the one
/// every despawn path routes through ([`crate::creatures::despawn_creature_entity`] for creatures, the
/// GO-copy loop in `instance::teardown_instance_inner` for chests) — so a new loot-family table is
/// added HERE, once, and every teardown picks it up.
///
/// Distinct from [`purge_corpse_residue`], which sits next door and looks similar: that one runs
/// BEFORE a fresh kill roll and deliberately purges unconditionally (nothing on a live mob's guid can
/// be owned by a roll that matters). This one runs when the corpse itself goes away, and must respect
/// the withheld gate below.
///
/// Fix: a `game_corpse_loot` row still `withheld` is locked by a live NEED/GREED roll —
/// which, in a sharded deployment, may be authoritative on realm-core and invisible to this
/// database's own `game_loot_roll` table (promoted away within ~200ms of kill-time; see
/// `creatures::tick::pass_decay`'s matching fix). Reaping it here would delete the item out from
/// under a roll that has not resolved yet, so `settle_loot_roll`'s eventual grant finds nothing and
/// silently no-ops. Leave a withheld row behind — orphaned-but-safe, the same posture the
/// inventory-full fallback already uses for a row exclusively reserved for a winner. [server]
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

/// Insert `winners` as sequential `game_corpse_loot` rows on `corpse_guid`, starting at slot 0, each
/// freshly unclaimed (`reserved_for = 0`) and FFA (group-loot stamping happens AFTER this returns, in
/// `apply_group_loot_rules` — never at insert time). The ONE insert loop [`roll_creature_loot`] and
/// [`roll_pickpocket_loot`] used to each carry a copy of (the dedup).
fn insert_corpse_rows(ctx: &ReducerContext, corpse_guid: u64, winners: Vec<(u32, u32, bool)>) {
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
    insert_corpse_rows(ctx, corpse_guid, winners);
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
/// Rolled by `professions::skin_corpse` in place of the flat `LEATHER_ENTRY` shortcut; a `skin_loot_id`
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
/// `professions::apply_fish` once the caster's zone is resolved (`terrain::zone_id_at`, the work-item
/// 209/375 terrain-area one-hop-parent-walk idiom); an unresolved zone OR a zone with no rows (or a roll that
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
    insert_corpse_rows(ctx, target_guid, winners);
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

/// Clone a quest-only creature-loot row for the other corpse-eligible Characters who still need
/// it. The resolved eligibility set is the whole recipient ceiling: a current group lookup would
/// let a later joiner receive a clone.
pub(crate) fn clone_quest_loot_for_eligible(
    ctx: &ReducerContext,
    taker_guid: u64,
    corpse_guid: u64,
    item_entry: u32,
    count: u32,
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
        });
    }
}

/// Max distance to loot a corpse: (10 yd)². Generous — the vanilla client walks into interaction
/// range itself before sending `CMSG_LOOT_MONEY`, so this only rejects clearly-out-of-range abuse.
pub(crate) const LOOT_RANGE_SQ: f32 = 100.0;

/// A corpse is ready for skinning once no item row or money remains.
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

/// Open a creature corpse for the read that follows. This reducer core only authorizes the read;
/// it does not create durable loot-window state.
pub(crate) fn open_creature_corpse(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
) -> Result<(), String> {
    let actor = ctx
        .db
        .game_world_entity()
        .guid()
        .find(actor_guid)
        .ok_or_else(|| "looter not in world".to_string())?;
    if actor.dead {
        return Err("dead Characters cannot loot".to_string());
    }
    let corpse = ctx
        .db
        .game_world_entity()
        .guid()
        .find(corpse_guid)
        .ok_or_else(|| "no such corpse".to_string())?;
    if corpse.is_player() || !corpse.dead {
        return Err("target is not a creature corpse".to_string());
    }
    if corpse.map_id != actor.map_id || corpse.instance_id != actor.instance_id {
        return Err("corpse is out of reach".to_string());
    }
    let (dx, dy, dz) = (corpse.x - actor.x, corpse.y - actor.y, corpse.z - actor.z);
    if dx * dx + dy * dy + dz * dz > LOOT_RANGE_SQ {
        return Err("corpse is out of reach".to_string());
    }
    corpse_access_gate(ctx, actor_guid, corpse_guid)
}

/// Shared core: take corpse money by explicit looter guid, behind the `loot_money` reducer and
/// `actor::loot_money`. The Loot Tag Gate runs before the corpse purse changes.
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

    corpse_access_gate(ctx, looter_guid, target_guid)?;
    if corpse.money == 0 {
        return Err("nothing to loot".to_string());
    }

    let amount = corpse.money;
    corpse.money = 0;
    entities.guid().update(corpse);

    // Split across the resolved corpse-eligibility set, never a current party lookup. The access
    // Gate above makes an empty set a Refusal. A one-recipient set is solo and sends no
    // `MONEY_SHARE` event.
    let recipients = corpse_eligible_recipients(ctx, target_guid);
    let grouped = money_is_grouped(&recipients);
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
/// DEFER verdict for the in-transit case. This is the one audited by-guid path a REFUSAL
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

/// Split `total` copper evenly across the resolved corpse-eligibility set, with the remainder to
/// `looter_guid`. The access Gate ensures a real caller is in a non-empty set. The empty case stays
/// defined for this pure helper's callers and tests, but never authorizes money loot. Pure.
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

/// Money shares are a party action only when the corpse has more than one eligible Character.
pub(crate) fn money_is_grouped(recipients: &[u64]) -> bool {
    recipients.len() > 1
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

    #[test]
    fn only_multiple_eligible_characters_share_money() {
        assert!(!money_is_grouped(&[]));
        assert!(!money_is_grouped(&[7]));
        assert!(money_is_grouped(&[7, 8]));
    }

    use crate::test_scan::code_of;

    /// `purge_corpse_residue` is the ONE helper `kill_creature` calls to close the
    /// pickpocket/kill slot collision, and the issue explicitly asks for it to cover BOTH residue
    /// tables (`game_corpse_loot` — the actual colliding rows — and `game_corpse_loot_eligible`, the
    /// pre-existing work-item-267 reused-guid residue). Losing either `by_corpse().filter(&guid)`
    /// sweep silently reopens one of the two residue classes on a reused/live-then-killed guid.
    #[test]
    fn purge_corpse_residue_sweeps_both_corpse_loot_tables() {
        let body = code_of(
            include_str!("mod.rs"),
            "pub(crate) fn purge_corpse_residue(ctx: &ReducerContext, guid: u64) {",
        );
        assert!(
            body.contains("ctx.db.game_corpse_loot();") && body.contains("loot.by_corpse().filter(&guid)"),
            "`purge_corpse_residue` no longer sweeps `game_corpse_loot` by corpse guid — a leftover \
             pickpocket row can collide with a freshly-rolled kill-drop slot again (issue #358). \
             Body was:\n{body}"
        );
        assert!(
            body.contains("ctx.db.game_corpse_loot_eligible();")
                && body.contains("eligible.by_corpse().filter(&guid)"),
            "`purge_corpse_residue` no longer sweeps `game_corpse_loot_eligible` by corpse guid — \
             the work-item-267 reused-guid residue (a departed group inheriting a stale eligibility \
             snapshot) would come back. Body was:\n{body}"
        );
    }
}
