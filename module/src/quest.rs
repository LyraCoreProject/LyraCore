//! Quests (server core) — accept a quest from a giver NPC, track kill objectives as creatures die,
//! and turn the completed quest in for XP / money / item rewards. This is the SERVER-AUTHORITATIVE
//! engine; the gateway's quest-giver dialog codec (the `!`/`?` overhead icons, the `CMSG_QUESTGIVER_*`
//! opcodes) reads these tables to drive the client UI, but the quest-log tracking window itself is a
//! later gateway slice — every reducer here is driven + verified server-side (debug twins in `debug.rs`).
//!
//! Data model — a clean header + list shape (NOT cmangos's one fat row), so the runtime joins are
//! direct and the importer maps cmangos's flattened columns onto normalized rows:
//!
//! - [`QuestTemplate`]   — the quest header (level gate, title, money/XP reward). [static]
//! - [`QuestObjective`]  — a quest's objective list (today: kill N of a creature). [static]
//! - [`QuestRewardItem`] — a quest's guaranteed reward items. [static]
//! - [`CreatureQuest`]   — which creature STARTS (`!`) / ENDS (`?`) which quest (many-to-many). [static]
//! - [`CharacterQuest`]  — a character's live quest log: one row per accepted quest with per-objective
//!   progress counts, RLS-scoped to the owner. Born in `accept_quest`. [entity]
//!
//! Flow: `accept_quest(giver, quest)` validates the giver offers it + the player qualifies, then opens
//! a [`CharacterQuest`] row. Each creature kill calls [`on_creature_killed`] (from the combat
//! killing-blow path), which bumps the matching objective counts. `turn_in_quest(giver, quest)`
//! validates the giver ends it + every objective is met, grants the rewards (reusing
//! [`crate::xp::grant_xp`] + [`crate::items::grant_item`]), and marks the row rewarded (kept, to block
//! a repeat). Purely additive: brand-new tables + one hook call in `kill_creature`.

use spacetimedb::{
    client_visibility_filter, reducer, table, Filter, Identity, ReducerContext, Table,
};

use crate::game_gameobject; // GAMEOBJECT quest givers (e.g. Wanted Poster, Lost Guards corpses)
use crate::game_item_instance; // ITEM quest givers (work-item 194: item_template.start_quest)
use crate::game_item_template;
use crate::game_world_entity;
use crate::game_xp_event; // quest XP "+N experience" relay (non-kill SMSG_LOG_XPGAIN)
use crate::helpers::entity_by_owner;

/// Objective kinds (`QuestObjective.kind`). Only KILL ships today; the others are the documented
/// extension points the schema already accommodates (a new kind needs only a new arm in
/// [`on_creature_killed`]'s sibling hooks, no migration).
pub mod objective_kind {
    /// Kill `required_count` of creature `target_entry`. [kill]
    pub const KILL_CREATURE: u8 = 0;
    /// Hold `required_count` of ITEM `target_entry` — completion is a LIVE inventory count (no kill
    /// counter), consumed on turn-in. Covers cmangos quest ReqItemId/ReqItemCount. [collect]
    pub const COLLECT_ITEM: u8 = 1;
    /// Use `required_count` of GAMEOBJECT `target_entry` (a lever/totem/quest-object). Credited by
    /// [`on_gameobject_used`] from the CMSG_GAMEOBJ_USE path, mirroring KILL_CREATURE. [use]
    pub const USE_GAMEOBJECT: u8 = 2;
    /// Enter AreaTrigger `target_entry` (a zone the client detects, e.g. a mine for an "explore" quest).
    /// Credited by [`on_areatrigger_entered`] from the CMSG_AREATRIGGER path, mirroring USE_GAMEOBJECT.
    /// `required_count` is 1 (a single explore). [explore]
    pub const EXPLORE_AREATRIGGER: u8 = 3;
}

/// `CreatureQuest.role` — whether a creature is where you GET the quest or where you HAND IT IN. A
/// quest is offered by its START giver(s) (`!`) and completed at its END giver(s) (`?`); the two are
/// often the same NPC but need not be (cmangos `creature_questrelation` vs `creature_involvedrelation`).
pub mod quest_role {
    /// The creature OFFERS this quest (`!` overhead) — gates [`accept_quest`].
    pub const START: u8 = 0;
    /// The creature COMPLETES this quest (`?` overhead) — gates [`turn_in_quest`].
    pub const END: u8 = 1;
}

/// Max distance to talk to a quest giver: (10 yd)². The client walks into range before sending the
/// `CMSG_QUESTGIVER_*` op, so this only rejects clearly-out-of-range abuse (mirrors `loot`/vendor range).
const QUEST_GIVER_RANGE_SQ: f32 = 100.0;

/// Party quest-SHARE range (work-item 194): reuses the group system's own kill-reward range
/// (`crate::group::GROUP_XP_RANGE_SQ`, vanilla's 74yd `CONFIG_FLOAT_GROUP_XP_DISTANCE`) rather than
/// the tight NPC-interaction [`QUEST_GIVER_RANGE_SQ`] (10yd) — sharing a quest is a party-visibility
/// action (no walking up to a person), the same range the game already treats as "close enough to
/// act together" for kill-XP splitting.
const PARTY_SHARE_RANGE_SQ: f32 = crate::group::GROUP_XP_RANGE_SQ;

/// Vanilla quest-log cap (`push_quest_to_party`'s LOG_FULL gate — a share target already holding 20
/// active quests can't accept a 21st). Matches the gateway's `player_quest_log` slot cap
/// (`gateway/src/stdb/reads.rs`) — both sides model the same 20-slot vanilla quest log.
const MAX_QUEST_LOG_SIZE: usize = 20;

/// Vanilla level cap (quest XP→money conversion gate, work-item 194(e)) — matches `xp::xp_to_next_level`'s
/// own hardcoded cap check (`level >= 60` there too; not re-exported as a shared const, so this is a
/// second literal by necessity — pinned by a test alongside `xp::xp_to_next_level`'s own cap tests).
const QUEST_MAX_LEVEL_PAYOUT: u32 = 60;

// ===========================================================================================
//  Static quest definition [static] — public, read-only, SQL-loadable (no Timestamp)
// ===========================================================================================

/// A quest definition (the cmangos `quest_template` analogue, deduped to what the engine needs).
/// Public + read-only, no Timestamp → loadable by the importer via plain SQL. `reward_xp == 0` means
/// "compute from `quest_level`" (`lyracore_shared::quest::xp_reward`); a non-zero value overrides it. [static]
#[table(accessor = game_quest_template, public)]
pub struct QuestTemplate {
    #[primary_key]
    pub entry: u32,
    pub min_level: u32,   // the character level required to ACCEPT (0 = no gate)
    pub quest_level: u32, // the quest's level — drives the computed XP reward
    pub title: String,
    pub reward_money: u32, // copper granted on turn-in (cmangos RewOrReqMoney, clamped ≥ 0)
    pub reward_xp: u32,    // explicit XP reward; 0 = compute (lyracore_shared::quest::xp_reward)
    // --- availability gates (END-appended, #[default(0)] → additive auto-migration) ---
    // Without these, EVERY quest a giver lists shows as available (the `!`-spam bug): a chain quest,
    // a wrong-race/class quest, all appear. `prev_quest_id` is the cmangos PrevQuestId — the quest that
    // must be turned in (rewarded) before this one unlocks (0 = no prerequisite). `required_races` /
    // `required_classes` are cmangos bitmasks (0 = all) checked via lyracore_shared::quest::{race,class}_allowed.
    #[default(0)]
    pub prev_quest_id: u32,
    #[default(0)]
    pub required_races: u32,
    #[default(0)]
    pub required_classes: u32,
    // cmangos quest_template.ZoneOrSort — drives the quest-log CATEGORY header (positive = AreaTable
    // zone id, negative = a QuestSort id). 0 → the client shows "Missing header! (quest designers)".
    // i32 to carry the sign. [default(0)] + end-appended → additive auto-migration.
    #[default(0)]
    pub zone_or_sort: i32,
    // --- reputation rewards (END-appended, #[default(0)] → additive auto-migration). cmangos
    // RewRepFaction1/2 + RewRepValue1/2: turn-in grants `value_n` rep with `faction_n` (0 faction = none).
    // INERT until a re-import populates them (the importer parses these; existing rows default 0, so no
    // re-import is forced). Granted via reputation::grant_reputation, which itself no-ops on a no-rep-bar
    // faction. [static]
    #[default(0)]
    pub rew_rep_faction_1: u32,
    #[default(0)]
    pub rew_rep_value_1: i32,
    #[default(0)]
    pub rew_rep_faction_2: u32,
    #[default(0)]
    pub rew_rep_value_2: i32,
    // --- provided "source" item (cmangos SrcItemId/SrcItemCount): the item HANDED to the player on
    // accept (apply_accept_quest grants it via grant_item), satisfying a COLLECT objective whose
    // ReqItemId == SrcItemId — delivery quests (McBride class letters 3100-3105, Report to Goldshire,
    // Return to Kristoff, …). END-appended + #[default(0)] → additive auto-migration. A dump
    // src_item_count of 0 means "1" (cmangos treats 0 as 1; grant_item floors at 1). INERT until a
    // re-import populates it (existing rows default to 0 = no provided item). [static]
    #[default(0)]
    pub src_item: u32,
    #[default(0)]
    pub src_item_count: u32,
    // Repeatable (cmangos SpecialFlags bit0, QUEST_SPECIAL_FLAGS_REPEATABLE): a repeatable quest can be
    // accepted again immediately after turn-in (Elwynn's 16/7677/7678/3861 are all repeatable-flagged).
    // END-appended + #[default(false)] → additive auto-migration; existing rows default to non-repeatable
    // (unchanged behavior) until a re-import populates this from SpecialFlags&1. apply_turn_in_quest marks
    // the CharacterQuest row rewarded=true like any other quest (NEVER deletes it — that would erase the
    // only record the quest was ever completed, permanently breaking any OTHER quest whose prev_quest_id
    // points at this one). apply_accept_quest instead special-cases the duplicate guard: a rewarded row
    // for a repeatable quest is reset in place (counts zeroed, rewarded=false) rather than rejected.
    #[default(false)]
    pub repeatable: bool,
    // --- forward chains + timed quests (work-item 194, END-appended, #[default(0)] → additive
    // auto-migration) ---
    // cmangos NextQuestId/NextQuestInChain: the successor quest auto-offered on this quest's turn-in
    // (0 = no successor). `prev_quest_id`/auto_finish already do the ACCEPT-side gating + the
    // "arm the accept button" wire bit (module/src/quest.rs's prereq check, gateway/src/codec/quest.rs's
    // hardcoded `auto_finish: true`) — this column is the missing TURN-IN-side piece: threaded into
    // `QuestDetailView.next_quest_id` and `SMSG_QUEST_QUERY_RESPONSE`'s `next_quest_in_chain` field
    // (previously a hardcoded 0 stub) so the client's own "Accept this quest?" chain-continuation
    // popup fires on complete. INERT until a re-import populates it (existing rows default 0).
    #[default(0)]
    pub next_quest_id: u32,
    // cmangos LimitTime (seconds from ACCEPT, 0 = untimed). `apply_accept_quest` stamps
    // `CharacterQuest.deadline_micros` from this; the `quest_timer_pass` tick fails the quest at
    // expiry. INERT until a re-import populates it (existing rows default 0 = untimed, unchanged
    // behavior).
    #[default(0)]
    pub limit_time: u32,
    // cmangos RewMoneyMaxLevel — the per-quest field vanilla's `Quest::XPValue` divides by 0.6 to get
    // the authentic quest XP (see `lyracore_shared::quest::quest_xp`). The old `xp_reward` placeholder gave
    // ~half this. INERT until a re-import populates it (existing rows default 0 → the award falls back
    // to `xp_reward(quest_level)`, i.e. today's behavior, so a publish-before-reimport never zeroes XP).
    #[default(0)]
    pub reward_money_max_level: u32,
}

