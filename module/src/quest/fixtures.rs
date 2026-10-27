use super::*;
use crate::group::{game_group, game_group_member};
use crate::reputation::game_player_reputation;
use crate::{game_creature_spawn, game_creature_template};
use lyracore_shared::quest::QUEST_TYPE_RAID;

const CHOICE_FIXTURE_PLAYER: u64 = 1;
const CHOICE_FIXTURE_GIVER: u64 = (0xF130_u64 << 48) | ((620_u64) << 24) | 0x00FF_FF00;
const CHOICE_FIXTURE_GIVER_ENTRY: u32 = 620;
const CHOICE_FIXTURE_GIVER_SOURCE: u64 = (0xF130_u64 << 48) | ((620_u64) << 24) | 1;
const CHOICE_FIXTURE_QUEST: u32 = 509_033;
const CHOICE_FIXTURE_OBJECTIVE_ITEM: u32 = 5_090_330;
const CHOICE_FIXTURE_CHOICE_0: u32 = 5_090_331;
const CHOICE_FIXTURE_CHOICE_1: u32 = 5_090_332;
const CHOICE_FIXTURE_GUARANTEED: u32 = 5_090_333;
const CHOICE_FIXTURE_FILLER: u32 = 5_090_334;

#[spacetimedb::reducer]
pub fn debug_stage_choice_reward_fixture(
    ctx: &ReducerContext,
    fill_inventory: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(CHOICE_FIXTURE_PLAYER)
        .ok_or_else(|| "choice fixture player is not live".to_string())?;

    let mut giver = entities
        .guid()
        .find(CHOICE_FIXTURE_GIVER_SOURCE)
        .ok_or_else(|| "choice fixture giver source is not live".to_string())?;
    crate::WorldEntity::delete(ctx, CHOICE_FIXTURE_GIVER);
    giver.guid = CHOICE_FIXTURE_GIVER;
    giver.map_id = player.map_id;
    giver.instance_id = player.instance_id;
    giver
        .place(
            player.x + 1.0,
            player.y,
            player.z,
            crate::world::MoveClock::Unchanged,
        )
        .unexplored();
    giver.insert(ctx);

    let templates = ctx.db.game_item_template();
    for entry in [
        CHOICE_FIXTURE_OBJECTIVE_ITEM,
        CHOICE_FIXTURE_CHOICE_0,
        CHOICE_FIXTURE_CHOICE_1,
        CHOICE_FIXTURE_GUARANTEED,
        CHOICE_FIXTURE_FILLER,
    ] {
        templates.entry().delete(entry);
    }
    templates.insert(crate::seed::tough_jerky_template(
        CHOICE_FIXTURE_OBJECTIVE_ITEM,
    ));
    templates.insert(crate::seed::tempered_blade_template(
        CHOICE_FIXTURE_CHOICE_0,
    ));
    templates.insert(crate::seed::tempered_blade_template(
        CHOICE_FIXTURE_CHOICE_1,
    ));
    templates.insert(crate::seed::tough_jerky_template(CHOICE_FIXTURE_GUARANTEED));
    templates.insert(crate::seed::tempered_blade_template(CHOICE_FIXTURE_FILLER));

    ctx.db
        .game_quest_template()
        .entry()
        .delete(CHOICE_FIXTURE_QUEST);
    ctx.db.game_quest_template().insert(QuestTemplate {
        entry: CHOICE_FIXTURE_QUEST,
        min_level: 0,
        quest_level: 2,
        title: "Choice reward tracer".to_string(),
        reward_money: 150,
        reward_xp: 90,
        prev_quest_id: 0,
        required_races: 0,
        required_classes: 0,
        zone_or_sort: 12,
        rew_rep_faction_1: crate::seed::FIXTURE_FACTION,
        rew_rep_value_1: 250,
        rew_rep_faction_2: 0,
        rew_rep_value_2: 0,
        src_item: 0,
        src_item_count: 0,
        repeatable: false,
        next_quest_id: 0,
        limit_time: 0,
        reward_money_max_level: 0,
        quest_type: 0,
    });

    let objectives = ctx.db.game_quest_objective();
    objectives.id().delete(5_090_330);
    objectives.insert(QuestObjective {
        id: 5_090_330,
        quest_entry: CHOICE_FIXTURE_QUEST,
        obj_index: 0,
        kind: objective_kind::COLLECT_ITEM,
        target_entry: CHOICE_FIXTURE_OBJECTIVE_ITEM,
        required_count: 8,
    });
    let guaranteed = ctx.db.game_quest_reward_item();
    guaranteed.id().delete(5_090_333);
    guaranteed.insert(QuestRewardItem {
        id: 5_090_333,
        quest_entry: CHOICE_FIXTURE_QUEST,
        item_entry: CHOICE_FIXTURE_GUARANTEED,
        count: 3,
    });
    let choices = ctx.db.game_quest_reward_choice();
    choices.id().delete(5_090_331);
    choices.id().delete(5_090_332);
    choices.insert(QuestRewardChoice {
        id: 5_090_331,
        quest_entry: CHOICE_FIXTURE_QUEST,
        choice_index: 0,
        item_entry: CHOICE_FIXTURE_CHOICE_0,
        count: 1,
    });
    choices.insert(QuestRewardChoice {
        id: 5_090_332,
        quest_entry: CHOICE_FIXTURE_QUEST,
        choice_index: 1,
        item_entry: CHOICE_FIXTURE_CHOICE_1,
        count: 2,
    });
    let relations = ctx.db.game_creature_quest();
    relations.id().delete(5_090_334);
    relations.insert(CreatureQuest {
        id: 5_090_334,
        creature_entry: CHOICE_FIXTURE_GIVER_ENTRY,
        quest_entry: CHOICE_FIXTURE_QUEST,
        role: quest_role::END,
    });

    for item in ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&CHOICE_FIXTURE_PLAYER)
        .collect::<Vec<_>>()
    {
        crate::ItemInstance::delete(ctx, item.guid);
    }
    for row in ctx
        .db
        .game_character_quest()
        .by_character()
        .filter(&CHOICE_FIXTURE_PLAYER)
        .filter(|row| row.quest_entry == CHOICE_FIXTURE_QUEST)
        .collect::<Vec<_>>()
    {
        crate::quest::CharacterQuest::delete(ctx, row.id);
    }
    for row in ctx
        .db
        .game_player_reputation()
        .by_character()
        .filter(&CHOICE_FIXTURE_PLAYER)
        .filter(|row| row.faction_id == crate::seed::FIXTURE_FACTION)
        .collect::<Vec<_>>()
    {
        crate::reputation::PlayerReputation::delete(ctx, row.id);
    }

    player.level = 1;
    player.xp = 0;
    player.next_level_xp = crate::xp::xp_to_next_level(1);
    player.money = 0;
    let owner_identity = player.owner_identity;
    player.update(ctx);
    crate::items::grant_item(ctx, CHOICE_FIXTURE_PLAYER, CHOICE_FIXTURE_OBJECTIVE_ITEM, 8)?;
    if fill_inventory {
        crate::items::grant_item(ctx, CHOICE_FIXTURE_PLAYER, CHOICE_FIXTURE_FILLER, 15)?;
    }
    crate::quest::CharacterQuest::insert(
        ctx,
        CharacterQuest {
            id: 0,
            character_guid: CHOICE_FIXTURE_PLAYER,
            owner_identity,
            quest_entry: CHOICE_FIXTURE_QUEST,
            counts: vec![0],
            rewarded: false,
            deadline_micros: 0,
            failed: false,
        },
    );
    Ok(())
}

