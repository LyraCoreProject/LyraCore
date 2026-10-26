use crate::actor::{ActionRefusal, ActionRefusalKind};
use std::collections::BTreeSet;

use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::game_area_trigger;
use crate::game_gameobject;
use crate::game_item_instance;
use crate::game_item_template;
use crate::game_world_entity;
#[cfg(feature = "debug_reducers")]
use crate::group::{game_group, game_group_member};
#[cfg(feature = "debug_reducers")]
use crate::reputation::game_player_reputation;
#[cfg(feature = "debug_reducers")]
use crate::{game_creature_spawn, game_creature_template};

pub use lyracore_shared::quest::objective_kind;

pub(crate) use lyracore_shared::quest::allowed_in_raid;
#[cfg(feature = "debug_reducers")]
pub(crate) use lyracore_shared::quest::QUEST_TYPE_RAID;

pub(crate) fn quest_type_of(ctx: &ReducerContext, quest_entry: u32) -> u32 {
    ctx.db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .map_or(0, |quest| quest.quest_type)
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuestCreditRecipientPolicy {
    SelectedCharacter,
    InvokerBeneficiary,
    TapGroup,
    EligibleGroup,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuestEvent {
    pub quest_entry: u32,
    pub recipient_policy: QuestCreditRecipientPolicy,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct CastCredit {
    pub creature_entry: u32,
    pub spell_id: u32,
    pub recipient_policy: QuestCreditRecipientPolicy,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct KillCredit {
    pub creature_entry: u32,
    pub recipient_policy: QuestCreditRecipientPolicy,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventAiQuestCredit {
    QuestEvent(QuestEvent),
    CastCredit(CastCredit),
    KillCredit(KillCredit),
}

#[derive(Clone, Debug)]
pub(crate) struct EventAiQuestCreditContext {
    pub source_creature_guid: u64,
    pub source_x: f32,
    pub source_y: f32,
    pub source_map_id: u32,
    pub source_instance_id: u64,
    pub selected_character: Option<u64>,
    pub invoker_beneficiary: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuestCreditOutcome {
    Applied,
    Refused,
}

pub mod quest_role {
    pub const START: u8 = 0;
    pub const END: u8 = 1;
}

const QUEST_GIVER_RANGE_SQ: f32 = 100.0;

const PARTY_SHARE_RANGE_SQ: f32 = crate::group::GROUP_XP_RANGE_SQ;

pub(crate) const MAX_QUEST_LOG_SIZE: usize = 20;

const QUEST_MAX_LEVEL_PAYOUT: u32 = 60;

#[table(accessor = game_quest_template, public)]
pub struct QuestTemplate {
    #[primary_key]
    pub entry: u32,
    pub min_level: u32,
    pub quest_level: u32,
    pub title: String,
    pub reward_money: u32,
    pub reward_xp: u32,
    #[default(0)]
    pub prev_quest_id: u32,
    #[default(0)]
    pub required_races: u32,
    #[default(0)]
    pub required_classes: u32,
    #[default(0)]
    pub zone_or_sort: i32,
    #[default(0)]
    pub rew_rep_faction_1: u32,
    #[default(0)]
    pub rew_rep_value_1: i32,
    #[default(0)]
    pub rew_rep_faction_2: u32,
    #[default(0)]
    pub rew_rep_value_2: i32,
    #[default(0)]
    pub src_item: u32,
    #[default(0)]
    pub src_item_count: u32,
    #[default(false)]
    pub repeatable: bool,
    #[default(0)]
    pub next_quest_id: u32,
    #[default(0)]
    pub limit_time: u32,
    #[default(0)]
    pub reward_money_max_level: u32,
    #[default(0)]
    pub quest_type: u32,
}

#[table(accessor = game_quest_text, public)]
pub struct QuestText {
    #[primary_key]
    pub quest_entry: u32,
    pub details: String,
    pub objectives: String,
    pub offer_reward_text: String,
    pub request_items_text: String,
}

#[table(accessor = game_quest_objective, public, index(accessor = by_quest, btree(columns = [quest_entry])))]
pub struct QuestObjective {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub quest_entry: u32,
    pub obj_index: u8,
    pub kind: u8,
    pub target_entry: u32,
    pub required_count: u32,
}

#[table(
    accessor = game_quest_event_requirement,
    index(accessor = by_quest, btree(columns = [quest_entry]))
)]
pub struct QuestEventRequirement {
    #[primary_key]
    pub id: u64,
    pub quest_entry: u32,
}

#[table(
    accessor = game_quest_cast_objective,
    index(accessor = by_quest, btree(columns = [quest_entry]))
)]
pub struct QuestCastObjective {
    #[primary_key]
    pub id: u64,
    pub quest_entry: u32,
    pub obj_index: u8,
    pub spell_id: u32,
}

#[table(accessor = game_quest_reward_item, public, index(accessor = by_quest, btree(columns = [quest_entry])))]
pub struct QuestRewardItem {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub quest_entry: u32,
    pub item_entry: u32,
    pub count: u32,
}

#[table(accessor = game_quest_reward_spell)]
pub struct QuestRewardSpell {
    #[primary_key]
    pub quest_entry: u32,
    pub spell_id: u32,
}

#[table(accessor = game_quest_reward_choice, public, index(accessor = by_quest, btree(columns = [quest_entry])))]
pub struct QuestRewardChoice {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub quest_entry: u32,
    pub choice_index: u8,
    pub item_entry: u32,
    pub count: u32,
}

#[table(accessor = game_creature_quest, public, index(accessor = by_creature, btree(columns = [creature_entry])))]
pub struct CreatureQuest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub quest_entry: u32,
    pub role: u8,
}

#[table(accessor = game_gameobject_quest, public, index(accessor = by_gameobject, btree(columns = [go_entry])))]
pub struct GameObjectQuest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub go_entry: u32,
    pub quest_entry: u32,
    pub role: u8,
}

#[table(
    accessor = game_character_quest,
    public,
    index(accessor = by_character, btree(columns = [character_guid])),
    index(
        accessor = by_character_quest,
        btree(columns = [character_guid, quest_entry])
    ),
    index(
        accessor = by_character_active,
        btree(columns = [character_guid, rewarded, failed])
    )
)]
pub struct CharacterQuest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub quest_entry: u32,
    pub counts: Vec<u32>,
    pub rewarded: bool,
    #[default(0i64)]
    pub deadline_micros: i64,
    #[default(false)]
    pub failed: bool,
}

#[table(
    accessor = game_character_quest_event_credit,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct CharacterQuestEventCredit {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub quest_entry: u32,
}

impl CharacterQuest {
    pub(crate) fn insert(ctx: &ReducerContext, row: Self) -> Self {
        let row = ctx.db.game_character_quest().insert(row);
        crate::native::quest_changed(ctx, None, Some(&row));
        row
    }
    pub(crate) fn update(self, ctx: &ReducerContext) -> Self {
        let table = ctx.db.game_character_quest();
        let old = table.id().find(self.id);
        let row = table.id().update(self);
        crate::native::quest_changed(ctx, old.as_ref(), Some(&row));
        row
    }
    pub(crate) fn delete(ctx: &ReducerContext, id: u64) -> bool {
        let table = ctx.db.game_character_quest();
        let old = table.id().find(id);
        let removed = table.id().delete(id);
        if let Some(row) = old {
            crate::native::quest_changed(ctx, Some(&row), None);
        }
        removed
    }
}
impl CharacterQuestEventCredit {
    fn insert(ctx: &ReducerContext, row: Self) -> Self {
        let row = ctx.db.game_character_quest_event_credit().insert(row);
        crate::native::quest_event_credit(ctx, &row, true);
        row
    }
    fn delete(ctx: &ReducerContext, id: u64) -> bool {
        let table = ctx.db.game_character_quest_event_credit();
        let old = table.id().find(id);
        let removed = table.id().delete(id);
        if let Some(row) = old {
            crate::native::quest_event_credit(ctx, &row, false);
        }
        removed
    }
}

