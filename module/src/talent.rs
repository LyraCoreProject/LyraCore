//! Talents — FIRST SLICE: the talent engine + a starter set of PASSIVE Warrior talents.
//!
//! A talent is a tree node (`game_talent`) a character spends points into (`game_character_talent`),
//! whose effect is a PASSIVE aura applied at login / on-learn. The effect rides the existing spell/aura
//! pipeline: each talent maps to ONE `game_spell` carrying an `A_MOD_*` effect, applied with
//! `points = rank * base_points` — so rank 3 of Cruelty (+100 crit/rank) applies +300 crit. Re-applying
//! refreshes the SAME aura (keyed by `effect_id`), so a rank-up updates the magnitude with no stacking and
//! a relog is idempotent. This reuses the proven combat folds (`effective_crit_bp`/`effective_armor`/…) —
//! a learned talent is a real, server-verifiable combat change (`debug_compute_swing`).
//!
//! Talent points are DERIVED, not stored: `max(0, level-9) - sum(learned ranks)` (the first point at L10,
//! 1/level to 51 at L60). No `Character` column → no gateway-binding change.
//!
//! BASELINE SAFETY: a character with zero learned-talent rows applies nothing → byte-identical combat. The
//! seeded talent data is inert until a point is spent. The two tables are NEW → `publish` auto-migrates.
//!
//! DEFERRED (this slice): ability-granting talents (Mortal Strike/Bloodthirst/… — need a per-player
//! learnable-spell system; `CASTABLE` is a hardcoded const); the client talent pane (the update-mask
//! descriptor wall — work-item 031's wire/system half); auto-granting points on a mid-session ding
//! (re-derived correctly at the next learn); gossip surfacing of "unlearn talents" at the trainer
//! (work-item 198 follow-up — the reducer is wired, the gateway arm is not). [entity]
//!
//! DEMO SEED (work-item 207): `seed_talents` inserts a small hand-authored Warrior-flavored tree (ids
//! 1-8) so a no-DBC sandbox has SOMETHING to learn/test against. KEPT rather than retired even after the
//! real `TalentTab.dbc`/`Talent.dbc` importer (`importer/src/talent.rs`) landed — a sandbox with no client
//! MPQ access still needs a talent tree to exercise `learn_talent`/`reset_talents`/the aura pipeline. A
//! REAL import is a WHOLESALE clear+reload (`DELETE ... WHERE talent_id >= 0` is a tautology on the
//! unsigned key — every row goes, demo ids 1-8 included): importing REPLACES the demo tree, it does not
//! coexist with it. A character who learned a demo talent before the import keeps an orphaned
//! `game_character_talent` row — `apply_learned_talents` logs and skips it at login (respec or
//! `debug_reset_talents` clears it; fine for the no-prod alpha). The reserved 51xxx demo spell ids
//! likewise never collide with imported Spell.dbc ids. An operator who imports for real ends up with BOTH
//! the demo rows (idle, orphaned in tree_id 0-2 alongside real tabs) and the real tree — harmless, since
//! nothing gates on "is this the only row in tree_id N".

use spacetimedb::{
    client_visibility_filter, reducer, table, Filter, Identity, ReducerContext, Table,
};

use crate::helpers::entity_by_owner;
use crate::{game_character, game_spell, game_spell_effect, game_world_entity, Spell, SpellEffect};

// ===========================================================================================
//  Tables
// ===========================================================================================

/// A talent-tree node (static metadata). One ROW per talent; the per-rank magnitude lives in the mapped
/// `spell_id`'s effect `base_points` (applied as `rank * base_points`). Public + SQL-seedable (no
/// Timestamp). A talent with no `game_character_talent` row is simply unlearned.
///
/// CONTRACT: a talent does something via a PASSIVE aura (`spell_id != 0`) and/or by TEACHING an active
/// (`grant_spell_id != 0`) — at least one should be set (both is allowed; the two are applied
/// independently). A talent with neither is an inert/malformed seed (logged at login). [static]
#[table(accessor = game_talent, public, index(accessor = by_tree, btree(columns = [tree_id])))]
pub struct Talent {
    #[primary_key]
    pub talent_id: u32,
    pub name: String,
    pub tree_id: u8, // 0 Arms, 1 Fury, 2 Protection
    pub tier: u8,    // 0-indexed row in the tree
    pub column: u8,  // 0-indexed column (for a future client pane; not gameplay-load-bearing)
    pub max_rank: u8,
    pub spell_id: u32, // PASSIVE aura spell (0 = none); applied with points = rank * base_points
    pub required_talent_id: u32, // 0 = none; else the prereq talent must be at rank >= 1
    pub required_points_in_tree: u32, // tier gate (tier * POINTS_PER_TIER); 0 for tier 0
    /// An ABILITY this talent TEACHES at rank 1 (0 = none) — a learnable active spell added to the player's
    /// spellbook (`spell::learn_spell`). For an ability talent (Death Wish, …) `spell_id` (the passive) is 0
    /// and this is the granted active. END-appended `#[default(0)]` → auto-migrates. [static]
    #[default(0)]
    pub grant_spell_id: u32,
    // --- Talent.dbc import columns (work-item 207, END-appended, #[default(0)] → additive auto-migration).
    // Every demo-seeded talent (ids 1-8) leaves these at 0 — byte-identical baseline behavior.
    /// The real `game_talent_tab.tab_id` this talent belongs to (Talent.dbc `Talent.tab`) — 0 for a
    /// demo-seeded talent (no real tab). Unlike `tree_id` (which repeats 0/1/2 across EVERY class's tabs),
    /// `tab_id` is globally unique, so it's the disambiguating join key for imported content.
    #[default(0)]
    pub tab_id: u32,
    /// Rank 2's passive spell id (Talent.dbc `spell_rank[1]`); 0 if this talent has fewer than 2 ranks.
    /// Applied by `apply_talent_rank` at pick/login (which also SUPERSEDES the lower rank's spell +
    /// aura, 031 residual). END-appended `#[default(0)]`.
    #[default(0)]
    pub rank_spell_2: u32,
    /// Rank 3's passive spell id (Talent.dbc `spell_rank[2]`); 0 if this talent has fewer than 3 ranks.
    #[default(0)]
    pub rank_spell_3: u32,
    /// Rank 4's passive spell id (Talent.dbc `spell_rank[3]`); 0 if this talent has fewer than 4 ranks.
    #[default(0)]
    pub rank_spell_4: u32,
    /// Rank 5's passive spell id (Talent.dbc `spell_rank[4]`); 0 if this talent has fewer than 5 ranks.
    #[default(0)]
    pub rank_spell_5: u32,
    /// The prerequisite talent's minimum learned RANK (Talent.dbc `prereq_ranks[0]`). 0 means "any rank"
    /// (both an un-set demo prereq and a genuine Talent.dbc same-tier prereq use 0) — `do_learn_talent`
    /// treats 0 as `max(1)` so the pre-import "learned at rank >= 1" gate is unchanged. Meaningless when
    /// `required_talent_id` is 0. END-appended `#[default(0)]`.
    #[default(0)]
    pub required_talent_rank: u8,
    /// A required SPELL the character must already know (Talent.dbc `required_spell`), 0 = none. NOT
    /// gated by `do_learn_talent` this slice (carried for completeness / a future gate). END-appended
    /// `#[default(0)]`.
    #[default(0)]
    pub required_spell_id: u32,
}

