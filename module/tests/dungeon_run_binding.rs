mod support;

use spacetimedb::Identity;
use support::{actor, group_op, Standalone};

const LEADER: u64 = 1;
const DEADMINES: u32 = 36;
const TO_DEADMINES: u32 = 5_272_001;

const INVITE: &str = "0";
const ACCEPT: &str = "1";
const LEAVE: &str = "3";
const SET_LEADER: &str = "7";

const GROUP: u64 = 5_272_100;
const ROSTER_REVISION: u64 = 1_000;
const TRANSFER: u64 = 5_272_200;

fn realm(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.insert_row(
        "game_map",
        &[
            ("id", &DEADMINES),
            ("internal_name", &"Fixture"),
            ("name", &"Fixture"),
            ("map_type", &1),
        ],
    );
    shard.insert_row(
        "game_area_trigger",
        &[
            ("id", &TO_DEADMINES),
            ("map_id", &0),
            ("x", &0.0f32),
            ("y", &0.0f32),
            ("z", &0.0f32),
            ("radius", &5.0f32),
        ],
    );
    shard.insert_row(
        "game_areatrigger_teleport",
        &[
            ("trigger_id", &TO_DEADMINES),
            ("target_map", &DEADMINES),
            ("x", &10.0f32),
            ("y", &20.0f32),
            ("z", &30.0f32),
            ("o", &0.0f32),
            ("name", &"Fixture entrance"),
        ],
    );
    shard.assert_call("debug_spawn_player_entity", &[&LEADER.to_string()]);
    shard
}

fn create_character(shard: &Standalone, name: &str) -> u64 {
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
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    guid
}

fn join(shard: &Standalone, inviter: u64, guid: u64) {
    group_op(shard, INVITE, inviter, guid, 0);
    group_op(shard, ACCEPT, guid, 0, 0);
}

fn group_of(shard: &Standalone, guid: u64) -> String {
    shard.query_rows(&format!(
        "SELECT group_id FROM game_group_member WHERE character_guid = {guid}"
    ))[0]["group_id"]
        .clone()
}

fn enter(shard: &Standalone, guid: u64) -> String {
    shard.assert_call(
        "debug_enter_areatrigger",
        &[&guid.to_string(), &TO_DEADMINES.to_string()],
    );
    let entity = shard
        .query_rows(&format!(
            "SELECT map_id, instance_id FROM game_world_entity WHERE guid = {guid}"
        ))
        .remove(0);
    assert_eq!(entity["map_id"], DEADMINES.to_string(), "{guid} entered");
    entity["instance_id"].clone()
}

fn walk_out(shard: &Standalone, guid: u64) {
    shard.assert_call(
        "debug_teleport",
        &[&guid.to_string(), "0", "0", "0", "0", "0"],
    );
}

fn owner_of(shard: &Standalone, run: &str) -> String {
    shard.query_rows(&format!(
        "SELECT party_id FROM game_instance WHERE instance_id = {run}"
    ))[0]["party_id"]
        .clone()
}

fn holder_of(shard: &Standalone, run: &str) -> u64 {
    shard.query_rows(&format!(
        "SELECT holder_guid FROM game_instance WHERE instance_id = {run}"
    ))[0]["holder_guid"]
        .parse()
        .unwrap()
}