crate::character_owned!(delete, fn sweep_delete_game_character_quest(ctx, character_guid) {
    let quests = ctx.db.game_character_quest();
    for r in quests.by_character().filter(&character_guid).collect::<Vec<_>>() {
        crate::quest::CharacterQuest::delete(ctx, r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_character_quest(ctx, character_guid, io) {
    table = game_character_quest,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_character_quest(ctx, character_guid, identity) {
    let quests = ctx.db.game_character_quest();
    for mut r in quests.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            r.update(ctx);
        }
    }
});

crate::character_owned!(delete, fn sweep_delete_game_character_quest_event_credit(ctx, character_guid) {
    let credits = ctx.db.game_character_quest_event_credit();
    for row in credits.by_character().filter(&character_guid).collect::<Vec<_>>() {
        CharacterQuestEventCredit::delete(ctx, row.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_character_quest_event_credit(ctx, character_guid, io) {
    table = game_character_quest_event_credit,
    by = by_character,
    remint = id,
});

pub const MAX_OBJECTIVES: usize = 4;

pub(crate) fn quest_is_complete(ctx: &ReducerContext, cq: &CharacterQuest) -> bool {
    let requires_event_credit = ctx
        .db
        .game_quest_event_requirement()
        .by_quest()
        .filter(&cq.quest_entry)
        .next()
        .is_some();
    let has_event_credit = ctx
        .db
        .game_character_quest_event_credit()
        .by_character()
        .filter(&cq.character_guid)
        .any(|credit| credit.quest_entry == cq.quest_entry);
    lyracore_shared::quest::objectives_complete(
        ctx.db
            .game_quest_objective()
            .by_quest()
            .filter(&cq.quest_entry)
            .map(|obj| lyracore_shared::quest::Objective {
                kind: obj.kind,
                index: obj.obj_index,
                target_entry: obj.target_entry,
                required_count: obj.required_count,
            }),
        &cq.counts,
        |entry| crate::items::item_count(ctx, cq.character_guid, entry),
        requires_event_credit,
        has_event_credit,
    )
}

fn creature_has_quest_role(
    ctx: &ReducerContext,
    creature_entry: u32,
    quest_entry: u32,
    role: u8,
) -> bool {
    ctx.db
        .game_creature_quest()
        .by_creature()
        .filter(&creature_entry)
        .any(|r| r.quest_entry == quest_entry && r.role == role)
}

fn gameobject_has_quest_role(
    ctx: &ReducerContext,
    go_entry: u32,
    quest_entry: u32,
    role: u8,
) -> bool {
    ctx.db
        .game_gameobject_quest()
        .by_gameobject()
        .filter(&go_entry)
        .any(|r| r.quest_entry == quest_entry && r.role == role)
}

pub(crate) fn character_quest_row(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Option<CharacterQuest> {
    ctx.db
        .game_character_quest()
        .by_character_quest()
        .filter((character_guid, quest_entry))
        .next()
}

pub(crate) fn quest_is_taken(ctx: &ReducerContext, character_guid: u64, quest_entry: u32) -> bool {
    character_quest_row(ctx, character_guid, quest_entry)
        .is_some_and(|quest| !quest.rewarded && !quest.failed)
}

pub(crate) fn quest_is_rewarded(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> bool {
    character_quest_row(ctx, character_guid, quest_entry).is_some_and(|q| q.rewarded)
}

enum GiverKind {
    Creature(u32),
    GameObject(u32),
    Item(u32),
    Party(u64),
}

fn validate_giver(
    ctx: &ReducerContext,
    player: &crate::WorldEntity,
    giver_guid: u64,
) -> Result<GiverKind, ActionRefusal> {
    if let Some(giver) = ctx.db.game_world_entity().guid().find(giver_guid) {
        if giver.is_player() {
            return validate_party_giver(ctx, player, giver_guid, &giver);
        }
        if giver.map_id != player.map_id || giver.instance_id != player.instance_id {
            return Err(ActionRefusal::new(
                ActionRefusalKind::OtherPartition,
                "quest giver on another map",
            ));
        }
        let (dx, dy, dz) = (giver.x - player.x, giver.y - player.y, giver.z - player.z);
        if dx * dx + dy * dy + dz * dz > QUEST_GIVER_RANGE_SQ {
            return Err(ActionRefusal::new(
                ActionRefusalKind::OutOfRange,
                "quest giver out of range",
            ));
        }
        return Ok(GiverKind::Creature(giver.entry));
    }
    if let Some(go) = ctx.db.game_gameobject().guid().find(giver_guid) {
        if go.map_id != player.map_id || go.instance_id != player.instance_id {
            return Err(ActionRefusal::new(
                ActionRefusalKind::OtherPartition,
                "quest giver on another map",
            ));
        }
        let (dx, dy, dz) = (go.x - player.x, go.y - player.y, go.z - player.z);
        if dx * dx + dy * dy + dz * dz > QUEST_GIVER_RANGE_SQ {
            return Err(ActionRefusal::new(
                ActionRefusalKind::OutOfRange,
                "quest giver out of range",
            ));
        }
        return Ok(GiverKind::GameObject(go.template_entry));
    }
    ctx.db
        .game_item_instance()
        .guid()
        .find(giver_guid)
        .filter(|i| i.owner_guid == player.guid)
        .map(|i| GiverKind::Item(i.entry))
        .ok_or_else(|| ActionRefusal::new(ActionRefusalKind::MissingTarget, "no such quest giver"))
}

pub(crate) fn quest_ender(
    ctx: &ReducerContext,
    giver_guid: u64,
) -> Option<lyracore_shared::mail::QuestGiver> {
    use lyracore_shared::mail::QuestGiver;
    if let Some(giver) = ctx.db.game_world_entity().guid().find(giver_guid) {
        return (!giver.is_player()).then_some(QuestGiver::Creature(giver.entry));
    }
    ctx.db
        .game_gameobject()
        .guid()
        .find(giver_guid)
        .map(|go| QuestGiver::Gameobject(go.template_entry))
}

fn validate_party_giver(
    ctx: &ReducerContext,
    player: &crate::WorldEntity,
    giver_guid: u64,
    giver: &crate::WorldEntity,
) -> Result<GiverKind, ActionRefusal> {
    let same_group = match (
        crate::group::group_of(ctx, player.guid),
        crate::group::group_of(ctx, giver_guid),
    ) {
        (Some(a), Some(b)) => a.group_id == b.group_id,
        _ => false,
    };
    if !same_group {
        return Err("target is not a quest giver".to_string().into());
    }
    if giver.map_id != player.map_id || giver.instance_id != player.instance_id {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "quest giver on another map",
        ));
    }
    let (dx, dy, dz) = (giver.x - player.x, giver.y - player.y, giver.z - player.z);
    if dx * dx + dy * dy + dz * dz > PARTY_SHARE_RANGE_SQ {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OutOfRange,
            "quest giver out of range",
        ));
    }
    Ok(GiverKind::Party(giver_guid))
}

fn giver_has_quest_role(
    ctx: &ReducerContext,
    giver: &GiverKind,
    quest_entry: u32,
    role: u8,
) -> bool {
    match *giver {
        GiverKind::Creature(entry) => creature_has_quest_role(ctx, entry, quest_entry, role),
        GiverKind::GameObject(entry) => gameobject_has_quest_role(ctx, entry, quest_entry, role),
        GiverKind::Item(entry) => {
            role == quest_role::START
                && ctx
                    .db
                    .game_item_template()
                    .entry()
                    .find(entry)
                    .is_some_and(|t| t.start_quest == quest_entry)
        }
        GiverKind::Party(sharer_guid) => {
            role == quest_role::START
                && character_quest_row(ctx, sharer_guid, quest_entry)
                    .is_some_and(|q| !q.rewarded && !q.failed)
        }
    }
}

pub(crate) fn accept_gates(
    ctx: &ReducerContext,
    player: &crate::WorldEntity,
    tmpl: &QuestTemplate,
) -> Result<Option<CharacterQuest>, String> {
    use lyracore_shared::quest::{acceptance, AcceptanceRequirements, Attempt};
    let existing = character_quest_row(ctx, player.guid, tmpl.entry);
    acceptance(
        AcceptanceRequirements {
            min_level: tmpl.min_level,
            required_races: tmpl.required_races,
            required_classes: tmpl.required_classes,
            repeatable: tmpl.repeatable,
        },
        player.level,
        player.race(),
        player.class(),
        tmpl.prev_quest_id == 0 || quest_is_rewarded(ctx, player.guid, tmpl.prev_quest_id),
        existing.as_ref().map(|row| Attempt {
            rewarded: row.rewarded,
            failed: row.failed,
        }),
    )
    .map_err(|refusal| refusal.text(tmpl.min_level))?;
    Ok(existing)
}

pub(crate) fn apply_accept_quest(
    ctx: &ReducerContext,
    player_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    request_accept_quest(ctx, player_guid, giver_guid, quest_entry).map_err(Into::into)
}

pub(crate) fn request_accept_quest(
    ctx: &ReducerContext,
    player_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), ActionRefusal> {
    let player = crate::helpers::live_entity(ctx, player_guid)
        .map_err(|_| ActionRefusal::new(ActionRefusalKind::MissingActor, "player not in world"))?;
    if player.dead {
        return Err(ActionRefusal::new(
            ActionRefusalKind::DeadActor,
            "dead players cannot accept quests",
        ));
    }
    let giver = validate_giver(ctx, &player, giver_guid)?;
    if !giver_has_quest_role(ctx, &giver, quest_entry, quest_role::START) {
        return Err("that quest giver does not offer this quest"
            .to_string()
            .into());
    }
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;
    let existing = accept_gates(ctx, &player, &tmpl)?;
    if existing.as_ref().is_none_or(|row| row.rewarded)
        && quest_slots_used(ctx, player_guid) >= MAX_QUEST_LOG_SIZE
    {
        return Err(lyracore_shared::quest::QUEST_LOG_FULL_TEXT
            .to_string()
            .into());
    }
    let deadline_micros = if tmpl.limit_time > 0 {
        ctx.timestamp.to_micros_since_unix_epoch() + (tmpl.limit_time as i64) * 1_000_000
    } else {
        0
    };
    apply_accept_effects(
        ctx,
        player_guid,
        player.owner_identity,
        quest_entry,
        &tmpl,
        existing,
        deadline_micros,
    )
}

fn apply_accept_effects(
    ctx: &ReducerContext,
    character_guid: u64,
    owner_identity: Identity,
    quest_entry: u32,
    tmpl: &QuestTemplate,
    existing: Option<CharacterQuest>,
    deadline_micros: i64,
) -> Result<(), ActionRefusal> {
    if tmpl.src_item != 0 {
        crate::items::request_grant_item(
            ctx,
            character_guid,
            tmpl.src_item,
            tmpl.src_item_count,
            None,
        )?;
    }
    let num_objectives = ctx
        .db
        .game_quest_objective()
        .by_quest()
        .filter(&quest_entry)
        .count()
        .min(MAX_OBJECTIVES);
    let counts = vec![0u32; num_objectives];
    let event_credits = ctx.db.game_character_quest_event_credit();
    for credit in event_credits
        .by_character()
        .filter(&character_guid)
        .filter(|credit| credit.quest_entry == quest_entry)
        .collect::<Vec<_>>()
    {
        CharacterQuestEventCredit::delete(ctx, credit.id);
    }
    match existing {
        Some(mut row) => {
            row.counts = counts;
            row.rewarded = false;
            row.failed = false;
            row.deadline_micros = deadline_micros;
            row.update(ctx);
        }
        None => {
            crate::quest::CharacterQuest::insert(
                ctx,
                CharacterQuest {
                    id: 0,
                    character_guid,
                    owner_identity,
                    quest_entry,
                    counts,
                    rewarded: false,
                    deadline_micros,
                    failed: false,
                },
            );
        }
    }
    crate::hooks::fire_on_quest_accept(
        ctx,
        &crate::hooks::QuestAcceptPayload {
            character_guid,
            quest_entry,
        },
    );
    Ok(())
}

#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn grant_quest_unchecked(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;
    if character_quest_row(ctx, character_guid, quest_entry).is_some() {
        return Err("already on or completed that quest".to_string());
    }
    let owner_identity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .map(|e| e.owner_identity)
        .ok_or_else(|| format!("character {character_guid} not in world"))?;
    apply_accept_effects(
        ctx,
        character_guid,
        owner_identity,
        quest_entry,
        &tmpl,
        None,
        0,
    )
    .map_err(Into::into)
}

fn pick_choice_reward(
    choices: &[(u8, u32, u32)],
    reward_index: u32,
    quest_entry: u32,
) -> Result<Option<(u32, u32)>, String> {
    if choices.is_empty() {
        return Ok(None);
    }
    choices
        .iter()
        .find(|(idx, _, _)| *idx as u32 == reward_index)
        .map(|&(_, item, count)| Some((item, count)))
        .ok_or_else(|| format!("invalid reward choice {reward_index} for quest {quest_entry}"))
}

pub(crate) fn apply_turn_in_quest(
    ctx: &ReducerContext,
    player_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
    reward_index: u32,
) -> Result<(), String> {
    request_turn_in_quest(ctx, player_guid, giver_guid, quest_entry, reward_index)
        .map_err(Into::into)
}

pub(crate) fn request_turn_in_quest(
    ctx: &ReducerContext,
    player_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
    reward_index: u32,
) -> Result<(), ActionRefusal> {
    let entities = ctx.db.game_world_entity();
    let mut player = entities.guid().find(player_guid).ok_or_else(|| {
        ActionRefusal::new(ActionRefusalKind::MissingActor, "player not in world")
    })?;
    if player.dead {
        return Err(ActionRefusal::new(
            ActionRefusalKind::DeadActor,
            "dead players cannot turn in quests",
        ));
    }
    let giver = validate_giver(ctx, &player, giver_guid)?;
    if !giver_has_quest_role(ctx, &giver, quest_entry, quest_role::END) {
        return Err("that quest giver does not complete this quest"
            .to_string()
            .into());
    }
    let mut cq = character_quest_row(ctx, player_guid, quest_entry)
        .ok_or_else(|| "you are not on that quest".to_string())?;
    if cq.rewarded {
        return Err("quest already turned in".to_string().into());
    }
    if cq.failed {
        return Err("quest has expired".to_string().into());
    }
    if !quest_is_complete(ctx, &cq) {
        return Err("quest objectives not complete".to_string().into());
    }
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;

    let reward_spell = ctx
        .db
        .game_quest_reward_spell()
        .quest_entry()
        .find(quest_entry);
    if let Some(reward) = &reward_spell {
        use crate::game_spell;
        if ctx
            .db
            .game_spell()
            .spell_id()
            .find(reward.spell_id)
            .is_none()
        {
            return Err(format!("quest reward spell {} is missing", reward.spell_id).into());
        }
    }
    let owner_identity = player.owner_identity;

    let collect: Vec<(u32, u32)> = ctx
        .db
        .game_quest_objective()
        .by_quest()
        .filter(&quest_entry)
        .filter(|o| o.kind == objective_kind::COLLECT_ITEM)
        .map(|o| (o.target_entry, o.required_count))
        .collect();
    let mut rewards: Vec<(u32, u32)> = ctx
        .db
        .game_quest_reward_item()
        .by_quest()
        .filter(&quest_entry)
        .map(|reward| (reward.item_entry, reward.count))
        .collect();
    let choices: Vec<(u8, u32, u32)> = ctx
        .db
        .game_quest_reward_choice()
        .by_quest()
        .filter(&quest_entry)
        .map(|c| (c.choice_index, c.item_entry, c.count))
        .collect();
    if let Some(reward) = pick_choice_reward(&choices, reward_index, quest_entry)? {
        rewards.push(reward);
    }
    player.money = player.money.saturating_add(tmpl.reward_money);
    let base_xp = lyracore_shared::quest::base_reward_xp(
        tmpl.reward_xp,
        tmpl.reward_money_max_level,
        tmpl.quest_level,
    );
    let xp = crate::xp::rated_xp(ctx, base_xp);
    if player.level >= QUEST_MAX_LEVEL_PAYOUT {
        player.money = player
            .money
            .saturating_add(lyracore_shared::quest::max_level_money_reward(xp));
    }
    // Packages can catch a Refusal; resolve every item Gate before the infallible quest effects.
    crate::items::exchange_items(ctx, player_guid, &collect, &rewards)?;
    if player.level < QUEST_MAX_LEVEL_PAYOUT {
        crate::xp::XpEvent::send(
            ctx,
            player_guid,
            crate::xp::XpEvent {
                id: 0,
                recipient_identity: player.owner_identity,
                killed_guid: 0,
                total_exp: xp,
                created_at: ctx.timestamp,
                is_kill: false,
                rested_bonus: 0,
            },
        );
        crate::xp::grant_xp(ctx, &mut player, xp);
    }
    player.update(ctx);

    crate::reputation::grant_reputation(
        ctx,
        player_guid,
        tmpl.rew_rep_faction_1,
        tmpl.rew_rep_value_1,
    );
    crate::reputation::grant_reputation(
        ctx,
        player_guid,
        tmpl.rew_rep_faction_2,
        tmpl.rew_rep_value_2,
    );

    if let Some(reward) = reward_spell {
        crate::spell::learn_spell(ctx, player_guid, owner_identity, reward.spell_id);
    }

    // Keep rewarded repeatable rows because later quests read them as prerequisite evidence.
    cq.rewarded = true;
    cq.update(ctx);
    crate::hooks::fire_on_quest_turnin(
        ctx,
        &crate::hooks::QuestTurninPayload {
            character_guid: player_guid,
            quest_entry,
        },
    );
    Ok(())
}

#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_PLAYER: u64 = 1;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_GIVER: u64 = (0xF130_u64 << 48) | ((620_u64) << 24) | 0x00FF_FF00;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_GIVER_ENTRY: u32 = 620;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_GIVER_SOURCE: u64 = (0xF130_u64 << 48) | ((620_u64) << 24) | 1;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_QUEST: u32 = 509_033;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_OBJECTIVE_ITEM: u32 = 5_090_330;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_CHOICE_0: u32 = 5_090_331;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_CHOICE_1: u32 = 5_090_332;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_GUARANTEED: u32 = 5_090_333;
#[cfg(feature = "debug_reducers")]
const CHOICE_FIXTURE_FILLER: u32 = 5_090_334;

#[cfg(feature = "debug_reducers")]
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
    giver.x = player.x + 1.0;
    giver.y = player.y;
    giver.z = player.z;
    giver.map_id = player.map_id;
    giver.instance_id = player.instance_id;
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(giver.x, giver.y);
    giver.grid_x = grid_x;
    giver.grid_y = grid_y;
    giver.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
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

#[cfg(feature = "debug_reducers")]
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

#[cfg(feature = "debug_reducers")]
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

#[cfg(feature = "debug_reducers")]
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

#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_PLAYER: u64 = 1;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_MEMBER: u64 = 509_296_001;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_DISTANT_MEMBER: u64 = 509_296_002;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_BENEFICIARY: u64 = 509_296_003;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_SOURCE: u64 = (0xF130_u64 << 48) | ((620_u64) << 24) | 1;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_GROUP: u64 = 509_296;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_CREATURE: u32 = 509_296;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_OTHER_CREATURE: u32 = 509_297;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_KILL_QUEST: u32 = 5_092_961;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_FALLBACK_QUEST: u32 = 5_092_962;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_MISSING_OBJECTIVE_QUEST: u32 = 5_092_963;
#[cfg(feature = "debug_reducers")]
const EVENTAI_CREDIT_FIXTURE_EVENT_QUEST: u32 = 5_092_964;

#[cfg(feature = "debug_reducers")]
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

#[cfg(feature = "debug_reducers")]
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
    player.x = source.x;
    player.y = source.y;
    player.z = source.z;
    player.dead = false;
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(player.x, player.y);
    player.grid_x = grid_x;
    player.grid_y = grid_y;
    player.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
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
    distant_member.x = source.x + 100.0;
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(distant_member.x, distant_member.y);
    distant_member.grid_x = grid_x;
    distant_member.grid_y = grid_y;
    distant_member.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
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
    tapped_player.x = source.x + 100.0;
    let (grid_x, grid_y) = lyracore_shared::spatial::grid_cell(tapped_player.x, tapped_player.y);
    tapped_player.grid_x = grid_x;
    tapped_player.grid_y = grid_y;
    tapped_player.cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
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

#[cfg(feature = "debug_reducers")]
const RQC_PLAYER: u64 = 1;
#[cfg(feature = "debug_reducers")]
const RQC_RAID_OTHER: u64 = 509_297_001;
#[cfg(feature = "debug_reducers")]
const RQC_PARTY_KILLER: u64 = 509_297_002;
#[cfg(feature = "debug_reducers")]
const RQC_RAID_FILLER_START: u64 = 509_297_010;
#[cfg(feature = "debug_reducers")]
const RQC_RAID_FILLER_COUNT: u64 = 10;
#[cfg(feature = "debug_reducers")]
const RQC_RAID_GROUP: u64 = 509_297;
#[cfg(feature = "debug_reducers")]
const RQC_PARTY_GROUP: u64 = 509_298;
#[cfg(feature = "debug_reducers")]
const RQC_NORMAL_QUEST: u32 = 5_092_970;
#[cfg(feature = "debug_reducers")]
const RQC_RAID_QUEST: u32 = 5_092_971;
#[cfg(feature = "debug_reducers")]
const RQC_CAST_QUEST: u32 = 5_092_972;
#[cfg(feature = "debug_reducers")]
const RQC_CREATURE_ENTRY: u32 = 509_297;
#[cfg(feature = "debug_reducers")]
const RQC_GAMEOBJECT_ENTRY: u32 = 509_298;
#[cfg(feature = "debug_reducers")]
const RQC_TRIGGER_ID: u32 = 509_299;
#[cfg(feature = "debug_reducers")]
const RQC_ITEM_NORMAL: u32 = 509_300;
#[cfg(feature = "debug_reducers")]
const RQC_ITEM_RAID: u32 = 509_301;
#[cfg(feature = "debug_reducers")]
const RQC_CAST_SPELL: u32 = 509_302;
#[cfg(feature = "debug_reducers")]
const RQC_WOLF_TEMPLATE: u32 = 51_000;
#[cfg(feature = "debug_reducers")]
const RQC_WOLF_GUID: u64 = (0xF130_u64 << 48) | ((RQC_WOLF_TEMPLATE as u64) << 24) | 509_297;

#[cfg(feature = "debug_reducers")]
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

#[cfg(feature = "debug_reducers")]
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
        crate::creatures::build_creature_entity(&spawn, &wolf_template, 0, player.instance_id);
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

fn credit_objective(
    ctx: &ReducerContext,
    character_guid: u64,
    kind: u8,
    target_entry: u32,
    spell_id: u32,
) {
    let kind_is_raid_gated = spell_id == 0
        && matches!(
            kind,
            objective_kind::KILL_CREATURE | objective_kind::USE_GAMEOBJECT
        );
    let in_raid = kind_is_raid_gated && crate::group::in_raid(ctx, character_guid);
    let log = ctx.db.game_character_quest();
    let active: Vec<CharacterQuest> = log
        .by_character()
        .filter(&character_guid)
        .filter(|q| !q.rewarded)
        .collect();
    for mut cq in active {
        if in_raid && !allowed_in_raid(quest_type_of(ctx, cq.quest_entry)) {
            continue;
        }
        let mut changed = false;
        for obj in ctx
            .db
            .game_quest_objective()
            .by_quest()
            .filter(&cq.quest_entry)
        {
            if obj.kind != kind
                || obj.target_entry != target_entry
                || !objective_matches_spell(ctx, cq.quest_entry, obj.obj_index, spell_id)
            {
                continue;
            }
            let idx = obj.obj_index as usize;
            let Some(have) = cq.counts.get(idx).copied() else {
                continue;
            };
            if have < obj.required_count {
                cq.counts[idx] = have + 1;
                changed = true;
            }
        }
        if changed {
            cq.update(ctx);
        }
    }
}

fn objective_matches_spell(
    ctx: &ReducerContext,
    quest_entry: u32,
    obj_index: u8,
    spell_id: u32,
) -> bool {
    ctx.db
        .game_quest_cast_objective()
        .by_quest()
        .filter(&quest_entry)
        .find(|objective| objective.obj_index == obj_index)
        .map_or(spell_id == 0, |objective| objective.spell_id == spell_id)
}

pub(crate) fn apply_eventai_credit(
    ctx: &ReducerContext,
    request: EventAiQuestCredit,
    credit_context: &EventAiQuestCreditContext,
) -> QuestCreditOutcome {
    let policy = match request {
        EventAiQuestCredit::QuestEvent(request) => request.recipient_policy,
        EventAiQuestCredit::CastCredit(request) => request.recipient_policy,
        EventAiQuestCredit::KillCredit(request) => request.recipient_policy,
    };
    let Ok(recipients) = eventai_credit_recipients(ctx, policy, credit_context) else {
        return QuestCreditOutcome::Refused;
    };

    match request {
        EventAiQuestCredit::QuestEvent(request) => {
            if ctx
                .db
                .game_quest_template()
                .entry()
                .find(request.quest_entry)
                .is_none()
            {
                return QuestCreditOutcome::Refused;
            }
            let credits = ctx.db.game_character_quest_event_credit();
            for character_guid in recipients {
                if !quest_is_taken(ctx, character_guid, request.quest_entry) {
                    continue;
                }
                if credits
                    .by_character()
                    .filter(&character_guid)
                    .any(|credit| credit.quest_entry == request.quest_entry)
                {
                    continue;
                }
                CharacterQuestEventCredit::insert(
                    ctx,
                    CharacterQuestEventCredit {
                        id: 0,
                        character_guid,
                        quest_entry: request.quest_entry,
                    },
                );
            }
        }
        EventAiQuestCredit::CastCredit(request) => {
            if request.creature_entry == 0 || request.spell_id == 0 {
                return QuestCreditOutcome::Refused;
            }
            for character_guid in recipients {
                credit_objective(
                    ctx,
                    character_guid,
                    objective_kind::KILL_CREATURE,
                    request.creature_entry,
                    request.spell_id,
                );
            }
        }
        EventAiQuestCredit::KillCredit(request) => {
            if request.creature_entry == 0 {
                return QuestCreditOutcome::Refused;
            }
            for character_guid in recipients {
                credit_objective(
                    ctx,
                    character_guid,
                    objective_kind::KILL_CREATURE,
                    request.creature_entry,
                    0,
                );
            }
        }
    }
    QuestCreditOutcome::Applied
}

fn eventai_credit_recipients(
    ctx: &ReducerContext,
    policy: QuestCreditRecipientPolicy,
    credit_context: &EventAiQuestCreditContext,
) -> Result<Vec<u64>, ()> {
    if policy == QuestCreditRecipientPolicy::TapGroup {
        return tap_credit_recipients(ctx, credit_context);
    }
    eventai_credit_recipient_set(
        policy,
        credit_context,
        |guid| live_character(ctx, guid),
        |root| {
            crate::group::kill_reward_recipients(
                ctx,
                root,
                credit_context.source_x,
                credit_context.source_y,
                credit_context.source_map_id,
                credit_context.source_instance_id,
            )
        },
    )
}

fn tap_credit_recipients(
    ctx: &ReducerContext,
    credit_context: &EventAiQuestCreditContext,
) -> Result<Vec<u64>, ()> {
    if let Some(entitlement) = crate::loot::tag::death_entitlement(
        ctx,
        credit_context.source_creature_guid,
        credit_context.source_x,
        credit_context.source_y,
        credit_context.source_map_id,
        credit_context.source_instance_id,
    ) {
        return (!entitlement.recipients.is_empty())
            .then_some(entitlement.recipients)
            .ok_or(());
    }

    let beneficiary = credit_context
        .invoker_beneficiary
        .filter(|guid| live_character(ctx, *guid))
        .ok_or(())?;
    let recipients = crate::group::kill_reward_recipients(
        ctx,
        beneficiary,
        credit_context.source_x,
        credit_context.source_y,
        credit_context.source_map_id,
        credit_context.source_instance_id,
    );
    (!recipients.is_empty()).then_some(recipients).ok_or(())
}

fn eventai_credit_recipient_set(
    policy: QuestCreditRecipientPolicy,
    credit_context: &EventAiQuestCreditContext,
    mut live_character: impl FnMut(u64) -> bool,
    mut eligible_group: impl FnMut(u64) -> Vec<u64>,
) -> Result<Vec<u64>, ()> {
    let selected = credit_context
        .selected_character
        .filter(|guid| live_character(*guid));
    let beneficiary = credit_context
        .invoker_beneficiary
        .filter(|guid| live_character(*guid));
    let recipients = match policy {
        QuestCreditRecipientPolicy::SelectedCharacter => vec![selected.ok_or(())?],
        QuestCreditRecipientPolicy::InvokerBeneficiary => vec![beneficiary.ok_or(())?],
        QuestCreditRecipientPolicy::TapGroup => return Err(()),
        QuestCreditRecipientPolicy::EligibleGroup => eligible_group(selected.ok_or(())?),
    };
    let recipients: BTreeSet<u64> = recipients.into_iter().collect();
    if recipients.is_empty() || !recipients.iter().copied().all(&mut live_character) {
        return Err(());
    }
    Ok(recipients.into_iter().collect())
}

fn live_character(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .is_some_and(|character| character.is_player() && !character.dead)
}

pub(crate) fn on_creature_killed(ctx: &ReducerContext, killer_guid: u64, creature_entry: u32) {
    credit_objective(
        ctx,
        killer_guid,
        objective_kind::KILL_CREATURE,
        creature_entry,
        0,
    );
}

pub(crate) fn on_gameobject_used(ctx: &ReducerContext, player_guid: u64, go_entry: u32) {
    credit_objective(
        ctx,
        player_guid,
        objective_kind::USE_GAMEOBJECT,
        go_entry,
        0,
    );
}

pub(crate) fn on_areatrigger_entered(ctx: &ReducerContext, player_guid: u64, trigger_id: u32) {
    credit_objective(
        ctx,
        player_guid,
        objective_kind::EXPLORE_AREATRIGGER,
        trigger_id,
        0,
    );
}

#[table(accessor = game_areatrigger_teleport)]
pub struct AreatriggerTeleport {
    #[primary_key]
    pub trigger_id: u32,
    pub target_map: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub o: f32,
    pub name: String,
}

#[derive(Clone, Copy)]
pub(crate) struct AreaTriggerRoute {
    pub source_map: u32,
    pub source_x: f32,
    pub source_y: f32,
    pub source_z: f32,
    pub target_map: u32,
    target_x: f32,
    target_y: f32,
    target_z: f32,
    target_o: f32,
    volume: AreaTriggerVolume,
}

impl AreaTriggerRoute {
    fn valid(self) -> bool {
        let coordinates = [
            self.source_x,
            self.source_y,
            self.source_z,
            self.target_x,
            self.target_y,
            self.target_z,
            self.target_o,
        ];
        coordinates.into_iter().all(f32::is_finite) && self.volume.valid()
    }

    #[cfg_attr(not(has_packages), allow(dead_code))]
    pub(crate) fn contains(self, x: f32, y: f32, z: f32) -> bool {
        self.valid()
            && self
                .volume
                .contains([self.source_x, self.source_y, self.source_z], [x, y, z])
    }
}

#[derive(Clone, Copy)]
struct AreaTriggerVolume {
    radius: f32,
    length: f32,
    width: f32,
    height: f32,
    yaw: f32,
}

impl AreaTriggerVolume {
    fn of(trigger: &crate::config::GameAreaTrigger) -> Self {
        Self {
            radius: trigger.radius,
            length: trigger.box_length,
            width: trigger.box_width,
            height: trigger.box_height,
            yaw: trigger.box_yaw,
        }
    }

    fn valid(self) -> bool {
        [self.radius, self.length, self.width, self.height, self.yaw]
            .into_iter()
            .all(f32::is_finite)
            && (self.radius > 0.0
                || (self.radius == 0.0
                    && self.length > 0.0
                    && self.width > 0.0
                    && self.height > 0.0))
    }

    fn contains(self, origin: [f32; 3], point: [f32; 3]) -> bool {
        if !self.valid() || !origin.into_iter().chain(point).all(f32::is_finite) {
            return false;
        }
        let [dx, dy, dz] =
            std::array::from_fn(|index| f64::from(point[index]) - f64::from(origin[index]));
        if self.radius > 0.0 {
            return dx * dx + dy * dy + dz * dz <= f64::from(self.radius).powi(2);
        }
        let (sin, cos) = f64::from(self.yaw).sin_cos();
        let local_x = dx * cos + dy * sin;
        let local_y = -dx * sin + dy * cos;
        local_x.abs() <= f64::from(self.length) / 2.0
            && local_y.abs() <= f64::from(self.width) / 2.0
            && dz.abs() <= f64::from(self.height) / 2.0
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn area_trigger_route(
    ctx: &ReducerContext,
    trigger_id: u32,
) -> Option<AreaTriggerRoute> {
    let volume = ctx.db.game_area_trigger().id().find(trigger_id)?;
    let teleport = ctx
        .db
        .game_areatrigger_teleport()
        .trigger_id()
        .find(trigger_id)?;
    let route = AreaTriggerRoute {
        source_map: volume.map_id,
        source_x: volume.x,
        source_y: volume.y,
        source_z: volume.z,
        target_map: teleport.target_map,
        target_x: teleport.x,
        target_y: teleport.y,
        target_z: teleport.z,
        target_o: teleport.o,
        volume: AreaTriggerVolume::of(&volume),
    };
    route.valid().then_some(route)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn enter_sessionless_areatrigger(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
    expected_map: u32,
    expected_instance: u64,
    controller_generation: u64,
) -> Result<u64, ActionRefusal> {
    crate::sessionless::transfer_authority_gate(ctx, character_guid)?;
    crate::sessionless::action_gate(ctx, character_guid)?;
    let route = area_trigger_route(ctx, trigger_id).ok_or_else(|| {
        ActionRefusal::new(
            ActionRefusalKind::MissingResource,
            format!("AreaTrigger {trigger_id} has no imported route"),
        )
    })?;
    if route.target_map != expected_map
        || (crate::instance::is_dungeon_map(expected_map) && expected_instance == 0)
        || (!crate::instance::is_dungeon_map(expected_map) && expected_instance != 0)
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "AreaTrigger does not enter the expected party partition",
        ));
    }
    let destination = crate::transfer::Destination {
        map_id: expected_map,
        instance_id: expected_instance,
        x: route.target_x,
        y: route.target_y,
        z: route.target_z,
        o: route.target_o,
    };
    if let Some(relocation_id) = crate::sessionless::completed_relocation(
        ctx,
        character_guid,
        trigger_id,
        (expected_map, expected_instance),
        controller_generation,
    ) {
        return Ok(relocation_id);
    }
    let entity = crate::helpers::live_entity(ctx, character_guid).map_err(|_| {
        ActionRefusal::new(ActionRefusalKind::MissingActor, "Character is not in world")
    })?;
    if entity.dead {
        return Err(ActionRefusal::new(
            ActionRefusalKind::DeadActor,
            "dead Characters cannot enter an AreaTrigger",
        ));
    }
    if crate::taxi::movement_is_suppressed(ctx, character_guid)
        || crate::spell::is_action_blocked(ctx, character_guid)
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "Character cannot enter an AreaTrigger",
        ));
    }
    if route.source_map != entity.map_id {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "AreaTrigger is on another map",
        ));
    }
    if !route.contains(entity.x, entity.y, entity.z) {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OutOfRange,
            "Character is outside the AreaTrigger",
        ));
    }
    if crate::instance::is_dungeon_map(expected_map) {
        let party_member_at_destination = crate::group::has_known_party_member_in_partition(
            ctx,
            character_guid,
            (expected_map, expected_instance),
        )
        .map_err(|_| {
            ActionRefusal::new(
                ActionRefusalKind::OtherPartition,
                "party partition facts are unavailable",
            )
        })?;
        if !party_member_at_destination {
            return Err(ActionRefusal::new(
                ActionRefusalKind::OtherPartition,
                "no party member is certified in the expected partition",
            ));
        }
    }
    let destination_instance = if crate::instance::is_dungeon_map(expected_map) {
        crate::instance::admit_existing_party_instance(
            ctx,
            character_guid,
            expected_map,
            expected_instance,
        )
        .map_err(|detail| ActionRefusal::new(ActionRefusalKind::CannotAct, detail))?
    } else {
        0
    };
    if destination_instance != expected_instance {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "AreaTrigger resolves to a different party instance",
        ));
    }
    Ok(crate::sessionless::relocate(
        ctx,
        character_guid,
        trigger_id,
        crate::transfer::Destination {
            instance_id: destination_instance,
            ..destination
        },
        controller_generation,
    ))
}