#[spacetimedb::reducer]
pub fn debug_turn_in_choice_reward_fixture(
    ctx: &ReducerContext,
    reward_index: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    apply_turn_in_quest(
        ctx,
        CHOICE_FIXTURE_PLAYER,
        CHOICE_FIXTURE_GIVER,
        CHOICE_FIXTURE_QUEST,
        reward_index,
    )
}

#[spacetimedb::reducer]
pub fn debug_verify_choice_reward_fixture(
    ctx: &ReducerContext,
    reward_index: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let (choice_0, choice_1) = match reward_index {
        0 => (1, 0),
        1 => (0, 2),
        other => {
            return Err(format!(
                "no successful fixture expectation for choice {other}"
            ))
        }
    };
    let expected = [
        (CHOICE_FIXTURE_OBJECTIVE_ITEM, 0),
        (CHOICE_FIXTURE_CHOICE_0, choice_0),
        (CHOICE_FIXTURE_CHOICE_1, choice_1),
        (CHOICE_FIXTURE_GUARANTEED, 3),
        (CHOICE_FIXTURE_FILLER, 0),
    ];
    for (entry, count) in expected {
        let actual = crate::items::item_count(ctx, CHOICE_FIXTURE_PLAYER, entry);
        if actual != count {
            return Err(format!("item {entry}: expected {count}, found {actual}"));
        }
    }
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(CHOICE_FIXTURE_PLAYER)
        .ok_or_else(|| "choice fixture player is not live".to_string())?;
    if player.money != 150 || player.xp != 90 {
        return Err(format!(
            "expected 150 copper and 90 XP, found {} copper and {} XP",
            player.money, player.xp
        ));
    }
    let reputation = ctx
        .db
        .game_player_reputation()
        .by_character()
        .filter(&CHOICE_FIXTURE_PLAYER)
        .find(|row| row.faction_id == crate::seed::FIXTURE_FACTION)
        .map(|row| row.standing);
    if reputation != Some(250) {
        return Err(format!("expected 250 reputation, found {reputation:?}"));
    }
    let quest = character_quest_row(ctx, CHOICE_FIXTURE_PLAYER, CHOICE_FIXTURE_QUEST)
        .ok_or_else(|| "choice fixture quest row is missing".to_string())?;
    if !quest.rewarded {
        return Err("choice fixture quest was not rewarded".to_string());
    }
    Ok(())
}

#[spacetimedb::reducer]
pub fn debug_verify_choice_reward_refusal_fixture(
    ctx: &ReducerContext,
    expected_fillers: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let expected = [
        (CHOICE_FIXTURE_OBJECTIVE_ITEM, 8),
        (CHOICE_FIXTURE_CHOICE_0, 0),
        (CHOICE_FIXTURE_CHOICE_1, 0),
        (CHOICE_FIXTURE_GUARANTEED, 0),
        (CHOICE_FIXTURE_FILLER, expected_fillers),
    ];
    for (entry, count) in expected {
        let actual = crate::items::item_count(ctx, CHOICE_FIXTURE_PLAYER, entry);
        if actual != count {
            return Err(format!("item {entry}: expected {count}, found {actual}"));
        }
    }
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(CHOICE_FIXTURE_PLAYER)
        .ok_or_else(|| "choice fixture player is not live".to_string())?;
    if player.money != 0 || player.xp != 0 {
        return Err(format!(
            "refusal changed rewards to {} copper and {} XP",
            player.money, player.xp
        ));
    }
    let reputation = ctx
        .db
        .game_player_reputation()
        .by_character()
        .filter(&CHOICE_FIXTURE_PLAYER)
        .find(|row| row.faction_id == crate::seed::FIXTURE_FACTION);
    if reputation.is_some() {
        return Err("refusal granted reputation".to_string());
    }
    let quest = character_quest_row(ctx, CHOICE_FIXTURE_PLAYER, CHOICE_FIXTURE_QUEST)
        .ok_or_else(|| "choice fixture quest row is missing".to_string())?;
    if quest.rewarded {
        return Err("refusal marked the choice fixture quest rewarded".to_string());
    }
    Ok(())
}

