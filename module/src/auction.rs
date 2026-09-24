//! Durable imported auction houses, value-preserving bid and Cancellation transport, and atomic
//! settlement.

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use lyracore_shared::auction::bid_outcome::{
    ACCEPTED as BID_ACCEPTED, BID_INCREMENT, BID_OWN, CANCELLED as BID_CANCELLED,
    DATABASE as BID_DATABASE, HIGHER_BID as BID_HIGHER, ITEM_NOT_FOUND as BID_ITEM_NOT_FOUND,
    PENDING as BID_PENDING,
};
use lyracore_shared::auction::{
    auction_cut, auction_notice, bid_increment, hold_operation, AuctionRefusal,
};
use lyracore_shared::mail::{MailSender, CHECK_MASK_COPIED};

use crate::import_meta::game_import_meta;
use crate::mail::game_mail;
use crate::{game_faction_template, game_item_instance, game_item_template, game_world_entity};

/// Client-authored auction-house policy imported from `AuctionHouse.dbc`.
#[table(accessor = game_auction_house, public)]
pub struct AuctionHouseDefinition {
    #[primary_key]
    pub id: u32,
    pub faction: u32,
    pub deposit_rate: u32,
    pub consignment_rate: u32,
    pub name: String,
}

/// One active listing in its imported house market. The complete item-instance snapshot is the
/// item while it is listed; no inventory row exists until ordinary mail returns or delivers it.
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
}

/// Source-shard value reserved by a sharded listing operation. An active operation receipt makes
/// the row recovery evidence rather than spendable value; it is deleted only after that evidence
/// is durably copied back to the source. A refused listing instead keeps this Hold until
/// Realm-core durably commits its refund Mail and zero-id receipt.
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
}

/// Durable idempotency receipt. The full listing payload makes identical replay distinguishable
/// from conflicting reuse even after the source Hold has been deleted. Realm-core alone uses
/// `auction_id == 0` as a refused-listing refund receipt; that sentinel never reaches the source.
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
}

/// Source-shard copper Hold for one caller-identified bid or Cancellation. `outcome == 0` is
/// pending; every nonzero outcome is terminal. `accepted_price` records realm-core's normalized
/// charge, while `deferred_refund` retains any remainder that could not fit back in the purse. On a
/// Cancellation (`operation == hold_operation::CANCEL`) `bidder_guid` is the seller and `offer` is
/// the Auction Cut the seller agreed to pay.
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

// A Hold is copper on the Character's Home Shard, so it travels with the Character. A Hold left on
// a Shard the Character has left could never be refunded there, because the refund credits the
// purse on the Shard that holds the Hold. Deletion is refused while a Hold is unfinished, so the
// delete sweep only removes finished rows.
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

/// Realm-core's terminal serialized decision for one bid or Cancellation payload. Auction changes,
/// buyout and Cancellation mail, displaced mail, and any later source-refund mail are exact-once
/// updates recorded on this row.
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
    /// The auctioned item's catalogue entry and Random Property, captured while the Auction row
    /// still exists. A deferred refund can fire after that row is gone (the auction settled or
    /// expired between the decision and the refund), and this is the only place left to read the
    /// item the refund's Outbid subject names.
    #[default(0u32)]
    pub item_entry: u32,
    #[default(0u32)]
    pub random_property_id: u32,
    #[default(0u8)]
    pub operation: u8,
}

/// One one-shot scheduler row for each active Auction.
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

/// Private Relay event: a live outbid/won/sold/expired/new-bid/removed notice for one online seller
/// or bidder,
/// inserted in the same transaction as the Auction Mail it accompanies. `kind` is one of
/// `lyracore_shared::auction::auction_notice`. Reaped by the shared event GC, same as
/// `game_whisper_event`.
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

// Auction durability belongs to the listing protocol, not character transport. Active Auction or
// Hold value blocks character deletion, and every row except the bid Hold stays on the database
// that owns its protocol phase rather than entering the character movement manifest.

