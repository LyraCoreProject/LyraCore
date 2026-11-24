mod support;

use lyracore_shared::guid;
use support::{actor, number, poll_until, Row, Standalone, POLL_TIMEOUT};

const HUNTER: &str = "1";
const BOAR: &str = "51008";
const TAME: &str = "50300";
/// `UNIT_FLAG_PET_RENAME`: the 1.12 client offers "Rename Pet" while the live pet carries it.
const PET_RENAME_FLAG: u32 = 0x10;
const PET_NAME_INVALID: &str = "PET_NAME_INVALID";
/// A player's hunter pet keeps the owner's low guid under the unit high part.
const HUNTER_PET: u64 = guid::compose(guid::UNIT, 1);

fn wild_boar(shard: &Standalone, owner: &str) -> String {
    let before = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {BOAR} AND owner_guid = 0"
    ));
    shard.assert_call("debug_spawn_at_feet", &[owner, BOAR, "5"]);
    let rows = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {BOAR} AND owner_guid = 0"
    ));
    let added: Vec<_> = rows
        .into_iter()
        .filter(|row| !before.contains(row))
        .collect();
    assert_eq!(added.len(), 1);
    added[0]["guid"].clone()
}

// `PetPresence` codes in `game_hunter_pet.presence`.
const DISMISSED: &str = "0";
const OUT: &str = "1";
const DEAD: &str = "2";
/// CMSG_PET_ACTION data 0x07000003: a command action carrying COMMAND_DISMISS.
const DISMISS: &str = "117440515";
/// CMSG_PET_ACTION data 0x07000000: a command action carrying COMMAND_STAY.
const STAY: &str = "117440512";

fn presence(shard: &Standalone) -> String {
    shard
        .query_rows("SELECT presence FROM game_hunter_pet WHERE owner_guid = 1")
        .remove(0)["presence"]
        .clone()
}

fn live_pet(shard: &Standalone) -> Option<Row> {
    shard
        .query_rows(&format!(
            "SELECT owner_guid, entry, level, health, max_health, display_id FROM game_world_entity WHERE guid = {HUNTER_PET}"
        ))
        .pop()
}

/// Rewrites the fixture Tame Beast into an instant self-cast of `kind` with `points` as its amount.
fn rewrite_tame(shard: &Standalone, kind: u8, points: i32) {
    use lyracore_shared::spell::taxonomy::T_SELF;
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_flags = 0, duration_ms = 0 WHERE spell_id = {TAME}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET kind = {kind}, target = {T_SELF}, p0 = 0, base_points = {points}, die_sides = 0 WHERE spell_id = {TAME}"
    ));
}

/// Call Pet is SPELL_EFFECT_SUMMON_PET with no creature entry.
fn make_tame_call_pet(shard: &Standalone) {
    rewrite_tame(shard, lyracore_shared::spell::taxonomy::E_SUMMON_PET, 0);
}

/// Revive Pet (982) returns the pet with 15 percent of its maximum health.
const REVIVE_PERCENT: u32 = 15;
const REVIVE_PET_SPELL: u64 = 982;

fn make_tame_revive_pet(shard: &Standalone) {
    rewrite_tame(
        shard,
        lyracore_shared::spell::taxonomy::E_REVIVE_PET,
        REVIVE_PERCENT as i32,
    );
}

fn cast_at_self(shard: &Standalone) {
    assert!(
        poll_until(POLL_TIMEOUT, || shard
            .call("debug_force_cast_at", &[HUNTER, TAME, HUNTER])
            .status
            .success()),
        "the cast must resolve after the global cooldown"
    );
}

fn assert_cast_refused(shard: &Standalone, reason: &str) {
    assert!(
        poll_until(POLL_TIMEOUT, || {
            let output = shard.call("debug_force_cast_at", &[HUNTER, TAME, HUNTER]);
            !output.status.success() && String::from_utf8_lossy(&output.stderr).contains(reason)
        }),
        "the cast must be refused because {reason}"
    );
}

fn saved_vitals(shard: &Standalone) -> (u32, u32) {
    let row = shard
        .query_rows("SELECT saved_health, saved_power FROM game_hunter_pet WHERE owner_guid = 1")
        .remove(0);
    (number(&row, "saved_health"), number(&row, "saved_power"))
}

