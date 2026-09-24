//! A Guild member stays out of the GuildRecruitment channel and hears nothing about it
//! (`cm:Channel.cpp:94-95`); a Character outside every Guild joins it as usual.

mod support;

use serde_json::json;
use support::{actor, Standalone};

const MEMBER: u64 = 5_093_101;
const STRANGER: u64 = 5_093_102;
const JOIN: &str = "0";
const HUMAN: u8 = 1;

fn join(realm: &Standalone, guid: u64, channel: &str) {
    let request = json!({
        "channel_name": channel,
        "password": "",
        "target_guid": 0,
        "target_name": "",
        "target_race": 0,
        "target_ignores_actor": false,
        "speaker": { "race": HUMAN, "chat_tag": 0 },
    })
    .to_string();
    realm.assert_call(
        "realm_channel_op",
        &[&actor(&guid.to_string()), JOIN, &request],
    );
}

fn memberships(realm: &Standalone, guid: u64) -> usize {
    realm
        .query_rows(&format!(
            "SELECT * FROM game_chat_channel_member WHERE character_guid = {guid}"
        ))
        .len()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_guild_member_never_joins_guild_recruitment() {
    let mut realm = Standalone::start("guild-recruitment");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    realm.assert_call(
        "realm_guild_op",
        &[
            &actor(&MEMBER.to_string()),
            &format!(
                r#"{{"gmCreate":{{"leader_guid":{MEMBER},"leader_name":"Member","leader_team":469,"leader_realm_account":{MEMBER},"gm_level":1,"name":"Recruiters"}}}}"#
            ),
        ],
    );

    join(&realm, MEMBER, "GuildRecruitment - City");
    assert!(
        realm
            .query_rows("SELECT * FROM game_chat_channel WHERE builtin_id = 25")
            .is_empty(),
        "a refused first join creates no channel"
    );
    assert_eq!(
        memberships(&realm, MEMBER),
        0,
        "the join is silent and admits nobody"
    );
    join(&realm, STRANGER, "GuildRecruitment - City");
    assert_eq!(memberships(&realm, STRANGER), 1);

    // Every other channel still admits a Guild member.
    join(&realm, MEMBER, "LookingForGroup");
    assert_eq!(memberships(&realm, MEMBER), 1);
}