fn duration_multiplier(duration_minutes: u32) -> Option<u64> {
    match duration_minutes {
        720 => Some(1),
        1_440 => Some(2),
        2_880 => Some(4),
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
struct AuctionHousePolicy {
    id: u32,
    deposit_rate: u32,
    consignment_rate: u32,
}

fn imported_house_policy(ctx: &ReducerContext, house: u32) -> Result<AuctionHousePolicy, String> {
    let row = ctx
        .db
        .game_auction_house()
        .id()
        .find(house)
        .ok_or_else(|| {
            refused(
                AuctionRefusal::InvalidTerms,
                "auction house is not imported",
            )
        })?;
    if row.id == 0 || !valid_rate(row.deposit_rate) || !valid_rate(row.consignment_rate) {
        return Err(refused(
            AuctionRefusal::InvalidTerms,
            "imported auction house policy is invalid",
        ));
    }
    Ok(AuctionHousePolicy {
        id: row.id,
        deposit_rate: row.deposit_rate,
        consignment_rate: row.consignment_rate,
    })
}

fn auction_house_for_interaction(
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
    /// `ITEM_FIELD_ITEM_TEXT_ID` off the live row, not the snapshot — `ItemSnapshot` does not
    /// carry it yet, so a listed-and-sold Plain Letter would arrive unreadable. Stopgap until a
    /// later change carries the id through a listing: refuse it here instead.
    item_text_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListingTerms {
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListingHold {
    listing: PreparedListing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ListingRefund {
    listing: PreparedListing,
    mail: AuctionMail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperationMatch {
    Replay(u32),
    Conflict,
    Fresh,
}

/// What a bid Hold row pays for. Stored as `hold_operation` codes by position, so only append.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldOperation {
    Bid,
    Cancel,
}

impl HoldOperation {
    fn code(self) -> u8 {
        match self {
            Self::Bid => hold_operation::BID,
            Self::Cancel => hold_operation::CANCEL,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            hold_operation::BID => Some(Self::Bid),
            hold_operation::CANCEL => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// One Hold's identity. On a Cancel, `bidder_guid` is the seller and `offer` is the Auction Cut
/// the Gateway read, which Realm-core must still find current.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HoldRequest {
    operation: HoldOperation,
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
enum HoldDecision {
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
    /// The seller withdrew the listing and pays `cut`. A displaced bidder gets `displaced_bid`
    /// back by mail.
    Cancelled {
        cut: u32,
        displaced_bidder_guid: u64,
        displaced_bid: u32,
    },
}

impl HoldDecision {
    /// Whether Realm-core can reach this decision for `operation`. A decision for the other
    /// operation is a forged or crossed payload.
    fn belongs_to(self, operation: HoldOperation) -> bool {
        match self {
            Self::Accepted(_) | Self::HigherBid { .. } | Self::BidIncrement | Self::BidOwn => {
                operation == HoldOperation::Bid
            }
            Self::Cancelled { .. } => operation == HoldOperation::Cancel,
            Self::ItemNotFound | Self::Database => true,
        }
    }
}

/// The vanilla `MailAuctionAnswers` action code embedded in an Auction Mail's subject
/// (`cm:Mail.h:100-109`). `Cancelled` (5) returns a cancelled listing's item to its seller; a
/// refused listing's return uses it too, as the closest client string for a return with no vanilla
/// twin. `CancelledToBidder` (4) refunds the displaced bid of a cancelled listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuctionMailAction {
    Outbid = 0,
    Won = 1,
    Successful = 2,
    Expired = 3,
    CancelledToBidder = 4,
    Cancelled = 5,
}

/// An Auction Mail's own vanilla shape: `MAIL_AUCTION` from the house, `checked = COPIED`
/// (`cm:Mail.cpp:81-84`), a machine subject the client turns into `AUCTION_*_MAIL_SUBJECT`
/// (`fx:GlobalStrings.lua:83,87,88,89,99`), and — for Won/Successful — an invoice body the client
/// turns into the auction details (`fx:MailFrame.lua:300-361`). `item_entry`/`random_property_id`
/// name the AUCTIONED item, which the subject always carries even when nothing is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AuctionMail {
    recipient_guid: u64,
    house: u32,
    action: AuctionMailAction,
    item_entry: u32,
    random_property_id: u32,
    money: u32,
    attached_item: crate::items::ItemSnapshot,
    /// The invoice's other party: the seller on a Won mail, the winning bidder on a Successful
    /// mail. Unused (0) on every other action.
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

/// Mail that returns a refused listing's item and deposit to the seller. No vanilla auction
/// refuses a listing after acceptance, so this renders as the closest client string, a Cancelled
/// mail carrying the item and the deposit, and hides Return the same way every other Auction Mail
/// does.
fn listing_release_mail(listing: &PreparedListing) -> AuctionMail {
    AuctionMail {
        recipient_guid: listing.request.seller_guid,
        house: listing.request.house.id,
        action: AuctionMailAction::Cancelled,
        item_entry: listing.snapshot.entry,
        random_property_id: listing.snapshot.random_property_id,
        money: listing.deposit,
        attached_item: listing.snapshot,
        counterparty_guid: 0,
        bid: 0,
        buyout: 0,
        deposit: 0,
        cut: 0,
    }
}

/// A Cancellation's mail (`cm:AuctionHouseHandler.cpp:167-189,440-456`): the item goes back to the
/// seller in a Cancelled mail, and a displaced bidder gets the bid back in a Cancelled-to-bidder
/// mail. No mail carries the deposit, because the house keeps it.
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

/// `"{item_entry}:{random_property_id}:{action}"` (`cm:AuctionHouseMgr.cpp:134,181,229`,
/// `cm:AuctionHouseHandler.cpp:155,180,451`). The client builds the visible subject from this and
/// the item's name (`fx:GlobalStrings.lua:83,87,88,89,99`).
fn auction_mail_subject(
    item_entry: u32,
    random_property_id: u32,
    action: AuctionMailAction,
) -> String {
    format!("{item_entry}:{random_property_id}:{}", action as u8)
}

/// The invoice the client renders from a Won or Successful mail's body
/// (`fx:MailFrame.lua:300-361`). The guid is lowercase hex, right-aligned in a 16-character,
/// space-filled field — cmangos prints the low guid, which a 1.12 character guid equals whole.
/// Every other action carries no body.
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

/// One `game_auction_notice` row before its id and timestamp are stamped.
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

fn insert_auction_notice(ctx: &ReducerContext, draft: AuctionNoticeDraft) {
    ctx.db.game_auction_notice().insert(AuctionNotice {
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
}

/// Won (to the buyer) and Sold (to the seller): the two notices every settled sale sends, whether
/// it settled by buyout or by expiry with a bid (`cm:AuctionHouseMgr.cpp:146-147,195-199`).
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

/// Outbid, to the displaced bidder, sent before the bid changes (`cm:AuctionHouseHandler.cpp:93-107,157-158`).
/// No displaced bidder (a fresh listing's first bid), or the displaced bidder raising their own
/// bid, sends nothing: cmangos's `UpdateBid` charges a self-raise only the delta and never calls
/// `SendAuctionBidderNotification` for it (`cm:AuctionHouseMgr.cpp:780-793`), because the bidder
/// was never actually outbid.
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

/// New bid, to the owner: the client refreshes its list and prints nothing
/// (`cm:AuctionHouseMgr.cpp:802-806`).
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

/// Expired, to the owner: an unsold listing (`cm:AuctionHouseMgr.cpp:231-232`).
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
        // `bid_increment` returns 0 for a zero bid — nothing to raise — but the client always
        // shows a nonzero minimum raise, even on an unsold listing. Floor it the same way cmangos
        // does (`cm:AuctionHouseMgr.cpp:739-745`: `if (!outbid) outbid = 1`).
        out_bid: 1,
        bidder_guid: 0,
    }
}

/// Removed, to the bidder a Cancellation displaces (`cm:AuctionHouseHandler.cpp:131-139,182-183`).
/// The client prints `ERR_AUCTION_REMOVED_S` with the item's name.
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

#[derive(Clone, Copy)]
struct BidDecisionFields {
    outcome: u8,
    revision: u64,
    result_bidder_guid: u64,
    result_bid: u32,
    minimum_increment: u32,
    accepted_price: u32,
}

fn bid_decision_fields(decision: HoldDecision) -> BidDecisionFields {
    let mut fields = BidDecisionFields {
        outcome: BID_DATABASE,
        revision: 0,
        result_bidder_guid: 0,
        result_bid: 0,
        minimum_increment: 0,
        accepted_price: 0,
    };
    match decision {
        HoldDecision::Accepted(BidAcceptance {
            price,
            effect,
            displaced_bidder_guid,
            displaced_bid,
        }) => {
            fields.outcome = BID_ACCEPTED;
            fields.revision = match effect {
                AuctionBidEffect::RemainActive { revision } => revision,
                AuctionBidEffect::SettleBuyout => 0,
            };
            fields.result_bidder_guid = displaced_bidder_guid;
            fields.result_bid = displaced_bid;
            fields.accepted_price = price;
        }
        HoldDecision::ItemNotFound => fields.outcome = BID_ITEM_NOT_FOUND,
        HoldDecision::HigherBid {
            bidder_guid,
            current_bid,
            minimum_increment,
        } => {
            fields.outcome = BID_HIGHER;
            fields.result_bidder_guid = bidder_guid;
            fields.result_bid = current_bid;
            fields.minimum_increment = minimum_increment;
        }
        HoldDecision::BidIncrement => fields.outcome = BID_INCREMENT,
        HoldDecision::BidOwn => fields.outcome = BID_OWN,
        HoldDecision::Database => {}
        HoldDecision::Cancelled {
            cut,
            displaced_bidder_guid,
            displaced_bid,
        } => {
            fields.outcome = BID_CANCELLED;
            fields.result_bidder_guid = displaced_bidder_guid;
            fields.result_bid = displaced_bid;
            fields.accepted_price = cut;
        }
    }
    fields
}

fn bid_decision_from_fields(fields: BidDecisionFields, legacy_offer: u32) -> Option<HoldDecision> {
    match fields.outcome {
        BID_PENDING => None,
        BID_ACCEPTED => Some(HoldDecision::Accepted(BidAcceptance {
            price: if fields.accepted_price == 0 {
                legacy_offer
            } else {
                fields.accepted_price
            },
            effect: if fields.revision == 0 {
                AuctionBidEffect::SettleBuyout
            } else {
                AuctionBidEffect::RemainActive {
                    revision: fields.revision,
                }
            },
            displaced_bidder_guid: fields.result_bidder_guid,
            displaced_bid: fields.result_bid,
        })),
        BID_ITEM_NOT_FOUND => Some(HoldDecision::ItemNotFound),
        BID_HIGHER => Some(HoldDecision::HigherBid {
            bidder_guid: fields.result_bidder_guid,
            current_bid: fields.result_bid,
            minimum_increment: fields.minimum_increment,
        }),
        BID_INCREMENT => Some(HoldDecision::BidIncrement),
        BID_OWN => Some(HoldDecision::BidOwn),
        // No legacy fallback: a Cancellation's cut of 0 is a real charge of nothing.
        BID_CANCELLED => Some(HoldDecision::Cancelled {
            cut: fields.accepted_price,
            displaced_bidder_guid: fields.result_bidder_guid,
            displaced_bid: fields.result_bid,
        }),
        _ => Some(HoldDecision::Database),
    }
}

fn held_bid_decision(row: &AuctionBidHold) -> Option<HoldDecision> {
    bid_decision_from_fields(
        BidDecisionFields {
            outcome: row.outcome,
            revision: row.revision,
            result_bidder_guid: row.result_bidder_guid,
            result_bid: row.result_bid,
            minimum_increment: row.minimum_increment,
            accepted_price: row.accepted_price,
        },
        row.offer,
    )
}

fn realm_bid_decision(row: &AuctionBidDecision) -> Option<HoldDecision> {
    bid_decision_from_fields(
        BidDecisionFields {
            outcome: row.outcome,
            revision: row.revision,
            result_bidder_guid: row.result_bidder_guid,
            result_bid: row.result_bid,
            minimum_increment: row.minimum_increment,
            accepted_price: row.accepted_price,
        },
        row.offer,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeldBid {
    request: HoldRequest,
    decision: Option<HoldDecision>,
    deferred_refund: u32,
}

trait BidSource {
    fn money(&self, bidder_guid: u64) -> Option<u32>;
    fn hold(&self, operation_id: u64) -> Option<HeldBid>;
    fn create_hold(&mut self, request: HoldRequest) -> Result<(), AuctionRefusal>;
    fn finish_hold(
        &mut self,
        request: HoldRequest,
        decision: HoldDecision,
    ) -> Result<(), AuctionRefusal>;
    fn confirm_refund(&mut self, request: HoldRequest) -> Result<(), AuctionRefusal>;
}

/// Move a Hold's value out of the purse. A bid holds its full offer. A Cancellation holds the Auction
/// Cut, which is 0 for a listing nobody bid on, and still takes the Hold so the interaction Gate
/// runs on the Home Shard.
fn fence_bid<S: BidSource>(source: &mut S, request: HoldRequest) -> Result<(), AuctionRefusal> {
    if request.operation_id == 0
        || request.bidder_guid == 0
        || request.auction_id == 0
        || request.house == 0
        || (request.offer == 0 && request.operation == HoldOperation::Bid)
    {
        return Err(AuctionRefusal::Database);
    }
    if let Some(hold) = source.hold(request.operation_id) {
        return if hold.request == request {
            Ok(())
        } else {
            Err(AuctionRefusal::Database)
        };
    }
    if source
        .money(request.bidder_guid)
        .is_none_or(|money| money < request.offer)
    {
        return Err(AuctionRefusal::NotEnoughMoney);
    }
    source.create_hold(request)?;
    Ok(())
}

fn finish_bid<S: BidSource>(
    source: &mut S,
    request: HoldRequest,
    decision: HoldDecision,
) -> Result<HoldDecision, AuctionRefusal> {
    let hold = source
        .hold(request.operation_id)
        .ok_or(AuctionRefusal::Database)?;
    if hold.request != request || !decision.belongs_to(request.operation) {
        return Err(AuctionRefusal::Database);
    }
    if let Some(existing) = hold.decision {
        return if existing == decision {
            Ok(existing)
        } else {
            Err(AuctionRefusal::Database)
        };
    }
    source.finish_hold(request, decision)?;
    Ok(decision)
}

fn refundable_bid_value(decision: HoldDecision, offer: u32) -> Option<u32> {
    match decision {
        HoldDecision::Accepted(BidAcceptance { price, .. }) if price != 0 && price <= offer => {
            offer.checked_sub(price)
        }
        HoldDecision::Accepted(_) => None,
        // A Cancellation spends exactly the cut it held; a different cut was never agreed.
        HoldDecision::Cancelled { cut, .. } if cut == offer => Some(0),
        HoldDecision::Cancelled { .. } => None,
        _ => Some(offer),
    }
}

fn minimum_next_bid(auction: BidAuction) -> Result<u32, HoldDecision> {
    lyracore_shared::auction::minimum_next_bid(auction.start_bid, auction.highest_bid)
        .ok_or(HoldDecision::Database)
}

fn decide_bid(auction: Option<BidAuction>, request: HoldRequest, now_micros: i64) -> HoldDecision {
    if request.operation != HoldOperation::Bid
        || request.operation_id == 0
        || request.bidder_guid == 0
        || request.auction_id == 0
        || request.house == 0
        || request.offer == 0
    {
        return HoldDecision::Database;
    }
    let Some(auction) = auction.filter(|auction| {
        auction.id == request.auction_id
            && lyracore_shared::auction::market_of(auction.house)
                == lyracore_shared::auction::market_of(request.house)
            && auction.expires_micros > now_micros
    }) else {
        return HoldDecision::ItemNotFound;
    };
    if auction.owner_guid == request.bidder_guid {
        return HoldDecision::BidOwn;
    }
    let is_buyout = auction.buyout != 0 && request.offer >= auction.buyout;
    let minimum_increment = lyracore_shared::auction::bid_increment(auction.highest_bid);
    let minimum = if is_buyout {
        None
    } else {
        let Ok(minimum) = minimum_next_bid(auction) else {
            return HoldDecision::Database;
        };
        Some(minimum)
    };
    if auction.highest_bid != 0 && request.offer <= auction.highest_bid {
        return HoldDecision::HigherBid {
            bidder_guid: auction.highest_bidder_guid,
            current_bid: auction.highest_bid,
            minimum_increment,
        };
    }
    if is_buyout {
        if seller_proceeds(auction.buyout, auction.deposit, auction.consignment_rate).is_none() {
            return HoldDecision::Database;
        }
        return HoldDecision::Accepted(BidAcceptance {
            price: auction.buyout,
            effect: AuctionBidEffect::SettleBuyout,
            displaced_bidder_guid: auction.highest_bidder_guid,
            displaced_bid: auction.highest_bid,
        });
    }
    let Some(minimum) = minimum else {
        return HoldDecision::Database;
    };
    if request.offer < minimum {
        return HoldDecision::BidIncrement;
    }
    if seller_proceeds(request.offer, auction.deposit, auction.consignment_rate).is_none() {
        return HoldDecision::Database;
    }
    let Some(revision) = auction.revision.checked_add(1) else {
        return HoldDecision::Database;
    };
    HoldDecision::Accepted(BidAcceptance {
        price: request.offer,
        effect: AuctionBidEffect::RemainActive { revision },
        displaced_bidder_guid: auction.highest_bidder_guid,
        displaced_bid: auction.highest_bid,
    })
}

/// Realm-core's Cancellation Gate (`cm:AuctionHouseHandler.cpp:403-468`). Only the seller cancels,
/// only an active listing of the auctioneer's market, and only at the cut the seller's Hold paid
/// for: a bid that landed after the Gateway read the listing changes the cut and refuses the
/// Cancellation, so the Hold refunds and the seller can try again.
fn decide_cancel(
    auction: Option<BidAuction>,
    request: HoldRequest,
    now_micros: i64,
) -> HoldDecision {
    if request.operation != HoldOperation::Cancel
        || request.operation_id == 0
        || request.bidder_guid == 0
        || request.auction_id == 0
        || request.house == 0
    {
        return HoldDecision::Database;
    }
    let Some(auction) = auction.filter(|auction| {
        auction.id == request.auction_id
            && auction.owner_guid == request.bidder_guid
            && lyracore_shared::auction::market_of(auction.house)
                == lyracore_shared::auction::market_of(request.house)
            && auction.expires_micros > now_micros
    }) else {
        return HoldDecision::ItemNotFound;
    };
    // A bid without a bidder, or a bidder without a bid, has no one to refund; expiry keeps the
    // same state for repair.
    if (auction.highest_bidder_guid == 0) != (auction.highest_bid == 0) {
        return HoldDecision::Database;
    }
    match auction_cut(auction.highest_bid, auction.consignment_rate) {
        Some(cut) if cut == request.offer => HoldDecision::Cancelled {
            cut,
            displaced_bidder_guid: auction.highest_bidder_guid,
            displaced_bid: auction.highest_bid,
        },
        _ => HoldDecision::Database,
    }
}

trait BidMarket {
    fn decision(&self, operation_id: u64) -> Option<(HoldRequest, HoldDecision)>;
    fn auction(&self, auction_id: u32) -> Option<BidAuction>;
    fn now_micros(&self) -> i64;
    fn commit_decision(
        &mut self,
        request: HoldRequest,
        auction: Option<BidAuction>,
        decision: HoldDecision,
    ) -> Result<(), AuctionRefusal>;
}

trait BidRefundSink {
    fn refund_decision(&self, operation_id: u64) -> Option<(HoldRequest, HoldDecision, u32)>;
    fn commit_refund(&mut self, request: HoldRequest, amount: u32) -> Result<(), AuctionRefusal>;
}

fn relay_bid_refund<S: BidRefundSink>(
    sink: &mut S,
    request: HoldRequest,
    amount: u32,
) -> Result<(), AuctionRefusal> {
    if amount == 0 {
        return Ok(());
    }
    let (existing_request, decision, recorded) = sink
        .refund_decision(request.operation_id)
        .ok_or(AuctionRefusal::Database)?;
    if existing_request != request
        || refundable_bid_value(decision, request.offer).is_none_or(|limit| amount > limit)
    {
        return Err(AuctionRefusal::Database);
    }
    if recorded != 0 {
        return if recorded == amount {
            Ok(())
        } else {
            Err(AuctionRefusal::Database)
        };
    }
    sink.commit_refund(request, amount)
}

fn confirm_bid_refund<S: BidSource>(
    source: &mut S,
    request: HoldRequest,
    amount: u32,
) -> Result<(), AuctionRefusal> {
    if amount == 0 {
        return Err(AuctionRefusal::Database);
    }
    let hold = source
        .hold(request.operation_id)
        .ok_or(AuctionRefusal::Database)?;
    let decision = hold.decision.ok_or(AuctionRefusal::Database)?;
    if hold.request != request
        || refundable_bid_value(decision, request.offer).is_none_or(|limit| amount > limit)
    {
        return Err(AuctionRefusal::Database);
    }
    if hold.deferred_refund == 0 {
        return Ok(());
    }
    if hold.deferred_refund != amount {
        return Err(AuctionRefusal::Database);
    }
    source.confirm_refund(request)
}

fn resolve_bid<S: BidMarket>(
    market: &mut S,
    request: HoldRequest,
) -> Result<HoldDecision, AuctionRefusal> {
    if let Some((existing_request, decision)) = market.decision(request.operation_id) {
        return if existing_request == request {
            Ok(decision)
        } else {
            Err(AuctionRefusal::Database)
        };
    }
    let auction = market.auction(request.auction_id);
    let decision = match request.operation {
        HoldOperation::Bid => decide_bid(auction, request, market.now_micros()),
        HoldOperation::Cancel => decide_cancel(auction, request, market.now_micros()),
    };
    market.commit_decision(request, auction, decision)?;
    Ok(decision)
}

fn drive_bid<S: BidSource, M: BidMarket>(
    source: &mut S,
    market: &mut M,
    request: HoldRequest,
) -> Result<HoldDecision, AuctionRefusal> {
    fence_bid(source, request)?;
    if let Some(decision) = source
        .hold(request.operation_id)
        .and_then(|hold| hold.decision)
    {
        return Ok(decision);
    }
    let decision = resolve_bid(market, request)?;
    finish_bid(source, request, decision)
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

trait HoldSink: ListingSource {
    fn hold(&self, operation_id: u64) -> Option<ListingHold>;
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt>;
    fn commit_hold(&mut self, hold: ListingHold);
    fn confirm_hold(&mut self, receipt: ListingReceipt);
    fn delete_hold(&mut self, operation_id: u64);
}

trait ListingRefundSink {
    fn refund(&self, operation_id: u64) -> Result<Option<ListingRefund>, AuctionRefusal>;
    fn commit_refund(&mut self, refund: ListingRefund);
}

trait MarketSink {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt>;
    fn commit_market(&mut self, listing: PreparedListing) -> u32;
}

fn fence_listing<S: HoldSink>(sink: &mut S, request: ListingRequest) -> Result<(), AuctionRefusal> {
    if let Some(receipt) = sink.receipt(request.operation_id) {
        return if receipt.auction_id != 0 && receipt.listing.request == request {
            Ok(())
        } else {
            Err(AuctionRefusal::InvalidTerms)
        };
    }
    if let Some(hold) = sink.hold(request.operation_id) {
        return if hold.listing.request == request {
            Ok(())
        } else {
            Err(AuctionRefusal::InvalidTerms)
        };
    }
    let listing = prepare_from_source(sink, request)?;
    sink.commit_hold(ListingHold { listing });
    Ok(())
}

fn commit_held_listing<S: MarketSink>(
    sink: &mut S,
    listing: PreparedListing,
) -> Result<u32, AuctionRefusal> {
    if let Some(receipt) = sink.receipt(listing.request.operation_id) {
        return if receipt.listing == listing {
            if receipt.auction_id == 0 {
                Err(AuctionRefusal::InvalidTerms)
            } else {
                Ok(receipt.auction_id)
            }
        } else {
            Err(AuctionRefusal::InvalidTerms)
        };
    }
    Ok(sink.commit_market(listing))
}

fn confirm_listing<S: HoldSink>(
    sink: &mut S,
    receipt: ListingReceipt,
) -> Result<(), AuctionRefusal> {
    if receipt.auction_id == 0 {
        return Err(AuctionRefusal::InvalidTerms);
    }
    if let Some(existing) = sink.receipt(receipt.listing.request.operation_id) {
        return if existing == receipt {
            Ok(())
        } else {
            Err(AuctionRefusal::InvalidTerms)
        };
    }
    let hold = sink
        .hold(receipt.listing.request.operation_id)
        .ok_or(AuctionRefusal::InvalidTerms)?;
    if hold.listing != receipt.listing {
        return Err(AuctionRefusal::InvalidTerms);
    }
    sink.confirm_hold(receipt);
    Ok(())
}

fn settle_listing<S: HoldSink>(sink: &mut S, operation_id: u64) -> Result<(), AuctionRefusal> {
    let receipt = sink
        .receipt(operation_id)
        .ok_or(AuctionRefusal::InvalidTerms)?;
    if receipt.auction_id == 0 {
        return Err(AuctionRefusal::InvalidTerms);
    }
    match sink.hold(operation_id) {
        None => Ok(()),
        Some(hold) if hold.listing == receipt.listing => {
            sink.delete_hold(operation_id);
            Ok(())
        }
        Some(_) => Err(AuctionRefusal::InvalidTerms),
    }
}

fn listing_refund(listing: &PreparedListing) -> ListingRefund {
    ListingRefund {
        listing: listing.clone(),
        mail: listing_release_mail(listing),
    }
}

fn refund_listing<S: ListingRefundSink>(
    sink: &mut S,
    refund: ListingRefund,
) -> Result<(), AuctionRefusal> {
    match sink.refund(refund.listing.request.operation_id)? {
        Some(existing) if existing == refund => Ok(()),
        Some(_) => Err(AuctionRefusal::InvalidTerms),
        None => {
            sink.commit_refund(refund);
            Ok(())
        }
    }
}

/// Give a Hold's value back after realm-core refused its listing. A Hold with a matching receipt
/// backs a live Auction and is never released; a missing Hold is a replay.
fn release_listing<S: HoldSink>(
    sink: &mut S,
    operation_id: u64,
    seller_guid: u64,
) -> Result<(), AuctionRefusal> {
    if sink.receipt(operation_id).is_some() {
        return Err(AuctionRefusal::InvalidTerms);
    }
    let Some(hold) = sink.hold(operation_id) else {
        return Ok(());
    };
    if hold.listing.request.seller_guid != seller_guid {
        return Err(AuctionRefusal::InvalidTerms);
    }
    sink.delete_hold(operation_id);
    Ok(())
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

/// Reducer edge: only the tag crosses to the gateway; the detail stays in module logs.
fn refused(refusal: AuctionRefusal, detail: &str) -> String {
    let tag = refusal.as_tag();
    spacetimedb::log::info!("auction refused {tag}: {detail}");
    tag.to_string()
}

fn listing_from_hold(row: AuctionHold) -> PreparedListing {
    PreparedListing {
        request: ListingRequest {
            operation_id: row.operation_id,
            seller_guid: row.seller_guid,
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
        },
        deposit: row.deposit,
        created_micros: row.created_micros,
        expires_micros: row.expires_micros,
    }
}

fn hold_from_listing(listing: PreparedListing) -> AuctionHold {
    AuctionHold {
        operation_id: listing.request.operation_id,
        seller_guid: listing.request.seller_guid,
        item_guid: listing.request.item_guid,
        item_entry: listing.snapshot.entry,
        item_stack_count: listing.snapshot.stack_count,
        item_durability: listing.snapshot.durability,
        item_enchant_id: listing.snapshot.enchant_id,
        item_soulbound: listing.snapshot.soulbound,
        random_property_id: listing.snapshot.random_property_id,
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
            item_text_id: item.item_text_id,
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
    ctx.db.game_world_entity().guid().update(seller);
    ctx.db
        .game_item_instance()
        .guid()
        .delete(listing.request.item_guid);
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

impl HoldSink for CtxSource<'_> {
    fn hold(&self, operation_id: u64) -> Option<ListingHold> {
        self.ctx
            .db
            .game_auction_hold()
            .operation_id()
            .find(operation_id)
            .map(|row| ListingHold {
                listing: listing_from_hold(row),
            })
    }

    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
        <Self as LocalListingSink>::receipt(self, operation_id)
    }

    fn commit_hold(&mut self, hold: ListingHold) {
        consume_listing_value(self.ctx, &hold.listing);
        self.ctx
            .db
            .game_auction_hold()
            .insert(hold_from_listing(hold.listing));
    }

    fn confirm_hold(&mut self, receipt: ListingReceipt) {
        self.ctx
            .db
            .game_auction_operation_receipt()
            .insert(receipt_from_listing(receipt.listing, receipt.auction_id));
    }

    fn delete_hold(&mut self, operation_id: u64) {
        self.ctx
            .db
            .game_auction_hold()
            .operation_id()
            .delete(operation_id);
    }
}

struct CtxMarket<'a> {
    ctx: &'a ReducerContext,
}

struct CtxListingRefund<'a> {
    ctx: &'a ReducerContext,
}

impl ListingRefundSink for CtxListingRefund<'_> {
    fn refund(&self, operation_id: u64) -> Result<Option<ListingRefund>, AuctionRefusal> {
        let row = self
            .ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(operation_id);
        match row {
            None => Ok(None),
            Some(row) if row.auction_id == 0 => {
                Ok(Some(listing_refund(&listing_from_receipt(row).listing)))
            }
            Some(_) => Err(AuctionRefusal::InvalidTerms),
        }
    }

    fn commit_refund(&mut self, refund: ListingRefund) {
        insert_auction_mail(self.ctx, refund.mail);
        self.ctx
            .db
            .game_auction_operation_receipt()
            .insert(receipt_from_listing(refund.listing, 0));
    }
}

impl MarketSink for CtxMarket<'_> {
    fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
        self.ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(operation_id)
            .map(listing_from_receipt)
    }

    fn commit_market(&mut self, listing: PreparedListing) -> u32 {
        let auction_id = insert_active_auction(self.ctx, &listing);
        self.ctx
            .db
            .game_auction_operation_receipt()
            .insert(receipt_from_listing(listing, auction_id));
        auction_id
    }
}

struct CtxBidSource<'a> {
    ctx: &'a ReducerContext,
}

impl BidSource for CtxBidSource<'_> {
    fn money(&self, bidder_guid: u64) -> Option<u32> {
        crate::helpers::acting_entity_by_guid(self.ctx, bidder_guid).map(|bidder| bidder.money)
    }

    fn hold(&self, operation_id: u64) -> Option<HeldBid> {
        let row = self
            .ctx
            .db
            .game_auction_bid_hold()
            .operation_id()
            .find(operation_id)?;
        Some(HeldBid {
            request: hold_request(
                HoldOperation::from_code(row.operation)?,
                row.operation_id,
                row.bidder_guid,
                row.auction_id,
                row.house,
                row.offer,
            ),
            decision: held_bid_decision(&row),
            deferred_refund: row.deferred_refund,
        })
    }

    fn create_hold(&mut self, request: HoldRequest) -> Result<(), AuctionRefusal> {
        let mut bidder = crate::helpers::acting_entity_by_guid(self.ctx, request.bidder_guid)
            .ok_or(AuctionRefusal::NotEnoughMoney)?;
        bidder.money = bidder
            .money
            .checked_sub(request.offer)
            .ok_or(AuctionRefusal::NotEnoughMoney)?;
        self.ctx.db.game_world_entity().guid().update(bidder);
        self.ctx.db.game_auction_bid_hold().insert(AuctionBidHold {
            operation_id: request.operation_id,
            bidder_guid: request.bidder_guid,
            auction_id: request.auction_id,
            house: request.house,
            offer: request.offer,
            outcome: BID_PENDING,
            revision: 0,
            result_bidder_guid: 0,
            result_bid: 0,
            minimum_increment: 0,
            accepted_price: 0,
            deferred_refund: 0,
            operation: request.operation.code(),
        });
        Ok(())
    }

    fn finish_hold(
        &mut self,
        request: HoldRequest,
        decision: HoldDecision,
    ) -> Result<(), AuctionRefusal> {
        let refund =
            refundable_bid_value(decision, request.offer).ok_or(AuctionRefusal::Database)?;
        let deferred_refund = if refund != 0 {
            let mut bidder = crate::helpers::acting_entity_by_guid(self.ctx, request.bidder_guid)
                .ok_or(AuctionRefusal::Database)?;
            let (money, deferred_refund) = crate::mail::split_refund(bidder.money, refund);
            bidder.money = money;
            self.ctx.db.game_world_entity().guid().update(bidder);
            deferred_refund
        } else {
            0
        };
        let fields = bid_decision_fields(decision);
        self.ctx
            .db
            .game_auction_bid_hold()
            .operation_id()
            .update(AuctionBidHold {
                operation_id: request.operation_id,
                bidder_guid: request.bidder_guid,
                auction_id: request.auction_id,
                house: request.house,
                offer: request.offer,
                outcome: fields.outcome,
                revision: fields.revision,
                result_bidder_guid: fields.result_bidder_guid,
                result_bid: fields.result_bid,
                minimum_increment: fields.minimum_increment,
                accepted_price: fields.accepted_price,
                deferred_refund,
                operation: request.operation.code(),
            });
        Ok(())
    }

    fn confirm_refund(&mut self, request: HoldRequest) -> Result<(), AuctionRefusal> {
        let mut row = self
            .ctx
            .db
            .game_auction_bid_hold()
            .operation_id()
            .find(request.operation_id)
            .ok_or(AuctionRefusal::Database)?;
        if row.operation != request.operation.code()
            || row.bidder_guid != request.bidder_guid
            || row.auction_id != request.auction_id
            || row.house != request.house
            || row.offer != request.offer
        {
            return Err(AuctionRefusal::Database);
        }
        row.deferred_refund = 0;
        self.ctx
            .db
            .game_auction_bid_hold()
            .operation_id()
            .update(row);
        Ok(())
    }
}

struct CtxBidMarket<'a> {
    ctx: &'a ReducerContext,
}

impl BidMarket for CtxBidMarket<'_> {
    fn decision(&self, operation_id: u64) -> Option<(HoldRequest, HoldDecision)> {
        let row = self
            .ctx
            .db
            .game_auction_bid_decision()
            .operation_id()
            .find(operation_id)?;
        Some((decided_request(&row)?, realm_bid_decision(&row)?))
    }

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

    fn commit_decision(
        &mut self,
        request: HoldRequest,
        auction: Option<BidAuction>,
        decision: HoldDecision,
    ) -> Result<(), AuctionRefusal> {
        if let HoldDecision::Accepted(accepted) = decision {
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
        if let HoldDecision::Cancelled {
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
        let fields = bid_decision_fields(decision);
        // The Auction row this decision is about may be deleted later (settled or expired) before
        // a purse-overflow refund on it fires. Keep the item the refund's subject needs here,
        // while the row is still guaranteed present.
        let (item_entry, random_property_id) =
            auction.map_or((0, 0), |a| (a.item.entry, a.item.random_property_id));
        self.ctx
            .db
            .game_auction_bid_decision()
            .insert(AuctionBidDecision {
                operation_id: request.operation_id,
                bidder_guid: request.bidder_guid,
                auction_id: request.auction_id,
                house: request.house,
                offer: request.offer,
                outcome: fields.outcome,
                revision: fields.revision,
                result_bidder_guid: fields.result_bidder_guid,
                result_bid: fields.result_bid,
                minimum_increment: fields.minimum_increment,
                accepted_price: fields.accepted_price,
                deferred_refund: 0,
                item_entry,
                random_property_id,
                operation: request.operation.code(),
            });
        Ok(())
    }
}

impl CtxBidMarket<'_> {
    /// Remove the listing the decision read, return its item to the seller, and refund the
    /// displaced bidder, all in the decision's transaction.
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

impl BidRefundSink for CtxBidMarket<'_> {
    fn refund_decision(&self, operation_id: u64) -> Option<(HoldRequest, HoldDecision, u32)> {
        let row = self
            .ctx
            .db
            .game_auction_bid_decision()
            .operation_id()
            .find(operation_id)?;
        Some((
            decided_request(&row)?,
            realm_bid_decision(&row)?,
            row.deferred_refund,
        ))
    }

    fn commit_refund(&mut self, request: HoldRequest, amount: u32) -> Result<(), AuctionRefusal> {
        let mut row = self
            .ctx
            .db
            .game_auction_bid_decision()
            .operation_id()
            .find(request.operation_id)
            .ok_or(AuctionRefusal::Database)?;
        if decided_request(&row) != Some(request) || row.deferred_refund != 0 {
            return Err(AuctionRefusal::Database);
        }
        // The Auction this Hold was about may already be settled and gone (this refund is the
        // purse-overflow remainder of a bid that did not win, or of a refused Cancellation's cut).
        // No vanilla auction sends this mail at all; it renders as the closest client string,
        // Outbid, carrying the refund. The item comes from the decision row itself, not a fresh
        // Auction lookup, because the row this refund is about can outlive the Auction it was
        // decided against.
        insert_auction_mail(
            self.ctx,
            AuctionMail {
                recipient_guid: request.bidder_guid,
                house: request.house,
                action: AuctionMailAction::Outbid,
                item_entry: row.item_entry,
                random_property_id: row.random_property_id,
                money: amount,
                attached_item: crate::items::ItemSnapshot::default(),
                counterparty_guid: 0,
                bid: 0,
                buyout: 0,
                deposit: 0,
                cut: 0,
            },
        );
        row.deferred_refund = amount;
        self.ctx
            .db
            .game_auction_bid_decision()
            .operation_id()
            .update(row);
        Ok(())
    }
}

fn hold_request(
    operation: HoldOperation,
    operation_id: u64,
    bidder_guid: u64,
    auction_id: u32,
    house: u32,
    offer: u32,
) -> HoldRequest {
    HoldRequest {
        operation,
        operation_id,
        bidder_guid,
        auction_id,
        house,
        offer,
    }
}

/// The request a Realm-core decision row records. An operation code this Module does not know
/// reads as no request, so every comparison against it fails closed.
fn decided_request(row: &AuctionBidDecision) -> Option<HoldRequest> {
    Some(hold_request(
        HoldOperation::from_code(row.operation)?,
        row.operation_id,
        row.bidder_guid,
        row.auction_id,
        row.house,
        row.offer,
    ))
}

/// The operation a stored Hold or decision row pays for. The later phases take their operation
/// from the row, never from the caller, so one reducer finishes bids and Cancellations alike. A
/// missing row reads as a bid; the phase then finds no matching Hold and refuses.
fn stored_hold_operation(ctx: &ReducerContext, operation_id: u64) -> HoldOperation {
    ctx.db
        .game_auction_bid_hold()
        .operation_id()
        .find(operation_id)
        .map(|row| row.operation)
        .or_else(|| {
            ctx.db
                .game_auction_bid_decision()
                .operation_id()
                .find(operation_id)
                .map(|row| row.operation)
        })
        .and_then(HoldOperation::from_code)
        .unwrap_or(HoldOperation::Bid)
}

fn validate_market_listing(ctx: &ReducerContext, listing: &PreparedListing) -> Result<(), String> {
    if listing.request.operation_id == 0
        || listing.request.house.id == 0
        || !valid_rate(listing.request.house.deposit_rate)
        || !valid_rate(listing.request.house.consignment_rate)
        || listing.snapshot.stack_count == 0
        || listing.snapshot.soulbound
        || listing.request.terms.start_bid == 0
        || (listing.request.terms.buyout != 0
            && listing.request.terms.buyout < listing.request.terms.start_bid)
    {
        return Err(refused(
            AuctionRefusal::InvalidTerms,
            "invalid held listing",
        ));
    }
    let template = ctx
        .db
        .game_item_template()
        .entry()
        .find(listing.snapshot.entry)
        .ok_or_else(|| refused(AuctionRefusal::InvalidTerms, "item template missing"))?;
    let expected_deposit = listing_deposit(
        template.sell_price,
        listing.snapshot.stack_count,
        listing.request.terms.duration_minutes,
        listing.request.house.deposit_rate,
    )
    .ok_or_else(|| refused(AuctionRefusal::InvalidTerms, "invalid listing arithmetic"))?;
    let expected_expiry = i64::from(listing.request.terms.duration_minutes)
        .checked_mul(MICROS_PER_MINUTE)
        .and_then(|duration| listing.created_micros.checked_add(duration))
        .ok_or_else(|| refused(AuctionRefusal::InvalidTerms, "auction expiry overflow"))?;
    if listing.deposit != expected_deposit
        || !listing_proceeds_are_representable(
            listing.request.terms,
            expected_deposit,
            listing.request.house.consignment_rate,
        )
        || listing.expires_micros != expected_expiry
    {
        return Err(refused(
            AuctionRefusal::InvalidTerms,
            "held listing payload changed",
        ));
    }
    Ok(())
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

/// Single-database listing: item, deposit, Auction, receipt, and expiry are one transaction.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn gw_auction_list_local(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    item_guid: u64,
    auctioneer_guid: u64,
    house: u32,
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seller_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let requested_house = house;
    let existing_house = ctx
        .db
        .game_auction_operation_receipt()
        .operation_id()
        .find(operation_id)
        .map(listing_from_receipt)
        .map(|receipt| receipt.listing.request.house);
    let replay = existing_house.is_some();
    let house = match existing_house {
        Some(policy) if policy.id == requested_house => policy,
        Some(_) => {
            return Err(refused(
                AuctionRefusal::InvalidTerms,
                "listing operation id conflict",
            ));
        }
        None => imported_house_policy(ctx, requested_house)?,
    };
    if !replay && auction_house_for_interaction(ctx, seller_guid, auctioneer_guid) != Some(house) {
        return Err(refused(
            AuctionRefusal::InvalidTerms,
            "auctioneer refused interaction",
        ));
    }
    create_local_listing(
        &mut CtxSource { ctx },
        ListingRequest {
            operation_id,
            seller_guid,
            item_guid,
            house,
            terms: ListingTerms {
                start_bid,
                buyout,
                duration_minutes,
            },
        },
    )
    .map(|_| ())
    .map_err(|refusal| refused(refusal, "listing rejected"))
}

/// Sharded listing phase 1: atomically move the source value into a caller-identified Hold.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn gw_auction_hold_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    item_guid: u64,
    auctioneer_guid: u64,
    house: u32,
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seller_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let requested_house = house;
    let existing_house = ctx
        .db
        .game_auction_hold()
        .operation_id()
        .find(operation_id)
        .map(listing_from_hold)
        .map(|listing| listing.request.house)
        .or_else(|| {
            ctx.db
                .game_auction_operation_receipt()
                .operation_id()
                .find(operation_id)
                .map(listing_from_receipt)
                .map(|receipt| receipt.listing.request.house)
        });
    let replay = existing_house.is_some();
    let house = match existing_house {
        Some(policy) if policy.id == requested_house => policy,
        Some(_) => {
            return Err(refused(
                AuctionRefusal::InvalidTerms,
                "listing operation id conflict",
            ));
        }
        None => imported_house_policy(ctx, requested_house)?,
    };
    if !replay && auction_house_for_interaction(ctx, seller_guid, auctioneer_guid) != Some(house) {
        return Err(refused(
            AuctionRefusal::InvalidTerms,
            "auctioneer refused interaction",
        ));
    }
    fence_listing(
        &mut CtxSource { ctx },
        ListingRequest {
            operation_id,
            seller_guid,
            item_guid,
            house,
            terms: ListingTerms {
                start_bid,
                buyout,
                duration_minutes,
            },
        },
    )
    .map_err(|refusal| refused(refusal, "listing Hold rejected"))
}

/// Sharded listing phase 2: create the realm Auction and idempotency receipt from a held payload.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn realm_auction_commit_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    item_guid: u64,
    item_entry: u32,
    item_stack_count: u32,
    item_durability: u32,
    item_enchant_id: u32,
    item_soulbound: bool,
    random_property_id: u32,
    house: u32,
    deposit_rate: u32,
    consignment_rate: u32,
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
    deposit: u32,
    created_micros: i64,
    expires_micros: i64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seller_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let listing = PreparedListing {
        request: ListingRequest {
            operation_id,
            seller_guid,
            item_guid,
            house: AuctionHousePolicy {
                id: house,
                deposit_rate,
                consignment_rate,
            },
            terms: ListingTerms {
                start_bid,
                buyout,
                duration_minutes,
            },
        },
        snapshot: crate::items::ItemSnapshot {
            entry: item_entry,
            stack_count: item_stack_count,
            durability: item_durability,
            enchant_id: item_enchant_id,
            soulbound: item_soulbound,
            random_property_id,
        },
        deposit,
        created_micros,
        expires_micros,
    };
    let mut market = CtxMarket { ctx };
    if market.receipt(operation_id).is_none() {
        validate_market_listing(ctx, &listing)?;
    }
    commit_held_listing(&mut market, listing)
        .map(|_| ())
        .map_err(|refusal| refused(refusal, "listing operation id conflict"))
}

fn require_listing_actor(
    ctx: &ReducerContext,
    operation_id: u64,
    actor: crate::SessionActor,
) -> Result<(), String> {
    crate::account_ownership::require_actor(ctx, actor)?;
    let seller = CtxSource { ctx }
        .hold(operation_id)
        .map(|hold| hold.listing.request.seller_guid)
        .or_else(|| {
            <CtxSource<'_> as HoldSink>::receipt(&CtxSource { ctx }, operation_id)
                .map(|receipt| receipt.listing.request.seller_guid)
        });
    if let Some(guid) = seller {
        crate::account_ownership::require_actor_for(ctx, actor, guid)?;
    }
    Ok(())
}

/// Sharded listing phase 3: copy the matching realm receipt onto the source shard.
#[reducer]
pub fn realm_auction_confirm_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    auction_id: u32,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    require_listing_actor(ctx, operation_id, request_actor)?;
    let listing = CtxSource { ctx }
        .hold(operation_id)
        .map(|hold| hold.listing)
        .or_else(|| {
            <CtxSource<'_> as HoldSink>::receipt(&CtxSource { ctx }, operation_id)
                .map(|receipt| receipt.listing)
        })
        .ok_or_else(|| refused(AuctionRefusal::InvalidTerms, "listing Hold missing"))?;
    confirm_listing(
        &mut CtxSource { ctx },
        ListingReceipt {
            listing,
            auction_id,
        },
    )
    .map_err(|refusal| refused(refusal, "listing receipt conflict"))
}

/// Sharded listing phase 4: delete the Hold only after the source has matching receipt evidence.
#[reducer]
pub fn realm_auction_settle_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    require_listing_actor(ctx, operation_id, request_actor)?;
    settle_listing(&mut CtxSource { ctx }, operation_id)
        .map_err(|refusal| refused(refusal, "listing Hold is not confirmed"))
}

