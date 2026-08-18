//! Per-player spellbook — `game_player_spell` rows are the ONE source of castability.
//!
//! `cast_spell`'s gate (`knows_spell`) accepts exactly the spells stored for the character; the gateway
//! builds `SMSG_INITIAL_SPELLS` from the same rows, so the client book and the server gate can never
//! drift. `learn_spell` is the single grant path: character creation copies the createinfo kit in
//! (`grant_createinfo_spells`, from the seeded `game_createinfo_spell` data), and talents/trainers/
//! quest rewards insert on top of it. [entity]

use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::{game_aura, game_spell}; // racial-passive attribute lookup (game_spell); aura strip (game_aura)

/// A spell a character has LEARNED (beyond the class baseline). Per-player, owner-scoped (RLS like
/// `game_character`/`game_player_skill`). Logical key `(character_guid, spell_id)` via an `#[auto_inc]` PK
/// + `by_character` btree. Durable (a learned ability persists across logout — no re-grant at login). [entity]
#[table(accessor = game_player_spell, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct PlayerSpell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub spell_id: u32,
}

// Character-owned sweeps: learned spells are deleted on character delete, re-owned
// (identity re-stamp) on a relog under a changed gateway identity.
crate::character_owned!(delete, fn sweep_delete_game_player_spell(ctx, character_guid) {
    let spells = ctx.db.game_player_spell();
    for r in spells.by_character().filter(&character_guid).collect::<Vec<_>>() {
        spells.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19), HOT: the spellbook is what the character can DO on arrival —
// without it the login spellbook is empty and every cast fails "spell not known". `id` is a
// surrogate PK, re-minted.
crate::character_owned!(transfer, fn sweep_transfer_game_player_spell(ctx, character_guid, io) {
    table = game_player_spell,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_player_spell(ctx, character_guid, identity) {
    let spells = ctx.db.game_player_spell();
    for mut r in spells.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            spells.id().update(r);
        }
    }
});

/// One createinfo entry: the spell a fresh character of a matching (race, class) starts with.
/// `race == 0` / `class == 0` are wildcards (a class kit row matches every race; a racial row matches
/// every class). Copied into `game_player_spell` at character creation — this table is never consulted
/// on the cast path, so editing it only affects characters created afterwards. Hand-authored seed data
/// (createinfo exists in no client DBC; values derived from the classic-db `playercreateinfo_spell`
/// dump — licensing firewall: derived, not bulk-imported). [reference]
#[table(accessor = game_createinfo_spell)]
pub struct CreateinfoSpell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub race: u8,
    pub class: u8,
    pub spell_id: u32,
}

/// Copy the createinfo kit for (`race`, `class`) into `guid`'s spellbook — the character-creation
/// grant. Idempotent via `learn_spell`. The table stays tiny (a few rows per class), so the full scan
/// with wildcard matching beats maintaining an index nobody else needs.
pub(crate) fn grant_createinfo_spells(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    race: u8,
    class: u8,
) {
    for row in ctx.db.game_createinfo_spell().iter() {
        if createinfo_row_matches(row.race, row.class, race, class) {
            learn_spell(ctx, guid, owner, row.spell_id);
        }
    }
}

/// Does a `game_createinfo_spell` row (`row_race`, `row_class`) apply to a fresh character of (`race`,
/// `class`)? Each side matches on EITHER an exact equal id OR the row's own 0-wildcard: `row_race == 0`
/// matches every race (a class-kit row); `row_class == 0` matches every class (a racial row); `(0, 0)`
/// matches everyone. Both sides must match for the row to apply. Pure — the wildcard join
/// `grant_createinfo_spells` scans is unit-tested here without a live ctx.
pub(crate) fn createinfo_row_matches(row_race: u8, row_class: u8, race: u8, class: u8) -> bool {
    (row_race == 0 || row_race == race) && (row_class == 0 || row_class == class)
}

/// Whether `guid` may cast `spell_id`: exactly the `game_player_spell` rows. The single spellbook gate
/// `cast_spell` enforces — creation grants the createinfo kit as rows, so there is no separate
/// "baseline" arm to drift from the stored book. [entity]
pub(crate) fn knows_spell(ctx: &ReducerContext, guid: u64, spell_id: u32) -> bool {
    ctx.db
        .game_player_spell()
        .by_character()
        .filter(&guid)
        .any(|s| s.spell_id == spell_id)
}

