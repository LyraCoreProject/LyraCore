mod support;

use support::{actor, Standalone};

const SOLO: u64 = 1;

const RAID_MAP: u32 = 409;
const DUNGEON_MAP: u32 = 36;
const TO_RAID: u32 = 5_330_001;
const TO_DUNGEON: u32 = 5_330_002;

const INVITE: &str = "0";
const ACCEPT: &str = "1";
const RAID_CONVERT: &str = "6";

fn group_op(shard: &Standalone, op: &str, actor_guid: u64, target_guid: u64) {
    shard.assert_call(
        "realm_group_op",
        &[
            op,
            &actor(actor_guid),
            &target_guid.to_string(),
            "0",
            "0",
            "0",
        ],
    );
}

fn join(shard: &Standalone, inviter: u64, joiner: u64) {
    group_op(shard, INVITE, inviter, joiner);
    group_op(shard, ACCEPT, joiner, 0);
}

fn map_of(shard: &Standalone, guid: u64) -> String {
    shard.query_rows(&format!(
        "SELECT map_id FROM game_character WHERE guid = {guid}"
    ))[0]["map_id"]
        .clone()
}

fn enter(shard: &Standalone, guid: u64, trigger: u32) {
    shard.assert_call(
        "debug_enter_areatrigger",
        &[&guid.to_string(), &trigger.to_string()],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn only_a_raid_member_enters_a_raid_map_through_its_area_trigger() {
    let mut shard = Standalone::start("raid-entry");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let [party_leader, party_member, raid_leader, raid_member] =
        ["PartyLeader", "PartyMember", "RaidLeader", "RaidMember"].map(|name| {
            shard.assert_call(
                "create_character",
                &[
                    "1",
                    &format!("\"{name}\""),
                    "1",
                    "1",
                    "0",
                    "0",
                    "0",
                    "0",
                    "0",
                    "0",
                ],
            );
            shard.query_rows(&format!(
                "SELECT guid FROM game_character WHERE name = '{name}'"
            ))[0]["guid"]
                .parse::<u64>()
                .unwrap()
        });
    for guid in [SOLO, party_leader, party_member, raid_leader, raid_member] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    for (map, map_type) in [(RAID_MAP, 2), (DUNGEON_MAP, 1)] {
        shard.insert_row(
            "game_map",
            &[
                ("id", &map),
                ("internal_name", &"Fixture"),
                ("name", &"Fixture"),
                ("map_type", &map_type),
            ],
        );
    }
    for (trigger, target_map) in [(TO_RAID, RAID_MAP), (TO_DUNGEON, DUNGEON_MAP)] {
        shard.insert_row(
            "game_area_trigger",
            &[
                ("id", &trigger),
                ("map_id", &0),
                ("x", &0.0f32),
                ("y", &0.0f32),
                ("z", &0.0f32),
                ("radius", &5.0f32),
            ],
        );
        shard.insert_row(
            "game_areatrigger_teleport",
            &[
                ("trigger_id", &trigger),
                ("target_map", &target_map),
                ("x", &10.0f32),
                ("y", &20.0f32),
                ("z", &30.0f32),
                ("o", &0.0f32),
                ("name", &"Fixture entrance"),
            ],
        );
    }
    join(&shard, party_leader, party_member);
    join(&shard, raid_leader, raid_member);
    group_op(&shard, RAID_CONVERT, raid_leader, 0);

    for refused in [SOLO, party_leader, party_member] {
        enter(&shard, refused, TO_RAID);
        assert_eq!(
            map_of(&shard, refused),
            "0",
            "{refused} stays outside the raid map"
        );
    }
    assert!(
        shard
            .query_rows("SELECT instance_id FROM game_instance")
            .is_empty(),
        "a refused entry leases no Instance"
    );

    enter(&shard, raid_member, TO_RAID);
    assert_eq!(map_of(&shard, raid_member), RAID_MAP.to_string());
    assert_eq!(
        shard
            .query_rows("SELECT instance_id FROM game_instance")
            .len(),
        1,
        "a raid member is admitted to one Instance"
    );

    enter(&shard, SOLO, TO_DUNGEON);
    assert_eq!(
        map_of(&shard, SOLO),
        DUNGEON_MAP.to_string(),
        "a solo Character still enters a five-player dungeon"
    );
}
