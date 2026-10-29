use std::collections::BTreeSet;

use lyracore_package_delta::ids::{
    is_fixture_reserved_item_id, is_operator_template_id, is_package_item_id,
    OPERATOR_TEMPLATE_ID_CEIL, OPERATOR_TEMPLATE_ID_FLOOR,
};
use lyracore_protocol::messages::vanilla::{
    Bonding, InventoryType, ItemClassAndSubClass, ItemQuality, SheatheType, SpellTriggerType,
};
use spacetimedb::{reducer, ReducerContext, SpacetimeType, Table};

use crate::items::{
    game_item_instance, game_item_template, game_npc_vendor, ItemTemplate, NpcVendor,
};
use crate::native::game_native_player;
use crate::{game_creature_template, game_spell};

const MAX_ITEM_EDITS: usize = 64;
const MAX_VENDOR_OFFERS: usize = 128;
const MAX_BATCH_OFFERS: usize = 512;
const MAX_ITEM_NAME_BYTES: usize = 255;

// The exhaustive constructor makes a new ItemTemplate column a compile failure here.
macro_rules! item_fields {
    ($($field:ident: $ty:ty),* $(,)?) => {
        #[derive(SpacetimeType, Clone, Default)]
        pub struct ItemFields {
            $(pub $field: $ty,)*
        }

        impl ItemFields {
            fn into_template(self, entry: u32) -> ItemTemplate {
                ItemTemplate { entry, $($field: self.$field,)* }
            }

            fn of_template(template: &ItemTemplate) -> Self {
                ItemFields { $($field: template.$field.clone(),)* }
            }

            fn changes(&self, old: Option<&ItemTemplate>) -> String {
                let mut changes = Vec::new();
                $(if old.is_none_or(|old| old.$field != self.$field) {
                    changes.push(format!("{} {:?} -> {:?}",
                        stringify!($field), old.map(|old| &old.$field), self.$field));
                })*
                changes.join(", ")
            }
        }
    };
}

item_fields! {
    class: u8,
    subclass: u8,
    name: String,
    display_id: u32,
    quality: u8,
    inventory_type: u8,
    item_level: u8,
    required_level: u8,
    max_durability: u32,
    buy_price: u32,
    sell_price: u32,
    max_stack: u32,
    damage_min: f32,
    damage_max: f32,
    delay_ms: u32,
    stat_strength: i32,
    stat_agility: i32,
    stat_stamina: i32,
    stat_intellect: i32,
    stat_spirit: i32,
    stat_crit: i32,
    stat_hit: i32,
    stat_armor: i32,
    block_value: i32,
    restores_power: bool,
    spellid_1: u32,
    spelltrigger_1: u8,
    spellid_2: u32,
    spelltrigger_2: u8,
    container_slots: u8,
    sheath: u8,
    bonding: u8,
    holy_res: i32,
    fire_res: i32,
    nature_res: i32,
    frost_res: i32,
    shadow_res: i32,
    arcane_res: i32,
    spellid_3: u32,
    spelltrigger_3: u8,
    spellid_4: u32,
    spelltrigger_4: u8,
    spellid_5: u32,
    spelltrigger_5: u8,
    required_skill: u32,
    required_skill_rank: u32,
    required_reputation_faction: u32,
    required_reputation_rank: u32,
    max_count: u32,
    item_flags: u32,
    page_text: u32,
    start_quest: u32,
    bag_family: u32,
    buy_count: u32,
    food_type: u8,
    allowed_class: u32,
    allowed_race: u32,
    random_property: u32,
}

#[derive(SpacetimeType, Clone)]
pub struct ItemCreate {
    pub fields: ItemFields,
}

