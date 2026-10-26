use spacetimedb::{ReducerContext, Table};

use crate::game_npc_vendor;
use crate::game_world_entity;
use crate::WorldEntity;

use super::inventory::is_carried_slot;
use super::ops::{store_item, StoreRefusal};
use super::rules::{buy_cost, equip_slot, repair_cost, sell_value};
use super::tables::{
    game_character_buyback, game_item_instance, game_item_template, item_in_slot, BuybackEntry,
    ItemInstance,
};

// Squared yards.
const VENDOR_RANGE_SQ: f32 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NpcGate {
    CharacterAbsent,
    CharacterDead,
    NpcAbsent,
    WrongService,
    NpcDead,
    OtherMap,
    OutOfRange,
    Unfriendly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NpcRefusal {
    pub(crate) gate: NpcGate,
    message: String,
}

impl NpcRefusal {
    pub(crate) fn new(gate: NpcGate, message: impl Into<String>) -> Self {
        Self {
            gate,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for NpcRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

impl From<NpcRefusal> for String {
    fn from(refusal: NpcRefusal) -> Self {
        refusal.message
    }
}

impl From<NpcRefusal> for crate::Refusal {
    fn from(refusal: NpcRefusal) -> Self {
        refusal.message.into()
    }
}

pub(crate) fn npc_interaction_gate(
    ctx: &ReducerContext,
    player_guid: u64,
    npc_guid: u64,
    required_flag: u32,
    noun: &str,
    flag_err: &str,
    dead_verb: &str,
) -> Result<(WorldEntity, WorldEntity), NpcRefusal> {
    let entities = ctx.db.game_world_entity();
    let player = entities
        .guid()
        .find(player_guid)
        .ok_or_else(|| NpcRefusal::new(NpcGate::CharacterAbsent, "user not in world"))?;
    if player.dead {
        return Err(NpcRefusal::new(
            NpcGate::CharacterDead,
            format!("dead players cannot {dead_verb}"),
        ));
    }
    let npc = entities
        .guid()
        .find(npc_guid)
        .ok_or_else(|| NpcRefusal::new(NpcGate::NpcAbsent, format!("no such {noun}")))?;
    if npc.is_player() || npc.npc_flags & required_flag == 0 {
        return Err(NpcRefusal::new(NpcGate::WrongService, flag_err));
    }
    if npc.dead || npc.health == 0 {
        return Err(NpcRefusal::new(NpcGate::NpcDead, format!("{noun} is dead")));
    }
    if npc.map_id != player.map_id || npc.instance_id != player.instance_id {
        return Err(NpcRefusal::new(
            NpcGate::OtherMap,
            format!("{noun} on another map"),
        ));
    }
    if crate::helpers::dist_sq(&player, &npc) > VENDOR_RANGE_SQ {
        return Err(NpcRefusal::new(
            NpcGate::OutOfRange,
            format!("{noun} out of range"),
        ));
    }
    if crate::reputation::npc_refuses_interaction(ctx, &npc, &player) {
        return Err(NpcRefusal::new(
            NpcGate::Unfriendly,
            format!("{noun} refuses interaction"),
        ));
    }
    Ok((player, npc))
}

pub(crate) fn bank_access_gate(ctx: &ReducerContext, player_guid: u64) -> Result<(), String> {
    use lyracore_shared::constants::npc_flags;
    flagged_npc_in_reach_gate(
        ctx,
        player_guid,
        npc_flags::BANKER,
        "banker",
        "use the bank",
    )
}

pub(crate) fn innkeeper_access_gate(
    ctx: &ReducerContext,
    character_guid: u64,
    innkeeper_guid: u64,
) -> Result<(), String> {
    npc_interaction_gate(
        ctx,
        character_guid,
        innkeeper_guid,
        lyracore_shared::constants::npc_flags::INNKEEPER,
        "innkeeper",
        "target is not an innkeeper",
        "bind a home",
    )?;
    Ok(())
}

fn flagged_npc_in_reach_gate(
    ctx: &ReducerContext,
    player_guid: u64,
    flag: u32,
    noun: &str,
    verb: &str,
) -> Result<(), String> {
    let player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(player_guid)
        .ok_or_else(|| "user not in world".to_string())?;
    if player.dead {
        return Err(format!("dead players cannot {verb}"));
    }
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
        flagged_npc_in_reach(
            flag,
            e.is_player(),
            e.npc_flags,
            e.dead,
            dx * dx + dy * dy + dz * dz,
        ) && e.health != 0
            && !crate::reputation::npc_refuses_interaction(ctx, e, &player)
    });
    if !open {
        return Err(format!("{noun} out of range"));
    }
    Ok(())
}

pub(crate) fn flagged_npc_in_reach(
    flag: u32,
    is_player: bool,
    npc_flags: u32,
    dead: bool,
    dist_sq: f32,
) -> bool {
    !is_player && !dead && npc_flags & flag != 0 && dist_sq <= VENDOR_RANGE_SQ
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BankSlotRefusal {
    AlreadyMaxed,
    CannotAfford { price: u32 },
    Banker(NpcRefusal),
}

// Gateway Verbs return this text, led by the SMSG_BUY_BANK_SLOT_RESULT code.
impl std::fmt::Display for BankSlotRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use lyracore_shared::bank::result;
        match self {
            Self::AlreadyMaxed => write!(
                f,
                "[{}] no bank bag slots left to buy",
                result::FAILED_TOO_MANY
            ),
            Self::CannotAfford { price } => write!(
                f,
                "[{}] not enough money (need {price})",
                result::INSUFFICIENT_FUNDS
            ),
            Self::Banker(refusal) => write!(f, "[{}] {refusal}", result::NOT_BANKER),
        }
    }
}

impl From<BankSlotRefusal> for String {
    fn from(refusal: BankSlotRefusal) -> Self {
        refusal.to_string()
    }
}

pub(crate) fn apply_buy_bank_slot(
    ctx: &ReducerContext,
    player_guid: u64,
    banker_guid: u64,
) -> Result<(), BankSlotRefusal> {
    let (mut player, _banker) = npc_interaction_gate(
        ctx,
        player_guid,
        banker_guid,
        lyracore_shared::constants::npc_flags::BANKER,
        "banker",
        "target is not a banker",
        "use the bank",
    )
    .map_err(BankSlotRefusal::Banker)?;
    let price = lyracore_shared::bank::next_bank_slot_price(player.bank_bag_slots)
        .ok_or(BankSlotRefusal::AlreadyMaxed)?;
    if player.money < price {
        return Err(BankSlotRefusal::CannotAfford { price });
    }
    player.money -= price;
    player.bank_bag_slots += 1;
    player.player_bytes_2 =
        lyracore_shared::packing::with_bank_bag_slots(player.player_bytes_2, player.bank_bag_slots);
    player.update(ctx);
    Ok(())
}

pub(crate) fn vendor_sells(ctx: &ReducerContext, vendor_entry: u32, item_entry: u32) -> bool {
    ctx.db
        .game_npc_vendor()
        .by_vendor()
        .filter(&vendor_entry)
        .any(|v| v.item_entry == item_entry)
}

pub(crate) fn buyback_ring_full(current_len: usize) -> bool {
    current_len >= 12
}

pub(crate) fn buyback_newest_first(mut ids: Vec<u64>) -> Vec<u64> {
    ids.sort_by(|a, b| b.cmp(a));
    ids
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SellRefusal {
    Vendor(NpcRefusal),
    NoItemInSlot(u8),
    Refused(crate::Refusal),
}

impl From<String> for SellRefusal {
    fn from(cause: String) -> Self {
        Self::Refused(cause.into())
    }
}

impl From<crate::Refusal> for SellRefusal {
    fn from(refusal: crate::Refusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<NpcRefusal> for SellRefusal {
    fn from(refusal: NpcRefusal) -> Self {
        Self::Vendor(refusal)
    }
}

impl std::fmt::Display for SellRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vendor(refusal) => refusal.fmt(f),
            Self::NoItemInSlot(slot) => write!(f, "no item in slot {slot}"),
            Self::Refused(refusal) => refusal.fmt(f),
        }
    }
}

impl From<SellRefusal> for String {
    fn from(refusal: SellRefusal) -> Self {
        refusal.to_string()
    }
}

pub(crate) fn apply_item_sell(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    slot: u8,
) -> Result<(), SellRefusal> {
    apply_item_sell_count(ctx, player_guid, vendor_guid, slot, 0)
}

pub(crate) fn apply_item_sell_count(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    slot: u8,
    count: u32,
) -> Result<(), SellRefusal> {
    let (mut player, _vendor) = npc_interaction_gate(
        ctx,
        player_guid,
        vendor_guid,
        lyracore_shared::constants::npc_flags::VENDOR,
        "vendor",
        "target is not a vendor",
        "sell",
    )?;
    let mut inst = item_in_slot(ctx, player_guid, slot).ok_or(SellRefusal::NoItemInSlot(slot))?;
    let count = if count == 0 { inst.stack_count } else { count };
    if count > inst.stack_count {
        return Err("invalid sell count".to_string().into());
    }
    if crate::trade::item_is_offered(ctx, player_guid, inst.guid) {
        return Err(super::refused(lyracore_shared::item::ItemRefusal::NotRightNow).into());
    }
    if inst.slot <= equip_slot::END {
        return Err("cannot sell an equipped item".to_string().into());
    }
    if !is_carried_slot(inst.slot) {
        return Err("cannot sell a banked item".to_string().into());
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(inst.entry)
        .ok_or_else(|| format!("no template for item entry {}", inst.entry))?;
    if tmpl.sell_price == 0 {
        return Err("item has no sell value".to_string().into());
    }
    let earned = sell_value(tmpl.sell_price, count);
    player.money = player.money.saturating_add(earned);
    player.update(ctx);
    let buyback_tab = ctx.db.game_character_buyback();
    let mut old: Vec<BuybackEntry> = buyback_tab.by_player_guid().filter(&player_guid).collect();
    old.sort_by_key(|e| e.id);
    if buyback_ring_full(old.len()) {
        buyback_tab.id().delete(old[0].id);
    }
    buyback_tab.insert(BuybackEntry {
        id: 0,
        player_guid,
        item_entry: inst.entry,
        stack_count: count,
        price: earned,
        soulbound: inst.soulbound,
        random_property_id: inst.random_property_id,
    });
    if count == inst.stack_count {
        crate::ItemInstance::delete(ctx, inst.guid);
    } else {
        inst.stack_count -= count;
        inst.update(ctx);
    }
    Ok(())
}

pub(crate) fn apply_buyback_item(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    slot_idx: u8,
) -> Result<(), crate::Refusal> {
    let (mut player, _vendor) = npc_interaction_gate(
        ctx,
        player_guid,
        vendor_guid,
        lyracore_shared::constants::npc_flags::VENDOR,
        "vendor",
        "target is not a vendor",
        "use buyback",
    )?;
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
        )
        .into());
    }
    let owner_identity = player.owner_identity;
    player.money -= entry.price;
    player.update(ctx);
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
        Some(entry.random_property_id),
    )?;
    buyback_tab.id().delete(entry.id);
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuyRefusal {
    Vendor(NpcRefusal),
    NotEnoughMoney { have: u32, need: u32 },
    InventoryFull,
    Refused(crate::Refusal),
}

