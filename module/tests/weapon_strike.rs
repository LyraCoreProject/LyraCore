mod support;

use lyracore_shared::spell::taxonomy::{E_WEAPON_STRIKE, T_TARGET_ENEMY};

use std::collections::BTreeMap;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use support::{event_rows, poll_until, Standalone, TableSubscription, POLL_TIMEOUT};

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;
const WOLF_HEALTH: u32 = 100_000;
const RANGED_SLOT: u8 = 17;
const THROW: u32 = 2764;
const SINISTER_STRIKE: u32 = 1752;
// Spell.dbc Attributes for Throw and Sinister Strike; 0x02 marks the ranged slot.
const THROW_ATTRIBUTES: u32 = 0x0041_0012;
const SINISTER_STRIKE_ATTRIBUTES: u32 = 0x0005_0010;
const THROWN: u8 = 16;
const INVTYPE_THROWN: u8 = 25;
const WEAPON_DAMAGE: u32 = 40;
const THROWN_STACK: u32 = 20;
const ATTEMPTS: u32 = 15;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_throw_strikes_with_the_thrown_weapon_and_spends_one_from_its_stack() {
    let mut standalone = Standalone::start("weapon-strike-throw");
    let wolf = stage(&mut standalone);
    stage_strike(&standalone, THROW, "Throw", THROW_ATTRIBUTES, 30, 0);
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET subclass = {THROWN}, inventory_type = {INVTYPE_THROWN}, \
         max_stack = 200, damage_min = {WEAPON_DAMAGE}.0, damage_max = {WEAPON_DAMAGE}.0 \
         WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_call(
        "debug_grant_item",
        &["1", &FIXTURE_BLADE.to_string(), &THROWN_STACK.to_string()],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = {RANGED_SLOT} \
         WHERE owner_guid = {PLAYER} AND entry = {FIXTURE_BLADE}"
    ));

    let (throws, damage) = strike_until_hit(&standalone, THROW, wolf);

    assert!(
        damage == WEAPON_DAMAGE || damage == 2 * WEAPON_DAMAGE,
        "a Throw deals the thrown weapon's damage, or double on a crit; got {damage}"
    );
    let stack = standalone.query_rows(&format!(
        "SELECT stack_count FROM game_item_instance \
         WHERE owner_guid = {PLAYER} AND slot = {RANGED_SLOT}"
    ));
    assert_eq!(
        stack[0]["stack_count"],
        (THROWN_STACK - throws).to_string(),
        "each Throw spends one thrown weapon"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_melee_weapon_spell_adds_its_bonus_to_the_weapon_roll() {
    const BONUS: i32 = 3;
    let mut standalone = Standalone::start("weapon-strike-melee");
    let wolf = stage(&mut standalone);
    stage_strike(
        &standalone,
        SINISTER_STRIKE,
        "Sinister Strike",
        SINISTER_STRIKE_ATTRIBUTES,
        5,
        BONUS,
    );
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET damage_min = {WEAPON_DAMAGE}.0, \
         damage_max = {WEAPON_DAMAGE}.0 WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);

    let (_, damage) = strike_until_hit(&standalone, SINISTER_STRIKE, wolf);

    assert!(
        damage >= WEAPON_DAMAGE + BONUS as u32,
        "the strike must deal the weapon roll plus its bonus, got {damage}"
    );
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    wolf
}

fn stage_strike(
    standalone: &Standalone,
    spell_id: u32,
    name: &str,
    attributes: u32,
    range_yd: u32,
    bonus: i32,
) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("range_yd", &range_yd),
            ("school_mask", &1),
            ("attributes", &attributes),
            ("is_negative", &true),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &E_WEAPON_STRIKE),
            ("base_points", &bonus),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &100),
            ("p_0_kind", &255),
        ],
    );
}

// Casts until one strike lands; returns the casts made and the health the hit took.
fn strike_until_hit(standalone: &Standalone, spell_id: u32, wolf: u64) -> (u32, u32) {
    for attempt in 1..=ATTEMPTS {
        let events = standalone
            .subscribe(&["SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1"]);
        standalone.assert_call(
            "debug_cast_at",
            &["1", &spell_id.to_string(), &wolf.to_string()],
        );
        let go = poll_until(POLL_TIMEOUT, || go_of(&events, spell_id).is_some());
        assert!(go, "the cast never reported its GO");
        if go_of(&events, spell_id).unwrap()["miss_info"] != "0" {
            continue;
        }
        let mut damage = 0;
        assert!(
            poll_until(POLL_TIMEOUT, || {
                damage = WOLF_HEALTH - health(standalone, wolf);
                damage > 0
            }),
            "a landed strike must damage the wolf"
        );
        return (attempt, damage);
    }
    panic!("no strike landed in {ATTEMPTS} attempts");
}

fn go_of(events: &TableSubscription, spell_id: u32) -> Option<SqlRow> {
    event_rows(events, "game_spell_cast_event_live")
        .into_iter()
        .find(|row| row["spell_id"] == spell_id.to_string() && row["kind"] == "2")
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .expect("health must be a number")
}