#[derive(SpacetimeType, Clone)]
pub struct ItemUpdate {
    pub entry: u32,
    pub fields: ItemFields,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct VendorOffer {
    pub item_entry: u32,
    pub max_count: u32,
}

#[derive(SpacetimeType, Clone)]
pub struct VendorOffers {
    pub creature_entry: u32,
    pub offers: Vec<VendorOffer>,
}

#[derive(SpacetimeType, Clone)]
pub enum ItemEdit {
    CreateItem(ItemCreate),
    UpdateItem(ItemUpdate),
    SetVendorOffers(VendorOffers),
}

/// Applies edits in order; any Refusal rolls back rows, stat refreshes and queued packets.
#[reducer]
pub fn operator_apply_item_edits(ctx: &ReducerContext, edits: Vec<ItemEdit>) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    check_batch(&edits)?;
    let mut ids = ItemIds::default();
    let mut changed = BTreeSet::new();
    let mut updated = BTreeSet::new();
    let mut changes = Vec::with_capacity(edits.len());
    for (index, edit) in edits.into_iter().enumerate() {
        let change = apply(ctx, &mut ids, edit, &mut changed, &mut updated)
            .map_err(|reason| format!("item edit {index}: {reason}"))?;
        changes.push(change);
    }
    if !updated.is_empty() {
        for connected in ctx.db.game_native_player().iter() {
            let guid = connected.character_guid;
            if ctx
                .db
                .game_item_instance()
                .by_owner_guid()
                .filter(guid)
                .any(|item| {
                    item.slot <= crate::items::equip_slot::END && updated.contains(&item.entry)
                })
            {
                crate::spell::recompute_vitals(ctx, guid);
                crate::spell::recompute_sheet(ctx, guid);
            }
        }
    }
    for entry in changed {
        crate::native::broadcast_item_query(ctx, entry)?;
    }
    for change in changes {
        super::log_change(ctx, &change);
    }
    Ok(())
}

fn check_batch(edits: &[ItemEdit]) -> Result<(), String> {
    if edits.len() > MAX_ITEM_EDITS {
        return Err(format!(
            "a batch holds at most {MAX_ITEM_EDITS} item edits, got {}",
            edits.len()
        ));
    }
    let offers: usize = edits
        .iter()
        .map(|edit| match edit {
            ItemEdit::SetVendorOffers(vendor) => vendor.offers.len(),
            _ => 0,
        })
        .sum();
    if offers > MAX_BATCH_OFFERS {
        return Err(format!(
            "a batch holds at most {MAX_BATCH_OFFERS} vendor offers, got {offers}"
        ));
    }
    Ok(())
}

#[derive(Default)]
struct ItemIds {
    item: Option<u32>,
    vendor: Option<u64>,
}

impl ItemIds {
    fn next_item(&mut self, ctx: &ReducerContext) -> Result<u32, String> {
        let last = *self.item.get_or_insert_with(|| {
            ctx.db
                .game_item_template()
                .iter()
                .map(|item| item.entry)
                .filter(|&entry| is_operator_template_id(entry))
                .max()
                .unwrap_or(OPERATOR_TEMPLATE_ID_FLOOR - 1)
        });
        let next = next_template_id(last)?;
        self.item = Some(next);
        Ok(next)
    }

    fn next_vendor(&mut self, ctx: &ReducerContext) -> Result<u64, String> {
        let last = *self.vendor.get_or_insert_with(|| {
            ctx.db
                .game_npc_vendor()
                .iter()
                .map(|offer| offer.id)
                .max()
                .unwrap_or(0)
        });
        let next = last.checked_add(1).ok_or("vendor id exhausted")?;
        self.vendor = Some(next);
        Ok(next)
    }
}

fn next_template_id(last: u32) -> Result<u32, String> {
    let next = last.saturating_add(1);
    if next > OPERATOR_TEMPLATE_ID_CEIL {
        return Err(format!("the Operator Template Range ({OPERATOR_TEMPLATE_ID_FLOOR} to {OPERATOR_TEMPLATE_ID_CEIL}) is exhausted"));
    }
    Ok(next)
}

fn apply(
    ctx: &ReducerContext,
    ids: &mut ItemIds,
    edit: ItemEdit,
    changed: &mut BTreeSet<u32>,
    updated: &mut BTreeSet<u32>,
) -> Result<String, String> {
    match edit {
        ItemEdit::CreateItem(create) => {
            check_fields(ctx, &create.fields, None)?;
            let entry = ids.next_item(ctx)?;
            let changes = create.fields.changes(None);
            ctx.db
                .game_item_template()
                .try_insert(create.fields.into_template(entry))
                .map_err(|reason| format!("item {entry} cannot be inserted: {reason}"))?;
            changed.insert(entry);
            Ok(format!("create item {entry}: {changes}"))
        }
        ItemEdit::UpdateItem(update) => {
            let entry = update.entry;
            if is_package_item_id(entry) {
                return Err(format!("item {entry} belongs to the Package Item Range"));
            }
            if is_fixture_reserved_item_id(entry) {
                return Err(format!("item {entry} is Fixture-Reserved"));
            }
            let old = ctx
                .db
                .game_item_template()
                .entry()
                .find(entry)
                .ok_or_else(|| format!("item {entry} does not exist"))?;
            check_fields(ctx, &update.fields, Some(&ItemFields::of_template(&old)))?;
            let changes = update.fields.changes(Some(&old));
            ctx.db
                .game_item_template()
                .entry()
                .update(update.fields.into_template(entry));
            changed.insert(entry);
            updated.insert(entry);
            Ok(format!("update item {entry}: {changes}"))
        }
        ItemEdit::SetVendorOffers(vendor) => replace_vendor(ctx, ids, vendor),
    }
}

