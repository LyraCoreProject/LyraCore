//! Item table definitions + RLS filter + the guid/slot primitives. The `game_item_template` static
//! definition, the per-player `game_item_instance` owned-item row, the owner-scoped RLS filter, and the
//! small pure-ish primitives every op builds on: guid composition/minting and the per-slot scans.

use spacetimedb::{table, Identity, ReducerContext, Table, Timestamp};

use lyracore_shared::constants::starter_item;

/// The nine class bits playable by the 1.12 client.  Class ids skip 6 and 10, so Druid (11) is bit
/// 0x400; the client hides the tooltip "Classes:" line only when every playable bit is set.
/// Item-template masks retain unknown bits from imported content; this value exists only to make
/// the source dump's unrestricted sentinel durable.
pub const ALL_PLAYABLE_CLASS_MASK: u32 = 0x5df;
/// The eight race bits playable by the 1.12 client.  As with class masks, imported restrictions are
/// otherwise opaque `u32` values and are never reconstructed from local variants.
pub const ALL_PLAYABLE_RACE_MASK: u32 = 0xff;

/// Static item definition (the cmangos `item_template` analogue) — hand-authored, public + read-only
/// to clients (answered via `CMSG_ITEM_QUERY_SINGLE`). A small subset of the ~130 vanilla
/// `item_template` columns: only the fields the slice's query response + bag render need. [reference]
#[table(accessor = game_item_template, public)]
pub struct ItemTemplate {
    #[primary_key]
    pub entry: u32,
    pub class: u8,    // item class — 2 = Weapon
    pub subclass: u8, // weapon subclass — 7 = Sword (one-hand)
    pub name: String,
    pub display_id: u32,    // inventory-icon / model display id
    pub quality: u8,        // 0 = Poor (grey)
    pub inventory_type: u8, // equip slot type — 21 = INVTYPE_WEAPONMAINHAND
    pub item_level: u8,
    pub required_level: u8,
    pub max_durability: u32,
    pub buy_price: u32,  // copper
    pub sell_price: u32, // copper
    pub max_stack: u32,  // 1 for a non-stackable weapon
    pub damage_min: f32, // physical melee damage range (tooltip)
    pub damage_max: f32,
    pub delay_ms: u32, // weapon swing speed (ms)
    // --- equip stat bonuses (END-appended, #[default(0)] → additive auto-migration) ---
    // Vanilla items carry up to 10 (stat_type, stat_value) pairs plus an armor value; this slice flattens
    // the handful that the EXISTING effective-* combat pipeline can fold into observable readouts into one
    // typed column each (the union-killed form, like the spell module's typed `p0`/`amount`). A piece with no
    // bonus leaves every column 0, so an existing template (e.g. the starter Worn Shortsword, entry 25)
    // is byte-identical after migration and the player's swing readout is unchanged (baseline-safe). The
    // five base attributes match the `UNIT_FIELD_STAT` order; `stat_crit`/`stat_hit` are rating points
    // (basis-point-ish, folded straight into the attack-table crit/miss bands); `stat_armor` adds on TOP
    // of the agility*2 base armor (the combat module ADDS, never double-counts). i32 so a (future) negative
    // suffix-enchant fits; today every seeded value is ≥ 0. [reference]
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
    pub stat_crit: i32, // crit RATING in attack-table basis points (CRIT_BP += this)
    #[default(0)]
    pub stat_hit: i32, // hit RATING in attack-table basis points (miss band -= this)
    #[default(0)]
    pub stat_armor: i32, // bonus armor, added ON TOP of the agility*2 base (not double-counted)
    /// SHIELD base block value: the flat physical damage a blocked melee swing absorbs (item-only — the
    /// Strength term is added live in `combat::effective_block_value`). 0 for every non-shield template,
    /// so a unit without a shield blocks nothing. END-appended + `#[default(0)]` → additive auto-migration.
    #[default(0)]
    pub block_value: i32,
    /// A DRINK: this consumable restores POWER (mana) instead of HEALTH when used. Vanilla splits
    /// food (E_HEAL) from drink (E_ENERGIZE) by the on-use spell, which we don't import — so the
    /// importer derives this from the item's class/subclass + name. `apply_item_use` reads it to pick
    /// health vs power; the restore AMOUNT reuses the same `consumable_heal(item_level)` formula.
    /// END-appended + `#[default(false)]` → additive auto-migration (existing food rows stay food). [reference]
    #[default(false)]
    pub restores_power: bool,
    /// On-use / on-equip spell slots 1-2 (vanilla items carry up to 5; slots 1-2 cover every 1-10
    /// consumable — food/drink/potion/scroll on-use + the common on-equip proc). Sent in
    /// SMSG_ITEM_QUERY_SINGLE_RESPONSE so the CLIENT renders the green "Use:/Equip:/Chance on hit:" text
    /// from its OWN Spell.dbc — we don't need the spell server-side (`restores_power` still drives the
    /// actual food/drink effect). `spelltrigger` is the vanilla ItemSpellTriggerType (0=on-use, 1=on-equip,
    /// 2=chance-on-hit). END-appended + `#[default(0)]` → additive auto-migration. Deliberate
    /// simplification: add slots 3-5 only if an on-equip trinket at 1-10 needs them. [reference]
    #[default(0)]
    pub spellid_1: u32,
    #[default(0)]
    pub spelltrigger_1: u8,
    #[default(0)]
    pub spellid_2: u32,
    #[default(0)]
    pub spelltrigger_2: u8,
    /// Number of container slots this item provides when equipped in a bag slot (19..=22).
    /// 0 = not a bag (every existing template → additive auto-migration baseline-safe). A bag with
    /// N slots expands the player's inventory by N: `store_item` searches these after the 16-slot
    /// backpack is full (see `first_free_bag_slot` in ops.rs). Vanilla max is 18 (Portable Hole).
    /// END-appended + `#[default(0)]` → additive auto-migration. [reference]
    #[default(0)]
    pub container_slots: u8,
    /// Vanilla item_template.sheath — where the item visually stows when sheathed. Read by the
    /// gateway item view and passed VERBATIM to `SMSG_ITEM_QUERY_SINGLE_RESPONSE.sheathe_type`
    /// (cmangos sends `pProto->Sheath` the same way). The byte is an opaque client posture index —
    /// gtker's `SheatheType` variant names do NOT describe the rendered posture, so trust dump
    /// values, not names: Worn Shortsword (25) = 3, Dented Buckler (1166, shield) = 4, 0 = no stow.
    /// END-appended + `#[default(0)]` → additive auto-migration. [reference]
    #[default(0)]
    pub sheath: u8,
    /// Item binding: the cmangos `item_template.bonding` value — 0 = NoBind, 1 = Bind on Pickup (BoP),
    /// 2 = Bind on Equip (BoE), 3 = Bind on Use, 4/5 = Quest Item (see [`super::rules::bonding`]).
    /// Drives the client's "Binds when picked up/equipped" tooltip line via
    /// `SMSG_ITEM_QUERY_SINGLE_RESPONSE.bonding`. Trade and mail enforcement (work-items 067, 068) are
    /// out of scope here — this is the data model only.
    /// END-appended + `#[default(0)]` → additive auto-migration (every existing template reads
    /// NoBind, baseline-safe). [reference]
    #[default(0)]
    pub bonding: u8,
    /// The 6 vanilla resistance schools (cmangos `item_template.holyres/fireres/natureres/frostres/
    /// shadowres/arcaneres`) — resist gear is dead weight until these land; the effective-resistance
    /// combat math already exists and can fold these in once a consumer reads them. Work-item 213:
    /// DATA PLUMBING ONLY — no consumer reads these yet, so every existing template stays at 0
    /// (baseline-safe). i32 matching `stat_armor`'s sibling convention (a future negative
    /// suffix-enchant fits). END-appended + `#[default(0)]` → additive auto-migration. [reference]
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
    /// On-use/on-equip spell slots 3-5 (vanilla items carry up to 5; slots 1-2 already cover every
    /// 1-10 consumable). Completes the 191 proc engine's item half — no consumer reads these yet
    /// (work-item 213: data plumbing only). Same shape as `spellid_1`/`spelltrigger_1`: `u32` id,
    /// `u8` `ItemSpellTriggerType`. END-appended + `#[default(0)]` → additive auto-migration.
    /// [reference]
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
    /// Weapon-skill proficiency gate (cmangos `RequiredSkill`/`RequiredSkillRank`) — mail/plate
    /// proficiency + weapon-skill requirements beyond the class tables. Work-item 213: data
    /// plumbing only, no consumer reads these yet (0 = no skill gate, baseline-safe).
    /// END-appended + `#[default(0)]` → additive auto-migration. [reference]
    #[default(0)]
    pub required_skill: u32,
    #[default(0)]
    pub required_skill_rank: u32,
    /// Reputation gate (cmangos `RequiredReputationFaction`/`RequiredReputationRank`) — the 195
    /// item half. Work-item 213: data plumbing only, no consumer reads these yet (0 = no rep gate,
    /// baseline-safe). END-appended + `#[default(0)]` → additive auto-migration. [reference]
    #[default(0)]
    pub required_reputation_faction: u32,
    #[default(0)]
    pub required_reputation_rank: u32,
    /// Unique-count cap (cmangos `maxcount`, 0 = no cap in-game convention but our importer/tooltip
    /// treats 0 as "no limit" same as vanilla), the raw `item_template.Flags` bitmask (unique/
    /// conjured/etc — `ItemFlag` on the wire), the readable-item page-text id (`PageText` — needs a
    /// page_text dump table + reader packet, deferred as its own follow-up), the quest-starter link
    /// (`startquest` — 194 consumes), and the bag-type restriction bitmask (`BagFamily`). Work-item
    /// 213: data plumbing only, no consumer reads these yet (all 0 = unrestricted/no-cap/no-quest,
    /// baseline-safe). END-appended + `#[default(0)]` → additive auto-migration. [reference]
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
    /// cmangos `BuyCount` (080): the stack a vendor hands over per purchase (water/food ×5, ammo
    /// ×200). Defaulted 1 so un-reimported rows keep single-unit sales. END-appended + defaulted →
    /// additive auto-migration.
    #[default(1u32)]
    pub buy_count: u32,
    /// Vanilla item_template.FoodType. Zero is not pet food; 1..=8 select the matching bit in a
    /// creature family's pet_food_mask. Kept at the end for additive schema migration.
    #[default(0)]
    pub food_type: u8,
    /// Explicit class/race eligibility masks.  The importer turns ClassicDB's `-1` unrestricted
    /// sentinel into the complete playable masks before rows become durable; zero remains a
    /// restrictive/malformed durable value.  END-appended defaults make existing rows unrestricted.
    #[default(ALL_PLAYABLE_CLASS_MASK)]
    pub allowed_class: u32,
    #[default(ALL_PLAYABLE_RACE_MASK)]
    pub allowed_race: u32,
}

