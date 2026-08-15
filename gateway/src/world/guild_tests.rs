//! Realm-wide guild state — the routing tests.
//!
//! What EXECUTES here is production `world::guild`, against the same in-memory multi-database
//! topology the party and transfer tests use. What the fakes stand in for is named at each seam:
//! `FakeGuild` models realm-core's authority (module reducer bodies cannot run in a gateway test —
//! there is no `ReducerContext`), and each shard's `guild_columns` is exactly what
//! `sync_guild_membership` wrote there.
//!
//! A child module of `world::tests` so it can reach `InMemoryStore` without widening anything.

use super::*;

use super::party_tests::{character, GINGER, VIM};

/// A live guild topology: realm-core plus the two world shards, wired the way the production
/// gateway wires them — every shard's `realm_store()` is the realm handle, and `world_stores()` is
/// every connected world shard (including the asking one, exactly as `Coordinator::all_shards`
/// answers). Ginger is resident on `world`, Vim on `instances`.
fn guild_topology() -> (
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
    let instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![character(VIM, "Vim")],
        live_guids: vec![VIM],
        ..Default::default()
    });
    for shard in [&world, &instances] {
        *shard.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    }
    (realm, world, instances, calls)
}

fn create(store: &InMemoryStore, self_guid: u64, name: &str) -> Result<()> {
    guild::run(store, 7, self_guid, guild::Op::Create(name.into()))
}

/// The op runs on the AUTHORITY, not on the shard the founder happens to stand on — which is the
/// whole reason guild state is realm-core's. A shard-local guild would split the moment its members
/// stood on two databases.
#[test]
fn a_create_runs_on_realm_core_and_never_on_the_founders_own_shard() {
    let (realm, world, instances, calls) = guild_topology();

    create(world.as_ref(), GINGER, "The Silver Hand").expect("the create reaches the authority");

    assert_eq!(
        realm.guild.lock().unwrap().ops.as_slice(),
        &[(
            lyracore_shared::guild::realm_op::CREATE,
            GINGER,
            0,
            0,
            "The Silver Hand".to_string()
        )],
        "realm-core ran the create, with the founder's guid in the actor slot"
    );
    assert!(
        world.guild.lock().unwrap().guilds.is_empty()
            && instances.guild.lock().unwrap().guilds.is_empty(),
        "no world shard may hold a guild row of its own"
    );
    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter()
            .any(|(shard, call)| shard == "lyracore-realm" && call == "realm_guild_op"),
        "the op must land on the realm database"
    );
    assert!(
        !log.iter().any(|(_, call)| call == "create_guild"),
        "the single-database reducer must not be called on a sharded gateway"
    );
}

/// D1 made visible: the only thing a world shard learns about a guild is the character's own guild
/// id and rank, and it learns it from the authority after the op — on EVERY connected shard, so a
/// member who walks into a dungeon arrives with the right columns already there.
#[test]
fn a_create_pushes_the_founders_own_guild_columns_onto_every_connected_shard() {
    let (_realm, world, instances, _calls) = guild_topology();

    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();

    for shard in [&world, &instances] {
        assert_eq!(
            shard.guild_columns.lock().unwrap().as_slice(),
            &[(GINGER, 1, 0)],
            "shard {} must carry the founder's guild id and rank 0",
            shard.shard
        );
    }
}

/// The push is best-effort: the authority has already committed, so an unreachable shard must not
/// turn a guild that WAS founded into an error the player sees as a failure.
#[test]
fn an_unreachable_shard_does_not_fail_a_create_the_authority_already_took() {
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
        guild_sync_error: Some("shard unreachable".into()),
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone()];

    create(world.as_ref(), GINGER, "The Silver Hand").expect("the create still succeeds");

    assert_eq!(
        realm.guild.lock().unwrap().guilds.len(),
        1,
        "the authority kept the guild it committed"
    );
    assert!(
        world.guild_columns.lock().unwrap().is_empty(),
        "the shard's columns stay stale until the next op or world entry"
    );
}

