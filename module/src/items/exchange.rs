use crate::actor::{ActionRefusal, ActionRefusalKind};
use lyracore_protocol::messages::vanilla::NewItemSource;
use spacetimedb::{Identity, ReducerContext, Timestamp};

use super::inventory::{free_inventory_slots, is_carried_slot, item_fits_bag};
use super::{
    allocate_item_guids, binds_on_grant, game_item_instance, game_item_template, merge_amount,
    select_property, ItemInstance, ItemTemplate,
};

// Refusals leave items unchanged; GUID reservations may still advance the range.
pub(crate) fn exchange_items(
    ctx: &ReducerContext,
    character_guid: u64,
    consumed: &[(u32, u32)],
    granted: &[(u32, u32)],
) -> Result<(), ActionRefusal> {
    let character = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|_| "player not in world".to_string())?;
    let mut plan = ItemStoragePlan::read(ctx, character_guid, character.owner_identity);
    for &(entry, count) in consumed {
        plan.consume(entry, count)?;
    }
    for &(entry, count) in granted {
        let template = ctx
            .db
            .game_item_template()
            .entry()
            .find(entry)
            .ok_or_else(|| format!("no such item {entry}"))?;
        plan.grant(ctx, &template, count.max(1), false, None)?;
    }
    plan.commit(ctx)
}

pub(super) fn consume_item_stacks(
    items: &mut [ItemInstance],
    entry: u32,
    mut count: u32,
) -> Result<(), ActionRefusal> {
    let mut matching: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| is_carried_slot(item.slot) && item.entry == entry)
        .map(|(index, _)| index)
        .collect();
    let available: u64 = matching
        .iter()
        .map(|&index| u64::from(items[index].stack_count))
        .sum();
    if available < u64::from(count) {
        return Err(format!("missing {} of item {entry}", u64::from(count) - available).into());
    }
    matching.sort_by_key(|&index| items[index].slot);
    for index in matching {
        let item = &mut items[index];
        let take = count.min(item.stack_count);
        item.stack_count -= take;
        count -= take;
        if count == 0 {
            break;
        }
    }
    Ok(())
}

// Reserve every new GUID before committing any inventory change.
pub(crate) struct ItemStoragePlan {
    character_guid: u64,
    owner_identity: Identity,
    created_at: Timestamp,
    items: Vec<ItemInstance>,
    original_stacks: Vec<(u8, u32, bool)>,
}

impl ItemStoragePlan {
    pub(crate) fn read(
        ctx: &ReducerContext,
        character_guid: u64,
        owner_identity: Identity,
    ) -> Self {
        Self::new(
            character_guid,
            owner_identity,
            crate::time::now(ctx),
            ctx.db
                .game_item_instance()
                .by_owner_guid()
                .filter(&character_guid)
                .collect(),
        )
    }

    fn new(
        character_guid: u64,
        owner_identity: Identity,
        created_at: Timestamp,
        mut items: Vec<ItemInstance>,
    ) -> Self {
        items.sort_by_key(|item| item.slot);
        let original_stacks = items
            .iter()
            .map(|item| (item.slot, item.stack_count, item.soulbound))
            .collect();
        Self {
            character_guid,
            owner_identity,
            created_at,
            items,
            original_stacks,
        }
    }

    pub(crate) fn consume(&mut self, entry: u32, count: u32) -> Result<(), ActionRefusal> {
        consume_item_stacks(&mut self.items, entry, count)
    }

    pub(crate) fn grant(
        &mut self,
        ctx: &ReducerContext,
        template: &ItemTemplate,
        count: u32,
        force_soulbound: bool,
        preselected_property: Option<u32>,
    ) -> Result<(), ActionRefusal> {
        self.plan_grant(
            template,
            count,
            force_soulbound,
            preselected_property,
            || select_property(ctx, template),
            |entry| {
                ctx.db
                    .game_item_template()
                    .entry()
                    .find(entry)
                    .map_or(0, |bag| bag.container_slots)
            },
        )
    }