/// Apply the PASSIVE racial spells' auras for `race` at login (parallel to `apply_learned_talents`). Only
/// spells flagged `SPELL_ATTR_PASSIVE` in the imported `game_spell` are applied — read from data, never a
/// hardcoded list — so the always-on racials (The Human Spirit, weapon specs, Diplomacy) get their aura
/// while an ACTIVE racial (Perception) is left for on-demand cast. Idempotent: `apply_spell_auras`'s
/// `aura_apply` refreshes by effect, never stacks. A racial not yet imported (no `game_spell` row) is
/// skipped, so this is safe before the racials are in the DB. [entity]
pub(crate) fn apply_racial_passives(ctx: &ReducerContext, guid: u64, race: u8, level: u8) {
    // The race's createinfo rows (class == 0 wildcard) — the same set creation copied into the book.
    let racials: Vec<u32> = ctx
        .db
        .game_createinfo_spell()
        .iter()
        .filter(|r| r.class == 0 && (r.race == 0 || r.race == race))
        .map(|r| r.spell_id)
        .collect();
    for spell_id in racials {
        let Some(hdr) = ctx.db.game_spell().spell_id().find(spell_id) else {
            continue;
        };
        if hdr.attributes & crate::spell::taxonomy::SPELL_ATTR_PASSIVE != 0 {
            crate::spell::apply_spell_auras(ctx, spell_id, guid, level, 1);
        }
    }
}

/// Teach `guid` the spell `spell_id` — insert the row if absent (idempotent; never duplicates).
/// The single grant path: character creation (createinfo kit), ability talents, trainers, quest
/// rewards all come through here. [entity]
pub(crate) fn learn_spell(ctx: &ReducerContext, guid: u64, owner: Identity, spell_id: u32) {
    let spells = ctx.db.game_player_spell();
    if spells
        .by_character()
        .filter(&guid)
        .any(|s| s.spell_id == spell_id)
    {
        return; // already learned
    }
    spells.insert(PlayerSpell {
        id: 0,
        character_guid: guid,
        owner_identity: owner,
        spell_id,
    });
    // Pick Lock (119): learning it grants the Lockpicking (633) skill line, so the first pick_lock cast
    // reads a real skill row (a no-op for every other spell). Runs on the single grant seam.
    crate::skill::grant_lockpicking_on_learn(ctx, guid, owner, spell_id);
}

/// Forget `guid`'s `spell_id` — delete every `game_player_spell` row for it (collect-then-delete; never
/// duplicates, so at most one row exists, but the pattern matches every other book sweep in this crate).
/// The unlearn twin of [`learn_spell`]: talent supersede/respec are the current callers, and any future
/// unlearn source (GM strip, a gossip respec option) should route through here rather than re-deriving
/// the collect-then-delete dance. Does not touch auras — pair with [`strip_spell_auras`] when the spell
/// also applied a passive. [entity]
pub(crate) fn forget_spell(ctx: &ReducerContext, guid: u64, spell_id: u32) {
    let spells = ctx.db.game_player_spell();
    for row in spells
        .by_character()
        .filter(&guid)
        .filter(|s| s.spell_id == spell_id)
        .collect::<Vec<_>>()
    {
        spells.id().delete(row.id);
    }
}

/// Strip every `game_aura` row `guid` has from `spell_id` (collect-then-delete — never mutate mid-scan,
/// matches `scheduler::cancel_aura`'s pattern). Returns whether any removed aura moved vitals
/// (`aura_moves_vitals`) — the caller's cue to run exactly ONE `recompute_vitals` after stripping a batch
/// of spells, never per-spell (this function deliberately does NOT call `recompute_vitals` itself: a
/// respec strips several passives per unlearn and folding the flag across all of them keeps that a single
/// call, mirroring the pre-existing talent.rs call sites this replaces). A PASSIVE aura has no
/// `cancel_aura` path (it refuses a passive spell by design), so this is the only way to remove one
/// server-side. [entity]
pub(crate) fn strip_spell_auras(ctx: &ReducerContext, guid: u64, spell_id: u32) -> bool {
    let auras = ctx.db.game_aura();
    let to_remove: Vec<(u64, bool, bool)> = auras
        .by_target()
        .filter(&guid)
        .filter(|a| a.spell_id == spell_id)
        .map(|a| {
            (
                a.id,
                crate::spell::aura_moves_vitals(a.eff_kind, a.eff_p0),
                crate::mount::mount_aura_moves_mount(a.eff_kind, a.eff_p0),
            )
        })
        .collect();
    let mut moved_vitals = false;
    let mut moved_mount = false;
    for (id, mv, mm) in to_remove {
        auras.id().delete(id);
        moved_vitals |= mv;
        moved_mount |= mm;
    }
    // Unlike vitals, the mount recompute runs HERE rather than being folded back to the caller: it is
    // already gated on a mount aura having been removed (never true on the respec path, the only
    // caller that batches several spells), so per-spell costs nothing and no call site has to know.
    if moved_mount {
        crate::mount::recompute_mount(ctx, guid);
    }
    moved_vitals
}

