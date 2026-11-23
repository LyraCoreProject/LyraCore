mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_CHANNELED, E_SUMMON_OBJECT, E_SUMMON_PLAYER, P_GAMEOBJECT_ENTRY, T_SELF,
    T_TARGET_ALLY,
};
use support::{
    distance, event_rows, poll_until, position, stage_group, Row, Standalone, POLL_TIMEOUT,
};

const WARLOCK: u64 = 509_4101;
const PARTICIPANT_A: u64 = 509_4102;
const PARTICIPANT_B: u64 = 509_4103;
const TARGET: u64 = 509_4104;
const HUMAN: u8 = 1;
const WARLOCK_CLASS: u8 = 9;
const WARRIOR_CLASS: u8 = 1;
const RITUAL_OF_SUMMONING: u32 = 698;
const RITUAL_OF_SUMMONING_EFFECT: u32 = 7720;
const SUMMONING_PORTAL: u32 = 36727;
// vmangos gameobject_template 36727: three Ritual Participants, spell 7720 and anim spell 698.
const REQUIRED_PARTICIPANTS: u32 = 3;
// Spell.dbc 698 lasts 600 s; 7720 casts for 5 s.
const RITUAL_DURATION_MS: u32 = 600_000;
const RITUAL_EFFECT_CAST_MS: u32 = 5_000;
const ALTAR: (f32, f32, f32) = (-8949.95, -132.493, 83.5312);
// The ritual spell's cast time, with a margin for the scheduler.
const PAST_THE_RITUAL_CAST: std::time::Duration = std::time::Duration::from_secs(7);
// Module state 1 is the client's GO_STATE_ACTIVE.
const PORTAL_ACTIVE: u8 = 1;

fn start(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    realm.assert_call("install_guid_range", &["0"]);
    realm.assert_call("debug_seed_scenario_fixtures", &[]);
    realm.assert_sql(&format!(
        "DELETE FROM game_creature_spawn WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    realm.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    stage_ritual(&realm);
    let (x, y, z) = ALTAR;
    stage(&realm, WARLOCK, WARLOCK_CLASS, (x, y, z));
    stage(&realm, PARTICIPANT_A, WARRIOR_CLASS, (x + 1.0, y, z));
    stage(&realm, PARTICIPANT_B, WARRIOR_CLASS, (x, y + 1.0, z));
    stage(&realm, TARGET, WARRIOR_CLASS, (x + 60.0, y, z));
    realm.assert_call(
        "debug_learn_spell",
        &[&WARLOCK.to_string(), &RITUAL_OF_SUMMONING.to_string()],
    );
    realm.assert_sql(&format!(
        "UPDATE game_world_entity SET target_guid = {TARGET} WHERE guid = {WARLOCK}"
    ));
    realm
}

fn stage(realm: &Standalone, guid: u64, class: u8, (x, y, z): (f32, f32, f32)) {
    realm.assert_call(
        "debug_stage_meeting_stone_character",
        &[
            &guid.to_string(),
            &guid.to_string(),
            &HUMAN.to_string(),
            &class.to_string(),
            "20",
            "0",
            &x.to_string(),
            &y.to_string(),
            &z.to_string(),
        ],
    );
}

fn stage_ritual(realm: &Standalone) {
    realm.insert_row(
        "game_spell",
        &[
            ("spell_id", &RITUAL_OF_SUMMONING),
            ("name", &"Ritual of Summoning"),
            ("cast_time_ms", &5_000),
            ("range_yd", &30),
            ("duration_ms", &RITUAL_DURATION_MS),
            ("school_mask", &32),
            ("spell_level", &20),
            ("cast_flags", &CAST_FLAG_CHANNELED),
        ],
    );
    realm.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(RITUAL_OF_SUMMONING, 0)),
            ("spell_id", &RITUAL_OF_SUMMONING),
            ("kind", &E_SUMMON_OBJECT),
            ("target", &T_SELF),
            ("p_0", &SUMMONING_PORTAL),
            ("p_0_kind", &P_GAMEOBJECT_ENTRY),
        ],
    );
    realm.insert_row(
        "game_spell",
        &[
            ("spell_id", &RITUAL_OF_SUMMONING_EFFECT),
            ("name", &"Ritual of Summoning Effect"),
            ("cast_time_ms", &RITUAL_EFFECT_CAST_MS),
            ("school_mask", &1),
        ],
    );
    realm.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(RITUAL_OF_SUMMONING_EFFECT, 0)),
            ("spell_id", &RITUAL_OF_SUMMONING_EFFECT),
            ("kind", &E_SUMMON_PLAYER),
            ("target", &T_TARGET_ALLY),
        ],
    );
    realm.insert_row(
        "game_gameobject_template",
        &[
            ("entry", &SUMMONING_PORTAL),
            ("type_id", &18),
            ("display_id", &1327),
            ("name", &"Summoning Portal"),
            ("size", &1.0),
        ],
    );
    realm.insert_row(
        "game_gameobject_ritual",
        &[
            ("entry", &SUMMONING_PORTAL),
            ("required_participants", &REQUIRED_PARTICIPANTS),
            ("spell_id", &RITUAL_OF_SUMMONING_EFFECT),
            ("anim_spell_id", &RITUAL_OF_SUMMONING),
            ("persistent", &false),
            ("casters_grouped", &true),
        ],
    );
}

