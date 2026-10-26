use spacetimedb::{table, Identity, ReducerContext, Table, Timestamp};

// Playable class IDs skip 6 and 10; Druid is bit 10.
pub const ALL_PLAYABLE_CLASS_MASK: u32 = 0x5df;
pub const ALL_PLAYABLE_RACE_MASK: u32 = 0xff;

#[table(accessor = game_item_template, public)]
pub struct ItemTemplate {
    #[primary_key]
    pub entry: u32,
    pub class: u8,
    pub subclass: u8,
    pub name: String,
    pub display_id: u32,
    pub quality: u8,
    pub inventory_type: u8,
    pub item_level: u8,
    pub required_level: u8,
    pub max_durability: u32,
    pub buy_price: u32,  // Copper.
    pub sell_price: u32, // Copper.
    pub max_stack: u32,
    pub damage_min: f32,
    pub damage_max: f32,
    pub delay_ms: u32,

    #[default(0)]
    pub stat_strength: i32,
    #[default(0)]
    pub stat_agility: i32,
    #[default(0)]
    pub stat_stamina: i32,
    #[default(0)]
    pub stat_intellect: i32,
    #[default(0)]
    pub stat_spirit: i32,
    #[default(0)]
    pub stat_crit: i32, // Attack-table basis points.
    #[default(0)]
    pub stat_hit: i32, // Attack-table basis points.
    #[default(0)]
    pub stat_armor: i32,
    // Item contribution only; combat::effective_block_value adds Strength.
    #[default(0)]
    pub block_value: i32,
    #[default(false)]
    pub restores_power: bool,
    // Spell triggers: 0 = on use, 1 = on equip, 2 = chance on hit.
    #[default(0)]
    pub spellid_1: u32,
    #[default(0)]
    pub spelltrigger_1: u8,
    #[default(0)]
    pub spellid_2: u32,
    #[default(0)]
    pub spelltrigger_2: u8,
    #[default(0)]
    pub container_slots: u8,
    // Preserve raw sheath values; gtker variant names do not describe the rendered posture.
    #[default(0)]
    pub sheath: u8,
    #[default(0)]
    pub bonding: u8,
    #[default(0)]
    pub holy_res: i32,
    #[default(0)]
    pub fire_res: i32,
    #[default(0)]
    pub nature_res: i32,
    #[default(0)]
    pub frost_res: i32,
    #[default(0)]
    pub shadow_res: i32,
    #[default(0)]
    pub arcane_res: i32,
    #[default(0)]
    pub spellid_3: u32,
    #[default(0)]
    pub spelltrigger_3: u8,
    #[default(0)]
    pub spellid_4: u32,
    #[default(0)]
    pub spelltrigger_4: u8,
    #[default(0)]
    pub spellid_5: u32,
    #[default(0)]
    pub spelltrigger_5: u8,
    #[default(0)]
    pub required_skill: u32,
    #[default(0)]
    pub required_skill_rank: u32,
    #[default(0)]
    pub required_reputation_faction: u32,
    #[default(0)]
    pub required_reputation_rank: u32,
    // Zero means unlimited; the cap includes carried and banked copies.
    #[default(0)]
    pub max_count: u32,
    #[default(0)]
    pub item_flags: u32,
    #[default(0)]
    pub page_text: u32,
    #[default(0)]
    pub start_quest: u32,
    #[default(0)]
    pub bag_family: u32,
    #[default(1u32)]
    pub buy_count: u32,
    // Zero means no pet food; 1..=8 select a bit in pet_food_mask.
    #[default(0)]
    pub food_type: u8,
    // Import maps the source -1 sentinel to playable masks; durable zero remains restrictive.
    #[default(ALL_PLAYABLE_CLASS_MASK)]
    pub allowed_class: u32,
    #[default(ALL_PLAYABLE_RACE_MASK)]
    pub allowed_race: u32,
    #[default(0)]
    pub random_property: u32,
}

#[table(accessor = game_item_instance, public, index(accessor = by_owner_guid, btree(columns = [owner_guid])))]
pub struct ItemInstance {
    #[primary_key]
    pub guid: u64,
    pub entry: u32,
    pub owner_identity: Identity,
    pub owner_guid: u64,
    pub slot: u8,
    pub stack_count: u32,
    pub durability: u32,
    pub created_at: Timestamp,
    #[default(0)]
    pub enchant_id: u32,
    #[default(false)]
    pub soulbound: bool,
    #[default(0)]
    pub random_property_id: u32,
    // ITEM_FIELD_ITEM_TEXT_ID; zero except on a Letter Copy.
    #[default(0u32)]
    pub item_text_id: u32,
}

