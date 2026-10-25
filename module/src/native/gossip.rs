use lyracore_protocol::{
    codec,
    messages::{
        vanilla::{
            opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
            MSG_TABARDVENDOR_ACTIVATE,
        },
        Guid,
    },
};
use lyracore_shared::constants::{gossip_option, npc_flags, player_flags};
use spacetimedb::{ConnectionId, ReducerContext, SpacetimeType};

use super::game_native_player;
use crate::{game_gossip_menu, game_gossip_menu_profile_option, game_gossip_option};

#[derive(SpacetimeType)]
pub struct NativeGossipMenu {
    npc_guid: u64,
    source_menu_id: u32,
    options: Vec<NativeGossipOption>,
}

#[derive(SpacetimeType)]
struct NativeGossipOption {
    row_id: u32,
    action: u32,
}

fn complete(ctx: &ReducerContext, connection: ConnectionId) -> Result<(), String> {
    super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE,
        false,
    )
}

fn hello(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    npc: u64,
) -> Result<(), String> {
    let character = super::actor(ctx, guid)?;
    let (_, npc) = if character.player_flags & player_flags::GHOST != 0 {
        crate::ghost::spirit_healer_gate(ctx, guid, npc)?
    } else {
        crate::items::npc_interaction_gate(
            ctx,
            guid,
            npc,
            u32::MAX,
            "gossip creature",
            "target has no gossip",
            "use gossip",
        )?
    };
    let menu = crate::gossip::active_menu(ctx, &npc);
    let source = menu.as_ref().map_or(0, |menu| menu.menu_id);
    let text = menu
        .map(|menu| menu.text_id)
        .or_else(|| {
            ctx.db
                .game_gossip_menu()
                .entry()
                .find(npc.entry)
                .map(|row| row.text_id)
        })
        .unwrap_or(codec::GOSSIP_GREETING_TEXT_ID);
    let mut options: Vec<_> = if source == 0 {
        ctx.db
            .game_gossip_option()
            .by_entry()
            .filter(npc.entry)
            .map(|row| {
                (
                    row.option_index,
                    codec::GossipOptionView {
                        row_id: row.row_id,
                        icon: row.icon,
                        text: row.text,
                        action: row.action,
                        action_menu_id: row.action_menu_id,
                        cond_type: row.cond_type,
                        cond_value1: row.cond_value1,
                        cond_value2: row.cond_value2,
                    },
                )
            })
            .collect()
    } else {
        ctx.db
            .game_gossip_menu_profile_option()
            .by_menu()
            .filter(source)
            .map(|row| {
                (
                    row.option_index,
                    codec::GossipOptionView {
                        row_id: row.row_id,
                        icon: row.icon,
                        text: row.text,
                        action: row.action,
                        action_menu_id: row.action_menu_id,
                        cond_type: row.cond_type,
                        cond_value1: row.cond_value1,
                        cond_value2: row.cond_value2,
                    },
                )
            })
            .collect()
    };
    options.sort_by_key(|(index, option)| (*index, option.row_id));
    let options: Vec<_> = codec::gossip_menu_options(
        options.into_iter().map(|(_, option)| option).collect(),
        npc.npc_flags & npc_flags::VENDOR != 0,
        npc.npc_flags & npc_flags::INNKEEPER != 0,
    )
    .into_iter()
    .filter(|option| {
        crate::gossip::select_gate(ctx, guid, npc.guid, option.row_id, source, option.action)
            .is_ok()
    })
    .collect();
    let mut player = ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection)
        .ok_or("gossip Character is unavailable")?;
    // A filtered list position must retain its meaning until the next menu.
    player.gossip_menu = Some(NativeGossipMenu {
        npc_guid: npc.guid,
        source_menu_id: source,
        options: options
            .iter()
            .map(|option| NativeGossipOption {
                row_id: option.row_id,
                action: option.action,
            })
            .collect(),
    });
    ctx.db.game_native_player().connection_id().update(player);
    let quests = codec::quest_menu_items(&super::quest::evaluations(ctx, npc.guid, guid));
    super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(Box::new(codec::build_gossip_message(
            npc.guid, text, quests, &options,
        ))),
        false,
    )
}

