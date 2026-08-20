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
        offline_guids: vec![DORMANT, BOT],
        ..Default::default()
    });
    let instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![character(VIM, "Vim"), character(FAR_BOT, "Farbotty")],
        live_guids: vec![VIM, FAR_BOT],
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
        party::resolve_by_name(world.as_ref(), "Vim").unwrap(),
        Some(VIM)
    );
    // …and it still resolves a name on the asking shard itself, from either side.
    assert_eq!(
        party::resolve_by_name(instances.as_ref(), "Ginger").unwrap(),
        Some(GINGER)
    );
    assert_eq!(
        party::resolve_by_name(world.as_ref(), "Nobody").unwrap(),
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
        party_state.roster(group_id).unwrap().members,
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
        !ops.iter().any(|(_, call)| call == "group_invite" || call == "group_accept"),
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

    let ginger_view = party::render_list(world.as_ref(), GINGER, &roster);
    let ServerOpcodeMessage::SMSG_GROUP_LIST(list) = ginger_view else {
        panic!("expected GROUP_LIST")
    };
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

    let vim_view = party::render_list(instances.as_ref(), VIM, &roster);
    let ServerOpcodeMessage::SMSG_GROUP_LIST(list) = vim_view else {
        panic!("expected GROUP_LIST")
    };
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
    assert_eq!(remaining.members, vec![GINGER, TRIN]);
    for (name, shard) in [("world", &world), ("instances", &instances)] {
        assert_eq!(
            shard.mirror.lock().unwrap().clone(),
            vec![remaining.clone()],
            "{name} still lists the member who left — the mirror push must cover the group the \
             actor was in BEFORE the op, not only the one they are in after it"
        );
    }
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
    let Outbound::One(ServerOpcodeMessage::SMSG_GROUP_LIST(list)) =
        rx.try_recv().expect("a GROUP_LIST is sent")
    else {
        panic!("expected SMSG_GROUP_LIST")
    };
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
            .map(|r| r.members),
        Some(vec![GINGER, TRIN]),
        "and the members who are STILL in that party must survive the repair — clearing the group \
         wholesale would be the opposite defect"
    );
}

/// The two gates realm-core cannot run for itself. Both are refused BEFORE the authority is touched,
/// and both carry the module's own error strings so `social::party_result_for` classifies them
/// identically on either plane.
#[test]
fn an_invite_to_a_missing_or_offline_target_never_reaches_realm_core() {
    let (realm, world, _instances, _calls) = party_topology();

    let err = party::run(world.as_ref(), 7, GINGER, party::Op::Invite(DORMANT))
        .expect_err("an offline target is refused");
    assert!(err.to_string().contains("player not online"), "got {err}");

    let err = party::run(world.as_ref(), 7, GINGER, party::Op::Invite(999))
        .expect_err("an unknown target is refused");
    assert!(err.to_string().contains("no such player"), "got {err}");

    assert!(
        realm.party.lock().unwrap().ops.is_empty(),
        "a refused invite must not reach the authority — the gate runs in the gateway precisely \
         because realm-core has neither characters nor live entities to run it against"
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
        Some((lyracore_shared::group::realm_op::INVITE, GINGER, BOT, 0, 0)),
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
        party_state.roster(group_id).unwrap().members,
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
        vec![(realm_op::INVITE, GINGER, BOT, 0, 0), (realm_op::ACCEPT, BOT, 0, 0, 0)],
        "the accept must run on realm-core with the BOT as the actor — never the inviter, and never 0"
    );
}

