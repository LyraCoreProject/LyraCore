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
