mod support;

use lyracore_package_delta::fixture_ids::PROFESSION_TRAINER_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{A_MOD_SKILL, T_SELF};
use support::{actor, Standalone};

fn fixture(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_set_level", &["1", "60"]);
    shard.assert_call("debug_set_money", &["1", "1000000"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &PROFESSION_TRAINER_ENTRY.to_string(), "2"],
    );
    let npc = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {PROFESSION_TRAINER_ENTRY}"
    ))[0]["guid"]
        .clone();
    (shard, npc)
}

fn offer(shard: &Standalone, id: u32, spell: u32, line: u32, cap: u32) {
    offer_requiring(shard, id, spell, (line, cap), (0, 0));
}

fn offer_requiring(
    shard: &Standalone,
    id: u32,
    spell: u32,
    (line, cap): (u32, u32),
    (required_skill, required_skill_value): (u32, u32),
) {
    shard.insert_row(
        "game_trainer_spell",
        &[
            ("id", &id),
            ("trainer_entry", &PROFESSION_TRAINER_ENTRY),
            ("spell_id", &spell),
            ("cost", &10),
            ("required_level", &1),
            ("learn_skill_line", &line),
            ("learn_skill_cap", &cap),
            ("required_skill", &required_skill),
            ("required_skill_value", &required_skill_value),
        ],
    );
}

fn wrapper(shard: &Standalone, wrapper: u32, trigger: u32) {
    let id = packed_spell_effect_id(wrapper, 0);
    let kind = lyracore_shared::spell::taxonomy::E_LEARN_SPELL;
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &id),
            ("spell_id", &wrapper),
            ("kind", &kind),
            ("trigger_spell", &trigger),
        ],
    );
}

fn assert_learned(shard: &Standalone, spell: u32, line: u32, cap: u32) {
    assert_eq!(shard.query_rows(&format!("SELECT spell_id FROM game_player_spell WHERE character_guid = 1 AND spell_id = {spell}")).len(), 1);
    let skill = shard.query_rows(&format!("SELECT current,max_rank FROM game_player_skill WHERE character_guid = 1 AND skill_line = {line}"));
    assert_eq!(skill.len(), 1);
    assert_eq!(skill[0]["current"], "1");
    assert_eq!(skill[0]["max_rank"], cap.to_string());
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.14.0 CLI and debug Module"]
fn profession_wrappers_teach_the_spell_at_every_skill_tier() {
    let (shard, npc) = fixture("trainer-profession-wrappers");
    for (profession, (line, spells)) in [
        (171, [2259, 3101, 3464, 11611]),
        (356, [7620, 7731, 7732, 18248]),
        (186, [2575, 2576, 3564, 10248]),
    ]
    .into_iter()
    .enumerate()
    {
        for (tier, spell) in spells.into_iter().enumerate() {
            let id = 5_090_800 + (profession * 4 + tier) as u32;
            let cap = (tier as u32 + 1) * 75;
            wrapper(&shard, id, spell);
            offer(&shard, id, id, line, cap);
            shard.insert_row(
                "game_skill_ability",
                &[("id", &id), ("spell_id", &spell), ("skill_line", &line)],
            );
            shard.assert_call("gw_trainer_buy", &[&actor("1"), &npc, &id.to_string()]);
            assert_learned(&shard, spell, line, cap);
            assert!(shard.query_rows(&format!("SELECT spell_id FROM game_player_spell WHERE character_guid = 1 AND spell_id = {id}")).is_empty());
        }
    }
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.14.0 CLI and debug Module"]
fn classic_wrapper_and_synthesized_profession_offers_keep_working_in_either_order() {
    let (shard, npc) = fixture("trainer-classic-professions");
    for (tier, spell) in [2259, 3101, 3464, 11611].into_iter().enumerate() {
        let id = 5_090_900 + tier as u32;
        let cap = (tier as u32 + 1) * 75;
        wrapper(&shard, id, spell);
        offer(&shard, id, id, 0, 75);
        offer(&shard, id + 10, spell, 171, cap);
        if tier % 2 == 0 {
            shard.assert_call("gw_trainer_buy", &[&actor("1"), &npc, &id.to_string()]);
        }
        shard.assert_call("gw_trainer_buy", &[&actor("1"), &npc, &spell.to_string()]);
        assert_learned(&shard, spell, 171, cap);
        let money = shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1");
        assert!(!shard
            .call("gw_trainer_buy", &[&actor("1"), &npc, &id.to_string()])
            .status
            .success());
        assert_eq!(
            shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1"),
            money
        );
    }
}

fn money(shard: &Standalone) -> u64 {
    shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1")[0]["money"]
        .parse()
        .unwrap()
}

fn buy(shard: &Standalone, npc: &str, spell: u32) {
    shard.assert_call("gw_trainer_buy", &[&actor("1"), npc, &spell.to_string()]);
}

fn refused_buy(shard: &Standalone, npc: &str, spell: u32, tag: &str) {
    let money_before = money(shard);
    let output = shard.call("gw_trainer_buy", &[&actor("1"), npc, &spell.to_string()]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{text}");
    assert!(text.contains(tag), "{text}");
    assert_eq!(money(shard), money_before);
}

fn knows(shard: &Standalone, spell: u32) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT spell_id FROM game_player_spell WHERE character_guid = 1 AND spell_id = {spell}"
        ))
        .is_empty()
}

