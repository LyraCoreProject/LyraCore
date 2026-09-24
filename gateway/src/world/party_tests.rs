//! Realm-wide party state — the routing tests.
//!
//! What EXECUTES here is production `world::party`, against the same in-memory multi-database
//! topology the cross-database transfer tests use. What the fakes stand in for is named at each
//! seam: `FakeParty` models realm-core's authority (module reducer bodies cannot run in a gateway
//! test — there is no `ReducerContext`), and each shard's `mirror` is exactly what
//! `sync_group_mirror` wrote there.
//!
//! A child module of `world::tests` so it can reach `InMemoryStore` without widening anything.

use super::*;
use lyracore_shared::group::{realm_op, GroupRefusal, RosterMember, RosterPayload};

pub(super) const GINGER: u64 = 1; // in the open world, on `world`
pub(super) const VIM: u64 = 2; // inside the dungeon, on `instances`
pub(super) const TRIN: u64 = 3; // in the open world — the third member
pub(super) const DORMANT: u64 = 4; // has a character row on `world`, but is offline
/// A PLAYERBOT: a live `game_world_entity` on `world` whose character row never ran `player_login`,
/// so `game_character.online` stays false for its whole life. The module's own invite gate reads the
/// ENTITY ("a session-less playerbot's live entity counts"); the session flag would refuse it.
pub(super) const BOT: u64 = 5;
/// A second PLAYERBOT, resident on the OTHER shard (`instances`) — the same session-less shape as
/// [`BOT`], on the far side of the boundary. The invite is authoritative on realm-core, so a bot
/// standing on a different database than the inviting player is reachable in principle; this pins it.
const FAR_BOT: u64 = 6;

fn topology_after_vim_is_deleted() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    ShardCallLog,
) {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).expect("invite the survivor");
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).expect("the survivor accepts");

    let deleted_from_instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        mirror: std::sync::Mutex::new(instances.mirror.lock().unwrap().clone()),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone(), deleted_from_instances.clone()];
    *deleted_from_instances.peers.lock().unwrap() =
        vec![world.clone(), deleted_from_instances.clone()];
    (realm, world, deleted_from_instances, calls)
}

fn two_member_topology_after_vim_is_deleted() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
) {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    let deleted_from_instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls,
        realm: Some(realm.clone()),
        mirror: std::sync::Mutex::new(instances.mirror.lock().unwrap().clone()),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone(), deleted_from_instances.clone()];
    *deleted_from_instances.peers.lock().unwrap() =
        vec![world.clone(), deleted_from_instances.clone()];
    (realm, world, deleted_from_instances)
}

/// Party members in join order, every one in Subgroup 0.
pub(super) fn party_members(guids: &[u64]) -> Vec<party::GroupRosterMember> {
    guids
        .iter()
        .map(|&guid| party::GroupRosterMember {
            guid,
            slot: RaidSlot::default(),
        })
        .collect()
}

pub(super) fn character(guid: u64, name: &str) -> codec::CharacterView {
    codec::CharacterView {
        guid,
        name: name.into(),
        race: 1,
        class: 1,
        level: 10,
        ..Default::default()
    }
}

/// A live party topology: realm-core (the party authority) plus the two world shards Phase A runs,
/// wired the way the production gateway wires them — every shard's `realm_store()` is the realm
/// handle, and `world_stores()` is every connected world shard (including the asking one, exactly
/// as `Coordinator::all_shards` answers).
///
/// Ginger is resident on `world`, Vim on `instances` — the SPLIT that the Phase A tracer could not
/// represent and that made a cross-boundary invite fail live (2026-07-25).
pub(super) fn party_topology_with(
    mirror_error: Option<&str>,
    accept_error: Option<&str>,
) -> (
    std::sync::Arc<InMemoryStore>, // realm-core
    std::sync::Arc<InMemoryStore>, // the open-world shard
    std::sync::Arc<InMemoryStore>, // the instances shard
    ShardCallLog,
) {
    let calls: ShardCallLog = Default::default();
    let realm = std::sync::Arc::new(InMemoryStore {
        shard: "lyracore-realm".into(),
        calls: calls.clone(),
        is_realm: true,
        party_accept_error: accept_error.map(|e| e.to_string()),
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![
            character(GINGER, "Ginger"),
            character(TRIN, "Trin"),
            character(DORMANT, "Dormant"),
            character(BOT, "Botty"),
        ],
        // `live_guids` = `game_world_entity`; `offline_guids` = `game_character.online == false`.
        // The bot is in BOTH, which is the production shape a playerbot has (spawned straight into
        // the entity table, never logged in) and the case the two gates disagree about.
        live_guids: vec![GINGER, TRIN, BOT],
        entity_partitions: std::sync::Mutex::new(vec![(GINGER, 0, 0), (TRIN, 0, 0), (BOT, 0, 0)]),
        offline_guids: vec![DORMANT, BOT],
        ..Default::default()
    });
    let instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![character(VIM, "Vim"), character(FAR_BOT, "Farbotty")],
        live_guids: vec![VIM, FAR_BOT],
        entity_partitions: std::sync::Mutex::new(vec![(VIM, 0, 0), (FAR_BOT, 0, 0)]),
        offline_guids: vec![FAR_BOT],
        mirror_error: mirror_error.map(|e| e.to_string()),
        ..Default::default()
    });
    for shard in [&world, &instances] {
        *shard.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    }
    (realm, world, instances, calls)
}

pub(super) fn party_topology() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    ShardCallLog,
) {
    party_topology_with(None, None)
}

/// Form the split party the live run could not: Ginger (open world) invites Vim (inside Deadmines),
/// Vim accepts. `pub(super)` (`loot_tests` reuses it — a disband-capable op needs a real
/// party to disband).
pub(super) fn form_split_party(world: &InMemoryStore, instances: &InMemoryStore) {
    party::run(world, 7, GINGER, party::Op::Invite(VIM)).expect("the invite crosses");
    party::run(instances, 8, VIM, party::Op::Accept).expect("the accept lands");
}

fn command_intent(bot_guid: u64) -> party::PartyCommandIntent {
    party::PartyCommandIntent {
        id: 41,
        source_identity: spacetimedb_sdk::Identity::from_byte_array([7; 32]),
        issuer_guid: GINGER,
        issuer_sequence: 1,
        kind: 0,
        bot_guid,
        authority_member_guid: 0,
        exact_target_guid: 0,
        expires_micros: i64::MAX,
    }
}

#[test]
fn party_command_abort_configuration_names_only_the_committed_apply_boundary() {
    assert_eq!(
        party::party_command_abort_configuration(None).unwrap(),
        None
    );
    assert_eq!(
        party::party_command_abort_configuration(Some(party::PARTY_COMMAND_ABORT_STEP.to_string()))
            .unwrap(),
        Some(party::PARTY_COMMAND_ABORT_STEP.to_string())
    );
    let error =
        party::party_command_abort_configuration(Some("finish_source".to_string())).unwrap_err();
    assert!(error.to_string().contains("names no party command step"));
}

