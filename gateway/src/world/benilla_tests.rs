//! Headless Client checks through the WorldStore Seam, using Benilla's independent codec.

use super::{tester_store, warrior_entity, world_session_socket_pair, WorldFake};
use crate::logon::{LogonAccount, LogonStore, RealmInfo};
use crate::world::run_world_session;
use benilla_protocol::messages::{self, opcode, Object, ServerPacket};
use benilla_srp::vanilla_header::{HeaderCrypto, ProofSeed};
use benilla_srp::NormalizedString;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "benilla_tests/durable.rs"]
mod durable;

struct Client {
    socket: UnixStream,
    crypto: HeaderCrypto,
    objects: std::collections::BTreeMap<u64, std::collections::BTreeMap<u16, u32>>,
}

impl Client {
    fn connect(socket: UnixStream, key: [u8; 40]) -> Self {
        Self::connect_named(socket, key, "TESTER")
    }

    fn connect_named(mut socket: UnixStream, key: [u8; 40], username: &str) -> Self {
        let (opcode, body) = read_frame(&mut socket, None);
        let ServerPacket::AuthChallenge { server_seed } = decode(opcode, &body) else {
            panic!("expected world authentication challenge");
        };
        let seed = ProofSeed::new();
        let client_seed = seed.seed();
        let (proof, crypto) = seed.into_client_header_crypto(
            &NormalizedString::new(username).unwrap(),
            key,
            server_seed,
        );
        let body = messages::auth_session(
            u32::from(benilla_protocol::CLIENT_BUILD),
            username,
            client_seed,
            &proof,
            &messages::STOCK_SECURE_ADDONS,
        );
        write_frame(&mut socket, None, opcode::CMSG_AUTH_SESSION, &body);
        let mut client = Self {
            socket,
            crypto,
            objects: Default::default(),
        };
        assert!(matches!(
            client.recv(),
            ServerPacket::AuthResponse {
                result: messages::AUTH_OK,
                ..
            }
        ));
        client
    }

    fn send(&mut self, opcode: u16, body: &[u8]) {
        write_frame(&mut self.socket, Some(&mut self.crypto), opcode, body);
    }

    fn recv(&mut self) -> ServerPacket {
        let (opcode, body) = read_frame(&mut self.socket, Some(&mut self.crypto));
        let packet = decode(opcode, &body);
        match &packet {
            ServerPacket::UpdateObject { objects } => {
                for object in objects {
                    match object {
                        Object::Create { guid, mask, .. } => {
                            self.objects.insert(*guid, mask.raw_fields().collect());
                        }
                        Object::Values { guid, mask } => {
                            self.objects
                                .entry(*guid)
                                .or_default()
                                .extend(mask.raw_fields());
                        }
                        Object::OutOfRange { guids } => {
                            for guid in guids {
                                self.objects.remove(guid);
                            }
                        }
                        _ => {}
                    }
                }
            }
            ServerPacket::DestroyObject { guid } => {
                self.objects.remove(guid);
            }
            _ => {}
        }
        packet
    }

    fn enter_world(&mut self) -> Vec<Object> {
        self.enter_world_as(1, "Tester")
    }

    fn enter_world_as(&mut self, self_guid: u64, name: &str) -> Vec<Object> {
        self.send(opcode::CMSG_CHAR_ENUM, &[]);
        loop {
            if let ServerPacket::CharEnum { characters } = self.recv() {
                assert_eq!(characters.len(), 1);
                assert_eq!(characters[0].guid, self_guid);
                assert_eq!(characters[0].name, name);
                break;
            }
        }
        self.send(opcode::CMSG_PLAYER_LOGIN, &messages::full_guid(self_guid));
        let mut verified_world = false;
        let mut created = Vec::new();
        loop {
            match self.recv() {
                ServerPacket::LoginVerifyWorld { map, position, .. } => {
                    assert_eq!(map, 0);
                    assert!((position.x - -8949.95).abs() < 0.01);
                    verified_world = true;
                }
                ServerPacket::UpdateObject { objects } => {
                    for object in objects {
                        if let Object::Create { guid, ref mask, .. } = object {
                            if guid == self_guid {
                                assert!(
                                    verified_world,
                                    "world verification must precede the self create"
                                );
                                assert!(mask.unit_level().is_some());
                                created.push(object);
                                return created;
                            }
                        }
                        created.push(object);
                    }
                }
                _ => {}
            }
        }
    }

    fn query_clock(&mut self) {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.send(opcode::CMSG_QUERY_TIME, &messages::query_time());
        loop {
            if let ServerPacket::QueryTimeResponse { unix_time } = self.recv() {
                let after = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                assert!((before..=after).contains(&u64::from(unix_time)));
                return;
            }
        }
    }

    fn ping(&mut self, sequence: u32) {
        self.send(opcode::CMSG_PING, &messages::ping(sequence, 37));
        match self.recv() {
            ServerPacket::Pong { sequence: actual } => assert_eq!(actual, sequence),
            other => panic!("expected SMSG_PONG, got {}", other.name()),
        }
    }

    fn logout(&mut self) {
        self.send(opcode::CMSG_LOGOUT_REQUEST, &[]);
        assert!(matches!(
            self.recv(),
            ServerPacket::LogoutResponse { reason: 0, .. }
        ));
        assert!(matches!(self.recv(), ServerPacket::LogoutComplete));
    }
}

fn write_frame(
    socket: &mut UnixStream,
    crypto: Option<&mut HeaderCrypto>,
    opcode: u16,
    body: &[u8],
) {
    let size = u16::try_from(body.len() + 4).unwrap().to_be_bytes();
    let opcode = u32::from(opcode).to_le_bytes();
    let mut header = [size[0], size[1], opcode[0], opcode[1], opcode[2], opcode[3]];
    if let Some(crypto) = crypto {
        crypto.encrypter().encrypt(&mut header);
    }
    socket.write_all(&header).unwrap();
    socket.write_all(body).unwrap();
}