const EVENTAI_CREDIT_FIXTURE_PLAYER: u64 = 1;
const EVENTAI_CREDIT_FIXTURE_MEMBER: u64 = 509_296_001;
const EVENTAI_CREDIT_FIXTURE_DISTANT_MEMBER: u64 = 509_296_002;
const EVENTAI_CREDIT_FIXTURE_BENEFICIARY: u64 = 509_296_003;
const EVENTAI_CREDIT_FIXTURE_SOURCE: u64 = (0xF130_u64 << 48) | ((620_u64) << 24) | 1;
const EVENTAI_CREDIT_FIXTURE_GROUP: u64 = 509_296;
const EVENTAI_CREDIT_FIXTURE_CREATURE: u32 = 509_296;
const EVENTAI_CREDIT_FIXTURE_OTHER_CREATURE: u32 = 509_297;
const EVENTAI_CREDIT_FIXTURE_KILL_QUEST: u32 = 5_092_961;
const EVENTAI_CREDIT_FIXTURE_FALLBACK_QUEST: u32 = 5_092_962;
const EVENTAI_CREDIT_FIXTURE_MISSING_OBJECTIVE_QUEST: u32 = 5_092_963;
const EVENTAI_CREDIT_FIXTURE_EVENT_QUEST: u32 = 5_092_964;

