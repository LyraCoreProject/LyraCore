//! Realm-wide whispers — the routing tests.
//!
//! What EXECUTES here is production `world::whisper`, against the same in-memory multi-database
//! topology the group slice and the cross-database transfer tests use (`party_tests::party_topology`
//! — Ginger in the open world on `world`, Vim inside the dungeon on `instances`, plus an offline
//! character and a playerbot). What the fakes stand in for is named at each seam: `realm_whispers` is
//! exactly the tuple the operator-gated `realm_whisper` reducer was handed, recorded before it judges
//! anything, and `whispers` is what the pre-realm-core player-facing `send_whisper` was handed
//! instead.

use super::party_tests::{character, party_topology, BOT, DORMANT, GINGER, TRIN, VIM};
use super::*;

/// **AC: a whisper reaches a target on ANOTHER database.**
///
/// The live failure was not delivery — it was RESOLUTION: `/w Vim` looks the typed name up in
/// `game_character` on ONE database, so to Ginger standing in Elwynn a Vim who had walked into
/// Deadmines did not exist, and the whisper died as "no player named Vim" before any delivery ran.
#[test]
fn a_whisper_reaches_a_target_standing_on_another_shard() {
    let (realm, world, instances, calls) = party_topology();
    // The pre-realm-core read, run against the shard the sender is on: Vim is simply not there.
    assert_eq!(
        world.character_guid_by_name("Vim").unwrap(),
        None,
        "the fixture must reproduce the live shape — Vim's row lives on the instances shard"
    );

    whisper::run(
        world.as_ref(),
        7,
        Some(GINGER),
        "Vim",
        "meet me at the gate".into(),
    )
    .expect("the whisper crosses the boundary");

    assert_eq!(
        realm.realm_whispers.lock().unwrap().clone(),
        vec![(GINGER, VIM, "meet me at the gate".to_string(), false)],
        "the whisper must reach REALM-CORE, addressed by guid — the only realm-wide name a \
         recipient has (a bound identity is minted per database and names nobody elsewhere)"
    );
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, c)| c == "send_whisper"),
        "a multi-database gateway must not run the whisper through a shard's own name lookup — that \
         is exactly the shard-local behaviour realm-wide whisper routing removes. Calls were {log:?}"
    );
    let _ = instances;
}

/// **AC: …both directions.** The same whisper typed from inside the dungeon, out.
#[test]
fn a_whisper_crosses_the_boundary_from_the_other_side_too() {
    let (realm, _world, instances, _calls) = party_topology();
    assert_eq!(
        instances.character_guid_by_name("Ginger").unwrap(),
        None,
        "fixture: from inside the instance, the open world's characters are on another database"
    );

    whisper::run(
        instances.as_ref(),
        8,
        Some(VIM),
        "ginger",
        "on my way".into(),
    )
    .expect("the whisper crosses back");

    assert_eq!(
        realm.realm_whispers.lock().unwrap().clone(),
        vec![(VIM, GINGER, "on my way".to_string(), false)],
        "and it resolves case-insensitively, as `/w bob` always has"
    );
}

/// **AC: \"No player named X\" still returns for a genuinely absent name** — and the refusal never
/// touches the authority, because the name was never resolvable anywhere.
#[test]
fn an_absent_name_is_refused_realm_wide_and_never_reaches_realm_core() {
    let (realm, world, _instances, _calls) = party_topology();

    let err = whisper::run(
        world.as_ref(),
        7,
        Some(GINGER),
        "Nobodyatall",
        "hello?".into(),
    )
    .expect_err("a name no shard holds is refused");
    assert!(
        err.to_string().contains("no player named Nobodyatall"),
        "the refusal must carry the module's own text (lyracore_shared::whisper), got {err}"
    );
    assert!(
        realm.realm_whispers.lock().unwrap().is_empty(),
        "an unresolvable whisper must not reach lyracore-realm"
    );
}

