mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::{type_mask, unit_flags};
use lyracore_shared::guid;
use lyracore_shared::spell::taxonomy::{
    A_MOD_STAT, CAST_FLAG_AREA_AURA, CAST_FLAG_PET_AREA_AURA, P_STAT_ID, SPELL_ATTR_PASSIVE,
    STAT_STR, TOTEM_TYPE, T_SELF,
};
use support::{actor, number, Standalone};

const CHARACTER: u64 = 1;
const GROUP: u64 = 13_740;
const CHARACTER_FACTION: u32 = 13_741;
const RADIUS_YD: f32 = 30.0;
// Devotion Aura 465 shape: effect 35 on the caster and permanent; two ranks form a chain.
const PARTY_AURA: u32 = 5_137_401;
const PARTY_AURA_POINTS: i32 = 25;
const LOW_RANK: u32 = 5_137_402;
const LOW_RANK_POINTS: i32 = 10;
const HIGH_RANK: u32 = 5_137_403;
const HIGH_RANK_POINTS: i32 = 40;
const HIGH_RANK_LEVEL: u8 = 30;
const PET_AURA: u32 = 5_137_404;
// Strength of Earth 8076 and 8162 shape: two ranks of one name with no chain.
const WEAK_TOTEM_AURA: u32 = 5_137_405;
const WEAK_TOTEM_POINTS: i32 = 10;
const STRONG_TOTEM_AURA: u32 = 5_137_406;
const STRONG_TOTEM_POINTS: i32 = 20;
// Tauren Nature Resistance 20551 shape: a passive of the party Aura's name and no chain.
const RACIAL: u32 = 5_137_407;
const RACIAL_POINTS: i32 = 7;
const TIMED_AURA: u32 = 5_137_408;
const PERMANENT_MS: u32 = u32::MAX;
const TIMED_MS: u32 = 60_000;
const TOTEM: u64 = guid::compose(guid::UNIT, 13_742);
const TRANSFER: &str = "513740";
// One Aura pass: game_aura_schedule runs every second, 20 ticks of the 50 ms virtual clock.
const AURA_PASS_TICKS: &str = "20";

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_party_member_holds_the_aura_in_range_and_loses_it_out_of_range() {
    let shard = stage("party-aura-range");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    cast(&shard, CHARACTER, PARTY_AURA);
    pass(&shard);
    assert_eq!(
        auras(&shard, member),
        vec![(PARTY_AURA, CHARACTER, PARTY_AURA_POINTS)],
        "vmangos AreaAura::Update gives a Subgroup member in range the caster's Aura"
    );

    let (_, x, _, _) = caster_position(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {} WHERE guid = {member}",
        x + 2.0 * RADIUS_YD
    ));
    pass(&shard);
    assert!(
        auras(&shard, member).is_empty(),
        "a member past the radius loses the copy"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_who_leaves_the_group_loses_the_aura() {
    let shard = stage("party-aura-leave");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    cast(&shard, CHARACTER, PARTY_AURA);
    pass(&shard);
    assert_eq!(auras(&shard, member).len(), 1);

    shard.assert_sql(&format!(
        "DELETE FROM game_group_member WHERE character_guid = {member}"
    ));
    pass(&shard);
    assert!(auras(&shard, member).is_empty());
    assert_eq!(
        auras(&shard, CHARACTER),
        vec![(PARTY_AURA, CHARACTER, PARTY_AURA_POINTS)],
        "the caster keeps its own Aura"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn removing_the_casters_aura_removes_every_copy_at_once() {
    let shard = stage("party-aura-cancel");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    cast(&shard, CHARACTER, PARTY_AURA);
    pass(&shard);
    assert_eq!(auras(&shard, member).len(), 1);

    shard.assert_call(
        "debug_shed_spell_auras",
        &[&CHARACTER.to_string(), &PARTY_AURA.to_string()],
    );
    assert!(
        auras(&shard, member).is_empty(),
        "the copy goes with the caster's Aura, before the next pass"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_totem_buffs_its_owners_party_and_its_own_aura_does_nothing() {
    let shard = stage("party-aura-totem");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET creature_type = {TOTEM_TYPE} WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    let totem = creature(&shard, TOTEM, CHARACTER, 3.0);
    cast(&shard, totem, PARTY_AURA);
    pass(&shard);
    assert_eq!(
        auras(&shard, totem),
        vec![(PARTY_AURA, totem, 0)],
        "vmangos AreaAura gives a totem's own party Aura SPELL_AURA_NONE"
    );
    for unit in [CHARACTER, member] {
        assert_eq!(
            auras(&shard, unit),
            vec![(PARTY_AURA, totem, PARTY_AURA_POINTS)],
            "the totem's owner and the owner's Subgroup hold the totem's Aura"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_lower_level_member_gets_the_rank_fit_for_their_level() {
    let shard = stage("party-aura-rank");
    let member = character(&shard, "Member", 5.0);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 10 WHERE guid = {member}"
    ));
    group(&shard, &[CHARACTER, member]);
    cast(&shard, CHARACTER, HIGH_RANK);
    pass(&shard);
    assert_eq!(
        auras(&shard, member),
        vec![(LOW_RANK, CHARACTER, LOW_RANK_POINTS)],
        "vmangos SelectAuraRankForLevel: rank 2 needs level {}, so a level 10 member gets rank 1",
        HIGH_RANK_LEVEL - 10
    );
    assert_eq!(
        auras(&shard, CHARACTER),
        vec![(HIGH_RANK, CHARACTER, HIGH_RANK_POINTS)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pets_party_aura_reaches_its_owner_without_a_group() {
    let shard = stage("party-aura-pet-caster");
    let pet = creature(&shard, pet_guid(CHARACTER), CHARACTER, 3.0);
    cast(&shard, pet, PARTY_AURA);
    pass(&shard);
    assert_eq!(
        auras(&shard, CHARACTER),
        vec![(PARTY_AURA, pet, PARTY_AURA_POINTS)],
        "Blood Pact 6307 shape: with no Group the imp's Aura reaches its owner"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pet_area_aura_reaches_only_its_owner() {
    let shard = stage("pet-aura-owner");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    let pet = creature(&shard, pet_guid(CHARACTER), CHARACTER, 3.0);
    cast(&shard, pet, PET_AURA);
    pass(&shard);
    assert_eq!(
        auras(&shard, CHARACTER),
        vec![(PET_AURA, pet, PARTY_AURA_POINTS)]
    );
    assert!(
        auras(&shard, member).is_empty(),
        "vmangos AREA_AURA_PET reaches the owner and no Subgroup member"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn two_casters_of_one_unchained_aura_leave_the_stronger_copy_and_it_stays() {
    let shard = stage("party-aura-two-casters");
    let shaman = character(&shard, "Shaman", 5.0);
    let mate = character(&shard, "Mate", 10.0);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60 WHERE guid = {shaman}"
    ));
    group(&shard, &[CHARACTER, shaman, mate]);
    cast(&shard, CHARACTER, WEAK_TOTEM_AURA);
    cast(&shard, shaman, STRONG_TOTEM_AURA);
    for _ in 0..3 {
        pass(&shard);
        assert_eq!(
            auras(&shard, mate),
            vec![(STRONG_TOTEM_AURA, shaman, STRONG_TOTEM_POINTS)],
            "the higher-level rank of one name stays on the member on every pass"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_copy_keeps_a_passive_of_the_same_name_from_another_chain() {
    let shard = stage("party-aura-racial");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    cast(&shard, member, RACIAL);
    cast(&shard, CHARACTER, PARTY_AURA);
    pass(&shard);
    assert_eq!(
        auras(&shard, member),
        vec![
            (PARTY_AURA, CHARACTER, PARTY_AURA_POINTS),
            (RACIAL, member, RACIAL_POINTS)
        ],
        "vmangos RemoveNoStackAurasDueToAuraHolder keeps a passive of another spell chain"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_flagged_member_gets_no_copy_from_an_unflagged_caster() {
    let shard = stage("party-aura-pvp");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    set_flagged(&shard, member);
    cast(&shard, CHARACTER, PARTY_AURA);
    pass(&shard);
    assert!(
        auras(&shard, member).is_empty(),
        "vmangos AreaAura::Update: a PvP-flagged member needs a flagged caster"
    );

    set_flagged(&shard, CHARACTER);
    pass(&shard);
    assert_eq!(
        auras(&shard, member),
        vec![(PARTY_AURA, CHARACTER, PARTY_AURA_POINTS)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_dead_members_pet_gets_no_copy() {
    let shard = stage("party-aura-dead-owner");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    let pet = creature(&shard, pet_guid(member), member, 6.0);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET dead = true, health = 0 WHERE guid = {member}"
    ));
    cast(&shard, CHARACTER, PARTY_AURA);
    pass(&shard);
    assert!(
        auras(&shard, pet).is_empty(),
        "vmangos adds a member's pet only when the member passes the checks"
    );

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET dead = false, health = 100 WHERE guid = {member}"
    ));
    pass(&shard);
    assert_eq!(
        auras(&shard, pet),
        vec![(PARTY_AURA, CHARACTER, PARTY_AURA_POINTS)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_recast_moves_every_copys_expiry_with_the_casters_aura() {
    let shard = stage("party-aura-refresh");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    cast(&shard, CHARACTER, TIMED_AURA);
    pass(&shard);
    let first = expiry(&shard, CHARACTER, TIMED_AURA);
    assert_eq!(expiry(&shard, member, TIMED_AURA), first);

    pass(&shard);
    cast(&shard, CHARACTER, TIMED_AURA);
    pass(&shard);
    let refreshed = expiry(&shard, CHARACTER, TIMED_AURA);
    assert_ne!(refreshed, first);
    assert_eq!(expiry(&shard, member, TIMED_AURA), refreshed);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_copy_whose_source_is_not_on_this_shard_goes() {
    let shard = stage("party-aura-orphan");
    let member = character(&shard, "Member", 5.0);
    group(&shard, &[CHARACTER, member]);
    cast(&shard, CHARACTER, PARTY_AURA);
    pass(&shard);
    assert_eq!(auras(&shard, member).len(), 1);

    shard.assert_sql("DELETE FROM game_area_aura_source");
    pass(&shard);
    assert!(auras(&shard, member).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_transfer_moves_own_auras_and_leaves_copies_behind() {
    let source = stage("party-aura-transfer-source");
    let mut destination = Standalone::start("party-aura-transfer-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
    let paladin = character(&source, "Paladin", 5.0);
    group(&source, &[CHARACTER, paladin]);
    cast(&source, paladin, PARTY_AURA);
    cast(&source, CHARACTER, PET_AURA);
    pass(&source);
    assert_eq!(
        auras(&source, CHARACTER),
        vec![
            (PARTY_AURA, paladin, PARTY_AURA_POINTS),
            (PET_AURA, CHARACTER, PARTY_AURA_POINTS)
        ]
    );

    source.assert_call(
        "begin_transfer",
        &[
            TRANSFER,
            &actor(CHARACTER),
            "0",
            "0",
            "1200",
            "1200",
            "50",
            "0",
            "true",
        ],
    );
    let out = source.query_rows(&format!(
        "SELECT blob FROM game_transfer_out WHERE transfer_id = {TRANSFER}"
    ));
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    destination.assert_call("import_character_blob", &[TRANSFER, &blob, &actor("0")]);
    assert_eq!(
        auras(&destination, CHARACTER),
        vec![(PET_AURA, CHARACTER, PARTY_AURA_POINTS)],
        "a copy stays with its source's Shard; vmangos drops a copy whose caster is gone"
    );
}

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.clear_schedule("game_creature_move_schedule");
    shard.clear_schedule("game_melee_schedule");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60 WHERE guid = {CHARACTER}"
    ));
    shard.insert_row(
        "game_faction_template",
        &[
            ("id", &CHARACTER_FACTION),
            ("faction", &CHARACTER_FACTION),
            ("faction_group", &1),
            ("friend_group", &1),
            ("enemy_group", &2),
        ],
    );
    set_faction(&shard, CHARACTER, CHARACTER_FACTION);
    for fixture in &FIXTURES {
        insert_area_aura(&shard, fixture);
    }
    for (spell_id, prev_spell, rank) in [(LOW_RANK, 0, 1), (HIGH_RANK, LOW_RANK, 2)] {
        shard.insert_row(
            "game_spell_chain",
            &[
                ("spell_id", &spell_id),
                ("prev_spell", &prev_spell),
                ("first_spell", &LOW_RANK),
                ("rank", &rank),
                ("req_spell", &0),
            ],
        );
    }
    shard
}

struct Fixture {
    spell_id: u32,
    // Ranks of one spell share its name, as Spell.dbc gives them.
    name: &'static str,
    spell_level: u8,
    points: i32,
    cast_flags: u32,
    attributes: u32,
    duration_ms: u32,
}

const FIXTURES: [Fixture; 8] = [
    area_fixture(PARTY_AURA, "Area aura fixture", 1, PARTY_AURA_POINTS),
    area_fixture(LOW_RANK, "Area aura fixture ranks", 1, LOW_RANK_POINTS),
    area_fixture(
        HIGH_RANK,
        "Area aura fixture ranks",
        HIGH_RANK_LEVEL,
        HIGH_RANK_POINTS,
    ),
    Fixture {
        cast_flags: CAST_FLAG_AREA_AURA | CAST_FLAG_PET_AREA_AURA,
        ..area_fixture(PET_AURA, "Area aura fixture pet", 1, PARTY_AURA_POINTS)
    },
    area_fixture(
        WEAK_TOTEM_AURA,
        "Area aura fixture totem",
        10,
        WEAK_TOTEM_POINTS,
    ),
    area_fixture(
        STRONG_TOTEM_AURA,
        "Area aura fixture totem",
        24,
        STRONG_TOTEM_POINTS,
    ),
    Fixture {
        cast_flags: 0,
        attributes: SPELL_ATTR_PASSIVE,
        ..area_fixture(RACIAL, "Area aura fixture", 1, RACIAL_POINTS)
    },
    Fixture {
        duration_ms: TIMED_MS,
        ..area_fixture(TIMED_AURA, "Area aura fixture timed", 1, PARTY_AURA_POINTS)
    },
];

/// A permanent party Strength Aura on its caster that reaches units within `RADIUS_YD`.
const fn area_fixture(spell_id: u32, name: &'static str, spell_level: u8, points: i32) -> Fixture {
    Fixture {
        spell_id,
        name,
        spell_level,
        points,
        cast_flags: CAST_FLAG_AREA_AURA,
        attributes: 0,
        duration_ms: PERMANENT_MS,
    }
}

fn insert_area_aura(shard: &Standalone, fixture: &Fixture) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &fixture.spell_id),
            ("name", &fixture.name),
            ("range_yd", &0),
            ("is_negative", &false),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
            ("duration_ms", &fixture.duration_ms),
            ("spell_level", &fixture.spell_level),
            ("cast_flags", &fixture.cast_flags),
            ("attributes", &fixture.attributes),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(fixture.spell_id, 0)),
            ("spell_id", &fixture.spell_id),
            ("kind", &A_MOD_STAT),
            ("target", &T_SELF),
            ("base_points", &fixture.points),
            ("radius_yd", &RADIUS_YD),
            ("p0", &i32::from(STAT_STR)),
            ("p0_kind", &P_STAT_ID),
        ],
    );
}

/// A Character of the fixture Account `offset` yards along +x from the caster.
fn character(shard: &Standalone, name: &str, offset: f32) -> u64 {
    let quoted = format!("\"{name}\"");
    shard.assert_call(
        "create_character",
        &["1", &quoted, "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let guid: u64 = number(
        &shard.query_rows(&format!(
            "SELECT guid FROM game_character WHERE name = '{name}'"
        ))[0],
        "guid",
    );
    let (map_id, x, y, z) = caster_position(shard);
    shard.assert_sql(&format!(
        "UPDATE game_character SET map_id = {map_id}, x = {}, y = {y}, z = {z} WHERE guid = {guid}",
        x + offset
    ));
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    set_faction(shard, guid, CHARACTER_FACTION);
    guid
}

/// A player's pet carries the owner's low guid, as `pet_guid_for` names it.
fn pet_guid(owner: u64) -> u64 {
    guid::compose(guid::UNIT, guid::low(owner))
}

/// A creature of `owner` `offset` yards along +x from the caster.
fn creature(shard: &Standalone, creature_guid: u64, owner: u64, offset: f32) -> u64 {
    let (map_id, x, y, z) = caster_position(shard);
    let no_identity = format!("0x{}", "00".repeat(32));
    shard.insert_row(
        "game_world_entity",
        &[
            ("guid", &creature_guid),
            ("owner_identity", &no_identity),
            ("map_id", &map_id),
            ("x", &(x + offset)),
            ("y", &y),
            ("z", &z),
            ("type_mask", &type_mask::CREATURE),
            ("entry", &TEST_WOLF_ENTRY),
            ("scale_x", &1.0f32),
            ("level", &60),
            ("health", &100),
            ("max_health", &100),
            ("faction_template", &CHARACTER_FACTION),
            ("owner_guid", &owner),
        ],
    );
    creature_guid
}

fn group(shard: &Standalone, members: &[u64]) {
    shard.insert_row(
        "game_group",
        &[("group_id", &GROUP), ("leader_guid", &members[0])],
    );
    let no_identity = format!("0x{}", "00".repeat(32));
    for (id, character_guid) in (GROUP * 1_000..).zip(members) {
        shard.insert_row(
            "game_group_member",
            &[
                ("id", &id),
                ("group_id", &GROUP),
                ("character_guid", character_guid),
                ("owner_identity", &no_identity),
            ],
        );
    }
}

fn cast(shard: &Standalone, caster: u64, spell_id: u32) {
    shard.assert_call(
        "debug_force_cast_at",
        &[
            &caster.to_string(),
            &spell_id.to_string(),
            &caster.to_string(),
        ],
    );
}

fn pass(shard: &Standalone) {
    shard.assert_call("debug_advance_roll_batch", &[AURA_PASS_TICKS]);
}

/// The fixture Auras `target` holds, as (spell, caster, amount), in spell order.
fn auras(shard: &Standalone, target: u64) -> Vec<(u32, u64, i32)> {
    let mut held: Vec<(u32, u64, i32)> = shard
        .query_rows(&format!(
            "SELECT spell_id, caster_guid, amount FROM game_aura WHERE target_guid = {target}"
        ))
        .iter()
        .map(|row| {
            (
                number(row, "spell_id"),
                number(row, "caster_guid"),
                number(row, "amount"),
            )
        })
        .filter(|&(spell_id, _, _)| (PARTY_AURA..=TIMED_AURA).contains(&spell_id))
        .collect();
    held.sort_unstable();
    held
}

fn caster_position(shard: &Standalone) -> (u32, f32, f32, f32) {
    let row = shard
        .query_rows(&format!(
            "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    (
        number(&row, "map_id"),
        number(&row, "x"),
        number(&row, "y"),
        number(&row, "z"),
    )
}

fn set_faction(shard: &Standalone, guid: u64, faction: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}"
    ));
}

fn set_flagged(shard: &Standalone, guid: u64) {
    let row = shard
        .query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {guid}"
        ))
        .remove(0);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {} WHERE guid = {guid}",
        number::<u32>(&row, "unit_flags") | unit_flags::PVP
    ));
}

fn expiry(shard: &Standalone, target: u64, spell_id: u32) -> String {
    shard
        .query_rows(&format!(
            "SELECT expires_at FROM game_aura WHERE target_guid = {target} AND spell_id = {spell_id}"
        ))
        .remove(0)["expires_at"]
        .clone()
}
