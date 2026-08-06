//! The `apply_*` mutation cores — the ctx/db functions that grant / use / sell / buy / split / move /
//! equip / unequip / repair items and take corpse loot, plus the starter-loadout grant and the
//! equipped-stat sum. Each is the shared core behind a thin player reducer and its debug twin (see
//! `reducers.rs`). All effects are additive: they touch only the item rows + the actor's health/money.

use spacetimedb::{Identity, ReducerContext, Table};

use lyracore_shared::constants::starter_item;

use crate::game_character; // the durable char holds `class` (the live WorldEntity does not)
use crate::game_corpse_loot; // the loot.rs accessor trait — re-exported at crate root (`pub use loot::*`)
use crate::game_gameobject;
use crate::game_spell_effect; // Gate A reads the on-use spell's effect kinds (by_spell) to mana-gate energize consumables
use crate::game_start_item; // per-class starter loadout (CharStartOutfit via importer --dbc)
use crate::game_world_entity; // gameobject (chest) loot source — apply_take_loot resolves it too

use super::rules::{
    binds_on_equip, binds_on_grant, buy_cost, can_equip_into, can_equip_proficiency,
    consumable_heal, enchant_stat, equip_slot, invtype, meets_required_level, merge_amount,
    repair_cost, resolve_equip_slot, sell_value, template_stat, EquipStat,
};
use super::tables::{
    game_character_buyback, game_item_instance, game_item_template, game_npc_vendor, item_guid_for,
    item_in_slot, item_is_broken, next_item_guid, slot_occupied, BuybackEntry, ItemInstance,
    ItemTemplate,
};

/// Max distance to interact with a vendor: (10 yd)². The client walks into range before sending
/// `CMSG_BUY_ITEM`, so this only rejects clearly-out-of-range abuse (mirrors `loot::LOOT_RANGE_SQ`).
const VENDOR_RANGE_SQ: f32 = 100.0;

/// Does the vendor creature `vendor_entry` stock `item_entry`? Gates `apply_buy_item` so a player can
/// only buy what a vendor actually sells (not any item with a buy price). Reads `game_npc_vendor`.
pub(crate) fn vendor_sells(ctx: &ReducerContext, vendor_entry: u32, item_entry: u32) -> bool {
    ctx.db
        .game_npc_vendor()
        .by_vendor()
        .filter(&vendor_entry)
        .any(|v| v.item_entry == item_entry)
}

/// Grant the starter loadout to a character the first time it logs in, idempotently. Called from
/// `player_login` once the live entity exists (so `owner_identity` is bound). A no-op if the
/// character already owns any item (so a relog never duplicates). The single ownership guard at the
/// top covers every grant below, so a relog never duplicates any of them. Each individual grant is
/// skipped if its template isn't seeded (so login never depends on item data being loaded). The
/// loadout is the EQUIPPED main-hand weapon plus a couple of loose backpack items, proving the
/// inventory renders multiple distinct items at distinct slots. Additive: it only ever inserts into
/// the two new item tables. [entity]
pub(crate) fn grant_starter_item(ctx: &ReducerContext, owner_guid: u64, owner_identity: Identity) {
    let instances = ctx.db.game_item_instance();
    // Already has items → nothing to do (idempotent across relogs). This one guard gates every grant
    // below, so the whole loadout is granted at most once per character.
    if instances
        .by_owner_guid()
        .filter(&owner_guid)
        .next()
        .is_some()
    {
        return;
    }
    // Grant one owned item at `slot`, only if its template is seeded (a missing template is skipped,
    // never fatal to login). `item_guid_for(owner_guid, slot)` derives a distinct guid per slot.
    let grant_one = |entry: u32, slot: u8, stack: u32| {
        let Some(tmpl) = ctx.db.game_item_template().entry().find(entry) else {
            return;
        };
        instances.insert(ItemInstance {
            guid: item_guid_for(owner_guid, slot),
            entry: tmpl.entry,
            owner_identity,
            owner_guid,
            slot,
            stack_count: stack,
            durability: tmpl.max_durability,
            created_at: ctx.timestamp,
            enchant_id: 0, // freshly minted loadout item — unenchanted
            // BoP (bonding::BIND_ON_PICKUP) binds the instant it's granted — the starter kit is a
            // grant source like any other.
            soulbound: binds_on_grant(tmpl.bonding),
        });
    };
    // Every character starts with a Hearthstone in the backpack (use it to recall to the bound home).
    // Granted BEFORE the outfit/fallback branches so both paths get it; HEARTHSTONE_SLOT (38, last
    // backpack) sits clear of the outfit's stow slots (from BACKPACK_SLOT_0 up). Skipped if unseeded.
    grant_one(
        starter_item::HEARTHSTONE_ENTRY,
        starter_item::HEARTHSTONE_SLOT,
        1,
    );
    // Per-class loadout from CharStartOutfit (game_start_item, importer --dbc): EQUIP each equippable
    // piece into its resolved slot (so weapons/armor render on the model) and stow the rest in the
    // backpack — so a Mage spawns with a staff/robe, not the Warrior's sword. Keyed by the character's
    // (race, class) (unit_bytes_0). Falls back to the hand-authored Warrior loadout below when the outfit
    // table is empty (pre-import) — login never breaks.
    // (race, class) from the DURABLE character row — NOT the live world entity: the creation-time
    // call site runs before any entity exists (that's the whole point — gear must show on the
    // char-select screen, which renders from item rows alone).
    let race_class: u16 = ctx
        .db
        .game_character()
        .guid()
        .find(owner_guid)
        .map(|c| ((c.race as u16) << 8) | c.class as u16)
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
            // Skip the Hearthstone — it was already granted explicitly into HEARTHSTONE_SLOT above.
            // CharStartOutfit lists 6948 for every race/class, so without this guard every new
            // character would receive two Hearthstone rows (one equipped slot + one backpack copy).
            if entry == starter_item::HEARTHSTONE_ENTRY {
                continue;
            }
            let Some(tmpl) = ctx.db.game_item_template().entry().find(entry) else {
                continue;
            };
            // Equippable (its inventory_type maps to a slot) → that slot; otherwise the next free backpack
            // slot. `occupied` tracks slots filled this pass so a paired ring/trinket takes the free finger.
            // `false` (Dual Wield) is correct here — a brand-new character has learned no spells yet,
            // so the second-one-hander-to-offhand redirect never applies at grant time.
            // Ammo (Projectile, class 6) starts as a FULL STACK. CharStartOutfit lists only the item id,
            // not a count, so a flat grant of 1 gave hunters/rogues a single round — the first Auto Shot
            // consumes it, then the ranged swing tick finds no ammo and cancels the auto-repeat (the shot
            // never lands). Vanilla starts a hunter with 200. Non-ammo items grant 1 as before.
            const CLASS_AMMO: u8 = 6;
            let count = if tmpl.class == CLASS_AMMO { 200 } else { 1 };
            match resolve_equip_slot(tmpl.inventory_type, false, |s| occupied.contains(&s)) {
                Some(eq) => {
                    occupied.insert(eq);
                    grant_one(entry, eq, count);
                }
                None => {
                    grant_one(entry, backpack, count);
                    backpack += 1;
                }
            }
        }
        return;
    }

    // FALLBACK (pre-import / unseeded race_class): the hand-authored Warrior loadout — a weapon EQUIPPED in
    // the main hand (slot 15) so the client renders it on the model, plus a couple of backpack items.
    grant_one(starter_item::ENTRY, starter_item::MAINHAND_SLOT, 1);
    grant_one(51, starter_item::BACKPACK_SLOT_0, 1);
    grant_one(52, starter_item::BACKPACK_SLOT_0 + 1, 5);
}

/// Grant `count`× `item_entry` to `player_guid` — the canonical "mint an owned item" core, used by
/// quest turn-in rewards ([`crate::quest`]) and available to any other give-an-item path. Looks up the
/// player (for the RLS `owner_identity`) and the item template (for durability), then delegates to
/// `store_item`, which tops up an existing partial stack first and spills the remainder across as many
/// backpack/bag slots as it takes. Returns `Err` (rolling the whole tx back) if the player isn't in
/// world, the template is missing, or there's no room left — so a reward that can't be delivered fails
/// the turn-in atomically rather than half-applying. `count` is floored at 1. Additive — inserts item
/// row(s). [entity]
pub(crate) fn grant_item(
    ctx: &ReducerContext,
    player_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(player_guid)
        .ok_or_else(|| "player not in world".to_string())?;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(item_entry)
        .ok_or_else(|| format!("no such item {item_entry}"))?;
    store_item(
        ctx,
        player_guid,
        player.owner_identity,
        &tmpl,
        count.max(1),
        false,
    )
}

/// Add `count` units of `tmpl` to `player_guid`'s backpack the way vanilla auto-store does (parity #14):
/// TOP UP existing partial stacks of the same entry first (lowest slot first), then spill the remainder
/// into free backpack slots (each new stack ≤ `max_stack`). `Err("inventory full")` if it can't all fit —
/// the caller's reducer rolls back (so a buy that overflows un-charges). Shared by grant / buy / loot so a
/// second food or arrow tops up the first stack instead of fragmenting, and a multi-stack grant spills.
fn store_item(
    ctx: &ReducerContext,
    player_guid: u64,
    owner_identity: spacetimedb::Identity,
    tmpl: &ItemTemplate,
    mut count: u32,
    force_soulbound: bool,
) -> Result<(), String> {
    let instances = ctx.db.game_item_instance();
    let max_stack = tmpl.max_stack.max(1);
    // 1. Merge into existing partial stacks (only stackables have headroom; lowest slot first).
    if max_stack > 1 {
        let mut partials: Vec<ItemInstance> = instances
            .by_owner_guid()
            .filter(&player_guid)
            .filter(|i| i.entry == tmpl.entry && i.stack_count < max_stack)
            .collect();
        partials.sort_by_key(|i| i.slot);
        for mut inst in partials {
            if count == 0 {
                break;
            }
            let add = merge_amount(count, inst.stack_count, max_stack);
            inst.stack_count += add;
            count -= add;
            instances.guid().update(inst);
        }
    }
    // 2. Spill the remainder into free slots: backpack first (23..=38), then equipped bags (in
    //    bag-equip order 19..22). Both searches land in the same flat `game_item_instance` model;
    //    the gateway routes items in the bag-content range (120..=191) via the container object.
    while count > 0 {
        let free_slot = first_free_backpack_slot(ctx, player_guid)
            .or_else(|| first_free_bag_slot(ctx, player_guid))
            .ok_or_else(|| "inventory full".to_string())?;
        let take = count.min(max_stack);
        let new_guid = next_item_guid(ctx, player_guid, free_slot);
        instances.insert(ItemInstance {
            guid: new_guid,
            entry: tmpl.entry,
            owner_identity,
            owner_guid: player_guid,
            slot: free_slot,
            stack_count: take,
            durability: tmpl.max_durability,
            created_at: ctx.timestamp,
            enchant_id: 0, // freshly stored stack — unenchanted (enchants apply to equipped non-stackables)
            // BoP binds the instant it's stored — covers every `store_item` caller (grant / buy / loot /
            // buyback) uniformly, matching vanilla: a Bind-on-Pickup item binds regardless of source.
            // `force_soulbound` preserves an already-bound instance's state across buyback: a BoE
            // item bound before being sold must come back bound, not re-derived from the template
            // alone.
            soulbound: force_soulbound || binds_on_grant(tmpl.bonding),
        });
        count -= take;
    }
    Ok(())
}