/// A talent TREE tab (one of a class's Arms/Fury/Protection-style columns), imported from
/// `TalentTab.dbc` by `importer/src/talent.rs`. Public + SQL-seedable (no Timestamp) — no demo seed (the
/// hand-authored `seed_talents` tree doesn't reference a `game_talent_tab` row; a demo talent's `tab_id`
/// stays 0). `class_mask`/`race_mask` are Talent.dbc's raw bitmasks (one-hot per class in vanilla);
/// `order_index` is the tab's 0-based position within its class (0/1/2), which `importer/src/talent.rs`
/// copies onto each of its talents' `tree_id`. [static]
#[table(accessor = game_talent_tab, public)]
pub struct TalentTab {
    #[primary_key]
    pub tab_id: u32,
    pub name: String,
    pub class_mask: u32,
    pub race_mask: u32,
    pub order_index: u8,
}

/// A character's learned rank in one talent. Per-player, owner-scoped (RLS like `game_character`). Logical
/// key `(character_guid, talent_id)` via an `#[auto_inc]` PK + `by_character` btree (mirrors
/// `game_player_skill`). `rank` is 1..=max_rank (a 0-rank talent simply has no row). [entity]
#[table(accessor = game_character_talent, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct CharacterTalent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub talent_id: u32,
    pub rank: u8,
}

/// A player connection sees only its own learned talents (mirrors the character/skill RLS filters).
#[client_visibility_filter]
const CHARACTER_TALENT_RLS: Filter =
    Filter::Sql("SELECT * FROM game_character_talent WHERE owner_identity = :sender");

