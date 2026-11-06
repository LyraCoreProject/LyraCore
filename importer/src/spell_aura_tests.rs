use super::aura_test_support::Character;

#[test]
#[ignore = "requires local World Source, stock client MPQs, SpacetimeDB 2.14.0 and Wasm"]
fn imported_mage_armor_and_innervate_apply_the_source_five_second_rule() {
    let mut character = Character::imported("imported-mana-rules", &[6117, 24363, 29166]);
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET spirit=100,unit_flags=524288,combat_until_ms=1800001000000 WHERE guid={}",
        character.guid
    ));
    character.cast(6117);
    character
        .shard
        .assert_call("debug_set_power", &[&character.guid.to_string(), "100"]);
    character.advance(1);
    assert_eq!(
        character.pool("power"),
        111,
        "Mage Armor permits 30% of 37.5 spirit mana per tick"
    );
    character.advance(1);
    assert_eq!(character.pool("power"), 122);
    character.advance(1);
    assert_eq!(character.pool("power"), 133);
    character.advance(1);
    assert_eq!(
        character.pool("power"),
        170,
        "the first tick after the five-second deadline restores full spirit in combat"
    );
    character.cast(24363);
    character.set_pools();
    character.advance(1);
    assert_eq!(
        character.pool("power"),
        116,
        "12 MP5 adds to the 30% casting allowance before truncation"
    );
    character.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET power=10000,max_power=10000 WHERE guid={}",
        character.guid
    ));
    character.cast(29166);
    character.set_pools();
    character.advance(1);
    assert_eq!(
        character.pool("power"),
        292,
        "Innervate multiplies spirit by five and permits all of it while casting"
    );
}

#[test]
#[ignore = "requires local World Source, stock client MPQs, SpacetimeDB 2.14.0 and Wasm"]
fn imported_evocation_uses_spirit_and_repairs_its_old_frozen_amount() {
    let mut character = Character::imported("imported-evocation", &[12051]);
    character.cast(12051);
    character.set_pools();
    character.advance(1);
    assert_eq!(
        character.pool("power"),
        700,
        "100 Spirit gives 600 mana per tick with the source 1500% bonus"
    );
    character.shard.assert_sql(&format!("UPDATE game_aura SET eff_kind=146,amount=15,period_ms=2000 WHERE target_guid={} AND effect_id=48204",character.guid));
    character
        .shard
        .assert_call("debug_repair_after_publish", &[]);
    let aura = character.shard.query_rows(&format!(
        "SELECT amount,eff_kind FROM game_aura WHERE target_guid={} AND effect_id=48204",
        character.guid
    ));
    assert_eq!(aura[0]["amount"], "1500");
    assert_eq!(aura[0]["eff_kind"], "183");
}

#[test]
#[ignore = "requires local World Source, stock client MPQs, SpacetimeDB 2.14.0 and Wasm"]
fn imported_mp5_ticks_on_equipment_and_casts_during_the_five_second_rule() {
    let mut character = Character::imported("imported-mp5-consumer", &[18378, 24363]);
    character.cast(24363);
    character.set_pools();
    character.advance(1);
    assert_eq!(
        character.pool("power"),
        104,
        "12 MP5 truncates to 4 mana each two-second tick"
    );
    character.advance(4);
    assert_eq!(
        character.pool("power"),
        120,
        "mana fractions must not accrue to 24"
    );
    character.equip(18378);
    character.set_pools();
    character.advance(1);
    assert_eq!(
        character.pool("power"),
        108,
        "cast contributes 12 MP5 and equip contributes 8 MP5 once"
    );
    character
        .shard
        .assert_call("debug_repair_after_publish", &[]);
    character.set_pools();
    character.advance(1);
    assert_eq!(
        character.pool("power"),
        108,
        "publish repair must not duplicate MP5"
    );
}

#[test]
#[ignore = "requires local World Source, stock client MPQs, SpacetimeDB 2.14.0 and Wasm"]
fn imported_demon_armor_regenerates_flat_health_and_carries_its_fraction() {
    let mut character = Character::imported("imported-hp5-consumer", &[687]);
    character.cast(687);
    character.set_pools();
    character.shard.assert_sql(&format!("UPDATE game_world_entity SET unit_flags=524288,combat_until_ms=1800001000000 WHERE guid={}",character.guid));
    character.advance(1);
    assert_eq!(
        character.pool("health"),
        101,
        "Demon Armor grants 1.2 health per tick"
    );
    character.advance(4);
    assert_eq!(
        character.pool("health"),
        106,
        "the health fraction carries across ticks"
    );
    character.shard.assert_sql(&format!("UPDATE game_aura SET eff_kind=169 WHERE target_guid={} AND spell_id=687 AND effect_id=2749",character.guid));
    character
        .shard
        .assert_call("debug_repair_after_publish", &[]);
    character.set_pools();
    character.advance(1);
    assert_eq!(
        character.pool("health"),
        101,
        "repair converts the stale percentage consumer to flat HP5"
    );
}
