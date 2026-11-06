use super::*;
use crate::standalone_support::{ReducerConnection, Standalone};
use serde_json::json;

struct Character {
    shard: Standalone,
    connection: ReducerConnection,
    guid: u64,
}

impl Character {
    fn imported(name: &str, spells: &[u32]) -> Self {
        let path = std::env::var("LYRACORE_VMANGOS_SQLITE").unwrap();
        let client = std::env::var("LYRACORE_TEST_DBC").unwrap();
        let source = crate::world_source::WorldSource::open(Path::new(&path)).unwrap();
        let plan = selected_world_plan(&source, &client, spells, &[]).unwrap();
        let mut shard = Standalone::start(name);
        shard.publish_module();
        shard.assert_call("claim_operator", &[]);
        shard.assert_call("install_guid_range", &["0"]);
        for sql in plan.stmts {
            shard.assert_sql(&sql);
        }
        shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
        shard.assert_call("debug_advance_roll_batch", &["40"]);
        let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
        shard.assert_call(
            "provision_account",
            &["\"AURAS\"", &credentials, &credentials],
        );
        let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'AURAS'")[0]
            ["id"]
            .clone();
        shard.assert_call(
            "create_character",
            &[
                &account,
                "\"Auracheck\"",
                "1",
                "8",
                "0",
                "0",
                "0",
                "0",
                "0",
                "0",
            ],
        );
        let guid: u64 = shard
            .query_rows("SELECT guid FROM game_character WHERE name = 'Auracheck'")[0]["guid"]
            .parse()
            .unwrap();
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
        shard.assert_call("debug_set_level", &[&guid.to_string(), "60"]);
        let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
        connection.call("native_world_open", json!([42])).unwrap();
        connection
            .call("debug_native_enter_world", json!([guid]))
            .unwrap();
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET power=10000,max_power=10000 WHERE guid={guid}"
        ));
        shard.assert_sql(&format!(
            "DELETE FROM game_item_instance WHERE owner_guid={guid}"
        ));
        shard.assert_call("debug_advance_roll_batch", &["1"]);
        Self {
            shard,
            connection,
            guid,
        }
    }

    fn request(&mut self, opcode: u32, body: Vec<u8>) {
        self.connection
            .call("native_world_receive", json!([0x1dc, vec![0u8; 8]]))
            .unwrap();
        if let Err(reason) = self
            .connection
            .call("native_world_receive", json!([opcode, body]))
        {
            panic!("native request {opcode:x}: {reason}");
        }
    }

    fn cast(&mut self, spell: u32) {
        self.shard.assert_call(
            "debug_learn_spell",
            &[&self.guid.to_string(), &spell.to_string()],
        );
        let mut body = spell.to_le_bytes().to_vec();
        body.extend(0u16.to_le_bytes());
        self.request(0x12e, body);
        assert!(!self
            .shard
            .query_rows(&format!(
                "SELECT * FROM game_aura WHERE target_guid = {} AND spell_id = {spell}",
                self.guid
            ))
            .is_empty());
    }

    fn advance(&self, ticks: u32) {
        self.shard
            .assert_call("debug_advance_roll_batch", &[&(ticks * 40).to_string()]);
    }

    fn pool(&self, column: &str) -> u32 {
        self.shard.query_rows(&format!(
            "SELECT {column} FROM game_world_entity WHERE guid = {}",
            self.guid
        ))[0][column]
            .parse()
            .unwrap()
    }

    fn set_pools(&self) {
        self.shard.assert_sql(&format!("UPDATE game_world_entity SET health=100,max_health=1000,power=100,max_power=1000,spirit=100,mana_regen_paused_until_ms=1800001000000 WHERE guid={}",self.guid));
        self.shard
            .assert_call("debug_set_health", &[&self.guid.to_string(), "100"]);
    }

    fn equip(&mut self, spell: u32) {
        self.shard.assert_call("debug_seed_scenario_fixtures", &[]);
        self.shard.assert_sql(&format!("UPDATE game_item_template SET class=4,subclass=1,inventory_type=10,spellid_1={spell},spelltrigger_1=1,spellid_2=0,spellid_3=0,spellid_4=0,spellid_5=0,required_level=0 WHERE entry=5090050"));
        self.shard.assert_call(
            "debug_grant_item",
            &[&self.guid.to_string(), "5090050", "1"],
        );
        self.shard.assert_sql(&format!(
            "UPDATE game_item_instance SET slot=23 WHERE owner_guid={} AND entry=5090050",
            self.guid
        ));
        self.request(0x10a, vec![255, 23]);
        assert!(
            !self
                .shard
                .query_rows("SELECT * FROM game_item_aura")
                .is_empty(),
            "inventory: {:?}; effects: {:?}",
            self.shard.query_rows(&format!(
                "SELECT guid,entry,slot,durability FROM game_item_instance WHERE owner_guid={}",
                self.guid
            )),
            self.shard.query_rows(&format!(
                "SELECT kind,equip_supported,target FROM game_spell_effect WHERE spell_id={spell}"
            ))
        );
    }
}

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
    assert_eq!(
        character.pool("power"),
        159,
        "full spirit returns after five seconds, even while in combat"
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
