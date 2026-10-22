use super::super::*;
use super::quest;
use super::taxi::open_taxi_outbound;
use super::trainer::settle_per_action;
use super::vendor::{vendor_has_stock, vendor_open_outbound};
use crate::stdb::{classify, DurableFailure};
use lyracore_shared::chat::broadcast_chat;

const NO_CHARACTER_FOR_GM_COMMAND: &str = "gm_command: actor_guid unresolved";

pub(crate) trait NpcStore: Send + Sync {
    fn creature_template(&self, entry: u32) -> Result<Option<codec::CreatureView>>;

    fn pet_name(
        &self,
        requester: Actor,
        pet_number: u32,
        pet_guid: u64,
    ) -> Result<Option<codec::PetNameView>>;

    fn gameobject_template(&self, entry: u32) -> Result<Option<codec::GameObjectTemplateView>>;

    fn gameobject_type(&self, go_guid: u64) -> Result<Option<u8>>;

    fn enter_areatrigger(&self, actor: Actor, trigger_id: u32) -> Result<()>;

    fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool>;

    fn bind_home(&self, actor: Actor, innkeeper_guid: u64) -> Result<InteractionOutcome>;

    fn npc_is_innkeeper(&self, guid: u64) -> Result<bool>;

    fn npc_gossip_text_id(&self, npc_guid: u64) -> u32;

    fn npc_text_for_id(&self, text_id: u32) -> Option<codec::NpcTextView>;

    /// Returns options sorted by option_index, unfiltered; empty menus retain their source id.
    fn gossip_options(&self, npc_guid: u64) -> Result<codec::GossipMenuView>;

    /// Unknown conditions are unavailable.
    fn gossip_option_available(
        &self,
        character_guid: u64,
        option: &codec::GossipOptionView,
    ) -> Result<bool>;

    fn inspect(&self, actor: Actor, target_guid: u64) -> Result<()>;

    /// Apply the Module Gate and hook before the Gateway executes the selected action.
    fn gossip_select(
        &self,
        actor: Actor,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
        source_menu_id: u32,
        selected_action: u32,
    ) -> Result<()>;
}

fn filtered_gossip_options<
    St: CharacterStore + NpcStore + QuestActionStore + TrainerStore + ?Sized,
>(
    store: &St,
    npc_guid: u64,
    player_guid: u64,
    is_vendor: bool,
    is_innkeeper: bool,
) -> Result<codec::GossipMenuView> {
    use lyracore_shared::constants::{gossip_option, MIN_TALENT_LEVEL};
    let level = store
        .character_by_guid(player_guid)?
        .map(|c| c.level)
        .unwrap_or(0);
    let serves_class = store.trainer_serves(player_guid, npc_guid).unwrap_or(true);
    let raw = store.gossip_options(npc_guid)?;
    let synthesize_vendor = is_vendor
        && !raw
            .options
            .iter()
            .any(|option| option.action == gossip_option::VENDOR);
    let synthesize_innkeeper = is_innkeeper
        && !raw
            .options
            .iter()
            .any(|option| option.action == gossip_option::INNKEEPER);
    let mut available = Vec::new();
    for option in raw.options {
        if !store.gossip_option_available(player_guid, &option)? {
            continue;
        }
        if option.action == gossip_option::UNLEARNTALENTS && level < MIN_TALENT_LEVEL {
            continue;
        }
        if !serves_class
            && matches!(
                option.action,
                gossip_option::TRAINER | gossip_option::UNLEARNTALENTS
            )
        {
            continue;
        }
        available.push(option);
    }
    Ok(codec::GossipMenuView {
        source_menu_id: raw.source_menu_id,
        options: codec::gossip_menu_options(available, synthesize_vendor, synthesize_innkeeper),
    })
}