/// The free-text body of a quest (cmangos Details / Objectives / OfferRewardText / RequestItemsText),
/// kept in a SIDE table (not on [`QuestTemplate`]) for two reasons: the long text stays out of the hot
/// template row, and a NEW table always auto-migrates (sidestepping the String-column default-migration
/// question). 1:1 with `QuestTemplate` by entry. Public + read-only, no Timestamp → SQL-loadable. The
/// gateway reads it to fill the quest detail / offer-reward / request-items dialogs. Vanilla quest
/// text embeds `$B` (line break) and `$N`/`$C`/`$R`/`$G`
/// (name/class/race/gender) tokens — stored verbatim for the client to interpret. [static]
#[table(accessor = game_quest_text, public)]
pub struct QuestText {
    #[primary_key]
    pub quest_entry: u32,
    pub details: String,
    pub objectives: String,
    pub offer_reward_text: String,
    pub request_items_text: String,
}

/// One objective of a quest (today: kill `required_count` of creature `target_entry`). A quest's rows
/// are ordered by `obj_index` (0-based, ≤ [`MAX_OBJECTIVES`]-1), which positionally indexes the
/// progress `counts` on the player's [`CharacterQuest`]. Public + read-only, SQL-loadable. [static]
#[table(accessor = game_quest_objective, public, index(accessor = by_quest, btree(columns = [quest_entry])))]
pub struct QuestObjective {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub quest_entry: u32,    // -> QuestTemplate.entry
    pub obj_index: u8,       // 0-based slot within the quest (indexes CharacterQuest.counts)
    pub kind: u8,            // objective_kind::* (KILL_CREATURE today)
    pub target_entry: u32,   // the creature entry to kill (kind == KILL_CREATURE)
    pub required_count: u32, // how many
}

/// A guaranteed reward item granted on turn-in (cmangos RewItemId/RewItemCount), granted alongside
/// any [`QuestRewardChoice`] pick. Public + read-only, SQL-loadable. [static]
#[table(accessor = game_quest_reward_item, public, index(accessor = by_quest, btree(columns = [quest_entry])))]
pub struct QuestRewardItem {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub quest_entry: u32, // -> QuestTemplate.entry
    pub item_entry: u32,  // -> game_item_template.entry
    pub count: u32,
}

/// A CHOICE reward item (pick-1-of-N) granted on turn-in (cmangos RewChoiceItemId/Count1-6). The player
/// picks ONE row by `choice_index`; that single item is granted IN ADDITION to every [`QuestRewardItem`].
/// `choice_index` is 0-based and matches the wire order of the offer/details `choice_item_rewards` array
/// (the client sends the picked index back as `CMSG_QUESTGIVER_CHOOSE_REWARD.reward`). Pinning the index
/// in the row means a re-import that reorders rows can't desync the index the client sends. Public,
/// SQL-loadable. [static]
#[table(accessor = game_quest_reward_choice, public, index(accessor = by_quest, btree(columns = [quest_entry])))]
pub struct QuestRewardChoice {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub quest_entry: u32, // -> QuestTemplate.entry
    pub choice_index: u8, // 0..=5, matches packet order / the CMSG `reward` index
    pub item_entry: u32,  // -> game_item_template.entry
    pub count: u32,
}

/// Which creature STARTS / ENDS which quest (a many-to-many: an NPC offers several quests; a quest can
/// be offered by several NPCs). `role` is [`quest_role::START`] or [`quest_role::END`]. Gates accept /
/// turn-in to a giver in range, and drives the gateway's `!`/`?` overhead icons. Loaded from cmangos
/// `creature_questrelation` (START) + `creature_involvedrelation` (END). SQL-loadable. [static]
#[table(accessor = game_creature_quest, public, index(accessor = by_creature, btree(columns = [creature_entry])))]
pub struct CreatureQuest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32, // -> game_creature_template.entry
    pub quest_entry: u32,    // -> QuestTemplate.entry
    pub role: u8,            // quest_role::START | quest_role::END
}

/// Which GAMEOBJECT STARTS / ENDS which quest — the GO twin of [`CreatureQuest`], needed because some
/// quests have no creature giver at all: GO 68 "Wanted Poster" starts q176 Wanted: Hogger with NO
/// creature start giver, and GO 55/56 "Lost Guards" corpses drive the q37/q45/q71 chain. Same
/// many-to-many / `role` shape, keyed on `go_entry` (-> [`crate::gameobject::GameObjectTemplate`].entry)
/// instead of a creature entry.
/// Loaded from cmangos `gameobject_questrelation` (START) + `gameobject_involvedrelation` (END).
/// A brand-new table — additive auto-migration, zero risk to the existing creature-giver path.
/// SQL-loadable. [static]
#[table(accessor = game_gameobject_quest, public, index(accessor = by_gameobject, btree(columns = [go_entry])))]
pub struct GameObjectQuest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub go_entry: u32,    // -> crate::gameobject::GameObjectTemplate.entry
    pub quest_entry: u32, // -> QuestTemplate.entry
    pub role: u8,         // quest_role::START | quest_role::END
}

// ===========================================================================================
//  Per-character quest log [entity] — public, RLS-restricted to the owner
// ===========================================================================================

/// A character's accepted quest — one row per quest in the log, carrying per-objective progress in
/// `counts` (parallel to the quest's [`QuestObjective`] rows by `obj_index`). Born in [`accept_quest`];
/// kept after turn-in with `rewarded = true` so a non-repeatable quest can't be re-done. RLS-scoped to
/// the owner like `game_character`/`game_item_instance`. `character_guid` is the player's world-entity
/// guid (== the character guid), so [`on_creature_killed`] can find a killer's quests directly. [entity]
#[table(
    accessor = game_character_quest,
    public,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct CharacterQuest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity, // RLS scope: the connection that owns this quest log entry
    pub quest_entry: u32,         // -> QuestTemplate.entry
    pub counts: Vec<u32>, // per-objective kill progress, indexed by QuestObjective.obj_index
    pub rewarded: bool,   // true once turned in — the row stays to block a repeat
    // --- timed quests (work-item 194, END-appended, #[default(...)] → additive auto-migration) ---
    // Micros-since-epoch deadline stamped by `apply_accept_quest` from `QuestTemplate.limit_time`
    // (`ctx.timestamp` micros + `limit_time * 1_000_000`); 0 = untimed (the vast majority of quests —
    // existing rows default to this, unchanged behavior). Cleared back to 0 the instant the quest
    // fails (`quest_timer_pass`) — a failed row's deadline is meaningless past that point.
    #[default(0i64)]
    pub deadline_micros: i64,
    // Set by `quest_timer_pass` when `deadline_micros` passes un-rewarded. A failed quest cannot be
    // turned in (`apply_turn_in_quest` rejects it) but CAN be re-accepted — `apply_accept_quest`'s
    // duplicate guard treats an existing `failed` row like a repeatable-rewarded one: reset in place
    // (counts zeroed, `failed`/`rewarded` cleared, a fresh deadline stamped), never a second row.
    #[default(false)]
    pub failed: bool,
}

