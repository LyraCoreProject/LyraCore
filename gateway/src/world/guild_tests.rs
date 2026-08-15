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

fn set_motd(store: &InMemoryStore, self_guid: u64, text: &str) -> Result<()> {
    guild::run(store, 7, self_guid, guild::Op::SetMotd(text.into()))
}

fn set_public_note(
    store: &InMemoryStore,
    self_guid: u64,
    target_guid: u64,
    note: &str,
) -> Result<()> {
    guild::run(
        store,
        7,
        self_guid,
        guild::Op::SetPublicNote {
            target_guid,
            note: note.into(),
        },
    )
}

/// The MOTD op runs on the authority, exactly like create — packed with `realm_op::SET_MOTD`, the
/// setter's own guid as the actor, and no membership-push side effect (T6's ops never change
/// membership, unlike T1's create).
#[test]
fn setting_the_motd_runs_on_realm_core_with_the_setters_guid_as_actor() {
    let (realm, world, _instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();

    set_motd(world.as_ref(), GINGER, "Raid at 8pm")
        .expect("the master's motd change reaches the authority");

    assert_eq!(
        realm.guild.lock().unwrap().ops.last(),
        Some(&(
            lyracore_shared::guild::realm_op::SET_MOTD,
            GINGER,
            0,
            0,
            "Raid at 8pm".to_string()
        ))
    );
    let view = guild::view(world.as_ref(), 1)
        .unwrap()
        .expect("the guild is there");
    assert_eq!(view.motd, "Raid at 8pm");
}

/// Acceptance criterion 4: an empty MOTD reaches the authority and clears the stored value — never
/// refused for being blank.
#[test]
fn an_empty_motd_clears_the_stored_value() {
    let (_realm, world, _instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();
    set_motd(world.as_ref(), GINGER, "Hello").unwrap();

    set_motd(world.as_ref(), GINGER, "").expect("an empty motd is a valid clear, not a refusal");

    let view = guild::view(world.as_ref(), 1).unwrap().unwrap();
    assert_eq!(view.motd, "");
}

/// Acceptance criterion 2: a non-master's MOTD change is refused with the module's own
/// `NOT_GUILD_MASTER` string, and the stored MOTD is untouched.
#[test]
fn a_non_masters_motd_change_is_refused_and_changes_nothing() {
    use lyracore_shared::guild::err;
    let (realm, world, instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();
    // Vim joins as a plain member via the fake's own membership list (no invite flow needed here).
    realm.guild.lock().unwrap().members.push((
        1,
        VIM,
        lyracore_shared::guild::GUILD_RANK_COUNT as u32 - 1,
    ));

    let refused =
        set_motd(instances.as_ref(), VIM, "Nope").expect_err("a non-master must be refused");
    assert!(format!("{refused:#}").contains(err::NOT_GUILD_MASTER));

    let view = guild::view(instances.as_ref(), 1).unwrap().unwrap();
    assert_eq!(view.motd, "", "the refused change must not have landed");
}

/// Acceptance criterion 6: a member may set their own public note; the master may set anyone's.
#[test]
fn a_member_sets_their_own_public_note_and_the_master_sets_anyones() {
    let (realm, world, instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();
    realm.guild.lock().unwrap().members.push((1, VIM, 4));

    set_public_note(instances.as_ref(), VIM, VIM, "my own note")
        .expect("a member may set their own public note");
    set_public_note(world.as_ref(), GINGER, VIM, "master's note on Vim")
        .expect("the master may set anyone's public note");

    assert_eq!(
        realm.guild.lock().unwrap().notes.get(&(1, VIM)).cloned(),
        Some(("master's note on Vim".to_string(), String::new())),
        "the master's write landed last"
    );
}

/// Acceptance criterion 6 (the refusal half): a member setting ANOTHER member's public note is
/// refused unless they are the master.
#[test]
fn a_member_setting_anothers_public_note_is_refused() {
    use lyracore_shared::guild::err;
    let (realm, world, instances, _calls) = guild_topology();
    create(world.as_ref(), GINGER, "The Silver Hand").unwrap();
    realm.guild.lock().unwrap().members.push((1, VIM, 4));

    let refused = set_public_note(instances.as_ref(), VIM, GINGER, "not mine to set")
        .expect_err("a plain member may not set someone else's public note");
    assert!(format!("{refused:#}").contains(err::NOT_GUILD_MASTER));
}

/// **The single-database assertion, T6's ops.** An unsharded gateway runs every setter through
/// `realm_guild_op` directly on the player's own database — the twin of
/// `an_unsharded_gateway_runs_every_guild_op_on_the_players_own_shard`, which pins T1's `Create`
/// taking the SEPARATE `create_guild` reducer instead. T6 adds no new single-database reducer (and
/// no new hand-spliced binding): `realm_guild_op` already runs correctly against any database, so
/// the unsharded arm calls it on `store` directly rather than duplicating a reducer per op.
#[test]
fn an_unsharded_gateway_runs_every_motd_and_note_setter_on_the_players_own_shard() {
    let calls: ShardCallLog = Default::default();
    let store = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        calls: calls.clone(),
        characters: vec![character(GINGER, "Ginger")],
        live_guids: vec![GINGER],
        ..Default::default() // no `realm`, no `peers` — the unconfigured gateway
    });
    create(store.as_ref(), GINGER, "The Silver Hand").expect("the legacy create path answers");
    calls.lock().unwrap().clear();

    set_motd(store.as_ref(), GINGER, "Hello").expect("the legacy motd path answers");

    let log = calls.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![("world".to_string(), "realm_guild_op".to_string())],
        "an unsharded gateway runs the setter through `realm_guild_op` on its OWN (only) database — \
         there is no separate plane to have skipped"
    );
    let view = guild::view(store.as_ref(), 1).unwrap().unwrap();
    assert_eq!(view.motd, "Hello");
}