fn portal(realm: &Standalone) -> Option<Row> {
    realm
        .query_rows(&format!(
            "SELECT guid, state, x, y, z FROM game_gameobject WHERE template_entry = {SUMMONING_PORTAL}"
        ))
        .into_iter()
        .next()
}

fn summon_offers(realm: &Standalone) -> Vec<Row> {
    realm.query_rows(&format!(
        "SELECT summoner_guid FROM game_summon_offer WHERE target_guid = {TARGET}"
    ))
}

fn entity(realm: &Standalone, guid: u64) -> Row {
    realm.query_rows(&format!(
        "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {guid}"
    ))[0]
        .clone()
}

fn open_ritual(realm: &Standalone) -> String {
    realm.assert_call(
        "debug_cast_spell",
        &[&WARLOCK.to_string(), &RITUAL_OF_SUMMONING.to_string(), "0"],
    );
    let portal = portal(realm).expect("the ritual places its Summoning Portal");
    assert!(
        distance(position(&portal), position(&entity(realm, WARLOCK))) < 0.01,
        "the portal stands at the Warlock's feet"
    );
    portal["guid"].clone()
}

fn assist(realm: &Standalone, portal: &str) {
    for participant in [PARTICIPANT_A, PARTICIPANT_B] {
        realm.assert_call("debug_use_gameobject", &[&participant.to_string(), portal]);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn two_participants_complete_the_ritual_and_the_accepted_target_lands_at_the_portal() {
    let realm = start("summoning-ritual-accept");
    stage_group(&realm, WARLOCK, &[PARTICIPANT_A, PARTICIPANT_B, TARGET]);
    let portal_guid = open_ritual(&realm);
    let altar = position(&portal(&realm).unwrap());

    realm.assert_call(
        "debug_use_gameobject",
        &[&PARTICIPANT_A.to_string(), &portal_guid],
    );
    assert!(
        summon_offers(&realm).is_empty(),
        "one Ritual Participant beside the Warlock is not enough"
    );
    realm.assert_call(
        "debug_use_gameobject",
        &[&PARTICIPANT_B.to_string(), &portal_guid],
    );

    assert!(
        poll_until(POLL_TIMEOUT, || !summon_offers(&realm).is_empty()),
        "the completed ritual offers the target a summon"
    );
    assert_eq!(
        summon_offers(&realm)[0]["summoner_guid"],
        WARLOCK.to_string()
    );
    assert!(
        portal(&realm).is_none(),
        "the finished ritual removes its portal"
    );
    assert!(
        distance(position(&entity(&realm, TARGET)), altar) > 50.0,
        "an offer moves nobody before it is accepted"
    );

    realm.assert_call(
        "debug_accept_summon",
        &[&TARGET.to_string(), &WARLOCK.to_string()],
    );

    let landed = entity(&realm, TARGET);
    assert_eq!(landed["map_id"], "0");
    assert!(
        distance(position(&landed), altar) < 2.0,
        "the target lands beside the portal: {landed:?} vs {altar:?}"
    );
    assert!(
        summon_offers(&realm).is_empty(),
        "an accepted offer is spent"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_target_outside_the_warlocks_group_is_refused_before_the_ritual_spell_starts() {
    let realm = start("summoning-ritual-ungrouped-target");
    stage_group(&realm, WARLOCK, &[PARTICIPANT_A, PARTICIPANT_B]);
    let casts = realm.subscribe(&[&format!(
        "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = {WARLOCK}"
    )]);
    let portal_guid = open_ritual(&realm);

    assist(&realm, &portal_guid);
    std::thread::sleep(PAST_THE_RITUAL_CAST);

    assert!(
        event_rows(&casts, "game_spell_cast_event_live")
            .iter()
            .all(|row| row["spell_id"] != RITUAL_OF_SUMMONING_EFFECT.to_string()),
        "a refused ritual spell starts no cast bar and never completes"
    );
    assert!(
        summon_offers(&realm).is_empty(),
        "a refused ritual offers no summon"
    );
    assert_eq!(
        portal(&realm).map(|portal| portal["state"].clone()),
        Some(PORTAL_ACTIVE.to_string()),
        "as in vmangos, the spent portal stays active while the Warlock channels"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_warlock_cancelling_during_the_ritual_cast_removes_the_portal_and_offers_nothing() {
    let realm = start("summoning-ritual-cancelled");
    stage_group(&realm, WARLOCK, &[PARTICIPANT_A, PARTICIPANT_B, TARGET]);
    let portal_guid = open_ritual(&realm);
    let before = position(&entity(&realm, TARGET));

    assist(&realm, &portal_guid);
    realm.assert_call("debug_cancel_cast", &[&WARLOCK.to_string()]);

    assert!(
        portal(&realm).is_none(),
        "a cancelled ritual removes its portal"
    );
    std::thread::sleep(PAST_THE_RITUAL_CAST);
    assert!(
        summon_offers(&realm).is_empty(),
        "a cancelled ritual offers no summon"
    );
    let refused = realm.call(
        "debug_accept_summon",
        &[&TARGET.to_string(), &WARLOCK.to_string()],
    );
    assert!(
        !refused.status.success(),
        "a summon response without an offer is refused"
    );
    assert!(
        distance(position(&entity(&realm, TARGET)), before) < 0.01,
        "a summon response without an offer leaves the target in place"
    );
}