/// The predicate the answer hangs on, over every shape a guid can be in. "Live entity AND no
/// session" is the whole of it, and each half is load-bearing: without the entity a `DORMANT` player
/// (offline, logged out, no entity) would read as a bot, and without the session flag every real
/// player in the world would.
#[test]
fn only_a_live_entity_without_a_session_reads_as_a_playerbot() {
    let (_realm, world, _instances, _calls) = party_topology();
    assert!(
        party::session_less_in_world(world.as_ref(), BOT),
        "live + no session = a playerbot"
    );
    assert!(
        party::session_less_in_world(world.as_ref(), FAR_BOT),
        "…on whichever connected shard it stands, like every other read in this module"
    );
    assert!(
        !party::session_less_in_world(world.as_ref(), TRIN),
        "a live player with a session has a client of their own to answer with"
    );
    assert!(
        !party::session_less_in_world(world.as_ref(), DORMANT),
        "offline with NO live entity is a logged-out PLAYER, not a bot — never answer for them"
    );
    assert!(
        !party::session_less_in_world(world.as_ref(), 999),
        "and an unknown guid is nobody"
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
    assert!(
        !party::session_less_in_world(far.as_ref(), SEEDED),
        "the session flag must come from the shard that HOLDS the live entity — a stale row on the \
         asking shard is not a licence to answer for somebody"
    );
    // …and end to end: an inviter on the far shard must leave that player's dialog alone.
    party::run(far.as_ref(), 9, VIM, party::Op::Invite(SEEDED)).expect("the invite itself is fine");
    let state = realm.party.lock().unwrap();
    assert_eq!(
        state.ops.clone(),
        vec![(lyracore_shared::group::realm_op::INVITE, VIM, SEEDED, 0, 0)],
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
            .map(|r| r.members),
        Some(vec![GINGER, BOT]),
        "the bot's own shard must already hold the roster — it is what `group_leader_entity` reads"
    );
    assert_eq!(
        world.group_roster_by_id(group_id).unwrap().map(|r| r.leader_guid),
        Some(GINGER),
        "and the leader in that mirror is the PLAYER: the follow-the-leader pass resolves its anchor \
         from `game_group.leader_guid` and never asks whether the leader is a bot"
    );
    assert_eq!(
        instances
            .group_roster_by_id(group_id)
            .unwrap()
            .map(|r| r.members),
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
        state.roster(group_id).unwrap().members,
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
        vec![(realm_op::INVITE, GINGER, TRIN, 0, 0)],
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
            (realm_op::INVITE, GINGER, BOT, 0, 0),
            (realm_op::ACCEPT, BOT, 0, 0, 0),
            // …and the decline is the bot's own too, not the inviter's.
            (realm_op::DECLINE, BOT, 0, 0, 0),
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
        !calls.lock().unwrap().iter().any(|(shard, call)| shard != "lyracore-realm"
            && (call == "group_accept" || call == "group_invite")),
        "the answer must never run on a world shard's own party tables — that would write membership \
         the authority does not have. Calls were {:?}",
        calls.lock().unwrap()
    );
}

/// `realm_group_op` packs six ops into five argument slots, and the packing is a WIRE contract with
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
    party::run(world.as_ref(), 7, GINGER, party::Op::Uninvite(VIM)).expect("kick");

    assert_eq!(
        realm.party.lock().unwrap().ops.clone(),
        vec![
            // INVITE: the target rides `target_guid`, nothing else is used.
            (realm_op::INVITE, GINGER, VIM, 0, 0),
            // ACCEPT: the actor alone.
            (realm_op::ACCEPT, VIM, 0, 0, 0),
            // LOOT_METHOD: setting in arg_a, MASTER in target_guid, threshold in arg_b —
            // CMSG_LOOT_METHOD's own field order.
            (realm_op::LOOT_METHOD, GINGER, VIM, 2, 4),
            // UNINVITE: the kicked member rides `target_guid`.
            (realm_op::UNINVITE, GINGER, VIM, 0, 0),
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
    // All SIX, not just the invite: `realm_group_op` takes the actor's guid as an ARGUMENT, so the
    // dispatch's choice of guid IS the authorization for every one of them, and the survivor the
    // author found (`0` instead of the session's guid) is a mutation each arm admits independently.
    // Pinning only the invite leaves the other five free to be attributed to anybody — verified by
    // mutation: passing the KICKED player's guid as the actor of `CMSG_GROUP_UNINVITE` left all 408
    // tests green.
    use wow_world_messages::vanilla::{
        CMSG_GROUP_ACCEPT, CMSG_GROUP_DECLINE, CMSG_GROUP_DISBAND, CMSG_GROUP_INVITE,
        CMSG_GROUP_UNINVITE, CMSG_LOOT_METHOD,
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
    CMSG_GROUP_DISBAND {}
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    CMSG_GROUP_UNINVITE { name: "vim".into() }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // The BARRIER: an invite for a name no shard can resolve never reaches `party::run`, so it adds
    // no op — but it always answers `SMSG_PARTY_COMMAND_RESULT`, and the dispatch is sequential on
    // one thread, so seeing ITS reply proves all six above have been dispatched. (No `join`: the
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
                break
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
            (realm_op::INVITE, GINGER, VIM, 0, 0),
            (realm_op::ACCEPT, GINGER, 0, 0, 0),
            (realm_op::DECLINE, GINGER, 0, 0, 0),
            // CMSG_LOOT_METHOD's own field order: setting in arg_a, MASTER in target_guid,
            // threshold in arg_b.
            (realm_op::LOOT_METHOD, GINGER, VIM, 2, 4),
            (realm_op::LEAVE, GINGER, 0, 0, 0),
            (realm_op::UNINVITE, GINGER, VIM, 0, 0),
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
        party_state.roster(group_id).unwrap().members,
        vec![BOT, FAR_BOT],
        "the inviting bot leads, the session-less target auto-accepts through `answer_for_session_less`"
    );
    assert_eq!(
        party_state.ops.clone(),
        vec![
            (realm_op::INVITE, BOT, FAR_BOT, 0, 0),
            (realm_op::ACCEPT, FAR_BOT, 0, 0, 0)
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
        !log.iter().any(|(_, call)| call == "group_invite" || call == "group_accept"),
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
    party::sync_mirrors(world.as_ref(), realm.as_ref(), 0, Some(group_id));

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
        members: vec![BOT, TRIN],
        ..Default::default()
    };
    world
        .sync_group_mirror(&phantom)
        .expect("simulate the bug's pre-fix shard-local-only write");
    assert_eq!(
        world.mirror.lock().unwrap().clone(),
        vec![phantom],
        "precondition"
    );
    assert!(
        realm
            .group_roster_by_id(phantom_group_id)
            .unwrap()
            .is_none(),
        "precondition: realm-core has never heard of this group — the whole bug"
    );

    party::sync_mirrors(world.as_ref(), realm.as_ref(), 0, Some(phantom_group_id));

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

    let err =
        party::run_bot_invite(world.as_ref(), BOT, DORMANT).expect_err("offline target refused");
    assert!(err.to_string().contains("player not online"), "got {err}");

    let err = party::run_bot_invite(world.as_ref(), BOT, 999).expect_err("unknown target refused");
    assert!(err.to_string().contains("no such player"), "got {err}");

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
        log.iter().any(|(shard, call)| shard == "world" && call == "realm_group_op"),
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
        Some((realm_op::INVITE, BOT, TRIN, 0, 0)),
        "the invite must be recorded with the bot as inviter"
    );
}
