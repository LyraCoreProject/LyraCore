use crate::actor::{ActionRefusal, ActionRefusalKind};
use spacetimedb::{Identity, ReducerContext};

use lyracore_package_delta::packed_race_class_id;
use lyracore_shared::constants::starter_item;
use lyracore_shared::item::ItemRefusal;

use super::{next_item_guid, refuse, select_property};
use crate::game_character;
use crate::game_corpse_loot;
use crate::game_gameobject;
use crate::game_spell_effect;
use crate::game_start_item;
use crate::game_world_entity;

use super::inventory::{first_free_backpack_slot, first_free_bag_slot, is_carried_slot};
use super::rules::{
    binds_on_grant, equip_slot, meets_required_level, resolve_equip_slot, template_stat, EquipStat,
};
use super::tables::{
    game_item_instance, game_item_template, item_in_slot, ItemInstance, ItemTemplate,
};

pub(crate) fn grant_starter_item(
    ctx: &ReducerContext,
    owner_guid: u64,
    owner_identity: Identity,
) -> Result<(), String> {
    let instances = ctx.db.game_item_instance();
    if instances
        .by_owner_guid()
        .filter(&owner_guid)
        .next()
        .is_some()
    {
        return Ok(());
    }
    let grant_one = |entry: u32, slot: u8, stack: u32| -> Result<(), String> {
        let Some(tmpl) = ctx.db.game_item_template().entry().find(entry) else {
            return Ok(());
        };
        let Ok(random_property_id) = select_property(ctx, &tmpl) else {
            return Ok(());
        };
        crate::ItemInstance::insert(
            ctx,
            ItemInstance {
                guid: next_item_guid(ctx)?,
                entry: tmpl.entry,
                owner_identity,
                owner_guid,
                slot,
                stack_count: stack,
                durability: tmpl.max_durability,
                created_at: crate::time::now(ctx),
                enchant_id: 0,
                soulbound: binds_on_grant(tmpl.bonding),
                random_property_id,
                item_text_id: 0,
            },
        );
        Ok(())
    };
    grant_one(
        starter_item::HEARTHSTONE_ENTRY,
        starter_item::HEARTHSTONE_SLOT,
        1,
    )?;

    let race_class: u16 = ctx
        .db
        .game_character()
        .guid()
        .find(owner_guid)
        .map(|c| packed_race_class_id(c.race, c.class) as u16)
        .unwrap_or(0);
    let outfit: Vec<u32> = ctx
        .db
        .game_start_item()
        .by_race_class()
        .filter(&race_class)
        .map(|s| s.item_entry)
        .collect();
    if !outfit.is_empty() {
        let mut occupied: std::collections::HashSet<u8> = std::collections::HashSet::new();
        let mut backpack = starter_item::BACKPACK_SLOT_0;
        for entry in outfit {
            // CharStartOutfit includes the Hearthstone already granted above.
            if entry == starter_item::HEARTHSTONE_ENTRY {
                continue;
            }
            let Some(tmpl) = ctx.db.game_item_template().entry().find(entry) else {
                continue;
            };
            // CharStartOutfit omits counts; vanilla starting ammunition is 200 rounds.
            const CLASS_AMMO: u8 = 6;
            let count = if tmpl.class == CLASS_AMMO { 200 } else { 1 };
            match resolve_equip_slot(tmpl.inventory_type, false, |s| occupied.contains(&s)) {
                Some(eq) => {
                    occupied.insert(eq);
                    grant_one(entry, eq, count)?;
                }
                None => {
                    grant_one(entry, backpack, count)?;
                    backpack += 1;
                }
            }
        }
        return Ok(());
    }

    grant_one(starter_item::ENTRY, starter_item::MAINHAND_SLOT, 1)?;
    grant_one(51, starter_item::BACKPACK_SLOT_0, 1)?;
    grant_one(52, starter_item::BACKPACK_SLOT_0 + 1, 5)?;
    Ok(())
}

pub(crate) fn grant_item(
    ctx: &ReducerContext,
    player_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    grant_item_property(ctx, player_guid, item_entry, count, None)
}

