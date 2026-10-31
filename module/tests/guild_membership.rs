mod support;

use support::Standalone;

const TEAM_ALLIANCE: u32 = 469;
const TEAM_HORDE: u32 = 67;

const GM: u64 = 5_091_001;
const LEADER: u64 = 5_091_002;
const BOB: u64 = 5_091_003;
const DAVE: u64 = 5_091_004;
const EVE: u64 = 5_091_005;
const FRANK: u64 = 5_091_006;
const GRACE: u64 = 5_091_007;
const HORDE_TARGET: u64 = 5_091_008;

fn actor(guid: u64) -> String {
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

fn gm_create(leader: u64, leader_name: &str, name: &str) -> String {
    format!(
        r#"{{"gmCreate":{{"leader_guid":{leader},"leader_name":"{leader_name}","leader_team":{TEAM_ALLIANCE},"leader_realm_account":{leader},"gm_level":1,"name":"{name}"}}}}"#
    )
}

fn invite(target: u64, actor_team: u32, target_team: u32, target_ignores_actor: bool) -> String {
    format!(
        r#"{{"invite":{{"target_guid":{target},"actor_team":{actor_team},"target_team":{target_team},"target_ignores_actor":{target_ignores_actor}}}}}"#
    )
}

fn accept(name: &str, team: u32) -> String {
    format!(r#"{{"accept":{{"actor_name":"{name}","actor_team":{team}}}}}"#)
}

fn decline(name: &str) -> String {
    format!(r#"{{"decline":"{name}"}}"#)
}

const LEAVE: &str = r#"{"leave":{}}"#;
const DISBAND: &str = r#"{"disband":{}}"#;

fn remove(target: u64) -> String {
    format!(r#"{{"remove":{target}}}"#)
}

fn promote(target: u64) -> String {
    format!(r#"{{"promote":{target}}}"#)
}

fn demote(target: u64) -> String {
    format!(r#"{{"demote":{target}}}"#)
}

fn set_leader(target: u64) -> String {
    format!(r#"{{"setLeader":{target}}}"#)
}

fn refused(realm: &Standalone, args: &[&str], tag: &str) {
    let result = realm.call("realm_guild_op", args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "expected {tag}: {output}"
    );
}

fn member_row(realm: &Standalone, guid: u64) -> Option<std::collections::BTreeMap<String, String>> {
    let rows = realm.query_rows(&format!(
        "SELECT * FROM game_guild_member WHERE character_guid = {guid}"
    ));
    rows.into_iter().next()
}

fn guild_id_of(realm: &Standalone, name: &str) -> String {
    realm.query_rows(&format!("SELECT * FROM game_guild WHERE name = '{name}'"))[0]["guild_id"]
        .clone()
}

fn invite_and_accept_phase(realm: &Standalone, guild_id: &str) {
    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(LEADER),
            &invite(BOB, TEAM_ALLIANCE, TEAM_ALLIANCE, false),
        ],
    );
    let invites = realm.query_rows(&format!(
        "SELECT * FROM game_guild_invite WHERE target_guid = {BOB}"
    ));
    assert_eq!(invites.len(), 1);
    assert_eq!(invites[0]["guild_id"], guild_id);
    assert_eq!(invites[0]["inviter_guid"], LEADER.to_string());

    refused(
        realm,
        &[
            &actor(LEADER),
            &invite(BOB, TEAM_ALLIANCE, TEAM_ALLIANCE, false),
        ],
        "guild:already_invited",
    );

    refused(
        realm,
        &[
            &actor(LEADER),
            &invite(HORDE_TARGET, TEAM_ALLIANCE, TEAM_HORDE, false),
        ],
        "guild:not_allied",
    );
    assert!(realm
        .query_rows(&format!(
            "SELECT * FROM game_guild_invite WHERE target_guid = {HORDE_TARGET}"
        ))
        .is_empty());

    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(LEADER),
            &invite(DAVE, TEAM_ALLIANCE, TEAM_ALLIANCE, true),
        ],
    );
    assert!(realm
        .query_rows(&format!(
            "SELECT * FROM game_guild_invite WHERE target_guid = {DAVE}"
        ))
        .is_empty());

    realm.assert_call(
        "realm_guild_op",
        &[&actor(BOB), &accept("Bob", TEAM_ALLIANCE)],
    );
    let bob = member_row(realm, BOB).expect("Bob joined");
    assert_eq!(bob["guild_id"], guild_id);
    assert_eq!(bob["rank_id"], "4");
    assert_eq!(bob["name"], "Bob");
    assert!(realm
        .query_rows(&format!(
            "SELECT * FROM game_guild_invite WHERE target_guid = {BOB}"
        ))
        .is_empty());

    refused(
        realm,
        &[&actor(BOB), &accept("Bob", TEAM_ALLIANCE)],
        "guild:no_pending_invite",
    );

    refused(
        realm,
        &[
            &actor(LEADER),
            &invite(BOB, TEAM_ALLIANCE, TEAM_ALLIANCE, false),
        ],
        "guild:already_in_guild",
    );
}

fn decline_then_join_phase(realm: &Standalone) {
    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(LEADER),
            &invite(DAVE, TEAM_ALLIANCE, TEAM_ALLIANCE, false),
        ],
    );
    realm.assert_call("realm_guild_op", &[&actor(DAVE), &decline("Dave")]);
    assert!(realm
        .query_rows(&format!(
            "SELECT * FROM game_guild_invite WHERE target_guid = {DAVE}"
        ))
        .is_empty());
    assert!(member_row(realm, DAVE).is_none());
    let decline_events = realm.query_rows(&format!(
        "SELECT * FROM game_guild_event WHERE recipient_guid = {LEADER} AND kind = 65"
    ));
    assert_eq!(decline_events.len(), 1, "0x41 == 65");
    assert!(decline_events[0]["strings"].contains("Dave"));

    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(LEADER),
            &invite(DAVE, TEAM_ALLIANCE, TEAM_ALLIANCE, false),
        ],
    );
    realm.assert_call(
        "realm_guild_op",
        &[&actor(DAVE), &accept("Dave", TEAM_ALLIANCE)],
    );
    assert!(member_row(realm, DAVE).is_some());
}