    pub(crate) fn purchase(
        &mut self,
        ctx: &ReducerContext,
        template: &ItemTemplate,
        count: u32,
        bag: u8,
        slot: u8,
    ) -> Result<u8, super::economy::BuyRefusal> {
        use lyracore_protocol::messages::vanilla::InventoryResult;
        if bag == 255 && !((0..39).contains(&slot) || (81..113).contains(&slot) || slot == 255) {
            return Err(InventoryResult::ItemDoesntGoToSlot.into());
        }
        let owned: u64 = self
            .items
            .iter()
            .filter(|item| item.entry == template.entry)
            .map(|item| u64::from(item.stack_count))
            .sum();
        if bag == 255 && slot < 23 && count != 1 {
            return Err(InventoryResult::ItemCantBeEquipped.into());
        }
        if template.max_count != 0 && owned + u64::from(count) > u64::from(template.max_count) {
            return Err(InventoryResult::CantCarryMoreOfThis.into());
        }
        if bag == 255 && slot < 23 {
            self.purchase_equipment(ctx, template, slot)
        } else {
            self.purchase_inventory(ctx, template, count, bag, slot)
        }
    }

    fn purchase_equipment(
        &mut self,
        ctx: &ReducerContext,
        template: &ItemTemplate,
        slot: u8,
    ) -> Result<u8, super::economy::BuyRefusal> {
        use lyracore_protocol::messages::vanilla::InventoryResult;
        let character = crate::helpers::live_entity(ctx, self.character_guid)?;
        if crate::spell::is_stunned(ctx, self.character_guid) {
            return Err(InventoryResult::YouAreStunned.into());
        }
        if character.combat_active
            && !matches!(template.class, 2 | 6)
            && !matches!(template.inventory_type, 14 | 23 | 28)
        {
            return Err(InventoryResult::NotInCombat.into());
        }
        if self.items.iter().any(|item| item.slot == slot) {
            return Err(InventoryResult::ItemCantBeEquipped.into());
        }
        super::inventory::validate_equipment(ctx, self.character_guid, template, slot).map_err(
            |result| super::economy::BuyRefusal::Equipment {
                result,
                required_level: template.required_level,
            },
        )?;
        if template.class == 11 {
            if let Some(quiver) = self
                .items
                .iter()
                .filter(|item| (19..23).contains(&item.slot))
                .filter_map(|item| ctx.db.game_item_template().entry().find(item.entry))
                .find(|bag| bag.class == 11)
            {
                return Err(if quiver.subclass == 3 {
                    InventoryResult::CanEquipOnly1Ammopouch
                } else {
                    InventoryResult::CanEquipOnly1Quiver
                }
                .into());
            }
        }
        if slot == 16
            && matches!(template.inventory_type, 13 | 22)
            && !crate::spell::knows_spell(
                ctx,
                self.character_guid,
                lyracore_shared::constants::dual_wield::SPELL_ID,
            )
        {
            return Err(InventoryResult::CantDualWield.into());
        }
        if slot == 16
            && self
                .items
                .iter()
                .find(|item| item.slot == 15)
                .and_then(|item| ctx.db.game_item_template().entry().find(item.entry))
                .is_some_and(|main| main.inventory_type == 17)
        {
            return Err(InventoryResult::CantEquipWithTwohanded.into());
        }
        self.items.push(self.new_stack(
            template,
            slot,
            1,
            select_property(ctx, template)?,
            binds_on_grant(template.bonding) || super::rules::binds_on_equip(template.bonding),
        ));
        if template.inventory_type == 17 {
            if let Some(offhand) =
                super::inventory::displaced_off_hand(ctx, self.items.clone(), &[Some(0), None])
                    .map_err(|_| InventoryResult::InventoryFull)?
            {
                self.items
                    .iter_mut()
                    .find(|item| item.guid == offhand.guid)
                    .expect("the off-hand item belongs to the plan")
                    .slot = offhand.slot;
            }
        }
        Ok(slot)
    }

    fn purchase_inventory(
        &mut self,
        ctx: &ReducerContext,
        template: &ItemTemplate,
        mut count: u32,
        bag: u8,
        slot: u8,
    ) -> Result<u8, super::economy::BuyRefusal> {
        use lyracore_protocol::messages::vanilla::InventoryResult;
        let slots = self.purchase_slots(ctx, template, bag, slot)?;
        let mut destinations = Vec::new();
        for slot in slots {
            if count == 0 {
                break;
            }
            if destinations.iter().any(|&(planned, _)| planned == slot) {
                continue;
            }
            let current_count = self
                .items
                .iter()
                .find(|item| item.slot == slot)
                .map_or(0, |item| item.stack_count);
            let take = count.min(template.max_stack.max(1).saturating_sub(current_count));
            if take != 0 {
                destinations.push((slot, take));
                count -= take;
            }
        }
        if count != 0 {
            return Err(InventoryResult::InventoryFull.into());
        }
        let property = select_property(ctx, template)?;
        let last_slot = destinations.last().ok_or(InventoryResult::InventoryFull)?.0;
        for (slot, count) in destinations {
            self.add_stack(
                template,
                slot,
                count,
                property,
                binds_on_grant(template.bonding),
            );
        }
        Ok(last_slot)
    }

