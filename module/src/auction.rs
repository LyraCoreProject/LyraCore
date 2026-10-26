pub(crate) mod native;

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use lyracore_shared::auction::{auction_cut, auction_notice, bid_increment, AuctionRefusal};
use lyracore_shared::mail::{MailSender, CHECK_MASK_COPIED};

use crate::import_meta::game_import_meta;
use crate::mail::game_mail;
use crate::{game_faction_template, game_item_instance, game_item_template, game_world_entity};

#[table(accessor = game_auction_house, public)]
pub struct AuctionHouseDefinition {
    #[primary_key]
    pub id: u32,
    pub faction: u32,
    pub deposit_rate: u32,
    pub consignment_rate: u32,
    pub name: String,
}

#[table(
    accessor = game_auction,
    public,
    index(accessor = by_owner, btree(columns = [owner_guid])),
    index(accessor = by_highest_bidder, btree(columns = [highest_bidder_guid]))
)]
pub struct Auction {
    #[primary_key]
    #[auto_inc]
    pub id: u32,
    pub listing_operation_id: u64,
    pub house: u32,
    pub owner_guid: u64,
    pub item_guid: u64,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    pub start_bid: u32,
    pub buyout: u32,
    pub highest_bidder_guid: u64,
    pub highest_bid: u32,
    pub deposit: u32,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub revision: u64,
    pub deposit_rate: u32,
    pub consignment_rate: u32,
    #[default(0)]
    pub random_property_id: u32,
    #[default(0u32)]
    pub item_text_id: u32,
}

#[table(
    accessor = game_auction_operation_receipt,
    index(accessor = by_actor, btree(columns = [actor_guid]))
)]
pub struct AuctionOperationReceipt {
    #[primary_key]
    pub operation_id: u64,
    pub auction_id: u32,
    pub actor_guid: u64,
    pub item_guid: u64,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    pub start_bid: u32,
    pub buyout: u32,
    pub duration_minutes: u32,
    pub deposit: u32,
    pub created_micros: i64,
    pub expires_micros: i64,
    pub house: u32,
    pub deposit_rate: u32,
    pub consignment_rate: u32,
    #[default(0)]
    pub random_property_id: u32,
    #[default(0u32)]
    pub item_text_id: u32,
}

#[table(
    accessor = game_auction_expiry,
    scheduled(expire_auction)
)]
pub struct AuctionExpiry {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub auction_id: u32,
}

#[table(
    accessor = game_auction_notice,
    index(accessor = by_recipient, btree(columns = [recipient_guid]))
)]
pub struct AuctionNotice {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_guid: u64,
    pub kind: u8,
    pub house: u32,
    pub auction_id: u32,
    pub item_entry: u32,
    pub random_property_id: u32,
    pub bid: u32,
    pub out_bid: u32,
    pub bidder_guid: u64,
    pub created_at: Timestamp,
}

// Retired cross-Shard Hold tables keep their declarations; see docs/danger-zones.md.
#[table(
    accessor = game_auction_hold,
    index(accessor = by_seller, btree(columns = [seller_guid]))
)]
pub struct AuctionHold {
    #[primary_key]
    pub operation_id: u64,
    pub seller_guid: u64,
    pub item_guid: u64,
    pub item_entry: u32,
    pub item_stack_count: u32,
    pub item_durability: u32,
    pub item_enchant_id: u32,
    pub item_soulbound: bool,
    pub start_bid: u32,
    pub buyout: u32,
    pub duration_minutes: u32,
    pub deposit: u32,
    pub created_micros: i64,
    pub expires_micros: i64,
    pub house: u32,
    pub deposit_rate: u32,
    pub consignment_rate: u32,
    #[default(0)]
    pub random_property_id: u32,
    #[default(0u32)]
    pub item_text_id: u32,
}

