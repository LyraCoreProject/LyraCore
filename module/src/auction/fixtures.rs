use super::*;

#[reducer]
pub fn debug_auction_due(ctx: &ReducerContext, auction_id: u32) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut auction = ctx
        .db
        .game_auction()
        .id()
        .find(auction_id)
        .filter(|auction| auction.item_entry == 509_0620)
        .ok_or("native auction fixture not found")?;
    let mut receipt = ctx
        .db
        .game_auction_operation_receipt()
        .operation_id()
        .find(auction.listing_operation_id)
        .ok_or("auction receipt not found")?;
    let due = ctx.timestamp.to_micros_since_unix_epoch() + 250_000;
    receipt.created_micros = due - i64::from(receipt.duration_minutes) * MICROS_PER_MINUTE;
    receipt.expires_micros = due;
    auction.created_at = Timestamp::from_micros_since_unix_epoch(receipt.created_micros);
    auction.expires_at = Timestamp::from_micros_since_unix_epoch(due);
    ctx.db.game_auction().id().update(auction);
    ctx.db
        .game_auction_operation_receipt()
        .operation_id()
        .update(receipt);
    ctx.db.game_auction_expiry().auction_id().delete(auction_id);
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(Timestamp::from_micros_since_unix_epoch(due)),
        auction_id,
    });
    Ok(())
}

const BUYOUT_FIXTURE_AUCTION_ID: u32 = 509_0050;
const BUYOUT_FIXTURE_OPERATION_ID: u64 = 509_0050;
const BUYOUT_FIXTURE_SELLER_GUID: u64 = 509_0050;
const BUYOUT_FIXTURE_WINNER_GUID: u64 = 509_0051;
const BUYOUT_FIXTURE_DISPLACED_GUID: u64 = 509_0052;
const BUYOUT_FIXTURE_NEW_BID_AUCTION_ID: u32 = 509_0056;
const BUYOUT_FIXTURE_NEW_BID_OPERATION_ID: u64 = 509_0056;
const BUYOUT_FIXTURE_NEW_BID_SELLER_GUID: u64 = 509_0057;
const BUYOUT_FIXTURE_NEW_BID_BIDDER_GUID: u64 = 509_0058;

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
        clear_auction_notices(ctx, recipient_guid);
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
        item_text_id: 0,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(expires_at),
        auction_id: BUYOUT_FIXTURE_AUCTION_ID,
    });

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
        item_text_id: 0,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(new_bid_expires_at),
        auction_id: BUYOUT_FIXTURE_NEW_BID_AUCTION_ID,
    });
    Ok(())
}

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
        item_text_id: 0,
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
        11,
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
        25,
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
        25,
        0,
    )?;

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
        3,
        BUYOUT_FIXTURE_NEW_BID_BIDDER_GUID,
    )?;
    Ok(())
}

const CANCEL_FIXTURE_AUCTION_IDS: [u32; 3] = [509_0086, 509_0088, 509_0089];
const CANCEL_FIXTURE_BIDDER_GUID: u64 = 509_0087;
const CANCEL_FIXTURE_OPERATION_IDS: std::ops::RangeInclusive<u64> = 509_0086..=509_0095;
const CANCEL_FIXTURE_FACTION_TEMPLATE: u32 = 120;

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
        clear_auction_notices(ctx, recipient_guid);
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
        auctioneer.update(ctx);
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
            item_text_id: 0,
        });
        ctx.db.game_auction_expiry().insert(AuctionExpiry {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Time(expires_at),
            auction_id,
        });
    }
    Ok(())
}

const EXPIRY_FIXTURE_AUCTION_ID: u32 = 509_0060;
const EXPIRY_FIXTURE_OPERATION_ID: u64 = 509_0060;
const EXPIRY_FIXTURE_SELLER_GUID: u64 = 509_0060;
const EXPIRY_FIXTURE_WINNER_GUID: u64 = 509_0061;
const EXPIRY_FIXTURE_ITEM: crate::items::ItemSnapshot = crate::items::ItemSnapshot {
    entry: 509_0060,
    stack_count: 2,
    durability: 17,
    enchant_id: 9,
    soulbound: false,
    random_property_id: 117,
    item_text_id: 0,
};
const EXPIRY_FIXTURE_UNSOLD_AUCTION_ID: u32 = 509_0065;
const EXPIRY_FIXTURE_UNSOLD_OPERATION_ID: u64 = 509_0065;
const EXPIRY_FIXTURE_UNSOLD_SELLER_GUID: u64 = 509_0065;
const EXPIRY_FIXTURE_UNSOLD_ITEM: crate::items::ItemSnapshot = crate::items::ItemSnapshot {
    entry: 509_0066,
    stack_count: 1,
    durability: 0,
    enchant_id: 0,
    soulbound: false,
    random_property_id: 0,
    item_text_id: 0,
};

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
        clear_auction_notices(ctx, recipient_guid);
    }

    let expires_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .checked_add(500_000)
        .ok_or_else(|| "auction expiry fixture deadline overflow".to_string())?;
    let created_micros = expires_micros
        .checked_sub(28_800_000_000)
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
            duration_minutes: 480,
            deposit: 10,
            created_micros,
            expires_micros,
            house: 1,
            deposit_rate: 5,
            consignment_rate: 5,
            item_text_id: 0,
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
        item_text_id: 0,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(expires_at),
        auction_id: EXPIRY_FIXTURE_AUCTION_ID,
    });

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
            duration_minutes: 480,
            deposit: 10,
            created_micros,
            expires_micros: expires_micros + 1,
            house: 1,
            deposit_rate: 5,
            consignment_rate: 5,
            item_text_id: 0,
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
        item_text_id: 0,
    });
    ctx.db.game_auction_expiry().insert(AuctionExpiry {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(unsold_expires_at),
        auction_id: EXPIRY_FIXTURE_UNSOLD_AUCTION_ID,
    });
    Ok(())
}

