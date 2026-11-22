use spacetimedb::{table, ReducerContext, Table};

use crate::actor::{ActionRefusal, ActionRefusalKind};
use crate::character::game_character;
use crate::WorldEntity;
use crate::{
    game_character_quest, game_gameobject, game_item_instance, game_item_template,
    game_quest_objective, game_world_entity,
};
use crate::{game_corpse_loot_eligible, game_loot_roll, game_loot_roll_vote};
use lyracore_shared::loot_roll::event_kind as roll_event_kind;

pub(crate) mod rolls;

pub(crate) use rolls::{
    apply_group_loot_rules, apply_master_give, cast_vote_on, force_resolve_rolls_for_disband,
    sweep_loot_rolls,
};

pub(crate) mod tag;
pub(crate) use tag::{
    corpse_access, corpse_eligible_for_access, corpse_eligible_recipients, death_entitlement,
};

mod take;
pub(crate) use take::{apply_take_loot, request_take_loot, take_item};

mod item;
pub(crate) use item::{
    forget_item_loot, has_generated_loot, open_item, release_item, OpenItemRefusal,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LootRefusal {
    LootTagIneligible,
    OutOfRange,
    NoLootSource,
    LooterUnavailable,
    LooterDead,
    NothingToLoot,
    ItemUnavailable,
    InventoryFull,
    UniqueItemHeld,
    /// Item storage refused for a reason other than space or a unique item limit.
    StorageRefused,
    RollUnavailable,
    NotMasterLooter,
    RecipientUnavailable,
    RecipientInventoryFull,
}

impl LootRefusal {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::LootTagIneligible => "loot:loot_tag_ineligible",
            Self::OutOfRange => "loot:out_of_range",
            Self::NoLootSource => "loot:no_loot_source",
            Self::LooterUnavailable => "loot:looter_unavailable",
            Self::LooterDead => "loot:looter_dead",
            Self::NothingToLoot => "loot:nothing_to_loot",
            Self::ItemUnavailable => "loot:item_unavailable",
            Self::InventoryFull => "loot:inventory_full",
            Self::UniqueItemHeld => "loot:unique_item_held",
            Self::StorageRefused => "loot:storage_refused",
            Self::RollUnavailable => "loot:roll_unavailable",
            Self::NotMasterLooter => "loot:not_master_looter",
            Self::RecipientUnavailable => "loot:recipient_unavailable",
            Self::RecipientInventoryFull => "loot:recipient_inventory_full",
        }
    }
}

impl From<LootRefusal> for String {
    fn from(refusal: LootRefusal) -> Self {
        refusal.as_tag().to_string()
    }
}

impl From<LootRefusal> for ActionRefusal {
    fn from(refusal: LootRefusal) -> Self {
        let kind = match refusal {
            LootRefusal::LooterUnavailable => ActionRefusalKind::MissingActor,
            LootRefusal::LooterDead => ActionRefusalKind::DeadActor,
            LootRefusal::NoLootSource | LootRefusal::NothingToLoot => {
                ActionRefusalKind::MissingTarget
            }
            LootRefusal::OutOfRange => ActionRefusalKind::OutOfRange,
            LootRefusal::InventoryFull => ActionRefusalKind::InventoryFull,
            LootRefusal::UniqueItemHeld => ActionRefusalKind::DuplicateItem,
            LootRefusal::StorageRefused => ActionRefusalKind::Other,
            LootRefusal::LootTagIneligible
            | LootRefusal::ItemUnavailable
            | LootRefusal::RollUnavailable
            | LootRefusal::NotMasterLooter
            | LootRefusal::RecipientUnavailable
            | LootRefusal::RecipientInventoryFull => ActionRefusalKind::CannotAct,
        };
        ActionRefusal::new(kind, refusal.as_tag())
    }
}

#[table(accessor = game_corpse_loot, public, index(accessor = by_corpse, btree(columns = [corpse_guid])))]
pub struct CorpseLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub corpse_guid: u64,
    pub slot: u8,
    pub item_entry: u32,
    pub count: u32,
    #[default(false)]
    pub quest_only: bool,
    // Nonzero reserves the row for one Character, including a Loot Roll winner with full bags.
    #[default(0u64)]
    pub reserved_for: u64,
    #[default(0u64)]
    pub designated_looter_guid: u64,
    // Even the master must use the explicit assignment path.
    #[default(false)]
    pub master_only: bool,
    // A withheld row belongs to a live Loot Roll.
    #[default(false)]
    pub withheld: bool,
    #[default(0)]
    pub random_property_id: u32,
}

#[table(accessor = game_creature_loot, public, index(accessor = by_creature, btree(columns = [creature_entry])))]
pub struct CreatureLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    #[default(false)]
    pub quest_only: bool,
    #[default(0)]
    pub min_count: u32,
}

// vmangos ItemPrototype.h: each looter gets their own copy of such an item.
const ITEM_FLAG_PARTY_LOOT: u32 = 0x800;

