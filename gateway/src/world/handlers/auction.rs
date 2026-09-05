//! Auction-house protocol family.

use super::super::*;
use lyracore_shared::auction::AuctionRefusal;
use spacetimedb_sdk::Table;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuctionHousePolicy {
    pub(crate) id: u32,
    pub(crate) deposit_rate: u32,
    pub(crate) consignment_rate: u32,
}

/// What an auction packet needs from the auctioneer: the house it serves, and whether its faction
/// refuses to talk to this Character. Every other interaction condition belongs to the Module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuctionInteraction {
    pub(crate) house: AuctionHousePolicy,
    pub(crate) refuses_interaction: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CreateAuctionRequest {
    pub(crate) actor_guid: u64,
    pub(crate) auctioneer_guid: u64,
    pub(crate) item_guid: u64,
    pub(crate) start_bid: u32,
    pub(crate) buyout: u32,
    pub(crate) duration_minutes: u32,
    pub(crate) house_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CreateAuctionOutcome {
    Created { auction_id: u32 },
    ItemNotFound,
    NotEnoughMoney,
    Database,
}

// The vanilla client has no code for invalid terms; it reads as the generic database failure.
impl From<AuctionRefusal> for CreateAuctionOutcome {
    fn from(refusal: AuctionRefusal) -> Self {
        match refusal {
            AuctionRefusal::ItemNotFound => Self::ItemNotFound,
            AuctionRefusal::NotEnoughMoney => Self::NotEnoughMoney,
            AuctionRefusal::InvalidTerms | AuctionRefusal::Database => Self::Database,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlaceBidRequest {
    pub(crate) actor_guid: u64,
    pub(crate) auctioneer_guid: u64,
    pub(crate) auction_id: u32,
    pub(crate) offer: u32,
    pub(crate) house_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlaceBidOutcome {
    Accepted {
        minimum_increment: u32,
    },
    ItemNotFound,
    NotEnoughMoney,
    HigherBid {
        bidder_guid: u64,
        current_bid: u32,
        minimum_increment: u32,
    },
    BidIncrement,
    BidOwn,
    Database,
}

impl From<AuctionRefusal> for PlaceBidOutcome {
    fn from(refusal: AuctionRefusal) -> Self {
        match refusal {
            AuctionRefusal::ItemNotFound => Self::ItemNotFound,
            AuctionRefusal::NotEnoughMoney => Self::NotEnoughMoney,
            AuctionRefusal::InvalidTerms | AuctionRefusal::Database => Self::Database,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuctionBrowseRequest {
    pub(crate) auctioneer_guid: u64,
    pub(crate) offset: u32,
    pub(crate) name: String,
    pub(crate) minimum_level: Option<u8>,
    pub(crate) maximum_level: Option<u8>,
    pub(crate) inventory_type: Option<u32>,
    pub(crate) item_class: Option<u32>,
    pub(crate) item_subclass: Option<u32>,
    pub(crate) quality: Option<u8>,
    pub(crate) usable_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AuctionQuery {
    Browse(AuctionBrowseRequest),
    Owner {
        offset: u32,
    },
    Bidder {
        offset: u32,
        outbid_auction_ids: Vec<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuctionPage {
    pub(crate) rows: Vec<codec::AuctionView>,
    pub(crate) total: u32,
    pub(crate) now_micros: i64,
}

pub(crate) trait AuctionActionStore: Send + Sync {
    fn auction_interaction(
        &self,
        player_guid: u64,
        auctioneer_guid: u64,
    ) -> Result<Option<AuctionInteraction>>;

    fn create_auction(&self, request: CreateAuctionRequest) -> Result<CreateAuctionOutcome>;

    fn place_bid(&self, request: PlaceBidRequest) -> Result<PlaceBidOutcome>;

    fn auction_query(
        &self,
        player_guid: u64,
        house_id: u32,
        query: AuctionQuery,
    ) -> Result<AuctionPage>;
}

impl AuctionActionStore for crate::stdb::Coordinator {
    fn auction_interaction(
        &self,
        player_guid: u64,
        auctioneer_guid: u64,
    ) -> Result<Option<AuctionInteraction>> {
        use crate::stdb::bindings::{
            GameAuctionHouseTableAccess, GameFactionTemplateTableAccess, GameWorldEntityTableAccess,
        };
        let house = {
            let guard = self.0.coord();
            let db = &guard.conn.db;
            let Some(auctioneer) = db.game_world_entity().guid().find(&auctioneer_guid) else {
                return Ok(None);
            };
            let Some(faction) = db
                .game_faction_template()
                .id()
                .find(&auctioneer.faction_template)
                .map(|template| template.faction)
            else {
                return Ok(None);
            };
            let Some(house) = db
                .game_auction_house()
                .iter()
                .find(|house| house.faction == faction)
                .map(|house| AuctionHousePolicy {
                    id: house.id,
                    deposit_rate: house.deposit_rate,
                    consignment_rate: house.consignment_rate,
                })
            else {
                return Ok(None);
            };
            house
        };
        let refuses_interaction = self.npc_refuses_interaction(auctioneer_guid, player_guid)?;
        Ok(Some(AuctionInteraction {
            house,
            refuses_interaction,
        }))
    }

    fn create_auction(&self, request: CreateAuctionRequest) -> Result<CreateAuctionOutcome> {
        crate::stdb::Coordinator::create_auction(self, request)
    }

    fn place_bid(&self, request: PlaceBidRequest) -> Result<PlaceBidOutcome> {
        crate::stdb::Coordinator::place_bid(self, request)
    }

    fn auction_query(
        &self,
        player_guid: u64,
        house_id: u32,
        query: AuctionQuery,
    ) -> Result<AuctionPage> {
        crate::stdb::Coordinator::auction_query(self, player_guid, house_id, query)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuctionActionPlayer {
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum AuctionActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

enum AuctionRequest {
    Hello(wow_world_messages::vanilla::Guid),
    Owner(u32),
    Bidder {
        offset: u32,
        outbid_auction_ids: Vec<u32>,
    },
}

pub(crate) const CMSG_AUCTION_LIST_ITEMS_OPCODE: u32 = 0x0258;

/// Decode build-5875 browse bodies locally because the protocol dependency narrows the wire's
/// `u32` quality to an enum before callers can observe the real client's `0xffff_ffff` sentinel.
pub(crate) fn decode_auction_browse(body: &[u8]) -> Result<AuctionBrowseRequest> {
    if !(32..=287).contains(&body.len()) {
        return Err(anyhow!(
            "invalid CMSG_AUCTION_LIST_ITEMS body size {}",
            body.len()
        ));
    }
    let auctioneer_guid = u64::from_le_bytes(body[0..8].try_into().unwrap());
    let offset = u32::from_le_bytes(body[8..12].try_into().unwrap());
    let name_end = body[12..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|position| position + 12)
        .ok_or_else(|| anyhow!("CMSG_AUCTION_LIST_ITEMS name is not terminated"))?;
    let name = std::str::from_utf8(&body[12..name_end])
        .map_err(|_| anyhow!("CMSG_AUCTION_LIST_ITEMS name is not UTF-8"))?
        .to_owned();
    let fields = &body[name_end + 1..];
    if fields.len() != 19 {
        return Err(anyhow!(
            "invalid CMSG_AUCTION_LIST_ITEMS trailing size {}",
            fields.len()
        ));
    }
    let minimum_level = fields[0];
    let maximum_level = fields[1];
    let inventory_type = u32::from_le_bytes(fields[2..6].try_into().unwrap());
    let item_class = u32::from_le_bytes(fields[6..10].try_into().unwrap());
    let item_subclass = u32::from_le_bytes(fields[10..14].try_into().unwrap());
    let quality = match u32::from_le_bytes(fields[14..18].try_into().unwrap()) {
        u32::MAX => None,
        value @ 0..=6 => Some(value as u8),
        value => return Err(anyhow!("invalid auction quality {value:#x}")),
    };
    let optional_filter = |value| (value != u32::MAX).then_some(value);
    Ok(AuctionBrowseRequest {
        auctioneer_guid,
        offset,
        name,
        minimum_level: (minimum_level != 0).then_some(minimum_level),
        maximum_level: (maximum_level != 0).then_some(maximum_level),
        inventory_type: optional_filter(inventory_type),
        item_class: optional_filter(item_class),
        item_subclass: optional_filter(item_subclass),
        quality,
        usable_only: fields[18] != 0,
    })
}

pub(crate) fn dispatch_auction_browse_action<St: AuctionActionStore + ?Sized>(
    store: &St,
    player: AuctionActionPlayer,
    request: AuctionBrowseRequest,
) -> Result<AuctionActionOutcome> {
    let Some((player_guid, interaction)) =
        auction_actor_interaction(store, player, request.auctioneer_guid)?
            .filter(|(_, interaction)| !interaction.refuses_interaction)
    else {
        return Ok(AuctionActionOutcome::Handled {
            outbound: vec![Outbound::One(
                ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(
                    codec::build_auction_list_result(&[], 0, 0),
                )),
            )],
        });
    };
    let page = store.auction_query(
        player_guid,
        interaction.house.id,
        AuctionQuery::Browse(request),
    )?;
    Ok(AuctionActionOutcome::Handled {
        outbound: vec![Outbound::One(
            ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(
                codec::build_auction_list_result(&page.rows, page.total, page.now_micros),
            )),
        )],
    })
}

/// Resolve the Character and auction house. Read paths apply the faction verdict; Durable
/// Requests leave the interaction Gate to the Module.
fn auction_actor_interaction<St: AuctionActionStore + ?Sized>(
    store: &St,
    player: AuctionActionPlayer,
    auctioneer_guid: u64,
) -> Result<Option<(u64, AuctionInteraction)>> {
    let Some(player_guid) = player.self_guid else {
        return Ok(None);
    };
    // A missing auctioneer or house is `None`; a failed Durable Read is a failure, not a Refusal.
    let interaction = store.auction_interaction(player_guid, auctioneer_guid)?;
    Ok(interaction.map(|interaction| (player_guid, interaction)))
}

fn create_result(outcome: CreateAuctionOutcome) -> AuctionActionOutcome {
    use wow_world_messages::vanilla::{
        SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction as Action,
        SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo as ResultTwo,
        SMSG_AUCTION_COMMAND_RESULT,
    };
    let (auction_id, result2) = match outcome {
        CreateAuctionOutcome::Created { auction_id } => (auction_id, ResultTwo::Ok),
        CreateAuctionOutcome::ItemNotFound => (0, ResultTwo::ErrItemNotFound),
        CreateAuctionOutcome::NotEnoughMoney => (0, ResultTwo::ErrNotEnoughMoney),
        CreateAuctionOutcome::Database => (0, ResultTwo::ErrDatabase),
    };
    AuctionActionOutcome::Handled {
        outbound: vec![Outbound::One(
            ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(Box::new(
                SMSG_AUCTION_COMMAND_RESULT {
                    auction_id,
                    action: Action::Started { result2 },
                },
            )),
        )],
    }
}

fn bid_result(auction_id: u32, outcome: PlaceBidOutcome) -> AuctionActionOutcome {
    use wow_world_messages::{
        vanilla::{
            SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction as Action,
            SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult as ResultOne,
            SMSG_AUCTION_COMMAND_RESULT,
        },
        Guid,
    };
    let result = match outcome {
        PlaceBidOutcome::Accepted { minimum_increment } => ResultOne::Ok {
            auction_outbid1: minimum_increment,
        },
        PlaceBidOutcome::ItemNotFound => ResultOne::ErrItemNotFound,
        PlaceBidOutcome::NotEnoughMoney => ResultOne::ErrNotEnoughMoney,
        PlaceBidOutcome::HigherBid {
            bidder_guid,
            current_bid,
            minimum_increment,
        } => ResultOne::ErrHigherBid {
            auction_outbid2: minimum_increment,
            higher_bidder: Guid::new(bidder_guid),
            new_bid: current_bid,
        },
        PlaceBidOutcome::BidIncrement => ResultOne::ErrBidIncrement,
        PlaceBidOutcome::BidOwn => ResultOne::ErrBidOwn,
        PlaceBidOutcome::Database => ResultOne::ErrDatabase,
    };
    AuctionActionOutcome::Handled {
        outbound: vec![Outbound::One(
            ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(Box::new(
                SMSG_AUCTION_COMMAND_RESULT {
                    auction_id,
                    action: Action::BidPlaced { result },
                },
            )),
        )],
    }
}

pub(crate) fn dispatch_auction_action<St: AuctionActionStore + ?Sized>(
    store: &St,
    player: AuctionActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<AuctionActionOutcome> {
    let (auctioneer, request) = match msg {
        ClientOpcodeMessage::MSG_AUCTION_HELLO(message) => (
            message.auctioneer,
            AuctionRequest::Hello(message.auctioneer),
        ),
        ClientOpcodeMessage::CMSG_AUCTION_LIST_ITEMS(message) => {
            return dispatch_auction_browse_action(
                store,
                player,
                AuctionBrowseRequest {
                    auctioneer_guid: message.auctioneer.guid(),
                    offset: message.list_start_item,
                    name: message.searched_name,
                    minimum_level: (message.minimum_level != 0).then_some(message.minimum_level),
                    maximum_level: (message.maximum_level != 0).then_some(message.maximum_level),
                    inventory_type: (message.auction_slot_id != u32::MAX)
                        .then_some(message.auction_slot_id),
                    item_class: (message.auction_main_category != u32::MAX)
                        .then_some(message.auction_main_category),
                    item_subclass: (message.auction_sub_category != u32::MAX)
                        .then_some(message.auction_sub_category),
                    quality: Some(message.auction_quality.as_int()),
                    usable_only: message.usable != 0,
                },
            );
        }
        ClientOpcodeMessage::CMSG_AUCTION_LIST_OWNER_ITEMS(message) => {
            (message.auctioneer, AuctionRequest::Owner(message.list_from))
        }
        ClientOpcodeMessage::CMSG_AUCTION_LIST_BIDDER_ITEMS(message) => (
            message.auctioneer,
            AuctionRequest::Bidder {
                offset: message.start_from_page,
                outbid_auction_ids: message.outbid_item_ids,
            },
        ),
        ClientOpcodeMessage::CMSG_AUCTION_PLACE_BID(message) => {
            let auction_id = message.auction_id;
            let auctioneer_guid = message.auctioneer.guid();
            let Some((player_guid, interaction)) =
                auction_actor_interaction(store, player, auctioneer_guid)?
            else {
                return Ok(bid_result(auction_id, PlaceBidOutcome::Database));
            };
            // A Refusal arrives as an outcome. An error is a failure with an unknown durable
            // result, so it ends the session instead of posing as a gameplay answer.
            let outcome = store.place_bid(PlaceBidRequest {
                actor_guid: player_guid,
                auctioneer_guid,
                auction_id,
                offer: message.price.as_int(),
                house_id: interaction.house.id,
            })?;
            return Ok(bid_result(auction_id, outcome));
        }
        ClientOpcodeMessage::CMSG_AUCTION_SELL_ITEM(message) => {
            let auctioneer_guid = message.auctioneer.guid();
            let Some((player_guid, interaction)) =
                auction_actor_interaction(store, player, auctioneer_guid)?
            else {
                return Ok(create_result(CreateAuctionOutcome::Database));
            };
            let outcome = store.create_auction(CreateAuctionRequest {
                actor_guid: player_guid,
                auctioneer_guid,
                item_guid: message.item.guid(),
                start_bid: message.starting_bid,
                buyout: message.buyout,
                duration_minutes: message.auction_duration_in_minutes,
                house_id: interaction.house.id,
            })?;
            return Ok(create_result(outcome));
        }
        other => return Ok(AuctionActionOutcome::PassThrough(other)),
    };
    let auctioneer_guid = auctioneer.guid();
    let Some((player_guid, interaction)) = auction_actor_interaction(store, player, auctioneer_guid)?
        .filter(|(_, interaction)| !interaction.refuses_interaction)
    else {
        return Ok(AuctionActionOutcome::Handled {
            outbound: Vec::new(),
        });
    };
    let house = interaction.house;
    use wow_world_messages::vanilla::{AuctionHouse, MSG_AUCTION_HELLO_Server};
    let message = match request {
        AuctionRequest::Hello(auctioneer) => {
            ServerOpcodeMessage::MSG_AUCTION_HELLO(Box::new(MSG_AUCTION_HELLO_Server {
                auctioneer,
                auction_house: AuctionHouse::try_from(house.id)
                    .map_err(|error| anyhow!("imported auction house {}: {error}", house.id))?,
            }))
        }
        AuctionRequest::Owner(offset) => {
            let page =
                store.auction_query(player_guid, house.id, AuctionQuery::Owner { offset })?;
            ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(Box::new(
                codec::build_auction_owner_list_result(&page.rows, page.total, page.now_micros),
            ))
        }
        AuctionRequest::Bidder {
            offset,
            outbid_auction_ids,
        } => {
            let page = store.auction_query(
                player_guid,
                house.id,
                AuctionQuery::Bidder {
                    offset,
                    outbid_auction_ids,
                },
            )?;
            ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(Box::new(
                codec::build_auction_bidder_list_result(&page.rows, page.total, page.now_micros),
            ))
        }
    };
    Ok(AuctionActionOutcome::Handled {
        outbound: vec![Outbound::One(message)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::shared::Gold;
    use wow_world_messages::vanilla::{
        Guid, MSG_AUCTION_HELLO_Client, SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction,
        SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult,
        SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo, CMSG_AUCTION_LIST_BIDDER_ITEMS,
        CMSG_AUCTION_LIST_ITEMS, CMSG_AUCTION_LIST_OWNER_ITEMS, CMSG_AUCTION_PLACE_BID,
        CMSG_AUCTION_SELL_ITEM,
    };

    struct InMemoryAuctionActions {
        result: Mutex<Result<Option<AuctionInteraction>, String>>,
        lookups: Mutex<Vec<(u64, u64)>>,
        creates: Mutex<Vec<CreateAuctionRequest>>,
        create_result: Mutex<Result<CreateAuctionOutcome, String>>,
        bids: Mutex<Vec<PlaceBidRequest>>,
        bid_result: Mutex<Result<PlaceBidOutcome, String>>,
        query_result: Mutex<Result<AuctionPage, String>>,
        queries: Mutex<Vec<(u64, u32, AuctionQuery)>>,
    }

    impl AuctionActionStore for InMemoryAuctionActions {
        fn auction_interaction(
            &self,
            player_guid: u64,
            auctioneer_guid: u64,
        ) -> Result<Option<AuctionInteraction>> {
            self.lookups
                .lock()
                .unwrap()
                .push((player_guid, auctioneer_guid));
            self.result
                .lock()
                .unwrap()
                .as_ref()
                .map(Clone::clone)
                .map_err(|error| anyhow::anyhow!(error.clone()))
        }

        fn create_auction(&self, request: CreateAuctionRequest) -> Result<CreateAuctionOutcome> {
            self.creates.lock().unwrap().push(request);
            self.create_result
                .lock()
                .unwrap()
                .as_ref()
                .copied()
                .map_err(|error| anyhow::anyhow!(error.clone()))
        }

        fn place_bid(&self, request: PlaceBidRequest) -> Result<PlaceBidOutcome> {
            self.bids.lock().unwrap().push(request);
            self.bid_result
                .lock()
                .unwrap()
                .as_ref()
                .copied()
                .map_err(|error| anyhow::anyhow!(error.clone()))
        }

        fn auction_query(
            &self,
            player_guid: u64,
            house_id: u32,
            query: AuctionQuery,
        ) -> Result<AuctionPage> {
            self.queries
                .lock()
                .unwrap()
                .push((player_guid, house_id, query));
            self.query_result
                .lock()
                .unwrap()
                .as_ref()
                .map(Clone::clone)
                .map_err(|error| anyhow::anyhow!(error.clone()))
        }
    }

    fn valid_interaction() -> AuctionInteraction {
        AuctionInteraction {
            house: AuctionHousePolicy {
                id: 4,
                deposit_rate: 5,
                consignment_rate: 5,
            },
            refuses_interaction: false,
        }
    }

    fn store_with(interaction: Option<AuctionInteraction>) -> InMemoryAuctionActions {
        InMemoryAuctionActions {
            result: Mutex::new(Ok(interaction)),
            lookups: Mutex::default(),
            creates: Mutex::default(),
            create_result: Mutex::new(Ok(CreateAuctionOutcome::Created { auction_id: 41 })),
            bids: Mutex::default(),
            bid_result: Mutex::new(Ok(PlaceBidOutcome::Accepted {
                minimum_increment: 6,
            })),
            query_result: Mutex::new(Ok(AuctionPage {
                rows: Vec::new(),
                total: 0,
                now_micros: 0,
            })),
            queries: Mutex::default(),
        }
    }

    fn store_error(error: &str) -> InMemoryAuctionActions {
        InMemoryAuctionActions {
            result: Mutex::new(Err(error.to_string())),
            lookups: Mutex::default(),
            creates: Mutex::default(),
            create_result: Mutex::new(Ok(CreateAuctionOutcome::Database)),
            bids: Mutex::default(),
            bid_result: Mutex::new(Ok(PlaceBidOutcome::Database)),
            query_result: Mutex::new(Ok(AuctionPage {
                rows: Vec::new(),
                total: 0,
                now_micros: 0,
            })),
            queries: Mutex::default(),
        }
    }

    fn raw_browse(quality: u32) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&99_u64.to_le_bytes());
        body.extend_from_slice(&50_u32.to_le_bytes());
        body.extend_from_slice(b"Sword\0");
        body.extend_from_slice(&[10, 20]);
        body.extend_from_slice(&13_u32.to_le_bytes());
        body.extend_from_slice(&2_u32.to_le_bytes());
        body.extend_from_slice(&7_u32.to_le_bytes());
        body.extend_from_slice(&quality.to_le_bytes());
        body.push(1);
        body
    }

    #[test]
    fn raw_browse_preserves_sentinels_and_rejects_unknown_quality_values() {
        assert_eq!(
            decode_auction_browse(&raw_browse(u32::MAX)).unwrap(),
            AuctionBrowseRequest {
                auctioneer_guid: 99,
                offset: 50,
                name: "Sword".to_owned(),
                minimum_level: Some(10),
                maximum_level: Some(20),
                inventory_type: Some(13),
                item_class: Some(2),
                item_subclass: Some(7),
                quality: None,
                usable_only: true,
            }
        );
        assert!(decode_auction_browse(&raw_browse(0x100)).is_err());

        let mut empty = raw_browse(u32::MAX);
        empty[18] = 0;
        empty[19] = 0;
        empty[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        empty[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        empty[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        let decoded = decode_auction_browse(&empty).unwrap();
        assert_eq!(decoded.minimum_level, None);
        assert_eq!(decoded.maximum_level, None);
        assert_eq!(decoded.inventory_type, None);
        assert_eq!(decoded.item_class, None);
        assert_eq!(decoded.item_subclass, None);
    }

    #[test]
    fn faction_refusal_keeps_browse_empty() {
        let store = store_with(Some(AuctionInteraction {
            refuses_interaction: true,
            ..valid_interaction()
        }));
        let outcome = dispatch_auction_browse_action(
            &store,
            AuctionActionPlayer { self_guid: Some(7) },
            decode_auction_browse(&raw_browse(u32::MAX)).unwrap(),
        )
        .unwrap();
        let AuctionActionOutcome::Handled { outbound } = outcome else {
            panic!("browse must be handled")
        };
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(message))]
                if message.total_amount_of_auctions == 0 && message.auctions.is_empty()
        ));
        assert!(store.queries.lock().unwrap().is_empty());
    }

    #[test]
    fn normalized_browse_queries_once_and_emits_one_ordered_packet_with_the_full_total() {
        let store = store_with(Some(valid_interaction()));
        *store.query_result.lock().unwrap() = Ok(AuctionPage {
            rows: vec![codec::AuctionView {
                id: 8,
                item_entry: 25,
                item_stack_count: 2,
                item_enchant_id: 7,
                owner_guid: 70,
                start_bid: 100,
                buyout: 500,
                highest_bidder_guid: 71,
                highest_bid: 201,
                expires_at_micros: 5_000_000,
            }],
            total: 51,
            now_micros: 1_000_000,
        });
        let request = AuctionBrowseRequest {
            auctioneer_guid: 42,
            offset: 50,
            name: "sword".to_owned(),
            minimum_level: None,
            maximum_level: None,
            inventory_type: None,
            item_class: None,
            item_subclass: None,
            quality: None,
            usable_only: false,
        };

        let outcome = dispatch_auction_browse_action(
            &store,
            AuctionActionPlayer { self_guid: Some(7) },
            request.clone(),
        )
        .unwrap();
        assert_eq!(
            store.queries.lock().unwrap().as_slice(),
            &[(7, 4, AuctionQuery::Browse(request))]
        );
        let AuctionActionOutcome::Handled { outbound } = outcome else {
            panic!("browse must be handled")
        };
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(message))]
                if message.total_amount_of_auctions == 51
                    && message.auctions.len() == 1
                    && message.auctions[0].id == 8
                    && message.auctions[0].minimum_bid == 212
        ));
    }

    #[test]
    fn owner_view_uses_its_zero_based_offset_and_the_shared_row_codec() {
        let store = store_with(Some(valid_interaction()));
        *store.query_result.lock().unwrap() = Ok(AuctionPage {
            rows: vec![codec::AuctionView {
                id: 19,
                item_entry: 35,
                item_stack_count: 3,
                item_enchant_id: 4,
                owner_guid: 7,
                start_bid: 80,
                buyout: 900,
                highest_bidder_guid: 12,
                highest_bid: 100,
                expires_at_micros: 4_000_000,
            }],
            total: 52,
            now_micros: 1_000_000,
        });
        let outcome = dispatch_auction_action(
            &store,
            AuctionActionPlayer { self_guid: Some(7) },
            CMSG_AUCTION_LIST_OWNER_ITEMS {
                auctioneer: Guid::new(42),
                list_from: 50,
            }
            .into(),
        )
        .unwrap();

        assert_eq!(
            store.queries.lock().unwrap().as_slice(),
            &[(7, 4, AuctionQuery::Owner { offset: 50 })]
        );
        let AuctionActionOutcome::Handled { outbound } = outcome else {
            panic!("owner view must be handled")
        };
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(message))]
                if message.total_amount_of_auctions == 52
                    && message.auctions.len() == 1
                    && message.auctions[0].id == 19
                    && message.auctions[0].minimum_bid == 105
                    && message.auctions[0].time_left == std::time::Duration::from_millis(3_000)
        ));
    }

    #[test]
    fn bidder_view_preserves_requested_outbid_ids_and_uses_current_highest_rows() {
        let store = store_with(Some(valid_interaction()));
        *store.query_result.lock().unwrap() = Ok(AuctionPage {
            rows: vec![codec::AuctionView {
                id: 19,
                item_entry: 35,
                item_stack_count: 3,
                item_enchant_id: 4,
                owner_guid: 7,
                start_bid: 80,
                buyout: 900,
                highest_bidder_guid: 8,
                highest_bid: 107,
                expires_at_micros: 4_000_000,
            }],
            total: 52,
            now_micros: 1_000_000,
        });
        let outcome = dispatch_auction_action(
            &store,
            AuctionActionPlayer { self_guid: Some(8) },
            CMSG_AUCTION_LIST_BIDDER_ITEMS {
                auctioneer: Guid::new(42),
                start_from_page: 50,
                outbid_item_ids: vec![19, 88],
            }
            .into(),
        )
        .unwrap();

        assert_eq!(
            store.queries.lock().unwrap().as_slice(),
            &[(
                8,
                4,
                AuctionQuery::Bidder {
                    offset: 50,
                    outbid_auction_ids: vec![19, 88],
                },
            )]
        );
        let AuctionActionOutcome::Handled { outbound } = outcome else {
            panic!("bidder view must be handled")
        };
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(message))]
                if message.total_amount_of_auctions == 52
                    && message.auctions.len() == 1
                    && message.auctions[0].id == 19
                    && message.auctions[0].minimum_bid == 113
                    && message.auctions[0].highest_bid == 107
                    && message.auctions[0].highest_bidder.guid() == 8
        ));
    }

    fn hello_outbound(store: &InMemoryAuctionActions) -> Result<Vec<Outbound>> {
        match dispatch_auction_action(
            store,
            AuctionActionPlayer { self_guid: Some(7) },
            MSG_AUCTION_HELLO_Client {
                auctioneer: Guid::new(42),
            }
            .into(),
        )? {
            AuctionActionOutcome::Handled { outbound } => Ok(outbound),
            AuctionActionOutcome::PassThrough(_) => {
                panic!("auction hello must never pass beyond its focused seam")
            }
        }
    }

    fn sell_outbound(store: &InMemoryAuctionActions) -> Result<Vec<Outbound>> {
        match dispatch_auction_action(
            store,
            AuctionActionPlayer { self_guid: Some(7) },
            CMSG_AUCTION_SELL_ITEM {
                auctioneer: Guid::new(42),
                item: Guid::new(70),
                starting_bid: 100,
                buyout: 500,
                auction_duration_in_minutes: 720,
            }
            .into(),
        )? {
            AuctionActionOutcome::Handled { outbound } => Ok(outbound),
            AuctionActionOutcome::PassThrough(_) => {
                panic!("auction sell must never pass beyond its focused seam")
            }
        }
    }

    fn session_error(result: Result<Vec<Outbound>>, what: &str) -> anyhow::Error {
        match result {
            Ok(_) => panic!("{what} must end the session"),
            Err(error) => error,
        }
    }

    fn bid_outbound(store: &InMemoryAuctionActions) -> Result<Vec<Outbound>> {
        match dispatch_auction_action(
            store,
            AuctionActionPlayer { self_guid: Some(8) },
            CMSG_AUCTION_PLACE_BID {
                auctioneer: Guid::new(42),
                auction_id: 41,
                price: Gold::new(107),
            }
            .into(),
        )? {
            AuctionActionOutcome::Handled { outbound } => Ok(outbound),
            AuctionActionOutcome::PassThrough(_) => {
                panic!("auction bid must never pass beyond its focused seam")
            }
        }
    }

    #[test]
    fn a_reachable_auctioneer_opens_its_imported_house() {
        let store = store_with(Some(valid_interaction()));
        let outcome = dispatch_auction_action(
            &store,
            AuctionActionPlayer { self_guid: Some(7) },
            MSG_AUCTION_HELLO_Client {
                auctioneer: Guid::new(42),
            }
            .into(),
        )
        .unwrap();

        let AuctionActionOutcome::Handled { outbound } = outcome else {
            panic!("auction hello must be handled by the auction seam");
        };
        assert_eq!(outbound.len(), 1);
        let Outbound::One(ServerOpcodeMessage::MSG_AUCTION_HELLO(message)) = &outbound[0] else {
            panic!("auction hello must reply with MSG_AUCTION_HELLO");
        };
        assert_eq!(message.auctioneer.guid(), 42);
        assert_eq!(message.auction_house.as_int(), 4);
        assert_eq!(store.lookups.lock().unwrap().as_slice(), &[(7, 42)]);
    }

    #[test]
    fn empty_queries_select_the_matching_build_5875_response_opcode() {
        let guid = Guid::new(42);
        let requests: Vec<ClientOpcodeMessage> = vec![
            CMSG_AUCTION_LIST_ITEMS {
                auctioneer: guid,
                ..Default::default()
            }
            .into(),
            CMSG_AUCTION_LIST_OWNER_ITEMS {
                auctioneer: guid,
                ..Default::default()
            }
            .into(),
            CMSG_AUCTION_LIST_BIDDER_ITEMS {
                auctioneer: guid,
                ..Default::default()
            }
            .into(),
        ];

        for (index, request) in requests.into_iter().enumerate() {
            let outcome = dispatch_auction_action(
                &store_with(Some(valid_interaction())),
                AuctionActionPlayer { self_guid: Some(7) },
                request,
            )
            .unwrap();
            let AuctionActionOutcome::Handled { outbound } = outcome else {
                panic!("auction query {index} must be handled by the auction seam");
            };
            assert_eq!(outbound.len(), 1, "query {index} has one ordered reply");
            match (index, &outbound[0]) {
                (0, Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(result))) => {
                    assert!(result.auctions.is_empty());
                    assert_eq!(result.total_amount_of_auctions, 0);
                }
                (1, Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(result))) => {
                    assert!(result.auctions.is_empty());
                    assert_eq!(result.total_amount_of_auctions, 0);
                }
                (
                    2,
                    Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(result)),
                ) => {
                    assert!(result.auctions.is_empty());
                    assert_eq!(result.total_amount_of_auctions, 0);
                }
                _ => panic!("query {index} selected the wrong response opcode"),
            }
        }
    }

    #[test]
    fn a_valid_sell_request_receives_the_specific_started_result() {
        let store = store_with(Some(valid_interaction()));
        let outbound = sell_outbound(&store).unwrap();
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                if message.auction_id == 41
                    && message.action
                        == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::Started {
                            result2: SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo::Ok,
                        }
        ));
        assert_eq!(
            store.creates.lock().unwrap().as_slice(),
            &[CreateAuctionRequest {
                actor_guid: 7,
                auctioneer_guid: 42,
                item_guid: 70,
                start_bid: 100,
                buyout: 500,
                duration_minutes: 720,
                house_id: 4,
            }]
        );
    }

    #[test]
    fn a_valid_bid_holds_the_full_offer_and_receives_the_specific_bid_result() {
        let store = store_with(Some(valid_interaction()));
        let outbound = bid_outbound(&store).unwrap();
        assert_eq!(
            store.bids.lock().unwrap().as_slice(),
            &[PlaceBidRequest {
                actor_guid: 8,
                auctioneer_guid: 42,
                auction_id: 41,
                offer: 107,
                house_id: 4,
            }]
        );
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                if message.auction_id == 41
                    && message.action
                        == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::BidPlaced {
                            result: SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::Ok {
                                auction_outbid1: 6,
                            },
                        }
        ));
    }

    #[test]
    fn a_normalized_buyout_uses_the_exact_bid_success_mapping() {
        let store = store_with(Some(valid_interaction()));
        *store.bid_result.lock().unwrap() = Ok(PlaceBidOutcome::Accepted {
            minimum_increment: 25,
        });

        let outbound = bid_outbound(&store).unwrap();

        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                if message.auction_id == 41
                    && message.action
                        == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::BidPlaced {
                            result: SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::Ok {
                                auction_outbid1: 25,
                            },
                        }
        ));
    }

    #[test]
    fn bid_refusals_use_the_specific_vanilla_result_variants() {
        let cases = [
            (
                PlaceBidOutcome::ItemNotFound,
                SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::ErrItemNotFound,
            ),
            (
                PlaceBidOutcome::NotEnoughMoney,
                SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::ErrNotEnoughMoney,
            ),
            (
                PlaceBidOutcome::BidIncrement,
                SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::ErrBidIncrement,
            ),
            (
                PlaceBidOutcome::BidOwn,
                SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::ErrBidOwn,
            ),
            (
                PlaceBidOutcome::Database,
                SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::ErrDatabase,
            ),
        ];
        for (outcome, expected) in cases {
            let store = store_with(Some(valid_interaction()));
            *store.bid_result.lock().unwrap() = Ok(outcome);
            let outbound = bid_outbound(&store).unwrap();
            assert!(matches!(
                outbound.as_slice(),
                [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                    if message.auction_id == 41
                        && message.action
                            == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::BidPlaced {
                                result: expected,
                            }
            ));
        }

        let store = store_with(Some(valid_interaction()));
        *store.bid_result.lock().unwrap() = Ok(PlaceBidOutcome::HigherBid {
            bidder_guid: 9,
            current_bid: 101,
            minimum_increment: 6,
        });
        let outbound = bid_outbound(&store).unwrap();
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                if matches!(message.action,
                    SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::BidPlaced {
                        result: SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::ErrHigherBid {
                            auction_outbid2: 6,
                            higher_bidder,
                            new_bid: 101,
                        },
                    } if higher_bidder.guid() == 9)
        ));
    }

    #[test]
    fn sell_refusals_use_the_specific_started_result_variants() {
        let cases = [
            (
                CreateAuctionOutcome::ItemNotFound,
                SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo::ErrItemNotFound,
            ),
            (
                CreateAuctionOutcome::NotEnoughMoney,
                SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo::ErrNotEnoughMoney,
            ),
            (
                CreateAuctionOutcome::Database,
                SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo::ErrDatabase,
            ),
        ];

        for (outcome, expected) in cases {
            let store = store_with(Some(valid_interaction()));
            *store.create_result.lock().unwrap() = Ok(outcome);
            let outbound = sell_outbound(&store).unwrap();
            assert!(matches!(
                outbound.as_slice(),
                [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                    if message.auction_id == 0
                        && message.action
                            == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::Started {
                                result2: expected,
                            }
            ));
        }
    }

    #[test]
    fn sell_checks_the_auctioneer_before_calling_the_listing_driver() {
        let store = store_with(None);
        let outbound = sell_outbound(&store).unwrap();
        assert!(store.creates.lock().unwrap().is_empty());
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                if message.action
                    == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::Started {
                        result2: SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo::ErrDatabase,
                    }
        ));
    }

    #[test]
    fn every_module_refusal_has_one_client_result_code() {
        use SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult as Bid;
        use SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo as Sell;
        let expected = |refusal| match refusal {
            AuctionRefusal::ItemNotFound => (Sell::ErrItemNotFound, Bid::ErrItemNotFound),
            AuctionRefusal::NotEnoughMoney => (Sell::ErrNotEnoughMoney, Bid::ErrNotEnoughMoney),
            AuctionRefusal::InvalidTerms | AuctionRefusal::Database => {
                (Sell::ErrDatabase, Bid::ErrDatabase)
            }
        };
        for refusal in AuctionRefusal::ALL {
            let (sell_code, bid_code) = expected(refusal);

            let store = store_with(Some(valid_interaction()));
            *store.create_result.lock().unwrap() = Ok(refusal.into());
            let outbound = sell_outbound(&store).unwrap();
            assert!(
                matches!(
                    outbound.as_slice(),
                    [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                        if message.action
                            == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::Started {
                                result2: sell_code,
                            }
                ),
                "{refusal:?}"
            );

            let store = store_with(Some(valid_interaction()));
            *store.bid_result.lock().unwrap() = Ok(refusal.into());
            let outbound = bid_outbound(&store).unwrap();
            assert!(
                matches!(
                    outbound.as_slice(),
                    [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                        if message.action
                            == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::BidPlaced {
                                result: bid_code,
                            }
                ),
                "{refusal:?}"
            );
        }
    }

    #[test]
    fn a_reducer_timeout_is_not_answered_as_a_refusal() {
        let store = store_with(Some(valid_interaction()));
        *store.create_result.lock().unwrap() =
            Err("gw_auction_hold_listing reducer timed out after 10s".to_string());
        let error = session_error(sell_outbound(&store), "an unknown listing outcome");
        assert!(error.to_string().contains("timed out"));

        let store = store_with(Some(valid_interaction()));
        *store.bid_result.lock().unwrap() =
            Err("gw_auction_hold_bid reducer timed out after 10s".to_string());
        let error = session_error(bid_outbound(&store), "an unknown bid outcome");
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn sell_transport_failure_is_fatal() {
        let store = store_with(Some(valid_interaction()));
        *store.create_result.lock().unwrap() =
            Err("auction reducer transport disconnected: channel closed".to_string());

        let error = match sell_outbound(&store) {
            Ok(_) => panic!("a dead reducer transport must end the session"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("transport disconnected"));
    }

    #[test]
    fn sell_realm_core_outage_is_fatal() {
        let store = store_with(Some(valid_interaction()));
        *store.create_result.lock().unwrap() =
            Err("realm-core database lyracore-realm is not connected".to_string());

        let error = match sell_outbound(&store) {
            Ok(_) => panic!("an unavailable realm plane must end the session"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("not connected"));
    }

    #[test]
    fn an_unresolved_or_refusing_auctioneer_is_handled_without_a_reply() {
        assert!(hello_outbound(&store_with(None)).unwrap().is_empty());

        let refusing = AuctionInteraction {
            refuses_interaction: true,
            ..valid_interaction()
        };
        assert!(hello_outbound(&store_with(Some(refusing)))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn sell_and_bid_leave_faction_refusals_to_the_module() {
        let store = store_with(Some(AuctionInteraction {
            refuses_interaction: true,
            ..valid_interaction()
        }));
        *store.create_result.lock().unwrap() = Ok(CreateAuctionOutcome::Database);
        let outbound = sell_outbound(&store).unwrap();
        assert_eq!(store.creates.lock().unwrap().len(), 1);
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                if message.action
                    == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::Started {
                        result2: SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo::ErrDatabase,
                    }
        ));

        let store = store_with(Some(AuctionInteraction {
            refuses_interaction: true,
            ..valid_interaction()
        }));
        *store.bid_result.lock().unwrap() = Ok(PlaceBidOutcome::Database);
        let outbound = bid_outbound(&store).unwrap();
        assert_eq!(store.bids.lock().unwrap().len(), 1);
        assert!(matches!(
            outbound.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_COMMAND_RESULT(message))]
                if message.action
                    == SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction::BidPlaced {
                        result: SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResult::ErrDatabase,
                    }
        ));
    }

    #[test]
    fn a_failed_interaction_read_is_fatal() {
        for message in [
            "auction state unavailable",
            "auction interaction reducer transport disconnected: channel closed",
        ] {
            let error = session_error(hello_outbound(&store_error(message)), "a failed read");
            assert_eq!(error.to_string(), message);
        }
    }
}