/// Total quantity of `item_entry` the player owns across ALL stacks (bag + equipped) — drives
/// collect-quest completion (parity #4). Sums `stack_count` over the owner's matching item rows. [entity]
pub(crate) fn item_count(ctx: &ReducerContext, owner_guid: u64, item_entry: u32) -> u32 {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&owner_guid)
        .filter(|i| i.entry == item_entry)
        .map(|i| i.stack_count)
        .sum()
}

/// Remove exactly `count` units of `item_entry` from the player's stacks (lowest slot first), deleting an
/// emptied row. `Err` if the player doesn't actually have enough (the caller's reducer rolls back, so a
/// quest turn-in stays un-rewarded). Consumes a collect-quest's required items on turn-in (parity #4). [entity]
pub(crate) fn remove_items(
    ctx: &ReducerContext,
    owner_guid: u64,
    item_entry: u32,
    mut count: u32,
) -> Result<(), String> {
    let instances = ctx.db.game_item_instance();
    let mut stacks: Vec<ItemInstance> = instances
        .by_owner_guid()
        .filter(&owner_guid)
        .filter(|i| i.entry == item_entry)
        .collect();
    stacks.sort_by_key(|i| i.slot);
    for mut inst in stacks {
        if count == 0 {
            break;
        }
        let take = count.min(inst.stack_count);
        inst.stack_count -= take;
        count -= take;
        if inst.stack_count == 0 {
            instances.guid().delete(inst.guid);
        } else {
            instances.guid().update(inst);
        }
    }
    if count > 0 {
        return Err(format!("missing {count} of item {item_entry}"));
    }
    Ok(())
}

/// Item class that `use_item` can consume here — 0 = Consumable (food/drink). Other classes (weapon,
/// armor) aren't "usable" in this minimal slice.
const ITEM_CLASS_CONSUMABLE: u8 = 0;

/// `(consumable item entry → its on-use spell id)` — the data-driven "this item casts THIS spell when
/// used" map, mirroring `spell::cast::RECIPES` exactly (a small const keyed off entry, never a hardcoded
/// branch in the handler). A consumable in this map casts its spell through the SHARED `begin_cast` core
/// (instant heal / channeled HoT / buff — `begin_cast` routes on the spell's header bits), reusing the
/// whole cast+effect+aura+channel pipeline with ZERO new effect-engine code. An item NOT in the map falls
/// through to the legacy vital-restore branch (water/drink), so that path is untouched (baseline-safe).
/// A future drop-in (mana potion, bandage rank, better food) = one tuple + one seed block.
const USE_EFFECTS: &[(u32, u32)] = &[
    (118, 50110), // Minor Healing Potion (real item 118) -> spell 50110 "Minor Healing" (instant E_HEAL, clamps to max)
    (1251, 50111), // Linen Bandage (real item 1251)       -> spell 50111 "Linen Bandage" (channeled A_PERIODIC_HEAL HoT + triggers the cooldown debuff)
    // ENGINEERING bomb (completing the 13): Rough Copper Bomb (real item 4360, class 7 Trade Goods) ->
    // spell 50096 "Rough Copper Bomb" — an E_DAMAGE T_AREA_ENEMY AoE cast through begin_cast (the AoE engine
    // fans it out to every in-8yd hostile). A bomb is "a crafted item whose USE casts an AoE-damage spell".
    (4360, 50096), // Rough Copper Bomb -> AoE fire damage through begin_cast (the AoE fan-out is already in the engine)
    // Roasted Boar Meat (real item 2681) is intentionally NOT mapped: a level-1 food only eats-to-heal in
    // vanilla (sit-and-eat HP regen), and the +STA/+SPI Well-Fed buff is a higher-level-food mechanic. So
    // it falls through to the legacy vital-restore (eat) branch — vanilla-faithful, no non-vanilla aura.
    // --- CONSUMABLE BREADTH (1-10 alpha): mana potion / drink / over-time food / Well-Fed Cooking buff +
    // the cheap rank-2s. All real vanilla items, real restore/heal/buff magnitudes. Each is
    // ANOTHER tuple + seed block — ZERO new engine code (begin_cast routes them all). The energize ones
    // (mana potion / drink) are MANA-class-gated in apply_item_use (Gate A) so a Warrior/Rogue can't preload
    // rage/energy off them. ---
    (2455, 50113), // Minor Mana Potion (real 2455)            -> spell 50113 E_ENERGIZE 160 mana (instant; mana-class only)
    (159, 50114), // Refreshing Spring Water (real 159)       -> spell 50114 A_PERIODIC_ENERGIZE mana-over-time (drink; mana-class only)
    (5350, 50114), // Conjured Water (real 5350)               -> same drink spell (mana-over-time)
    (4540, 50115), // Tough Hunk of Bread (real 4540)          -> spell 50115 A_PERIODIC_HEAL hp-over-time (food)
    (117, 50115),  // Tough Jerky (real 117)                   -> same food spell (hp-over-time)
    (2680, 50116), // Spiced Wolf Meat (real 2680, made by the real cooking recipe) -> spell 50116 Well Fed +Sta/+Spi buff
    (858, 50117), // Lesser Healing Potion (real 858)         -> spell 50117 E_HEAL 160 (health rank-2 of 118/50110)
    (2581, 50118), // Heavy Linen Bandage (real 2581)          -> spell 50118 channeled HoT 144 + Recently-Bandaged (rank-2 of 1251/50111)
];

/// "Recently Bandaged" debuff spell id (vanilla 11196) — already seeded (a 60s `A_FLAG` marker aura, no
/// tick). The bandage's eff1 `E_TRIGGER` applies it, and `apply_item_use` gates on `has_aura(.., this)`
/// BEFORE consuming/casting so a player can't re-bandage while it is live (vanilla's anti-spam window).
/// Reusing the existing 11196 row avoids seeding a new cooldown spell entirely.
pub(crate) const RECENTLY_BANDAGED_SPELL: u32 = 11196;

/// The on-use spell for consumable `entry`, or `None` if the item has no mapped on-use cast (then the
/// legacy vital-restore branch runs unchanged). Mirrors `recipe_for` — a pure lookup, unit-tested. The
/// `begin_cast` core then routes the spell instant/channel/buff off its OWN header (no per-item code).
fn use_spell_for(entry: u32) -> Option<u32> {
    USE_EFFECTS
        .iter()
        .find_map(|&(item, spell)| (item == entry).then_some(spell))
}

/// Is `entry` a BANDAGE AND is its cooldown debuff (`RECENTLY_BANDAGED_SPELL`) currently live on the
/// user? The pure half of the re-bandage gate (the `has_aura` presence is the ctx half) — split out so
/// the "a bandage is cooldown-gated, other consumables are not" decision is unit-testable without a ctx.
/// BOTH bandage entries (real items 1251 Linen + 2581 Heavy Linen) are gated — in vanilla ALL bandages
/// share the single "Recently Bandaged" lockout (Gate B), so a player can't alternate 1251/2581 to bypass
/// the window. Every other consumable returns false (no cooldown), so the gate is a no-op for the
/// potion/food/water (baseline-safe).
fn bandage_cooldown_blocks(entry: u32, has_cooldown_debuff: bool) -> bool {
    matches!(entry, 1251 | 2581) && has_cooldown_debuff
}

/// Is `player_guid`'s class a MANA class? (class lives on `game_character`, not the live entity). Gates
/// every POWER-restoring consumable — the legacy drink branch (water without an on-use spell) AND Gate A
/// for the new mapped energize consumables (mana potion / drink). A non-mana class (Warrior/Rogue) using a
/// power consumable would otherwise preload rage/energy out of combat (the reducer-validation-sweep
/// exploit). A missing character row fails closed (treated as non-mana). [entity]
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

/// Does the on-use `spell_id` RESTORE POWER (an `E_ENERGIZE` instant or an `A_PERIODIC_ENERGIZE` drink)?
/// The pure-data half of Gate A: a power-restoring consumable is gated to mana classes (`is_mana_class`).
/// Reads `game_spell_effect` by the `by_spell` index — `true` if ANY effect is an energize kind. A heal /
/// HoT / food / buff has no energize effect → `false` → ungated (class-agnostic). [entity]
fn spell_restores_power(ctx: &ReducerContext, spell_id: u32) -> bool {
    ctx.db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .any(|e| e.kind == crate::spell::E_ENERGIZE || e.kind == crate::spell::A_PERIODIC_ENERGIZE)
}