#[test]
fn a_companion_command_uses_realm_authority_and_the_bots_actual_world_shard() {
    let (realm, world, instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();
    let outcome =
        party::run_party_command_intent(world.as_ref(), &command_intent(FAR_BOT), 9001).unwrap();
    assert_eq!(outcome, party::CompanionCommandOutcome::Applied);
    assert_eq!(instances.admitted_party_commands.lock().unwrap().len(), 1);
    assert!(world.admitted_party_commands.lock().unwrap().is_empty());
    assert_eq!(world.party_command_finishes.lock().unwrap().len(), 1);
    assert!(realm.group_roster(GINGER).unwrap().is_some());
}

#[test]
fn changed_leadership_is_terminal_before_target_application() {
    let (realm, world, _instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).unwrap();
    let group_id = realm.party.lock().unwrap().group_of(GINGER).unwrap();
    realm
        .party
        .lock()
        .unwrap()
        .groups
        .iter_mut()
        .find(|(id, ..)| *id == group_id)
        .unwrap()
        .1 = BOT;
    let outcome = party::run_party_command_intent(world.as_ref(), &command_intent(BOT), 9).unwrap();
    assert_eq!(outcome, party::CompanionCommandOutcome::NotLeader);
    assert!(world.admitted_party_commands.lock().unwrap().is_empty());
    assert_eq!(world.party_command_finishes.lock().unwrap()[0].2, outcome);
}

#[test]
fn configured_realm_core_unavailability_fails_closed() {
    let store = InMemoryStore {
        characters: vec![character(BOT, "Bot")],
        entity_in_world: true,
        entity_partitions: std::sync::Mutex::new(vec![(GINGER, 0, 0), (BOT, 0, 0)]),
        party_command_realm_error: Some("Realm-core unavailable".into()),
        ..Default::default()
    };
    let error = party::run_party_command_intent(&store, &command_intent(BOT), 9).unwrap_err();
    assert!(error.to_string().contains("Realm-core unavailable"));
    assert!(store.party_command_finishes.lock().unwrap().is_empty());
}

#[test]
fn a_missing_bot_is_terminal_before_party_authority_is_consulted() {
    let (_realm, world, _instances, _) = party_topology();
    let outcome =
        party::run_party_command_intent(world.as_ref(), &command_intent(99_999), 9).unwrap();
    assert_eq!(outcome, party::CompanionCommandOutcome::MissingBot);
    assert!(world.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn a_nonmember_assist_target_is_refused_by_realm_authority() {
    let (_realm, world, _instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).unwrap();
    let mut intent = command_intent(BOT);
    intent.kind = 2;
    intent.authority_member_guid = TRIN;
    let outcome = party::run_party_command_intent(world.as_ref(), &intent, 9).unwrap();
    assert_eq!(outcome, party::CompanionCommandOutcome::NotMember);
    assert!(world.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn a_stale_target_mirror_cannot_grant_command_authority() {
    let (_realm, world, _instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).unwrap();
    world
        .mirror
        .lock()
        .unwrap()
        .iter_mut()
        .for_each(|roster| roster.members.retain(|member| member.guid != BOT));
    let outcome = party::run_party_command_intent(world.as_ref(), &command_intent(BOT), 9).unwrap();
    assert_eq!(outcome, party::CompanionCommandOutcome::StalePartyMirror);
    assert!(world.admitted_party_commands.lock().unwrap().is_empty());
}

/// A local roster of `extra + 2` members: Ginger leads, and the bot takes Companion Orders.
fn store_with_roster_of(extra: usize) -> InMemoryStore {
    let mut members = vec![GINGER, BOT];
    members.extend((0..extra).map(|offset| 80_000 + offset as u64));
    InMemoryStore {
        entity_in_world: true,
        characters: vec![character(GINGER, "Ginger"), character(BOT, "Bot")],
        entity_partitions: std::sync::Mutex::new(vec![(GINGER, 0, 0), (BOT, 0, 0)]),
        mirror: std::sync::Mutex::new(vec![party::GroupRoster {
            group_id: 7,
            leader_guid: GINGER,
            kind: GroupKind::Raid,
            members: party_members(&members),
            ..Default::default()
        }]),
        ..Default::default()
    }
}

/// Companion Orders keep the Party cap. A Raid above five is a gameplay answer, so the order
/// finishes at once with the documented terminal outcome instead of retrying until it expires.
#[test]
fn a_companion_order_in_a_raid_above_five_finishes_as_a_stale_party_mirror() {
    let store = store_with_roster_of(lyracore_shared::group::GROUP_MAX_MEMBERS);

    let outcome = party::run_party_command_intent(&store, &command_intent(BOT), 9).unwrap();

    assert_eq!(outcome, party::CompanionCommandOutcome::StalePartyMirror);
    assert_eq!(
        store.party_command_finishes.lock().unwrap().clone(),
        vec![(41, 9, party::CompanionCommandOutcome::StalePartyMirror)]
    );
    assert!(store.admitted_party_commands.lock().unwrap().is_empty());
}

/// A roster longer than any Raid is a damaged cache, not a gameplay answer: the intent stays
/// pending.
#[test]
fn a_roster_longer_than_a_raid_is_not_sent_for_command_authority() {
    let store = store_with_roster_of(lyracore_shared::group::RAID_MAX_MEMBERS);

    let error = party::run_party_command_intent(&store, &command_intent(BOT), 9).unwrap_err();

    assert!(error.to_string().contains("member limit"));
    assert!(store.party_command_finishes.lock().unwrap().is_empty());
    assert!(store.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn realm_admission_rejects_a_roster_changed_after_the_gateway_read() {
    let (realm, world, instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).unwrap();
    party::run(world.as_ref(), 8, GINGER, party::Op::Invite(FAR_BOT)).unwrap();
    let mut changed = realm.group_roster(GINGER).unwrap().unwrap().member_guids();
    changed.retain(|guid| *guid != BOT);
    *realm.party_command_authority_members.lock().unwrap() = Some(changed);

    let outcome =
        party::run_party_command_intent(world.as_ref(), &command_intent(FAR_BOT), 9).unwrap();

    assert_eq!(outcome, party::CompanionCommandOutcome::StalePartyMirror);
    assert!(instances.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn an_unavailable_receipt_shard_keeps_the_source_intent_pending() {
    let (_realm, world, instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();
    *instances.party_command_receipt_error.lock().unwrap() =
        Some("receipt read unavailable".to_string());

    let error =
        party::run_party_command_intent(world.as_ref(), &command_intent(FAR_BOT), 9).unwrap_err();

    assert!(error.to_string().contains("receipt read unavailable"));
    assert!(world.party_command_finishes.lock().unwrap().is_empty());
    assert!(instances.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn an_in_transit_holder_is_retried_without_a_missing_bot_result() {
    let (_realm, world, instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();
    instances
        .party_command_in_transit
        .lock()
        .unwrap()
        .push(FAR_BOT);

    let error =
        party::run_party_command_intent(world.as_ref(), &command_intent(FAR_BOT), 9).unwrap_err();

    assert!(error.to_string().contains("Transfer"));
    assert!(world.party_command_finishes.lock().unwrap().is_empty());
    assert!(instances.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn a_remote_party_member_cannot_direct_a_bot_in_another_partition() {
    let (_realm, world, instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();
    instances
        .entity_partitions
        .lock()
        .unwrap()
        .iter_mut()
        .find(|(guid, _, _)| *guid == FAR_BOT)
        .unwrap()
        .1 = 1;
    let outcome =
        party::run_party_command_intent(world.as_ref(), &command_intent(FAR_BOT), 9).unwrap();
    assert_eq!(outcome, party::CompanionCommandOutcome::WrongPartition);
    assert!(instances.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn a_bot_without_a_partition_cannot_reach_target_application() {
    let (_realm, world, _instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).unwrap();
    world
        .entity_partitions
        .lock()
        .unwrap()
        .retain(|(guid, _, _)| *guid != BOT);

    let outcome = party::run_party_command_intent(world.as_ref(), &command_intent(BOT), 9).unwrap();

    assert_eq!(outcome, party::CompanionCommandOutcome::WrongPartition);
    assert!(world.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn an_assist_member_without_a_partition_cannot_reach_target_application() {
    let (_realm, world, _instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).unwrap();
    party::run(world.as_ref(), 8, GINGER, party::Op::Invite(TRIN)).unwrap();
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).unwrap();
    world
        .entity_partitions
        .lock()
        .unwrap()
        .retain(|(guid, _, _)| *guid != TRIN);
    let mut intent = command_intent(BOT);
    intent.kind = 2;
    intent.authority_member_guid = TRIN;

    let outcome = party::run_party_command_intent(world.as_ref(), &intent, 10).unwrap();

    assert_eq!(outcome, party::CompanionCommandOutcome::WrongPartition);
    assert!(world.admitted_party_commands.lock().unwrap().is_empty());
}

#[test]
fn an_unsharded_gateway_uses_the_owning_local_party_authority() {
    let roster = party::GroupRoster {
        group_id: 7,
        leader_guid: GINGER,
        members: party_members(&[GINGER, BOT]),
        ..Default::default()
    };
    let store = InMemoryStore {
        characters: vec![character(BOT, "Bot")],
        entity_in_world: true,
        entity_partitions: std::sync::Mutex::new(vec![(GINGER, 0, 0), (BOT, 0, 0)]),
        mirror: std::sync::Mutex::new(vec![roster]),
        ..Default::default()
    };
    let outcome = party::run_party_command_intent(&store, &command_intent(BOT), 9).unwrap();
    assert_eq!(outcome, party::CompanionCommandOutcome::Applied);
    assert_eq!(store.admitted_party_commands.lock().unwrap().len(), 1);
}

#[test]
fn a_target_receipt_finishes_a_crashed_attempt_without_reapplying() {
    let (_realm, world, instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();
    let intent = command_intent(FAR_BOT);
    let authority = world
        .realm
        .as_ref()
        .unwrap()
        .group_roster(GINGER)
        .unwrap()
        .unwrap();
    let admitted = party::AdmittedCompanionCommand {
        source_identity: intent.source_identity,
        intent_id: intent.id,
        issuer_guid: intent.issuer_guid,
        issuer_sequence: intent.issuer_sequence,
        group_id: authority.group_id,
        leader_guid: authority.leader_guid,
        members: authority.member_guids(),
        kind: intent.kind,
        bot_guid: intent.bot_guid,
        authority_member_guid: 0,
        exact_target_guid: 0,
        expires_micros: i64::MAX - 1,
        receipt_retain_until_micros: i64::MAX,
    };
    assert_eq!(
        instances.apply_admitted_party_command(&admitted).unwrap(),
        party::CompanionCommandOutcome::Applied
    );
    let outcome = party::run_party_command_intent(world.as_ref(), &intent, 10).unwrap();
    assert_eq!(outcome, party::CompanionCommandOutcome::Applied);
    assert_eq!(instances.admitted_party_commands.lock().unwrap().len(), 1);
    assert_eq!(world.party_command_finishes.lock().unwrap().len(), 1);
}

#[test]
fn an_expired_intent_past_the_receipt_guarantee_reports_unknown() {
    let store = InMemoryStore::default();
    let mut intent = command_intent(BOT);
    intent.expires_micros = 0;

    let outcome = party::finish_expired_party_command_intent(&store, &intent, 55).unwrap();

    assert_eq!(outcome, party::CompanionCommandOutcome::OutcomeUnknown);
    assert_eq!(
        store.party_command_finishes.lock().unwrap()[0],
        (
            intent.id,
            55,
            party::CompanionCommandOutcome::OutcomeUnknown
        )
    );
}

#[test]
fn equal_numeric_intents_from_distinct_modules_have_distinct_receipts() {
    let (_realm, world, instances, _) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();
    let first = command_intent(FAR_BOT);
    let mut second = first.clone();
    second.source_identity = spacetimedb_sdk::Identity::from_byte_array([8; 32]);
    assert_eq!(
        party::run_party_command_intent(world.as_ref(), &first, 11).unwrap(),
        party::CompanionCommandOutcome::Applied
    );
    assert_eq!(
        party::run_party_command_intent(world.as_ref(), &second, 12).unwrap(),
        party::CompanionCommandOutcome::Applied
    );
    assert_eq!(instances.admitted_party_commands.lock().unwrap().len(), 2);
    assert_eq!(instances.party_command_receipts.lock().unwrap().len(), 2);
}

/// **AC: an invite works across a shard boundary.**
///
/// The live failure was not the party logic — it was the target RESOLUTION: `/invite` looks a typed
/// name up in `game_character` on ONE database, so a player inside Deadmines had no row on the open
/// world's database and the invite died as `BadPlayerName` before any rule ran.
#[test]
fn an_invite_resolves_a_target_standing_on_another_shard() {
    let (_realm, world, instances, _calls) = party_topology();
    // The pre-realm-core read, run against the shard the inviter is on: Vim is simply not there.
    assert_eq!(
        world.character_guid_by_name("Vim").unwrap(),
        None,
        "the fixture must reproduce the live shape — Vim's row lives on the instances shard"
    );
    // The realm-core read, from the same handle: the union finds them.
    assert_eq!(
        presence::resolve_by_name(world.as_ref(), "Vim").unwrap(),
        Some(VIM)
    );
    // …and it still resolves a name on the asking shard itself, from either side.
    assert_eq!(
        presence::resolve_by_name(instances.as_ref(), "Ginger").unwrap(),
        Some(GINGER)
    );
    assert_eq!(
        presence::resolve_by_name(world.as_ref(), "Nobody").unwrap(),
        None
    );
}

/// **AC: an invite works across a shard boundary** — the op itself, end to end.
#[test]
fn a_cross_shard_invite_and_accept_form_one_party_on_realm_core() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);

    let party_state = realm.party.lock().unwrap();
    let group_id = party_state.group_of(GINGER).expect("Ginger is in a party");
    assert_eq!(
        party_state.group_of(VIM),
        Some(group_id),
        "both members are in the SAME party"
    );
    assert_eq!(
        party_state.roster(group_id).unwrap().member_guids(),
        vec![GINGER, VIM],
        "the inviter leads and joins first, the acceptor second (join order)"
    );
    drop(party_state);
    // The op ran on REALM-CORE, not on either world shard — the whole point of the slice.
    let ops = calls.lock().unwrap().clone();
    assert!(
        ops.iter()
            .any(|(shard, call)| shard == "lyracore-realm" && call == "realm_group_op"),
        "no party op reached realm-core; calls were {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|(_, call)| call == "group_invite" || call == "group_accept"),
        "a multi-database gateway must not run the party op on a world shard's own tables — that is \
         exactly the shard-local behaviour realm-wide party routing removes. Calls were {ops:?}"
    );
}

/// **AC: a SPLIT party sees each other's frames**, without the `begin_transfer` snapshot.
///
/// Both members render an `SMSG_GROUP_LIST` naming the other with its ONLINE flag set — and the name
/// and the liveness each come from the shard that actually holds them, which no single database
/// could answer and which realm-core (no character rows) cannot answer either.
#[test]
fn a_split_party_renders_both_members_from_either_side_of_the_boundary() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    let roster = realm
        .group_roster(GINGER)
        .unwrap()
        .expect("realm-core holds the roster");

    let list = group_list(party::render_list(
        world.as_ref(),
        GINGER,
        &roster.list_payload(),
    ));
    assert_eq!(
        list.members.len(),
        1,
        "the viewer is excluded from their own member list"
    );
    assert_eq!(list.members[0].name, "Vim");
    assert_eq!(list.members[0].guid.guid(), VIM);
    assert!(
        list.members[0].is_online,
        "a member live on ANOTHER shard is online, not offline"
    );
    assert_eq!(list.leader.guid(), GINGER);

    let list = group_list(party::render_list(
        instances.as_ref(),
        VIM,
        &roster.list_payload(),
    ));
    assert_eq!(list.members.len(), 1);
    assert_eq!(
        list.members[0].name, "Ginger",
        "rendered from inside the instance, across the boundary"
    );
    assert!(list.members[0].is_online);
}

/// **AC: world shards read membership through the gateway rather than owning copies** — the
/// write-through mirror, and the fan-out that keeps every shard's copy honest.
#[test]
fn every_world_shard_mirrors_the_authoritative_roster_after_a_party_op() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    let authoritative = realm.group_roster(GINGER).unwrap().unwrap();

    for (name, shard) in [("world", &world), ("instances", &instances)] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![authoritative.clone()],
            "{name} does not mirror realm-core's roster. Every in-world membership read on that \
             shard — the kill-XP split, quest credit, loot rules, the party's dungeon binding — \
             resolves against this copy, so a shard that misses the push runs the party's gameplay \
             against a roster that does not exist"
        );
    }
}

/// The mirror is a WRITE-THROUGH cache, so a party that DISBANDS has to be forgotten everywhere —
/// otherwise each shard keeps a party whose members left, and their local reads keep splitting XP
/// with a group that no longer exists. (This is also the live artifact that motivated this slice: an
/// orphaned `game_group` row, leader Ginger, zero members, left on the instances shard.)
#[test]
fn a_disbanded_party_is_tombstoned_on_every_world_shard() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    assert!(
        !world.mirror.lock().unwrap().is_empty(),
        "precondition: the party is mirrored"
    );

    // Two members: one leaving disbands the party (vanilla — a party of one is no party).
    party::run(instances.as_ref(), 8, VIM, party::Op::Leave).expect("Vim leaves");

    assert!(
        realm.group_roster(GINGER).unwrap().is_none(),
        "realm-core disbanded the party"
    );
    for (name, shard) in [("world", &world), ("instances", &instances)] {
        assert!(
            shard.mirror.lock().unwrap().is_empty(),
            "{name} still mirrors a party that realm-core has disbanded — this is the orphaned \
             `game_group` row the live Phase A run left behind, reproduced"
        );
    }
}

/// The leaver's own shard is not the only one that has to hear about it: the mirror push has to
/// cover the group the actor was in BEFORE the op, or the members still in it keep a roster listing
/// someone who left. The actor's membership row is gone from the authority by then, which is why the
/// group id is captured up front.
#[test]
fn leaving_a_party_re_pushes_the_roster_of_the_group_the_leaver_left() {
    let (realm, world, instances, _calls) = party_topology();
    // Three members, so the party SURVIVES the leave and there is a remaining roster to compare.
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).expect("invite the third");
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).expect("the third accepts");

    party::run(instances.as_ref(), 8, VIM, party::Op::Leave).expect("Vim leaves");

    let remaining = realm
        .group_roster(GINGER)
        .unwrap()
        .expect("the party survives at 2 members");
    assert_eq!(remaining.member_guids(), vec![GINGER, TRIN]);
    for (name, shard) in [("world", &world), ("instances", &instances)] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![remaining.clone()],
            "{name} still lists the member who left — the mirror push must cover the group the \
             actor was in BEFORE the op, not only the one they are in after it"
        );
    }
}

#[test]
fn an_uncertain_character_read_preserves_realm_core_membership() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    let uncertain_instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        realm: Some(realm.clone()),
        character_read_error: Some("subscription unavailable".into()),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone(), uncertain_instances];

    let error = party::cleanup_deleted_character(world.as_ref(), VIM)
        .expect_err("unknown absence must stop cleanup");

    assert!(error.to_string().contains("subscription unavailable"));
    assert!(
        realm.group_roster(VIM).unwrap().is_some(),
        "an unreadable Shard cannot prove deletion, so Realm-core membership must stay unchanged"
    );
}

#[test]
fn a_deleted_member_leaves_realm_core_and_both_shards_receive_the_surviving_roster() {
    let (realm, world, instances, _calls) = topology_after_vim_is_deleted();

    assert_eq!(
        party::cleanup_deleted_character(world.as_ref(), VIM).unwrap(),
        party::DeletedCharacterPartyCleanup::Removed
    );

    let survivors = realm.group_roster(GINGER).unwrap().unwrap();
    assert_eq!(survivors.member_guids(), vec![GINGER, TRIN]);
    for shard in [&world, &instances] {
        assert_eq!(
            shard.mirror.lock().unwrap().as_slice(),
            std::slice::from_ref(&survivors)
        );
    }
}

#[test]
fn a_character_visible_on_the_destination_shard_keeps_party_membership() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    calls.lock().unwrap().clear();

    assert_eq!(
        party::cleanup_deleted_character(world.as_ref(), VIM).unwrap(),
        party::DeletedCharacterPartyCleanup::Preserved
    );
    assert!(realm.group_roster(VIM).unwrap().is_some());
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn an_unavailable_configured_shard_preserves_realm_core_membership() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    let incomplete = InMemoryStore {
        shard: "world".into(),
        calls,
        realm: Some(realm.clone()),
        world_shard_set_error: Some("instances has no healthy Coordinator subscription".into()),
        ..Default::default()
    };

    let error = party::cleanup_deleted_character(&incomplete, VIM)
        .expect_err("an incomplete Shard set cannot establish absence");

    assert!(error.to_string().contains("no healthy Coordinator"));
    assert!(realm.group_roster(VIM).unwrap().is_some());
}

#[test]
fn an_unavailable_realm_core_preserves_party_membership() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    let unavailable = InMemoryStore {
        shard: "world".into(),
        calls,
        realm: Some(realm.clone()),
        party_cleanup_realm_error: Some("Realm-core is not connected".into()),
        ..Default::default()
    };

    let error = party::cleanup_deleted_character(&unavailable, VIM)
        .expect_err("unavailable Realm-core cannot become an unsharded cleanup");

    assert!(error.to_string().contains("not connected"));
    assert!(realm.group_roster(VIM).unwrap().is_some());
}

#[test]
fn repeated_deleted_character_cleanup_is_harmless() {
    let (realm, world, _instances, _calls) = topology_after_vim_is_deleted();
    assert_eq!(
        party::cleanup_deleted_character(world.as_ref(), VIM).unwrap(),
        party::DeletedCharacterPartyCleanup::Removed
    );

    assert_eq!(
        party::cleanup_deleted_character(world.as_ref(), VIM).unwrap(),
        party::DeletedCharacterPartyCleanup::AlreadyClean
    );
    assert_eq!(
        realm.group_roster(GINGER).unwrap().unwrap().member_guids(),
        vec![GINGER, TRIN]
    );
}

/// The deleted Character's LEAVE names its cause, so the party authority also drops the Target
/// Icons on it: the World Shard's delete sweep cannot reach Realm-core's icon rows.
#[test]
fn a_deleted_characters_leave_tells_the_authority_it_was_deleted() {
    let (realm, world, _instances, _calls) = topology_after_vim_is_deleted();

    party::cleanup_deleted_character(world.as_ref(), VIM).unwrap();

    assert_eq!(
        realm.party.lock().unwrap().ops.last().copied(),
        Some((realm_op::LEAVE, VIM, 0, 1, 0, 0)),
        "LEAVE with arg_a = CHARACTER_DELETED"
    );
    assert_eq!(lyracore_shared::group::leave_cause::CHARACTER_DELETED, 1);
}

#[test]
fn deleted_character_cleanup_retries_a_transient_realm_core_leave_failure() {
    let (realm, world, _instances, _calls) = topology_after_vim_is_deleted();
    realm
        .party_leave_failures
        .store(2, std::sync::atomic::Ordering::SeqCst);

    assert_eq!(
        party::cleanup_deleted_character(world.as_ref(), VIM).unwrap(),
        party::DeletedCharacterPartyCleanup::Removed
    );
    assert_eq!(realm.group_roster(VIM).unwrap(), None);
    assert_eq!(
        realm
            .party
            .lock()
            .unwrap()
            .ops
            .iter()
            .filter(|(op, actor, ..)| *op == realm_op::LEAVE && *actor == VIM)
            .count(),
        3
    );
}

#[test]
fn a_lost_realm_core_leave_reply_still_refreshes_the_surviving_party() {
    let (realm, world, instances, _calls) = topology_after_vim_is_deleted();
    realm
        .party_leave_commit_then_error
        .store(true, std::sync::atomic::Ordering::SeqCst);

    assert_eq!(
        party::cleanup_deleted_character(world.as_ref(), VIM).unwrap(),
        party::DeletedCharacterPartyCleanup::Removed
    );

    let survivors = realm.group_roster(GINGER).unwrap().unwrap();
    assert_eq!(survivors.member_guids(), vec![GINGER, TRIN]);
    for shard in [&world, &instances] {
        assert_eq!(
            shard.mirror.lock().unwrap().as_slice(),
            std::slice::from_ref(&survivors)
        );
    }
}

#[test]
fn deleted_character_cleanup_retries_a_transient_mirror_failure() {
    let (realm, world, instances, calls) = topology_after_vim_is_deleted();
    instances
        .mirror_failures
        .store(2, std::sync::atomic::Ordering::SeqCst);
    calls.lock().unwrap().clear();

    assert_eq!(
        party::cleanup_deleted_character(world.as_ref(), VIM).unwrap(),
        party::DeletedCharacterPartyCleanup::Removed
    );

    let survivors = realm.group_roster(GINGER).unwrap().unwrap();
    assert_eq!(
        instances.mirror.lock().unwrap().as_slice(),
        std::slice::from_ref(&survivors)
    );
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(shard, call)| shard == "instances" && call == "sync_group_mirror")
            .count(),
        3
    );
}

#[test]
fn reconciliation_repairs_a_mirror_left_stale_after_membership_cleanup() {
    let (realm, world, instances) = two_member_topology_after_vim_is_deleted();
    instances
        .mirror_failures
        .store(7, std::sync::atomic::Ordering::SeqCst);

    let error = party::cleanup_deleted_character(world.as_ref(), VIM)
        .expect_err("three failed mirror writes must keep cleanup pending");
    assert!(error.to_string().contains("3 attempts"));
    assert_eq!(realm.group_roster(VIM).unwrap(), None);
    assert_eq!(realm.party_group_ids().unwrap(), Vec::<u64>::new());
    assert!(instances
        .mirror
        .lock()
        .unwrap()
        .iter()
        .any(|roster| roster.has_member(VIM)));

    party::reconcile_deleted_character_parties(world.as_ref())
        .expect_err("the first reconciliation exhausts its bounded mirror retries");
    assert!(!instances.mirror.lock().unwrap().is_empty());

    party::reconcile_deleted_character_parties(world.as_ref()).unwrap();

    assert!(instances.mirror.lock().unwrap().is_empty());
}

#[test]
fn deleted_character_cleanup_stops_after_three_realm_core_leave_failures() {
    let (realm, world, _instances, _calls) = topology_after_vim_is_deleted();
    realm
        .party_leave_failures
        .store(4, std::sync::atomic::Ordering::SeqCst);

    let error = party::cleanup_deleted_character(world.as_ref(), VIM)
        .expect_err("cleanup must report a persistent Realm-core failure");

    assert!(error.to_string().contains("connection interrupted"));
    assert!(realm.group_roster(VIM).unwrap().is_some());
    assert_eq!(
        realm
            .party
            .lock()
            .unwrap()
            .ops
            .iter()
            .filter(|(op, actor, ..)| *op == realm_op::LEAVE && *actor == VIM)
            .count(),
        3
    );
}

#[test]
fn roster_confirmation_failure_preserves_the_realm_core_leave_error() {
    let (realm, world, _instances, _calls) = topology_after_vim_is_deleted();
    realm
        .party_leave_failures
        .store(3, std::sync::atomic::Ordering::SeqCst);
    *realm.group_roster_error_on_read.lock().unwrap() =
        Some((2, "Realm-core roster confirmation unavailable".into()));

    let error = party::cleanup_deleted_character(world.as_ref(), VIM)
        .expect_err("cleanup must retain the failed LEAVE as its primary error");

    assert!(error.to_string().contains("LEAVE connection interrupted"));
    assert!(!error
        .to_string()
        .contains("roster confirmation unavailable"));
    assert!(realm.group_roster(VIM).unwrap().is_some());
}

#[test]
fn reconciliation_continues_when_one_shard_cannot_enumerate_mirrored_groups() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    world.mirror.lock().unwrap().clear();
    *instances.party_group_ids_error.lock().unwrap() =
        Some("instances party subscription unavailable".into());

    let error = party::reconcile_deleted_character_parties(world.as_ref())
        .expect_err("the unavailable Shard read must keep reconciliation pending");

    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains("1 deleted Character party reconciliation attempt"));
    assert!(diagnostic.contains("instances party subscription unavailable"));
    let authoritative = realm.group_roster(GINGER).unwrap().unwrap();
    assert_eq!(
        world.mirror.lock().unwrap().as_slice(),
        std::slice::from_ref(&authoritative),
        "the known realm-core group must still repair healthy Shard mirrors"
    );
}

#[test]
fn unsharded_deleted_character_cleanup_stays_on_the_module_sweep() {
    let store = InMemoryStore {
        shard: "world".into(),
        ..Default::default()
    };

    assert_eq!(
        party::cleanup_deleted_character(&store, VIM).unwrap(),
        party::DeletedCharacterPartyCleanup::AlreadyClean
    );
    assert!(store.calls.lock().unwrap().is_empty());
}

#[test]
fn reconnect_reconciliation_removes_a_member_whose_delete_event_was_missed() {
    let (realm, world, instances, _calls) = topology_after_vim_is_deleted();

    party::reconcile_deleted_character_parties(world.as_ref()).unwrap();

    let survivors = realm.group_roster(GINGER).unwrap().unwrap();
    assert_eq!(survivors.member_guids(), vec![GINGER, TRIN]);
    assert_eq!(
        instances.mirror.lock().unwrap().as_slice(),
        std::slice::from_ref(&survivors)
    );
}

#[test]
fn deleted_character_cleanup_flushes_pending_loot_before_leaving_realm_core() {
    let (_realm, world, instances, calls) = topology_after_vim_is_deleted();
    *instances.pending_rolls.lock().unwrap() = vec![loot::PendingLootRoll {
        promotion_source: spacetimedb_sdk::Identity::from_byte_array([7; 32]),
        roll_id: 40,
        corpse_guid: 90,
        slot: 1,
        item_entry: 100,
        random_property_id: 0,
        deadline_micros: 500,
        recipients: vec![GINGER, VIM, TRIN],
    }];
    calls.lock().unwrap().clear();

    party::cleanup_deleted_character(world.as_ref(), VIM).unwrap();

    let names: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, call)| call.clone())
        .collect();
    let promotion = names
        .iter()
        .position(|name| name == "realm_loot_op")
        .unwrap();
    let leave = names
        .iter()
        .position(|name| name == "realm_group_op")
        .unwrap();
    assert!(
        promotion < leave,
        "pending loot from the deleted Character's former Shard must reach Realm-core before a \
         disband-capable leave"
    );
}

