mod support;

use support::Standalone;

const GM: u64 = 5_090_001;
const LEADER: u64 = 5_090_002;
const OTHER: u64 = 5_090_003;

/// A tokenless actor: Realm-core holds no Account Claim for the reserved guids.
fn actor(guid: u64) -> String {
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

fn gm_create(leader: u64, leader_name: &str, gm_level: u8, name: &str) -> String {
    format!(
        r#"{{"gmCreate":{{"leader_guid":{leader},"leader_name":"{leader_name}","leader_team":469,"leader_realm_account":{leader},"gm_level":{gm_level},"name":"{name}"}}}}"#
    )
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

fn guild_rows(realm: &Standalone) -> (usize, usize, usize) {
    (
        realm.query_rows("SELECT * FROM game_guild").len(),
        realm.query_rows("SELECT * FROM game_guild_rank").len(),
        realm.query_rows("SELECT * FROM game_guild_member").len(),
    )
}

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_gm_founds_a_guild_on_realm_core_and_refusals_change_nothing() {
    let mut realm = Standalone::start("guild-core");
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);

    let gm = actor(GM);
    realm.assert_call(
        "realm_guild_op",
        &[&gm, &gm_create(LEADER, "Leader", 1, "Tracer Guild")],
    );

    let guilds = realm.query_rows("SELECT * FROM game_guild");
    assert_eq!(guilds.len(), 1);
    let guild = &guilds[0];
    assert_eq!(guild["name"], "Tracer Guild");
    assert_eq!(guild["name_key"], "tracer guild");
    assert_eq!(guild["leader_guid"], LEADER.to_string());
    assert_eq!(guild["team"], "469");
    assert_eq!(guild["motd"], "No message set.");
    let guild_id = guild["guild_id"].clone();

    let ranks = realm.query_rows(&format!(
        "SELECT * FROM game_guild_rank WHERE guild_id = {guild_id}"
    ));
    let mut ranks: Vec<(u32, String, String)> = ranks
        .iter()
        .map(|rank| {
            (
                rank["rank_id"].parse().unwrap(),
                rank["name"].clone(),
                rank["rights"].clone(),
            )
        })
        .collect();
    ranks.sort();
    assert_eq!(
        ranks,
        vec![
            (0, "Guild Master".into(), 0xF_F1FF.to_string()),
            (1, "Officer".into(), 0xF_F1FF.to_string()),
            (2, "Veteran".into(), 0x43.to_string()),
            (3, "Member".into(), 0x43.to_string()),
            (4, "Initiate".into(), 0x43.to_string()),
        ]
    );

    let members = realm.query_rows("SELECT * FROM game_guild_member");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["character_guid"], LEADER.to_string());
    assert_eq!(members[0]["guild_id"], guild_id);
    assert_eq!(members[0]["rank_id"], "0");
    assert_eq!(members[0]["name"], "Leader");
    assert_eq!(members[0]["realm_account_id"], LEADER.to_string());

    let founded = guild_rows(&realm);
    assert_eq!(founded, (1, 5, 1));
    refused(
        &realm,
        &[&gm, &gm_create(OTHER, "Other", 1, "tracer guild")],
        "guild:name_exists",
    );
    refused(
        &realm,
        &[&gm, &gm_create(OTHER, "Other", 0, "Knights")],
        "guild:not_game_master",
    );
    refused(
        &realm,
        &[&gm, &gm_create(LEADER, "Leader", 1, "Knights")],
        "guild:already_in_guild",
    );
    refused(
        &realm,
        &[&gm, &gm_create(OTHER, "Other", 1, "Knights!")],
        "guild:name_invalid",
    );
    assert_eq!(guild_rows(&realm), founded);

    realm.assert_call(
        "realm_guild_op",
        &[&actor(LEADER), r#"{"signOn":"Renamed"}"#],
    );
    assert_eq!(
        realm.query_rows("SELECT * FROM game_guild_member")[0]["name"],
        "Renamed"
    );
    realm.assert_call("realm_guild_op", &[&actor(LEADER), r#"{"signOff":{}}"#]);
    refused(
        &realm,
        &[&actor(OTHER), r#"{"signOn":"Other"}"#],
        "guild:not_in_guild",
    );
}
