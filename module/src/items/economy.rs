//! The vendor/armorer economy — sell / buyback / buy / repair, plus the shared NPC-interaction trust
//! gate all four reduce to (#387: split off `ops.rs`, pure code-motion, no behavior change). Each
//! `apply_*` is the shared core behind a thin player reducer and its debug twin (see `reducers.rs`).

use spacetimedb::{ReducerContext, Table};

use crate::game_npc_vendor;
use crate::game_world_entity;
use crate::WorldEntity; // npc_interaction_gate's return type

use super::inventory::is_carried_slot;
use super::ops::store_item;
use super::rules::{buy_cost, equip_slot, repair_cost, sell_value};
use super::tables::{
    game_character_buyback, game_item_instance, game_item_template, item_in_slot, BuybackEntry,
    ItemInstance,
};

/// Max distance to interact with a vendor: (10 yd)². The client walks into range before sending
/// `CMSG_BUY_ITEM`, so this only rejects clearly-out-of-range abuse (mirrors `loot::LOOT_RANGE_SQ`).
const VENDOR_RANGE_SQ: f32 = 100.0;

/// The shared trust-boundary gate for a player-initiated NPC interaction — vendor sell/buyback/buy and
/// armorer repair all reduce to the SAME five checks: the player must be alive, the target must be a
/// real NPC (never another player, `is_player()`) carrying `required_flag`, on the player's own
/// map+instance, within `VENDOR_RANGE_SQ`. Extracted (issue #372, "the exact place drift happens" — the
/// 190 review already caught one copy of this shape missing the instance check once) from
/// `apply_item_sell` / `apply_buyback_item` / `apply_buy_item` / `apply_player_repair`, which used to
/// paste this ~20-line block four times.
///
/// The four call sites' WIRE-VISIBLE error text differs in wording, not in the checks themselves, so
/// three of the five messages stay parameterized rather than folded into one template:
///   * `dead_verb` — the dead-player message's verb ("sell" / "use buyback" / "buy" / "repair"),
///     formatted as `"dead players cannot {dead_verb}"`.
///   * `noun` — the NPC-kind word used in the not-found / map / range messages ("vendor" / "armorer"),
///     formatted as `"no such {noun}"` / `"{noun} on another map"` / `"{noun} out of range"`.
///   * `flag_err` — the flag-mismatch message verbatim, since its wording does NOT follow the `noun`
///     template (repair's is `"target cannot repair"`, not `"target is not a armorer"`).
///
/// Every string this replaces is preserved byte-for-byte at each call site — `build_buy_failed`
/// (gateway/src/codec/item.rs) substring-matches `"out of range"` / `"another map"` / `"not enough
/// money"` on these paths, so the SHAPE of each message (not just its presence) must survive.
fn npc_interaction_gate(
    ctx: &ReducerContext,
    player_guid: u64,
    npc_guid: u64,
    required_flag: u32,
    noun: &str,
    flag_err: &str,
    dead_verb: &str,
) -> Result<(WorldEntity, WorldEntity), String> {
    let entities = ctx.db.game_world_entity();
    let player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    if player.dead {
        return Err(format!("dead players cannot {dead_verb}"));
    }
    let npc = entities
        .guid()
        .find(npc_guid)
        .ok_or_else(|| format!("no such {noun}"))?;
    if npc.is_player() || npc.npc_flags & required_flag == 0 {
        return Err(flag_err.to_string());
    }
    if npc.map_id != player.map_id || npc.instance_id != player.instance_id {
        return Err(format!("{noun} on another map"));
    }
    let (dx, dy, dz) = (npc.x - player.x, npc.y - player.y, npc.z - player.z);
    if dx * dx + dy * dy + dz * dz > VENDOR_RANGE_SQ {
        return Err(format!("{noun} out of range"));
    }
    Ok((player, npc))
}

