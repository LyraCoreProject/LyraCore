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

use super::party_tests::{character, GINGER, TRIN, VIM};
use lyracore_shared::guild::{err, event_kind, realm_op};

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

// ===========================================================================================
//  Roster
// ===========================================================================================

/// A guild member whose character row nothing holds — a shard that is down, or a row the sweeps
/// have already taken. It still belongs to the guild as far as the authority is concerned.
const MYSTERY: u64 = 77;

/// A character row with every shard-resolved field set to something DISTINCT, so a roster entry
/// that quietly took a default reads as a wrong value rather than as a coincidence.
fn resident(guid: u64, name: &str, level: u8, class: u8, zone_id: u32) -> codec::CharacterView {
    codec::CharacterView {
        guid,
        name: name.into(),
        level,
        class,
        zone_id,
        ..Default::default()
    }
}

/// [`guild_topology`] with the roster's own fixtures: Ginger's row on `world` and Vim's on
/// `instances`, carrying different names, levels, classes and zones. Ginger is live; Vim's row
/// exists with nobody at the keyboard, which is exactly what an offline guild member is.
///
/// The split is the point. A roster rendered from one shard alone, or from realm-core alone, cannot
/// produce both rows.
fn roster_topology() -> (
    std::sync::Arc<InMemoryStore>, // realm-core
    std::sync::Arc<InMemoryStore>, // the open-world shard
    std::sync::Arc<InMemoryStore>, // the instances shard
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
        characters: vec![resident(GINGER, "Ginger", 60, 2, 12)], // Paladin, Elwynn Forest
        live_guids: vec![GINGER],
        ..Default::default()
    });
    let instances = std::sync::Arc::new(InMemoryStore {
        shard: "instances".into(),
        calls: calls.clone(),
        realm: Some(realm.clone()),
        characters: vec![resident(VIM, "Vim", 24, 8, 14)], // Mage, Durotar
        ..Default::default()
    });
    for shard in [&world, &instances] {
        *shard.peers.lock().unwrap() = vec![world.clone(), instances.clone()];
    }
    seed_silver_hand(&realm);
    (realm, world, instances)
}

/// Seed the authority with the roster realm-core owns: Ginger as master, Vim at rank 3, the guild
/// text and both members' notes. Seeded directly rather than through an invite — membership writes
/// belong to another slice, and the roster only ever reads.
fn seed_silver_hand(realm: &InMemoryStore) {
    let mut g = realm.guild.lock().unwrap();
    let guild_id = g
        .create(GINGER, "The Silver Hand", GUILD_FIXTURE_MICROS)
        .expect("the fixture guild is founded");
    g.members.push((guild_id, VIM, 3));
    g.guild_text.push((
        guild_id,
        "Raid at eight".into(),
        "Founded on the Sunday".into(),
    ));
    g.member_notes
        .push((GINGER, "founder".into(), "trusted".into()));
    g.member_notes
        .push((VIM, "alt of Ginger".into(), String::new()));
}

/// **AC2, and the reason this ticket exists.** Realm-core knows guids, ranks and notes and nothing
/// else, so every human-readable field has to come from the shard that holds that character. The
/// two members sit on DIFFERENT databases with different levels, classes and zones — an
/// implementation that skipped the fan-out would still build a well-formed packet, and every entry
/// in it would be a level-zero unknown class in zone zero.
#[test]
fn every_roster_entry_carries_the_real_character_from_the_shard_that_holds_it() {
    let (_realm, world, instances) = roster_topology();

    let roster = guild::roster(world.as_ref(), 1)
        .unwrap()
        .expect("the authority has the guild");

    assert_eq!(roster.motd, "Raid at eight");
    assert_eq!(roster.info_text, "Founded on the Sunday");
    assert_eq!(
        roster.rank_rights,
        lyracore_shared::guild::DEFAULT_RANK_RIGHTS.to_vec(),
        "one rights entry per rank, in rank order"
    );

    let [ginger, vim] = roster.members.as_slice() else {
        panic!("both members must be listed, got {:?}", roster.members);
    };
    assert_eq!(
        (
            ginger.guid,
            ginger.rank,
            ginger.name.as_str(),
            ginger.level,
            ginger.class,
            ginger.zone_id
        ),
        (GINGER, 0, "Ginger", 60, 2, 12),
        "the master's row came off his own shard"
    );
    assert_eq!(
        (
            vim.guid,
            vim.rank,
            vim.name.as_str(),
            vim.level,
            vim.class,
            vim.zone_id
        ),
        (VIM, 3, "Vim", 24, 8, 14),
        "the member inside the dungeon is on a database the viewer's shard never reads"
    );
    assert_eq!(
        (ginger.public_note.as_str(), ginger.officer_note.as_str()),
        ("founder", "trusted"),
        "the notes are realm-core's half of the entry"
    );

    // And the answer does not depend on where the viewer stands.
    let from_the_dungeon = guild::roster(instances.as_ref(), 1).unwrap().unwrap();
    assert_eq!(from_the_dungeon, roster);
}