#[test]
fn deleted_character_leave_returns_with_the_committed_roster_visible() {
    let src = include_str!("../stdb/reducers.rs");
    let body = crate::test_scan::code_of(src, "pub fn deleted_character_party_leave(");
    assert!(
        body.contains("let coordinator = self.0.visibility_pipe()")
            && body.contains("coordinator.conn.reducers"),
        "cleanup reads Realm-core immediately after LEAVE, so the Durable Request must return a \
         Coordinator visibility receipt. Body was:\n{body}"
    );

    let ordinary = crate::test_scan::code_of(src, "pub fn realm_group_op(");
    assert!(
        ordinary.contains("self.0.call_pipe().conn.reducers"),
        "an op that pushes no Group mirror needs no visibility receipt, so it keeps the \
         independent call pipe. Body was:\n{ordinary}"
    );

    let visible = crate::test_scan::code_of(src, "pub fn realm_group_op_visible(");
    assert!(
        visible.contains("let coordinator = self.0.visibility_pipe()")
            && visible.contains("coordinator.conn.reducers"),
        "a World Session's party op pushes the mirror from a read right after it, so it must \
         return a Coordinator visibility receipt. Body was:\n{visible}"
    );
}

/// The production pipes cannot run in a Gateway test, so the choice of op in `party::run` is pinned
/// here and the lagging Fake above proves what it buys.
#[test]
fn a_world_session_runs_its_realm_party_op_on_the_visibility_pipe() {
    let run = crate::test_scan::code_of(include_str!("party.rs"), "pub(crate) fn run<");
    assert!(run.contains("run_on_authority_visible(realm.as_ref(), self_guid, op)"));
    let visible =
        crate::test_scan::code_of(include_str!("party.rs"), "fn run_on_authority_visible<");
    assert!(visible.contains("authority.realm_group_op_visible("));
}

/// **The invariant this batch has broken five times: unset config changes NOTHING.**
///
/// A single-database gateway has no realm-core to route to, so every op takes the pre-realm-core
/// path — the player's own connection, the player-facing reducer, that database's own tables — and neither
/// the realm plane nor the mirror is touched at all.
#[test]
fn an_unsharded_gateway_runs_every_party_op_on_the_players_own_shard() {
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        characters: vec![character(VIM, "Vim")],
        live_guids: vec![VIM],
        ..Default::default() // no `realm`, no `peers` — the unconfigured gateway
    });
    assert!(
        store.realm_store().is_none(),
        "an unsharded store must not name a realm database"
    );

    for op in [
        party::Op::Invite(VIM),
        party::Op::Accept,
        party::Op::Decline,
        party::Op::Leave,
        party::Op::Uninvite(VIM),
        party::Op::LootMethod {
            setting: 2,
            master: VIM,
            threshold: 3,
        },
    ] {
        party::run(store.as_ref(), 7, GINGER, op).expect("the legacy path answers");
    }

    let log = calls.lock().unwrap().clone();
    let ran: Vec<&str> = log.iter().map(|(_, c)| c.as_str()).collect();
    assert_eq!(
        ran,
        vec![
            "group_invite",
            "group_accept",
            "group_decline",
            "group_leave",
            "group_uninvite",
            "group_loot_method",
        ],
        "an unsharded gateway must call exactly the six player-facing reducers it called before \
         realm-core, \
         in the order the client asked for them"
    );
    assert!(
        log.iter().all(|(shard, _)| shard == "world"),
        "every call must land on the player's own database"
    );
    assert!(
        !log.iter()
            .any(|(_, c)| c == "realm_group_op" || c == "sync_group_mirror"),
        "the realm plane and the mirror must be untouched on a single-database gateway"
    );
    assert_eq!(
        *store.group_invites.lock().unwrap(),
        vec![VIM],
        "with the same arguments as before"
    );
    assert_eq!(*store.group_loot_methods.lock().unwrap(), vec![(2, VIM, 3)]);
}

/// World ENTRY is what carries a party across the boundary now that the character-transfer blob's
/// party mirror is gone: the arriving shard gets realm-core's roster pushed onto it, and the player
/// gets their frame back.
#[test]
fn world_entry_pushes_the_authoritative_roster_onto_the_shard_the_player_arrives_on() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    // A fresh instances shard: the character arrived through a transfer, so it has NO party rows —
    // exactly the state `import_character_blob` leaves now that membership does not ride the blob.
    instances.mirror.lock().unwrap().clear();

    let (tx, rx) = crate::world::SessionTx::with_depth(0);
    party::on_world_entry(&tx, instances.as_ref(), VIM).expect("world entry syncs the party");

    let authoritative = realm.group_roster(VIM).unwrap().unwrap();
    assert_eq!(
        instances.mirror.lock().unwrap().clone(),
        vec![authoritative],
        "the arriving shard must be given the party the character is ACTUALLY in — the blob no \
         longer carries membership, so this push is the only thing that makes it whole"
    );
    let list = group_list(rx.try_recv().expect("a GROUP_LIST is sent"));
    assert_eq!(list.members.len(), 1);
    assert_eq!(list.members[0].name, "Ginger");
}

/// A character in no party must not have a roster pushed for them — and an unsharded gateway must
/// not read anything at world entry at all.
#[test]
fn world_entry_is_a_no_op_for_an_ungrouped_character_and_on_a_single_database() {
    let (_realm, world, _instances, calls) = party_topology();
    let (tx, rx) = crate::world::SessionTx::with_depth(0);
    party::on_world_entry(&tx, world.as_ref(), GINGER).expect("ungrouped entry is fine");
    assert!(rx.try_recv().is_err(), "no party, no party frame");
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|(_, c)| c == "sync_group_mirror"),
        "nothing to mirror for a character in no party"
    );

    let solo = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        ..Default::default()
    });
    let (tx2, rx2) = crate::world::SessionTx::with_depth(0);
    party::on_world_entry(&tx2, solo.as_ref(), GINGER).expect("unsharded entry is fine");
    assert!(
        rx2.try_recv().is_err(),
        "a single-database login sends no extra packet"
    );
}

/// **The mirror's self-healing claim, in the direction it does not hold by construction.**
///
/// This module promises that a shard which misses a push
/// "re-syncs on the next op or world entry". For a member who is still IN the party that is true —
/// the arriving roster is pushed. For a member the authority DROPPED while that shard was
/// unreachable it was not: with no roster to push, world entry returned before touching anything and
/// the stale membership row survived every arrival. The shard then ran that character's gameplay —
/// kill-XP split, quest credit, loot method and round-robin, `/p` chat, the dungeon binding — against
/// a party they are not in, and no future op could ever repair it (the ops of a party they left never
/// name them again).
#[test]
fn world_entry_clears_a_mirror_that_still_lists_a_character_the_authority_dropped() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).expect("invite the third");
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).expect("the third accepts");
    let group_id = realm.group_roster(VIM).unwrap().unwrap().group_id;

    // Vim leaves while the instances shard is unreachable: the authority commits, that shard's push
    // is dropped on the floor (the documented best-effort ceiling), so its mirror still lists Vim.
    {
        let mut p = realm.party.lock().unwrap();
        p.members.retain(|(_, g)| *g != VIM);
    }
    assert!(
        instances.group_roster(VIM).unwrap().is_some(),
        "precondition: the instances mirror still has Vim in the party they already left"
    );

    // Vim comes back to that shard. This is the "or world entry" half of the self-healing promise.
    let (tx, _rx) = crate::world::SessionTx::with_depth(0);
    party::on_world_entry(&tx, instances.as_ref(), VIM).expect("world entry is fine for a loner");

    assert_eq!(
        instances.group_roster(VIM).unwrap(),
        None,
        "the instances shard still has Vim in a party realm-core dropped them from. Nothing else \
         will ever fix it — that party's future ops never name Vim again — so every kill, loot roll \
         and `/p` line Vim makes on this shard runs against a membership that does not exist"
    );
    assert_eq!(
        instances
            .group_roster_by_id(group_id)
            .unwrap()
            .map(|r| r.member_guids()),
        Some(vec![GINGER, TRIN]),
        "and the members who are STILL in that party must survive the repair — clearing the group \
         wholesale would be the opposite defect"
    );
}

/// The two gates realm-core cannot run for itself. Both are refused BEFORE the authority is touched,
/// and both answer the module's own Refusals so `social::party_result_for` classifies them
/// identically on either plane.
#[test]
fn an_invite_to_a_missing_or_offline_target_never_reaches_realm_core() {
    let (realm, world, _instances, _calls) = party_topology();

    assert_eq!(
        party::run(world.as_ref(), 7, GINGER, party::Op::Invite(DORMANT)).unwrap(),
        PartyOutcome::Refused(GroupRefusal::TargetOffline)
    );
    assert_eq!(
        party::run(world.as_ref(), 7, GINGER, party::Op::Invite(999)).unwrap(),
        PartyOutcome::Refused(GroupRefusal::NoSuchPlayer)
    );

    assert!(
        realm.party.lock().unwrap().ops.is_empty(),
        "a refused invite must not reach the authority — the gate runs in the gateway precisely \
         because realm-core has neither characters nor live entities to run it against"
    );
}

/// An Orc, for the faction Gate. Race 2 is Horde; every other fixture Character is a Human.
const GRUNT: u64 = 8;
/// An Orc with a character row but no live entity.
const SLEEPING_GRUNT: u64 = 9;

fn orc(guid: u64, name: &str) -> codec::CharacterView {
    codec::CharacterView {
        race: 2,
        ..character(guid, name)
    }
}

/// Vanilla's default refuses a party across factions (cm:GroupHandler.cpp:80,
/// `AllowTwoSide.Interaction.Group = 0`), after the target lookup and before anything else. The
/// Gateway reads both races realm-wide, so an Orc standing on another Shard is refused before the
/// authority is touched.
#[test]
fn a_cross_faction_invite_is_refused_before_realm_core() {
    let calls: ShardCallLog = Default::default();
    let realm = std::sync::Arc::new(InMemoryStore {
        shard: "lyracore-realm".into(),
        calls: calls.clone(),
        is_realm: true,
        ..Default::default()
    });
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![character(GINGER, "Ginger")],
        live_guids: vec![GINGER],
        ..Default::default()
    });
    let horde = std::sync::Arc::new(InMemoryStore {
        shard: "horde".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![orc(GRUNT, "Grunt"), orc(SLEEPING_GRUNT, "Sleeper")],
        live_guids: vec![GRUNT],
        ..Default::default()
    });
    for shard in [&world, &horde] {
        *shard.peers.lock().unwrap() = vec![world.clone(), horde.clone()];
    }

    assert_eq!(
        party::run(world.as_ref(), 7, GINGER, party::Op::Invite(GRUNT)).unwrap(),
        PartyOutcome::Refused(GroupRefusal::WrongFaction)
    );
    assert_eq!(
        party::run(world.as_ref(), 7, GINGER, party::Op::Invite(SLEEPING_GRUNT)).unwrap(),
        PartyOutcome::Refused(GroupRefusal::TargetOffline),
        "an offline target is refused as offline first, as vanilla's online lookup does"
    );
    assert!(
        realm.party.lock().unwrap().ops.is_empty(),
        "a cross-faction invite must not reach the authority"
    );
}

/// The unsharded plane refuses the same invite before the player-facing reducer runs.
#[test]
fn an_unsharded_gateway_refuses_a_cross_faction_invite_too() {
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        characters: vec![character(GINGER, "Ginger"), orc(GRUNT, "Grunt")],
        live_guids: vec![GINGER, GRUNT],
        ..Default::default()
    });
    assert_eq!(
        party::run(store.as_ref(), 7, GINGER, party::Op::Invite(GRUNT)).unwrap(),
        PartyOutcome::Refused(GroupRefusal::WrongFaction)
    );
    assert!(
        store.group_invites.lock().unwrap().is_empty(),
        "the player-facing invite reducer must not run"
    );
}

/// The moved ONLINE gate has to be the module's gate, not a lookalike.
///
/// The module refuses an invite when the target has no `game_world_entity` row, and says so in its
/// own comment: *"a session-less playerbot's live entity counts"*. `game_character.online` is a
/// different fact — it is set by `player_login` and cleared by logout, and a playerbot runs neither
/// (its spawn reducer inserts the entity directly), so every bot in the tree has a live entity and
/// `online == false` for its whole life.
///
/// Gating on the session flag therefore refuses `/invite <bot>` on a MULTI-DATABASE gateway while
/// the single-database plane still accepts it — the moved gate answering differently than the module
/// did, which is a behaviour change wearing a refactor's clothes. The playerbot real-player-simulation
/// runs are driven by exactly this opcode.
#[test]
fn a_playerbot_is_invitable_because_the_online_gate_reads_the_entity_not_the_session_flag() {
    let (realm, world, _instances, _calls) = party_topology();
    assert!(
        !world.character_presence(BOT).unwrap().unwrap().0,
        "fixture: a playerbot's `game_character.online` is false — it never runs `player_login`"
    );
    assert!(
        world.entity_in_world(BOT),
        "fixture: …but its live entity is right there"
    );

    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT))
        .expect("the invite gate must read the LIVE ENTITY, exactly as the module's own gate does");
    assert_eq!(
        realm.party.lock().unwrap().ops.first().copied(),
        Some((
            lyracore_shared::group::realm_op::INVITE,
            GINGER,
            BOT,
            0,
            0,
            0
        )),
        "the invite must reach the authority"
    );
}

// ===========================================================================================
//  Somebody has to ANSWER a bot's invite
// ===========================================================================================

/// **AC: a bot accepts a pending group invite from a player.**
///
/// The invite landed correctly and nothing ever answered it (observed live 2026-07-26). On a
/// single-database gateway the module answers in-transaction — `invite_core` fires `on_group_invite`
/// and `brain.rs`'s `playerbots_auto_accept` accepts through it — but moving the invite onto
/// realm-core, where `pkg_playerbots_bot` is empty, makes the hook a no-op there, and the dialog hung
/// until the 2-minute GC. A human therefore could not group with a bot at all, which is the single
/// most useful manual test the bots exist to support.
///
/// Pinned here as BEHAVIOUR, not as a source scan: after one `/invite Botty` and nothing else, the
/// authority holds a two-member party — and the acting guid on the ACCEPT is the BOT'S OWN.
#[test]
fn a_players_invite_to_a_session_less_bot_is_answered_by_the_bot_itself() {
    use lyracore_shared::group::realm_op;
    let (realm, world, _instances, _calls) = party_topology();

    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).expect("the invite lands");

    let party_state = realm.party.lock().unwrap();
    let group_id = party_state
        .group_of(GINGER)
        .expect("the invite formed Ginger's party");
    assert_eq!(
        party_state.roster(group_id).unwrap().member_guids(),
        vec![GINGER, BOT],
        "the bot must be IN the party after a single invite — nobody else is going to answer for it"
    );
    assert!(
        party_state.invites.is_empty(),
        "the pending invite must be CONSUMED; a leftover row is the hung dialog this fixes"
    );
    // IMPERSONATION (the hazard this batch already hit once): `realm_group_op` takes the actor as an
    // ARGUMENT, so the gateway could trivially accept as somebody else. The bot acts as ITSELF.
    assert_eq!(
        party_state.ops.clone(),
        vec![
            (realm_op::INVITE, GINGER, BOT, 0, 0, 0),
            (realm_op::ACCEPT, BOT, 0, 0, 0, 0)
        ],
        "the accept must run on realm-core with the BOT as the actor — never the inviter, and never 0"
    );
}

/// **A REAL PLAYER, ANSWERED FOR — the impersonation this predicate has to refuse.**
///
/// Found by adversarial review and reproduced here before it was fixed. The two halves
/// of the predicate used to read DIFFERENT databases: the entity check UNIONED every shard, while the
/// session flag came from [`presence`], which is first-hit-wins over `game_character`. So a guid with
/// a stale row on the ASKING shard and its live, logged-in self on another one had its session flag
/// resolved off the stale copy, and the gateway accepted a group invite on a real player's behalf.
///
/// Not hypothetical, and not a race: `init` seeds character guid 1 ("Tester") into every database it
/// is published to, so on the live three-database stack a player logged in as guid 1 on
/// `lyracore` has an `online = false` row sitting on `lyracore-instances` — and an inviter
/// standing inside a dungeon asks that shard first. The fix is to read the flag on the shard that
/// HOLDS the live entity; the fixture below is exactly that shape.
#[test]
fn a_stale_character_row_on_another_shard_cannot_make_a_logged_in_player_look_session_less() {
    /// The seeded `init` character: a row on EVERY database, `online = false` in the seed.
    const SEEDED: u64 = 1;
    let (realm, world, instances, _calls) = party_topology();
    // `instances` carries the stale seed copy (offline)…
    let mut i_chars = instances.characters.clone();
    i_chars.push(character(SEEDED, "Tester"));
    let far = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        realm: Some(realm.clone()),
        characters: i_chars,
        live_guids: vec![VIM, FAR_BOT],
        offline_guids: vec![FAR_BOT, SEEDED],
        ..Default::default()
    });
    // …while `world` holds the real, LOGGED-IN character and its live entity.
    let mut w_chars = world.characters.clone();
    w_chars.push(character(SEEDED, "Tester"));
    let home = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        realm: Some(realm.clone()),
        characters: w_chars,
        live_guids: vec![GINGER, TRIN, BOT, SEEDED],
        offline_guids: vec![DORMANT, BOT],
        ..Default::default()
    });
    for shard in [&home, &far] {
        *shard.peers.lock().unwrap() = vec![home.clone(), far.clone()];
    }
    // …and end to end: an inviter on the far shard must leave that player's dialog alone.
    party::run(far.as_ref(), 9, VIM, party::Op::Invite(SEEDED)).expect("the invite itself is fine");
    let state = realm.party.lock().unwrap();
    assert_eq!(
        state.ops.clone(),
        vec![(
            lyracore_shared::group::realm_op::INVITE,
            VIM,
            SEEDED,
            0,
            0,
            0
        )],
        "no ACCEPT may be forged for a character whose own client is logged in and can answer"
    );
    assert_eq!(
        state.group_of(SEEDED),
        None,
        "and they are NOT in a party they never joined"
    );
}

