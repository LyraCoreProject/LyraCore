use std::time::Duration;

use lyracore_shared::spell::taxonomy::{
    A_MOD_STAT, A_MOD_STAT_PCT, SPELL_ATTR_AREA_AURA, SPELL_ATTR_PASSIVE, STAT_SPI, STAT_STR,
    TOTEM_TYPE,
};
use lyracore_test_support::{poll_until, ReducerConnection, Standalone};
use wire_client::decode::Field;

use super::{decoded_taxi_entry_packets, enter, fixture};

fn starting_passives(shard: &Standalone) {
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_sql("DELETE FROM game_level_stats WHERE race = 1 AND class = 1 AND level = 1");
    shard.assert_sql("INSERT INTO game_level_stats (race_class_level,race,class,level,strength,agility,stamina,intellect,spirit) VALUES (65793,1,1,1,23,20,22,20,20)");
    for (spell, kind, points, stat, visual) in [
        (20598, A_MOD_STAT_PCT, 5, STAT_SPI, 0),
        (28330, A_MOD_STAT, 3, STAT_STR, 3580),
    ] {
        shard.assert_sql(&format!("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms,visual_id) VALUES ({spell},'Starting passive fixture {spell}',0,0,0,0,0,0,0,1,0,0,0,0,{SPELL_ATTR_PASSIVE},1,0,false,0,0,0,0,0,0,0,0,{visual})"));
        shard.assert_sql(&format!("INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES ({},{spell},0,{kind},{points},0,0.0,0,0,0.0,0,0,0,{stat},1,0,0,false,0.0)", spell * 4));
        shard.assert_sql(&format!(
            "DELETE FROM game_createinfo_spell WHERE spell_id = {spell}"
        ));
        shard.assert_sql(&format!("INSERT INTO game_createinfo_spell (id,race,class,spell_id) VALUES ({spell},1,1,{spell})"));
    }
}

fn slots(shard: &Standalone, character: u64) {
    let auras = shard.query_rows(&format!(
        "SELECT spell_id,slot FROM game_aura WHERE target_guid = {character}"
    ));
    let hidden = auras
        .iter()
        .find(|aura| aura["spell_id"] == "20598")
        .unwrap_or_else(|| panic!("missing hidden passive: {auras:?}"));
    let visible = auras
        .iter()
        .find(|aura| aura["spell_id"] == "28330")
        .unwrap_or_else(|| panic!("missing visual passive: {auras:?}"));
    assert_eq!(
        hidden["slot"], "255",
        "ordinary starting passive must be hidden"
    );
    assert!(
        visible["slot"].parse::<u8>().unwrap() < 32,
        "visual passive must remain visible"
    );
}

fn entry(connection: &mut ReducerConnection, character: u64) {
    let packets = decoded_taxi_entry_packets(&connection.drain_packets());
    let objects = packets
        .iter()
        .flat_map(|packet| match packet.fields.get("objects") {
            Some(Field::List(objects)) => objects.iter(),
            _ => [].iter(),
        })
        .filter_map(|object| match object {
            Field::Object(object) if object.get("guid") == Some(&Field::Guid(character)) => {
                Some(object)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let create = objects
        .iter()
        .find(|object| object.get("kind") == Some(&Field::Unsigned(2)))
        .expect("self CREATE");
    let Some(Field::Object(values)) = create.get("values") else {
        panic!("CREATE values")
    };
    assert_eq!(
        values.get("field_154"),
        Some(&Field::Unsigned(21)),
        "Spirit must include the hidden passive"
    );
    assert_eq!(
        values.get("field_150"),
        Some(&Field::Unsigned(26)),
        "Strength must include the visual passive"
    );
    assert!(
        values.contains_key("field_1110"),
        "crit belongs in self CREATE"
    );
    assert!(
        !values.contains_key("field_1181"),
        "a percentage on base Spirit adds no flat bonus"
    );
    let mut visual_delivered = false;
    for object in objects {
        let Some(Field::Object(values)) = object.get("values") else {
            continue;
        };
        if object.get("kind") == Some(&Field::Unsigned(0)) {
            assert!(
                !values.contains_key("field_1181") && !values.contains_key("field_1110"),
                "sheet state must not replay as VALUES: {values:?}"
            );
        }
        for index in 47..95 {
            visual_delivered |=
                values.get(&format!("field_{index}")) == Some(&Field::Unsigned(28330));
            assert_ne!(
                values.get(&format!("field_{index}")),
                Some(&Field::Unsigned(20598)),
                "ordinary passive must not reach an aura slot"
            );
        }
    }
    assert!(
        visual_delivered,
        "the visual passive must reach its client aura slot"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn starting_passive_visibility_survives_login_reconnect_and_repair() {
    let (shard, character, _) = fixture("native-passive-visibility");
    starting_passives(&shard);
    let mut first = enter(&shard, character);
    slots(&shard, character);
    entry(&mut first, character);
    shard.assert_sql(&format!(
        "UPDATE game_aura SET slot = 5 WHERE target_guid = {character} AND spell_id = 20598"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_aura SET slot = 255 WHERE target_guid = {character} AND spell_id = 28330"
    ));
    shard.assert_call("debug_repair_after_publish", &[]);
    slots(&shard, character);
    drop(first);
    assert!(poll_until(Duration::from_secs(5), || shard
        .query_rows("SELECT * FROM game_native_player")
        .is_empty()));
    let mut second = enter(&shard, character);
    slots(&shard, character);
    entry(&mut second, character);
    for spell in [20598, 28330] {
        assert_eq!(shard.query_rows(&format!("SELECT id FROM game_aura WHERE target_guid = {character} AND spell_id = {spell}")).len(), 1);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn login_delivers_sheet_stats_in_create() {
    let (shard, character, _) = fixture("native-passive-sheet");
    starting_passives(&shard);
    let mut connection = enter(&shard, character);
    entry(&mut connection, character);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn passive_area_and_totem_auras_keep_their_visible_slots() {
    let (shard, character, _) = fixture("native-area-passive-visibility");
    starting_passives(&shard);
    shard.assert_sql(&format!("UPDATE game_creature_template SET creature_type = {TOTEM_TYPE}, faction_template = 1 WHERE entry = 51000"));
    let _connection = enter(&shard, character);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&character.to_string(), "51000", "2"],
    );
    let totem = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
        .iter()
        .map(|row| row["guid"].parse::<u64>().unwrap())
        .max()
        .unwrap();
    shard.assert_sql("UPDATE game_spell_effect SET target = 3 WHERE spell_id = 20598");
    shard.assert_sql(&format!("UPDATE game_spell SET duration_ms = 4294967295, cast_flags = {SPELL_ATTR_AREA_AURA}, visual_id = 3580 WHERE spell_id = 20598"));
    let apply = |caster: u64, target: u64, visible: bool| {
        shard.assert_sql(&format!(
            "DELETE FROM game_aura WHERE target_guid = {target} AND spell_id = 20598"
        ));
        shard.assert_call(
            "debug_force_cast_at",
            &[&caster.to_string(), "20598", &target.to_string()],
        );
        let rows = shard.query_rows(&format!(
            "SELECT slot FROM game_aura WHERE target_guid = {target} AND spell_id = 20598"
        ));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["slot"].parse::<u8>().unwrap() < 48, visible);
    };
    apply(character, character, false);
    apply(character, totem, true);
    shard.assert_sql("UPDATE game_spell SET cast_flags = 0, visual_id = 0 WHERE spell_id = 20598");
    apply(totem, character, true);
    apply(totem, totem, true);
    apply(character, character, false);
}