/// The bank-access trust gate: a move or split touching a bank slot needs an OPEN bank, so the player
/// must be alive with a live BANKER npc in reach. Unlike the vendor paths the client names no banker
/// guid, so this searches the player's own partition instead of validating a named target.
pub(crate) fn bank_access_gate(ctx: &ReducerContext, player_guid: u64) -> Result<(), String> {
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    if player.dead {
        return Err("dead players cannot use the bank".to_string());
    }
    // Grid-indexed and partition-scoped: a spatial table must never be scanned whole.
    let near = crate::helpers::entities_near(
        ctx,
        player.map_id,
        player.instance_id,
        player.x,
        player.y,
        VENDOR_RANGE_SQ.sqrt(),
    );
    let open = near.iter().any(|e| {
        let (dx, dy, dz) = (e.x - player.x, e.y - player.y, e.z - player.z);
        banker_in_reach(
            e.is_player(),
            e.npc_flags,
            e.dead,
            dx * dx + dy * dy + dz * dz,
        )
    });
    if !open {
        return Err("banker out of range".to_string());
    }
    Ok(())
}

/// Does one nearby entity open the bank? A live NPC (never another player) carrying the BANKER flag,
/// inside the interaction radius. Pure so the refusal cases are unit-tested without a live module.
pub(crate) fn banker_in_reach(is_player: bool, npc_flags: u32, dead: bool, dist_sq: f32) -> bool {
    !is_player
        && !dead
        && npc_flags & lyracore_shared::constants::npc_flags::BANKER != 0
        && dist_sq <= VENDOR_RANGE_SQ
}

