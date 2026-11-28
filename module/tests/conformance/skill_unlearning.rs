// Behavior checks for CMSG_UNLEARN_SKILL against vmangos d9173eca
// WorldSession::HandleUnlearnSkillOpcode.
use lyracore_test_support::{ReducerConnection, Standalone};
use wire_client::decode::Field;

use super::buy_in_slot::request;
use super::{enter, fixture};

const CMSG_UNLEARN_SKILL: u32 = 0x202;
const SMSG_REMOVED_SPELL: u16 = 0x203;
const SMSG_UPDATE_OBJECT: u16 = 0xa9;
const PLAYER_CHARACTER_POINTS2: &str = "field_1103";
const ALCHEMY: u32 = 171;
const UNLEARNABLE: u32 = 0x20;
const MAGE_CLASS_MASK: u32 = 0x80;

const APPRENTICE_SPELL: u32 = 900_801;
const JOURNEYMAN_SPELL: u32 = 900_802;
const EXPERT_SPELL: u32 = 900_803;
const TAUGHT_SPELL: u32 = 900_804;
const OTHER_SPELL: u32 = 900_805;
const REQUIRING_SPELL: u32 = 900_806;
const ALCHEMY_QUEST: u32 = 900_810;
const OTHER_QUEST: u32 = 900_811;

struct Stage {
    shard: Standalone,
    connection: ReducerConnection,
    character: u64,
}

fn availability(shard: &Standalone, id: u32, class_mask: u32, flags: u32) {
    shard.insert_row(
        "game_skill_availability",
        &[
            ("id", &id),
            ("skill_line", &ALCHEMY),
            ("class_mask", &class_mask),
            ("flags", &flags),
        ],
    );
}

fn stage(name: &str) -> Stage {
    let (shard, character, _) = fixture(name);
    shard.assert_call("debug_spawn_player_entity", &[&character.to_string()]);
    for (id, spell) in [APPRENTICE_SPELL, JOURNEYMAN_SPELL].into_iter().enumerate() {
        shard.insert_row(
            "game_skill_ability",
            &[
                ("id", &(id as u32 + 1)),
                ("spell_id", &spell),
                ("skill_line", &ALCHEMY),
                ("acquire_method", &1),
                ("min_skill", &1),
            ],
        );
    }
    shard.insert_row(
        "game_spell_chain",
        &[
            ("spell_id", &EXPERT_SPELL),
            ("prev_spell", &JOURNEYMAN_SPELL),
            ("first_spell", &APPRENTICE_SPELL),
            ("rank", &3),
        ],
    );
    shard.insert_row(
        "game_spell_chain",
        &[
            ("spell_id", &JOURNEYMAN_SPELL),
            ("prev_spell", &APPRENTICE_SPELL),
            ("first_spell", &APPRENTICE_SPELL),
            ("rank", &2),
        ],
    );
    shard.insert_row(
        "game_spell_chain",
        &[
            ("spell_id", &REQUIRING_SPELL),
            ("first_spell", &REQUIRING_SPELL),
            ("rank", &1),
            ("req_spell", &APPRENTICE_SPELL),
        ],
    );
    shard.insert_row(
        "game_spell_learn",
        &[
            ("parent_spell", &APPRENTICE_SPELL),
            ("learn_spell", &TAUGHT_SPELL),
        ],
    );
    for (quest, skill) in [(ALCHEMY_QUEST, ALCHEMY), (OTHER_QUEST, 0)] {
        shard.insert_row(
            "game_quest_template",
            &[
                ("entry", &quest),
                ("quest_level", &2),
                ("title", &"Unlearning fixture"),
                ("required_skill", &skill),
            ],
        );
    }
    let connection = enter(&shard, character);
    Stage {
        shard,
        connection,
        character,
    }
}

impl Stage {
    fn learn_alchemy(&self) {
        let character = self.character.to_string();
        self.shard.assert_call(
            "debug_learn_profession",
            &[&character, &ALCHEMY.to_string(), "75"],
        );
        for spell in [EXPERT_SPELL, TAUGHT_SPELL, REQUIRING_SPELL, OTHER_SPELL] {
            self.shard
                .assert_call("debug_learn_spell", &[&character, &spell.to_string()]);
        }
        for quest in [ALCHEMY_QUEST, OTHER_QUEST] {
            self.shard
                .assert_call("debug_grant_quest", &[&character, &quest.to_string()]);
        }
    }

    fn unlearn(&mut self, skill: u32) -> Vec<(u16, Vec<u8>)> {
        self.connection.drain_packets();
        request(
            &mut self.connection,
            CMSG_UNLEARN_SKILL,
            skill.to_le_bytes().to_vec(),
        )
    }

    fn known_spells(&self) -> Vec<u32> {
        let mut spells: Vec<u32> = self
            .shard
            .query_rows(&format!(
                "SELECT spell_id FROM game_player_spell WHERE character_guid = {}",
                self.character
            ))
            .iter()
            .map(|row| row["spell_id"].parse().unwrap())
            .collect();
        spells.sort_unstable();
        spells
    }

