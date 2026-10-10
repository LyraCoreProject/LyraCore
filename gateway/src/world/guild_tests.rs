//! Guild session facts over an encrypted World Session: sign-on at world entry, sign-off at
//! logout, and the guild cleanup a Character delete triggers. Guild opcode behaviour is tested
//! against `InMemoryGuildActions` in `handlers/guild.rs`.

use super::*;

/// Character 1 as a member of Guild 7 at rank 3.
fn guild_member_store() -> WorldFake {
    let base = tester_store(7);
    WorldFake {
        session: SessionState {
            login_entity: Some(warrior_entity()),
            ..base.session
        },
        guild: GuildState {
            guild_memberships: vec![codec::GuildMemberView {
                character_guid: 1,
                guild_id: 7,
                rank_id: 3,
                name: "Warrior".into(),
                ..Default::default()
            }],
            ..base.guild
        },
        ..base
    }
}

/// The recorded operations, in call order, without their Shard.
fn recorded(store: &WorldFake) -> Vec<String> {
    store
        .topology
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, what)| what.clone())
        .collect()
}

#[test]
fn a_member_enters_the_world_with_its_guild_on_the_self_create_and_signs_on() {
    let store = std::sync::Arc::new(guild_member_store());
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    let mut guild = None;
    for message in drain_world_entry(&mut client, &mut c_dec) {
        if let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update) = message {
            if let [Object::CreateObject2 {
                mask2: wow_world_messages::vanilla::UpdateMask::Player(player),
                ..
            }] = update.objects.as_slice()
            {
                guild = Some((player.player_guildid(), player.player_guildrank()));
            }
        }
    }
    assert_eq!(guild, Some((Some(7), Some(3))));
    drop(client);
    server.join().unwrap();
    assert!(recorded(&store).contains(&"guild_op:SignOn".to_string()));
}