impl From<String> for BuyRefusal {
    fn from(cause: String) -> Self {
        Self::Refused(cause.into())
    }
}

impl From<crate::Refusal> for BuyRefusal {
    fn from(refusal: crate::Refusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<NpcRefusal> for BuyRefusal {
    fn from(refusal: NpcRefusal) -> Self {
        Self::Vendor(refusal)
    }
}

impl From<StoreRefusal> for BuyRefusal {
    fn from(refusal: StoreRefusal) -> Self {
        match refusal {
            StoreRefusal::InventoryFull => Self::InventoryFull,
            StoreRefusal::Refused(refusal) => Self::Refused(refusal),
        }
    }
}

impl std::fmt::Display for BuyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vendor(refusal) => refusal.fmt(f),
            Self::NotEnoughMoney { have, need } => {
                write!(f, "not enough money: have {have}, need {need}")
            }
            Self::InventoryFull => lyracore_shared::mail::INVENTORY_FULL.fmt(f),
            Self::Refused(refusal) => refusal.fmt(f),
        }
    }
}

impl From<BuyRefusal> for String {
    fn from(refusal: BuyRefusal) -> Self {
        refusal.to_string()
    }
}

pub(crate) fn apply_buy_item(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), BuyRefusal> {
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
        return Err("invalid count".to_string().into());
    }
    if !vendor_sells(ctx, vendor.entry, item_entry) {
        return Err("vendor does not sell that item".to_string().into());
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(item_entry)
        .ok_or_else(|| format!("no such item {item_entry}"))?;
    if tmpl.buy_price == 0 {
        return Err("item cannot be bought".to_string().into());
    }
    let cost = {
        let base = buy_cost(tmpl.buy_price, count);
        let pct = crate::reputation::vendor_discount_pct(ctx, player_guid, vendor.faction_template);
        base - base.saturating_mul(pct) / 100
    };
    if player.money < cost {
        return Err(BuyRefusal::NotEnoughMoney {
            have: player.money,
            need: cost,
        });
    }
    // Propagate storage failure so the reducer rolls back the debit.
    player.money = player.money.saturating_sub(cost);
    let owner_identity = player.owner_identity;
    player.update(ctx);
    Ok(store_item(
        ctx,
        player_guid,
        owner_identity,
        &tmpl,
        count,
        false,
        None,
    )?)
}

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
    inst.update(ctx);
    Ok(())
}

