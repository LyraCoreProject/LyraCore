use spacetimedb::{log, reducer, table, ReducerContext, Table};

use crate::spell::stacking::{game_spell_group, game_spell_group_rule};
use crate::{
    game_area, game_area_trigger, game_areatrigger_teleport, game_char_base_info,
    game_class_level_stats, game_createinfo_action, game_createinfo_spell, game_creature_family,
    game_faction, game_faction_template, game_graveyard, game_graveyard_zone, game_item_template,
    game_level_stats, game_lock, game_race_info, game_skill_ability, game_skill_availability,
    game_skill_line, game_spell, game_spell_chain, game_spell_effect, game_spell_learn,
    game_spell_reagent, game_start_item, game_start_position, game_talent, game_talent_tab,
    game_tavern_area_trigger,
};

#[table(accessor = game_catalogue_fingerprint, public)]
pub struct CatalogueFingerprint {
    #[primary_key]
    pub family: String,
    pub row_count: u64,
    pub fingerprint: u64,
    pub tables: String,
}

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

macro_rules! fold_family {
    ($ctx:expr, $h:expr, $n:expr; $($table:ident),+ $(,)?) => {{
        $( fold_table(&mut $h, &mut $n, stringify!($table), $ctx.db.$table().iter()); )+
        [$(stringify!($table)),+].join(",")
    }};
}

#[reducer]
pub fn debug_catalogue_fingerprint(ctx: &ReducerContext) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;

    let out = ctx.db.game_catalogue_fingerprint();
    for row in out.iter().collect::<Vec<_>>() {
        out.family().delete(&row.family);
    }

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

    {
        let mut h = DefaultHasher::new();
        let mut n = 0u64;
        let tables = fold_family!(ctx, h, n;
            game_area,
            game_area_trigger,
            game_tavern_area_trigger,
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
