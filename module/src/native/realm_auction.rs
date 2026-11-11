use lyracore_protocol::messages::vanilla::{
    opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
    SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction as Action,
    SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult as BidResult,
    SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo as ResultTwo,
};
use lyracore_shared::auction::{bid_increment, AuctionRefusal};
use spacetimedb::{
    reducer, sats::bsatn, table, ConnectionId, Identity, ReducerContext, ScheduleAt, SpacetimeType,
    Table, TimeDuration,
};

use super::{
    auction as wire,
    directory::{self, game_character_directory},
    game_native_player,
    realm_shard::game_native_away,
    Rollback,
};
use crate::{
    auction::{
        self as auctions, game_auction, game_auction_expiry, game_auction_operation_receipt,
        Auction, AuctionOperationReceipt,
    },
    game_character, game_item_instance,
    mail_text::game_item_text,
    peer::{realm_shard, require_sender},
    remote_bindings::peer as remote,
};

#[derive(SpacetimeType)]
pub enum Request {
    Query(QueryRequest),
    List(ListingRequest),
    RepeatListing(AuctionOperationReceipt),
    Bid(BidRequest),
    Quote(CancellationQuote),
    Cancel(Cancellation),
    Import(ListingImport),
}

#[derive(SpacetimeType)]
pub enum Answer {
    Query(Vec<AuctionPacket>),
    Quote(u32),
    Complete(Completion),
    Listed(u32),
}

#[derive(SpacetimeType)]
pub struct AuctionOutcome {
    accepted: bool,
    packets: Vec<AuctionPacket>,
}

#[table(accessor = game_auction_transaction, index(accessor = by_character, btree(columns = [guid])))]
pub struct AuctionTransaction {
    #[primary_key]
    pub operation: u64,
    pub guid: u64,
    pub connection: u128,
    pub request: Vec<u8>,
    pub copper: u32,
    pub outcome: Option<AuctionOutcome>,
}

#[table(accessor = game_auction_deletion)]
pub struct AuctionDeletion {
    #[primary_key]
    pub guid: u64,
    #[unique]
    pub selection_id: u64,
    pub world: Identity,
}

pub(super) fn begin_deletion(
    ctx: &ReducerContext,
    guid: u64,
    selection_id: u64,
    world: Identity,
) -> Result<(), Rollback> {
    if auctions::character_has_auction_value(ctx, guid)
        || ctx.db.game_auction_deletion().guid().find(guid).is_some()
    {
        return Err(Rollback::reply(
            ServerOpcodeMessage::SMSG_CHAR_DELETE(
                lyracore_protocol::codec::build_char_delete_response(
                    lyracore_protocol::codec::CharDeleteOutcome::Failed,
                ),
            ),
            "Character has Auction value or deletion pending",
        ));
    }
    ctx.db.game_auction_deletion().insert(AuctionDeletion {
        guid,
        selection_id,
        world,
    });
    Ok(())
}

pub(super) fn finish_deletion(ctx: &ReducerContext, selection_id: u64, world: Identity) {
    if let Some(row) = ctx
        .db
        .game_auction_deletion()
        .selection_id()
        .find(selection_id)
        .filter(|row| row.world == world)
    {
        ctx.db.game_auction_deletion().guid().delete(row.guid);
    }
}

#[table(accessor = game_realm_auction_request, outbox(remote::idc_realm_auction_request))]
pub struct RealmAuctionRequest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub operation: u64,
    pub guid: u64,
    pub connection: u128,
    pub request: Vec<u8>,
}

#[table(accessor = game_world_auction_answer, outbox(remote::idc_world_auction_answer))]
pub struct WorldAuctionAnswer {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub operation: u64,
    pub guid: u64,
    pub connection: u128,
    pub answer: Vec<u8>,
}

#[derive(SpacetimeType)]
pub enum Retry {
    Settle(u64),
    Deliver(AuctionDelivery),
}

#[table(accessor = game_auction_retry, scheduled(retry_auction))]
pub struct AuctionRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub retry: Retry,
}
crate::virtual_clock_exempt!(game_auction_retry);