/// Shared use-an-item logic for the player + debug paths: consume one unit of the item in inventory
/// `slot` and apply its effect. Today only CONSUMABLES (class 0) are usable — they restore HP, or mana
/// for a drink (`restores_power`) (`consumable_heal`, clamped to max), then the stack is decremented
/// (the row is deleted at 0). Errors if the user is dead, the slot is empty, the template is missing,
/// or the item isn't a consumable. Additive — touches only the item + the user's health or power. [entity]
pub(crate) fn apply_item_use(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot use items".to_string());
    }
    let instances = ctx.db.game_item_instance();
    let mut inst =
        item_in_slot(ctx, player_guid, slot).ok_or_else(|| format!("no item in slot {slot}"))?;
    // Hearthstone (entry 6948) recalls to the bound home instead of the consume path (it isn't a
    // consumable, so it'd be rejected below). Player liveness already checked above. Slice: immediate
    // teleport — the vanilla ~10s channel is a follow-up.
    if inst.entry == lyracore_shared::constants::starter_item::HEARTHSTONE_ENTRY {
        crate::world::recall_to_home(ctx, player_guid);
        return Ok(());
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(inst.entry)
        .ok_or_else(|| format!("no template for item entry {}", inst.entry))?;
    // A CONSUMABLE (class 0) is always usable; otherwise the item must carry a mapped on-use spell to be
    // "used" — this admits a class-7 Trade Good like the Engineering bomb (Rough Copper Bomb 4360, an
    // AoE-damage on-use) without loosening the gate to every trade good. Keeping the gate strict-by-default
    // means a plain reagent (no USE_EFFECTS entry) is still rejected (baseline-safe). The on-use cast itself
    // runs in the USE_EFFECTS branch below.
    if tmpl.class != ITEM_CLASS_CONSUMABLE && use_spell_for(inst.entry).is_none() {
        return Err(format!("item {} is not consumable", inst.entry));
    }
    // Required-level gate: a too-high item can sit in the bag, but can't be USED. Reuse the player
    // entity already read above for its level. Seeded items are required_level 1, so this never trips.
    if !meets_required_level(player.level, tmpl.required_level) {
        return Err(format!("requires level {}", tmpl.required_level));
    }
    // ON-USE SPELL path (potion / bandage / food). A consumable mapped in `USE_EFFECTS` casts a SPELL
    // when used (the vanilla model: a potion casts a heal, a bandage casts a channeled HoT, food casts a
    // well-fed buff) — routed through the SHARED `begin_cast` core, which picks instant vs channeled off
    // the spell header. Checked FIRST so it short-circuits before the legacy vital-restore branch below;
    // an unmapped item (water/drink) falls through and that branch is untouched (no regression).
    if let Some(spell_id) = use_spell_for(inst.entry) {
        // RE-BANDAGE GATE (vanilla "Recently Bandaged"): refuse the BANDAGE while its cooldown debuff is
        // live — BEFORE consuming the item or casting, so a blocked re-bandage costs nothing. `has_aura`
        // keys on the existing 11196 marker (applied by the bandage's E_TRIGGER eff1). Only the bandage is
        // gated; the potion/food skip it (pure `bandage_cooldown_blocks` decides). The level here is the
        // player's level (the cast level fed to magnitude scaling).
        if bandage_cooldown_blocks(
            inst.entry,
            crate::spell::has_aura(ctx, player_guid, RECENTLY_BANDAGED_SPELL),
        ) {
            return Err("Recently Bandaged".to_string());
        }
        // GATE A (mana-class gate for power consumables): a mana potion / drink restores POWER through
        // E_ENERGIZE / A_PERIODIC_ENERGIZE — for a non-mana class that is rage/energy, the out-of-combat
        // preload exploit. `begin_cast` has NO power-class gate (only the legacy drink branch below did),
        // so DENY a power-restoring on-use spell here, BEFORE consuming/casting, when the user isn't a mana
        // class. Only energize spells are gated (`spell_restores_power`); a heal/HoT/food/buff is class-
        // agnostic (Warriors can quaff a healing potion / eat). Mirrors the legacy water gate's intent.
        if spell_restores_power(ctx, spell_id) && !is_mana_class(ctx, player_guid) {
            return Err("only mana users can use that".to_string());
        }
        // Consume one unit FIRST (the cast is the effect; a failed cast still consumed the item, matching
        // vanilla — a fizzled potion is gone), then cast at SELF. `begin_cast` runs the full effect list,
        // emits the cast-START/GO events the gateway relays, and routes channeled (bandage) vs instant
        // (potion/food) on the `SPELL_ATTR_CHANNELED` header bit — no per-item branching here.
        if inst.stack_count > 1 {
            inst.stack_count -= 1;
            instances.guid().update(inst);
        } else {
            instances.guid().delete(inst.guid);
        }
        return crate::spell::begin_cast(
            ctx,
            player_guid,
            spell_id,
            player.level as u8,
            player_guid,
            false,
            None,
        );
    }
    // Restore the consumable's vital (clamped to max), then consume one from the stack. A DRINK
    // (tmpl.restores_power) refills POWER, but ONLY for a MANA class. The water items carry no class
    // restriction, so without the mana gate a Warrior/Rogue drinking water would refill rage/energy
    // (max_power 1000/100) out of combat — a pre-loaded-rage exploit. Food / everything else / a non-mana
    // class refills HEALTH. The amount reuses the same `consumable_heal(item_level)` formula either way.
    let restore = consumable_heal(tmpl.item_level);
    // class lives on game_character (the WorldEntity has no class); a drink refills POWER only for a MANA
    // class — else a Warrior/Rogue would pre-load rage/energy off water (the items carry no class gate).
    // Shares the `is_mana_class` helper with Gate A (the mapped energize-consumable gate above).
    if tmpl.restores_power && is_mana_class(ctx, player_guid) {
        player.power = (player.power + restore).min(player.max_power);
    } else {
        player.health = (player.health + restore).min(player.max_health);
    }
    entities.guid().update(player);
    if inst.stack_count > 1 {
        inst.stack_count -= 1;
        instances.guid().update(inst);
    } else {
        instances.guid().delete(inst.guid);
    }
    Ok(())
}

/// FIFO eviction gate for the 12-slot buyback ring: once it already holds 12 entries, the OLDEST
/// (ascending by auto_inc id, so the caller's already-sorted first element) is evicted before the new
/// sale is pushed — so the ring never grows past 12. Extracted from `apply_item_sell` (pure code-motion)
/// so the `>= 12` boundary is unit-tested without a live module.
pub(crate) fn buyback_ring_full(current_len: usize) -> bool {
    current_len >= 12
}

/// The buyback ring's re-purchase ORDER: given the ring's entry ids (any order), return them NEWEST
/// FIRST (descending by auto_inc id) — slot 0 is the most recently sold item. Extracted from
/// `apply_buyback_item` (pure code-motion): the live path sorts entries by this exact id order then
/// indexes by `slot_idx`.
pub(crate) fn buyback_newest_first(mut ids: Vec<u64>) -> Vec<u64> {
    ids.sort_by(|a, b| b.cmp(a));
    ids
}

/// Shared sell-to-vendor logic for the player + debug paths: sell the whole stack in inventory `slot`
/// for copper. Looks up the item's template; a 0 `sell_price` means the item has no vendor value
/// (quest/soulbound-junk analogue) and is rejected. On success it credits the player's `money`
/// (saturating, so the copper counter never wraps) by `sell_value(sell_price, stack_count)` and then
/// DELETES the whole item row — vanilla sells the entire stack in one action. Errors if the user
/// isn't in world, the slot is empty, the template is missing, or the item is valueless. Additive —
/// touches only the user's money + the one item row. [entity]
pub(crate) fn apply_item_sell(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    slot: u8,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    // Vendor gating, mirroring apply_buy_item: a dead/ghost player can't vendor; the sale must be to a
    // real VENDOR creature within range on the same map. Unlike buy there is NO vendor_sells() check —
    // vanilla lets you sell any sellable item to any vendor (junk goes to anyone), only the proximity +
    // "is a vendor" gates apply.
    if player.dead {
        return Err("dead players cannot sell".to_string());
    }
    let vendor = entities
        .guid()
        .find(vendor_guid)
        .ok_or_else(|| "no such vendor".to_string())?;
    if vendor.is_player() || vendor.npc_flags & lyracore_shared::constants::npc_flags::VENDOR == 0 {
        return Err("target is not a vendor".to_string());
    }
    if vendor.map_id != player.map_id || vendor.instance_id != player.instance_id {
        return Err("vendor on another map".to_string());
    }
    let (dx, dy, dz) = (
        vendor.x - player.x,
        vendor.y - player.y,
        vendor.z - player.z,
    );
    if dx * dx + dy * dy + dz * dz > VENDOR_RANGE_SQ {
        return Err("vendor out of range".to_string());
    }
    let instances = ctx.db.game_item_instance();
    let inst =
        item_in_slot(ctx, player_guid, slot).ok_or_else(|| format!("no item in slot {slot}"))?;
    // Only BACKPACK/bag items are sellable. Refuse an EQUIPPED item (slots 0..=18): vanilla's vendor
    // window can't target worn gear, so a client must not be able to sell items off the body.
    if inst.slot <= equip_slot::END {
        return Err("cannot sell an equipped item".to_string());
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(inst.entry)
        .ok_or_else(|| format!("no template for item entry {}", inst.entry))?;
    if tmpl.sell_price == 0 {
        return Err("item has no sell value".to_string());
    }
    // Credit the copper for the whole stack (saturating), then remove the sold item.
    let earned = sell_value(tmpl.sell_price, inst.stack_count);
    player.money = player.money.saturating_add(earned);
    entities.guid().update(player);
    // Push to the 12-slot buyback ring (oldest evicted when full).
    let buyback_tab = ctx.db.game_character_buyback();
    let mut old: Vec<BuybackEntry> = buyback_tab.by_player_guid().filter(&player_guid).collect();
    old.sort_by_key(|e| e.id);
    if buyback_ring_full(old.len()) {
        buyback_tab.id().delete(old[0].id);
    }
    buyback_tab.insert(BuybackEntry {
        id: 0, // auto_inc
        player_guid,
        item_entry: inst.entry,
        stack_count: inst.stack_count,
        price: earned,
        soulbound: inst.soulbound,
    });
    instances.guid().delete(inst.guid);
    Ok(())
}

/// Re-purchase an item from the buyback ring (`CMSG_BUYBACK_ITEM`). Mirrors the vendor gating of
/// [`apply_item_sell`]; `slot_idx` is 0-based (the gateway maps `BuybackSlot.as_int() - 69`). The
/// buyback ring is ordered newest-first by id; slot 0 = most recently sold. [entity]
pub(crate) fn apply_buyback_item(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    slot_idx: u8,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot use buyback".to_string());
    }
    let vendor = entities
        .guid()
        .find(vendor_guid)
        .ok_or_else(|| "no such vendor".to_string())?;
    if vendor.is_player() || vendor.npc_flags & lyracore_shared::constants::npc_flags::VENDOR == 0 {
        return Err("target is not a vendor".to_string());
    }
    if vendor.map_id != player.map_id || vendor.instance_id != player.instance_id {
        return Err("vendor on another map".to_string());
    }
    let (dx, dy, dz) = (
        vendor.x - player.x,
        vendor.y - player.y,
        vendor.z - player.z,
    );
    if dx * dx + dy * dy + dz * dz > VENDOR_RANGE_SQ {
        return Err("vendor out of range".to_string());
    }
    // Find the buyback entry: sort by id desc (newest first = slot 0), take Nth.
    let buyback_tab = ctx.db.game_character_buyback();
    let entries: Vec<BuybackEntry> = buyback_tab.by_player_guid().filter(&player_guid).collect();
    let ordered_ids = buyback_newest_first(entries.iter().map(|e| e.id).collect());
    let target_id = ordered_ids
        .get(slot_idx as usize)
        .copied()
        .ok_or_else(|| "no item in buyback slot".to_string())?;
    let entry = entries
        .into_iter()
        .find(|e| e.id == target_id)
        .ok_or_else(|| "no item in buyback slot".to_string())?;
    if player.money < entry.price {
        return Err(format!(
            "not enough money: have {}, need {}",
            player.money, entry.price
        ));
    }
    let owner_identity = player.owner_identity;
    player.money -= entry.price;
    entities.guid().update(player);
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(entry.item_entry)
        .ok_or_else(|| format!("no template for item {}", entry.item_entry))?;
    store_item(
        ctx,
        player_guid,
        owner_identity,
        &tmpl,
        entry.stack_count,
        entry.soulbound,
    )?;
    buyback_tab.id().delete(entry.id);
    Ok(())
}

