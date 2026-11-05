use spacetimedb::ReducerContext;

use crate::game_npc_vendor;
use crate::game_world_entity;
use crate::WorldEntity;

use super::inventory::is_carried_slot;
use super::rules::{buy_cost, equip_slot, repair_cost, sell_value};
use super::tables::{
    game_item_instance, game_item_template, item_in_slot, ItemInstance, ItemTemplate, NpcVendor,
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

pub(crate) fn vendor_offers(
    ctx: &ReducerContext,
    vendor_entry: u32,
) -> Vec<(NpcVendor, ItemTemplate)> {
    let mut offers: Vec<_> = ctx
        .db
        .game_npc_vendor()
        .by_vendor()
        .filter(vendor_entry)
        .filter_map(|offer| {
            ctx.db
                .game_item_template()
                .entry()
                .find(offer.item_entry)
                .map(|template| (offer, template))
        })
        .collect();
    offers.sort_by_key(|(offer, _)| (offer.slot, offer.id));
    offers
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
    let inst = item_in_slot(ctx, player_guid, slot).ok_or(SellRefusal::NoItemInSlot(slot))?;
    let count = if count == 0 { inst.stack_count } else { count };
    if count > inst.stack_count {
        return Err("invalid sell count".to_string().into());
    }
    if crate::trade::item_is_offered(ctx, player_guid, inst.guid) {
        return Err(super::refused(lyracore_shared::item::ItemRefusal::NotRightNow).into());
    }
    if super::inventory::bag_has_contents(ctx, player_guid, inst.slot) {
        return Err("cannot sell a nonempty bag".to_string().into());
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
    super::buyback::sell(ctx, inst, &tmpl, count, earned)?;
    player.money = player.money.saturating_add(earned);
    player.update(ctx);
    if slot <= equip_slot::END {
        crate::spell::recompute_vitals(ctx, player_guid);
        crate::spell::recompute_sheet(ctx, player_guid);
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) enum BuybackRefusal {
    Vendor(NpcRefusal),
    Empty,
    Money {
        entry: u32,
    },
    Limit {
        item_guid: u64,
    },
    Storage {
        item_guid: u64,
        cause: crate::actor::ActionRefusal,
    },
    Refused(crate::Refusal),
}

impl std::fmt::Display for BuybackRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vendor(cause) => cause.fmt(f),
            Self::Empty => f.write_str("no item in buyback slot"),
            Self::Money { .. } => f.write_str("not enough money"),
            Self::Limit { .. } => f.write_str("unique item limit"),
            Self::Storage { cause, .. } => cause.fmt(f),
            Self::Refused(cause) => cause.fmt(f),
        }
    }
}

impl From<BuybackRefusal> for String {
    fn from(cause: BuybackRefusal) -> Self {
        cause.to_string()
    }
}