/// A spell's rank-chain link within a spell family (e.g. Fireball Rank 1→2→3): `prev_spell` is the
/// immediately-lower rank (0 for the family's first rank), `first_spell` the family's rank-1 id, `rank`
/// this row's own ordinal, `req_spell` an additional required spell (unused this slice — read/stored for
/// completeness, not yet consulted by any gate). Read by `trainer::apply_trainer_buy`'s rank-prereq gate
/// (`by_first` is not used there today — kept for a future "list every rank in a family" query, the same
/// speculative-index posture `game_trainer_spell::by_trainer` set). Sourced from cmangos `spell_chain`
/// (work-item 102, reduced scope) — PUBLIC since 258 (rank supersession): the gateway subscribes it to
/// (a) send SMSG_SUPERCEDED_SPELL instead of LEARNED_SPELL when a trainer buy upgrades a known rank and
/// (b) collapse superseded ranks out of the login SMSG_INITIAL_SPELLS. No Timestamp → plain
/// `spacetime sql` clear+reload, exactly like `game_createinfo_spell`. [reference]
#[table(accessor = game_spell_chain, public, index(accessor = by_first, btree(columns = [first_spell])))]
pub struct SpellChain {
    #[primary_key]
    pub spell_id: u32,
    pub prev_spell: u32,
    pub first_spell: u32,
    pub rank: u8,
    pub req_spell: u32,
}

/// One "learning `parent_spell` also auto-teaches `learn_spell`" dependent (cmangos `spell_learn_spell`,
/// work-item 102 reduced scope) — e.g. a weapon-skill spell that auto-grants a companion passive. Read
/// by [`learn_spell_with_dependents`] via `by_parent`. Module-only, no Timestamp, unsubscribed — same
/// shape/rationale as `SpellChain` above. [reference]
#[table(accessor = game_spell_learn, index(accessor = by_parent, btree(columns = [parent_spell])))]
pub struct SpellLearn {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub parent_spell: u32,
    pub learn_spell: u32,
}

/// Learn `spell_id`, then auto-teach each ONE-LEVEL dependent listed in `game_spell_learn.by_parent`
/// (cmangos `spell_learn_spell`, work-item 102 reduced scope). ONE level only, deliberately not
/// recursive: cmangos classic data never nests an auto-learn chain more than one hop deep, so a second
/// lookup pass would only ever re-scan empty results — not worth paying for a deeper walk (or its
/// call-stack/cycle exposure) for data that doesn't need it. `learn_spell`'s own check-before-insert
/// already makes any individual grant idempotent (so even a malformed cyclic row could never double-teach
/// or loop within this one level), but capping the walk at one hop means we never even attempt a second
/// one. TRAINER-PATH ONLY — call this from `trainer::apply_trainer_buy`'s spell-grant arm, NEVER from
/// plain [`learn_spell`] (character creation / talents / quest rewards): firing dependents universally
/// would change the createinfo grant's spell COUNT and break work-item 212's provenance count-parity
/// runbook, which assumes createinfo grants exactly its listed rows. [entity]
pub(crate) fn learn_spell_with_dependents(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    spell_id: u32,
) {
    learn_spell(ctx, guid, owner, spell_id);
    let rows: Vec<(u32, u32)> = ctx
        .db
        .game_spell_learn()
        .by_parent()
        .filter(&spell_id)
        .map(|r| (r.parent_spell, r.learn_spell))
        .collect();
    for dep in dependent_spell_ids(&rows, spell_id) {
        learn_spell(ctx, guid, owner, dep);
    }
}