pub(crate) fn loot_drops(chance_bp: u32, roll_0_9999: u32) -> bool {
    roll_0_9999 < chance_bp
}

pub(crate) fn group_pick(member_bp: &[u32], roll_0_9999: u32) -> Option<usize> {
    let mut cum = 0u32;
    for (i, &bp) in member_bp.iter().enumerate() {
        cum = cum.saturating_add(bp);
        if roll_0_9999 < cum {
            return Some(i);
        }
    }
    None
}

pub(crate) fn rank_money_multiplier_permille(rank: u8) -> u32 {
    match rank {
        0 => 1000, // Normal.
        1 => 1500, // Elite.
        2 => 2000, // Rare elite.
        3 => 3000, // Boss.
        4 => 2000, // Rare.
        _ => 1000,
    }
}

pub(crate) fn scale_money_for_rank(money: u32, rank: u8) -> u32 {
    let permille = rank_money_multiplier_permille(rank);
    ((money as u64 * permille as u64) / 1000) as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LootRow {
    pub(crate) item_entry: u32,
    // Basis points; a nonzero group_id uses it as a weight within the group.
    pub(crate) chance_bp: u32,
    // Zero keeps the stack fixed at `count`, as rows stored before stack rolls do.
    pub(crate) min_count: u32,
    pub(crate) count: u32,
    // Zero is independent; nonzero selects a pool with at most one drop.
    pub(crate) group_id: u32,
    pub(crate) quest_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LootDrop {
    pub(crate) item_entry: u32,
    pub(crate) count: u32,
    pub(crate) quest_only: bool,
}

impl LootRow {
    fn stack_range(self) -> (u32, u32) {
        let maximum = self.count.max(1);
        match self.min_count {
            0 => (maximum, maximum),
            minimum => (minimum.min(maximum), maximum),
        }
    }

    // vmangos LootItem draws urand(mincountOrRef, maxcount) as the row drops.
    fn drop(self, ctx: &ReducerContext, source_guid: u64) -> LootDrop {
        let (minimum, maximum) = self.stack_range();
        let count = if minimum == maximum {
            minimum
        } else {
            minimum + source_draw(ctx, source_guid) % (maximum - minimum + 1)
        };
        LootDrop {
            item_entry: self.item_entry,
            count,
            quest_only: self.quest_only,
        }
    }
}

impl From<CreatureLoot> for LootRow {
    fn from(row: CreatureLoot) -> Self {
        Self {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            min_count: row.min_count,
            count: row.count,
            group_id: row.group_id,
            quest_only: row.quest_only,
        }
    }
}

impl From<GamePickpocketLoot> for LootRow {
    fn from(row: GamePickpocketLoot) -> Self {
        Self {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            min_count: row.min_count,
            count: row.count,
            group_id: row.group_id,
            quest_only: row.quest_only,
        }
    }
}

impl From<GameObjectLoot> for LootRow {
    fn from(row: GameObjectLoot) -> Self {
        Self {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            min_count: row.min_count,
            count: row.count,
            group_id: row.group_id,
            quest_only: row.quest_only,
        }
    }
}

impl From<GameItemLoot> for LootRow {
    fn from(row: GameItemLoot) -> Self {
        Self {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            min_count: row.min_count,
            count: row.count,
            group_id: row.group_id,
            quest_only: row.quest_only,
        }
    }
}

impl From<GameSkinningLoot> for LootRow {
    fn from(row: GameSkinningLoot) -> Self {
        Self {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            min_count: row.min_count,
            count: row.count,
            group_id: row.group_id,
            quest_only: false,
        }
    }
}

impl From<GameFishingLoot> for LootRow {
    fn from(row: GameFishingLoot) -> Self {
        Self {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            min_count: row.min_count,
            count: row.count,
            group_id: row.group_id,
            quest_only: false,
        }
    }
}

fn source_draw(ctx: &ReducerContext, source_guid: u64) -> u32 {
    crate::conformance_rolls::draw(
        ctx,
        crate::conformance_rolls::RollStream::Target(source_guid),
    )
}

/// Draws on `source_guid`'s stream each ungrouped row's chance, then each group's pick; a winner
/// draws its stack next.
pub(crate) fn roll_loot_rows(
    ctx: &ReducerContext,
    source_guid: u64,
    rows: impl IntoIterator<Item = LootRow>,
) -> Vec<LootDrop> {
    let mut winners = Vec::new();
    let mut grouped: std::collections::BTreeMap<u32, Vec<LootRow>> =
        std::collections::BTreeMap::new();
    for row in rows {
        if row.group_id == 0 {
            if loot_drops(row.chance_bp, source_draw(ctx, source_guid) % 10000) {
                winners.push(row.drop(ctx, source_guid));
            }
        } else {
            grouped.entry(row.group_id).or_default().push(row);
        }
    }
    for members in grouped.into_values() {
        let weights: Vec<u32> = members.iter().map(|row| row.chance_bp).collect();
        if let Some(i) = group_pick(&weights, source_draw(ctx, source_guid) % 10000) {
            winners.push(members[i].drop(ctx, source_guid));
        }
    }
    winners
}

// vmangos checks the quest need again at take, even for a looter's own party-loot copy.
fn quest_take_allowed(reserved_for: u64, taker_guid: u64, taker_needs_item: bool) -> bool {
    taker_needs_item && (reserved_for == 0 || reserved_for == taker_guid)
}

fn group_loot_take_allowed(
    withheld: bool,
    reserved_for: u64,
    master_only: bool,
    designated_looter_guid: u64,
    taker_guid: u64,
) -> bool {
    if withheld {
        return false;
    }
    if reserved_for != 0 {
        return reserved_for == taker_guid;
    }
    !master_only && (designated_looter_guid == 0 || designated_looter_guid == taker_guid)
}

impl CorpseLoot {
    pub(crate) fn takeable_by(&self, ctx: &ReducerContext, taker_guid: u64) -> bool {
        if self.quest_only {
            quest_take_allowed(
                self.reserved_for,
                taker_guid,
                killer_needs_item(ctx, Some(taker_guid), self.item_entry),
            )
        } else {
            group_loot_take_allowed(
                self.withheld,
                self.reserved_for,
                self.master_only,
                self.designated_looter_guid,
                taker_guid,
            )
        }
    }
}

pub(crate) fn killer_needs_item(ctx: &ReducerContext, killer: Option<u64>, item: u32) -> bool {
    let Some(killer_guid) = killer else {
        return false;
    };
    let in_raid = crate::group::in_raid(ctx, killer_guid);
    let active: Vec<u32> = ctx
        .db
        .game_character_quest()
        .by_character()
        .filter(&killer_guid)
        .filter(|quest| quest.is_active())
        .map(|quest| quest.quest_entry)
        .collect();
    let objectives = active.iter().flat_map(|quest_entry| {
        let quest_type = crate::quest::quest_type_of(ctx, *quest_entry);
        ctx.db
            .game_quest_objective()
            .by_quest()
            .filter(quest_entry)
            .map(move |objective| QuestItemObjective {
                quest_entry: objective.quest_entry,
                quest_type,
                kind: objective.kind,
                target_entry: objective.target_entry,
                required_count: objective.required_count,
            })
    });
    needs_quest_item(
        in_raid,
        &active,
        objectives,
        item,
        crate::items::item_count(ctx, killer_guid, item),
    )
}

#[derive(Clone, Copy)]
struct QuestItemObjective {
    quest_entry: u32,
    quest_type: u32,
    kind: u8,
    target_entry: u32,
    required_count: u32,
}

fn needs_quest_item(
    in_raid: bool,
    active_quests: &[u32],
    objectives: impl IntoIterator<Item = QuestItemObjective>,
    item: u32,
    held: u32,
) -> bool {
    objectives.into_iter().any(|objective| {
        objective.kind == crate::quest::objective_kind::COLLECT_ITEM
            && active_quests.contains(&objective.quest_entry)
            && objective.target_entry == item
            && held < objective.required_count.max(1)
            && lyracore_shared::quest::quest_progresses_for_raid(in_raid, objective.quest_type)
    })
}

// Purge pickpocket residue before death loot reuses the same guid and slot numbers.
pub(crate) fn purge_corpse_residue(ctx: &ReducerContext, guid: u64) {
    let loot = ctx.db.game_corpse_loot();
    let stale_loot: Vec<u64> = loot.by_corpse().filter(&guid).map(|l| l.id).collect();
    for id in stale_loot {
        loot.id().delete(id);
    }
    let eligible = ctx.db.game_corpse_loot_eligible();
    let stale_eligible: Vec<u64> = eligible.by_corpse().filter(&guid).map(|e| e.id).collect();
    for id in stale_eligible {
        eligible.id().delete(id);
    }
}

pub(crate) fn reap_corpse_loot_family(ctx: &ReducerContext, corpse_guid: u64) {
    ctx.db.game_loot_money().source_guid().delete(corpse_guid);
    let loot = ctx.db.game_corpse_loot();
    let stale: Vec<u64> = loot
        .by_corpse()
        .filter(&corpse_guid)
        .filter(|l| !l.withheld)
        .map(|l| l.id)
        .collect();
    for id in stale {
        loot.id().delete(id);
    }
    let eligible = ctx.db.game_corpse_loot_eligible();
    let stale: Vec<u64> = eligible
        .by_corpse()
        .filter(&corpse_guid)
        .map(|e| e.id)
        .collect();
    for id in stale {
        eligible.id().delete(id);
    }
    let rolls = ctx.db.game_loot_roll();
    let votes = ctx.db.game_loot_roll_vote();
    let stale_rolls: Vec<u64> = rolls
        .by_corpse()
        .filter(&corpse_guid)
        .map(|r| r.id)
        .collect();
    for roll_id in stale_rolls {
        let stale_votes: Vec<u64> = votes.by_roll().filter(&roll_id).map(|v| v.id).collect();
        for id in stale_votes {
            votes.id().delete(id);
        }
        rolls.id().delete(roll_id);
    }
}

/// Each needer in `looters` gets a reserved copy of a party-loot quest drop; every other drop is
/// one shared row.
pub(crate) fn insert_corpse_rows(
    ctx: &ReducerContext,
    corpse_guid: u64,
    winners: Vec<LootDrop>,
    looters: &[u64],
) -> bool {
    let (mut shared, mut reserved) = (Vec::new(), Vec::new());
    for winner in winners {
        let item_entry = winner.item_entry;
        let random_property_id = match crate::items::select_loot_property(ctx, item_entry) {
            Ok(id) => id,
            Err(reason) => {
                spacetimedb::log::error!("loot item {item_entry}: {reason}");
                continue;
            }
        };
        for reserved_for in reservations(ctx, winner, looters) {
            let row = CorpseLoot {
                id: 0,
                corpse_guid,
                slot: 0,
                item_entry,
                count: winner.count,
                quest_only: winner.quest_only,
                reserved_for,
                designated_looter_guid: 0,
                master_only: false,
                withheld: false,
                random_property_id,
            };
            if reserved_for == 0 {
                shared.push(row);
            } else {
                reserved.push(row);
            }
        }
    }
    // Shared rows take the low slots, so every Loot Window lists regular loot without gaps.
    let rows = shared.into_iter().chain(reserved);
    let mut inserted = false;
    for (slot, row) in (0..=u8::MAX).zip(rows) {
        ctx.db.game_corpse_loot().insert(CorpseLoot { slot, ..row });
        inserted = true;
    }
    inserted
}

// vmangos Loot::FillQuestLoot lists a party-loot quest item once for each needer at fill time.
fn reservations(ctx: &ReducerContext, winner: LootDrop, looters: &[u64]) -> Vec<u64> {
    if !winner.quest_only || !is_party_loot(ctx, winner.item_entry) {
        return vec![0];
    }
    let needers: Vec<u64> = looters
        .iter()
        .copied()
        .filter(|&looter| killer_needs_item(ctx, Some(looter), winner.item_entry))
        .collect();
    if needers.is_empty() {
        vec![0]
    } else {
        needers
    }
}

fn is_party_loot(ctx: &ReducerContext, item_entry: u32) -> bool {
    ctx.db
        .game_item_template()
        .entry()
        .find(item_entry)
        .is_some_and(|template| template.item_flags & ITEM_FLAG_PARTY_LOOT != 0)
}

pub(crate) fn roll_creature_loot(
    ctx: &ReducerContext,
    creature_entry: u32,
    corpse_guid: u64,
) -> bool {
    let rows = ctx
        .db
        .game_creature_loot()
        .by_creature()
        .filter(&creature_entry)
        .map(LootRow::from);
    let winners = roll_loot_rows(ctx, corpse_guid, rows);
    let looters = corpse_eligible_recipients(ctx, corpse_guid);
    insert_corpse_rows(ctx, corpse_guid, winners, &looters)
}

// The importer resolves PickpocketLootId to creature_entry before storage.
#[table(accessor = game_pickpocket_loot, public, index(accessor = by_creature, btree(columns = [creature_entry])))]
pub struct GamePickpocketLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    pub quest_only: bool,
    #[default(0)]
    pub min_count: u32,
}

#[table(accessor = game_gameobject_loot, public, index(accessor = by_loot, btree(columns = [loot_id])))]
pub struct GameObjectLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub loot_id: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    pub quest_only: bool,
    #[default(0)]
    pub min_count: u32,
}

