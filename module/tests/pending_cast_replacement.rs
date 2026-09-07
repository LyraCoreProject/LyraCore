mod support;

use std::collections::BTreeMap;

use support::Standalone;

const PLAYER_GUID: u64 = 1;
const MANA_BURN: u32 = 8129;
const TEST_WOLF_GUID: u64 = (0xF130_u64 << 48) | (51000_u64 << 24) | 1;
const TRAINER_GUID: u64 = (0xF130_u64 << 48) | (51001_u64 << 24) | 1;

type SqlRow = BTreeMap<String, String>;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_new_timed_cast_replaces_only_the_casters_pending_rows() {
    let mut standalone = Standalone::start("pending-cast-replacement");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql("UPDATE game_spell SET cast_time_ms = 60000 WHERE spell_id = 8129");

    assert!(pending_casts(&standalone).is_empty());

    cast_at(
        &standalone,
        PLAYER_GUID,
        TEST_WOLF_GUID,
        [12.5, 13.25, 42.0],
    );
    let first_rows = pending_casts(&standalone);
    let first = only_cast_for(&first_rows, PLAYER_GUID);
    assert_pending_payload(first, PLAYER_GUID, TEST_WOLF_GUID, ["12.5", "13.25", "42"]);
    assert_one_shot(first);
    let first_id = first["scheduled_id"].clone();

    cast_at(&standalone, TEST_WOLF_GUID, PLAYER_GUID, [20.0, 21.0, 22.0]);
    let two_casters = pending_casts(&standalone);
    assert_eq!(two_casters.len(), 2);
    let wolf_before = only_cast_for(&two_casters, TEST_WOLF_GUID).clone();

    // Stage a second stale row through another real cast, then re-key it to the player. SpacetimeDB
    // accepts this committed update on a scheduled table and preserves its one-shot deadline.
    cast_at(&standalone, TRAINER_GUID, PLAYER_GUID, [30.0, 31.0, 32.0]);
    standalone.assert_sql(&format!(
        "UPDATE game_pending_cast SET caster_guid = {PLAYER_GUID} WHERE caster_guid = {TRAINER_GUID}"
    ));
    let staged = pending_casts(&standalone);
    assert_eq!(casts_for(&staged, PLAYER_GUID).len(), 2);
    assert_eq!(only_cast_for(&staged, TEST_WOLF_GUID), &wolf_before);
    let stale_ids: Vec<String> = casts_for(&staged, PLAYER_GUID)
        .into_iter()
        .map(|row| row["scheduled_id"].clone())
        .collect();
    assert!(stale_ids.contains(&first_id));

    cast_at(
        &standalone,
        PLAYER_GUID,
        TEST_WOLF_GUID,
        [101.5, 102.25, 103.75],
    );

    let replaced = pending_casts(&standalone);
    assert_eq!(replaced.len(), 2);
    assert_eq!(only_cast_for(&replaced, TEST_WOLF_GUID), &wolf_before);
    let player = only_cast_for(&replaced, PLAYER_GUID);
    assert!(!stale_ids.contains(&player["scheduled_id"]));
    assert_pending_payload(
        player,
        PLAYER_GUID,
        TEST_WOLF_GUID,
        ["101.5", "102.25", "103.75"],
    );
    assert_one_shot(player);
}

fn pending_casts(standalone: &Standalone) -> Vec<SqlRow> {
    standalone.query_rows("SELECT * FROM game_pending_cast")
}

fn cast_at(standalone: &Standalone, caster: u64, target: u64, dest: [f32; 3]) {
    let args = [
        caster.to_string(),
        MANA_BURN.to_string(),
        target.to_string(),
        dest[0].to_string(),
        dest[1].to_string(),
        dest[2].to_string(),
    ];
    standalone.assert_call(
        "debug_cast_spell_at",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

fn casts_for(rows: &[SqlRow], caster: u64) -> Vec<&SqlRow> {
    let caster = caster.to_string();
    rows.iter()
        .filter(|row| row["caster_guid"] == caster)
        .collect()
}

fn only_cast_for(rows: &[SqlRow], caster: u64) -> &SqlRow {
    let matches = casts_for(rows, caster);
    assert_eq!(matches.len(), 1, "expected one pending cast for {caster}");
    matches[0]
}

fn assert_pending_payload(row: &SqlRow, caster: u64, target: u64, dest: [&str; 3]) {
    assert_eq!(row["caster_guid"], caster.to_string());
    assert_eq!(row["spell_id"], MANA_BURN.to_string());
    assert_eq!(row["level"], "1");
    assert_eq!(row["target_guid"], target.to_string());
    assert_eq!(row["pushback_count"], "0");
    assert_eq!(row["has_dest"], "true");
    assert_eq!(row["dest_x"], dest[0]);
    assert_eq!(row["dest_y"], dest[1]);
    assert_eq!(row["dest_z"], dest[2]);
}

fn assert_one_shot(row: &SqlRow) {
    let scheduled_at = &row["scheduled_at"];
    assert!(
        scheduled_at.starts_with("(Time = (__timestamp_micros_since_unix_epoch__ = "),
        "pending cast was not a one-shot schedule: {scheduled_at}"
    );
    assert!(
        scheduled_at.ends_with("+00:00))"),
        "pending cast deadline was not a UTC timestamp: {scheduled_at}"
    );
}
