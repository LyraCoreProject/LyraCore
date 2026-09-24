mod support;

use support::Standalone;

const TEAM_ALLIANCE: u32 = 469;
const TEAM_HORDE: u32 = 67;

/// The seeded Character, in the world: the Guild Charter's buyer and owner.
const OWNER: u64 = 1;
/// Nine tokenless signers. Their Realm Accounts are unknown (0), so none matches another.
const SIGNERS: [u64; 9] = [
    5_090_511, 5_090_512, 5_090_513, 5_090_514, 5_090_515, 5_090_516, 5_090_517, 5_090_518,
    5_090_519,
];
const TENTH_SIGNER: u64 = 5_090_520;
const HORDE_SIGNER: u64 = 5_090_521;
const GUILDED_SIGNER: u64 = 5_090_522;

/// Scenario vendor 51004, spawned at the owner's feet and made a Petitioner and a Tabard Designer.
const VENDOR_ENTRY: &str = "51004";
const PETITIONER_AND_TABARDDESIGNER: u32 = 0x200 | 0x400;
const GUILD_CHARTER: u32 = 5863;

/// A tokenless actor: Realm-core holds no Account Claim for these guids.
fn actor(guid: u64) -> String {
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

fn charter_request(npc_guid: &str, name: &str) -> String {
    format!(r#"{{"charter":{{"npc_guid":{npc_guid},"name":"{name}"}}}}"#)
}

fn charter_terms(charter_item_guid: &str, name: &str) -> String {
    format!(
        r#"{{"charter":{{"charter_item_guid":{charter_item_guid},"name":"{name}","payer_name":"Tester","payer_team":{TEAM_ALLIANCE}}}}}"#
    )
}

fn sign(charter_item_guid: &str, name: &str, team: u32) -> String {
    format!(
        r#"{{"signPetition":{{"charter_item_guid":{charter_item_guid},"actor_name":"{name}","actor_team":{team}}}}}"#
    )
}

fn turn_in(charter_item_guid: &str) -> String {
    format!(r#"{{"turnInPetition":{charter_item_guid}}}"#)
}

fn gm_create(leader: u64, name: &str) -> String {
    format!(
        r#"{{"gmCreate":{{"leader_guid":{leader},"leader_name":"Leader{leader}","leader_team":{TEAM_ALLIANCE},"leader_realm_account":0,"gm_level":1,"name":"{name}"}}}}"#
    )
}

fn refused(shard: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let result = shard.call(reducer, args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "{reducer}: expected {tag}: {output}"
    );
}

fn purse(shard: &Standalone) -> u32 {
    shard.query_rows(&format!(
        "SELECT money FROM game_world_entity WHERE guid = {OWNER}"
    ))[0]["money"]
        .parse()
        .unwrap()
}

fn charters(shard: &Standalone) -> Vec<String> {
    shard
        .query_rows(&format!(
            "SELECT guid FROM game_item_instance WHERE entry = {GUILD_CHARTER} AND owner_guid = {OWNER}"
        ))
        .into_iter()
        .map(|row| row["guid"].clone())
        .collect()
}

fn count(shard: &Standalone, table: &str) -> usize {
    shard.query_rows(&format!("SELECT * FROM {table}")).len()
}

/// The imported world has the Guild Charter template; a bare test database does not. Copy the
/// starter item 51 into entry 5863 only when it is absent.
fn stage_charter_template(shard: &Standalone) {
    let existing = shard.query_rows(&format!(
        "SELECT entry FROM game_item_template WHERE entry = {GUILD_CHARTER}"
    ));
    if !existing.is_empty() {
        return;
    }
    let mut row = shard
        .query_rows("SELECT * FROM game_item_template WHERE entry = 51")
        .remove(0);
    row.insert("entry".into(), GUILD_CHARTER.to_string());
    row.insert("name".into(), "Guild Charter".into());
    row.insert("max_stack".into(), "1".into());
    row.insert("max_count".into(), "0".into());
    let columns = "entry,class,subclass,name,display_id,quality,inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race,random_property";
    let values = columns
        .split(',')
        .map(|key| {
            if key == "name" {
                format!("'{}'", row[key])
            } else {
                row[key].clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    shard.assert_sql(&format!(
        "INSERT INTO game_item_template ({columns}) VALUES ({values})"
    ));
}

/// The owner in the world with `copper`, standing at a Petitioner that is also a Tabard Designer.
/// One database plays both the Home Shard and Realm-core. Answers the NPC's guid.
fn fixture(name: &str, copper: u32) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    // Guild Event rows live one second; the checks below read them back.
    shard.assert_sql("DELETE FROM game_event_reaper_schedule");
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    stage_charter_template(&shard);
    shard.assert_call("debug_spawn_player_entity", &[&OWNER.to_string()]);
    shard.assert_call(
        "debug_set_money",
        &[&OWNER.to_string(), &copper.to_string()],
    );
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&OWNER.to_string(), VENDOR_ENTRY, "1"],
    );
    let npc = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {VENDOR_ENTRY}"
    ))[0]["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {PETITIONER_AND_TABARDDESIGNER} WHERE guid = {npc}"
    ));
    (shard, npc)
}

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_charter_with_nine_signatures_founds_the_guild() {
    let (shard, npc) = fixture("guild-petition", 1_500);
    let owner = actor(OWNER);
    let request = charter_request(&npc, "Night Watch");

    // The hold takes ten silver and mints one Charter; a replay of the same hold takes nothing.
    shard.assert_call("gw_guild_fee_hold", &["5090501", &owner, &request]);
    shard.assert_call("gw_guild_fee_hold", &["5090501", &owner, &request]);
    assert_eq!(purse(&shard), 500);
    let [charter] = <[String; 1]>::try_from(charters(&shard)).expect("one Guild Charter");
    let hold = &shard.query_rows("SELECT * FROM game_guild_fee_hold")[0];
    assert_eq!(
        (hold["kind"].as_str(), hold["copper"].as_str()),
        ("2", "1000")
    );
    assert_eq!(hold["charter_item_guid"], charter);
    assert_eq!(hold["charter_name"], "Night Watch");

    let terms = charter_terms(&charter, "Night Watch");
    shard.assert_call("realm_guild_fee_decide", &["5090501", &owner, &terms]);
    shard.assert_call("realm_guild_fee_decide", &["5090501", &owner, &terms]);
    shard.assert_call("gw_guild_fee_finish", &["5090501", &owner, "true"]);
    assert_eq!(purse(&shard), 500);
    assert_eq!(count(&shard, "game_guild_fee_hold"), 0);
    let petitions = shard.query_rows("SELECT * FROM game_guild_petition");
    assert_eq!(petitions.len(), 1, "a replayed decision opens one Petition");
    let petition = &petitions[0];
    assert_eq!(petition["charter_item_guid"], charter);
    assert_eq!(petition["owner_guid"], OWNER.to_string());
    assert_eq!(petition["name"], "Night Watch");
    let decision = &shard.query_rows("SELECT * FROM game_guild_fee_decision")[0];
    assert_eq!(decision["accepted"], "true");
    assert_eq!(decision["petition_id"], petition["petition_id"]);

    // Refusals add no Signature.
    let own = sign(&charter, "Tester", TEAM_ALLIANCE);
    refused(
        &shard,
        "realm_guild_op",
        &[&owner, &own],
        "guild:cant_sign_own",
    );
    let horde = sign(&charter, "Horde", TEAM_HORDE);
    refused(
        &shard,
        "realm_guild_op",
        &[&actor(HORDE_SIGNER), &horde],
        "guild:not_allied",
    );
    shard.assert_call(
        "realm_guild_op",
        &[
            &actor(GUILDED_SIGNER),
            &gm_create(GUILDED_SIGNER, "Knights"),
        ],
    );
    let guilded = sign(&charter, "Guilded", TEAM_ALLIANCE);
    refused(
        &shard,
        "realm_guild_op",
        &[&actor(GUILDED_SIGNER), &guilded],
        "guild:already_in_guild",
    );
    assert_eq!(count(&shard, "game_guild_petition_signature"), 0);

    for (n, signer) in SIGNERS[..8].iter().enumerate() {
        let request = sign(&charter, &format!("Signer{n}"), TEAM_ALLIANCE);
        shard.assert_call("realm_guild_op", &[&actor(*signer), &request]);
    }
    refused(
        &shard,
        "realm_guild_op",
        &[&owner, &turn_in(&charter)],
        "guild:need_more_signatures",
    );
    let request = sign(&charter, "Signer8", TEAM_ALLIANCE);
    shard.assert_call("realm_guild_op", &[&actor(SIGNERS[8]), &request]);
    let tenth = sign(&charter, "Tenth", TEAM_ALLIANCE);
    refused(
        &shard,
        "realm_guild_op",
        &[&actor(TENTH_SIGNER), &tenth],
        "guild:petition_full",
    );
    assert_eq!(count(&shard, "game_guild_petition_signature"), 9);
    let signed_events = shard.query_rows(&format!(
        "SELECT * FROM game_guild_event WHERE kind = 113 AND recipient_guid = {OWNER}"
    ));
    assert_eq!(signed_events.len(), 9, "the owner hears each Signature");

    refused(
        &shard,
        "realm_guild_op",
        &[&actor(SIGNERS[0]), &turn_in(&charter)],
        "guild:not_petition_owner",
    );
    shard.assert_call("realm_guild_op", &[&owner, &turn_in(&charter)]);

    let guild = &shard.query_rows("SELECT * FROM game_guild WHERE name_key = 'night watch'")[0];
    assert_eq!(guild["leader_guid"], OWNER.to_string());
    let guild_id = guild["guild_id"].clone();
    let members = shard.query_rows(&format!(
        "SELECT * FROM game_guild_member WHERE guild_id = {guild_id}"
    ));
    assert_eq!(members.len(), 10);
    for member in &members {
        let expected_rank = if member["character_guid"] == OWNER.to_string() {
            "0"
        } else {
            "4"
        };
        assert_eq!(member["rank_id"], expected_rank, "{member:?}");
    }
    assert_eq!(count(&shard, "game_guild_petition"), 0);
    assert_eq!(count(&shard, "game_guild_petition_signature"), 0);
    let founders = shard.query_rows("SELECT * FROM game_guild_event WHERE kind = 116");
    assert_eq!(founders.len(), 9, "each signer hears FOUNDER");

    // The Charter is destroyed on the Home Shard after the founding; a retry finds it gone.
    shard.assert_call("gw_destroy_guild_charter", &[&owner, &charter]);
    shard.assert_call("gw_destroy_guild_charter", &[&owner, &charter]);
    assert!(charters(&shard).is_empty());
}