fn encode<T: spacetimedb::Serialize>(value: &T) -> Vec<u8> {
    bsatn::to_vec(value).expect("auction request encodes")
}

fn packet(message: ServerOpcodeMessage) -> Vec<AuctionPacket> {
    let packet = lyracore_protocol::world::encode(message)
        .expect("bounded auction reply fits a world frame");
    vec![AuctionPacket {
        opcode: packet.opcode,
        body: packet.body,
    }]
}

fn unavailable() -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(
        lyracore_protocol::messages::vanilla::SMSG_NOTIFICATION {
            notification: "Auction request unavailable. Please try again.".into(),
        },
    ))
}

pub(crate) fn pending(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_auction_transaction()
        .by_character()
        .filter(guid)
        .next()
        .is_some()
}

fn ready(ctx: &ReducerContext, guid: u64) -> Result<(), Rollback> {
    if pending(ctx, guid) || ctx.db.game_auction().iter().next().is_some() {
        return Err(Rollback::reply(
            unavailable(),
            "Auction transaction or migration is pending",
        ));
    }
    Ok(())
}

fn queue(
    ctx: &ReducerContext,
    operation: u64,
    guid: u64,
    connection: u128,
    request: Vec<u8>,
) -> Result<(), String> {
    let realm = realm_shard(ctx).ok_or("World Shard has no Realm Shard")?;
    ctx.db
        .game_realm_auction_request()
        .insert(RealmAuctionRequest {
            id: 0,
            database: remote::Identity(realm),
            operation,
            guid,
            connection,
            request,
        });
    Ok(())
}

fn start(
    ctx: &ReducerContext,
    operation: u64,
    guid: u64,
    connection: ConnectionId,
    request: Request,
    copper: u32,
) -> Result<(), String> {
    let request = encode(&request);
    queue(ctx, operation, guid, connection.to_u128(), request.clone())?;
    ctx.db
        .game_auction_transaction()
        .insert(AuctionTransaction {
            operation,
            guid,
            connection: connection.to_u128(),
            request,
            copper,
            outcome: None,
        });
    Ok(())
}

fn operation(ctx: &ReducerContext) -> Result<u64, String> {
    Ok(*crate::auth::reserve_guids(ctx, 1, u64::MAX)?.start())
}

fn text(ctx: &ReducerContext, id: u32) -> Result<Option<String>, String> {
    if id == 0 {
        return Ok(None);
    }
    ctx.db
        .game_item_text()
        .id()
        .find(id)
        .map(|row| Some(row.text))
        .ok_or("Listing is missing its Item Text".into())
}

fn import_text(ctx: &ReducerContext, id: u32, text: Option<String>) -> Result<u32, String> {
    match (id, text) {
        (0, None) => Ok(0),
        (_, Some(text)) => crate::mail_text::import_text(ctx, text, 1),
        (_, None) => Err("Listing is missing its Item Text".into()),
    }
}

fn debit(ctx: &ReducerContext, guid: u64, copper: u32) -> Result<(), AuctionRefusal> {
    let mut actor =
        crate::helpers::acting_entity_by_guid(ctx, guid).ok_or(AuctionRefusal::NotEnoughMoney)?;
    actor.money = actor
        .money
        .checked_sub(copper)
        .ok_or(AuctionRefusal::NotEnoughMoney)?;
    actor.update(ctx);
    Ok(())
}

pub(super) fn browse(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    auctioneer: u64,
    body: &[u8],
) -> Result<(), Rollback> {
    query(ctx, connection, guid, auctioneer, 0, body.to_vec())
}