fn read_frame(socket: &mut UnixStream, crypto: Option<&mut HeaderCrypto>) -> (u16, Vec<u8>) {
    let mut header = [0; 4];
    socket
        .read_exact(&mut header)
        .expect("Gateway reply before the socket deadline");
    if let Some(crypto) = crypto {
        crypto.decrypter().decrypt(&mut header);
    }
    let size = u16::from_be_bytes([header[0], header[1]]);
    let opcode = u16::from_le_bytes([header[2], header[3]]);
    let mut body = vec![0; usize::from(size.checked_sub(2).unwrap())];
    socket.read_exact(&mut body).unwrap();
    (opcode, body)
}

fn decode(opcode: u16, body: &[u8]) -> ServerPacket {
    let (packet, tail) = messages::parse_server_with_tail(opcode, body)
        .unwrap_or_else(|error| panic!("Benilla could not decode {opcode:#06x}: {error}"));
    assert_eq!(tail, 0, "unread bytes for {}", packet.name());
    if let ServerPacket::Other { .. } = packet {
        // Benilla does not model these two world-entry messages. Check their 5875 lengths.
        match opcode {
            0x0209 => assert_eq!(body.len(), 128, "SMSG_ACCOUNT_DATA_TIMES"),
            0x021e => assert_eq!(body.len(), 4, "SMSG_SET_REST_START"),
            _ => panic!("unmodelled packet {opcode:#06x}"),
        }
    }
    packet
}

#[test]
fn benilla_login_clock_ping_logout_and_reconnect() {
    let (client_key, saved_key) = authenticate();
    let mut store = tester_store(7);
    store.session.login_entity = Some(warrior_entity());
    store.characters = vec![super::party_tests::character(1, "Tester")];
    store.session.session = Some(crate::world::WorldSession {
        account_id: 7,
        session_key: saved_key,
    });
    let store = Arc::new(store);
    for sequence in [0x1234_5678, u32::MAX] {
        let (socket, gateway_socket) = world_session_socket_pair();
        let gateway_store = store.clone();
        let gateway = std::thread::spawn(move || run_world_session(gateway_socket, gateway_store));
        let mut client = Client::connect(socket, client_key);
        client.enter_world();
        assert_eq!(client.objects[&1].get(&22), Some(&60), "UNIT_FIELD_HEALTH");
        assert_eq!(client.objects[&1].get(&34), Some(&1), "UNIT_FIELD_LEVEL");
        client.query_clock();
        client.ping(sequence);
        client.logout();
        client.enter_world();
        client.query_clock();
        client.ping(sequence.wrapping_add(1));
        client.logout();
        drop(client);
        gateway.join().unwrap().unwrap();
    }
}

pub(crate) struct MovementWorld {
    view: Arc<crate::stdb::world_view::WorldView>,
    relay: fn(&crate::stdb::world_view::WorldView, &crate::stdb::bindings::EntityMotion),
    positions: Mutex<std::collections::BTreeMap<u64, wow_world_messages::vanilla::MovementInfo>>,
}

impl MovementWorld {
    pub(crate) fn update(
        &self,
        guid: u64,
        opcode: u32,
        info: &wow_world_messages::vanilla::MovementInfo,
    ) -> anyhow::Result<()> {
        let movement_info = crate::codec::movement_info_to_bytes(info)?;
        self.positions.lock().unwrap().insert(guid, info.clone());
        let (grid_x, grid_y) =
            lyracore_shared::spatial::grid_cell(info.position.x, info.position.y);
        (self.relay)(
            &self.view,
            &crate::stdb::bindings::EntityMotion {
                guid,
                map_id: 0,
                instance_id: 0,
                grid_x,
                grid_y,
                cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
                opcode: u16::try_from(opcode)?,
                movement_info,
                seq: info.timestamp,
            },
        );
        Ok(())
    }
}

// The caller lives beside the shard callbacks, so it can supply the production motion callback
// without exposing a private production operation for tests.
pub(crate) fn two_clients_observe_movement(
    relay: fn(&crate::stdb::world_view::WorldView, &crate::stdb::bindings::EntityMotion),
) {
    use crate::codec;
    use crate::stdb::world_view::{OwnerGuid, WorldView};
    use crate::world::Outbound;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

    let view = Arc::new(WorldView::new(true));
    let world = Arc::new(MovementWorld {
        view: view.clone(),
        relay,
        positions: Mutex::default(),
    });
    let mut entities = [warrior_entity(), warrior_entity()];
    entities[1].guid = 0x0100_0003;
    let mut clients = Vec::new();
    let mut gateways = Vec::new();
    for (entity, name) in entities.iter().zip(["Tester", "Observer"]) {
        let mut store = tester_store(entity.guid + 7);
        store.session.username = name.to_uppercase();
        store.session.login_entity = Some(entity.clone());
        store.characters = vec![super::party_tests::character(entity.guid, name)];
        store.session.relay_view = Some(view.clone());
        store.session.movement_world = Some(world.clone());
        let store = Arc::new(store);
        let (socket, gateway_socket) = world_session_socket_pair();
        gateways.push(std::thread::spawn(move || {
            run_world_session(gateway_socket, store)
        }));
        let mut client = Client::connect_named(socket, super::K, name);
        client.enter_world_as(entity.guid, name);
        client.query_clock();
        clients.push(client);
    }

    // Supply the Fake's initial peer snapshot. Movement delivery below uses the real cell audience,
    // known-object Gate, writer queue and encryption.
    for index in 0..2 {
        let peer = &entities[1 - index];
        let viewer = view
            .viewer_of_owner(OwnerGuid(entities[index].guid))
            .unwrap();
        viewer.created.lock().unwrap().insert(peer.guid);
        viewer
            .tx
            .send(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(
                    codec::build_create_object(peer, codec::CreateKind::Peer, &[], &[]).unwrap(),
                ),
            )))
            .unwrap();
        let ServerPacket::UpdateObject { objects } = clients[index].recv() else {
            panic!("expected peer creation");
        };
        assert!(objects
            .iter()
            .any(|object| matches!(object, Object::Create { guid, .. } if *guid == peer.guid)));
    }

    for sender in 0..2 {
        for (opcode, flags) in [
            (opcode::MSG_MOVE_START_FORWARD, 1),
            (opcode::MSG_MOVE_JUMP, 0x2000),
            (opcode::MSG_MOVE_START_SWIM, 0x20_0000),
            (opcode::MSG_MOVE_HEARTBEAT, 0x0200_0000),
            (opcode::MSG_MOVE_FALL_LAND, 0x0220_2001),
        ] {
            let mut info = movement_info(flags);
            info.position.x = -8949.0 + sender as f32;
            info.position.y = -132.0;
            let body = messages::movement(&info);
            clients[sender].send(opcode, &body);
            clients[sender].ping(u32::from(opcode)); // Also proves no movement echo to the mover.
            assert_movement_packet(
                clients[1 - sender].recv(),
                opcode,
                entities[sender].guid,
                &info,
            );
            let positions = world.positions.lock().unwrap();
            let stored = positions.get(&entities[sender].guid).unwrap();
            assert_eq!(stored.position.x, info.position.x);
            assert_eq!(stored.position.y, -132.0);
            assert_eq!(stored.position.z, 83.5);
            assert_eq!(stored.timestamp, 123_456);
        }
    }

    let mut distant = movement_info(0);
    distant.position.x = -7949.0;
    distant.position.y = -132.0;
    clients[1].send(opcode::MSG_MOVE_STOP, &messages::movement(&distant));
    clients[1].ping(100);
    clients[0].ping(101); // The out-of-range movement must not precede this pong.
    for client in &mut clients {
        client.logout();
    }
    drop(clients);
    for gateway in gateways {
        gateway.join().unwrap().unwrap();
    }
}

