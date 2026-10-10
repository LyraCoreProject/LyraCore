//! Benilla packets across the Gateway and a private Module instance.

use super::*;
use crate::accept::BlockingTaskCapacity;
use crate::config::GatewayConfig;
use crate::durable_test_support::Standalone;
use crate::stdb::Coordinator;
use std::time::{Duration, Instant};

struct Realm {
    coordinator: Coordinator,
    runtime: tokio::runtime::Runtime,
    standalone: Standalone,
}

impl Realm {
    fn start(name: &str) -> Self {
        for variable in [
            "LYRACORE_SHARD_MAP",
            "LYRACORE_SHARD_MAP_FILE",
            "LYRACORE_REALM_CORE",
        ] {
            assert!(std::env::var_os(variable).is_none(), "unset {variable}");
        }
        let mut standalone = Standalone::start(name);
        standalone.publish_module();
        standalone.assert_call("claim_operator", &[]);
        standalone.assert_call("install_guid_range", &["0"]);
        standalone.assert_call("gw_heartbeat", &[]);
        let cfg = GatewayConfig {
            logon_bind: "127.0.0.1:0".into(),
            world_bind: "127.0.0.1:0".into(),
            stdb_uri: standalone.server().into(),
            module_name: standalone.shard_name().into(),
            coordinator_token: Some(standalone.owner_token()),
            gateway_id: name.into(),
            blocking_task_capacity: BlockingTaskCapacity::new(4),
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let coordinator = runtime.block_on(Coordinator::connect(&cfg)).unwrap();
        {
            let _entered = runtime.enter();
            coordinator.spawn_gateway_heartbeat();
        }
        Self {
            coordinator,
            runtime,
            standalone,
        }
    }

    fn connect(&self, username: &str) -> (Client, std::thread::JoinHandle<anyhow::Result<()>>) {
        let account = self
            .coordinator
            .account_by_username(username)
            .unwrap()
            .unwrap()
            .id;
        self.coordinator
            .establish_session(
                account,
                &[7; 40],
                self.coordinator.bound_identity(account).unwrap(),
            )
            .unwrap();
        let (socket, gateway_socket) = world_session_socket_pair();
        let coordinator = self.coordinator.clone();
        let runtime = self.runtime.handle().clone();
        let gateway = std::thread::spawn(move || {
            let _entered = runtime.enter();
            run_world_session(gateway_socket, Arc::new(coordinator))
        });
        (Client::connect_named(socket, [7; 40], username), gateway)
    }

    fn create_observer(&self) -> u64 {
        self.coordinator
            .provision_account("OBSERVER", &[0; 32], &[0; 32])
            .unwrap();
        let account = self
            .coordinator
            .account_by_username("OBSERVER")
            .unwrap()
            .unwrap()
            .id;
        assert_eq!(
            self.coordinator
                .create_character(
                    account,
                    "Observer",
                    1,
                    1,
                    0,
                    crate::codec::Appearance::default(),
                )
                .unwrap(),
            crate::codec::CharCreateOutcome::Success
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(character) = self.coordinator.characters(account).unwrap().first() {
                return character.guid;
            }
            assert!(
                Instant::now() < deadline,
                "Character projection did not arrive"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Client {
    fn fields(&self, guid: u64) -> messages::ObjectFields {
        messages::ObjectFields::from_pairs(
            &self
                .objects
                .get(&guid)
                .unwrap()
                .iter()
                .map(|(&field, &value)| (field, value))
                .collect::<Vec<_>>(),
        )
    }

    fn created_entry(&mut self, entry: u32) -> u64 {
        let ServerPacket::UpdateObject { objects } = self.until("creature creation", |packet| {
            matches!(packet, ServerPacket::UpdateObject { objects } if objects.iter().any(|object|
                matches!(object, Object::Create { mask, .. } if mask.object_entry() == Some(entry))))
        }) else { unreachable!() };
        objects
            .into_iter()
            .find_map(|object| match object {
                Object::Create { guid, mask, .. } if mask.object_entry() == Some(entry) => {
                    Some(guid)
                }
                _ => None,
            })
            .unwrap()
    }

    fn until_view(&mut self, description: &str, matches: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !matches(self) {
            assert!(Instant::now() < deadline, "waiting for {description}");
            if let ServerPacket::InventoryChangeFailure { reason, .. } = self.recv() {
                panic!("waiting for {description}: inventory Refusal {reason}");
            }
        }
    }

    fn item_stacks(&self, entry: u32) -> Vec<u32> {
        let mut stacks: Vec<_> = self
            .objects
            .values()
            .filter(|fields| fields.get(&3) == Some(&entry))
            .map(|fields| fields.get(&14).copied().unwrap_or(0))
            .collect();
        stacks.sort_unstable();
        stacks
    }

    fn backpack_slot(&self, entry: u32) -> Option<u8> {
        let owner = self.fields(1);
        (0..16).find_map(|slot| {
            let guid = owner.player_pack_slot(slot)?;
            (self.objects.get(&guid)?.get(&3) == Some(&entry)).then_some(slot + 23)
        })
    }

    fn until(
        &mut self,
        description: &str,
        mut matches: impl FnMut(&ServerPacket) -> bool,
    ) -> ServerPacket {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut seen = Vec::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "waiting for {description}; saw {seen:?}"
            );
            let packet = self.recv();
            if matches(&packet) {
                return packet;
            }
            seen.push(packet.name());
        }
    }

    fn peer(&mut self, guid: u64) -> messages::MovementBlock {
        let ServerPacket::UpdateObject { objects } = self.until("peer creation", |packet| {
            matches!(packet, ServerPacket::UpdateObject { objects } if objects.iter().any(|object|
                matches!(object, Object::Create { guid: actual, .. } if *actual == guid)))
        }) else {
            unreachable!()
        };
        objects
            .into_iter()
            .find_map(|object| match object {
                Object::Create {
                    guid: actual,
                    movement,
                    ..
                } if actual == guid => Some(movement),
                _ => None,
            })
            .unwrap()
    }

    fn finish(mut self) {
        self.send(opcode::CMSG_LOGOUT_REQUEST, &[]);
        self.until("logout complete", |packet| {
            matches!(packet, ServerPacket::LogoutComplete)
        });
    }
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn benilla_durable_kill_quest_loot_rewards_and_reconnect() {
    const QUEST: u32 = 50900;
    const WOLF: u32 = 51000;
    const JERKY: u32 = 5_090_052;
    let realm = Realm::start("benilla-gameplay");
    realm
        .standalone
        .assert_call("debug_seed_scenario_fixtures", &[]);
    realm.standalone.assert_sql("UPDATE game_creature_template SET health = 1, faction_template = 14, aggro_range = 0, money_min = 25, money_max = 25 WHERE entry = 51000");
    realm.standalone.assert_sql("INSERT INTO game_creature_loot (id, creature_entry, item_entry, chance_bp, count, group_id, quest_only) VALUES (5090099, 51000, 5090052, 10000, 3, 0, false)");
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    client.query_clock();
    realm
        .standalone
        .assert_call("debug_spawn_at_feet", &["1", "51003", "1"]);
    let giver = client.created_entry(51003);
    client.send(
        opcode::CMSG_QUESTGIVER_ACCEPT_QUEST,
        &messages::questgiver_accept_quest(giver, QUEST),
    );
    client.until_view("accepted quest", |client| {
        client
            .fields(1)
            .player_quest_log(0)
            .is_some_and(|q| q.quest_id == QUEST)
    });
    for count in 1..=2 {
        realm
            .standalone
            .assert_call("debug_spawn_at_feet", &["1", "51000", "2"]);
        let wolf = client.created_entry(WOLF);
        client.send(opcode::CMSG_ATTACKSWING, &messages::attack_swing(wolf));
        client.until("quest kill credit", |packet| matches!(packet,
            ServerPacket::QuestUpdateAddKill { quest_id: QUEST, entry: WOLF, count: actual, required: 2, .. } if *actual == count));
        client.until_view("slain creature", |client| {
            client.fields(wolf).unit_health() == Some(0)
        });
        client.send(opcode::CMSG_ATTACKSTOP, &[]);
        client.send(opcode::CMSG_LOOT, &messages::loot(wolf));
        let ServerPacket::LootResponse { gold, items, .. } = client.until(
            "loot window",
            |packet| matches!(packet, ServerPacket::LootResponse { guid, .. } if *guid == wolf),
        ) else {
            unreachable!()
        };
        assert_eq!(gold, 25);
        let item = items.iter().find(|item| item.item_id == JERKY).unwrap();
        assert_eq!(item.count, 3);
        client.send(opcode::CMSG_LOOT_MONEY, &messages::loot_money());
        client.send(
            opcode::CMSG_AUTOSTORE_LOOT_ITEM,
            &messages::autostore_loot_item(item.slot),
        );
        client.until_view("looted money and item", |client| {
            client.fields(1).player_money() == Some(count * 25)
                && client.item_stacks(JERKY) == [count * 3]
        });
        client.send(opcode::CMSG_LOOT_RELEASE, &messages::loot_release(wolf));
        client.until("loot closed", |packet| matches!(packet, ServerPacket::LootReleaseResponse { guid, .. } if *guid == wolf));
    }
    client.send(
        opcode::CMSG_QUESTGIVER_REQUEST_REWARD,
        &messages::questgiver_request_reward(giver, QUEST),
    );
    client.until("quest reward screen", |packet| matches!(packet, ServerPacket::QuestGiverOfferReward(offer) if offer.quest_id == QUEST));
    client.send(
        opcode::CMSG_QUESTGIVER_CHOOSE_REWARD,
        &messages::questgiver_choose_reward(giver, QUEST, 0),
    );
    let ServerPacket::QuestGiverComplete(reward) = client.until("quest completion", |packet|
        matches!(packet, ServerPacket::QuestGiverComplete(reward) if reward.quest_id == QUEST)) else { unreachable!() };
    assert_eq!((reward.money, reward.xp), (150, 90));
    client.until_view("quest rewards", |client| {
        client.fields(1).player_money() == Some(200) && client.item_stacks(JERKY) == [8]
    });
    let xp = client.fields(1).player_xp().unwrap();
    client.finish();
    gateway.join().unwrap().unwrap();
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    assert_eq!(client.fields(1).player_money(), Some(200));
    assert_eq!(client.fields(1).player_xp(), Some(xp));
    assert_eq!(client.item_stacks(JERKY), [8]);
    client.finish();
    gateway.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn benilla_durable_inventory_split_updates_both_stacks_and_survives_reconnect() {
    const JERKY: u32 = 5_090_052;
    let realm = Realm::start("benilla-inventory");
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    realm
        .standalone
        .assert_call("debug_grant_item", &["1", &JERKY.to_string(), "10"]);
    client.until_view("granted stack", |client| {
        client.item_stacks(JERKY) == [10] && client.backpack_slot(JERKY).is_some()
    });
    let source = client.backpack_slot(JERKY).unwrap();
    let destination = (0..16)
        .find(|slot| client.fields(1).player_pack_slot(*slot).unwrap_or(0) == 0)
        .unwrap()
        + 23;
    for (slot, count) in [
        (0, 3),
        (19, 3),
        (source, 3),
        (destination, 0),
        (destination, 10),
    ] {
        client.send(
            opcode::CMSG_SPLIT_ITEM,
            &messages::split_item(255, source, 255, slot, count),
        );
        client.until("invalid split Refusal", |packet| {
            matches!(
                packet,
                ServerPacket::InventoryChangeFailure { reason: 3, .. }
            )
        });
        assert_eq!(client.item_stacks(JERKY), [10]);
    }
    client.send(
        opcode::CMSG_SPLIT_ITEM,
        &messages::split_item(255, source, 255, destination, 3),
    );
    client.until_view("split stacks", |client| client.item_stacks(JERKY) == [3, 7]);
    client.finish();
    gateway.join().unwrap().unwrap();
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    assert_eq!(client.item_stacks(JERKY), [3, 7]);
    client.finish();
    gateway.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn benilla_durable_destroy_item_updates_the_stack_and_preserves_indestructible_items() {
    const JERKY: u32 = 5_090_052;
    let realm = Realm::start("benilla-destroy-item");
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    realm
        .standalone
        .assert_call("debug_grant_item", &["1", &JERKY.to_string(), "10"]);
    client.until_view("granted stack", |client| {
        client.item_stacks(JERKY) == [10] && client.backpack_slot(JERKY).is_some()
    });
    let slot = client.backpack_slot(JERKY).unwrap();
    client.send(
        opcode::CMSG_DESTROYITEM,
        &messages::destroy_item(255, slot, 3),
    );
    client.until_view("partial destruction", |client| {
        client.item_stacks(JERKY) == [7]
    });
    realm
        .standalone
        .assert_sql("UPDATE game_item_template SET item_flags = 32 WHERE entry = 5090052");
    client.send(
        opcode::CMSG_DESTROYITEM,
        &messages::destroy_item(255, slot, 0),
    );
    client.until("indestructible Refusal", |packet| {
        matches!(
            packet,
            ServerPacket::InventoryChangeFailure { reason: 24, .. }
        )
    });
    assert_eq!(client.item_stacks(JERKY), [7]);
    realm
        .standalone
        .assert_sql("UPDATE game_item_template SET item_flags = 0 WHERE entry = 5090052");
    client.send(
        opcode::CMSG_DESTROYITEM,
        &messages::destroy_item(255, slot, 0),
    );
    client.until_view("whole stack destruction", |client| {
        client.item_stacks(JERKY).is_empty()
            && client.fields(1).player_pack_slot(slot - 23) == Some(0)
    });
    client.finish();
    gateway.join().unwrap().unwrap();
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    assert!(client.item_stacks(JERKY).is_empty());
    client.finish();
    gateway.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn benilla_durable_destroying_equipped_weapon_updates_the_character_sheet() {
    const BLADE: u32 = 5_090_050;
    let realm = Realm::start("benilla-destroy-equipped");
    realm.standalone.assert_sql(
        "UPDATE game_item_template SET damage_min = 100, damage_max = 150 WHERE entry = 5090050",
    );
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    realm
        .standalone
        .assert_call("debug_grant_item", &["1", &BLADE.to_string(), "1"]);
    client.until_view("blade granted", |client| {
        client.backpack_slot(BLADE).is_some()
    });
    client.send(
        opcode::CMSG_AUTOEQUIP_ITEM,
        &messages::auto_equip_item(255, client.backpack_slot(BLADE).unwrap()),
    );
    client.until_view("blade equipped", |client| {
        client
            .fields(1)
            .player_inv_slot(15)
            .is_some_and(|guid| client.fields(guid).object_entry() == Some(BLADE))
    });
    let weapon = client.fields(1).player_inv_slot(15).unwrap();
    assert_ne!(weapon, 0);
    let damage = || {
        realm
            .standalone
            .query_rows("SELECT sheet_dmg_min FROM game_world_entity WHERE guid = 1")[0]
            ["sheet_dmg_min"]
            .parse::<f32>()
            .unwrap()
    };
    let armed = damage();
    assert!(armed >= 100.0, "equipped blade damage is {armed}");
    client.send(
        opcode::CMSG_DESTROYITEM,
        &messages::destroy_item(255, 15, 0),
    );
    client.until_view("equipped weapon destruction", |client| {
        !client.objects.contains_key(&weapon) && client.fields(1).player_inv_slot(15) == Some(0)
    });
    let unarmed = damage();
    assert!(unarmed < 100.0, "unarmed damage is {unarmed}, was {armed}");
    client.finish();
    gateway.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn benilla_durable_bag_contents_remain_addressable_after_reconnect() {
    const JERKY: u32 = 5_090_052;
    const BAG: u32 = 5_090_054;
    let realm = Realm::start("benilla-bags");
    realm.standalone.assert_sql("UPDATE game_item_template SET class = 1, inventory_type = 18, container_slots = 4, required_level = 0 WHERE entry = 5090054");
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    for (entry, count) in [(BAG, 1), (JERKY, 10)] {
        realm.standalone.assert_call(
            "debug_grant_item",
            &["1", &entry.to_string(), &count.to_string()],
        );
        client.until_view("granted item", |client| {
            client.backpack_slot(entry).is_some()
        });
    }
    client.send(
        opcode::CMSG_AUTOEQUIP_ITEM,
        &messages::auto_equip_item(255, client.backpack_slot(BAG).unwrap()),
    );
    client.until_view("equipped bag", |client| {
        client.fields(1).player_inv_slot(19).unwrap_or(0) != 0
    });
    let bag_guid = client.fields(1).player_inv_slot(19).unwrap();
    assert_eq!(client.fields(bag_guid).container_num_slots(), Some(4));
    let source = client.backpack_slot(JERKY).unwrap();
    client.send(
        opcode::CMSG_SPLIT_ITEM,
        &messages::split_item(255, source, 19, 0, 3),
    );
    client.until_view("split into bag", |client| {
        client.item_stacks(JERKY) == [3, 7]
            && client.fields(bag_guid).container_slot(0).unwrap_or(0) != 0
    });
    for (bag, slot) in [(19, 4), (22, 0), (255, 120)] {
        client.send(
            opcode::CMSG_SPLIT_ITEM,
            &messages::split_item(255, source, bag, slot, 1),
        );
        client.until("invalid bag slot Refusal", |packet| {
            matches!(
                packet,
                ServerPacket::InventoryChangeFailure { reason: 3, .. }
            )
        });
        assert_eq!(client.item_stacks(JERKY), [3, 7]);
    }
    client.send(
        opcode::CMSG_DESTROYITEM,
        &messages::destroy_item(255, 19, 0),
    );
    client.until("nonempty bag Refusal", |packet| {
        matches!(
            packet,
            ServerPacket::InventoryChangeFailure { reason: 31, .. }
        )
    });
    client.send(
        opcode::CMSG_SWAP_ITEM,
        &messages::swap_item(255, 30, 255, 19),
    );
    client.until("moving a nonempty bag is refused", |packet| {
        matches!(
            packet,
            ServerPacket::InventoryChangeFailure { reason: 31, .. }
        )
    });
    client.send(
        opcode::CMSG_SWAP_ITEM,
        &messages::swap_item(255, 20, 255, source),
    );
    client.until("only bags fit bag equipment slots", |packet| {
        matches!(packet, ServerPacket::InventoryChangeFailure { .. })
    });
    client.send(
        opcode::CMSG_SWAP_ITEM,
        &messages::swap_item(255, source, 255, 15),
    );
    client.until("reverse swap cannot equip food", |packet| {
        matches!(packet, ServerPacket::InventoryChangeFailure { .. })
    });
    assert_eq!(client.fields(1).player_inv_slot(19), Some(bag_guid));
    assert_eq!(client.item_stacks(JERKY), [3, 7]);
    client.finish();
    gateway.join().unwrap().unwrap();
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    client.until_view("reconnected bag contents", |client| {
        client.fields(bag_guid).container_slot(0).unwrap_or(0) != 0
    });
    assert_eq!(client.item_stacks(JERKY), [3, 7]);
    client.send(opcode::CMSG_SWAP_ITEM, &messages::swap_item(19, 1, 19, 0));
    client.until_view("move within bag", |client| {
        client.fields(bag_guid).container_slot(0) == Some(0)
            && client.fields(bag_guid).container_slot(1).unwrap_or(0) != 0
    });
    const BLADE: u32 = 5_090_050;
    realm
        .standalone
        .assert_call("debug_grant_item", &["1", &BLADE.to_string(), "1"]);
    client.until_view("blade granted", |client| {
        client.backpack_slot(BLADE).is_some()
    });
    client.send(
        opcode::CMSG_SWAP_ITEM,
        &messages::swap_item(19, 0, 255, client.backpack_slot(BLADE).unwrap()),
    );
    client.until_view("blade stored in bag", |client| {
        client.fields(bag_guid).container_slot(0).unwrap_or(0) != 0
    });
    let blade_guid = client.fields(bag_guid).container_slot(0).unwrap();
    client.send(
        opcode::CMSG_AUTOEQUIP_ITEM,
        &messages::auto_equip_item(19, 0),
    );
    client.until_view("blade equipped from bag", |client| {
        client.fields(1).player_inv_slot(15) == Some(blade_guid)
    });
    client.send(opcode::CMSG_DESTROYITEM, &messages::destroy_item(19, 0, 0));
    client.until_view("replaced weapon removed", |client| {
        client.fields(bag_guid).container_slot(0) == Some(0)
    });
    realm
        .standalone
        .assert_sql("UPDATE game_world_entity SET health = 40 WHERE guid = 1");
    client.until_view("injured Character", |client| {
        client.fields(1).unit_health() == Some(40)
    });
    client.send(
        opcode::CMSG_USE_ITEM,
        &messages::use_item(19, 1, 0, messages::UseItemTarget::SelfImplicit),
    );
    client.until_view("food consumed from bag", |client| {
        client.item_stacks(JERKY) == [2, 7]
    });
    client.send(opcode::CMSG_DESTROYITEM, &messages::destroy_item(19, 1, 0));
    client.until_view("bag item destruction", |client| {
        client.item_stacks(JERKY) == [7] && client.fields(bag_guid).container_slot(1) == Some(0)
    });
    client.send(
        opcode::CMSG_DESTROYITEM,
        &messages::destroy_item(255, 19, 0),
    );
    client.until_view("empty bag destruction", |client| {
        !client.objects.contains_key(&bag_guid) && client.fields(1).player_inv_slot(19) == Some(0)
    });
    client.finish();
    gateway.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn benilla_durable_cancel_channelling_stops_the_active_channel() {
    let realm = Realm::start("benilla-channel-cancel");
    realm.standalone.assert_sql(
        "UPDATE game_spell SET cast_flags = 128, duration_ms = 60000 WHERE spell_id = 1120",
    );
    realm
        .standalone
        .assert_sql("UPDATE game_creature_template SET health = 100000 WHERE entry = 51000");
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    realm
        .standalone
        .assert_call("debug_learn_spell", &["1", "1120"]);
    realm
        .standalone
        .assert_call("debug_spawn_at_feet", &["1", "51000", "2"]);
    let target = client.created_entry(51000);
    realm.standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 100000, max_health = 100000 WHERE guid = {target}"
    ));
    client.until_view("channel target health", |client| {
        client.fields(target).unit_health() == Some(100000)
    });
    client.send(
        opcode::CMSG_CAST_SPELL,
        &messages::cast_spell(1120, Some(target)),
    );
    client.until("active channel", |packet| {
        matches!(
            packet,
            ServerPacket::ChannelStart {
                spell_id: 1120,
                duration_ms: 60000
            }
        )
    });
    let cancelled_at = Instant::now();
    client.send(opcode::CMSG_CANCEL_CHANNELLING, &1120u32.to_le_bytes());
    client.until("cancelled channel", |packet| {
        matches!(packet, ServerPacket::ChannelUpdate { remaining_ms: 0 })
    });
    assert!(
        cancelled_at.elapsed() < Duration::from_secs(2),
        "channel ended without prompt cancellation"
    );
    client.finish();
    gateway.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn benilla_durable_peer_discovery_movement_and_aoi() {
    let realm = Realm::start("benilla-movement");
    let observer_guid = realm.create_observer();
    let (mut first, first_gateway) = realm.connect("TEST");
    first.enter_world();
    let (mut second, second_gateway) = realm.connect("OBSERVER");
    second.enter_world_as(observer_guid, "Observer");
    second.peer(1);
    first.peer(observer_guid);

    let mut movement = movement_info(1);
    movement.position.x = -8948.0;
    movement.position.y = -132.0;
    first.send(
        opcode::MSG_MOVE_START_FORWARD,
        &messages::movement(&movement),
    );
    let packet = second.until("peer movement", |packet| {
        matches!(packet, ServerPacket::PlayerMove { guid: 1, .. })
    });
    assert_movement_packet(packet, opcode::MSG_MOVE_START_FORWARD, 1, &movement);

    let mut transported = movement_info(0x0200_0001);
    transported.position = movement.position;
    transported.timestamp += 100;
    first.send(
        opcode::MSG_MOVE_HEARTBEAT,
        &messages::movement(&transported),
    );
    let packet = second.until("transport movement", |packet| {
        matches!(packet,
        ServerPacket::PlayerMove { guid: 1, time, .. } if *time == transported.timestamp)
    });
    assert_movement_packet(packet, opcode::MSG_MOVE_HEARTBEAT, 1, &transported);
    movement = transported;

    second.finish();
    second_gateway.join().unwrap().unwrap();
    let (mut late, late_gateway) = realm.connect("OBSERVER");
    late.enter_world_as(observer_guid, "Observer");
    let created = late.peer(1);
    assert_eq!(created.position.unwrap().0, movement.position);
    assert_eq!(created.mover.unwrap().flags & 1, 1);
    assert_eq!(created.transport, movement.transport);

    movement.position.x += 1000.0;
    movement.timestamp += 100;
    movement.flags = 0;
    movement.transport = None;
    first.send(opcode::MSG_MOVE_STOP, &messages::movement(&movement));
    late.until("peer leaving the AOI", |packet| match packet {
        ServerPacket::DestroyObject { guid: 1 } => true,
        ServerPacket::UpdateObject { objects } => objects
            .iter()
            .any(|object| matches!(object, Object::OutOfRange { guids } if guids.contains(&1))),
        _ => false,
    });
    movement.position.x -= 1000.0;
    movement.timestamp += 100;
    first.send(opcode::MSG_MOVE_STOP, &messages::movement(&movement));
    assert_eq!(late.peer(1).position.unwrap().0, movement.position);
    first.finish();
    late.finish();
    first_gateway.join().unwrap().unwrap();
    late_gateway.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn benilla_durable_binding_refreshes_the_hearth_point_immediately() {
    let realm = Realm::start("benilla-bind");
    realm
        .standalone
        .assert_call("debug_seed_scenario_fixtures", &[]);
    realm
        .standalone
        .assert_sql("UPDATE game_character SET home_x = 0 WHERE guid = 1");
    realm
        .standalone
        .assert_sql("UPDATE game_creature_template SET npc_flags = 128 WHERE entry = 51003");
    let (mut client, gateway) = realm.connect("TEST");
    client.enter_world();
    realm
        .standalone
        .assert_call("debug_spawn_at_feet", &["1", "51003", "2"]);
    let binder = client.created_entry(51003);
    let character = realm.coordinator.character_by_guid(1).unwrap().unwrap();
    realm.standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = 128, x = {}, y = {}, z = {} WHERE guid = {binder}",
        character.x, character.y, character.z,
    ));
    client.until_view("innkeeper flags", |client| {
        client.fields(binder).unit_npc_flags() == 128
    });
    realm
        .standalone
        .assert_call("debug_spawn_at_feet", &["1", "51003", "100"]);
    let distant = realm.standalone.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = 51003 AND guid != {binder}"
    ))[0]["guid"]
        .parse::<u64>()
        .unwrap();
    client.send(
        opcode::CMSG_BINDER_ACTIVATE,
        &messages::binder_activate(distant),
    );
    client.until("distant innkeeper refusal", |packet| {
        matches!(packet, ServerPacket::MessageChat(_))
    });
    client.until("refused bind complete", |packet| {
        matches!(packet, ServerPacket::GossipComplete)
    });
    assert_eq!(
        realm
            .standalone
            .query_rows("SELECT home_x FROM game_character WHERE guid = 1")[0]["home_x"],
        "0"
    );
    for change in ["npc_flags = 0", "npc_flags = 128, dead = true"] {
        realm.standalone.assert_sql(&format!(
            "UPDATE game_world_entity SET {change} WHERE guid = {binder}"
        ));
        client.send(
            opcode::CMSG_BINDER_ACTIVATE,
            &messages::binder_activate(binder),
        );
        client.until("unavailable innkeeper refusal", |packet| {
            matches!(packet, ServerPacket::MessageChat(_))
        });
        client.until("refused bind complete", |packet| {
            matches!(packet, ServerPacket::GossipComplete)
        });
        assert_eq!(
            realm
                .standalone
                .query_rows("SELECT home_x FROM game_character WHERE guid = 1")[0]["home_x"],
            "0"
        );
    }
    realm.standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = 128, dead = false WHERE guid = {binder}"
    ));
    client.query_clock();
    client.send(
        opcode::CMSG_BINDER_ACTIVATE,
        &messages::binder_activate(binder),
    );
    client.until("updated hearth point", |packet| {
        matches!(packet, ServerPacket::BindPoint { position, map, .. }
            if position.x == character.x && position.y == character.y && position.z == character.z && *map == character.map_id)
    });
    client.finish();
    gateway.join().unwrap().unwrap();
}