fn replace_vendor(
    ctx: &ReducerContext,
    ids: &mut ItemIds,
    vendor: VendorOffers,
) -> Result<String, String> {
    let entry = vendor.creature_entry;
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(entry)
        .is_none()
    {
        return Err(format!("creature template {entry} does not exist"));
    }
    if vendor.offers.len() > MAX_VENDOR_OFFERS {
        return Err(format!("a vendor holds at most {MAX_VENDOR_OFFERS} offers"));
    }
    let mut seen = BTreeSet::new();
    for offer in &vendor.offers {
        if !seen.insert(offer.item_entry) {
            return Err(format!("duplicate vendor item {}", offer.item_entry));
        }
        if ctx
            .db
            .game_item_template()
            .entry()
            .find(offer.item_entry)
            .is_none()
        {
            return Err(format!("item {} does not exist", offer.item_entry));
        }
    }
    let table = ctx.db.game_npc_vendor();
    let mut old: Vec<_> = table.by_vendor().filter(entry).collect();
    old.sort_by_key(|row| (row.slot, row.id));
    // Allocate before deleting: imported explicit ids do not advance auto_inc.
    let rows: Vec<_> = vendor
        .offers
        .iter()
        .enumerate()
        .map(|(slot, offer)| {
            Ok(NpcVendor {
                id: ids.next_vendor(ctx)?,
                creature_entry: entry,
                item_entry: offer.item_entry,
                slot: slot as u32,
                max_count: offer.max_count,
            })
        })
        .collect::<Result<_, String>>()?;
    for row in &old {
        table.id().delete(row.id);
    }
    for row in rows {
        table
            .try_insert(row)
            .map_err(|reason| format!("vendor {entry} offer cannot be inserted: {reason}"))?;
    }
    let old: Vec<_> = old
        .into_iter()
        .map(|row| VendorOffer {
            item_entry: row.item_entry,
            max_count: row.max_count,
        })
        .collect();
    Ok(format!(
        "vendor {entry} offers {old:?} -> {:?}",
        vendor.offers
    ))
}

/// Refuses a value an edit introduces. A value the row already holds passes, so an edit of one column
/// is not refused for an imported value elsewhere in the row.
fn check_fields(
    ctx: &ReducerContext,
    fields: &ItemFields,
    old: Option<&ItemFields>,
) -> Result<(), String> {
    let held = old.map(violations).unwrap_or_default();
    if let Some(violation) = violations(fields)
        .into_iter()
        .find(|violation| !held.contains(violation))
    {
        return Err(violation);
    }
    let spells = |fields: &ItemFields| {
        [
            fields.spellid_1,
            fields.spellid_2,
            fields.spellid_3,
            fields.spellid_4,
            fields.spellid_5,
        ]
    };
    let held = old.map(spells).unwrap_or_default();
    for (spell, held) in spells(fields).into_iter().zip(held) {
        if spell != 0 && spell != held && ctx.db.game_spell().spell_id().find(spell).is_none() {
            return Err(format!("spell {spell} does not exist"));
        }
    }
    Ok(())
}

#[cfg(test)]
fn check_values(fields: &ItemFields) -> Result<(), String> {
    violations(fields).into_iter().next().map_or(Ok(()), Err)
}