#[reducer]
pub fn debug_replay_auction_expiry_fixture(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    expire_active(&mut CtxExpiry { ctx }, EXPIRY_FIXTURE_AUCTION_ID)?;
    expire_active(&mut CtxExpiry { ctx }, EXPIRY_FIXTURE_UNSOLD_AUCTION_ID)
}

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
        11,
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
        11,
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
        1,
        0,
    )?;
    Ok(())
}

const LEGACY_MAIL_FIXTURE_RECIPIENT_GUID: u64 = 509_0070;
const LEGACY_MAIL_FIXTURE_ITEM_ENTRY: u32 = 509_0071;
const LEGACY_MAIL_LOOKALIKE_RECIPIENT_GUID: u64 = 509_0072;
const LEGACY_MAIL_LOOKALIKE_SENDER_GUID: u64 = 509_0073;
const LEGACY_MAIL_LOOKALIKE_ITEM_ENTRY: u32 = 509_0074;
const LEGACY_MAIL_LOOKALIKE_COD: u32 = 250;
const LEGACY_MAIL_NO_COD_LOOKALIKE_RECIPIENT_GUID: u64 = 509_0076;
const LEGACY_MAIL_NO_COD_LOOKALIKE_SENDER_GUID: u64 = 509_0077;
const LEGACY_MAIL_NO_COD_LOOKALIKE_ITEM_ENTRY: u32 = 509_0078;
const LEGACY_MAIL_WON_RECEIPT_OPERATION_ID: u64 = 509_0075;
const LEGACY_MAIL_WON_RECEIPT_AUCTION_ID: u32 = 509_0075;
const LEGACY_MAIL_WON_RECEIPT_HOUSE: u32 = 6;
const LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID: u64 = 509_0079;
const LEGACY_MAIL_SOLD_RECEIPT_OPERATION_ID: u64 = 509_0080;
const LEGACY_MAIL_SOLD_RECEIPT_AUCTION_ID: u32 = 509_0080;
const LEGACY_MAIL_SOLD_RECEIPT_ITEM_ENTRY: u32 = 509_0081;
const LEGACY_MAIL_SOLD_RECEIPT_HOUSE: u32 = 1;
const LEGACY_MAIL_REFUSED_RECEIPT_OPERATION_ID: u64 = 509_0082;
const LEGACY_MAIL_REFUSED_RECEIPT_ITEM_ENTRY: u32 = 509_0083;
const LEGACY_MAIL_DECOY_RECEIPT_OPERATION_ID: u64 = 509_0084;
const LEGACY_MAIL_DECOY_RECEIPT_AUCTION_ID: u32 = 509_0084;
const LEGACY_MAIL_DECOY_RECEIPT_ITEM_ENTRY: u32 = 509_0085;
const LEGACY_MAIL_WRONG_RECEIPT_HOUSE: u32 = 7;

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
        duration_minutes: 480,
        deposit: 10,
        created_micros: now,
        expires_micros: now + 1,
        house: 0,
        deposit_rate: 5,
        consignment_rate: 5,
        random_property_id: 0,
        item_text_id: 0,
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
    receipts.insert(AuctionOperationReceipt {
        operation_id: LEGACY_MAIL_REFUSED_RECEIPT_OPERATION_ID,
        auction_id: 0,
        actor_guid: LEGACY_MAIL_SOLD_FIXTURE_RECIPIENT_GUID,
        item_entry: LEGACY_MAIL_REFUSED_RECEIPT_ITEM_ENTRY,
        house: LEGACY_MAIL_WRONG_RECEIPT_HOUSE,
        ..receipt_template
    });
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
                item_text_id: 0,
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
                item_text_id: 0,
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
                item_text_id: 0,
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