/// Shared buy-from-vendor logic for the player + debug paths — the inverse of [`apply_item_sell`],
/// closing the money loop. Charges `buy_cost(buy_price, count)` copper for `count` units of `item_entry`
/// and grants them via `store_item`, which tops up an existing partial stack first and then spills the
/// remainder across as many stacks as it takes (each ≤ the template's `max_stack`), so buying more than
/// one stack's worth doesn't reject the purchase. A 0 `buy_price` means the item has no vendor value
/// (quest/junk analogue) and is unbuyable. MONEY IS DEBITED BEFORE THE ITEM IS GRANTED: deduct
/// (saturating, so the copper counter never wraps) and persist the player, THEN place the item — so a
/// mid-call "inventory full" still fails the whole reducer (SpacetimeDB rolls back the tx on `Err`),
/// never charging without delivering. Errors if the user isn't in world, is dead, `count == 0`, the
/// template is missing, the item is valueless, the player can't afford it, or there's no room left for
/// it all. Additive — touches only the player's money + the granted item row(s). [entity]
pub(crate) fn apply_buy_item(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    // Death is server-authoritative everywhere — a dead/ghost player can't shop (mirrors the other paths).
    if player.dead {
        return Err("dead players cannot buy".to_string());
    }
    if count == 0 {
        return Err("invalid count".to_string());
    }
    // Vendor gating: the purchase must come from a real VENDOR creature the player is standing at, and
    // that vendor must actually stock the item — you can't buy arbitrary items or from a non-vendor.
    let vendor = entities
        .guid()
        .find(vendor_guid)
        .ok_or_else(|| "no such vendor".to_string())?;
    if vendor.is_player() || vendor.npc_flags & lyracore_shared::constants::npc_flags::VENDOR == 0 {
        return Err("target is not a vendor".to_string());
    }
    if vendor.map_id != player.map_id || vendor.instance_id != player.instance_id {
        return Err("vendor on another map".to_string());
    }
    let (dx, dy, dz) = (
        vendor.x - player.x,
        vendor.y - player.y,
        vendor.z - player.z,
    );
    if dx * dx + dy * dy + dz * dz > VENDOR_RANGE_SQ {
        return Err("vendor out of range".to_string());
    }
    if !vendor_sells(ctx, vendor.entry, item_entry) {
        return Err("vendor does not sell that item".to_string());
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(item_entry)
        .ok_or_else(|| format!("no such item {item_entry}"))?;
    // A 0 buy_price is the "vendor never sells this" sentinel (quest/junk analogue) — reject before charging.
    if tmpl.buy_price == 0 {
        return Err("item cannot be bought".to_string());
    }
    // Reputation vendor discount (195): Honored+ with the vendor's parent faction cuts the BUY price
    // (vanilla 5% per rank above Neutral — Honored 10%). Sell is unchanged. Neutral / no-standing / a
    // factionless vendor → 0% → full price. Applied at this single buy chokepoint, after the count math.
    let cost = {
        let base = buy_cost(tmpl.buy_price, count);
        let pct = crate::reputation::vendor_discount_pct(ctx, player_guid, vendor.faction_template);
        base - base.saturating_mul(pct) / 100 // floor rounding (matches cmangos' truncating multiply)
    };
    if player.money < cost {
        return Err(format!(
            "not enough money: have {}, need {}",
            player.money, cost
        ));
    }
    // DEBIT FIRST: deduct (saturating, never wraps) and persist the player BEFORE granting the item, so a
    // subsequent failure (inventory full) rolls the whole tx back rather than charging for nothing. A buy
    // of more than `max_stack` now SPILLS across stacks (parity #14) — and tops up an existing partial
    // stack of the same item — instead of being rejected.
    player.money = player.money.saturating_sub(cost);
    let owner_identity = player.owner_identity;
    entities.guid().update(player);
    store_item(ctx, player_guid, owner_identity, &tmpl, count, false)
}

/// Shared stack-split logic for the player + debug paths: split `count` units off the stack in `slot`
/// into the empty `to_slot`, leaving the remainder in the source. Vanilla only splits a STRICT subset
/// (you can't split off the whole stack — that's a move), so `count == 0` or `count >= stack_count` is
/// rejected; the destination must be empty. The new partial-stack row reuses the source's entry /
/// owner / durability and takes a fresh per-slot guid (`item_guid_for`) + the current timestamp.
/// Errors if the source slot is empty, the count is invalid, or the destination is occupied. Additive
/// — decrements the source row and inserts one new item row. [entity]
pub(crate) fn apply_item_split(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
    count: u32,
    to_slot: u8,
) -> Result<(), String> {
    let instances = ctx.db.game_item_instance();
    let mut inst =
        item_in_slot(ctx, player_guid, slot).ok_or_else(|| format!("no item in slot {slot}"))?;
    // A split must leave at least one unit in BOTH slots — splitting off none or the whole stack isn't
    // a split (the latter is a move).
    if !valid_split_count(count, inst.stack_count) {
        return Err("invalid split count".to_string());
    }
    // Reject an out-of-range destination (anti-overflow; same phantom-slot dupe vector as apply_item_move).
    if !valid_dest_slot(to_slot) {
        return Err(format!("invalid destination slot {to_slot}"));
    }
    // Bag-content destination: validate that the corresponding bag is equipped and the slot is
    // within its capacity — same phantom-slot dupe vector as apply_item_move (a modified client
    // can CMSG_SPLIT_ITEM with to_slot=120 even when no bag is equipped in slot 19, creating an
    // orphaned item row that is invisible and can never be freed by normal play).
    if to_slot >= BAG_CONTENT_OFFSET {
        let (bag_idx, slot_in_bag) = bag_content_decompose(to_slot);
        let bag_equip_slot = BAG_SLOT_START + bag_idx;
        let bag_inst = item_in_slot(ctx, player_guid, bag_equip_slot)
            .ok_or_else(|| format!("no bag equipped in slot {bag_equip_slot}"))?;
        let bag_tmpl = ctx
            .db
            .game_item_template()
            .entry()
            .find(bag_inst.entry)
            .ok_or_else(|| "equipped bag has no template".to_string())?;
        if slot_in_bag >= bag_tmpl.container_slots.min(MAX_BAG_SIZE) {
            return Err(format!(
                "slot {} out of range for bag with {} slots",
                slot_in_bag, bag_tmpl.container_slots
            ));
        }
    }
    // The destination slot must be free; we never merge/swap on a split.
    if slot_occupied(ctx, player_guid, to_slot) {
        return Err("destination slot occupied".to_string());
    }
    inst.stack_count -= count;
    let entry = inst.entry;
    let owner_identity = inst.owner_identity;
    let durability = inst.durability;
    let soulbound = inst.soulbound; // the split half carries the SAME binding state as its source stack
    instances.guid().update(inst);
    let new_guid = next_item_guid(ctx, player_guid, to_slot);
    instances.insert(ItemInstance {
        guid: new_guid,
        entry,
        owner_identity,
        owner_guid: player_guid,
        slot: to_slot,
        stack_count: count,
        durability,
        created_at: ctx.timestamp,
        enchant_id: 0, // a split is only ever on a stackable (non-equippable) item → never enchanted
        soulbound,
    });
    Ok(())
}

/// The total `which` bonus from every piece of gear `owner_guid` has EQUIPPED — the sum of
/// `template_stat(.., which)` over the owner's item instances in the equipment region (slots 0..=18),
/// each joined to its `game_item_template`. The gear twin of the spell module's `stat_bonus`/`combat_field_bonus`
/// (which sum the matching auras): the combat module folds THIS alongside those aura sums into the same
/// effective-* helper, so equipping a +stat piece is mechanically real (it moves the swing/dodge/
/// mitigation/crit/hit readout) without any new readback. A CREATURE (or an unequipped player) has no
/// equipped item rows → the sum is 0, so its readout is byte-identical to before (baseline-safe). An item
/// whose template isn't loaded contributes 0 (a missing join never poisons the sum). [entity]
pub(crate) fn equipped_stat_bonus(ctx: &ReducerContext, owner_guid: u64, which: EquipStat) -> i32 {
    let templates = ctx.db.game_item_template();
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&owner_guid)
        .filter(|i| i.slot <= equip_slot::END) // equipment region only (0..=18); bags/backpack don't count
        .filter_map(|i| {
            let tmpl = templates.entry().find(i.entry)?;
            if item_is_broken(&tmpl, &i) {
                return None; // a broken item grants no stats until repaired
            }
            // The template stat PLUS this instance's per-instance ENCHANT overlay (ENCHANTING, completing
            // the 13). `enchant_stat` is 0 for an unenchanted item (enchant_id 0) → byte-identical readout
            // for every existing/unenchanted piece (baseline-safe). A broken item already returned above, so
            // a broken-but-enchanted piece grants neither — the enchant rides the item's working state.
            Some(template_stat(&tmpl, which) + enchant_stat(i.enchant_id, which))
        })
        .sum()
}

/// The pure per-item durability-loss formula `apply_death_durability_loss` applies to each equipped
/// piece: 10% of MAX durability (rounded down, at least 1), saturating so it never wraps below 0.
/// Returns the item UNCHANGED (no loss) when it has no durability concept (`max_durability == 0`) or is
/// already broken (`durability == 0`) — matching the live gate that skips both cases. Extracted from
/// `apply_death_durability_loss` (pure code-motion) so the formula is unit-tested without a live module.
pub(crate) fn death_durability_loss(max_durability: u32, durability: u32) -> u32 {
    if max_durability == 0 || durability == 0 {
        return durability; // no durability concept, or already broken — untouched
    }
    let loss = (max_durability / 10).max(1); // 10% of max, at least 1
    durability.saturating_sub(loss)
}

