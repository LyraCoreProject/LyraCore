use crate::actor::{ActionRefusal, ActionRefusalKind};
use lyracore_shared::quest::AcceptanceRefusal;
use std::collections::BTreeSet;

use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::game_area_trigger;
use crate::game_gameobject;
use crate::game_item_instance;
use crate::game_item_template;
use crate::game_world_entity;

pub use lyracore_shared::quest::objective_kind;

pub(crate) use lyracore_shared::quest::allowed_in_raid;

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

#[derive(Debug, PartialEq, Eq)]
struct QuestPayout {
    money: u32,
    xp: Option<u32>,
}

fn quest_payout(level: u32, xp: u32) -> QuestPayout {
    if level >= QUEST_MAX_LEVEL_PAYOUT {
        QuestPayout {
            money: lyracore_shared::quest::max_level_money_reward(xp),
            xp: None,
        }
    } else {
        QuestPayout {
            money: 0,
            xp: Some(xp),
        }
    }
}

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
    acceptance_gate(ctx, player, tmpl).map_err(|refusal| refusal.text(tmpl.min_level))
}

fn acceptance_gate(
    ctx: &ReducerContext,
    player: &crate::WorldEntity,
    tmpl: &QuestTemplate,
) -> Result<Option<CharacterQuest>, AcceptanceRefusal> {
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
    )?;
    Ok(existing)
}

pub(crate) enum AcceptQuestRefusal {
    Acceptance {
        refusal: AcceptanceRefusal,
        min_level: u32,
    },
    LogFull,
    Refused(ActionRefusal),
}

impl From<ActionRefusal> for AcceptQuestRefusal {
    fn from(refusal: ActionRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<String> for AcceptQuestRefusal {
    fn from(detail: String) -> Self {
        Self::Refused(detail.into())
    }
}

impl From<AcceptQuestRefusal> for ActionRefusal {
    fn from(refusal: AcceptQuestRefusal) -> Self {
        match refusal {
            AcceptQuestRefusal::Acceptance { refusal, min_level } => refusal.text(min_level).into(),
            AcceptQuestRefusal::LogFull => lyracore_shared::quest::QUEST_LOG_FULL_TEXT
                .to_string()
                .into(),
            AcceptQuestRefusal::Refused(refusal) => refusal,
        }
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
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
    accept_offered_quest(ctx, player_guid, giver_guid, quest_entry).map_err(Into::into)
}

pub(crate) fn accept_offered_quest(
    ctx: &ReducerContext,
    player_guid: u64,
    giver_guid: u64,
    quest_entry: u32,
) -> Result<(), AcceptQuestRefusal> {
    let player = crate::helpers::live_entity(ctx, player_guid)
        .map_err(|_| ActionRefusal::new(ActionRefusalKind::MissingActor, "player not in world"))?;
    if player.dead {
        return Err(ActionRefusal::new(
            ActionRefusalKind::DeadActor,
            "dead players cannot accept quests",
        )
        .into());
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
    let existing =
        acceptance_gate(ctx, &player, &tmpl).map_err(|refusal| AcceptQuestRefusal::Acceptance {
            refusal,
            min_level: tmpl.min_level,
        })?;
    if existing.as_ref().is_none_or(|row| row.rewarded)
        && quest_slots_used(ctx, player_guid) >= MAX_QUEST_LOG_SIZE
    {
        return Err(AcceptQuestRefusal::LogFull);
    }
    let deadline_micros = if tmpl.limit_time > 0 {
        ctx.timestamp.to_micros_since_unix_epoch() + (tmpl.limit_time as i64) * 1_000_000
    } else {
        0
    };
    Ok(apply_accept_effects(
        ctx,
        player_guid,
        player.owner_identity,
        quest_entry,
        &tmpl,
        existing,
        deadline_micros,
    )?)
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
    let payout = quest_payout(player.level, crate::xp::rated_xp(ctx, base_xp));
    player.money = player.money.saturating_add(payout.money);
    // Packages can catch a Refusal; resolve every item Gate before the infallible quest effects.
    crate::items::exchange_items(ctx, player_guid, &collect, &rewards)?;
    if let Some(xp) = payout.xp {
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
) -> Result<(), crate::Refusal> {
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
) -> Result<(), crate::Refusal> {
    if !character_quest_row(ctx, sender_guid, quest_entry).is_some_and(|q| !q.rewarded && !q.failed)
    {
        return Err("you are not on that quest".into());
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
) -> Result<(), crate::Refusal> {
    let cq = character_quest_row(ctx, player_guid, quest_entry)
        .ok_or_else(|| "you are not on that quest".to_string())?;
    if cq.rewarded {
        return Err("cannot abandon a completed quest".into());
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

#[cfg(feature = "debug_reducers")]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::{
        eventai_credit_recipient_set, is_expired, pick_choice_reward, quest_payout,
        AreaTriggerRoute, AreaTriggerVolume, EventAiQuestCreditContext, QuestCreditRecipientPolicy,
        QuestPayout,
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
    fn a_level_59_turn_in_grants_the_quest_xp() {
        assert_eq!(
            quest_payout(59, 1_450),
            QuestPayout {
                money: 0,
                xp: Some(1_450)
            }
        );
    }

    #[test]
    fn a_level_60_turn_in_pays_six_copper_per_xp_instead() {
        assert_eq!(
            quest_payout(60, 1_450),
            QuestPayout {
                money: 8_700,
                xp: None
            }
        );
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