pub(crate) fn request_enter_areatrigger(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
) -> Result<(), String> {
    let actor = crate::helpers::live_entity(ctx, character_guid)?;
    let trigger = ctx
        .db
        .game_area_trigger()
        .id()
        .find(trigger_id)
        .ok_or("AreaTrigger is unavailable")?;
    if actor.map_id != trigger.map_id
        || !AreaTriggerVolume::of(&trigger).contains(
            [trigger.x, trigger.y, trigger.z],
            [actor.x, actor.y, actor.z],
        )
    {
        return Err("Character is outside the AreaTrigger".into());
    }
    apply_enter_areatrigger(ctx, character_guid, trigger_id);
    Ok(())
}

// A trigger may grant exploration credit and teleport; both effects are independent.
pub(crate) fn apply_enter_areatrigger(ctx: &ReducerContext, player_guid: u64, trigger_id: u32) {
    let teleport_target = ctx
        .db
        .game_areatrigger_teleport()
        .trigger_id()
        .find(trigger_id)
        .map(|t| (t.target_map, t.x, t.y, t.z, t.o));
    on_areatrigger_entered(ctx, player_guid, trigger_id);
    if let Some((target_map, x, y, z, o)) = teleport_target {
        let instance_id = if crate::instance::is_dungeon_map(target_map) {
            match crate::instance::resolve_or_create_instance(ctx, player_guid, target_map) {
                Ok(id) => id,
                Err(e) => {
                    spacetimedb::log::warn!(
                        "areatrigger {trigger_id}: dungeon entry refused for {player_guid}: {e}"
                    );
                    return;
                }
            }
        } else {
            0
        };
        crate::world::teleport_player(ctx, player_guid, target_map, instance_id, x, y, z, o);
    }
}

