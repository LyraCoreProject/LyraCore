//! `package_account::create_package_character` against a real Shard, through
//! `debug_create_package_character`.

mod support;

use support::Standalone;

const PACKAGE: &str = "example";

fn arg(value: &str) -> String {
    serde_json::to_string(value).expect("a string encodes as JSON")
}

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard
}

fn create(shard: &Standalone, name: &str, race: &str, class: &str) -> std::process::Output {
    shard.call(
        "debug_create_package_character",
        &[&arg(PACKAGE), &arg(name), race, class],
    )
}

fn assert_refusal(shard: &Standalone, name: &str, race: &str, class: &str, tag: &str) {
    let output = create(shard, name, race, class);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{name} was created: {text}");
    assert!(text.contains(tag), "{name}: expected {tag}, got {text}");
}

fn package_accounts(shard: &Standalone) -> Vec<String> {
    shard
        .query_rows(&format!(
            "SELECT account_id FROM game_package_account WHERE package_name = '{PACKAGE}'"
        ))
        .into_iter()
        .map(|row| row["account_id"].clone())
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_package_character_lands_on_an_account_that_records_its_package() {
    let shard = stage("package-account-owner");
    for index in 0..11 {
        let output = create(&shard, &format!("Bot{}", (b'a' + index) as char), "1", "1");
        assert!(
            output.status.success(),
            "creation {index} failed: {output:?}"
        );
    }

    let owned = package_accounts(&shard);
    assert_eq!(
        owned.len(),
        2,
        "the eleventh Character needs a second Account: {owned:?}"
    );
    for account_id in &owned {
        let account = shard.query_rows(&format!(
            "SELECT username, verifier FROM game_account WHERE id = {account_id}"
        ));
        assert_eq!(account.len(), 1, "owned Account {account_id} is missing");
        assert!(
            account[0]["username"].starts_with("example#"),
            "unexpected username: {account:?}"
        );
        assert_eq!(
            account[0]["verifier"], "0x",
            "a Package Account has no credentials"
        );
    }
    let first = shard.query_rows(&format!(
        "SELECT guid FROM game_character WHERE account_id = {}",
        owned
            .iter()
            .min_by_key(|id| id.parse::<u64>().unwrap())
            .unwrap()
    ));
    assert_eq!(
        first.len(),
        10,
        "the first Account fills before another is made"
    );
    let session = shard.query_rows("SELECT account_id FROM game_session");
    assert!(session.is_empty(), "creation opened a Session: {session:?}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_package_character_meets_the_client_name_race_and_class_refusals() {
    let shard = stage("package-account-refusals");
    shard.assert_call(
        "create_character",
        &["1", &arg("Taken"), "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    // Human Warrior is the one legal pair, so Human Shaman is illegal.
    shard
        .assert_sql("INSERT INTO game_char_base_info (race_class, race, class) VALUES (257, 1, 1)");

    assert_refusal(&shard, "Taken", "1", "1", "NAME_IN_USE");
    assert_refusal(&shard, "Shaman", "1", "7", "INVALID_RACE_CLASS");
    assert!(
        package_accounts(&shard).is_empty(),
        "a Refusal created a Package Account"
    );

    let output = create(&shard, "Warrior", "1", "1");
    assert!(
        output.status.success(),
        "a legal pair was refused: {output:?}"
    );
}