fn eventai_credit_fixture_count(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<u32, String> {
    character_quest_row(ctx, character_guid, quest_entry)
        .and_then(|quest| quest.counts.first().copied())
        .ok_or_else(|| {
            format!("fixture quest is missing for character {character_guid}: {quest_entry}")
        })
}

#[spacetimedb::reducer]
#[allow(clippy::too_many_lines)]
pub fn debug_verify_eventai_quest_credit_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    let fixture_characters = [
        EVENTAI_CREDIT_FIXTURE_PLAYER,
        EVENTAI_CREDIT_FIXTURE_MEMBER,
        EVENTAI_CREDIT_FIXTURE_DISTANT_MEMBER,
        EVENTAI_CREDIT_FIXTURE_BENEFICIARY,
    ];
    let fixture_quests = [
        EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        EVENTAI_CREDIT_FIXTURE_FALLBACK_QUEST,
        EVENTAI_CREDIT_FIXTURE_MISSING_OBJECTIVE_QUEST,
        EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
    ];

    crate::loot::tag::clear(ctx, EVENTAI_CREDIT_FIXTURE_SOURCE);
    let logs = ctx.db.game_character_quest();
    for character_guid in fixture_characters {
        for quest in logs
            .by_character()
            .filter(&character_guid)
            .filter(|quest| fixture_quests.contains(&quest.quest_entry))
            .collect::<Vec<_>>()
        {
            crate::quest::CharacterQuest::delete(ctx, quest.id);
        }
    }
    let event_credits = ctx.db.game_character_quest_event_credit();
    for character_guid in fixture_characters {
        for credit in event_credits
            .by_character()
            .filter(&character_guid)
            .filter(|credit| fixture_quests.contains(&credit.quest_entry))
            .collect::<Vec<_>>()
        {
            CharacterQuestEventCredit::delete(ctx, credit.id);
        }
    }
    let templates = ctx.db.game_quest_template();
    for quest_entry in fixture_quests {
        templates.entry().delete(quest_entry);
    }
    let objectives = ctx.db.game_quest_objective();
    for id in 509_296_100..=509_296_103 {
        objectives.id().delete(id);
    }
    ctx.db
        .game_quest_event_requirement()
        .id()
        .delete(509_296_104);

    let entities = ctx.db.game_world_entity();
    for guid in [
        EVENTAI_CREDIT_FIXTURE_MEMBER,
        EVENTAI_CREDIT_FIXTURE_DISTANT_MEMBER,
        EVENTAI_CREDIT_FIXTURE_BENEFICIARY,
    ] {
        crate::WorldEntity::delete(ctx, guid);
    }
    let source = entities
        .guid()
        .find(EVENTAI_CREDIT_FIXTURE_SOURCE)
        .ok_or_else(|| "EventAI credit fixture source is not live".to_string())?;
    let mut player = entities
        .guid()
        .find(EVENTAI_CREDIT_FIXTURE_PLAYER)
        .ok_or_else(|| "EventAI credit fixture player is not live".to_string())?;
    player.map_id = source.map_id;
    player.instance_id = source.instance_id;
    player
        .place(
            source.x,
            source.y,
            source.z,
            crate::world::MoveClock::Unchanged,
        )
        .unexplored();
    player.dead = false;
    let fixture_owner = player.owner_identity;
    player.update(ctx);

    let mut member = entities
        .guid()
        .find(EVENTAI_CREDIT_FIXTURE_PLAYER)
        .ok_or_else(|| "EventAI credit fixture player disappeared".to_string())?;
    member.guid = EVENTAI_CREDIT_FIXTURE_MEMBER;
    member.insert(ctx);
    let mut distant_member = entities
        .guid()
        .find(EVENTAI_CREDIT_FIXTURE_PLAYER)
        .ok_or_else(|| "EventAI credit fixture player disappeared".to_string())?;
    distant_member.guid = EVENTAI_CREDIT_FIXTURE_DISTANT_MEMBER;
    let (y, z) = (distant_member.y, distant_member.z);
    distant_member
        .place(source.x + 100.0, y, z, crate::world::MoveClock::Unchanged)
        .unexplored();
    distant_member.insert(ctx);
    let mut beneficiary = entities
        .guid()
        .find(EVENTAI_CREDIT_FIXTURE_PLAYER)
        .ok_or_else(|| "EventAI credit fixture player disappeared".to_string())?;
    beneficiary.guid = EVENTAI_CREDIT_FIXTURE_BENEFICIARY;
    beneficiary.insert(ctx);

    let groups = ctx.db.game_group();
    groups.group_id().delete(EVENTAI_CREDIT_FIXTURE_GROUP);
    groups.insert(crate::Group {
        group_id: EVENTAI_CREDIT_FIXTURE_GROUP,
        leader_guid: EVENTAI_CREDIT_FIXTURE_PLAYER,
        loot_method: crate::group::loot_method::FFA,
        loot_threshold: 0,
        rr_cursor: 0,
        master_looter_guid: 0,
        group_type: 0,
    });
    let group_members = ctx.db.game_group_member();
    for member in group_members
        .iter()
        .filter(|member| fixture_characters.contains(&member.character_guid))
        .collect::<Vec<_>>()
    {
        group_members.id().delete(member.id);
    }
    for (id, character_guid) in [
        (509_296_010, EVENTAI_CREDIT_FIXTURE_PLAYER),
        (509_296_011, EVENTAI_CREDIT_FIXTURE_MEMBER),
        (509_296_012, EVENTAI_CREDIT_FIXTURE_DISTANT_MEMBER),
    ] {
        group_members.insert(crate::GroupMember {
            id,
            group_id: EVENTAI_CREDIT_FIXTURE_GROUP,
            character_guid,
            owner_identity: fixture_owner,
            raid_slot: 0,
        });
    }

    let quest_template = |entry, title: &str| QuestTemplate {
        entry,
        min_level: 0,
        quest_level: 1,
        title: title.to_string(),
        reward_money: 0,
        reward_xp: 0,
        prev_quest_id: 0,
        required_races: 0,
        required_classes: 0,
        zone_or_sort: 0,
        rew_rep_faction_1: 0,
        rew_rep_value_1: 0,
        rew_rep_faction_2: 0,
        rew_rep_value_2: 0,
        src_item: 0,
        src_item_count: 0,
        repeatable: false,
        next_quest_id: 0,
        limit_time: 0,
        reward_money_max_level: 0,
        quest_type: 0,
    };
    templates.insert(quest_template(
        EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        "EventAI tap credit",
    ));
    templates.insert(quest_template(
        EVENTAI_CREDIT_FIXTURE_FALLBACK_QUEST,
        "EventAI beneficiary fallback",
    ));
    templates.insert(quest_template(
        EVENTAI_CREDIT_FIXTURE_MISSING_OBJECTIVE_QUEST,
        "EventAI missing objective",
    ));
    templates.insert(quest_template(
        EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
        "EventAI event gate",
    ));
    for (id, quest_entry, target_entry) in [
        (
            509_296_100,
            EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
            EVENTAI_CREDIT_FIXTURE_CREATURE,
        ),
        (
            509_296_101,
            EVENTAI_CREDIT_FIXTURE_FALLBACK_QUEST,
            EVENTAI_CREDIT_FIXTURE_CREATURE,
        ),
        (
            509_296_102,
            EVENTAI_CREDIT_FIXTURE_MISSING_OBJECTIVE_QUEST,
            EVENTAI_CREDIT_FIXTURE_OTHER_CREATURE,
        ),
        (
            509_296_103,
            EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
            EVENTAI_CREDIT_FIXTURE_OTHER_CREATURE,
        ),
    ] {
        objectives.insert(QuestObjective {
            id,
            quest_entry,
            obj_index: 0,
            kind: objective_kind::KILL_CREATURE,
            target_entry,
            required_count: 1,
        });
    }
    ctx.db
        .game_quest_event_requirement()
        .insert(QuestEventRequirement {
            id: 509_296_104,
            quest_entry: EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
        });
    for (id, character_guid, quest_entry) in [
        (
            509_296_200,
            EVENTAI_CREDIT_FIXTURE_PLAYER,
            EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        ),
        (
            509_296_201,
            EVENTAI_CREDIT_FIXTURE_MEMBER,
            EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        ),
        (
            509_296_202,
            EVENTAI_CREDIT_FIXTURE_DISTANT_MEMBER,
            EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        ),
        (
            509_296_203,
            EVENTAI_CREDIT_FIXTURE_BENEFICIARY,
            EVENTAI_CREDIT_FIXTURE_FALLBACK_QUEST,
        ),
        (
            509_296_204,
            EVENTAI_CREDIT_FIXTURE_BENEFICIARY,
            EVENTAI_CREDIT_FIXTURE_MISSING_OBJECTIVE_QUEST,
        ),
        (
            509_296_205,
            EVENTAI_CREDIT_FIXTURE_PLAYER,
            EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
        ),
    ] {
        crate::quest::CharacterQuest::delete(ctx, id);
        crate::quest::CharacterQuest::insert(
            ctx,
            CharacterQuest {
                id,
                character_guid,
                owner_identity: fixture_owner,
                quest_entry,
                counts: vec![0],
                rewarded: false,
                deadline_micros: 0,
                failed: false,
            },
        );
    }

    let tap_context = EventAiQuestCreditContext {
        source_creature_guid: EVENTAI_CREDIT_FIXTURE_SOURCE,
        source_x: source.x,
        source_y: source.y,
        source_map_id: source.map_id,
        source_instance_id: source.instance_id,
        selected_character: Some(EVENTAI_CREDIT_FIXTURE_PLAYER),
        invoker_beneficiary: Some(EVENTAI_CREDIT_FIXTURE_BENEFICIARY),
    };
    let kill_request = EventAiQuestCredit::KillCredit(KillCredit {
        creature_entry: EVENTAI_CREDIT_FIXTURE_CREATURE,
        recipient_policy: QuestCreditRecipientPolicy::TapGroup,
    });

    crate::loot::tag::record_first_threat(
        ctx,
        EVENTAI_CREDIT_FIXTURE_SOURCE,
        EVENTAI_CREDIT_FIXTURE_PLAYER,
    );
    for member in group_members
        .by_character()
        .filter(&EVENTAI_CREDIT_FIXTURE_MEMBER)
        .collect::<Vec<_>>()
    {
        group_members.id().delete(member.id);
    }
    if apply_eventai_credit(ctx, kill_request, &tap_context) != QuestCreditOutcome::Applied {
        return Err("tap-group credit was refused".to_string());
    }
    if eventai_credit_fixture_count(
        ctx,
        EVENTAI_CREDIT_FIXTURE_PLAYER,
        EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
    )? != 1
        || eventai_credit_fixture_count(
            ctx,
            EVENTAI_CREDIT_FIXTURE_MEMBER,
            EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        )? != 0
        || eventai_credit_fixture_count(
            ctx,
            EVENTAI_CREDIT_FIXTURE_DISTANT_MEMBER,
            EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        )? != 0
    {
        return Err(
            "tap-group credit did not atomically reward only the original eligible roster"
                .to_string(),
        );
    }
    if apply_eventai_credit(ctx, kill_request, &tap_context) != QuestCreditOutcome::Applied
        || eventai_credit_fixture_count(
            ctx,
            EVENTAI_CREDIT_FIXTURE_PLAYER,
            EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        )? != 1
        || eventai_credit_fixture_count(
            ctx,
            EVENTAI_CREDIT_FIXTURE_MEMBER,
            EVENTAI_CREDIT_FIXTURE_KILL_QUEST,
        )? != 0
    {
        return Err("duplicate EventAI kill credit changed quest progress".to_string());
    }

    crate::loot::tag::clear(ctx, EVENTAI_CREDIT_FIXTURE_SOURCE);
    let fallback_context = EventAiQuestCreditContext {
        selected_character: None,
        ..tap_context
    };
    if apply_eventai_credit(ctx, kill_request, &fallback_context) != QuestCreditOutcome::Applied
        || eventai_credit_fixture_count(
            ctx,
            EVENTAI_CREDIT_FIXTURE_BENEFICIARY,
            EVENTAI_CREDIT_FIXTURE_FALLBACK_QUEST,
        )? != 1
        || eventai_credit_fixture_count(
            ctx,
            EVENTAI_CREDIT_FIXTURE_BENEFICIARY,
            EVENTAI_CREDIT_FIXTURE_MISSING_OBJECTIVE_QUEST,
        )? != 0
    {
        return Err("beneficiary fallback did not preserve missing objectives".to_string());
    }

    crate::loot::tag::record_first_threat(
        ctx,
        EVENTAI_CREDIT_FIXTURE_SOURCE,
        EVENTAI_CREDIT_FIXTURE_PLAYER,
    );
    let mut tapped_player = entities
        .guid()
        .find(EVENTAI_CREDIT_FIXTURE_PLAYER)
        .ok_or_else(|| "EventAI credit fixture player disappeared".to_string())?;
    let (y, z) = (tapped_player.y, tapped_player.z);
    tapped_player
        .place(source.x + 100.0, y, z, crate::world::MoveClock::Unchanged)
        .unexplored();
    tapped_player.update(ctx);
    if apply_eventai_credit(ctx, kill_request, &fallback_context) != QuestCreditOutcome::Refused
        || eventai_credit_fixture_count(
            ctx,
            EVENTAI_CREDIT_FIXTURE_BENEFICIARY,
            EVENTAI_CREDIT_FIXTURE_FALLBACK_QUEST,
        )? != 1
    {
        return Err(
            "a present but ineligible tap fell back to the invoker beneficiary".to_string(),
        );
    }

    let event_request = EventAiQuestCredit::QuestEvent(QuestEvent {
        quest_entry: EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
        recipient_policy: QuestCreditRecipientPolicy::SelectedCharacter,
    });
    if quest_is_complete(
        ctx,
        &character_quest_row(
            ctx,
            EVENTAI_CREDIT_FIXTURE_PLAYER,
            EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
        )
        .ok_or_else(|| "EventAI event fixture quest is missing".to_string())?,
    ) {
        return Err(
            "event-required quest completed before ordinary objective and event credit".to_string(),
        );
    }
    if apply_eventai_credit(ctx, event_request, &tap_context) != QuestCreditOutcome::Applied {
        return Err("quest event credit was refused".to_string());
    }
    let mut event_quest = character_quest_row(
        ctx,
        EVENTAI_CREDIT_FIXTURE_PLAYER,
        EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
    )
    .ok_or_else(|| "EventAI event fixture quest is missing".to_string())?;
    if quest_is_complete(ctx, &event_quest) {
        return Err("event-required quest completed without its ordinary objective".to_string());
    }
    event_quest.counts[0] = 1;
    event_quest.update(ctx);
    let event_quest = character_quest_row(
        ctx,
        EVENTAI_CREDIT_FIXTURE_PLAYER,
        EVENTAI_CREDIT_FIXTURE_EVENT_QUEST,
    )
    .ok_or_else(|| "EventAI event fixture quest is missing".to_string())?;
    if !quest_is_complete(ctx, &event_quest) {
        return Err("event-required quest stayed incomplete after both requirements".to_string());
    }
    if apply_eventai_credit(ctx, event_request, &tap_context) != QuestCreditOutcome::Applied
        || event_credits
            .by_character()
            .filter(&EVENTAI_CREDIT_FIXTURE_PLAYER)
            .filter(|credit| credit.quest_entry == EVENTAI_CREDIT_FIXTURE_EVENT_QUEST)
            .count()
            != 1
    {
        return Err("duplicate quest event credit was not idempotent".to_string());
    }

    crate::loot::tag::clear(ctx, EVENTAI_CREDIT_FIXTURE_SOURCE);
    Ok(())
}

