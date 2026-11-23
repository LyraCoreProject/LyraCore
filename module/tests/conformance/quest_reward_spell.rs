// Behavior checks for the quest reward spell against vmangos d9173eca Player::RewardQuest.
use lyracore_package_delta::{fixture_ids::SCENARIO_QUESTGIVER_ENTRY, packed_spell_effect_id};
use lyracore_shared::constants::npc_flags;
use lyracore_shared::spell::taxonomy::{
    A_MOD_STAT, E_LEARN_SPELL, P_STAT_ID, STAT_STR, T_SELF, T_TARGET_ALLY,
};
use lyracore_test_support::{ReducerConnection, Standalone};

use super::buy_in_slot::request;
use super::{enter, fixture};

const QUEST: u32 = 5_091_300;
const ALLY_BUFF: u32 = 5_091_301;
const SELF_BUFF: u32 = 5_091_302;
const TEACHER: u32 = 5_091_303;
const FIRST_TAUGHT: u32 = 5_091_304;
const SECOND_TAUGHT: u32 = 5_091_305;

const CMSG_QUESTGIVER_REQUEST_REWARD: u32 = 0x18C;
const CMSG_QUESTGIVER_CHOOSE_REWARD: u32 = 0x18E;
const SMSG_QUESTGIVER_OFFER_REWARD: u16 = 0x18D;

struct Stage {
    shard: Standalone,
    connection: ReducerConnection,
    character: u64,
    giver: u64,
}

fn stage(name: &str, reward_spell: u32, reward_spell_cast: u32) -> Stage {
    let (shard, character, _) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    // The seeded chicken and wolf wander into the replies unless the creature cycle stops.
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call("debug_spawn_player_entity", &[&character.to_string()]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[
            &character.to_string(),
            &SCENARIO_QUESTGIVER_ENTRY.to_string(),
            "2.0",
        ],
    );
    let giver: u64 = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {SCENARIO_QUESTGIVER_ENTRY}"
        ))
        .iter()
        .map(|row| row["guid"].parse().unwrap())
        .max()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {}, faction_template = 35 WHERE guid = {giver}",
        npc_flags::QUESTGIVER
    ));
    shard.insert_row(
        "game_quest_template",
        &[
            ("entry", &QUEST),
            ("quest_level", &2),
            ("title", &"Reward spell quest"),
            ("zone_or_sort", &12),
            ("reward_spell", &reward_spell),
            ("reward_spell_cast", &reward_spell_cast),
        ],
    );
    shard.insert_row(
        "game_creature_quest",
        &[
            ("id", &u64::from(QUEST)),
            ("creature_entry", &SCENARIO_QUESTGIVER_ENTRY),
            ("quest_entry", &QUEST),
            ("role", &1),
        ],
    );
    shard.assert_call(
        "debug_grant_quest",
        &[&character.to_string(), &QUEST.to_string()],
    );
    let mut connection = enter(&shard, character);
    connection.drain_packets();
    Stage {
        shard,
        connection,
        character,
        giver,
    }
}

fn spell(shard: &Standalone, spell_id: u32, effects: &[(u8, u8, u32)]) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &format!("Reward spell fixture {spell_id}")),
            ("school_mask", &1),
            ("range_yd", &30),
            ("duration_ms", &60_000),
            ("spell_level", &1),
        ],
    );
    for (index, &(kind, target, trigger_spell)) in effects.iter().enumerate() {
        shard.insert_row(
            "game_spell_effect",
            &[
                ("id", &packed_spell_effect_id(spell_id, index as u8)),
                ("spell_id", &spell_id),
                ("effect_index", &(index as u8)),
                ("kind", &kind),
                ("base_points", &5),
                ("target", &target),
                ("trigger_spell", &trigger_spell),
                ("p_0", &i32::from(STAT_STR)),
                ("p_0_kind", &P_STAT_ID),
            ],
        );
    }
}