/// Sharded listing abort phase 3: Realm-core commits the seller's exact return Mail and durable
/// refund receipt together. Replays with the same payload do not create a second Mail.
#[reducer]
#[allow(clippy::too_many_arguments)] // The persisted refund receipt's value columns.
pub fn realm_auction_refund_listing(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    item_guid: u64,
    item_entry: u32,
    item_stack_count: u32,
    item_durability: u32,
    item_enchant_id: u32,
    item_soulbound: bool,
    random_property_id: u32,
    house: u32,
    deposit_rate: u32,
    consignment_rate: u32,
    start_bid: u32,
    buyout: u32,
    duration_minutes: u32,
    deposit: u32,
    created_micros: i64,
    expires_micros: i64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seller_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    refund_listing(
        &mut CtxListingRefund { ctx },
        listing_refund(&listing_from_hold(AuctionHold {
            operation_id,
            seller_guid,
            item_guid,
            item_entry,
            item_stack_count,
            item_durability,
            item_enchant_id,
            item_soulbound,
            random_property_id,
            house,
            deposit_rate,
            consignment_rate,
            start_bid,
            buyout,
            duration_minutes,
            deposit,
            created_micros,
            expires_micros,
        })),
    )
    .map_err(|refusal| refused(refusal, "listing refund conflict"))
}

/// Sharded listing abort: after realm-core refuses phase 2, mail the held item and deposit back
/// to the seller and delete the Hold. The gateway calls this only after Realm-core has committed
/// the matching refund receipt and Mail. Refused once the Hold has a receipt.
#[reducer]
pub fn gw_auction_release_listing_hold(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seller_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    release_listing(&mut CtxSource { ctx }, operation_id, seller_guid)
        .map_err(|refusal| refused(refusal, "listing Hold is confirmed"))
}

/// The Home Shard half every Hold starts with: the interaction Gate on a fresh operation, then the
/// fence. A replay skips the Gate, because the Hold already proves it passed.
fn gate_and_fence(
    ctx: &ReducerContext,
    request: HoldRequest,
    auctioneer_guid: u64,
) -> Result<(), String> {
    let replay = ctx
        .db
        .game_auction_bid_hold()
        .operation_id()
        .find(request.operation_id)
        .is_some();
    if !replay
        && auction_house_for_interaction(ctx, request.bidder_guid, auctioneer_guid)
            .is_none_or(|policy| policy.id != request.house)
    {
        return Err(refused(
            AuctionRefusal::Database,
            "auctioneer refused interaction",
        ));
    }
    fence_bid(&mut CtxBidSource { ctx }, request)
        .map_err(|refusal| refused(refusal, "Hold rejected"))
}

/// Single-database Hold: the fence, the realm decision, its Auction and mail effects, the terminal
/// source outcome and any purse-overflow refund mail commit in one transaction.
fn drive_local_hold(
    ctx: &ReducerContext,
    request: HoldRequest,
    auctioneer_guid: u64,
) -> Result<(), String> {
    gate_and_fence(ctx, request, auctioneer_guid)?;
    drive_bid(
        &mut CtxBidSource { ctx },
        &mut CtxBidMarket { ctx },
        request,
    )
    .map_err(|refusal| refused(refusal, "local Hold rejected"))?;
    let deferred_refund = CtxBidSource { ctx }
        .hold(request.operation_id)
        .ok_or_else(|| refused(AuctionRefusal::Database, "local Hold missing"))?
        .deferred_refund;
    if deferred_refund != 0 {
        relay_bid_refund(&mut CtxBidMarket { ctx }, request, deferred_refund)
            .map_err(|refusal| refused(refusal, "local Hold refund conflict"))?;
        confirm_bid_refund(&mut CtxBidSource { ctx }, request, deferred_refund)
            .map_err(|refusal| refused(refusal, "local Hold refund confirmation conflict"))?;
    }
    Ok(())
}

/// Single-database bid: full-offer Hold, realm decision, Auction update or buyout settlement,
/// ordinary mail, and terminal source outcome commit atomically.
#[reducer]
pub fn gw_auction_bid_local(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auctioneer_guid: u64,
    auction_id: u32,
    house: u32,
    offer: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let bidder_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let request = hold_request(
        HoldOperation::Bid,
        operation_id,
        bidder_guid,
        auction_id,
        house,
        offer,
    );
    drive_local_hold(ctx, request, auctioneer_guid)
}

/// Single-database Cancellation: the cut's Hold, the realm decision, the listing's removal, its
/// Cancelled and refund mail, and the terminal source outcome commit atomically.
#[reducer]
pub fn gw_auction_cancel_local(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auctioneer_guid: u64,
    auction_id: u32,
    house: u32,
    cut: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seller_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let request = hold_request(
        HoldOperation::Cancel,
        operation_id,
        seller_guid,
        auction_id,
        house,
        cut,
    );
    drive_local_hold(ctx, request, auctioneer_guid)
}

/// Sharded bid phase 1: move the complete offer into a source-shard Hold before realm-core decides.
#[reducer]
pub fn gw_auction_hold_bid(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auctioneer_guid: u64,
    auction_id: u32,
    house: u32,
    offer: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let bidder_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let request = hold_request(
        HoldOperation::Bid,
        operation_id,
        bidder_guid,
        auction_id,
        house,
        offer,
    );
    gate_and_fence(ctx, request, auctioneer_guid)
}

/// Sharded Cancellation phase 1: move the Auction Cut into a source-shard Hold before realm-core
/// decides. A seller who cannot pay it is refused with nothing held.
#[reducer]
pub fn gw_auction_hold_cancel(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auctioneer_guid: u64,
    auction_id: u32,
    house: u32,
    cut: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seller_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let request = hold_request(
        HoldOperation::Cancel,
        operation_id,
        seller_guid,
        auction_id,
        house,
        cut,
    );
    gate_and_fence(ctx, request, auctioneer_guid)
}

/// Sharded bid phase 2: serialize against the realm Auction and persist one terminal decision.
#[reducer]
pub fn realm_auction_decide_bid(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auction_id: u32,
    house: u32,
    offer: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let bidder_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    resolve_bid(
        &mut CtxBidMarket { ctx },
        hold_request(
            HoldOperation::Bid,
            operation_id,
            bidder_guid,
            auction_id,
            house,
            offer,
        ),
    )
    .map(|_| ())
    .map_err(|refusal| refused(refusal, "bid decision conflict"))
}

/// Sharded Cancellation phase 2: serialize against the realm Auction and persist one terminal
/// decision. A Cancelled decision removes the listing and writes its mail and notice in the same
/// transaction; a replay changes nothing.
#[reducer]
pub fn realm_auction_decide_cancel(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auction_id: u32,
    house: u32,
    cut: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let seller_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    resolve_bid(
        &mut CtxBidMarket { ctx },
        hold_request(
            HoldOperation::Cancel,
            operation_id,
            seller_guid,
            auction_id,
            house,
            cut,
        ),
    )
    .map(|_| ())
    .map_err(|refusal| refused(refusal, "Cancellation decision conflict"))
}

/// Sharded phase 3 for a bid or a Cancellation: consume the accepted price or the cut, or restore
/// refused value, exactly once. The operation comes from the stored Hold.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn gw_auction_finish_bid(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auction_id: u32,
    house: u32,
    offer: u32,
    outcome: u8,
    revision: u64,
    result_bidder_guid: u64,
    result_bid: u32,
    minimum_increment: u32,
    accepted_price: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let bidder_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let decision = bid_decision_from_fields(
        BidDecisionFields {
            outcome,
            revision,
            result_bidder_guid,
            result_bid,
            minimum_increment,
            accepted_price,
        },
        offer,
    )
    .ok_or_else(|| refused(AuctionRefusal::Database, "Hold decision is pending"))?;
    finish_bid(
        &mut CtxBidSource { ctx },
        hold_request(
            stored_hold_operation(ctx, operation_id),
            operation_id,
            bidder_guid,
            auction_id,
            house,
            offer,
        ),
        decision,
    )
    .map(|_| ())
    .map_err(|refusal| refused(refusal, "Hold outcome conflict"))
}

/// Sharded phase 4 for a bid or a Cancellation: place an unrepresentable purse refund in
/// realm-core mail exactly once. The operation comes from the stored decision.
#[reducer]
pub fn realm_auction_refund_bid(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auction_id: u32,
    house: u32,
    offer: u32,
    deferred_refund: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let bidder_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    relay_bid_refund(
        &mut CtxBidMarket { ctx },
        hold_request(
            stored_hold_operation(ctx, operation_id),
            operation_id,
            bidder_guid,
            auction_id,
            house,
            offer,
        ),
        deferred_refund,
    )
    .map_err(|refusal| refused(refusal, "Hold refund conflict"))
}

/// Sharded phase 5 for a bid or a Cancellation: record on the source that realm-core durably
/// accepted the refund mail. The operation comes from the stored Hold.
#[reducer]
pub fn gw_auction_confirm_bid_refund(
    ctx: &ReducerContext,
    operation_id: u64,
    request_actor: crate::SessionActor,
    auction_id: u32,
    house: u32,
    offer: u32,
    deferred_refund: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let bidder_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    confirm_bid_refund(
        &mut CtxBidSource { ctx },
        hold_request(
            stored_hold_operation(ctx, operation_id),
            operation_id,
            bidder_guid,
            auction_id,
            house,
            offer,
        ),
        deferred_refund,
    )
    .map_err(|refusal| refused(refusal, "Hold refund confirmation conflict"))
}

/// Scheduler-only one-shot expiry. Replays see no active Auction and therefore create no mail.
#[reducer]
pub fn expire_auction(ctx: &ReducerContext, schedule: AuctionExpiry) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    expire_active(&mut CtxExpiry { ctx }, schedule.auction_id)
}

#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_AUCTION_ID: u32 = 509_0050;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_OPERATION_ID: u64 = 509_0050;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_SELLER_GUID: u64 = 509_0050;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_WINNER_GUID: u64 = 509_0051;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_DISPLACED_GUID: u64 = 509_0052;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_NEW_BID_AUCTION_ID: u32 = 509_0056;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_NEW_BID_OPERATION_ID: u64 = 509_0056;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_NEW_BID_SELLER_GUID: u64 = 509_0057;
#[cfg(feature = "debug_reducers")]
const BUYOUT_FIXTURE_NEW_BID_BIDDER_GUID: u64 = 509_0058;

/// Stage one reserved Auction row for the standalone buyout integration test, plus a second,
/// unbid, no-buyout Auction (`BUYOUT_FIXTURE_NEW_BID_*`) the same test drives through
/// `AuctionBidEffect::RemainActive` to cover the New Bid notice.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_stage_auction_buyout_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    ctx.db
        .game_auction_expiry()
        .auction_id()
        .delete(BUYOUT_FIXTURE_AUCTION_ID);
    ctx.db.game_auction().id().delete(BUYOUT_FIXTURE_AUCTION_ID);
    ctx.db
        .game_auction_bid_decision()
        .operation_id()
        .delete(BUYOUT_FIXTURE_OPERATION_ID);
    ctx.db
        .game_auction_expiry()
        .auction_id()
        .delete(BUYOUT_FIXTURE_NEW_BID_AUCTION_ID);
    ctx.db
        .game_auction()
        .id()
        .delete(BUYOUT_FIXTURE_NEW_BID_AUCTION_ID);
    ctx.db
        .game_auction_bid_decision()
        .operation_id()
        .delete(BUYOUT_FIXTURE_NEW_BID_OPERATION_ID);

    let mails = ctx.db.game_mail();
    let notices = ctx.db.game_auction_notice();
    for recipient_guid in [
        BUYOUT_FIXTURE_SELLER_GUID,
        BUYOUT_FIXTURE_WINNER_GUID,
        BUYOUT_FIXTURE_DISPLACED_GUID,
        BUYOUT_FIXTURE_NEW_BID_SELLER_GUID,
        BUYOUT_FIXTURE_NEW_BID_BIDDER_GUID,
    ] {
        let stale_mail: Vec<u64> = mails
            .by_recipient()
            .filter(&recipient_guid)
            .filter(|mail| mail.sender_kind == lyracore_shared::mail::SENDER_KIND_AUCTION)
            .map(|mail| mail.id)
            .collect();
        for id in stale_mail {
            crate::mail::delete_mail(ctx, id);
        }
        let stale_notices: Vec<u64> = notices
            .by_recipient()
            .filter(&recipient_guid)
            .map(|notice| notice.id)
            .collect();
        for id in stale_notices {
            notices.id().delete(id);
        }
    }

    let expires_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .checked_add(3_600_000_000)
        .ok_or_else(|| "auction buyout fixture expiry overflow".to_string())?;
    let expires_at = Timestamp::from_micros_since_unix_epoch(expires_micros);
    ctx.db.game_auction().insert(Auction {
        id: BUYOUT_FIXTURE_AUCTION_ID,
        listing_operation_id: BUYOUT_FIXTURE_OPERATION_ID - 1,
        house: 1,
        owner_guid: BUYOUT_FIXTURE_SELLER_GUID,
        item_guid: 509_0053,
        item_entry: 509_0050,
        item_stack_count: 2,
        item_durability: 17,
        item_enchant_id: 9,
        item_soulbound: false,
        random_property_id: 117,
        start_bid: 100,
        buyout: 500,
        highest_bidder_guid: BUYOUT_FIXTURE_DISPLACED_GUID,
        highest_bid: 201,
        deposit: 10,
        created_at: ctx.timestamp,
        expires_at,
        revision: 3,
        deposit_rate: 5,
        consignment_rate: 5,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(expires_at),
        auction_id: BUYOUT_FIXTURE_AUCTION_ID,
    });

    // No buyout term, so any accepted offer is RemainActive — an ordinary raise, never a
    // settlement — and no prior bidder, so the decision carries no displaced bidder either.
    let new_bid_expires_at =
        Timestamp::from_micros_since_unix_epoch(expires_micros.saturating_add(1));
    ctx.db.game_auction().insert(Auction {
        id: BUYOUT_FIXTURE_NEW_BID_AUCTION_ID,
        listing_operation_id: BUYOUT_FIXTURE_NEW_BID_OPERATION_ID - 1,
        house: 1,
        owner_guid: BUYOUT_FIXTURE_NEW_BID_SELLER_GUID,
        item_guid: 509_0059,
        item_entry: BUYOUT_FIXTURE_NEW_BID_AUCTION_ID,
        item_stack_count: 1,
        item_durability: 0,
        item_enchant_id: 0,
        item_soulbound: false,
        random_property_id: 0,
        start_bid: 50,
        buyout: 0,
        highest_bidder_guid: 0,
        highest_bid: 0,
        deposit: 5,
        created_at: ctx.timestamp,
        expires_at: new_bid_expires_at,
        revision: 0,
        deposit_rate: 5,
        consignment_rate: 5,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(new_bid_expires_at),
        auction_id: BUYOUT_FIXTURE_NEW_BID_AUCTION_ID,
    });
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn auction_fixture_mail(
    ctx: &ReducerContext,
    recipient_guid: u64,
    house: u32,
    item_entry: u32,
    random_property_id: u32,
    action: AuctionMailAction,
) -> Result<crate::Mail, String> {
    let subject = auction_mail_subject(item_entry, random_property_id, action);
    let mut matches: Vec<_> = ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&recipient_guid)
        .filter(|mail| mail.subject == subject && mail.sender() == MailSender::AuctionHouse(house))
        .collect();
    if matches.len() != 1 {
        return Err(format!(
            "expected one {subject:?} mail for {recipient_guid}, found {}",
            matches.len()
        ));
    }
    Ok(matches.remove(0))
}

/// The exactly-one notice of `kind` for `recipient_guid`, verifying every wire field the
/// gateway relays.
#[cfg(feature = "debug_reducers")]
#[allow(clippy::too_many_arguments)]
fn auction_fixture_notice(
    ctx: &ReducerContext,
    recipient_guid: u64,
    kind: u8,
    house: u32,
    auction_id: u32,
    item_entry: u32,
    random_property_id: u32,
    bid: u32,
    out_bid: u32,
    bidder_guid: u64,
) -> Result<(), String> {
    let matches: Vec<_> = ctx
        .db
        .game_auction_notice()
        .by_recipient()
        .filter(&recipient_guid)
        .filter(|notice| notice.kind == kind)
        .collect();
    if matches.len() != 1 {
        return Err(format!(
            "expected one kind {kind} notice for {recipient_guid}, found {}",
            matches.len()
        ));
    }
    let notice = &matches[0];
    if notice.house != house
        || notice.auction_id != auction_id
        || notice.item_entry != item_entry
        || notice.random_property_id != random_property_id
        || notice.bid != bid
        || notice.out_bid != out_bid
        || notice.bidder_guid != bidder_guid
    {
        return Err(format!("kind {kind} notice for {recipient_guid} changed"));
    }
    Ok(())
}

/// Verify the real realm reducer committed exact settlement rows in a prior transaction, including
/// every Auction Notice. Auction Notices are a one-shot, TTL-reaped relay (see `gc.rs`); the
/// durable test disarms the reaper schedule before staging this fixture, so nothing claims a
/// notice row while this test runs and this check is safe to call whenever the test wants it,
/// regardless of what ran before it.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_auction_buyout_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if ctx
        .db
        .game_auction()
        .id()
        .find(BUYOUT_FIXTURE_AUCTION_ID)
        .is_some()
        || ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .find(BUYOUT_FIXTURE_AUCTION_ID)
            .is_some()
    {
        return Err("settled Auction or expiry schedule is still active".to_string());
    }

    let decision = ctx
        .db
        .game_auction_bid_decision()
        .operation_id()
        .find(BUYOUT_FIXTURE_OPERATION_ID)
        .ok_or_else(|| "buyout decision was not committed".to_string())?;
    if decision.bidder_guid != BUYOUT_FIXTURE_WINNER_GUID
        || decision.auction_id != BUYOUT_FIXTURE_AUCTION_ID
        || decision.offer != 900
        || decision.outcome != BID_ACCEPTED
        || decision.revision != 0
        || decision.result_bidder_guid != BUYOUT_FIXTURE_DISPLACED_GUID
        || decision.result_bid != 201
        || decision.accepted_price != 500
    {
        return Err("buyout decision payload changed".to_string());
    }

    const HOUSE: u32 = 1;
    let item = crate::items::ItemSnapshot {
        entry: 509_0050,
        stack_count: 2,
        durability: 17,
        enchant_id: 9,
        soulbound: false,
        random_property_id: 117,
    };

    let refund = auction_fixture_mail(
        ctx,
        BUYOUT_FIXTURE_DISPLACED_GUID,
        HOUSE,
        item.entry,
        item.random_property_id,
        AuctionMailAction::Outbid,
    )?;
    if refund.money != 201
        || refund.cod != 0
        || refund.was_read
        || refund.check_flags != CHECK_MASK_COPIED
        || !refund.body.is_empty()
        || !refund.snapshot().is_empty()
    {
        return Err("displaced-bidder refund mail changed".to_string());
    }

    let winner = auction_fixture_mail(
        ctx,
        BUYOUT_FIXTURE_WINNER_GUID,
        HOUSE,
        item.entry,
        item.random_property_id,
        AuctionMailAction::Won,
    )?;
    if winner.money != 0
        || winner.cod != 0
        || winner.was_read
        || winner.check_flags != CHECK_MASK_COPIED
        || winner.body != format!("{:>16x}:{}:{}", BUYOUT_FIXTURE_SELLER_GUID, 500, 500)
        || winner.snapshot() != item
    {
        return Err("winner item mail changed".to_string());
    }

    let seller = auction_fixture_mail(
        ctx,
        BUYOUT_FIXTURE_SELLER_GUID,
        HOUSE,
        item.entry,
        item.random_property_id,
        AuctionMailAction::Successful,
    )?;
    if seller.money != 485
        || seller.cod != 0
        || seller.was_read
        || seller.check_flags != CHECK_MASK_COPIED
        || seller.body
            != format!(
                "{:>16x}:{}:{}:{}:{}",
                BUYOUT_FIXTURE_WINNER_GUID, 500, 500, 10, 25
            )
        || !seller.snapshot().is_empty()
    {
        return Err("seller proceeds mail changed".to_string());
    }

    auction_fixture_notice(
        ctx,
        BUYOUT_FIXTURE_DISPLACED_GUID,
        auction_notice::OUTBID,
        HOUSE,
        BUYOUT_FIXTURE_AUCTION_ID,
        item.entry,
        item.random_property_id,
        201,
        11, // vanilla minimum raise on a 201 bid: 5% rounded up
        BUYOUT_FIXTURE_DISPLACED_GUID,
    )?;
    auction_fixture_notice(
        ctx,
        BUYOUT_FIXTURE_WINNER_GUID,
        auction_notice::WON,
        HOUSE,
        BUYOUT_FIXTURE_AUCTION_ID,
        item.entry,
        item.random_property_id,
        0,
        25, // vanilla minimum raise on a 500 bid: 5% rounded up
        BUYOUT_FIXTURE_WINNER_GUID,
    )?;
    auction_fixture_notice(
        ctx,
        BUYOUT_FIXTURE_SELLER_GUID,
        auction_notice::SOLD,
        HOUSE,
        BUYOUT_FIXTURE_AUCTION_ID,
        item.entry,
        item.random_property_id,
        500,
        25, // vanilla minimum raise on a 500 bid: 5% rounded up
        0,
    )?;

    // RemainActive coverage: an ordinary raise on a fresh listing settles nothing and displaces
    // nobody, so it must record the new bid, mail nobody, and write exactly one New Bid notice to
    // the owner.
    let new_bid_auction = ctx
        .db
        .game_auction()
        .id()
        .find(BUYOUT_FIXTURE_NEW_BID_AUCTION_ID)
        .ok_or_else(|| "the RemainActive Auction was settled or expired".to_string())?;
    if new_bid_auction.highest_bidder_guid != BUYOUT_FIXTURE_NEW_BID_BIDDER_GUID
        || new_bid_auction.highest_bid != 60
        || new_bid_auction.revision != 1
    {
        return Err("the RemainActive Auction did not record the new bid".to_string());
    }
    if ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&BUYOUT_FIXTURE_NEW_BID_BIDDER_GUID)
        .any(|mail| mail.sender_kind == lyracore_shared::mail::SENDER_KIND_AUCTION)
    {
        return Err(
            "a fresh bid with no displaced bidder must not mail its own bidder".to_string(),
        );
    }
    auction_fixture_notice(
        ctx,
        BUYOUT_FIXTURE_NEW_BID_SELLER_GUID,
        auction_notice::NEW_BID,
        HOUSE,
        BUYOUT_FIXTURE_NEW_BID_AUCTION_ID,
        BUYOUT_FIXTURE_NEW_BID_AUCTION_ID,
        0,
        60,
        3, // vanilla minimum raise on a 60 bid: 5% rounded up
        BUYOUT_FIXTURE_NEW_BID_BIDDER_GUID,
    )?;
    Ok(())
}

/// The listing with a bid, the unbid listing, and the listing whose cut the test offers wrong.
#[cfg(feature = "debug_reducers")]
const CANCEL_FIXTURE_AUCTION_IDS: [u32; 3] = [509_0086, 509_0088, 509_0089];
#[cfg(feature = "debug_reducers")]
const CANCEL_FIXTURE_BIDDER_GUID: u64 = 509_0087;
/// Every operation id the Cancellation test may use, cleared before each staging.
#[cfg(feature = "debug_reducers")]
const CANCEL_FIXTURE_OPERATION_IDS: std::ops::RangeInclusive<u64> = 509_0086..=509_0095;
/// Blackwater's faction template (`cm:AuctionHouseMgr.cpp:461-518`), so the auctioneer serves the
/// neutral house 7.
#[cfg(feature = "debug_reducers")]
const CANCEL_FIXTURE_FACTION_TEMPLATE: u32 = 120;