pub(crate) fn apply_buyback_item(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    slot_idx: u8,
) -> Result<(), BuybackRefusal> {
    let (mut player, _vendor) = npc_interaction_gate(
        ctx,
        player_guid,
        vendor_guid,
        lyracore_shared::constants::npc_flags::VENDOR,
        "vendor",
        "target is not a vendor",
        "use buyback",
    )
    .map_err(BuybackRefusal::Vendor)?;
    let row = super::buyback::entries(ctx, player_guid)
        .into_iter()
        .find(|row| {
            u16::from(row.slot) == u16::from(super::buyback::BUYBACK_START) + u16::from(slot_idx)
        })
        .ok_or(BuybackRefusal::Empty)?;
    if player.money < row.price {
        return Err(BuybackRefusal::Money {
            entry: row.item.entry,
        });
    }
    let template = ctx
        .db
        .game_item_template()
        .entry()
        .find(row.item.entry)
        .ok_or_else(|| BuybackRefusal::Refused("buyback item has no template".into()))?;
    let owned: u64 = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(player_guid)
        .filter(|item| item.entry == row.item.entry)
        .map(|item| u64::from(item.stack_count))
        .sum();
    if template.max_count != 0
        && owned + u64::from(row.item.stack_count) > u64::from(template.max_count)
    {
        return Err(BuybackRefusal::Limit {
            item_guid: row.item_guid,
        });
    }
    let mut plan = super::ItemStoragePlan::read(ctx, player_guid, player.owner_identity);
    let retained =
        plan.restore(ctx, &template, &row.item)
            .map_err(|cause| BuybackRefusal::Storage {
                item_guid: row.item_guid,
                cause,
            })?;
    plan.commit_restored(ctx)
        .map_err(|cause| BuybackRefusal::Storage {
            item_guid: row.item_guid,
            cause,
        })?;
    player.money -= row.price;
    player.update(ctx);
    super::buyback::take(ctx, &row);
    if retained {
        if let Some(paused) = &row.temporary_enchantment {
            let item = ctx
                .db
                .game_item_instance()
                .guid()
                .find(row.item_guid)
                .expect("restore kept the Item GUID");
            super::temporary_enchantment::resume(ctx, &item, paused);
        }
    } else {
        crate::native::buyback_destroyed(ctx, &row);
        crate::mail_text::drop_letter_text(ctx, row.item.item_text_id);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuyRefusal {
    Vendor(NpcRefusal),
    NotEnoughMoney {
        have: u32,
        need: u32,
    },
    InventoryFull,
    SoldOut,
    UnknownItem,
    Reputation,
    Equipment {
        result: lyracore_protocol::messages::vanilla::InventoryResult,
        required_level: u8,
    },
    Refused(crate::Refusal),
    /// The caller must roll back the transaction.
    CommitFailed(Box<BuyRefusal>),
}

impl From<lyracore_protocol::messages::vanilla::InventoryResult> for BuyRefusal {
    fn from(result: lyracore_protocol::messages::vanilla::InventoryResult) -> Self {
        Self::Equipment {
            result,
            required_level: 0,
        }
    }
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

impl From<crate::actor::ActionRefusal> for BuyRefusal {
    fn from(refusal: crate::actor::ActionRefusal) -> Self {
        match refusal.kind {
            crate::actor::ActionRefusalKind::InventoryFull => Self::InventoryFull,
            _ => Self::Refused(refusal.into()),
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
            Self::SoldOut => f.write_str("vendor item already sold"),
            Self::Equipment { result, .. } => write!(f, "{result:?}"),
            Self::UnknownItem => f.write_str("unknown purchase item"),
            Self::Reputation => f.write_str("purchase requires reputation"),
            Self::Refused(refusal) => refusal.fmt(f),
            Self::CommitFailed(refusal) => refusal.fmt(f),
        }
    }
}

impl From<BuyRefusal> for String {
    fn from(refusal: BuyRefusal) -> Self {
        refusal.to_string()
    }
}

pub(crate) fn vendor_purchase_price(
    ctx: &ReducerContext,
    character_guid: u64,
    faction_template: u32,
    price: u32,
) -> u32 {
    let discount = crate::reputation::vendor_discount_pct(ctx, character_guid, faction_template);
    (price as f32 * (1.0 - discount as f32 / 100.0) + 0.5) as u32
}

pub(crate) fn apply_buy_item(
    ctx: &ReducerContext,
    player_guid: u64,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), BuyRefusal> {
    apply_buy_item_to(ctx, player_guid, vendor_guid, item_entry, count, None)
}

pub(crate) fn apply_buy_item_to(
    ctx: &ReducerContext,
    character_guid: u64,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
    destination: Option<(u8, u8)>,
) -> Result<(), BuyRefusal> {
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(item_entry)
        .ok_or(BuyRefusal::UnknownItem)?;
    let (mut character, vendor) = npc_interaction_gate(
        ctx,
        character_guid,
        vendor_guid,
        lyracore_shared::constants::npc_flags::VENDOR,
        "vendor",
        "target is not a vendor",
        "buy",
    )?;
    let lots = count.max(1);
    let offer = vendor_offers(ctx, vendor.entry)
        .into_iter()
        .find(|(offer, _)| offer.item_entry == item_entry)
        .map(|(offer, _)| offer)
        .ok_or_else(|| "vendor does not sell that item".to_string())?;
    let total_count = lots.saturating_mul(tmpl.buy_count.max(1));
    let available = super::vendor_stock::available(ctx, vendor_guid, &offer, &tmpl);
    if offer.restock_seconds != 0 && offer.max_count != 0 && available < total_count {
        return Err(BuyRefusal::SoldOut);
    }
    if tmpl.required_reputation_rank != 0 {
        use crate::{game_faction_template, game_player_reputation};
        let faction = if tmpl.required_reputation_faction != 0 {
            tmpl.required_reputation_faction
        } else {
            ctx.db
                .game_faction_template()
                .id()
                .find(vendor.faction_template)
                .map_or(0, |faction| faction.faction)
        };
        let standing = ctx
            .db
            .game_player_reputation()
            .by_character()
            .filter(character_guid)
            .find(|reputation| reputation.faction_id == faction)
            .map_or(0, |reputation| reputation.standing);
        if u32::from(crate::reputation::reputation_rank(standing)) < tmpl.required_reputation_rank {
            return Err(BuyRefusal::Reputation);
        }
    }
    let cost = vendor_purchase_price(
        ctx,
        character_guid,
        vendor.faction_template,
        buy_cost(tmpl.buy_price, lots),
    );
    if character.money < cost {
        return Err(BuyRefusal::NotEnoughMoney {
            have: character.money,
            need: cost,
        });
    }
    let mut plan = super::ItemStoragePlan::read(ctx, character_guid, character.owner_identity);
    let last_slot = if let Some((bag, slot)) = destination {
        Some(plan.purchase(ctx, &tmpl, total_count, bag, slot)?)
    } else {
        plan.grant(ctx, &tmpl, total_count, false, None)?;
        None
    };
    crate::native::vendor_purchase_started(ctx, character_guid, vendor_guid, item_entry, lots);
    // A commit failure must roll back payment, stock debit and the queued reply.
    character.money = character.money.saturating_sub(cost);
    character.update(ctx);
    super::vendor_stock::purchase(
        ctx,
        vendor_guid,
        &offer,
        available.saturating_sub(total_count),
    );
    let committed = if let Some(slot) = last_slot {
        plan.commit_purchase(ctx, slot, total_count)
    } else {
        plan.commit_with_source(
            ctx,
            lyracore_protocol::messages::vanilla::NewItemSource::FromNpc,
        )
    };
    committed.map_err(|cause| BuyRefusal::CommitFailed(Box::new(cause.into())))
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
    use super::{flagged_npc_in_reach, BankSlotRefusal, NpcGate, NpcRefusal, VENDOR_RANGE_SQ};
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
}