crate::character_owned!(delete, fn sweep_delete_game_auction_hold(ctx, character_guid) {
    let operations: Vec<u64> = ctx
.db
.game_auction_hold()
.by_seller()
.filter(&character_guid)
.map(|hold| hold.operation_id)
.collect();
    for operation_id in operations {
        ctx.db.game_auction_hold().operation_id().delete(operation_id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_auction_hold(ctx, character_guid, io) {
    table = game_auction_hold,
    by = by_seller,
    keep_key,
});

#[table(
    accessor = game_auction_bid_hold,
    index(accessor = by_bidder, btree(columns = [bidder_guid]))
)]
pub struct AuctionBidHold {
    #[primary_key]
    pub operation_id: u64,
    pub bidder_guid: u64,
    pub auction_id: u32,
    pub offer: u32,
    pub outcome: u8,
    pub revision: u64,
    pub result_bidder_guid: u64,
    pub result_bid: u32,
    pub minimum_increment: u32,
    #[default(0)]
    pub deferred_refund: u32,
    #[default(0)]
    pub accepted_price: u32,
    pub house: u32,
    #[default(0u8)]
    pub operation: u8,
}

crate::character_owned!(delete, fn sweep_delete_game_auction_bid_hold(ctx, character_guid) {
    let operations: Vec<u64> = ctx
.db
.game_auction_bid_hold()
.by_bidder()
.filter(&character_guid)
.map(|hold| hold.operation_id)
.collect();
    for operation_id in operations {
        ctx.db.game_auction_bid_hold().operation_id().delete(operation_id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_auction_bid_hold(ctx, character_guid, io) {
    table = game_auction_bid_hold,
    by = by_bidder,
    keep_key,
});

#[table(accessor = game_auction_bid_decision)]
pub struct AuctionBidDecision {
    #[primary_key]
    pub operation_id: u64,
    pub bidder_guid: u64,
    pub auction_id: u32,
    pub offer: u32,
    pub outcome: u8,
    pub revision: u64,
    pub result_bidder_guid: u64,
    pub result_bid: u32,
    pub minimum_increment: u32,
    #[default(0)]
    pub deferred_refund: u32,
    #[default(0)]
    pub accepted_price: u32,
    pub house: u32,
    #[default(0u32)]
    pub item_entry: u32,
    #[default(0u32)]
    pub random_property_id: u32,
    #[default(0u8)]
    pub operation: u8,
}

fn duration_multiplier(duration_minutes: u32) -> Option<u64> {
    match duration_minutes {
        120 => Some(1),
        480 => Some(4),
        1_440 => Some(12),
        _ => None,
    }
}

fn valid_rate(rate: u32) -> bool {
    rate <= 100
}

fn listing_deposit(
    sell_price: u32,
    stack_count: u32,
    duration_minutes: u32,
    deposit_rate: u32,
) -> Option<u32> {
    if !valid_rate(deposit_rate) {
        return None;
    }
    let multiplier = duration_multiplier(duration_minutes)?;
    let deposit = u64::from(sell_price)
        .checked_mul(u64::from(stack_count))?
        .checked_mul(u64::from(deposit_rate))?
        .checked_mul(multiplier)?
        / 100;
    u32::try_from(deposit.max(1)).ok()
}

fn seller_proceeds(winning_price: u32, deposit: u32, consignment_rate: u32) -> Option<u32> {
    let cut = auction_cut(winning_price, consignment_rate)?;
    let after_cut = winning_price.checked_sub(cut)?;
    after_cut.checked_add(deposit)
}

fn listing_proceeds_are_representable(
    terms: ListingTerms,
    deposit: u32,
    consignment_rate: u32,
) -> bool {
    seller_proceeds(terms.start_bid, deposit, consignment_rate).is_some()
        && (terms.buyout == 0 || seller_proceeds(terms.buyout, deposit, consignment_rate).is_some())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuctionHousePolicy {
    pub(crate) id: u32,
    deposit_rate: u32,
    consignment_rate: u32,
}

pub(crate) fn auction_house_for_interaction(
    ctx: &ReducerContext,
    player_guid: u64,
    auctioneer_guid: u64,
) -> Option<AuctionHousePolicy> {
    let player = crate::helpers::acting_entity_by_guid(ctx, player_guid)?;
    let auctioneer = ctx.db.game_world_entity().guid().find(auctioneer_guid)?;
    if !player.is_player()
        || player.dead
        || player.health == 0
        || auctioneer.dead
        || auctioneer.health == 0
        || auctioneer.is_player()
        || auctioneer.type_mask & lyracore_shared::constants::type_mask::CREATURE
            != lyracore_shared::constants::type_mask::CREATURE
        || auctioneer.npc_flags & lyracore_shared::constants::npc_flags::AUCTIONEER == 0
        || auctioneer.unit_flags & lyracore_shared::constants::unit_flags::NOT_SELECTABLE != 0
        || player.map_id != auctioneer.map_id
        || player.instance_id != auctioneer.instance_id
        || crate::helpers::dist_sq(&player, &auctioneer)
            > lyracore_shared::auction::INTERACTION_RANGE_SQ
        || crate::reputation::npc_refuses_interaction(ctx, &auctioneer, &player)
    {
        return None;
    }
    let faction_group = ctx
        .db
        .game_faction_template()
        .id()
        .find(auctioneer.faction_template)?
        .faction_group;
    let house_id = lyracore_shared::auction::house_for_faction_template(
        auctioneer.faction_template,
        faction_group,
    );
    ctx.db
        .game_auction_house()
        .id()
        .find(house_id)
        .filter(|house| {
            house.id != 0 && valid_rate(house.deposit_rate) && valid_rate(house.consignment_rate)
        })
        .map(|house| AuctionHousePolicy {
            id: house.id,
            deposit_rate: house.deposit_rate,
            consignment_rate: house.consignment_rate,
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListingItem {
    guid: u64,
    owner_guid: u64,
    slot: u8,
    mailable: bool,
    snapshot: crate::items::ItemSnapshot,
    sell_price: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ListingTerms {
    pub(crate) start_bid: u32,
    pub(crate) buyout: u32,
    pub(crate) duration_minutes: u32,
}

const FIRST_BACKPACK_SLOT: u8 = 23;
const MICROS_PER_MINUTE: i64 = 60_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListingRequest {
    operation_id: u64,
    seller_guid: u64,
    item_guid: u64,
    house: AuctionHousePolicy,
    terms: ListingTerms,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PreparedListing {
    request: ListingRequest,
    snapshot: crate::items::ItemSnapshot,
    deposit: u32,
    created_micros: i64,
    expires_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListingReceipt {
    listing: PreparedListing,
    auction_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationMatch {
    Replay(u32),
    Conflict,
    Fresh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BidOperation {
    Bid,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BidRequest {
    operation: BidOperation,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    house: u32,
    offer: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BidAuction {
    id: u32,
    house: u32,
    owner_guid: u64,
    item: crate::items::ItemSnapshot,
    highest_bidder_guid: u64,
    highest_bid: u32,
    start_bid: u32,
    buyout: u32,
    deposit: u32,
    consignment_rate: u32,
    expires_micros: i64,
    revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuctionBidEffect {
    RemainActive { revision: u64 },
    SettleBuyout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BidAcceptance {
    price: u32,
    effect: AuctionBidEffect,
    displaced_bidder_guid: u64,
    displaced_bid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BidDecision {
    Accepted(BidAcceptance),
    ItemNotFound,
    HigherBid {
        bidder_guid: u64,
        current_bid: u32,
        minimum_increment: u32,
    },
    BidIncrement,
    BidOwn,
    Database,
    Cancelled {
        cut: u32,
        displaced_bidder_guid: u64,
        displaced_bid: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuctionMailAction {
    Outbid = 0,
    Won = 1,
    Successful = 2,
    Expired = 3,
    CancelledToBidder = 4,
    Cancelled = 5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuctionMail {
    recipient_guid: u64,
    house: u32,
    action: AuctionMailAction,
    item_entry: u32,
    random_property_id: u32,
    money: u32,
    attached_item: crate::items::ItemSnapshot,
    counterparty_guid: u64,
    bid: u32,
    buyout: u32,
    deposit: u32,
    cut: u32,
}

fn displaced_bid_refund_mail(
    house: u32,
    item: crate::items::ItemSnapshot,
    bidder_guid: u64,
    bid: u32,
) -> Option<AuctionMail> {
    (bidder_guid != 0).then_some(AuctionMail {
        recipient_guid: bidder_guid,
        house,
        action: AuctionMailAction::Outbid,
        item_entry: item.entry,
        random_property_id: item.random_property_id,
        money: bid,
        attached_item: crate::items::ItemSnapshot::default(),
        counterparty_guid: 0,
        bid: 0,
        buyout: 0,
        deposit: 0,
        cut: 0,
    })
}

#[allow(clippy::too_many_arguments)]
fn sale_settlement_mail(
    house: u32,
    owner_guid: u64,
    item: crate::items::ItemSnapshot,
    winner_guid: u64,
    winning_price: u32,
    buyout: u32,
    deposit: u32,
    consignment_rate: u32,
) -> Option<[AuctionMail; 2]> {
    let cut = auction_cut(winning_price, consignment_rate)?;
    let proceeds = seller_proceeds(winning_price, deposit, consignment_rate)?;
    Some([
        AuctionMail {
            recipient_guid: winner_guid,
            house,
            action: AuctionMailAction::Won,
            item_entry: item.entry,
            random_property_id: item.random_property_id,
            money: 0,
            attached_item: item,
            counterparty_guid: owner_guid,
            bid: winning_price,
            buyout,
            deposit: 0,
            cut: 0,
        },
        AuctionMail {
            recipient_guid: owner_guid,
            house,
            action: AuctionMailAction::Successful,
            item_entry: item.entry,
            random_property_id: item.random_property_id,
            money: proceeds,
            attached_item: crate::items::ItemSnapshot::default(),
            counterparty_guid: winner_guid,
            bid: winning_price,
            buyout,
            deposit,
            cut,
        },
    ])
}

fn buyout_settlement_mail(
    auction: BidAuction,
    winner_guid: u64,
    price: u32,
) -> Option<[AuctionMail; 2]> {
    if auction.buyout == 0 || price != auction.buyout {
        return None;
    }
    sale_settlement_mail(
        auction.house,
        auction.owner_guid,
        auction.item,
        winner_guid,
        price,
        auction.buyout,
        auction.deposit,
        auction.consignment_rate,
    )
}

fn cancellation_mail(
    auction: BidAuction,
    displaced_bidder_guid: u64,
    displaced_bid: u32,
) -> Vec<AuctionMail> {
    let mail = |recipient_guid, action, money, attached_item| AuctionMail {
        recipient_guid,
        house: auction.house,
        action,
        item_entry: auction.item.entry,
        random_property_id: auction.item.random_property_id,
        money,
        attached_item,
        counterparty_guid: 0,
        bid: 0,
        buyout: 0,
        deposit: 0,
        cut: 0,
    };
    let refund = (displaced_bidder_guid != 0).then(|| {
        mail(
            displaced_bidder_guid,
            AuctionMailAction::CancelledToBidder,
            displaced_bid,
            crate::items::ItemSnapshot::default(),
        )
    });
    refund
        .into_iter()
        .chain([mail(
            auction.owner_guid,
            AuctionMailAction::Cancelled,
            0,
            auction.item,
        )])
        .collect()
}

// The client parses subjects as item_entry:random_property_id:action.
fn auction_mail_subject(
    item_entry: u32,
    random_property_id: u32,
    action: AuctionMailAction,
) -> String {
    format!("{item_entry}:{random_property_id}:{}", action as u8)
}

// Invoice guids use lowercase hex in a 16-character, space-filled field.
fn auction_mail_body(mail: &AuctionMail) -> String {
    match mail.action {
        AuctionMailAction::Won => format!(
            "{:>16x}:{}:{}",
            mail.counterparty_guid, mail.bid, mail.buyout
        ),
        AuctionMailAction::Successful => format!(
            "{:>16x}:{}:{}:{}:{}",
            mail.counterparty_guid, mail.bid, mail.buyout, mail.deposit, mail.cut
        ),
        AuctionMailAction::Outbid
        | AuctionMailAction::Expired
        | AuctionMailAction::CancelledToBidder
        | AuctionMailAction::Cancelled => String::new(),
    }
}

fn insert_auction_mail(ctx: &ReducerContext, mail: AuctionMail) {
    crate::mail::insert_letter(
        ctx,
        crate::mail::Letter {
            recipient_guid: mail.recipient_guid,
            sender: MailSender::AuctionHouse(mail.house),
            subject: auction_mail_subject(mail.item_entry, mail.random_property_id, mail.action),
            body: auction_mail_body(&mail),
            money: mail.money,
            cod: 0,
            item: mail.attached_item,
            mail_template_id: 0,
            check_flags: CHECK_MASK_COPIED,
            deliver_micros: 0,
        },
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuctionNoticeDraft {
    recipient_guid: u64,
    kind: u8,
    house: u32,
    auction_id: u32,
    item_entry: u32,
    random_property_id: u32,
    bid: u32,
    out_bid: u32,
    bidder_guid: u64,
}

#[cfg(feature = "debug_reducers")]
fn clear_auction_notices(ctx: &ReducerContext, recipient_guid: u64) {
    let notices = ctx.db.game_auction_notice();
    let ids: Vec<_> = notices
        .by_recipient()
        .filter(recipient_guid)
        .map(|notice| notice.id)
        .collect();
    for id in ids {
        notices.id().delete(id);
    }
}

fn insert_auction_notice(ctx: &ReducerContext, draft: AuctionNoticeDraft) {
    let notice = ctx.db.game_auction_notice().insert(AuctionNotice {
        id: 0,
        recipient_guid: draft.recipient_guid,
        kind: draft.kind,
        house: draft.house,
        auction_id: draft.auction_id,
        item_entry: draft.item_entry,
        random_property_id: draft.random_property_id,
        bid: draft.bid,
        out_bid: draft.out_bid,
        bidder_guid: draft.bidder_guid,
        created_at: ctx.timestamp,
    });
    crate::native::auction_notice(ctx, &notice);
}

#[allow(clippy::too_many_arguments)]
fn settlement_notices(
    house: u32,
    auction_id: u32,
    item_entry: u32,
    random_property_id: u32,
    owner_guid: u64,
    winner_guid: u64,
    winning_price: u32,
) -> [AuctionNoticeDraft; 2] {
    let out_bid = bid_increment(winning_price);
    [
        AuctionNoticeDraft {
            recipient_guid: winner_guid,
            kind: auction_notice::WON,
            house,
            auction_id,
            item_entry,
            random_property_id,
            bid: 0,
            out_bid,
            bidder_guid: winner_guid,
        },
        AuctionNoticeDraft {
            recipient_guid: owner_guid,
            kind: auction_notice::SOLD,
            house,
            auction_id,
            item_entry,
            random_property_id,
            bid: winning_price,
            out_bid,
            bidder_guid: 0,
        },
    ]
}

#[allow(clippy::too_many_arguments)]
fn outbid_notice(
    house: u32,
    auction_id: u32,
    item: crate::items::ItemSnapshot,
    displaced_bidder_guid: u64,
    new_bidder_guid: u64,
    displaced_bid: u32,
) -> Option<AuctionNoticeDraft> {
    (displaced_bidder_guid != 0 && displaced_bidder_guid != new_bidder_guid).then_some(
        AuctionNoticeDraft {
            recipient_guid: displaced_bidder_guid,
            kind: auction_notice::OUTBID,
            house,
            auction_id,
            item_entry: item.entry,
            random_property_id: item.random_property_id,
            bid: displaced_bid,
            out_bid: bid_increment(displaced_bid),
            bidder_guid: displaced_bidder_guid,
        },
    )
}

fn new_bid_notice(
    house: u32,
    auction_id: u32,
    item: crate::items::ItemSnapshot,
    owner_guid: u64,
    bidder_guid: u64,
    price: u32,
) -> AuctionNoticeDraft {
    AuctionNoticeDraft {
        recipient_guid: owner_guid,
        kind: auction_notice::NEW_BID,
        house,
        auction_id,
        item_entry: item.entry,
        random_property_id: item.random_property_id,
        bid: price,
        out_bid: bid_increment(price),
        bidder_guid,
    }
}

fn expired_notice(
    house: u32,
    auction_id: u32,
    item: crate::items::ItemSnapshot,
    owner_guid: u64,
) -> AuctionNoticeDraft {
    AuctionNoticeDraft {
        recipient_guid: owner_guid,
        kind: auction_notice::EXPIRED,
        house,
        auction_id,
        item_entry: item.entry,
        random_property_id: item.random_property_id,
        bid: 0,
        // The client requires a nonzero minimum raise even on an unsold listing.
        out_bid: 1,
        bidder_guid: 0,
    }
}

fn removed_notice(
    house: u32,
    auction_id: u32,
    item: crate::items::ItemSnapshot,
    displaced_bidder_guid: u64,
    displaced_bid: u32,
) -> Option<AuctionNoticeDraft> {
    (displaced_bidder_guid != 0).then_some(AuctionNoticeDraft {
        recipient_guid: displaced_bidder_guid,
        kind: auction_notice::REMOVED,
        house,
        auction_id,
        item_entry: item.entry,
        random_property_id: item.random_property_id,
        bid: displaced_bid,
        out_bid: 0,
        bidder_guid: displaced_bidder_guid,
    })
}

fn request_is_malformed(request: &BidRequest) -> bool {
    request.operation_id == 0
        || request.bidder_guid == 0
        || request.auction_id == 0
        || request.house == 0
        || (request.operation == BidOperation::Bid && request.offer == 0)
}

fn active_listing(
    auction: Option<BidAuction>,
    request: &BidRequest,
    now_micros: i64,
) -> Option<BidAuction> {
    auction.filter(|auction| {
        auction.id == request.auction_id
            && lyracore_shared::auction::market_of(auction.house)
                == lyracore_shared::auction::market_of(request.house)
            && auction.expires_micros > now_micros
    })
}

fn minimum_next_bid(auction: BidAuction) -> Result<u32, BidDecision> {
    lyracore_shared::auction::minimum_next_bid(auction.start_bid, auction.highest_bid)
        .ok_or(BidDecision::Database)
}

fn decide_bid(auction: Option<BidAuction>, request: BidRequest, now_micros: i64) -> BidDecision {
    if request.operation != BidOperation::Bid || request_is_malformed(&request) {
        return BidDecision::Database;
    }
    let Some(auction) = active_listing(auction, &request, now_micros) else {
        return BidDecision::ItemNotFound;
    };
    if auction.owner_guid == request.bidder_guid {
        return BidDecision::BidOwn;
    }
    let is_buyout = auction.buyout != 0 && request.offer >= auction.buyout;
    let minimum_increment = lyracore_shared::auction::bid_increment(auction.highest_bid);
    let minimum = if is_buyout {
        None
    } else {
        let Ok(minimum) = minimum_next_bid(auction) else {
            return BidDecision::Database;
        };
        Some(minimum)
    };
    if auction.highest_bid != 0 && request.offer <= auction.highest_bid {
        return BidDecision::HigherBid {
            bidder_guid: auction.highest_bidder_guid,
            current_bid: auction.highest_bid,
            minimum_increment,
        };
    }
    if is_buyout {
        if seller_proceeds(auction.buyout, auction.deposit, auction.consignment_rate).is_none() {
            return BidDecision::Database;
        }
        return BidDecision::Accepted(BidAcceptance {
            price: auction.buyout,
            effect: AuctionBidEffect::SettleBuyout,
            displaced_bidder_guid: auction.highest_bidder_guid,
            displaced_bid: auction.highest_bid,
        });
    }
    let Some(minimum) = minimum else {
        return BidDecision::Database;
    };
    if request.offer < minimum {
        return BidDecision::BidIncrement;
    }
    if seller_proceeds(request.offer, auction.deposit, auction.consignment_rate).is_none() {
        return BidDecision::Database;
    }
    let Some(revision) = auction.revision.checked_add(1) else {
        return BidDecision::Database;
    };
    BidDecision::Accepted(BidAcceptance {
        price: request.offer,
        effect: AuctionBidEffect::RemainActive { revision },
        displaced_bidder_guid: auction.highest_bidder_guid,
        displaced_bid: auction.highest_bid,
    })
}

fn decide_cancel(auction: Option<BidAuction>, request: BidRequest, now_micros: i64) -> BidDecision {
    if request.operation != BidOperation::Cancel || request_is_malformed(&request) {
        return BidDecision::Database;
    }
    let Some(auction) = active_listing(auction, &request, now_micros)
        .filter(|auction| auction.owner_guid == request.bidder_guid)
    else {
        return BidDecision::ItemNotFound;
    };
    if (auction.highest_bidder_guid == 0) != (auction.highest_bid == 0) {
        return BidDecision::Database;
    }
    match auction_cut(auction.highest_bid, auction.consignment_rate) {
        Some(cut) if cut == request.offer => BidDecision::Cancelled {
            cut,
            displaced_bidder_guid: auction.highest_bidder_guid,
            displaced_bid: auction.highest_bid,
        },
        _ => BidDecision::Database,
    }
}

trait ListingSource {
    fn seller_money(&self, seller_guid: u64) -> Option<u32>;
    fn item(&self, item_guid: u64) -> Option<ListingItem>;
    fn now_micros(&self) -> i64;
}

trait LocalListingSink: ListingSource {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt>;
    fn commit_local(&mut self, listing: PreparedListing) -> u32;
}

fn operation_match<S: LocalListingSink>(sink: &S, request: ListingRequest) -> OperationMatch {
    match sink.receipt(request.operation_id) {
        None => OperationMatch::Fresh,
        Some(receipt) if receipt.auction_id != 0 && receipt.listing.request == request => {
            OperationMatch::Replay(receipt.auction_id)
        }
        Some(_) => OperationMatch::Conflict,
    }
}

fn prepare_from_source<S: ListingSource>(
    sink: &S,
    request: ListingRequest,
) -> Result<PreparedListing, AuctionRefusal> {
    if request.operation_id == 0 || request.house.id == 0 {
        return Err(AuctionRefusal::InvalidTerms);
    }
    let item = sink.item(request.item_guid);
    let seller_money = sink
        .seller_money(request.seller_guid)
        .ok_or(AuctionRefusal::ItemNotFound)?;
    let deposit = prepare_listing(
        item.as_ref(),
        request.seller_guid,
        seller_money,
        request.terms,
        request.house,
    )?;
    let created_micros = sink.now_micros();
    let expires_micros = i64::from(request.terms.duration_minutes)
        .checked_mul(MICROS_PER_MINUTE)
        .and_then(|duration| created_micros.checked_add(duration))
        .ok_or(AuctionRefusal::InvalidTerms)?;
    Ok(PreparedListing {
        request,
        snapshot: item.expect("validated listing item is present").snapshot,
        deposit,
        created_micros,
        expires_micros,
    })
}

fn create_local_listing<S: LocalListingSink>(
    sink: &mut S,
    request: ListingRequest,
) -> Result<u32, AuctionRefusal> {
    match operation_match(sink, request) {
        OperationMatch::Replay(auction_id) => return Ok(auction_id),
        OperationMatch::Conflict => return Err(AuctionRefusal::InvalidTerms),
        OperationMatch::Fresh => {}
    }
    let listing = prepare_from_source(sink, request)?;
    Ok(sink.commit_local(listing))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveAuction {
    id: u32,
    listing: PreparedListing,
    highest_bidder_guid: u64,
    highest_bid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExpiryCompletion {
    Unsold(AuctionMail),
    Sold([AuctionMail; 2]),
}

trait ExpirySink {
    fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String>;
    fn complete_expiry(&mut self, auction: ActiveAuction, completion: ExpiryCompletion);
}

fn expiry_completion(auction: &ActiveAuction) -> Result<ExpiryCompletion, String> {
    match (auction.highest_bidder_guid, auction.highest_bid) {
        (0, 0) => Ok(ExpiryCompletion::Unsold(AuctionMail {
            recipient_guid: auction.listing.request.seller_guid,
            house: auction.listing.request.house.id,
            action: AuctionMailAction::Expired,
            item_entry: auction.listing.snapshot.entry,
            random_property_id: auction.listing.snapshot.random_property_id,
            money: 0,
            attached_item: auction.listing.snapshot,
            counterparty_guid: 0,
            bid: 0,
            buyout: 0,
            deposit: 0,
            cut: 0,
        })),
        (winner_guid, winning_price) if winner_guid != 0 && winning_price != 0 => {
            sale_settlement_mail(
                auction.listing.request.house.id,
                auction.listing.request.seller_guid,
                auction.listing.snapshot,
                winner_guid,
                winning_price,
                auction.listing.request.terms.buyout,
                auction.listing.deposit,
                auction.listing.request.house.consignment_rate,
            )
            .map(ExpiryCompletion::Sold)
            .ok_or_else(|| format!("auction {} sale settlement overflow", auction.id))
        }
        _ => Err(format!(
            "auction {} has inconsistent highest-bid state; preserving it for repair",
            auction.id
        )),
    }
}

fn expire_active<S: ExpirySink>(sink: &mut S, auction_id: u32) -> Result<(), String> {
    let Some(auction) = sink.auction(auction_id)? else {
        return Ok(());
    };
    let completion = expiry_completion(&auction)?;
    sink.complete_expiry(auction, completion);
    Ok(())
}

fn listing_from_receipt(row: AuctionOperationReceipt) -> ListingReceipt {
    ListingReceipt {
        listing: PreparedListing {
            request: ListingRequest {
                operation_id: row.operation_id,
                seller_guid: row.actor_guid,
                item_guid: row.item_guid,
                house: AuctionHousePolicy {
                    id: row.house,
                    deposit_rate: row.deposit_rate,
                    consignment_rate: row.consignment_rate,
                },
                terms: ListingTerms {
                    start_bid: row.start_bid,
                    buyout: row.buyout,
                    duration_minutes: row.duration_minutes,
                },
            },
            snapshot: crate::items::ItemSnapshot {
                entry: row.item_entry,
                stack_count: row.item_stack_count,
                durability: row.item_durability,
                enchant_id: row.item_enchant_id,
                soulbound: row.item_soulbound,
                random_property_id: row.random_property_id,
                item_text_id: row.item_text_id,
            },
            deposit: row.deposit,
            created_micros: row.created_micros,
            expires_micros: row.expires_micros,
        },
        auction_id: row.auction_id,
    }
}

fn receipt_from_listing(listing: PreparedListing, auction_id: u32) -> AuctionOperationReceipt {
    AuctionOperationReceipt {
        operation_id: listing.request.operation_id,
        auction_id,
        actor_guid: listing.request.seller_guid,
        item_guid: listing.request.item_guid,
        item_entry: listing.snapshot.entry,
        item_stack_count: listing.snapshot.stack_count,
        item_durability: listing.snapshot.durability,
        item_enchant_id: listing.snapshot.enchant_id,
        item_soulbound: listing.snapshot.soulbound,
        random_property_id: listing.snapshot.random_property_id,
        item_text_id: listing.snapshot.item_text_id,
        start_bid: listing.request.terms.start_bid,
        buyout: listing.request.terms.buyout,
        duration_minutes: listing.request.terms.duration_minutes,
        deposit: listing.deposit,
        created_micros: listing.created_micros,
        expires_micros: listing.expires_micros,
        house: listing.request.house.id,
        deposit_rate: listing.request.house.deposit_rate,
        consignment_rate: listing.request.house.consignment_rate,
    }
}

struct CtxSource<'a> {
    ctx: &'a ReducerContext,
}

impl ListingSource for CtxSource<'_> {
    fn seller_money(&self, seller_guid: u64) -> Option<u32> {
        crate::helpers::acting_entity_by_guid(self.ctx, seller_guid).map(|seller| seller.money)
    }

    fn item(&self, item_guid: u64) -> Option<ListingItem> {
        let item = self.ctx.db.game_item_instance().guid().find(item_guid)?;
        let template = self.ctx.db.game_item_template().entry().find(item.entry)?;
        Some(ListingItem {
            guid: item.guid,
            owner_guid: item.owner_guid,
            slot: item.slot,
            mailable: crate::items::validate_bag_dest_slot(self.ctx, item.owner_guid, item.slot)
                .is_ok(),
            snapshot: crate::items::ItemSnapshot::from(&item),
            sell_price: template.sell_price,
        })
    }

    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }
}

fn insert_active_auction(ctx: &ReducerContext, listing: &PreparedListing) -> u32 {
    let created_at = Timestamp::from_micros_since_unix_epoch(listing.created_micros);
    let expires_at = Timestamp::from_micros_since_unix_epoch(listing.expires_micros);
    let auction = ctx.db.game_auction().insert(Auction {
        id: 0,
        listing_operation_id: listing.request.operation_id,
        house: listing.request.house.id,
        owner_guid: listing.request.seller_guid,
        item_guid: listing.request.item_guid,
        item_entry: listing.snapshot.entry,
        item_stack_count: listing.snapshot.stack_count,
        item_durability: listing.snapshot.durability,
        item_enchant_id: listing.snapshot.enchant_id,
        item_soulbound: listing.snapshot.soulbound,
        random_property_id: listing.snapshot.random_property_id,
        item_text_id: listing.snapshot.item_text_id,
        start_bid: listing.request.terms.start_bid,
        buyout: listing.request.terms.buyout,
        highest_bidder_guid: 0,
        highest_bid: 0,
        deposit: listing.deposit,
        created_at,
        expires_at,
        revision: 0,
        deposit_rate: listing.request.house.deposit_rate,
        consignment_rate: listing.request.house.consignment_rate,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(expires_at),
        auction_id: auction.id,
    });
    auction.id
}

fn consume_listing_value(ctx: &ReducerContext, listing: &PreparedListing) {
    let mut seller = crate::helpers::acting_entity_by_guid(ctx, listing.request.seller_guid)
        .expect("validated listing seller remains present in the reducer transaction");
    seller.money -= listing.deposit;
    seller.update(ctx);
    crate::ItemInstance::delete(ctx, listing.request.item_guid);
}

impl LocalListingSink for CtxSource<'_> {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
        self.ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(operation_id)
            .map(listing_from_receipt)
    }

    fn commit_local(&mut self, listing: PreparedListing) -> u32 {
        consume_listing_value(self.ctx, &listing);
        let auction_id = insert_active_auction(self.ctx, &listing);
        self.ctx
            .db
            .game_auction_operation_receipt()
            .insert(receipt_from_listing(listing, auction_id));
        auction_id
    }
}

struct CtxBidMarket<'a> {
    ctx: &'a ReducerContext,
}

impl CtxBidMarket<'_> {
    fn auction(&self, auction_id: u32) -> Option<BidAuction> {
        self.ctx
            .db
            .game_auction()
            .id()
            .find(auction_id)
            .map(|auction| BidAuction {
                id: auction.id,
                house: auction.house,
                owner_guid: auction.owner_guid,
                item: crate::items::ItemSnapshot {
                    entry: auction.item_entry,
                    stack_count: auction.item_stack_count,
                    durability: auction.item_durability,
                    enchant_id: auction.item_enchant_id,
                    soulbound: auction.item_soulbound,
                    random_property_id: auction.random_property_id,
                    item_text_id: auction.item_text_id,
                },
                highest_bidder_guid: auction.highest_bidder_guid,
                highest_bid: auction.highest_bid,
                start_bid: auction.start_bid,
                buyout: auction.buyout,
                deposit: auction.deposit,
                consignment_rate: auction.consignment_rate,
                expires_micros: auction.expires_at.to_micros_since_unix_epoch(),
                revision: auction.revision,
            })
    }

    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }

    fn apply_decision(
        &mut self,
        request: BidRequest,
        auction: Option<BidAuction>,
        decision: BidDecision,
    ) -> Result<(), AuctionRefusal> {
        if let BidDecision::Accepted(accepted) = decision {
            let expected = auction.ok_or(AuctionRefusal::Database)?;
            let mut row = self
                .ctx
                .db
                .game_auction()
                .id()
                .find(request.auction_id)
                .ok_or(AuctionRefusal::Database)?;
            if row.revision != expected.revision
                || row.highest_bidder_guid != expected.highest_bidder_guid
                || row.highest_bid != expected.highest_bid
            {
                return Err(AuctionRefusal::Database);
            }
            let refund_mail = displaced_bid_refund_mail(
                expected.house,
                expected.item,
                accepted.displaced_bidder_guid,
                accepted.displaced_bid,
            );
            outbid_notice(
                expected.house,
                request.auction_id,
                expected.item,
                accepted.displaced_bidder_guid,
                request.bidder_guid,
                accepted.displaced_bid,
            )
            .into_iter()
            .for_each(|notice| insert_auction_notice(self.ctx, notice));
            match accepted.effect {
                AuctionBidEffect::SettleBuyout => {
                    let sale_mail =
                        buyout_settlement_mail(expected, request.bidder_guid, accepted.price)
                            .ok_or(AuctionRefusal::Database)?;
                    self.ctx
                        .db
                        .game_auction_expiry()
                        .auction_id()
                        .delete(row.id);
                    self.ctx.db.game_auction().id().delete(row.id);
                    refund_mail
                        .into_iter()
                        .chain(sale_mail)
                        .for_each(|mail| insert_auction_mail(self.ctx, mail));
                    settlement_notices(
                        expected.house,
                        request.auction_id,
                        expected.item.entry,
                        expected.item.random_property_id,
                        expected.owner_guid,
                        request.bidder_guid,
                        accepted.price,
                    )
                    .into_iter()
                    .for_each(|notice| insert_auction_notice(self.ctx, notice));
                }
                AuctionBidEffect::RemainActive { revision } => {
                    row.highest_bidder_guid = request.bidder_guid;
                    row.highest_bid = accepted.price;
                    row.revision = revision;
                    self.ctx.db.game_auction().id().update(row);
                    refund_mail
                        .into_iter()
                        .for_each(|mail| insert_auction_mail(self.ctx, mail));
                    insert_auction_notice(
                        self.ctx,
                        new_bid_notice(
                            expected.house,
                            request.auction_id,
                            expected.item,
                            expected.owner_guid,
                            request.bidder_guid,
                            accepted.price,
                        ),
                    );
                }
            }
        }
        if let BidDecision::Cancelled {
            displaced_bidder_guid,
            displaced_bid,
            ..
        } = decision
        {
            self.commit_cancellation(
                auction.ok_or(AuctionRefusal::Database)?,
                displaced_bidder_guid,
                displaced_bid,
            )?;
        }
        Ok(())
    }

    fn commit_cancellation(
        &mut self,
        expected: BidAuction,
        displaced_bidder_guid: u64,
        displaced_bid: u32,
    ) -> Result<(), AuctionRefusal> {
        let row = self
            .ctx
            .db
            .game_auction()
            .id()
            .find(expected.id)
            .ok_or(AuctionRefusal::Database)?;
        if row.revision != expected.revision
            || row.highest_bidder_guid != displaced_bidder_guid
            || row.highest_bid != displaced_bid
        {
            return Err(AuctionRefusal::Database);
        }
        self.ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .delete(row.id);
        self.ctx.db.game_auction().id().delete(row.id);
        cancellation_mail(expected, displaced_bidder_guid, displaced_bid)
            .into_iter()
            .for_each(|mail| insert_auction_mail(self.ctx, mail));
        removed_notice(
            expected.house,
            expected.id,
            expected.item,
            displaced_bidder_guid,
            displaced_bid,
        )
        .into_iter()
        .for_each(|notice| insert_auction_notice(self.ctx, notice));
        Ok(())
    }
}

fn bid_request(
    operation: BidOperation,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    house: u32,
    offer: u32,
) -> BidRequest {
    BidRequest {
        operation,
        operation_id,
        bidder_guid,
        auction_id,
        house,
        offer,
    }
}

struct CtxExpiry<'a> {
    ctx: &'a ReducerContext,
}

impl ExpirySink for CtxExpiry<'_> {
    fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String> {
        let Some(auction) = self.ctx.db.game_auction().id().find(auction_id) else {
            return Ok(None);
        };
        let receipt = self
            .ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(auction.listing_operation_id)
            .map(listing_from_receipt)
            .ok_or_else(|| {
                format!("auction {auction_id} has no operation receipt; preserving it for repair")
            })?;
        Ok(Some(ActiveAuction {
            id: auction.id,
            listing: receipt.listing,
            highest_bidder_guid: auction.highest_bidder_guid,
            highest_bid: auction.highest_bid,
        }))
    }

    fn complete_expiry(&mut self, auction: ActiveAuction, completion: ExpiryCompletion) {
        let house = auction.listing.request.house.id;
        let item = auction.listing.snapshot;
        let owner_guid = auction.listing.request.seller_guid;
        match completion {
            ExpiryCompletion::Unsold(mail) => {
                insert_auction_mail(self.ctx, mail);
                insert_auction_notice(
                    self.ctx,
                    expired_notice(house, auction.id, item, owner_guid),
                );
            }
            ExpiryCompletion::Sold(mail) => {
                mail.into_iter()
                    .for_each(|mail| insert_auction_mail(self.ctx, mail));
                settlement_notices(
                    house,
                    auction.id,
                    item.entry,
                    item.random_property_id,
                    owner_guid,
                    auction.highest_bidder_guid,
                    auction.highest_bid,
                )
                .into_iter()
                .for_each(|notice| insert_auction_notice(self.ctx, notice));
            }
        }
        self.ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .delete(auction.id);
        self.ctx.db.game_auction().id().delete(auction.id);
    }
}

#[reducer]
pub fn expire_auction(ctx: &ReducerContext, schedule: AuctionExpiry) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    expire_active(&mut CtxExpiry { ctx }, schedule.auction_id)
}

// Character deletion must not destroy listed value or a standing bid.
pub(crate) fn character_has_auction_value(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_auction()
        .by_owner()
        .filter(character_guid)
        .next()
        .is_some()
        || ctx
            .db
            .game_auction()
            .by_highest_bidder()
            .filter(character_guid)
            .next()
            .is_some()
}

const LEGACY_AUCTION_SUBJECTS: &[(&str, AuctionMailAction)] = &[
    ("Auction outbid", AuctionMailAction::Outbid),
    ("Auction won", AuctionMailAction::Won),
    ("Auction sold", AuctionMailAction::Successful),
    ("Auction expired", AuctionMailAction::Expired),
    ("Auction listing refused", AuctionMailAction::Cancelled),
    ("Auction bid refund", AuctionMailAction::Outbid),
];

fn legacy_auction_action(subject: &str) -> Option<AuctionMailAction> {
    LEGACY_AUCTION_SUBJECTS
        .iter()
        .find(|(known, _)| *known == subject)
        .map(|(_, action)| *action)
}

fn legacy_mail_matches_shape(mail: &crate::Mail) -> bool {
    if mail.cod != 0 || !mail.body.is_empty() {
        return false;
    }
    let has_item = !mail.snapshot().is_empty();
    match mail.subject.as_str() {
        "Auction outbid" | "Auction bid refund" => mail.sender_guid == 0 && !has_item,
        "Auction expired" => mail.sender_guid == 0 && has_item && mail.money == 0,
        "Auction listing refused" => mail.sender_guid == 0 && has_item && mail.money != 0,
        "Auction won" => has_item && mail.money == 0,
        "Auction sold" => !has_item && mail.money != 0,
        _ => false,
    }
}

struct LegacyRepairTarget {
    house: u32,
    item_entry: u32,
    random_property_id: u32,
}

// Won/Sold subjects can be real letters; retag them only with a matching durable listing receipt.
fn legacy_repair_authorization(
    ctx: &ReducerContext,
    mail: &crate::Mail,
    action: AuctionMailAction,
) -> Option<LegacyRepairTarget> {
    match action {
        AuctionMailAction::Won => {
            let receipt = ctx
                .db
                .game_auction_operation_receipt()
                .by_actor()
                .filter(&mail.sender_guid)
                .find(|receipt| {
                    receipt.item_entry == mail.item_entry
                        && receipt.item_stack_count == mail.item_stack_count
                })?;
            Some(LegacyRepairTarget {
                house: receipt.house,
                item_entry: mail.item_entry,
                random_property_id: mail.random_property_id,
            })
        }
        AuctionMailAction::Successful => {
            let candidates: Vec<AuctionOperationReceipt> = ctx
                .db
                .game_auction_operation_receipt()
                .by_actor()
                .filter(&mail.recipient_guid)
                .filter(|receipt| receipt.auction_id != 0)
                .collect();
            let receipt = match candidates.len() {
                0 => return None,
                1 => candidates.into_iter().next()?,
                _ => {
                    let mut paying = candidates
                        .into_iter()
                        .filter(|receipt| receipt_could_pay_out(receipt, mail.money));
                    let only = paying.next()?;
                    if paying.next().is_some() {
                        return None;
                    }
                    only
                }
            };
            Some(LegacyRepairTarget {
                house: receipt.house,
                item_entry: receipt.item_entry,
                random_property_id: receipt.random_property_id,
            })
        }
        AuctionMailAction::Expired | AuctionMailAction::Cancelled => {
            let house = ctx
                .db
                .game_auction_operation_receipt()
                .by_actor()
                .filter(&mail.recipient_guid)
                .find(|receipt| {
                    receipt.item_entry == mail.item_entry
                        && receipt.item_stack_count == mail.item_stack_count
                })
                .map_or_else(
                    || legacy_character_house(ctx, mail.recipient_guid),
                    |r| r.house,
                );
            Some(LegacyRepairTarget {
                house,
                item_entry: mail.item_entry,
                random_property_id: mail.random_property_id,
            })
        }
        _ => Some(LegacyRepairTarget {
            house: legacy_character_house(ctx, mail.recipient_guid),
            item_entry: mail.item_entry,
            random_property_id: mail.random_property_id,
        }),
    }
}

fn receipt_could_pay_out(receipt: &AuctionOperationReceipt, money: u32) -> bool {
    let Some(low) = seller_proceeds(receipt.start_bid, receipt.deposit, receipt.consignment_rate)
    else {
        return false;
    };
    if money < low {
        return false;
    }
    if receipt.buyout == 0 {
        return true;
    }
    match seller_proceeds(receipt.buyout, receipt.deposit, receipt.consignment_rate) {
        Some(high) => money <= high,
        None => true,
    }
}

fn legacy_character_house(ctx: &ReducerContext, recipient_guid: u64) -> u32 {
    match crate::helpers::character_by_guid(ctx, recipient_guid) {
        Some(character)
            if lyracore_shared::faction::team_for_race(character.race)
                == lyracore_shared::faction::TEAM_HORDE =>
        {
            6
        }
        Some(_) => 1,
        None => 7,
    }
}

const LEGACY_AUCTION_MAIL_REPAIR_FAMILY: &str = "repair_legacy_auction_mail";

pub(crate) fn repair_legacy_auction_mail(ctx: &ReducerContext) -> (u64, u64) {
    if ctx
        .db
        .game_import_meta()
        .family()
        .find(LEGACY_AUCTION_MAIL_REPAIR_FAMILY.to_string())
        .is_some()
    {
        return (0, 0);
    }
    let mails = ctx.db.game_mail();
    let candidates: Vec<crate::Mail> = mails
        .iter()
        .filter(|m| {
            m.sender_kind == lyracore_shared::mail::SENDER_KIND_CHARACTER
                && m.subject.starts_with("Auction ")
        })
        .collect();
    let mut converted = 0u64;
    let mut unmapped = 0u64;
    for mut mail in candidates {
        let Some(action) = legacy_auction_action(&mail.subject) else {
            unmapped += 1;
            continue;
        };
        if !legacy_mail_matches_shape(&mail) {
            unmapped += 1;
            continue;
        }
        let Some(target) = legacy_repair_authorization(ctx, &mail, action) else {
            unmapped += 1;
            continue;
        };
        let (sender_kind, sender_guid, sender_entry) =
            MailSender::AuctionHouse(target.house).columns();
        mail.sender_kind = sender_kind;
        mail.sender_guid = sender_guid;
        mail.sender_entry = sender_entry;
        mail.subject = auction_mail_subject(target.item_entry, target.random_property_id, action);
        mail.check_flags = CHECK_MASK_COPIED;
        mails.id().update(mail);
        converted += 1;
    }
    crate::import_meta::stamp(ctx, LEGACY_AUCTION_MAIL_REPAIR_FAMILY, "", "", converted);
    if unmapped > 0 {
        spacetimedb::log::warn!(
            "repair_legacy_auction_mail: {unmapped} auction-looking mail row(s) did not match a \
             known legacy shape and were left unchanged"
        );
    }
    spacetimedb::log::info!(
        "repair_legacy_auction_mail: re-tagged {converted} legacy auction mail row(s) to the \
         AuctionHouse sender"
    );
    (converted, unmapped)
}

fn prepare_listing(
    item: Option<&ListingItem>,
    seller_guid: u64,
    seller_money: u32,
    terms: ListingTerms,
    house: AuctionHousePolicy,
) -> Result<u32, AuctionRefusal> {
    let Some(item) = item else {
        return Err(AuctionRefusal::ItemNotFound);
    };
    if item.owner_guid != seller_guid
        || item.slot < FIRST_BACKPACK_SLOT
        || !crate::items::is_carried_slot(item.slot)
        || !item.mailable
        || item.snapshot.stack_count == 0
        || item.snapshot.soulbound
    {
        return Err(AuctionRefusal::ItemNotFound);
    }
    if terms.start_bid == 0 || (terms.buyout != 0 && terms.buyout < terms.start_bid) {
        return Err(AuctionRefusal::InvalidTerms);
    }
    let deposit = listing_deposit(
        item.sell_price,
        item.snapshot.stack_count,
        terms.duration_minutes,
        house.deposit_rate,
    )
    .ok_or(AuctionRefusal::InvalidTerms)?;
    if !listing_proceeds_are_representable(terms, deposit, house.consignment_rate) {
        return Err(AuctionRefusal::InvalidTerms);
    }
    if seller_money < deposit {
        return Err(AuctionRefusal::NotEnoughMoney);
    }
    Ok(deposit)
}

#[cfg(feature = "debug_reducers")]
mod fixtures;

#[cfg(test)]
mod tests;