pub(crate) fn grant_item_property(
    ctx: &ReducerContext,
    player_guid: u64,
    item_entry: u32,
    count: u32,
    property: Option<u32>,
) -> Result<(), String> {
    request_grant_item(ctx, player_guid, item_entry, count, property).map_err(Into::into)
}

pub(crate) fn request_grant_item(
    ctx: &ReducerContext,
    player_guid: u64,
    item_entry: u32,
    count: u32,
    property: Option<u32>,
) -> Result<(), ActionRefusal> {
    let player = crate::helpers::live_entity(ctx, player_guid).map_err(|_| {
        ActionRefusal::new(
            crate::actor::ActionRefusalKind::MissingActor,
            "player not in world",
        )
    })?;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(item_entry)
        .ok_or_else(|| {
            ActionRefusal::new(
                crate::actor::ActionRefusalKind::MissingResource,
                format!("no such item {item_entry}"),
            )
        })?;
    store_item_typed(
        ctx,
        player_guid,
        player.owner_identity,
        &tmpl,
        count.max(1),
        false,
        property,
    )
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn request_profile_item(
    ctx: &ReducerContext,
    player_guid: u64,
    item_entry: u32,
    target_count: u32,
) -> Result<u32, ActionRefusal> {
    profile_item_target_admitted(target_count)?;
    let owned = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .filter(|row| row.entry == item_entry)
        .map(|row| row.stack_count)
        .fold(0u32, u32::saturating_add);
    let missing = target_count.saturating_sub(owned);
    if missing == 0 {
        return Ok(0);
    }
    request_grant_item(ctx, player_guid, item_entry, missing, None)?;
    Ok(missing)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
const PROFILE_ITEM_LIMIT: u32 = 200;

#[cfg_attr(not(has_packages), allow(dead_code))]
fn profile_item_target_admitted(target_count: u32) -> Result<(), ActionRefusal> {
    if (1..=PROFILE_ITEM_LIMIT).contains(&target_count) {
        return Ok(());
    }
    Err(ActionRefusal::new(
        crate::actor::ActionRefusalKind::ProfileLimit,
        format!("profile item target {target_count} is outside 1..={PROFILE_ITEM_LIMIT}"),
    ))
}

// Callers may catch a Refusal, so failed storage must leave inventory unchanged.
pub(crate) fn store_item(
    ctx: &ReducerContext,
    player_guid: u64,
    owner_identity: spacetimedb::Identity,
    tmpl: &ItemTemplate,
    count: u32,
    force_soulbound: bool,
    preselected_property: Option<u32>,
) -> Result<(), StoreRefusal> {
    store_item_typed(
        ctx,
        player_guid,
        owner_identity,
        tmpl,
        count,
        force_soulbound,
        preselected_property,
    )
    .map_err(|refusal| match refusal.kind {
        ActionRefusalKind::InventoryFull => StoreRefusal::InventoryFull,
        _ => StoreRefusal::Refused(refusal.into()),
    })
}

fn store_item_typed(
    ctx: &ReducerContext,
    player_guid: u64,
    owner_identity: spacetimedb::Identity,
    tmpl: &ItemTemplate,
    count: u32,
    force_soulbound: bool,
    preselected_property: Option<u32>,
) -> Result<(), ActionRefusal> {
    if count == 0 {
        return Ok(());
    }
    let mut plan = super::exchange::ItemStoragePlan::read(ctx, player_guid, owner_identity);
    plan.grant(ctx, tmpl, count, force_soulbound, preselected_property)?;
    plan.commit(ctx)
}

pub(crate) enum StoreRefusal {
    InventoryFull,
    Refused(crate::Refusal),
}

impl From<String> for StoreRefusal {
    fn from(cause: String) -> Self {
        Self::Refused(cause.into())
    }
}

impl From<StoreRefusal> for String {
    fn from(refusal: StoreRefusal) -> Self {
        match refusal {
            StoreRefusal::InventoryFull => lyracore_shared::mail::INVENTORY_FULL.into(),
            StoreRefusal::Refused(refusal) => refusal.into(),
        }
    }
}

impl From<StoreRefusal> for crate::Refusal {
    fn from(refusal: StoreRefusal) -> Self {
        String::from(refusal).into()
    }
}

pub(crate) fn free_slot(ctx: &ReducerContext, player_guid: u64) -> Result<u8, StoreRefusal> {
    first_free_backpack_slot(ctx, player_guid)
        .or_else(|| first_free_bag_slot(ctx, player_guid))
        .ok_or(StoreRefusal::InventoryFull)
}

pub(crate) fn has_free_slot(ctx: &ReducerContext, player_guid: u64) -> bool {
    free_slot(ctx, player_guid).is_ok()
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct ItemSnapshot {
    pub entry: u32,
    pub stack_count: u32,
    pub durability: u32,
    pub enchant_id: u32,
    pub soulbound: bool,
    pub random_property_id: u32,
    // ITEM_FIELD_ITEM_TEXT_ID; zero except on a Letter Copy.
    pub item_text_id: u32,
}

impl ItemSnapshot {
    pub(crate) fn is_empty(&self) -> bool {
        self.entry == 0
    }
}

impl From<&ItemInstance> for ItemSnapshot {
    fn from(item: &ItemInstance) -> Self {
        Self {
            entry: item.entry,
            stack_count: item.stack_count,
            durability: item.durability,
            enchant_id: item.enchant_id,
            soulbound: item.soulbound,
            random_property_id: item.random_property_id,
            item_text_id: item.item_text_id,
        }
    }
}

pub(crate) fn store_instance_state(
    ctx: &ReducerContext,
    player_guid: u64,
    owner_identity: Identity,
    tmpl: &ItemTemplate,
    preallocated_guid: Option<u64>,
    snapshot: &ItemSnapshot,
) -> Result<(), StoreRefusal> {
    let slot = free_slot(ctx, player_guid)?;
    let guid = match preallocated_guid {
        Some(guid) => guid,
        None => next_item_guid(ctx)?,
    };
    crate::ItemInstance::insert(
        ctx,
        ItemInstance {
            guid,
            entry: tmpl.entry,
            owner_identity,
            owner_guid: player_guid,
            slot,
            // Snapshots can outlive template edits, including a lower stack cap.
            stack_count: snapshot.stack_count.max(1).min(tmpl.max_stack.max(1)),
            durability: snapshot.durability,
            created_at: crate::time::now(ctx),
            enchant_id: snapshot.enchant_id,
            soulbound: snapshot.soulbound || binds_on_grant(tmpl.bonding),
            random_property_id: snapshot.random_property_id,
            item_text_id: snapshot.item_text_id,
        },
    );
    Ok(())
}

// Retry identity is item_text_id, so a lost reply cannot duplicate a Letter Copy.
pub(crate) fn grant_letter_item(
    ctx: &ReducerContext,
    player_guid: u64,
    item_entry: u32,
    item_text_id: u32,
) -> Result<(), StoreRefusal> {
    let already_granted = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .any(|i| item_text_id != 0 && i.item_text_id == item_text_id);
    if already_granted {
        return Ok(());
    }
    let player = crate::helpers::acting_entity_by_guid(ctx, player_guid)
        .ok_or_else(|| lyracore_shared::mail::NOT_IN_WORLD.to_string())?;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(item_entry)
        .ok_or_else(|| format!("mail: no template for letter item {item_entry}"))?;
    let slot = free_slot(ctx, player_guid)?;
    let guid = next_item_guid(ctx)?;
    crate::ItemInstance::insert(
        ctx,
        ItemInstance {
            guid,
            entry: tmpl.entry,
            owner_identity: player.owner_identity,
            owner_guid: player_guid,
            slot,
            stack_count: 1,
            durability: tmpl.max_durability,
            created_at: crate::time::now(ctx),
            enchant_id: 0,
            soulbound: binds_on_grant(tmpl.bonding),
            random_property_id: 0,
            item_text_id,
        },
    );
    Ok(())
}

pub(crate) fn item_count(ctx: &ReducerContext, owner_guid: u64, item_entry: u32) -> u32 {
    lyracore_shared::item::carried_quantity(
        item_entry,
        ctx.db
            .game_item_instance()
            .by_owner_guid()
            .filter(&owner_guid)
            .map(|item| (item.slot, item.entry, item.stack_count)),
    )
}

// A caught Refusal must leave every stack unchanged.
pub(crate) fn remove_items(
    ctx: &ReducerContext,
    owner_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    let instances = ctx.db.game_item_instance();
    let mut stacks: Vec<ItemInstance> = instances
        .by_owner_guid()
        .filter(&owner_guid)
        .filter(|i| is_carried_slot(i.slot) && i.entry == item_entry)
        .collect();
    let original_counts: Vec<u32> = stacks.iter().map(|item| item.stack_count).collect();
    super::exchange::consume_item_stacks(&mut stacks, item_entry, count).map_err(String::from)?;
    if stacks
        .iter()
        .zip(&original_counts)
        .any(|(item, original_count)| {
            item.stack_count != *original_count
                && crate::trade::item_is_offered(ctx, owner_guid, item.guid)
        })
    {
        return Err(super::refused(ItemRefusal::NotRightNow));
    }
    for (inst, original_count) in stacks.into_iter().zip(original_counts) {
        if inst.stack_count == original_count {
            continue;
        }
        if inst.stack_count == 0 {
            crate::ItemInstance::delete(ctx, inst.guid);
        } else {
            inst.update(ctx);
        }
    }
    Ok(())
}

pub(crate) const RECENTLY_BANDAGED_SPELL: u32 = 11196;

fn use_spell_for(tmpl: &ItemTemplate) -> Option<u32> {
    (tmpl.spellid_1 != 0 && tmpl.spelltrigger_1 == 0).then_some(tmpl.spellid_1)
}

fn spell_keeps_item(ctx: &ReducerContext, spell_id: u32) -> bool {
    let reusable_effect = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .any(|e| {
            matches!(
                e.kind,
                crate::spell::E_RECALL_HOME | crate::spell::E_SUMMON_HOSTILE
            )
        });
    reusable_effect || crate::mount::spell_is_mount(ctx, spell_id)
}

fn bandage_cooldown_blocks(entry: u32, has_cooldown_debuff: bool) -> bool {
    matches!(entry, 1251 | 2581) && has_cooldown_debuff
}

fn is_mana_class(ctx: &ReducerContext, player_guid: u64) -> bool {
    ctx.db
        .game_character()
        .guid()
        .find(player_guid)
        .map(|c| {
            lyracore_shared::packing::power_type::for_class(c.class)
                == lyracore_shared::packing::power_type::MANA
        })
        .unwrap_or(false)
}

fn spell_restores_power(ctx: &ReducerContext, spell_id: u32) -> bool {
    ctx.db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .any(|e| e.kind == crate::spell::E_ENERGIZE || e.kind == crate::spell::A_PERIODIC_ENERGIZE)
}

pub(crate) fn apply_item_use(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
) -> Result<(), ItemRefusal> {
    apply_item_use_on(ctx, player_guid, slot, None)
}

/// Uses an item at another of the user's items, such as a poison at a weapon.
pub(crate) fn apply_item_use_on(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
    item_target: Option<u64>,
) -> Result<(), ItemRefusal> {
    let entities = ctx.db.game_world_entity();
    let player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| refuse(ItemRefusal::Internal, "user not in world"))?;
    if player.dead {
        return Err(refuse(
            ItemRefusal::PlayerDead,
            "dead players cannot use items",
        ));
    }
    let mut inst = item_in_slot(ctx, player_guid, slot)
        .ok_or_else(|| refuse(ItemRefusal::ItemNotFound, format!("no item in slot {slot}")))?;
    if crate::trade::item_is_offered(ctx, player_guid, inst.guid) {
        return Err(ItemRefusal::NotRightNow);
    }
    if !is_carried_slot(inst.slot) {
        return Err(refuse(
            ItemRefusal::ItemNotUsable,
            "cannot use a banked item",
        ));
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(inst.entry)
        .ok_or_else(|| {
            refuse(
                ItemRefusal::ItemNotFound,
                format!("no template for item entry {}", inst.entry),
            )
        })?;
    let spell_id = use_spell_for(&tmpl).ok_or_else(|| {
        refuse(
            ItemRefusal::ItemNotUsable,
            format!("item {} is not consumable", inst.entry),
        )
    })?;
    // The level-specific packet needs a level payload, which ItemRefusal cannot carry.
    if !meets_required_level(player.level, tmpl.required_level) {
        return Err(refuse(
            ItemRefusal::ItemNotUsable,
            format!("requires level {}", tmpl.required_level),
        ));
    }
    if bandage_cooldown_blocks(
        inst.entry,
        crate::spell::has_aura(ctx, player_guid, RECENTLY_BANDAGED_SPELL),
    ) {
        return Err(refuse(ItemRefusal::NotRightNow, "Recently Bandaged"));
    }
    // Energize restores rage or energy on non-mana classes, allowing out-of-combat preloading.
    if spell_restores_power(ctx, spell_id) && !is_mana_class(ctx, player_guid) {
        return Err(refuse(
            ItemRefusal::ItemNotUsable,
            "only mana users can use that",
        ));
    }
    // Propagate cast Refusals so the reducer rolls back item consumption.
    if !spell_keeps_item(ctx, spell_id) {
        if inst.stack_count > 1 {
            inst.stack_count -= 1;
            inst.update(ctx);
        } else {
            crate::ItemInstance::delete(ctx, inst.guid);
        }
    }
    match item_target {
        Some(item_guid) => crate::spell::begin_item_cast(
            ctx,
            player_guid,
            spell_id,
            player.level as u8,
            item_guid,
            false,
        ),
        None => crate::spell::begin_cast(
            ctx,
            player_guid,
            spell_id,
            player.level as u8,
            player_guid,
            false,
            None,
        ),
    }
    .map_err(|detail| refuse(ItemRefusal::NotRightNow, detail))
}

pub(crate) fn resistance_items(
    ctx: &ReducerContext,
    owner_guid: u64,
) -> impl Iterator<Item = lyracore_shared::resistance::ResistanceItem> + '_ {
    use crate::game_item_random_property;
    use lyracore_shared::resistance::ResistanceItem;
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(owner_guid)
        .filter_map(|item| {
            let tmpl = ctx.db.game_item_template().entry().find(item.entry)?;
            let property = (item.random_property_id != 0)
                .then(|| {
                    ctx.db
                        .game_item_random_property()
                        .property_id()
                        .find(item.random_property_id)
                })
                .flatten();
            Some(ResistanceItem {
                slot: item.slot,
                max_durability: tmpl.max_durability,
                durability: item.durability,
                template: [
                    EquipStat::Armor,
                    EquipStat::HolyResistance,
                    EquipStat::FireResistance,
                    EquipStat::NatureResistance,
                    EquipStat::FrostResistance,
                    EquipStat::ShadowResistance,
                    EquipStat::ArcaneResistance,
                ]
                .map(|which| template_stat(&tmpl, which)),
                enchant_id: item.enchant_id,
                property_enchants: property
                    .map_or([0; 3], |p| [p.enchant_id_1, p.enchant_id_2, p.enchant_id_3]),
            })
        })
}

fn item_stat_bonus_for_school(
    ctx: &ReducerContext,
    item: &ItemInstance,
    tmpl: &ItemTemplate,
    which: EquipStat,
    school_mask: u32,
) -> i32 {
    let temporary = super::temporary_enchant_id(ctx, item.guid);
    let (enchant, property) = if school_mask == u32::MAX {
        (
            super::enchant_stat(ctx, item.enchant_id, which.kind())
                .saturating_add(super::enchant_stat(ctx, temporary, which.kind())),
            super::property_stat(ctx, item.random_property_id, which.kind()),
        )
    } else {
        (
            super::enchant_stat_for_school(ctx, item.enchant_id, which.kind(), school_mask)
                .saturating_add(super::enchant_stat_for_school(
                    ctx,
                    temporary,
                    which.kind(),
                    school_mask,
                )),
            super::property_stat_for_school(
                ctx,
                item.random_property_id,
                which.kind(),
                school_mask,
            ),
        )
    };
    lyracore_shared::item_property::item_stat(template_stat(tmpl, which), enchant, property)
}

pub(crate) fn item_stat_bonus(
    ctx: &ReducerContext,
    item: &ItemInstance,
    tmpl: &ItemTemplate,
    which: EquipStat,
) -> i32 {
    item_stat_bonus_for_school(ctx, item, tmpl, which, u32::MAX)
}

fn equipped_stat_bonus_for_school(
    ctx: &ReducerContext,
    owner_guid: u64,
    which: EquipStat,
    school_mask: u32,
) -> i32 {
    let templates = ctx.db.game_item_template();
    lyracore_shared::item_property::equipped_stat(
        ctx.db
            .game_item_instance()
            .by_owner_guid()
            .filter(&owner_guid)
            .filter_map(|item| {
                let tmpl = templates.entry().find(item.entry)?;
                if !lyracore_shared::item::grants_equipped_stats(
                    item.slot,
                    tmpl.max_durability,
                    item.durability,
                ) {
                    return None;
                }
                Some((
                    item.slot,
                    tmpl.max_durability,
                    item.durability,
                    item_stat_bonus_for_school(ctx, &item, &tmpl, which, school_mask),
                ))
            }),
    )
}

pub(crate) fn equipped_stat_bonus(ctx: &ReducerContext, owner_guid: u64, which: EquipStat) -> i32 {
    equipped_stat_bonus_for_school(ctx, owner_guid, which, u32::MAX)
}

pub(crate) fn equipped_school_stat_bonus(
    ctx: &ReducerContext,
    owner_guid: u64,
    which: EquipStat,
    school_mask: u8,
) -> i32 {
    equipped_stat_bonus_for_school(ctx, owner_guid, which, u32::from(school_mask))
}

const DEATH_DURABILITY_LOSS_PERCENT: u32 = 10;
const SPIRIT_HEALER_DURABILITY_LOSS_PERCENT: u32 = 25;

fn durability_loss(max_durability: u32, durability: u32, percent: u32) -> u32 {
    if max_durability == 0 || durability == 0 {
        return durability;
    }
    let loss = (max_durability * percent / 100).max(1);
    durability.saturating_sub(loss)
}

pub(crate) fn apply_death_durability_loss(ctx: &ReducerContext, player_guid: u64) -> bool {
    lose_durability(
        ctx,
        player_guid,
        |slot| slot <= equip_slot::END,
        DEATH_DURABILITY_LOSS_PERCENT,
    )
}

pub(crate) fn apply_spirit_healer_durability_loss(ctx: &ReducerContext, player_guid: u64) {
    lose_durability(
        ctx,
        player_guid,
        is_carried_slot,
        SPIRIT_HEALER_DURABILITY_LOSS_PERCENT,
    );
}

fn lose_durability(
    ctx: &ReducerContext,
    owner_guid: u64,
    in_scope: impl Fn(u8) -> bool,
    percent: u32,
) -> bool {
    let templates = ctx.db.game_item_template();
    let items: Vec<_> = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&owner_guid)
        .filter(|item| in_scope(item.slot))
        .collect();
    let mut any_lost = false;
    for mut inst in items {
        if let Some(tmpl) = templates.entry().find(inst.entry) {
            let new_durability = durability_loss(tmpl.max_durability, inst.durability, percent);
            if new_durability != inst.durability {
                inst.durability = new_durability;
                inst.update(ctx);
                any_lost = true;
            }
        }
    }
    any_lost
}

pub(crate) enum TakeLootRefusal {
    Loot(lyracore_shared::loot::LootRefusal),
    Refused(ActionRefusal),
}

impl From<ActionRefusal> for TakeLootRefusal {
    fn from(refusal: ActionRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<TakeLootRefusal> for ActionRefusal {
    fn from(refusal: TakeLootRefusal) -> Self {
        match refusal {
            TakeLootRefusal::Loot(refusal) => {
                ActionRefusal::new(ActionRefusalKind::CannotAct, refusal.as_tag())
            }
            TakeLootRefusal::Refused(refusal) => refusal,
        }
    }
}

pub(crate) fn request_take_loot(
    ctx: &ReducerContext,
    player_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
) -> Result<(), ActionRefusal> {
    take_loot_item(ctx, player_guid, corpse_guid, loot_slot).map_err(Into::into)
}

pub(crate) fn take_loot_item(
    ctx: &ReducerContext,
    player_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
) -> Result<(), TakeLootRefusal> {
    let entities = ctx.db.game_world_entity();
    let player = entities.guid().find(player_guid).ok_or_else(|| {
        ActionRefusal::new(ActionRefusalKind::MissingActor, "looter not in world")
    })?;
    if player.dead {
        return Err(
            ActionRefusal::new(ActionRefusalKind::DeadActor, "dead players cannot loot").into(),
        );
    }
    let (src_map, src_instance, sx, sy, sz, source_is_corpse) = if let Some(corpse) =
        entities.guid().find(corpse_guid)
    {
        if !corpse.dead {
            return Err(ActionRefusal::new(
                ActionRefusalKind::MissingTarget,
                "target is not a corpse",
            )
            .into());
        }
        (
            corpse.map_id,
            corpse.instance_id,
            corpse.x,
            corpse.y,
            corpse.z,
            true,
        )
    } else if let Some(go) = ctx.db.game_gameobject().guid().find(corpse_guid) {
        (go.map_id, go.instance_id, go.x, go.y, go.z, false)
    } else {
        return Err(
            ActionRefusal::new(ActionRefusalKind::MissingTarget, "no such loot source").into(),
        );
    };
    if source_is_corpse {
        crate::loot::corpse_access(ctx, player_guid, corpse_guid).map_err(|refusal| {
            TakeLootRefusal::Loot(crate::loot::refuse(
                refusal,
                &format!("actor_guid={player_guid} corpse_guid={corpse_guid}"),
            ))
        })?;
    }
    if src_map != player.map_id {
        return Err(
            ActionRefusal::new(ActionRefusalKind::OtherPartition, "loot on another map").into(),
        );
    }

    if src_instance != player.instance_id {
        return Err(ActionRefusal::new(
            ActionRefusalKind::OtherPartition,
            "loot in another instance",
        )
        .into());
    }
    let (dx, dy, dz) = (sx - player.x, sy - player.y, sz - player.z);
    if dx * dx + dy * dy + dz * dz > crate::loot::LOOT_RANGE_SQ {
        return Err(ActionRefusal::new(ActionRefusalKind::OutOfRange, "loot out of range").into());
    }
    let loot = ctx.db.game_corpse_loot();
    let row = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == loot_slot)
        .ok_or_else(|| {
            ActionRefusal::new(ActionRefusalKind::MissingTarget, "no loot in that slot")
        })?;
    if row.quest_only {
        let needs = crate::loot::killer_needs_item(ctx, Some(player_guid), row.item_entry);
        if !crate::loot::quest_take_allowed(row.reserved_for, player_guid, needs) {
            return Err(ActionRefusal::new(
                ActionRefusalKind::CannotAct,
                if row.reserved_for != 0 && row.reserved_for != player_guid {
                    "this item is reserved for another player"
                } else {
                    "you do not need this quest item"
                },
            )
            .into());
        }
    } else if !crate::loot::group_loot_take_allowed(
        row.withheld,
        row.reserved_for,
        row.master_only,
        row.designated_looter_guid,
        player_guid,
    ) {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            if row.master_only {
                "this item requires the master looter to distribute it"
            } else {
                "this item belongs to another looter right now"
            },
        )
        .into());
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(row.item_entry)
        .ok_or_else(|| {
            ActionRefusal::new(
                ActionRefusalKind::MissingTarget,
                format!("no template for item entry {}", row.item_entry),
            )
        })?;
    // Grant before deleting the loot row; failed storage leaves it available for retry.
    store_item_typed(
        ctx,
        player_guid,
        player.owner_identity,
        &tmpl,
        row.count.max(1),
        false,
        Some(row.random_property_id),
    )?;
    let (item_entry, count) = (row.item_entry, row.count.max(1));
    let (was_quest_only, was_unreserved) = (row.quest_only, row.reserved_for == 0);
    loot.id().delete(row.id);
    if was_quest_only && was_unreserved {
        if source_is_corpse {
            crate::loot::clone_quest_loot_for_eligible(
                ctx,
                player_guid,
                corpse_guid,
                item_entry,
                count,
                row.random_property_id,
            );
        } else {
            crate::loot::clone_quest_loot_for_group(
                ctx,
                player_guid,
                corpse_guid,
                item_entry,
                count,
                row.random_property_id,
            );
        }
    }
    if source_is_corpse {
        crate::loot::refresh_lootable(ctx, corpse_guid);
    }
    crate::hooks::fire_on_loot(
        ctx,
        &crate::hooks::LootPayload {
            looter_guid: player_guid,
            corpse_guid,
            item_entry,
            count,
        },
    );
    Ok(())
}

pub(crate) fn apply_take_loot(
    ctx: &ReducerContext,
    player_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
) -> Result<(), String> {
    request_take_loot(ctx, player_guid, corpse_guid, loot_slot).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{
        bandage_cooldown_blocks, durability_loss, profile_item_target_admitted, use_spell_for,
        RECENTLY_BANDAGED_SPELL,
    };

    #[test]
    fn profile_item_target_refuses_zero_and_over_limit_counts() {
        for target in [0, 201] {
            assert_eq!(
                profile_item_target_admitted(target).unwrap_err().kind,
                crate::actor::ActionRefusalKind::ProfileLimit
            );
        }
        assert!(profile_item_target_admitted(1).is_ok());
        assert!(profile_item_target_admitted(200).is_ok());
    }

    #[test]
    fn use_spell_for_reads_spellid_1_gated_on_the_on_use_trigger() {
        let mut tmpl = crate::seed::base_item(1, "x");
        tmpl.spellid_1 = 50110;
        tmpl.spelltrigger_1 = 0;
        assert_eq!(use_spell_for(&tmpl), Some(50110));

        tmpl.spellid_1 = 0;
        assert_eq!(use_spell_for(&tmpl), None);

        tmpl.spellid_1 = 50110;
        tmpl.spelltrigger_1 = 1;
        assert_eq!(use_spell_for(&tmpl), None);
    }

    #[test]
    fn bandage_cooldown_gate_blocks_only_the_bandage_while_debuff_is_live() {
        assert!(
            bandage_cooldown_blocks(1251, true),
            "re-bandage blocked while Recently Bandaged is live"
        );
        assert!(
            !bandage_cooldown_blocks(1251, false),
            "bandage usable once the cooldown expired"
        );
        assert!(
            bandage_cooldown_blocks(2581, true),
            "the rank-2 Heavy Linen shares the same lockout"
        );
        assert!(
            !bandage_cooldown_blocks(2581, false),
            "rank-2 bandage usable once the cooldown expired"
        );
        assert!(!bandage_cooldown_blocks(118, true));
        assert!(!bandage_cooldown_blocks(2681, true));
        assert_eq!(RECENTLY_BANDAGED_SPELL, 11196);
    }

    #[test]
    fn durability_loss_is_a_percent_of_max_floored_at_one_and_saturating() {
        assert_eq!(durability_loss(100, 100, 10), 90);
        assert_eq!(durability_loss(10, 10, 10), 9);
        assert_eq!(durability_loss(5, 5, 10), 4);
        assert_eq!(durability_loss(100, 5, 10), 0);
        assert_eq!(durability_loss(100, 0, 10), 0);
        assert_eq!(durability_loss(0, 50, 10), 50);
        assert_eq!(durability_loss(100, 100, 25), 75);
        assert_eq!(durability_loss(3, 3, 25), 2);
    }
}