/// AC3: the online column is resolved per member across the shards, and an offline member is listed
/// with their last-known level, class and area rather than dropped — an absent row in the guild
/// panel reads as "they left".
#[test]
fn offline_members_are_listed_with_their_last_known_row() {
    let (_realm, world, _instances) = roster_topology();

    let roster = guild::roster(world.as_ref(), 1).unwrap().unwrap();

    assert!(roster.members[0].online, "Ginger is live on `world`");
    assert!(
        !roster.members[1].online,
        "Vim has a character row and no live entity anywhere"
    );
    assert_eq!(roster.members[1].level, 24, "and still carries their row");
}

/// AC8: a member no shard can answer for keeps the guid, rank and notes realm-core knows and takes
/// the defaults for the rest. Never dropped, never a panic.
#[test]
fn a_member_no_shard_can_resolve_still_appears_with_what_the_authority_knows() {
    let (realm, world, _instances) = roster_topology();
    realm.guild.lock().unwrap().members.push((1, MYSTERY, 4));
    realm
        .guild
        .lock()
        .unwrap()
        .member_notes
        .push((MYSTERY, "on leave".into(), String::new()));

    let roster = guild::roster(world.as_ref(), 1).unwrap().unwrap();

    let mystery = roster
        .members
        .iter()
        .find(|m| m.guid == MYSTERY)
        .expect("an unresolvable member is still a member");
    assert_eq!(
        (mystery.rank, mystery.public_note.as_str()),
        (4, "on leave")
    );
    assert_eq!(
        (
            mystery.name.as_str(),
            mystery.level,
            mystery.class,
            mystery.zone_id,
            mystery.online
        ),
        ("", 0, 0, 0, false)
    );
}

/// **AC9, the single-database assertion.** An unsharded gateway reads the roster from the player's
/// own database and renders it from that same database's character rows — the realm plane is never
/// touched, and the answer is the one the sharded path gives.
#[test]
fn an_unsharded_gateway_renders_the_roster_from_the_players_own_shard() {
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        characters: vec![resident(GINGER, "Ginger", 60, 2, 12)],
        live_guids: vec![GINGER],
        ..Default::default() // no `realm`, no `peers` — the unconfigured gateway
    });
    seed_silver_hand(&store);
    calls.lock().unwrap().clear(); // the seeding is not a routed call

    let roster = guild::roster(store.as_ref(), 1)
        .unwrap()
        .expect("the one database is the authority");

    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter().all(|(shard, _)| shard == "world"),
        "every read must land on the player's own database, got {log:?}"
    );
    assert!(
        !log.iter().any(|(_, call)| call == "realm_guild_op"),
        "the realm plane must be untouched on a single-database gateway"
    );
    assert_eq!(roster.motd, "Raid at eight");
    assert_eq!(
        (
            roster.members[0].name.as_str(),
            roster.members[0].level,
            roster.members[0].class,
            roster.members[0].zone_id,
            roster.members[0].online
        ),
        ("Ginger", 60, 2, 12, true)
    );
    // Vim has no character row on this one database, so the entry degrades rather than vanishing.
    assert_eq!(roster.members[1].guid, VIM);
    assert_eq!(roster.members[1].rank, 3);
    assert_eq!(roster.members[1].name, "");
}

/// A guild id the authority does not have is absent, not an error — the same posture the query read
/// takes for a stale id the client still holds.
#[test]
fn an_unknown_guild_id_has_no_roster() {
    let (_realm, world, _instances) = roster_topology();

    assert_eq!(guild::roster(world.as_ref(), 99).unwrap(), None);
}

// --- Teardown: leave, kick, disband and the leadership transfer -----------------------------