fn is_expired(now_micros: i64, deadline_micros: i64, rewarded: bool, failed: bool) -> bool {
    deadline_micros != 0 && !rewarded && !failed && now_micros >= deadline_micros
}

crate::game_tick_pass!(fn quest_timer_pass(ctx) {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let log = ctx.db.game_character_quest();
    let expired: Vec<CharacterQuest> = log
.iter()
.filter(|q| is_expired(now, q.deadline_micros, q.rewarded, q.failed))
.filter(|q| !crate::transfer::is_in_transit(ctx, q.character_guid))
.collect();
    for mut cq in expired {
        cq.failed = true;
        cq.deadline_micros = 0;
        cq.update(ctx);
    }
});

pub(crate) fn apply_push_quest_to_party(
    ctx: &ReducerContext,
    sender_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    if !character_quest_row(ctx, sender_guid, quest_entry).is_some_and(|q| !q.rewarded && !q.failed)
    {
        return Err("you are not on that quest".to_string());
    }
    let sender_entity = crate::helpers::live_entity(ctx, sender_guid)
        .map_err(|_| "player not in world".to_string())?;
    let m = crate::group::group_of(ctx, sender_guid).ok_or_else(|| "not in a group".to_string())?;
    let tmpl = ctx
        .db
        .game_quest_template()
        .entry()
        .find(quest_entry)
        .ok_or_else(|| format!("no such quest {quest_entry}"))?;
    for member in crate::group::members_of(ctx, m.group_id) {
        if member.character_guid == sender_guid {
            continue;
        }
        let result = evaluate_share(ctx, &sender_entity, &tmpl, member.character_guid);
        if result == lyracore_shared::quest::share_result::SHARING_QUEST {
            crate::group::push_event(
                ctx,
                member.character_guid,
                lyracore_shared::quest::share_event_kind::QUEST_SHARE,
                sender_guid,
                quest_entry.to_string(),
            );
        }
        crate::group::push_event(
            ctx,
            sender_guid,
            lyracore_shared::quest::share_event_kind::QUEST_PUSH_RESULT,
            member.character_guid,
            result.to_string(),
        );
    }
    Ok(())
}