/// A connection sees only its own quest log (mirrors the character / item RLS filters).
#[client_visibility_filter]
const CHARACTER_QUEST_RLS: Filter =
    Filter::Sql("SELECT * FROM game_character_quest WHERE owner_identity = :sender");

// Character-owned sweeps: the quest log is deleted on character delete, re-owned (identity
// re-stamp) on a relog under a changed gateway identity.
crate::character_owned!(delete, fn sweep_delete_game_character_quest(ctx, character_guid) {
    let quests = ctx.db.game_character_quest();
    for r in quests.by_character().filter(&character_guid).collect::<Vec<_>>() {
        quests.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19): the quest log IS the run — a Deadmines party arriving with
// an empty log could not turn anything in. `id` is a surrogate PK, re-minted.
crate::character_owned!(transfer, fn sweep_transfer_game_character_quest(ctx, character_guid, io) {
    crate::transfer::move_rows(
        ctx,
        io,
        || ctx.db.game_character_quest().by_character().filter(&character_guid).collect::<Vec<_>>(),
        |ctx, mut r| {
            r.id = 0;
            ctx.db.game_character_quest().insert(r);
        },
    );
});
crate::character_owned!(restamp, fn sweep_restamp_game_character_quest(ctx, character_guid, identity) {
    let quests = ctx.db.game_character_quest();
    for mut r in quests.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            quests.id().update(r);
        }
    }
});

// ===========================================================================================
//  Pure helpers
// ===========================================================================================

/// Hard cap on a quest's objectives — vanilla's quest log shows 4, and the progress `counts` vector is
/// sized to the quest's objective count (≤ this). Objectives past this are ignored at accept time.
pub const MAX_OBJECTIVES: usize = 4;

/// Is `cq` complete — has every objective of its quest reached its `required_count`? Joins the quest's
/// [`QuestObjective`] rows and compares each against `counts[obj_index]` (a missing/short count = not
/// met). A quest with NO objectives is trivially complete (a pure talk-to-giver quest). Used by
/// [`turn_in_quest`] and surfaced for tests.
fn quest_is_complete(ctx: &ReducerContext, cq: &CharacterQuest) -> bool {
    ctx.db
        .game_quest_objective()
        .by_quest()
        .filter(&cq.quest_entry)
        .all(|obj| {
            // KILL objectives read the per-objective kill counter; COLLECT objectives read a LIVE
            // inventory count of the required item — no counter, completion follows the bag.
            let have = match obj.kind {
                objective_kind::COLLECT_ITEM => {
                    crate::items::item_count(ctx, cq.character_guid, obj.target_entry)
                }
                _ => cq.counts.get(obj.obj_index as usize).copied().unwrap_or(0),
            };
            have >= obj.required_count
        })
}

/// Does creature `creature_entry` have the `role` relation to `quest_entry` (i.e. is it a valid giver
/// for this accept / turn-in)? Scans `game_creature_quest` by creature.
fn creature_has_quest_role(
    ctx: &ReducerContext,
    creature_entry: u32,
    quest_entry: u32,
    role: u8,
) -> bool {
    ctx.db
        .game_creature_quest()
        .by_creature()
        .filter(&creature_entry)
        .any(|r| r.quest_entry == quest_entry && r.role == role)
}

/// Does GAMEOBJECT template `go_entry` have the `role` relation to `quest_entry`? The [`GameObjectQuest`]
/// twin of [`creature_has_quest_role`] — scans `game_gameobject_quest` by GO template entry.
fn gameobject_has_quest_role(
    ctx: &ReducerContext,
    go_entry: u32,
    quest_entry: u32,
    role: u8,
) -> bool {
    ctx.db
        .game_gameobject_quest()
        .by_gameobject()
        .filter(&go_entry)
        .any(|r| r.quest_entry == quest_entry && r.role == role)
}