// vmangos item_loot_template: what opening a lootable item of `lootable_entry` can give.
#[table(accessor = game_item_loot, public, index(accessor = by_lootable, btree(columns = [lootable_entry])))]
pub struct GameItemLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub lootable_entry: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    pub quest_only: bool,
    #[default(0)]
    pub min_count: u32,
}

/// The copper a chest or an opened item still holds. An item's row marks its loot as rolled.
#[table(accessor = game_loot_money, index(accessor = by_owner, btree(columns = [owner_guid])))]
pub struct LootMoney {
    #[primary_key]
    pub source_guid: u64,
    // The Character holding an opened item; zero for a chest.
    pub owner_guid: u64,
    pub money: u32,
}

crate::outside_table_catalog!(game_loot_money);

crate::character_owned!(delete, fn sweep_delete_game_loot_money(ctx, character_guid) {
    let purses: Vec<u64> = ctx
        .db
        .game_loot_money()
        .by_owner()
        .filter(&character_guid)
        .map(|purse| purse.source_guid)
        .collect();
    for item_guid in purses {
        forget_item_loot(ctx, item_guid);
    }
});
// An opened item keeps its money across a Transfer, so the destination does not roll it again.
crate::character_owned!(transfer, fn sweep_transfer_game_loot_money(ctx, character_guid, io) {
    table = game_loot_money,
    by = by_owner,
    keep_key,
});