/// [`guild_topology`] with a THREE-member guild already on the authority: Ginger is the master and
/// stands on `world`, Vim stands on `instances`, Trin stands on `world`.
///
/// Seeded straight into realm-core's roster rather than through an invite flow — teardown does not
/// depend on how a member joined, and the invite flow is another ticket's.
fn teardown_topology() -> (
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
        characters: vec![character(GINGER, "Ginger"), character(TRIN, "Trin")],
        live_guids: vec![GINGER, TRIN],
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
    {
        let mut g = realm.guild.lock().unwrap();
        g.create(GINGER, "The Silver Hand", 0).unwrap();
        g.members.push((1, VIM, 4));
        g.members.push((1, TRIN, 4));
    }
    calls.lock().unwrap().clear();
    (realm, world, instances, calls)
}

/// The authority's roster, as guids in join order.
fn roster(realm: &InMemoryStore) -> Vec<u64> {
    realm.guild.lock().unwrap().member_guids(1)
}

/// Every notification the authority queued for `kind`, as `(recipient, other_guid, other_name)`.
fn notices(realm: &InMemoryStore, kind: u8) -> Vec<(u64, u64, String)> {
    realm
        .guild
        .lock()
        .unwrap()
        .events
        .iter()
        .filter(|(_, k, ..)| *k == kind)
        .map(|(recipient, _, other, name)| (*recipient, *other, name.clone()))
        .collect()
}

/// What `sync_guild_membership` last wrote for `guid` on `shard`. `None` = never pushed there.
fn columns(shard: &InMemoryStore, guid: u64) -> Option<(u64, u32)> {
    shard
        .guild_columns
        .lock()
        .unwrap()
        .iter()
        .find(|(g, ..)| *g == guid)
        .map(|(_, guild_id, rank)| (*guild_id, *rank))
}

fn leave(store: &InMemoryStore, self_guid: u64, actor_name: &str) -> Result<()> {
    guild::run(
        store,
        7,
        self_guid,
        guild::Op::Leave {
            actor_name: actor_name.into(),
        },
    )
}

/// AC1: a non-master leaving takes exactly their own membership with them, the guild survives, the
/// remaining members are told, and EVERY shard learns the leaver is guildless — including the one
/// they are not standing on.
#[test]
fn a_non_master_leave_removes_only_that_member_and_clears_their_columns_everywhere() {
    let (realm, world, instances, _calls) = teardown_topology();

    leave(instances.as_ref(), VIM, "Vim").expect("a plain member may always leave");

    assert_eq!(roster(realm.as_ref()), vec![GINGER, TRIN]);
    assert!(
        realm.guild.lock().unwrap().view(1).is_some(),
        "the guild survives a member leaving it"
    );
    assert_eq!(
        notices(realm.as_ref(), event_kind::LEFT),
        vec![
            (GINGER, VIM, "Vim".to_string()),
            (TRIN, VIM, "Vim".to_string())
        ],
        "everyone who stayed is told who left, by name"
    );
    for shard in [&world, &instances] {
        assert_eq!(
            columns(shard, VIM),
            Some((0, 0)),
            "shard {} still thinks Vim is in a guild",
            shard.shard
        );
    }
}

/// AC2: the master cannot simply walk out. Succession is a decision the player makes; the realm
/// refuses rather than promoting somebody, and nothing moves.
#[test]
fn a_master_leaving_with_members_remaining_is_refused_and_changes_nothing() {
    let (realm, world, _instances, _calls) = teardown_topology();

    let refused = leave(world.as_ref(), GINGER, "Ginger").expect_err("the master may not leave");

    assert!(format!("{refused:#}").contains(err::MASTER_MUST_TRANSFER_OR_DISBAND));
    assert_eq!(roster(realm.as_ref()), vec![GINGER, VIM, TRIN]);
    assert!(notices(realm.as_ref(), event_kind::LEFT).is_empty());
}

/// AC3: a master who is the LAST member may leave, and the guild goes with them — otherwise a
/// one-member guild would hold its unique name forever.
#[test]
fn a_last_member_master_may_leave_and_the_guild_goes_with_them() {
    let (realm, world, _instances, _calls) = teardown_topology();
    realm
        .guild
        .lock()
        .unwrap()
        .members
        .retain(|(_, guid, _)| *guid == GINGER);

    leave(world.as_ref(), GINGER, "Ginger").expect("the last member may always leave");

    assert!(roster(realm.as_ref()).is_empty());
    assert_eq!(realm.guild.lock().unwrap().disbanded, vec![1]);
    assert_eq!(columns(world.as_ref(), GINGER), Some((0, 0)));
}