/// The character's existing quest-log row for `quest_entry`, if any (a point scan over the character's
/// rows — the log is small). `Some(rewarded == false)` = active; `Some(true)` = already done.
fn character_quest_row(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Option<CharacterQuest> {
    ctx.db
        .game_character_quest()
        .by_character()
        .filter(&character_guid)
        .find(|q| q.quest_entry == quest_entry)
}

/// Has the character turned in (been rewarded for) `quest_entry`? The prerequisite-chain test for
/// [`apply_accept_quest`] — a `prev_quest_id` is satisfied only by a `rewarded` row, not merely an
/// accepted one.
pub(crate) fn quest_is_rewarded(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> bool {
    character_quest_row(ctx, character_guid, quest_entry).is_some_and(|q| q.rewarded)
}

/// A validated quest giver's identity: which `game_*_quest` relation table + template entry
/// [`creature_has_quest_role`] / [`gameobject_has_quest_role`] should check. Only the entry + kind
/// survive validation — accept/turn-in never touch the giver's live position again.
enum GiverKind {
    /// A live creature entity (`game_world_entity`) — checked against `game_creature_quest`.
    Creature(u32),
    /// A spawned gameobject (`game_gameobject`) — checked against `game_gameobject_quest`
    /// (some quests, like GO 68 "Wanted Poster", have no creature giver at all).
    GameObject(u32),
    /// Work-item 194 (item-starts-quest): a USED item whose template names `start_quest`
    /// (cmangos `item_template.startquest`, the "dropped tabard starts a quest" pattern) — carries the
    /// item's TEMPLATE entry (checked against `ItemTemplate.start_quest`, never a `game_*_quest`
    /// relation table). The gateway sends the item's OWN instance guid as the "giver" guid
    /// (`CMSG_USE_ITEM` → `SMSG_QUESTGIVER_QUEST_DETAILS`); `validate_giver` resolves it back against
    /// the PLAYER'S OWN `game_item_instance` row (never someone else's bag). Satisfies START only.
    Item(u32),
    /// Work-item 194 (sharing): a live PLAYER entity that shared this quest via `push_quest_to_party`
    /// — carries the SHARER's character guid. Re-validated FRESH here every time (grouped + in range,
    /// via [`validate_party_giver`]) — a spoofed/stale giver_guid can never self-authorize, and
    /// [`giver_has_quest_role`] additionally requires the sharer to be CURRENTLY, ACTIVELY on the
    /// quest. Satisfies START only, never END.
    Party(u64),
}

/// A live quest giver `giver_guid` validated for `player`: it must exist (a creature OR a
/// gameobject, never a player), be on the same map, and be in range. Returns the giver's
/// [`GiverKind`] (its template entry, tagged by kind) or an `Err` string. Shared by accept +
/// turn-in so the giver gate is identical (mirrors the vendor gate in `items::ops`).
fn validate_giver(
    ctx: &ReducerContext,
    player: &crate::WorldEntity,
    giver_guid: u64,
) -> Result<GiverKind, String> {
    if let Some(giver) = ctx.db.game_world_entity().guid().find(giver_guid) {
        if giver.is_player() {
            // Work-item 194 (sharing): the ONLY case a live PLAYER entity is a valid giver — a
            // party-share re-validated fresh (never trusts the earlier push_quest_to_party call).
            return validate_party_giver(ctx, player, giver_guid, &giver);
        }
        if giver.map_id != player.map_id || giver.instance_id != player.instance_id {
            return Err("quest giver on another map".to_string());
        }
        let (dx, dy, dz) = (giver.x - player.x, giver.y - player.y, giver.z - player.z);
        if dx * dx + dy * dy + dz * dz > QUEST_GIVER_RANGE_SQ {
            return Err("quest giver out of range".to_string());
        }
        return Ok(GiverKind::Creature(giver.entry));
    }
    // Not a live creature/player entity — fall back to a GAMEOBJECT quest giver: GO 68 "Wanted
    // Poster" starts q176 Wanted: Hogger (no creature start giver at all), and the GO 55/56 "Lost
    // Guards" corpses drive the q37/q45/q71 chain. Same map + range gate.
    if let Some(go) = ctx.db.game_gameobject().guid().find(giver_guid) {
        // GameObject giver: map + instance gated (190 slice 2 — GO rows carry `instance_id` now;
        // dungeon GOs are per-instance copies, so a giver in another party's Deadmines can never
        // serve this player).
        if go.map_id != player.map_id || go.instance_id != player.instance_id {
            return Err("quest giver on another map".to_string());
        }
        let (dx, dy, dz) = (go.x - player.x, go.y - player.y, go.z - player.z);
        if dx * dx + dy * dy + dz * dz > QUEST_GIVER_RANGE_SQ {
            return Err("quest giver out of range".to_string());
        }
        return Ok(GiverKind::GameObject(go.template_entry));
    }
    // Work-item 194 (item-starts-quest): not a live creature/GO/player entity — try the PLAYER'S
    // OWN item instance (never someone else's bag; a used item's instance guid is what the gateway
    // sends as "giver" for CMSG_USE_ITEM's start_quest path). Tried LAST — the common paths above are
    // cheap and never collide with an item's HIGHGUID_ITEM-tagged guid space anyway.
    ctx.db
        .game_item_instance()
        .guid()
        .find(giver_guid)
        .filter(|i| i.owner_guid == player.guid)
        .map(|i| GiverKind::Item(i.entry))
        .ok_or_else(|| "no such quest giver".to_string())
}

/// Work-item 194 (sharing): validates a live PLAYER `giver_guid` as a party-share giver for `player`
/// — grouped with the accepting player (the SAME group id), on the same map/instance, within
/// [`PARTY_SHARE_RANGE_SQ`]. Every field is re-read LIVE here — never trusts that
/// `push_quest_to_party` ran, so a spoofed/stale giver_guid that no longer names a real grouped,
/// in-range peer is rejected exactly like a fabricated creature guid would be.
/// [`giver_has_quest_role`] (not this fn) separately checks the sharer is actually ON the quest now.
fn validate_party_giver(
    ctx: &ReducerContext,
    player: &crate::WorldEntity,
    giver_guid: u64,
    giver: &crate::WorldEntity,
) -> Result<GiverKind, String> {
    let same_group = match (
        crate::group::group_of(ctx, player.guid),
        crate::group::group_of(ctx, giver_guid),
    ) {
        (Some(a), Some(b)) => a.group_id == b.group_id,
        _ => false,
    };
    if !same_group {
        return Err("target is not a quest giver".to_string());
    }
    if giver.map_id != player.map_id || giver.instance_id != player.instance_id {
        return Err("quest giver on another map".to_string());
    }
    let (dx, dy, dz) = (giver.x - player.x, giver.y - player.y, giver.z - player.z);
    if dx * dx + dy * dy + dz * dz > PARTY_SHARE_RANGE_SQ {
        return Err("quest giver out of range".to_string());
    }
    Ok(GiverKind::Party(giver_guid))
}

/// Does `giver` (a validated [`GiverKind`]) have the `role` relation to `quest_entry`? Dispatches to
/// [`creature_has_quest_role`] / [`gameobject_has_quest_role`] by kind — the single call site
/// `apply_accept_quest` / `apply_turn_in_quest` use so neither has to match on `GiverKind` itself.
fn giver_has_quest_role(
    ctx: &ReducerContext,
    giver: &GiverKind,
    quest_entry: u32,
    role: u8,
) -> bool {
    match *giver {
        GiverKind::Creature(entry) => creature_has_quest_role(ctx, entry, quest_entry, role),
        GiverKind::GameObject(entry) => gameobject_has_quest_role(ctx, entry, quest_entry, role),
        // Item-starts-quest (work-item 194): satisfies START iff the item's OWN template names this
        // quest — never END (an item never completes a quest).
        GiverKind::Item(entry) => {
            role == quest_role::START
                && ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(entry)
                    .is_some_and(|t| t.start_quest == quest_entry)
        }
        // Party share (work-item 194): satisfies START iff the sharer is CURRENTLY, ACTIVELY on the
        // quest (an un-rewarded, un-failed row) — never END. This is the hard gate against a spoofed
        // giver_guid self-authorizing: even a genuinely grouped, in-range peer can't hand out a quest
        // they aren't actually holding right now.
        GiverKind::Party(sharer_guid) => {
            role == quest_role::START
                && character_quest_row(ctx, sharer_guid, quest_entry)
                    .is_some_and(|q| !q.rewarded && !q.failed)
        }
    }
}

// ===========================================================================================
//  Mutation cores (shared by the player reducers + their debug twins)
// ===========================================================================================

/// Accept quest `quest_entry` from giver `giver_guid` for `player_guid` — the shared core behind the
/// player `accept_quest` reducer and its debug twin. Validates: the player is in world + alive; the
/// giver is a real creature in range that OFFERS the quest; the quest exists and the player meets its
/// `min_level`; and the player doesn't already have it (active or already rewarded — non-repeatable).
/// On success opens a [`CharacterQuest`] row with zeroed `counts` sized to the quest's objective count.
/// Additive — inserts one quest-log row. [entity]
pub(crate) fn apply_accept_quest(
    ctx: &ReducerContext,
    player_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(player_guid)
        .ok_or_else(|| "player not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot accept quests".to_string());
    }
    let giver = validate_giver(ctx, &player, giver_guid)?;
    if !giver_has_quest_role(ctx, &giver, quest_entry, quest_role::START) {
        return Err("that quest giver does not offer this quest".to_string());
    }
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;
    if player.level < tmpl.min_level {
        return Err(format!("requires level {}", tmpl.min_level));
    }
    // The gates below (level/race/class/prereq/not-held) MUST stay in lockstep with the
    // gateway's `!`-status `startable` (gateway/src/stdb/reads.rs `eval_relation`), or the icon and
    // this reducer disagree. Race/class gates (cmangos bitmasks; 0 = all). Race/class live packed in
    // unit_bytes_0 (race | class<<8 | gender<<16 | power<<24) — the same source the XP curve reads.
    let race = player.race();
    let class = player.class();
    if !lyracore_shared::quest::race_allowed(tmpl.required_races, race) {
        return Err("quest not available to your race".to_string());
    }
    if !lyracore_shared::quest::class_allowed(tmpl.required_classes, class) {
        return Err("quest not available to your class".to_string());
    }
    // Prerequisite chain: the previous quest in the chain must be turned in (rewarded) first. This is
    // what stops a fresh character from accepting a mid-chain quest (the `!`-over-everything bug).
    if tmpl.prev_quest_id != 0 && !quest_is_rewarded(ctx, player_guid, tmpl.prev_quest_id) {
        return Err("must complete the prerequisite quest first".to_string());
    }
    // A repeatable quest already turned in (rewarded=true) is RESET in place rather than rejected as a
    // duplicate — this is the ONLY way a repeatable quest is ever re-taken. The row is UPDATED, never
    // deleted, so a re-accept only ever flips `rewarded` back to false for the DURATION of the new attempt
    // (exactly like any other active quest); it never erases the fact the quest was completed before, and
    // apply_turn_in_quest below always marks it rewarded again on the next turn-in. Any other existing row
    // (still active, or a non-repeatable already-rewarded row) is a hard duplicate.
    // Work-item 194 (timed quests): an existing FAILED row (expired past its deadline) is re-acceptable
    // exactly like a repeatable-rewarded one — reset in place, never a hard duplicate. This is
    // independent of `repeatable`: ANY timed quest can be retried after it fails, not just repeatable
    // ones (vanilla: a failed escort/timed quest can always be picked up again).
    if let Some(existing) = character_quest_row(ctx, player_guid, quest_entry) {
        if !(existing.failed || (tmpl.repeatable && existing.rewarded)) {
            return Err("already on or completed that quest".to_string());
        }
    }
    // Size the progress vector to the quest's objective count (capped) so each objective's obj_index
    // is a valid slot; a quest with no objectives gets an empty vector (trivially complete).
    // Provided "source" item (cmangos SrcItemId): handed to the player on accept. For a DELIVERY quest
    // (SrcItemId == ReqItemId) this is what makes the COLLECT objective satisfiable — the item is now in
    // bags, so quest_is_complete's live item_count picks it up. Granted on every accept the duplicate guard
    // above lets through: once for a first-time accept, and once more on each repeatable re-accept (the
    // repeatable-turn-in path consumes/keeps the item like any other reward, so re-supplying it on re-accept
    // is correct — the same rule vanilla applies). `?` rolls the whole accept back on a full backpack — the quest is NOT
    // accepted if its item can't be delivered (you can't carry the letter). count 0 means 1; grant_item
    // already floors at 1.
    if tmpl.src_item != 0 {
        crate::items::grant_item(ctx, player_guid, tmpl.src_item, tmpl.src_item_count)?;
    }
    let num_objectives = ctx
        .db
        .game_quest_objective()
        .by_quest()
        .filter(&quest_entry)
        .count()
        .min(MAX_OBJECTIVES);
    let counts = vec![0u32; num_objectives];
    // Timed quests (work-item 194): a `limit_time > 0` template stamps a deadline (accept-time
    // ctx.timestamp micros + limit_time seconds, as micros); 0 = untimed (the vast majority).
    let deadline_micros = if tmpl.limit_time > 0 {
        ctx.timestamp.to_micros_since_unix_epoch() + (tmpl.limit_time as i64) * 1_000_000
    } else {
        0
    };
    match character_quest_row(ctx, player_guid, quest_entry) {
        // Re-accepting a repeatable-rewarded OR failed quest: reset the SAME row (id kept) rather than
        // inserting a second one — this is what the duplicate guard above allows through, and keeping
        // one row per (character, quest) is what `character_quest_row`'s point-scan-by-quest_entry
        // assumes everywhere else.
        Some(mut existing) => {
            existing.counts = counts;
            existing.rewarded = false;
            existing.failed = false;
            existing.deadline_micros = deadline_micros;
            ctx.db.game_character_quest().id().update(existing);
        }
        None => {
            ctx.db.game_character_quest().insert(CharacterQuest {
                id: 0,
                character_guid: player_guid,
                owner_identity: player.owner_identity,
                quest_entry,
                counts,
                rewarded: false,
                deadline_micros,
                failed: false,
            });
        }
    }
    // Notify-hook: the quest landed in the log (real accept — all gates passed).
    crate::hooks::fire_on_quest_accept(
        ctx,
        &crate::hooks::QuestAcceptPayload {
            character_guid: player_guid,
            quest_entry,
        },
    );
    Ok(())
}

/// Grant `quest_entry` to `character_guid` WITHOUT a giver — the test path behind [`debug_grant_quest`].
/// Inserts the SAME CharacterQuest row `apply_accept_quest` does (counts sized to the objective count,
/// rewarded=false) but SKIPS every accept gate (giver range/role, level, race/class, prereq) so a harness
/// can stage a quest by guid alone — the real accept needs the giver's creature guid, which the CLI
/// `spacetime call` mangles (u64 > 2^53). Keeps the duplicate guard. owner_identity comes from the live
/// entity (the player is in-world when staging). [entity]
// Sole consumer today is the feature-gated harness (via `actor::stage_quest`); a default build
// compiles debug.rs out, so the lint is silenced ONLY there — a debug_reducers build still flags
// this dead if the harness stops staging quests.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn grant_quest_unchecked(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;
    if character_quest_row(ctx, character_guid, quest_entry).is_some() {
        return Err("already on or completed that quest".to_string());
    }
    // Provided source item, same as apply_accept_quest — granted once (duplicate guard above gates
    // re-accept/relog), so the harness can stage a delivery quest with its item already in bags.
    if tmpl.src_item != 0 {
        crate::items::grant_item(ctx, character_guid, tmpl.src_item, tmpl.src_item_count)?;
    }
    let owner_identity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .map(|e| e.owner_identity)
        .ok_or_else(|| format!("character {character_guid} not in world"))?;
    let num_objectives = ctx
        .db
        .game_quest_objective()
        .by_quest()
        .filter(&quest_entry)
        .count()
        .min(MAX_OBJECTIVES);
    ctx.db.game_character_quest().insert(CharacterQuest {
        id: 0,
        character_guid,
        owner_identity,
        quest_entry,
        counts: vec![0u32; num_objectives],
        rewarded: false,
        deadline_micros: 0, // harness staging skips the timer — same as an untimed accept
        failed: false,
    });
    // Notify-hook: same event as the real accept — a consumer sees one "quest in the log"
    // signal regardless of which path staged it (the row shapes are identical).
    crate::hooks::fire_on_quest_accept(
        ctx,
        &crate::hooks::QuestAcceptPayload {
            character_guid,
            quest_entry,
        },
    );
    Ok(())
}