    fn skill_rows(&self) -> usize {
        self.shard
            .query_rows(&format!(
                "SELECT id FROM game_player_skill WHERE character_guid = {} AND skill_line = {ALCHEMY}",
                self.character
            ))
            .len()
    }

    fn quests(&self) -> Vec<u32> {
        let mut quests: Vec<u32> = self
            .shard
            .query_rows(&format!(
                "SELECT quest_entry FROM game_character_quest WHERE character_guid = {}",
                self.character
            ))
            .iter()
            .map(|row| row["quest_entry"].parse().unwrap())
            .collect();
        quests.sort_unstable();
        quests
    }
}

fn free_primary_professions(packets: &[(u16, Vec<u8>)], guid: u64) -> Option<u64> {
    packets
        .iter()
        .filter(|(opcode, _)| *opcode == SMSG_UPDATE_OBJECT)
        .filter_map(|(opcode, body)| {
            let packet = wire_client::decode::decode(*opcode, body).unwrap();
            let Some(Field::List(objects)) = packet.fields.get("objects") else {
                return None;
            };
            objects.iter().find_map(|object| {
                let Field::Object(object) = object else {
                    return None;
                };
                let Some(Field::Object(values)) = object.get("values") else {
                    return None;
                };
                if object.get("guid") != Some(&Field::Guid(guid)) {
                    return None;
                }
                match values.get(PLAYER_CHARACTER_POINTS2) {
                    Some(Field::Unsigned(free)) => Some(*free),
                    _ => None,
                }
            })
        })
        .next_back()
}

fn removed_spells(packets: &[(u16, Vec<u8>)]) -> Vec<u16> {
    let mut removed: Vec<u16> = packets
        .iter()
        .filter(|(opcode, _)| *opcode == SMSG_REMOVED_SPELL)
        .map(|(_, body)| u16::from_le_bytes([body[0], body[1]]))
        .collect();
    removed.sort_unstable();
    removed
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn unlearning_a_skill_removes_its_spells_ranks_taught_spells_and_quests() {
    let mut stage = stage("skill-unlearning-removes");
    availability(&stage.shard, 1, 0, UNLEARNABLE);
    stage.learn_alchemy();
    assert_eq!(stage.skill_rows(), 1);
    let packets = stage.unlearn(ALCHEMY);
    assert_eq!(stage.skill_rows(), 0);
    assert_eq!(stage.known_spells(), vec![OTHER_SPELL]);
    assert_eq!(stage.quests(), vec![OTHER_QUEST]);
    assert_eq!(
        removed_spells(&packets),
        vec![
            APPRENTICE_SPELL as u16,
            JOURNEYMAN_SPELL as u16,
            EXPERT_SPELL as u16,
            TAUGHT_SPELL as u16,
            REQUIRING_SPELL as u16
        ]
    );
    assert_eq!(
        free_primary_professions(&packets, stage.character),
        Some(2),
        "Alchemy held one of the two primary profession slots"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_skill_the_race_and_class_may_not_unlearn_is_kept() {
    let mut stage = stage("skill-unlearning-kept");
    availability(&stage.shard, 1, 0, 0);
    stage.learn_alchemy();
    let spells = stage.known_spells();
    let packets = stage.unlearn(ALCHEMY);
    assert_eq!(stage.skill_rows(), 1);
    assert_eq!(stage.known_spells(), spells);
    assert_eq!(stage.quests(), vec![ALCHEMY_QUEST, OTHER_QUEST]);
    assert!(
        packets.is_empty(),
        "an ignored request answers nothing: {packets:?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_first_availability_row_that_admits_the_character_decides() {
    let mut stage = stage("skill-unlearning-first-row");
    availability(&stage.shard, 1, MAGE_CLASS_MASK, UNLEARNABLE);
    availability(&stage.shard, 2, 0, 0);
    stage.learn_alchemy();
    stage.unlearn(ALCHEMY);
    assert_eq!(
        stage.skill_rows(),
        1,
        "a Warrior skips the Mage row and meets the row that forbids unlearning"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn unlearning_a_skill_the_character_lacks_removes_nothing() {
    let mut stage = stage("skill-unlearning-lacks");
    availability(&stage.shard, 1, 0, UNLEARNABLE);
    stage.learn_alchemy();
    stage.shard.assert_sql(&format!(
        "DELETE FROM game_player_skill WHERE character_guid = {} AND skill_line = {ALCHEMY}",
        stage.character
    ));
    let spells = stage.known_spells();
    let packets = stage.unlearn(ALCHEMY);
    assert_eq!(stage.known_spells(), spells);
    assert_eq!(stage.quests(), vec![ALCHEMY_QUEST, OTHER_QUEST]);
    assert!(removed_spells(&packets).is_empty());
}