fn violations(fields: &ItemFields) -> Vec<String> {
    let mut out = Vec::new();
    if fields.name.trim().is_empty()
        || fields.name.len() > MAX_ITEM_NAME_BYTES
        || fields.name.contains('\0')
    {
        out.push(format!(
            "name must contain text, no NUL, and at most {MAX_ITEM_NAME_BYTES} UTF-8 bytes"
        ));
    }
    if !fields.damage_min.is_finite()
        || !fields.damage_max.is_finite()
        || fields.damage_min < 0.0
        || fields.damage_min > fields.damage_max
    {
        out.push("damage must be finite with 0 <= damage_min <= damage_max".into());
    }
    if ItemClassAndSubClass::try_from((u64::from(fields.subclass) << 32) | u64::from(fields.class))
        .is_err()
    {
        out.push(format!(
            "unsupported class/subclass {}/{}",
            fields.class, fields.subclass
        ));
    }
    for (name, valid) in [
        (
            "quality",
            ItemQuality::try_from(u32::from(fields.quality)).is_ok(),
        ),
        (
            "inventory_type",
            InventoryType::try_from(u32::from(fields.inventory_type)).is_ok(),
        ),
        (
            "bonding",
            Bonding::try_from(u32::from(fields.bonding)).is_ok(),
        ),
        ("sheath", SheatheType::try_from(fields.sheath).is_ok()),
        ("food_type", fields.food_type <= 8),
        (
            "required_reputation_rank",
            fields.required_reputation_rank <= 7,
        ),
        ("max_stack", fields.max_stack > 0),
        ("buy_count", fields.buy_count > 0),
    ] {
        if !valid {
            out.push(format!("unsupported {name}"));
        }
    }
    for (index, trigger) in [
        fields.spelltrigger_1,
        fields.spelltrigger_2,
        fields.spelltrigger_3,
        fields.spelltrigger_4,
        fields.spelltrigger_5,
    ]
    .into_iter()
    .enumerate()
    {
        if SpellTriggerType::try_from(u32::from(trigger)).is_err() {
            out.push(format!("unsupported spelltrigger_{}: {trigger}", index + 1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> ItemFields {
        ItemFields {
            name: "Test item".into(),
            max_stack: 1,
            buy_count: 1,
            ..Default::default()
        }
    }

    #[test]
    fn allocation_starts_in_range_and_refuses_exhaustion() {
        assert_eq!(
            next_template_id(OPERATOR_TEMPLATE_ID_FLOOR - 1),
            Ok(5_200_500)
        );
        assert_eq!(next_template_id(5_200_541), Ok(5_200_542));
        assert!(next_template_id(OPERATOR_TEMPLATE_ID_CEIL).is_err());
    }

    #[test]
    fn item_fields_refuse_invalid_wire_values_and_damage() {
        assert!(check_values(&fields()).is_ok());
        let cases = [
            ItemFields {
                name: " ".into(),
                ..fields()
            },
            ItemFields {
                name: "bad\0name".into(),
                ..fields()
            },
            ItemFields {
                name: "x".repeat(256),
                ..fields()
            },
            ItemFields {
                damage_min: f32::NAN,
                ..fields()
            },
            ItemFields {
                damage_max: f32::INFINITY,
                ..fields()
            },
            ItemFields {
                damage_min: 2.0,
                damage_max: 1.0,
                ..fields()
            },
            ItemFields {
                class: 16,
                ..fields()
            },
            ItemFields {
                subclass: 99,
                ..fields()
            },
            ItemFields {
                quality: 8,
                ..fields()
            },
            ItemFields {
                inventory_type: 29,
                ..fields()
            },
            ItemFields {
                bonding: 6,
                ..fields()
            },
            ItemFields {
                sheath: 8,
                ..fields()
            },
            ItemFields {
                spelltrigger_5: 3,
                ..fields()
            },
            ItemFields {
                food_type: 9,
                ..fields()
            },
            ItemFields {
                required_reputation_rank: 8,
                ..fields()
            },
            ItemFields {
                max_stack: 0,
                ..fields()
            },
            ItemFields {
                buy_count: 0,
                ..fields()
            },
        ];
        for fields in cases {
            assert!(check_values(&fields).is_err());
        }
    }

    #[test]
    fn a_value_the_row_already_holds_is_not_a_new_violation() {
        let imported = ItemFields {
            sheath: 8,
            ..fields()
        };
        let renamed = ItemFields {
            name: "Renamed".into(),
            ..imported.clone()
        };
        let held = violations(&imported);
        assert!(violations(&renamed).iter().all(|v| held.contains(v)));
        let broken = ItemFields {
            quality: 8,
            ..renamed
        };
        assert!(violations(&broken).iter().any(|v| !held.contains(v)));
    }

    #[test]
    fn update_log_contains_only_changed_columns_and_escapes_newlines() {
        let old = fields().into_template(25);
        let fields = ItemFields {
            name: "Changed\nname".into(),
            ..fields()
        };
        assert_eq!(
            fields.changes(Some(&old)),
            "name Some(\"Test item\") -> \"Changed\\nname\""
        );
    }
}