struct LogonFixture {
    account: LogonAccount,
    saved_key: Mutex<Option<[u8; 40]>>,
}

#[derive(Default)]
pub(crate) struct GameplayState {
    pub(crate) selected: u64,
    pub(crate) attacking: Option<u64>,
    pub(crate) copper: u32,
    pub(crate) corpse_money: u32,
    pub(crate) loot: Vec<crate::codec::LootItemView>,
    pub(crate) inventory: Vec<crate::codec::ItemInstanceView>,
}

fn gameplay_store(state: GameplayState) -> Arc<WorldFake> {
    let mut store = tester_store(7);
    store.session.login_entity = Some(warrior_entity());
    store.characters = vec![super::party_tests::character(1, "Tester")];
    store.benilla_gameplay = Some(Mutex::new(state));
    Arc::new(store)
}

fn client_in_world(store: Arc<WorldFake>) -> (Client, std::thread::JoinHandle<anyhow::Result<()>>) {
    let (socket, gateway_socket) = world_session_socket_pair();
    let gateway = std::thread::spawn(move || run_world_session(gateway_socket, store));
    let mut client = Client::connect(socket, super::K);
    client.enter_world();
    client.query_clock();
    (client, gateway)
}

#[test]
fn benilla_selects_a_target_and_starts_and_stops_melee() {
    let store = gameplay_store(GameplayState::default());
    let (mut client, gateway) = client_in_world(store.clone());
    let target = 0xf130_0000_0000_003c;
    client.send(opcode::CMSG_SET_SELECTION, &messages::full_guid(target));
    client.send(opcode::CMSG_ATTACKSWING, &messages::attack_swing(target));
    assert!(
        matches!(client.recv(), ServerPacket::AttackStart { attacker: 1, victim } if victim == target)
    );
    {
        let state = store.benilla_gameplay.as_ref().unwrap().lock().unwrap();
        assert_eq!(state.selected, target);
        assert_eq!(state.attacking, Some(target));
    }
    client.send(opcode::CMSG_ATTACKSTOP, &[]);
    assert!(
        matches!(client.recv(), ServerPacket::AttackStop { attacker: 1, victim, .. } if victim == target)
    );
    assert_eq!(
        store
            .benilla_gameplay
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .attacking,
        None
    );
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_loots_money_and_an_item_then_moves_it_in_inventory() {
    use super::CastStore;
    let store = gameplay_store(GameplayState {
        corpse_money: 25,
        loot: vec![(2, 117, 3, 1956, 0)],
        ..Default::default()
    });
    let (mut client, gateway) = client_in_world(store.clone());
    client.send(opcode::CMSG_LOOT, &messages::loot(60));
    let ServerPacket::LootResponse {
        guid, gold, items, ..
    } = client.recv()
    else {
        panic!("expected loot window");
    };
    assert_eq!(guid, 60);
    assert_eq!(gold, 25);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].slot, 2);
    assert_eq!(items[0].item_id, 117);
    assert_eq!(items[0].count, 3);
    client.send(opcode::CMSG_LOOT_MONEY, &messages::loot_money());
    assert!(matches!(client.recv(), ServerPacket::LootClearMoney));
    client.send(
        opcode::CMSG_AUTOSTORE_LOOT_ITEM,
        &messages::autostore_loot_item(2),
    );
    assert!(matches!(
        client.recv(),
        ServerPacket::LootRemoved { slot: 2 }
    ));
    client.send(opcode::CMSG_LOOT_RELEASE, &messages::loot_release(60));
    assert!(matches!(
        client.recv(),
        ServerPacket::LootReleaseResponse {
            guid: 60,
            result: 1
        }
    ));
    {
        let state = store.benilla_gameplay.as_ref().unwrap().lock().unwrap();
        assert_eq!(state.copper, 25);
        assert_eq!(state.corpse_money, 0);
        assert!(state.loot.is_empty());
    }
    let inventory = store.player_items(1).unwrap();
    assert_eq!(inventory.len(), 1);
    assert_eq!(
        (
            inventory[0].entry,
            inventory[0].stack_count,
            inventory[0].slot
        ),
        (117, 3, 23)
    );
    client.send(opcode::CMSG_SWAP_INV_ITEM, &messages::swap_inv_item(23, 24));
    client.ping(500);
    let inventory = store.player_items(1).unwrap();
    assert_eq!(inventory[0].slot, 24);
    client.logout();
    let created = client.enter_world();
    let mut found = false;
    for object in created {
        if let Object::Create { guid, mask, .. } = object {
            if guid == inventory[0].guid {
                let fields: std::collections::BTreeMap<_, _> = mask.raw_fields().collect();
                assert_eq!(fields.get(&3), Some(&117)); // OBJECT_FIELD_ENTRY
                assert_eq!(fields.get(&6), Some(&1)); // ITEM_FIELD_OWNER
                assert_eq!(fields.get(&14), Some(&3)); // ITEM_FIELD_STACK_COUNT
                found = true;
            }
        }
    }
    assert!(
        found,
        "the acquired item must precede the Character's inventory pointers"
    );
    client.query_clock();
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_accepts_a_quest_requests_its_reward_and_collects_it() {
    use super::QuestActionStore;
    let mut detail = super::detail_view(1234, "A delivery");
    detail.money_reward = 50;
    detail.reward_xp = 40;
    let mut store = gameplay_store(GameplayState::default());
    let setup = Arc::get_mut(&mut store).unwrap();
    setup.quest.quest_details = vec![detail];
    setup.quest.quest_evals = vec![super::eval(1234, crate::codec::ROLE_END, true, true)];
    let (mut client, gateway) = client_in_world(store.clone());
    client.send(
        opcode::CMSG_QUESTGIVER_QUERY_QUEST,
        &messages::questgiver_query_quest(50, 1234),
    );
    let ServerPacket::QuestGiverDetails(details) = client.recv() else {
        panic!("expected quest details");
    };
    assert_eq!(
        (details.npc, details.quest_id, details.money),
        (50, 1234, 50)
    );
    client.send(
        opcode::CMSG_QUESTGIVER_ACCEPT_QUEST,
        &messages::questgiver_accept_quest(50, 1234),
    );
    client.ping(600);
    assert_eq!(store.quest_status(1, 1234), (true, false));
    client.send(
        opcode::CMSG_QUESTGIVER_REQUEST_REWARD,
        &messages::questgiver_request_reward(50, 1234),
    );
    let ServerPacket::QuestGiverOfferReward(offer) = client.recv() else {
        panic!("expected the quest reward screen");
    };
    assert_eq!((offer.npc, offer.quest_id, offer.money), (50, 1234, 50));
    client.send(
        opcode::CMSG_QUESTGIVER_CHOOSE_REWARD,
        &messages::questgiver_choose_reward(50, 1234, 0),
    );
    let ServerPacket::QuestGiverComplete(complete) = client.recv() else {
        panic!("expected quest completion");
    };
    assert_eq!(
        (complete.quest_id, complete.money, complete.xp),
        (1234, 50, 40)
    );
    assert_eq!(store.quest_status(1, 1234), (true, true));
    assert_eq!(
        store
            .benilla_gameplay
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .copper,
        50
    );
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_reward_request_for_an_incomplete_quest_keeps_the_progress_screen() {
    use super::QuestActionStore;
    let mut store = gameplay_store(GameplayState::default());
    let setup = Arc::get_mut(&mut store).unwrap();
    setup.quest.quest_details = vec![super::detail_view(1234, "An unfinished quest")];
    setup.quest.quest_evals = vec![super::eval(1234, crate::codec::ROLE_END, true, false)];
    setup.quest.quest_log = Mutex::new(vec![(1234, false)]);
    let (mut client, gateway) = client_in_world(store.clone());
    client.send(
        opcode::CMSG_QUESTGIVER_REQUEST_REWARD,
        &messages::questgiver_request_reward(50, 1234),
    );
    let ServerPacket::QuestGiverRequestItems(progress) = client.recv() else {
        panic!("expected the incomplete quest's progress screen");
    };
    assert_eq!((progress.npc, progress.quest_id), (50, 1234));
    assert!(!progress.is_complete);
    assert_eq!(store.quest_status(1, 1234), (true, false));
    assert_eq!(
        store
            .benilla_gameplay
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .copper,
        0
    );
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

impl LogonStore for LogonFixture {
    fn account(&self, username: &str) -> anyhow::Result<Option<LogonAccount>> {
        Ok((username == "TESTER").then(|| self.account.clone()))
    }

    fn bound_identity(&self, _: u64, _: &str) -> anyhow::Result<[u8; 32]> {
        Ok([7; 32])
    }

    fn save_session(&self, _: u64, _: &str, key: &[u8; 40], _: [u8; 32]) -> anyhow::Result<()> {
        *self.saved_key.lock().unwrap() = Some(*key);
        Ok(())
    }

    fn realms(&self, _: u64, _: &str) -> anyhow::Result<Vec<RealmInfo>> {
        Ok(vec![RealmInfo {
            id: 1,
            name: "Fixture".into(),
            address: "127.0.0.1:8085".into(),
            realm_type: 0,
            population: 0.0,
            number_of_characters: 1,
        }])
    }
}

fn authenticate() -> ([u8; 40], [u8; 40]) {
    use benilla_protocol::auth;
    use benilla_srp::{PublicKey, SrpClientChallenge};
    use wow_srp::server::SrpVerifier;

    let verifier =
        SrpVerifier::from_username_and_password(super::ns("TESTER"), super::ns("PASSWORD"));
    let store = Arc::new(LogonFixture {
        account: LogonAccount {
            id: 7,
            salt: *verifier.salt(),
            verifier: *verifier.password_verifier(),
            banned: false,
        },
        saved_key: Mutex::new(None),
    });
    // Match Benilla's logon policy for public keys whose integer encoding has an ambiguous width.
    for _ in 0..8 {
        let (mut socket, mut gateway_socket) = world_session_socket_pair();
        let logon_store = store.clone();
        let gateway = std::thread::spawn(move || {
            crate::logon::handle_logon(&mut gateway_socket, logon_store.as_ref())
        });
        auth::write_logon_challenge(&mut socket, "TESTER", 5875).unwrap();
        let reply = auth::read_challenge_reply(&mut socket).unwrap();
        let public_key = PublicKey::from_le_bytes(reply.server_public_key).unwrap();
        if !public_key.is_width_stable() {
            drop(socket);
            gateway.join().unwrap().unwrap();
            continue;
        }
        let challenge = SrpClientChallenge::new(
            NormalizedString::new("TESTER").unwrap(),
            NormalizedString::new("PASSWORD").unwrap(),
            reply.generator,
            reply.large_safe_prime,
            public_key,
            reply.salt,
        );
        auth::write_logon_proof(
            &mut socket,
            challenge.client_public_key(),
            challenge.client_proof(),
            &reply.crc_salt,
        )
        .unwrap();
        let proof = auth::read_proof_reply(&mut socket).unwrap();
        let authenticated = challenge.verify_server_proof(proof).unwrap();
        auth::write_realm_list_request(&mut socket).unwrap();
        let realms = auth::read_realm_list(&mut socket).unwrap();
        assert_eq!(realms.len(), 1);
        assert_eq!(realms[0].name, "Fixture");
        assert_eq!(realms[0].characters, 1);
        drop(socket);
        gateway.join().unwrap().unwrap();
        return (
            *authenticated.session_key(),
            store.saved_key.lock().unwrap().unwrap(),
        );
    }
    panic!("no width-stable SRP challenge in eight attempts");
}

#[test]
fn benilla_movement_tails_survive_gateway_decode_and_relay() {
    for flags in [0, 0x2000, 0x20_0000] {
        assert_movement_relay(movement_info(flags));
    }
}

#[test]
fn benilla_movement_acknowledgements_keep_the_world_session_usable() {
    use messages::{MoveMode, SpeedKind};
    let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
    let info = movement_info(0x0220_2001);
    for kind in [
        SpeedKind::Walk,
        SpeedKind::Run,
        SpeedKind::RunBack,
        SpeedKind::Swim,
        SpeedKind::SwimBack,
        SpeedKind::TurnRate,
    ] {
        client.send(
            kind.ack_opcode(),
            &messages::force_speed_ack(1, 4, &info, 7.0),
        );
        client.ping(u32::from(kind.ack_opcode()));
    }
    for mode in [
        MoveMode::Root,
        MoveMode::WaterWalk,
        MoveMode::FeatherFall,
        MoveMode::Hover,
    ] {
        for apply in [true, false] {
            client.send(
                mode.ack_opcode(apply),
                &messages::move_flag_ack(1, 4, &info, mode.ack_carries_apply().then_some(apply)),
            );
            client.ping(u32::from(mode.ack_opcode(apply)));
        }
    }
    client.send(
        opcode::CMSG_MOVE_KNOCK_BACK_ACK,
        &messages::knock_back_ack(1, 4, &info),
    );
    client.ping(900);
    client.send(
        opcode::MSG_MOVE_TELEPORT_ACK,
        &messages::teleport_ack(1, 4, 123_456),
    );
    client.ping(901);
    client.send(
        opcode::CMSG_MOVE_SPLINE_DONE,
        &messages::move_spline_done(&info, 4),
    );
    client.ping(902);
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_transport_movement_survives_gateway_decode_and_relay() {
    for flags in [0x0200_0000, 0x0220_2001, 0x200] {
        for guid in [0, 1, 0x0100_0000_0000_0000, u64::MAX] {
            let mut info = movement_info(flags);
            if let Some(transport) = &mut info.transport {
                transport.guid = guid;
            }
            assert_movement_relay(info);
        }
    }
}

#[test]
fn benilla_movement_refuses_truncated_and_trailing_bytes() {
    let body = messages::movement(&movement_info(0x0220_2001));
    for end in 0..body.len() {
        assert!(crate::codec::read_movement_client(
            u32::from(opcode::MSG_MOVE_HEARTBEAT),
            &body[..end]
        )
        .is_err());
    }
    let mut trailing = body.clone();
    trailing.push(0);
    assert!(
        crate::codec::read_movement_client(u32::from(opcode::MSG_MOVE_HEARTBEAT), &trailing)
            .is_err()
    );
    assert_eq!(
        lyracore_shared::env::fall_time_from_movement_info(&body),
        None
    );
}

fn assert_movement_relay(info: messages::MovementInfo) {
    use crate::codec;
    let sent = messages::movement(&info);
    for &opcode in lyracore_shared::opcodes::movement::SLICE_MOVE_OPCODES {
        let decoded = codec::read_movement_client(opcode, &sent).unwrap().unwrap();
        let stored = codec::movement_info_to_bytes(decoded.movement_info().unwrap()).unwrap();
        assert_eq!(stored, sent, "movement flags {:#010x}", info.flags);
        let (opcode, body) =
            codec::build_movement_relay_raw(opcode, 0x0102_0000_0000_0304, &stored).unwrap();
        assert_movement_packet(decode(opcode, &body), opcode, 0x0102_0000_0000_0304, &info);
    }
}

fn movement_info(flags: u32) -> messages::MovementInfo {
    use benilla_protocol::messages::{JumpInfo, MovementInfo, TransportPose};
    use benilla_protocol::wire::Vector3d;
    MovementInfo {
        flags,
        timestamp: 123_456,
        position: Vector3d {
            x: -100.5,
            y: 20.25,
            z: 83.5,
        },
        orientation: 1.5,
        transport: (flags & 0x0200_0000 != 0).then_some(TransportPose {
            guid: 0xf120_0000_0000_0012,
            pos: Vector3d {
                x: 1.0,
                y: -2.5,
                z: 3.25,
            },
            orientation: 0.5,
        }),
        pitch: if flags & 0x20_0000 != 0 { 0.25 } else { 0.0 },
        fall_time: 2345,
        jump: (flags & 0x2000 != 0).then_some(JumpInfo {
            zspeed: -7.5,
            cos_angle: 0.6,
            sin_angle: 0.8,
            xy_speed: 7.0,
        }),
    }
}

fn assert_movement_packet(
    packet: ServerPacket,
    expected_opcode: u16,
    expected_guid: u64,
    info: &messages::MovementInfo,
) {
    let ServerPacket::PlayerMove {
        guid,
        opcode,
        flags: actual_flags,
        position: actual_pos,
        orientation,
        pitch,
        time,
        fall_time,
        jump,
        transport,
        ..
    } = packet
    else {
        panic!("expected a movement relay");
    };
    assert_eq!(guid, expected_guid);
    assert_eq!(opcode, expected_opcode);
    assert_eq!(actual_flags, info.flags);
    assert_eq!(actual_pos, info.position);
    assert_eq!(orientation, info.orientation);
    assert_eq!(time, info.timestamp);
    assert_eq!(fall_time, info.fall_time);
    assert_eq!(pitch, info.pitch);
    assert_eq!(jump, info.jump);
    assert_eq!(transport, info.transport);
}

#[test]
fn benilla_ticket_refusals_accept_harassment_and_long_text() {
    let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
    let text = "a".repeat(500);
    let mut body = messages::gm_ticket_create(2, 0, [0.0; 3], &text);
    client.send(opcode::CMSG_GMTICKET_CREATE, &body);
    assert!(matches!(
        client.recv(),
        ServerPacket::GmTicketCreated { response: 3 }
    ));
    // The unavailable service does not decompress or allocate a transcript from its header.
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(&u32::MAX.to_le_bytes());
    body.extend_from_slice(b"opaque transcript");
    client.send(opcode::CMSG_GMTICKET_CREATE, &body);
    assert!(matches!(
        client.recv(),
        ServerPacket::GmTicketCreated { response: 3 }
    ));
    client.send(
        opcode::CMSG_GMTICKET_UPDATETEXT,
        &messages::gm_ticket_updatetext(2, &text),
    );
    assert!(matches!(
        client.recv(),
        ServerPacket::GmTicketUpdated { response: 5 }
    ));
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_unavailable_services_return_refusals_and_empty_reads() {
    let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
    client.send(opcode::CMSG_GMTICKET_SYSTEMSTATUS, &[]);
    assert!(matches!(
        client.recv(),
        ServerPacket::GmTicketSystemStatus { status: 0 }
    ));
    client.send(opcode::CMSG_GMTICKET_GETTICKET, &[]);
    assert!(matches!(
        client.recv(),
        ServerPacket::GmTicketAnswer { ticket: None }
    ));
    client.send(
        opcode::CMSG_GMTICKET_CREATE,
        &messages::gm_ticket_create(1, 0, [0.0; 3], "Test ticket"),
    );
    assert!(matches!(
        client.recv(),
        ServerPacket::GmTicketCreated { response: 3 }
    ));
    client.send(
        opcode::CMSG_GMTICKET_UPDATETEXT,
        &messages::gm_ticket_updatetext(1, "Updated ticket"),
    );
    assert!(matches!(
        client.recv(),
        ServerPacket::GmTicketUpdated { response: 5 }
    ));
    client.send(opcode::CMSG_GMTICKET_DELETETICKET, &[]);
    assert!(matches!(
        client.recv(),
        ServerPacket::GmTicketDeleted { response: 9 }
    ));
    for (opcode, body) in [
        (
            opcode::MSG_LIST_STABLED_PETS,
            messages::list_stabled_pets(99),
        ),
        (opcode::CMSG_STABLE_PET, messages::stable_pet(99)),
        (opcode::CMSG_UNSTABLE_PET, messages::unstable_pet(99, 1)),
        (opcode::CMSG_BUY_STABLE_SLOT, messages::buy_stable_slot(99)),
        (
            opcode::CMSG_STABLE_SWAP_PET,
            messages::stable_swap_pet(99, 1),
        ),
    ] {
        client.send(opcode, &body);
        assert!(matches!(
            client.recv(),
            ServerPacket::StableResult { result: 6 }
        ));
    }
    for (opcode, body) in [
        (opcode::CMSG_OPEN_ITEM, messages::open_item(255, 23)),
        (
            opcode::CMSG_WRAP_ITEM,
            messages::wrap_item(255, 23, 255, 24),
        ),
    ] {
        client.send(opcode, &body);
        assert!(matches!(
            client.recv(),
            ServerPacket::InventoryChangeFailure { reason: 39, .. }
        ));
    }
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_battleground_queries_report_no_instances_or_queues() {
    let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
    client.send(
        opcode::CMSG_BATTLEFIELD_LIST,
        &messages::battlefield_list(529),
    );
    assert!(
        matches!(client.recv(), ServerPacket::BattlefieldList(list) if list.map_id == 529 && list.instances.is_empty())
    );
    client.send(opcode::CMSG_BATTLEFIELD_STATUS, &[]);
    for slot in 0..3 {
        assert!(
            matches!(client.recv(), ServerPacket::BattlefieldStatus(status) if status.slot == slot && status.map_id == 0)
        );
    }
    for (opcode, body) in [
        (
            opcode::CMSG_BATTLEFIELD_JOIN,
            messages::battlefield_join(529, 0, false),
        ),
        (
            opcode::CMSG_BATTLEMASTER_JOIN,
            messages::battlemaster_join(99, 529, 0, false),
        ),
        (
            opcode::CMSG_BATTLEFIELD_PORT,
            messages::battlefield_port(529, true),
        ),
    ] {
        client.send(opcode, &body);
        assert!(matches!(client.recv(), ServerPacket::MessageChat(_)));
        for slot in 0..3 {
            assert!(
                matches!(client.recv(), ServerPacket::BattlefieldStatus(status) if status.slot == slot && status.map_id == 0)
            );
        }
    }
    client.send(
        opcode::CMSG_LEAVE_BATTLEFIELD,
        &messages::leave_battlefield(529),
    );
    for slot in 0..3 {
        assert!(
            matches!(client.recv(), ServerPacket::BattlefieldStatus(status) if status.slot == slot && status.map_id == 0)
        );
    }
    client.send(opcode::MSG_PVP_LOG_DATA, &[]);
    assert!(
        matches!(client.recv(), ServerPacket::PvpLogData(board) if !board.ended && board.rows.is_empty())
    );
    client.send(opcode::MSG_BATTLEGROUND_PLAYER_POSITIONS, &[]);
    assert!(
        matches!(client.recv(), ServerPacket::BattlefieldPositions(positions) if positions.players.is_empty() && positions.carrier.is_none())
    );
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_missing_gameplay_returns_a_specific_refusal_without_disconnect() {
    let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
    let requests = [
        (opcode::CMSG_OPENING_CINEMATIC, vec![]),
        (
            opcode::CMSG_SET_LOOKING_FOR_GROUP,
            messages::set_looking_for_group([1, 2, 3], "Deadmines"),
        ),
        (
            opcode::CMSG_PET_SET_ACTION,
            messages::pet_set_action(99, &[(0, 1), (1, 0)]),
        ),
        (opcode::CMSG_PET_ABANDON, messages::pet_abandon(99)),
        (opcode::CMSG_PET_RENAME, messages::pet_rename(99, "Wolf")),
        (opcode::CMSG_PET_STOP_ATTACK, messages::pet_stop_attack(99)),
        (
            opcode::CMSG_PET_CANCEL_AURA,
            messages::pet_cancel_aura(99, 1),
        ),
        (opcode::CMSG_PET_UNLEARN, messages::pet_unlearn(99)),
        (
            opcode::CMSG_PET_SPELL_AUTOCAST,
            messages::pet_spell_autocast(99, 1, true),
        ),
        (opcode::CMSG_UNLEARN_SKILL, 164u32.to_le_bytes().to_vec()),
        (opcode::CMSG_SET_AMMO, messages::set_ammo(2512)),
        (
            opcode::CMSG_STANDSTATECHANGE,
            messages::stand_state_change(1),
        ),
        (opcode::CMSG_MOUNTSPECIAL_ANIM, vec![]),
        (opcode::CMSG_TOGGLE_PVP, vec![]),
        (opcode::CMSG_TOGGLE_HELM, vec![]),
        (opcode::CMSG_TOGGLE_CLOAK, vec![]),
        (
            opcode::CMSG_SET_ACTIONBAR_TOGGLES,
            messages::set_actionbar_toggles(3),
        ),
        (opcode::CMSG_TUTORIAL_FLAG, messages::tutorial_flag(15)),
        (
            opcode::CMSG_SET_FACTION_INACTIVE,
            messages::set_faction_inactive(2, true),
        ),
        (
            opcode::CMSG_SET_WATCHED_FACTION,
            messages::set_watched_faction(-1),
        ),
        (opcode::CMSG_FAR_SIGHT, vec![1]),
        (opcode::CMSG_SUMMON_RESPONSE, messages::summon_response(99)),
        (
            opcode::CMSG_QUEST_CONFIRM_ACCEPT,
            1u32.to_le_bytes().to_vec(),
        ),
        (opcode::CMSG_RESET_INSTANCES, vec![]),
        (opcode::CMSG_REQUEST_RAID_INFO, vec![]),
        (
            opcode::MSG_INSPECT_HONOR_STATS,
            messages::inspect_honor_stats(1),
        ),
    ];
    for (opcode, body) in requests {
        eprintln!("request {opcode:#06x}");
        client.send(opcode, &body);
        assert!(
            matches!(client.recv(), ServerPacket::MessageChat(_)),
            "opcode {opcode:#06x}"
        );
    }
    client.send(
        opcode::CMSG_ACTIVATETAXIEXPRESS,
        &messages::activate_taxi_express(99, 100, &[1, 2, 3]),
    );
    assert!(matches!(
        client.recv(),
        ServerPacket::ActivateTaxiReply { code: 1 }
    ));
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_page_queries_and_explicit_purchase_slots_report_missing_support() {
    let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
    client.send(
        opcode::CMSG_PAGE_TEXT_QUERY,
        &messages::page_text_query(333, 99),
    );
    assert!(
        matches!(client.recv(), ServerPacket::PageTextQueryResponse { page_id: 333, text, next_page_id: 0 } if text.contains("not available"))
    );
    client.send(
        opcode::CMSG_BUY_ITEM_IN_SLOT,
        &messages::buy_item_in_slot(99, 2512, 1, 23, 1),
    );
    assert!(matches!(
        client.recv(),
        ServerPacket::InventoryChangeFailure { reason: 39, .. }
    ));
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_automatic_requests_announce_missing_features_once_per_world_session() {
    let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
    for (opcode, body) in [
        (
            opcode::CMSG_STANDSTATECHANGE,
            messages::stand_state_change(1),
        ),
        (
            opcode::CMSG_SET_ACTIONBAR_TOGGLES,
            messages::set_actionbar_toggles(3),
        ),
        (opcode::CMSG_TUTORIAL_FLAG, messages::tutorial_flag(15)),
    ] {
        client.send(opcode, &body);
        assert!(matches!(client.recv(), ServerPacket::MessageChat(_)));
        client.send(opcode, &body);
        client.ping(u32::from(opcode));
    }
    for opcode in [opcode::CMSG_TUTORIAL_CLEAR, opcode::CMSG_TUTORIAL_RESET] {
        client.send(opcode, &[]);
        client.ping(u32::from(opcode));
    }
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_connection_receipts_and_forced_logout_keep_character_selection_usable() {
    let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
    for opcode in [
        opcode::CMSG_NEXT_CINEMATIC_CAMERA,
        opcode::CMSG_COMPLETE_CINEMATIC,
    ] {
        client.send(opcode, &[]);
        client.ping(u32::from(opcode));
    }
    client.send(opcode::CMSG_SET_ACTIVE_MOVER, &messages::full_guid(1));
    client.ping(1);
    client.send(opcode::CMSG_LOGOUT_CANCEL, &[]);
    assert!(matches!(client.recv(), ServerPacket::LogoutCancelAck));
    client.send(opcode::CMSG_PLAYER_LOGOUT, &[]);
    assert!(matches!(
        client.recv(),
        ServerPacket::LogoutResponse { reason: 0, .. }
    ));
    assert!(matches!(client.recv(), ServerPacket::LogoutComplete));
    client.enter_world();
    client.query_clock();
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_bind_and_respec_confirmations_reach_existing_gameplay() {
    let mut store = gameplay_store(GameplayState::default());
    Arc::get_mut(&mut store).unwrap().npc.innkeeper = true;
    let (mut client, gateway) = client_in_world(store.clone());
    client.send(opcode::CMSG_GOSSIP_HELLO, &messages::gossip_hello(99));
    assert!(matches!(client.recv(), ServerPacket::GossipMessage { .. }));
    client.send(
        opcode::CMSG_GOSSIP_SELECT_OPTION,
        &messages::gossip_select_option(99, 0, None),
    );
    assert!(matches!(client.recv(), ServerPacket::GossipComplete));
    assert!(matches!(
        client.recv(),
        ServerPacket::BinderConfirm { binder: 99 }
    ));
    assert!(!store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
    client.send(opcode::CMSG_BINDER_ACTIVATE, &messages::binder_activate(99));
    assert!(matches!(client.recv(), ServerPacket::GossipComplete));
    assert!(store
        .npc
        .home_bound
        .load(std::sync::atomic::Ordering::SeqCst));
    client.send(
        opcode::MSG_TALENT_WIPE_CONFIRM,
        &messages::talent_wipe_confirm(99),
    );
    assert!(matches!(client.recv(), ServerPacket::GossipComplete));
    assert_eq!(
        *store.trainer.reset_talents_calls.lock().unwrap(),
        [(1, 99)]
    );
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_missing_talent_quote_reports_unavailability() {
    let mut store = gameplay_store(GameplayState::default());
    let setup = Arc::get_mut(&mut store).unwrap();
    setup.characters[0].level = 10;
    setup.trainer.talent_reset_quote = None;
    setup.npc.gossip_opts = vec![super::opt(
        0,
        "Reset talents",
        lyracore_shared::constants::gossip_option::UNLEARNTALENTS,
    )];
    let (mut client, gateway) = client_in_world(store.clone());
    client.send(opcode::CMSG_GOSSIP_HELLO, &messages::gossip_hello(99));
    assert!(matches!(client.recv(), ServerPacket::GossipMessage { .. }));
    client.send(
        opcode::CMSG_GOSSIP_SELECT_OPTION,
        &messages::gossip_select_option(99, 0, None),
    );
    assert!(matches!(client.recv(), ServerPacket::GossipComplete));
    assert!(matches!(client.recv(), ServerPacket::MessageChat(_)));
    assert!(store.trainer.reset_talents_calls.lock().unwrap().is_empty());
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_at_war_uses_a_four_byte_reputation_index_and_one_byte_boolean() {
    let store = gameplay_store(GameplayState::default());
    let (mut client, gateway) = client_in_world(store.clone());
    client.send(
        opcode::CMSG_SET_FACTION_ATWAR,
        &messages::set_faction_at_war(0, true),
    );
    client.ping(1);
    client.send(
        opcode::CMSG_SET_FACTION_ATWAR,
        &messages::set_faction_at_war(63, false),
    );
    client.ping(2);
    assert_eq!(
        *store.trainer.reputation_at_war.lock().unwrap(),
        [(0, true), (63, false)].into_iter().collect()
    );
    client.logout();
    drop(client);
    gateway.join().unwrap().unwrap();
}

#[test]
fn benilla_compatibility_requests_refuse_truncation_and_trailing_bytes() {
    for (opcode, valid) in [
        (
            opcode::CMSG_BATTLEFIELD_JOIN,
            messages::battlefield_join(529, 0, false),
        ),
        (
            opcode::CMSG_PAGE_TEXT_QUERY,
            messages::page_text_query(1, 99),
        ),
        (
            opcode::CMSG_SET_FACTION_ATWAR,
            messages::set_faction_at_war(0, true),
        ),
        (
            opcode::CMSG_SET_FACTION_INACTIVE,
            messages::set_faction_inactive(0, true),
        ),
        (
            opcode::CMSG_SET_WATCHED_FACTION,
            messages::set_watched_faction(-1),
        ),
        (
            opcode::CMSG_ACTIVATETAXIEXPRESS,
            messages::activate_taxi_express(99, 10, &[1, 2]),
        ),
        (
            opcode::CMSG_SET_LOOKING_FOR_GROUP,
            messages::set_looking_for_group([0; 3], "test"),
        ),
        (opcode::CMSG_OPENING_CINEMATIC, vec![]),
    ] {
        let mut bodies = vec![[valid.as_slice(), &[0]].concat()];
        if !valid.is_empty() {
            bodies.push(valid[..valid.len() - 1].to_vec());
        }
        for body in bodies {
            let (mut client, gateway) = client_in_world(gameplay_store(GameplayState::default()));
            client.send(opcode, &body);
            assert_eq!(
                client.socket.read(&mut [0; 1]).unwrap(),
                0,
                "opcode {opcode:#06x}"
            );
            drop(client);
            assert!(gateway.join().unwrap().is_err());
        }
    }
}