/// Stage three listings of `seller_guid` in house 7 at a consignment rate of 15, each with its
/// expiry row: 509_0086 carries a bid of 1000 by 509_0087 (cut 150), 509_0088 has no bid (cut 0),
/// and 509_0089 carries the same bid. A nonzero `auctioneer_guid` becomes an auctioneer of house 7,
/// and the fixture imports house 7 and its faction template when the database has none.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_stage_auction_cancel_fixture(
    ctx: &ReducerContext,
    seller_guid: u64,
    auctioneer_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    const HOUSE: u32 = 7;

    for auction_id in CANCEL_FIXTURE_AUCTION_IDS {
        ctx.db.game_auction_expiry().auction_id().delete(auction_id);
        ctx.db.game_auction().id().delete(auction_id);
    }
    for operation_id in CANCEL_FIXTURE_OPERATION_IDS {
        ctx.db
            .game_auction_bid_hold()
            .operation_id()
            .delete(operation_id);
        ctx.db
            .game_auction_bid_decision()
            .operation_id()
            .delete(operation_id);
    }
    let notices = ctx.db.game_auction_notice();
    for recipient_guid in [seller_guid, CANCEL_FIXTURE_BIDDER_GUID] {
        let stale_mail: Vec<u64> = ctx
            .db
            .game_mail()
            .by_recipient()
            .filter(&recipient_guid)
            .filter(|mail| mail.sender_kind == lyracore_shared::mail::SENDER_KIND_AUCTION)
            .map(|mail| mail.id)
            .collect();
        for id in stale_mail {
            crate::mail::delete_mail(ctx, id);
        }
        let stale_notices: Vec<u64> = notices
            .by_recipient()
            .filter(&recipient_guid)
            .map(|notice| notice.id)
            .collect();
        for id in stale_notices {
            notices.id().delete(id);
        }
    }

    if ctx.db.game_auction_house().id().find(HOUSE).is_none() {
        ctx.db.game_auction_house().insert(AuctionHouseDefinition {
            id: HOUSE,
            faction: 0,
            deposit_rate: 25,
            consignment_rate: 15,
            name: "Blackwater Auction House".to_string(),
        });
    }
    if auctioneer_guid != 0 {
        if ctx
            .db
            .game_faction_template()
            .id()
            .find(CANCEL_FIXTURE_FACTION_TEMPLATE)
            .is_none()
        {
            ctx.db
                .game_faction_template()
                .insert(crate::faction::FactionTemplate {
                    id: CANCEL_FIXTURE_FACTION_TEMPLATE,
                    faction: 0,
                    faction_group: 0,
                    friend_group: 0,
                    enemy_group: 0,
                    enemy_0: 0,
                    enemy_1: 0,
                    enemy_2: 0,
                    enemy_3: 0,
                    friend_0: 0,
                    friend_1: 0,
                    friend_2: 0,
                    friend_3: 0,
                });
        }
        let mut auctioneer = ctx
            .db
            .game_world_entity()
            .guid()
            .find(auctioneer_guid)
            .ok_or_else(|| format!("auctioneer {auctioneer_guid} is not in the world"))?;
        auctioneer.faction_template = CANCEL_FIXTURE_FACTION_TEMPLATE;
        auctioneer.npc_flags |= lyracore_shared::constants::npc_flags::AUCTIONEER;
        ctx.db.game_world_entity().guid().update(auctioneer);
    }

    let expires_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .checked_add(3_600_000_000)
        .ok_or_else(|| "auction Cancellation fixture expiry overflow".to_string())?;
    let expires_at = Timestamp::from_micros_since_unix_epoch(expires_micros);
    for (auction_id, highest_bidder_guid, highest_bid) in [
        (
            CANCEL_FIXTURE_AUCTION_IDS[0],
            CANCEL_FIXTURE_BIDDER_GUID,
            1_000,
        ),
        (CANCEL_FIXTURE_AUCTION_IDS[1], 0, 0),
        (
            CANCEL_FIXTURE_AUCTION_IDS[2],
            CANCEL_FIXTURE_BIDDER_GUID,
            1_000,
        ),
    ] {
        ctx.db.game_auction().insert(Auction {
            id: auction_id,
            listing_operation_id: u64::from(auction_id),
            house: HOUSE,
            owner_guid: seller_guid,
            item_guid: u64::from(auction_id),
            item_entry: auction_id,
            item_stack_count: 3,
            item_durability: 17,
            item_enchant_id: 9,
            item_soulbound: false,
            random_property_id: 117,
            start_bid: 500,
            buyout: 0,
            highest_bidder_guid,
            highest_bid,
            deposit: 40,
            created_at: ctx.timestamp,
            expires_at,
            revision: u64::from(highest_bid != 0),
            deposit_rate: 25,
            consignment_rate: 15,
        });
        ctx.db.game_auction_expiry().insert(AuctionExpiry {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Time(expires_at),
            auction_id,
        });
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_AUCTION_ID: u32 = 509_0060;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_OPERATION_ID: u64 = 509_0060;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_SELLER_GUID: u64 = 509_0060;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_WINNER_GUID: u64 = 509_0061;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_ITEM: crate::items::ItemSnapshot = crate::items::ItemSnapshot {
    entry: 509_0060,
    stack_count: 2,
    durability: 17,
    enchant_id: 9,
    soulbound: false,
    random_property_id: 117,
};
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_UNSOLD_AUCTION_ID: u32 = 509_0065;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_UNSOLD_OPERATION_ID: u64 = 509_0065;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_UNSOLD_SELLER_GUID: u64 = 509_0065;
#[cfg(feature = "debug_reducers")]
const EXPIRY_FIXTURE_UNSOLD_ITEM: crate::items::ItemSnapshot = crate::items::ItemSnapshot {
    entry: 509_0066,
    stack_count: 1,
    durability: 0,
    enchant_id: 0,
    soulbound: false,
    random_property_id: 0,
};

/// Stage a valid winning-bid Auction whose one-shot schedule fires shortly after this
/// transaction, plus a second, unbid Auction (`EXPIRY_FIXTURE_UNSOLD_*`) whose expiry has no
/// winner, to cover the Expired notice.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_stage_auction_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;

    ctx.db
        .game_auction_expiry()
        .auction_id()
        .delete(EXPIRY_FIXTURE_AUCTION_ID);
    ctx.db.game_auction().id().delete(EXPIRY_FIXTURE_AUCTION_ID);
    ctx.db
        .game_auction_operation_receipt()
        .operation_id()
        .delete(EXPIRY_FIXTURE_OPERATION_ID);
    ctx.db
        .game_auction_expiry()
        .auction_id()
        .delete(EXPIRY_FIXTURE_UNSOLD_AUCTION_ID);
    ctx.db
        .game_auction()
        .id()
        .delete(EXPIRY_FIXTURE_UNSOLD_AUCTION_ID);
    ctx.db
        .game_auction_operation_receipt()
        .operation_id()
        .delete(EXPIRY_FIXTURE_UNSOLD_OPERATION_ID);

    let mails = ctx.db.game_mail();
    let notices = ctx.db.game_auction_notice();
    for recipient_guid in [
        EXPIRY_FIXTURE_SELLER_GUID,
        EXPIRY_FIXTURE_WINNER_GUID,
        EXPIRY_FIXTURE_UNSOLD_SELLER_GUID,
    ] {
        let stale_mail: Vec<u64> = mails
            .by_recipient()
            .filter(&recipient_guid)
            .filter(|mail| mail.sender_kind == lyracore_shared::mail::SENDER_KIND_AUCTION)
            .map(|mail| mail.id)
            .collect();
        for id in stale_mail {
            crate::mail::delete_mail(ctx, id);
        }
        let stale_notices: Vec<u64> = notices
            .by_recipient()
            .filter(&recipient_guid)
            .map(|notice| notice.id)
            .collect();
        for id in stale_notices {
            notices.id().delete(id);
        }
    }

    let expires_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .checked_add(500_000)
        .ok_or_else(|| "auction expiry fixture deadline overflow".to_string())?;
    let created_micros = expires_micros
        .checked_sub(43_200_000_000)
        .ok_or_else(|| "auction expiry fixture creation time underflow".to_string())?;
    let expires_at = Timestamp::from_micros_since_unix_epoch(expires_micros);
    let created_at = Timestamp::from_micros_since_unix_epoch(created_micros);
    ctx.db
        .game_auction_operation_receipt()
        .insert(AuctionOperationReceipt {
            operation_id: EXPIRY_FIXTURE_OPERATION_ID,
            auction_id: EXPIRY_FIXTURE_AUCTION_ID,
            actor_guid: EXPIRY_FIXTURE_SELLER_GUID,
            item_guid: 509_0063,
            item_entry: EXPIRY_FIXTURE_ITEM.entry,
            item_stack_count: EXPIRY_FIXTURE_ITEM.stack_count,
            item_durability: EXPIRY_FIXTURE_ITEM.durability,
            item_enchant_id: EXPIRY_FIXTURE_ITEM.enchant_id,
            item_soulbound: EXPIRY_FIXTURE_ITEM.soulbound,
            random_property_id: EXPIRY_FIXTURE_ITEM.random_property_id,
            start_bid: 100,
            buyout: 500,
            duration_minutes: 720,
            deposit: 10,
            created_micros,
            expires_micros,
            house: 1,
            deposit_rate: 5,
            consignment_rate: 5,
        });
    ctx.db.game_auction().insert(Auction {
        id: EXPIRY_FIXTURE_AUCTION_ID,
        listing_operation_id: EXPIRY_FIXTURE_OPERATION_ID,
        house: 1,
        owner_guid: EXPIRY_FIXTURE_SELLER_GUID,
        item_guid: 509_0063,
        item_entry: EXPIRY_FIXTURE_ITEM.entry,
        item_stack_count: EXPIRY_FIXTURE_ITEM.stack_count,
        item_durability: EXPIRY_FIXTURE_ITEM.durability,
        item_enchant_id: EXPIRY_FIXTURE_ITEM.enchant_id,
        item_soulbound: EXPIRY_FIXTURE_ITEM.soulbound,
        random_property_id: EXPIRY_FIXTURE_ITEM.random_property_id,
        start_bid: 100,
        buyout: 500,
        highest_bidder_guid: EXPIRY_FIXTURE_WINNER_GUID,
        highest_bid: 201,
        deposit: 10,
        created_at,
        expires_at,
        revision: 3,
        deposit_rate: 5,
        consignment_rate: 5,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(expires_at),
        auction_id: EXPIRY_FIXTURE_AUCTION_ID,
    });

    // Unsold: no bidder ever placed an offer, so expiry returns the item and mails the owner an
    // Expired notice rather than settling a sale.
    let unsold_expires_at = Timestamp::from_micros_since_unix_epoch(expires_micros + 1);
    ctx.db
        .game_auction_operation_receipt()
        .insert(AuctionOperationReceipt {
            operation_id: EXPIRY_FIXTURE_UNSOLD_OPERATION_ID,
            auction_id: EXPIRY_FIXTURE_UNSOLD_AUCTION_ID,
            actor_guid: EXPIRY_FIXTURE_UNSOLD_SELLER_GUID,
            item_guid: 509_0068,
            item_entry: EXPIRY_FIXTURE_UNSOLD_ITEM.entry,
            item_stack_count: EXPIRY_FIXTURE_UNSOLD_ITEM.stack_count,
            item_durability: EXPIRY_FIXTURE_UNSOLD_ITEM.durability,
            item_enchant_id: EXPIRY_FIXTURE_UNSOLD_ITEM.enchant_id,
            item_soulbound: EXPIRY_FIXTURE_UNSOLD_ITEM.soulbound,
            random_property_id: EXPIRY_FIXTURE_UNSOLD_ITEM.random_property_id,
            start_bid: 100,
            buyout: 500,
            duration_minutes: 720,
            deposit: 10,
            created_micros,
            expires_micros: expires_micros + 1,
            house: 1,
            deposit_rate: 5,
            consignment_rate: 5,
        });
    ctx.db.game_auction().insert(Auction {
        id: EXPIRY_FIXTURE_UNSOLD_AUCTION_ID,
        listing_operation_id: EXPIRY_FIXTURE_UNSOLD_OPERATION_ID,
        house: 1,
        owner_guid: EXPIRY_FIXTURE_UNSOLD_SELLER_GUID,
        item_guid: 509_0068,
        item_entry: EXPIRY_FIXTURE_UNSOLD_ITEM.entry,
        item_stack_count: EXPIRY_FIXTURE_UNSOLD_ITEM.stack_count,
        item_durability: EXPIRY_FIXTURE_UNSOLD_ITEM.durability,
        item_enchant_id: EXPIRY_FIXTURE_UNSOLD_ITEM.enchant_id,
        item_soulbound: EXPIRY_FIXTURE_UNSOLD_ITEM.soulbound,
        random_property_id: EXPIRY_FIXTURE_UNSOLD_ITEM.random_property_id,
        start_bid: 100,
        buyout: 500,
        highest_bidder_guid: 0,
        highest_bid: 0,
        deposit: 10,
        created_at,
        expires_at: unsold_expires_at,
        revision: 0,
        deposit_rate: 5,
        consignment_rate: 5,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(unsold_expires_at),
        auction_id: EXPIRY_FIXTURE_UNSOLD_AUCTION_ID,
    });
    Ok(())
}

/// Re-drive the callback body after settlement to prove the missing Auction is a durable no-op.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_replay_auction_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    expire_active(&mut CtxExpiry { ctx }, EXPIRY_FIXTURE_AUCTION_ID)?;
    expire_active(&mut CtxExpiry { ctx }, EXPIRY_FIXTURE_UNSOLD_AUCTION_ID)
}

/// Verify the scheduler committed the exact bid-expiry mail, removed only active state, and fired
/// every Auction Notice. Auction Notices are a one-shot, TTL-reaped relay (see `gc.rs`); the
/// durable test disarms the reaper schedule before staging this fixture, so nothing claims a
/// notice row while this test runs and this check is safe to call whenever the test wants it,
/// regardless of what ran before it.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_auction_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if ctx
        .db
        .game_auction()
        .id()
        .find(EXPIRY_FIXTURE_AUCTION_ID)
        .is_some()
        || ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .find(EXPIRY_FIXTURE_AUCTION_ID)
            .is_some()
        || ctx
            .db
            .game_auction()
            .id()
            .find(EXPIRY_FIXTURE_UNSOLD_AUCTION_ID)
            .is_some()
        || ctx
            .db
            .game_auction_expiry()
            .auction_id()
            .find(EXPIRY_FIXTURE_UNSOLD_AUCTION_ID)
            .is_some()
    {
        return Err("expired Auction or schedule is still active".to_string());
    }
    if ctx
        .db
        .game_auction_operation_receipt()
        .operation_id()
        .find(EXPIRY_FIXTURE_OPERATION_ID)
        .is_none()
        || ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(EXPIRY_FIXTURE_UNSOLD_OPERATION_ID)
            .is_none()
    {
        return Err("expiry removed the durable listing receipt".to_string());
    }

    const HOUSE: u32 = 1;
    let winner = auction_fixture_mail(
        ctx,
        EXPIRY_FIXTURE_WINNER_GUID,
        HOUSE,
        EXPIRY_FIXTURE_ITEM.entry,
        EXPIRY_FIXTURE_ITEM.random_property_id,
        AuctionMailAction::Won,
    )?;
    if winner.money != 0
        || winner.cod != 0
        || winner.was_read
        || winner.check_flags != CHECK_MASK_COPIED
        || winner.body != format!("{:>16x}:{}:{}", EXPIRY_FIXTURE_SELLER_GUID, 201, 500)
        || winner.snapshot() != EXPIRY_FIXTURE_ITEM
    {
        return Err("expiry winner item mail changed".to_string());
    }

    let seller = auction_fixture_mail(
        ctx,
        EXPIRY_FIXTURE_SELLER_GUID,
        HOUSE,
        EXPIRY_FIXTURE_ITEM.entry,
        EXPIRY_FIXTURE_ITEM.random_property_id,
        AuctionMailAction::Successful,
    )?;
    if seller.money != 201
        || seller.cod != 0
        || seller.was_read
        || seller.check_flags != CHECK_MASK_COPIED
        || seller.body
            != format!(
                "{:>16x}:{}:{}:{}:{}",
                EXPIRY_FIXTURE_WINNER_GUID, 201, 500, 10, 10
            )
        || !seller.snapshot().is_empty()
    {
        return Err("expiry seller proceeds mail changed".to_string());
    }

    // Unsold coverage: no bidder means expiry returns the item, not a Won/Sold pair.
    let returned = auction_fixture_mail(
        ctx,
        EXPIRY_FIXTURE_UNSOLD_SELLER_GUID,
        HOUSE,
        EXPIRY_FIXTURE_UNSOLD_ITEM.entry,
        EXPIRY_FIXTURE_UNSOLD_ITEM.random_property_id,
        AuctionMailAction::Expired,
    )?;
    if returned.money != 0
        || returned.cod != 0
        || returned.was_read
        || returned.check_flags != CHECK_MASK_COPIED
        || !returned.body.is_empty()
        || returned.snapshot() != EXPIRY_FIXTURE_UNSOLD_ITEM
    {
        return Err("unsold expiry return mail changed".to_string());
    }

    auction_fixture_notice(
        ctx,
        EXPIRY_FIXTURE_WINNER_GUID,
        auction_notice::WON,
        HOUSE,
        EXPIRY_FIXTURE_AUCTION_ID,
        EXPIRY_FIXTURE_ITEM.entry,
        EXPIRY_FIXTURE_ITEM.random_property_id,
        0,
        11, // vanilla minimum raise on a 201 bid: 5% rounded up
        EXPIRY_FIXTURE_WINNER_GUID,
    )?;
    auction_fixture_notice(
        ctx,
        EXPIRY_FIXTURE_SELLER_GUID,
        auction_notice::SOLD,
        HOUSE,
        EXPIRY_FIXTURE_AUCTION_ID,
        EXPIRY_FIXTURE_ITEM.entry,
        EXPIRY_FIXTURE_ITEM.random_property_id,
        201,
        11, // vanilla minimum raise on a 201 bid: 5% rounded up
        0,
    )?;
    auction_fixture_notice(
        ctx,
        EXPIRY_FIXTURE_UNSOLD_SELLER_GUID,
        auction_notice::EXPIRED,
        HOUSE,
        EXPIRY_FIXTURE_UNSOLD_AUCTION_ID,
        EXPIRY_FIXTURE_UNSOLD_ITEM.entry,
        EXPIRY_FIXTURE_UNSOLD_ITEM.random_property_id,
        0,
        1, // bid_increment(0) is 0, but expired_notice floors an unsold listing's raise to 1.
        0,
    )?;
    Ok(())
}

#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_FIXTURE_RECIPIENT_GUID: u64 = 509_0070;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_FIXTURE_ITEM_ENTRY: u32 = 509_0071;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_LOOKALIKE_RECIPIENT_GUID: u64 = 509_0072;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_LOOKALIKE_SENDER_GUID: u64 = 509_0073;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_LOOKALIKE_ITEM_ENTRY: u32 = 509_0074;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_LOOKALIKE_COD: u32 = 250;
// A real letter titled "Auction won" can carry cod == 0 too (nothing stops a player from gifting
// an item for free); the guid-0 look-alike above already covers the COD case, so this covers the
// other one. It has no receipt naming its sender as this item's seller, so it must stay untouched
// even though it clears `legacy_mail_matches_shape`.
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_NO_COD_LOOKALIKE_RECIPIENT_GUID: u64 = 509_0076;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_NO_COD_LOOKALIKE_SENDER_GUID: u64 = 509_0077;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_NO_COD_LOOKALIKE_ITEM_ENTRY: u32 = 509_0078;
// The receipt that authorizes the legitimate "Auction won" row above. Its house (6, Horde) is
// deliberately not the neutral fallback (7): Realm-core carries no Character rows, so a converted
// row's house only comes from here if `legacy_repair_authorization` truly reads the receipt
// instead of silently falling back.
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_WON_RECEIPT_OPERATION_ID: u64 = 509_0075;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_WON_RECEIPT_AUCTION_ID: u32 = 509_0075;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_WON_RECEIPT_HOUSE: u32 = 6;
// A legitimate "Auction sold" row: proceeds mail with no item attached, so its vanilla subject can
// only name the sold item by borrowing the authorizing receipt's own entry and random property id.
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID: u64 = 509_0079;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_SOLD_RECEIPT_OPERATION_ID: u64 = 509_0080;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_SOLD_RECEIPT_AUCTION_ID: u32 = 509_0080;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_SOLD_RECEIPT_ITEM_ENTRY: u32 = 509_0081;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_SOLD_RECEIPT_HOUSE: u32 = 1;
// The same seller has two other receipts on file: a refused listing (the `auction_id == 0`
// sentinel — excluded outright, whatever its proceeds) and a second real listing whose price range
// could never have paid out the Sold mail's money (excluded by `receipt_could_pay_out`). Both use a
// house the repair must never select, so picking either fails the test loudly instead of quietly.
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_REFUSED_RECEIPT_OPERATION_ID: u64 = 509_0082;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_REFUSED_RECEIPT_ITEM_ENTRY: u32 = 509_0083;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_DECOY_RECEIPT_OPERATION_ID: u64 = 509_0084;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_DECOY_RECEIPT_AUCTION_ID: u32 = 509_0084;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_DECOY_RECEIPT_ITEM_ENTRY: u32 = 509_0085;
#[cfg(feature = "debug_reducers")]
const LEGACY_MAIL_WRONG_RECEIPT_HOUSE: u32 = 7;

/// Stage `Character`-sender rows covering every outcome `repair_legacy_auction_mail` must tell
/// apart: a legitimate "Auction won" row backed by a listing receipt, a legitimate "Auction sold"
/// row backed by one of three receipts on the same seller (a refused listing, a real listing that
/// could not have paid this exact price, and the real listing that did), and two look-alikes a real
/// player could send today — one with a cash-on-delivery price (the shape check alone rules this
/// out), one without (only the missing receipt rules this out). Proves `repair_legacy_auction_mail`
/// against real rows on a real database: it must convert the two legitimate rows, picking the one
/// true receipt for Sold out of three candidates, and leave both look-alikes alone.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_stage_legacy_auction_mail_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mails = ctx.db.game_mail();
    let stale: Vec<u64> = mails
        .by_recipient()
        .filter(&LEGACY_MAIL_FIXTURE_RECIPIENT_GUID)
        .chain(
            mails
                .by_recipient()
                .filter(&LEGACY_MAIL_LOOKALIKE_RECIPIENT_GUID),
        )
        .chain(
            mails
                .by_recipient()
                .filter(&LEGACY_MAIL_NO_COD_LOOKALIKE_RECIPIENT_GUID),
        )
        .chain(
            mails
                .by_recipient()
                .filter(&LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID),
        )
        .map(|mail| mail.id)
        .collect();
    for id in stale {
        crate::mail::delete_mail(ctx, id);
    }
    let receipts = ctx.db.game_auction_operation_receipt();
    receipts
        .operation_id()
        .delete(LEGACY_MAIL_WON_RECEIPT_OPERATION_ID);
    receipts
        .operation_id()
        .delete(LEGACY_MAIL_SOLD_RECEIPT_OPERATION_ID);
    receipts
        .operation_id()
        .delete(LEGACY_MAIL_REFUSED_RECEIPT_OPERATION_ID);
    receipts
        .operation_id()
        .delete(LEGACY_MAIL_DECOY_RECEIPT_OPERATION_ID);

    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let receipt_template = AuctionOperationReceipt {
        operation_id: 0,
        auction_id: 0,
        actor_guid: 0,
        item_guid: 0,
        item_entry: 0,
        item_stack_count: 1,
        item_durability: 0,
        item_enchant_id: 0,
        item_soulbound: false,
        start_bid: 100,
        buyout: 500,
        duration_minutes: 720,
        deposit: 10,
        created_micros: now,
        expires_micros: now + 1,
        house: 0,
        deposit_rate: 5,
        consignment_rate: 5,
        random_property_id: 0,
    };
    receipts.insert(AuctionOperationReceipt {
        operation_id: LEGACY_MAIL_WON_RECEIPT_OPERATION_ID,
        auction_id: LEGACY_MAIL_WON_RECEIPT_AUCTION_ID,
        actor_guid: LEGACY_MAIL_FIXTURE_RECIPIENT_GUID + 1,
        item_entry: LEGACY_MAIL_FIXTURE_ITEM_ENTRY,
        house: LEGACY_MAIL_WON_RECEIPT_HOUSE,
        ..receipt_template
    });
    receipts.insert(AuctionOperationReceipt {
        operation_id: LEGACY_MAIL_SOLD_RECEIPT_OPERATION_ID,
        auction_id: LEGACY_MAIL_SOLD_RECEIPT_AUCTION_ID,
        actor_guid: LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID,
        item_entry: LEGACY_MAIL_SOLD_RECEIPT_ITEM_ENTRY,
        random_property_id: 117,
        house: LEGACY_MAIL_SOLD_RECEIPT_HOUSE,
        ..receipt_template
    });
    // A refused listing: the `auction_id == 0` sentinel, excluded outright regardless of price.
    receipts.insert(AuctionOperationReceipt {
        operation_id: LEGACY_MAIL_REFUSED_RECEIPT_OPERATION_ID,
        auction_id: 0,
        actor_guid: LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID,
        item_entry: LEGACY_MAIL_REFUSED_RECEIPT_ITEM_ENTRY,
        house: LEGACY_MAIL_WRONG_RECEIPT_HOUSE,
        ..receipt_template
    });
    // A second real listing: seller_proceeds(50, 10, 5) == 58 through seller_proceeds(200, 10, 5)
    // == 200, a range that never reaches the Sold mail's money (485).
    receipts.insert(AuctionOperationReceipt {
        operation_id: LEGACY_MAIL_DECOY_RECEIPT_OPERATION_ID,
        auction_id: LEGACY_MAIL_DECOY_RECEIPT_AUCTION_ID,
        actor_guid: LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID,
        item_entry: LEGACY_MAIL_DECOY_RECEIPT_ITEM_ENTRY,
        start_bid: 50,
        buyout: 200,
        house: LEGACY_MAIL_WRONG_RECEIPT_HOUSE,
        ..receipt_template
    });

    // Every row enters `game_mail` through `mail::insert_letter` (the sole writer; the
    // `every_mail_row_is_created_by_insert_letter` tripwire enforces it), so every fixture goes
    // through `Letter::from_character` exactly as the sending path that produced each shape did.
    crate::mail::insert_letter(
        ctx,
        crate::mail::Letter::from_character(
            LEGACY_MAIL_FIXTURE_RECIPIENT_GUID + 1,
            LEGACY_MAIL_FIXTURE_RECIPIENT_GUID,
            "Auction won".to_string(),
            String::new(),
            0,
            0,
            crate::items::ItemSnapshot {
                entry: LEGACY_MAIL_FIXTURE_ITEM_ENTRY,
                stack_count: 1,
                durability: 0,
                enchant_id: 0,
                soulbound: false,
                random_property_id: 0,
            },
        ),
    );
    crate::mail::insert_letter(
        ctx,
        crate::mail::Letter::from_character(
            LEGACY_MAIL_LOOKALIKE_SENDER_GUID,
            LEGACY_MAIL_LOOKALIKE_RECIPIENT_GUID,
            "Auction won".to_string(),
            String::new(),
            0,
            LEGACY_MAIL_LOOKALIKE_COD,
            crate::items::ItemSnapshot {
                entry: LEGACY_MAIL_LOOKALIKE_ITEM_ENTRY,
                stack_count: 1,
                durability: 0,
                enchant_id: 0,
                soulbound: false,
                random_property_id: 0,
            },
        ),
    );
    crate::mail::insert_letter(
        ctx,
        crate::mail::Letter::from_character(
            LEGACY_MAIL_NO_COD_LOOKALIKE_SENDER_GUID,
            LEGACY_MAIL_NO_COD_LOOKALIKE_RECIPIENT_GUID,
            "Auction won".to_string(),
            String::new(),
            0,
            0,
            crate::items::ItemSnapshot {
                entry: LEGACY_MAIL_NO_COD_LOOKALIKE_ITEM_ENTRY,
                stack_count: 1,
                durability: 0,
                enchant_id: 0,
                soulbound: false,
                random_property_id: 0,
            },
        ),
    );
    crate::mail::insert_letter(
        ctx,
        crate::mail::Letter::from_character(
            0,
            LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID,
            "Auction sold".to_string(),
            String::new(),
            485,
            0,
            crate::items::ItemSnapshot::default(),
        ),
    );
    Ok(())
}