/// A per-player owned item. Public but RLS-restricted to the owner (like `game_character`), so a
/// connection only ever sees its own items. The `guid` carries `HIGHGUID_ITEM` (0x4000) in its high
/// bits so the client routes it as an item. Timestamp-bearing → born only in [`grant_starter_item`]
/// at login (no SQL import path — see the no-Timestamp-literal rule). [entity]
#[table(accessor = game_item_instance, public, index(accessor = by_owner_guid, btree(columns = [owner_guid])))]
pub struct ItemInstance {
    #[primary_key]
    pub guid: u64,
    pub entry: u32,               // -> ItemTemplate.entry
    pub owner_identity: Identity, // RLS scope: the connection that owns this item
    pub owner_guid: u64,          // the owning character's guid (ITEM_FIELD_OWNER / _CONTAINED)
    pub slot: u8,                 // the player inventory slot it occupies (ItemSlot ordinal)
    pub stack_count: u32,
    pub durability: u32,
    pub created_at: Timestamp,
    /// ENCHANTING (completing the 13): the per-instance enchant applied to THIS item (0 = none). A small
    /// const table (`rules::ENCHANTS`) maps the id → (EquipStat, amount); `equipped_stat_bonus` adds that
    /// amount alongside the template stat for an equipped enchanted piece, so an enchant is server-real
    /// everywhere the effective-* pipeline reads (combat swing/dodge/armor/crit/hit + max-HP/mana). The
    /// 5875 client caches item stats BY ENTRY, so the per-instance enchant glow/green-text is NOT
    /// client-displayed — DEFERRED (launcher/addon territory). END-appended + `#[default(0)]` → additive
    /// auto-migration (every existing row reads 0 = unenchanted, baseline-safe). [entity]
    #[default(0)]
    pub enchant_id: u32,
    /// Item binding state: true once this SPECIFIC instance has bound to its owner and cannot be
    /// traded or mailed away (enforcement lands via work-items 067/068 — this column is the data
    /// model they gate on). Set `true` at grant time for a BoP-templated item (quest reward, starter
    /// kit, loot, vendor buy — any `store_item`/`grant_starter_item` insert) and on first EQUIP for a
    /// BoE-templated item (`apply_item_move`'s equip-validation branch). A BoU/unbound-type template
    /// never flips this today (binds-on-use is a future add).
    /// END-appended + `#[default(false)]` → additive auto-migration (every existing item reads
    /// unbound, baseline-safe). [entity]
    #[default(false)]
    pub soulbound: bool,
}