/// Regeneration may add a little health between the build and the read, never a full heal.
fn assert_wounded(live: &Row, at_least: u32) {
    let health: u32 = number(live, "health");
    assert!(
        (at_least..number::<u32>(live, "max_health")).contains(&health),
        "health {health} is below {at_least} or a full heal in {live:?}"
    );
}

fn bind_session(shard: &Standalone) {
    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    shard.assert_call(
        "establish_session",
        &[HUNTER, &key, r#"{"__identity__":"0x1"}"#],
    );
}

fn relog(shard: &Standalone) {
    shard.assert_call("debug_logout_character", &[HUNTER]);
    assert!(live_pet(shard).is_none(), "logout removes the live pet");
    shard.assert_call("gw_player_login", &[HUNTER, &actor(HUNTER)]);
}

fn hunter(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("UPDATE game_character SET class = 3, level = 10 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &[HUNTER]);
    shard.assert_call("debug_set_level", &[HUNTER, "60"]);
    shard
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn tame_and_dismiss_keep_one_durable_identity_and_a_second_tame_is_refused() {
    let shard = hunter("hunter-pet-lifetime");
    let wild = wild_boar(&shard, HUNTER);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    let pet = HUNTER_PET.to_string();
    assert!(shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {wild}"
        ))
        .is_empty());
    let live = shard.query_rows(&format!(
        "SELECT owner_guid, entry FROM game_world_entity WHERE guid = {pet}"
    ));
    assert_eq!(live.len(), 1);
    assert_eq!(live[0]["owner_guid"], HUNTER);
    assert_eq!(live[0]["entry"], BOAR);
    let identities =
        shard.query_rows("SELECT pet_id, creature_entry FROM game_hunter_pet WHERE owner_guid = 1");
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0]["pet_id"], HUNTER);
    let spawn = shard.query_rows(&format!(
        "SELECT respawn_at, despawn_at FROM game_creature_spawn WHERE guid = {wild}"
    ));
    assert_eq!(spawn.len(), 1, "taming preserves the authored spawn");
    let respawn = spacetimedb::Timestamp::parse_from_rfc3339(&spawn[0]["respawn_at"])
        .expect("respawn timestamp")
        .to_micros_since_unix_epoch();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64;
    assert!(
        (now..now + 600_000_000).contains(&respawn),
        "taming rearms a finite future respawn"
    );
    let despawn = spacetimedb::Timestamp::parse_from_rfc3339(&spawn[0]["despawn_at"])
        .expect("despawn timestamp")
        .to_micros_since_unix_epoch();
    assert!(
        despawn > now + 100 * 365 * 86_400 * 1_000_000,
        "taming disarms the despawn timer"
    );
    let protocol =
        shard.query_rows("SELECT live_pet_guid FROM game_hunter_pet_protocol WHERE owner_guid = 1");
    assert_eq!(number::<u64>(&protocol[0], "live_pet_guid"), HUNTER_PET);

    shard.assert_call("debug_pet_command", &[HUNTER, DISMISS, "0"]);
    for table in [
        "game_world_entity",
        "game_entity_motion",
        "game_live_pet_kind",
    ] {
        let key = if table == "game_live_pet_kind" {
            "pet_guid"
        } else {
            "guid"
        };
        assert!(
            shard
                .query_rows(&format!("SELECT * FROM {table} WHERE {key} = {pet}"))
                .is_empty(),
            "{table} retains the dismissed pet"
        );
    }
    assert!(shard
        .query_rows("SELECT * FROM game_pet_command WHERE owner_guid = 1")
        .is_empty());
    assert_eq!(
        shard.query_rows("SELECT pet_id, creature_entry FROM game_hunter_pet WHERE owner_guid = 1"),
        identities
    );

    assert_eq!(presence(&shard), DISMISSED);

    let replacement = wild_boar(&shard, HUNTER);
    assert!(
        poll_until(POLL_TIMEOUT, || {
            let output = shard.call("debug_force_cast_at", &[HUNTER, TAME, &replacement]);
            !output.status.success()
                && String::from_utf8_lossy(&output.stderr).contains("already has a pet")
        }),
        "a Hunter with a durable pet cannot tame another"
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT owner_guid FROM game_world_entity WHERE guid = {replacement}"
        ))[0]["owner_guid"],
        "0"
    );
    assert!(live_pet(&shard).is_none());
    assert_eq!(
        shard.query_rows("SELECT pet_id, creature_entry FROM game_hunter_pet WHERE owner_guid = 1"),
        identities
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn call_pet_builds_the_dismissed_pet_from_its_durable_identity() {
    let shard = hunter("hunter-pet-call");
    let wild = wild_boar(&shard, HUNTER);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    let tamed_display = live_pet(&shard).expect("tamed pet")["display_id"].clone();
    assert!(rename(&shard, HUNTER, HUNTER_PET, "Fang").status.success());
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 7 WHERE guid = {HUNTER_PET}"
    ));
    shard.assert_call("debug_pet_command", &[HUNTER, DISMISS, "0"]);
    let saved_health = saved_vitals(&shard).0;
    assert!(
        saved_health >= 7,
        "a dismiss saves the wounded pet's health"
    );
    shard.assert_sql(
        "UPDATE game_hunter_pet SET level = 12, loyalty_level = 3, happiness = 700000 WHERE owner_guid = 1",
    );

    make_tame_call_pet(&shard);
    cast_at_self(&shard);
    let live = live_pet(&shard).expect("Call Pet builds the live pet");
    assert_eq!(live["owner_guid"], HUNTER);
    assert_eq!(live["entry"], BOAR);
    assert_eq!(live["level"], "12");
    assert_wounded(&live, saved_health);
    assert_eq!(
        saved_vitals(&shard),
        (0, 0),
        "the built pet holds its vitals"
    );
    assert_eq!(live["display_id"], tamed_display);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT hunter_pet_id FROM game_live_pet_kind WHERE pet_guid = {HUNTER_PET}"
        ))[0]["hunter_pet_id"],
        HUNTER
    );
    let protocol = shard
        .query_rows(
            "SELECT live_pet_guid, name, level, loyalty_level, happiness FROM game_hunter_pet_protocol WHERE owner_guid = 1",
        )
        .remove(0);
    assert_eq!(number::<u64>(&protocol, "live_pet_guid"), HUNTER_PET);
    assert_eq!(protocol["name"], "Fang");
    assert_eq!(protocol["level"], "12");
    assert_eq!(protocol["loyalty_level"], "3");
    assert_eq!(protocol["happiness"], "700000");
    assert_eq!(live_pet_flags(&shard) & PET_RENAME_FLAG, 0);
    assert_eq!(presence(&shard), OUT);

    cast_at_self(&shard);
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE owner_guid = {HUNTER}"
            ))
            .len(),
        1,
        "Call Pet with the pet out adds nothing"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_out_pet_returns_at_world_entry_and_a_dismissed_pet_stays_away() {
    let shard = hunter("hunter-pet-world-entry");
    let wild = wild_boar(&shard, HUNTER);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    let tamed = live_pet(&shard).expect("tamed pet");
    bind_session(&shard);

    relog(&shard);
    assert_eq!(live_pet(&shard), Some(tamed));
    assert_eq!(presence(&shard), OUT);
    shard.assert_call("gw_player_world_port", &[HUNTER, &actor(HUNTER)]);
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE owner_guid = {HUNTER}"
            ))
            .len(),
        1,
        "a world port keeps the one live pet"
    );

    shard.assert_call("debug_pet_command", &[HUNTER, DISMISS, "0"]);
    assert_eq!(presence(&shard), DISMISSED);
    relog(&shard);
    assert!(live_pet(&shard).is_none());
    assert_eq!(presence(&shard), DISMISSED);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_dead_pet_answers_neither_call_pet_nor_world_entry() {
    let shard = hunter("hunter-pet-dead");
    let wild = wild_boar(&shard, HUNTER);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    shard.assert_call("debug_kill_creature", &[HUNTER, &HUNTER_PET.to_string()]);
    assert!(live_pet(&shard).is_none());
    assert_eq!(presence(&shard), DEAD);
    bind_session(&shard);
    relog(&shard);
    assert!(live_pet(&shard).is_none());

    make_tame_call_pet(&shard);
    cast_at_self(&shard);
    assert!(
        live_pet(&shard).is_none(),
        "a dead pet waits for Revive Pet"
    );
    assert_eq!(presence(&shard), DEAD);

    shard.assert_sql("DELETE FROM game_hunter_pet WHERE owner_guid = 1");
    cast_at_self(&shard);
    assert!(
        live_pet(&shard).is_none(),
        "Call Pet without a pet brings none"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn revive_pet_returns_only_a_dead_pet_and_it_stays_out_across_login() {
    let shard = hunter("hunter-pet-revive");
    let wild = wild_boar(&shard, HUNTER);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    make_tame_revive_pet(&shard);
    assert_cast_refused(&shard, "already has a pet out");
    assert_eq!(presence(&shard), OUT);

    shard.assert_call("debug_kill_creature", &[HUNTER, &HUNTER_PET.to_string()]);
    assert_eq!(presence(&shard), DEAD);
    cast_at_self(&shard);
    let live = live_pet(&shard).expect("Revive Pet builds the dead pet");
    assert_eq!(live["owner_guid"], HUNTER);
    assert_eq!(live["entry"], BOAR);
    let max_health: u32 = number(&live, "max_health");
    assert_wounded(&live, (max_health * REVIVE_PERCENT / 100).max(1));
    assert_eq!(presence(&shard), OUT);

    bind_session(&shard);
    shard.assert_call("debug_logout_character", &[HUNTER]);
    let saved_health = saved_vitals(&shard).0;
    shard.assert_call("gw_player_login", &[HUNTER, &actor(HUNTER)]);
    let relogged = live_pet(&shard).expect("a revived pet is out at login");
    assert_wounded(&relogged, saved_health);

    shard.assert_call("debug_pet_command", &[HUNTER, DISMISS, "0"]);
    assert_cast_refused(&shard, "pet is not dead");
    assert!(live_pet(&shard).is_none());
    shard.assert_sql("DELETE FROM game_hunter_pet WHERE owner_guid = 1");
    assert_cast_refused(&shard, "has no pet");
    assert!(live_pet(&shard).is_none());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_tame_replaces_a_dead_pet_only_while_revive_pet_is_not_imported() {
    use lyracore_shared::spell::taxonomy::E_REVIVE_PET;

    let shard = hunter("hunter-pet-dead-retame");
    let wild = wild_boar(&shard, HUNTER);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    assert!(rename(&shard, HUNTER, HUNTER_PET, "Fang").status.success());
    shard.assert_call("debug_kill_creature", &[HUNTER, &HUNTER_PET.to_string()]);
    assert_eq!(presence(&shard), DEAD);

    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &(REVIVE_PET_SPELL << 2)),
            ("spell_id", &REVIVE_PET_SPELL),
            ("kind", &E_REVIVE_PET),
        ],
    );
    let replacement = wild_boar(&shard, HUNTER);
    assert!(
        poll_until(POLL_TIMEOUT, || {
            let output = shard.call("debug_force_cast_at", &[HUNTER, TAME, &replacement]);
            !output.status.success()
                && String::from_utf8_lossy(&output.stderr).contains("already has a pet")
        }),
        "with Revive Pet imported, a dead pet still blocks a tame"
    );
    assert_eq!(identity(&shard)["name"], "Fang");
    assert_eq!(presence(&shard), DEAD);

    shard.assert_sql(&format!(
        "DELETE FROM game_spell_effect WHERE spell_id = {REVIVE_PET_SPELL}"
    ));
    assert!(
        poll_until(POLL_TIMEOUT, || shard
            .call("debug_force_cast_at", &[HUNTER, TAME, &replacement])
            .status
            .success()),
        "without Revive Pet imported, a tame replaces the dead pet"
    );
    let replaced = identity(&shard);
    assert_ne!(replaced["name"], "Fang");
    assert_eq!(replaced["rename_allowed"], "true");
    assert_eq!(presence(&shard), OUT);
    assert!(live_pet(&shard).is_some());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn abandon_removes_the_hunter_pet_for_good_and_a_new_tame_succeeds() {
    use std::io::Cursor;
    use wow_world_messages::vanilla::{ClientMessage, CMSG_PET_ABANDON};

    let mut shard = Standalone::start("hunter-pet-abandon");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"ABANDON\"", &credentials, &credentials],
    );
    let account =
        shard.query_rows("SELECT id FROM game_account WHERE username = 'ABANDON'")[0]["id"].clone();
    // Race 3 (Dwarf) with class 3 (Hunter); the remaining arguments are appearance.
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Abandoner\"",
            "3",
            "3",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let hunter: u64 = number(
        &shard.query_rows("SELECT guid FROM game_character WHERE name = 'Abandoner'")[0],
        "guid",
    );
    let owner = hunter.to_string();
    let pet = guid::compose(guid::UNIT, guid::low(hunter));
    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection
        .call("native_world_open", serde_json::json!([42]))
        .unwrap();
    connection
        .call("debug_native_enter_world", serde_json::json!([hunter]))
        .unwrap();
    shard.assert_call("debug_set_level", &[&owner, "60"]);
    let wild = wild_boar(&shard, &owner);
    shard.assert_call("debug_force_cast_at", &[&owner, TAME, &wild]);
    shard.assert_call("debug_pet_command", &[&owner, STAY, "0"]);
    assert!(!shard
        .query_rows(&format!(
            "SELECT * FROM game_pet_command WHERE owner_guid = {hunter}"
        ))
        .is_empty());

    let mut frame = Vec::new();
    CMSG_PET_ABANDON {
        pet: wow_world_messages::Guid::new(pet),
    }
    .write_unencrypted_client(&mut Cursor::new(&mut frame))
    .unwrap();
    // A client frame is a two-byte size and a four-byte opcode before the body.
    connection
        .call(
            "native_world_receive",
            serde_json::json!([0x0176u32, &frame[6..]]),
        )
        .unwrap();
    for (table, key, value) in [
        ("game_world_entity", "guid", pet),
        ("game_live_pet_kind", "pet_guid", pet),
        ("game_hunter_pet", "owner_guid", hunter),
        ("game_hunter_pet_protocol", "owner_guid", hunter),
        ("game_pet_command", "owner_guid", hunter),
    ] {
        assert!(
            shard
                .query_rows(&format!("SELECT * FROM {table} WHERE {key} = {value}"))
                .is_empty(),
            "{table} keeps the abandoned pet"
        );
    }

    let replacement = wild_boar(&shard, &owner);
    assert!(
        poll_until(POLL_TIMEOUT, || shard
            .call("debug_force_cast_at", &[&owner, TAME, &replacement])
            .status
            .success()),
        "a Hunter tames again after abandoning its pet"
    );
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT presence FROM game_hunter_pet WHERE owner_guid = {hunter}"
            ))
            .remove(0)["presence"],
        OUT
    );
}