/// Verify `debug_repair_after_publish` re-tagged both legitimate legacy rows to the vanilla
/// `AuctionHouse` sender, each receipt's own house, a machine subject built from the item that
/// receipt names, and COPIED — and left both look-alikes exactly as sent: still `Character` mail,
/// one still carrying its cash on delivery price, the other still from its uninvolved real sender.
/// Run twice in a row in the durable test to prove the repair is idempotent.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_verify_legacy_auction_mail_repaired(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut matches: Vec<crate::Mail> = ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&LEGACY_MAIL_FIXTURE_RECIPIENT_GUID)
        .collect();
    if matches.len() != 1 {
        return Err(format!(
            "expected exactly one legacy auction mail fixture row, found {}",
            matches.len()
        ));
    }
    let mail = matches.remove(0);
    let expected_subject =
        auction_mail_subject(LEGACY_MAIL_FIXTURE_ITEM_ENTRY, 0, AuctionMailAction::Won);
    if mail.sender() != MailSender::AuctionHouse(LEGACY_MAIL_WON_RECEIPT_HOUSE)
        || mail.subject != expected_subject
        || mail.check_flags != CHECK_MASK_COPIED
    {
        return Err(format!(
            "legacy Won mail was not repaired: sender={:?}, subject={:?}, check_flags={}",
            mail.sender(),
            mail.subject,
            mail.check_flags
        ));
    }

    let mut sold_matches: Vec<crate::Mail> = ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID)
        .collect();
    if sold_matches.len() != 1 {
        return Err(format!(
            "expected exactly one legacy auction mail fixture row, found {}",
            sold_matches.len()
        ));
    }
    let sold = sold_matches.remove(0);
    let expected_sold_subject = auction_mail_subject(
        LEGACY_MAIL_SOLD_RECEIPT_ITEM_ENTRY,
        117,
        AuctionMailAction::Successful,
    );
    if sold.sender() != MailSender::AuctionHouse(LEGACY_MAIL_SOLD_RECEIPT_HOUSE)
        || sold.subject != expected_sold_subject
        || sold.check_flags != CHECK_MASK_COPIED
    {
        return Err(format!(
            "legacy Sold mail was not repaired: sender={:?}, subject={:?}, check_flags={}",
            sold.sender(),
            sold.subject,
            sold.check_flags
        ));
    }

    let mut lookalike_matches: Vec<crate::Mail> = ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&LEGACY_MAIL_LOOKALIKE_RECIPIENT_GUID)
        .collect();
    if lookalike_matches.len() != 1 {
        return Err(format!(
            "expected exactly one look-alike player mail row, found {}",
            lookalike_matches.len()
        ));
    }
    let lookalike = lookalike_matches.remove(0);
    if lookalike.sender() != MailSender::Character(LEGACY_MAIL_LOOKALIKE_SENDER_GUID)
        || lookalike.subject != "Auction won"
        || lookalike.cod != LEGACY_MAIL_LOOKALIKE_COD
    {
        return Err(format!(
            "the repair touched a real player's look-alike mail: sender={:?}, subject={:?}, cod={}",
            lookalike.sender(),
            lookalike.subject,
            lookalike.cod
        ));
    }

    let mut no_cod_lookalike_matches: Vec<crate::Mail> = ctx
        .db
        .game_mail()
        .by_recipient()
        .filter(&LEGACY_MAIL_NO_COD_LOOKALIKE_RECIPIENT_GUID)
        .collect();
    if no_cod_lookalike_matches.len() != 1 {
        return Err(format!(
            "expected exactly one no-COD look-alike player mail row, found {}",
            no_cod_lookalike_matches.len()
        ));
    }
    let no_cod_lookalike = no_cod_lookalike_matches.remove(0);
    if no_cod_lookalike.sender() != MailSender::Character(LEGACY_MAIL_NO_COD_LOOKALIKE_SENDER_GUID)
        || no_cod_lookalike.subject != "Auction won"
        || no_cod_lookalike.cod != 0
    {
        return Err(format!(
            "the repair touched a real player's no-COD look-alike mail: sender={:?}, subject={:?}",
            no_cod_lookalike.sender(),
            no_cod_lookalike.subject,
        ));
    }
    Ok(())
}

/// Character deletion must not destroy value held by or listed for that character.
pub(crate) fn character_has_auction_value(ctx: &ReducerContext, character_guid: u64) -> bool {
    ctx.db
        .game_auction_bid_hold()
        .by_bidder()
        .filter(character_guid)
        .any(|hold| hold.outcome == BID_PENDING || hold.deferred_refund != 0)
        || ctx
            .db
            .game_auction_hold()
            .by_seller()
            .filter(character_guid)
            .next()
            .is_some()
        || ctx
            .db
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

/// The exact English subjects LyraCore's earlier auction code sent, each a `Character` mail with
/// no vanilla twin. A real player's letter can carry the same words, so the subject alone never
/// authorizes a re-tag; [`legacy_mail_matches_shape`] is the rest of the check.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
const LEGACY_AUCTION_SUBJECTS: &[(&str, AuctionMailAction)] = &[
    ("Auction outbid", AuctionMailAction::Outbid),
    ("Auction won", AuctionMailAction::Won),
    ("Auction sold", AuctionMailAction::Successful),
    ("Auction expired", AuctionMailAction::Expired),
    ("Auction listing refused", AuctionMailAction::Cancelled),
    // The deferred bid refund shares no subject with the ordinary Outbid mail, so it needs its
    // own row here; it also renders as Outbid (see `CtxBidMarket::commit_refund`).
    ("Auction bid refund", AuctionMailAction::Outbid),
];

/// The pure half of the repair: which `AuctionMailAction` a legacy row's exact English subject
/// maps to, or `None` for a subject this repair does not recognize.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
fn legacy_auction_action(subject: &str) -> Option<AuctionMailAction> {
    LEGACY_AUCTION_SUBJECTS
        .iter()
        .find(|(known, _)| *known == subject)
        .map(|(_, action)| *action)
}

/// Whether `mail` carries the exact field shape the earlier auction code produced for its
/// subject — the first of two checks that tell a real letter apart from a look-alike. A player's
/// own mail can share the subject text (nothing stops them from titling a letter "Auction won"),
/// but every legacy row also carries `cod == 0` and an empty body (auction mail never used
/// either), and:
/// - Outbid, Expired, Cancelled and the deferred bid refund came from guid 0 — no player-sent
///   letter ever does, since a send always stamps the sender's own actor guid. That alone rules
///   out a look-alike for these four subjects.
/// - Expired and Cancelled carry an item; Outbid and the bid refund carry none.
/// - Won carries an item and no money; Sold carries money and no item. Neither has a guid-0 tell —
///   a real player letter can be titled either with a real sender and the right shape — so
///   [`legacy_repair_authorization`] is what actually clears them, not this function.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
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

/// A converted row's house and the item reference its vanilla subject encodes.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
struct LegacyRepairTarget {
    house: u32,
    item_entry: u32,
    random_property_id: u32,
}

/// The second, and for Won/Sold the decisive, check: whether a durable listing receipt backs
/// `mail`'s claim, and if so, the house and subject-item that receipt supplies. Every other legacy
/// subject already cleared [`legacy_mail_matches_shape`]'s guid-0 tell, so a receipt only sharpens
/// its house; Won and Sold do not have that tell, because nothing else in the mail row tells a
/// real settlement apart from a same-titled player letter with an unrelated real sender:
/// - Won: some receipt's actor is the mail's claimed sender (the seller who listed this exact
///   item) and its item entry and stack count match what the mail carries. A look-alike sent by an
///   uninvolved player never has a receipt naming them as that item's seller.
/// - Sold: some receipt's actor is the mail's recipient and its `auction_id` is not the
///   refused-listing sentinel (0) — proceeds mail carries no item of its own to cross-check, so a
///   refused listing's refund receipt is not a settlement at all. A seller can have more than one
///   real receipt, so when more than one candidate remains, only the one whose listing terms could
///   have produced this exact proceeds figure ([`receipt_could_pay_out`]) is trusted; if that still
///   leaves more than one, or none, the row is left unmapped rather than guessed at. The subject
///   also borrows the winning receipt's item, since the mail's own item fields are 0 (Sold never
///   attaches the item).
/// - Expired and Cancelled: some receipt's actor is the mail's recipient (the seller the item
///   returned to) and its item entry and stack count match what the mail carries, the same
///   assurance Won uses. A `Cancelled` (refused-listing) row's own receipt IS the `auction_id == 0`
///   sentinel, so unlike Sold this does not exclude it.
///
/// Every subject above takes its house from that receipt rather than the recipient's own race
/// (`house_for_faction_template`'s approach for a live sale,
/// `crates/lyracore-shared/src/auction.rs`): Realm-core, the database this repair runs against,
/// carries no Character rows at all, so a race lookup would always miss and fall back to the
/// neutral house. A receipt's `house` is durable state recorded when the listing was created, so
/// it needs no Character lookup and is correct on Realm-core too. Expired and Cancelled fall back
/// to the race lookup only if no receipt matches their item (a legacy row that predates receipts,
/// say); Outbid and the deferred bid refund have no receipt to draw from at all — a bidder is never
/// a receipt's actor — and always use the race lookup.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
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
                        return None; // still ambiguous — more than one listing could have paid this
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

/// Whether some winning price within `receipt`'s listed range (`start_bid..=buyout`, or
/// `start_bid..` when `buyout` is 0 — an auction the vanilla protocol lets bidding pass without a
/// cap) pays the seller exactly `money`. `seller_proceeds` is non-decreasing in price and never
/// skips a whole copper as price climbs by one (its cut grows by at most one copper per copper of
/// price), so every integer between its low and high ends is reachable — checking the two ends
/// bounds every price in between too.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
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
        None => true, // the buyout's own proceeds overflow u32; some in-range price still might not
    }
}

/// The house a converted row shows the client on a deployment where Realm-core's Character rows
/// exist, chosen from the recipient's own team: Alliance goes to Stormwind (1), Horde to Orgrimmar
/// (6), matching `house_for_faction_template`'s own team fallback
/// (`crates/lyracore-shared/src/auction.rs`). Falls back to the neutral house (7) for a recipient
/// with no Character row — the same fallback that function uses for a template it cannot place on
/// either team, and Realm-core's only outcome, since it carries no Character rows at all.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
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

/// Family name `repair_legacy_auction_mail` stamps in `game_import_meta` once it has run on a
/// database, so a later publish's repair pass does not re-scan mail a player wrote after the
/// re-tag — including a letter that happens to name one of [`LEGACY_AUCTION_SUBJECTS`].
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
const LEGACY_AUCTION_MAIL_REPAIR_FAMILY: &str = "repair_legacy_auction_mail";