/// The reads answer from the AUTHORITY, not from the shard the reader stands on — a member inside a
/// dungeon queries the same guild everyone else sees.
#[test]
fn the_query_and_membership_reads_go_to_the_authority_from_any_shard() {
    let (_realm, world, instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();

    let view = guild::view(instances.as_ref(), 1)
        .unwrap()
        .expect("the guild is visible from the other shard");
    assert_eq!(view.name, "The Silver Hand");
    assert_eq!(view.master_guid, GINGER);
    assert_eq!(view.member_count, 1);
    assert_eq!(view.rank_names.len(), 10, "ten ranks, always");

    assert_eq!(
        guild::guild_of(instances.as_ref(), GINGER).unwrap(),
        Some(1)
    );
    assert_eq!(guild::guild_of(instances.as_ref(), VIM).unwrap(), None);
    assert_eq!(
        guild::view(instances.as_ref(), 99).unwrap(),
        None,
        "an unknown guild id is absent, not an error"
    );
}

/// World entry is the other half of the "pushed at world entry and on membership change" rule: a
/// character who founded a guild while standing somewhere else arrives with the right columns.
#[test]
fn world_entry_puts_the_authoritys_membership_onto_the_shard_just_entered() {
    let (_realm, world, instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();
    instances.guild_columns.lock().unwrap().clear(); // a shard that missed the push

    guild::on_world_entry(instances.as_ref(), GINGER).expect("world entry syncs the guild");

    assert_eq!(
        instances.guild_columns.lock().unwrap().as_slice(),
        &[(GINGER, 1, 0)]
    );
}

/// A guildless arrival is stamped with zeroes rather than skipped: a shard that still remembers a
/// guild the character has left would render a stale name plate forever.
#[test]
fn world_entry_clears_the_columns_of_a_character_the_authority_says_is_guildless() {
    let (_realm, _world, instances, _calls) = guild_topology();
    instances.guild_columns.lock().unwrap().push((VIM, 42, 3)); // a stale membership left on this shard

    guild::on_world_entry(instances.as_ref(), VIM).unwrap();

    assert_eq!(
        instances.guild_columns.lock().unwrap().as_slice(),
        &[(VIM, 0, 0)]
    );
}

/// **The single-database assertion.** An unsharded gateway runs create and both reads on the
/// player's OWN shard, through the player-facing reducer, byte-identically to a gateway that has
/// never heard of realm-core. The twin of
/// `an_unsharded_gateway_runs_every_party_op_on_the_players_own_shard`.
#[test]
fn an_unsharded_gateway_runs_every_guild_op_on_the_players_own_shard() {
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        characters: vec![character(GINGER, "Ginger")],
        live_guids: vec![GINGER],
        ..Default::default() // no `realm`, no `peers` — the unconfigured gateway
    });
    assert!(
        store.realm_store().is_none(),
        "an unsharded store must not name a realm database"
    );

    create(store.as_ref(), GINGER, "The Silver Hand").expect("the legacy path answers");

    let log = calls.lock().unwrap().clone();
    let ran: Vec<&str> = log.iter().map(|(_, c)| c.as_str()).collect();
    assert_eq!(
        ran,
        vec!["create_guild", "sync_guild_membership"],
        "an unsharded gateway calls the player-facing reducer, then stamps the character's own \
         columns in the same place the module's create core does"
    );
    assert!(
        log.iter().all(|(shard, _)| shard == "world"),
        "every call must land on the player's own database"
    );
    assert!(
        !log.iter().any(|(_, c)| c == "realm_guild_op"),
        "the realm plane must be untouched on a single-database gateway"
    );

    // And the answers are the same ones the sharded path gives, read off the one database.
    let view = guild::view(store.as_ref(), 1)
        .unwrap()
        .expect("the guild is there");
    assert_eq!(view.name, "The Silver Hand");
    assert_eq!(view.master_guid, GINGER);
    assert_eq!(view.rank_names.len(), 10);
    assert_eq!(guild::guild_of(store.as_ref(), GINGER).unwrap(), Some(1));
    assert_eq!(
        store.guild_columns.lock().unwrap().as_slice(),
        &[(GINGER, 1, 0)]
    );
    // World entry has nothing to do with one database: the shard's own tables ARE the authority.
    let before = calls.lock().unwrap().len();
    guild::on_world_entry(store.as_ref(), GINGER).unwrap();
    assert_eq!(
        calls.lock().unwrap().len(),
        before,
        "an unsharded world entry must read and write nothing"
    );
}

// --- the invite handshake ---------------------------------------------------------------------

fn invite(store: &InMemoryStore, self_guid: u64, name: &str) -> Result<()> {
    guild::invite(store, 7, self_guid, name)
}