// Talents are deleted on character delete, and re-owned (identity re-stamp) on a relog under a
// changed gateway identity.
crate::character_owned!(delete, fn sweep_delete_game_character_talent(ctx, character_guid) {
    let talents = ctx.db.game_character_talent();
    for r in talents.by_character().filter(&character_guid).collect::<Vec<_>>() {
        talents.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19): spent talent ranks are durable progression — a character
// arriving without them is a respec nobody asked for. `id` is a surrogate PK, re-minted.
crate::character_owned!(transfer, fn sweep_transfer_game_character_talent(ctx, character_guid, io) {
    table = game_character_talent,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_character_talent(ctx, character_guid, identity) {
    let talents = ctx.db.game_character_talent();
    for mut r in talents.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            talents.id().update(r);
        }
    }
});

// ===========================================================================================
//  Pure rules (unit-tested) [server]
// ===========================================================================================

/// The character level at which the FIRST talent point is granted (vanilla: level 10).
pub const TALENT_START_LEVEL: u32 = 10;
/// Points that must be spent in a tree to unlock each next tier (vanilla: 5 per tier row).
pub const POINTS_PER_TIER: u32 = 5;

/// Talent points AVAILABLE to spend: `(level - 9)` earned (1/level from L10, 51 at L60) minus the points
/// already `spent`, saturating at 0. Pure — unit-tested.
pub fn talent_points_available(level: u32, spent: u32) -> u32 {
    level
        .saturating_sub(TALENT_START_LEVEL - 1)
        .saturating_sub(spent)
}

/// Validate a learn of a talent: returns the NEXT rank on success, else a distinguished error. Gates, in
/// order: a point must be available; the talent must be below max rank; the tier must be unlocked (enough
/// points spent in that tree); the prerequisite (if any) must be met. Pure — unit-tested.
pub fn validate_learn(
    current_rank: u8,
    max_rank: u8,
    available: u32,
    points_in_tree: u32,
    required_points_in_tree: u32,
    prereq_ok: bool,
) -> Result<u8, String> {
    if available == 0 {
        return Err("no talent points available".to_string());
    }
    if current_rank >= max_rank {
        return Err("talent already at max rank".to_string());
    }
    if points_in_tree < required_points_in_tree {
        return Err("tier locked: not enough points spent in this tree".to_string());
    }
    if !prereq_ok {
        return Err("prerequisite talent not learned".to_string());
    }
    Ok(current_rank + 1)
}

/// Whether a talent's prerequisite gate (feeding `validate_learn`'s `prereq_ok`) is satisfied: no
/// prerequisite (`required_talent_id == 0`) is always fine; else the prereq must be learned to at least
/// `required_talent_rank.max(1)`. `required_talent_rank == 0` means "learned at all" (rank >= 1) — true for
/// every demo-seeded talent (which never set a rank requirement) AND for a genuine Talent.dbc prereq that
/// only requires the SAME-tab talent below it at rank 1 — so a real import behaves exactly like the
/// pre-import demo gate unless the DBC row explicitly requires a higher rank. Pure — unit-tested.
pub fn prereq_satisfied(
    required_talent_id: u32,
    learned_prereq_rank: u8,
    required_talent_rank: u8,
) -> bool {
    required_talent_id == 0 || learned_prereq_rank >= required_talent_rank.max(1)
}

// ===========================================================================================
//  ctx reads
// ===========================================================================================

/// `character_guid`'s learned rank in `talent_id` (0 if unlearned). Scans the character's small talent set.
fn learned_rank(ctx: &ReducerContext, guid: u64, talent_id: u32) -> u8 {
    ctx.db
        .game_character_talent()
        .by_character()
        .filter(&guid)
        .find(|t| t.talent_id == talent_id)
        .map(|t| t.rank)
        .unwrap_or(0)
}

/// Total talent points `character_guid` has spent (sum of all learned ranks).
fn total_spent(ctx: &ReducerContext, guid: u64) -> u32 {
    ctx.db
        .game_character_talent()
        .by_character()
        .filter(&guid)
        .map(|t| t.rank as u32)
        .sum()
}

/// Points `character_guid` has spent in `tree_id` (sum of learned ranks of talents in that tree). Joins
/// each learned row to its `game_talent` row for the tree id.
fn points_in_tree(ctx: &ReducerContext, guid: u64, tree_id: u8) -> u32 {
    let talents = ctx.db.game_talent();
    ctx.db
        .game_character_talent()
        .by_character()
        .filter(&guid)
        .filter_map(|t| match talents.talent_id().find(t.talent_id) {
            Some(def) => Some((def.tree_id, t.rank)),
            None => {
                // A learned talent with no metadata row would under-count tree points (loosening the
                // tier gate). game_talent is static + seeded, so this only fires on a data bug — surface it.
                spacetimedb::log::info!(
                    "points_in_tree: character {guid} has orphaned talent {} (no game_talent row)",
                    t.talent_id
                );
                None
            }
        })
        .filter(|(tree, _)| *tree == tree_id)
        .map(|(_, rank)| rank as u32)
        .sum()
}

// ===========================================================================================
//  Learn + apply
// ===========================================================================================

/// Shared validated core of learning a talent (the player + debug reducers both call this). Validates
/// against the live entity's level + the character's learned set, increments the rank, and applies the
/// talent's passive aura at the new rank so it takes effect immediately (no relog). Returns the new rank.
pub(crate) fn do_learn_talent(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    talent_id: u32,
) -> Result<u8, String> {
    let talent = ctx
        .db
        .game_talent()
        .talent_id()
        .find(talent_id)
        .ok_or_else(|| format!("unknown talent {talent_id}"))?;
    let entity = crate::helpers::live_entity(ctx, guid)
        .map_err(|_| format!("no live entity for guid {guid} (must be in world to learn)"))?;
    let level = entity.level;

    let available = talent_points_available(level, total_spent(ctx, guid));
    let current = learned_rank(ctx, guid, talent_id);
    let pit = points_in_tree(ctx, guid, talent.tree_id);
    let prereq_ok = prereq_satisfied(
        talent.required_talent_id,
        learned_rank(ctx, guid, talent.required_talent_id),
        talent.required_talent_rank,
    );
    let next = validate_learn(
        current,
        talent.max_rank,
        available,
        pit,
        talent.required_points_in_tree,
        prereq_ok,
    )?;

    // Upsert the learned rank.
    let talents = ctx.db.game_character_talent();
    if let Some(mut row) = talents
        .by_character()
        .filter(&guid)
        .find(|t| t.talent_id == talent_id)
    {
        row.rank = next;
        talents.id().update(row);
    } else {
        talents.insert(CharacterTalent {
            id: 0,
            character_guid: guid,
            owner_identity: owner,
            talent_id,
            rank: next,
        });
    }

    // Teach the just-learned RANK's spell + apply its effects. This is what makes the CLIENT show the
    // talent as learned: the 5875 client infers a talent's rank from WHICH of its rank-spells you know in
    // your spellbook (there is no separate "learned talents" packet), so recording the rank server-side is
    // not enough — the rank-spell must reach `game_player_spell` (SMSG_INITIAL_SPELLS / SMSG_LEARNED_SPELL).
    // It ALSO makes an ACTIVE talent castable (Holy talent 1435 IS Consecration 26573 — putting a point in
    // it must add Consecration to the spellbook so `cast_spell` accepts it).
    apply_talent_rank(ctx, &talent, guid, owner, next, entity.level as u8);
    Ok(next)
}

/// The spell id for `talent` at `rank`. IMPORTED talents (Talent.dbc) carry a DISTINCT spell per rank
/// (`spell_id` = rank 1, `rank_spell_2..5` = ranks 2-5); the hand-authored DEMO tree stores a single
/// `spell_id` scaled by rank (its `rank_spell_*` are 0), so a 0 rank-field falls back to `spell_id`. Pure.
pub(crate) fn talent_rank_spell(t: &Talent, rank: u8) -> u32 {
    pick_rank_spell(
        rank,
        t.spell_id,
        t.rank_spell_2,
        t.rank_spell_3,
        t.rank_spell_4,
        t.rank_spell_5,
    )
}

/// Pure core of [`talent_rank_spell`] — unit-tested. `r1` = `spell_id` (rank 1 / the demo base), `r2..r5`
/// the imported per-rank spells. A 0 higher-rank field (demo tree, or a single-rank imported talent) falls
/// back to `r1`. Rank 0 → 0 (unlearned).
fn pick_rank_spell(rank: u8, r1: u32, r2: u32, r3: u32, r4: u32, r5: u32) -> u32 {
    let s = match rank {
        0 => return 0,
        1 => r1,
        2 => r2,
        3 => r3,
        4 => r4,
        _ => r5,
    };
    if s != 0 {
        s
    } else {
        r1
    }
}

/// Teach `talent`'s `rank`-spell into `guid`'s spellbook AND apply its effects — the shared body of
/// do_learn_talent (a new point) and apply_learned_talents (login/relog, which heals a char learned
/// before this fix). `learn_spell` puts the rank-spell in `game_player_spell` so the client renders the
/// talent as learned + an active talent becomes castable; `apply_spell_auras` lights any passive (a no-op
/// for an active like Consecration — it has no aura-kind effects). IMPORTED ranks carry their own values
/// (stack 1); the demo tree scales `spell_id` by rank (stack = rank). A rank-spell not yet in `game_spell`
/// (Talent.dbc/Spell.dbc are separate importer passes) logs + no-ops — the rank stays recorded for a
/// later relog to pick up. An ABILITY-only talent (`grant_spell_id`) still teaches that on rank 1.
fn apply_talent_rank(
    ctx: &ReducerContext,
    talent: &Talent,
    guid: u64,
    owner: Identity,
    rank: u8,
    level: u8,
) {
    let rank_spell = talent_rank_spell(talent, rank);
    if rank_spell != 0 {
        if ctx.db.game_spell().spell_id().find(rank_spell).is_some() {
            crate::spell::learn_spell(ctx, guid, owner, rank_spell);
            let stack = if talent.tab_id != 0 { 1 } else { rank }; // imported: own values; demo: scale by rank
            crate::spell::apply_spell_auras(ctx, rank_spell, guid, level, stack);
            // Server-side rank SUPERSEDE (031 residual): the imported tree carries a DISTINCT spell
            // per rank, so without this every lower rank's book row lingered (the login
            // INITIAL_SPELLS re-grew both ranks) and a mapped-kind passive STACKED with the new
            // rank's aura. Sweep EVERY lower rank (not just N-1) so pre-fix double-rows heal on the
            // next pick. Runs only when the NEW rank actually taught (an un-imported rank-spell must
            // not strip the old working one); the demo tree's ranks share one spell id, so
            // `prev != rank_spell` makes this a structural no-op there. The gateway's
            // SMSG_SUPERCEDED_SPELL relay already tells the live client; this is the durable half.
            let mut revitalize = false;
            for prev_rank in 1..rank {
                let prev = talent_rank_spell(talent, prev_rank);
                if prev == 0 || prev == rank_spell {
                    continue;
                }
                // Canonical unlearn primitive (spellbook::forget_spell + strip_spell_auras) — ONE
                // recompute after the sweep (a stat passive removed without it leaves stale vitals).
                crate::spell::forget_spell(ctx, guid, prev);
                revitalize |= crate::spell::strip_spell_auras(ctx, guid, prev);
            }
            if revitalize {
                crate::spell::recompute_vitals(ctx, guid);
            }
        } else {
            spacetimedb::log::info!(
                "talent {} rank {rank}: rank spell {rank_spell} not imported yet",
                talent.talent_id
            );
        }
    }
    // ABILITY talent (grant_spell_id, distinct from a rank-spell): teach the granted active on rank 1.
    if talent.grant_spell_id != 0 && rank == 1 {
        crate::spell::learn_spell(ctx, guid, owner, talent.grant_spell_id);
    }
}

/// Re-apply every learned passive talent's aura to `guid` at login (mirrors `ensure_player_skills`). Each
/// learned talent's spell is applied at its current rank; idempotent (aura_apply refreshes by effect_id),
/// so a relog never stacks. A character with no learned talents applies nothing (baseline-safe). [entity]
pub fn apply_learned_talents(ctx: &ReducerContext, guid: u64, owner: Identity, level: u32) {
    let talents = ctx.db.game_talent();
    let learned: Vec<(u32, u8)> = ctx
        .db
        .game_character_talent()
        .by_character()
        .filter(&guid)
        .filter(|t| t.rank >= 1)
        .map(|t| (t.talent_id, t.rank))
        .collect();
    for (talent_id, rank) in learned {
        let Some(def) = talents.talent_id().find(talent_id) else {
            // Orphaned learned row: its talent definition is gone (a real import wholesale-replaces
            // the demo tree — see the module doc). Log loudly instead of silently dropping the
            // character's aura; a respec clears the residue.
            spacetimedb::log::warn!("apply_learned_talents: character {guid} knows talent {talent_id} rank {rank} but no such talent exists (replaced by an import?) — skipping; respec clears it");
            continue;
        };
        // Re-teach the rank-spell (heals a char who learned BEFORE the spellbook-teach fix: their client
        // shows the talent as learned again on relog) AND re-apply its passive aura. Same shared body as a
        // fresh learn — idempotent (learn_spell + aura refresh don't stack).
        apply_talent_rank(ctx, &def, guid, owner, rank, level as u8);
    }
}

/// Player entry (`CMSG_LEARN_TALENT` — gateway wiring is a follow-up; driven by `debug_learn_talent` for
/// now). Spends a point on `talent_id` for the calling player.
#[reducer]
pub fn learn_talent(ctx: &ReducerContext, talent_id: u32) -> Result<(), String> {
    let e = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "caster not in world".to_string())?;
    do_learn_talent(ctx, e.guid, ctx.sender(), talent_id).map(|_| ())
}