// Character-owned sweeps: items are deleted on character delete, re-owned (identity re-stamp) on a
// relog under a changed gateway identity.
crate::character_owned!(delete, fn sweep_delete_game_item_instance(ctx, character_guid) {
    let items = ctx.db.game_item_instance();
    for r in items.by_owner_guid().filter(&character_guid).collect::<Vec<_>>() {
        items.guid().delete(r.guid);
    }
});
// CROSS-DATABASE transport (issue #19), HOT: worn gear + bags. The item `guid` is PRESERVED, unlike
// every surrogate-PK table here: it is derived from the OWNER guid (`item_guid_for`), so it is
// already namespaced per character and cannot collide with the destination's own items — and it is
// the id the CLIENT knows an item by (equipment slots, loot, trade). Re-minting it would make every
// item look brand new to the arriving client.
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
            items.guid().update(r);
        }
    }
});

/// Compose an item GUID: `HIGHGUID_ITEM` (0x4000) in bits 48..63, the low id in the low bits
/// (the vanilla item-guid packing the 5875 client expects). The low id is derived from the owning character
/// so a re-grant after a wipe is deterministic and never collides with a unit/player guid.
pub(crate) fn item_guid_for(owner_guid: u64, slot: u8) -> u64 {
    (starter_item::HIGHGUID_ITEM << 48) | ((owner_guid & 0x00FF_FFFF) << 8) | slot as u64
}