const RQC_PLAYER: u64 = 1;
const RQC_RAID_OTHER: u64 = 509_297_001;
const RQC_PARTY_KILLER: u64 = 509_297_002;
const RQC_RAID_FILLER_START: u64 = 509_297_010;
const RQC_RAID_FILLER_COUNT: u64 = 10;
const RQC_RAID_GROUP: u64 = 509_297;
const RQC_PARTY_GROUP: u64 = 509_298;
const RQC_NORMAL_QUEST: u32 = 5_092_970;
const RQC_RAID_QUEST: u32 = 5_092_971;
const RQC_CAST_QUEST: u32 = 5_092_972;
const RQC_CREATURE_ENTRY: u32 = 509_297;
const RQC_GAMEOBJECT_ENTRY: u32 = 509_298;
const RQC_TRIGGER_ID: u32 = 509_299;
const RQC_ITEM_NORMAL: u32 = 509_300;
const RQC_ITEM_RAID: u32 = 509_301;
const RQC_CAST_SPELL: u32 = 509_302;
const RQC_WOLF_TEMPLATE: u32 = 51_000;
const RQC_WOLF_GUID: u64 = (0xF130_u64 << 48) | ((RQC_WOLF_TEMPLATE as u64) << 24) | 509_297;

fn rqc_progress(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
    obj_index: usize,
) -> Result<u32, String> {
    character_quest_row(ctx, character_guid, quest_entry)
        .and_then(|quest| quest.counts.get(obj_index).copied())
        .ok_or_else(|| {
            format!("fixture quest is missing for character {character_guid}: {quest_entry}")
        })
}