fn query(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    auctioneer: u64,
    kind: u8,
    body: Vec<u8>,
) -> Result<(), Rollback> {
    ready(ctx, guid)?;
    let Some(house) = auctions::auction_house_for_interaction(ctx, guid, auctioneer) else {
        if kind == 0 {
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(
                    lyracore_protocol::codec::build_auction_list_result(
                        &[],
                        0,
                        crate::time::now(ctx).to_micros_since_unix_epoch(),
                    ),
                )),
                false,
            )?;
        }
        return Ok(());
    };
    let viewer = wire::viewer(ctx, guid).ok_or("Auction viewer is missing")?;
    queue(
        ctx,
        operation(ctx)?,
        guid,
        connection.to_u128(),
        encode(&Request::Query(QueryRequest {
            viewer,
            house: house.id,
            kind,
            body,
        })),
    )?;
    Ok(())
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    ready(ctx, guid)?;
    use ClientOpcodeMessage as C;
    let operation = operation(ctx)?;
    match message {
        C::CMSG_AUCTION_LIST_OWNER_ITEMS(request) => query(
            ctx,
            connection,
            guid,
            request.auctioneer.guid(),
            1,
            request.list_from.to_le_bytes().to_vec(),
        ),
        C::CMSG_AUCTION_LIST_BIDDER_ITEMS(request) => query(
            ctx,
            connection,
            guid,
            request.auctioneer.guid(),
            2,
            request.start_from_page.to_le_bytes().to_vec(),
        ),
        C::CMSG_AUCTION_SELL_ITEM(request) => {
            if ctx
                .db
                .game_item_instance()
                .guid()
                .find(request.item.guid())
                .is_none()
            {
                let house =
                    auctions::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
                        .ok_or_else(|| {
                        Rollback::reply(
                            wire::command(
                                0,
                                Action::Started {
                                    result2: ResultTwo::ErrDatabase,
                                },
                            ),
                            "auctioneer unavailable",
                        )
                    })?;
                if let Some(receipt) = ctx
                    .db
                    .game_auction_operation_receipt()
                    .by_actor()
                    .filter(guid)
                    .find(|row| {
                        row.item_guid == request.item.guid()
                            && row.house == house.id
                            && row.start_bid == request.starting_bid
                            && row.buyout == request.buyout
                            && row.duration_minutes == request.auction_duration_in_minutes
                    })
                {
                    start(
                        ctx,
                        operation,
                        guid,
                        connection,
                        Request::RepeatListing(receipt),
                        0,
                    )?;
                    return Ok(());
                }
            }
            let receipt = auctions::realm::reserve_listing(
                ctx,
                operation,
                guid,
                request.auctioneer.guid(),
                request.item.guid(),
                auctions::ListingTerms {
                    start_bid: request.starting_bid,
                    buyout: request.buyout,
                    duration_minutes: request.auction_duration_in_minutes,
                },
            )
            .map_err(|error| {
                Rollback::reply(
                    wire::command(
                        0,
                        Action::Started {
                            result2: wire::result_two(error),
                        },
                    ),
                    error.as_tag(),
                )
            })?;
            let copper = receipt.deposit;
            let text = text(ctx, receipt.item_text_id)?;
            start(
                ctx,
                operation,
                guid,
                connection,
                Request::List(ListingRequest { receipt, text }),
                copper,
            )?;
            Ok(())
        }
        C::CMSG_AUCTION_PLACE_BID(request) => {
            let house =
                auctions::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
                    .ok_or_else(|| {
                        Rollback::reply(
                            wire::bid_refusal(
                                request.auction_id,
                                &auctions::native::BidRefusal::Auction(AuctionRefusal::Database),
                            ),
                            "auctioneer unavailable",
                        )
                    })?;
            let offer = request.price.as_int();
            if offer == 0 || request.auction_id == 0 {
                return Err(Rollback::reply(unavailable(), "invalid bid"));
            }
            debit(ctx, guid, offer).map_err(|error| {
                Rollback::reply(
                    wire::bid_refusal(request.auction_id, &error.into()),
                    error.as_tag(),
                )
            })?;
            start(
                ctx,
                operation,
                guid,
                connection,
                Request::Bid(BidRequest {
                    house: house.id,
                    id: request.auction_id,
                    offer,
                }),
                offer,
            )?;
            Ok(())
        }
        C::CMSG_AUCTION_REMOVE_ITEM(request) => {
            let Some(house) =
                auctions::auction_house_for_interaction(ctx, guid, request.auctioneer.guid())
            else {
                return Ok(());
            };
            start(
                ctx,
                operation,
                guid,
                connection,
                Request::Quote(CancellationQuote {
                    house: house.id,
                    id: request.auction_id,
                }),
                0,
            )?;
            Ok(())
        }
        _ => Err("opcode is not an auction request".into()),
    }
}

