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

pub(crate) trait AuctionActionStore: Send + Sync {
    fn auction_entities(
        &self,
        player_guid: u64,
        auctioneer_guid: u64,
    ) -> Result<Option<AuctionInteraction>>;
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
    Browse,
    Owner,
    Bidder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuctionActionErrorClass {
    GameplayRefusal,
    Fatal,
}

fn classify_auction_action_error(error: &anyhow::Error) -> AuctionActionErrorClass {
    if error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
    {
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
            (message.auctioneer, AuctionRequest::Browse)
        }
        ClientOpcodeMessage::CMSG_AUCTION_LIST_OWNER_ITEMS(message) => {
            (message.auctioneer, AuctionRequest::Owner)
        }
        ClientOpcodeMessage::CMSG_AUCTION_LIST_BIDDER_ITEMS(message) => {
            (message.auctioneer, AuctionRequest::Bidder)
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
    let Some(AuctionInteraction {
        player: actor,
        auctioneer,
    }) = entities
    else {
        return Ok(AuctionActionOutcome::Handled {
            outbound: Vec::new(),
        });
    };
    let dx = actor.x - auctioneer.x;
    let dy = actor.y - auctioneer.y;
    let dz = actor.z - auctioneer.z;
    let allowed = actor.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0
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
        && dx * dx + dy * dy + dz * dz <= lyracore_shared::auction::INTERACTION_RANGE_SQ;
    if !allowed {
        return Ok(AuctionActionOutcome::Handled {
            outbound: Vec::new(),
        });
    }
    use wow_world_messages::vanilla::{
        AuctionHouse, MSG_AUCTION_HELLO_Server, SMSG_AUCTION_BIDDER_LIST_RESULT,
        SMSG_AUCTION_LIST_RESULT, SMSG_AUCTION_OWNER_LIST_RESULT,
    };
    let message = match request {
        AuctionRequest::Hello(auctioneer) => {
            ServerOpcodeMessage::MSG_AUCTION_HELLO(Box::new(MSG_AUCTION_HELLO_Server {
                auctioneer,
                auction_house: AuctionHouse::try_from(lyracore_shared::auction::STORMWIND_HOUSE_ID)
                    .expect("the shared Stormwind house id must be a vanilla AuctionHouse"),
            }))
        }
        AuctionRequest::Browse => {
            ServerOpcodeMessage::SMSG_AUCTION_LIST_RESULT(Box::new(SMSG_AUCTION_LIST_RESULT {
                auctions: Vec::new(),
                total_amount_of_auctions: 0,
            }))
        }
        AuctionRequest::Owner => ServerOpcodeMessage::SMSG_AUCTION_OWNER_LIST_RESULT(Box::new(
            SMSG_AUCTION_OWNER_LIST_RESULT {
                auctions: Vec::new(),
                total_amount_of_auctions: 0,
            },
        )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        Guid, MSG_AUCTION_HELLO_Client, CMSG_AUCTION_LIST_BIDDER_ITEMS, CMSG_AUCTION_LIST_ITEMS,
        CMSG_AUCTION_LIST_OWNER_ITEMS,
    };

    struct InMemoryAuctionActions {
        result: Mutex<Result<Option<AuctionInteraction>, String>>,
        lookups: Mutex<Vec<(u64, u64)>>,
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
        }
    }

    fn store_error(error: &str) -> InMemoryAuctionActions {
        InMemoryAuctionActions {
            result: Mutex::new(Err(error.to_string())),
            lookups: Mutex::default(),
        }
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