/// **AC2, and the reason guild state sits on realm-core at all.** The master stands on `world` and
/// the invitee is inside a dungeon on `instances`: the invite still lands, and the popup is written
/// for the target. Resolving the typed name inside the calling database — what the pre-realm-core
/// party code did — answers "no player named Vim" for a character who plainly exists.
#[test]
fn an_invite_crosses_a_shard_boundary_and_reaches_the_target() {
    use lyracore_shared::guild::{event_kind, realm_op};
    let (realm, world, _instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();

    invite(world.as_ref(), GINGER, "Vim").expect("the invitee is on the other shard, not missing");

    let g = realm.guild.lock().unwrap();
    assert_eq!(
        g.ops.last(),
        Some(&(realm_op::INVITE, GINGER, VIM, 0, "Ginger".to_string())),
        "the authority ran the invite with the target resolved and the inviter's own name in `text`"
    );
    assert_eq!(
        g.invites.as_slice(),
        &[(VIM, GINGER, 1)],
        "exactly one pending invite"
    );
    assert_eq!(
        g.events.as_slice(),
        &[(
            VIM,
            event_kind::INVITE,
            GINGER,
            "The Silver Hand".to_string()
        )],
        "exactly one notification, addressed to the target"
    );
}

/// The name is resolved across every shard, and being in the world is the disambiguator: character
/// names are unique per DATABASE, so the same name can name two people, and only one of them can
/// answer a popup. A name nobody online carries is `no such player`, which the seam turns into
/// `GuildPlayerNotFoundS`.
#[test]
fn an_invite_to_a_name_nobody_online_carries_is_refused_before_the_authority_hears_it() {
    use lyracore_shared::guild::err;
    let calls: ShardCallLog = Default::default();
    let realm = std::sync::Arc::new(InMemoryStore {
        shard: "lyracore-realm".into(),
        calls: calls.clone(),
        is_realm: true,
        ..Default::default()
    });
    // Vim has a character row on this shard but no live entity: logged out, dialog unanswerable.
    let world = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![character(GINGER, "Ginger"), character(VIM, "Vim")],
        live_guids: vec![GINGER],
        ..Default::default()
    });
    *world.peers.lock().unwrap() = vec![world.clone()];
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();

    let missing = invite(world.as_ref(), GINGER, "Nobody").expect_err("no such character anywhere");
    assert!(format!("{missing:#}").contains(err::TARGET_NOT_FOUND));

    let offline = invite(world.as_ref(), GINGER, "Vim").expect_err("nobody can answer the popup");
    assert!(format!("{offline:#}").contains(err::TARGET_NOT_FOUND));

    assert!(
        realm.guild.lock().unwrap().invites.is_empty(),
        "a name the gateway could not resolve never reaches the authority"
    );
}