#[reducer]
pub fn idc_realm_auction_request(
    ctx: &ReducerContext,
    operation: u64,
    guid: u64,
    connection: u128,
    request: Vec<u8>,
) -> Result<(), String> {
    directory::require_world(ctx)?;
    let request: Request = bsatn::from_slice(&request).map_err(|error| error.to_string())?;
    if ctx.db.game_auction_deletion().guid().find(guid).is_some() {
        return Err("Character deletion is pending".into());
    }
    if !matches!(&request, Request::Import(ListingImport { .. }))
        && ctx
            .db
            .game_character_directory()
            .guid()
            .find(guid)
            .is_none_or(|entry| entry.world != ctx.sender())
    {
        return Err("World Shard does not own this Character".into());
    }
    let answer = match request {
        Request::Query(QueryRequest {
            viewer,
            house,
            kind,
            body,
        }) => {
            if viewer.guid != guid {
                return Err("Auction viewer changed".into());
            }
            Answer::Query(packet(wire::realm_page(ctx, &viewer, house, kind, &body)?))
        }
        Request::RepeatListing(receipt) => {
            if receipt.actor_guid != guid
                || ctx
                    .db
                    .game_auction()
                    .id()
                    .find(receipt.auction_id)
                    .is_none_or(|row| {
                        row.owner_guid != guid || row.listing_operation_id != receipt.operation_id
                    })
            {
                return Err(Rollback::reply(
                    wire::command(
                        0,
                        Action::Started {
                            result2: ResultTwo::ErrItemNotFound,
                        },
                    ),
                    "Listing is no longer active",
                )
                .into_reducer_error());
            }
            Answer::Complete(Completion {
                spent: 0,
                packets: packet(wire::command(
                    receipt.auction_id,
                    Action::Started {
                        result2: ResultTwo::Ok,
                    },
                )),
            })
        }
        Request::List(ListingRequest { mut receipt, text }) => {
            if receipt.actor_guid != guid || receipt.operation_id != operation {
                return Err("Listing owner changed".into());
            }
            receipt.item_text_id = import_text(ctx, receipt.item_text_id, text)?;
            let id = auctions::realm::accept_listing(ctx, receipt);
            Answer::Listed(id)
        }
        Request::Bid(BidRequest { house, id, offer }) => {
            let spent =
                auctions::realm::bid(ctx, operation, guid, house, id, offer).map_err(|error| {
                    Rollback::reply(wire::bid_refusal(id, &error), "bid refused")
                        .into_reducer_error()
                })?;
            Answer::Complete(Completion {
                spent,
                packets: packet(wire::command(
                    id,
                    Action::BidPlaced {
                        result: BidResult::Ok {
                            auction_outbid1: bid_increment(spent),
                        },
                    },
                )),
            })
        }
        Request::Quote(CancellationQuote { house, id }) => Answer::Quote(
            auctions::realm::cancellation_cut(ctx, guid, house, id).map_err(|error| {
                Rollback::reply(
                    wire::command(
                        0,
                        Action::Removed {
                            result2: ResultTwo::ErrDatabase,
                        },
                    ),
                    error.as_tag(),
                )
                .into_reducer_error()
            })?,
        ),
        Request::Cancel(Cancellation { house, id, paid }) => {
            auctions::realm::cancel(ctx, operation, guid, house, id, paid).map_err(|error| {
                Rollback::reply(
                    wire::command(
                        0,
                        Action::Removed {
                            result2: ResultTwo::ErrDatabase,
                        },
                    ),
                    error.as_tag(),
                )
                .into_reducer_error()
            })?;
            Answer::Complete(Completion {
                spent: paid,
                packets: packet(wire::command(
                    id,
                    Action::Removed {
                        result2: ResultTwo::Ok,
                    },
                )),
            })
        }
        Request::Import(ListingImport {
            mut listing,
            mut receipt,
            text,
        }) => {
            if listing.owner_guid != guid || listing.item_text_id != receipt.item_text_id {
                return Err("Listing import owner or text changed".into());
            }
            if ctx.db.game_native_away().iter().next().is_some() {
                return Err("Listing import requires closed gameplay".into());
            }
            let text = import_text(ctx, listing.item_text_id, text)?;
            listing.item_text_id = text;
            receipt.item_text_id = text;
            auctions::realm::import(ctx, listing, receipt)?;
            Answer::Complete(Completion {
                spent: 0,
                packets: Vec::new(),
            })
        }
    };
    deliver(
        ctx,
        ctx.sender(),
        operation,
        guid,
        connection,
        encode(&answer),
    );
    Ok(())
}