#[spacetimedb::reducer]
#[allow(clippy::too_many_lines)]
pub fn debug_verify_raid_quest_credit_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    let raid_members: Vec<u64> = std::iter::once(RQC_PLAYER)
        .chain(std::iter::once(RQC_RAID_OTHER))
        .chain((0..RQC_RAID_FILLER_COUNT).map(|i| RQC_RAID_FILLER_START + i))
        .collect();
    let fixture_characters: Vec<u64> = raid_members
        .iter()
        .copied()
        .chain([RQC_PARTY_KILLER])
        .collect();
    let fixture_quests = [RQC_NORMAL_QUEST, RQC_RAID_QUEST, RQC_CAST_QUEST];

    let logs = ctx.db.game_character_quest();
    for &character_guid in &fixture_characters {
        for quest in logs
            .by_character()
            .filter(&character_guid)
            .filter(|q| fixture_quests.contains(&q.quest_entry))
            .collect::<Vec<_>>()
        {
            crate::quest::CharacterQuest::delete(ctx, quest.id);
        }
    }
    let templates = ctx.db.game_quest_template();
    for quest_entry in fixture_quests {
        templates.entry().delete(quest_entry);
    }
    let objectives = ctx.db.game_quest_objective();
    for id in 509_297_100..509_297_110 {
        objectives.id().delete(id);
    }
    ctx.db.game_quest_cast_objective().id().delete(509_297_108);
    let groups = ctx.db.game_group();
    groups.group_id().delete(RQC_RAID_GROUP);
    groups.group_id().delete(RQC_PARTY_GROUP);
    let group_members = ctx.db.game_group_member();
    for member in group_members
        .iter()
        .filter(|m| fixture_characters.contains(&m.character_guid))
        .collect::<Vec<_>>()
    {
        group_members.id().delete(member.id);
    }
    let entities = ctx.db.game_world_entity();
    for &guid in fixture_characters.iter().filter(|&&g| g != RQC_PLAYER) {
        crate::WorldEntity::delete(ctx, guid);
    }
    crate::loot::tag::clear(ctx, RQC_WOLF_GUID);
    crate::WorldEntity::delete(ctx, RQC_WOLF_GUID);
    ctx.db.game_creature_spawn().guid().delete(RQC_WOLF_GUID);

    let player = entities
        .guid()
        .find(RQC_PLAYER)
        .ok_or_else(|| "raid quest credit fixture player is not live".to_string())?;
    let fixture_owner = player.owner_identity;
    for &guid in fixture_characters.iter().filter(|&&g| g != RQC_PLAYER) {
        let mut clone = entities
            .guid()
            .find(RQC_PLAYER)
            .ok_or_else(|| "raid quest credit fixture player disappeared".to_string())?;
        clone.guid = guid;
        clone.insert(ctx);
    }

    groups.insert(crate::Group {
        group_id: RQC_RAID_GROUP,
        leader_guid: RQC_PLAYER,
        loot_method: crate::group::loot_method::FFA,
        loot_threshold: 0,
        rr_cursor: 0,
        master_looter_guid: 0,
        group_type: 1,
    });
    for (offset, &character_guid) in raid_members.iter().enumerate() {
        let subgroup = (offset / lyracore_shared::group::SUBGROUP_SIZE) as u8;
        let raid_slot = lyracore_shared::group::RaidSlot::new(subgroup, false)
            .expect("RQC_RAID_FILLER_COUNT keeps every Subgroup index below RAID_SUBGROUPS")
            .wire();
        group_members.insert(crate::GroupMember {
            id: 509_297_200 + offset as u64,
            group_id: RQC_RAID_GROUP,
            character_guid,
            owner_identity: fixture_owner,
            raid_slot,
        });
    }
    groups.insert(crate::Group {
        group_id: RQC_PARTY_GROUP,
        leader_guid: RQC_PARTY_KILLER,
        loot_method: crate::group::loot_method::FFA,
        loot_threshold: 0,
        rr_cursor: 0,
        master_looter_guid: 0,
        group_type: 0,
    });
    group_members.insert(crate::GroupMember {
        id: 509_297_220,
        group_id: RQC_PARTY_GROUP,
        character_guid: RQC_PARTY_KILLER,
        owner_identity: fixture_owner,
        raid_slot: 0,
    });
    if !crate::group::in_raid(ctx, RQC_PLAYER) {
        return Err("the fixture Raid leader must read as in_raid".to_string());
    }
    if crate::group::in_raid(ctx, RQC_PARTY_KILLER) {
        return Err("the fixture Party killer must not read as in_raid".to_string());
    }

    let quest_template = |entry: u32, title: &str, quest_type: u32| QuestTemplate {
        entry,
        min_level: 0,
        quest_level: 1,
        title: title.to_string(),
        reward_money: 0,
        reward_xp: 0,
        prev_quest_id: 0,
        required_races: 0,
        required_classes: 0,
        zone_or_sort: 0,
        rew_rep_faction_1: 0,
        rew_rep_value_1: 0,
        rew_rep_faction_2: 0,
        rew_rep_value_2: 0,
        src_item: 0,
        src_item_count: 0,
        repeatable: false,
        next_quest_id: 0,
        limit_time: 0,
        reward_money_max_level: 0,
        quest_type,
    };
    templates.insert(quest_template(
        RQC_NORMAL_QUEST,
        "Raid credit: normal quest",
        0,
    ));
    templates.insert(quest_template(
        RQC_RAID_QUEST,
        "Raid credit: raid quest",
        QUEST_TYPE_RAID,
    ));
    templates.insert(quest_template(
        RQC_CAST_QUEST,
        "Raid credit: cast-gated kill",
        0,
    ));
    for (id, quest_entry, obj_index, kind, target_entry) in [
        (
            509_297_100,
            RQC_NORMAL_QUEST,
            0u8,
            objective_kind::KILL_CREATURE,
            RQC_CREATURE_ENTRY,
        ),
        (
            509_297_101,
            RQC_NORMAL_QUEST,
            1u8,
            objective_kind::USE_GAMEOBJECT,
            RQC_GAMEOBJECT_ENTRY,
        ),
        (
            509_297_102,
            RQC_NORMAL_QUEST,
            2u8,
            objective_kind::EXPLORE_AREATRIGGER,
            RQC_TRIGGER_ID,
        ),
        (
            509_297_103,
            RQC_NORMAL_QUEST,
            3u8,
            objective_kind::COLLECT_ITEM,
            RQC_ITEM_NORMAL,
        ),
        (
            509_297_104,
            RQC_RAID_QUEST,
            0u8,
            objective_kind::KILL_CREATURE,
            RQC_CREATURE_ENTRY,
        ),
        (
            509_297_105,
            RQC_RAID_QUEST,
            1u8,
            objective_kind::USE_GAMEOBJECT,
            RQC_GAMEOBJECT_ENTRY,
        ),
        (
            509_297_106,
            RQC_RAID_QUEST,
            2u8,
            objective_kind::EXPLORE_AREATRIGGER,
            RQC_TRIGGER_ID,
        ),
        (
            509_297_107,
            RQC_RAID_QUEST,
            3u8,
            objective_kind::COLLECT_ITEM,
            RQC_ITEM_RAID,
        ),
        (
            509_297_109,
            RQC_CAST_QUEST,
            0u8,
            objective_kind::KILL_CREATURE,
            RQC_CREATURE_ENTRY,
        ),
    ] {
        objectives.insert(QuestObjective {
            id,
            quest_entry,
            obj_index,
            kind,
            target_entry,
            required_count: 1,
        });
    }
    ctx.db
        .game_quest_cast_objective()
        .insert(QuestCastObjective {
            id: 509_297_108,
            quest_entry: RQC_CAST_QUEST,
            obj_index: 0,
            spell_id: RQC_CAST_SPELL,
        });
    for (id, character_guid, quest_entry) in [
        (509_297_300, RQC_PLAYER, RQC_NORMAL_QUEST),
        (509_297_301, RQC_RAID_OTHER, RQC_RAID_QUEST),
        (509_297_302, RQC_PARTY_KILLER, RQC_NORMAL_QUEST),
        (509_297_303, RQC_PLAYER, RQC_CAST_QUEST),
    ] {
        crate::quest::CharacterQuest::delete(ctx, id);
        crate::quest::CharacterQuest::insert(
            ctx,
            CharacterQuest {
                id,
                character_guid,
                owner_identity: fixture_owner,
                quest_entry,
                counts: vec![0, 0, 0, 0],
                rewarded: false,
                deadline_micros: 0,
                failed: false,
            },
        );
    }

    on_creature_killed(ctx, RQC_PLAYER, RQC_CREATURE_ENTRY);
    if rqc_progress(ctx, RQC_PLAYER, RQC_NORMAL_QUEST, 0)? != 0 {
        return Err("a Raid member's kill credited a non-Raid quest".to_string());
    }
    on_creature_killed(ctx, RQC_RAID_OTHER, RQC_CREATURE_ENTRY);
    if rqc_progress(ctx, RQC_RAID_OTHER, RQC_RAID_QUEST, 0)? != 1 {
        return Err("a Raid member's kill did not credit its own Raid Quest".to_string());
    }
    on_creature_killed(ctx, RQC_PARTY_KILLER, RQC_CREATURE_ENTRY);
    if rqc_progress(ctx, RQC_PARTY_KILLER, RQC_NORMAL_QUEST, 0)? != 1 {
        return Err("a Party member's kill credit changed behavior".to_string());
    }

    let cast_context = EventAiQuestCreditContext {
        source_creature_guid: 0,
        source_x: 0.0,
        source_y: 0.0,
        source_map_id: 0,
        source_instance_id: 0,
        selected_character: Some(RQC_PLAYER),
        invoker_beneficiary: None,
    };
    let cast_request = EventAiQuestCredit::CastCredit(CastCredit {
        creature_entry: RQC_CREATURE_ENTRY,
        spell_id: RQC_CAST_SPELL,
        recipient_policy: QuestCreditRecipientPolicy::SelectedCharacter,
    });
    if apply_eventai_credit(ctx, cast_request, &cast_context) != QuestCreditOutcome::Applied {
        return Err("cast credit was refused".to_string());
    }
    if rqc_progress(ctx, RQC_PLAYER, RQC_CAST_QUEST, 0)? != 1 {
        return Err("a Raid member's spell-gated cast credit was wrongly raid-gated".to_string());
    }
    if rqc_progress(ctx, RQC_PLAYER, RQC_NORMAL_QUEST, 0)? != 0 {
        return Err(
            "cast credit's setup leaked into the unrelated normal-quest kill objective".to_string(),
        );
    }

    on_gameobject_used(ctx, RQC_PLAYER, RQC_GAMEOBJECT_ENTRY);
    if rqc_progress(ctx, RQC_PLAYER, RQC_NORMAL_QUEST, 1)? != 0 {
        return Err("a Raid member's gameobject use credited a non-Raid quest".to_string());
    }
    on_gameobject_used(ctx, RQC_RAID_OTHER, RQC_GAMEOBJECT_ENTRY);
    if rqc_progress(ctx, RQC_RAID_OTHER, RQC_RAID_QUEST, 1)? != 1 {
        return Err("a Raid member's gameobject use did not credit its own Raid Quest".to_string());
    }

    on_areatrigger_entered(ctx, RQC_PLAYER, RQC_TRIGGER_ID);
    if rqc_progress(ctx, RQC_PLAYER, RQC_NORMAL_QUEST, 2)? != 1 {
        return Err("a Raid member's area-trigger credit was gated".to_string());
    }

    if crate::loot::killer_needs_item(ctx, Some(RQC_PLAYER), RQC_ITEM_NORMAL) {
        return Err("a Raid member still needed a non-Raid quest's item".to_string());
    }
    if !crate::loot::killer_needs_item(ctx, Some(RQC_RAID_OTHER), RQC_ITEM_RAID) {
        return Err("a Raid member stopped needing its own Raid Quest's item".to_string());
    }
    if !crate::loot::killer_needs_item(ctx, Some(RQC_PARTY_KILLER), RQC_ITEM_NORMAL) {
        return Err("a Party member's quest-item need changed behavior".to_string());
    }

    let wolf_template = ctx
        .db
        .game_creature_template()
        .entry()
        .find(RQC_WOLF_TEMPLATE)
        .ok_or_else(|| "the seeded Test Wolf template is missing".to_string())?;
    let spawn = crate::CreatureSpawn {
        guid: RQC_WOLF_GUID,
        entry: RQC_WOLF_TEMPLATE,
        map_id: player.map_id,
        x: player.x + 2.0,
        y: player.y,
        z: player.z,
        orientation: player.orientation,
        respawn_at: crate::creatures::timer_never(ctx),
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_IDLE,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: player.instance_id,
    };
    let wolf_entity =
        crate::creatures::build_creature_entity(ctx, &spawn, &wolf_template, 0, player.instance_id);
    ctx.db.game_creature_spawn().insert(spawn);
    crate::creatures::insert_creature_entity(ctx, wolf_entity);
    if !crate::loot::tag::record_first_threat(ctx, RQC_WOLF_GUID, RQC_PLAYER) {
        return Err("the raid quest credit fixture kill was not tapped".to_string());
    }
    let entitlement = crate::loot::tag::death_entitlement(
        ctx,
        RQC_WOLF_GUID,
        player.x,
        player.y,
        player.map_id,
        player.instance_id,
    )
    .ok_or_else(|| "the raid quest credit fixture kill has no death entitlement".to_string())?;
    if entitlement.recipients.len() != raid_members.len() {
        return Err(format!(
            "expected {} death-entitlement recipients for the 12-member Raid, got {}",
            raid_members.len(),
            entitlement.recipients.len()
        ));
    }
    crate::loot::tag::record_corpse_eligibility(ctx, RQC_WOLF_GUID, &entitlement.recipients);
    crate::loot::tag::corpse_access_gate(ctx, RQC_RAID_FILLER_START, RQC_WOLF_GUID)?;
    if crate::loot::tag::live_loot_tag_eligibility(ctx, RQC_WOLF_GUID, RQC_RAID_FILLER_START)
        != crate::loot::tag::LiveLootTagEligibility::Available
    {
        return Err(
            "live_loot_tag_eligibility's raised read bound did not cover a 12-member Raid"
                .to_string(),
        );
    }

    crate::loot::tag::clear(ctx, RQC_WOLF_GUID);
    crate::WorldEntity::delete(ctx, RQC_WOLF_GUID);
    ctx.db.game_creature_spawn().guid().delete(RQC_WOLF_GUID);
    Ok(())
}