/// Pure choice-reward pick (testable without a live `ReducerContext`): given a quest's choice rows as
/// `(choice_index, item_entry, count)` and the player's picked `reward_index`, decide what to grant.
/// `Ok(None)` when the quest has NO choice rows (the index is ignored — a guaranteed-only quest);
/// `Ok(Some((item, count)))` for the row whose `choice_index == reward_index`; `Err` when choices exist
/// but none match the index — REJECT (the client must pick a valid slot), never silently grant nothing
/// or the wrong item.
fn pick_choice_reward(
    choices: &[(u8, u32, u32)],
    reward_index: u32,
    quest_entry: u32,
) -> Result<Option<(u32, u32)>, String> {
    if choices.is_empty() {
        return Ok(None);
    }
    choices
        .iter()
        .find(|(idx, _, _)| *idx as u32 == reward_index)
        .map(|&(_, item, count)| Some((item, count)))
        .ok_or_else(|| format!("invalid reward choice {reward_index} for quest {quest_entry}"))
}

/// Turn quest `quest_entry` in to giver `giver_guid` for `player_guid` — the shared core behind the
/// player `turn_in_quest` reducer and its debug twin. Validates: the player is in world + alive; the
/// giver is a real creature in range that ENDS the quest; the player has it ACTIVE (a row, not yet
/// rewarded); and every objective is complete. Then grants the rewards atomically — reward ITEMS first
/// (the only step that can fail, on a full backpack, rolling the whole tx back so nothing is granted
/// without the items landing), then money + XP — and marks the row `rewarded` (kept, to block a
/// repeat). Reuses [`crate::items::grant_item`] + [`crate::xp::grant_xp`]. [entity]
pub(crate) fn apply_turn_in_quest(
    ctx: &ReducerContext,
    player_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
    reward_index: u32,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "player not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot turn in quests".to_string());
    }
    let giver = validate_giver(ctx, &player, giver_guid)?;
    if !giver_has_quest_role(ctx, &giver, quest_entry, quest_role::END) {
        return Err("that quest giver does not complete this quest".to_string());
    }
    let mut cq = character_quest_row(ctx, player_guid, quest_entry)
        .ok_or_else(|| "you are not on that quest".to_string())?;
    if cq.rewarded {
        return Err("quest already turned in".to_string());
    }
    // Work-item 194 (timed quests): a FAILED (expired) quest can't be turned in — it must be
    // re-accepted first (the duplicate guard in `apply_accept_quest` lets that through).
    if cq.failed {
        return Err("quest has expired".to_string());
    }
    if !quest_is_complete(ctx, &cq) {
        return Err("quest objectives not complete".to_string());
    }
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;

    // Consume the COLLECT objectives' required items BEFORE granting rewards — frees the bag
    // space the reward may need, and is atomic with the grant (any later Err rolls the whole tx back, so a
    // failed turn-in neither eats the items nor hands out the reward). `quest_is_complete` already verified
    // the player holds enough, so `remove_items` won't normally Err (it still does defensively).
    let collect: Vec<_> = ctx
        .db
        .game_quest_objective()
        .by_quest()
        .filter(&quest_entry)
        .filter(|o| o.kind == objective_kind::COLLECT_ITEM)
        .collect();
    for obj in collect {
        crate::items::remove_items(ctx, player_guid, obj.target_entry, obj.required_count)?;
    }

    // Rewards. Items FIRST (the fallible step — a full backpack must fail the whole turn-in before any
    // money/XP/state change; SpacetimeDB rolls the tx back on Err). Then money (saturating) + XP via the
    // shared ding loop. The player entity is updated once after grant_xp folds in any level-ups.
    for r in ctx
        .db
        .game_quest_reward_item()
        .by_quest()
        .filter(&quest_entry)
    {
        crate::items::grant_item(ctx, player_guid, r.item_entry, r.count)?;
    }
    // Choice reward (pick-1-of-N): grant the single row whose choice_index == reward_index, IN ADDITION to
    // the guaranteed items above and atomic with them (any Err rolls the whole tx back, before money/XP).
    // A quest with NO choice rows ignores reward_index; a choice-quest whose index matches no slot is
    // REJECTED (the client must pick a valid slot) — never silently grant nothing / the wrong item.
    let choices: Vec<(u8, u32, u32)> = ctx
        .db
        .game_quest_reward_choice()
        .by_quest()
        .filter(&quest_entry)
        .map(|c| (c.choice_index, c.item_entry, c.count))
        .collect();
    if let Some((item_entry, count)) = pick_choice_reward(&choices, reward_index, quest_entry)? {
        crate::items::grant_item(ctx, player_guid, item_entry, count)?;
    }
    player.money = player.money.saturating_add(tmpl.reward_money);
    // Quest XP is scaled by the realm xp_rate at the source (like kill XP), so a custom-rate realm
    // speeds up quest leveling too. Quest XP has no rested bonus, so this is a plain scale.
    // Quest XP source, in priority: an explicit reward_xp override → authentic cmangos
    // RewMoneyMaxLevel/0.6 (once a re-import populates reward_money_max_level) → the xp_reward
    // placeholder for un-reimported rows. The gateway preview (reads.rs quest_detail_view) resolves the
    // SAME three-way so the completion popup equals this grant.
    let base_xp = if tmpl.reward_xp > 0 {
        tmpl.reward_xp
    } else if tmpl.reward_money_max_level > 0 {
        lyracore_shared::quest::quest_xp(tmpl.reward_money_max_level)
    } else {
        lyracore_shared::quest::xp_reward(tmpl.quest_level)
    };
    let xp = crate::xp::rated_xp(ctx, base_xp);
    // Level-cap payout (work-item 194(e)): at the vanilla level cap, quest XP is USELESS (no next
    // level to fill) — real vanilla instead converts it to copper. Money-converted, never granted as
    // XP (vanilla behaviour: a capped character never dings off a quest). Below the cap, unchanged: XP
    // granted normally, no conversion. `max_level_money_reward` (the gateway's quest-log-preview stub,
    // `codec/quest.rs`) uses the SAME `lyracore_shared::quest::max_level_money_reward` conversion so the
    // preview number always matches what a level-60 turn-in actually pays.
    if player.level >= QUEST_MAX_LEVEL_PAYOUT {
        player.money = player
            .money
            .saturating_add(lyracore_shared::quest::max_level_money_reward(xp));
    } else {
        // "+N experience" feedback: a non-kill game_xp_event → SMSG_LOG_XPGAIN, mirroring the kill
        // (award_xp) and exploration paths. grant_xp itself deliberately never emits one, so quest XP
        // showed no floating text without this. killed_guid 0 = no source unit (the NonKill form).
        ctx.db.game_xp_event().insert(crate::xp::XpEvent {
            id: 0,
            recipient_identity: player.owner_identity,
            killed_guid: 0,
            total_exp: xp,
            created_at: ctx.timestamp,
            is_kill: false,
        });
        crate::xp::grant_xp(ctx, &mut player, xp);
    }
    entities.guid().update(player);

    // Reputation rewards: grant each non-zero (faction, value) pair. grant_reputation no-ops on a
    // 0 faction id (no such faction) and on no-rep-bar factions, so the unconditional calls are safe; the
    // gateway relays SMSG_SET_FACTION_STANDING off the resulting row write. INERT until a re-import fills
    // these columns (default 0 today).
    crate::reputation::grant_reputation(
        ctx,
        player_guid,
        tmpl.rew_rep_faction_1,
        tmpl.rew_rep_value_1,
    );
    crate::reputation::grant_reputation(
        ctx,
        player_guid,
        tmpl.rew_rep_faction_2,
        tmpl.rew_rep_value_2,
    );

    // Mark the row rewarded and KEEP it — for every quest, repeatable or not. Deleting it here for a
    // repeatable quest would erase the only record it was ever completed, which would permanently break
    // any OTHER quest whose `prev_quest_id` points at this one (quest_is_rewarded reads this row). A
    // repeatable quest is instead re-taken by `apply_accept_quest`'s duplicate-guard exception, which
    // resets this SAME row (rewarded -> false) rather than requiring it gone.
    cq.rewarded = true;
    ctx.db.game_character_quest().id().update(cq);
    // Notify-hook: the quest was turned in — rewards granted, row marked rewarded.
    crate::hooks::fire_on_quest_turnin(
        ctx,
        &crate::hooks::QuestTurninPayload {
            character_guid: player_guid,
            quest_entry,
        },
    );
    Ok(())
}