impl ItemInstance {
    pub(crate) fn insert(ctx: &ReducerContext, row: Self) -> Self {
        Self::insert_as(ctx, row, true)
    }

    /// Inserts a stack split off the character's own, without the item push or quest toast.
    pub(crate) fn insert_split(ctx: &ReducerContext, row: Self) -> Self {
        Self::insert_as(ctx, row, false)
    }

    fn insert_as(ctx: &ReducerContext, row: Self, received: bool) -> Self {
        let row = ctx.db.game_item_instance().insert(row);
        crate::native::item_changed(ctx, None, Some(&row), received);
        row
    }

    pub(crate) fn update(self, ctx: &ReducerContext) -> Self {
        self.update_as(ctx, true)
    }

    /// Updates a stack merged from the character's own, without the item push or quest toast.
    pub(crate) fn update_merged(self, ctx: &ReducerContext) -> Self {
        self.update_as(ctx, false)
    }

    fn update_as(self, ctx: &ReducerContext, received: bool) -> Self {
        let items = ctx.db.game_item_instance();
        let previous = items.guid().find(self.guid);
        let row = items.guid().update(self);
        crate::native::item_changed(ctx, previous.as_ref(), Some(&row), received);
        row
    }

    pub(crate) fn delete(ctx: &ReducerContext, guid: u64) -> bool {
        let items = ctx.db.game_item_instance();
        let previous = items.guid().find(guid);
        let removed = items.guid().delete(guid);
        if let Some(row) = previous {
            crate::native::item_changed(ctx, Some(&row), None, false);
        }
        removed
    }
}

crate::character_owned!(delete, fn sweep_delete_game_item_instance(ctx, character_guid) {
    let items = ctx.db.game_item_instance();
    for r in items.by_owner_guid().filter(&character_guid).collect::<Vec<_>>() {
        crate::ItemInstance::delete(ctx, r.guid);
    }
});
// Transfer preserves item GUIDs; legacy formats still need collision checks.
crate::character_owned!(transfer, fn sweep_transfer_game_item_instance(ctx, character_guid, io) {
    table = game_item_instance,
    by = by_owner_guid,
    keep_key,
});
crate::character_owned!(restamp, fn sweep_restamp_game_item_instance(ctx, character_guid, identity) {
    let items = ctx.db.game_item_instance();
    for mut r in items.by_owner_guid().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            r.update(ctx);
        }
    }
});

pub(crate) fn item_in_slot(
    ctx: &ReducerContext,
    owner_guid: u64,
    slot: u8,
) -> Option<ItemInstance> {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&owner_guid)
        .find(|i| i.slot == slot)
}

pub(crate) fn slot_occupied(ctx: &ReducerContext, owner_guid: u64, slot: u8) -> bool {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&owner_guid)
        .any(|i| i.slot == slot)
}

pub(crate) fn item_is_broken(tmpl: &ItemTemplate, inst: &ItemInstance) -> bool {
    tmpl.max_durability > 0 && inst.durability == 0
}

#[table(accessor = game_npc_vendor, public, index(accessor = by_vendor, btree(columns = [creature_entry])))]
pub struct NpcVendor {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub item_entry: u32,
    pub slot: u32,
    pub max_count: u32, // Zero means unlimited stock.
}

#[table(accessor = game_character_buyback, index(accessor = by_player_guid, btree(columns = [player_guid])))]
pub struct BuybackEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub player_guid: u64,
    pub item_entry: u32,
    pub stack_count: u32,
    pub price: u32, // Copper paid at sell time.
    // Buyback must preserve an already-bound BoE item independently of template binding.
    #[default(false)]
    pub soulbound: bool,
    #[default(0)]
    pub random_property_id: u32,
}

crate::character_owned!(delete, fn sweep_delete_game_character_buyback(ctx, character_guid) {
    let buybacks = ctx.db.game_character_buyback();
    for r in buybacks.by_player_guid().filter(&character_guid).collect::<Vec<_>>() {
        buybacks.id().delete(r.id);
    }
});
// Re-minted buyback IDs must preserve export order because IDs order the ring.
crate::character_owned!(transfer, fn sweep_transfer_game_character_buyback(ctx, character_guid, io) {
    table = game_character_buyback,
    by = by_player_guid,
    remint = id,
});