fn deliver(
    ctx: &ReducerContext,
    database: Identity,
    operation: u64,
    guid: u64,
    connection: u128,
    answer: Vec<u8>,
) {
    ctx.db
        .game_world_auction_answer()
        .insert(WorldAuctionAnswer {
            id: 0,
            database: remote::Identity(database),
            operation,
            guid,
            connection,
            answer,
        });
}

fn send(ctx: &ReducerContext, guid: u64, connection: u128, packets: Vec<AuctionPacket>) {
    let connection = ConnectionId::from_u128(connection);
    if ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection)
        .is_some_and(|player| player.character_guid == guid)
    {
        for AuctionPacket { opcode, body } in packets {
            super::send_raw(ctx, connection, opcode, body, false);
        }
    }
}

#[reducer]
pub fn idc_world_auction_answer(
    ctx: &ReducerContext,
    operation: u64,
    guid: u64,
    connection: u128,
    answer: Vec<u8>,
) -> Result<(), String> {
    require_sender(ctx, realm_shard(ctx))?;
    let answer: Answer = bsatn::from_slice(&answer).map_err(|error| error.to_string())?;
    if let Answer::Query(packets) = answer {
        send(ctx, guid, connection, packets);
        return Ok(());
    }
    let Some(mut row) = ctx
        .db
        .game_auction_transaction()
        .operation()
        .find(operation)
        .filter(|row| row.guid == guid && row.outcome.is_none())
    else {
        return Ok(());
    };
    match answer {
        Answer::Listed(id) => {
            let Request::List(ListingRequest { mut receipt, .. }) =
                bsatn::from_slice(&row.request).map_err(|error| error.to_string())?
            else {
                return Err("Listing answer does not match its request".into());
            };
            receipt.auction_id = id;
            ctx.db.game_auction_operation_receipt().insert(receipt);
            let spent = row.copper;
            finish(
                ctx,
                row,
                true,
                spent,
                packet(wire::command(
                    id,
                    Action::Started {
                        result2: ResultTwo::Ok,
                    },
                )),
            )?;
        }
        Answer::Quote(copper) => {
            let Request::Quote(CancellationQuote { house, id }) =
                bsatn::from_slice(&row.request).map_err(|error| error.to_string())?
            else {
                return Ok(());
            };
            if debit(ctx, guid, copper).is_err() {
                finish(ctx, row, false, 0, Vec::new())?;
                return Ok(());
            }
            row.request = encode(&Request::Cancel(Cancellation {
                house,
                id,
                paid: copper,
            }));
            row.copper = copper;
            queue(ctx, operation, guid, connection, row.request.clone())?;
            ctx.db.game_auction_transaction().operation().update(row);
        }
        Answer::Complete(Completion { spent, packets }) => finish(ctx, row, true, spent, packets)?,
        Answer::Query(_) => unreachable!(),
    }
    Ok(())
}