/// Quest progress hook — called from the combat killing-blow path ([`crate::combat::kill_creature`])
/// when a PLAYER `killer_guid` kills a creature of `creature_entry`. Bumps every active (un-rewarded)
/// quest of the killer that has an incomplete KILL objective for this creature, capping each count at
/// its `required_count`. A non-player killer (or one with no matching quests) finds no rows → no-op.
/// Only writes back a quest row whose counts actually changed. Additive — touches only quest-log rows.
pub(crate) fn on_creature_killed(ctx: &ReducerContext, killer_guid: u64, creature_entry: u32) {
    let log = ctx.db.game_character_quest();
    // Snapshot the killer's active quests first (we mutate rows in the loop).
    let active: Vec<CharacterQuest> = log
        .by_character()
        .filter(&killer_guid)
        .filter(|q| !q.rewarded)
        .collect();
    for mut cq in active {
        let mut changed = false;
        for obj in ctx
            .db
            .game_quest_objective()
            .by_quest()
            .filter(&cq.quest_entry)
        {
            if obj.kind != objective_kind::KILL_CREATURE || obj.target_entry != creature_entry {
                continue;
            }
            let idx = obj.obj_index as usize;
            let Some(have) = cq.counts.get(idx).copied() else {
                continue; // objective beyond the sized progress vector (>MAX_OBJECTIVES) — skip
            };
            if have < obj.required_count {
                cq.counts[idx] = have + 1;
                changed = true;
            }
        }
        if changed {
            log.id().update(cq);
        }
    }
}

/// Quest credit for using a gameobject (the GOOBER path from CMSG_GAMEOBJ_USE) — the gameobject twin of
/// [`on_creature_killed`]. Bumps each incomplete USE_GAMEOBJECT objective whose `target_entry` matches
/// the used gameobject's template entry, capped at `required_count`. No matching quest → no-op. Additive.
pub(crate) fn on_gameobject_used(ctx: &ReducerContext, player_guid: u64, go_entry: u32) {
    let log = ctx.db.game_character_quest();
    let active: Vec<CharacterQuest> = log
        .by_character()
        .filter(&player_guid)
        .filter(|q| !q.rewarded)
        .collect();
    for mut cq in active {
        let mut changed = false;
        for obj in ctx
            .db
            .game_quest_objective()
            .by_quest()
            .filter(&cq.quest_entry)
        {
            if obj.kind != objective_kind::USE_GAMEOBJECT || obj.target_entry != go_entry {
                continue;
            }
            let idx = obj.obj_index as usize;
            let Some(have) = cq.counts.get(idx).copied() else {
                continue;
            };
            if have < obj.required_count {
                cq.counts[idx] = have + 1;
                changed = true;
            }
        }
        if changed {
            log.id().update(cq);
        }
    }
}

/// Quest credit for entering an area trigger (the CMSG_AREATRIGGER path) — the explore twin of
/// [`on_gameobject_used`]. Bumps each incomplete EXPLORE_AREATRIGGER objective whose `target_entry`
/// matches `trigger_id`, capped at `required_count`. No matching quest → no-op. Additive.
pub(crate) fn on_areatrigger_entered(ctx: &ReducerContext, player_guid: u64, trigger_id: u32) {
    let log = ctx.db.game_character_quest();
    let active: Vec<CharacterQuest> = log
        .by_character()
        .filter(&player_guid)
        .filter(|q| !q.rewarded)
        .collect();
    for mut cq in active {
        let mut changed = false;
        for obj in ctx
            .db
            .game_quest_objective()
            .by_quest()
            .filter(&cq.quest_entry)
        {
            if obj.kind != objective_kind::EXPLORE_AREATRIGGER || obj.target_entry != trigger_id {
                continue;
            }
            let idx = obj.obj_index as usize;
            let Some(have) = cq.counts.get(idx).copied() else {
                continue;
            };
            if have < obj.required_count {
                cq.counts[idx] = have + 1;
                changed = true;
            }
        }
        if changed {
            log.id().update(cq);
        }
    }
}

/// One cmangos `areatrigger_teleport` row — a dungeon entrance/exit portal (work-item 225).
/// Loaded by the importer's `--dump` "globals" family
/// (`importer/src/main.rs::build_areatrigger_teleport_sql`). Module-only: deliberately NOT `public` —
/// no client or gateway ever reads a teleport TARGET directly, only its EFFECT (the cross-map teleport
/// handshake, work-item 224, which already carries its own `game_teleport_event` wire path), so this
/// table needs no gateway binding at all (danger-zones.md §1's "new table → regenerate bindings" rule
/// only applies to tables a gateway subscription reads). [`apply_enter_areatrigger`] looks this up by
/// `trigger_id`; a hit routes the player through [`crate::world::teleport_player`], a miss leaves the
/// pre-225 quest-credit-only behavior byte-identical. No Timestamp → plain SQL, clear+reload. [static]
#[table(accessor = game_areatrigger_teleport)]
pub struct AreatriggerTeleport {
    #[primary_key]
    pub trigger_id: u32,
    pub target_map: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub o: f32,
    pub name: String,
}

/// The two ARMS [`apply_enter_areatrigger`] can fire, and their fixed order (quest credit is always
/// evaluated; teleport only when a target row exists) — pure and unit-testable without a
/// `ReducerContext` (per the repo's "extract pure functions" test convention — the module crate has no
/// ctx test harness by design). `teleport_target` is the `(target_map, x, y, z, o)` tuple read from a
/// `game_areatrigger_teleport` row, or `None` if `trigger_id` has no such row.
pub(crate) fn plan_areatrigger_entry(
    teleport_target: Option<(u32, f32, f32, f32, f32)>,
) -> AreatriggerPlan {
    AreatriggerPlan {
        credit: true,
        teleport: teleport_target,
    }
}

/// See [`plan_areatrigger_entry`]. `credit` is always `true` today (kept as an explicit field, not a
/// bare `Option`, so a future gate on the credit arm — e.g. a one-shot trigger — has somewhere to live
/// without reshaping the type); `teleport` is the routed destination, if any.
pub(crate) struct AreatriggerPlan {
    pub credit: bool,
    pub teleport: Option<(u32, f32, f32, f32, f32)>,
}

/// The shared core behind [`enter_areatrigger`] and `debug_enter_areatrigger` (work-item 225). Quest
/// credit fires FIRST (unchanged from pre-225 behavior), then — iff `trigger_id` has an imported
/// `game_areatrigger_teleport` row — the player is routed through 224's cross-map teleport. A trigger
/// can be BOTH a quest-explore objective AND a teleport (Deadmines' Moonbrook entrance/exit triggers,
/// `[V]` ids ~1447/~1448 per the design doc) — this is deliberate, not a bug: the two arms are
/// independent and both fire on the same `CMSG_AREATRIGGER`.
///
/// 190 slice 2 (the former "HOOK POINT"): a teleport whose `target_map` is a DUNGEON
/// (`instance::is_dungeon_map` — Deadmines = map 36 today) resolves-or-creates the caller's
/// instance (`instance::resolve_or_create_instance`: own live binding → party's live instance →
/// create; solo allowed; 5-player cap enforced here at trigger time) and teleports into it. Every
/// NON-dungeon target keeps the byte-identical instance-0 path (`is_dungeon_map` false → the same
/// `teleport_player(..., 0, ...)` call as before) — the Deadmines EXIT trigger (map 36 → map 0)
/// rides that arm, correctly landing the leaver back in the open world.
pub(crate) fn apply_enter_areatrigger(ctx: &ReducerContext, player_guid: u64, trigger_id: u32) {
    let teleport_target = ctx
        .db
        .game_areatrigger_teleport()
        .trigger_id()
        .find(trigger_id)
        .map(|t| (t.target_map, t.x, t.y, t.z, t.o));
    let plan = plan_areatrigger_entry(teleport_target);
    if plan.credit {
        on_areatrigger_entered(ctx, player_guid, trigger_id);
    }
    if let Some((target_map, x, y, z, o)) = plan.teleport {
        let instance_id = if crate::instance::is_dungeon_map(target_map) {
            match crate::instance::resolve_or_create_instance(ctx, player_guid, target_map) {
                Ok(id) => id,
                Err(e) => {
                    // Refused LOUDLY (cap breach / population-guid exhaustion): quest credit above
                    // already fired; the teleport does not — the player stays outside the portal.
                    // 1.12 has no "instance is full" packet on the areatrigger path, so the server
                    // warn is the loud signal (and the player visibly doesn't zone).
                    spacetimedb::log::warn!(
                        "areatrigger {trigger_id}: dungeon entry refused for {player_guid}: {e}"
                    );
                    return;
                }
            }
        } else {
            0
        };
        crate::world::teleport_player(ctx, player_guid, target_map, instance_id, x, y, z, o);
    }
}