#[table(accessor = game_skinning_loot, public, index(accessor = by_skin, btree(columns = [skin_loot_id])))]
pub struct GameSkinningLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub skin_loot_id: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    #[default(0)]
    pub min_count: u32,
}

#[table(accessor = game_fishing_loot, public, index(accessor = by_zone, btree(columns = [zone_id])))]
pub struct GameFishingLoot {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub zone_id: u32,
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    #[default(0)]
    pub min_count: u32,
}

pub(crate) fn roll_pickpocket_loot(ctx: &ReducerContext, creature_entry: u32, target_guid: u64) {
    let rows = ctx
        .db
        .game_pickpocket_loot()
        .by_creature()
        .filter(&creature_entry)
        .map(LootRow::from);
    let winners = roll_loot_rows(ctx, target_guid, rows);
    if winners.is_empty() {
        return;
    }
    insert_corpse_rows(ctx, target_guid, winners, &[]);
    refresh_lootable(ctx, target_guid);
}

pub(crate) fn corpse_is_looted(ctx: &ReducerContext, corpse_guid: u64, money: u32) -> bool {
    money == 0
        && ctx
            .db
            .game_corpse_loot()
            .by_corpse()
            .filter(&corpse_guid)
            .next()
            .is_none()
}

pub(crate) fn refused(refusal: LootRefusal, detail: &str) -> String {
    refuse(refusal, detail).as_tag().to_string()
}