#[reducer(on_result(game_realm_auction_request))]
pub fn realm_auction_request_answered(
    ctx: &ReducerContext,
    call: RealmAuctionRequest,
    result: Result<(), String>,
) -> Result<(), String> {
    let Err(error) = result else { return Ok(()) };
    let packets = super::rollback::replies_in(&error)
        .map(|packets| {
            packets
                .into_iter()
                .map(|(opcode, body)| AuctionPacket { opcode, body })
                .collect()
        })
        .unwrap_or_else(|| packet(unavailable()));
    if let Some(row) = ctx
        .db
        .game_auction_transaction()
        .operation()
        .find(call.operation)
        .filter(|row| row.outcome.is_none() && row.request == call.request)
    {
        finish(ctx, row, false, 0, packets)?;
    } else {
        send(ctx, call.guid, call.connection, packets);
    }
    Ok(())
}

fn finish(
    ctx: &ReducerContext,
    mut row: AuctionTransaction,
    accepted: bool,
    spent: u32,
    packets: Vec<AuctionPacket>,
) -> Result<(), String> {
    row.copper = row
        .copper
        .checked_sub(spent)
        .ok_or("Auction charge exceeds reserved copper")?;
    let operation = row.operation;
    row.outcome = Some(AuctionOutcome { accepted, packets });
    ctx.db.game_auction_transaction().operation().update(row);
    settle(ctx, operation)
}

fn refund(ctx: &ReducerContext, guid: u64, copper: u32) -> u32 {
    if let Some(mut entity) = crate::helpers::acting_entity_by_guid(ctx, guid) {
        let credited = copper.min(u32::MAX - entity.money);
        entity.money += credited;
        entity.update(ctx);
        copper - credited
    } else if let Some(mut character) = crate::helpers::character_by_guid(ctx, guid) {
        let credited = copper.min(u32::MAX - character.money);
        character.money += credited;
        ctx.db.game_character().guid().update(character);
        copper - credited
    } else {
        copper
    }
}

fn settle(ctx: &ReducerContext, operation: u64) -> Result<(), String> {
    let Some(mut row) = ctx
        .db
        .game_auction_transaction()
        .operation()
        .find(operation)
    else {
        return Ok(());
    };
    let Some(outcome) = &row.outcome else {
        return Ok(());
    };
    row.copper = refund(ctx, row.guid, row.copper);
    if row.copper != 0 {
        ctx.db.game_auction_transaction().operation().update(row);
        later(ctx, Retry::Settle(operation));
        return Ok(());
    }
    let request: Request = bsatn::from_slice(&row.request).map_err(|error| error.to_string())?;
    match request {
        Request::List(ListingRequest { receipt, .. }) => {
            if outcome.accepted {
                crate::mail_text::drop_letter_text(ctx, receipt.item_text_id);
            } else {
                auctions::realm::return_listing(ctx, receipt);
            }
        }
        Request::Import(ListingImport { listing, .. }) if outcome.accepted => {
            let original = ctx
                .db
                .game_auction()
                .id()
                .find(listing.id)
                .ok_or("Imported Listing disappeared from its source")?;
            if encode(&original) != encode(&listing) {
                return Err("Imported Listing changed on its source".into());
            }
            ctx.db.game_auction_expiry().auction_id().delete(listing.id);
            ctx.db.game_auction().id().delete(listing.id);
            ctx.db
                .game_auction_operation_receipt()
                .operation_id()
                .delete(listing.listing_operation_id);
            crate::mail_text::drop_letter_text(ctx, listing.item_text_id);
        }
        _ => {}
    }
    send(
        ctx,
        row.guid,
        row.connection,
        row.outcome.expect("settlement has an outcome").packets,
    );
    ctx.db
        .game_auction_transaction()
        .operation()
        .delete(operation);
    Ok(())
}

fn later(ctx: &ReducerContext, retry: Retry) {
    ctx.db.game_auction_retry().insert(AuctionRetry {
        id: 0,
        scheduled_at: (ctx.timestamp + TimeDuration::from_micros(1_000_000)).into(),
        retry,
    });
}