// ===========================================================================================
//  Respec — unlearn every learned talent for an escalating gold cost (work-item 198)
// ===========================================================================================

/// Gold cost (copper) of a character's NEXT respec, given `respec_count` prior resets: vanilla's
/// `Player::resetTalentsCost` step table — 1g / 5g / 10g, then +5g per further reset, capped at 50g
/// (1.12 has no cost decay — that's a TBC addition). Expressed directly in copper (1g = 10_000c, the
/// server's existing money unit — see `game_trainer_spell.cost`). Pure — unit-tested.
pub(crate) fn respec_cost_copper(respec_count: u32) -> u32 {
    const CAP_COPPER: u32 = 500_000;
    let copper = match respec_count {
        0 => 10_000,
        1 => 50_000,
        2 => 100_000,
        n => 100_000 + 50_000 * (n - 2),
    };
    copper.min(CAP_COPPER)
}

/// Shared validated core of a talent respec (the player + debug reducers both call this). Gates
/// EXACTLY like `trainer::apply_trainer_buy` (a real in-range TRAINER on the same map), then charges
/// `respec_cost_copper(character.respec_count)` and unwinds every learned talent: the passive auras
/// those talents applied (deleted directly — `cancel_aura` refuses a PASSIVE spell) and the abilities
/// they taught (removed from the spellbook). Talent points are DERIVED (`talent_points_available`), so
/// deleting the learned rows alone gives them all back — no separate refund step. Returns the copper
/// charged on success. [entity]
pub(crate) fn do_reset_talents(
    ctx: &ReducerContext,
    character_guid: u64,
    trainer_guid: u64,
) -> Result<u32, String> {
    let entities = ctx.db.game_world_entity();
    let mut entity = entities
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no live entity for guid {character_guid}"))?;
    // Same gates as `apply_trainer_buy` (issue #372's shared validate_trainer_interaction): a real
    // TRAINER, on the caller's map, within interaction range.
    crate::trainer::validate_trainer_interaction(ctx, &entity, trainer_guid)?;

    let chars = ctx.db.game_character();
    let mut character = chars
        .guid()
        .find(character_guid)
        .ok_or_else(|| format!("no character {character_guid}"))?;
    let cost = respec_cost_copper(character.respec_count);
    if entity.money < cost {
        return Err(format!("not enough money (need {cost})"));
    }

    // Snapshot every learned talent's removable spells BEFORE deleting the rows — the join target
    // (`game_talent`) is static, so this is a plain read, not a mutation-order hazard.
    // 031 residual fix: key on the PER-RANK spells (rank 1..=learned rank), not `def.spell_id`
    // alone — an imported rank-3 talent's live aura + book row belong to `rank_spell_3`, which the
    // old snapshot missed (rank-3 aura lingered through a respec). Lower ranks are swept too so
    // pre-supersede legacy book rows heal here as well; the demo tree's shared spell id dedups.
    let talents = ctx.db.game_talent();
    let learned = ctx.db.game_character_talent();
    let mut passives: Vec<u32> = Vec::new(); // per-rank passive spells: strip auras + book rows
    let mut granted: Vec<u32> = Vec::new(); // ability grants: book rows only (no aura)
    for t in learned.by_character().filter(&character_guid) {
        let Some(def) = talents.talent_id().find(t.talent_id) else {
            continue;
        };
        for r in 1..=t.rank {
            let s = talent_rank_spell(&def, r);
            if s != 0 && !passives.contains(&s) {
                passives.push(s);
            }
        }
        if def.grant_spell_id != 0 {
            granted.push(def.grant_spell_id);
        }
    }

    // Delete the learned-talent rows (collect-then-delete — never mutate mid-iteration).
    for row in learned
        .by_character()
        .filter(&character_guid)
        .collect::<Vec<_>>()
    {
        learned.id().delete(row.id);
    }

    // Strip each talent's PASSIVE aura directly: `cancel_aura` refuses a PASSIVE spell (by design, so a
    // player can't right-click one off), so a respec must delete the `game_aura` row itself. Canonical
    // unlearn primitive (`spellbook::strip_spell_auras`), single `recompute_vitals` call for the batch.
    let mut revitalize = false;
    for spell_id in &passives {
        revitalize |= crate::spell::strip_spell_auras(ctx, character_guid, *spell_id);
    }
    if revitalize {
        crate::spell::recompute_vitals(ctx, character_guid);
    }

    // Forget every spell the reset talents had put in the book: granted ABILITIES and (031 residual
    // fix) the passive RANK-SPELLS themselves — the book previously kept every passive through a
    // respec, so the client re-rendered the talents as still learned after relog.
    for spell_id in granted.iter().chain(passives.iter()) {
        crate::spell::forget_spell(ctx, character_guid, *spell_id);
    }

    // Charge the cost (ONE chokepoint) and bump the escalation counter.
    entity.money = entity.money.saturating_sub(cost);
    entities.guid().update(entity);
    character.respec_count += 1;
    chars.guid().update(character);
    Ok(cost)
}