/// Enter an AreaTrigger zone (`CMSG_AREATRIGGER`) — credits any "explore" quest tied to `trigger_id`,
/// then (work-item 225) routes the player through a cross-map teleport if `trigger_id` is an imported
/// dungeon entrance/exit. Player-authorized via `ctx.sender`; `debug_enter_areatrigger` drives the same
/// core ([`apply_enter_areatrigger`]) by explicit guid.
// note: client-driven — the client only sends CMSG_AREATRIGGER when it physically enters the zone, and
// the server has no AreaTrigger.dbc coords, so it trusts the id (a modified client could auto-complete an
// explore quest it already holds, or self-teleport into a dungeon it hasn't earned). Acceptable for
// alpha; the full fix is loading the trigger coords and range-checking the player's position here.
#[reducer]
pub fn enter_areatrigger(ctx: &ReducerContext, trigger_id: u32) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "player not in world".to_string())?;
    apply_enter_areatrigger(ctx, player.guid, trigger_id);
    Ok(())
}

// ===========================================================================================
//  Timed quests (work-item 194) — expiry tick
// ===========================================================================================

/// Is a timed quest EXPIRED? Pure: `deadline_micros == 0` (untimed) never expires; an already
/// `rewarded` or already `failed` row never re-expires (idempotent — the tick must not re-process a
/// terminal row every 0.5s forever); otherwise expired iff `now_micros >= deadline_micros`
/// (inclusive — the deadline instant itself counts as expired, not one tick later).
fn is_expired(now_micros: i64, deadline_micros: i64, rewarded: bool, failed: bool) -> bool {
    deadline_micros != 0 && !rewarded && !failed && now_micros >= deadline_micros
}

// The timed-quest expiry tick (work-item 194): every `tick_creatures` cadence (0.5s, via the
// `game_tick_pass!` marker — zero core edits needed elsewhere), scan the quest log for rows whose
// deadline has passed and fail them (`failed = true`, `deadline_micros` cleared back to 0 — a failed
// row's deadline is meaningless past this point). The gateway's `on_quest_update` relay diffs
// `!old.failed && row.failed` to send `SMSG_QUESTUPDATE_FAILEDTIMER`. A failed quest is NOT deleted —
// `apply_turn_in_quest` rejects it, and `apply_accept_quest`'s duplicate guard lets it be re-accepted
// (reset in place), mirroring the repeatable-rewarded exception.
crate::game_tick_pass!(fn quest_timer_pass(ctx) {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let log = ctx.db.game_character_quest();
    // `is_expired`'s own `deadline_micros != 0` check makes this a cheap no-op scan for the (typical)
    // all-untimed quest log — no separate index needed for this table's expected small size.
    // REFUSE verdict (issue #30) — the production twin of `debug::debug_expire_quest`, fenced there
    // for exactly this reason: the write lands in `game_character_quest`, a MANIFEST table, so
    // post-`begin_transfer` it is a lost write cross-database. This pass is BACKGROUND, so the
    // "the guid came from a live entity" argument that fences the rest of `quest.rs` does not reach
    // it — it scans the quest log directly and fires every 0.5s.
    //
    // Refusing DEFERS rather than drops: the deadline is left set, so the first pass after the
    // transfer settles fails the quest exactly as it would have. It is also the better player
    // outcome — a character frozen on a shard-hop loading screen cannot act on a timer.
    let expired: Vec<CharacterQuest> = log
        .iter()
        .filter(|q| is_expired(now, q.deadline_micros, q.rewarded, q.failed))
        .filter(|q| !crate::transfer::is_in_transit(ctx, q.character_guid))
        .collect();
    for mut cq in expired {
        cq.failed = true;
        cq.deadline_micros = 0;
        log.id().update(cq);
    }
});

// ===========================================================================================
//  Sharing (work-item 194) — CMSG_PUSHQUESTTOPARTY
// ===========================================================================================

/// Share `quest_entry` with the sender's party (`CMSG_PUSHQUESTTOPARTY`, `MSG_QUEST_PUSH_RESULT`'s
/// producer). The sender must be in-world, grouped, and ACTIVELY on the quest (an un-rewarded,
/// un-failed row) — you can't share a quest you don't hold. For every OTHER group member, a PURE
/// [`share_result`] decides the outcome; an eligible (`SHARING_QUEST`) member gets a `QUEST_SHARE`
/// event (the gateway relay opens the quest's DETAILS screen with the SENDER as the "giver" — see
/// [`GiverKind::Party`]); the SENDER gets one `QUEST_PUSH_RESULT` event per member regardless (the
/// client's "so-and-so already has it / is too far / etc" feedback line). This reducer NEVER
/// authorizes an accept by itself — the recipient's own `accept_quest` call re-validates group
/// membership + range + the sender's live quest state FRESH via [`validate_party_giver`], so a
/// spoofed giver_guid can't self-authorize even if this reducer never ran.
#[reducer]
pub fn push_quest_to_party(ctx: &ReducerContext, quest_entry: u32) -> Result<(), String> {
    let sender =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "player not in world".to_string())?;
    apply_push_quest_to_party(ctx, sender.guid, quest_entry)
}

/// The shared core behind [`push_quest_to_party`] and its debug twin `debug_push_quest`.
pub(crate) fn apply_push_quest_to_party(
    ctx: &ReducerContext,
    sender_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    if !character_quest_row(ctx, sender_guid, quest_entry).is_some_and(|q| !q.rewarded && !q.failed)
    {
        return Err("you are not on that quest".to_string());
    }
    let sender_entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(sender_guid)
        .ok_or_else(|| "player not in world".to_string())?;
    let m = crate::group::group_of(ctx, sender_guid).ok_or_else(|| "not in a group".to_string())?;
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;
    for member in crate::group::members_of(ctx, m.group_id) {
        if member.character_guid == sender_guid {
            continue;
        }
        let result = evaluate_share(ctx, &sender_entity, &tmpl, member.character_guid);
        if result == lyracore_shared::quest::share_result::SHARING_QUEST {
            crate::group::push_event(
                ctx,
                member.character_guid,
                lyracore_shared::quest::share_event_kind::QUEST_SHARE,
                sender_guid,
                quest_entry.to_string(),
            );
        }
        crate::group::push_event(
            ctx,
            sender_guid,
            lyracore_shared::quest::share_event_kind::QUEST_PUSH_RESULT,
            member.character_guid,
            result.to_string(),
        );
    }
    Ok(())
}

/// Gathers one group member's live state for [`share_result`] — the ctx-consuming half; the decision
/// itself is the pure fn so it's unit-testable without a `ReducerContext`.
fn evaluate_share(
    ctx: &ReducerContext,
    sender: &crate::WorldEntity,
    tmpl: &QuestTemplate,
    member_guid: u64,
) -> u8 {
    let member_entity = ctx.db.game_world_entity().guid().find(member_guid);
    let (same_map_instance, in_range) = match &member_entity {
        Some(e) => {
            let (dx, dy, dz) = (e.x - sender.x, e.y - sender.y, e.z - sender.z);
            (
                e.map_id == sender.map_id && e.instance_id == sender.instance_id,
                dx * dx + dy * dy + dz * dz <= PARTY_SHARE_RANGE_SQ,
            )
        }
        None => (false, false),
    };
    let existing = character_quest_row(ctx, member_guid, tmpl.entry);
    let member_active = existing.as_ref().is_some_and(|q| !q.rewarded && !q.failed);
    let member_finished = existing.as_ref().is_some_and(|q| q.rewarded) && !tmpl.repeatable;
    let log_full = active_quest_count(ctx, member_guid) >= MAX_QUEST_LOG_SIZE;
    // Same gate set `apply_accept_quest` itself enforces (level/race/class/prereq) — this is a
    // PREVIEW only (the CANT_TAKE_QUEST feedback line); the actual accept re-validates every one of
    // these independently, so an inaccurate preview here could never grant a quest the member doesn't
    // qualify for. ExclusiveGroup + reputation-requirement gates are OUT of scope (194 cuts).
    let gates_ok = member_entity.as_ref().is_some_and(|e| {
        e.level >= tmpl.min_level
            && lyracore_shared::quest::race_allowed(tmpl.required_races, e.race())
            && lyracore_shared::quest::class_allowed(tmpl.required_classes, e.class())
            && (tmpl.prev_quest_id == 0 || quest_is_rewarded(ctx, member_guid, tmpl.prev_quest_id))
    });
    lyracore_shared::quest::share_result(
        member_entity.is_some(),
        same_map_instance,
        in_range,
        member_active,
        member_finished,
        log_full,
        gates_ok,
    )
}