fn evaluate_share(
    ctx: &ReducerContext,
    sender: &crate::WorldEntity,
    tmpl: &QuestTemplate,
    member_guid: u64,
) -> u8 {
    let member_entity = ctx.db.game_world_entity().guid().find(member_guid);
    let (same_map_instance, in_range) = match &member_entity {
        Some(e) => {
            let (dx, dy, dz) = (e.x - sender.x, e.y - sender.y, e.z - sender.z);
            (
                e.map_id == sender.map_id && e.instance_id == sender.instance_id,
                dx * dx + dy * dy + dz * dz <= PARTY_SHARE_RANGE_SQ,
            )
        }
        None => (false, false),
    };
    let existing = character_quest_row(ctx, member_guid, tmpl.entry);
    let member_active = existing.as_ref().is_some_and(|q| !q.rewarded && !q.failed);
    let member_finished = existing.as_ref().is_some_and(|q| q.rewarded) && !tmpl.repeatable;
    let log_full = quest_slots_used(ctx, member_guid) >= MAX_QUEST_LOG_SIZE;
    let gates_ok = member_entity
        .as_ref()
        .is_some_and(|e| accept_gates(ctx, e, tmpl).is_ok());
    lyracore_shared::quest::share_result(
        member_entity.is_some(),
        same_map_instance,
        in_range,
        member_active,
        member_finished,
        log_full,
        gates_ok,
    )
}