/// A refused Charter is destroyed and its copper returned; a signer who joins another Guild takes
/// its Signature away and the owner gets a fresh petition query.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_refused_charter_is_refunded_and_a_joining_signer_loses_its_signature() {
    let (shard, npc) = fixture("guild-petition-refusals", 999);
    let owner = actor(OWNER);
    let request = charter_request(&npc, "Night Watch");

    refused(
        &shard,
        "gw_guild_fee_hold",
        &["5090531", &owner, &request],
        "guild:not_enough_money",
    );
    assert_eq!((purse(&shard), charters(&shard).len()), (999, 0));
    shard.assert_call("debug_set_money", &[&OWNER.to_string(), "1500"]);

    // A Guild takes the name between the hold and the decision: the Charter goes back.
    shard.assert_call("gw_guild_fee_hold", &["5090532", &owner, &request]);
    shard.assert_call(
        "realm_guild_op",
        &[
            &actor(GUILDED_SIGNER),
            &gm_create(GUILDED_SIGNER, "Night Watch"),
        ],
    );
    let [charter] = <[String; 1]>::try_from(charters(&shard)).expect("one Guild Charter");
    let terms = charter_terms(&charter, "Night Watch");
    shard.assert_call("realm_guild_fee_decide", &["5090532", &owner, &terms]);
    let decision = &shard.query_rows("SELECT * FROM game_guild_fee_decision")[0];
    assert_eq!(decision["refusal"], "guild:name_exists");
    shard.assert_call("gw_guild_fee_finish", &["5090532", &owner, "false"]);
    shard.assert_call("gw_guild_fee_finish", &["5090532", &owner, "false"]);
    assert_eq!((purse(&shard), charters(&shard).len()), (1_500, 0));
    assert_eq!(count(&shard, "game_guild_petition"), 0);

    // A second name opens a Petition. One of its two signers joins another Guild.
    let owner_request = charter_request(&npc, "Day Watch");
    shard.assert_call("gw_guild_fee_hold", &["5090533", &owner, &owner_request]);
    let [charter] = <[String; 1]>::try_from(charters(&shard)).expect("one Guild Charter");
    let terms = charter_terms(&charter, "Day Watch");
    shard.assert_call("realm_guild_fee_decide", &["5090533", &owner, &terms]);
    shard.assert_call("gw_guild_fee_finish", &["5090533", &owner, "true"]);
    assert_eq!(purse(&shard), 500);
    for signer in &SIGNERS[..2] {
        let request = sign(&charter, "Signer", TEAM_ALLIANCE);
        shard.assert_call("realm_guild_op", &[&actor(*signer), &request]);
    }
    shard.assert_call(
        "realm_guild_op",
        &[&actor(SIGNERS[0]), &gm_create(SIGNERS[0], "Moon Watch")],
    );
    let left = shard.query_rows("SELECT signer_guid FROM game_guild_petition_signature");
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["signer_guid"], SIGNERS[1].to_string());
    let changed = shard.query_rows(&format!(
        "SELECT * FROM game_guild_event WHERE kind = 117 AND recipient_guid = {OWNER}"
    ));
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["subject_guid"], SIGNERS[0].to_string());
    assert_eq!(changed[0]["other_guid"], charter);

    // The Charter is lost while its Petition stays open. Realm-core refuses a second Petition, so
    // the new Charter goes back with its copper. The Gateway closes the stranded Petition first.
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE guid = {charter}"
    ));
    shard.assert_call("debug_set_money", &[&OWNER.to_string(), "1500"]);
    let second = buy_attempt_refused(&shard, &npc);
    assert_eq!(second, "guild:already_has_petition");
    assert_eq!((purse(&shard), charters(&shard).len()), (1_500, 0));
    let petition_id =
        shard.query_rows("SELECT petition_id FROM game_guild_petition")[0]["petition_id"].clone();
    let close = format!(r#"{{"closePetition":{petition_id}}}"#);
    shard.assert_call("realm_guild_op", &[&owner, &close]);
    assert_eq!(count(&shard, "game_guild_petition"), 0);
    assert_eq!(count(&shard, "game_guild_petition_signature"), 0);
}

/// Buy "Night Watch" while a Petition is still open. Answers the decision's refusal.
fn buy_attempt_refused(shard: &Standalone, npc: &str) -> String {
    let owner = actor(OWNER);
    let request = charter_request(npc, "Night Watch");
    shard.assert_call("gw_guild_fee_hold", &["5090534", &owner, &request]);
    let [charter] = <[String; 1]>::try_from(charters(shard)).expect("one Guild Charter");
    let terms = charter_terms(&charter, "Night Watch");
    shard.assert_call("realm_guild_fee_decide", &["5090534", &owner, &terms]);
    shard.assert_call("gw_guild_fee_finish", &["5090534", &owner, "false"]);
    shard.query_rows("SELECT refusal FROM game_guild_fee_decision WHERE operation_id = 5090534")[0]
        ["refusal"]
        .clone()
}