/// AC4: the master kicks by NAME, the name resolves against the guild's own roster, and everyone
/// who stayed hears about it. The kicked member is cleared on every shard, not just the kicker's.
#[test]
fn a_kick_by_the_master_removes_the_named_member_and_broadcasts_removed() {
    use super::handlers::GuildActionStore;
    let (realm, world, instances, _calls) = teardown_topology();

    world
        .guild_remove(7, GINGER, "Vim")
        .expect("the master may remove a member");

    assert_eq!(roster(realm.as_ref()), vec![GINGER, TRIN]);
    assert_eq!(
        notices(realm.as_ref(), event_kind::REMOVED),
        vec![
            (GINGER, VIM, "Vim".to_string()),
            (TRIN, VIM, "Vim".to_string())
        ]
    );
    assert_eq!(
        columns(instances.as_ref(), VIM),
        Some((0, 0)),
        "the kicked member's own shard has to hear it too"
    );
}

/// AC5, AC6 and AC7: the three kicks the authority refuses. Each one leaves the roster exactly as
/// it was — a refused kick that still removed a row would be the worst of both.
#[test]
fn a_kick_is_refused_for_a_non_master_a_non_member_and_the_master_themselves() {
    use super::handlers::GuildActionStore;
    let (realm, world, instances, _calls) = teardown_topology();

    let not_master = instances
        .guild_remove(7, VIM, "Trin")
        .expect_err("a plain member may not kick");
    assert!(format!("{not_master:#}").contains(err::NOT_GUILD_MASTER));

    let not_member = world
        .guild_remove(7, GINGER, "Nobody")
        .expect_err("a name nobody in the guild answers to is refused");
    assert!(format!("{not_member:#}").contains(err::TARGET_NOT_IN_GUILD));

    let themselves = world
        .guild_remove(7, GINGER, "Ginger")
        .expect_err("a kick is never a leave");
    assert!(format!("{themselves:#}").contains(err::CANNOT_REMOVE_SELF));

    assert_eq!(roster(realm.as_ref()), vec![GINGER, VIM, TRIN]);
    assert!(notices(realm.as_ref(), event_kind::REMOVED).is_empty());
}

/// **AC8, the criterion this ticket exists for.** A disband leaves ZERO rows for the guild, and
/// every ex-member's guild columns are zeroed on every shard — including the member standing in a
/// dungeon on the other database. One member left holding a dead guild id can never join another
/// guild, because `GuildMember.character_guid` is unique and enforced in code.
#[test]
fn a_disband_leaves_no_row_behind_and_clears_every_ex_members_columns_on_every_shard() {
    let (realm, world, instances, _calls) = teardown_topology();

    guild::run(world.as_ref(), 7, GINGER, guild::Op::Disband).expect("the master may disband");

    let g = realm.guild.lock().unwrap();
    assert!(g.members.is_empty(), "not one member row may survive");
    assert!(g.guilds.is_empty(), "the guild row goes with them");
    assert_eq!(g.disbanded, vec![1]);
    drop(g);
    assert_eq!(
        notices(realm.as_ref(), event_kind::DISBANDED)
            .into_iter()
            .map(|(recipient, ..)| recipient)
            .collect::<Vec<_>>(),
        vec![GINGER, VIM, TRIN],
        "every member the guild had is told, while it still has them"
    );
    for guid in [GINGER, VIM, TRIN] {
        for shard in [&world, &instances] {
            assert_eq!(
                columns(shard, guid),
                Some((0, 0)),
                "shard {} still holds a dead guild for {guid}",
                shard.shard
            );
        }
    }
}

/// AC9: a disband by anybody but the master is refused, and the guild is untouched.
#[test]
fn a_disband_by_a_non_master_is_refused_and_changes_nothing() {
    let (realm, _world, instances, _calls) = teardown_topology();

    let refused = guild::run(instances.as_ref(), 7, VIM, guild::Op::Disband)
        .expect_err("a plain member may not disband");

    assert!(format!("{refused:#}").contains(err::NOT_GUILD_MASTER));
    assert_eq!(roster(realm.as_ref()), vec![GINGER, VIM, TRIN]);
    assert!(realm.guild.lock().unwrap().disbanded.is_empty());
}

