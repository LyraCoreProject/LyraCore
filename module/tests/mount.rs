mod support;

use support::{actor, number, poll_until, Row, Standalone, POLL_TIMEOUT};

const RIDER: &str = "1";
const REINS: &str = "5090054";
const MOUNT: &str = "50310";

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[RIDER]);
    shard.assert_call("debug_set_level", &[RIDER, "20"]);
    shard.assert_call("debug_set_money", &[RIDER, "1000"]);
    shard.assert_call("debug_grant_item", &[RIDER, REINS, "1"]);
    shard
}

fn reins(shard: &Standalone) -> Vec<Row> {
    shard.query_rows("SELECT * FROM game_item_instance WHERE owner_guid = 1 AND entry = 5090054")
}

fn use_reins(shard: &Standalone) {
    let items = reins(shard);
    assert_eq!(items.len(), 1);
    assert!(
        poll_until(POLL_TIMEOUT, || shard
            .call("gw_use_item", &[&actor(RIDER), &items[0]["slot"]])
            .status
            .success()),
        "reusable reins must cast after the global cooldown"
    );
}

fn mount_auras(shard: &Standalone) -> Vec<Row> {
    shard.query_rows("SELECT id, target_guid, caster_guid, spell_id, effect_id, eff_kind, amount, eff_p0, eff_p1 FROM game_aura WHERE target_guid = 1 AND spell_id = 50310")
}

fn assert_projection(shard: &Standalone, display: u32, speed_bp: u32) {
    let rows = shard.query_rows(
        "SELECT mount_display_id, run_speed_mult_bp FROM game_world_entity WHERE guid = 1",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(number::<u32>(&rows[0], "mount_display_id"), display);
    assert_eq!(number::<u32>(&rows[0], "run_speed_mult_bp"), speed_bp);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn an_untrained_mount_refusal_keeps_the_item_vitals_and_auras_unchanged() {
    let shard = fixture("mount-untrained");
    let items = reins(&shard);
    assert_eq!(items.len(), 1);
    let vitals = shard.query_rows(
        "SELECT power, money, mount_display_id, run_speed_mult_bp \
         FROM game_world_entity WHERE guid = 1",
    );
    let auras = shard.query_rows("SELECT id, target_guid, caster_guid, spell_id, effect_id, eff_kind, amount, eff_p0, eff_p1 FROM game_aura WHERE target_guid = 1");

    let refused = shard.call("gw_use_item", &[&actor(RIDER), &items[0]["slot"]]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!refused.status.success(), "{text}");
    assert!(
        text.contains(lyracore_shared::item::ItemRefusal::NotRightNow.as_tag()),
        "{text}"
    );
    assert_eq!(reins(&shard), items);
    assert_eq!(
        shard.query_rows(
            "SELECT power, money, mount_display_id, run_speed_mult_bp \
             FROM game_world_entity WHERE guid = 1"
        ),
        vitals
    );
    assert_eq!(
        shard.query_rows("SELECT id, target_guid, caster_guid, spell_id, effect_id, eff_kind, amount, eff_p0, eff_p1 FROM game_aura WHERE target_guid = 1"),
        auras
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn reusable_reins_restore_the_mount_on_rebuild_and_clear_it_on_dismount() {
    let shard = fixture("mount-lifetime");
    shard.assert_call("debug_spawn_at_feet", &[RIDER, "51007", "5"]);
    let trainers = shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51007");
    assert_eq!(trainers.len(), 1);
    shard.assert_call(
        "gw_trainer_buy",
        &[&actor(RIDER), &trainers[0]["guid"], "50132"],
    );
    let riding = shard.query_rows(
        "SELECT current, max_rank FROM game_player_skill \
         WHERE character_guid = 1 AND skill_line = 762",
    );
    assert_eq!(riding.len(), 1);
    assert_eq!(riding[0]["current"], "75");
    assert_eq!(riding[0]["max_rank"], "75");
    let items = reins(&shard);

    use_reins(&shard);
    assert_eq!(mount_auras(&shard).len(), 2);
    assert_projection(&shard, 1147, 16_000);
    assert_eq!(reins(&shard), items);

    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &[RIDER]);
    assert_eq!(mount_auras(&shard).len(), 2);
    assert_projection(&shard, 1147, 16_000);

    shard.assert_call("gw_cancel_aura", &[&actor(RIDER), MOUNT]);
    assert!(mount_auras(&shard).is_empty());
    assert_projection(&shard, 0, 10_000);

    use_reins(&shard);
    assert_projection(&shard, 1147, 16_000);
    let previous_targets =
        shard.query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000");
    shard.assert_call("debug_spawn_at_feet", &[RIDER, "51000", "5"]);
    let targets: Vec<_> = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .into_iter()
        .filter(|target| {
            !previous_targets
                .iter()
                .any(|row| row["guid"] == target["guid"])
        })
        .collect();
    assert_eq!(targets.len(), 1);
    shard.assert_call("gw_attack", &[&actor(RIDER), &targets[0]["guid"]]);
    assert!(mount_auras(&shard).is_empty());
    assert_projection(&shard, 0, 10_000);
    assert_eq!(reins(&shard), items);
}