/// **AC: the offline case still behaves as it does today.** The gate is the module's own — and for
/// whisper that is `game_character.online`, the SESSION flag.
#[test]
fn an_offline_target_is_refused_with_the_modules_own_text() {
    let (realm, world, _instances, _calls) = party_topology();
    assert!(
        !world.character_presence(DORMANT).unwrap().unwrap().0,
        "fixture: Dormant has a character row and is offline"
    );

    let err = whisper::run(
        world.as_ref(),
        7,
        Some(GINGER),
        "Dormant",
        "you there?".into(),
    )
    .expect_err("no offline whispering in vanilla");
    assert!(err.to_string().contains("Dormant is offline"), "got {err}");
    assert!(realm.realm_whispers.lock().unwrap().is_empty());
}

/// **The gate this slice must NOT copy from the group slice.**
///
/// Adversarial review found the moved ONLINE gate reading `game_character.online` where the module
/// read `game_world_entity` — a behaviour change in a refactor's clothes, because every playerbot
/// has a live entity and `online == false` forever. Whisper is the MIRROR case: `send_whisper`
/// gates on the session flag, so a whisper to a bot is refused today, and reaching for the group
/// slice's `live_anywhere` here would silently start accepting them on a multi-database gateway alone.
///
/// Both facts are asserted, so the divergence is visible in the test rather than only in the fixture:
/// the bot IS live (the invite gate's answer) and IS refused (the whisper gate's answer).
#[test]
fn a_playerbot_is_not_whisperable_because_the_online_gate_reads_the_session_flag() {
    let (realm, world, _instances, _calls) = party_topology();
    assert!(
        party::live_anywhere(world.as_ref(), BOT),
        "fixture: a playerbot's live entity is right there — this is what `/invite <bot>` reads"
    );
    assert!(
        !world.character_presence(BOT).unwrap().unwrap().0,
        "fixture: …and its `game_character.online` is false — it never runs `player_login`"
    );

    let err = whisper::run(world.as_ref(), 7, Some(GINGER), "Botty", "hi".into()).expect_err(
        "whisper's own gate is the SESSION flag, and it refuses a bot — as it does today",
    );
    assert!(err.to_string().contains("Botty is offline"), "got {err}");
    assert!(
        realm.realm_whispers.lock().unwrap().is_empty(),
        "a bot whisper must be refused on the multi-database plane exactly as on the single-database \
         one — using the group slice's entity test here would deliver it on one plane only"
    );
}

/// The ignore rule survives the move: the target gets no line, and the sender is told NOTHING (the
/// whisper "succeeds" and their own echo still appears — vanilla reports no error to someone being
/// ignored). The verdict is the gateway's because the contact rows are not on realm-core; the RULE
/// stays in the module, on the shared `whisper_rows` core.
#[test]
fn an_ignored_sender_still_succeeds_and_the_verdict_travels_with_the_call() {
    let (realm, world, _instances, _calls) = party_topology();
    // Trin ignores Ginger, on Trin's own shard (contact rows are character-owned and travel with
    // the character).
    world.contacts.lock().unwrap().push((TRIN, GINGER, true));

    whisper::run(world.as_ref(), 7, Some(GINGER), "Trin", "hey".into())
        .expect("an ignored whisper is not an error to the sender");

    assert_eq!(
        realm.realm_whispers.lock().unwrap().clone(),
        vec![(GINGER, TRIN, "hey".to_string(), true)],
        "realm-core must be told the sender is ignored — it has no contact rows to discover it, so a \
         dropped verdict delivers every blocked whisper"
    );
}

/// The ignore verdict has to be read from the shard that HOLDS the target, which is not the shard the
/// whisper was typed on. Reading only the sender's own database answers `false` for every
/// cross-boundary ignore — the ignore list silently stops working the moment a second database exists.
#[test]
fn the_ignore_verdict_is_read_from_whichever_shard_holds_the_target() {
    let (realm, world, instances, _calls) = party_topology();
    // Vim (inside the instance) ignores Ginger. The row is on the INSTANCES database.
    instances.contacts.lock().unwrap().push((VIM, GINGER, true));
    assert!(
        world.contact_lists(VIM).unwrap().1.is_empty(),
        "fixture: the sender's own shard knows nothing about Vim's ignore list"
    );

    whisper::run(world.as_ref(), 7, Some(GINGER), "Vim", "hello".into()).expect("still no error");

    assert_eq!(
        realm.realm_whispers.lock().unwrap().clone(),
        vec![(GINGER, VIM, "hello".to_string(), true)],
        "the ignore union must reach the shard that holds the target's contact rows"
    );
}