/// Re-tag auction mail written before the vanilla Auction Mail format shipped to the vanilla
/// `AuctionHouse` sender and machine subject, once per database. A legacy row still reads as
/// `Character` mail: once the Mail Timer is live, its expiry returns an already-settled "Auction
/// won" to the seller, who was already paid — cmangos deletes a Character mail's item at expiry
/// only when it is unreturned (`cm:ObjectMgr.cpp:6188-6232`), and a legacy Auction Mail was never
/// meant to be returnable at all. Re-tagging moves it out from under that rule, the same way
/// `mail::plan_return` already refuses Return on any current Auction Mail.
///
/// Runs at most once: a marker in `game_import_meta` (`LEGACY_AUCTION_MAIL_REPAIR_FAMILY`) records
/// that this database has already been swept, so a later publish's repair pass leaves every mail
/// alone, including a real player letter that happens to match one of the legacy subjects. Within
/// that one run, [`legacy_mail_matches_shape`] and [`legacy_repair_authorization`] are the two
/// safeguards for every row already on the table: a row converts only if its sender, cod, body,
/// item and money exactly match what the old code produced, and — for "Auction won" and "Auction
/// sold", which carry no tell of their own — a durable listing receipt backs its claim. Returns
/// `(converted, unmapped)`; a nonzero `unmapped` is an auction-looking row this repair left alone,
/// worth a human look.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
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
        || item.item_text_id != 0
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

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> AuctionHousePolicy {
        AuctionHousePolicy {
            id: 1,
            deposit_rate: 5,
            consignment_rate: 5,
        }
    }

    #[test]
    fn seller_proceeds_apply_the_imported_cut_and_return_the_deposit_checked() {
        assert_eq!(seller_proceeds(100, 10, 5), Some(105));
        assert_eq!(seller_proceeds(100, 10, 15), Some(95));
        assert_eq!(seller_proceeds(19, 1, 5), Some(20));
        assert_eq!(seller_proceeds(20, 1, 5), Some(20));
        assert_eq!(seller_proceeds(u32::MAX, u32::MAX, 5), None);
        assert_eq!(seller_proceeds(100, 10, 101), None);
    }

    // The subject is "{item_entry}:{random_property_id}:{action}" (`cm:AuctionHouseMgr.cpp:134,
    // 181,229`), and the action codes are `MailAuctionAnswers` (`cm:Mail.h:100-109`): every string
    // below is written out by hand from that format, not produced by calling the function under
    // test with different inputs.
    #[test]
    fn the_subject_is_the_item_the_random_property_and_the_mail_auction_answers_code() {
        assert_eq!(
            auction_mail_subject(1234, 56, AuctionMailAction::Outbid),
            "1234:56:0"
        );
        assert_eq!(
            auction_mail_subject(1234, 56, AuctionMailAction::Won),
            "1234:56:1"
        );
        assert_eq!(
            auction_mail_subject(1234, 56, AuctionMailAction::Successful),
            "1234:56:2"
        );
        assert_eq!(
            auction_mail_subject(1234, 56, AuctionMailAction::Expired),
            "1234:56:3"
        );
        assert_eq!(
            auction_mail_subject(1234, 56, AuctionMailAction::CancelledToBidder),
            "1234:56:4"
        );
        assert_eq!(
            auction_mail_subject(1234, 0, AuctionMailAction::Cancelled),
            "1234:0:5",
            "a plain item's random property id is 0"
        );
    }

    fn mail_for_body(action: AuctionMailAction) -> AuctionMail {
        AuctionMail {
            recipient_guid: 1,
            house: 1,
            action,
            item_entry: 1234,
            random_property_id: 56,
            money: 0,
            attached_item: crate::items::ItemSnapshot::default(),
            counterparty_guid: 0,
            bid: 0,
            buyout: 0,
            deposit: 0,
            cut: 0,
        }
    }

    /// The invoice body's guid is lowercase hex, right-aligned in a 16-character, space-filled
    /// field (`fx:MailFrame.lua:300-361`): the padding widths below are counted by hand, not
    /// produced by the `{:>16x}` specifier under test.
    #[test]
    fn the_won_and_successful_bodies_are_the_padded_invoice_cmangos_sends() {
        let won = AuctionMail {
            counterparty_guid: 9,
            bid: 100,
            buyout: 500,
            ..mail_for_body(AuctionMailAction::Won)
        };
        assert_eq!(
            auction_mail_body(&won),
            format!("{}9:100:500", " ".repeat(15)),
            "guid 9 is one hex digit, padded with 15 leading spaces to a 16-wide field"
        );

        let successful = AuctionMail {
            counterparty_guid: 0x123,
            bid: 250,
            buyout: 0,
            deposit: 12,
            cut: 6,
            ..mail_for_body(AuctionMailAction::Successful)
        };
        assert_eq!(
            auction_mail_body(&successful),
            format!("{}123:250:0:12:6", " ".repeat(13)),
            "guid 0x123 is three hex digits, padded with 13 leading spaces"
        );
    }

    #[test]
    fn outbid_expired_and_cancelled_mail_carries_no_body() {
        for action in [
            AuctionMailAction::Outbid,
            AuctionMailAction::Expired,
            AuctionMailAction::CancelledToBidder,
            AuctionMailAction::Cancelled,
        ] {
            assert_eq!(auction_mail_body(&mail_for_body(action)), "");
        }
    }

    #[test]
    fn outbid_notice_fires_only_for_a_real_displaced_bidder() {
        let item = crate::items::ItemSnapshot {
            entry: 1234,
            random_property_id: 56,
            ..crate::items::ItemSnapshot::default()
        };
        assert_eq!(
            outbid_notice(1, 41, item, 0, 8, 0),
            None,
            "a fresh listing's first bid displaces nobody"
        );
        assert_eq!(
            outbid_notice(1, 41, item, 9, 8, 100),
            Some(AuctionNoticeDraft {
                recipient_guid: 9,
                kind: auction_notice::OUTBID,
                house: 1,
                auction_id: 41,
                item_entry: 1234,
                random_property_id: 56,
                bid: 100,
                out_bid: 5, // vanilla minimum raise on a 100 bid: 5% rounded up
                bidder_guid: 9,
            })
        );
    }

    /// cmangos's `UpdateBid` never calls `SendAuctionBidderNotification` when the new bidder is
    /// the bidder it displaces — raising your own bid is not being outbid
    /// (`cm:AuctionHouseMgr.cpp:780-793`).
    #[test]
    fn a_bidder_who_raises_their_own_bid_gets_no_outbid_notice() {
        let item = crate::items::ItemSnapshot {
            entry: 1234,
            random_property_id: 56,
            ..crate::items::ItemSnapshot::default()
        };
        assert_eq!(
            outbid_notice(1, 41, item, 9, 9, 100),
            None,
            "the displaced bidder and the new bidder are the same Character"
        );
    }

    #[test]
    fn new_bid_and_expired_notices_address_the_owner() {
        let item = crate::items::ItemSnapshot {
            entry: 1234,
            random_property_id: 56,
            ..crate::items::ItemSnapshot::default()
        };
        assert_eq!(
            new_bid_notice(1, 41, item, 7, 9, 107),
            AuctionNoticeDraft {
                recipient_guid: 7,
                kind: auction_notice::NEW_BID,
                house: 1,
                auction_id: 41,
                item_entry: 1234,
                random_property_id: 56,
                bid: 107,
                out_bid: 6, // vanilla minimum raise on a 107 bid: 5% rounded up
                bidder_guid: 9,
            }
        );
        assert_eq!(
            expired_notice(1, 41, item, 7),
            AuctionNoticeDraft {
                recipient_guid: 7,
                kind: auction_notice::EXPIRED,
                house: 1,
                auction_id: 41,
                item_entry: 1234,
                random_property_id: 56,
                bid: 0,
                // bid_increment(0) is 0, but expired_notice floors an unsold listing's raise to 1.
                out_bid: 1,
                bidder_guid: 0,
            }
        );
    }

    #[test]
    fn settlement_notices_are_won_to_the_buyer_and_sold_to_the_seller() {
        let item = crate::items::ItemSnapshot {
            entry: 1234,
            random_property_id: 56,
            ..crate::items::ItemSnapshot::default()
        };
        let [won, sold] = settlement_notices(1, 41, item.entry, item.random_property_id, 7, 9, 500);
        assert_eq!(
            won,
            AuctionNoticeDraft {
                recipient_guid: 9,
                kind: auction_notice::WON,
                house: 1,
                auction_id: 41,
                item_entry: 1234,
                random_property_id: 56,
                bid: 0,
                out_bid: 25, // vanilla minimum raise on a 500 bid: 5% rounded up
                bidder_guid: 9,
            }
        );
        assert_eq!(
            sold,
            AuctionNoticeDraft {
                recipient_guid: 7,
                kind: auction_notice::SOLD,
                house: 1,
                auction_id: 41,
                item_entry: 1234,
                random_property_id: 56,
                bid: 500,
                out_bid: 25, // vanilla minimum raise on a 500 bid: 5% rounded up
                bidder_guid: 0,
            }
        );
    }

    #[test]
    fn every_legacy_auction_subject_maps_to_its_action_and_an_unknown_one_does_not() {
        assert_eq!(
            legacy_auction_action("Auction outbid"),
            Some(AuctionMailAction::Outbid)
        );
        assert_eq!(
            legacy_auction_action("Auction won"),
            Some(AuctionMailAction::Won)
        );
        assert_eq!(
            legacy_auction_action("Auction sold"),
            Some(AuctionMailAction::Successful)
        );
        assert_eq!(
            legacy_auction_action("Auction expired"),
            Some(AuctionMailAction::Expired)
        );
        assert_eq!(
            legacy_auction_action("Auction listing refused"),
            Some(AuctionMailAction::Cancelled)
        );
        assert_eq!(
            legacy_auction_action("Auction bid refund"),
            Some(AuctionMailAction::Outbid),
            "the deferred bid refund has no vanilla twin and renders as Outbid"
        );
        assert_eq!(legacy_auction_action("Auction House of Cards"), None);
        assert_eq!(legacy_auction_action("meet me at the gate"), None);
    }

    #[test]
    fn listing_deposit_uses_the_imported_rate_and_supported_duration_ladder() {
        assert_eq!(listing_deposit(100, 2, 720, 5), Some(10));
        assert_eq!(listing_deposit(100, 2, 720, 25), Some(50));
        assert_eq!(listing_deposit(100, 2, 1_440, 5), Some(20));
        assert_eq!(listing_deposit(100, 2, 2_880, 5), Some(40));
        assert_eq!(listing_deposit(1, 1, 720, 5), Some(1));
        assert_eq!(listing_deposit(100, 1, 60, 5), None);
        assert_eq!(listing_deposit(u32::MAX, u32::MAX, 2_880, 5), None);
        assert_eq!(listing_deposit(100, 1, 720, 101), None);
    }

    fn item(slot: u8) -> ListingItem {
        ListingItem {
            guid: 70,
            owner_guid: 7,
            slot,
            mailable: true,
            snapshot: crate::items::ItemSnapshot {
                entry: 25,
                stack_count: 2,
                durability: 17,
                enchant_id: 9,
                soulbound: false,
                random_property_id: 117,
            },
            sell_price: 100,
            item_text_id: 0,
        }
    }

    fn active_bid_auction() -> BidAuction {
        let snapshot = item(23).snapshot;
        BidAuction {
            id: 41,
            house: 1,
            owner_guid: 7,
            item: snapshot,
            highest_bidder_guid: 0,
            highest_bid: 0,
            start_bid: 100,
            buyout: 0,
            deposit: 10,
            consignment_rate: 5,
            expires_micros: 2_000,
            revision: 3,
        }
    }

    fn accepted_active(
        price: u32,
        revision: u64,
        displaced_bidder_guid: u64,
        displaced_bid: u32,
    ) -> HoldDecision {
        HoldDecision::Accepted(BidAcceptance {
            price,
            effect: AuctionBidEffect::RemainActive { revision },
            displaced_bidder_guid,
            displaced_bid,
        })
    }

    fn accepted_buyout(price: u32, displaced_bidder_guid: u64, displaced_bid: u32) -> HoldDecision {
        HoldDecision::Accepted(BidAcceptance {
            price,
            effect: AuctionBidEffect::SettleBuyout,
            displaced_bidder_guid,
            displaced_bid,
        })
    }

    fn terms() -> ListingTerms {
        ListingTerms {
            start_bid: 10,
            buyout: 20,
            duration_minutes: 720,
        }
    }

    #[test]
    fn listing_accepts_only_one_owned_mailable_bag_stack_and_valid_terms() {
        assert_eq!(
            prepare_listing(Some(&item(23)), 7, 10, terms(), policy()),
            Ok(10)
        );
        assert_eq!(
            prepare_listing(Some(&item(120)), 7, 10, terms(), policy()),
            Ok(10)
        );

        for slot in [15, 19, 39, 63, 119, 192] {
            assert_eq!(
                prepare_listing(Some(&item(slot)), 7, 10, terms(), policy()),
                Err(AuctionRefusal::ItemNotFound),
                "slot {slot} must not be auctionable"
            );
        }

        let mut foreign = item(23);
        foreign.owner_guid = 8;
        assert_eq!(
            prepare_listing(Some(&foreign), 7, 10, terms(), policy()),
            Err(AuctionRefusal::ItemNotFound)
        );

        let mut soulbound = item(23);
        soulbound.snapshot.soulbound = true;
        assert_eq!(
            prepare_listing(Some(&soulbound), 7, 10, terms(), policy()),
            Err(AuctionRefusal::ItemNotFound)
        );

        let mut not_mailable = item(120);
        not_mailable.mailable = false;
        assert_eq!(
            prepare_listing(Some(&not_mailable), 7, 10, terms(), policy()),
            Err(AuctionRefusal::ItemNotFound)
        );

        // Stopgap: a Plain Letter's readable text does not survive a listing yet
        // (`ItemSnapshot` carries no text id), so refuse it the same way a soulbound item is
        // refused, rather than let it sell and arrive blank.
        let mut readable = item(23);
        readable.item_text_id = 1;
        assert_eq!(
            prepare_listing(Some(&readable), 7, 10, terms(), policy()),
            Err(AuctionRefusal::ItemNotFound)
        );

        assert_eq!(
            prepare_listing(None, 7, 10, terms(), policy()),
            Err(AuctionRefusal::ItemNotFound)
        );
        assert_eq!(
            prepare_listing(Some(&item(23)), 7, 9, terms(), policy()),
            Err(AuctionRefusal::NotEnoughMoney)
        );

        for invalid in [
            ListingTerms {
                start_bid: 0,
                ..terms()
            },
            ListingTerms {
                start_bid: 20,
                buyout: 19,
                ..terms()
            },
            ListingTerms {
                duration_minutes: 60,
                ..terms()
            },
        ] {
            assert_eq!(
                prepare_listing(Some(&item(23)), 7, 10, invalid, policy()),
                Err(AuctionRefusal::InvalidTerms)
            );
        }
    }

    #[derive(Clone)]
    struct FakeLocal {
        money: Option<u32>,
        item: Option<ListingItem>,
        now_micros: i64,
        next_auction_id: u32,
        committed: Option<(PreparedListing, u32)>,
    }

    impl ListingSource for FakeLocal {
        fn seller_money(&self, _seller_guid: u64) -> Option<u32> {
            self.money
        }

        fn item(&self, _item_guid: u64) -> Option<ListingItem> {
            self.item.clone()
        }

        fn now_micros(&self) -> i64 {
            self.now_micros
        }
    }

    impl LocalListingSink for FakeLocal {
        fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
            self.committed
                .as_ref()
                .filter(|(listing, _)| listing.request.operation_id == operation_id)
                .map(|(listing, auction_id)| ListingReceipt {
                    listing: listing.clone(),
                    auction_id: *auction_id,
                })
        }

        fn commit_local(&mut self, listing: PreparedListing) -> u32 {
            self.money = self.money.map(|money| money - listing.deposit);
            self.item = None;
            let auction_id = self.next_auction_id;
            self.next_auction_id += 1;
            self.committed = Some((listing, auction_id));
            auction_id
        }
    }

    fn request() -> ListingRequest {
        ListingRequest {
            operation_id: 900,
            seller_guid: 7,
            item_guid: 70,
            house: policy(),
            terms: terms(),
        }
    }

    fn local() -> FakeLocal {
        FakeLocal {
            money: Some(50),
            item: Some(item(23)),
            now_micros: 1_000,
            next_auction_id: 41,
            committed: None,
        }
    }

    #[test]
    fn local_listing_atomically_moves_the_exact_item_and_deposit_into_one_scheduled_auction() {
        let mut store = local();

        assert_eq!(create_local_listing(&mut store, request()), Ok(41));

        assert_eq!(store.money, Some(40));
        assert!(store.item.is_none());
        let (listing, auction_id) = store.committed.as_ref().expect("one committed Auction");
        assert_eq!(*auction_id, 41);
        assert_eq!(listing.snapshot, item(23).snapshot);
        assert_eq!(listing.deposit, 10);
        assert_eq!(listing.created_micros, 1_000);
        assert_eq!(listing.expires_micros, 43_200_001_000);

        assert_eq!(create_local_listing(&mut store, request()), Ok(41));
        assert_eq!(store.money, Some(40), "replay must not charge again");
        assert_eq!(store.next_auction_id, 42, "replay must not mint another id");
    }

    #[test]
    fn local_listing_refusals_leave_item_purse_and_market_unchanged() {
        let cases = [
            {
                let mut store = local();
                store.money = Some(9);
                (store, request())
            },
            {
                let mut bad = request();
                bad.terms.duration_minutes = 60;
                (local(), bad)
            },
            {
                let mut bad = request();
                bad.operation_id = 0;
                (local(), bad)
            },
        ];

        for (mut store, request) in cases {
            let before = store.clone();
            assert!(create_local_listing(&mut store, request).is_err());
            assert_eq!(store.money, before.money);
            assert_eq!(store.item, before.item);
            assert_eq!(store.next_auction_id, before.next_auction_id);
            assert!(store.committed.is_none());
        }
    }

    #[derive(Clone)]
    struct FakeSource {
        money: Option<u32>,
        item: Option<ListingItem>,
        now_micros: i64,
        hold: Option<ListingHold>,
        receipt: Option<ListingReceipt>,
    }

    impl ListingSource for FakeSource {
        fn seller_money(&self, _seller_guid: u64) -> Option<u32> {
            self.money
        }

        fn item(&self, _item_guid: u64) -> Option<ListingItem> {
            self.item.clone()
        }

        fn now_micros(&self) -> i64 {
            self.now_micros
        }
    }

    impl HoldSink for FakeSource {
        fn hold(&self, operation_id: u64) -> Option<ListingHold> {
            self.hold
                .as_ref()
                .filter(|hold| hold.listing.request.operation_id == operation_id)
                .cloned()
        }

        fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
            self.receipt
                .as_ref()
                .filter(|receipt| receipt.listing.request.operation_id == operation_id)
                .cloned()
        }

        fn commit_hold(&mut self, hold: ListingHold) {
            self.money = self.money.map(|money| money - hold.listing.deposit);
            self.item = None;
            self.hold = Some(hold);
        }

        fn confirm_hold(&mut self, receipt: ListingReceipt) {
            self.receipt = Some(receipt);
        }

        fn delete_hold(&mut self, operation_id: u64) {
            if self
                .hold
                .as_ref()
                .is_some_and(|hold| hold.listing.request.operation_id == operation_id)
            {
                self.hold = None;
            }
        }
    }

    #[derive(Clone, Default)]
    struct FakeRefundCore {
        refund: Option<ListingRefund>,
        mails: Vec<AuctionMail>,
    }

    impl ListingRefundSink for FakeRefundCore {
        fn refund(&self, operation_id: u64) -> Result<Option<ListingRefund>, AuctionRefusal> {
            Ok(self
                .refund
                .as_ref()
                .filter(|refund| refund.listing.request.operation_id == operation_id)
                .cloned())
        }

        fn commit_refund(&mut self, refund: ListingRefund) {
            self.mails.push(refund.mail);
            self.refund = Some(refund);
        }
    }

    #[derive(Clone)]
    struct FakeMarket {
        next_auction_id: u32,
        receipt: Option<ListingReceipt>,
        auction_count: usize,
    }

    impl MarketSink for FakeMarket {
        fn receipt(&self, operation_id: u64) -> Option<ListingReceipt> {
            self.receipt
                .as_ref()
                .filter(|receipt| receipt.listing.request.operation_id == operation_id)
                .cloned()
        }

        fn commit_market(&mut self, listing: PreparedListing) -> u32 {
            let auction_id = self.next_auction_id;
            self.next_auction_id += 1;
            self.auction_count += 1;
            self.receipt = Some(ListingReceipt {
                listing,
                auction_id,
            });
            auction_id
        }
    }

    fn source() -> FakeSource {
        FakeSource {
            money: Some(50),
            item: Some(item(23)),
            now_micros: 1_000,
            hold: None,
            receipt: None,
        }
    }

    #[test]
    fn listing_rejects_overflowing_possible_proceeds_before_moving_value() {
        for terms in [
            ListingTerms {
                start_bid: u32::MAX,
                buyout: 0,
                duration_minutes: 720,
            },
            ListingTerms {
                start_bid: 10,
                buyout: u32::MAX,
                duration_minutes: 720,
            },
        ] {
            let mut local = local();
            local.money = Some(u32::MAX);
            local.item.as_mut().unwrap().sell_price = u32::MAX;
            let before = local.clone();
            let request = ListingRequest { terms, ..request() };

            assert_eq!(
                create_local_listing(&mut local, request),
                Err(AuctionRefusal::InvalidTerms)
            );
            assert_eq!(local.money, before.money);
            assert_eq!(local.item, before.item);
            assert!(local.committed.is_none());

            let mut source = source();
            source.money = Some(u32::MAX);
            source.item.as_mut().unwrap().sell_price = u32::MAX;
            let before = source.clone();

            assert_eq!(
                fence_listing(&mut source, request),
                Err(AuctionRefusal::InvalidTerms)
            );
            assert_eq!(source.money, before.money);
            assert_eq!(source.item, before.item);
            assert!(source.hold.is_none());
            assert!(source.receipt.is_none());
        }
    }

    fn market() -> FakeMarket {
        FakeMarket {
            next_auction_id: 41,
            receipt: None,
            auction_count: 0,
        }
    }

    fn drive_sharded(
        source: &mut FakeSource,
        market: &mut FakeMarket,
        request: ListingRequest,
    ) -> Result<u32, AuctionRefusal> {
        fence_listing(source, request)?;
        if let Some(receipt) = source.receipt(request.operation_id) {
            settle_listing(source, request.operation_id)?;
            return Ok(receipt.auction_id);
        }
        let hold = source
            .hold(request.operation_id)
            .expect("a successful fresh fence leaves a durable Hold");
        let auction_id = commit_held_listing(market, hold.listing.clone())?;
        confirm_listing(
            source,
            ListingReceipt {
                listing: hold.listing,
                auction_id,
            },
        )?;
        settle_listing(source, request.operation_id)?;
        Ok(auction_id)
    }

    #[test]
    fn sharded_listing_retries_every_interruption_without_losing_or_duplicating_value() {
        for killed_after in 0..=3 {
            let mut source = source();
            let mut market = market();
            let request = request();

            fence_listing(&mut source, request).expect("fenced");
            if killed_after >= 1 {
                let hold = source.hold(request.operation_id).unwrap();
                let auction_id = commit_held_listing(&mut market, hold.listing.clone()).unwrap();
                if killed_after >= 2 {
                    confirm_listing(
                        &mut source,
                        ListingReceipt {
                            listing: hold.listing,
                            auction_id,
                        },
                    )
                    .unwrap();
                    if killed_after >= 3 {
                        settle_listing(&mut source, request.operation_id).unwrap();
                    }
                }
            }

            assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
            assert_eq!(source.money, Some(40));
            assert!(source.item.is_none());
            assert!(source.hold.is_none(), "Hold is deleted last");
            assert_eq!(source.receipt.as_ref().unwrap().auction_id, 41);
            assert_eq!(market.auction_count, 1);
            assert_eq!(
                market.receipt.as_ref().unwrap().listing.snapshot,
                item(23).snapshot
            );

            assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
            assert_eq!(source.money, Some(40));
            assert_eq!(market.auction_count, 1);
        }
    }

    #[test]
    fn sharded_listing_accounts_for_the_item_and_deposit_at_every_durable_boundary() {
        let mut source = source();
        let mut market = market();
        let request = request();

        assert_eq!(source.money, Some(50));
        assert_eq!(
            source.item.as_ref().map(|item| item.snapshot),
            Some(item(23).snapshot)
        );
        assert!(source.hold.is_none());
        assert!(market.receipt.is_none());

        fence_listing(&mut source, request).unwrap();
        let hold = source.hold(request.operation_id).unwrap();
        assert_eq!(source.money, Some(40));
        assert!(source.item.is_none());
        assert_eq!(hold.listing.snapshot, item(23).snapshot);
        assert_eq!(40 + hold.listing.deposit, 50);
        assert!(market.receipt.is_none());

        let auction_id = commit_held_listing(&mut market, hold.listing.clone()).unwrap();
        let market_receipt = market.receipt.as_ref().unwrap();
        assert_eq!(auction_id, 41);
        assert_eq!(market_receipt.listing.snapshot, item(23).snapshot);
        assert_eq!(40 + market_receipt.listing.deposit, 50);
        assert_eq!(
            source.hold.as_ref().unwrap().listing.snapshot,
            market_receipt.listing.snapshot,
            "the source Hold is recovery evidence for the one market item"
        );

        confirm_listing(
            &mut source,
            ListingReceipt {
                listing: hold.listing,
                auction_id,
            },
        )
        .unwrap();
        assert_eq!(
            source.receipt.as_ref().unwrap(),
            market.receipt.as_ref().unwrap()
        );
        assert!(source.hold.is_some());

        settle_listing(&mut source, request.operation_id).unwrap();
        assert!(source.hold.is_none());
        assert_eq!(source.money, Some(40));
        assert_eq!(market.auction_count, 1);
        assert_eq!(
            market.receipt.as_ref().unwrap().listing.snapshot,
            item(23).snapshot
        );
        assert_eq!(40 + market.receipt.as_ref().unwrap().listing.deposit, 50);
    }

    #[test]
    fn conflicting_operation_id_reuse_fails_closed_on_both_planes() {
        let mut source = source();
        let mut market = market();
        let original = request();
        fence_listing(&mut source, original).unwrap();
        let hold = source.hold(original.operation_id).unwrap();
        commit_held_listing(&mut market, hold.listing).unwrap();

        let mut changed = original;
        changed.terms.buyout += 1;
        assert_eq!(
            fence_listing(&mut source, changed),
            Err(AuctionRefusal::InvalidTerms)
        );

        let mut changed_payload = source.hold(original.operation_id).unwrap().listing;
        changed_payload.snapshot.durability += 1;
        assert_eq!(
            commit_held_listing(&mut market, changed_payload),
            Err(AuctionRefusal::InvalidTerms)
        );
        assert_eq!(source.money, Some(40));
        assert_eq!(market.auction_count, 1);
    }

    #[test]
    fn realm_refusal_releases_the_hold_and_mails_the_item_and_deposit_back() {
        let mut source = source();
        let mut realm = FakeRefundCore::default();
        let request = request();
        fence_listing(&mut source, request).unwrap();
        assert_eq!(source.money, Some(40));
        assert!(source.item.is_none());

        // Realm-core refused phase 2 (for example, its item catalogue lacks the template), so
        // the market never took the listing and the source Hold is the only copy of the value.
        // The refund Mail and its receipt are written on Realm-core before its source Hold moves.
        let refund = listing_refund(&source.hold(request.operation_id).unwrap().listing);
        assert_eq!(refund_listing(&mut realm, refund.clone()), Ok(()));
        assert_eq!(realm.mails, vec![refund.mail]);
        assert!(source.hold.is_some());
        let mut changed_refund = refund.clone();
        changed_refund.listing.snapshot.durability += 1;
        assert_eq!(
            refund_listing(&mut realm, changed_refund),
            Err(AuctionRefusal::InvalidTerms),
            "the durable refund receipt refuses conflicting operation-id reuse"
        );
        assert_eq!(realm.mails.len(), 1);
        assert_eq!(
            release_listing(&mut source, request.operation_id, request.seller_guid + 1),
            Err(AuctionRefusal::InvalidTerms),
            "a wrong seller never releases another character's Hold"
        );
        assert!(source.hold.is_some());
        assert_eq!(
            release_listing(&mut source, request.operation_id, request.seller_guid),
            Ok(())
        );
        assert!(source.hold.is_none());

        // A gateway crash after Realm-core commits but before source cleanup replays one Mail.
        assert_eq!(refund_listing(&mut realm, refund), Ok(()));
        assert_eq!(realm.mails.len(), 1);
        assert_eq!(
            release_listing(&mut source, request.operation_id, request.seller_guid),
            Ok(()),
            "a replayed release finds no Hold and returns nothing twice"
        );
        assert_eq!(realm.mails.len(), 1);
    }

    #[test]
    fn refund_receipt_replays_as_a_refusal_and_never_confirms_the_source_hold() {
        let mut source = source();
        let mut realm = market();
        let request = request();
        fence_listing(&mut source, request).unwrap();
        let listing = source.hold(request.operation_id).unwrap().listing;
        realm.receipt = Some(ListingReceipt {
            listing: listing.clone(),
            auction_id: 0,
        });

        assert_eq!(
            commit_held_listing(&mut realm, listing.clone()),
            Err(AuctionRefusal::InvalidTerms),
            "a Realm-core refund receipt is not an Auction"
        );
        let mut local = local();
        local.committed = Some((listing.clone(), 0));
        assert_eq!(
            operation_match(&local, listing.request),
            OperationMatch::Conflict,
            "a sentinel is never a local-listing replay"
        );
        source.receipt = Some(ListingReceipt {
            listing: listing.clone(),
            auction_id: 0,
        });
        assert_eq!(
            fence_listing(&mut source, listing.request),
            Err(AuctionRefusal::InvalidTerms),
            "a misrouted sentinel must fail closed on the source"
        );
        assert_eq!(
            confirm_listing(
                &mut source,
                ListingReceipt {
                    listing,
                    auction_id: 0,
                },
            ),
            Err(AuctionRefusal::InvalidTerms),
            "the sentinel must never cross back to the source"
        );
        assert_eq!(
            settle_listing(&mut source, request.operation_id),
            Err(AuctionRefusal::InvalidTerms),
            "source cleanup may not treat a sentinel as receipt evidence"
        );
        assert!(source.hold.is_some());
        assert_eq!(
            source.receipt.as_ref().map(|receipt| receipt.auction_id),
            Some(0)
        );
    }

    #[test]
    fn release_refuses_a_hold_that_backs_a_live_auction() {
        let mut source = source();
        let mut market = market();
        let request = request();
        fence_listing(&mut source, request).unwrap();
        let hold = source.hold(request.operation_id).unwrap();
        let auction_id = commit_held_listing(&mut market, hold.listing.clone()).unwrap();
        confirm_listing(
            &mut source,
            ListingReceipt {
                listing: hold.listing,
                auction_id,
            },
        )
        .unwrap();

        assert_eq!(
            release_listing(&mut source, request.operation_id, request.seller_guid),
            Err(AuctionRefusal::InvalidTerms)
        );
        assert!(source.hold.is_some());
        assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
        assert!(source.hold.is_none());
        assert_eq!(market.auction_count, 1);
    }

    #[derive(Clone)]
    struct FakeExpiry {
        auction: Option<ActiveAuction>,
        schedule_count: usize,
        mail: Vec<AuctionMail>,
        notices: Vec<AuctionNoticeDraft>,
        returned_items: Vec<crate::items::ItemSnapshot>,
        refunded_copper: u32,
    }

    impl ExpirySink for FakeExpiry {
        fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String> {
            Ok(self
                .auction
                .as_ref()
                .filter(|auction| auction.id == auction_id)
                .cloned())
        }

        fn complete_expiry(&mut self, auction: ActiveAuction, completion: ExpiryCompletion) {
            let house = auction.listing.request.house.id;
            let item = auction.listing.snapshot;
            let owner_guid = auction.listing.request.seller_guid;
            match completion {
                ExpiryCompletion::Unsold(mail) => {
                    self.returned_items.push(mail.attached_item);
                    self.notices
                        .push(expired_notice(house, auction.id, item, owner_guid));
                    self.mail.push(mail);
                }
                ExpiryCompletion::Sold(mail) => {
                    self.notices.extend(settlement_notices(
                        house,
                        auction.id,
                        item.entry,
                        item.random_property_id,
                        owner_guid,
                        auction.highest_bidder_guid,
                        auction.highest_bid,
                    ));
                    self.mail.extend(mail);
                }
            }
            self.auction = None;
            self.schedule_count = 0;
        }
    }

    #[test]
    fn replayed_unbid_expiry_returns_the_exact_item_once_and_forfeits_the_deposit() {
        let listing = PreparedListing {
            request: request(),
            snapshot: item(23).snapshot,
            deposit: 10,
            created_micros: 1_000,
            expires_micros: 43_200_001_000,
        };
        let mut store = FakeExpiry {
            auction: Some(ActiveAuction {
                id: 41,
                listing,
                highest_bidder_guid: 0,
                highest_bid: 0,
            }),
            schedule_count: 1,
            mail: Vec::new(),
            notices: Vec::new(),
            returned_items: Vec::new(),
            refunded_copper: 0,
        };

        assert_eq!(expire_active(&mut store, 41), Ok(()));
        assert_eq!(expire_active(&mut store, 41), Ok(()));

        assert!(store.auction.is_none());
        assert_eq!(store.schedule_count, 0);
        assert_eq!(store.returned_items, vec![item(23).snapshot]);
        assert_eq!(store.refunded_copper, 0, "the listing deposit is forfeited");
    }

    #[test]
    fn local_and_sharded_listings_produce_the_same_unbid_and_bid_expiry_mail() {
        let mut local_store = local();
        create_local_listing(&mut local_store, request()).unwrap();
        let local_listing = local_store.committed.unwrap().0;

        let mut source = source();
        let mut market = market();
        drive_sharded(&mut source, &mut market, request()).unwrap();
        let sharded_listing = market.receipt.unwrap().listing;
        assert_eq!(local_listing, sharded_listing);

        let expire = |listing: PreparedListing, highest_bidder_guid, highest_bid| {
            let mut store = FakeExpiry {
                auction: Some(ActiveAuction {
                    id: 41,
                    listing,
                    highest_bidder_guid,
                    highest_bid,
                }),
                schedule_count: 1,
                mail: Vec::new(),
                notices: Vec::new(),
                returned_items: Vec::new(),
                refunded_copper: 0,
            };
            expire_active(&mut store, 41).unwrap();
            store
        };
        for (highest_bidder_guid, highest_bid) in [(0, 0), (8, 201)] {
            let local_expiry = expire(local_listing.clone(), highest_bidder_guid, highest_bid);
            let sharded_expiry = expire(sharded_listing.clone(), highest_bidder_guid, highest_bid);
            assert_eq!(local_expiry.mail, sharded_expiry.mail);
            assert_eq!(local_expiry.returned_items, sharded_expiry.returned_items);
            assert_eq!(local_expiry.refunded_copper, sharded_expiry.refunded_copper);
            assert_eq!(local_expiry.schedule_count, sharded_expiry.schedule_count);
        }
    }

    #[test]
    fn replayed_bid_expiry_mails_the_exact_item_and_imported_rate_proceeds_once() {
        let listing = PreparedListing {
            request: request(),
            snapshot: item(23).snapshot,
            deposit: 10,
            created_micros: 1_000,
            expires_micros: 43_200_001_000,
        };
        let mut store = FakeExpiry {
            auction: Some(ActiveAuction {
                id: 41,
                listing,
                highest_bidder_guid: 8,
                highest_bid: 201,
            }),
            schedule_count: 1,
            mail: Vec::new(),
            notices: Vec::new(),
            returned_items: Vec::new(),
            refunded_copper: 0,
        };

        assert_eq!(expire_active(&mut store, 41), Ok(()));
        assert_eq!(expire_active(&mut store, 41), Ok(()));

        assert!(store.auction.is_none());
        assert_eq!(store.schedule_count, 0);
        assert_eq!(store.returned_items, Vec::new());
        assert_eq!(
            store.mail,
            vec![
                AuctionMail {
                    recipient_guid: 8,
                    house: 1,
                    action: AuctionMailAction::Won,
                    item_entry: 25,
                    random_property_id: 117,
                    money: 0,
                    attached_item: item(23).snapshot,
                    counterparty_guid: 7,
                    bid: 201,
                    buyout: 20,
                    deposit: 0,
                    cut: 0,
                },
                AuctionMail {
                    recipient_guid: 7,
                    house: 1,
                    action: AuctionMailAction::Successful,
                    item_entry: 25,
                    random_property_id: 117,
                    money: 201,
                    attached_item: crate::items::ItemSnapshot::default(),
                    counterparty_guid: 8,
                    bid: 201,
                    buyout: 20,
                    deposit: 10,
                    cut: 10,
                },
            ],
            "one Sold mail settles at the LISTING's buyout term, not the expiring bid"
        );
        assert_eq!(
            store.notices,
            vec![
                AuctionNoticeDraft {
                    recipient_guid: 8,
                    kind: auction_notice::WON,
                    house: 1,
                    auction_id: 41,
                    item_entry: 25,
                    random_property_id: 117,
                    bid: 0,
                    out_bid: 11, // vanilla minimum raise on a 201 bid: 5% rounded up
                    bidder_guid: 8,
                },
                AuctionNoticeDraft {
                    recipient_guid: 7,
                    kind: auction_notice::SOLD,
                    house: 1,
                    auction_id: 41,
                    item_entry: 25,
                    random_property_id: 117,
                    bid: 201,
                    out_bid: 11, // vanilla minimum raise on a 201 bid: 5% rounded up
                    bidder_guid: 0,
                },
            ],
            "a replayed expiry callback must not write a second notice"
        );
    }

    #[test]
    fn realm_bid_decision_enforces_the_vanilla_price_ladder_and_revision() {
        let active = active_bid_auction();
        let request = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 900,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 100,
        };

        assert_eq!(
            decide_bid(Some(active), request, 1_000),
            accepted_active(100, 4, 0, 0)
        );

        let bid = BidAuction {
            highest_bidder_guid: 9,
            highest_bid: 101,
            ..active
        };
        assert_eq!(minimum_next_bid(bid), Ok(107));
        assert_eq!(
            decide_bid(
                Some(bid),
                HoldRequest {
                    offer: 101,
                    ..request
                },
                1_000
            ),
            HoldDecision::HigherBid {
                bidder_guid: 9,
                current_bid: 101,
                minimum_increment: 6,
            }
        );
        assert_eq!(
            decide_bid(
                Some(bid),
                HoldRequest {
                    offer: 106,
                    ..request
                },
                1_000
            ),
            HoldDecision::BidIncrement
        );
        assert_eq!(
            decide_bid(
                Some(active),
                HoldRequest {
                    offer: 99,
                    ..request
                },
                1_000
            ),
            HoldDecision::BidIncrement
        );
        assert_eq!(
            decide_bid(
                Some(BidAuction {
                    owner_guid: 8,
                    ..active
                }),
                request,
                1_000,
            ),
            HoldDecision::BidOwn
        );
        assert_eq!(decide_bid(None, request, 1_000), HoldDecision::ItemNotFound);
        assert_eq!(
            decide_bid(
                Some(active),
                HoldRequest {
                    house: 7,
                    ..request
                },
                1_000,
            ),
            HoldDecision::ItemNotFound
        );
        assert_eq!(
            decide_bid(
                Some(BidAuction {
                    expires_micros: 1_000,
                    ..active
                }),
                request,
                1_000,
            ),
            HoldDecision::ItemNotFound
        );
        assert_eq!(
            decide_bid(
                Some(active),
                HoldRequest {
                    operation_id: 0,
                    ..request
                },
                1_000
            ),
            HoldDecision::Database
        );
        assert!(matches!(
            decide_bid(
                Some(BidAuction {
                    highest_bid: u32::MAX,
                    buyout: 0,
                    ..bid
                }),
                request,
                1_000,
            ),
            HoldDecision::Database
        ));
    }

    /// The seven imported houses pool into three markets: a bid through any Alliance house
    /// reaches a listing at any Alliance house, but never a Horde or neutral one.
    #[test]
    fn realm_bid_decision_accepts_a_bid_within_the_listings_market_and_refuses_outside_it() {
        let active = active_bid_auction(); // lists in house 1 (Stormwind, Alliance).
        let request = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 900,
            bidder_guid: 8,
            auction_id: 41,
            house: 2, // a different Alliance house.
            offer: 100,
        };

        assert_eq!(
            decide_bid(Some(active), request, 1_000),
            accepted_active(100, 4, 0, 0)
        );
        assert_eq!(
            decide_bid(
                Some(active),
                HoldRequest {
                    house: 3,
                    ..request
                },
                1_000
            ),
            accepted_active(100, 4, 0, 0)
        );
        for horde_house in [4, 5, 6] {
            assert_eq!(
                decide_bid(
                    Some(active),
                    HoldRequest {
                        house: horde_house,
                        ..request
                    },
                    1_000
                ),
                HoldDecision::ItemNotFound
            );
        }
        assert_eq!(
            decide_bid(
                Some(active),
                HoldRequest {
                    house: 7,
                    ..request
                },
                1_000
            ),
            HoldDecision::ItemNotFound
        );
    }

    #[test]
    fn realm_buyout_normalizes_the_offer_and_checks_settlement_arithmetic() {
        let active = BidAuction {
            highest_bidder_guid: 9,
            highest_bid: 201,
            buyout: 500,
            ..active_bid_auction()
        };
        let request = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 905,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 900,
        };

        for offer in [500, 900] {
            assert_eq!(
                decide_bid(Some(active), HoldRequest { offer, ..request }, 1_000),
                accepted_buyout(500, 9, 201)
            );
        }
        assert_eq!(
            decide_bid(
                Some(BidAuction {
                    deposit: u32::MAX,
                    buyout: u32::MAX,
                    ..active
                }),
                HoldRequest {
                    offer: u32::MAX,
                    ..request
                },
                1_000,
            ),
            HoldDecision::Database
        );
        assert!(matches!(
            decide_bid(
                Some(BidAuction {
                    highest_bidder_guid: 9,
                    highest_bid: u32::MAX - 1,
                    buyout: u32::MAX,
                    deposit: 1,
                    ..active
                }),
                HoldRequest {
                    offer: u32::MAX,
                    ..request
                },
                1_000,
            ),
            HoldDecision::Accepted(BidAcceptance {
                price: u32::MAX,
                effect: AuctionBidEffect::SettleBuyout,
                ..
            })
        ));
    }

    #[derive(Clone, Copy)]
    struct FakeBidSource {
        money: u32,
        deferred_refund: u32,
        hold: Option<HeldBid>,
    }

    impl FakeBidSource {
        fn new(money: u32) -> Self {
            Self {
                money,
                deferred_refund: 0,
                hold: None,
            }
        }
    }

    impl BidSource for FakeBidSource {
        fn money(&self, _bidder_guid: u64) -> Option<u32> {
            Some(self.money)
        }

        fn hold(&self, operation_id: u64) -> Option<HeldBid> {
            self.hold
                .filter(|hold| hold.request.operation_id == operation_id)
        }

        fn create_hold(&mut self, request: HoldRequest) -> Result<(), AuctionRefusal> {
            self.money -= request.offer;
            self.hold = Some(HeldBid {
                request,
                decision: None,
                deferred_refund: 0,
            });
            Ok(())
        }

        fn finish_hold(
            &mut self,
            request: HoldRequest,
            decision: HoldDecision,
        ) -> Result<(), AuctionRefusal> {
            let refund =
                refundable_bid_value(decision, request.offer).ok_or(AuctionRefusal::Database)?;
            if refund != 0 {
                let (money, deferred_refund) = crate::mail::split_refund(self.money, refund);
                self.money = money;
                self.deferred_refund += deferred_refund;
            }
            self.hold = Some(HeldBid {
                request,
                decision: Some(decision),
                deferred_refund: self.deferred_refund,
            });
            Ok(())
        }

        fn confirm_refund(&mut self, request: HoldRequest) -> Result<(), AuctionRefusal> {
            self.deferred_refund = 0;
            self.hold = self.hold.map(|mut hold| {
                assert_eq!(hold.request, request);
                hold.deferred_refund = 0;
                hold
            });
            Ok(())
        }
    }

    #[test]
    fn bid_hold_decisions_are_terminal_replay_safe_and_conserve_copper() {
        let request = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 901,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 107,
        };
        let rejected = HoldDecision::BidIncrement;
        let accepted = accepted_active(107, 4, 9, 101);

        let mut rejection = FakeBidSource::new(200);
        assert_eq!(fence_bid(&mut rejection, request), Ok(()));
        assert_eq!(rejection.money, 93, "the full offer is held first");
        assert_eq!(finish_bid(&mut rejection, request, rejected), Ok(rejected));
        assert_eq!(rejection.money, 200, "a rejection restores the offer");
        assert_eq!(finish_bid(&mut rejection, request, rejected), Ok(rejected));
        assert_eq!(rejection.money, 200, "replay cannot restore twice");

        let mut overflowed_rejection = FakeBidSource::new(200);
        assert_eq!(fence_bid(&mut overflowed_rejection, request), Ok(()));
        overflowed_rejection.money = u32::MAX;
        assert_eq!(
            finish_bid(&mut overflowed_rejection, request, rejected),
            Ok(rejected),
            "an intervening purse credit cannot strand a rejected Hold"
        );
        assert_eq!(overflowed_rejection.money, u32::MAX);
        assert_eq!(overflowed_rejection.deferred_refund, request.offer);
        assert_eq!(
            finish_bid(&mut overflowed_rejection, request, rejected),
            Ok(rejected)
        );
        assert_eq!(
            overflowed_rejection.deferred_refund, request.offer,
            "replay cannot defer the refund twice"
        );
        assert_eq!(
            confirm_bid_refund(&mut overflowed_rejection, request, request.offer),
            Ok(())
        );
        assert_eq!(overflowed_rejection.deferred_refund, 0);
        assert_eq!(
            confirm_bid_refund(&mut overflowed_rejection, request, request.offer),
            Ok(()),
            "confirmation replay is idempotent"
        );

        let mut acceptance = FakeBidSource::new(200);
        assert_eq!(fence_bid(&mut acceptance, request), Ok(()));
        assert_eq!(finish_bid(&mut acceptance, request, accepted), Ok(accepted));
        assert_eq!(finish_bid(&mut acceptance, request, accepted), Ok(accepted));
        assert_eq!(acceptance.money, 93, "accepted value is consumed once");

        let normalized = accepted_active(100, 4, 9, 101);
        let mut normalized_acceptance = FakeBidSource::new(200);
        assert_eq!(fence_bid(&mut normalized_acceptance, request), Ok(()));
        assert_eq!(
            finish_bid(&mut normalized_acceptance, request, normalized),
            Ok(normalized)
        );
        assert_eq!(normalized_acceptance.money, 100);
        assert_eq!(
            finish_bid(&mut normalized_acceptance, request, normalized),
            Ok(normalized)
        );
        assert_eq!(normalized_acceptance.money, 100);

        let mut interrupted_normalization = FakeBidSource::new(200);
        fence_bid(&mut interrupted_normalization, request).unwrap();
        interrupted_normalization.money = u32::MAX;
        finish_bid(&mut interrupted_normalization, request, normalized).unwrap();
        assert_eq!(interrupted_normalization.deferred_refund, 7);
        assert_eq!(
            confirm_bid_refund(&mut interrupted_normalization, request, 7),
            Ok(())
        );
        assert_eq!(interrupted_normalization.deferred_refund, 0);

        assert_eq!(
            finish_bid(
                &mut acceptance,
                HoldRequest {
                    offer: 108,
                    ..request
                },
                accepted
            ),
            Err(AuctionRefusal::Database),
            "changed-payload identifier reuse fails closed"
        );

        for malformed in [
            HoldRequest {
                operation_id: 0,
                ..request
            },
            HoldRequest {
                bidder_guid: 0,
                ..request
            },
            HoldRequest {
                auction_id: 0,
                ..request
            },
            HoldRequest {
                offer: 0,
                ..request
            },
        ] {
            let mut source = FakeBidSource::new(200);
            assert_eq!(
                fence_bid(&mut source, malformed),
                Err(AuctionRefusal::Database)
            );
            assert_eq!(source.money, 200);
            assert!(source.hold.is_none());
        }
        let mut poor = FakeBidSource::new(106);
        assert_eq!(
            fence_bid(&mut poor, request),
            Err(AuctionRefusal::NotEnoughMoney)
        );
        assert_eq!(poor.money, 106);
        assert!(poor.hold.is_none());
    }

    #[derive(Clone)]
    struct FakeBidMarket {
        auction: Option<BidAuction>,
        decisions: Vec<(HoldRequest, HoldDecision)>,
        mail: Vec<AuctionMail>,
        notices: Vec<AuctionNoticeDraft>,
        expiry_armed: bool,
        now_micros: i64,
    }

    impl FakeBidMarket {
        fn mailbox(&self, recipient_guid: u64) -> Vec<AuctionMail> {
            self.mail
                .iter()
                .copied()
                .filter(|mail| mail.recipient_guid == recipient_guid)
                .collect()
        }
    }

    impl BidMarket for FakeBidMarket {
        fn decision(&self, operation_id: u64) -> Option<(HoldRequest, HoldDecision)> {
            self.decisions
                .iter()
                .copied()
                .find(|(request, _)| request.operation_id == operation_id)
        }

        fn auction(&self, auction_id: u32) -> Option<BidAuction> {
            self.auction.filter(|auction| auction.id == auction_id)
        }

        fn now_micros(&self) -> i64 {
            self.now_micros
        }

        fn commit_decision(
            &mut self,
            request: HoldRequest,
            auction: Option<BidAuction>,
            decision: HoldDecision,
        ) -> Result<(), AuctionRefusal> {
            if let HoldDecision::Accepted(accepted) = decision {
                let mut auction = auction.expect("only an active Auction can accept a bid");
                self.mail.extend(displaced_bid_refund_mail(
                    auction.house,
                    auction.item,
                    accepted.displaced_bidder_guid,
                    accepted.displaced_bid,
                ));
                self.notices.extend(outbid_notice(
                    auction.house,
                    request.auction_id,
                    auction.item,
                    accepted.displaced_bidder_guid,
                    request.bidder_guid,
                    accepted.displaced_bid,
                ));
                match accepted.effect {
                    AuctionBidEffect::SettleBuyout => {
                        self.expiry_armed = false;
                        self.mail.extend(
                            buyout_settlement_mail(auction, request.bidder_guid, accepted.price)
                                .expect("accepted buyout arithmetic was checked"),
                        );
                        self.notices.extend(settlement_notices(
                            auction.house,
                            request.auction_id,
                            auction.item.entry,
                            auction.item.random_property_id,
                            auction.owner_guid,
                            request.bidder_guid,
                            accepted.price,
                        ));
                        self.auction = None;
                    }
                    AuctionBidEffect::RemainActive { revision } => {
                        self.notices.push(new_bid_notice(
                            auction.house,
                            request.auction_id,
                            auction.item,
                            auction.owner_guid,
                            request.bidder_guid,
                            accepted.price,
                        ));
                        auction.highest_bidder_guid = request.bidder_guid;
                        auction.highest_bid = accepted.price;
                        auction.revision = revision;
                        self.auction = Some(auction);
                    }
                }
            }
            if let HoldDecision::Cancelled {
                displaced_bidder_guid,
                displaced_bid,
                ..
            } = decision
            {
                let auction = auction.expect("only an active Auction can be cancelled");
                self.mail.extend(cancellation_mail(
                    auction,
                    displaced_bidder_guid,
                    displaced_bid,
                ));
                self.notices.extend(removed_notice(
                    auction.house,
                    auction.id,
                    auction.item,
                    displaced_bidder_guid,
                    displaced_bid,
                ));
                self.auction = None;
                self.expiry_armed = false;
            }
            self.decisions.push((request, decision));
            Ok(())
        }
    }

    struct BidMarketExpiry<'a> {
        market: &'a mut FakeBidMarket,
        listing: PreparedListing,
    }

    impl ExpirySink for BidMarketExpiry<'_> {
        fn auction(&self, auction_id: u32) -> Result<Option<ActiveAuction>, String> {
            Ok(self
                .market
                .auction
                .filter(|auction| auction.id == auction_id)
                .map(|auction| ActiveAuction {
                    id: auction.id,
                    listing: self.listing.clone(),
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
                    self.market
                        .notices
                        .push(expired_notice(house, auction.id, item, owner_guid));
                    self.market.mail.push(mail);
                }
                ExpiryCompletion::Sold(mail) => {
                    self.market.notices.extend(settlement_notices(
                        house,
                        auction.id,
                        item.entry,
                        item.random_property_id,
                        owner_guid,
                        auction.highest_bidder_guid,
                        auction.highest_bid,
                    ));
                    self.market.mail.extend(mail);
                }
            }
            self.market.auction = None;
            self.market.expiry_armed = false;
        }
    }

    fn expire_bid_market(
        market: &mut FakeBidMarket,
        listing: PreparedListing,
    ) -> Result<(), String> {
        let auction_id = market.auction.map_or(41, |auction| auction.id);
        expire_active(&mut BidMarketExpiry { market, listing }, auction_id)
    }

    struct FakeBidRefundSink {
        request: HoldRequest,
        decision: HoldDecision,
        recorded: u32,
        mails: Vec<(u64, u32)>,
    }

    impl BidRefundSink for FakeBidRefundSink {
        fn refund_decision(&self, operation_id: u64) -> Option<(HoldRequest, HoldDecision, u32)> {
            (self.request.operation_id == operation_id).then_some((
                self.request,
                self.decision,
                self.recorded,
            ))
        }

        fn commit_refund(
            &mut self,
            request: HoldRequest,
            amount: u32,
        ) -> Result<(), AuctionRefusal> {
            self.recorded = amount;
            self.mails.push((request.bidder_guid, amount));
            Ok(())
        }
    }

    #[test]
    fn deferred_bid_refund_relay_is_terminal_and_payload_safe() {
        let request = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 904,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 107,
        };
        let mut sink = FakeBidRefundSink {
            request,
            decision: HoldDecision::BidIncrement,
            recorded: 0,
            mails: Vec::new(),
        };

        assert_eq!(relay_bid_refund(&mut sink, request, 7), Ok(()));
        assert_eq!(relay_bid_refund(&mut sink, request, 7), Ok(()));
        assert_eq!(sink.mails, vec![(8, 7)]);
        assert_eq!(
            relay_bid_refund(
                &mut sink,
                HoldRequest {
                    offer: 108,
                    ..request
                },
                7
            ),
            Err(AuctionRefusal::Database)
        );
        assert_eq!(
            relay_bid_refund(&mut sink, request, 8),
            Err(AuctionRefusal::Database)
        );

        sink.recorded = 0;
        sink.decision = accepted_active(100, 4, 9, 101);
        assert_eq!(relay_bid_refund(&mut sink, request, 7), Ok(()));
        assert_eq!(relay_bid_refund(&mut sink, request, 7), Ok(()));
        assert_eq!(sink.mails, vec![(8, 7), (8, 7)]);
        assert_eq!(
            relay_bid_refund(&mut sink, request, 8),
            Err(AuctionRefusal::Database)
        );
    }

    #[test]
    fn realm_bid_replay_updates_once_and_returns_the_displaced_bid_once() {
        let request = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 902,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 107,
        };
        let mut market = FakeBidMarket {
            auction: Some(BidAuction {
                highest_bidder_guid: 9,
                highest_bid: 101,
                ..active_bid_auction()
            }),
            decisions: Vec::new(),
            mail: Vec::new(),
            notices: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };

        let expected = accepted_active(107, 4, 9, 101);
        assert_eq!(resolve_bid(&mut market, request), Ok(expected));
        assert_eq!(resolve_bid(&mut market, request), Ok(expected));
        assert_eq!(
            market.mail,
            vec![displaced_bid_refund_mail(1, item(23).snapshot, 9, 101).unwrap()]
        );
        assert_eq!(
            market.notices,
            vec![
                outbid_notice(1, 41, item(23).snapshot, 9, 8, 101).unwrap(),
                new_bid_notice(1, 41, item(23).snapshot, 7, 8, 107),
            ],
            "the replayed decision must not write a second Outbid or New bid notice"
        );
        assert_eq!(
            market.auction.map(|auction| (
                auction.highest_bidder_guid,
                auction.highest_bid,
                auction.revision,
            )),
            Some((8, 107, 4))
        );

        assert_eq!(
            resolve_bid(
                &mut market,
                HoldRequest {
                    offer: 108,
                    ..request
                }
            ),
            Err(AuctionRefusal::Database)
        );
        assert_eq!(
            market.mail,
            vec![displaced_bid_refund_mail(1, item(23).snapshot, 9, 101).unwrap()]
        );
    }

    #[test]
    fn local_and_interrupted_sharded_bids_and_buyouts_have_equivalent_state() {
        for (operation_id, offer) in [(903, 107), (904, 900)] {
            let request = HoldRequest {
                operation: HoldOperation::Bid,
                operation_id,
                bidder_guid: 8,
                auction_id: 41,
                house: 1,
                offer,
            };
            let source = FakeBidSource::new(1_000);
            let market = FakeBidMarket {
                auction: Some(BidAuction {
                    highest_bidder_guid: 9,
                    highest_bid: 101,
                    buyout: 500,
                    ..active_bid_auction()
                }),
                decisions: Vec::new(),
                mail: Vec::new(),
                notices: Vec::new(),
                expiry_armed: true,
                now_micros: 1_000,
            };

            let mut local_source = source;
            let mut local_market = market.clone();
            let expected = drive_bid(&mut local_source, &mut local_market, request).unwrap();

            for killed_after in 0..=2 {
                let mut sharded_source = source;
                let mut sharded_market = market.clone();
                if killed_after >= 1 {
                    fence_bid(&mut sharded_source, request).unwrap();
                }
                if killed_after >= 2 {
                    resolve_bid(&mut sharded_market, request).unwrap();
                }

                assert_eq!(
                    drive_bid(&mut sharded_source, &mut sharded_market, request),
                    Ok(expected)
                );
                assert_eq!(sharded_source.money, local_source.money);
                assert_eq!(sharded_source.hold, local_source.hold);
                assert_eq!(sharded_market.auction, local_market.auction);
                assert_eq!(sharded_market.decisions, local_market.decisions);
                assert_eq!(sharded_market.mail, local_market.mail);
                assert_eq!(sharded_market.expiry_armed, local_market.expiry_armed);
                assert_eq!(
                    drive_bid(&mut sharded_source, &mut sharded_market, request),
                    Ok(expected)
                );
                assert_eq!(sharded_market.decisions, local_market.decisions);
                assert_eq!(sharded_market.mail, local_market.mail);
            }
        }
    }

    fn listing_for_flow(sharded: bool, operation_id: u64) -> PreparedListing {
        let request = ListingRequest {
            operation_id,
            ..request()
        };
        if sharded {
            let mut source = source();
            let mut market = market();
            fence_listing(&mut source, request).unwrap();
            assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
            assert_eq!(drive_sharded(&mut source, &mut market, request), Ok(41));
            market.receipt.unwrap().listing
        } else {
            let mut store = local();
            assert_eq!(create_local_listing(&mut store, request), Ok(41));
            assert_eq!(create_local_listing(&mut store, request), Ok(41));
            store.committed.unwrap().0
        }
    }

    fn bid_market_for(listing: &PreparedListing) -> FakeBidMarket {
        FakeBidMarket {
            auction: Some(BidAuction {
                id: 41,
                house: listing.request.house.id,
                owner_guid: listing.request.seller_guid,
                item: listing.snapshot,
                highest_bidder_guid: 0,
                highest_bid: 0,
                start_bid: listing.request.terms.start_bid,
                buyout: listing.request.terms.buyout,
                deposit: listing.deposit,
                consignment_rate: listing.request.house.consignment_rate,
                expires_micros: listing.expires_micros,
                revision: 0,
            }),
            decisions: Vec::new(),
            mail: Vec::new(),
            notices: Vec::new(),
            expiry_armed: true,
            now_micros: listing.created_micros,
        }
    }

    fn bid_for_flow(
        sharded: bool,
        source: &mut FakeBidSource,
        market: &mut FakeBidMarket,
        request: HoldRequest,
    ) -> HoldDecision {
        if sharded {
            fence_bid(source, request).unwrap();
            resolve_bid(market, request).unwrap();
        }
        let decision = drive_bid(source, market, request).unwrap();
        assert_eq!(drive_bid(source, market, request), Ok(decision));
        decision
    }

    #[derive(Debug, PartialEq, Eq)]
    struct CompleteFlowOutcome {
        browse_search_rows: Vec<BidAuction>,
        decisions: Vec<HoldDecision>,
        collected_mail: Vec<AuctionMail>,
        collected_copper: Vec<(u64, u32)>,
        collected_items: Vec<(u64, crate::items::ItemSnapshot)>,
        bidder_money: Vec<u32>,
    }

    fn collect_flow_mail(outcome: &mut CompleteFlowOutcome, mail: Vec<AuctionMail>) {
        for mail in mail {
            if mail.money != 0 {
                assert_eq!(
                    crate::mail::plan_take_money(
                        Some((mail.recipient_guid, mail.money)),
                        mail.recipient_guid,
                    ),
                    crate::mail::TakeMoney::Take(mail.money),
                );
                assert_eq!(
                    crate::mail::plan_take_money(
                        Some((mail.recipient_guid, 0)),
                        mail.recipient_guid,
                    ),
                    crate::mail::TakeMoney::NothingToTake,
                );
                outcome
                    .collected_copper
                    .push((mail.recipient_guid, crate::mail::credited(0, mail.money)));
            }
            if !mail.attached_item.is_empty() {
                assert_eq!(
                    crate::mail::plan_take_item(
                        Some((mail.recipient_guid, mail.attached_item.entry)),
                        mail.recipient_guid,
                    ),
                    crate::mail::TakeItem::Take,
                );
                assert_eq!(
                    crate::mail::plan_take_item(
                        Some((mail.recipient_guid, 0)),
                        mail.recipient_guid,
                    ),
                    crate::mail::TakeItem::NothingToTake,
                );
                outcome
                    .collected_items
                    .push((mail.recipient_guid, mail.attached_item));
            }
            outcome.collected_mail.push(mail);
        }
    }

    fn complete_flow(sharded: bool) -> CompleteFlowOutcome {
        let buyout_listing = listing_for_flow(sharded, 910);
        let mut buyout_market = bid_market_for(&buyout_listing);
        // Browse/search renders the same authoritative Auction row that the later writes consume.
        let browse_search_rows = buyout_market
            .auction
            .into_iter()
            .filter(|auction| auction.item.entry == item(23).snapshot.entry)
            .collect();
        let bid = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 911,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 10,
        };
        let mut bidder = FakeBidSource::new(100);
        let bid_decision = bid_for_flow(sharded, &mut bidder, &mut buyout_market, bid);
        let buyout = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 912,
            bidder_guid: 9,
            auction_id: 41,
            house: 1,
            offer: 50,
        };
        let mut buyer = FakeBidSource::new(100);
        let buyout_decision = bid_for_flow(sharded, &mut buyer, &mut buyout_market, buyout);
        expire_bid_market(&mut buyout_market, buyout_listing.clone()).unwrap();
        let mut outcome = CompleteFlowOutcome {
            browse_search_rows,
            decisions: vec![bid_decision, buyout_decision],
            collected_mail: Vec::new(),
            collected_copper: Vec::new(),
            collected_items: Vec::new(),
            bidder_money: vec![bidder.money, buyer.money],
        };
        collect_flow_mail(&mut outcome, std::mem::take(&mut buyout_market.mail));

        let unbid_listing = listing_for_flow(sharded, 913);
        let mut unbid_market = bid_market_for(&unbid_listing);
        expire_bid_market(&mut unbid_market, unbid_listing.clone()).unwrap();
        expire_bid_market(&mut unbid_market, unbid_listing).unwrap();
        collect_flow_mail(&mut outcome, std::mem::take(&mut unbid_market.mail));

        let bid_expiry_listing = listing_for_flow(sharded, 914);
        let mut bid_expiry_market = bid_market_for(&bid_expiry_listing);
        let expiry_bid = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 915,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 10,
        };
        let mut expiry_bidder = FakeBidSource::new(100);
        let expiry_bid_decision = bid_for_flow(
            sharded,
            &mut expiry_bidder,
            &mut bid_expiry_market,
            expiry_bid,
        );
        expire_bid_market(&mut bid_expiry_market, bid_expiry_listing.clone()).unwrap();
        expire_bid_market(&mut bid_expiry_market, bid_expiry_listing).unwrap();
        collect_flow_mail(&mut outcome, std::mem::take(&mut bid_expiry_market.mail));
        outcome.decisions.push(expiry_bid_decision);
        outcome.bidder_money.push(expiry_bidder.money);
        outcome
    }

    #[test]
    fn complete_flow_is_equivalent_across_local_and_interrupted_sharded_topologies() {
        let local = complete_flow(false);
        let sharded = complete_flow(true);
        assert_eq!(sharded, local);
        assert_eq!(local.browse_search_rows.len(), 1);
        assert_eq!(local.collected_mail.len(), 6);
        assert_eq!(
            local
                .collected_mail
                .iter()
                .map(|mail| (mail.recipient_guid, mail.action, mail.money))
                .collect::<Vec<_>>(),
            vec![
                (8, AuctionMailAction::Outbid, 10),
                (9, AuctionMailAction::Won, 0),
                (7, AuctionMailAction::Successful, 29),
                (7, AuctionMailAction::Expired, 0),
                (8, AuctionMailAction::Won, 0),
                (7, AuctionMailAction::Successful, 20),
            ]
        );
        assert_eq!(
            local.collected_items,
            vec![
                (9, item(23).snapshot),
                (7, item(23).snapshot),
                (8, item(23).snapshot),
            ],
            "buyout, unbid expiry, and bid expiry each collect one exact item"
        );
        assert_eq!(local.collected_copper, vec![(8, 10), (7, 29), (7, 20)]);
        assert_eq!(local.bidder_money, vec![90, 80, 90]);
    }

    #[test]
    fn buyout_atomically_delivers_exact_mail_and_conserves_copper() {
        let request = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 906,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 900,
        };
        let mut source = FakeBidSource::new(1_000);
        let mut market = FakeBidMarket {
            auction: Some(BidAuction {
                highest_bidder_guid: 9,
                highest_bid: 201,
                buyout: 500,
                ..active_bid_auction()
            }),
            decisions: Vec::new(),
            mail: Vec::new(),
            notices: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };

        assert_eq!(
            drive_bid(&mut source, &mut market, request),
            Ok(accepted_buyout(500, 9, 201))
        );
        assert_eq!(source.money, 500);
        assert!(market.auction.is_none());
        assert!(!market.expiry_armed);
        assert_eq!(
            market.mailbox(9),
            vec![displaced_bid_refund_mail(1, item(23).snapshot, 9, 201).unwrap()]
        );
        let winner_mail = market.mailbox(8);
        assert_eq!(winner_mail.len(), 1, "winner mail is visible immediately");
        assert_eq!(winner_mail[0].action, AuctionMailAction::Won);
        assert_eq!(winner_mail[0].counterparty_guid, 7);
        assert_eq!(winner_mail[0].money, 0);
        assert_eq!(winner_mail[0].attached_item, item(23).snapshot);
        let seller_mail = market.mailbox(7);
        assert_eq!(seller_mail.len(), 1, "seller mail is visible immediately");
        assert_eq!(seller_mail[0].action, AuctionMailAction::Successful);
        assert_eq!(seller_mail[0].counterparty_guid, 8);
        assert_eq!(seller_mail[0].money, 485);
        assert!(seller_mail[0].attached_item.is_empty());
        assert_eq!(market.mail.len(), 3);
        assert_eq!(1_000_u64 + 201 + 10, 500_u64 + 201 + 485 + 25);
        let [won_notice, sold_notice] = settlement_notices(1, 41, 25, 117, 7, 8, 500);
        assert_eq!(
            market.notices,
            vec![
                outbid_notice(1, 41, item(23).snapshot, 9, 8, 201).unwrap(),
                won_notice,
                sold_notice,
            ],
            "SettleBuyout writes Outbid, then Won and Sold — never New bid"
        );

        assert_eq!(
            drive_bid(&mut source, &mut market, request),
            Ok(accepted_buyout(500, 9, 201))
        );
        assert_eq!(source.money, 500);
        assert_eq!(market.mail.len(), 3);
        assert_eq!(
            market.notices.len(),
            3,
            "a replayed buyout writes no second notice"
        );
    }

    #[test]
    fn concurrent_buyouts_have_one_winner_and_restore_the_loser_once() {
        let winner = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 907,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 900,
        };
        let loser = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 908,
            bidder_guid: 10,
            auction_id: 41,
            house: 1,
            offer: 500,
        };
        let mut winner_source = FakeBidSource::new(1_000);
        let mut loser_source = FakeBidSource::new(700);
        let mut market = FakeBidMarket {
            auction: Some(BidAuction {
                buyout: 500,
                ..active_bid_auction()
            }),
            decisions: Vec::new(),
            mail: Vec::new(),
            notices: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };

        fence_bid(&mut winner_source, winner).unwrap();
        fence_bid(&mut loser_source, loser).unwrap();
        let winner_decision = resolve_bid(&mut market, winner).unwrap();
        let loser_decision = resolve_bid(&mut market, loser).unwrap();
        assert!(matches!(
            winner_decision,
            HoldDecision::Accepted(BidAcceptance {
                price: 500,
                effect: AuctionBidEffect::SettleBuyout,
                ..
            })
        ));
        assert_eq!(loser_decision, HoldDecision::ItemNotFound);
        finish_bid(&mut winner_source, winner, winner_decision).unwrap();
        finish_bid(&mut loser_source, loser, loser_decision).unwrap();
        assert_eq!(winner_source.money, 500);
        assert_eq!(loser_source.money, 700);
        assert_eq!(market.mail.len(), 2);
        assert_eq!(market.decisions.len(), 2);

        assert_eq!(
            drive_bid(&mut loser_source, &mut market, loser),
            Ok(HoldDecision::ItemNotFound)
        );
        assert_eq!(loser_source.money, 700);
        assert_eq!(market.mail.len(), 2);
        assert_eq!(market.decisions.len(), 2);
    }

    #[test]
    fn expiry_and_buyout_race_in_either_order_completes_once_and_conserves_value() {
        let listing = PreparedListing {
            request: request(),
            snapshot: item(23).snapshot,
            deposit: 10,
            created_micros: 1_000,
            expires_micros: 43_200_001_000,
        };
        let bid_auction = BidAuction {
            highest_bidder_guid: 9,
            highest_bid: 201,
            buyout: 500,
            ..active_bid_auction()
        };
        let buyout = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 909,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 500,
        };

        let mut buyout_source = FakeBidSource::new(700);
        let mut buyout_market = FakeBidMarket {
            auction: Some(bid_auction),
            decisions: Vec::new(),
            mail: Vec::new(),
            notices: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };
        assert_eq!(
            drive_bid(&mut buyout_source, &mut buyout_market, buyout),
            Ok(accepted_buyout(500, 9, 201))
        );
        let buyout_mail = buyout_market.mail.clone();
        expire_bid_market(&mut buyout_market, listing.clone()).unwrap();
        expire_bid_market(&mut buyout_market, listing.clone()).unwrap();
        assert_eq!(buyout_market.mail, buyout_mail);
        assert_eq!(
            u64::from(buyout_source.money)
                + buyout_market
                    .mail
                    .iter()
                    .map(|mail| u64::from(mail.money))
                    .sum::<u64>()
                + 25,
            700 + 201 + 10,
            "the post-buyout purse, refund, proceeds, and cut account for all copper"
        );
        assert_eq!(
            buyout_market
                .mail
                .iter()
                .filter(|mail| !mail.attached_item.is_empty())
                .count(),
            1
        );

        let mut expiry_market = FakeBidMarket {
            auction: Some(bid_auction),
            decisions: Vec::new(),
            mail: Vec::new(),
            notices: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        };
        expire_bid_market(&mut expiry_market, listing.clone()).unwrap();
        expire_bid_market(&mut expiry_market, listing).unwrap();
        let expiry_mail = expiry_market.mail.clone();
        let mut late_buyout_source = FakeBidSource::new(700);
        assert_eq!(
            drive_bid(&mut late_buyout_source, &mut expiry_market, buyout),
            Ok(HoldDecision::ItemNotFound)
        );
        assert_eq!(
            drive_bid(&mut late_buyout_source, &mut expiry_market, buyout),
            Ok(HoldDecision::ItemNotFound)
        );
        assert_eq!(
            late_buyout_source.money, 700,
            "the losing Hold is restored once"
        );
        assert_eq!(expiry_market.mail, expiry_mail);
        assert_eq!(
            u64::from(late_buyout_source.money)
                + expiry_mail
                    .iter()
                    .map(|mail| u64::from(mail.money))
                    .sum::<u64>()
                + 10,
            700 + 201 + 10,
            "the post-expiry purse, proceeds, and cut account for all copper"
        );
        assert_eq!(
            expiry_mail
                .iter()
                .filter(|mail| !mail.attached_item.is_empty())
                .count(),
            1
        );
    }

    #[test]
    fn expiry_preserves_inconsistent_or_overflowing_sale_state_for_repair() {
        let listing = PreparedListing {
            request: request(),
            snapshot: item(23).snapshot,
            deposit: 10,
            created_micros: 1_000,
            expires_micros: 43_200_001_000,
        };
        for (highest_bidder_guid, highest_bid) in [(8, 0), (0, 201)] {
            let mut store = FakeExpiry {
                auction: Some(ActiveAuction {
                    id: 41,
                    listing: listing.clone(),
                    highest_bidder_guid,
                    highest_bid,
                }),
                schedule_count: 1,
                mail: Vec::new(),
                notices: Vec::new(),
                returned_items: Vec::new(),
                refunded_copper: 0,
            };
            assert!(expire_active(&mut store, 41).is_err());
            assert!(store.auction.is_some());
            assert_eq!(store.schedule_count, 1);
            assert!(store.mail.is_empty());
        }

        let mut overflowing = listing;
        overflowing.deposit = u32::MAX;
        let mut store = FakeExpiry {
            auction: Some(ActiveAuction {
                id: 41,
                listing: overflowing,
                highest_bidder_guid: 8,
                highest_bid: u32::MAX,
            }),
            schedule_count: 1,
            mail: Vec::new(),
            notices: Vec::new(),
            returned_items: Vec::new(),
            refunded_copper: 0,
        };
        assert!(expire_active(&mut store, 41).is_err());
        assert!(store.auction.is_some());
        assert_eq!(store.schedule_count, 1);
        assert!(store.mail.is_empty());
    }

    /// Seller 7's listing 41 in house 1 at a 5% cut, with bidder 9's bid of 201. The cut is
    /// 201 * 5 / 100 = 10.05, truncated to 10.
    fn bid_listing() -> BidAuction {
        BidAuction {
            highest_bidder_guid: 9,
            highest_bid: 201,
            ..active_bid_auction()
        }
    }

    fn cancel_request(operation_id: u64, cut: u32) -> HoldRequest {
        HoldRequest {
            operation: HoldOperation::Cancel,
            operation_id,
            bidder_guid: 7,
            auction_id: 41,
            house: 1,
            offer: cut,
        }
    }

    fn cancel_market(auction: BidAuction) -> FakeBidMarket {
        FakeBidMarket {
            auction: Some(auction),
            decisions: Vec::new(),
            mail: Vec::new(),
            notices: Vec::new(),
            expiry_armed: true,
            now_micros: 1_000,
        }
    }

    const CANCELLED_WITH_BID: HoldDecision = HoldDecision::Cancelled {
        cut: 10,
        displaced_bidder_guid: 9,
        displaced_bid: 201,
    };

    #[test]
    fn realm_cancellation_decision_checks_owner_market_expiry_and_the_held_cut() {
        let listing = Some(bid_listing());
        assert_eq!(
            decide_cancel(listing, cancel_request(950, 10), 1_000),
            CANCELLED_WITH_BID
        );
        assert_eq!(
            decide_cancel(Some(active_bid_auction()), cancel_request(950, 0), 1_000),
            HoldDecision::Cancelled {
                cut: 0,
                displaced_bidder_guid: 0,
                displaced_bid: 0,
            },
            "an unbid listing costs nothing"
        );
        assert_eq!(
            decide_cancel(
                listing,
                HoldRequest {
                    house: 3,
                    ..cancel_request(950, 10)
                },
                1_000
            ),
            CANCELLED_WITH_BID,
            "Darnassus shares the Alliance market with Stormwind"
        );

        for (cut, why) in [
            (11, "a bid raised the cut"),
            (0, "the listing gained a bid"),
        ] {
            assert_eq!(
                decide_cancel(listing, cancel_request(950, cut), 1_000),
                HoldDecision::Database,
                "{why}"
            );
        }
        for (auction, request, now, why) in [
            (None, cancel_request(950, 10), 1_000, "missing"),
            (listing, cancel_request(950, 10), 2_000, "expired"),
            (
                listing,
                HoldRequest {
                    bidder_guid: 8,
                    ..cancel_request(950, 10)
                },
                1_000,
                "another player's listing",
            ),
            (
                listing,
                HoldRequest {
                    house: 4,
                    ..cancel_request(950, 10)
                },
                1_000,
                "the Horde market",
            ),
        ] {
            assert_eq!(
                decide_cancel(auction, request, now),
                HoldDecision::ItemNotFound,
                "{why}"
            );
        }
        for (bidder, bid) in [(9, 0), (0, 201)] {
            let inconsistent = BidAuction {
                highest_bidder_guid: bidder,
                highest_bid: bid,
                ..active_bid_auction()
            };
            assert_eq!(
                decide_cancel(Some(inconsistent), cancel_request(950, 10), 1_000),
                HoldDecision::Database
            );
        }
        let as_bid = HoldRequest {
            operation: HoldOperation::Bid,
            ..cancel_request(950, 10)
        };
        assert_eq!(
            decide_cancel(listing, as_bid, 1_000),
            HoldDecision::Database
        );
        assert_eq!(
            decide_bid(listing, cancel_request(950, 300), 1_000),
            HoldDecision::Database,
            "a Cancellation Hold never buys"
        );
    }

    #[test]
    fn a_cancellation_hold_spends_exactly_the_cut_and_a_refusal_restores_it() {
        let request = cancel_request(951, 10);

        let mut cancelled = FakeBidSource::new(100);
        assert_eq!(fence_bid(&mut cancelled, request), Ok(()));
        assert_eq!(cancelled.money, 90, "the cut is held first");
        for _ in 0..2 {
            assert_eq!(
                finish_bid(&mut cancelled, request, CANCELLED_WITH_BID),
                Ok(CANCELLED_WITH_BID)
            );
            assert_eq!(cancelled.money, 90, "the cut is spent once");
        }

        let mut refused = FakeBidSource::new(100);
        fence_bid(&mut refused, request).unwrap();
        for _ in 0..2 {
            assert_eq!(
                finish_bid(&mut refused, request, HoldDecision::Database),
                Ok(HoldDecision::Database)
            );
            assert_eq!(refused.money, 100, "a refusal restores the cut once");
        }

        let unbid = cancel_request(952, 0);
        let mut free = FakeBidSource::new(0);
        assert_eq!(
            fence_bid(&mut free, unbid),
            Ok(()),
            "a zero cut still Holds"
        );
        assert!(free.hold.is_some());
        let no_cut = HoldDecision::Cancelled {
            cut: 0,
            displaced_bidder_guid: 0,
            displaced_bid: 0,
        };
        assert_eq!(finish_bid(&mut free, unbid, no_cut), Ok(no_cut));
        assert_eq!((free.money, free.deferred_refund), (0, 0));

        let mut overflowed = FakeBidSource::new(100);
        fence_bid(&mut overflowed, request).unwrap();
        overflowed.money = u32::MAX;
        assert_eq!(
            finish_bid(&mut overflowed, request, HoldDecision::ItemNotFound),
            Ok(HoldDecision::ItemNotFound)
        );
        assert_eq!(overflowed.money, u32::MAX);
        assert_eq!(
            overflowed.deferred_refund, 10,
            "a full purse defers the cut"
        );
        assert_eq!(confirm_bid_refund(&mut overflowed, request, 10), Ok(()));
        assert_eq!(overflowed.deferred_refund, 0);

        let mut poor = FakeBidSource::new(9);
        assert_eq!(
            fence_bid(&mut poor, request),
            Err(AuctionRefusal::NotEnoughMoney)
        );
        assert_eq!((poor.money, poor.hold), (9, None));

        let mut crossed = FakeBidSource::new(100);
        fence_bid(&mut crossed, request).unwrap();
        for wrong in [
            HoldDecision::Cancelled {
                cut: 11,
                displaced_bidder_guid: 9,
                displaced_bid: 201,
            },
            accepted_active(10, 4, 9, 201),
        ] {
            assert_eq!(
                finish_bid(&mut crossed, request, wrong),
                Err(AuctionRefusal::Database)
            );
            assert_eq!(crossed.money, 90);
        }
        let mut free_bid = FakeBidSource::new(100);
        assert_eq!(
            fence_bid(
                &mut free_bid,
                HoldRequest {
                    operation: HoldOperation::Bid,
                    ..unbid
                }
            ),
            Err(AuctionRefusal::Database),
            "only a Cancellation may hold nothing"
        );
    }

    #[test]
    fn a_cancellation_removes_the_listing_and_mails_the_item_and_the_bid_back_once() {
        let item = item(23).snapshot;
        let mut market = cancel_market(bid_listing());
        for _ in 0..2 {
            assert_eq!(
                resolve_bid(&mut market, cancel_request(953, 10)),
                Ok(CANCELLED_WITH_BID)
            );
        }
        assert_eq!(market.auction, None);
        assert!(!market.expiry_armed);
        let blank = AuctionMail {
            recipient_guid: 0,
            house: 1,
            action: AuctionMailAction::Cancelled,
            item_entry: 25,
            random_property_id: 117,
            money: 0,
            attached_item: crate::items::ItemSnapshot::default(),
            counterparty_guid: 0,
            bid: 0,
            buyout: 0,
            deposit: 0,
            cut: 0,
        };
        assert_eq!(
            market.mail,
            vec![
                AuctionMail {
                    recipient_guid: 9,
                    action: AuctionMailAction::CancelledToBidder,
                    money: 201,
                    ..blank
                },
                AuctionMail {
                    recipient_guid: 7,
                    attached_item: item,
                    ..blank
                },
            ],
            "the bid goes back to the bidder, the item to the seller, the deposit to nobody"
        );
        assert_eq!(
            market.notices,
            vec![AuctionNoticeDraft {
                recipient_guid: 9,
                kind: 5,
                house: 1,
                auction_id: 41,
                item_entry: 25,
                random_property_id: 117,
                bid: 201,
                out_bid: 0,
                bidder_guid: 9,
            }]
        );
        assert_eq!(market.decisions.len(), 1, "a replay records nothing");

        for crossed in [
            HoldRequest {
                operation: HoldOperation::Bid,
                ..cancel_request(953, 10)
            },
            cancel_request(953, 11),
        ] {
            assert_eq!(
                resolve_bid(&mut market, crossed),
                Err(AuctionRefusal::Database)
            );
        }
        assert_eq!(market.mail.len(), 2);

        let mut unbid = cancel_market(active_bid_auction());
        resolve_bid(&mut unbid, cancel_request(954, 0)).unwrap();
        assert_eq!(
            unbid.mail,
            vec![AuctionMail {
                recipient_guid: 7,
                attached_item: item,
                ..blank
            }]
        );
        assert!(unbid.notices.is_empty(), "nobody was displaced");
    }

    #[test]
    fn a_bid_or_a_settlement_after_the_read_refuses_the_cancellation_and_refunds_the_cut() {
        let mut seller = FakeBidSource::new(100);
        let mut market = cancel_market(bid_listing());
        let cancel = cancel_request(955, 10);
        fence_bid(&mut seller, cancel).unwrap();
        assert_eq!(seller.money, 90);

        let raise = HoldRequest {
            operation: HoldOperation::Bid,
            operation_id: 956,
            bidder_guid: 8,
            auction_id: 41,
            house: 1,
            offer: 300,
        };
        let mut bidder = FakeBidSource::new(1_000);
        assert_eq!(
            drive_bid(&mut bidder, &mut market, raise),
            Ok(accepted_active(300, 4, 9, 201))
        );

        assert_eq!(
            drive_bid(&mut seller, &mut market, cancel),
            Ok(HoldDecision::Database)
        );
        assert_eq!(seller.money, 100, "the seller gets the whole cut back");
        assert_eq!(
            market
                .auction
                .map(|auction| (auction.highest_bidder_guid, auction.highest_bid)),
            Some((8, 300)),
            "the listing stays with the new bid"
        );
        assert!(market
            .mail
            .iter()
            .all(|mail| mail.action == AuctionMailAction::Outbid));

        // A settlement that wins the race removes the listing first.
        let mut settled = cancel_market(bid_listing());
        let mut late_seller = FakeBidSource::new(100);
        let late = cancel_request(957, 10);
        fence_bid(&mut late_seller, late).unwrap();
        settled.auction = None;
        assert_eq!(
            drive_bid(&mut late_seller, &mut settled, late),
            Ok(HoldDecision::ItemNotFound)
        );
        assert_eq!(late_seller.money, 100);
        assert!(settled.mail.is_empty());
    }

    #[test]
    fn local_and_interrupted_sharded_cancellations_have_equivalent_state() {
        for (operation_id, auction, cut) in
            [(958, bid_listing(), 10), (959, active_bid_auction(), 0)]
        {
            let request = cancel_request(operation_id, cut);
            let source = FakeBidSource::new(100);
            let market = cancel_market(auction);

            let mut local_source = source;
            let mut local_market = market.clone();
            let expected = drive_bid(&mut local_source, &mut local_market, request).unwrap();
            assert!(matches!(expected, HoldDecision::Cancelled { .. }));
            assert_eq!(local_source.money, 100 - cut);

            for killed_after in 0..=2 {
                let mut sharded_source = source;
                let mut sharded_market = market.clone();
                if killed_after >= 1 {
                    fence_bid(&mut sharded_source, request).unwrap();
                }
                if killed_after >= 2 {
                    resolve_bid(&mut sharded_market, request).unwrap();
                }
                for _ in 0..2 {
                    assert_eq!(
                        drive_bid(&mut sharded_source, &mut sharded_market, request),
                        Ok(expected)
                    );
                }
                assert_eq!(sharded_source.money, local_source.money);
                assert_eq!(sharded_source.hold, local_source.hold);
                assert_eq!(sharded_market.auction, local_market.auction);
                assert_eq!(sharded_market.decisions, local_market.decisions);
                assert_eq!(sharded_market.mail, local_market.mail);
                assert_eq!(sharded_market.notices, local_market.notices);
                assert_eq!(sharded_market.expiry_armed, local_market.expiry_armed);
            }
        }
    }

    #[test]
    fn rows_written_before_cancellation_existed_read_as_bids() {
        assert_eq!(HoldOperation::from_code(0), Some(HoldOperation::Bid));
        assert_eq!(HoldOperation::from_code(1), Some(HoldOperation::Cancel));
        assert_eq!(HoldOperation::from_code(2), None);

        let legacy_acceptance = BidDecisionFields {
            outcome: 1,
            revision: 4,
            result_bidder_guid: 9,
            result_bid: 201,
            minimum_increment: 0,
            accepted_price: 0,
        };
        assert_eq!(
            bid_decision_from_fields(legacy_acceptance, 107),
            Some(accepted_active(107, 4, 9, 201)),
            "a bid row from before accepted_price existed still charges its offer"
        );
        let free_cancellation = BidDecisionFields {
            outcome: 7,
            accepted_price: 0,
            ..legacy_acceptance
        };
        assert_eq!(
            bid_decision_from_fields(free_cancellation, 107),
            Some(HoldDecision::Cancelled {
                cut: 0,
                displaced_bidder_guid: 9,
                displaced_bid: 201,
            }),
            "a zero cut is a real cut, never the offer"
        );
    }

    #[test]
    fn auction_write_reducers_gate_before_reading_caller_named_state() {
        use crate::test_scan::code_of;

        for signature in [
            "pub fn gw_auction_list_local(",
            "pub fn gw_auction_hold_listing(",
            "pub fn realm_auction_commit_listing(",
            "pub fn realm_auction_confirm_listing(",
            "pub fn realm_auction_settle_listing(",
            "pub fn gw_auction_bid_local(",
            "pub fn gw_auction_hold_bid(",
            "pub fn realm_auction_decide_bid(",
            "pub fn gw_auction_finish_bid(",
            "pub fn realm_auction_refund_bid(",
            "pub fn gw_auction_confirm_bid_refund(",
            "pub fn gw_auction_cancel_local(",
            "pub fn gw_auction_hold_cancel(",
            "pub fn realm_auction_decide_cancel(",
            "pub fn debug_stage_auction_cancel_fixture(",
            "pub fn debug_stage_auction_buyout_fixture(",
            "pub fn debug_verify_auction_buyout_fixture(",
            "pub fn debug_stage_auction_expiry_fixture(",
            "pub fn debug_replay_auction_expiry_fixture(",
            "pub fn debug_verify_auction_expiry_fixture(",
            "pub fn debug_stage_legacy_auction_mail_fixture(",
            "pub fn debug_verify_legacy_auction_mail_repaired(",
        ] {
            let body = code_of(include_str!("auction.rs"), signature);
            let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{signature}` no longer opens with the operator gate. Body was:\n{body}"
            );
        }
    }

    #[test]
    fn auction_expiry_is_scheduler_only() {
        let body = crate::test_scan::code_of(include_str!("auction.rs"), "pub fn expire_auction(");
        let normalized = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(normalized.starts_with(
            "{ if ctx.sender() != ctx.database_identity() { return Err(\"scheduler only\".to_string());"
        ));
    }

    #[test]
    fn character_delete_refuses_auction_value_before_the_cascade() {
        let body = crate::test_scan::code_of(include_str!("auth.rs"), "pub fn delete_character(");
        let auction_gate = body
            .find("crate::auction::character_has_auction_value")
            .expect("character deletion must check Auction value");
        let cascade = body
            .find("crate::world::cascade_delete_character")
            .expect("character deletion still needs its normal cascade");
        assert!(
            auction_gate < cascade,
            "Auction value must be fenced before deletion"
        );
    }
}
