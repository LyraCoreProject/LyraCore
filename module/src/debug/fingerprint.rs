//! Catalogue parity fingerprint — content-hash the tables that are supposed to be
//! IDENTICAL on every shard (spells, items, DBC-derived reference tables), so a partial or stale
//! re-import shows up as a loud mismatch instead of "this spell behaves differently in Durotar".
//! Row counts alone are not enough (a count matches trivially while contents differ): every row is
//! BSATN-encoded — the same canonical byte encoding SpacetimeDB already uses to store/diff rows
//! (`spacetimedb::sats::bsatn`), so this needs no hand-written per-table serializer — and the SORTED
//! set of encodings for a table is folded into one hash with `DefaultHasher` (stdlib, no new
//! dependency; deterministic for identical input on the SAME compiled wasm, which is exactly the
//! "did every shard run the same import?" question that the internal cross-database parity check
//! uses this table to answer).

use spacetimedb::{log, reducer, table, ReducerContext, Table};

use crate::spell::stacking::{game_spell_group, game_spell_group_rule};
use crate::{
    game_area, game_area_trigger, game_areatrigger_teleport, game_char_base_info,
    game_class_level_stats, game_createinfo_action, game_createinfo_spell, game_creature_family,
    game_faction, game_faction_template, game_graveyard, game_graveyard_zone, game_item_template,
    game_level_stats, game_lock, game_race_info, game_skill_ability, game_skill_availability,
    game_skill_line, game_spell, game_spell_chain, game_spell_effect, game_spell_learn,
    game_spell_reagent, game_start_item, game_start_position, game_talent, game_talent_tab,
};

/// A family's content fingerprint, recomputed wholesale by [`debug_catalogue_fingerprint`]. Public so
/// `spacetime sql` (or an equivalent cross-database parity check) can read it per database. [reference]
#[table(accessor = game_catalogue_fingerprint, public)]
pub struct CatalogueFingerprint {
    #[primary_key]
    pub family: String,
    pub row_count: u64,
    pub fingerprint: u64,
    /// Comma-separated table list actually hashed — so a family definition drifting between two
    /// module versions (a table added/renamed here but not there) shows up as a visible diff
    /// instead of a silent apples-to-oranges compare.
    pub tables: String,
}

/// Fold one table's rows into `hasher`/`count`: BSATN-encode every row, SORT the encodings (content
/// order, not iteration order — SpacetimeDB does not guarantee `.iter()` order is stable across
/// deployments/inserts), then hash the table name followed by the sorted encodings. The table name is
/// hashed first so two families with the same total content but a table added/renamed still differ.
fn fold_table<Row: spacetimedb::Serialize>(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    count: &mut u64,
    table_name: &str,
    rows: impl Iterator<Item = Row>,
) {
    use std::hash::Hash;
    table_name.hash(hasher);
    let mut encoded: Vec<Vec<u8>> = rows
        .map(|r| {
            spacetimedb::sats::bsatn::to_vec(&r)
                .expect("row must BSATN-encode (already required for SpacetimeDB row storage)")
        })
        .collect();
    encoded.sort();
    *count += encoded.len() as u64;
    encoded.hash(hasher);
}

/// Fold a CURATED family of tables into one hash+count+comma-joined table list in one shot — the
/// local macro `debug_catalogue_fingerprint`'s per-family blocks expand to, replacing what used to be
/// a hand-written `fold_table(&mut h, &mut n, "name", ctx.db.name().iter())` call PER TABLE (6 lines
/// each, rustfmt-exploded on the `impl Iterator` arg) with one line naming the members. `$ctx.db
/// .$table().iter()` reproduces exactly what those hand-written calls did; the returned `tables`
/// string is the same comma-joined list the deleted code built by hand, in the same member order.
macro_rules! fold_family {
    ($ctx:expr, $h:expr, $n:expr; $($table:ident),+ $(,)?) => {{
        $( fold_table(&mut $h, &mut $n, stringify!($table), $ctx.db.$table().iter()); )+
        [$(stringify!($table)),+].join(",")
    }};
}

/// Recompute all catalogue fingerprints (clear+reinsert — idempotent, call any time). Driven once per
/// connected shard as part of the deploy/verify path by an internal cross-database parity check. The
/// family list below is deliberately curated — see the per-family comments for why each table is (or,
/// for the excluded ones, is not) included.
#[reducer]
pub fn debug_catalogue_fingerprint(ctx: &ReducerContext) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;

    let out = ctx.db.game_catalogue_fingerprint();
    for row in out.iter().collect::<Vec<_>>() {
        out.family().delete(&row.family);
    }

    // SPELLS — Spell.dbc (`importer/src/spell.rs::run_spells`) plus its box-INDEPENDENT dump
    // companions (spell_chain/spell_learn_spell/createinfo_spell — none of their `build_*_sql`
    // functions in `importer/src/main.rs` take a box/local-entry argument), plus the hand-seeded
    // stacking-group starter set (`module/src/seed.rs`), which can independently skew if
    // `debug_reseed_stacking_groups` was run on one shard's auto-migration and not another's.
    {
        let mut h = DefaultHasher::new();
        let mut n = 0u64;
        let tables = fold_family!(ctx, h, n;
            game_spell,
            game_spell_effect,
            game_spell_reagent,
            game_spell_chain,
            game_spell_learn,
            game_spell_group,
            game_spell_group_rule,
            game_createinfo_spell,
        );
        out.insert(CatalogueFingerprint {
            family: "spells".into(),
            row_count: n,
            fingerprint: h.finish(),
            tables,
        });
    }

    // ITEMS — item_template is imported WHOLE regardless of box (`importer/src/main.rs`: "3)
    // item_template rows — the FULL vanilla item set"); every shard should carry the identical set.
    {
        let mut h = DefaultHasher::new();
        let mut n = 0u64;
        let tables = fold_family!(ctx, h, n; game_item_template);
        out.insert(CatalogueFingerprint {
            family: "items".into(),
            row_count: n,
            fingerprint: h.finish(),
            tables,
        });
    }

    // DBC_REFERENCE — every table the client-DBC importer (`importer/src/dbc.rs`) produces, plus the
    // handful of cmangos-dump tables that are ALSO box-independent (`build_start_position_sql` /
    // `build_graveyard_zone_sql` / `build_areatrigger_teleport_sql` / `build_createinfo_action_sql`,
    // none of which take a box/local-entry argument) — world-wide reference data with no geographic
    // scoping, so every shard should carry the identical set.
    {
        let mut h = DefaultHasher::new();
        let mut n = 0u64;
        let tables = fold_family!(ctx, h, n;
            game_area,
            game_area_trigger,
            game_areatrigger_teleport,
            game_faction,
            game_faction_template,
            game_graveyard,
            game_graveyard_zone,
            game_creature_family,
            game_lock,
            game_skill_line,
            game_skill_ability,
            game_skill_availability,
            game_race_info,
            game_char_base_info,
            game_start_item,
            game_start_position,
            game_createinfo_action,
            game_talent,
            game_talent_tab,
            game_class_level_stats,
            game_level_stats,
        );
        out.insert(CatalogueFingerprint {
            family: "dbc_reference".into(),
            row_count: n,
            fingerprint: h.finish(),
            tables,
        });
    }

    log::info!("debug_catalogue_fingerprint: recomputed 3 families (spells, items, dbc_reference)");
}