/// An unreachable shard must not turn every whisper into a refusal — and, the sharper half, must not
/// turn every whisper into an IGNORED one. The union fails OPEN in both directions: `Err` from a shard
/// contributes `false`, not `true`.
///
/// Failing closed here would be the worse defect and the invisible one: the whisper still "succeeds"
/// (an ignored sender is told nothing, by design), so the sender sees their own echo and the recipient
/// silently never gets the line — for every whisper in the realm, for as long as one shard is
/// unreachable. Verified by mutation: `.unwrap_or(true)` left this test green until it asserted the
/// VERDICT rather than merely the absence of an error.
#[test]
fn an_unreachable_shard_neither_fails_a_whisper_nor_makes_it_look_ignored() {
    let (realm, world, _instances, _calls) = party_topology();
    // Nobody ignores anybody — the ONLY thing that can make this whisper look ignored is the shard
    // that cannot answer, which is the whole point.
    let broken = std::sync::Arc::new(InMemoryStore {
        shard: "unreachable".into(),
        contact_lists_error: Some("shard is unreachable".into()),
        ..Default::default()
    });
    world.peers.lock().unwrap().push(broken);

    whisper::run(world.as_ref(), 7, Some(GINGER), "Trin", "hey".into())
        .expect("one unreachable database must not stop a whisper between two live players");
    assert_eq!(
        realm.realm_whispers.lock().unwrap().clone(),
        vec![(GINGER, TRIN, "hey".to_string(), false)],
        "an unreadable shard must contribute NO ignore verdict. Reading its `Err` as \"ignored\" \
         silently drops the recipient's line for every whisper on the realm while it is down, and \
         tells nobody — the sender's echo still appears"
    );
}

/// The SENDER gate: `send_whisper` starts with `entity_by_owner`, so a caller with no live entity is
/// refused before anything else. Realm-core has no entities, so the gateway answers it — with the
/// group slice's `live_anywhere`, which IS the module's read here (`game_world_entity`), unioned.
///
/// This is also the impersonation fence: the sender guid is an ARGUMENT to the operator-gated
/// reducer, so a `None` that fell through as `0` would whisper as guid 0.
#[test]
fn a_sender_with_no_live_character_anywhere_is_refused() {
    let (realm, world, _instances, _calls) = party_topology();

    // Character select: no in-world character at all.
    let err = whisper::run(world.as_ref(), 7, None, "Trin", "hi".into())
        .expect_err("a session with no in-world character cannot whisper");
    assert!(
        err.to_string().contains("whisperer not in world"),
        "got {err}"
    );

    // In-world state claiming a guid that has no live entity on any shard (a stale socket, or a
    // character mid-shard-hop — `begin_transfer` deleted its entity).
    let err = whisper::run(world.as_ref(), 7, Some(4242), "Trin", "hi".into())
        .expect_err("a sender with no live entity anywhere is refused, as the module refuses it");
    assert!(
        err.to_string().contains("whisperer not in world"),
        "got {err}"
    );

    assert!(
        realm.realm_whispers.lock().unwrap().is_empty(),
        "neither refusal may reach the authority — and neither may be attributed to a guid the \
         gateway did not authenticate"
    );
}