/// On a player's DEATH, every equipped item with a durability concept loses 10% of its MAX durability
/// (rounded down, at least 1) — vanilla's dominant durability sink, on top of the slow per-swing wear
/// (`combat::wear_weapon`). Saturates at 0 (a broken item gives unarmed/no-stats until repaired).
/// Player-death only — creatures don't wear gear. Collect-then-mutate (don't write the table mid-scan).
pub(crate) fn apply_death_durability_loss(ctx: &ReducerContext, player_guid: u64) {
    let instances = ctx.db.game_item_instance();
    let templates = ctx.db.game_item_template();
    let equipped: Vec<_> = instances
        .by_owner_guid()
        .filter(&player_guid)
        .filter(|i| i.slot <= equip_slot::END)
        .collect();
    for mut inst in equipped {
        if let Some(tmpl) = templates.entry().find(inst.entry) {
            let new_durability = death_durability_loss(tmpl.max_durability, inst.durability);
            if new_durability != inst.durability {
                inst.durability = new_durability;
                instances.guid().update(inst);
            }
        }
    }
}

/// Repair the item in `slot` back to its template's `max_durability` — the FREE restore the harness
/// drives via `debug_repair_item` (deterministic durability resets without money bookkeeping). The
/// PLAYER path is `apply_player_repair` below (NPC gate + cost + batch), which deliberately does not
/// share this body: its restore is fused with the debit so the whole tx rolls back together.
// Sole consumer is the feature-gated debug reducer — the `debug_only!` rule (actor.rs): silence
// dead-code ONLY in a default build; a debug_reducers build still flags it if the harness stops
// consuming it, which is the cue to delete it.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn apply_repair_item(
    ctx: &ReducerContext,
    owner_guid: u64,
    slot: u8,
) -> Result<(), String> {
    let mut inst =
        item_in_slot(ctx, owner_guid, slot).ok_or_else(|| format!("no item in slot {slot}"))?;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(inst.entry)
        .ok_or_else(|| format!("item {} has no template", inst.entry))?;
    if tmpl.max_durability == 0 {
        return Err(format!("item in slot {slot} has no durability to repair"));
    }
    inst.durability = tmpl.max_durability;
    ctx.db.game_item_instance().guid().update(inst);
    Ok(())
}

/// Sentinel `slot` for `apply_player_repair`: repair every equipped item (the whole body) rather than
/// one slot. The 1.12 client sends one `CMSG_REPAIR_ITEM` per damaged item (no repair-all bit), so the
/// live gateway path passes a specific slot; this is reachable via the reducer (harness / future convenience).
pub(crate) const REPAIR_ALL: u8 = u8::MAX;

/// Player repair: restore durability at a REPAIR-flagged NPC, charging copper. Mirrors the vendor gate
/// (`apply_item_sell`): the player must be alive, the `npc_guid` a real REPAIR creature on the same map
/// within `VENDOR_RANGE_SQ`. `slot == REPAIR_ALL` repairs every equipped item; otherwise just `slot`.
/// Cost (`rules::repair_cost` summed) is debited BEFORE the restore and the whole tx rolls back on Err
/// (never charge without repairing). A no-cost call (nothing damaged) is a successful no-op. [entity]
pub(crate) fn apply_player_repair(
    ctx: &ReducerContext,
    player_guid: u64,
    npc_guid: u64,
    slot: u8,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot repair".to_string());
    }
    // REPAIR-NPC gate — same shape as apply_item_sell's vendor gate, keyed on the REPAIR flag.
    let npc = entities
        .guid()
        .find(npc_guid)
        .ok_or_else(|| "no such armorer".to_string())?;
    if npc.is_player() || npc.npc_flags & lyracore_shared::constants::npc_flags::REPAIR == 0 {
        return Err("target cannot repair".to_string());
    }
    if npc.map_id != player.map_id || npc.instance_id != player.instance_id {
        return Err("armorer on another map".to_string());
    }
    let (dx, dy, dz) = (npc.x - player.x, npc.y - player.y, npc.z - player.z);
    if dx * dx + dy * dy + dz * dz > VENDOR_RANGE_SQ {
        return Err("armorer out of range".to_string());
    }

    let instances = ctx.db.game_item_instance();
    let templates = ctx.db.game_item_template();

    // Collect the damaged item(s) to repair: one slot, or every equipped item (slot 0..=END).
    let targets: Vec<ItemInstance> = instances
        .by_owner_guid()
        .filter(&player_guid)
        .filter(|i| {
            if slot == REPAIR_ALL {
                i.slot <= equip_slot::END
            } else {
                i.slot == slot
            }
        })
        .collect();
    if slot != REPAIR_ALL && targets.is_empty() {
        return Err(format!("no item in slot {slot}"));
    }

    // Sum the cost over items that actually have durability damage; skip no-durability items silently.
    let mut total_cost: u32 = 0;
    let mut to_repair: Vec<(ItemInstance, u32)> = Vec::new();
    for inst in targets {
        if let Some(tmpl) = templates.entry().find(inst.entry) {
            if tmpl.max_durability > 0 && inst.durability < tmpl.max_durability {
                total_cost =
                    total_cost.saturating_add(repair_cost(tmpl.max_durability, inst.durability));
                to_repair.push((inst, tmpl.max_durability));
            }
        }
    }
    if to_repair.is_empty() {
        return Ok(()); // nothing damaged — successful no-op (vanilla: button greyed)
    }
    if player.money < total_cost {
        return Err("not enough money to repair".to_string());
    }
    // Debit first, persist, THEN restore — Err anywhere rolls the whole tx back (no charge-without-repair).
    player.money -= total_cost;
    entities.guid().update(player);
    for (mut inst, max_dur) in to_repair {
        inst.durability = max_dur;
        instances.guid().update(inst);
    }
    Ok(())
}

/// Shared move/swap logic for the player + debug paths: move the item in `from_slot` to `to_slot`.
/// If `to_slot` holds an item too, the two SWAP slots; if it's empty, the item just moves. The item
/// GUID is its stable identity (only `slot` changes) — `item_guid_for`'s guid↔slot derivation is a
/// grant-time convenience, not an invariant the client relies on after that (it tracks items by guid).
/// One EXCEPTION to the swap (FEATURE B): if the destination holds the SAME stackable item
/// (`dst.entry == src.entry` and the template's `max_stack > 1`), the stacks MERGE instead of swapping
/// — `merge_amount` units flow from src into dst (capped by dst's headroom); src is deleted if drained,
/// else left with the remainder. A non-matching or non-stackable destination keeps the SWAP byte-for-byte.
/// Errors if `from_slot` is empty. A no-op when the slots are equal. Additive — touches only the two
/// item rows' `slot`/`stack_count` (and may delete the drained src on a full merge). [entity]
pub(crate) fn apply_item_move(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
    to_slot: u8,
) -> Result<(), String> {
    if from_slot == to_slot {
        return Ok(());
    }
    let instances = ctx.db.game_item_instance();
    let mut src = item_in_slot(ctx, player_guid, from_slot)
        .ok_or_else(|| format!("no item in slot {from_slot}"))?;
    // Reject an out-of-range destination. Valid slots: equipment 0..=18, bag-equip 19..=22,
    // backpack 23..=38, or a bag-content slot (120..=191) for items landing inside an equipped bag.
    // Anything outside these ranges (e.g. 39..=119 = bank/keyring we don't model, or 192..255) is
    // an inventory-overflow dupe vector from a modified client and is rejected.
    if !valid_dest_slot(to_slot) {
        return Err(format!("invalid destination slot {to_slot}"));
    }
    // If the destination is in the bag-content region, validate that the corresponding bag is
    // equipped and the slot is within its capacity (prevents stashing items in unequipped bags).
    if to_slot >= BAG_CONTENT_OFFSET {
        let (bag_idx, slot_in_bag) = bag_content_decompose(to_slot);
        let bag_equip_slot = BAG_SLOT_START + bag_idx;
        let bag_inst = item_in_slot(ctx, player_guid, bag_equip_slot)
            .ok_or_else(|| format!("no bag equipped in slot {bag_equip_slot}"))?;
        let bag_tmpl = ctx
            .db
            .game_item_template()
            .entry()
            .find(bag_inst.entry)
            .ok_or_else(|| "equipped bag has no template".to_string())?;
        if slot_in_bag >= bag_tmpl.container_slots.min(MAX_BAG_SIZE) {
            return Err(format!(
                "slot {} out of range for bag with {} slots",
                slot_in_bag, bag_tmpl.container_slots
            ));
        }
    }
    // Equip-validation: moving INTO any EQUIPMENT slot (0..=18) requires the SOURCE item's
    // `inventory_type` map to that slot (`can_equip_into`). A missing template fails closed (we can't
    // prove it's wearable). Slot 15 (main-hand) defers to the stricter `can_equip_mainhand` rule inside
    // `can_equip_into`; the other 18 slots use the general resolver. Non-equipment destinations (>18:
    // bag/backpack/bank) don't reach this branch — the move/swap/merge below run unconditionally.
    if to_slot <= equip_slot::END {
        // Dual Wield: a caster who has LEARNED spell 674 may equip a second one-hander into OFFHAND —
        // `can_equip_into` only accepts that combination when this is true.
        let can_dual_wield = crate::spell::knows_spell(
            ctx,
            player_guid,
            lyracore_shared::constants::dual_wield::SPELL_ID,
        );
        let tmpl = match ctx.db.game_item_template().entry().find(src.entry) {
            Some(tmpl)
                if can_equip_into(tmpl.class, tmpl.inventory_type, to_slot, can_dual_wield) =>
            {
                tmpl
            }
            _ => return Err(format!("cannot equip that item in slot {to_slot}")),
        };
        // Required-level gate: you can carry a too-high item in the bag, but can't EQUIP it. Read the
        // player entity just for its level here (the move path doesn't otherwise need it); a missing
        // entity fails closed. Seeded items are required_level 1, so this never trips for the loadout.
        let player = ctx
            .db
            .game_world_entity()
            .guid()
            .find(player_guid)
            .ok_or_else(|| "user not in world".to_string())?;
        if !meets_required_level(player.level, tmpl.required_level) {
            return Err(format!("requires level {}", tmpl.required_level));
        }
        // Proficiency gate: enforce class armor/weapon restrictions (e.g. a Mage can't equip
        // plate). The class is byte 1 of unit_bytes_0 (race | class<<8 | gender<<16 | power<<24).
        // Creatures (class 0) never call equip_item; fail closed for unknown classes.
        let player_class = player.class();
        if !can_equip_proficiency(player_class, tmpl.class, tmpl.subclass) {
            return Err(format!(
                "class {} lacks proficiency for item class {}/subclass {}",
                player_class, tmpl.class, tmpl.subclass
            ));
        }
        // BoE: a Bind-on-Equip item binds the FIRST time it lands on the body — not on pickup. Every
        // equip lands here (apply_equip_item and a direct manual move both route through this branch),
        // so this is the single BoE binding trigger. Idempotent: an already-bound item just stays
        // bound (no-op re-equip / re-swap).
        if binds_on_equip(tmpl.bonding) {
            src.soulbound = true;
        }
    }
    // If the destination is occupied, either MERGE (same stackable item) or SWAP (anything else).
    if let Some(mut dst) = item_in_slot(ctx, player_guid, to_slot) {
        // MERGE only when dropping onto the SAME entry AND the item is stackable (max_stack > 1).
        // A missing template can't be merged (we can't know max_stack) — fall through to SWAP, which
        // needs no template, so the move never wedges on unseeded item data.
        if dst.entry == src.entry {
            if let Some(tmpl) = ctx.db.game_item_template().entry().find(src.entry) {
                if tmpl.max_stack > 1 {
                    let moved = merge_amount(src.stack_count, dst.stack_count, tmpl.max_stack);
                    if moved > 0 {
                        dst.stack_count += moved;
                        instances.guid().update(dst);
                        if moved >= src.stack_count {
                            // The whole source stack flowed into dst → remove the now-empty src row.
                            instances.guid().delete(src.guid);
                        } else {
                            // Partial merge (dst hit max_stack) → leave the remainder in src, in place.
                            src.stack_count -= moved;
                            instances.guid().update(src);
                        }
                    }
                    // `moved == 0` (dst already full) is a no-op: both stacks stay exactly as they were,
                    // which matches vanilla refusing to merge into a full stack (no swap, no error).
                    return Ok(());
                }
            }
        }
        // Not a same-item stackable pair → the original SWAP: dst falls back into the source slot.
        dst.slot = from_slot;
        instances.guid().update(dst);
    }
    src.slot = to_slot;
    instances.guid().update(src);
    // Parity #8: if either endpoint is an EQUIPMENT slot (0..=18), gear just changed on the body, so
    // re-derive the owner's max HP/mana (recompute_vitals now folds equipped Stamina/Intellect). The
    // health bar grows when you equip a +Sta piece and shrinks when you take it off. A pure bag↔bag move
    // touches no equip slot → skipped, so loose-inventory shuffles are byte-identical. (recompute_vitals
    // is a no-op for non-players + when the derived max is unchanged.)
    if from_slot <= equip_slot::END || to_slot <= equip_slot::END {
        crate::spell::recompute_vitals(ctx, player_guid);
    }
    Ok(())
}

