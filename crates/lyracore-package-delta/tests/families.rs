//! The table catalogue and the Import Family each table belongs to.
//!
//! An applier is called for one family at a time, so `Table::family` decides which claims that call
//! may touch. These cases hold the catalogue, the parser and the family map together.

use lyracore_package_delta::{
    Table, CAST_FAMILY, GLOBALS_FAMILY, GOSSIP_FAMILY, ITEM_FAMILY, LOOT_FAMILY, QUEST_FAMILY,
    SPELLMETA_FAMILY, SPELL_FAMILY, TRAINER_FAMILY,
};

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
            Table::Quest => "game_quest_template",
            Table::QuestText => "game_quest_text",
            Table::QuestObjective => "game_quest_objective",
            Table::QuestCastObjective => "game_quest_cast_objective",
            Table::QuestRewardItem => "game_quest_reward_item",
            Table::QuestRewardChoice => "game_quest_reward_choice",
            Table::PickpocketLoot => "game_pickpocket_loot",
            Table::GameobjectLoot => "game_gameobject_loot",
            Table::SkinningLoot => "game_skinning_loot",
            Table::FishingLoot => "game_fishing_loot",
            Table::CreatureCast => "game_creature_cast",
            Table::CreatureSpell => "game_creature_spell",
            Table::TrainerSpell => "game_trainer_spell",
            Table::GossipMenu => "game_gossip_menu",
            Table::GossipMenuProfile => "game_gossip_menu_profile",
            Table::GossipMenuProfileOption => "game_gossip_menu_profile_option",
            Table::GossipOption => "game_gossip_option",
            Table::NpcText => "game_npc_text",
            Table::NpcTextSlot => "game_npc_text_slot",
            Table::ClassLevelStats => "game_class_level_stats",
            Table::LevelStats => "game_level_stats",
            Table::StartPosition => "game_start_position",
            Table::GraveyardZone => "game_graveyard_zone",
            Table::AreatriggerTeleport => "game_areatrigger_teleport",
            Table::CreateinfoSpell => "game_createinfo_spell",
            Table::CreateinfoAction => "game_createinfo_action",
            Table::SpellChain => "game_spell_chain",
            Table::SpellLearn => "game_spell_learn",
            Table::SpellProcEvent => "game_spell_proc_event",
        };

        assert_eq!(table.as_str(), name);
        assert_eq!(Table::parse(name), Some(*table));
    }

    assert_eq!(
        Table::ALL.len(),
        32,
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

#[test]
fn the_quest_tables_belong_to_the_quests_import_family() {
    for table in [
        Table::Quest,
        Table::QuestText,
        Table::QuestObjective,
        Table::QuestCastObjective,
        Table::QuestRewardItem,
        Table::QuestRewardChoice,
    ] {
        assert_eq!(table.family(), QUEST_FAMILY, "{table}");
    }
    assert_eq!(QUEST_FAMILY, "quests");
}

#[test]
fn the_loot_tables_belong_to_the_loot_import_family() {
    for table in [
        Table::PickpocketLoot,
        Table::GameobjectLoot,
        Table::SkinningLoot,
        Table::FishingLoot,
    ] {
        assert_eq!(table.family(), LOOT_FAMILY, "{table}");
    }
    assert_eq!(LOOT_FAMILY, "loot");
}

#[test]
fn the_cast_tables_belong_to_the_casts_import_family() {
    for table in [Table::CreatureCast, Table::CreatureSpell] {
        assert_eq!(table.family(), CAST_FAMILY, "{table}");
    }
    assert_eq!(CAST_FAMILY, "casts");
}

#[test]
fn the_trainer_table_belongs_to_the_trainers_import_family() {
    assert_eq!(Table::TrainerSpell.family(), TRAINER_FAMILY);
    assert_eq!(TRAINER_FAMILY, "trainers");
}

#[test]
fn the_gossip_tables_belong_to_the_gossip_import_family() {
    for table in [
        Table::GossipMenu,
        Table::GossipMenuProfile,
        Table::GossipMenuProfileOption,
        Table::GossipOption,
        Table::NpcText,
        Table::NpcTextSlot,
    ] {
        assert_eq!(table.family(), GOSSIP_FAMILY, "{table}");
    }
    assert_eq!(GOSSIP_FAMILY, "gossip");
}

#[test]
fn the_globals_tables_belong_to_the_globals_import_family() {
    for table in [
        Table::ClassLevelStats,
        Table::LevelStats,
        Table::StartPosition,
        Table::GraveyardZone,
        Table::AreatriggerTeleport,
        Table::CreateinfoSpell,
        Table::CreateinfoAction,
    ] {
        assert_eq!(table.family(), GLOBALS_FAMILY, "{table}");
    }
    assert_eq!(GLOBALS_FAMILY, "globals");
}

/// The spell metadata tables belong to `spellmeta`, NOT to `spell`, even though two of the three
/// key on a spell identifier. A base import reloads them in their own family's block, so an apply
/// that ran them under `spell` would be reverted by the block that owns them.
#[test]
fn the_spell_metadata_tables_belong_to_the_spellmeta_import_family() {
    for table in [Table::SpellChain, Table::SpellLearn, Table::SpellProcEvent] {
        assert_eq!(table.family(), SPELLMETA_FAMILY, "{table}");
    }
    assert_eq!(SPELLMETA_FAMILY, "spellmeta");
    assert_ne!(Table::SpellChain.family(), Table::Spell.family());
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