/// **The invariant this batch has broken six times: unset config changes NOTHING.**
///
/// A single-database gateway has no realm-core to route to, so a whisper takes the pre-realm-core
/// path — the player's own connection, the player-facing `send_whisper` reducer, the TYPED NAME still
/// unresolved (the module resolves it, gates it and delivers it), and not one realm-wide read.
#[test]
fn an_unsharded_gateway_whispers_through_the_players_own_reducer() {
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

    // Three shapes the sharded plane treats differently, and the unsharded plane must not: a normal
    // whisper, a name nothing resolves, and a caller the gateway has no guid for. All three go to the
    // module verbatim — every gate on this plane is the module's, exactly as before realm-core.
    store
        .send_whisper(7, 0, "Vim".into(), "hi".into())
        .expect("baseline: the mock accepts a direct call");
    store.whispers.lock().unwrap().clear();
    calls.lock().unwrap().clear();

    whisper::run(store.as_ref(), 7, Some(GINGER), "vim", "hi".into())
        .expect("the legacy path answers");
    whisper::run(
        store.as_ref(),
        7,
        Some(GINGER),
        "Nobodyatall",
        "hello?".into(),
    )
    .expect("and it does not pre-resolve the name");
    whisper::run(store.as_ref(), 7, None, "vim", "still?".into()).expect("nor pre-gate the sender");

    assert_eq!(
        store.whispers.lock().unwrap().clone(),
        vec![
            ("vim".to_string(), "hi".to_string()),
            ("Nobodyatall".to_string(), "hello?".to_string()),
            ("vim".to_string(), "still?".to_string()),
        ],
        "an unsharded gateway must call `send_whisper` with the name the player TYPED, in order, \
         exactly as it did before realm-core — resolving or gating anything here would fork the \
         behaviour of the deployment nobody configured"
    );
    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter()
            .all(|(shard, call)| shard == "world" && call == "send_whisper"),
        "every call must land on the player's own database, through the player-facing reducer. \
         Calls were {log:?}"
    );
    assert!(
        !log.iter().any(|(_, c)| c == "realm_whisper"),
        "the realm plane must be untouched on a single-database gateway"
    );
}

/// **Two characters, one name, two databases** — found in review of this PR, and not a hypothetical:
/// the operator's live stack held two characters called `dfsdfsd` on 2026-07-25, guid 5 on the
/// instances shard and guid 8 on the world shard, after a suite re-created one the single-database
/// gateway could not see. `create_character`'s uniqueness constraint is a PER-DATABASE index, so
/// nothing stops it.
///
/// First-hit-wins then resolves the typed name against whichever homonym sits on the SENDER's own
/// shard, which produced two defects with every test in this file green:
/// - an OFFLINE homonym on the sender's shard shadows the live target entirely — `/w Vim` answers
///   "Vim is offline" while Vim stands in the dungeon, logged in. That is the very failure this slice
///   exists to remove, wearing the offline refusal's clothes;
/// - and with both live, a PRIVATE message goes to whichever one the sender happens to be co-located
///   with — reproduced: the same `/w Vim` reached guid 9 from the open world and guid 2 from inside
///   the instance.
///
/// The ONLINE gate is the disambiguator (vanilla `/w` addresses the character that is logged in), so
/// the union hands it every candidate. Several ONLINE homonyms remain arbitrary — the real fix is a
/// realm-wide name constraint, which belongs to realm-wide social & economy on realm-core, not this
/// slice's.
#[test]
fn a_homonym_on_the_senders_own_shard_does_not_shadow_the_live_target() {
    const HOMONYM: u64 = 9; // a SECOND character named "Vim", on the OPEN-WORLD shard
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
        characters: vec![character(GINGER, "Ginger"), character(HOMONYM, "Vim")],
        live_guids: vec![GINGER, HOMONYM],
        offline_guids: vec![HOMONYM], // logged out — the live Vim is the one on `instances`
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
    assert_eq!(
        party::resolve_by_name(world.as_ref(), "Vim").unwrap(),
        Some(HOMONYM),
        "fixture: first-hit-wins resolves the name to the sender's OWN shard's homonym"
    );

    whisper::run(world.as_ref(), 7, Some(GINGER), "Vim", "you inside?".into())
        .expect("a logged-out homonym must not refuse a whisper to the Vim who IS online");

    assert_eq!(
        realm.realm_whispers.lock().unwrap().clone(),
        vec![(GINGER, VIM, "you inside?".to_string(), false)],
        "the whisper must address the ONLINE Vim (guid {VIM}, on the instances shard), not the \
         logged-out homonym on the sender's own shard — and never both"
    );
}