#[test]
fn logout_signs_the_member_off_before_the_account_claim_is_released() {
    let store = std::sync::Arc::new(guild_member_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    CMSG_LOGOUT_REQUEST {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    for _ in 0..2 {
        ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap();
    }
    drop(client);
    server.join().unwrap();
    let calls = recorded(&store);
    assert!(
        position(&calls, "guild_op:SignOff") < position(&calls, "logout"),
        "Realm-core refuses a sign-off after the Account Claim closes: {calls:?}"
    );
}

#[test]
fn world_entry_finishes_a_leftover_fee_hold_on_the_home_shard() {
    let store = std::sync::Arc::new({
        let base = guild_member_store();
        WorldFake {
            guild: GuildState {
                guild_fee_hold: std::sync::Mutex::new(Some(crate::world::guild_fee::FeeHold {
                    operation_id: 5_090_401,
                    terms: crate::world::guild_fee::FeeTerms::Emblem(Default::default()),
                })),
                ..base.guild
            },
            ..base
        }
    });
    let (client, _c_enc, _c_dec, server) = enter_world(store.clone(), 1);
    drop(client);
    server.join().unwrap();
    let calls = recorded(&store);
    assert!(
        position(&calls, "player_login") < position(&calls, "guild_fee_decide")
            && position(&calls, "guild_fee_decide") < position(&calls, "guild_fee_finish"),
        "{calls:?}"
    );
    assert_eq!(*store.guild.guild_fee_hold.lock().unwrap(), None);
}

#[test]
fn a_world_port_that_fails_after_sign_on_still_signs_off() {
    let store = std::sync::Arc::new({
        let base = guild_member_store();
        WorldFake {
            session: SessionState {
                // The world-port ack is only actionable after teleport_player removed the old entity.
                entity_in_world: false,
                worldport_login_error: Some("character 1 is stranded on map 36".into()),
                ..base.session
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || run_world_session(server_end, server_store.clone()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    drain_world_entry(&mut client, &mut c_dec);
    MSG_MOVE_WORLDPORT_ACK {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    while ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_ok() {}
    drop(client);
    assert!(
        server.join().unwrap().is_err(),
        "the failed entry ends the session"
    );
    let calls = recorded(&store);
    assert!(
        position(&calls, "guild_op:SignOn") < position(&calls, "guild_op:SignOff"),
        "{calls:?}"
    );
    assert!(position(&calls, "guild_op:SignOff") < position(&calls, "logout"));
}

/// Guild state on Realm-core for the deleted-Character reconciliation: Leader 5 and member 6 in
/// one Guild, and a Petition owned by 7 with a Signature by 8. Characters 5 and 7 still exist on
/// the `instances` peer; 6 and 8 exist on no World Shard.
fn guild_cleanup_topology() -> std::sync::Arc<WorldFake> {
    guild_cleanup_topology_failing_lookup_for(None)
}

fn guild_cleanup_topology_failing_lookup_for(
    guild_lookup_error_for: Option<u64>,
) -> std::sync::Arc<WorldFake> {
    let instances = std::sync::Arc::new(WorldFake {
        topology: TopologyState {
            shard: "instances".into(),
            ..Default::default()
        },
        characters: [5, 7]
            .into_iter()
            .map(|guid| codec::CharacterView {
                guid,
                name: format!("Kept{guid}"),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    let world = std::sync::Arc::new(WorldFake {
        topology: TopologyState {
            shard: "world".into(),
            ..Default::default()
        },
        guild: GuildState {
            guild_memberships: [(5, 0), (6, 4)]
                .into_iter()
                .map(|(character_guid, rank_id)| codec::GuildMemberView {
                    character_guid,
                    guild_id: 7,
                    rank_id,
                    ..Default::default()
                })
                .collect(),
            guild_petitions: vec![codec::PetitionView {
                petition_id: 3,
                charter_item_guid: 90,
                owner_guid: 7,
                name: "Boundary Test".into(),
                signers: vec![8],
            }],
            guild_lookup_error_for,
            ..Default::default()
        },
        ..Default::default()
    });
    *world.topology.peers.lock().unwrap() = vec![world.clone(), instances];
    world
}

fn forgotten(store: &WorldFake) -> Vec<String> {
    store
        .topology
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, call)| call.clone())
        .filter(|call| call.starts_with("guild_op:ForgetDeletedCharacter"))
        .collect()
}

fn sweep() -> crate::world::GuildCleanup {
    crate::world::GuildCleanup {
        sweep: true,
        ..Default::default()
    }
}

fn deleted(guids: &[u64]) -> crate::world::GuildCleanup {
    crate::world::GuildCleanup {
        sweep: false,
        deleted: guids.iter().copied().collect(),
    }
}

fn durable_absence_checks(store: &WorldFake) -> usize {
    store
        .character
        .durable_absence_checks
        .load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn only_characters_absent_from_every_world_shard_are_forgotten() {
    let world = guild_cleanup_topology();
    crate::world::reconcile_deleted_guild_characters(world.as_ref(), &sweep()).unwrap();
    assert_eq!(
        forgotten(&world),
        [
            "guild_op:ForgetDeletedCharacter:6",
            "guild_op:ForgetDeletedCharacter:8"
        ]
    );
}

/// A Character row in any Shard's cache proves the Character exists, so the pass takes no durable
/// snapshot for it. Only 6 and 8, gone from every cache, pay for the absence check.
#[test]
fn a_character_still_in_a_shard_cache_takes_no_durable_snapshot() {
    let world = guild_cleanup_topology();
    crate::world::reconcile_deleted_guild_characters(world.as_ref(), &sweep()).unwrap();
    assert_eq!(durable_absence_checks(&world), 2);

    let kept = guild_cleanup_topology();
    crate::world::reconcile_deleted_guild_characters(kept.as_ref(), &deleted(&[5, 7])).unwrap();
    assert_eq!(durable_absence_checks(&kept), 0);
    assert!(forgotten(&kept).is_empty());
}

/// A `game_character` delete owes cleanup for that Character only, and none at all for a
/// Character that no Guild, Petition or Signature names.
#[test]
fn a_single_delete_checks_only_its_own_character() {
    let world = guild_cleanup_topology();
    crate::world::reconcile_deleted_guild_characters(world.as_ref(), &deleted(&[8, 40])).unwrap();
    assert_eq!(forgotten(&world), ["guild_op:ForgetDeletedCharacter:8"]);
    assert_eq!(durable_absence_checks(&world), 1);
}

/// A lookup that fails for one deleted Character does not hold up the others. The pass still
/// fails, so the worker keeps the work and retries it.
#[test]
fn a_failed_lookup_still_cleans_the_other_deleted_characters() {
    let world = guild_cleanup_topology_failing_lookup_for(Some(6));
    let error = crate::world::reconcile_deleted_guild_characters(world.as_ref(), &deleted(&[6, 8]))
        .expect_err("the failed lookup is owed a retry");
    assert!(error.to_string().contains("lookup for 6"));
    assert_eq!(forgotten(&world), ["guild_op:ForgetDeletedCharacter:8"]);
}

#[test]
fn an_unavailable_world_shard_defers_every_guild_cleanup() {
    let world = guild_cleanup_topology();
    let incomplete = WorldFake {
        topology: TopologyState {
            shard: "world".into(),
            calls: world.topology.calls.clone(),
            world_shard_set_error: Some("instances has no healthy Coordinator subscription".into()),
            ..Default::default()
        },
        guild: GuildState {
            guild_memberships: world.guild.guild_memberships.clone(),
            guild_petitions: world.guild.guild_petitions.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    let error = crate::world::reconcile_deleted_guild_characters(&incomplete, &sweep())
        .expect_err("an incomplete Shard set cannot prove a deletion");
    assert!(error.to_string().contains("no healthy Coordinator"));
    assert!(forgotten(&incomplete).is_empty());
}

/// A Guild Leader's delete answers 0x3A, FAILED_GUILD_LEADER in mangos, and never reaches the
/// Home Shard (`cm:CharacterHandler.cpp:540-546`).
#[test]
fn char_delete_of_a_guild_leader_replies_failed_and_deletes_nothing() {
    let store = std::sync::Arc::new({
        let base = tester_store(7);
        WorldFake {
            guild: GuildState {
                guild_memberships: vec![codec::GuildMemberView {
                    character_guid: 5,
                    guild_id: 7,
                    rank_id: 0,
                    name: "Tester".into(),
                    ..Default::default()
                }],
                guilds: vec![codec::GuildView {
                    guild_id: 7,
                    name: "Boundary Test".into(),
                    leader_guid: 5,
                    ..Default::default()
                }],
                ..base.guild
            },
            ..base
        }
    });
    let (mut client, server_end) = world_session_socket_pair();
    let server_store = store.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.clone()).unwrap();
    });

    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_CHAR_DELETE { guid: Guid::new(5) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_CHAR_DELETE(m) => {
            assert_eq!(m.result.as_int(), 0x3A)
        }
        other => panic!("expected SMSG_CHAR_DELETE, got {other}"),
    }
    assert!(store.character.deleted.lock().unwrap().is_empty());

    drop(client);
    server.join().unwrap();
}
