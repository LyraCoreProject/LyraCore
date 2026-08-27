//! The table catalogue and the Import Family each table belongs to.
//!
//! An applier is called for one family at a time, so `Table::family` decides which claims that call
//! may touch. These cases hold the catalogue, the parser and the family map together.

use lyracore_package_delta::{Table, ITEM_FAMILY, SPELL_FAMILY};

/// `Table::ALL`, `Table::as_str` and `Table::parse` are three hand-maintained lists of one
/// catalogue. The match below carries no wildcard, so a table added to the enum stops this case
/// compiling until all three learn it.
#[test]
fn every_table_in_the_catalogue_parses_back_to_itself() {
    for table in Table::ALL {
        let name = match table {
            Table::Spell => "game_spell",
            Table::SpellEffect => "game_spell_effect",
            Table::Item => "game_item_template",
        };

        assert_eq!(table.as_str(), name);
        assert_eq!(Table::parse(name), Some(*table));
    }

    assert_eq!(
        Table::ALL.len(),
        3,
        "a table reached the enum without reaching `Table::ALL`"
    );
}

#[test]
fn the_spell_tables_belong_to_the_spell_import_family() {
    assert_eq!(Table::Spell.family(), SPELL_FAMILY);
    assert_eq!(Table::SpellEffect.family(), SPELL_FAMILY);
    assert_eq!(SPELL_FAMILY, "spell");
}

#[test]
fn the_item_table_belongs_to_the_items_import_family() {
    assert_eq!(Table::Item.family(), ITEM_FAMILY);
    assert_eq!(ITEM_FAMILY, "items");
}

/// A family name travels as a reducer argument and as `game_import_meta.family`, where the importer
/// writes it in lowercase. A name that did not match would route an apply at nothing.
#[test]
fn every_family_name_is_a_lowercase_import_family_name() {
    for table in Table::ALL {
        let family = table.family();

        assert!(!family.is_empty(), "`{table}` names no family");
        assert!(
            family.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
            "`{table}` names family `{family}`"
        );
    }
}

/// A table with no claimable columns would accept an `insert` that carries nothing.
#[test]
fn every_table_has_claimable_columns() {
    for table in Table::ALL {
        assert!(
            !table.columns().is_empty(),
            "`{table}` has no claimable column"
        );
    }
}