/// **AC: the bot's membership reaches the shard it stands on.**
///
/// The bot's own in-world behaviour — follow-the-leader (the playerbot simulation's slice 2), the
/// kill-XP split, `/p` — all read the SHARD's mirror, not realm-core. The answer therefore has to
/// happen before the mirror push of the op that caused it, or the bot is a member the shard does not
/// know about until the party's next op (and a bot party has no next op — the human does everything).
#[test]
fn the_bots_new_membership_is_mirrored_onto_its_own_shard_by_the_same_op() {
    let (realm, world, instances, _calls) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).expect("the invite lands");
    let group_id = realm
        .party
        .lock()
        .unwrap()
        .group_of(BOT)
        .expect("the bot joined");

    assert_eq!(
        world
            .group_roster_by_id(group_id)
            .unwrap()
            .map(|r| r.member_guids()),
        Some(vec![GINGER, BOT]),
        "the bot's own shard must already hold the roster — it is what `group_leader_entity` reads"
    );
    assert_eq!(
        world
            .group_roster_by_id(group_id)
            .unwrap()
            .map(|r| r.leader_guid),
        Some(GINGER),
        "and the leader in that mirror is the PLAYER: the follow-the-leader pass resolves its anchor \
         from `game_group.leader_guid` and never asks whether the leader is a bot"
    );
    assert_eq!(
        instances
            .group_roster_by_id(group_id)
            .unwrap()
            .map(|r| r.member_guids()),
        Some(vec![GINGER, BOT]),
        "every connected shard is mirrored, as for any other op"
    );
}

/// **AC: a bot on ANOTHER shard than the inviting player is reachable.**
///
/// Free, now that the invite is authoritative on realm-core: the answer is a realm-core op too, so the
/// boundary never enters into it. Asserted once so a future change that resolves the bot against the
/// inviter's own database is caught.
#[test]
fn a_bot_standing_on_another_shard_answers_the_invite_too() {
    let (realm, world, _instances, _calls) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT))
        .expect("a cross-shard bot invite lands");
    let state = realm.party.lock().unwrap();
    let group_id = state.group_of(GINGER).expect("Ginger's party formed");
    assert_eq!(
        state.roster(group_id).unwrap().member_guids(),
        vec![GINGER, FAR_BOT]
    );
}

/// **AC: a real player's invite is NOT answered for them.**
///
/// The counter-case, and the one that must never regress: Trin has a client, so Trin's dialog is
/// Trin's to answer. Auto-accepting for a human would be a party the player never agreed to join.
#[test]
fn a_real_players_invite_dialog_is_left_for_their_own_client_to_answer() {
    use lyracore_shared::group::realm_op;
    let (realm, world, _instances, _calls) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).expect("the invite lands");

    let state = realm.party.lock().unwrap();
    assert_eq!(
        state.ops.clone(),
        vec![(realm_op::INVITE, GINGER, TRIN, 0, 0, 0)],
        "the gateway must not answer for a character that has a session"
    );
    assert_eq!(
        state.group_of(TRIN),
        None,
        "Trin is not in a party until Trin says so"
    );
    assert_eq!(
        state.invites.clone(),
        vec![(TRIN, GINGER)],
        "the dialog is still pending, as it must be"
    );
}

/// **AC: a refusal DECLINES explicitly rather than being ignored.**
///
/// An ignored refusal leaves the invite row standing (every accept gate in the module returns `Err`,
/// which rolls its transaction back) — and a hanging dialog is indistinguishable, from the player's
/// side, from the bug this whole issue is about. So the bot says no out loud: the invite is consumed
/// and the inviter gets `SMSG_GROUP_DECLINE` off the DECLINE event.
#[test]
fn a_bot_that_cannot_join_declines_out_loud_instead_of_leaving_the_dialog_hanging() {
    use lyracore_shared::group::{event_kind, realm_op};
    let (realm, world, _instances, _calls) =
        party_topology_with(None, Some("the party is already full"));

    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT))
        .expect("a bot that cannot join must not fail the PLAYER's invite — it already committed");

    let state = realm.party.lock().unwrap();
    assert_eq!(
        state.ops.clone(),
        vec![
            (realm_op::INVITE, GINGER, BOT, 0, 0, 0),
            (realm_op::ACCEPT, BOT, 0, 0, 0, 0),
            // …and the decline is the bot's own too, not the inviter's.
            (realm_op::DECLINE, BOT, 0, 0, 0, 0),
        ],
        "a refused accept must be followed by an explicit decline"
    );
    assert!(
        state.events.contains(&(GINGER, event_kind::DECLINE)),
        "the inviter has to be TOLD; events were {:?}",
        state.events
    );
    assert!(
        state.invites.is_empty(),
        "and the pending invite is consumed either way"
    );
}

/// The answer is not a new tick and not a poll: it runs INSIDE the invite op, so a bot is in the party
/// by the time `/invite` returns. Pinned because the alternative shape the issue suggested — a poll on
/// the playerbots goal tick — would have read the SHARD's `game_group_invite`, which a sharded
/// deployment never writes (the invite lives on realm-core), and would have been a no-op that a source
/// scan could not tell from a fix.
#[test]
fn the_bot_answers_within_the_invite_op_itself_with_no_second_call() {
    let (realm, world, _instances, calls) = party_topology();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(BOT)).expect("the invite lands");
    assert!(
        realm.party.lock().unwrap().group_of(BOT).is_some(),
        "joined already"
    );
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|(shard, call)| shard != "lyracore-realm"
            && (call == "group_accept" || call == "group_invite")),
        "the answer must never run on a world shard's own party tables — that would write membership \
         the authority does not have. Calls were {:?}",
        calls.lock().unwrap()
    );
}

/// `realm_group_op` packs seven ops into six argument slots, and the packing is a WIRE contract with
/// the module (`lyracore_shared::group::realm_op`). A slot swap is silent — a loot-method change would
/// arrive as a kick of the master looter — so every op's packing is pinned as it is SENT.
#[test]
fn every_party_op_reaches_realm_core_in_its_declared_argument_slots() {
    use lyracore_shared::group::realm_op;
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::LootMethod {
            setting: 2,
            master: VIM,
            threshold: 4,
        },
    )
    .expect("the leader sets master loot");
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).expect("convert");
    party::run(world.as_ref(), 7, GINGER, party::Op::Uninvite(VIM)).expect("kick");

    assert_eq!(
        realm.party.lock().unwrap().ops.clone(),
        vec![
            // INVITE: the target rides `target_guid`, nothing else is used.
            (realm_op::INVITE, GINGER, VIM, 0, 0, 0),
            // ACCEPT: the actor alone.
            (realm_op::ACCEPT, VIM, 0, 0, 0, 0),
            // LOOT_METHOD: setting in arg_a, MASTER in target_guid, threshold in arg_b —
            // CMSG_LOOT_METHOD's own field order.
            (realm_op::LOOT_METHOD, GINGER, VIM, 2, 4, 0),
            // RAID_CONVERT: the actor alone.
            (realm_op::RAID_CONVERT, GINGER, 0, 0, 0, 0),
            // UNINVITE: the kicked member rides `target_guid`.
            (realm_op::UNINVITE, GINGER, VIM, 0, 0, 0),
        ]
    );
}

/// The END-TO-END pin for the two production CALL SITES this slice adds — driven over a real
/// socket, through `run_world_session`'s own dispatch, not by calling `world::party` directly.
///
/// Deleting either call site is otherwise a mutation every other test in this file survives:
/// `enter_world`'s `party::on_world_entry` (a party frame the arriving player never gets, and an
/// unmirrored shard) and `social`'s `party::run` (a party op that quietly goes back to being
/// shard-local). Both are asserted here as the CLIENT sees them.
#[test]
fn a_real_session_syncs_its_party_at_login_and_routes_an_invite_to_realm_core() {
    let (realm, _world, _instances, calls) = party_topology();
    // The session's own shard, wired into the same realm + peer set as the topology's `world`,
    // plus what `run_world_session` needs to handshake and enter the world as Ginger.
    let session_shard = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        login_entity: Some(warrior_entity()),
        realm: Some(realm.clone()),
        characters: vec![character(GINGER, "Ginger"), character(VIM, "Vim")],
        live_guids: vec![GINGER, VIM],
        ..Default::default()
    });
    *session_shard.peers.lock().unwrap() = vec![session_shard.clone()];
    // Ginger is ALREADY in a party on realm-core when they log in — a party formed while they were
    // on the loading screen, which is exactly what the deleted character-transfer blob mirror could
    // never carry.
    {
        let mut p = realm.party.lock().unwrap();
        p.next_group_id = 5;
        p.groups.push((5, GINGER, 3, 2, 0));
        p.members.push((5, GINGER));
        p.members.push((5, VIM));
    }

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = session_shard.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    // A READ DEADLINE, and it is the point of the test rather than hygiene: the mutation this pins
    // (deleting `party::on_world_entry`) makes the party frame never arrive, and a blocking read on
    // a packet that will never come turns a test that must go RED into one that HANGS — which reads
    // as neither a pass nor a fail (`no_hang`'s lesson, applied at the socket instead of the thread).
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(GINGER),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();

    // The realm-wide party slice appends the party frame right after world entry.
    let mut roster_named: Option<String> = None;
    for _ in 0..WORLD_ENTRY_PACKETS + 1 {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
            Ok(ServerOpcodeMessage::SMSG_GROUP_LIST(list)) => {
                roster_named = list.members.first().map(|m| m.name.clone());
            }
            Ok(_) => {}
            Err(_) => break, // timed out or undecodable — the assertions below say what was missing
        }
    }
    assert_eq!(
        roster_named.as_deref(),
        Some("Vim"),
        "a player who logs in ALREADY in a party got no party frame — `enter_world` no longer syncs \
         the realm-core roster, so the arriving shard is unmirrored too"
    );
    assert_eq!(
        session_shard.mirror.lock().unwrap().len(),
        1,
        "world entry must push the authoritative roster onto the shard the player entered"
    );

    // …and EVERY party op typed in-world goes to realm-core, not to this shard's own tables — each
    // one attributed to the character this socket authenticated as.
    //
    // All SEVEN, not just the invite: `realm_group_op` takes the actor's guid as an ARGUMENT, so the
    // dispatch's choice of guid IS the authorization for every one of them, and the survivor the
    // author found (`0` instead of the session's guid) is a mutation each arm admits independently.
    // Pinning only the invite leaves the others free to be attributed to anybody — verified by
    // mutation: passing the KICKED player's guid as the actor of `CMSG_GROUP_UNINVITE` left all 408
    // tests green.
    use wow_world_messages::vanilla::{
        CMSG_GROUP_ACCEPT, CMSG_GROUP_DECLINE, CMSG_GROUP_DISBAND, CMSG_GROUP_INVITE,
        CMSG_GROUP_RAID_CONVERT, CMSG_GROUP_UNINVITE, CMSG_LOOT_METHOD,
    };
    CMSG_GROUP_INVITE { name: "vim".into() }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_GROUP_ACCEPT {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_GROUP_DECLINE {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_LOOT_METHOD {
        loot_setting: GroupLootSetting::MasterLoot,
        loot_master: Guid::new(VIM),
        loot_threshold: ItemQuality::Epic,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    CMSG_GROUP_RAID_CONVERT {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_GROUP_DISBAND {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_GROUP_UNINVITE { name: "vim".into() }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // The BARRIER: an invite for a name no shard can resolve never reaches `party::run`, so it adds
    // no op — but it always answers `SMSG_PARTY_COMMAND_RESULT`, and the dispatch is sequential on
    // one thread, so seeing ITS reply proves all seven above have been dispatched. (No `join`: the
    // session thread outlives the socket by design, and waiting on it would reintroduce the hang the
    // deadline above removes.)
    CMSG_GROUP_INVITE {
        name: "Nobodyatall".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..WORLD_ENTRY_PACKETS {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
            Ok(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r)) if r.member == "Nobodyatall" => {
                break;
            }
            Ok(_) => {}
            Err(_) => break, // the deadline fired — the assertions below say what was missing
        }
    }
    drop(client);
    drop(server);

    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter()
            .any(|(shard, call)| shard == "lyracore-realm" && call == "realm_group_op"),
        "CMSG_GROUP_INVITE did not reach realm-core; calls were {log:?}"
    );
    assert!(
        !log.iter().any(|(_, call)| call == "group_invite"),
        "the invite ran against the session shard's own party tables — the shard-local behaviour \
         realm-wide party routing removes. Calls were {log:?}"
    );
    // …AS the character this socket authenticated into the world with. `realm_group_op` takes the
    // actor's guid as an ARGUMENT (realm-core has no live entity to derive it from), so the guid the
    // dispatch threads in IS the authorization. A mutation that passed 0 — or any other player's
    // guid — invited on behalf of somebody else with every other assertion here still green.
    //
    // Every op, with its argument slots, exactly as the dispatch sent it. The AUTHORITY refuses most
    // of these (Ginger has no pending invite, and is no longer in a party after the disband) — the
    // mock records the tuple before it judges it, which is the point: what is pinned here is what the
    // GATEWAY claimed, not what realm-core decided to do about it.
    use lyracore_shared::group::realm_op;
    assert_eq!(
        realm.party.lock().unwrap().ops.clone(),
        vec![
            // World entry asks for the Party's Target Icons after the list.
            (realm_op::TARGET_ICON, GINGER, 0, 0xFF, 0, 0),
            (realm_op::INVITE, GINGER, VIM, 0, 0, 0),
            (realm_op::ACCEPT, GINGER, 0, 0, 0, 0),
            (realm_op::DECLINE, GINGER, 0, 0, 0, 0),
            // CMSG_LOOT_METHOD's own field order: setting in arg_a, MASTER in target_guid,
            // threshold in arg_b.
            (realm_op::LOOT_METHOD, GINGER, VIM, 2, 4, 0),
            (realm_op::RAID_CONVERT, GINGER, 0, 0, 0, 0),
            (realm_op::LEAVE, GINGER, 0, 0, 0, 0),
            (realm_op::UNINVITE, GINGER, VIM, 0, 0, 0),
        ],
        "every party op must reach realm-core attributed to the session's own character, in its \
         declared argument slots — the actor guid is the whole authorization on this plane"
    );
}

/// A failed mirror push must not turn a party op that DID commit into a failure the client renders
/// as one: realm-core has already accepted the change, and the party frame is relayed from there,
/// not from the mirror.
#[test]
fn a_shard_that_refuses_the_mirror_does_not_fail_the_party_op() {
    // The instances shard rejects `sync_group_mirror` (a database that went away mid-op).
    let (realm, world, instances, _calls) =
        party_topology_with(Some("instances is unreachable"), None);

    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(VIM))
        .expect("the invite must succeed even though one shard cannot be mirrored");
    party::run(instances.as_ref(), 8, VIM, party::Op::Accept).expect("and so must the accept");

    assert!(
        realm.group_roster(GINGER).unwrap().is_some(),
        "the authority took the change"
    );
    assert!(
        !world.mirror.lock().unwrap().is_empty(),
        "the shard that COULD be mirrored still was — one unreachable database must not stop the rest"
    );
    assert!(
        instances.mirror.lock().unwrap().is_empty(),
        "and the refusing shard is left with no mirror at all, which is the documented ceiling: it \
         re-syncs at that member's next world entry or next party op"
    );
}

// ===========================================================================================
//  Bot-initiated (serendipity) invites go through the SAME authority a player's own
//  CMSG_GROUP_INVITE does, so a `sync_group_mirror` push never contradicts a bot-formed party.
// ===========================================================================================

/// **AC: bot-formed parties are created through the same authority as player parties.**
///
/// `run_bot_invite` — not `invite_core` — is what a playerbot's serendipity pick now runs through.
/// The bot has no client and no account connection, so this must reach realm-core the same guid-based
/// way [`answer_for_session_less`] already does, and must NOT touch either shard's own `game_group`/
/// `game_group_member` tables directly (the exact shard-local write realm-wide party routing already
/// removed once).
#[test]
fn a_bot_invite_forms_a_party_on_realm_core_across_a_shard_boundary() {
    use lyracore_shared::group::realm_op;
    let (realm, world, instances, calls) = party_topology();

    // BOT (on `world`) invites FAR_BOT (on `instances`) — the far side of the boundary, same shape
    // `a_bot_standing_on_another_shard_answers_the_invite_too` pins for the player-initiated case.
    party::run_bot_invite(world.as_ref(), BOT, FAR_BOT).expect("a bot-initiated invite must land");

    let party_state = realm.party.lock().unwrap();
    let group_id = party_state
        .group_of(BOT)
        .expect("the bot's invite formed a party");
    assert_eq!(
        party_state.roster(group_id).unwrap().member_guids(),
        vec![BOT, FAR_BOT],
        "the inviting bot leads, the session-less target auto-accepts through `answer_for_session_less`"
    );
    assert_eq!(
        party_state.ops.clone(),
        vec![
            (realm_op::INVITE, BOT, FAR_BOT, 0, 0, 0),
            (realm_op::ACCEPT, FAR_BOT, 0, 0, 0, 0)
        ],
        "both halves must run on realm-core, attributed to the right actor each time — the bot as \
         itself for both the invite and (through the session-less answer) the accept"
    );
    drop(party_state);

    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter()
            .any(|(shard, call)| shard == "lyracore-realm" && call == "realm_group_op"),
        "the op must reach realm-core; calls were {log:?}"
    );
    assert!(
        !log.iter()
            .any(|(_, call)| call == "group_invite" || call == "group_accept"),
        "a bot invite must not write either shard's own party tables directly — that is the \
         serendipity-invite shard-local-write bug. \
         Calls were {log:?}"
    );
    for (name, shard) in [("world", &world), ("instances", &instances)] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![realm.group_roster(BOT).unwrap().unwrap()],
            "{name} must be mirrored by the SAME op — the shard's own kill-XP/`/p`/follow-the-leader \
             reads need the party immediately, not after some later push"
        );
    }
}

/// Two Gateways observe the same subscribed row. The World Shard claim is the only arbitration
/// point, so exactly one consumer may reach Realm-core even when both callbacks run concurrently.
#[test]
fn two_relay_consumers_execute_one_bot_invite() {
    use lyracore_shared::group::{bot_op, realm_op};

    const INTENT_ID: u64 = 41;

    let (realm, world, _instances, _calls) = party_topology();
    world.bot_invite_intents.lock().unwrap().push(INTENT_ID);

    let start = std::sync::Arc::new(std::sync::Barrier::new(3));
    let consumers: Vec<_> = (0..2)
        .map(|_| {
            let world = world.clone();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                party::run_bot_invite_intent(
                    world.as_ref(),
                    INTENT_ID,
                    bot_op::INVITE,
                    BOT,
                    FAR_BOT,
                )
            })
        })
        .collect();
    start.wait();
    for consumer in consumers {
        consumer.join().unwrap().unwrap();
    }

    let party = realm.party.lock().unwrap();
    assert_eq!(
        party
            .ops
            .iter()
            .filter(|(op, ..)| *op == realm_op::INVITE)
            .count(),
        1,
        "one intent must produce one durable INVITE"
    );
    let group_id = party
        .group_of(BOT)
        .expect("the winning consumer formed a party");
    assert_eq!(
        party.roster(group_id).unwrap().member_guids(),
        vec![BOT, FAR_BOT]
    );
    assert!(world.bot_invite_intents.lock().unwrap().is_empty());
}