pub(crate) const REPAIR_ALL: u8 = u8::MAX;

pub(crate) fn apply_player_repair(
    ctx: &ReducerContext,
    player_guid: u64,
    npc_guid: u64,
    slot: u8,
) -> Result<(), String> {
    let (mut player, _npc) = npc_interaction_gate(
        ctx,
        player_guid,
        npc_guid,
        lyracore_shared::constants::npc_flags::REPAIR,
        "armorer",
        "target cannot repair",
        "repair",
    )?;
    let instances = ctx.db.game_item_instance();
    let templates = ctx.db.game_item_template();

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
        return Ok(());
    }
    if player.money < total_cost {
        return Err("not enough money to repair".to_string());
    }
    // Debit and repair must commit in the same transaction.
    player.money -= total_cost;
    player.update(ctx);
    for (mut inst, max_dur) in to_repair {
        inst.durability = max_dur;
        inst.update(ctx);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        buyback_newest_first, buyback_ring_full, flagged_npc_in_reach, BankSlotRefusal, NpcGate,
        NpcRefusal, VENDOR_RANGE_SQ,
    };
    use lyracore_shared::constants::npc_flags::{BANKER, VENDOR};

    #[test]
    fn banker_in_reach_accepts_only_a_live_banker_inside_the_radius() {
        let edge = VENDOR_RANGE_SQ;
        let banker_in_reach =
            |is_player, flags, dead, d| flagged_npc_in_reach(BANKER, is_player, flags, dead, d);
        assert!(banker_in_reach(false, BANKER, false, 0.0));
        assert!(banker_in_reach(false, BANKER | VENDOR, false, edge));
        assert!(!banker_in_reach(true, BANKER, false, 0.0));
        assert!(!banker_in_reach(false, BANKER, true, 0.0));
        assert!(!banker_in_reach(false, VENDOR, false, 0.0));
        assert!(!banker_in_reach(false, 0, false, 0.0));
        assert!(!banker_in_reach(false, BANKER, false, edge + 1.0));
    }

    #[test]
    fn bank_slot_refusal_text_leads_with_the_buy_bank_slot_result_code() {
        let out_of_range = NpcRefusal::new(NpcGate::OutOfRange, "banker out of range");
        for (refusal, text) in [
            (
                BankSlotRefusal::AlreadyMaxed,
                "[0] no bank bag slots left to buy",
            ),
            (
                BankSlotRefusal::CannotAfford { price: 10_000 },
                "[1] not enough money (need 10000)",
            ),
            (
                BankSlotRefusal::Banker(out_of_range),
                "[2] banker out of range",
            ),
        ] {
            assert_eq!(String::from(refusal), text);
        }
    }

    #[test]
    fn buyback_ring_full_evicts_at_twelve_not_before() {
        assert!(!buyback_ring_full(0));
        assert!(!buyback_ring_full(11));
        assert!(buyback_ring_full(12));
        assert!(buyback_ring_full(13));
    }

    #[test]
    fn buyback_newest_first_orders_ids_descending_by_auto_inc() {
        assert_eq!(buyback_newest_first(vec![5, 1, 9, 3]), vec![9, 5, 3, 1]);
        assert_eq!(buyback_newest_first(vec![]), Vec::<u64>::new());
        assert_eq!(buyback_newest_first(vec![7]), vec![7]);
    }
}