/// Player entry (`CMSG_GOSSIP_...` unlearn-talents option — gateway wiring is a follow-up; driven by
/// `debug_reset_talents` for now). Resets every learned talent at `trainer_guid` for the calling
/// player's escalating gold cost.
#[reducer]
pub fn reset_talents(ctx: &ReducerContext, trainer_guid: u64) -> Result<(), String> {
    let e = entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "caster not in world".to_string())?;
    do_reset_talents(ctx, e.guid, trainer_guid).map(|_| ())
}

// ===========================================================================================
//  Seed (shared by init + debug_seed_talents — init does NOT re-run on an auto-migrate publish)
// ===========================================================================================

/// Seed the first-slice Warrior talents: the `game_talent` metadata + the per-talent passive spell
/// (`game_spell` + a single `game_spell_effect`). IDEMPOTENT — inserts only rows that are absent — so it is
/// safe to call from `seed::init` (fresh install) AND from `debug_seed_talents` on an already-migrated dev
/// DB (where `init` did not re-run). Spell ids live in a reserved 51xxx range, ABOVE the vanilla spell ids
/// the importer loads, and are NOT in `CASTABLE` (passive, applied at login, never player-cast). [server]
pub(crate) fn seed_talents(ctx: &ReducerContext) {
    // ~49.7 days (the u32-ms max) — effectively permanent; re-applied every login (apply_learned_talents).
    const TALENT_DURATION_MS: u32 = u32::MAX;

    // (talent_id, name, tree, tier, column, max_rank, spell_id, req_talent, req_points, grant_spell_id)
    let talent = |talent_id: u32,
                  name: &str,
                  tree_id: u8,
                  tier: u8,
                  column: u8,
                  max_rank: u8,
                  spell_id: u32,
                  required_talent_id: u32,
                  required_points_in_tree: u32,
                  grant_spell_id: u32| {
        if ctx.db.game_talent().talent_id().find(talent_id).is_none() {
            ctx.db.game_talent().insert(Talent {
                talent_id,
                name: name.to_string(),
                tree_id,
                tier,
                column,
                max_rank,
                spell_id,
                required_talent_id,
                required_points_in_tree,
                grant_spell_id,
                // No real Talent.dbc data behind a demo-seeded talent — every import column stays 0.
                tab_id: 0,
                rank_spell_2: 0,
                rank_spell_3: 0,
                rank_spell_4: 0,
                rank_spell_5: 0,
                required_talent_rank: 0,
                required_spell_id: 0,
            });
        }
    };
    // The passive talent spell + its single A_MOD_* effect. base_points is the PER-RANK magnitude
    // (applied as rank * base_points). school_mask 1 (physical), not negative, single stack.
    let talent_spell =
        |spell_id: u32, name: &str, kind: u8, per_rank: i32, p0: i32, p0_kind: u8| {
            if ctx.db.game_spell().spell_id().find(spell_id).is_none() {
                ctx.db.game_spell().insert(Spell {
                    spell_id,
                    family_name: 0,
                    family_flags: 0,
                    name: name.to_string(),
                    power_type: 0,
                    cost: 0,
                    cast_time_ms: 0,
                    gcd_ms: 0,
                    cooldown_ms: 0,
                    range_yd: 0,
                    duration_ms: TALENT_DURATION_MS,
                    school_mask: 1,
                    dispel_type: 0,
                    mechanic: 0,
                    max_stacks: 1,
                    aura_interrupt: 0,
                    // PASSIVE: server-enforced, never player-cast or player-cancelable (cancel_aura refuses it).
                    attributes: crate::spell::SPELL_ATTR_PASSIVE,
                    spell_level: 0,
                    max_level: 0,
                    is_negative: false,
                    cast_flags: 0,
                    stances: 0, // a talent passive has no stance requirement (usable in any stance)
                });
                // Single effect at index 0; id keeps the canonical `(spell_id << 2) | effect_index` form
                // (== `<< 2` here) so a future multi-effect talent spell won't collide on effect_id.
                let effect_index: u8 = 0;
                ctx.db.game_spell_effect().insert(SpellEffect {
                    id: ((spell_id as u64) << 2) | (effect_index as u64),
                    spell_id,
                    effect_index,
                    kind,
                    base_points: per_rank,
                    die_sides: 0,
                    per_level: 0.0,
                    period_ms: 0,
                    target: 0, // T_SELF
                    radius_yd: 0.0,
                    chain_targets: 0,
                    trigger_spell: 0,
                    effect_mechanic: 0,
                    p0,
                    p0_kind,
                    p1: 0,
                    script_id: 0,
                    enters_combat: false,
                });
            }
        };

    // An ACTIVE spell an ABILITY talent teaches (NOT passive — cast on demand, has a cooldown). Reserved
    // 513xx id (above the importer range, learned into the spellbook by the talent, NOT in CASTABLE).
    // Covers both a self-buff (Death Wish: A_MOD_COMBAT aura, self) and an enemy strike (Mortal Strike:
    // E_WEAPON_STRIKE, enemy) via the target/range/cost/is_negative params.
    #[allow(clippy::too_many_arguments)]
    let ability_spell = |spell_id: u32,
                         name: &str,
                         power_type: u8,
                         cost: u32,
                         range_yd: u32,
                         cooldown_ms: u32,
                         duration_ms: u32,
                         is_negative: bool,
                         kind: u8,
                         amount: i32,
                         target: u8,
                         p0: i32,
                         p0_kind: u8| {
        if ctx.db.game_spell().spell_id().find(spell_id).is_none() {
            ctx.db.game_spell().insert(Spell {
                spell_id,
                family_name: 0,
                family_flags: 0,
                name: name.to_string(),
                power_type,
                cost,
                cast_time_ms: 0,
                gcd_ms: 1500,
                cooldown_ms,
                range_yd,
                duration_ms,
                school_mask: 1,
                dispel_type: 0,
                mechanic: 0,
                max_stacks: 1,
                aura_interrupt: 0,
                attributes: 0, // ACTIVE (not passive) — castable from the bar
                spell_level: 0,
                max_level: 0,
                is_negative,
                cast_flags: 0,
                stances: 0, // a replicated talent-granted spell has no stance requirement (usable in any stance)
            });
            ctx.db.game_spell_effect().insert(SpellEffect {
                id: (spell_id as u64) << 2,
                spell_id,
                effect_index: 0,
                kind,
                base_points: amount,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target,
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0,
                p0_kind,
                p1: 0,
                script_id: 0,
                enters_combat: false,
            });
        }
    };

    // --- Fury: Cruelty (tier 0) — +1% melee crit per rank (A_MOD_COMBAT(CRIT), +100 bp/rank). Folds into
    //     effective_crit_bp → verifiable as crit_bp in debug_compute_swing.
    talent(1, "Cruelty", 1, 0, 0, 5, 51000, 0, 0, 0);
    talent_spell(51000, "Cruelty", 0xA3, 100, 1, 5); // A_MOD_COMBAT, COMBAT_CRIT (1), P_COMBAT_FIELD (5)

    // --- Protection: Toughness (tier 0) — +armor per rank (A_MOD_RESISTANCE(armor)). A FLAT per-rank
    //     approximation of vanilla's 2%/rank (no percent-armor fold exists). Folds into effective_armor →
    //     verifiable as mitigation_pct in debug_compute_swing.
    talent(2, "Toughness", 2, 0, 0, 5, 51100, 0, 0, 0);
    talent_spell(51100, "Toughness", 0xA1, 240, 1, 2); // A_MOD_RESISTANCE, RESIST_ARMOR (1), P_SCHOOL_MASK (2)

    // --- Fury: Death Wish (tier 0 for the slice; real DW is deep Fury) — an ABILITY talent: 1 rank, no
    //     passive (spell_id 0), TEACHES the Death Wish active (51300). Casting it self-buffs +50 attack
    //     power for 30s (A_MOD_COMBAT(AP) → effective swing range), 3-min cooldown. Proves the learnable
    //     spellbook: cast_spell(51300) is rejected UNTIL the talent is learned, then accepted.
    talent(3, "Death Wish", 1, 0, 2, 1, 0, 0, 0, 51300);
    // Death Wish: self-buff +50 AP for 30s, 3min CD (power_type 0, cost 0, range 0, self-target, A_MOD_COMBAT(AP)).
    ability_spell(
        51300,
        "Death Wish",
        0,
        0,
        0,
        180000,
        30000,
        false,
        0xA3,
        50,
        0,
        0,
        5,
    );

    // --- Passive talents built on the defender combat-field folds. All A_MOD_COMBAT
    //     (0xA3) with p0_kind P_COMBAT_FIELD (5), folded into the matching attack-table band / skill /
    //     threat. (Tier 0 here for a clean demo; real tree positions are a data follow-up.)
    // Arms: Deflection — +1% parry/rank (COMBAT_PARRY 6). Folds into effective_parry_bp.
    talent(4, "Deflection", 0, 0, 0, 5, 51200, 0, 0, 0);
    talent_spell(51200, "Deflection", 0xA3, 100, 6, 5);
    // Protection: Shield Specialization — +1% block/rank (COMBAT_BLOCK 8). Folds into effective_block_bp
    //   (only matters with a shield equipped — the block band is 0 unarmed).
    talent(5, "Shield Specialization", 2, 0, 1, 5, 51110, 0, 0, 0);
    talent_spell(51110, "Shield Specialization", 0xA3, 100, 8, 5);
    // Protection: Anticipation — +2 defense skill/rank (COMBAT_DEFENSE 9). Folds into effective_defense_skill
    //   → raises skill_diff → tightens the attacker's miss/dodge/parry/block (better avoidance). Vanilla is +2/rank.
    talent(6, "Anticipation", 2, 0, 2, 5, 51120, 0, 0, 0);
    talent_spell(51120, "Anticipation", 0xA3, 2, 9, 5);
    // Protection: Defiance — +5% threat/rank (COMBAT_THREAT 10), 3 ranks. Folds into threat::add_threat
    //   (the source's threat-generation multiplier) — verifiable via game_threat magnitudes.
    talent(7, "Defiance", 2, 0, 3, 3, 51130, 0, 0, 0);
    talent_spell(51130, "Defiance", 0xA3, 5, 10, 5);

    // --- Arms FLAGSHIP ability talent: Mortal Strike — the payoff of the learnable-spellbook mechanic. A
    //     TIER-1 talent (required_points_in_tree 5 → must spend 5 Arms points first, e.g. Deflection 5/5),
    //     so it exercises the tier gate LIVE. Grants the Mortal Strike active (51210): an instant
    //     E_WEAPON_STRIKE — weapon swing damage + 85 — at an enemy, 30 rage, 6s cooldown. (Real MS is the
    //     31-point talent at tier 7; tier 1 here keeps the demo learnable. Mortal Wounds healing-debuff is
    //     deferred — needs a healing-taken fold.)
    talent(8, "Mortal Strike", 0, 1, 0, 1, 0, 0, 5, 51210);
    ability_spell(
        51210,
        "Mortal Strike",
        1,
        30,
        5,
        6000,
        0,
        true,
        0x08,
        85,
        1,
        0,
        0,
    ); // E_WEAPON_STRIKE +85, enemy
}