/// **AC: one intent table, two party ops.** The row's `op` byte is the whole dispatch, so an INVITE
/// and a LEAVE written to the same table must reach two different ops on the party authority. A
/// dispatch that ignored the byte would silently re-invite a bot that asked to leave.
#[test]
fn the_intent_op_byte_picks_the_party_op_that_runs() {
    use lyracore_shared::group::{bot_op, realm_op};

    const INVITE_INTENT: u64 = 71;
    const LEAVE_INTENT: u64 = 72;

    let (realm, world, _instances, _calls) = party_topology();
    world
        .bot_invite_intents
        .lock()
        .unwrap()
        .extend([INVITE_INTENT, LEAVE_INTENT]);

    party::run_bot_invite_intent(world.as_ref(), INVITE_INTENT, bot_op::INVITE, BOT, FAR_BOT)
        .expect("op 0 forms the party");
    let group_id = realm
        .party
        .lock()
        .unwrap()
        .group_of(BOT)
        .expect("the invite intent formed a party");

    party::run_bot_invite_intent(world.as_ref(), LEAVE_INTENT, bot_op::LEAVE, BOT, 0)
        .expect("op 1 leaves the party");

    let party_state = realm.party.lock().unwrap();
    assert_eq!(
        party_state.ops.clone(),
        vec![
            (realm_op::INVITE, BOT, FAR_BOT, 0, 0, 0),
            (realm_op::ACCEPT, FAR_BOT, 0, 0, 0, 0),
            (realm_op::LEAVE, BOT, 0, 0, 0, 0),
        ],
        "the invite runs INVITE (plus the session-less answer) and the leave runs LEAVE, each \
         attributed to the bot itself"
    );
    assert_eq!(
        party_state.group_of(BOT),
        None,
        "the leaving bot is out of the party it led"
    );
    assert_eq!(
        party_state.roster(group_id),
        None,
        "a party of one disbands, which is what puts both bots back in the pool the invite scan \
         draws from"
    );
}

/// A module newer than this gateway can write an op byte this build has never heard of. Running it
/// as an invite would form a party nobody asked for, so it is refused by name — and the claim has
/// already consumed the row, so it is refused once rather than every second.
#[test]
fn an_unknown_intent_op_is_refused_rather_than_run_as_an_invite() {
    const INTENT_ID: u64 = 73;

    let (realm, world, _instances, _calls) = party_topology();
    world.bot_invite_intents.lock().unwrap().push(INTENT_ID);

    let err = party::run_bot_invite_intent(world.as_ref(), INTENT_ID, 200, BOT, FAR_BOT)
        .expect_err("an unknown op must not fall through to an invite");
    assert!(
        err.to_string().contains("200"),
        "the refusal must name the byte it could not run: {err:#}"
    );
    assert!(
        realm.party.lock().unwrap().ops.is_empty(),
        "nothing may reach the party authority"
    );
}

/// **The regression test the issue asks for.** A bot party must survive the next
/// `sync_group_mirror` push that touches its group id — the failure mode was that realm-core had
/// never heard of the group, so the mirror read that as "this party does not exist" and tombstoned
/// it. Simulated here as a SECOND, independent push (a later world entry or another member's op would
/// trigger exactly this) rather than the op's own immediate push, so it is not just re-testing the
/// invite path above.
#[test]
fn a_bots_party_survives_the_next_sync_group_mirror_push_that_touches_it() {
    let (realm, world, _instances, _calls) = party_topology();
    // A second BOT as the target (not TRIN, a real player) so the invite auto-accepts and actually
    // forms a party in one call — a pending invite has no mirror to survive anything.
    party::run_bot_invite(world.as_ref(), BOT, FAR_BOT).expect("forms the party");
    let group_id = realm
        .party
        .lock()
        .unwrap()
        .group_of(BOT)
        .expect("the bot is in a party");
    let authoritative = realm.group_roster_by_id(group_id).unwrap().unwrap();
    assert_eq!(
        world.mirror.lock().unwrap().clone(),
        vec![authoritative.clone()],
        "precondition: the party is already mirrored by the invite's own push"
    );

    // The next push that touches this exact group id — nobody's `self_guid`, `before` names the group
    // directly, matching how `on_world_entry`/`sync_mirrors` reach a group that isn't the actor's own.
    party::sync_mirrors(
        world.as_ref(),
        realm.as_ref(),
        0,
        realm.group_roster_by_id(group_id).unwrap(),
    );

    assert_eq!(
        world.mirror.lock().unwrap().clone(),
        vec![authoritative],
        "the bot party must survive the push — realm-core has a real row for it, so the mirror must \
         reconfirm the roster rather than tombstone it. Wiping it here is the serendipity-invite \
         shard-local-write bug, reproduced"
    );
}

/// **The counterfactual, proving the mechanism above is real.** A group that realm-core has never
/// heard of — modelling the bug's PRE-fix shape, where a bot wrote this shard's
/// `game_group`/`game_group_member` rows directly and realm-core's authority never gained a matching
/// row — IS wiped by the next push
/// that touches its id. This is not a hypothetical: the world shard's own `game_group.group_id` and
/// realm-core's run independent `#[auto_inc]` counters, so a shard-local-only id colliding with some
/// unrelated REAL realm-core party's id was exactly how the live bug manifested — any op on that real
/// party pushed realm-core's (different) roster for the same number over the bot's local rows.
#[test]
fn a_shard_local_only_group_realm_core_never_heard_of_is_wiped_by_the_next_push() {
    let (realm, world, _instances, _calls) = party_topology();
    let phantom_group_id = 4242;
    let phantom = party::GroupRoster {
        group_id: phantom_group_id,
        leader_guid: BOT,
        members: party_members(&[BOT, TRIN]),
        ..Default::default()
    };
    world
        .sync_group_mirror(&phantom)
        .expect("simulate the bug's pre-fix shard-local-only write");
    assert_eq!(
        world.mirror.lock().unwrap().clone(),
        vec![phantom.clone()],
        "precondition"
    );
    assert!(
        realm
            .group_roster_by_id(phantom_group_id)
            .unwrap()
            .is_none(),
        "precondition: realm-core has never heard of this group — the whole bug"
    );

    party::sync_mirrors(world.as_ref(), realm.as_ref(), 0, Some(phantom));

    assert!(
        world.mirror.lock().unwrap().is_empty(),
        "a group realm-core does not know about must read as tombstoned — this is the \
         serendipity-invite bug's exact \
         mechanism, which is why routing bot invites through realm-core (not writing shard-local rows) \
         is the fix rather than teaching the mirror to tolerate unknown groups"
    );
}

/// Same existence/online gate a player's own invite uses (`presence`/`live_anywhere`) — a bot invite
/// must not skip it just because there is no client waiting on the `SMSG_PARTY_COMMAND_RESULT`.
#[test]
fn a_bot_invite_to_a_missing_or_offline_target_never_reaches_realm_core() {
    let (realm, world, _instances, _calls) = party_topology();

    assert_eq!(
        party::run_bot_invite(world.as_ref(), BOT, DORMANT).unwrap(),
        PartyOutcome::Refused(GroupRefusal::TargetOffline)
    );
    assert_eq!(
        party::run_bot_invite(world.as_ref(), BOT, 999).unwrap(),
        PartyOutcome::Refused(GroupRefusal::NoSuchPlayer)
    );

    assert!(
        realm.party.lock().unwrap().ops.is_empty(),
        "a refused bot invite must not reach the authority"
    );
}

/// **AC, unsharded half.** A bot has no per-account connection, on EITHER topology — so unlike
/// [`run`], `run_bot_invite` must not fall back to the account-based player reducers when there is no
/// realm-core to route to. Unsharded, this database is its own authority: the guid-based
/// `realm_group_op` still runs, against the ONE database there is.
#[test]
fn an_unsharded_deployment_still_routes_a_bot_invite_through_realm_group_op() {
    use lyracore_shared::group::realm_op;
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        is_realm: true, // this database IS its own authority — nothing else to route to
        characters: vec![character(BOT, "Botty"), character(TRIN, "Trin")],
        live_guids: vec![BOT, TRIN],
        offline_guids: vec![BOT],
        ..Default::default() // no `realm`, no `peers` — the unconfigured gateway
    });
    assert!(
        store.realm_store().is_none(),
        "an unsharded store must not name a realm database"
    );

    party::run_bot_invite(store.as_ref(), BOT, TRIN)
        .expect("a bot invite must work with no realm-core to route to");

    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter()
            .any(|(shard, call)| shard == "world" && call == "realm_group_op"),
        "an unsharded deployment must still use the guid-based realm_group_op — a bot has no account \
         connection for `run`'s unsharded arm to call the player-facing reducers as. Calls were {log:?}"
    );
    assert!(
        !log.iter().any(|(_, call)| call == "group_invite"),
        "must not take `run`'s account-based arm at all (there is no account to run it as). Calls \
         were {log:?}"
    );
    assert_eq!(
        store.party.lock().unwrap().ops.first().copied(),
        Some((realm_op::INVITE, BOT, TRIN, 0, 0, 0)),
        "the invite must be recorded with the bot as inviter"
    );
}

#[test]
fn suppressed_automatic_answers_leave_human_invitations_pending_on_realm_core() {
    let (realm, world, instances, _) = party_topology();
    instances
        .sessionless_admission
        .lock()
        .unwrap()
        .insert(FAR_BOT, GroupRefusal::ActionSuppressed);

    assert_eq!(
        party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap(),
        PartyOutcome::Ran
    );

    let state = realm.party.lock().unwrap();
    assert!(state.group_of(FAR_BOT).is_none());
    assert_eq!(
        state.ops,
        vec![(realm_op::INVITE, GINGER, FAR_BOT, 0, 0, 0)]
    );
}

#[test]
fn suppressed_automatic_answers_leave_bot_invitations_pending_on_realm_core() {
    let (realm, world, instances, _) = party_topology();
    instances
        .sessionless_admission
        .lock()
        .unwrap()
        .insert(FAR_BOT, GroupRefusal::ActionSuppressed);

    assert_eq!(
        party::run_bot_invite(world.as_ref(), BOT, FAR_BOT).unwrap(),
        PartyOutcome::Ran
    );

    let state = realm.party.lock().unwrap();
    assert!(state.group_of(FAR_BOT).is_none());
    assert_eq!(state.ops, vec![(realm_op::INVITE, BOT, FAR_BOT, 0, 0, 0)]);
}

#[test]
fn unavailable_admission_leaves_the_invitation_unanswered() {
    let (realm, world, instances, _) = party_topology();
    instances
        .sessionless_admission_unavailable
        .lock()
        .unwrap()
        .push(FAR_BOT);

    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();

    let state = realm.party.lock().unwrap();
    assert!(state.group_of(FAR_BOT).is_none());
    assert_eq!(
        state.ops,
        vec![(realm_op::INVITE, GINGER, FAR_BOT, 0, 0, 0)]
    );
}

#[test]
fn current_admission_refuses_a_stale_sessionless_presence_read() {
    let (realm, world, instances, _) = party_topology();
    instances
        .sessionless_admission
        .lock()
        .unwrap()
        .insert(FAR_BOT, GroupRefusal::ActorUnavailable);

    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();

    let state = realm.party.lock().unwrap();
    assert!(state.group_of(FAR_BOT).is_none());
    assert_eq!(
        state.ops,
        vec![(realm_op::INVITE, GINGER, FAR_BOT, 0, 0, 0)]
    );
}

#[test]
fn a_suppressed_group_intent_never_reaches_realm_core() {
    let (realm, world, _, _) = party_topology();
    world
        .bot_intent_claim_refusals
        .lock()
        .unwrap()
        .insert(77, GroupRefusal::ActionSuppressed);

    assert_eq!(
        party::run_bot_invite_intent(
            world.as_ref(),
            77,
            lyracore_shared::group::bot_op::INVITE,
            BOT,
            FAR_BOT
        )
        .unwrap(),
        PartyOutcome::Refused(GroupRefusal::ActionSuppressed)
    );

    assert!(realm.party.lock().unwrap().ops.is_empty());
}

#[test]
fn a_controller_selection_after_admission_does_not_recall_the_answer() {
    let (realm, world, instances, _) = party_topology();
    instances
        .suppress_after_admission
        .lock()
        .unwrap()
        .push(FAR_BOT);

    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(FAR_BOT)).unwrap();

    assert!(realm.party.lock().unwrap().group_of(FAR_BOT).is_some());
    assert_eq!(
        instances.admit_sessionless_group_action(FAR_BOT).unwrap(),
        PartyOutcome::Refused(GroupRefusal::ActionSuppressed)
    );
}

#[test]
fn unsharded_bot_invitations_leave_a_suppressed_target_unanswered() {
    let store = InMemoryStore {
        characters: vec![character(BOT, "Botty"), character(FAR_BOT, "Farbotty")],
        live_guids: vec![BOT, FAR_BOT],
        offline_guids: vec![BOT, FAR_BOT],
        ..Default::default()
    };
    store
        .sessionless_admission
        .lock()
        .unwrap()
        .insert(FAR_BOT, GroupRefusal::ActionSuppressed);

    party::run_bot_invite(&store, BOT, FAR_BOT).unwrap();

    let state = store.party.lock().unwrap();
    assert!(state.group_of(FAR_BOT).is_none());
    assert_eq!(state.invites, vec![(FAR_BOT, BOT)]);
    assert_eq!(state.ops, vec![(realm_op::INVITE, BOT, FAR_BOT, 0, 0, 0)]);
}

// ---- Raids ----

/// Decode one sent `SMSG_GROUP_LIST`. With a loot block it must end with the extra byte 0 the 1.12
/// servers send after the loot threshold.
fn group_list(packet: Outbound) -> wow_world_messages::vanilla::SMSG_GROUP_LIST {
    let Outbound::Raw { opcode, body } = packet else {
        panic!("expected a raw SMSG_GROUP_LIST")
    };
    assert_eq!(opcode, 0x007D);
    let mut framed = u16::try_from(body.len() + 2)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    framed.extend(opcode.to_le_bytes());
    framed.extend(&body);
    let ServerOpcodeMessage::SMSG_GROUP_LIST(list) =
        ServerOpcodeMessage::read_unencrypted(framed.as_slice()).expect("a group list")
    else {
        panic!("expected SMSG_GROUP_LIST")
    };
    if list.group_not_empty.is_some() {
        assert_eq!(body.last(), Some(&0), "the byte after the loot threshold");
        assert_eq!(body.len(), codec::build_group_list_raw(&list).1.len());
    }
    *list
}

fn mirror_calls(calls: &ShardCallLog) -> usize {
    calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, call)| call == "sync_group_mirror")
        .count()
}

/// **AC: the leader converts; every member gets a raid list and every shard mirrors the Raid.**
#[test]
fn a_leader_converts_the_party_on_realm_core_and_every_shard_mirrors_the_raid() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).unwrap();
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).unwrap();
    let events_before = realm.party.lock().unwrap().events.len();

    let outcome = party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();

    assert_eq!(outcome, PartyOutcome::Ran);
    let state = realm.party.lock().unwrap();
    assert_eq!(
        state.ops.last().copied(),
        Some((realm_op::RAID_CONVERT, GINGER, 0, 0, 0, 0))
    );
    let mut listed: Vec<_> = state.events[events_before..]
        .iter()
        .filter(|(_, kind)| *kind == lyracore_shared::group::event_kind::LIST)
        .map(|(guid, _)| *guid)
        .collect();
    listed.sort_unstable();
    assert_eq!(
        listed,
        [GINGER, VIM, TRIN],
        "every member receives the list"
    );
    drop(state);
    let authority = realm.group_roster(GINGER).unwrap().unwrap();
    assert_eq!(authority.kind, GroupKind::Raid);
    assert!(authority
        .members
        .iter()
        .all(|member| member.slot == RaidSlot::default()));
    for (name, shard) in [("world", &world), ("instances", &instances)] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![authority.clone()],
            "{name} must mirror the kind and every Raid Slot, or its local raid reads disagree \
             with Realm-core"
        );
    }
    let list = group_list(party::render_list(
        instances.as_ref(),
        VIM,
        &authority.list_payload(),
    ));
    assert_eq!(
        list.group_type,
        wow_world_messages::vanilla::GroupType::Raid
    );
    assert_eq!(list.flags, 0);
    assert!(list.members.iter().all(|member| member.flags == 0));
}

/// Converting a Raid again changes nothing on Realm-core, so it must not cost a mirror push to
/// every World Shard each time a client repeats the opcode.
#[test]
fn converting_a_raid_again_pushes_no_mirror() {
    let (_realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    let mirrors_before = mirror_calls(&calls);

    let outcome = party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();

    assert_eq!(outcome, PartyOutcome::Ran, "the client still hears success");
    assert_eq!(mirror_calls(&calls), mirrors_before);
}

/// **AC: a non-leader's convert changes nothing and sends nothing.**
#[test]
fn a_member_who_does_not_lead_cannot_convert_and_no_mirror_is_pushed() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    let events_before = realm.party.lock().unwrap().events.len();
    let mirrors_before = mirror_calls(&calls);

    let outcome = party::run(instances.as_ref(), 8, VIM, party::Op::RaidConvert).unwrap();

    assert_eq!(outcome, PartyOutcome::Refused(GroupRefusal::NotLeader));
    assert_eq!(
        realm.group_roster(GINGER).unwrap().unwrap().kind,
        GroupKind::Party
    );
    assert_eq!(realm.party.lock().unwrap().events.len(), events_before);
    assert_eq!(mirror_calls(&calls), mirrors_before);
}

