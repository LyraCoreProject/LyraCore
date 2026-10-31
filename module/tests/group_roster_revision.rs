mod support;

use lyracore_shared::group::{event_kind, realm_op};
use serde_json::json;
use support::Standalone;

const HUMAN: u8 = 1;
const ORC: u8 = 2;
const WARRIOR: u8 = 1;
const STONE_AT: (f32, f32, f32) = (-8949.95, -132.493, 83.5312);

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    realm.clear_schedule("game_event_reaper_schedule");
    realm
}

fn stage(realm: &Standalone, guid: u64, race: u8) {
    let (x, y, z) = STONE_AT;
    realm.assert_call(
        "debug_stage_meeting_stone_character",
        &[
            &guid.to_string(),
            &guid.to_string(),
            &race.to_string(),
            &WARRIOR.to_string(),
            "18",
            "0",
            &x.to_string(),
            &y.to_string(),
            &z.to_string(),
        ],
    );
}

fn session(guid: u64) -> String {
    json!({
        "guid": guid,
        "ownership": { "some": { "account_id": guid, "generation": 1, "request_nonce": guid } },
    })
    .to_string()
}

fn realm_group_op(realm: &Standalone, op: u8, actor_guid: u64, target_guid: u64) {
    realm.assert_call(
        "realm_group_op",
        &[
            &op.to_string(),
            &session(actor_guid),
            &target_guid.to_string(),
            "0",
            "0",
            "0",
        ],
    );
}

fn package_invite_and_accept(realm: &Standalone, leader: u64, joiner: u64) {
    realm.assert_call("gw_group_invite", &[&session(leader), &joiner.to_string()]);
    realm.assert_call("gw_accept_group_invite", &[&session(joiner)]);
}

fn group_of(realm: &Standalone, guid: u64) -> u64 {
    realm.query_rows(&format!(
        "SELECT group_id FROM game_group_member WHERE character_guid = {guid}"
    ))[0]["group_id"]
        .parse()
        .unwrap()
}

fn roster_revision(realm: &Standalone, group_id: u64) -> Option<(u64, bool)> {
    realm
        .query_rows(&format!(
            "SELECT revision, active FROM game_group_roster_revision WHERE group_id = {group_id}"
        ))
        .first()
        .map(|row| (row["revision"].parse().unwrap(), row["active"] == "true"))
}

fn rosters_heard(realm: &Standalone, guid: u64) -> usize {
    realm
        .query_rows(&format!(
            "SELECT id FROM game_group_event WHERE recipient_guid = {guid} AND kind = {}",
            event_kind::LIST
        ))
        .len()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_acceptance_forms_a_party_at_the_first_roster_revision_like_a_realm_acceptance() {
    let realm = start("group-roster-package-accept");
    const REALM_LEADER: u64 = 509_7100;
    const REALM_JOINER: u64 = 509_7101;
    const PACKAGE_LEADER: u64 = 509_7102;
    const PACKAGE_JOINER: u64 = 509_7103;
    for guid in [REALM_LEADER, REALM_JOINER, PACKAGE_LEADER, PACKAGE_JOINER] {
        stage(&realm, guid, HUMAN);
    }

    realm_group_op(&realm, realm_op::INVITE, REALM_LEADER, REALM_JOINER);
    realm_group_op(&realm, realm_op::ACCEPT, REALM_JOINER, 0);
    package_invite_and_accept(&realm, PACKAGE_LEADER, PACKAGE_JOINER);

    let realm_party = group_of(&realm, REALM_JOINER);
    let package_party = group_of(&realm, PACKAGE_JOINER);
    assert_eq!(group_of(&realm, PACKAGE_LEADER), package_party);
    assert_eq!(roster_revision(&realm, realm_party), Some((1, true)));
    assert_eq!(roster_revision(&realm, package_party), Some((1, true)));
    assert_eq!(rosters_heard(&realm, PACKAGE_LEADER), 1);
    assert_eq!(rosters_heard(&realm, PACKAGE_JOINER), 1);
}

fn party_of_three(realm: &Standalone, leader: u64, joiners: [u64; 2]) -> u64 {
    for guid in std::iter::once(leader).chain(joiners) {
        stage(realm, guid, HUMAN);
    }
    for joiner in joiners {
        realm_group_op(realm, realm_op::INVITE, leader, joiner);
        realm_group_op(realm, realm_op::ACCEPT, joiner, 0);
    }
    let party = group_of(realm, leader);
    assert_eq!(roster_revision(realm, party), Some((2, true)));
    party
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_departure_advances_the_roster_revision_of_the_surviving_party() {
    let realm = start("group-roster-package-leave");
    const LEADER: u64 = 509_7110;
    const LEAVER: u64 = 509_7111;
    let party = party_of_three(&realm, LEADER, [LEAVER, 509_7112]);

    realm.assert_call("gw_group_leave", &[&session(LEAVER)]);

    assert_eq!(roster_revision(&realm, party), Some((3, true)));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn deleting_a_member_advances_the_roster_revision_of_the_surviving_party() {
    let realm = start("group-roster-character-deleted");
    const LEADER: u64 = 509_7130;
    const DELETED: u64 = 509_7131;
    let party = party_of_three(&realm, LEADER, [DELETED, 509_7132]);

    realm.assert_call("debug_delete_character", &[&DELETED.to_string()]);

    assert_eq!(roster_revision(&realm, party), Some((3, true)));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_cross_faction_acceptance_through_the_package_path_is_refused() {
    let realm = start("group-roster-package-faction");
    const ALLIANCE_LEADER: u64 = 509_7120;
    const HORDE_JOINER: u64 = 509_7121;
    stage(&realm, ALLIANCE_LEADER, HUMAN);
    stage(&realm, HORDE_JOINER, ORC);
    realm_group_op(&realm, realm_op::INVITE, ALLIANCE_LEADER, HORDE_JOINER);
    let invitation = realm.query_rows("SELECT * FROM game_group_invite");
    assert_eq!(invitation.len(), 1);

    let output = realm.call("gw_accept_group_invite", &[&session(HORDE_JOINER)]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(!output.status.success(), "acceptance committed: {text}");
    assert!(text.contains("group:wrong_faction"), "{text}");
    assert_eq!(
        realm.query_rows("SELECT * FROM game_group_invite"),
        invitation
    );
    assert!(realm
        .query_rows("SELECT * FROM game_group_member")
        .is_empty());
    assert!(realm
        .query_rows("SELECT * FROM game_group_roster_revision")
        .is_empty());
}