fn buff(shard: &Standalone, spell_id: u32, target: u8) {
    spell(shard, spell_id, &[(A_MOD_STAT, target, 0)]);
}

fn turn_in(stage: &mut Stage) -> Vec<(u16, Vec<u8>)> {
    let mut body = stage.giver.to_le_bytes().to_vec();
    body.extend(QUEST.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    request(&mut stage.connection, CMSG_QUESTGIVER_CHOOSE_REWARD, body)
}

fn aura_casters(shard: &Standalone, target: u64, spell_id: u32) -> Vec<String> {
    shard
        .query_rows(&format!(
            "SELECT caster_guid FROM game_aura WHERE target_guid = {target} AND spell_id = {spell_id}"
        ))
        .into_iter()
        .map(|row| row["caster_guid"].clone())
        .collect()
}

fn knows(shard: &Standalone, character: u64, spell_id: u32) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT id FROM game_player_spell WHERE character_guid = {character} AND spell_id = {spell_id}"
        ))
        .is_empty()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_ally_buff_reward_lands_on_the_character_from_the_quest_giver() {
    let mut stage = stage("quest-reward-spell-ally-buff", ALLY_BUFF, 0);
    buff(&stage.shard, ALLY_BUFF, T_TARGET_ALLY);

    turn_in(&mut stage);

    assert_eq!(
        aura_casters(&stage.shard, stage.character, ALLY_BUFF),
        [stage.giver.to_string()]
    );
    assert!(aura_casters(&stage.shard, stage.giver, ALLY_BUFF).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_self_buff_reward_is_cast_by_the_character_on_itself() {
    let mut stage = stage("quest-reward-spell-self-buff", 0, SELF_BUFF);
    buff(&stage.shard, SELF_BUFF, T_SELF);

    turn_in(&mut stage);

    assert_eq!(
        aura_casters(&stage.shard, stage.character, SELF_BUFF),
        [stage.character.to_string()]
    );
    assert!(aura_casters(&stage.shard, stage.giver, SELF_BUFF).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_reward_spell_with_two_learn_effects_teaches_both_spells() {
    let mut stage = stage("quest-reward-spell-learn", 0, TEACHER);
    buff(&stage.shard, FIRST_TAUGHT, T_SELF);
    buff(&stage.shard, SECOND_TAUGHT, T_SELF);
    spell(
        &stage.shard,
        TEACHER,
        &[
            (E_LEARN_SPELL, T_TARGET_ALLY, FIRST_TAUGHT),
            (E_LEARN_SPELL, T_TARGET_ALLY, SECOND_TAUGHT),
        ],
    );

    turn_in(&mut stage);

    assert!(knows(&stage.shard, stage.character, FIRST_TAUGHT));
    assert!(knows(&stage.shard, stage.character, SECOND_TAUGHT));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_offer_shows_the_reward_spell_and_the_turn_in_casts_the_reward_cast_spell() {
    let mut stage = stage("quest-reward-spell-display", ALLY_BUFF, SELF_BUFF);
    buff(&stage.shard, ALLY_BUFF, T_TARGET_ALLY);
    buff(&stage.shard, SELF_BUFF, T_SELF);

    let mut body = stage.giver.to_le_bytes().to_vec();
    body.extend(QUEST.to_le_bytes());
    let offer = request(&mut stage.connection, CMSG_QUESTGIVER_REQUEST_REWARD, body);
    assert_eq!(offer.len(), 1);
    assert_eq!(offer[0].0, SMSG_QUESTGIVER_OFFER_REWARD);
    let tail = &offer[0].1[offer[0].1.len() - 4..];
    assert_eq!(
        tail,
        ALLY_BUFF.to_le_bytes(),
        "the offer ends with RewSpell"
    );

    turn_in(&mut stage);

    assert_eq!(
        aura_casters(&stage.shard, stage.character, SELF_BUFF).len(),
        1
    );
    assert!(aura_casters(&stage.shard, stage.character, ALLY_BUFF).is_empty());
}