/// How many of `character_guid`'s quest-log rows are ACTIVE (un-rewarded, un-failed) — the
/// `push_quest_to_party` LOG_FULL gate.
fn active_quest_count(ctx: &ReducerContext, character_guid: u64) -> usize {
    ctx.db
        .game_character_quest()
        .by_character()
        .filter(&character_guid)
        .filter(|q| !q.rewarded && !q.failed)
        .count()
}

/// Force-expire `character_guid`'s `quest_entry` row — the harness twin of [`quest_timer_pass`]'s
/// natural expiry (`debug_expire_quest`), for the timed-quest runbook without waiting out
/// `limit_time` seconds live. A no-op (`Err`) for a missing row or one already terminal
/// (rewarded/failed) — mirrors the tick's own idempotence.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn debug_force_expire(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    let mut cq = character_quest_row(ctx, character_guid, quest_entry)
        .ok_or_else(|| "you are not on that quest".to_string())?;
    if cq.rewarded || cq.failed {
        return Err("quest is already terminal".to_string());
    }
    cq.failed = true;
    cq.deadline_micros = 0;
    ctx.db.game_character_quest().id().update(cq);
    Ok(())
}

// ===========================================================================================
//  Player reducers — authorized via ctx.sender, delegate to the cores
// ===========================================================================================

/// Accept a quest from a giver NPC (`CMSG_QUESTGIVER_ACCEPT_QUEST`). Player-authorized via
/// `ctx.sender`; `debug_accept_quest` drives the same [`apply_accept_quest`] by explicit guid for the
/// harness (the CLI identity owns no entity, and the gossip/quest UI is mouse-only / not yet wired).
#[reducer]
pub fn accept_quest(ctx: &ReducerContext, giver_guid: u64, quest_entry: u32) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "player not in world".to_string())?;
    apply_accept_quest(ctx, player.guid, giver_guid, quest_entry)
}

/// Turn a completed quest in to a giver NPC for its rewards (`CMSG_QUESTGIVER_COMPLETE_QUEST` /
/// reward). Player-authorized via `ctx.sender`; `debug_turn_in_quest` drives the same
/// [`apply_turn_in_quest`] by explicit guid for the harness.
#[reducer]
pub fn turn_in_quest(
    ctx: &ReducerContext,
    giver_guid: u64,
    quest_entry: u32,
    reward_index: u32,
) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "player not in world".to_string())?;
    apply_turn_in_quest(ctx, player.guid, giver_guid, quest_entry, reward_index)
}

/// Abandon an active quest (`CMSG_QUESTLOG_REMOVE_QUEST` — the "Abandon Quest" button). Deletes the
/// player's `game_character_quest` row, which fires the gateway's quest-log relay to clear the slot.
/// Player-authorized via `ctx.sender`. Rejects abandoning an already-rewarded quest (it's done, not
/// active) and a quest not in the log. No giver needed (you abandon from the log, anywhere).
#[reducer]
pub fn abandon_quest(ctx: &ReducerContext, quest_entry: u32) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "player not in world".to_string())?;
    let cq = character_quest_row(ctx, player.guid, quest_entry)
        .ok_or_else(|| "you are not on that quest".to_string())?;
    if cq.rewarded {
        return Err("cannot abandon a completed quest".to_string());
    }
    ctx.db.game_character_quest().id().delete(cq.id);
    Ok(())
}
// The XP-reward formula + its test live in `lyracore_shared::quest::xp_reward` (shared with the gateway's
// completion-popup display so the two can't drift). No module-local duplicate.

#[cfg(test)]
mod tests {
    use super::{is_expired, pick_choice_reward, plan_areatrigger_entry, QUEST_MAX_LEVEL_PAYOUT};

    /// The pure choice-pick grants choice[k], NOT choice[j]: a 3-row pick-list returns the item whose
    /// `choice_index` equals the requested `reward_index`, never a neighbor's.
    #[test]
    fn pick_choice_reward_grants_the_chosen_index() {
        // (choice_index, item_entry, count) — three distinct items.
        let choices = [(0u8, 100u32, 1u32), (1, 200, 1), (2, 300, 5)];
        assert_eq!(pick_choice_reward(&choices, 0, 7).unwrap(), Some((100, 1)));
        assert_eq!(pick_choice_reward(&choices, 1, 7).unwrap(), Some((200, 1)));
        // choice[2] carries count 5 — the count rides with the chosen item.
        assert_eq!(pick_choice_reward(&choices, 2, 7).unwrap(), Some((300, 5)));
        // Picking index 1 must NOT hand index 0's or 2's item.
        let got = pick_choice_reward(&choices, 1, 7).unwrap();
        assert_ne!(got, Some((100, 1)));
        assert_ne!(got, Some((300, 5)));
    }

    /// A quest with NO choice rows ignores `reward_index` (returns None → guaranteed-only turn-in).
    #[test]
    fn pick_choice_reward_no_choices_is_noop() {
        assert_eq!(pick_choice_reward(&[], 0, 7).unwrap(), None);
        // Even a nonzero index is harmless when there are no choices.
        assert_eq!(pick_choice_reward(&[], 99, 7).unwrap(), None);
    }

    /// A choice-quest with an out-of-range `reward_index` is REJECTED (Err), not silently granted nothing.
    #[test]
    fn pick_choice_reward_out_of_range_is_rejected() {
        let choices = [(0u8, 100u32, 1u32), (1, 200, 1)];
        let err = pick_choice_reward(&choices, 2, 42).unwrap_err();
        assert!(err.contains("invalid reward choice 2"), "err was: {err}");
        assert!(err.contains("quest 42"), "err names the quest: {err}");
        // A wildly-out-of-range index is rejected just the same (no panic, no wrap).
        assert!(pick_choice_reward(&choices, u32::MAX, 42).is_err());
    }

    // ---- Timed quests (work-item 194): is_expired boundary tests ----

    #[test]
    fn untimed_deadline_never_expires() {
        // deadline_micros == 0 is the untimed marker — never expires regardless of `now`.
        assert!(!is_expired(0, 0, false, false));
        assert!(!is_expired(i64::MAX, 0, false, false));
    }

    #[test]
    fn is_expired_is_inclusive_at_the_deadline_instant() {
        // Exactly AT the deadline is expired (inclusive), one micro before is not.
        assert!(is_expired(1_000, 1_000, false, false));
        assert!(!is_expired(999, 1_000, false, false));
        assert!(is_expired(1_001, 1_000, false, false));
    }

    #[test]
    fn is_expired_never_fires_on_a_terminal_row() {
        // A REWARDED row never re-expires (turned in before the deadline hit, or the deadline is stale).
        assert!(!is_expired(5_000, 1_000, true, false));
        // An already-FAILED row never re-processes (idempotent — the tick must not loop forever).
        assert!(!is_expired(5_000, 1_000, false, true));
    }

    #[test]
    fn quest_max_level_payout_matches_the_vanilla_cap() {
        assert_eq!(QUEST_MAX_LEVEL_PAYOUT, 60);
    }

    // ---- AreaTrigger teleport arm (work-item 225): plan_areatrigger_entry ----

    /// No `game_areatrigger_teleport` row for this trigger → quest-credit-only plan (byte-identical to
    /// pre-225 `enter_areatrigger` behavior): `credit` still true, `teleport` is `None`.
    #[test]
    fn plan_areatrigger_entry_without_a_teleport_row_is_credit_only() {
        let plan = plan_areatrigger_entry(None);
        assert!(plan.credit);
        assert_eq!(plan.teleport, None);
    }

    /// A `game_areatrigger_teleport` row present → BOTH arms plan to fire: `credit` is still true
    /// (unconditional — a trigger can be both a quest-explore objective and a portal, e.g. Deadmines'
    /// Moonbrook entrance), and `teleport` carries the exact target tuple through untouched.
    #[test]
    // The trailing `3.14` is an arbitrary FACING (radians, 0..2π), not an approximation of π — the
    // test only asserts the tuple passes through untouched, so any value does.
    #[allow(clippy::approx_constant)]
    fn plan_areatrigger_entry_with_a_teleport_row_plans_both_arms() {
        let target = (36u32, 100.5f32, -200.25f32, 10.0f32, 3.14f32);
        let plan = plan_areatrigger_entry(Some(target));
        assert!(
            plan.credit,
            "credit must still fire even when the trigger is also a portal"
        );
        assert_eq!(plan.teleport, Some(target));
    }

    /// `apply_enter_areatrigger`'s own source order calls `on_areatrigger_entered` (credit) BEFORE
    /// `crate::world::teleport_player` (the teleport arm) — this test can't observe that ordering
    /// without a `ReducerContext` (no ctx harness by design), so it instead pins the CONTRACT the
    /// ordering depends on: `AreatriggerPlan.credit` is `true` in every case a `teleport` target
    /// exists, so the "credit always happens" invariant can never be silently dropped by a future edit
    /// that makes `credit` conditional on the teleport arm.
    #[test]
    fn plan_areatrigger_entry_credit_is_never_gated_by_teleport_presence() {
        assert!(plan_areatrigger_entry(None).credit);
        assert!(plan_areatrigger_entry(Some((36, 0.0, 0.0, 0.0, 0.0))).credit);
    }
}