/// **AC: a single-database Gateway converts through `realm_group_op` on its only shard.** A raid op
/// has no player-facing reducer, so the home shard is the party authority there.
#[test]
fn an_unsharded_gateway_converts_through_realm_group_op_on_its_own_shard() {
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        ..Default::default()
    });
    {
        let mut p = store.party.lock().unwrap();
        p.groups.push((5, GINGER, 3, 2, 0));
        p.members.push((5, GINGER));
        p.members.push((5, VIM));
    }

    let outcome = party::run(store.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();

    assert_eq!(outcome, PartyOutcome::Ran);
    assert_eq!(
        calls.lock().unwrap().clone(),
        vec![("world".to_string(), "realm_group_op".to_string())],
        "one call, on the player's own shard, and no mirror push"
    );
    let state = store.party.lock().unwrap();
    assert_eq!(
        state.ops,
        vec![(realm_op::RAID_CONVERT, GINGER, 0, 0, 0, 0)]
    );
    assert_eq!(state.kind_of(5), GroupKind::Raid);
}

/// **AC: with Subgroup 0 full, the next joiner lands in Subgroup 1, and every member's list shows
/// its flags byte as 1.** The placement is the Module's rule; the Gateway must carry it to every
/// mirror and every list.
#[test]
fn a_raid_joiner_past_a_full_first_subgroup_shows_subgroup_one_in_every_list() {
    let (realm, world, instances, _calls) = party_topology();
    {
        let mut p = realm.party.lock().unwrap();
        p.next_group_id = 9;
        p.groups.push((9, GINGER, 3, 2, 0));
        for guid in [GINGER, VIM, 101, 102, 103] {
            p.members.push((9, guid));
        }
        p.raids.push(9);
    }
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).expect("the leader invites");
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).expect("the sixth member accepts");

    let authority = realm.group_roster(GINGER).unwrap().unwrap();
    let joiner = authority
        .members
        .iter()
        .find(|member| member.guid == TRIN)
        .unwrap();
    assert_eq!(joiner.slot, RaidSlot::new(1, false).unwrap());
    for shard in [&world, &instances] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![authority.clone()]
        );
    }
    for (viewer, shard) in [
        (GINGER, world.as_ref()),
        (VIM, instances.as_ref()),
        (TRIN, world.as_ref()),
    ] {
        let list = group_list(party::render_list(shard, viewer, &authority.list_payload()));
        assert_eq!(
            list.group_type,
            wow_world_messages::vanilla::GroupType::Raid
        );
        assert_eq!(list.flags, u8::from(viewer == TRIN), "{viewer}'s own flags");
        for member in &list.members {
            assert_eq!(
                member.flags,
                u8::from(member.guid.guid() == TRIN),
                "{viewer} sees {}",
                member.guid.guid()
            );
        }
    }
}

/// **AC: a member on another shard renders online in a LIST pushed from Realm-core.**
///
/// Realm-core has no live entities. The roster payload used to carry an online flag the Module
/// computed there anyway, which was 0 for every member, and the relay trusted it: on a sharded
/// Realm every party op re-rendered every member offline until the next world entry. The payload
/// now carries no presence, and the relay reads presence from the World Shard caches. This drives
/// the relay's own decode body, `group_event_outbound`, with the Realm-core handle.
#[test]
fn the_realm_core_list_relay_renders_a_member_on_another_shard_online() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    // The production Realm-core Coordinator reads every World Shard.
    *realm.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    // What the Module writes on Realm-core: no names, no presence.
    let payload = RosterPayload {
        leader: GINGER,
        loot_method: 3,
        loot_threshold: 2,
        master_looter_guid: 0,
        kind: GroupKind::Party,
        members: [GINGER, VIM]
            .into_iter()
            .map(|guid| RosterMember {
                guid,
                name: String::new(),
                slot: RaidSlot::default(),
            })
            .collect(),
        target_icons: Vec::new(),
    }
    .encode();
    let row = crate::stdb::bindings::GroupEvent {
        id: 1,
        recipient_identity: spacetimedb_sdk::Identity::ZERO,
        kind: lyracore_shared::group::event_kind::LIST,
        other_guid: 0,
        other_name: String::new(),
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        payload,
        recipient_guid: GINGER,
    };

    let packets = crate::stdb::subscriptions::group_event_outbound(realm.as_ref(), GINGER, &row);

    let mut packets = packets.into_iter();
    let (Some(packet), None) = (packets.next(), packets.next()) else {
        panic!("expected one SMSG_GROUP_LIST")
    };
    let list = group_list(packet);
    assert_eq!(list.members.len(), 1);
    assert_eq!(list.members[0].guid.guid(), VIM);
    assert_eq!(list.members[0].name, "Vim");
    assert!(
        list.members[0].is_online,
        "Vim is live on the instances shard, so the Realm-core list must show Vim online"
    );
}

/// A name the payload carries is kept; a blank one is read from the shards; a member no shard can
/// name stays in the list, blank and offline, because a missing row reads as "they left".
#[test]
fn a_list_keeps_payload_names_and_lists_a_member_no_shard_can_name() {
    let (_realm, world, _instances, _calls) = party_topology();
    let member = |guid, name: &str| RosterMember {
        guid,
        name: name.to_string(),
        slot: RaidSlot::default(),
    };
    let roster = RosterPayload {
        leader: GINGER,
        loot_method: 3,
        loot_threshold: 2,
        master_looter_guid: 0,
        kind: GroupKind::Party,
        members: vec![
            member(GINGER, ""),
            member(TRIN, "Trinity"),
            member(VIM, ""),
            member(404, ""),
        ],
        target_icons: Vec::new(),
    };

    let list = group_list(party::render_list(world.as_ref(), GINGER, &roster));

    let named: Vec<_> = list
        .members
        .iter()
        .map(|member| (member.guid.guid(), member.name.as_str(), member.is_online))
        .collect();
    assert_eq!(
        named,
        [
            (TRIN, "Trinity", true),
            (VIM, "Vim", true),
            (404, "", false)
        ]
    );
}

/// **AC: `CMSG_GROUP_RAID_CONVERT` in, `SMSG_PARTY_COMMAND_RESULT(Invite, "", Success)` out**
/// (cm:GroupHandler.cpp:488), and a refused convert sends nothing (cm:GroupHandler.cpp:483-484).
#[test]
fn raid_convert_answers_the_leader_with_success_and_a_refusal_with_silence() {
    use wow_world_messages::vanilla::{
        PartyOperation, PartyResult, CMSG_GROUP_INVITE, CMSG_GROUP_RAID_CONVERT,
    };
    let s = quest_store();
    {
        let mut p = s.party.lock().unwrap();
        p.groups.push((5, 1, 3, 2, 0));
        p.members.push((5, 1));
        p.members.push((5, 2));
    }
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);

    CMSG_GROUP_RAID_CONVERT {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(r.operation, PartyOperation::Invite);
            assert_eq!(r.member, "");
            assert_eq!(r.result, PartyResult::Success);
        }
        other => panic!("expected SMSG_PARTY_COMMAND_RESULT, got {other}"),
    }
    assert_eq!(store.party.lock().unwrap().kind_of(5), GroupKind::Raid);

    // Another member now leads, so the session's convert is refused. The barrier invite's reply
    // must be the next packet: the refusal sent nothing.
    store.party.lock().unwrap().groups[0].1 = 2;
    CMSG_GROUP_RAID_CONVERT {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_GROUP_INVITE {
        name: "Nobodyatall".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => {
            assert_eq!(
                r.member, "Nobodyatall",
                "the refused convert answered the client"
            );
        }
        other => panic!("expected the barrier's SMSG_PARTY_COMMAND_RESULT, got {other}"),
    }
    drop(client);
    let _ = server.join();
}

// ---- Leadership and Assistants ----

/// Ginger leads a Party of Ginger (world), Vim (instances) and Trin (world).
fn party_of_three() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    ShardCallLog,
) {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).unwrap();
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).unwrap();
    (realm, world, instances, calls)
}

/// The kinds `guid` received since event `from`.
fn events_for(realm: &InMemoryStore, from: usize, guid: u64) -> Vec<u8> {
    realm.party.lock().unwrap().events[from..]
        .iter()
        .filter(|(recipient, _)| *recipient == guid)
        .map(|(_, kind)| *kind)
        .collect()
}

/// **AC: the leader passes the lead to an online member. Every member hears the new leader, then
/// gets a list naming it, and every shard mirrors the new leader.** The target is live on the far
/// shard, which only the Gateway can see from here.
#[test]
fn the_leader_passes_the_lead_to_a_member_live_on_the_far_shard() {
    use lyracore_shared::group::event_kind::{LIST, SET_LEADER};
    let (realm, world, instances, _calls) = party_of_three();
    let events_before = realm.party.lock().unwrap().events.len();

    let outcome = party::run(world.as_ref(), 7, GINGER, party::Op::SetLeader(VIM)).unwrap();

    assert_eq!(outcome, PartyOutcome::Ran);
    let authority = realm.group_roster(GINGER).unwrap().unwrap();
    assert_eq!(authority.leader_guid, VIM);
    for guid in [GINGER, VIM, TRIN] {
        assert_eq!(
            events_for(&realm, events_before, guid),
            [SET_LEADER, LIST],
            "{guid} hears the new leader before the list"
        );
    }
    for shard in [&world, &instances] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![authority.clone()]
        );
    }
}

/// **AC: passing the lead to an offline member changes nothing.** Realm-core cannot see presence,
/// so the Gateway refuses before it calls the authority, and a live target on either shard passes.
#[test]
fn the_lead_passes_only_to_a_member_live_on_some_shard() {
    let (realm, world, _instances, calls) = party_of_three();
    realm.party.lock().unwrap().members.push((1, DORMANT));
    let ops_before = realm.party.lock().unwrap().ops.len();
    let mirrors_before = mirror_calls(&calls);

    let outcome = party::run(world.as_ref(), 7, GINGER, party::Op::SetLeader(DORMANT)).unwrap();

    assert_eq!(outcome, PartyOutcome::Refused(GroupRefusal::TargetOffline));
    assert_eq!(realm.party.lock().unwrap().ops.len(), ops_before);
    assert_eq!(mirror_calls(&calls), mirrors_before);
    assert_eq!(
        realm.group_roster(GINGER).unwrap().unwrap().leader_guid,
        GINGER
    );

    for (leader, next) in [(GINGER, TRIN), (TRIN, VIM)] {
        let outcome = party::run(world.as_ref(), 7, leader, party::Op::SetLeader(next)).unwrap();
        assert_eq!(
            outcome,
            PartyOutcome::Ran,
            "{next} is live, so the lead passes"
        );
    }
    assert_eq!(
        realm.group_roster(GINGER).unwrap().unwrap().leader_guid,
        VIM
    );
}

/// The two leadership ops pack their arguments into `realm_group_op`'s slots as the shared contract
/// names them: the member in `target_guid`, and promote or demote in `arg_a`.
#[test]
fn leadership_ops_reach_realm_core_in_their_declared_argument_slots() {
    let (realm, world, _instances, _calls) = party_of_three();
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    let ops_before = realm.party.lock().unwrap().ops.len();

    for op in [
        party::Op::SetAssistant {
            target: TRIN,
            promote: true,
        },
        party::Op::SetAssistant {
            target: TRIN,
            promote: false,
        },
        party::Op::SetLeader(VIM),
    ] {
        party::run(world.as_ref(), 7, GINGER, op).unwrap();
    }

    assert_eq!(
        realm.party.lock().unwrap().ops[ops_before..],
        [
            (realm_op::SET_ASSISTANT, GINGER, TRIN, 1, 0, 0),
            (realm_op::SET_ASSISTANT, GINGER, TRIN, 0, 0, 0),
            (realm_op::SET_LEADER, GINGER, VIM, 0, 0, 0),
        ]
    );
}

/// **AC: the raid leader promotes a member, and every list shows its flags byte with `0x80`;
/// demoting clears it.** Each change resyncs every mirror.
#[test]
fn a_promoted_assistant_shows_0x80_in_every_list_and_every_mirror() {
    let (realm, world, instances, _calls) = party_of_three();
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    let assistant = |promote| party::Op::SetAssistant {
        target: VIM,
        promote,
    };

    for (promote, flags) in [(true, 0x80), (false, 0x00)] {
        let outcome = party::run(world.as_ref(), 7, GINGER, assistant(promote)).unwrap();
        assert_eq!(outcome, PartyOutcome::Ran);
        let authority = realm.group_roster(GINGER).unwrap().unwrap();
        for shard in [&world, &instances] {
            assert_eq!(
                shard.mirror.lock().unwrap().clone(),
                vec![authority.clone()]
            );
        }
        let payload = authority.list_payload();
        let own = group_list(party::render_list(instances.as_ref(), VIM, &payload));
        assert_eq!(own.flags, flags, "Vim's own flags byte");
        for viewer in [GINGER, TRIN] {
            let list = group_list(party::render_list(world.as_ref(), viewer, &payload));
            let vim = list.members.iter().find(|m| m.guid.guid() == VIM).unwrap();
            assert_eq!(vim.flags, flags, "{viewer} sees Vim");
        }
    }
}

/// Repeating a promotion changes nothing on Realm-core, so it costs no mirror push.
#[test]
fn repeating_a_promotion_pushes_no_mirror() {
    let (_realm, world, _instances, calls) = party_of_three();
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    let promote = party::Op::SetAssistant {
        target: VIM,
        promote: true,
    };
    party::run(world.as_ref(), 7, GINGER, promote).unwrap();
    let mirrors_before = mirror_calls(&calls);

    let outcome = party::run(world.as_ref(), 7, GINGER, promote).unwrap();

    assert_eq!(outcome, PartyOutcome::Ran);
    assert_eq!(mirror_calls(&calls), mirrors_before);
}

/// **AC: a single-database Gateway runs both leadership ops through `realm_group_op` on its own
/// shard**, after the same presence Gate.
#[test]
fn an_unsharded_gateway_runs_leadership_ops_on_its_own_shard() {
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        live_guids: vec![GINGER, VIM],
        ..Default::default()
    });
    {
        let mut p = store.party.lock().unwrap();
        p.groups.push((5, GINGER, 3, 2, 0));
        p.members.push((5, GINGER));
        p.members.push((5, VIM));
        p.raids.push(5);
    }
    let promote = party::Op::SetAssistant {
        target: VIM,
        promote: true,
    };

    party::run(store.as_ref(), 7, GINGER, promote).unwrap();
    party::run(store.as_ref(), 7, GINGER, party::Op::SetLeader(VIM)).unwrap();

    let ran: Vec<_> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, call)| call.clone())
        .collect();
    assert_eq!(ran, ["realm_group_op", "realm_group_op"]);
    let state = store.party.lock().unwrap();
    assert_eq!(state.leader_of(5), VIM);
    assert!(state.slots[&VIM].is_assistant());
}

/// A `SET_LEADER` row as the relay decodes it.
fn set_leader_row(leader: u64) -> crate::stdb::bindings::GroupEvent {
    crate::stdb::bindings::GroupEvent {
        id: 1,
        recipient_identity: spacetimedb_sdk::Identity::ZERO,
        kind: lyracore_shared::group::event_kind::SET_LEADER,
        other_guid: leader,
        other_name: String::new(),
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        payload: String::new(),
        recipient_guid: GINGER,
    }
}

/// The one `SMSG_GROUP_SET_LEADER` name the relay sends for `row`, or `None` for no packet.
fn relayed_leader_name(
    realm: &InMemoryStore,
    row: &crate::stdb::bindings::GroupEvent,
) -> Option<String> {
    let packets = crate::stdb::subscriptions::group_event_outbound(realm, GINGER, row);
    match &packets[..] {
        [] => None,
        [Outbound::One(ServerOpcodeMessage::SMSG_GROUP_SET_LEADER(announced))] => {
            Some(announced.name.clone())
        }
        other => panic!(
            "expected at most one SMSG_GROUP_SET_LEADER, got {} packets",
            other.len()
        ),
    }
}

/// **AC: every member gets `SMSG_GROUP_SET_LEADER` with the new leader's name.** The relay reads
/// the name from whichever shard holds the leader. A leader no shard can name sends nothing rather
/// than a blank "is now the group leader" line.
#[test]
fn the_set_leader_relay_names_the_leader_from_the_far_shard() {
    let (realm, world, instances, _calls) = party_topology();
    *realm.peers.lock().unwrap() = vec![world.clone(), instances.clone()];

    assert_eq!(
        relayed_leader_name(&realm, &set_leader_row(VIM)).as_deref(),
        Some("Vim")
    );
    assert_eq!(relayed_leader_name(&realm, &set_leader_row(404)), None);
}

/// Decode one hand-written client frame: size (u16 BE, opcode plus body), opcode (u32 LE), body.
fn client_frame(opcode: u32, body: &[u8]) -> ClientOpcodeMessage {
    let mut framed = u16::try_from(body.len() + 4)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    framed.extend(opcode.to_le_bytes());
    framed.extend(body);
    ClientOpcodeMessage::read_unencrypted(&mut framed.as_slice()).expect("a client frame")
}

/// The three bodies as cmangos reads them: a u64 member guid, and for the Assistant opcode one
/// flag byte after it (cm:GroupHandler.cpp:252-253, 346-347, 529-532).
#[test]
fn leadership_opcodes_decode_from_the_vanilla_layout() {
    let guid = 0x0102_0304_0506_0708u64;
    let guid_bytes = [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];

    let ClientOpcodeMessage::CMSG_GROUP_SET_LEADER(set_leader) = client_frame(0x0078, &guid_bytes)
    else {
        panic!("0x0078 is CMSG_GROUP_SET_LEADER")
    };
    assert_eq!(set_leader.guid.guid(), guid);
    let ClientOpcodeMessage::CMSG_GROUP_UNINVITE_GUID(kick) = client_frame(0x0076, &guid_bytes)
    else {
        panic!("0x0076 is CMSG_GROUP_UNINVITE_GUID")
    };
    assert_eq!(kick.guid.guid(), guid);
    for (flag, promote) in [(0x01, true), (0x00, false)] {
        let body = [&guid_bytes[..], &[flag]].concat();
        let ClientOpcodeMessage::CMSG_GROUP_ASSISTANT_LEADER(assistant) =
            client_frame(0x028F, &body)
        else {
            panic!("0x028F is CMSG_GROUP_ASSISTANT_LEADER")
        };
        assert_eq!(assistant.guid.guid(), guid);
        assert_eq!(assistant.set_assistant, promote);
    }
}

/// A session on a shard that reaches `realm`, entered as Ginger.
fn realm_session(
    realm: &std::sync::Arc<InMemoryStore>,
) -> (
    UnixStream,
    EncrypterHalf,
    DecrypterHalf,
    std::thread::JoinHandle<()>,
) {
    let store = std::sync::Arc::new(InMemoryStore {
        realm: Some(realm.clone()),
        ..quest_store()
    });
    enter_world(store, GINGER)
}