// ===========================================================================================
//  Tests
// ===========================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_spell_picks_the_imported_per_rank_spell_or_falls_back_to_base() {
        // Imported 5-rank talent (Divine Intellect): each rank its own spell.
        assert_eq!(pick_rank_spell(1, 20257, 20258, 20259, 20260, 20261), 20257);
        assert_eq!(pick_rank_spell(5, 20257, 20258, 20259, 20260, 20261), 20261);
        // Single-rank imported talent (Consecration): only r1 set → any rank resolves to it.
        assert_eq!(pick_rank_spell(1, 26573, 0, 0, 0, 0), 26573);
        // Demo tree: one spell scaled by rank, higher-rank fields 0 → fall back to the base spell.
        assert_eq!(pick_rank_spell(3, 500, 0, 0, 0, 0), 500);
        // Rank 0 = unlearned → no spell.
        assert_eq!(pick_rank_spell(0, 20257, 20258, 20259, 20260, 20261), 0);
    }

    #[test]
    fn points_available_is_level_minus_nine_minus_spent() {
        assert_eq!(talent_points_available(1, 0), 0);
        assert_eq!(talent_points_available(9, 0), 0); // no points before L10
        assert_eq!(talent_points_available(10, 0), 1); // first point at L10
        assert_eq!(talent_points_available(60, 0), 51); // 51 at cap
        assert_eq!(talent_points_available(20, 5), 6); // 11 earned − 5 spent
        assert_eq!(talent_points_available(10, 5), 0); // saturates (spent exceeds earned)
        assert_eq!(talent_points_available(20, 30), 0);
    }

    #[test]
    fn validate_learn_gates_in_order() {
        // No points → rejected regardless of everything else.
        assert!(validate_learn(0, 5, 0, 0, 0, true).is_err());
        // At max rank → rejected.
        assert!(validate_learn(5, 5, 10, 0, 0, true).is_err());
        // Tier locked (need 10 points in tree, have 5) → rejected; unlocked at >= 10.
        assert!(validate_learn(0, 5, 10, 5, 10, true).is_err());
        assert_eq!(validate_learn(0, 5, 10, 10, 10, true), Ok(1));
        // Prereq unmet → rejected; met → ok.
        assert!(validate_learn(0, 5, 10, 0, 0, false).is_err());
        // Happy path increments the rank.
        assert_eq!(validate_learn(0, 5, 1, 0, 0, true), Ok(1));
        assert_eq!(validate_learn(4, 5, 1, 0, 0, true), Ok(5));
        // Tier-0 talent (required_points_in_tree 0) is never tier-gated.
        assert_eq!(validate_learn(0, 5, 3, 0, 0, true), Ok(1));
    }

    #[test]
    fn prereq_satisfied_gates_on_required_rank() {
        // No prereq (id 0) → always satisfied, regardless of rank/required_talent_rank.
        assert!(prereq_satisfied(0, 0, 0));
        assert!(prereq_satisfied(0, 0, 5));
        // required_talent_rank == 0 (every demo talent, and a same-tier DBC prereq) → "learned at all"
        // (rank >= 1) — the pre-import gate exactly.
        assert!(!prereq_satisfied(4, 0, 0));
        assert!(prereq_satisfied(4, 1, 0));
        assert!(prereq_satisfied(4, 5, 0)); // any nonzero rank clears the max(1) gate
                                            // required_talent_rank == 2 (a real Talent.dbc row requiring 2 ranks of the prereq) → rank 1 is
                                            // NOT enough; rank 2+ is.
        assert!(!prereq_satisfied(4, 1, 2));
        assert!(prereq_satisfied(4, 2, 2));
        assert!(prereq_satisfied(4, 3, 2));
    }

    #[test]
    fn respec_cost_escalates_then_caps() {
        assert_eq!(respec_cost_copper(0), 10_000); // 1g
        assert_eq!(respec_cost_copper(1), 50_000); // 5g
        assert_eq!(respec_cost_copper(2), 100_000); // 10g
        assert_eq!(respec_cost_copper(3), 150_000); // 10g + 5g
        assert_eq!(respec_cost_copper(4), 200_000);
        assert_eq!(respec_cost_copper(9), 450_000); // 10g + 7*5g
        assert_eq!(respec_cost_copper(10), 500_000); // cap (50g)
        assert_eq!(respec_cost_copper(11), 500_000); // stays capped
        assert_eq!(respec_cost_copper(1000), 500_000);
    }
}
