//! The `apply_*` mutation cores that don't belong to the vendor economy (`economy.rs`) or the
//! move/split/equip slot machinery (`inventory.rs`, #387 split): grant / use items, take corpse loot,
//! the starter-loadout grant, and the equipped-stat sum. Each is the shared core behind a thin player
//! reducer and its debug twin (see `reducers.rs`). All effects are additive: they touch only the item
//! rows + the actor's health/money.

use spacetimedb::{Identity, ReducerContext, Table};

use lyracore_shared::constants::starter_item;

use crate::game_character; // the durable char holds `class` (the live WorldEntity does not)
use crate::game_corpse_loot; // the loot.rs accessor trait — re-exported at crate root (`pub use loot::*`)
use crate::game_gameobject;
use crate::game_spell_effect; // Gate A reads the on-use spell's effect kinds (by_spell) to mana-gate energize consumables
use crate::game_start_item; // per-class starter loadout (CharStartOutfit via importer --dbc)
use crate::game_world_entity; // gameobject (chest) loot source — apply_take_loot resolves it too

use super::inventory::{first_free_backpack_slot, first_free_bag_slot, is_carried_slot};
use super::rules::{
    binds_on_grant, enchant_stat, equip_slot, meets_required_level, merge_amount,
    resolve_equip_slot, template_stat, EquipStat,
};
use super::tables::{
    game_item_instance, game_item_template, item_guid_for, item_in_slot, item_is_broken,
    next_item_guid, ItemInstance, ItemTemplate,
};

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
    let player = crate::helpers::live_entity(ctx, player_guid)
        .map_err(|_| "player not in world".to_string())?;
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
pub(crate) fn store_item(
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
            // Carried stacks only — loot never merges into a banked stack.
            .filter(|i| {
                is_carried_slot(i.slot) && i.entry == tmpl.entry && i.stack_count < max_stack
            })
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
        let slot = free_slot(ctx, player_guid)?;
        let take = count.min(max_stack);
        let new_guid = next_item_guid(ctx, player_guid, slot);
        instances.insert(ItemInstance {
            guid: new_guid,
            entry: tmpl.entry,
            owner_identity,
            owner_guid: player_guid,
            slot,
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

/// The next slot an incoming item can land in: backpack first (23..=38), then the content slots of
/// equipped bags in bag-equip order. `Err` is the full-bag refusal, and it is the ONE the whole
/// server answers with — [`store_item`] and [`store_instance_state`] both come here, so a mail take
/// and a vendor buy cannot disagree about whether there is room.
pub(crate) fn free_slot(ctx: &ReducerContext, player_guid: u64) -> Result<u8, String> {
    first_free_backpack_slot(ctx, player_guid)
        .or_else(|| first_free_bag_slot(ctx, player_guid))
        .ok_or_else(|| lyracore_shared::mail::INVENTORY_FULL.to_string())
}

/// Is there anywhere for one more item to land? [`free_slot`]'s boolean twin, for a caller that
/// wants the answer BEFORE it starts moving value it cannot put back — the sharded mail take probes
/// the recipient's shard with this so a full bag refuses the click instead of stranding the item in
/// an escrow.
pub(crate) fn has_free_slot(ctx: &ReducerContext, player_guid: u64) -> bool {
    free_slot(ctx, player_guid).is_ok()
}

/// Put ONE item into `player_guid`'s bags carrying an exact recorded state — the same entry, stack,
/// durability, enchant and bind state it had before it was taken away.
///
/// This is the other half of a snapshot move (mail today, trade next): the source deletes its
/// `game_item_instance` row and records these columns, and this re-creates them. Re-granting from
/// the TEMPLATE instead would hand back a repaired, unenchanted item — a free repair and an
/// enchant-laundering exploit — which is the whole reason a snapshot exists.
///
/// One row into one free slot, never a merge into an existing partial stack: a merge would blend
/// two instances' bind states, and the stack it merged into would silently absorb a soulbound one.
/// Room is the item module's own [`free_slot`], so the full-bag refusal is the same one every other
/// grant path answers with and the caller's transaction rolls back on it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn store_instance_state(
    ctx: &ReducerContext,
    player_guid: u64,
    owner_identity: Identity,
    tmpl: &ItemTemplate,
    preallocated_guid: Option<u64>,
    stack_count: u32,
    durability: u32,
    enchant_id: u32,
    soulbound: bool,
) -> Result<(), String> {
    let slot = free_slot(ctx, player_guid)?;
    // Trade allocates before deleting either side's outgoing rows: otherwise an emptied inventory
    // can re-mint a guid deleted earlier in the same transaction and the item relay sees UPDATE
    // instead of CREATE. Mail can safely allocate at insertion time.
    let guid = preallocated_guid.unwrap_or_else(|| next_item_guid(ctx, player_guid, slot));
    ctx.db.game_item_instance().insert(ItemInstance {
        guid,
        entry: tmpl.entry,
        owner_identity,
        owner_guid: player_guid,
        slot,
        // Clamped to the template's own stack cap: the snapshot is durable data that outlives a
        // template edit, and a row above the cap would render as an impossible stack.
        stack_count: stack_count.max(1).min(tmpl.max_stack.max(1)),
        durability,
        created_at: ctx.timestamp,
        enchant_id,
        // The recorded bind state, ORed with the template's grant-time rule — `store_item`'s
        // expression exactly, so an arriving item cannot end up less bound than a granted one.
        soulbound: soulbound || binds_on_grant(tmpl.bonding),
    });
    Ok(())
}

/// Total quantity of `item_entry` the player owns across ALL stacks (bag + equipped) — drives
/// collect-quest completion (parity #4). Sums `stack_count` over the owner's matching item rows. [entity]
pub(crate) fn item_count(ctx: &ReducerContext, owner_guid: u64, item_entry: u32) -> u32 {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&owner_guid)
        // Banked copies don't count toward a collect quest.
        .filter(|i| is_carried_slot(i.slot) && i.entry == item_entry)
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
        // A turn-in consumes carried stacks only; the bank is never silently emptied.
        .filter(|i| is_carried_slot(i.slot) && i.entry == item_entry)
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

/// "Recently Bandaged" debuff spell id (vanilla 11196) — already seeded (a 60s `A_FLAG` marker aura, no
/// tick). The bandage's eff1 `E_TRIGGER` applies it, and `apply_item_use` gates on `has_aura(.., this)`
/// BEFORE consuming/casting so a player can't re-bandage while it is live (vanilla's anti-spam window).
/// Reusing the existing 11196 row avoids seeding a new cooldown spell entirely.
pub(crate) const RECENTLY_BANDAGED_SPELL: u32 = 11196;

/// The on-use spell for `tmpl`, or `None` if it carries no on-use effect. `spellid_1`/`spelltrigger_1`
/// (`ItemTemplate`, shipped to the client for tooltips) are the SINGLE on-use authority (#387, finishing
/// the migration `rules.rs` used to name as the end state): a nonzero `spellid_1` whose trigger slot is
/// `ItemSpellTriggerType::OnUse` (0) IS the item's on-use cast. This retires the old hardcoded
/// `USE_EFFECTS` shadow-map (a duplicate of exactly this column) — the importer/seed data populate the
/// column directly, so a newly-imported consumable works with zero code, and `apply_item_use` reads
/// this ONE thing instead of two competing mechanisms. Pure — unit-tested. The `begin_cast` core then
/// routes the spell instant/channel/buff off its OWN header (no per-item code).
fn use_spell_for(tmpl: &ItemTemplate) -> Option<u32> {
    (tmpl.spellid_1 != 0 && tmpl.spelltrigger_1 == 0).then_some(tmpl.spellid_1)
}

/// Does the on-use `spell_id` teleport the caster HOME (`E_RECALL_HOME`, kind 0x1F — the Hearthstone's
/// shape, #387)? The data-driven twin of `spell_restores_power`'s mana gate: `apply_item_use` reads this
/// to skip the stack-consumption every OTHER on-use spell takes — a recall trinket is never used up,
/// unlike a potion/bandage/food. Reads `game_spell_effect` by `by_spell` — `true` if ANY effect carries
/// the kind. Keyed on the SPELL's effect kind, not the item's entry id, so any future recall item (not
/// just the Hearthstone) is automatically non-consuming. [entity]
fn spell_is_recall_home(ctx: &ReducerContext, spell_id: u32) -> bool {
    ctx.db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .any(|e| e.kind == crate::spell::E_RECALL_HOME)
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

/// Shared use-an-item logic for the player + debug paths: resolve the item -> run its gates -> consume
/// one unit (unless it's a non-consuming recall item) -> `begin_cast` its `spellid_1` (#387). `spellid_1`
/// (trigger 0) is the SINGLE on-use authority — this retired the old three-mechanism `apply_item_use`
/// (a hardcoded Hearthstone special case, the `USE_EFFECTS` shadow-map, and a legacy vital-restore
/// fallback with its own duplicated mana gate): every usable item, consumable or not (a potion, a
/// bandage, the Engineering bomb, the Hearthstone), now resolves through ONE lookup and ONE cast path.
/// Errors if the user is dead, the slot is empty, the template is missing, or the item carries no on-use
/// spell. Additive — touches only the item row(s) + whatever `begin_cast`'s effect list touches. [entity]
pub(crate) fn apply_item_use(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot use items".to_string());
    }
    let instances = ctx.db.game_item_instance();
    let mut inst =
        item_in_slot(ctx, player_guid, slot).ok_or_else(|| format!("no item in slot {slot}"))?;
    // A banked item must be taken out before it can be used — the bank is not a second action bar.
    if !is_carried_slot(inst.slot) {
        return Err("cannot use a banked item".to_string());
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(inst.entry)
        .ok_or_else(|| format!("no template for item entry {}", inst.entry))?;
    // spellid_1 (trigger 0) is the item's on-use spell — the single authority now (#387). No mapped
    // spell means nothing to "use" (a plain reagent, a piece of gear, an un-migrated item).
    let spell_id =
        use_spell_for(&tmpl).ok_or_else(|| format!("item {} is not consumable", inst.entry))?;
    // Required-level gate: a too-high item can sit in the bag, but can't be USED. Seeded items are
    // required_level 1, so this never trips for the loadout.
    if !meets_required_level(player.level, tmpl.required_level) {
        return Err(format!("requires level {}", tmpl.required_level));
    }
    // RE-BANDAGE GATE (vanilla "Recently Bandaged"): refuse the BANDAGE while its cooldown debuff is
    // live — BEFORE consuming the item or casting, so a blocked re-bandage costs nothing. `has_aura`
    // keys on the existing 11196 marker (applied by the bandage's E_TRIGGER eff1). Only the bandage is
    // gated; every other consumable skips it (pure `bandage_cooldown_blocks` decides).
    if bandage_cooldown_blocks(
        inst.entry,
        crate::spell::has_aura(ctx, player_guid, RECENTLY_BANDAGED_SPELL),
    ) {
        return Err("Recently Bandaged".to_string());
    }
    // GATE A (the one surviving mana-class gate, #387 retired its legacy-branch duplicate): a mana potion
    // / drink restores POWER through E_ENERGIZE / A_PERIODIC_ENERGIZE — for a non-mana class that is
    // rage/energy, the out-of-combat preload exploit. `begin_cast` has no power-class gate of its own, so
    // DENY a power-restoring on-use spell here, BEFORE consuming/casting, when the user isn't a mana
    // class. Only energize spells are gated (`spell_restores_power`); a heal/HoT/food/buff/recall is
    // class-agnostic (Warriors can quaff a healing potion / eat / hearth).
    if spell_restores_power(ctx, spell_id) && !is_mana_class(ctx, player_guid) {
        return Err("only mana users can use that".to_string());
    }
    // Consume one unit FIRST — UNLESS this is a permanent recall item (the Hearthstone): a recall trinket
    // is never used up, so `spell_is_recall_home` is the ONE data-driven exception to "using an item
    // consumes it" (keyed on the spell's effect kind, not a hardcoded item entry). Every other on-use
    // spell consumes: the cast is the effect, and a failed cast still consumed the item, matching vanilla
    // — a fizzled potion is gone.
    if !spell_is_recall_home(ctx, spell_id) {
        if inst.stack_count > 1 {
            inst.stack_count -= 1;
            instances.guid().update(inst);
        } else {
            instances.guid().delete(inst.guid);
        }
    }
    // Cast at SELF. `begin_cast` runs the full effect list, emits the cast-START/GO events the gateway
    // relays, and routes channeled (bandage) vs instant (potion/food/recall) on the spell header — no
    // per-item branching here.
    crate::spell::begin_cast(
        ctx,
        player_guid,
        spell_id,
        player.level as u8,
        player_guid,
        false,
        None,
    )
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
        bandage_cooldown_blocks, death_durability_loss, use_spell_for, RECENTLY_BANDAGED_SPELL,
    };

    /// `use_spell_for` (#387): `spellid_1` is the on-use spell IFF it's nonzero AND `spelltrigger_1` names
    /// the on-use trigger slot (0 — `ItemSpellTriggerType::OnUse`). A trigger-1 (on-equip) spell is NOT
    /// an on-use cast even with a nonzero id, and a zero `spellid_1` (a plain reagent/gear piece/
    /// un-migrated item) resolves to `None` regardless of its trigger byte. Replaces the old hardcoded
    /// `USE_EFFECTS` shadow-map drift guard — now there is nothing to drift, `spellid_1` IS the data.
    #[test]
    fn use_spell_for_reads_spellid_1_gated_on_the_on_use_trigger() {
        let mut tmpl = crate::seed::base_item(1, "x");
        tmpl.spellid_1 = 50110;
        tmpl.spelltrigger_1 = 0;
        assert_eq!(use_spell_for(&tmpl), Some(50110)); // nonzero + trigger 0 -> the on-use cast

        tmpl.spellid_1 = 0;
        assert_eq!(use_spell_for(&tmpl), None); // no spell at all

        tmpl.spellid_1 = 50110;
        tmpl.spelltrigger_1 = 1; // trigger 1 = on-equip, NOT on-use
        assert_eq!(use_spell_for(&tmpl), None);
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
}