/// Projects `game_spell_learn`-shaped `(parent_spell, learn_spell)` pairs down to the dependent spell ids
/// taught alongside `parent_spell` — the pure part of `learn_spell_with_dependents`'s DB read, extracted
/// so the "ONE level of dependents" selection is unit-tested without a `ReducerContext` (the module crate
/// has no ctx harness by design). Named honestly: the ctx-bound `by_parent().filter(...)` read itself is
/// NOT covered by a test here — only this projection over already-fetched rows is.
pub(crate) fn dependent_spell_ids(rows: &[(u32, u32)], parent_spell: u32) -> Vec<u32> {
    rows.iter()
        .filter(|&&(parent, _)| parent == parent_spell)
        .map(|&(_, learn)| learn)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{createinfo_row_matches, dependent_spell_ids};
    use crate::seed::CREATEINFO_KIT;

    // --- work-item 102 (reduced scope): spell-chain / spell-learn dependents ------------------------

    /// `dependent_spell_ids` returns ONLY the rows whose parent matches, in row order, and is empty for
    /// a parent with no dependents at all (the "no game_spell_learn row" fallback: a plain rank-only
    /// spell with nothing to auto-teach, or an empty/un-imported table).
    #[test]
    fn dependent_spell_ids_filters_by_parent_and_is_empty_for_no_match_or_empty_table() {
        let rows = vec![(100u32, 200u32), (100, 201), (300, 400)];
        assert_eq!(dependent_spell_ids(&rows, 100), vec![200, 201]);
        assert_eq!(dependent_spell_ids(&rows, 300), vec![400]);
        assert_eq!(
            dependent_spell_ids(&rows, 999),
            Vec::<u32>::new(),
            "no dependents for an unrelated parent"
        );
        assert_eq!(
            dependent_spell_ids(&[], 100),
            Vec::<u32>::new(),
            "empty table -> no dependents, never a panic"
        );
    }

    /// An exact (race, class) match always applies, regardless of any wildcards elsewhere.
    #[test]
    fn createinfo_row_matches_exact_race_and_class() {
        assert!(createinfo_row_matches(1, 1, 1, 1)); // Human Warrior row on a Human Warrior
        assert!(!createinfo_row_matches(1, 1, 1, 4)); // same row, a Human Mage → class mismatch
        assert!(!createinfo_row_matches(1, 1, 3, 1)); // same row, a Dwarf Warrior → race mismatch
    }

    /// `row.race == 0` is a TRUE wildcard: a class-kit row (e.g. every Warrior's Heroic Strike) matches
    /// EVERY race, as long as the class still matches exactly.
    #[test]
    fn createinfo_row_matches_race_wildcard_matches_every_race() {
        assert!(createinfo_row_matches(0, 1, 1, 1)); // Human Warrior
        assert!(createinfo_row_matches(0, 1, 3, 1)); // Dwarf Warrior
        assert!(createinfo_row_matches(0, 1, 7, 1)); // Orc Warrior
        assert!(!createinfo_row_matches(0, 1, 1, 4)); // class still gates: a Human MAGE does not match
    }

    /// `row.class == 0` is a TRUE wildcard: a racial row (e.g. Human's The Human Spirit) matches EVERY
    /// class, as long as the race still matches exactly.
    #[test]
    fn createinfo_row_matches_class_wildcard_matches_every_class() {
        assert!(createinfo_row_matches(1, 0, 1, 1)); // Human Warrior
        assert!(createinfo_row_matches(1, 0, 1, 8)); // Human Priest
        assert!(!createinfo_row_matches(1, 0, 3, 1)); // race still gates: a Dwarf does not match a Human row
    }

    /// `(row.race, row.class) == (0, 0)` — both wildcards — matches EVERY (race, class) pair (a
    /// universal grant row, if one were ever seeded).
    #[test]
    fn createinfo_row_matches_double_wildcard_matches_everyone() {
        assert!(createinfo_row_matches(0, 0, 1, 1));
        assert!(createinfo_row_matches(0, 0, 7, 4));
        assert!(createinfo_row_matches(0, 0, 255, 255));
    }

    #[test]
    fn createinfo_kit_holds_starting_spells_and_nothing_trainer_taught() {
        let has = |race: u8, class: u8, id: u32| {
            CREATEINFO_KIT
                .iter()
                .any(|&(r, c, s)| r == race && c == class && s == id)
        };
        // The Warrior createinfo kit is just Heroic Strike + Battle Stance.
        assert!(has(0, 1, 78)); // Heroic Strike
        assert!(has(0, 1, 2457)); // Battle Stance
                                  // Human racials ride class-wildcard rows.
        assert!(has(1, 0, 20598)); // The Human Spirit
                                   // TRAINER-taught spells are NOT createinfo — they must be learned to cast.
        let ids: Vec<u32> = CREATEINFO_KIT.iter().map(|&(_, _, s)| s).collect();
        assert!(!ids.contains(&6673)); // Battle Shout
        assert!(!ids.contains(&871)); // Shield Wall
        assert!(!ids.contains(&51300)); // Death Wish (talent-granted)
        assert!(!ids.contains(&51000)); // Cruelty passive (applied, never player-cast)
    }
}