/// …and when NO candidate is online, the refusal is still the offline one, naming what was typed.
#[test]
fn homonyms_that_are_all_offline_still_refuse_as_offline() {
    let (realm, world, _instances, _calls) = party_topology();
    let err = whisper::run(world.as_ref(), 7, Some(GINGER), "Dormant", "hi".into())
        .expect_err("no offline whispering, however many characters share the name");
    assert!(err.to_string().contains("Dormant is offline"), "got {err}");
    assert!(realm.realm_whispers.lock().unwrap().is_empty());
}

/// **AC: `/say`, `/yell` and targeted emotes stay shard-local.** They are spatial by nature, which is
/// the same partition rule that moved the whisper — which is not — onto realm-core. A sharded
/// topology must still run them on the player's own shard, with no realm-core involvement at all.
#[test]
fn say_yell_and_emotes_stay_on_the_players_own_shard_when_sharded() {
    let (realm, world, _instances, calls) = party_topology();

    world
        .send_chat(7, 0, 0, 0, "hello Elwynn".into())
        .expect("say");
    world.send_chat(7, 0, 1, 0, "HELP".into()).expect("yell");
    world.send_emote(7, 0, 4, 4, TRIN).expect("targeted emote");

    assert_eq!(
        world.chats.lock().unwrap().clone(),
        vec![
            (0, 0, "hello Elwynn".to_string()),
            (1, 0, "HELP".to_string())
        ],
        "say/yell must still be the shard's own broadcast rows"
    );
    let log = calls.lock().unwrap().clone();
    assert!(
        log.iter().all(|(shard, _)| shard == "world"),
        "no spatial chat may reach another database. Calls were {log:?}"
    );
    assert!(
        realm.realm_whispers.lock().unwrap().is_empty(),
        "the realm plane carries whispers only — routing proximity chat through it would break the \
         spec's partition rule (and range-scoping, which only the owning shard can do)"
    );
}

/// A cross-shard whisper arrives carrying the sender's GUID: `SMSG_MESSAGECHAT` has no name field for
/// a whisper, and the client resolves it over `CMSG_NAME_QUERY`. Answered from the sender's own shard
/// alone, that query has no row — so the line would render with nobody's name on it, which is a
/// whisper that "arrived" and cannot be read or replied to.
#[test]
fn a_name_query_resolves_a_character_on_another_shard() {
    let (_realm, world, instances, _calls) = party_topology();
    assert!(
        world.character_by_guid(VIM).unwrap().is_none(),
        "fixture: the asking session's shard has no row for a character inside the instance"
    );

    assert_eq!(
        party::character_anywhere(world.as_ref(), VIM)
            .unwrap()
            .map(|c| c.name),
        Some("Vim".to_string()),
        "CMSG_NAME_QUERY must resolve realm-wide, or every cross-boundary whisper renders nameless"
    );
    // …and from the other side, and still on the asking shard itself.
    assert_eq!(
        party::character_anywhere(instances.as_ref(), GINGER)
            .unwrap()
            .map(|c| c.name),
        Some("Ginger".to_string())
    );
    assert_eq!(
        party::character_anywhere(world.as_ref(), GINGER)
            .unwrap()
            .map(|c| c.name),
        Some("Ginger".to_string())
    );
    assert!(party::character_anywhere(world.as_ref(), 4242)
        .unwrap()
        .is_none());
}

