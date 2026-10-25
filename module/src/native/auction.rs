use lyracore_protocol::{
    codec,
    messages::{
        vanilla::{
            opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
            AuctionHouse, MSG_AUCTION_HELLO_Server,
            SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction as Action,
            SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult as BidResult,
            SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo as ResultTwo,
            SMSG_AUCTION_COMMAND_RESULT,
        },
        Guid,
    },
};
use lyracore_shared::auction::{auction_notice, bid_increment, market_of, AuctionRefusal};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::game_native_player;
use crate::{
    auction::{self as core, game_auction, native::BidRefusal, AuctionNotice},
    game_item_template, game_player_spell, game_world_entity,
};

enum Query {
    Browse(codec::AuctionBrowseRequest),
    Owner(u32),
    Bidder(u32),
}

pub(super) fn browse(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    body: &[u8],
) -> Result<(), String> {
    let request = codec::decode_auction_browse(body).map_err(|cause| cause.to_string())?;
    super::actor(ctx, guid).map_err(super::rollback_silent)?;
    let (rows, total) = core::auction_house_for_interaction(ctx, guid, request.auctioneer_guid)
        .map_or_else(
            || (Vec::new(), 0),
            |house| page(ctx, guid, house.id, Query::Browse(request)),
        );
    super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(codec::build_auction_list_result(
            &rows,
            total,
            ctx.timestamp.to_micros_since_unix_epoch(),
        ))),
        false,
    )
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    super::actor(ctx, guid).map_err(super::rollback_silent)?;
    use ClientOpcodeMessage as C;
    let packet = match message {
        C::MSG_AUCTION_HELLO(request) => {
            let Some(house) =
                core::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
            else {
                return Ok(());
            };
            ServerOpcodeMessage::MSG_AUCTION_HELLO(Box::new(MSG_AUCTION_HELLO_Server {
                auctioneer: request.auctioneer,
                auction_house: AuctionHouse::try_from(house.id)
                    .map_err(|cause| cause.to_string())?,
            }))
        }
        C::CMSG_AUCTION_LIST_OWNER_ITEMS(request) => {
            let Some(house) =
                core::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
            else {
                return Ok(());
            };
            let (rows, total) = page(ctx, guid, house.id, Query::Owner(request.list_from));
            ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(Box::new(
                codec::build_auction_owner_list_result(
                    &rows,
                    total,
                    ctx.timestamp.to_micros_since_unix_epoch(),
                ),
            ))
        }
        C::CMSG_AUCTION_LIST_BIDDER_ITEMS(request) => {
            let Some(house) =
                core::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
            else {
                return Ok(());
            };
            let (rows, total) = page(ctx, guid, house.id, Query::Bidder(request.start_from_page));
            ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(Box::new(
                codec::build_auction_bidder_list_result(
                    &rows,
                    total,
                    ctx.timestamp.to_micros_since_unix_epoch(),
                ),
            ))
        }
        C::CMSG_AUCTION_SELL_ITEM(request) => {
            let auction_id = core::native::list(
                ctx,
                guid,
                request.auctioneer.guid(),
                request.item.guid(),
                core::ListingTerms {
                    start_bid: request.starting_bid,
                    buyout: request.buyout,
                    duration_minutes: request.auction_duration_in_minutes,
                },
            )
            .map_err(|refusal| {
                super::rollback_reply(
                    command(
                        0,
                        Action::Started {
                            result2: result_two(refusal),
                        },
                    ),
                    refusal.as_tag().into(),
                )
            })?;
            command(
                auction_id,
                Action::Started {
                    result2: ResultTwo::Ok,
                },
            )
        }
        C::CMSG_AUCTION_PLACE_BID(request) => {
            let price = core::native::bid(
                ctx,
                guid,
                request.auctioneer.guid(),
                request.auction_id,
                request.price.as_int(),
            )
            .map_err(|refusal| {
                let result = match refusal {
                    BidRefusal::Auction(AuctionRefusal::ItemNotFound) => BidResult::ErrItemNotFound,
                    BidRefusal::Auction(AuctionRefusal::NotEnoughMoney) => {
                        BidResult::ErrNotEnoughMoney
                    }
                    BidRefusal::Auction(_) => BidResult::ErrDatabase,
                    BidRefusal::Own => BidResult::ErrBidOwn,
                    BidRefusal::Increment => BidResult::ErrBidIncrement,
                    BidRefusal::HigherBid {
                        bidder_guid,
                        current_bid,
                        minimum_increment,
                    } => BidResult::ErrHigherBid {
                        auction_outbid2: minimum_increment,
                        higher_bidder: Guid::new(bidder_guid),
                        new_bid: current_bid,
                    },
                };
                super::rollback_reply(
                    command(request.auction_id, Action::BidPlaced { result }),
                    format!("auction bid: {refusal:?}"),
                )
            })?;
            command(
                request.auction_id,
                Action::BidPlaced {
                    result: BidResult::Ok {
                        auction_outbid1: bid_increment(price),
                    },
                },
            )
        }
        C::CMSG_AUCTION_REMOVE_ITEM(request) => {
            if ctx
                .db
                .game_world_entity()
                .guid()
                .find(request.auctioneer.guid())
                .is_none()
            {
                return Ok(());
            }
            core::native::cancel(ctx, guid, request.auctioneer.guid(), request.auction_id)
                .map_err(|refusal| {
                    if refusal == AuctionRefusal::NotEnoughMoney {
                        super::rollback_silent(refusal.as_tag().into())
                    } else {
                        super::rollback_reply(
                            command(
                                0,
                                Action::Removed {
                                    result2: ResultTwo::ErrDatabase,
                                },
                            ),
                            refusal.as_tag().into(),
                        )
                    }
                })?;
            command(
                request.auction_id,
                Action::Removed {
                    result2: ResultTwo::Ok,
                },
            )
        }
        _ => return Err("opcode is not an auction request".into()),
    };
    super::send(ctx, connection, packet, false)
}