fn skill_row(shard: &Standalone, line: u32) -> Vec<std::collections::BTreeMap<String, String>> {
    shard.query_rows(&format!(
        "SELECT current,max_rank FROM game_player_skill WHERE character_guid = 1 AND skill_line = {line}"
    ))
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.14.0 CLI and debug Module"]
fn a_recipe_is_refused_below_its_skill_requirement_and_learnable_at_it() {
    use lyracore_shared::trainer::TrainerRefusal;
    const RECIPE: u32 = 5_091_101;
    const TAILORING: u32 = 197;
    let (shard, npc) = fixture("trainer-recipe-skill");
    offer_requiring(&shard, 5_091_101, RECIPE, (0, 75), (TAILORING, 50));

    refused_buy(&shard, &npc, RECIPE, TrainerRefusal::SkillTooLow.as_tag());
    shard.assert_call("debug_set_skill", &["1", "197", "49"]);
    refused_buy(&shard, &npc, RECIPE, TrainerRefusal::SkillTooLow.as_tag());
    assert!(!knows(&shard, RECIPE));

    shard.assert_call("debug_set_skill", &["1", "197", "50"]);
    let money_before = money(&shard);
    buy(&shard, &npc, RECIPE);
    assert!(knows(&shard, RECIPE));
    assert_eq!(money(&shard), money_before - 10);
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.14.0 CLI and debug Module"]
fn a_permanent_skill_bonus_counts_toward_a_required_skill() {
    use lyracore_shared::trainer::TrainerRefusal;
    const RECIPE: u32 = 5_091_111;
    const BONUS: u32 = 5_091_112;
    const TAILORING: u32 = 197;
    let (shard, npc) = fixture("trainer-permanent-skill-bonus");
    offer_requiring(&shard, 5_091_111, RECIPE, (0, 75), (TAILORING, 50));
    shard.assert_call("debug_set_skill", &["1", "197", "45"]);
    refused_buy(&shard, &npc, RECIPE, TrainerRefusal::SkillTooLow.as_tag());

    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &BONUS),
            ("name", &"Trainer permanent skill bonus fixture"),
            ("duration_ms", &600_000),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(BONUS, 0)),
            ("spell_id", &BONUS),
            ("kind", &A_MOD_SKILL),
            ("base_points", &10),
            ("target", &T_SELF),
            ("p_0", &TAILORING),
            ("p_1", &1),
        ],
    );
    shard.assert_call("debug_force_cast", &["1", &BONUS.to_string()]);
    buy(&shard, &npc, RECIPE);
    assert!(knows(&shard, RECIPE));
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.14.0 CLI and debug Module"]
fn a_third_primary_profession_is_refused_while_a_secondary_one_and_a_later_rank_are_not() {
    use lyracore_shared::trainer::TrainerRefusal;
    const ALCHEMY: (u32, u32) = (171, 75);
    const BLACKSMITHING: (u32, u32) = (164, 75);
    const MINING: (u32, u32) = (186, 75);
    const COOKING: (u32, u32) = (185, 75);
    const JOURNEYMAN_ALCHEMY: (u32, u32) = (171, 150);
    let (shard, npc) = fixture("trainer-primary-limit");
    let offers = [
        (5_091_201, ALCHEMY),
        (5_091_202, BLACKSMITHING),
        (5_091_203, MINING),
        (5_091_204, COOKING),
        (5_091_205, JOURNEYMAN_ALCHEMY),
    ];
    for (spell, line_and_cap) in offers {
        offer_requiring(&shard, spell, spell, line_and_cap, (0, 0));
    }

    buy(&shard, &npc, 5_091_201);
    buy(&shard, &npc, 5_091_202);
    refused_buy(
        &shard,
        &npc,
        5_091_203,
        TrainerRefusal::PrimaryProfessionLimit.as_tag(),
    );
    assert!(skill_row(&shard, 186).is_empty());

    buy(&shard, &npc, 5_091_204);
    assert_eq!(skill_row(&shard, 185).len(), 1);
    buy(&shard, &npc, 5_091_205);
    assert_eq!(skill_row(&shard, 171)[0]["max_rank"], "150");
}

#[test]
#[ignore = "requires the local SpacetimeDB 2.14.0 CLI and debug Module"]
fn a_higher_rank_is_refused_until_the_previous_rank_is_known() {
    use lyracore_shared::trainer::TrainerRefusal;
    const FIRST: u32 = 5_091_301;
    const SECOND: u32 = 5_091_302;
    let (shard, npc) = fixture("trainer-previous-rank");
    offer(&shard, 5_091_301, FIRST, 0, 75);
    offer(&shard, 5_091_302, SECOND, 0, 75);
    shard.insert_row(
        "game_spell_chain",
        &[
            ("spell_id", &SECOND),
            ("prev_spell", &FIRST),
            ("first_spell", &FIRST),
            ("rank", &2),
        ],
    );

    refused_buy(
        &shard,
        &npc,
        SECOND,
        TrainerRefusal::PreviousRankMissing.as_tag(),
    );
    buy(&shard, &npc, FIRST);
    buy(&shard, &npc, SECOND);
    assert!(knows(&shard, SECOND));
}