pub(crate) fn refuse(refusal: LootRefusal, detail: &str) -> LootRefusal {
    spacetimedb::log::info!("loot refused {}: {detail}", refusal.as_tag());
    refusal
}

pub(crate) enum LootSource {
    Corpse(Box<WorldEntity>),
    GameObject,
    Item,
}

/// A live looter within reach of a GameObject, of a corpse it holds Loot Tag access to, or holding
/// an opened item.
pub(crate) fn reach(
    ctx: &ReducerContext,
    looter_guid: u64,
    source_guid: u64,
) -> Result<(WorldEntity, LootSource), LootRefusal> {
    let entities = ctx.db.game_world_entity();
    let looter = entities
        .guid()
        .find(looter_guid)
        .ok_or_else(|| refuse(LootRefusal::LooterUnavailable, "looter not in world"))?;
    if looter.dead {
        return Err(refuse(
            LootRefusal::LooterDead,
            "dead Characters cannot loot",
        ));
    }
    if lyracore_shared::guid::is_item(source_guid) {
        let held = ctx
            .db
            .game_item_instance()
            .guid()
            .find(source_guid)
            .is_some_and(|item| item.owner_guid == looter_guid);
        if !held || !has_generated_loot(ctx, source_guid) {
            return Err(refuse(LootRefusal::NoLootSource, "no opened item held"));
        }
        return Ok((looter, LootSource::Item));
    }
    let (partition, at, source) = if let Some(corpse) = entities.guid().find(source_guid) {
        if corpse.is_player() || !corpse.dead {
            return Err(refuse(
                LootRefusal::NoLootSource,
                "Loot Source is not a creature corpse",
            ));
        }
        (
            (corpse.map_id, corpse.instance_id),
            [corpse.x, corpse.y, corpse.z],
            LootSource::Corpse(Box::new(corpse)),
        )
    } else if let Some(go) = ctx.db.game_gameobject().guid().find(source_guid) {
        (
            (go.map_id, go.instance_id),
            [go.x, go.y, go.z],
            LootSource::GameObject,
        )
    } else {
        return Err(refuse(LootRefusal::NoLootSource, "no such Loot Source"));
    };
    if partition != (looter.map_id, looter.instance_id) {
        return Err(refuse(
            LootRefusal::OutOfRange,
            "Loot Source is on another map or instance",
        ));
    }
    let [dx, dy, dz] = [at[0] - looter.x, at[1] - looter.y, at[2] - looter.z];
    if dx * dx + dy * dy + dz * dz > crate::npc_interaction::INTERACTION_RANGE_SQ {
        return Err(refuse(
            LootRefusal::OutOfRange,
            "Loot Source is out of reach",
        ));
    }
    if let LootSource::Corpse(_) = source {
        corpse_access(ctx, looter_guid, source_guid).map_err(|refusal| {
            refuse(
                refusal,
                &format!("actor_guid={looter_guid} corpse_guid={source_guid}"),
            )
        })?;
    }
    Ok((looter, source))
}