/// Does the vendor creature `vendor_entry` stock `item_entry`? Gates `apply_buy_item` so a player can
/// only buy what a vendor actually sells (not any item with a buy price). Reads `game_npc_vendor`.
pub(crate) fn vendor_sells(ctx: &ReducerContext, vendor_entry: u32, item_entry: u32) -> bool {
    ctx.db
        .game_npc_vendor()
        .by_vendor()
        .filter(&vendor_entry)
        .any(|v| v.item_entry == item_entry)
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
    // Vendor gating, mirroring apply_buy_item/apply_buyback_item/apply_player_repair (issue #372's
    // shared npc_interaction_gate): a dead/ghost player can't vendor; the sale must be to a real
    // VENDOR creature within range on the same map+instance. Unlike buy there is NO vendor_sells()
    // check — vanilla lets you sell any sellable item to any vendor (junk goes to anyone), only the
    // proximity + "is a vendor" gates apply.
    let (mut player, _vendor) = npc_interaction_gate(
        ctx,
        player_guid,
        vendor_guid,
        lyracore_shared::constants::npc_flags::VENDOR,
        "vendor",
        "target is not a vendor",
        "sell",
    )?;
    let entities = ctx.db.game_world_entity();
    let instances = ctx.db.game_item_instance();
    let inst =
        item_in_slot(ctx, player_guid, slot).ok_or_else(|| format!("no item in slot {slot}"))?;
    // Only BACKPACK/bag items are sellable. Refuse an EQUIPPED item (slots 0..=18): vanilla's vendor
    // window can't target worn gear, so a client must not be able to sell items off the body.
    if inst.slot <= equip_slot::END {
        return Err("cannot sell an equipped item".to_string());
    }
    // Selling is a bag-only action: an item sitting in the bank is out of the vendor window's reach.
    if !is_carried_slot(inst.slot) {
        return Err("cannot sell a banked item".to_string());
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
    let (mut player, _vendor) = npc_interaction_gate(
        ctx,
        player_guid,
        vendor_guid,
        lyracore_shared::constants::npc_flags::VENDOR,
        "vendor",
        "target is not a vendor",
        "use buyback",
    )?;
    let entities = ctx.db.game_world_entity();
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
    // Death is server-authoritative everywhere — a dead/ghost player can't shop (mirrors the other
    // paths). Vendor gating: the purchase must come from a real VENDOR creature the player is
    // standing at, and that vendor must actually stock the item — you can't buy arbitrary items or
    // from a non-vendor.
    //
    // ORDERING NOTE (issue #372): before this extraction, `count == 0` was checked BETWEEN the dead
    // check and the vendor-resolution gate; `npc_interaction_gate` folds dead+vendor into one atomic
    // check, so `count == 0` now runs AFTER the vendor/range gate instead. This only changes which
    // error string comes back for a MALFORMED packet that combines count=0 with a bogus/out-of-range
    // vendor — the real client's Buy dialog never sends count=0, so no legitimate interaction changes.
    let (mut player, vendor) = npc_interaction_gate(
        ctx,
        player_guid,
        vendor_guid,
        lyracore_shared::constants::npc_flags::VENDOR,
        "vendor",
        "target is not a vendor",
        "buy",
    )?;
    if count == 0 {
        return Err("invalid count".to_string());
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
    ctx.db.game_world_entity().guid().update(player);
    store_item(ctx, player_guid, owner_identity, &tmpl, count, false)
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
    // REPAIR-NPC gate — same shape as apply_item_sell's vendor gate, keyed on the REPAIR flag
    // (issue #372's shared npc_interaction_gate).
    let (mut player, _npc) = npc_interaction_gate(
        ctx,
        player_guid,
        npc_guid,
        lyracore_shared::constants::npc_flags::REPAIR,
        "armorer",
        "target cannot repair",
        "repair",
    )?;
    let entities = ctx.db.game_world_entity();
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

#[cfg(test)]
mod tests {
    use super::{banker_in_reach, buyback_newest_first, buyback_ring_full, VENDOR_RANGE_SQ};
    use crate::test_scan::code_of;
    use lyracore_shared::constants::npc_flags::{BANKER, VENDOR};

    /// BANK ACCESS: only a live BANKER-flagged NPC inside the interaction radius opens the bank. A
    /// player, a corpse, a non-banker NPC, and a banker one yard too far all leave it shut — so a move
    /// into a bank slot with no banker in range is refused.
    #[test]
    fn banker_in_reach_accepts_only_a_live_banker_inside_the_radius() {
        let edge = VENDOR_RANGE_SQ;
        assert!(banker_in_reach(false, BANKER, false, 0.0));
        assert!(banker_in_reach(false, BANKER | VENDOR, false, edge));
        assert!(!banker_in_reach(true, BANKER, false, 0.0)); // another player, however flagged
        assert!(!banker_in_reach(false, BANKER, true, 0.0)); // a banker's corpse
        assert!(!banker_in_reach(false, VENDOR, false, 0.0)); // wrong npc kind
        assert!(!banker_in_reach(false, 0, false, 0.0)); // no npc flags at all
        assert!(!banker_in_reach(false, BANKER, false, edge + 1.0)); // out of range
    }

    /// The banker search is a spatial query, so it must go through the partition-scoped, grid-indexed
    /// helper — a whole-table read would see only the caller's own shard after a split.
    #[test]
    fn bank_access_gate_searches_through_the_partition_scoped_helper() {
        let body = code_of(
            include_str!("economy.rs"),
            "pub(crate) fn bank_access_gate(",
        );
        assert!(
            body.contains("crate::helpers::entities_near("),
            "the banker proximity search must use `helpers::entities_near`"
        );
        assert!(
            body.contains("player.dead"),
            "a dead player must not reach the bank"
        );
    }

    /// #514: this crate has no `ReducerContext` harness by design (`test_scan`'s doc comment /
    /// playbook §7), so `apply_player_repair`'s actual gating/cost/durability-restore behavior
    /// against real table state is verified live via the wire harness, not here — the same boundary
    /// every other reducer in this module lives behind (see `combat/swing.rs`'s chokepoint tests for
    /// the same disclosure). This pins the two invariants a source-text scan CAN catch: the debit
    /// happens (and is persisted) before the durability restore loop — so a rolled-back transaction
    /// never leaves a charge without a repair — and every collected target actually gets its
    /// durability written back to its template's max, not just summed into the cost.
    #[test]
    fn apply_player_repair_debits_before_it_restores_every_target() {
        let src = include_str!("economy.rs");
        let body = code_of(src, "pub(crate) fn apply_player_repair(");
        let debit_at = body
            .find("player.money -= total_cost;")
            .expect("apply_player_repair must debit money before persisting");
        let persist_at = body
            .find("entities.guid().update(player);")
            .expect("the debited player must be persisted");
        let restore_at = body
            .find("inst.durability = max_dur;")
            .expect("apply_player_repair must restore durability to the template max");
        let restore_persist_at = body
            .find("instances.guid().update(inst);")
            .expect("the restored item must be persisted");
        assert!(
            debit_at < persist_at && persist_at < restore_at && restore_at < restore_persist_at,
            "expected debit → persist → restore → persist, in that order"
        );
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