/// AC4/AC5: accepting seats the character at the join rank on the AUTHORITY, tells every member,
/// and — the thing that rots silently if it is skipped — pushes the new member's own guild columns
/// onto every connected shard, so `SMSG_CHAR_ENUM` stops saying they are guildless.
#[test]
fn accepting_seats_the_new_member_tells_the_guild_and_pushes_their_guild_columns() {
    use lyracore_shared::guild::{event_kind, GUILD_JOIN_RANK};
    let (realm, world, instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();
    invite(world.as_ref(), GINGER, "Vim").unwrap();

    guild::answer_invite(instances.as_ref(), 7, VIM, true).expect("Vim joins from the other shard");

    let g = realm.guild.lock().unwrap();
    assert_eq!(g.guild_of(VIM), Some((1, GUILD_JOIN_RANK)));
    assert!(g.invites.is_empty(), "the dialog is consumed");
    let joined: Vec<u64> = g
        .events
        .iter()
        .filter(|(_, kind, ..)| *kind == event_kind::JOINED)
        .map(|(recipient, ..)| *recipient)
        .collect();
    assert_eq!(
        joined,
        vec![GINGER, VIM],
        "every member hears it, the new one included"
    );
    drop(g);

    for shard in [&world, &instances] {
        assert!(
            shard
                .guild_columns
                .lock()
                .unwrap()
                .contains(&(VIM, 1, GUILD_JOIN_RANK)),
            "shard {} must carry the new member's guild id and rank",
            shard.shard
        );
    }
}

/// AC6: a decline consumes the same dialog, seats nobody, and notifies the INVITER — who is on the
/// other shard, which is the only reason the notification has to be written on the authority.
#[test]
fn declining_consumes_the_invite_seats_nobody_and_notifies_the_inviter() {
    use lyracore_shared::guild::event_kind;
    let (realm, world, instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();
    invite(world.as_ref(), GINGER, "Vim").unwrap();

    guild::answer_invite(instances.as_ref(), 7, VIM, false).expect("Vim refuses");

    let g = realm.guild.lock().unwrap();
    assert_eq!(g.guild_of(VIM), None, "a decline seats nobody");
    assert!(g.invites.is_empty(), "the dialog is consumed either way");
    assert_eq!(
        g.events.last(),
        Some(&(GINGER, event_kind::DECLINED, VIM, String::new())),
        "the inviter is the one who hears about it"
    );
}

/// AC7: answering an invite that is not there — never sent, already answered, or reaped by the
/// two-minute GC — refuses with the shared string the seam drops silently. Nothing is written.
#[test]
fn answering_an_invite_that_is_not_there_writes_nothing() {
    use lyracore_shared::guild::err;
    let (realm, _world, instances, _calls) = guild_topology();

    let e = guild::answer_invite(instances.as_ref(), 7, VIM, true).expect_err("no dialog is open");

    assert!(format!("{e:#}").contains(err::NO_PENDING_INVITE));
    assert!(realm.guild.lock().unwrap().members.is_empty());
}

/// AC9: signing on and off tells the REST of the guild, and nobody else. The op carries the
/// member's own name, because realm-core has no character row to read one from.
#[test]
fn signing_on_and_off_broadcasts_to_the_rest_of_the_guild() {
    use lyracore_shared::guild::{event_kind, realm_op};
    let (realm, world, instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();
    invite(world.as_ref(), GINGER, "Vim").unwrap();
    guild::answer_invite(instances.as_ref(), 7, VIM, true).unwrap();
    realm.guild.lock().unwrap().events.clear();

    guild::broadcast_presence(instances.as_ref(), VIM, true);
    guild::broadcast_presence(instances.as_ref(), VIM, false);

    let g = realm.guild.lock().unwrap();
    assert_eq!(
        g.events.as_slice(),
        &[
            (
                GINGER,
                event_kind::PRESENCE,
                VIM,
                event_kind::PRESENCE_ONLINE.to_string()
            ),
            (
                GINGER,
                event_kind::PRESENCE,
                VIM,
                event_kind::PRESENCE_OFFLINE.to_string()
            ),
        ],
        "only the OTHER members hear it"
    );
    assert_eq!(
        g.ops.last(),
        Some(&(
            realm_op::PRESENCE,
            VIM,
            0,
            realm_op::PRESENCE_OFF,
            "Vim".to_string()
        ))
    );
}

/// A guildless character costs no reducer call at all: this runs on every login and every logout,
/// and most characters are in no guild.
#[test]
fn a_guildless_character_broadcasts_no_presence_at_all() {
    let (_realm, _world, instances, calls) = guild_topology();

    guild::broadcast_presence(instances.as_ref(), VIM, true);

    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|(_, call)| call == "realm_guild_op"),
        "nobody to tell, so nothing to say"
    );
}

/// **AC10, the single-database assertion for the handshake.** An unsharded gateway runs invite,
/// accept and decline on the player's OWN shard, byte-identically — that one database already is
/// the authority, so there is no second copy to diverge from.
#[test]
fn an_unsharded_gateway_runs_the_invite_handshake_on_the_players_own_shard() {
    use lyracore_shared::guild::{event_kind, realm_op, GUILD_JOIN_RANK};
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        characters: vec![character(GINGER, "Ginger"), character(VIM, "Vim")],
        live_guids: vec![GINGER, VIM],
        ..Default::default() // no `realm`, no `peers` — the unconfigured gateway
    });
    assert!(store.realm_store().is_none());

    create(store.as_ref(), GINGER, "The Silver Hand").unwrap();
    invite(store.as_ref(), GINGER, "Vim").expect("the invite runs on the one database");
    guild::answer_invite(store.as_ref(), 7, VIM, true).expect("and so does the accept");

    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .all(|(shard, _)| shard == "world"),
        "every call must land on the player's own database"
    );
    let g = store.guild.lock().unwrap();
    assert_eq!(
        g.ops.as_slice(),
        &[
            (realm_op::INVITE, GINGER, VIM, 0, "Ginger".to_string()),
            (
                realm_op::ANSWER,
                VIM,
                0,
                realm_op::ANSWER_ACCEPT,
                "Vim".to_string()
            ),
        ],
        "the same op bytes and the same slots the sharded plane sends"
    );
    assert_eq!(g.guild_of(VIM), Some((1, GUILD_JOIN_RANK)));
    assert!(g
        .events
        .iter()
        .any(|(recipient, kind, ..)| *recipient == VIM && *kind == event_kind::JOINED));
}

/// The module owns the create gates, so the routing's job is to carry the refusal back unchanged —
/// the gateway classifies `SMSG_GUILD_COMMAND_RESULT` off these exact strings.
#[test]
fn a_refused_create_propagates_the_authoritys_own_error_string() {
    use lyracore_shared::guild::err;
    let (_realm, world, _instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();

    let taken = create(world.as_ref(), VIM, "The Silver Hand").expect_err("the name is taken");
    assert!(format!("{taken:#}").contains(err::NAME_TAKEN));

    let already = create(world.as_ref(), GINGER, "Another").expect_err("Ginger already leads one");
    assert!(format!("{already:#}").contains(err::ALREADY_IN_GUILD));

    let invalid = create(world.as_ref(), VIM, "X").expect_err("one character is too short");
    assert!(format!("{invalid:#}").contains(err::NAME_INVALID));
}