/// Shared EQUIP logic for the player + debug paths: take the item in backpack/inventory `from_slot` and
/// equip it into the correct `EQUIPMENT_SLOT_*` for its `inventory_type` (auto-resolved, including the
/// first-free of a finger/trinket pair). Rejects an item whose type isn't equippable (`inventory_type`
/// has no slot — e.g. food/junk). Delegates the actual placement/swap (and the required-level gate) to
/// the shared `apply_item_move`, so equipping is exactly "a move into the resolved equip slot": an empty
/// target just receives the item; an occupied target SWAPS its resident back into `from_slot`. Additive
/// — touches only the item rows' `slot` (no new fields). Errors if `from_slot` is empty, the template is
/// missing, or the item isn't equippable. [entity]
pub(crate) fn apply_equip_item(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
) -> Result<(), String> {
    let src = item_in_slot(ctx, player_guid, from_slot)
        .ok_or_else(|| format!("no item in slot {from_slot}"))?;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(src.entry)
        .ok_or_else(|| format!("no template for item entry {}", src.entry))?;
    // Bags (INVTYPE_BAG) equip into bag-equip slots 19..=22, not the equipment region 0..=18.
    // Route them separately: find the first free bag-equip slot and move the bag there. A bag
    // already in the bag slots (dragged manually) won't come through here, but autoequip does.
    if tmpl.inventory_type == invtype::BAG {
        let to_slot = first_free_bag_equip_slot(ctx, player_guid)
            .ok_or_else(|| "all four bag slots are full".to_string())?;
        return apply_item_move(ctx, player_guid, from_slot, to_slot);
    }
    // Dual Wield: redirect a second one-hander to OFFHAND (instead of swapping MAINHAND) when the
    // caster has learned spell 674 — see `resolve_equip_slot`'s doc comment.
    let can_dual_wield = crate::spell::knows_spell(
        ctx,
        player_guid,
        lyracore_shared::constants::dual_wield::SPELL_ID,
    );
    // Snapshot which equip slots are occupied so the pair-resolver can prefer a free finger/trinket.
    let to_slot = resolve_equip_slot(tmpl.inventory_type, can_dual_wield, |s| {
        slot_occupied(ctx, player_guid, s)
    })
    .ok_or_else(|| format!("item {} is not equippable", src.entry))?;
    // Equip == a validated move into the resolved equip slot (reuses the equip-validation + swap there).
    apply_item_move(ctx, player_guid, from_slot, to_slot)
}

/// Shared UNEQUIP logic for the player + debug paths: take the item in equipment `from_slot` (0..=18)
/// and move it to the first free backpack slot. Rejects a non-equipment source slot (it's not equipped)
/// or a full backpack. Delegates to `apply_item_move` so the placement is the exact same move primitive;
/// the destination is a backpack slot, so the equip-validation branch there is a no-op for it. Additive.
pub(crate) fn apply_unequip_item(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
) -> Result<(), String> {
    if from_slot > equip_slot::END {
        return Err(format!("slot {from_slot} is not an equipment slot"));
    }
    // Must actually hold an equipped item to unequip.
    if !slot_occupied(ctx, player_guid, from_slot) {
        return Err(format!("no item equipped in slot {from_slot}"));
    }
    let free =
        first_free_backpack_slot(ctx, player_guid).ok_or_else(|| "inventory full".to_string())?;
    apply_item_move(ctx, player_guid, from_slot, free)
}

/// The loose-inventory ("backpack") slot range in this minimal model: 16 slots, 23..=38, mirroring
/// `starter_item::BACKPACK_SLOT_0` (23). Equip slots (0..=18) and bag-container slots aren't loose
/// storage, so auto-store only ever lands an item in the backpack. Expressed as an exclusive 23..39.
const BACKPACK_SLOT_END: u8 = starter_item::BACKPACK_SLOT_0 + 16; // 39

/// First of the four bag-equip slots (the bags themselves live here; NOT their contents).
const BAG_SLOT_START: u8 = 19;
/// Last bag-equip slot (inclusive). Vanilla has four: 19..=22.
const BAG_SLOT_END_INCL: u8 = 22;
/// Number of bag-equip slots.
const BAG_SLOT_COUNT: u8 = 4;
/// Base of the flat bag-content slot space. All values from here to `BAG_CONTENT_END - 1` are
/// used exclusively for items stored INSIDE equipped bags. The range starts above the largest
/// vanilla `ItemSlot` ordinal (Keyring32 = 112), so `ItemSlot::try_from(slot)` returns `Err`
/// for bag-content slots — the gateway correctly sends no `PLAYER_FIELD_INV_SLOT` pointer for
/// them (items in bags are tracked via the container object, not the player descriptor).
const BAG_CONTENT_OFFSET: u8 = 120;
/// Maximum container slots any bag can hold — vanilla's largest (Portable Hole) is 18 slots.
/// Each bag-equip position (19..=22) is assigned this many flat content slots.
const MAX_BAG_SIZE: u8 = 18;
/// Exclusive upper bound of the bag-content region: 120 + 4 × 18 = 192. Slots 120..=191.
const BAG_CONTENT_END: u8 = BAG_CONTENT_OFFSET + BAG_SLOT_COUNT * MAX_BAG_SIZE; // 192

/// The anti-dupe destination-slot gate shared by `apply_item_move` and `apply_item_split`: a valid
/// destination is either the flat equip+bag-equip+backpack range (0..=38) or a bag-content slot
/// (120..=191). Anything else (39..=119 = bank/keyring we don't model, or 192..255) is an
/// inventory-overflow dupe vector from a modified client and is rejected. Extracted (pure code-motion,
/// deduplicating the two identical inline expressions) so the slot-range boundaries are unit-tested
/// without a live module.
pub(crate) fn valid_dest_slot(to_slot: u8) -> bool {
    to_slot < BACKPACK_SLOT_END // 0..=38 (equip + bag-equip + backpack)
        || (BAG_CONTENT_OFFSET..BAG_CONTENT_END).contains(&to_slot) // 120..=191
}

/// Decompose a bag-content slot (120..=191) into `(bag_idx, slot_in_bag)`: which of the four equipped
/// bags (0..=3) and which position within it. Extracted from `apply_item_move` / `apply_item_split`
/// (pure code-motion, deduplicating the two identical inline expressions) — the caller adds `bag_idx` to
/// `BAG_SLOT_START` to find the bag's equip slot. Only meaningful for `to_slot >= BAG_CONTENT_OFFSET`
/// (the caller guards that); this does not itself validate the range.
pub(crate) fn bag_content_decompose(to_slot: u8) -> (u8, u8) {
    let offset = to_slot - BAG_CONTENT_OFFSET;
    (offset / MAX_BAG_SIZE, offset % MAX_BAG_SIZE)
}

/// A split must leave at least one unit in BOTH the source and the new stack — splitting off none
/// (`count == 0`) or the whole stack (`count >= stack_count`, that's a move) is rejected. Extracted from
/// `apply_item_split` (pure code-motion) so the count-gate boundaries are unit-tested without a live
/// module.
pub(crate) fn valid_split_count(count: u32, stack_count: u32) -> bool {
    count != 0 && count < stack_count
}

/// First backpack slot (23..39) not occupied by any of the player's items, or `None` if the backpack
/// is full. Pure scan over the owner's item rows — vanilla auto-store fills the first free bag slot.
fn first_free_backpack_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    let instances = ctx.db.game_item_instance();
    (starter_item::BACKPACK_SLOT_0..BACKPACK_SLOT_END).find(|&slot| {
        instances
            .by_owner_guid()
            .filter(&player_guid)
            .all(|i| i.slot != slot)
    })
}

/// First free bag-equip slot (19..=22) not currently holding a bag, or `None` if all four are
/// occupied. Used by `apply_equip_item` to auto-resolve where to drop a bag. Pure scan.
fn first_free_bag_equip_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    (BAG_SLOT_START..=BAG_SLOT_END_INCL).find(|&slot| !slot_occupied(ctx, player_guid, slot))
}

