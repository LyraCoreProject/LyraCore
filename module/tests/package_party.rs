mod support;

use support::Standalone;

fn fixture(name: &str) -> (Standalone, Vec<u64>) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let mut members = Vec::new();
    for name in ["Partya", "Partyb", "Partyc", "Partyd", "Partye"] {
        shard.assert_call(
            "create_character",
            &[
                "1",
                &format!("\"{name}\""),
                "1",
                "1",
                "0",
                "0",
                "0",
                "0",
                "0",
                "0",
            ],
        );
        let guid = shard.query_rows(&format!(
            "SELECT guid FROM game_character WHERE name = '{name}'"
        ))[0]["guid"]
            .clone();
        shard.assert_call("debug_spawn_player_entity", &[&guid]);
        members.push(guid.parse().unwrap());
    }
    (shard, members)
}

fn certify(shard: &Standalone, gateway: &str, single: bool) {
    shard.assert_call(
        "gw_certify_package_party_authority",
        &[
            &format!("\"{gateway}\""),
            if single { "true" } else { "false" },
        ],
    );
}

fn form(shard: &Standalone, members: &[u64]) {
    shard.assert_call(
        "debug_package_form_party",
        &[&serde_json::to_string(members).unwrap()],
    );
}

fn refused(shard: &Standalone, members: &[u64], reason: &str) {
    let before = [
        "game_group",
        "game_group_member",
        "game_group_roster_revision",
    ]
    .map(|table| shard.query_rows(&format!("SELECT * FROM {table}")));
    let result = shard.call(
        "debug_package_form_party",
        &[&serde_json::to_string(members).unwrap()],
    );
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(reason),
        "{output}"
    );
    for (table, before) in [
        "game_group",
        "game_group_member",
        "game_group_roster_revision",
    ]
    .into_iter()
    .zip(before)
    {
        assert_eq!(shard.query_rows(&format!("SELECT * FROM {table}")), before);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn package_parties_require_current_unanimous_single_shard_authority() {
    let (shard, members) = fixture("package-party-authority");
    refused(&shard, &members, "certified single-Shard Realm");
    certify(&shard, "one", true);
    shard.assert_sql("UPDATE game_package_party_authority SET expires_micros = 0");
    refused(&shard, &members, "certified single-Shard Realm");
    certify(&shard, "one", true);
    certify(&shard, "two", false);
    refused(&shard, &members, "certified single-Shard Realm");
    certify(&shard, "two", true);
    form(&shard, &members);
    assert_eq!(shard.query_rows("SELECT * FROM game_group").len(), 1);
    assert_eq!(shard.query_rows("SELECT * FROM game_group_member").len(), 5);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn package_party_formation_preserves_whole_premades_and_is_idempotent() {
    let (shard, members) = fixture("package-party-premades");
    certify(&shard, "one", true);
    form(&shard, &members[..2]);
    let retained = shard.query_rows("SELECT group_id FROM game_group")[0]["group_id"].clone();
    form(&shard, &members[2..4]);
    refused(
        &shard,
        &[members[0], members[2], members[4]],
        "every premade member",
    );
    form(&shard, &members);
    let groups = shard.query_rows("SELECT * FROM game_group");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["group_id"], retained);
    let roster = shard.query_rows("SELECT * FROM game_group_member");
    let revisions = shard.query_rows("SELECT * FROM game_group_roster_revision");
    assert_eq!(roster.len(), 5);
    assert_eq!(revisions.len(), 2);
    for revision in &revisions {
        assert_eq!(
            revision["active"],
            if revision["group_id"] == retained {
                "true"
            } else {
                "false"
            }
        );
    }
    form(&shard, &members);
    assert_eq!(shard.query_rows("SELECT * FROM game_group"), groups);
    assert_eq!(shard.query_rows("SELECT * FROM game_group_member"), roster);
    assert_eq!(
        shard.query_rows("SELECT * FROM game_group_roster_revision"),
        revisions
    );
}

fn replacement_character(shard: &Standalone, name: &str) -> u64 {
    shard.assert_call(
        "create_character",
        &[
            "1",
            &format!("\"{name}\""),
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let guid: u64 = shard.query_rows(&format!(
        "SELECT guid FROM game_character WHERE name = '{name}'"
    ))[0]["guid"]
        .parse()
        .unwrap();
    shard.assert_sql(&format!(
        "UPDATE game_character SET map_id = 189, online = true WHERE guid = {guid}"
    ));
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    guid
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn package_party_replacement_preserves_a_full_roster_and_refuses_a_foreign_party() {
    let (shard, members) = fixture("package-party-replacement");
    certify(&shard, "one", true);
    form(&shard, &members);
    let group = shard.query_rows("SELECT group_id FROM game_group")[0]["group_id"].clone();
    shard.assert_call(
        "debug_defense_create_instance",
        &["\"fixture\"", "189", &group],
    );
    let incoming = replacement_character(&shard, "Partyf");
    let revision: u64 = shard.query_rows(&format!(
        "SELECT revision FROM game_group_roster_revision WHERE group_id = {group}"
    ))[0]["revision"]
        .parse()
        .unwrap();
    let args = [
        "\"fixture\"".to_string(),
        group.clone(),
        members[0].to_string(),
        incoming.to_string(),
    ];
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    shard.assert_call("debug_package_replace_party_member", &args);
    let roster = shard.query_rows(&format!(
        "SELECT character_guid FROM game_group_member WHERE group_id = {group}"
    ));
    assert_eq!(roster.len(), 5);
    assert!(!roster
        .iter()
        .any(|member| member["character_guid"] == members[0].to_string()));
    assert!(roster
        .iter()
        .any(|member| member["character_guid"] == incoming.to_string()));
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT revision FROM game_group_roster_revision WHERE group_id = {group}"
        ))[0]["revision"],
        (revision + 1).to_string()
    );
    shard.assert_call("debug_package_replace_party_member", &args);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT revision FROM game_group_roster_revision WHERE group_id = {group}"
        ))[0]["revision"],
        (revision + 1).to_string()
    );
    let foreign = replacement_character(&shard, "Partyg");
    let peer = replacement_character(&shard, "Partyh");
    form(&shard, &[foreign, peer]);
    let before = shard.query_rows("SELECT * FROM game_group_member");
    let result = shard.call(
        "debug_package_replace_party_member",
        &[
            "\"fixture\"",
            &group,
            &members[1].to_string(),
            &foreign.to_string(),
        ],
    );
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("already belongs to a Party"));
    assert_eq!(shard.query_rows("SELECT * FROM game_group_member"), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn package_parties_refuse_duplicate_dead_and_other_partition_members() {
    let (shard, members) = fixture("package-party-members");
    certify(&shard, "one", true);
    refused(&shard, &[members[0], members[0]], "distinct Characters");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 0 WHERE guid = {}",
        members[4]
    ));
    refused(&shard, &members, "unavailable");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 1, instance_id = 999 WHERE guid = {}",
        members[4]
    ));
    refused(&shard, &members, "one partition");
}