    fn purchase_slots(
        &self,
        ctx: &ReducerContext,
        template: &ItemTemplate,
        bag: u8,
        slot: u8,
    ) -> Result<Vec<u8>, super::economy::BuyRefusal> {
        use lyracore_protocol::{codec, messages::vanilla::InventoryResult};
        let character = crate::helpers::live_entity(ctx, self.character_guid)?;
        let keyring_end = 81
            + match character.level {
                61.. => 16,
                50..=60 => 12,
                40..=49 => 8,
                _ => 4,
            };
        let (preferred, explicit) = if bag == 255 {
            if (81..keyring_end).contains(&slot)
                && template.bag_family != 9
                && !self.items.iter().any(|item| item.slot == slot)
            {
                return Err(InventoryResult::ItemDoesntGoIntoBag2.into());
            }
            (
                (81..113).chain(23..39).collect::<Vec<_>>(),
                (slot != 255).then_some(slot),
            )
        } else {
            let bag_item = self
                .items
                .iter()
                .find(|item| item.slot == bag)
                .ok_or(InventoryResult::IntBagError)?;
            let bag_template = ctx
                .db
                .game_item_template()
                .entry()
                .find(bag_item.entry)
                .ok_or(InventoryResult::IntBagError)?;
            let occupied = codec::inventory_slot(bag, slot)
                .is_some_and(|slot| self.items.iter().any(|item| item.slot == slot));
            if !occupied && slot != 255 && slot >= bag_template.container_slots.min(18) {
                return Err(InventoryResult::IntBagError.into());
            }
            if !occupied && !item_fits_bag(template, &bag_template) {
                return Err(InventoryResult::ItemDoesntGoIntoBag2.into());
            }
            let explicit = if slot == 255 {
                None
            } else {
                Some(codec::inventory_slot(bag, slot).ok_or(InventoryResult::IntBagError)?)
            };
            (
                (0..bag_template.container_slots.min(18))
                    .filter_map(|slot| codec::inventory_slot(bag, slot))
                    .collect(),
                explicit,
            )
        };
        if let Some(slot) = explicit {
            if let Some(current) = self.items.iter().find(|item| item.slot == slot) {
                if current.entry != template.entry
                    || current.stack_count >= template.max_stack.max(1)
                {
                    return Err(InventoryResult::ItemCantStack.into());
                }
            }
        }
        let (specialized, regular) = self.purchase_bag_slots(ctx, template);
        let merges: Vec<_> = (81..113)
            .chain(23..39)
            .chain(specialized.iter().copied())
            .chain(regular.iter().copied())
            .collect();
        let empty: Vec<_> = (81..keyring_end)
            .filter(|_| template.bag_family == 9)
            .chain(specialized.iter().copied())
            .chain(23..39)
            .chain(regular.iter().copied())
            .collect();
        let mut order: Vec<_> = explicit.into_iter().collect();
        let mut append = |slots: &[u8], merge: bool| {
            order.extend(slots.iter().copied().filter(|slot| {
                match self.items.iter().find(|item| item.slot == *slot) {
                    Some(item) => {
                        merge
                            && item.entry == template.entry
                            && item.stack_count < template.max_stack.max(1)
                    }
                    None => !merge && empty.contains(slot),
                }
            }));
        };
        append(&preferred, true);
        append(&preferred, false);
        append(&merges, true);
        append(&empty, false);
        Ok(order)
    }

    fn purchase_bag_slots(
        &self,
        ctx: &ReducerContext,
        template: &ItemTemplate,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut specialized = Vec::new();
        let mut regular = Vec::new();
        for slot in 19..23 {
            let Some(bag) = self.items.iter().find(|item| item.slot == slot) else {
                continue;
            };
            let Some(bag) = ctx.db.game_item_template().entry().find(bag.entry) else {
                continue;
            };
            if item_fits_bag(template, &bag) {
                let slots = if bag.class == 1 && bag.subclass == 0 {
                    &mut regular
                } else {
                    &mut specialized
                };
                slots.extend((0..bag.container_slots.min(18)).filter_map(|position| {
                    lyracore_protocol::codec::inventory_slot(slot, position)
                }));
            }
        }
        (specialized, regular)
    }