/// First free content slot across all equipped bags (19..=22), scanning in bag-equip order (bag 1
/// first). Returns `None` if every equipped bag is full or no bags are equipped. Used by
/// `store_item` as the fallback after the 16-slot backpack is exhausted — bags thus act as
/// overflow storage. A bag equip slot with no item is skipped; a bag whose template is missing
/// or has `container_slots == 0` (not a real bag) is also skipped. [entity]
fn first_free_bag_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    let instances = ctx.db.game_item_instance();
    let templates = ctx.db.game_item_template();
    for bag_idx in 0..BAG_SLOT_COUNT {
        let bag_equip_slot = BAG_SLOT_START + bag_idx;
        let Some(bag_inst) = instances
            .by_owner_guid()
            .filter(&player_guid)
            .find(|i| i.slot == bag_equip_slot)
        else {
            continue;
        };
        let Some(bag_tmpl) = templates.entry().find(bag_inst.entry) else {
            continue;
        };
        let slots_in_bag = bag_tmpl.container_slots.min(MAX_BAG_SIZE);
        if slots_in_bag == 0 {
            continue;
        }
        let base = BAG_CONTENT_OFFSET + bag_idx * MAX_BAG_SIZE;
        for si in 0..slots_in_bag {
            let content_slot = base + si;
            if instances
                .by_owner_guid()
                .filter(&player_guid)
                .all(|i| i.slot != content_slot)
            {
                return Some(content_slot);
            }
        }
    }
    None
}