/// Send `CMSG_GROUP_UNINVITE_GUID`, then the barrier.
fn kick_by_guid(
    client: &mut UnixStream,
    c_enc: &mut EncrypterHalf,
    c_dec: &mut DecrypterHalf,
    guid: u64,
) -> wow_world_messages::vanilla::SMSG_PARTY_COMMAND_RESULT {
    wow_world_messages::vanilla::CMSG_GROUP_UNINVITE_GUID {
        guid: Guid::new(guid),
    }
    .write_encrypted_client(&mut *client, &mut *c_enc)
    .unwrap();
    barrier(client, c_enc, c_dec)
}

/// Send an invite for a name no shard knows, which is always answered. Returns the first
/// `SMSG_PARTY_COMMAND_RESULT` the session sends back, past the party frame world entry sent. The
/// dispatch is sequential, so a result naming the barrier proves that no op before it answered.
fn barrier(
    client: &mut UnixStream,
    c_enc: &mut EncrypterHalf,
    c_dec: &mut DecrypterHalf,
) -> wow_world_messages::vanilla::SMSG_PARTY_COMMAND_RESULT {
    wow_world_messages::vanilla::CMSG_GROUP_INVITE {
        name: "Nobodyatall".into(),
    }
    .write_encrypted_client(&mut *client, &mut *c_enc)
    .unwrap();
    loop {
        match ServerOpcodeMessage::read_encrypted(&mut *client, &mut *c_dec).unwrap() {
            ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(result) => return *result,
            ServerOpcodeMessage::SMSG_GROUP_LIST(_) => {}
            other => panic!("expected SMSG_PARTY_COMMAND_RESULT, got {other}"),
        }
    }
}

/// **AC: an Assistant who names the leader in the raid frame's "Remove from group" gets
/// `SMSG_PARTY_COMMAND_RESULT(Leave, "", NotLeader)`, and nothing changes**
/// (cm:GroupHandler.cpp:274-276).
#[test]
fn a_guid_kick_of_the_leader_answers_not_leader() {
    use wow_world_messages::vanilla::{PartyOperation, PartyResult};
    let realm = std::sync::Arc::new(InMemoryStore {
        is_realm: true,
        ..Default::default()
    });
    {
        let mut p = realm.party.lock().unwrap();
        p.groups.push((5, VIM, 3, 2, 0));
        p.members.push((5, VIM));
        p.members.push((5, GINGER));
        p.raids.push(5);
        p.slots.insert(GINGER, RaidSlot::new(0, true).unwrap());
    }
    let (mut client, mut c_enc, mut c_dec, server) = realm_session(&realm);

    let result = kick_by_guid(&mut client, &mut c_enc, &mut c_dec, VIM);

    assert_eq!(result.operation, PartyOperation::Leave);
    assert_eq!(result.member, "");
    assert_eq!(result.result, PartyResult::NotLeader);
    assert_eq!(realm.party.lock().unwrap().member_guids(5), [VIM, GINGER]);
    drop(client);
    let _ = server.join();
}

/// **AC: set leader and set Assistant reach the party authority as the session's own character,
/// and neither answers the client, whatever the outcome** (cm:GroupHandler.cpp:344-362, 527-545).
/// The last set leader is refused, because Ginger no longer leads; the barrier's answer is still
/// the first packet.
#[test]
fn set_leader_and_set_assistant_run_as_the_session_and_answer_nothing() {
    use wow_world_messages::vanilla::{CMSG_GROUP_ASSISTANT_LEADER, CMSG_GROUP_SET_LEADER};
    let realm = std::sync::Arc::new(InMemoryStore {
        is_realm: true,
        ..Default::default()
    });
    {
        let mut p = realm.party.lock().unwrap();
        p.groups.push((5, GINGER, 3, 2, 0));
        p.members.push((5, GINGER));
        p.members.push((5, VIM));
        p.raids.push(5);
    }
    let (mut client, mut c_enc, mut c_dec, server) = realm_session(&realm);

    CMSG_GROUP_ASSISTANT_LEADER {
        guid: Guid::new(VIM),
        set_assistant: true,
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..2 {
        CMSG_GROUP_SET_LEADER {
            guid: Guid::new(VIM),
        }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    }
    let first_answer = barrier(&mut client, &mut c_enc, &mut c_dec);

    assert_eq!(
        first_answer.member, "Nobodyatall",
        "a leadership op answered"
    );
    let state = realm.party.lock().unwrap();
    assert_eq!(
        state.ops[..3],
        [
            (realm_op::SET_ASSISTANT, GINGER, VIM, 1, 0, 0),
            (realm_op::SET_LEADER, GINGER, VIM, 0, 0, 0),
            (realm_op::SET_LEADER, GINGER, VIM, 0, 0, 0),
        ]
    );
    assert_eq!(state.leader_of(5), VIM);
    assert!(state.slots[&VIM].is_assistant());
    drop(state);
    drop(client);
    let _ = server.join();
}

/// A guid kick naming the sender is dropped without an answer, as cmangos drops it
/// (cm:GroupHandler.cpp:255-260). It never reaches the party authority.
#[test]
fn a_guid_kick_of_yourself_answers_nothing() {
    let realm = std::sync::Arc::new(InMemoryStore {
        is_realm: true,
        ..Default::default()
    });
    {
        let mut p = realm.party.lock().unwrap();
        p.groups.push((5, GINGER, 3, 2, 0));
        p.members.push((5, GINGER));
        p.members.push((5, VIM));
    }
    let (mut client, mut c_enc, mut c_dec, server) = realm_session(&realm);

    let first_answer = kick_by_guid(&mut client, &mut c_enc, &mut c_dec, GINGER);

    assert_eq!(first_answer.member, "Nobodyatall", "the self kick answered");
    let state = realm.party.lock().unwrap();
    assert!(state.ops.iter().all(|op| op.0 != realm_op::UNINVITE));
    assert_eq!(state.member_guids(5), [GINGER, VIM]);
    drop(state);
    drop(client);
    let _ = server.join();
}

/// **AC: the leader removes a member by guid.** No name is looked up, so a member no shard can name
/// is still removed, and a kick that ran answers nothing: the next packet is the barrier's.
#[test]
fn a_guid_kick_removes_a_member_no_shard_can_name() {
    let realm = std::sync::Arc::new(InMemoryStore {
        is_realm: true,
        ..Default::default()
    });
    {
        let mut p = realm.party.lock().unwrap();
        p.groups.push((5, GINGER, 3, 2, 0));
        for guid in [GINGER, 404, 405] {
            p.members.push((5, guid));
        }
    }
    let (mut client, mut c_enc, mut c_dec, server) = realm_session(&realm);

    let first_answer = kick_by_guid(&mut client, &mut c_enc, &mut c_dec, 404);

    assert_eq!(
        first_answer.member, "Nobodyatall",
        "the kick itself answered"
    );
    let state = realm.party.lock().unwrap();
    assert_eq!(state.member_guids(5), [GINGER, 405]);
    assert_eq!(
        state.ops.last().copied(),
        Some((realm_op::UNINVITE, GINGER, 404, 0, 0, 0))
    );
    drop(state);
    drop(client);
    let _ = server.join();
}
/// The `realm_group_op` argument slots [`party::Op::ChangeSubgroup`] and [`party::Op::SwapSubgroup`]
/// declare: the mover in `target_guid` and the destination Subgroup in `arg_a` for a move, one
/// member in `target_guid` and the other in `arg_c` for a swap.
#[test]
fn subgroup_ops_reach_realm_core_in_their_declared_argument_slots() {
    use lyracore_shared::group::realm_op;
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();

    party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::ChangeSubgroup {
            target: VIM,
            subgroup: 2,
        },
    )
    .expect("the leader moves Vim");
    party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::SwapSubgroup {
            first: GINGER,
            second: VIM,
        },
    )
    .expect("the leader swaps with Vim");

    let ops = realm.party.lock().unwrap().ops.clone();
    assert_eq!(
        ops[ops.len() - 2..],
        [
            (realm_op::CHANGE_SUBGROUP, GINGER, VIM, 2, 0, 0),
            (realm_op::SWAP_SUBGROUP, GINGER, GINGER, 0, 0, VIM),
        ]
    );
}

/// **AC 1, 9: the leader moves a member to another Subgroup; every shard mirrors the new slot with
/// its Assistant bit kept, and the Roster Revision advances.**
#[test]
fn a_leader_moves_a_member_to_another_subgroup_and_every_shard_mirrors_the_slot() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    realm
        .party
        .lock()
        .unwrap()
        .slots
        .insert(VIM, RaidSlot::new(0, true).unwrap());
    let revision_before = realm.group_roster(GINGER).unwrap().unwrap().roster_revision;

    let outcome = party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::ChangeSubgroup {
            target: VIM,
            subgroup: 2,
        },
    )
    .unwrap();

    assert_eq!(outcome, PartyOutcome::Ran);
    let authority = realm.group_roster(GINGER).unwrap().unwrap();
    assert!(
        authority.roster_revision > revision_before,
        "a Subgroup move advances the Roster Revision"
    );
    let moved = authority.members.iter().find(|m| m.guid == VIM).unwrap();
    assert_eq!(
        moved.slot,
        RaidSlot::new(2, true).unwrap(),
        "the Assistant bit survives the move"
    );
    for (name, shard) in [("world", &world), ("instances", &instances)] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![authority.clone()],
            "{name} must mirror Vim's new Subgroup"
        );
    }
}

/// Moving into the Subgroup a member already holds pushes no mirror to either shard.
#[test]
fn changing_into_the_same_subgroup_pushes_no_mirror() {
    let (_realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    let mirrors_before = mirror_calls(&calls);

    let outcome = party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::ChangeSubgroup {
            target: VIM,
            subgroup: 0,
        },
    )
    .unwrap();

    assert_eq!(outcome, PartyOutcome::Ran, "Vim already holds Subgroup 0");
    assert_eq!(mirror_calls(&calls), mirrors_before);
}

/// **AC 2: an Assistant may move a member; a plain member may not, and nothing changes.**
#[test]
fn an_assistant_can_move_a_member_a_plain_member_cannot() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).unwrap();
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).unwrap();
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    realm
        .party
        .lock()
        .unwrap()
        .slots
        .insert(VIM, RaidSlot::new(0, true).unwrap());

    let outcome = party::run(
        instances.as_ref(),
        8,
        VIM,
        party::Op::ChangeSubgroup {
            target: GINGER,
            subgroup: 3,
        },
    )
    .unwrap();
    assert_eq!(outcome, PartyOutcome::Ran, "an Assistant may move a member");
    assert_eq!(
        realm
            .group_roster(GINGER)
            .unwrap()
            .unwrap()
            .members
            .iter()
            .find(|m| m.guid == GINGER)
            .unwrap()
            .slot,
        RaidSlot::new(3, false).unwrap()
    );

    let before = realm.group_roster(GINGER).unwrap().unwrap();
    let outcome = party::run(
        world.as_ref(),
        9,
        TRIN,
        party::Op::ChangeSubgroup {
            target: GINGER,
            subgroup: 4,
        },
    )
    .unwrap();
    assert_eq!(outcome, PartyOutcome::Refused(GroupRefusal::NotLeader));
    assert_eq!(
        realm.group_roster(GINGER).unwrap().unwrap(),
        before,
        "a plain member's attempt changes nothing"
    );
}

/// **AC 4: a Raid has only 8 Subgroups, 0 to 7.**
#[test]
fn a_move_to_subgroup_eight_is_refused() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    let before = realm.group_roster(GINGER).unwrap().unwrap();

    let outcome = party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::ChangeSubgroup {
            target: VIM,
            subgroup: 8,
        },
    )
    .unwrap();

    assert_eq!(
        outcome,
        PartyOutcome::Refused(GroupRefusal::InvalidSubgroup)
    );
    assert_eq!(realm.group_roster(GINGER).unwrap().unwrap(), before);
}

/// **AC 5: in a Party, both ops are refused and change nothing.**
#[test]
fn subgroup_ops_in_a_party_are_refused_and_change_nothing() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    let before = realm.group_roster(GINGER).unwrap().unwrap();

    let change = party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::ChangeSubgroup {
            target: VIM,
            subgroup: 1,
        },
    )
    .unwrap();
    let swap = party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::SwapSubgroup {
            first: GINGER,
            second: VIM,
        },
    )
    .unwrap();

    assert_eq!(change, PartyOutcome::Refused(GroupRefusal::NotRaid));
    assert_eq!(swap, PartyOutcome::Refused(GroupRefusal::NotRaid));
    assert_eq!(realm.group_roster(GINGER).unwrap().unwrap(), before);
}

/// A Raid whose Subgroup 0 and Subgroup 1 each hold 5 members: `GINGER` leads Subgroup 0,
/// `VIM` leads Subgroup 1. AC 3 and AC 6 need Subgroups already full, which is a different
/// scenario from a member joining one with room.
fn seed_raid_with_two_full_subgroups(realm: &InMemoryStore) {
    let mut p = realm.party.lock().unwrap();
    let group_id = 20;
    p.next_group_id = group_id;
    p.groups.push((group_id, GINGER, 3, 2, 0));
    for guid in [GINGER, 101, 102, 103, 104] {
        p.members.push((group_id, guid));
    }
    for guid in [VIM, 105, 106, 107, 108] {
        p.members.push((group_id, guid));
        p.slots.insert(guid, RaidSlot::new(1, false).unwrap());
    }
    p.raids.push(group_id);
}

/// **AC 3: a move into a full Subgroup is refused, changes nothing, and pushes no mirror.**
#[test]
fn a_move_into_a_full_subgroup_is_refused() {
    let (realm, world, _instances, calls) = party_topology();
    seed_raid_with_two_full_subgroups(&realm);
    let before = realm.group_roster(GINGER).unwrap().unwrap();
    let mirrors_before = mirror_calls(&calls);

    let outcome = party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::ChangeSubgroup {
            target: GINGER,
            subgroup: 1,
        },
    )
    .unwrap();

    assert_eq!(outcome, PartyOutcome::Refused(GroupRefusal::SubgroupFull));
    assert_eq!(realm.group_roster(GINGER).unwrap().unwrap(), before);
    assert_eq!(mirror_calls(&calls), mirrors_before);
}

/// **AC 6: swapping two members of two full Subgroups needs no capacity Gate, and sends exactly
/// one list per member.**
#[test]
fn swapping_members_of_two_full_subgroups_succeeds_and_sends_one_list_per_member() {
    let (realm, world, instances, _calls) = party_topology();
    seed_raid_with_two_full_subgroups(&realm);
    let events_before = realm.party.lock().unwrap().events.len();

    let outcome = party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::SwapSubgroup {
            first: GINGER,
            second: VIM,
        },
    )
    .unwrap();

    assert_eq!(outcome, PartyOutcome::Ran);
    let authority = realm.group_roster(GINGER).unwrap().unwrap();
    let subgroup_of = |guid| {
        authority
            .members
            .iter()
            .find(|m| m.guid == guid)
            .unwrap()
            .slot
            .subgroup()
    };
    assert_eq!(subgroup_of(GINGER), 1);
    assert_eq!(subgroup_of(VIM), 0);
    let listed = realm.party.lock().unwrap().events[events_before..]
        .iter()
        .filter(|(_, kind)| *kind == lyracore_shared::group::event_kind::LIST)
        .count();
    assert_eq!(listed, 10, "one list per member, not two moves' worth");
    for (name, shard) in [("world", &world), ("instances", &instances)] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![authority.clone()],
            "{name} must mirror both swapped Subgroups"
        );
    }
}

/// **AC 7: swapping two members of one Subgroup succeeds and pushes no mirror.**
#[test]
fn swapping_members_of_one_subgroup_pushes_no_mirror() {
    let (_realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    let mirrors_before = mirror_calls(&calls);

    let outcome = party::run(
        world.as_ref(),
        7,
        GINGER,
        party::Op::SwapSubgroup {
            first: GINGER,
            second: VIM,
        },
    )
    .unwrap();

    assert_eq!(
        outcome,
        PartyOutcome::Ran,
        "cmangos answers a same-Subgroup swap as success"
    );
    assert_eq!(mirror_calls(&calls), mirrors_before);
}

/// **AC 8: a name outside the actor's OWN roster resolves to nothing, even when a Character with
/// that name exists elsewhere on the realm.** cmangos matches Change/Swap Subgroup names against
/// the member list alone (cm:GroupHandler.cpp:919-936), never realm-wide.
#[test]
fn resolve_roster_member_by_name_ignores_a_namesake_outside_the_roster() {
    let (_realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);

    let outside = party::resolve_roster_member_by_name(world.as_ref(), GINGER, "Dormant").unwrap();
    assert_eq!(
        outside, None,
        "Dormant exists on `world` but never joined Ginger's party"
    );

    let inside = party::resolve_roster_member_by_name(world.as_ref(), GINGER, "vim").unwrap();
    assert_eq!(
        inside,
        Some(VIM),
        "a roster member resolves, case-insensitively"
    );
}

// ---- Group Broadcasts ----

/// `realm_group_op`'s slots after the actor: `(op, target_guid, arg_a, arg_b, arg_c)`.
type RealmArgs = (u8, u64, u8, u8, u64);

/// One packet as the client receives it: `(opcode, body)`.
type Wire = (u16, Vec<u8>);

/// Every Group Broadcast op and the `realm_group_op` slots it must fill: the op byte, the target
/// slot, `arg_a`, `arg_b` and `arg_c`. Pinned by hand against `realm_op`'s slot table. The ping's
/// floats go as bit patterns: 0.5 is `0x3F00_0000` and -0.25 is `0xBE80_0000`.
fn broadcast_ops() -> [(party::Op, RealmArgs); 6] {
    [
        (party::Op::ReadyCheckStart, (11, 0, 0, 0, 0)),
        (party::Op::ReadyCheckAnswer(1), (12, 0, 1, 0, 0)),
        (
            party::Op::TargetIcon {
                icon: 7,
                target: 900,
            },
            (13, 900, 7, 0, 0),
        ),
        (
            party::Op::TargetIcon {
                icon: 0xFF,
                target: 0,
            },
            (13, 0, 0xFF, 0, 0),
        ),
        (
            party::Op::MinimapPing { x: 0.5, y: -0.25 },
            (14, 0x3F00_0000, 0, 0, 0xBE80_0000),
        ),
        (
            party::Op::RandomRoll { min: 1, max: 100 },
            (15, 1, 0, 0, 100),
        ),
    ]
}

/// **AC: none of the broadcast ops advances the Roster Revision or pushes a mirror.** On a sharded
/// Realm each one is one `realm_group_op` call on Realm-core and nothing else: no roster read, no
/// loot-roll flush, no World Shard write.
#[test]
fn a_group_broadcast_runs_once_on_realm_core_and_pushes_no_mirror() {
    let (realm, world, instances, calls) = party_topology();
    form_split_party(&world, &instances);
    for (op, (code, target, arg_a, arg_b, arg_c)) in broadcast_ops() {
        let calls_before = calls.lock().unwrap().len();
        let roster_reads_before = realm
            .group_roster_reads
            .load(std::sync::atomic::Ordering::SeqCst);

        let outcome = party::run(world.as_ref(), 7, GINGER, op).unwrap();

        assert_eq!(outcome, PartyOutcome::Ran, "{op:?}");
        assert_eq!(
            calls.lock().unwrap()[calls_before..],
            [("lyracore-realm".to_string(), "realm_group_op".to_string())],
            "{op:?} must be one call on the party authority and no mirror push"
        );
        assert_eq!(
            realm
                .group_roster_reads
                .load(std::sync::atomic::Ordering::SeqCst),
            roster_reads_before,
            "{op:?} read a roster it does not need"
        );
        assert_eq!(
            realm.party.lock().unwrap().ops.last().copied(),
            Some((code, GINGER, target, arg_a, arg_b, arg_c)),
            "{op:?}"
        );
    }
}

/// **AC: a single-database Gateway runs all five on its only shard.** No player-facing reducer
/// exists for them, so the home shard's `realm_group_op` runs each one.
#[test]
fn an_unsharded_gateway_runs_every_group_broadcast_on_its_own_shard() {
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        ..Default::default()
    });
    {
        let mut p = store.party.lock().unwrap();
        p.groups.push((5, GINGER, 3, 2, 0));
        p.members.push((5, GINGER));
        p.members.push((5, VIM));
    }
    for (op, (code, target, arg_a, arg_b, arg_c)) in broadcast_ops() {
        calls.lock().unwrap().clear();

        let outcome = party::run(store.as_ref(), 7, GINGER, op).unwrap();

        assert_eq!(outcome, PartyOutcome::Ran, "{op:?}");
        assert_eq!(
            calls.lock().unwrap().clone(),
            vec![("world".to_string(), "realm_group_op".to_string())],
            "{op:?}"
        );
        assert_eq!(
            store.party.lock().unwrap().ops.last().copied(),
            Some((code, GINGER, target, arg_a, arg_b, arg_c)),
            "{op:?}"
        );
    }
}