pub(crate) fn reach_corpse(
    ctx: &ReducerContext,
    looter_guid: u64,
    corpse_guid: u64,
) -> Result<(WorldEntity, WorldEntity), LootRefusal> {
    match reach(ctx, looter_guid, corpse_guid)? {
        (looter, LootSource::Corpse(corpse)) => Ok((looter, *corpse)),
        (_, LootSource::GameObject | LootSource::Item) => Err(refuse(
            LootRefusal::NoLootSource,
            "Loot Source is not a creature corpse",
        )),
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn request_open_creature_corpse(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
) -> Result<(), ActionRefusal> {
    Ok(open_creature_corpse(ctx, actor_guid, corpse_guid)?)
}

pub(crate) fn open_creature_corpse(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
) -> Result<(), LootRefusal> {
    reach_corpse(ctx, actor_guid, corpse_guid).map(drop)
}

pub(crate) fn apply_loot_money(
    ctx: &ReducerContext,
    looter_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    Ok(claim_money(ctx, looter_guid, target_guid)?)
}

pub(crate) fn claim_money(
    ctx: &ReducerContext,
    looter_guid: u64,
    source_guid: u64,
) -> Result<(), LootRefusal> {
    let (looter, source) = reach(ctx, looter_guid, source_guid)?;
    let (amount, recipients) = match source {
        LootSource::Corpse(corpse) => {
            let mut corpse = *corpse;
            if corpse.money == 0 {
                return Err(refuse(LootRefusal::NothingToLoot, "corpse purse is empty"));
            }
            let amount = std::mem::take(&mut corpse.money);
            corpse.update(ctx);
            (amount, corpse_eligible_recipients(ctx, source_guid))
        }
        // vmangos HandleLootMoneyOpcode shares a chest's money with the group members near the looter.
        LootSource::GameObject => (
            take_purse(ctx, source_guid)?,
            nearby_group_members(ctx, &looter),
        ),
        LootSource::Item => (take_purse(ctx, source_guid)?, Vec::new()),
    };
    pay_money(ctx, looter, amount, &recipients);
    refresh_lootable(ctx, source_guid);
    Ok(())
}

fn pay_money(ctx: &ReducerContext, mut looter: WorldEntity, amount: u32, recipients: &[u64]) {
    let looter_guid = looter.guid;
    let shares = split_money(amount, recipients, looter_guid);
    // Credit the looter in this local copy to avoid overwriting a separately updated purse.
    for &(recipient_guid, share) in &shares {
        if share == 0 {
            continue;
        }
        if recipient_guid == looter_guid {
            looter.money = looter.money.saturating_add(share);
        } else {
            credit_purse(ctx, recipient_guid, share);
        }
    }
    looter.update(ctx);

    if money_is_grouped(recipients) {
        for &(recipient_guid, share) in &shares {
            if share == 0 {
                continue;
            }
            crate::group::push_event(
                ctx,
                recipient_guid,
                roll_event_kind::MONEY_SHARE,
                0,
                lyracore_shared::loot_roll::encode_money_share(share),
            );
        }
    }
}

fn nearby_group_members(ctx: &ReducerContext, looter: &WorldEntity) -> Vec<u64> {
    let Some(member) = crate::group::group_of(ctx, looter.guid) else {
        return Vec::new();
    };
    let entities = ctx.db.game_world_entity();
    crate::group::members_of(ctx, member.group_id)
        .into_iter()
        .filter(|member| {
            entities
                .guid()
                .find(member.character_guid)
                .is_some_and(|e| {
                    let [dx, dy, dz] = [e.x - looter.x, e.y - looter.y, e.z - looter.z];
                    (e.map_id, e.instance_id) == (looter.map_id, looter.instance_id)
                        && dx * dx + dy * dy + dz * dz <= crate::group::GROUP_XP_RANGE_SQ
                })
        })
        .map(|member| member.character_guid)
        .collect()
}

pub(crate) fn purse(ctx: &ReducerContext, source_guid: u64) -> u32 {
    ctx.db
        .game_loot_money()
        .source_guid()
        .find(source_guid)
        .map_or(0, |purse| purse.money)
}

fn take_purse(ctx: &ReducerContext, source_guid: u64) -> Result<u32, LootRefusal> {
    let purses = ctx.db.game_loot_money();
    let mut purse = purses
        .source_guid()
        .find(source_guid)
        .filter(|purse| purse.money != 0)
        .ok_or_else(|| refuse(LootRefusal::NothingToLoot, "Loot Source purse is empty"))?;
    let amount = std::mem::take(&mut purse.money);
    purses.source_guid().update(purse);
    Ok(amount)
}

/// Rolls a Loot Source's money and replaces whatever purse it held.
pub(crate) fn stock_purse(
    ctx: &ReducerContext,
    source_guid: u64,
    owner_guid: u64,
    (minimum, maximum): (u32, u32),
) {
    let money = money_in_range(minimum, maximum, || source_draw(ctx, source_guid));
    let purses = ctx.db.game_loot_money();
    purses.source_guid().delete(source_guid);
    purses.insert(LootMoney {
        source_guid,
        owner_guid,
        money,
    });
}

// vmangos Loot::GenerateMoneyLoot at a drop rate of 1; a wide range rolls in steps of 256 copper.
fn money_in_range(minimum: u32, maximum: u32, draw: impl FnOnce() -> u32) -> u32 {
    if maximum == 0 {
        0
    } else if maximum <= minimum {
        maximum
    } else if maximum - minimum < 32_700 {
        minimum + draw() % (maximum - minimum + 1)
    } else {
        let (low, high) = (minimum >> 8, maximum >> 8);
        (low + draw() % (high - low + 1)) << 8
    }
}

// During Transfer, defer the share so an in-flight recipient still gets paid.
fn credit_purse(ctx: &ReducerContext, recipient_guid: u64, share: u32) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(recipient_guid) {
        e.money = e.money.saturating_add(share);
        e.update(ctx);
        return;
    }
    crate::transfer::defer_money_delta(ctx, recipient_guid, share);
    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(recipient_guid) {
        c.money = c.money.saturating_add(share);
        chars.guid().update(c);
    }
}

pub(crate) fn split_money(total: u32, recipients: &[u64], looter_guid: u64) -> Vec<(u64, u32)> {
    if recipients.is_empty() {
        return vec![(looter_guid, total)];
    }
    let n = recipients.len() as u32;
    let share = total / n;
    let remainder = total - share * n;
    let mut looter_found = false;
    let mut shares: Vec<(u64, u32)> = recipients
        .iter()
        .map(|&guid| {
            let mut amt = share;
            if guid == looter_guid {
                amt += remainder;
                looter_found = true;
            }
            (guid, amt)
        })
        .collect();
    if !looter_found && remainder > 0 {
        shares.push((looter_guid, remainder));
    }
    shares
}

pub(crate) fn money_is_grouped(recipients: &[u64]) -> bool {
    recipients.len() > 1
}

pub(crate) fn refresh_lootable(ctx: &ReducerContext, corpse_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut corpse) = entities.guid().find(corpse_guid) else {
        return;
    };
    let has_rows = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&corpse_guid)
        .next()
        .is_some();
    let flag = lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
    let want = if has_rows || corpse.money > 0 {
        flag
    } else {
        0
    };
    if corpse.dynamic_flags & flag != want {
        corpse.dynamic_flags = (corpse.dynamic_flags & !flag) | want;
        corpse.update(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_pick_is_one_weighted_band() {
        let w = [2000u32, 3000, 1000];
        assert_eq!(group_pick(&w, 0), Some(0));
        assert_eq!(group_pick(&w, 1999), Some(0));
        assert_eq!(group_pick(&w, 2000), Some(1));
        assert_eq!(group_pick(&w, 4999), Some(1));
        assert_eq!(group_pick(&w, 5000), Some(2));
        assert_eq!(group_pick(&w, 5999), Some(2));
        assert_eq!(group_pick(&w, 6000), None);
        assert_eq!(group_pick(&w, 9999), None);
        let eq = [2500u32, 2500, 2500, 2500];
        assert!(group_pick(&eq, 9999).is_some());
        assert_eq!(group_pick(&eq, 2500), Some(1));
    }

    #[test]
    fn a_stack_spans_the_row_minimum_to_its_count_and_zero_keeps_it_fixed() {
        let row = |min_count, count| LootRow {
            item_entry: 25,
            chance_bp: 10000,
            min_count,
            count,
            group_id: 0,
            quest_only: false,
        };
        assert_eq!(row(1, 3).stack_range(), (1, 3));
        assert_eq!(row(2, 2).stack_range(), (2, 2));
        assert_eq!(row(0, 3).stack_range(), (3, 3));
        assert_eq!(row(4, 3).stack_range(), (3, 3));
        assert_eq!(row(0, 0).stack_range(), (1, 1));
    }

    #[test]
    fn money_rolls_the_open_range_and_a_closed_range_gives_its_maximum() {
        assert_eq!(money_in_range(0, 0, || unreachable!()), 0);
        assert_eq!(money_in_range(50, 0, || unreachable!()), 0);
        assert_eq!(money_in_range(40, 40, || unreachable!()), 40);
        assert_eq!(money_in_range(90, 40, || unreachable!()), 40);
        assert_eq!(money_in_range(10, 20, || 0), 10);
        assert_eq!(money_in_range(10, 20, || 10), 20);
        assert_eq!(money_in_range(10, 20, || 11), 10);
    }

    #[test]
    fn a_range_of_32700_copper_or_more_rolls_whole_steps_of_256() {
        assert_eq!(money_in_range(0, 32_699, || 32_699), 32_699);
        assert_eq!(money_in_range(0, 32_700, || 0), 0);
        assert_eq!(money_in_range(0, 32_700, || 127), 127 * 256);
        assert_eq!(money_in_range(0, 32_700, || 128), 0);
        assert_eq!(money_in_range(1_000, 100_000, || 0), 768);
    }

    #[test]
    fn rank_money_scales_and_rank0_is_identity() {
        for money in [0u32, 1, 17, 1000, 1_000_000, u32::MAX] {
            assert_eq!(
                scale_money_for_rank(money, 0),
                money,
                "rank-0 money must be identity for {money}"
            );
        }
        assert_eq!(scale_money_for_rank(100, 1), 150);
        assert_eq!(scale_money_for_rank(100, 2), 200);
        assert_eq!(scale_money_for_rank(100, 3), 300);
        assert_eq!(scale_money_for_rank(100, 4), 200);
        assert_eq!(scale_money_for_rank(100, 9), 100);
        assert_eq!(scale_money_for_rank(0, 3), 0);
        assert_eq!(rank_money_multiplier_permille(0), 1000);
        assert!(rank_money_multiplier_permille(1) > rank_money_multiplier_permille(0));
        assert!(rank_money_multiplier_permille(3) > rank_money_multiplier_permille(1));
        assert!(scale_money_for_rank(u32::MAX, 3) >= u32::MAX / 2);
    }

    #[test]
    fn collect_need_respects_raid_type_quantity_and_the_minimum_requirement() {
        let collect = |quest_type, target_entry, required_count| QuestItemObjective {
            quest_entry: 100,
            quest_type,
            kind: crate::quest::objective_kind::COLLECT_ITEM,
            target_entry,
            required_count,
        };
        assert!(needs_quest_item(false, &[100], [collect(0, 55, 10)], 55, 9));
        assert!(!needs_quest_item(
            false,
            &[100],
            [collect(0, 55, 10)],
            55,
            10
        ));
        assert!(!needs_quest_item(true, &[100], [collect(0, 55, 10)], 55, 0));
        assert!(needs_quest_item(true, &[100], [collect(62, 55, 10)], 55, 0));
        assert!(needs_quest_item(false, &[100], [collect(0, 55, 0)], 55, 0));
        assert!(!needs_quest_item(false, &[100], [collect(0, 55, 0)], 55, 1));
        assert!(!needs_quest_item(
            false,
            &[100],
            [collect(0, 56, 10)],
            55,
            0
        ));
        assert!(!needs_quest_item(false, &[], [collect(0, 55, 10)], 55, 0));
        let kill = QuestItemObjective {
            kind: crate::quest::objective_kind::KILL_CREATURE,
            ..collect(0, 55, 10)
        };
        assert!(!needs_quest_item(false, &[100], [kill], 55, 0));
    }

    #[test]
    fn group_loot_take_allowed_gates_withheld_reserved_master_and_designated_rows() {
        assert!(
            !group_loot_take_allowed(true, 0, false, 0, 7),
            "a live roll owns the row"
        );
        assert!(
            !group_loot_take_allowed(true, 7, false, 0, 7),
            "even the future reservee waits for resolution"
        );
        assert!(group_loot_take_allowed(false, 0, false, 0, 7));
        assert!(group_loot_take_allowed(false, 7, false, 0, 7));
        assert!(
            !group_loot_take_allowed(false, 7, false, 0, 8),
            "reserved for someone else"
        );
        assert!(
            !group_loot_take_allowed(false, 7, true, 99, 8),
            "reserved wins over master_only/designated too"
        );
        assert!(
            !group_loot_take_allowed(false, 0, true, 42, 42),
            "master_only blocks even the master's OWN take"
        );
        assert!(!group_loot_take_allowed(false, 0, true, 42, 8));
        assert!(group_loot_take_allowed(false, 0, false, 42, 42));
        assert!(
            !group_loot_take_allowed(false, 0, false, 42, 8),
            "not the designated looter"
        );
    }

    #[test]
    fn split_money_solo_passthrough_credits_the_whole_amount_to_the_looter() {
        assert_eq!(split_money(137, &[], 7), vec![(7, 137)]);
        assert_eq!(split_money(0, &[], 7), vec![(7, 0)]);
        assert_eq!(split_money(u32::MAX, &[], 7), vec![(7, u32::MAX)]);
    }

    #[test]
    fn split_money_even_split_with_no_remainder() {
        assert_eq!(
            split_money(30, &[1, 2, 3], 1),
            vec![(1, 10), (2, 10), (3, 10)]
        );
        assert_eq!(
            split_money(100, &[10, 20, 30, 40], 10),
            vec![(10, 25), (20, 25), (30, 25), (40, 25)]
        );
    }

    #[test]
    fn split_money_remainder_goes_to_the_looter() {
        assert_eq!(
            split_money(100, &[1, 2, 3], 2),
            vec![(1, 33), (2, 34), (3, 33)]
        );
        assert_eq!(
            split_money(100, &[1, 2, 3], 1),
            vec![(1, 34), (2, 33), (3, 33)]
        );
        assert_eq!(
            split_money(100, &[1, 2, 3], 3),
            vec![(1, 33), (2, 33), (3, 34)]
        );
        assert_eq!(split_money(7, &[5, 9], 9), vec![(5, 3), (9, 4)]);
    }

    #[test]
    fn split_money_looter_outside_recipients_gets_an_extra_remainder_entry() {
        assert_eq!(
            split_money(100, &[10, 20, 30], 99),
            vec![(10, 33), (20, 33), (30, 33), (99, 1)]
        );
        assert_eq!(
            split_money(90, &[10, 20, 30], 99),
            vec![(10, 30), (20, 30), (30, 30)]
        );
    }

    #[test]
    fn split_money_single_recipient_degrades_to_whole_amount() {
        assert_eq!(split_money(50, &[7], 7), vec![(7, 50)]);
    }
}
