use super::*;
use crate::standalone_support::{ReducerConnection, Standalone};
use serde_json::json;

pub(super) struct Character {
    pub(super) shard: Standalone,
    pub(super) connection: ReducerConnection,
    pub(super) guid: u64,
}

impl Character {
    pub(super) fn imported(name: &str, spells: &[u32]) -> Self {
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

    pub(super) fn request(&mut self, opcode: u32, body: Vec<u8>) {
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

    pub(super) fn cast(&mut self, spell: u32) {
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

    pub(super) fn advance(&self, ticks: u32) {
        self.shard
            .assert_call("debug_advance_roll_batch", &[&(ticks * 40).to_string()]);
    }

    pub(super) fn pool(&self, column: &str) -> u32 {
        self.shard.query_rows(&format!(
            "SELECT {column} FROM game_world_entity WHERE guid = {}",
            self.guid
        ))[0][column]
            .parse()
            .unwrap()
    }

    pub(super) fn set_pools(&self) {
        self.shard.assert_sql(&format!("UPDATE game_world_entity SET health=100,max_health=1000,power=100,max_power=1000,spirit=100,mana_regen_paused_until_ms=1800001000000 WHERE guid={}",self.guid));
        self.shard
            .assert_call("debug_set_health", &[&self.guid.to_string(), "100"]);
    }

    pub(super) fn equip(&mut self, spell: u32) {
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
