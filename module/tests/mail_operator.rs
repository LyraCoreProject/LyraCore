mod support;

use support::{actor, Standalone};

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn mail_requests_refuse_a_non_operator_before_touching_durable_state() {
    let mut shard = Standalone::start("mail-operator-boundary");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let request_actor = actor("1");
    let requests: &[(&str, &[&str])] = &[
        ("realm_mail_mark_read", &[&request_actor, "1"]),
        (
            "realm_mail_send",
            &[
                &request_actor,
                "1",
                "\"boundary letter\"",
                "\"boundary letter\"",
                "1",
                "1",
                "1",
                "false",
            ],
        ),
        ("realm_mail_take_item", &[&request_actor, "1"]),
        ("realm_mail_take_money", &[&request_actor, "1"]),
        ("realm_mail_item_room", &[&request_actor]),
        ("realm_mail_delete", &[&request_actor, "1"]),
        ("realm_mail_return", &[&request_actor, "1", "false"]),
        (
            "realm_mail_fence",
            &[
                "1",
                &request_actor,
                "1",
                "\"boundary letter\"",
                "\"boundary letter\"",
                "1",
                "1",
                "1",
                "1",
                "1",
                "false",
            ],
        ),
        (
            "realm_mail_commit",
            &[
                "1",
                &request_actor,
                "1",
                "\"boundary letter\"",
                "\"boundary letter\"",
                "1",
                "1",
                "1",
                "1",
                "1",
                "false",
                "1",
                "1",
                "1",
                "1",
                "1",
                "1",
                "1",
                "1",
            ],
        ),
        (
            "realm_mail_take_money_fence",
            &["1", &request_actor, "1", "1"],
        ),
        ("realm_mail_payout", &["1", &request_actor, "1", "1"]),
        (
            "realm_mail_take_item_fence",
            &["1", &request_actor, "1", "1"],
        ),
        (
            "realm_mail_item_payout",
            &[
                "1",
                &request_actor,
                "1",
                "1",
                "1",
                "1",
                "1",
                "false",
                "1",
                "1",
            ],
        ),
        ("realm_mail_confirm_delivery", &["1", &request_actor]),
        ("realm_mail_settle", &["1", &request_actor]),
        ("realm_mail_copy_text", &[&request_actor, "1"]),
        ("gw_mail_grant_letter", &[&request_actor, "1"]),
        ("realm_mail_mark_letter_granted", &[&request_actor, "1"]),
    ];
    let tables = [
        "game_mail",
        "game_mail_escrow",
        "game_item_instance",
        "game_item_text",
    ];
    let before: Vec<_> = tables
        .iter()
        .map(|table| shard.query_rows(&format!("SELECT * FROM {table}")))
        .collect();
    let purse = shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1");
    for (reducer, args) in requests {
        let refused = shard.call_anonymous(reducer, args);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&refused.stdout),
            String::from_utf8_lossy(&refused.stderr)
        );
        assert!(!refused.status.success(), "{reducer}: {text}");
        assert!(text.contains("operator only"), "{reducer}: {text}");
        for (table, expected) in tables.iter().zip(&before) {
            assert_eq!(
                &shard.query_rows(&format!("SELECT * FROM {table}")),
                expected,
                "{reducer} changed {table}"
            );
        }
        assert_eq!(
            shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1"),
            purse
        );
    }
}