fn promote_and_demote_phase(realm: &Standalone) {
    realm.assert_call("realm_guild_op", &[&actor(LEADER), &promote(BOB)]);
    assert_eq!(member_row(realm, BOB).unwrap()["rank_id"], "3");
    let promotion_events =
        realm.query_rows("SELECT * FROM game_guild_event WHERE kind = 0 AND recipient_guid = 0");
    assert!(promotion_events
        .iter()
        .any(|row| row["strings"].contains("Member")));

    realm.assert_call("realm_guild_op", &[&actor(LEADER), &promote(BOB)]);
    realm.assert_call("realm_guild_op", &[&actor(LEADER), &promote(BOB)]);
    realm.assert_call("realm_guild_op", &[&actor(LEADER), &promote(DAVE)]);
    realm.assert_call("realm_guild_op", &[&actor(LEADER), &promote(DAVE)]);
    refused(realm, &[&actor(BOB), &promote(DAVE)], "guild:rank_too_high");

    realm.assert_call("realm_guild_op", &[&actor(LEADER), &demote(BOB)]);
    realm.assert_call("realm_guild_op", &[&actor(LEADER), &demote(BOB)]);
    realm.assert_call("realm_guild_op", &[&actor(LEADER), &demote(BOB)]);
    assert_eq!(member_row(realm, BOB).unwrap()["rank_id"], "4");
    realm.assert_call("realm_guild_op", &[&actor(LEADER), &demote(DAVE)]);
    realm.assert_call("realm_guild_op", &[&actor(LEADER), &demote(DAVE)]);

    refused(
        realm,
        &[&actor(LEADER), &demote(DAVE)],
        "guild:rank_too_low",
    );
}