/// Shared take-an-item-from-a-corpse logic for the player + debug paths (`CMSG_AUTOSTORE_LOOT_ITEM`):
/// move one `game_corpse_loot` item into the looter's backpack. Validates the looter is in world and
/// alive, the corpse exists / is dead / is on the same map; resolves the loot row at `loot_slot`, mints
/// a fresh owned `ItemInstance` in the first free backpack slot, then DELETES the consumed loot row.
/// Re-derives the corpse `LOOTABLE` flag after consuming (`loot::refresh_lootable` — cleared only
/// when no rows remain AND money is 0). Errors if the looter is dead, the corpse is
/// missing/alive/on another map, the slot has no loot, the item template is missing, or the backpack is
/// full. Additive — inserts one item row and deletes one loot row. [entity]
pub(crate) fn apply_take_loot(
    ctx: &ReducerContext,
    player_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "looter not in world".to_string())?;
    // Death is server-authoritative everywhere — a dead/ghost looter can't take loot (mirrors loot_money).
    if player.dead {
        return Err("dead players cannot loot".to_string());
    }
    // The loot source is EITHER a dead creature corpse (game_world_entity) OR a used gameobject
    // (game_gameobject — a chest, whose `use_gameobject` rolled rows into game_corpse_loot keyed on its
    // guid). Resolve the source position for the range gate; a missing/alive corpse or an unknown guid
    // is rejected. The dead-creature branch is byte-identical to before.
    let (src_map, src_instance, sx, sy, sz, source_is_corpse) =
        if let Some(corpse) = entities.guid().find(corpse_guid) {
            if !corpse.dead {
                return Err("target is not a corpse".to_string());
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
            return Err("no such loot source".to_string());
        };
    if src_map != player.map_id {
        return Err("loot on another map".to_string());
    }
    // Instance gate (190 slice 2 review HIGH): instances overlay IDENTICAL coordinates, so the
    // range gate below is routinely satisfiable across the instance wall — party B's looter in
    // their own Deadmines stands within 10yd of party A's corpse/chest position. This was the ONE
    // loot path with no slice-1 DEFERRED marker, so the gate sweep missed it: the module is the
    // authority regardless of what any client shows.
    if src_instance != player.instance_id {
        return Err("loot in another instance".to_string());
    }
    // Range gate (anti-exploit), mirroring the money path `loot_money` — a client can't autostore a
    // corpse's item from across the map. Shares the same `LOOT_RANGE_SQ` (10 yd)² so the loot paths
    // never drift.
    let (dx, dy, dz) = (sx - player.x, sy - player.y, sz - player.z);
    if dx * dx + dy * dy + dz * dz > crate::loot::LOOT_RANGE_SQ {
        return Err("loot out of range".to_string());
    }
    // The specific loot-window row the client asked for. `by_corpse` then match the slot index.
    let loot = ctx.db.game_corpse_loot();
    let row = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == loot_slot)
        .ok_or_else(|| "no loot in that slot".to_string())?;
    // Quest-only rows (work-item 187 slice 0): the TAKER's OWN need is re-validated server-side (the
    // gateway's window is a display hint, not authoritative) — an unreserved row (`reserved_for == 0`,
    // the shared row nobody has split yet) is claimable by anyone who currently needs it; an already
    // per-member-reserved clone is claimable ONLY by its reserved owner. A non-quest row (the common
    // case) skips this entirely — unconditional FFA, byte-identical to before.
    if row.quest_only {
        let needs = crate::loot::killer_needs_item(ctx, Some(player_guid), row.item_entry);
        if !crate::loot::quest_take_allowed(row.reserved_for, player_guid, needs) {
            return Err(
                if row.reserved_for != 0 && row.reserved_for != player_guid {
                    "this item is reserved for another player".to_string()
                } else {
                    "you do not need this quest item".to_string()
                },
            );
        }
    } else if !crate::loot::group_loot_take_allowed(
        row.withheld,
        row.reserved_for,
        row.master_only,
        row.designated_looter_guid,
        player_guid,
    ) {
        // Group loot methods (work-item 187 slices 2-4): a NEED/GREED winner-locked row
        // (`reserved_for`), a MASTER-only row, or a round-robin/below-threshold row designated to
        // someone else all reject the plain autostore path here — server-authoritative, the gateway's
        // per-viewer loot-window filter (`reads.rs`) is a display hint only.
        return Err(if row.master_only {
            "this item requires the master looter to distribute it".to_string()
        } else {
            "this item belongs to another looter right now".to_string()
        });
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(row.item_entry)
        .ok_or_else(|| format!("no template for item entry {}", row.item_entry))?;
    // Auto-store with stacking (parity #14): a looted stackable tops up a matching partial stack first,
    // then spills into free slots (visible only to its owner via owner_identity). `?` so an inventory-full
    // loot rolls back and the loot row stays for a retry.
    store_item(
        ctx,
        player_guid,
        player.owner_identity,
        &tmpl,
        row.count.max(1),
        false,
    )?;
    // Consume the loot: remove the row so a second take can't dupe it.
    let (item_entry, count) = (row.item_entry, row.count.max(1));
    let (was_quest_only, was_unreserved) = (row.quest_only, row.reserved_for == 0);
    loot.id().delete(row.id);
    // Per-member cloning (work-item 187 slice 0): the FIRST take of a still-shared quest_only row
    // splits it — every OTHER grouped member who still needs the item gets their own independently
    // reserved clone at a fresh slot, so the item doesn't vanish for them the instant this player
    // takes theirs. A solo player (no group) or an already-reserved clone (a later take) is a no-op —
    // see `clone_quest_loot_for_group`.
    if was_quest_only && was_unreserved {
        crate::loot::clone_quest_loot_for_group(ctx, player_guid, corpse_guid, item_entry, count);
    }
    // The flag follows the rule (rows remain OR money > 0) — taking the LAST item of a no-money
    // corpse must drop the loot cursor. Gameobject sources carry no dynamic flag; skip them.
    if source_is_corpse {
        crate::loot::refresh_lootable(ctx, corpse_guid);
    }
    // Notify-hook: one item stack looted (player + debug loot both route through this core).
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

#[cfg(test)]
mod tests {
    use super::{
        bag_content_decompose, bandage_cooldown_blocks, buyback_newest_first, buyback_ring_full,
        death_durability_loss, use_spell_for, valid_dest_slot, valid_split_count,
        RECENTLY_BANDAGED_SPELL,
    };

    /// The data-driven consumable→spell map: the two crafted consumables (real items) resolve to their
    /// on-use spell; every other item (food/water/drink/reagent/unknown) resolves to `None` so it falls
    /// through to the legacy vital-restore branch (the water/drink/eat path stays untouched — no regression).
    /// Guards the `USE_EFFECTS` wiring against a silent edit, exactly like `recipe_for`'s lookup test.
    #[test]
    fn use_spell_map_resolves_the_mapped_consumables_only() {
        assert_eq!(use_spell_for(118), Some(50110)); // Minor Healing Potion (real 118) -> instant heal
        assert_eq!(use_spell_for(1251), Some(50111)); // Linen Bandage (real 1251) -> channeled HoT
                                                      // ENGINEERING bomb (completing the 13): Rough Copper Bomb (real 4360, class-7 Trade Good) -> the
                                                      // AoE-damage on-use spell 50096. This is what makes its `apply_item_use` gate admit a non-consumable.
        assert_eq!(use_spell_for(4360), Some(50096));
        // CONSUMABLE BREADTH: the new mapped consumables (all real vanilla items) resolve to their fresh
        // 50113-50118 on-use spells. Two pairs share ONE spell (water 159/5350 → drink; bread/jerky
        // 4540/117 → food), proving the by-entry lookup fans both onto the same cast.
        assert_eq!(use_spell_for(2455), Some(50113)); // Minor Mana Potion -> E_ENERGIZE 160 mana
        assert_eq!(use_spell_for(159), Some(50114)); // Refreshing Spring Water -> drink (periodic energize)
        assert_eq!(use_spell_for(5350), Some(50114)); // Conjured Water -> same drink spell
        assert_eq!(use_spell_for(4540), Some(50115)); // Tough Hunk of Bread -> food (periodic heal)
        assert_eq!(use_spell_for(117), Some(50115)); // Tough Jerky -> same food spell
        assert_eq!(use_spell_for(2680), Some(50116)); // Spiced Wolf Meat -> Well Fed +Sta/+Spi
        assert_eq!(use_spell_for(858), Some(50117)); // Lesser Healing Potion -> E_HEAL 160 (rank-2)
        assert_eq!(use_spell_for(2581), Some(50118)); // Heavy Linen Bandage -> channeled HoT 144 (rank-2)
                                                      // Roasted Boar Meat (real 2681, a level-1 food) is NOT mapped -> None -> the legacy eat branch runs
                                                      // (vanilla-faithful: no Well-Fed buff on a level-1 food — the Well-Fed payoff rides the NEW Cooking
                                                      // product 2680 instead). Linen Cloth (2589) is a reagent (no entry), and unknown ids are likewise
                                                      // unmapped -> the legacy restore branch runs (baseline-safe).
        assert_eq!(use_spell_for(2681), None);
        assert_eq!(use_spell_for(2589), None);
        assert_eq!(use_spell_for(0), None);
        assert_eq!(use_spell_for(u32::MAX), None);
    }

    /// The re-bandage cooldown gate (the pure half — `has_aura` is the ctx half): a BANDAGE use is blocked
    /// ONLY while the "Recently Bandaged" (11196) debuff is live; once it expires the bandage is usable
    /// again. NO other consumable is cooldown-gated — a potion/food use is never blocked, even if a stray
    /// 11196 aura were present (only the bandage entry gates). This is the load-bearing anti-spam gate.
    #[test]
    fn bandage_cooldown_gate_blocks_only_the_bandage_while_debuff_is_live() {
        // BOTH bandages (real items 1251 Linen + 2581 Heavy Linen) are blocked iff the cooldown debuff is up
        // and usable once it has expired — Gate B: vanilla shares ONE "Recently Bandaged" lockout across all
        // bandages, so 1251/2581 can't be alternated to bypass the window.
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
        // Potion (real 118) and food (real 2681) are NEVER cooldown-gated — the gate is bandage-only.
        assert!(!bandage_cooldown_blocks(118, true));
        assert!(!bandage_cooldown_blocks(2681, true));
        // The gate keys on the real seeded debuff id (a drift guard if 11196 ever moves).
        assert_eq!(RECENTLY_BANDAGED_SPELL, 11196);
    }

    /// Potion heal CLAMPS to max health — the potion routes through `E_HEAL`→`apply_heal`→`healed_value`,
    /// so a heal that would overflow max is capped (no over-heal past the pool) and a heal below max lands
    /// in full. Drives the SAME `healed_value` the live cast uses with the seeded potion magnitude (100),
    /// so the clamp the verify recipe checks (health rises by ~100, never above max) is proven here.
    #[test]
    fn potion_heal_clamps_to_max_health() {
        use crate::spell::healed_value;
        const POTION_HEAL: i32 = 80; // game_spell_effect 200440 base_points (vanilla Minor Healing ~70-90, fixed 80)
                                     // From 1 HP with plenty of headroom: full +80 lands.
        assert_eq!(healed_value(1, 500, POTION_HEAL), 81);
        // Near max: the heal is CLAMPED to max (no overheal past the pool).
        assert_eq!(healed_value(450, 500, POTION_HEAL), 500);
        // Already at max: stays at max.
        assert_eq!(healed_value(500, 500, POTION_HEAL), 500);
    }

    /// CONSUMABLE BREADTH — the MANA POTION (2455→50113) ENERGIZES, clamped to max power. The potion routes
    /// through `E_ENERGIZE`→`energized_value` with the seeded magnitude (160 = vanilla Restore Mana midpoint
    /// 140-180). Drives the SAME `energized_value` the live cast uses, so the verify recipe's "power rises by
    /// 160, never above max_power" is proven here: full restore with headroom, clamp at max, no overflow.
    #[test]
    fn mana_potion_energizes_and_clamps_to_max_power() {
        use crate::spell::energized_value;
        const POTION_MANA: i32 = 160; // game_spell_effect 200452 base_points (Restore Mana 140-180, midpoint 160)
                                      // From near-empty with plenty of headroom: the full +160 lands.
        assert_eq!(energized_value(40, 500, POTION_MANA), 200);
        // Near max: the restore is CLAMPED to max_power (no overflow past the pool).
        assert_eq!(energized_value(400, 500, POTION_MANA), 500);
        // Already at max: stays at max.
        assert_eq!(energized_value(500, 500, POTION_MANA), 500);
    }

    /// CONSUMABLE BREADTH — the WELL-FED buff (Spiced Wolf Meat 2680→50116) is an `A_MOD_STAT` whose +Stamina
    /// effect MOVES the max-health pool: `aura_moves_vitals(A_MOD_STAT, STAT_STA)` is true, so applying the
    /// buff re-derives max HP (the Cooking payoff is mechanically live). The +Spirit effect is summed by
    /// `stat_bonus` but does NOT move vitals (no max-pool consumer for SPI) — staged/inert, like Mark of the
    /// Wild's non-STA stats. This guards the kind/p0 the Well-Fed seed must carry to be a real HP buff.
    #[test]
    fn well_fed_stamina_moves_max_health_spirit_is_inert() {
        use crate::spell::{aura_moves_vitals, A_MOD_STAT, STAT_SPI, STAT_STA};
        assert!(aura_moves_vitals(A_MOD_STAT, STAT_STA as i32)); // +Sta grows max HP on apply
        assert!(!aura_moves_vitals(A_MOD_STAT, STAT_SPI as i32)); // +Spi is summed but moves no pool (inert)
    }

    /// CONSUMABLE BREADTH — the DRINK (water 159/5350→50114) SCHEDULES a periodic energize: an
    /// `A_PERIODIC_ENERGIZE` aura that the scheduler folds `energized_value` into `power` each tick. Modeling
    /// the 6 ticks (40 mana / 5s × 6 = 240 over 30s), power climbs +40 per tick and CLAMPS at max — exactly
    /// the over-time restore the verify recipe polls. Proves the per-tick fold + the clamp without a ctx.
    #[test]
    fn drink_periodic_energize_climbs_per_tick_and_clamps() {
        use crate::spell::energized_value;
        const DRINK_PER_TICK: i32 = 41; // game_spell_effect 200456 base_points (Drink 430 base 41, mana / 5s)
                                        // Six ticks from empty with headroom → +246 total (41 × 6), each tick a clean +41.
        let mut power = 0u32;
        for _ in 0..6 {
            power = energized_value(power, 500, DRINK_PER_TICK);
        }
        assert_eq!(power, 246);
        // A tick near the cap clamps to max_power (no overflow on the last tick of a near-full drinker).
        assert_eq!(energized_value(480, 500, DRINK_PER_TICK), 500);
    }

    /// DEATH DURABILITY LOSS: 10% of MAX durability, rounded down, floored at 1 — and saturating (never
    /// wraps below 0). A 0-durability item (already broken) or a no-durability-concept template
    /// (`max_durability == 0`) is left UNTOUCHED, matching the live gate that skips both.
    #[test]
    fn death_durability_loss_is_ten_percent_of_max_floored_at_one_and_saturating() {
        assert_eq!(death_durability_loss(100, 100), 90); // 10% of 100 = 10
        assert_eq!(death_durability_loss(10, 10), 9); // 10% of 10 = 1 (exact)
        assert_eq!(death_durability_loss(5, 5), 4); // 10% of 5 floors to 0 -> floored up to 1
        assert_eq!(death_durability_loss(100, 5), 0); // loss (10) saturates at 0, never wraps
        assert_eq!(death_durability_loss(100, 0), 0); // already broken -> untouched
        assert_eq!(death_durability_loss(0, 50), 50); // no durability concept -> untouched
    }

    /// SPLIT COUNT GATE: `count == 0` and `count == stack_count` (splitting off nothing, or the whole
    /// stack — that's a move) are rejected; every count strictly between 0 and the stack passes.
    #[test]
    fn valid_split_count_rejects_zero_and_the_whole_stack_only() {
        const STACK: u32 = 5;
        assert!(!valid_split_count(0, STACK)); // splitting off nothing isn't a split
        assert!(!valid_split_count(STACK, STACK)); // splitting off the whole stack is a move, not a split
        assert!(!valid_split_count(STACK + 1, STACK)); // over the stack is never valid either
        for count in 1..STACK {
            assert!(
                valid_split_count(count, STACK),
                "count {count} of {STACK} should be a valid split"
            );
        }
    }

    /// ANTI-DUPE DESTINATION-SLOT GATE (`apply_item_move` / `apply_item_split`): the equip+bag-equip+
    /// backpack range (0..=38) and the bag-content range (120..=191) are valid; the unmodeled bank/keyring
    /// gap (39..=119) and anything past the bag-content region (192+) are rejected.
    #[test]
    fn valid_dest_slot_admits_the_two_modeled_ranges_only() {
        // Inside 0..=38 (equip 0..=18, bag-equip 19..=22, backpack 23..=38).
        for slot in [0u8, 18, 19, 22, 23, 38] {
            assert!(valid_dest_slot(slot), "slot {slot} is in the 0..=38 range");
        }
        // The exclusive boundary just past 38, and the unmodeled bank/keyring gap.
        assert!(!valid_dest_slot(39));
        assert!(!valid_dest_slot(119));
        // Inside the bag-content region 120..=191.
        assert!(valid_dest_slot(120));
        assert!(valid_dest_slot(191));
        // Just past the bag-content region.
        assert!(!valid_dest_slot(192));
        assert!(!valid_dest_slot(255));
    }

    /// BAG-CONTENT SLOT DECOMPOSITION: a flat bag-content slot (120..=191) decomposes into
    /// `(bag_idx, slot_in_bag)` — 18 slots per bag, in bag-equip order.
    #[test]
    fn bag_content_decompose_maps_flat_slot_to_bag_index_and_position() {
        assert_eq!(bag_content_decompose(120), (0, 0)); // first slot of the first bag
        assert_eq!(bag_content_decompose(137), (0, 17)); // last slot of the first bag (18 slots: 120..=137)
        assert_eq!(bag_content_decompose(138), (1, 0)); // first slot of the second bag
        assert_eq!(bag_content_decompose(191), (3, 17)); // last slot of the fourth (last) bag
    }

    /// BUYBACK RING FIFO EVICTION: the ring evicts its oldest entry once it already holds 12 (so the
    /// 13th sale pushes one out and the ring never grows past 12); below that it never evicts.
    #[test]
    fn buyback_ring_full_evicts_at_twelve_not_before() {
        assert!(!buyback_ring_full(0));
        assert!(!buyback_ring_full(11));
        assert!(buyback_ring_full(12));
        assert!(buyback_ring_full(13));
    }

    /// BUYBACK NEWEST-FIRST SLOT ORDER: re-purchase slot 0 is the most recently sold item (the highest
    /// auto_inc id), descending from there — independent of the ids' original (insertion) order.
    #[test]
    fn buyback_newest_first_orders_ids_descending_by_auto_inc() {
        assert_eq!(buyback_newest_first(vec![5, 1, 9, 3]), vec![9, 5, 3, 1]);
        assert_eq!(buyback_newest_first(vec![]), Vec::<u64>::new());
        assert_eq!(buyback_newest_first(vec![7]), vec![7]);
    }
}