/// Mint a guid STRICTLY above all of this owner's existing item guids — never reuse
/// `item_guid_for(owner, fallback_slot)`. That slot→guid derivation is only a one-time birth convention
/// on an empty inventory; `move_item` keeps an item's guid fixed while changing its slot, so a
/// slot-derived guid can collide with an already-moved item's PK and panic the insert. `max+1` is
/// unique by construction; fall back to the slot derivation only on an empty inventory.
pub(crate) fn next_item_guid(ctx: &ReducerContext, owner_guid: u64, fallback_slot: u8) -> u64 {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&owner_guid)
        .map(|i| i.guid)
        .max()
        .map(|m| m + 1)
        .unwrap_or_else(|| item_guid_for(owner_guid, fallback_slot))
}

/// The owner's item instance occupying inventory `slot`, or `None` if that slot is empty. A point scan
/// over the owner's rows (the inventory model has no per-slot index, so this matches each call site's
/// existing `by_owner_guid().filter(&owner).find(|i| i.slot == slot)` exactly).
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

/// Whether any of the owner's items occupies inventory `slot` — the boolean twin of [`item_in_slot`]
/// for the call sites that only need occupancy (an `.any(|i| i.slot == slot)` over the owner's rows).
pub(crate) fn slot_occupied(ctx: &ReducerContext, owner_guid: u64, slot: u8) -> bool {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&owner_guid)
        .any(|i| i.slot == slot)
}

