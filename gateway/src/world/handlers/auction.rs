//! Stormwind auction-house protocol family.

use super::super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct AuctionEntity {
    pub(crate) type_mask: u32,
    pub(crate) map_id: u32,
    pub(crate) instance_id: u64,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) z: f32,
    pub(crate) dead: bool,
    pub(crate) npc_flags: u32,
    pub(crate) race: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AuctionInteraction {
    pub(crate) player: AuctionEntity,
    pub(crate) auctioneer: AuctionEntity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CreateAuctionRequest {
    pub(crate) actor_guid: u64,
    pub(crate) auctioneer_guid: u64,
    pub(crate) item_guid: u64,
    pub(crate) start_bid: u32,
    pub(crate) buyout: u32,
    pub(crate) duration_minutes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CreateAuctionOutcome {
    Created { auction_id: u32 },
    ItemNotFound,
    NotEnoughMoney,
    Database,
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
    Owner { offset: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuctionPage {
    pub(crate) rows: Vec<codec::AuctionView>,
    pub(crate) total: u32,
    pub(crate) now_micros: i64,
}

pub(crate) trait AuctionActionStore: Send + Sync {
    fn auction_entities(
        &self,
        player_guid: u64,
        auctioneer_guid: u64,
    ) -> Result<Option<AuctionInteraction>>;

    fn create_auction(&self, request: CreateAuctionRequest) -> Result<CreateAuctionOutcome>;

    fn auction_query(&self, player_guid: u64, query: AuctionQuery) -> Result<AuctionPage>;
}

impl AuctionActionStore for crate::stdb::Coordinator {
    fn auction_entities(
        &self,
        player_guid: u64,
        auctioneer_guid: u64,
    ) -> Result<Option<AuctionInteraction>> {
        use crate::stdb::bindings::GameWorldEntityTableAccess;
        let guard = self.0.coord();
        let entities = guard.conn.db.game_world_entity();
        let (Some(player), Some(auctioneer)) = (
            entities.guid().find(&player_guid),
            entities.guid().find(&auctioneer_guid),
        ) else {
            return Ok(None);
        };
        let view = |entity: crate::stdb::bindings::WorldEntity| AuctionEntity {
            type_mask: entity.type_mask,
            map_id: entity.map_id,
            instance_id: entity.instance_id,
            x: entity.x,
            y: entity.y,
            z: entity.z,
            dead: entity.dead,
            npc_flags: entity.npc_flags,
            race: (entity.unit_bytes_0 & 0xff) as u8,
        };
        Ok(Some(AuctionInteraction {
            player: view(player),
            auctioneer: view(auctioneer),
        }))
    }

    fn create_auction(&self, request: CreateAuctionRequest) -> Result<CreateAuctionOutcome> {
        crate::stdb::Coordinator::create_auction(self, request)
    }

    fn auction_query(&self, player_guid: u64, query: AuctionQuery) -> Result<AuctionPage> {
        crate::stdb::Coordinator::auction_query(self, player_guid, query)
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
    Bidder,
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
    let Some(player_guid) =
        validated_auction_player_guid(store, player, request.auctioneer_guid)?
    else {
        return Ok(AuctionActionOutcome::Handled {
            outbound: vec![Outbound::One(
                ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(
                    codec::build_auction_list_result(&[], 0, 0),
                )),
            )],
        });
    };
    let page = store.auction_query(player_guid, AuctionQuery::Browse(request))?;
    Ok(AuctionActionOutcome::Handled {
        outbound: vec![Outbound::One(
            ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(
                codec::build_auction_list_result(&page.rows, page.total, page.now_micros),
            )),
        )],
    })
}

fn validated_auction_player_guid<St: AuctionActionStore + ?Sized>(
    store: &St,
    player: AuctionActionPlayer,
    auctioneer_guid: u64,
) -> Result<Option<u64>> {
    let Some(player_guid) = player.self_guid else {
        return Ok(None);
    };
    let entities = match store.auction_entities(player_guid, auctioneer_guid) {
        Ok(entities) => entities,
        Err(error)
            if classify_auction_action_error(&error)
                == AuctionActionErrorClass::GameplayRefusal =>
        {
            log::debug!(
                "world: auctioneer {auctioneer_guid} interaction unavailable for player \
                 {player_guid}: {error}"
            );
            None
        }
        Err(error) => return Err(error),
    };
    Ok(interaction_allowed(entities).then_some(player_guid))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuctionActionErrorClass {
    GameplayRefusal,
    Fatal,
}

fn classify_auction_action_error(error: &anyhow::Error) -> AuctionActionErrorClass {
    if error.chain().any(|cause| {
        let text = cause.to_string();
        text.contains("reducer transport disconnected")
            || (text.contains("realm-core database") && text.contains("is not connected"))
    }) {
        AuctionActionErrorClass::Fatal
    } else {
        AuctionActionErrorClass::GameplayRefusal
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
        ClientOpcodeMessage::CMSG_AUCTION_LIST_BIDDER_ITEMS(message) => {
            (message.auctioneer, AuctionRequest::Bidder)
        }
        ClientOpcodeMessage::CMSG_AUCTION_SELL_ITEM(message) => {
            let Some(player_guid) = player.self_guid else {
                return Ok(create_result(CreateAuctionOutcome::Database));
            };
            let auctioneer_guid = message.auctioneer.guid();
            let entities = match store.auction_entities(player_guid, auctioneer_guid) {
                Ok(entities) => entities,
                Err(error)
                    if classify_auction_action_error(&error)
                        == AuctionActionErrorClass::GameplayRefusal =>
                {
                    return Ok(create_result(CreateAuctionOutcome::Database));
                }
                Err(error) => return Err(error),
            };
            if !interaction_allowed(entities) {
                return Ok(create_result(CreateAuctionOutcome::Database));
            }
            let outcome = match store.create_auction(CreateAuctionRequest {
                actor_guid: player_guid,
                auctioneer_guid,
                item_guid: message.item.guid(),
                start_bid: message.starting_bid,
                buyout: message.buyout,
                duration_minutes: message.auction_duration_in_minutes,
            }) {
                Ok(outcome) => outcome,
                Err(error)
                    if classify_auction_action_error(&error)
                        == AuctionActionErrorClass::GameplayRefusal =>
                {
                    CreateAuctionOutcome::Database
                }
                Err(error) => return Err(error),
            };
            return Ok(create_result(outcome));
        }
        other => return Ok(AuctionActionOutcome::PassThrough(other)),
    };
    let Some(player_guid) = player.self_guid else {
        return Ok(AuctionActionOutcome::Handled {
            outbound: Vec::new(),
        });
    };
    let auctioneer_guid = auctioneer.guid();
    let entities = match store.auction_entities(player_guid, auctioneer_guid) {
        Ok(entities) => entities,
        Err(error)
            if classify_auction_action_error(&error)
                == AuctionActionErrorClass::GameplayRefusal =>
        {
            log::debug!(
                "world: auctioneer {auctioneer_guid} interaction unavailable for player \
                 {player_guid}: {error}"
            );
            None
        }
        Err(error) => return Err(error),
    };
    if !interaction_allowed(entities) {
        return Ok(AuctionActionOutcome::Handled {
            outbound: Vec::new(),
        });
    }
    use wow_world_messages::vanilla::{
        AuctionHouse, MSG_AUCTION_HELLO_Server, SMSG_AUCTION_BIDDER_LIST_RESULT,
    };
    let message = match request {
        AuctionRequest::Hello(auctioneer) => {
            ServerOpcodeMessage::MSG_AUCTION_HELLO(Box::new(MSG_AUCTION_HELLO_Server {
                auctioneer,
                auction_house: AuctionHouse::try_from(lyracore_shared::auction::STORMWIND_HOUSE_ID)
                    .expect("the shared Stormwind house id must be a vanilla AuctionHouse"),
            }))
        }
        AuctionRequest::Owner(offset) => {
            let page = store.auction_query(player_guid, AuctionQuery::Owner { offset })?;
            ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(Box::new(
                codec::build_auction_owner_list_result(&page.rows, page.total, page.now_micros),
            ))
        }
        AuctionRequest::Bidder => ServerOpcodeMessage::SMSG_AUCTION_BIDDER_LIST_RESULT(Box::new(
            SMSG_AUCTION_BIDDER_LIST_RESULT {
                auctions: Vec::new(),
                total_amount_of_auctions: 0,
            },
        )),
    };
    Ok(AuctionActionOutcome::Handled {
        outbound: vec![Outbound::One(message)],
    })
}

fn interaction_allowed(entities: Option<AuctionInteraction>) -> bool {
    let Some(AuctionInteraction {
        player: actor,
        auctioneer,
    }) = entities
    else {
        return false;
    };
    let dx = actor.x - auctioneer.x;
    let dy = actor.y - auctioneer.y;
    let dz = actor.z - auctioneer.z;
    actor.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0
        && !actor.dead
        && lyracore_shared::faction::team_for_race(actor.race)
            == lyracore_shared::faction::TEAM_ALLIANCE
        && auctioneer.type_mask & lyracore_shared::constants::type_mask::CREATURE
            == lyracore_shared::constants::type_mask::CREATURE
        && auctioneer.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT == 0
        && !auctioneer.dead
        && auctioneer.npc_flags & lyracore_shared::constants::npc_flags::AUCTIONEER != 0
        && actor.map_id == auctioneer.map_id
        && actor.instance_id == auctioneer.instance_id
        && dx * dx + dy * dy + dz * dz <= lyracore_shared::auction::INTERACTION_RANGE_SQ
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        Guid, MSG_AUCTION_HELLO_Client, SMSG_AUCTION_COMMAND_RESULT_AuctionCommandAction,
        SMSG_AUCTION_COMMAND_RESULT_AuctionCommandResultTwo, CMSG_AUCTION_LIST_BIDDER_ITEMS,
        CMSG_AUCTION_LIST_ITEMS, CMSG_AUCTION_LIST_OWNER_ITEMS, CMSG_AUCTION_SELL_ITEM,
    };

    struct InMemoryAuctionActions {
        result: Mutex<Result<Option<AuctionInteraction>, String>>,
        lookups: Mutex<Vec<(u64, u64)>>,
        creates: Mutex<Vec<CreateAuctionRequest>>,
        create_result: Mutex<Result<CreateAuctionOutcome, String>>,
        query_result: Mutex<Result<AuctionPage, String>>,
        queries: Mutex<Vec<(u64, AuctionQuery)>>,
    }

    impl AuctionActionStore for InMemoryAuctionActions {
        fn auction_entities(
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

        fn auction_query(&self, player_guid: u64, query: AuctionQuery) -> Result<AuctionPage> {
            self.queries.lock().unwrap().push((player_guid, query));
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
            player: AuctionEntity {
                type_mask: lyracore_shared::constants::type_mask::PLAYER,
                race: 1,
                ..Default::default()
            },
            auctioneer: AuctionEntity {
                type_mask: lyracore_shared::constants::type_mask::CREATURE,
                npc_flags: lyracore_shared::constants::npc_flags::AUCTIONEER,
                x: 10.0,
                ..Default::default()
            },
        }
    }

    fn store_with(interaction: Option<AuctionInteraction>) -> InMemoryAuctionActions {
        InMemoryAuctionActions {
            result: Mutex::new(Ok(interaction)),
            lookups: Mutex::default(),
            creates: Mutex::default(),
            create_result: Mutex::new(Ok(CreateAuctionOutcome::Created { auction_id: 41 })),
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
            &[(7, AuctionQuery::Browse(request))]
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
                    && message.auctions[0].minimum_bid == 11
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
            &[(7, AuctionQuery::Owner { offset: 50 })]
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
                    && message.auctions[0].minimum_bid == 5
                    && message.auctions[0].time_left == std::time::Duration::from_millis(3_000)
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

    #[test]
    fn a_reachable_stormwind_auctioneer_opens_the_named_house() {
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
        assert_eq!(message.auction_house.as_int(), 1);
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
            }]
        );
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
    fn invalid_auctioneer_interactions_are_handled_without_a_reply() {
        assert!(hello_outbound(&store_with(None)).unwrap().is_empty());

        let AuctionInteraction {
            player: valid_actor,
            auctioneer: valid_auctioneer,
        } = valid_interaction();
        let cases = [
            AuctionInteraction {
                player: AuctionEntity {
                    dead: true,
                    ..valid_actor
                },
                auctioneer: valid_auctioneer,
            },
            AuctionInteraction {
                player: AuctionEntity {
                    race: 2,
                    ..valid_actor
                },
                auctioneer: valid_auctioneer,
            },
            AuctionInteraction {
                player: AuctionEntity {
                    type_mask: lyracore_shared::constants::type_mask::CREATURE,
                    ..valid_actor
                },
                auctioneer: valid_auctioneer,
            },
            AuctionInteraction {
                player: valid_actor,
                auctioneer: AuctionEntity {
                    dead: true,
                    ..valid_auctioneer
                },
            },
            AuctionInteraction {
                player: valid_actor,
                auctioneer: AuctionEntity {
                    type_mask: lyracore_shared::constants::type_mask::PLAYER,
                    ..valid_auctioneer
                },
            },
            AuctionInteraction {
                player: valid_actor,
                auctioneer: AuctionEntity {
                    type_mask: lyracore_shared::constants::type_mask::OBJECT,
                    ..valid_auctioneer
                },
            },
            AuctionInteraction {
                player: valid_actor,
                auctioneer: AuctionEntity {
                    npc_flags: 0,
                    ..valid_auctioneer
                },
            },
            AuctionInteraction {
                player: valid_actor,
                auctioneer: AuctionEntity {
                    map_id: 1,
                    ..valid_auctioneer
                },
            },
            AuctionInteraction {
                player: valid_actor,
                auctioneer: AuctionEntity {
                    instance_id: 1,
                    ..valid_auctioneer
                },
            },
            AuctionInteraction {
                player: valid_actor,
                auctioneer: AuctionEntity {
                    x: 10.01,
                    ..valid_auctioneer
                },
            },
        ];

        for (index, entities) in cases.into_iter().enumerate() {
            assert!(
                hello_outbound(&store_with(Some(entities)))
                    .unwrap()
                    .is_empty(),
                "invalid interaction case {index} must be a handled refusal"
            );
        }
    }

    #[test]
    fn gameplay_read_failures_are_refusals_but_a_dead_transport_is_fatal() {
        assert!(hello_outbound(&store_error("auction state unavailable"))
            .unwrap()
            .is_empty());

        let error = match hello_outbound(&store_error(
            "auction interaction reducer transport disconnected: channel closed",
        )) {
            Ok(_) => panic!("a dead reducer transport must end the session"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("transport disconnected"));
    }
}