fn holders_of(shard: &Standalone, run: &str) -> Vec<u64> {
    let mut holders: Vec<u64> = shard
        .query_rows(&format!(
            "SELECT character_guid FROM game_instance_binding WHERE instance_id = {run}"
        ))
        .into_iter()
        .map(|row| row["character_guid"].parse().unwrap())
        .collect();
    holders.sort_unstable();
    holders
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_leaders_solo_run_becomes_the_groups_run_when_the_group_forms() {
    let shard = realm("dungeon-run-formation");
    let member = create_character(&shard, "Member");
    let solo = enter(&shard, LEADER);
    walk_out(&shard, LEADER);

    join(&shard, LEADER, member);
    let group = group_of(&shard, LEADER);

    assert_eq!(
        owner_of(&shard, &solo),
        group,
        "the Group takes the leader's run"
    );
    assert_eq!(
        enter(&shard, member),
        solo,
        "a member who enters first resumes it"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_new_leader_takes_the_group_to_its_own_run_and_the_old_leader_keeps_the_groups_run() {
    let shard = realm("dungeon-run-leader-change");
    let heir = create_character(&shard, "Heir");
    let other = create_character(&shard, "Other");
    let heirs_run = enter(&shard, heir);
    join(&shard, LEADER, heir);
    join(&shard, LEADER, other);
    let group = group_of(&shard, LEADER);
    let groups_run = enter(&shard, LEADER);
    assert_ne!(groups_run, heirs_run, "the heir joined inside its own run");
    walk_out(&shard, LEADER);

    group_op(&shard, SET_LEADER, LEADER, heir, 0);

    assert_eq!(owner_of(&shard, &heirs_run), group);
    assert_eq!(owner_of(&shard, &groups_run), "0");
    assert_eq!(holder_of(&shard, &groups_run), LEADER);
    assert_eq!(holders_of(&shard, &groups_run), [LEADER]);
    assert_eq!(
        enter(&shard, other),
        heirs_run,
        "the Group enters the new leader's run"
    );
    group_op(&shard, LEAVE, LEADER, 0, 0);
    assert_eq!(
        enter(&shard, LEADER),
        groups_run,
        "the old leader keeps the Group's former run"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_last_member_in_the_groups_run_keeps_it_when_the_group_disbands() {
    let shard = realm("dungeon-run-disband");
    let survivor = create_character(&shard, "Survivor");
    join(&shard, LEADER, survivor);
    let run = enter(&shard, LEADER);
    assert_eq!(enter(&shard, survivor), run);
    walk_out(&shard, LEADER);

    group_op(&shard, LEAVE, LEADER, 0, 0);

    assert_eq!(owner_of(&shard, &run), "0", "the run is the survivor's own");
    assert_eq!(holder_of(&shard, &run), survivor);
    assert_eq!(holders_of(&shard, &run), [survivor]);
    walk_out(&shard, survivor);
    assert_eq!(enter(&shard, survivor), run);
    assert_ne!(enter(&shard, LEADER), run, "the leaver starts a fresh run");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_who_left_the_group_enters_a_fresh_run_alone() {
    let shard = realm("dungeon-run-left-group");
    let leaver = create_character(&shard, "Leaver");
    let stays = create_character(&shard, "Stays");
    join(&shard, LEADER, leaver);
    join(&shard, LEADER, stays);
    let group = group_of(&shard, LEADER);
    let run = enter(&shard, LEADER);
    assert_eq!(enter(&shard, leaver), run);

    walk_out(&shard, leaver);
    group_op(&shard, LEAVE, leaver, 0, 0);
    let fresh = enter(&shard, leaver);

    assert_ne!(fresh, run, "the Group's run stays with the Group");
    assert_eq!(owner_of(&shard, &fresh), "0", "the leaver's run is its own");
    assert_eq!(owner_of(&shard, &run), group);
    assert_eq!(enter(&shard, stays), run);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_who_joined_another_group_enters_that_groups_run() {
    let shard = realm("dungeon-run-other-group");
    let leaver = create_character(&shard, "Leaver");
    let stays = create_character(&shard, "Stays");
    let other_leader = create_character(&shard, "Other");
    join(&shard, LEADER, leaver);
    join(&shard, LEADER, stays);
    let run = enter(&shard, LEADER);
    assert_eq!(enter(&shard, leaver), run);

    walk_out(&shard, leaver);
    group_op(&shard, LEAVE, leaver, 0, 0);
    join(&shard, other_leader, leaver);
    let other_group = group_of(&shard, leaver);
    let other_run = enter(&shard, leaver);

    assert_ne!(other_run, run, "the former Group's run is not entered");
    assert_eq!(owner_of(&shard, &other_run), other_group);
    assert_eq!(enter(&shard, other_leader), other_run);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_who_joins_a_group_leaves_its_solo_run_behind() {
    let shard = realm("dungeon-run-join-group");
    let joiner = create_character(&shard, "Joiner");
    let solo = enter(&shard, joiner);
    walk_out(&shard, joiner);

    join(&shard, LEADER, joiner);
    let group = group_of(&shard, joiner);
    assert!(
        shard
            .query_rows(&format!(
                "SELECT id FROM game_instance_binding WHERE character_guid = {joiner}"
            ))
            .is_empty(),
        "joining the Group resets the joiner's own runs"
    );
    let run = enter(&shard, joiner);

    assert_ne!(run, solo, "the Group does not take the joiner's solo run");
    assert_eq!(owner_of(&shard, &run), group);
    assert_eq!(enter(&shard, LEADER), run);
}

// Mirrors the wire layouts in native/realm_party.rs and native/instance_shard.rs.
#[derive(spacetimedb::SpacetimeType)]
struct Member {
    guid: u64,
    raid_slot: u8,
}

#[derive(spacetimedb::SpacetimeType)]
struct Roster {
    group_id: u64,
    leader: u64,
    loot_method: u8,
    loot_threshold: u8,
    master: u64,
    kind: u8,
    members: Vec<Member>,
    icons: Vec<u8>,
    disband: Option<u8>,
}

#[derive(spacetimedb::SpacetimeType)]
struct PartyUpdate {
    revision: u64,
    characters: Vec<u8>,
    rosters: Vec<Roster>,
    events: Vec<u8>,
    absent: Vec<u64>,
    events_expire_micros: i64,
}

#[derive(spacetimedb::SpacetimeType)]
struct PartyCharacter {
    guid: u64,
    account_id: u64,
    connection: u128,
    world: Identity,
    name: String,
    race: u8,
    class: u8,
    chat_tag: u8,
    identity: Identity,
    online: bool,
    stats: Option<u8>,
    revision: u64,
}

#[derive(spacetimedb::SpacetimeType)]
struct EntryRequest {
    id: u64,
    actor: PartyCharacter,
    map: u32,
    binding: u64,
    trigger: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    expires: i64,
}

#[derive(spacetimedb::SpacetimeType)]
struct Admission {
    request: EntryRequest,
    binding_world: Option<Identity>,
    queued_group: Option<u64>,
}

#[derive(spacetimedb::SpacetimeType)]
struct AdmissionParty {
    group: u64,
    snapshot: Vec<u8>,
}

fn bsatn<T: spacetimedb::sats::Serialize>(value: &T) -> String {
    serde_json::to_string(&spacetimedb::sats::bsatn::to_vec(value).unwrap()).unwrap()
}

/// Makes the test's Operator the Realm Shard of `shard`, so the test sends its Group projections.
fn serve_the_operator_realm(shard: &Standalone) -> String {
    let operator = shard.query_rows("SELECT identity FROM game_operator")[0]["identity"].clone();
    let realm = format!(r#"{{"__identity__":"{operator}"}}"#);
    shard.assert_call("operator_add_peer_shard", &[&realm, "\"realm\""]);
    shard.assert_call(
        "operator_set_shard_role",
        &[&format!(r#"{{"some":{{"world":{realm}}}}}"#)],
    );
    realm
}

fn database_identity(shard: &Standalone) -> String {
    let decimal =
        &shard.query_rows("SELECT database_identity FROM st_module")[0]["database_identity"];
    let mut limbs = [0u64; 4];
    for digit in decimal.bytes() {
        let mut carry = u128::from(digit - b'0');
        for limb in limbs.iter_mut().rev() {
            let wide = u128::from(*limb) * 10 + carry;
            *limb = wide as u64;
            carry = wide >> 64;
        }
    }
    let hex: String = limbs.iter().map(|limb| format!("{limb:016x}")).collect();
    format!(r#"{{"__identity__":"0x{hex}"}}"#)
}

fn roster_naming(joiner: u64) -> PartyUpdate {
    PartyUpdate {
        revision: ROSTER_REVISION,
        characters: Vec::new(),
        rosters: vec![Roster {
            group_id: GROUP,
            leader: LEADER,
            loot_method: 0,
            loot_threshold: 2,
            master: 0,
            kind: 0,
            members: [LEADER, joiner]
                .map(|guid| Member { guid, raid_slot: 0 })
                .into(),
            icons: Vec::new(),
            disband: None,
        }],
        events: Vec::new(),
        absent: Vec::new(),
        events_expire_micros: 0,
    }
}

fn project_join(shard: &Standalone, joiner: u64) {
    shard.assert_call("idc_world_party_update", &[&bsatn(&roster_naming(joiner))]);
}

fn world_shard(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    serve_the_operator_realm(&shard);
    shard
}

fn export(source: &Standalone, guid: u64) -> String {
    source.assert_call(
        "begin_transfer",
        &[
            &TRANSFER.to_string(),
            &actor(guid),
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "true",
        ],
    );
    let out = source.query_rows(&format!(
        "SELECT blob FROM game_transfer_out WHERE transfer_id = {TRANSFER}"
    ));
    serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap()
}

fn import(destination: &Standalone, blob: &str) {
    destination.assert_call(
        "import_character_blob",
        &[&TRANSFER.to_string(), blob, &actor("0")],
    );
}

fn runs_bound(shard: &Standalone, guid: u64) -> Vec<String> {
    shard
        .query_rows(&format!(
            "SELECT instance_id FROM game_instance_binding WHERE character_guid = {guid} AND map_id = {DEADMINES}"
        ))
        .into_iter()
        .map(|row| row["instance_id"].clone())
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_who_joins_while_between_shards_arrives_without_its_solo_run() {
    let source = realm("dungeon-run-join-in-transit-source");
    let joiner = create_character(&source, "Joiner");
    let solo = enter(&source, joiner);
    walk_out(&source, joiner);
    assert_eq!(
        runs_bound(&source, joiner),
        [solo],
        "the Transfer carries the solo run"
    );
    let blob = export(&source, joiner);

    let destination = world_shard("dungeon-run-join-in-transit-destination");
    project_join(&destination, joiner);
    import(&destination, &blob);

    assert!(
        runs_bound(&destination, joiner).is_empty(),
        "the join the destination projected before the import resets the solo run"
    );
    assert!(destination
        .query_rows(&format!(
            "SELECT character_guid FROM game_group_join_reset WHERE character_guid = {joiner}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_who_joined_before_its_transfer_keeps_its_groups_run() {
    let source = realm("dungeon-run-join-before-transfer-source");
    let joiner = create_character(&source, "Joiner");
    let solo = enter(&source, joiner);
    walk_out(&source, joiner);
    serve_the_operator_realm(&source);
    project_join(&source, joiner);
    let run = enter(&source, joiner);
    assert_ne!(
        run, solo,
        "the source reset the solo run when it applied the join"
    );
    walk_out(&source, joiner);
    let blob = export(&source, joiner);

    let destination = world_shard("dungeon-run-join-before-transfer-destination");
    project_join(&destination, joiner);
    import(&destination, &blob);

    assert_eq!(
        runs_bound(&destination, joiner),
        [run],
        "the source applied the join, so the destination keeps the Group's run"
    );
}

fn admit(shard: &Standalone, realm: &str, joiner: u64, solo: &str, queued_group: u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64;
    let request = Admission {
        request: EntryRequest {
            id: 1,
            actor: PartyCharacter {
                guid: joiner,
                account_id: 1,
                connection: 7,
                world: Identity::ZERO,
                name: "Joiner".into(),
                race: 1,
                class: 1,
                chat_tag: 0,
                identity: Identity::ZERO,
                online: true,
                stats: None,
                revision: 1,
            },
            map: DEADMINES,
            binding: solo.parse().unwrap(),
            trigger: TO_DEADMINES,
            x: 10.0,
            y: 20.0,
            z: 30.0,
            o: 0.0,
            expires: now + 60_000_000,
        },
        binding_world: None,
        queued_group: Some(queued_group),
    };
    let party = AdmissionParty {
        group: GROUP,
        snapshot: spacetimedb::sats::bsatn::to_vec(&roster_naming(joiner)).unwrap(),
    };
    shard.assert_call(
        "idc_world_instance_entry",
        &[realm, &bsatn(&request), &bsatn(&party)],
    );
}

fn admitted_run(shard: &Standalone, joiner: u64) -> String {
    shard.query_rows(&format!(
        "SELECT instance_id FROM game_instance_entry_lease WHERE guid = {joiner}"
    ))[0]["instance_id"]
        .clone()
}

/// An Instance Shard holding a solo run whose binding a joiner from another Shard carries.
fn instance_shard(name: &str) -> (Standalone, String, String) {
    let shard = realm(name);
    let other = create_character(&shard, "Other");
    let solo = enter(&shard, other);
    walk_out(&shard, other);
    let realm = serve_the_operator_realm(&shard);
    shard.assert_call(
        "operator_set_map_owner",
        &[&DEADMINES.to_string(), &database_identity(&shard)],
    );
    (shard, realm, solo)
}

const ARRIVING_JOINER: u64 = 5_272_300;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_who_joins_during_its_dungeon_admission_does_not_bring_its_solo_run() {
    let (shard, realm, solo) = instance_shard("dungeon-run-join-during-admission");

    admit(&shard, &realm, ARRIVING_JOINER, &solo, 0);

    assert_ne!(admitted_run(&shard, ARRIVING_JOINER), solo);
    assert_eq!(
        owner_of(&shard, &solo),
        "0",
        "the Group does not take the solo run"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_already_in_its_group_when_it_queued_brings_its_solo_run() {
    let (shard, realm, solo) = instance_shard("dungeon-run-member-admission");

    admit(&shard, &realm, ARRIVING_JOINER, &solo, GROUP);

    assert_eq!(admitted_run(&shard, ARRIVING_JOINER), solo);
    assert_eq!(owner_of(&shard, &solo), GROUP.to_string());
}
