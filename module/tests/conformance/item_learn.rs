// Behavior checks for items that teach a spell against vmangos d9173eca Player::CanUseItem.
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{E_LEARN_SPELL, E_SCRIPTED, T_CASTER_PET, T_TARGET_ALLY};
use lyracore_test_support::{Field, Standalone};

use super::fixture;

const BOOK: u32 = 900_701;
const TEACHER: u32 = 900_702;
const TAUGHT: u32 = 900_703;
const REQUIRED_SPELL: u32 = 900_704;
const TAILORING: u32 = 197;
const MAGE_CLASS_MASK: u32 = 0x80;
const FACTION: u32 = 900_710;
const FRIENDLY_RANK: u32 = 4;
const FRIENDLY_STANDING: i32 = 3000;

struct Stage {
    shard: Standalone,
    character: u64,
}

fn stage(name: &str, learn_kind: u8, target: u8) -> Stage {
    let (shard, character, _) = fixture(name);
    shard.assert_call("debug_spawn_player_entity", &[&character.to_string()]);
    for (spell_id, spell_name) in [(TEACHER, "Learn fixture"), (TAUGHT, "Taught fixture")] {
        shard.insert_row(
            "game_spell",
            &[
                ("spell_id", &spell_id),
                ("name", &spell_name),
                ("spell_level", &1),
            ],
        );
    }
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(TEACHER, 0)),
            ("spell_id", &TEACHER),
            ("kind", &learn_kind),
            ("target", &target),
            ("trigger_spell", &TAUGHT),
        ],
    );
    Stage { shard, character }
}

impl Stage {
    fn book(&self, requirement: &[(&str, u32)]) -> String {
        let mut fields: Vec<Field> = vec![
            ("entry", &BOOK),
            ("class", &9),
            ("name", &"Fixture book"),
            ("max_stack", &1),
            ("spellid_1", &TEACHER),
        ];
        for (column, value) in requirement {
            fields.push((*column, value));
        }
        self.shard.insert_row("game_item_template", &fields);
        self.shard.assert_call(
            "debug_grant_item",
            &[&self.character.to_string(), &BOOK.to_string(), "1"],
        );
        self.shard.query_rows(&format!(
            "SELECT slot FROM game_item_instance WHERE owner_guid = {} AND entry = {BOOK}",
            self.character
        ))[0]["slot"]
            .clone()
    }

    fn use_book(&self, slot: &str) -> Result<(), String> {
        let output = self
            .shard
            .call("debug_use_item", &[&self.character.to_string(), slot]);
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        }
    }

    fn knows_taught_spell(&self) -> bool {
        !self
            .shard
            .query_rows(&format!(
                "SELECT id FROM game_player_spell WHERE character_guid = {} AND spell_id = {TAUGHT}",
                self.character
            ))
            .is_empty()
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pattern_teaches_only_a_character_with_its_profession() {
    let stage = stage("item-learn-pattern", E_LEARN_SPELL, T_TARGET_ALLY);
    let slot = stage.book(&[("required_skill", TAILORING), ("required_skill_rank", 1)]);

    let refusal = stage.use_book(&slot).unwrap_err();
    assert!(refusal.contains("item:no_proficiency"), "{refusal}");
    assert!(!stage.knows_taught_spell());

    stage.shard.assert_call(
        "debug_learn_profession",
        &[&stage.character.to_string(), &TAILORING.to_string(), "75"],
    );
    stage.use_book(&slot).unwrap();
    assert!(stage.knows_taught_spell());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_warrior_can_never_use_a_mage_tome() {
    let stage = stage("item-learn-class", E_LEARN_SPELL, T_TARGET_ALLY);
    let slot = stage.book(&[("allowed_class", MAGE_CLASS_MASK)]);

    let refusal = stage.use_book(&slot).unwrap_err();
    assert!(refusal.contains("item:item_not_usable"), "{refusal}");
    assert!(!stage.knows_taught_spell());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_grimoire_teaches_its_user_nothing() {
    // The importer keeps a learn effect aimed at the caster's pet as E_SCRIPTED.
    let stage = stage("item-learn-grimoire", E_SCRIPTED, T_CASTER_PET);
    let slot = stage.book(&[]);

    let _ = stage.use_book(&slot);
    assert!(!stage.knows_taught_spell());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_reputation_pattern_teaches_only_from_its_required_rank() {
    let stage = stage("item-learn-reputation", E_LEARN_SPELL, T_TARGET_ALLY);
    stage.shard.insert_row(
        "game_faction",
        &[
            ("faction_id", &FACTION),
            ("reputation_index", &50),
            ("base_standing", &0),
        ],
    );
    let slot = stage.book(&[
        ("required_reputation_faction", FACTION),
        ("required_reputation_rank", FRIENDLY_RANK),
    ]);

    let refusal = stage.use_book(&slot).unwrap_err();
    assert!(refusal.contains("item:required_reputation"), "{refusal}");
    assert!(!stage.knows_taught_spell());

    stage.shard.assert_call(
        "debug_grant_reputation",
        &[
            &stage.character.to_string(),
            &FACTION.to_string(),
            &FRIENDLY_STANDING.to_string(),
        ],
    );
    stage.use_book(&slot).unwrap();
    assert!(stage.knows_taught_spell());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_needing_a_spell_teaches_only_a_character_who_knows_it() {
    let stage = stage("item-learn-required-spell", E_LEARN_SPELL, T_TARGET_ALLY);
    stage.shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &REQUIRED_SPELL),
            ("name", &"Required fixture"),
            ("spell_level", &1),
        ],
    );
    let slot = stage.book(&[("required_spell", REQUIRED_SPELL)]);

    let refusal = stage.use_book(&slot).unwrap_err();
    assert!(refusal.contains("item:no_proficiency"), "{refusal}");
    assert!(!stage.knows_taught_spell());

    stage.shard.assert_call(
        "debug_learn_spell",
        &[&stage.character.to_string(), &REQUIRED_SPELL.to_string()],
    );
    stage.use_book(&slot).unwrap();
    assert!(stage.knows_taught_spell());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_needing_an_honor_rank_is_refused_a_character_without_one() {
    let stage = stage("item-learn-honor-rank", E_LEARN_SPELL, T_TARGET_ALLY);
    let slot = stage.book(&[("required_honor_rank", 1)]);

    let refusal = stage.use_book(&slot).unwrap_err();
    assert!(refusal.contains("item:cant_equip_rank"), "{refusal}");
    assert!(!stage.knows_taught_spell());
}