fn command(auction_id: u32, action: Action) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(Box::new(SMSG_AUCTION_COMMAND_RESULT {
        auction_id,
        action,
    }))
}

fn result_two(refusal: AuctionRefusal) -> ResultTwo {
    match refusal {
        AuctionRefusal::ItemNotFound => ResultTwo::ErrItemNotFound,
        AuctionRefusal::NotEnoughMoney => ResultTwo::ErrNotEnoughMoney,
        AuctionRefusal::InvalidTerms | AuctionRefusal::Database => ResultTwo::ErrDatabase,
    }
}

fn page(
    ctx: &ReducerContext,
    guid: u64,
    house: u32,
    query: Query,
) -> (Vec<codec::AuctionView>, u32) {
    let Some(actor) = crate::helpers::acting_entity_by_guid(ctx, guid) else {
        return (Vec::new(), 0);
    };
    let (rows, offset) = match &query {
        Query::Browse(request) => (
            ctx.db.game_auction().iter().collect::<Vec<_>>(),
            request.offset,
        ),
        Query::Owner(offset) => (
            ctx.db.game_auction().by_owner().filter(guid).collect(),
            *offset,
        ),
        Query::Bidder(offset) => (
            ctx.db
                .game_auction()
                .by_highest_bidder()
                .filter(guid)
                .collect(),
            *offset,
        ),
    };
    let learned = ctx
        .db
        .game_player_spell()
        .by_character()
        .filter(guid)
        .map(|spell| spell.spell_id)
        .collect::<Vec<_>>();
    let proficiency = codec::learned_proficiency(((actor.unit_bytes_0 >> 8) & 255) as u8, &learned);
    let name = if let Query::Browse(request) = &query {
        request.name.to_lowercase()
    } else {
        String::new()
    };
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let mut rows = rows
        .into_iter()
        .filter(|row| {
            if market_of(row.house) != market_of(house)
                || row.expires_at.to_micros_since_unix_epoch() <= now
            {
                return false;
            }
            let Query::Browse(request) = &query else {
                return true;
            };
            ctx.db
                .game_item_template()
                .entry()
                .find(row.item_entry)
                .is_some_and(|item| {
                    (name.is_empty() || item.name.to_lowercase().contains(&name))
                        && request
                            .minimum_level
                            .is_none_or(|minimum| item.required_level >= minimum)
                        && request
                            .maximum_level
                            .is_none_or(|maximum| item.required_level <= maximum)
                        && request
                            .inventory_type
                            .is_none_or(|wanted| u32::from(item.inventory_type) == wanted)
                        && request
                            .item_class
                            .is_none_or(|wanted| u32::from(item.class) == wanted)
                        && request
                            .item_subclass
                            .is_none_or(|wanted| u32::from(item.subclass) == wanted)
                        && request.quality.is_none_or(|wanted| item.quality == wanted)
                        && (!request.usable_only
                            || (u32::from(item.required_level) <= actor.level
                                && proficiency.can_equip(item.class, item.subclass)))
                })
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.id);
    let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    (
        rows.into_iter()
            .skip(offset as usize)
            .take(50)
            .map(|row| codec::AuctionView {
                id: row.id,
                item_entry: row.item_entry,
                item_stack_count: row.item_stack_count,
                item_enchant_id: row.item_enchant_id,
                owner_guid: row.owner_guid,
                start_bid: row.start_bid,
                buyout: row.buyout,
                highest_bidder_guid: row.highest_bidder_guid,
                highest_bid: row.highest_bid,
                expires_at_micros: row.expires_at.to_micros_since_unix_epoch(),
                random_property_id: row.random_property_id,
            })
            .collect(),
        total,
    )
}

pub(crate) fn notice(ctx: &ReducerContext, row: &AuctionNotice) {
    let Some(recipient) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.recipient_guid)
    else {
        return;
    };
    let Ok(house) = AuctionHouse::try_from(row.house) else {
        spacetimedb::log::warn!(
            "auction notice {} has an unknown house {}",
            row.id,
            row.house
        );
        return;
    };
    let packet = match row.kind {
        auction_notice::OUTBID | auction_notice::WON => {
            ServerOpcodeMessage::SMSG_AUCTION_BIDDER_NOTIFICATION(Box::new(
                codec::build_auction_bidder_notification(
                    house,
                    row.auction_id,
                    row.bidder_guid,
                    row.bid,
                    row.out_bid,
                    row.item_entry,
                    row.random_property_id,
                ),
            ))
        }
        auction_notice::SOLD | auction_notice::EXPIRED | auction_notice::NEW_BID => {
            ServerOpcodeMessage::SMSG_AUCTION_OWNER_NOTIFICATION(Box::new(
                codec::build_auction_owner_notification(
                    row.auction_id,
                    row.bid,
                    row.out_bid,
                    row.bidder_guid,
                    row.item_entry,
                    row.random_property_id,
                ),
            ))
        }
        auction_notice::REMOVED => ServerOpcodeMessage::SMSG_AUCTION_REMOVED_NOTIFICATION(
            Box::new(codec::build_auction_removed_notification(
                row.auction_id,
                row.item_entry,
                row.random_property_id,
            )),
        ),
        _ => {
            spacetimedb::log::warn!("auction notice {} has an unknown kind {}", row.id, row.kind);
            return;
        }
    };
    super::send(ctx, recipient.connection_id, packet, false)
        .expect("bounded auction notice fits a world frame");
}