#[reducer(on_result(game_world_auction_answer))]
pub fn world_auction_answered(
    ctx: &ReducerContext,
    call: WorldAuctionAnswer,
    result: Result<(), String>,
) {
    if let Err(error) = result {
        spacetimedb::log::warn!(
            "Auction result {} to {} waits: {error}",
            call.operation,
            call.database.0
        );
        later(
            ctx,
            Retry::Deliver(AuctionDelivery {
                database: call.database.0,
                operation: call.operation,
                guid: call.guid,
                connection: call.connection,
                answer: call.answer,
            }),
        );
    }
}

#[reducer]
pub fn retry_auction(ctx: &ReducerContext, retry: AuctionRetry) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".into());
    }
    match retry.retry {
        Retry::Settle(operation) => settle(ctx, operation)?,
        Retry::Deliver(AuctionDelivery {
            database,
            operation,
            guid,
            connection,
            answer,
        }) => deliver(ctx, database, operation, guid, connection, answer),
    }
    Ok(())
}

pub(super) fn notice(ctx: &ReducerContext, guid: u64, message: ServerOpcodeMessage) {
    let Some(entry) = ctx.db.game_character_directory().guid().find(guid) else {
        return;
    };
    let Some(away) = ctx
        .db
        .game_native_away()
        .account_id()
        .find(entry.account_id)
        .filter(|away| away.character_guid == guid)
    else {
        return;
    };
    super::realm_communication::deliver(
        ctx,
        super::realm_communication::Participant {
            guid,
            account_id: entry.account_id,
            connection: away.connection_id.to_u128(),
        },
        packet(message)
            .into_iter()
            .map(|packet| (packet.opcode, packet.body))
            .collect(),
        None,
    );
}

#[reducer]
pub fn operator_route_auctions(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if realm_shard(ctx).is_none() || ctx.db.game_native_player().iter().next().is_some() {
        return Err("Listing migration requires a World Shard with gameplay closed".into());
    }
    let waiting: std::collections::BTreeSet<_> = ctx
        .db
        .game_auction_transaction()
        .iter()
        .map(|row| row.guid)
        .collect();
    for listing in ctx.db.game_auction().iter() {
        if waiting.contains(&listing.owner_guid) {
            continue;
        }
        let receipt = ctx
            .db
            .game_auction_operation_receipt()
            .operation_id()
            .find(listing.listing_operation_id)
            .ok_or("Listing has no receipt")?;
        let text = text(ctx, listing.item_text_id)?;
        start(
            ctx,
            operation(ctx)?,
            listing.owner_guid,
            ConnectionId::from_u128(0),
            Request::Import(ListingImport {
                listing,
                receipt,
                text,
            }),
            0,
        )?;
    }
    Ok(())
}

#[derive(SpacetimeType)]
pub struct QueryRequest {
    viewer: wire::Viewer,
    house: u32,
    kind: u8,
    body: Vec<u8>,
}

#[derive(SpacetimeType)]
pub struct ListingRequest {
    receipt: AuctionOperationReceipt,
    text: Option<String>,
}

#[derive(SpacetimeType)]
pub struct BidRequest {
    house: u32,
    id: u32,
    offer: u32,
}

#[derive(SpacetimeType)]
pub struct CancellationQuote {
    house: u32,
    id: u32,
}

#[derive(SpacetimeType)]
pub struct Cancellation {
    house: u32,
    id: u32,
    paid: u32,
}

#[derive(SpacetimeType)]
pub struct ListingImport {
    listing: Auction,
    receipt: AuctionOperationReceipt,
    text: Option<String>,
}

#[derive(SpacetimeType)]
pub struct Completion {
    spent: u32,
    packets: Vec<AuctionPacket>,
}

#[derive(SpacetimeType)]
pub struct AuctionDelivery {
    database: Identity,
    operation: u64,
    guid: u64,
    connection: u128,
    answer: Vec<u8>,
}

#[derive(SpacetimeType)]
pub struct AuctionPacket {
    opcode: u16,
    body: Vec<u8>,
}