fn identity(shard: &Standalone) -> Row {
    shard
        .query_rows(
            "SELECT name, rename_allowed, name_timestamp FROM game_hunter_pet WHERE owner_guid = 1",
        )
        .remove(0)
}

fn protocol_name(shard: &Standalone) -> (String, u32) {
    let row = shard
        .query_rows(
            "SELECT name, name_timestamp FROM game_hunter_pet_protocol WHERE owner_guid = 1",
        )
        .remove(0);
    (row["name"].clone(), number(&row, "name_timestamp"))
}

fn live_pet_flags(shard: &Standalone) -> u32 {
    let row = shard
        .query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {}",
            HUNTER_PET
        ))
        .remove(0);
    number(&row, "unit_flags")
}

fn rename(shard: &Standalone, owner: &str, pet_guid: u64, name: &str) -> std::process::Output {
    shard.call(
        "gw_pet_rename",
        &[
            &actor(owner),
            &pet_guid.to_string(),
            &serde_json::to_string(name).unwrap(),
        ],
    )
}

fn assert_refused(output: std::process::Output, reason: &str) {
    assert!(!output.status.success(), "the rename must be refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(reason), "expected {reason:?} in {stderr}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hunter_renames_its_own_pet_once_and_the_name_outlives_logout() {
    let shard = hunter("hunter-pet-rename");
    let wild = wild_boar(&shard, HUNTER);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    let pet = HUNTER_PET;
    let template_name = shard
        .query_rows(&format!(
            "SELECT name FROM game_creature_template WHERE entry = {BOAR}"
        ))
        .remove(0)["name"]
        .clone();
    let tamed = identity(&shard);
    assert_eq!(tamed["name"], template_name);
    assert_eq!(tamed["rename_allowed"], "true");
    let tamed_at: u32 = number(&tamed, "name_timestamp");
    assert!(tamed_at > 0, "taming stamps the name");
    assert_eq!(protocol_name(&shard), (template_name.clone(), tamed_at));
    assert_ne!(live_pet_flags(&shard) & PET_RENAME_FLAG, 0);

    for (pet_guid, name, reason) in [
        (wild.parse().unwrap(), "Fang", "has no live pet"),
        (pet, "Fang 2", PET_NAME_INVALID),
        (pet, "F", PET_NAME_INVALID),
        (pet, "Fangfangfangf", PET_NAME_INVALID),
    ] {
        assert_refused(rename(&shard, HUNTER, pet_guid, name), reason);
        assert_eq!(identity(&shard), tamed, "{name:?} changed the identity");
        assert_eq!(protocol_name(&shard), (template_name.clone(), tamed_at));
        assert_ne!(live_pet_flags(&shard) & PET_RENAME_FLAG, 0);
    }

    assert!(rename(&shard, HUNTER, pet, "Fang").status.success());
    let renamed = identity(&shard);
    assert_eq!(renamed["name"], "Fang");
    assert_eq!(renamed["rename_allowed"], "false");
    let renamed_at: u32 = number(&renamed, "name_timestamp");
    assert!(
        renamed_at > tamed_at,
        "the client asks again only for a new stamp"
    );
    assert_eq!(protocol_name(&shard), ("Fang".to_string(), renamed_at));
    assert_eq!(live_pet_flags(&shard) & PET_RENAME_FLAG, 0);

    assert_refused(rename(&shard, HUNTER, pet, "Claw"), "already renamed");
    assert_eq!(identity(&shard), renamed);

    shard.assert_call("debug_logout_character", &[HUNTER]);
    assert!(shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {pet}"
        ))
        .is_empty());
    assert_eq!(identity(&shard), renamed);
    assert_eq!(protocol_name(&shard), ("Fang".to_string(), renamed_at));
    assert_refused(rename(&shard, HUNTER, pet, "Claw"), "not in world");
    assert_eq!(identity(&shard), renamed);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_summoned_pet_cannot_be_renamed_as_a_hunter_pet() {
    use lyracore_shared::spell::taxonomy::{E_SUMMON_PET, T_SELF};

    let shard = hunter("summoned-pet-rename");
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_flags = 0, duration_ms = 0 WHERE spell_id = {TAME}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET kind = {E_SUMMON_PET}, target = {T_SELF}, p0 = {BOAR} WHERE spell_id = {TAME}"
    ));
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, HUNTER]);
    let pet = HUNTER_PET;
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT hunter_pet_id FROM game_live_pet_kind WHERE pet_guid = {pet}"
        ))[0]["hunter_pet_id"],
        "0"
    );
    for name in ["Fang", "Fang 2"] {
        assert_refused(rename(&shard, HUNTER, pet, name), "not a Hunter pet");
        assert!(shard.query_rows("SELECT * FROM game_hunter_pet").is_empty());
        assert!(shard
            .query_rows("SELECT * FROM game_hunter_pet_protocol")
            .is_empty());
        assert_eq!(
            shard.query_rows(&format!(
                "SELECT owner_guid FROM game_world_entity WHERE guid = {pet}"
            ))[0]["owner_guid"],
            HUNTER
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn existing_live_pet_recovers_rename_eligibility_without_losing_other_flags() {
    let shard = hunter("hunter-pet-name-upgrade");
    let wild = wild_boar(&shard, HUNTER);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    let pet = HUNTER_PET;
    let original_name = protocol_name(&shard);
    shard.assert_sql("UPDATE game_hunter_pet SET name_timestamp = 0 WHERE owner_guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, unit_flags = 512 WHERE guid = {pet}"
    ));
    assert!(poll_until(POLL_TIMEOUT, || live_pet_flags(&shard)
        & PET_RENAME_FLAG
        != 0));
    assert_ne!(live_pet_flags(&shard) & 512, 0);
    assert_eq!(protocol_name(&shard), original_name);

    assert!(rename(&shard, HUNTER, pet, "Fang").status.success());
    let renamed_name = protocol_name(&shard);
    assert!(renamed_name.1 > original_name.1);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, unit_flags = 528 WHERE guid = {pet}"
    ));
    assert!(poll_until(POLL_TIMEOUT, || live_pet_flags(&shard)
        & PET_RENAME_FLAG
        == 0));
    assert_ne!(live_pet_flags(&shard) & 512, 0);
    assert_eq!(protocol_name(&shard), renamed_name);
}