/// The END-TO-END pin for the production CALL SITE, driven over a real socket through
/// `run_world_session`'s own dispatch rather than by calling `world::whisper` directly.
///
/// Two mutations live here and nowhere else. Deleting the dispatch's route to `world::whisper` sends
/// every whisper back to the shard-local reducer with every test above still green; and passing a
/// literal (or another player's guid) as the sender attributes the whisper — and the "X whispers:"
/// line the recipient sees — to somebody else, which is impersonation rather than a misroute.
/// Adversarial review found exactly that survivor in the party ops, so the sender guid realm-core
/// is told to act as is asserted here, from the socket that authenticated it.
#[test]
fn a_real_session_routes_a_whisper_to_realm_core_as_its_own_character() {
    let (realm, _world, instances, calls) = party_topology();
    // The session's own shard: Ginger lives here, Vim does NOT (they are on `instances`) — so the
    // whisper below can only resolve through the realm-wide union.
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
        characters: vec![character(GINGER, "Ginger")],
        live_guids: vec![GINGER],
        ..Default::default()
    });
    *session_shard.peers.lock().unwrap() = vec![session_shard.clone(), instances.clone()];
    *instances.peers.lock().unwrap() = vec![session_shard.clone(), instances.clone()];

    let (mut client, server_end) = world_session_socket_pair();
    let server_store = session_shard.clone();
    let server = std::thread::spawn(move || {
        run_world_session(server_end, server_store.as_ref()).unwrap();
    });
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN {
        guid: Guid::new(GINGER),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    for _ in 0..10 {
        if ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec).is_err() {
            break;
        }
    }

    // The client's own name resolution, for a guid it can only have met across the boundary. A
    // whisper's `SMSG_MESSAGECHAT` carries no name — the client asks for it with this opcode — so a
    // shard-local answer here renders every cross-shard whisper nameless and unreplyable.
    wow_world_messages::vanilla::CMSG_NAME_QUERY {
        guid: Guid::new(VIM),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // The whisper that could not be typed before this slice: a target on another database.
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Whisper {
            target_player: "vim".into(),
        },
        language: Language::Universal,
        message: "meet me inside".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    // The BARRIER, and an assertion in its own right: a whisper to a name NO shard can resolve must
    // still answer `SMSG_CHAT_PLAYER_NOT_FOUND` with the name the player typed — the same packet the
    // single-database plane sends. Seeing it also proves the whisper above was dispatched (one
    // thread, sequential dispatch).
    CMSG_MESSAGECHAT {
        chat_type: CMSG_MESSAGECHAT_ChatType::Whisper {
            target_player: "Nobodyatall".into(),
        },
        language: Language::Universal,
        message: "hello?".into(),
    }
    .write_encrypted_client(&mut client, &mut c_enc)
    .unwrap();
    let mut not_found: Option<String> = None;
    let mut queried_name: Option<String> = None;
    for _ in 0..8 {
        match ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec) {
            Ok(ServerOpcodeMessage::SMSG_NAME_QUERY_RESPONSE(m)) => {
                queried_name = Some(m.character_name.clone());
            }
            Ok(ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(m)) => {
                not_found = Some(m.name.clone());
                break;
            }
            Ok(_) => {}
            Err(_) => break, // the deadline fired — the assertions below say what was missing
        }
    }
    drop(client);
    drop(server);

    assert_eq!(
        queried_name.as_deref(),
        Some("Vim"),
        "CMSG_NAME_QUERY for a character on ANOTHER database went unanswered. The whisper would still          arrive, and render with nobody's name on it — the client resolves a whisper's sender itself"
    );

    assert_eq!(
        not_found.as_deref(),
        Some("Nobodyatall"),
        "an unresolvable whisper must still answer SMSG_CHAT_PLAYER_NOT_FOUND naming what the player \
         typed — the wire is unchanged on both planes"
    );
    assert_eq!(
        realm.realm_whispers.lock().unwrap().clone(),
        vec![(GINGER, VIM, "meet me inside".to_string(), false)],
        "the whisper must reach realm-core AS the character this socket authenticated into the world \
         with. The sender guid is an ARGUMENT on this plane, so it is the whole authorization: a \
         literal 0 — or another player's guid — whispers on somebody else's behalf, and the \
         recipient's client renders their name on the line"
    );
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter().any(|(_, c)| c == "send_whisper"),
        "the whisper ran through the session shard's own name lookup — the shard-local behaviour \
         realm-wide whisper routing removes. Calls were {log:?}"
    );
}
