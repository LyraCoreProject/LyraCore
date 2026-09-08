//! Plan item consumption and grants together before changing the Character's inventory.

use crate::actor::{ActionRefusal, ActionRefusalKind};
use spacetimedb::{Identity, ReducerContext, Table, Timestamp};

use super::inventory::{free_inventory_slots, is_carried_slot};
use super::{
    allocate_item_guids, binds_on_grant, game_item_instance, game_item_template, merge_amount,
    select_property, ItemInstance, ItemTemplate,
};

/// Quest collect items and all rewards form one exchange. Every Refusal precedes item changes;
/// GUID reservation may consume unused identities, but never returns them to the range.
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

/// Check the complete carried quantity before changing any stack. Bank copies stay untouched.
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

/// Existing rows retain their identities. New rows receive GUIDs only after the full plan passes.
pub(super) struct ItemStoragePlan {
    character_guid: u64,
    owner_identity: Identity,
    created_at: Timestamp,
    items: Vec<ItemInstance>,
    original_stacks: Vec<(u32, bool)>,
}

impl ItemStoragePlan {
    pub(super) fn read(
        ctx: &ReducerContext,
        character_guid: u64,
        owner_identity: Identity,
    ) -> Self {
        Self::new(
            character_guid,
            owner_identity,
            ctx.timestamp,
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
            .map(|item| (item.stack_count, item.soulbound))
            .collect();
        Self {
            character_guid,
            owner_identity,
            created_at,
            items,
            original_stacks,
        }
    }

    fn consume(&mut self, entry: u32, count: u32) -> Result<(), ActionRefusal> {
        consume_item_stacks(&mut self.items, entry, count)
    }

    pub(super) fn grant(
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
            let item = &mut self.items[index];
            let add = merge_amount(count, item.stack_count, max_stack);
            item.stack_count += add;
            item.soulbound |= soulbound;
            count -= add;
            if count == 0 {
                break;
            }
        }
        for (slot, random_property_id) in slots.into_iter().zip(properties) {
            let take = count.min(max_stack);
            self.items.push(ItemInstance {
                guid: 0,
                entry: template.entry,
                owner_identity: self.owner_identity,
                owner_guid: self.character_guid,
                slot,
                stack_count: take,
                durability: template.max_durability,
                created_at: self.created_at,
                enchant_id: 0,
                soulbound,
                random_property_id,
            });
            count -= take;
        }
        Ok(())
    }

    pub(super) fn commit(self, ctx: &ReducerContext) -> Result<(), ActionRefusal> {
        let mut guids =
            allocate_item_guids(ctx, self.items.len() - self.original_stacks.len())?.into_iter();
        let instances = ctx.db.game_item_instance();
        for (index, mut item) in self.items.into_iter().enumerate() {
            match self.original_stacks.get(index) {
                Some(&(count, soulbound))
                    if (count, soulbound) != (item.stack_count, item.soulbound) =>
                {
                    if item.stack_count == 0 {
                        instances.guid().delete(item.guid);
                    } else {
                        instances.guid().update(item);
                    }
                }
                Some(_) => {}
                None => {
                    // The reservation above issued one GUID for every appended item.
                    item.guid = guids.next().expect("item plan reserved every new GUID");
                    instances.insert(item);
                }
            }
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
        grant(&mut plan, &crate::seed::base_item(2, "reward"), 1).unwrap();
        grant(&mut plan, &crate::seed::base_item(3, "choice"), 1).unwrap();
        let rewards: Vec<_> = plan
            .items
            .iter()
            .filter(|item| item.entry != 1)
            .map(|item| (item.slot, item.entry, item.stack_count))
            .collect();
        assert_eq!(rewards, vec![(23, 2, 1), (24, 3, 1)]);
    }

    #[test]
    fn reward_set_refuses_when_only_the_first_reward_fits() {
        let mut plan = plan((23..39).map(|slot| item(slot, 1, 1)).collect());
        plan.consume(1, 1).unwrap();
        grant(&mut plan, &crate::seed::base_item(2, "reward"), 1).unwrap();
        let refusal = grant(&mut plan, &crate::seed::base_item(3, "choice"), 1).unwrap_err();
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
        let mut template = crate::seed::base_item(1, "stackable reward");
        template.max_stack = 10;
        grant(&mut plan, &template, 7).unwrap();
        grant(&mut plan, &template, 8).unwrap();
        let stacks: Vec<_> = plan
            .items
            .iter()
            .map(|item| (item.slot, item.stack_count))
            .collect();
        assert_eq!(stacks, vec![(23, 10), (24, 10), (25, 3)]);
    }

    #[test]
    fn bank_ownership_counts_toward_a_unique_reward_limit() {
        let mut plan = plan(vec![item(39, 1, 1)]);
        let mut template = crate::seed::base_item(1, "unique reward");
        template.max_count = 1;
        let refusal = grant(&mut plan, &template, 1).unwrap_err();
        assert_eq!(refusal.detail, "unique item limit 1 for item 1");
    }

    #[test]
    fn duplicate_rewards_share_one_unique_limit() {
        let mut plan = plan(Vec::new());
        let mut template = crate::seed::base_item(1, "unique reward");
        template.max_count = 1;
        grant(&mut plan, &template, 1).unwrap();
        assert!(grant(&mut plan, &template, 1).is_err());
    }

    #[test]
    fn consuming_a_unique_item_allows_its_replacement() {
        let mut plan = plan(vec![item(23, 1, 1)]);
        let mut template = crate::seed::base_item(1, "unique reward");
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
        let mut template = crate::seed::base_item(1, "stackable reward");
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
        let mut template = crate::seed::base_item(1, "stackable reward");
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
            &crate::seed::base_item(4, "reward"),
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