    fn add_stack(
        &mut self,
        template: &ItemTemplate,
        slot: u8,
        count: u32,
        property: u32,
        soulbound: bool,
    ) {
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.slot == slot && item.stack_count > 0)
        {
            item.stack_count += count;
            item.soulbound |= soulbound;
        } else {
            self.items
                .push(self.new_stack(template, slot, count, property, soulbound));
        }
    }

    fn new_stack(
        &self,
        template: &ItemTemplate,
        slot: u8,
        count: u32,
        random_property_id: u32,
        soulbound: bool,
    ) -> ItemInstance {
        ItemInstance {
            guid: 0,
            entry: template.entry,
            owner_identity: self.owner_identity,
            owner_guid: self.character_guid,
            slot,
            stack_count: count,
            durability: template.max_durability,
            created_at: self.created_at,
            enchant_id: 0,
            soulbound,
            random_property_id,
            item_text_id: 0,
        }
    }

    // Returns whether storage retains the sold Item GUID instead of consuming it in existing stacks.
    pub(crate) fn restore(
        &mut self,
        ctx: &ReducerContext,
        template: &ItemTemplate,
        item: &ItemInstance,
    ) -> Result<bool, ActionRefusal> {
        let max_stack = template.max_stack.max(1);
        let partials: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, current)| {
                current.slot >= lyracore_shared::constants::starter_item::BACKPACK_SLOT_0
                    && is_carried_slot(current.slot)
                    && current.entry == item.entry
                    && current.stack_count > 0
                    && current.stack_count < max_stack
            })
            .map(|(index, _)| index)
            .collect();
        let mut count = item.stack_count;
        let remaining = partials.iter().fold(count, |left, &index| {
            left.saturating_sub(max_stack - self.items[index].stack_count)
        });
        let new_stacks = remaining.div_ceil(max_stack) as usize;
        let slots = free_inventory_slots(&self.items, |entry| {
            ctx.db
                .game_item_template()
                .entry()
                .find(entry)
                .map_or(0, |bag| bag.container_slots)
        });
        if new_stacks > slots.len() {
            return Err(ActionRefusal::new(
                ActionRefusalKind::InventoryFull,
                lyracore_shared::mail::INVENTORY_FULL,
            ));
        }
        for index in partials {
            let current = &mut self.items[index];
            let add = merge_amount(count, current.stack_count, max_stack);
            current.stack_count += add;
            current.soulbound |= binds_on_grant(template.bonding);
            count -= add;
            if count == 0 {
                break;
            }
        }
        for (index, slot) in slots.into_iter().take(new_stacks).enumerate() {
            let last = index + 1 == new_stacks;
            let mut restored = item.clone();
            restored.slot = slot;
            restored.stack_count = count.min(max_stack);
            restored.soulbound |= binds_on_grant(template.bonding);
            if !last {
                restored.guid = 0;
                restored.durability = template.max_durability;
                restored.enchant_id = 0;
                restored.item_text_id = 0;
                restored.created_at = self.created_at;
            }
            count -= restored.stack_count;
            self.items.push(restored);
        }
        Ok(new_stacks != 0)
    }

    fn plan_grant(
        &mut self,
        template: &ItemTemplate,
        mut count: u32,
        force_soulbound: bool,
        preselected_property: Option<u32>,
        mut property: impl FnMut() -> Result<u32, String>,
        bag_capacity: impl FnMut(u32) -> u8,
    ) -> Result<(), ActionRefusal> {
        if count == 0 {
            return Ok(());
        }
        let owned_count: u64 = self
            .items
            .iter()
            .filter(|item| item.entry == template.entry)
            .map(|item| u64::from(item.stack_count))
            .sum();
        if template.max_count != 0 && owned_count + u64::from(count) > u64::from(template.max_count)
        {
            return Err(format!(
                "unique item limit {} for item {}",
                template.max_count, template.entry
            )
            .into());
        }
        let max_stack = template.max_stack.max(1);
        let random_property_id = match preselected_property {
            Some(property) => property,
            None => property()?,
        };
        let mut partials: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                is_carried_slot(item.slot)
                    && item.stack_count > 0
                    && item.entry == template.entry
                    && item.random_property_id == random_property_id
                    && item.stack_count < max_stack
            })
            .map(|(index, _)| index)
            .collect();
        partials.sort_by_key(|&index| self.items[index].slot);
        let remaining = partials.iter().fold(count, |left, &index| {
            left.saturating_sub(max_stack - self.items[index].stack_count)
        });
        let new_stacks = remaining.div_ceil(max_stack) as usize;
        let slots = free_inventory_slots(&self.items, bag_capacity);
        if new_stacks > slots.len() {
            return Err(ActionRefusal::new(
                ActionRefusalKind::InventoryFull,
                lyracore_shared::mail::INVENTORY_FULL,
            ));
        }
        let properties: Vec<u32> = (0..new_stacks)
            .map(|index| {
                if index == 0 || preselected_property.is_some() {
                    Ok(random_property_id)
                } else {
                    property()
                }
            })
            .collect::<Result<_, _>>()?;
        let soulbound = force_soulbound || binds_on_grant(template.bonding);
        for index in partials {
            let item = &self.items[index];
            let add = merge_amount(count, item.stack_count, max_stack);
            self.add_stack(template, item.slot, add, random_property_id, soulbound);
            count -= add;
            if count == 0 {
                break;
            }
        }
        for (slot, random_property_id) in slots.into_iter().zip(properties) {
            let take = count.min(max_stack);
            self.add_stack(template, slot, take, random_property_id, soulbound);
            count -= take;
        }
        Ok(())
    }

    pub(crate) fn check_trade_offers(&self, ctx: &ReducerContext) -> Result<bool, ActionRefusal> {
        let mut changed_offer = false;
        for (item, &(slot, count, soulbound)) in self.items.iter().zip(&self.original_stacks) {
            if (item.slot, item.stack_count, item.soulbound) != (slot, count, soulbound)
                && crate::trade::item_is_offered(ctx, self.character_guid, item.guid)
            {
                if item.stack_count < count || item.slot != slot {
                    return Err(
                        super::refused(lyracore_shared::item::ItemRefusal::NotRightNow).into(),
                    );
                }
                changed_offer = true;
            }
        }
        Ok(changed_offer)
    }

    pub(crate) fn commit(self, ctx: &ReducerContext) -> Result<(), ActionRefusal> {
        self.commit_with_source(ctx, NewItemSource::Looted)
    }

    pub(crate) fn commit_with_source(
        self,
        ctx: &ReducerContext,
        source: NewItemSource,
    ) -> Result<(), ActionRefusal> {
        self.commit_planned(ctx, Some(source), false)
    }

    pub(crate) fn commit_restored(self, ctx: &ReducerContext) -> Result<(), ActionRefusal> {
        self.commit_planned(ctx, None, true)
    }

    pub(crate) fn commit_purchase(
        self,
        ctx: &ReducerContext,
        last_slot: u8,
        count: u32,
    ) -> Result<(), ActionRefusal> {
        let character_guid = self.character_guid;
        let purchased = self
            .items
            .iter()
            .find(|item| item.slot == last_slot)
            .expect("the purchase plan has a final destination");
        crate::native::item_purchase_started(ctx, purchased, count);
        self.commit_planned(ctx, None, false)?;
        let item = super::item_in_slot(ctx, character_guid, last_slot)
            .expect("the purchase plan committed its final destination");
        crate::native::item_purchased(ctx, &item, count);
        if last_slot < 19 {
            crate::spell::recompute_vitals(ctx, character_guid);
            crate::spell::recompute_sheet(ctx, character_guid);
        }
        Ok(())
    }

    fn commit_planned(
        self,
        ctx: &ReducerContext,
        source: Option<NewItemSource>,
        restored: bool,
    ) -> Result<(), ActionRefusal> {
        let changed_offer = self.check_trade_offers(ctx)?;
        if ctx
            .db
            .game_item_instance()
            .by_owner_guid()
            .filter(self.character_guid)
            .count()
            != self.original_stacks.len()
        {
            return Err(super::refused(lyracore_shared::item::ItemRefusal::NotRightNow).into());
        }
        for (item, &(slot, count, soulbound)) in self.items.iter().zip(&self.original_stacks) {
            let current = ctx.db.game_item_instance().guid().find(item.guid);
            if !current.is_some_and(|current| {
                current.owner_guid == self.character_guid
                    && current.slot == slot
                    && current.entry == item.entry
                    && current.stack_count == count
                    && current.soulbound == soulbound
                    && current.durability == item.durability
                    && current.enchant_id == item.enchant_id
                    && current.random_property_id == item.random_property_id
                    && current.item_text_id == item.item_text_id
            }) {
                return Err(super::refused(lyracore_shared::item::ItemRefusal::NotRightNow).into());
            }
        }
        let new_guids = self
            .items
            .iter()
            .skip(self.original_stacks.len())
            .filter(|item| item.guid == 0)
            .count();
        let mut guids = allocate_item_guids(ctx, new_guids)?.into_iter();
        for (index, mut item) in self.items.into_iter().enumerate() {
            match self.original_stacks.get(index) {
                Some(&(slot, count, soulbound))
                    if (slot, count, soulbound)
                        != (item.slot, item.stack_count, item.soulbound) =>
                {
                    if item.stack_count == 0 {
                        crate::ItemInstance::delete(ctx, item.guid);
                    } else if restored {
                        item.update_restored(ctx);
                    } else {
                        item.update_as(ctx, source);
                    }
                }
                Some(_) => {}
                None => {
                    if item.guid == 0 {
                        item.guid = guids.next().expect("item plan reserved every new GUID");
                    }
                    if restored {
                        item.insert_restored(ctx);
                    } else {
                        crate::ItemInstance::insert_as(ctx, item, source);
                    }
                }
            }
        }
        if changed_offer {
            crate::trade::refresh_offer(ctx, self.character_guid);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(slot: u8, entry: u32, count: u32) -> ItemInstance {
        ItemInstance {
            guid: u64::from(slot) + 1,
            entry,
            owner_identity: Identity::ZERO,
            owner_guid: 1,
            slot,
            stack_count: count,
            durability: 50,
            created_at: Timestamp::UNIX_EPOCH,
            enchant_id: 0,
            soulbound: false,
            random_property_id: 0,
            item_text_id: 0,
        }
    }

    fn plan(items: Vec<ItemInstance>) -> ItemStoragePlan {
        ItemStoragePlan::new(1, Identity::ZERO, Timestamp::UNIX_EPOCH, items)
    }

    fn grant(
        plan: &mut ItemStoragePlan,
        template: &ItemTemplate,
        count: u32,
    ) -> Result<(), ActionRefusal> {
        plan.plan_grant(template, count, false, None, || Ok(0), |_| 0)
    }

    #[test]
    fn consumption_frees_capacity_for_the_complete_reward_set() {
        let mut plan = plan((23..39).map(|slot| item(slot, 1, 1)).collect());
        plan.consume(1, 2).unwrap();
        grant(&mut plan, &crate::ItemTemplate::blank(2, "reward"), 1).unwrap();
        grant(&mut plan, &crate::ItemTemplate::blank(3, "choice"), 1).unwrap();
        let rewards: Vec<_> = plan
            .items
            .iter()
            .filter(|item| item.entry != 1)
            .map(|item| (item.slot, item.entry, item.stack_count))
            .collect();
        assert_eq!(rewards, vec![(23, 2, 1), (24, 3, 1)]);
        assert!(plan
            .items
            .iter()
            .filter(|item| item.entry == 1 && item.slot < 25)
            .all(|item| item.stack_count == 0));
    }

    #[test]
    fn reward_set_refuses_when_only_the_first_reward_fits() {
        let mut plan = plan((23..39).map(|slot| item(slot, 1, 1)).collect());
        plan.consume(1, 1).unwrap();
        grant(&mut plan, &crate::ItemTemplate::blank(2, "reward"), 1).unwrap();
        let refusal = grant(&mut plan, &crate::ItemTemplate::blank(3, "choice"), 1).unwrap_err();
        assert_eq!(refusal.kind, ActionRefusalKind::InventoryFull);
        assert_eq!(refusal.detail, lyracore_shared::mail::INVENTORY_FULL);
    }

    #[test]
    fn repeated_collect_objectives_cannot_consume_the_same_units_twice() {
        let mut plan = plan(vec![item(23, 1, 5), item(39, 1, 20)]);
        plan.consume(1, 3).unwrap();
        let refusal = plan.consume(1, 3).unwrap_err();
        assert_eq!(refusal.detail, "missing 1 of item 1");
        assert_eq!(plan.items[1].stack_count, 20, "bank copies stay owned");
    }

    #[test]
    fn insufficient_carried_quantity_leaves_every_stack_unchanged() {
        let mut items = vec![item(23, 1, 2), item(24, 2, 3), item(39, 1, 99)];
        let before: Vec<_> = items
            .iter()
            .map(|item| (item.guid, item.slot, item.entry, item.stack_count))
            .collect();
        let refusal = consume_item_stacks(&mut items, 1, 3).unwrap_err();
        assert_eq!(refusal.detail, "missing 1 of item 1");
        let after: Vec<_> = items
            .iter()
            .map(|item| (item.guid, item.slot, item.entry, item.stack_count))
            .collect();
        assert_eq!(after, before);
    }

    #[test]
    fn repeated_rewards_share_partial_stacks_without_exceeding_the_cap() {
        let mut plan = plan(vec![item(23, 1, 8)]);
        let mut template = crate::ItemTemplate::blank(1, "stackable reward");
        template.max_stack = 10;
        template.bonding = super::super::rules::bonding::BIND_ON_PICKUP;
        grant(&mut plan, &template, 7).unwrap();
        grant(&mut plan, &template, 8).unwrap();
        let stacks: Vec<_> = plan
            .items
            .iter()
            .map(|item| (item.slot, item.stack_count))
            .collect();
        assert_eq!(stacks, vec![(23, 10), (24, 10), (25, 3)]);
        assert!(plan.items.iter().all(|item| item.soulbound));
    }

    #[test]
    fn bank_ownership_counts_toward_a_unique_reward_limit() {
        let mut plan = plan(vec![item(39, 1, 1)]);
        let mut template = crate::ItemTemplate::blank(1, "unique reward");
        template.max_count = 1;
        let refusal = grant(&mut plan, &template, 1).unwrap_err();
        assert_eq!(refusal.detail, "unique item limit 1 for item 1");
    }

    #[test]
    fn duplicate_rewards_share_one_unique_limit() {
        let mut plan = plan(Vec::new());
        let mut template = crate::ItemTemplate::blank(1, "unique reward");
        template.max_count = 1;
        grant(&mut plan, &template, 1).unwrap();
        assert!(grant(&mut plan, &template, 1).is_err());
    }

    #[test]
    fn consuming_a_unique_item_allows_its_replacement() {
        let mut plan = plan(vec![item(23, 1, 1)]);
        let mut template = crate::ItemTemplate::blank(1, "unique reward");
        template.max_count = 1;
        plan.consume(1, 1).unwrap();
        grant(&mut plan, &template, 1).unwrap();
        assert_eq!(
            plan.items.iter().map(|item| item.stack_count).sum::<u32>(),
            1
        );
    }

    #[test]
    fn every_new_stack_resolves_its_property_before_a_grant_changes_items() {
        let mut plan = plan(vec![item(23, 1, 8)]);
        let mut template = crate::ItemTemplate::blank(1, "stackable reward");
        template.max_stack = 10;
        let mut properties = [Ok(0), Err("invalid property pool".to_string())].into_iter();
        let refusal = plan
            .plan_grant(
                &template,
                20,
                false,
                None,
                || properties.next().unwrap(),
                |_| 0,
            )
            .unwrap_err();
        assert_eq!(refusal.detail, "invalid property pool");
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].stack_count, 8);
    }

    #[test]
    fn property_mismatch_does_not_fill_an_existing_stack() {
        let mut plan = plan(vec![item(23, 1, 8)]);
        let mut template = crate::ItemTemplate::blank(1, "stackable reward");
        template.max_stack = 10;
        plan.plan_grant(&template, 5, true, Some(7), || unreachable!(), |_| 0)
            .unwrap();
        assert_eq!(plan.items[0].stack_count, 8);
        assert_eq!(plan.items[1].stack_count, 5);
        assert_eq!(plan.items[1].random_property_id, 7);
        assert!(plan.items[1].soulbound);
    }

    #[test]
    fn equipped_bag_slots_follow_a_full_backpack() {
        let mut owned: Vec<_> = (23..39).map(|slot| item(slot, 1, 1)).collect();
        owned.push(item(19, 2, 1));
        owned.push(item(120, 3, 1));
        let mut plan = plan(owned);
        plan.plan_grant(
            &crate::ItemTemplate::blank(4, "reward"),
            1,
            false,
            None,
            || Ok(0),
            |entry| {
                assert_eq!(entry, 2);
                2
            },
        )
        .unwrap();
        assert_eq!(plan.items.last().unwrap().slot, 121);
    }
}