/// Whether an equipped item is BROKEN — it HAS a durability concept (`max_durability > 0`) AND has worn
/// to 0. A broken item grants none of its damage/stats until repaired (`apply_repair_item`). Items with
/// no durability concept (`max_durability == 0` — trinkets/consumables) are never broken. The single
/// source of the rule, shared by [`equipped_stat_bonus`] (stats) and `combat::equipped_weapon_damage`
/// (weapon damage) so the two readouts can never desync.
pub(crate) fn item_is_broken(tmpl: &ItemTemplate, inst: &ItemInstance) -> bool {
    tmpl.max_durability > 0 && inst.durability == 0
}

/// One row of a creature's vendor inventory: vendor `creature_entry` sells `item_entry`. Loaded from
/// cmangos `npc_vendor` by the importer; read by the gateway to build `SMSG_LIST_INVENTORY` and by
/// `apply_buy_item` to gate purchases (you can only buy what a vendor *in range* actually stocks).
/// `slot` orders the window; `max_count` is the stock limit (0 = unlimited — the common case). [static]
#[table(accessor = game_npc_vendor, public, index(accessor = by_vendor, btree(columns = [creature_entry])))]
pub struct NpcVendor {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_entry: u32,
    pub item_entry: u32,
    pub slot: u32,      // display order in the vendor window
    pub max_count: u32, // stock limit (0 = unlimited)
}

/// Per-player buyback ring — the last ≤12 items sold to any vendor. Newest = highest id; the client
/// numbers slots 69–81 (SLOT1 = 0-index 0 = most recent). On sell: oldest is evicted if at capacity,
/// new row inserted. On buyback: the player pays `price` copper and the item is granted back via
/// `store_item`. The gateway subscribes (coordinator) to build the vendor-window buyback tab (248).
#[table(accessor = game_character_buyback, index(accessor = by_player_guid, btree(columns = [player_guid])))]
pub struct BuybackEntry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub player_guid: u64,
    pub item_entry: u32,
    pub stack_count: u32,
    pub price: u32, // copper paid at sell time = buy-back cost
    // Per-instance bind state at sell time: without this, buyback would re-derive soulbound from the
    // template alone (BoP-only), silently un-binding an already-bound BoE item on a sell→buyback round
    // trip. END-appended + `#[default(false)]` → auto-migrates.
    #[default(false)]
    pub soulbound: bool,
}

// Character-owned sweep: the buyback ring is deleted (not re-owned) on character delete — there's no
// restamp counterpart for this one.
crate::character_owned!(delete, fn sweep_delete_game_character_buyback(ctx, character_guid) {
    let buybacks = ctx.db.game_character_buyback();
    for r in buybacks.by_player_guid().filter(&character_guid).collect::<Vec<_>>() {
        buybacks.id().delete(r.id);
    }
});
// CROSS-DATABASE transport (issue #19): the buyback ring is real value the player can still reclaim
// (they paid for the item once). Dropping it at a shard boundary is a silent refund window closing.
// `id` orders the ring (newest = highest), so re-minting preserves the order: `move_rows` inserts in
// export order, and export order is index order.
crate::character_owned!(transfer, fn sweep_transfer_game_character_buyback(ctx, character_guid, io) {
    table = game_character_buyback,
    by = by_player_guid,
    remint = id,
});