/// Send one hand-written client frame over the encrypted session.
fn send_client_frame(
    client: &mut UnixStream,
    encrypter: &mut EncrypterHalf,
    opcode: u32,
    body: &[u8],
) {
    use std::io::Write;
    let size = u16::try_from(body.len() + 4).unwrap();
    client
        .write_all(&encrypter.encrypt_client_header(size, opcode))
        .unwrap();
    client.write_all(body).unwrap();
}

/// A barrier: the reply to an invite of an unknown name must be the next packet, so nothing the
/// frames before it sent reached the client.
fn assert_nothing_sent_before_the_barrier(
    client: &mut UnixStream,
    encrypter: &mut EncrypterHalf,
    decrypter: &mut DecrypterHalf,
) {
    wow_world_messages::vanilla::CMSG_GROUP_INVITE {
        name: "Nobodyatall".into(),
    }
    .write_encrypted_client(&mut *client, encrypter)
    .unwrap();
    match ServerOpcodeMessage::read_encrypted(client, decrypter).unwrap() {
        ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(r) => assert_eq!(r.member, "Nobodyatall"),
        other => panic!("a Group Broadcast answered the client directly: {other}"),
    }
}

/// Client bytes in, `realm_group_op` slots out, for every opcode (cmangos: GroupHandler.cpp
/// 395-415 ping, 417-442 roll, 444-471 icons, 547-583 ready check). The actor's own packets ride
/// the group event relay, so the session answers none of them directly.
#[test]
fn every_group_broadcast_opcode_reaches_the_party_authority_with_its_client_values() {
    let s = quest_store();
    {
        let mut p = s.party.lock().unwrap();
        p.groups.push((5, 1, 3, 2, 0));
        p.members.push((5, 1));
        p.members.push((5, 2));
    }
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let skull_on: Vec<u8> = [&[0x07][..], &0xF130_0000_0000_0042u64.to_le_bytes()].concat();
    let frames: [(u32, Vec<u8>); 6] = [
        // MSG_RAID_READY_CHECK, start.
        (0x0322, vec![]),
        // MSG_RAID_READY_CHECK, answer "ready".
        (0x0322, vec![0x01]),
        // MSG_RAID_TARGET_UPDATE, skull on a creature.
        (0x0321, skull_on),
        // MSG_RAID_TARGET_UPDATE, the list request.
        (0x0321, vec![0xFF]),
        // MSG_MINIMAP_PING at (0.5, -0.25).
        (0x01D5, vec![0, 0, 0, 0x3F, 0, 0, 0x80, 0xBE]),
        // MSG_RANDOM_ROLL with inverted bounds, min 100 and max 1.
        (0x01FB, vec![100, 0, 0, 0, 1, 0, 0, 0]),
    ];
    for (opcode, body) in &frames {
        send_client_frame(&mut client, &mut c_enc, *opcode, body);
    }

    assert_nothing_sent_before_the_barrier(&mut client, &mut c_enc, &mut c_dec);

    assert_eq!(
        store.party.lock().unwrap().ops,
        vec![
            (11, 1, 0, 0, 0, 0),
            (12, 1, 0, 1, 0, 0),
            (13, 1, 0xF130_0000_0000_0042, 7, 0, 0),
            (13, 1, 0, 0xFF, 0, 0),
            (14, 1, 0x3F00_0000, 0, 0, 0xBE80_0000),
            // The Module normalizes the bounds; the Gateway passes what the client sent.
            (15, 1, 100, 0, 0, 1),
        ]
    );
    drop(client);
    let _ = server.join();
}

/// cmangos answers a refused Ready Check, mark or ping with silence. A ninth icon index reaches
/// the Module, which refuses it.
#[test]
fn a_refused_group_broadcast_sends_nothing() {
    let store = std::sync::Arc::new(quest_store());
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    let ninth_icon: Vec<u8> = [&[0x08][..], &900u64.to_le_bytes()].concat();
    send_client_frame(&mut client, &mut c_enc, 0x0322, &[]);
    send_client_frame(&mut client, &mut c_enc, 0x0321, &ninth_icon);
    send_client_frame(&mut client, &mut c_enc, 0x01D5, &[0; 8]);

    assert_nothing_sent_before_the_barrier(&mut client, &mut c_enc, &mut c_dec);

    let ops: Vec<_> = store.party.lock().unwrap().ops.clone();
    assert_eq!(
        ops.iter().map(|op| (op.0, op.3)).collect::<Vec<_>>(),
        [(11, 0), (13, 8), (14, 0)],
        "each opcode reached the authority, which refused it"
    );
    drop(client);
    let _ = server.join();
}

/// One sent packet as `(opcode, body)`, the way the client receives it.
fn wire(packet: &Outbound) -> Wire {
    match packet {
        Outbound::One(message) => {
            let mut framed = Vec::new();
            message.write_unencrypted_server(&mut framed).unwrap();
            let opcode = u16::from_le_bytes([framed[2], framed[3]]);
            (opcode, framed.split_off(4))
        }
        Outbound::Raw { opcode, body } => (*opcode, body.clone()),
        _ => panic!("expected one packet"),
    }
}

fn group_event(kind: u8, other_guid: u64, payload: &str) -> crate::stdb::bindings::GroupEvent {
    crate::stdb::bindings::GroupEvent {
        id: 1,
        recipient_identity: spacetimedb_sdk::Identity::ZERO,
        kind,
        other_guid,
        other_name: String::new(),
        created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        payload: payload.to_string(),
        recipient_guid: GINGER,
    }
}

/// **Each Group Broadcast kind renders its vanilla packet**, pinned byte for byte against the
/// cmangos writers: ready check (GroupHandler.cpp:562-563, 576-579), the partial icon update
/// (Group.cpp:595-600), the full icon list (Group.cpp:648-666), the ping (GroupHandler.cpp:410-413)
/// and the roll (GroupHandler.cpp:434-438).
#[test]
fn the_relay_renders_each_group_broadcast_kind() {
    use lyracore_shared::group::event_kind;
    let (realm, _world, _instances, _calls) = party_topology();
    let skull = 900u64.to_le_bytes();
    let star = 901u64.to_le_bytes();
    let none = 0u64.to_le_bytes();
    let vim = VIM.to_le_bytes();
    // cm:Group.cpp:648-666: the update type, then only the held icons, in icon order.
    let full_list: Vec<u8> = [
        &[0x01][..], // update type: full
        &[0x00],     // star
        &star,
        &[0x07], // skull
        &skull,
    ]
    .concat();
    let cases: [(u8, &str, Wire); 7] = [
        (event_kind::READY_CHECK, "", (0x0322, vec![])),
        (
            event_kind::READY_CHECK_ANSWER,
            "1",
            (0x0322, [&vim[..], &[0x01]].concat()),
        ),
        (
            event_kind::TARGET_ICON_UPDATE,
            "7,900",
            (0x0321, [&[0x00, 0x07][..], &skull].concat()),
        ),
        (
            event_kind::TARGET_ICON_UPDATE,
            "7,0",
            (0x0321, [&[0x00, 0x07][..], &none].concat()),
        ),
        (
            event_kind::TARGET_ICON_LIST,
            "7,900;0,901",
            (0x0321, full_list),
        ),
        (
            event_kind::MINIMAP_PING,
            "1056964608,3196059648",
            (
                0x01D5,
                [&vim[..], &[0, 0, 0, 0x3F, 0, 0, 0x80, 0xBE]].concat(),
            ),
        ),
        (
            event_kind::RANDOM_ROLL,
            "1,100,42",
            (
                0x01FB,
                [&[1, 0, 0, 0, 100, 0, 0, 0, 42, 0, 0, 0][..], &vim].concat(),
            ),
        ),
    ];
    for (kind, payload, expected) in cases {
        let row = group_event(kind, VIM, payload);

        let packets =
            crate::stdb::subscriptions::group_event_outbound(realm.as_ref(), GINGER, &row);

        let sent: Vec<_> = packets.iter().map(wire).collect();
        assert_eq!(sent, [expected], "kind {kind} payload {payload:?}");
    }
}

/// A payload that does not decode reaches no client.
#[test]
fn the_relay_drops_a_group_broadcast_it_cannot_decode() {
    use lyracore_shared::group::event_kind;
    let (realm, _world, _instances, _calls) = party_topology();
    for (kind, payload) in [
        (event_kind::READY_CHECK_ANSWER, "ready"),
        (event_kind::TARGET_ICON_UPDATE, "8,900"),
        (event_kind::TARGET_ICON_LIST, "7,900;7,901"),
        (event_kind::MINIMAP_PING, "1"),
        (event_kind::RANDOM_ROLL, "1,100,101"),
    ] {
        let row = group_event(kind, VIM, payload);
        assert!(
            crate::stdb::subscriptions::group_event_outbound(realm.as_ref(), GINGER, &row)
                .is_empty(),
            "kind {kind} payload {payload:?}"
        );
    }
}

/// **AC: in a Party the icons survive a list.** The Party client clears its marks on every
/// `SMSG_GROUP_LIST`, so the LIST job sends the list and then the icons, in that order
/// (vm:Group.cpp:1343-1360, 1403-1408). A list without icons sends the list alone.
#[test]
fn a_party_list_with_target_icons_sends_the_list_then_the_icons() {
    use lyracore_shared::group::{event_kind, TargetIcon};
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    *realm.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    let mut roster = realm.group_roster(GINGER).unwrap().unwrap().list_payload();
    roster.target_icons = vec![TargetIcon {
        icon: 7,
        target_guid: 900,
    }];
    let row = group_event(event_kind::LIST, 0, &roster.encode());

    let packets = crate::stdb::subscriptions::group_event_outbound(realm.as_ref(), GINGER, &row);

    let mut packets = packets.into_iter();
    let (Some(list), Some(icons), None) = (packets.next(), packets.next(), packets.next()) else {
        panic!("expected the list and then the icons")
    };
    assert_eq!(group_list(list).members[0].guid.guid(), VIM);
    let (opcode, body) = wire(&icons);
    assert_eq!(opcode, 0x0321);
    assert_eq!(
        body,
        [&[0x01, 0x07][..], &900u64.to_le_bytes()].concat(),
        "the full list with skull on 900"
    );

    roster.target_icons.clear();
    let row = group_event(event_kind::LIST, 0, &roster.encode());
    assert_eq!(
        crate::stdb::subscriptions::group_event_outbound(realm.as_ref(), GINGER, &row).len(),
        1
    );
}

/// **A Party member who zones in gets its Target Icons again.** World entry sends a list, and the
/// Party client clears its marks on every list, so the Gateway asks the party authority for the full
/// icon list after the list. A Raid keeps its marks and asks for nothing.
#[test]
fn world_entry_asks_for_a_partys_target_icons_after_the_list() {
    let (realm, world, instances, _calls) = party_topology();
    form_split_party(&world, &instances);
    let (tx, rx) = crate::world::SessionTx::with_depth(0);

    party::on_world_entry(&tx, instances.as_ref(), VIM).expect("world entry");

    group_list(rx.try_recv().expect("the list goes out first"));
    assert_eq!(
        realm.party.lock().unwrap().ops.last().copied(),
        Some((realm_op::TARGET_ICON, VIM, 0, 0xFF, 0, 0)),
        "then the list request, whose answer rides the relay behind the list"
    );

    party::run(world.as_ref(), 7, GINGER, party::Op::RaidConvert).unwrap();
    let ops_before = realm.party.lock().unwrap().ops.len();
    let (tx, _rx) = crate::world::SessionTx::with_depth(0);
    party::on_world_entry(&tx, instances.as_ref(), VIM).expect("world entry");
    assert_eq!(
        realm.party.lock().unwrap().ops.len(),
        ops_before,
        "a Raid keeps its marks through a list"
    );
}

/// A lost broadcast must not end the session: nothing waits on a ping, and the roster is unchanged.
#[test]
fn a_group_broadcast_lost_in_transport_keeps_the_session() {
    let s = InMemoryStore {
        group_broadcast_error: true,
        ..quest_store()
    };
    {
        let mut p = s.party.lock().unwrap();
        p.groups.push((5, 1, 3, 2, 0));
        p.members.push((5, 1));
        p.members.push((5, 2));
    }
    let store = std::sync::Arc::new(s);
    let (mut client, mut c_enc, mut c_dec, server) = enter_world(store.clone(), 1);
    send_client_frame(&mut client, &mut c_enc, 0x0322, &[]);
    send_client_frame(&mut client, &mut c_enc, 0x01D5, &[0; 8]);
    send_client_frame(&mut client, &mut c_enc, 0x01FB, &[1, 0, 0, 0, 100, 0, 0, 0]);

    assert_nothing_sent_before_the_barrier(&mut client, &mut c_enc, &mut c_dec);
    drop(client);
    let _ = server.join();
}

/// Whether `shard`'s mirror of Group `group_id` lists `member`.
fn mirror_lists(shard: &InMemoryStore, group_id: u64, member: u64) -> bool {
    shard
        .mirror
        .lock()
        .unwrap()
        .iter()
        .find(|roster| roster.group_id == group_id)
        .is_some_and(|roster| roster.has_member(member))
}

/// A three-member party with Vim inside the dungeon on `instances`. From here on Realm-core's
/// Coordinator cache lags every call-pipe commit.
fn lagging_three_member_party() -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    u64,
) {
    let (realm, world, instances, _) = party_topology();
    form_split_party(&world, &instances);
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(TRIN)).unwrap();
    party::run(world.as_ref(), 9, TRIN, party::Op::Accept).unwrap();
    let group_id = realm.group_roster(VIM).unwrap().unwrap().group_id;
    realm
        .cache_lags
        .store(true, std::sync::atomic::Ordering::SeqCst);
    (world, instances, group_id)
}

/// The Instance Pool starts and cancels an Instance Removal from its mirror. A World Session's
/// party op returns only after the Coordinator cache holds the commit, so a cache that lags the
/// commit still pushes the leave and the rejoin, and the Pool never sends home a Character whom
/// Realm-core shows back in its Group.
#[test]
fn a_leave_and_a_rejoin_reach_the_instance_pool_while_the_realm_cache_lags() {
    let (world, instances, group_id) = lagging_three_member_party();

    party::run(instances.as_ref(), 8, VIM, party::Op::Leave).unwrap();
    assert!(
        !mirror_lists(&instances, group_id, VIM),
        "the Pool learns that Vim left"
    );

    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(VIM)).unwrap();
    party::run(instances.as_ref(), 8, VIM, party::Op::Accept).unwrap();
    assert!(
        mirror_lists(&instances, group_id, VIM),
        "the Pool learns that Vim is back"
    );
}

/// A bot leader that leaves a two-member party disbands it. The player in the Group's dungeon must
/// lose the Group on the Instance Pool, or its Instance Removal never starts. The bot intent runs
/// on its own thread, so it takes the same visibility receipt a World Session does.
#[test]
fn a_bot_leave_that_disbands_reaches_the_instance_pool_while_the_realm_cache_lags() {
    let (realm, world, instances, _) = party_topology();
    party::run_bot_invite(world.as_ref(), BOT, VIM).unwrap();
    party::run(instances.as_ref(), 8, VIM, party::Op::Accept).unwrap();
    let group_id = realm.group_roster(VIM).unwrap().unwrap().group_id;
    assert!(mirror_lists(&instances, group_id, VIM));
    realm
        .cache_lags
        .store(true, std::sync::atomic::Ordering::SeqCst);

    assert_eq!(
        party::run_bot_leave(world.as_ref(), BOT).unwrap(),
        PartyOutcome::Ran
    );
    assert!(
        !instances
            .mirror
            .lock()
            .unwrap()
            .iter()
            .any(|roster| roster.group_id == group_id),
        "the Pool drops the disbanded Group"
    );
}

/// A membership op retries a failed mirror push, so one dropped push cannot strand a rejoin.
#[test]
fn a_rejoin_retries_a_failed_mirror_push() {
    let (world, instances, group_id) = lagging_three_member_party();
    party::run(instances.as_ref(), 8, VIM, party::Op::Leave).unwrap();
    party::run(world.as_ref(), 7, GINGER, party::Op::Invite(VIM)).unwrap();

    instances
        .mirror_failures
        .store(2, std::sync::atomic::Ordering::SeqCst);
    party::run(instances.as_ref(), 8, VIM, party::Op::Accept).unwrap();
    assert!(mirror_lists(&instances, group_id, VIM));
}