/// Every un-rewarded row, failed ones included, because the client log shows them.
fn quest_slots_used(ctx: &ReducerContext, character_guid: u64) -> usize {
    ctx.db
        .game_character_quest()
        .by_character()
        .filter(&character_guid)
        .filter(|q| !q.rewarded)
        .count()
}

#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn debug_force_expire(
    ctx: &ReducerContext,
    character_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    let mut cq = character_quest_row(ctx, character_guid, quest_entry)
        .ok_or_else(|| "you are not on that quest".to_string())?;
    if cq.rewarded || cq.failed {
        return Err("quest is already terminal".to_string());
    }
    cq.failed = true;
    cq.deadline_micros = 0;
    cq.update(ctx);
    Ok(())
}

pub(crate) fn apply_abandon_quest(
    ctx: &ReducerContext,
    player_guid: u64,
    quest_entry: u32,
) -> Result<(), String> {
    let cq = character_quest_row(ctx, player_guid, quest_entry)
        .ok_or_else(|| "you are not on that quest".to_string())?;
    if cq.rewarded {
        return Err("cannot abandon a completed quest".to_string());
    }
    if let Some(tmpl) = ctx.db.game_quest_template().entry().find(quest_entry) {
        if tmpl.src_item != 0 {
            let available = crate::items::item_count(ctx, player_guid, tmpl.src_item);
            let count = available.min(tmpl.src_item_count.max(1));
            crate::items::remove_items(ctx, player_guid, tmpl.src_item, count)?;
        }
    }
    crate::quest::CharacterQuest::delete(ctx, cq.id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        eventai_credit_recipient_set, is_expired, pick_choice_reward, AreaTriggerRoute,
        AreaTriggerVolume, EventAiQuestCreditContext, QuestCreditRecipientPolicy,
        QUEST_MAX_LEVEL_PAYOUT,
    };

    fn quest_credit_context() -> EventAiQuestCreditContext {
        EventAiQuestCreditContext {
            source_creature_guid: 1,
            source_x: 0.0,
            source_y: 0.0,
            source_map_id: 0,
            source_instance_id: 0,
            selected_character: Some(10),
            invoker_beneficiary: Some(20),
        }
    }

    #[test]
    fn eventai_credit_recipient_policies_keep_only_authority_eligible_characters() {
        let context = quest_credit_context();
        let selected = eventai_credit_recipient_set(
            QuestCreditRecipientPolicy::SelectedCharacter,
            &context,
            |_| true,
            |_| unreachable!("selected credit does not resolve a group"),
        )
        .unwrap();
        assert_eq!(selected, vec![10]);

        let beneficiary = eventai_credit_recipient_set(
            QuestCreditRecipientPolicy::InvokerBeneficiary,
            &context,
            |_| true,
            |_| unreachable!("beneficiary credit does not resolve a group"),
        )
        .unwrap();
        assert_eq!(beneficiary, vec![20]);

        let eligible_group = eventai_credit_recipient_set(
            QuestCreditRecipientPolicy::EligibleGroup,
            &context,
            |_| true,
            |root| {
                assert_eq!(root, 10);
                vec![10, 30, 30]
            },
        )
        .unwrap();
        assert_eq!(eligible_group, vec![10, 30]);

        assert!(eventai_credit_recipient_set(
            QuestCreditRecipientPolicy::TapGroup,
            &context,
            |_| true,
            |_| unreachable!("tap credit resolves its durable snapshot"),
        )
        .is_err());
    }

    #[test]
    fn eventai_credit_recipient_loss_refuses_without_a_substitute() {
        let mut context = quest_credit_context();
        context.selected_character = None;
        assert!(eventai_credit_recipient_set(
            QuestCreditRecipientPolicy::SelectedCharacter,
            &context,
            |_| true,
            |_| unreachable!("a missing selected Character cannot resolve a group"),
        )
        .is_err());

        context.selected_character = Some(10);
        assert!(eventai_credit_recipient_set(
            QuestCreditRecipientPolicy::EligibleGroup,
            &context,
            |guid| guid != 30,
            |_| vec![10, 30],
        )
        .is_err());
    }

    #[test]
    fn pick_choice_reward_grants_the_chosen_index() {
        let choices = [(0u8, 100u32, 1u32), (1, 200, 1), (2, 300, 5)];
        assert_eq!(pick_choice_reward(&choices, 0, 7).unwrap(), Some((100, 1)));
        assert_eq!(pick_choice_reward(&choices, 1, 7).unwrap(), Some((200, 1)));
        assert_eq!(pick_choice_reward(&choices, 2, 7).unwrap(), Some((300, 5)));
        let got = pick_choice_reward(&choices, 1, 7).unwrap();
        assert_ne!(got, Some((100, 1)));
        assert_ne!(got, Some((300, 5)));
    }

    #[test]
    fn pick_choice_reward_no_choices_is_noop() {
        assert_eq!(pick_choice_reward(&[], 0, 7).unwrap(), None);
        assert_eq!(pick_choice_reward(&[], 99, 7).unwrap(), None);
    }

    #[test]
    fn pick_choice_reward_out_of_range_is_rejected() {
        let choices = [(0u8, 100u32, 1u32), (1, 200, 1)];
        let err = pick_choice_reward(&choices, 2, 42).unwrap_err();
        assert!(err.contains("invalid reward choice 2"), "err was: {err}");
        assert!(err.contains("quest 42"), "err names the quest: {err}");
        assert!(pick_choice_reward(&choices, u32::MAX, 42).is_err());
    }

    #[test]
    fn untimed_deadline_never_expires() {
        assert!(!is_expired(0, 0, false, false));
        assert!(!is_expired(i64::MAX, 0, false, false));
    }

    #[test]
    fn is_expired_is_inclusive_at_the_deadline_instant() {
        assert!(is_expired(1_000, 1_000, false, false));
        assert!(!is_expired(999, 1_000, false, false));
        assert!(is_expired(1_001, 1_000, false, false));
    }

    #[test]
    fn is_expired_never_fires_on_a_terminal_row() {
        assert!(!is_expired(5_000, 1_000, true, false));
        assert!(!is_expired(5_000, 1_000, false, true));
    }

    #[test]
    fn quest_max_level_payout_matches_the_vanilla_cap() {
        assert_eq!(QUEST_MAX_LEVEL_PAYOUT, 60);
    }

    #[test]
    fn areatrigger_sphere_includes_its_three_dimensional_boundary() {
        let route = AreaTriggerRoute {
            source_map: 0,
            source_x: 10.0,
            source_y: 20.0,
            source_z: 30.0,
            target_map: 36,
            target_x: 0.0,
            target_y: 0.0,
            target_z: 0.0,
            target_o: 0.0,
            volume: AreaTriggerVolume {
                radius: 5.0,
                length: 0.0,
                width: 0.0,
                height: 0.0,
                yaw: 0.0,
            },
        };
        assert!(route.contains(13.0, 24.0, 30.0));
        assert!(!route.contains(13.1, 24.0, 30.0));
        assert!(!route.contains(10.0, 20.0, 35.1));
    }

    #[test]
    fn finite_trigger_coordinates_cannot_overflow_the_range_gate() {
        let volume = AreaTriggerVolume {
            radius: f32::MAX,
            length: 0.0,
            width: 0.0,
            height: 0.0,
            yaw: 0.0,
        };
        assert!(volume.contains([0.0; 3], [f32::MAX, 0.0, 0.0]));
        assert!(!volume.contains([-f32::MAX, 0.0, 0.0], [f32::MAX, 0.0, 0.0]));
        assert!(!volume.contains([0.0; 3], [f32::NAN, 0.0, 0.0]));
    }

    #[test]
    fn areatrigger_box_applies_yaw_before_its_half_extents() {
        let route = AreaTriggerRoute {
            source_map: 0,
            source_x: 10.0,
            source_y: 20.0,
            source_z: 30.0,
            target_map: 36,
            target_x: 0.0,
            target_y: 0.0,
            target_z: 0.0,
            target_o: 0.0,
            volume: AreaTriggerVolume {
                radius: 0.0,
                length: 8.0,
                width: 2.0,
                height: 4.0,
                yaw: std::f32::consts::FRAC_PI_2,
            },
        };
        assert!(route.contains(10.5, 24.0, 32.0));
        assert!(!route.contains(11.1, 24.0, 30.0));
        assert!(!route.contains(10.0, 20.0, 32.1));
    }

    #[test]
    fn areatrigger_route_refuses_non_finite_coordinates_and_invalid_shapes() {
        let mut route = AreaTriggerRoute {
            source_map: 0,
            source_x: 10.0,
            source_y: 20.0,
            source_z: 30.0,
            target_map: 36,
            target_x: 1.0,
            target_y: 2.0,
            target_z: 3.0,
            target_o: 0.0,
            volume: AreaTriggerVolume {
                radius: 5.0,
                length: 0.0,
                width: 0.0,
                height: 0.0,
                yaw: 0.0,
            },
        };
        assert!(route.valid());
        route.volume.radius = f32::INFINITY;
        assert!(!route.valid());
        route.volume.radius = 0.0;
        assert!(!route.valid());
        route.volume.length = 8.0;
        route.volume.width = 2.0;
        route.volume.height = 4.0;
        assert!(route.valid());
        route.source_x = f32::NAN;
        assert!(!route.valid());
        route.source_x = 10.0;
        route.target_z = f32::INFINITY;
        assert!(!route.valid());
    }
}