/// AC10 and AC11: the transfer moves the master, swaps the two ranks and tells the guild; a
/// transfer to somebody outside the guild is refused.
#[test]
fn a_leadership_transfer_moves_the_master_and_both_ranks() {
    use super::handlers::GuildActionStore;
    let (realm, world, instances, _calls) = teardown_topology();

    world
        .guild_set_master(7, GINGER, "Vim")
        .expect("the master may hand the guild on");

    let g = realm.guild.lock().unwrap();
    assert_eq!(g.master_of(1), Some(VIM));
    assert_eq!(g.guild_of(VIM), Some((1, 0)), "the new master takes rank 0");
    assert_eq!(
        g.guild_of(GINGER),
        Some((1, 1)),
        "the old master keeps officer standing rather than dropping to the bottom"
    );
    drop(g);
    assert_eq!(
        notices(realm.as_ref(), event_kind::LEADER_CHANGED)
            .into_iter()
            .map(|(recipient, other, _)| (recipient, other))
            .collect::<Vec<_>>(),
        vec![(GINGER, VIM), (VIM, VIM), (TRIN, VIM)]
    );
    assert_eq!(columns(world.as_ref(), GINGER), Some((1, 1)));
    assert_eq!(columns(instances.as_ref(), VIM), Some((1, 0)));

    // …and the same transfer to somebody the guild does not have is refused. Vim leads now.
    let stranger = instances
        .guild_set_master(7, VIM, "Nobody")
        .expect_err("a non-member cannot be made master");
    assert!(format!("{stranger:#}").contains(err::TARGET_NOT_IN_GUILD));
}

/// **AC13, the single-database assertion.** An unsharded gateway runs leave, kick, disband and the
/// leadership transfer on the player's OWN database, byte-identically — no realm handle exists to
/// route to, and the module's cores stamp the character's own columns in the same transaction.
#[test]
fn an_unsharded_gateway_runs_every_teardown_op_on_the_players_own_shard() {
    use super::handlers::GuildActionStore;
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        characters: vec![character(GINGER, "Ginger"), character(VIM, "Vim")],
        live_guids: vec![GINGER, VIM],
        ..Default::default() // no `realm`, no `peers` — the unconfigured gateway
    });
    assert!(store.realm_store().is_none());
    {
        let mut g = store.guild.lock().unwrap();
        g.create(GINGER, "The Silver Hand", 0).unwrap();
        g.members.push((1, VIM, 4));
    }

    store
        .guild_remove(7, GINGER, "Vim")
        .expect("the kick runs on the one database");

    assert_eq!(
        store.guild.lock().unwrap().member_guids(1),
        vec![GINGER],
        "the one database is the authority"
    );
    assert_eq!(
        columns(store.as_ref(), VIM),
        Some((0, 0)),
        "the module's core stamps the kicked member's own columns in the same transaction"
    );
    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter().all(|(shard, _)| shard == "world"),
        "every call must land on the player's own database"
    );
    assert_eq!(
        log.iter().filter(|(_, c)| c == "realm_guild_op").count(),
        1,
        "one op, run once, against the player's own database"
    );

    // The master is alone now, so the leave is the one a last member is allowed — and it disbands.
    store
        .guild_leave(7, GINGER)
        .expect("the last member may leave");
    assert!(store.guild.lock().unwrap().guilds.is_empty());
    assert_eq!(columns(store.as_ref(), GINGER), Some((0, 0)));
}

/// The op reaches the AUTHORITY with the slots the shared contract documents. The gateway and the
/// module are deployed separately, so the packing is a wire fact, not an implementation detail.
#[test]
fn every_teardown_op_reaches_realm_core_in_its_contracted_slots() {
    use super::handlers::GuildActionStore;
    let (realm, world, _instances, _calls) = teardown_topology();

    world.guild_remove(7, GINGER, "Vim").unwrap();
    world.guild_set_master(7, GINGER, "Trin").unwrap();
    guild::run(world.as_ref(), 7, TRIN, guild::Op::Disband).unwrap();

    assert_eq!(
        realm.guild.lock().unwrap().ops.as_slice(),
        &[
            (realm_op::REMOVE, GINGER, VIM, 0, "Vim".to_string()),
            (realm_op::LEADER, GINGER, TRIN, 0, "Trin".to_string()),
            (realm_op::DISBAND, TRIN, 0, 0, String::new()),
        ]
    );
}