fn speak_nearby<St: SpeechStore + ?Sized>(
    store: &St,
    session: &ProtocolSession,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<Vec<Outbound>> {
    let Some(actor) = session.actor() else {
        log::debug!(
            "world: broadcast chat type {chat_type} dropped (account {}): no Character",
            session.account_id
        );
        return Ok(Vec::new());
    };
    let refusal = match store.send_chat(actor, chat_type, language, message) {
        Ok(ChatOutcome::Delivered) => None,
        Ok(ChatOutcome::Refused(refusal)) => Some(refusal),
        Err(error) => match classify(&error) {
            DurableFailure::Refusal { reason } => {
                log::debug!(
                    "world: broadcast chat type {chat_type} dropped (account {}): {reason}",
                    session.account_id
                );
                None
            }
            DurableFailure::TransportLoss => return Err(error),
        },
    };
    Ok(super::chat::refusal_outbound(session, refusal))
}

pub(crate) struct Query;

impl<
        St: CastStore
            + ChatActionStore
            + CharacterStore
            + GuildActionStore
            + NpcStore
            + PartyStore
            + QuestActionStore
            + SessionStore
            + ShardRoutingStore
            + SocialStore
            + SpeechStore
            + TaxiActionStore
            + TrainerStore
            + VendorActionStore
            + ?Sized,
    > ProtocolFamily<St> for Query
{
    #[allow(clippy::too_many_lines)]
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let msg = request.message()?;
        let mut outbound = Vec::new();
        let actor = session.actor();

        match msg {
            ClientOpcodeMessage::CMSG_BINDER_ACTIVATE(request) => {
                let Some(actor) = actor else {
                    return Ok(outbound.into());
                };
                let innkeeper_guid = request.guid.guid();
                match store.bind_home(actor, innkeeper_guid)? {
                    InteractionOutcome::Refused(reason) => {
                        outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                            Box::new(codec::build_gm_system_message(reason)),
                        )));
                    }
                    InteractionOutcome::Done => match store.character_by_guid(actor.guid()) {
                        Ok(Some(character)) => {
                            let (opcode, body) =
                                codec::build_player_bound(innkeeper_guid, character.zone_id);
                            outbound.push(Outbound::Raw { opcode, body });
                        }
                        Ok(None) => {}
                        Err(error) => log::warn!(
                            "world: bound-home read for guid {} failed: {error:#}",
                            actor.guid()
                        ),
                    },
                }
                outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE));
            }
            ClientOpcodeMessage::MSG_TALENT_WIPE_CONFIRM(request) => {
                let Some(actor) = actor else {
                    return Ok(outbound.into());
                };
                if let InteractionOutcome::Refused(reason) =
                    store.reset_talents(actor, request.wiping_npc.guid())?
                {
                    outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                        Box::new(codec::build_gm_system_message(reason)),
                    )));
                }
                outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE));
            }

            ClientOpcodeMessage::CMSG_NAME_QUERY(q) => {
                let guid = q.guid.guid();
                match presence::character_anywhere(store, guid)? {
                    Some(c) => outbound.push(Outbound::One(
                        ServerOpcodeMessage::SMSG_NAME_QUERY_RESPONSE(Box::new(
                            codec::build_name_query_response(&c)?,
                        )),
                    )),
                    None => log::debug!("world: name query for unknown guid {guid}"),
                }
            }
            ClientOpcodeMessage::CMSG_PET_NAME_QUERY(q) => {
                let pet_guid = q.guid.guid();
                let pet = match actor {
                    Some(actor) => store.pet_name(actor, q.pet_number, pet_guid)?,
                    None => None,
                };
                match pet {
                    Some(pet) => outbound.push(Outbound::One(
                        ServerOpcodeMessage::SMSG_PET_NAME_QUERY_RESPONSE(Box::new(
                            codec::build_pet_name_query_response(&pet),
                        )),
                    )),
                    None => log::debug!(
                        "world: pet name query ignored for unknown or foreign guid {pet_guid}"
                    ),
                }
            }
            ClientOpcodeMessage::CMSG_INSPECT(i) => {
                let target_guid = i.guid.guid();
                if let Some(actor) = actor {
                    match store.inspect(actor, target_guid) {
                        Ok(()) => outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_INSPECT(
                            codec::build_inspect_response(target_guid),
                        ))),
                        Err(error) => settle_per_action("inspect", session.account_id, Err(error))?,
                    }
                }
            }
            ClientOpcodeMessage::CMSG_CREATURE_QUERY(q) => {
                match store.creature_template(q.creature)? {
                    Some(c) => outbound.push(Outbound::One(
                        ServerOpcodeMessage::SMSG_CREATURE_QUERY_RESPONSE(Box::new(
                            codec::build_creature_query_response(&c),
                        )),
                    )),
                    None => log::debug!("world: creature query for unknown entry {}", q.creature),
                }
            }
            ClientOpcodeMessage::CMSG_GOSSIP_HELLO(h) => {
                let npc = h.guid.guid();
                let player_guid = actor.map_or(0, Actor::guid);
                if player_guid != 0
                    && store
                        .npc_refuses_interaction(npc, player_guid)
                        .unwrap_or(false)
                {
                    return Ok(outbound.into());
                }
                let quests = match &session.state {
                    WorldState::InWorld(iw) => quest::gossip_quest_items(store, npc, iw.self_guid)?,
                    WorldState::CharSelect => Vec::new(),
                };
                let is_vendor = vendor_has_stock(store, npc)?;
                let is_innkeeper = store.npc_is_innkeeper(npc)?;
                let menu =
                    filtered_gossip_options(store, npc, player_guid, is_vendor, is_innkeeper)?;
                // Refiltering can change which option a client list position names.
                session.gossip_menu = Some(GossipMenuSnapshot {
                    npc_guid: npc,
                    source_menu_id: menu.source_menu_id,
                    options: menu.options.iter().map(|o| (o.row_id, o.action)).collect(),
                });
                let title_text_id = store.npc_gossip_text_id(npc);
                outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(
                    Box::new(codec::build_gossip_message(
                        npc,
                        title_text_id,
                        quests,
                        &menu.options,
                    )),
                )));
            }
            ClientOpcodeMessage::CMSG_NPC_TEXT_QUERY(q) => {
                let view = store.npc_text_for_id(q.text_id);
                outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_NPC_TEXT_UPDATE(
                    Box::new(codec::build_npc_text_update(q.text_id, view.as_ref())),
                )));
            }
            ClientOpcodeMessage::CMSG_GOSSIP_SELECT_OPTION(c) => {
                let npc = c.guid.guid();
                let player_guid = actor.map_or(0, Actor::guid);
                let clicked = session
                    .gossip_menu
                    .as_ref()
                    .filter(|snap| snap.npc_guid == npc)
                    .and_then(|snap| {
                        snap.options
                            .get(c.gossip_list_id as usize)
                            .map(|&(row_id, action)| (row_id, action, snap.source_menu_id))
                    });
                let (Some(actor), Some((option_row_id, action, source_menu_id))) = (actor, clicked)
                else {
                    session.gossip_menu = None;
                    return Ok(
                        vec![Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE)].into(),
                    );
                };
                if let Err(error) = store.gossip_select(
                    actor,
                    npc,
                    c.gossip_list_id,
                    option_row_id,
                    source_menu_id,
                    action,
                ) {
                    match classify(&error) {
                        DurableFailure::Refusal { .. } => {
                            session.gossip_menu = None;
                            return Ok(vec![Outbound::One(
                                ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE,
                            )]
                            .into());
                        }
                        DurableFailure::TransportLoss => return Err(error),
                    }
                }
                use lyracore_shared::constants::gossip_option;
                match action {
                    gossip_option::VENDOR => {
                        for message in vendor_open_outbound(store, npc)? {
                            outbound.push(message);
                        }
                    }
                    gossip_option::INNKEEPER => {
                        outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE));
                        outbound.push(Outbound::Raw {
                            opcode: 0x02eb,
                            body: npc.to_le_bytes().to_vec(),
                        });
                    }
                    gossip_option::TRAINER => {
                        let spells = store.trainer_list(player_guid, npc)?;
                        let list = codec::build_trainer_list(
                            npc,
                            &spells,
                            "I can teach you a thing or two.",
                        );
                        outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_LIST(
                            Box::new(list),
                        )));
                    }
                    gossip_option::UNLEARNTALENTS => {
                        outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE));
                        if let Some(cost) = store.talent_reset_cost(player_guid) {
                            let mut body = npc.to_le_bytes().to_vec();
                            body.extend_from_slice(&cost.to_le_bytes());
                            outbound.push(Outbound::Raw {
                                opcode: 0x02aa,
                                body,
                            });
                        } else {
                            outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                                Box::new(codec::build_gm_system_message(
                                    "The talent reset quote is not available. Try again.".into(),
                                )),
                            )));
                        }
                    }
                    gossip_option::TAXI => {
                        for message in open_taxi_outbound(store, Some(actor), npc)? {
                            outbound.push(message);
                        }
                    }
                    gossip_option::BANKER => outbound.push(super::show_bank(npc)),
                    gossip_option::SPIRITHEALER => {
                        outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE));
                        outbound.push(Outbound::One(
                            ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(
                                SMSG_SPIRIT_HEALER_CONFIRM { guid: c.guid },
                            ),
                        ));
                    }
                    gossip_option::TABARDDESIGNER => {
                        outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE));
                        outbound.push(super::guild::tabard_designer_window(npc));
                    }
                    gossip_option::PETITIONER => {
                        outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE));
                        outbound.push(super::guild::petition_showlist(npc));
                    }
                    _ => outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE)),
                }
            }
            ClientOpcodeMessage::CMSG_ITEM_QUERY_SINGLE(q) => {
                let resp = Box::new(codec::build_item_query_response(
                    q.item,
                    store.item_template(q.item)?.as_ref(),
                ));
                outbound.push(Outbound::One(
                    ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(resp),
                ));
            }
            ClientOpcodeMessage::CMSG_MESSAGECHAT(c) => {
                if !matches!(
                    &c.chat_type,
                    CMSG_MESSAGECHAT_ChatType::Say
                        | CMSG_MESSAGECHAT_ChatType::Yell
                        | CMSG_MESSAGECHAT_ChatType::Emote
                ) {
                    return super::Chat::handle(
                        store,
                        session,
                        ClientOpcodeMessage::CMSG_MESSAGECHAT(c).into(),
                    );
                }
                let CMSG_MESSAGECHAT {
                    chat_type,
                    language,
                    message,
                } = *c;
                let lang = language.as_int() as u8;
                match chat_type {
                    CMSG_MESSAGECHAT_ChatType::Say if super::is_guild_dot_command(&message) => {
                        if let Some(line) = super::run_guild_dot_command(store, session, &message)?
                        {
                            outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                                Box::new(codec::build_gm_system_message(line)),
                            )));
                        }
                    }
                    CMSG_MESSAGECHAT_ChatType::Say if message.starts_with('.') => {
                        let reply = match actor {
                            Some(actor) => {
                                match store.gm_command(&session.account_name, actor, message) {
                                    Ok(()) => None,
                                    Err(error) => match classify(&error) {
                                        DurableFailure::Refusal { reason } => {
                                            Some(reason.to_string())
                                        }
                                        DurableFailure::TransportLoss => return Err(error),
                                    },
                                }
                            }
                            None => Some(NO_CHARACTER_FOR_GM_COMMAND.to_string()),
                        };
                        if let Some(line) = reply {
                            outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                                Box::new(codec::build_gm_system_message(line)),
                            )));
                        }
                    }
                    CMSG_MESSAGECHAT_ChatType::Say => {
                        outbound.extend(speak_nearby(
                            store,
                            session,
                            broadcast_chat::SAY,
                            lang,
                            message,
                        )?);
                    }
                    CMSG_MESSAGECHAT_ChatType::Yell => {
                        outbound.extend(speak_nearby(
                            store,
                            session,
                            broadcast_chat::YELL,
                            lang,
                            message,
                        )?);
                    }
                    CMSG_MESSAGECHAT_ChatType::Emote => {
                        outbound.extend(speak_nearby(
                            store,
                            session,
                            broadcast_chat::EMOTE,
                            lang,
                            message,
                        )?);
                    }
                    _ => return Err(anyhow!("non-Speech chat request bypassed Chat")),
                }
            }
            ClientOpcodeMessage::CMSG_TEXT_EMOTE(c) => {
                if let Some(actor) = actor {
                    settle_per_action(
                        "send_emote",
                        session.account_id,
                        store.send_emote(actor, c.text_emote.as_int(), c.emote, c.target.guid()),
                    )?;
                }
            }
            ClientOpcodeMessage::MSG_RANDOM_ROLL(r) => {
                let op = party::Op::RandomRoll {
                    min: r.minimum,
                    max: r.maximum,
                };
                social::run_group_broadcast(store, session, op);
            }
            _ => return Err(anyhow!("opcode routed to the wrong Protocol Family")),
        }
        Ok(outbound.into())
    }
}
