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

use super::{Rollback, Unhandled};
use crate::{
    auction::{self as auctions, game_auction, native::BidRefusal, AuctionNotice},
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
) -> Result<(), Rollback> {
    let request = codec::decode_auction_browse(body).map_err(|cause| cause.to_string())?;
    super::actor(ctx, guid)?;
    if crate::peer::realm_shard(ctx).is_some() {
        return super::realm_auction::browse(ctx, connection, guid, request.auctioneer_guid, body);
    }
    let (rows, total) = auctions::auction_house_for_interaction(ctx, guid, request.auctioneer_guid)
        .map_or_else(
            || (Vec::new(), 0),
            |house| page(ctx, guid, house.id, Query::Browse(request)),
        );
    Ok(super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(codec::build_auction_list_result(
            &rows,
            total,
            crate::time::now(ctx).to_micros_since_unix_epoch(),
        ))),
        false,
    )?)
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    use ClientOpcodeMessage as C;
    let packet = match message {
        C::MSG_AUCTION_HELLO(request) => {
            super::actor(ctx, guid)?;
            let Some(house) =
                auctions::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
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
            if forwards(ctx, guid)? {
                return realm(
                    ctx,
                    connection,
                    guid,
                    C::CMSG_AUCTION_LIST_OWNER_ITEMS(request),
                );
            }
            let Some(house) =
                auctions::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
            else {
                return Ok(());
            };
            let (rows, total) = page(ctx, guid, house.id, Query::Owner(request.list_from));
            ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(Box::new(
                codec::build_auction_owner_list_result(
                    &rows,
                    total,
                    crate::time::now(ctx).to_micros_since_unix_epoch(),
                ),
            ))
        }
        C::CMSG_AUCTION_LIST_BIDDER_ITEMS(request) => {
            if forwards(ctx, guid)? {
                return realm(
                    ctx,
                    connection,
                    guid,
                    C::CMSG_AUCTION_LIST_BIDDER_ITEMS(request),
                );
            }
            let Some(house) =
                auctions::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
            else {
                return Ok(());
            };
            let (rows, total) = page(ctx, guid, house.id, Query::Bidder(request.start_from_page));
            ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(Box::new(
                codec::build_auction_bidder_list_result(
                    &rows,
                    total,
                    crate::time::now(ctx).to_micros_since_unix_epoch(),
                ),
            ))
        }
        C::CMSG_AUCTION_SELL_ITEM(request) => {
            if forwards(ctx, guid)? {
                return realm(ctx, connection, guid, C::CMSG_AUCTION_SELL_ITEM(request));
            }
            let auction_id = auctions::native::list(
                ctx,
                guid,
                request.auctioneer.guid(),
                request.item.guid(),
                auctions::ListingTerms {
                    start_bid: request.starting_bid,
                    buyout: request.buyout,
                    duration_minutes: request.auction_duration_in_minutes,
                },
            )
            .map_err(|refusal| {
                Rollback::reply(
                    command(
                        0,
                        Action::Started {
                            result2: result_two(refusal),
                        },
                    ),
                    refusal.as_tag(),
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
            if forwards(ctx, guid)? {
                return realm(ctx, connection, guid, C::CMSG_AUCTION_PLACE_BID(request));
            }
            let price = auctions::native::bid(
                ctx,
                guid,
                request.auctioneer.guid(),
                request.auction_id,
                request.price.as_int(),
            )
            .map_err(|refusal| {
                Rollback::reply(
                    bid_refusal(request.auction_id, &refusal),
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
            if forwards(ctx, guid)? {
                return realm(ctx, connection, guid, C::CMSG_AUCTION_REMOVE_ITEM(request));
            }
            if ctx
                .db
                .game_world_entity()
                .guid()
                .find(request.auctioneer.guid())
                .is_none()
            {
                return Ok(());
            }
            auctions::native::cancel(ctx, guid, request.auctioneer.guid(), request.auction_id)
                .map_err(|refusal| {
                    if refusal == AuctionRefusal::NotEnoughMoney {
                        Rollback::silent(refusal.as_tag())
                    } else {
                        Rollback::reply(
                            command(
                                0,
                                Action::Removed {
                                    result2: ResultTwo::ErrDatabase,
                                },
                            ),
                            refusal.as_tag(),
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
        other => return Err(Unhandled::Declined(other)),
    };
    Ok(super::send(ctx, connection, packet, false)?)
}

/// Runs the actor Gate; true when the Realm, not this World, answers the request.
fn forwards(ctx: &ReducerContext, guid: u64) -> Result<bool, Rollback> {
    super::actor(ctx, guid)?;
    Ok(crate::peer::realm_shard(ctx).is_some())
}

fn realm(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    Ok(super::realm_auction::receive(
        ctx, connection, guid, message,
    )?)
}

pub(super) fn command(auction_id: u32, action: Action) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(Box::new(SMSG_AUCTION_COMMAND_RESULT {
        auction_id,
        action,
    }))
}

pub(super) fn result_two(refusal: AuctionRefusal) -> ResultTwo {
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
    let Some(viewer) = viewer(ctx, guid) else {
        return (Vec::new(), 0);
    };
    page_for(ctx, &viewer, house, query)
}

#[derive(spacetimedb::SpacetimeType)]
pub struct Viewer {
    pub guid: u64,
    pub level: u32,
    pub class: u8,
    pub learned: Vec<u32>,
}

pub(super) fn viewer(ctx: &ReducerContext, guid: u64) -> Option<Viewer> {
    let actor = crate::helpers::acting_entity_by_guid(ctx, guid)?;
    Some(Viewer {
        guid,
        level: actor.level,
        class: ((actor.unit_bytes_0 >> 8) & 255) as u8,
        learned: ctx
            .db
            .game_player_spell()
            .by_character()
            .filter(guid)
            .map(|spell| spell.spell_id)
            .collect(),
    })
}

fn page_for(
    ctx: &ReducerContext,
    viewer: &Viewer,
    house: u32,
    query: Query,
) -> (Vec<codec::AuctionView>, u32) {
    let guid = viewer.guid;
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
    let proficiency = codec::learned_proficiency(viewer.class, &viewer.learned);
    let name = if let Query::Browse(request) = &query {
        request.name.to_lowercase()
    } else {
        String::new()
    };
    let now = crate::time::now(ctx).to_micros_since_unix_epoch();
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
                            || (u32::from(item.required_level) <= viewer.level
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
    if crate::peer::world_shard(ctx).is_some() {
        super::realm_auction::notice(ctx, row.recipient_guid, packet);
        return;
    }
    super::send_to_character(ctx, row.recipient_guid, packet, super::EncodeFailure::Panic);
}

pub(super) fn bid_refusal(id: u32, refusal: &BidRefusal) -> ServerOpcodeMessage {
    let result = match *refusal {
        BidRefusal::Auction(AuctionRefusal::ItemNotFound) => BidResult::ErrItemNotFound,
        BidRefusal::Auction(AuctionRefusal::NotEnoughMoney) => BidResult::ErrNotEnoughMoney,
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
    command(id, Action::BidPlaced { result })
}

pub(super) fn realm_page(
    ctx: &ReducerContext,
    viewer: &Viewer,
    house: u32,
    kind: u8,
    body: &[u8],
) -> Result<ServerOpcodeMessage, String> {
    let query = match kind {
        0 => Query::Browse(codec::decode_auction_browse(body).map_err(|error| error.to_string())?),
        1 | 2 => {
            let offset = u32::from_le_bytes(body.try_into().map_err(|_| "invalid Listing offset")?);
            if kind == 1 {
                Query::Owner(offset)
            } else {
                Query::Bidder(offset)
            }
        }
        _ => return Err("unknown Listing query".into()),
    };
    let (rows, total) = page_for(ctx, viewer, house, query);
    let now = crate::time::now(ctx).to_micros_since_unix_epoch();
    Ok(match kind {
        0 => ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(
            codec::build_auction_list_result(&rows, total, now),
        )),
        1 => ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(Box::new(
            codec::build_auction_owner_list_result(&rows, total, now),
        )),
        _ => ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(Box::new(
            codec::build_auction_bidder_list_result(&rows, total, now),
        )),
    })
}