fn remove_and_set_leader_phase(realm: &Standalone, guild_id: &str) {
    refused(realm, &[&actor(BOB), &remove(DAVE)], "guild:no_permission");

    realm.assert_call("realm_guild_op", &[&actor(LEADER), &remove(DAVE)]);
    assert!(member_row(realm, DAVE).is_none());

    refused(
        realm,
        &[&actor(LEADER), &remove(LEADER)],
        "guild:leader_cannot_leave",
    );

    realm.assert_call("realm_guild_op", &[&actor(LEADER), &set_leader(BOB)]);
    assert_eq!(member_row(realm, BOB).unwrap()["rank_id"], "0");
    assert_eq!(member_row(realm, LEADER).unwrap()["rank_id"], "1");
    assert_eq!(
        realm.query_rows(&format!(
            "SELECT * FROM game_guild WHERE guild_id = {guild_id}"
        ))[0]["leader_guid"],
        BOB.to_string()
    );
}

fn disband_phase(realm: &Standalone, guild_id: &str) {
    refused(realm, &[&actor(LEADER), DISBAND], "guild:not_leader");

    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(BOB),
            &invite(GM, TEAM_ALLIANCE, TEAM_ALLIANCE, false),
        ],
    );
    assert_eq!(
        realm
            .query_rows(&format!(
                "SELECT * FROM game_guild_invite WHERE target_guid = {GM}"
            ))
            .len(),
        1
    );

    realm.assert_call("realm_guild_op", &[&actor(BOB), DISBAND]);
    assert!(realm
        .query_rows(&format!(
            "SELECT * FROM game_guild WHERE guild_id = {guild_id}"
        ))
        .is_empty());
    assert!(realm
        .query_rows(&format!(
            "SELECT * FROM game_guild_rank WHERE guild_id = {guild_id}"
        ))
        .is_empty());
    assert!(realm
        .query_rows(&format!(
            "SELECT * FROM game_guild_member WHERE guild_id = {guild_id}"
        ))
        .is_empty());
    assert!(realm
        .query_rows(&format!(
            "SELECT * FROM game_guild_invite WHERE target_guid = {GM}"
        ))
        .is_empty());
    for member in [LEADER, BOB] {
        let disbanded = realm.query_rows(&format!(
            "SELECT * FROM game_guild_event WHERE kind = 8 AND recipient_guid = {member}"
        ));
        assert_eq!(
            disbanded.len(),
            1,
            "member {member} did not get a DISBANDED row"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn membership_moves_through_every_op_as_tokenless_actors() {
    let mut realm = Standalone::start("guild-membership");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);

    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &gm_create(LEADER, "Leader", "Tracer Guild")],
    );
    let guild_id = guild_id_of(&realm, "Tracer Guild");

    invite_and_accept_phase(&realm, &guild_id);
    decline_then_join_phase(&realm);
    promote_and_demote_phase(&realm);
    remove_and_set_leader_phase(&realm, &guild_id);
    disband_phase(&realm, &guild_id);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn leave_disbands_a_lone_leader_and_refuses_a_leader_with_company() {
    let mut realm = Standalone::start("guild-membership-leave");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);

    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &gm_create(EVE, "Eve", "Solo Guild")],
    );
    realm.assert_call("realm_guild_op", &[&actor(EVE), LEAVE]);
    assert!(realm
        .query_rows("SELECT * FROM game_guild WHERE name = 'Solo Guild'")
        .is_empty());

    realm.assert_call(
        "realm_guild_op",
        &[&actor(GM), &gm_create(FRANK, "Frank", "Company Guild")],
    );
    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(FRANK),
            &invite(GRACE, TEAM_ALLIANCE, TEAM_ALLIANCE, false),
        ],
    );
    realm.assert_call(
        "realm_guild_op",
        &[&actor(GRACE), &accept("Grace", TEAM_ALLIANCE)],
    );
    refused(&realm, &[&actor(FRANK), LEAVE], "guild:leader_cannot_leave");
    assert!(!realm
        .query_rows("SELECT * FROM game_guild WHERE name = 'Company Guild'")
        .is_empty());

    realm.assert_call("realm_guild_op", &[&actor(GRACE), LEAVE]);
    assert!(member_row(&realm, GRACE).is_none());
    assert!(!realm
        .query_rows("SELECT * FROM game_guild WHERE name = 'Company Guild'")
        .is_empty());
}