fn select(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    npc: u64,
    index: u32,
) -> Result<(), String> {
    let mut player = ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection)
        .ok_or("gossip Character is unavailable")?;
    let clicked = player
        .gossip_menu
        .as_ref()
        .filter(|menu| menu.npc_guid == npc)
        .and_then(|menu| {
            menu.options
                .get(index as usize)
                .map(|option| (option.row_id, option.action, menu.source_menu_id))
        });
    let Some((row_id, action, source)) = clicked else {
        player.gossip_menu = None;
        ctx.db.game_native_player().connection_id().update(player);
        return complete(ctx, connection);
    };
    super::actor(ctx, guid)?;
    crate::world::apply_gossip_select(ctx, guid, npc, index, row_id, source, action)?;
    match action {
        gossip_option::VENDOR => super::economy::vendor(ctx, connection, guid, npc),
        gossip_option::TRAINER => super::trainer::list(ctx, connection, guid, npc),
        gossip_option::BANKER => super::economy::bank(ctx, connection, guid, npc),
        gossip_option::TAXI => super::taxi::open(ctx, connection, guid, npc),
        gossip_option::INNKEEPER => {
            complete(ctx, connection)?;
            super::send_raw(ctx, connection, 0x02eb, npc.to_le_bytes().to_vec(), false);
            Ok(())
        }
        gossip_option::UNLEARNTALENTS => {
            complete(ctx, connection)?;
            let character =
                crate::helpers::character_by_guid(ctx, guid).ok_or("Character is unavailable")?;
            let mut body = npc.to_le_bytes().to_vec();
            body.extend_from_slice(
                &crate::talent::respec_cost_copper(character.respec_count).to_le_bytes(),
            );
            super::send_raw(ctx, connection, 0x02aa, body, false);
            Ok(())
        }
        gossip_option::SPIRITHEALER => {
            complete(ctx, connection)?;
            super::death::confirm_spirit_healer(ctx, connection, npc)
        }
        gossip_option::TABARDDESIGNER => {
            complete(ctx, connection)?;
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(MSG_TABARDVENDOR_ACTIVATE {
                    guid: Guid::new(npc),
                }),
                false,
            )
        }
        gossip_option::PETITIONER => {
            complete(ctx, connection)?;
            super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_PETITION_SHOWLIST(Box::new(
                    codec::build_petition_showlist(npc),
                )),
                false,
            )
        }
        _ => complete(ctx, connection),
    }
}

fn interaction_refusal(reason: String) -> String {
    let messages = [
        ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(codec::build_gm_system_message(
            reason.clone(),
        ))),
        ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE,
    ];
    match messages
        .into_iter()
        .map(lyracore_protocol::world::encode)
        .collect()
    {
        Ok(packets) => super::rollback_packets(packets, reason),
        Err(error) => error.to_string(),
    }
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    match message {
        ClientOpcodeMessage::CMSG_GOSSIP_HELLO(c) => {
            hello(ctx, connection, guid, c.guid.guid()).map_err(super::rollback_silent)
        }
        ClientOpcodeMessage::CMSG_GOSSIP_SELECT_OPTION(c) => {
            select(ctx, connection, guid, c.guid.guid(), c.gossip_list_id).map_err(|reason| {
                super::rollback_reply(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE, reason)
            })
        }
        ClientOpcodeMessage::CMSG_BINDER_ACTIVATE(c) => {
            super::actor(ctx, guid)
                .and_then(|_| crate::items::innkeeper_access_gate(ctx, guid, c.guid.guid()))
                .map_err(interaction_refusal)?;
            crate::world::set_home(ctx, guid);
            if let Some(character) = crate::helpers::character_by_guid(ctx, guid) {
                let (opcode, body) = codec::build_player_bound(c.guid.guid(), character.zone_id);
                super::send_raw(ctx, connection, opcode, body, false);
            }
            complete(ctx, connection)
        }
        ClientOpcodeMessage::MSG_TALENT_WIPE_CONFIRM(c) => {
            super::actor(ctx, guid)
                .and_then(|_| crate::talent::do_reset_talents(ctx, guid, c.wiping_npc.guid()))
                .map_err(interaction_refusal)?;
            complete(ctx, connection)
        }
        _ => Err("invalid gossip opcode".into()),
    }
}
