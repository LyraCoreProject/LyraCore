//! Query / social family: name / creature / item lookups + the gossip / npc-text round-trips,
//! plus who/friend/ignore. Pure code-motion out of `world/mod.rs`.

use super::super::*;
use super::quest;
use super::taxi::open_taxi_outbound;
use super::vendor::{vendor_has_stock, vendor_open_outbound};
use lyracore_shared::chat::broadcast_chat;

/// The NPC's imported gossip options, condition-filtered against `player_guid`'s quest
/// state — the SINGLE chokepoint both `CMSG_GOSSIP_HELLO` (render) and `CMSG_GOSSIP_SELECT_OPTION`
/// (re-derive the click) call, so the two can never disagree about which options are visible (the
/// "HELLO/SELECT_OPTION alignment" trap: a click's `gossip_list_id` indexes into whatever list HELLO
/// actually sent, so SELECT must reproduce that exact list, not just re-read the raw unfiltered rows).
/// Preserves `option_index` order (already sorted by the store read).
fn filtered_gossip_options<St: WorldStore + ?Sized>(
    store: &St,
    npc_guid: u64,
    player_guid: u64,
) -> Result<Vec<codec::GossipOptionView>> {
    use lyracore_shared::constants::{gossip_option, MIN_TALENT_LEVEL};
    // The unlearn-talents row has no condition of its own (cmangos gates it in C++ code at
    // GossipHello, not via a `conditions` row — see `gossip_option::UNLEARNTALENTS`'s doc), so it
    // needs its own level check here rather than falling through `option_condition_holds`. Below
    // level 10 a character literally cannot have a talent point, so the option would be inert even
    // if shown (#516).
    let level = store
        .character_by_guid(player_guid)?
        .map(|c| c.level)
        .unwrap_or(0);
    // The module refuses both training and respec for the wrong class, so either option would
    // advertise a guaranteed failure. Fail-open: a read error must not hide a working trainer.
    let serves_class = store.trainer_serves(player_guid, npc_guid).unwrap_or(true);
    let raw = store.gossip_options(npc_guid)?;
    Ok(raw
        .into_iter()
        .filter(|opt| {
            let (taken, rewarded) = quest::quest_gate_state(store, player_guid, opt.cond_value1);
            codec::option_condition_holds(opt.cond_type, taken, rewarded)
        })
        .filter(|opt| opt.action != gossip_option::UNLEARNTALENTS || level >= MIN_TALENT_LEVEL)
        .filter(|opt| {
            serves_class
                || !matches!(
                    opt.action,
                    gossip_option::TRAINER | gossip_option::UNLEARNTALENTS
                )
        })
        .collect())
}

/// Say, yell or `/e` (a `broadcast_chat` type) through the speaker's Home Shard. The line itself
/// returns on the Relay; a Refusal gets the answer every chat line shares, and only a lost reducer
/// transport ends the World Session.
fn speak_nearby<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &WorldConn,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<()> {
    let player = super::ChatActionPlayer {
        account_id: conn.account_id,
        self_guid: social::self_guid(conn),
    };
    let sent = store.send_chat(
        conn.account_id,
        player.self_guid.unwrap_or(0),
        chat_type,
        language,
        message,
    );
    let refusal = super::chat::settle(
        player,
        format_args!("broadcast chat type {chat_type}"),
        sent,
    )?;
    for message in super::chat::refusal_outbound(player, refusal) {
        send(tx, message)?;
    }
    Ok(())
}

/// Query / social family: name / creature / item lookups + the gossip / npc-text round-trips, plus
/// the social tier (say / yell / `/e` chat + text emotes), grouped as the stateless
/// request→reply / broadcast opcodes.
#[allow(clippy::too_many_lines)] // One arm per query and social opcode.
pub(crate) fn handle_query<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    // The shared-call path names the actor by guid; 0 (not in world) forces the
    // per-player path in the store.
    let self_guid = match &conn.state {
        WorldState::InWorld(iw) => iw.self_guid,
        _ => 0,
    };

    match msg {
        // Name resolution: the client asks for a guid's name to render its plate (else "Unknown").
        //
        // Resolved across every connected shard, not just this one. A guid the
        // client has met across a database boundary — the sender of a cross-shard whisper, which
        // arrives as a GUID because the client resolves whisper names itself — has no row on the
        // asking session's shard, and a dropped reply renders the line with nobody's name on it. On a
        // single-database gateway `world_stores()` is empty, so this is exactly the one read it was.
        ClientOpcodeMessage::CMSG_NAME_QUERY(q) => {
            let guid = q.guid.guid();
            match presence::character_anywhere(store, guid)? {
                Some(c) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_NAME_QUERY_RESPONSE(Box::new(
                        codec::build_name_query_response(&c)?,
                    ))),
                )?,
                None => log::debug!("world: name query for unknown guid {guid}"),
            }
        }
        ClientOpcodeMessage::CMSG_PET_NAME_QUERY(q) => {
            let pet_guid = q.guid.guid();
            match store.pet_name(self_guid, q.pet_number, pet_guid)? {
                Some(pet) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_PET_NAME_QUERY_RESPONSE(Box::new(
                        codec::build_pet_name_query_response(&pet),
                    ))),
                )?,
                None => log::debug!(
                    "world: pet name query ignored for unknown or foreign guid {pet_guid}"
                ),
            }
        }
        // Inspect: validate range + friendly target server-side (the `inspect`
        // reducer), then ack with SMSG_INSPECT(target guid) so the client opens the paperdoll — it
        // renders the target's equipment from fields the client already has (the visible-item relay
        // is the follow-up for full paperdoll correctness). Out of range / hostile /
        // no-such-target → the reducer errors and we silently drop the request, same as the other
        // stateless gates (CMSG_GAMEOBJ_USE, CMSG_AREATRIGGER).
        ClientOpcodeMessage::CMSG_INSPECT(i) => {
            let target_guid = i.guid.guid();
            match store.inspect(conn.account_id, self_guid, target_guid) {
                Ok(()) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_INSPECT(
                        codec::build_inspect_response(target_guid),
                    )),
                )?,
                Err(e) => log::debug!("world: inspect ignored (account {}): {e}", conn.account_id),
            }
        }
        // Creature name resolution (the NPC analogue of CMSG_NAME_QUERY).
        ClientOpcodeMessage::CMSG_CREATURE_QUERY(q) => {
            match store.creature_template(q.creature)? {
                Some(c) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_CREATURE_QUERY_RESPONSE(Box::new(
                        codec::build_creature_query_response(&c),
                    ))),
                )?,
                None => log::debug!("world: creature query for unknown entry {}", q.creature),
            }
        }
        // Gossip (rank 12, extended with imported gossip menus): the player right-clicked a gossip NPC
        // (npc_flags GOSSIP bit). Reply with a title (resolved via the NPC_TEXT round-trip below) +
        // either the NPC's IMPORTED menu options (precedence) or the flag-derived vendor/innkeeper
        // synthesis (fallback) + the QUEST section. A gossip-FLAGGED questgiver (npc_flags
        // GOSSIP|QUESTGIVER, e.g. Marshal McBride) delivers its quests here, not via
        // CMSG_QUESTGIVER_HELLO, so fold the same quest menu in (empty for a plain gossip NPC →
        // unchanged).
        ClientOpcodeMessage::CMSG_GOSSIP_HELLO(h) => {
            let npc = h.guid.guid();
            let player_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            // A gossip NPC that dislikes you doesn't open its menu (silent drop —
            // vanilla unfriendly NPCs just ignore the click).
            if player_guid != 0
                && store
                    .npc_refuses_interaction(npc, player_guid)
                    .unwrap_or(false)
            {
                return Ok(None);
            }
            let quests = match &conn.state {
                WorldState::InWorld(iw) => quest::gossip_quest_items(store, npc, iw.self_guid)?,
                WorldState::CharSelect => Vec::new(),
            };
            // A vendor that ALSO has the gossip bit gets a "browse goods" menu entry (rank-vendor #6);
            // having stock is the is-vendor signal, so no npc_flags read is needed. An innkeeper gets a
            // "Make this inn your home." entry (hearthstone bind) — that one DOES need the npc_flags
            // read. Both are APPENDED to the imported options rather than replaced by them: a dump menu
            // that omits the row would otherwise strand the NPC's stock or its bind.
            let is_vendor = vendor_has_stock(store, npc)?;
            let is_innkeeper = store.npc_is_innkeeper(npc)?;
            let options = codec::gossip_menu_options(
                filtered_gossip_options(store, npc, player_guid)?,
                is_vendor,
                is_innkeeper,
            );
            // Snapshot what this client is about to look at — the select handler resolves the clicked
            // POSITION against this, never against a fresh read (see `GossipMenuSnapshot`).
            conn.gossip_menu = Some(GossipMenuSnapshot {
                npc_guid: npc,
                options: options.iter().map(|o| (o.row_id, o.action)).collect(),
            });
            let title_text_id = store.npc_gossip_text_id(npc);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(Box::new(
                    codec::build_gossip_message(npc, title_text_id, quests, &options),
                ))),
            )?;
        }
        // The client resolves a gossip/quest title text id (sent in SMSG_GOSSIP_MESSAGE) → reply with
        // the NPC's imported (weighted) text, or the generic greeting when none is
        // imported yet.
        ClientOpcodeMessage::CMSG_NPC_TEXT_QUERY(q) => {
            let view = store.npc_text_for_id(q.text_id);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_NPC_TEXT_UPDATE(Box::new(
                    codec::build_npc_text_update(q.text_id, view.as_ref()),
                ))),
            )?;
        }
        // The player clicked a gossip option. The click carries a POSITION, resolved against the
        // snapshot HELLO took (`GossipMenuSnapshot`), then routed by ACTION: vendor → inventory,
        // innkeeper → bind_home, trainer → SMSG_TRAINER_LIST, taxi → the same cohesive
        // open operation as CMSG_TAXIQUERYAVAILABLENODES, banker → SMSG_SHOW_BANK, everything
        // else including the trailing Farewell → SMSG_GOSSIP_COMPLETE. Submenu navigation is
        // deferred (`action_menu_id` stays inert).
        ClientOpcodeMessage::CMSG_GOSSIP_SELECT_OPTION(c) => {
            let npc = c.guid.guid();
            let player_guid = match &conn.state {
                WorldState::InWorld(iw) => iw.self_guid,
                WorldState::CharSelect => 0,
            };
            // A click naming an NPC other than the one the open menu belongs to is stale (the client
            // sends HELLO before it can show a menu), so it selects nothing.
            let clicked = conn
                .gossip_menu
                .as_ref()
                .filter(|snap| snap.npc_guid == npc)
                .and_then(|snap| snap.options.get(c.gossip_list_id as usize))
                .copied();
            // The module is told the clicked row's `row_id`, not its position: a position is
            // per-viewer (a cond-gated row renumbers it), so it identifies nothing to a package.
            let option_row_id = clicked.map_or(codec::SYNTHESIZED_ROW_ID, |(row_id, _)| row_id);
            // Notify the module (the on_gossip_select hook chokepoint) — best-effort,
            // so a module hiccup never blocks the gossip reply below.
            let _ = store.gossip_select(
                conn.account_id,
                self_guid,
                npc,
                c.gossip_list_id,
                option_row_id,
            );
            use lyracore_shared::constants::gossip_option;
            match clicked.map(|(_, action)| action) {
                // The gossip click does not run the interaction gate — it never has: only the
                // direct CMSG_LIST_INVENTORY open does.
                Some(gossip_option::VENDOR) => {
                    for message in vendor_open_outbound(store, npc)? {
                        send(tx, message)?;
                    }
                }
                Some(gossip_option::INNKEEPER) => {
                    // Bind failure (not in world) is per-action; close the window either way (the
                    // post-bind SMSG_BINDPOINTUPDATE confirmation is cosmetic — sent fresh at next
                    // login; the recall is server-authoritative regardless).
                    let _ = store.bind_home(conn.account_id, social::self_guid(conn).unwrap_or(0));
                    send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?;
                }
                Some(gossip_option::TRAINER) => {
                    let spells = store.trainer_list(player_guid, npc)?;
                    let list =
                        codec::build_trainer_list(npc, &spells, "I can teach you a thing or two.");
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_LIST(Box::new(list))),
                    )?;
                }
                Some(gossip_option::UNLEARNTALENTS) => {
                    // Respec (#516). Errors (out of range / not enough gold) are per-action —
                    // the window closes either way, same as bind_home above.
                    let _ = store.reset_talents(conn.account_id, player_guid, npc);
                    send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?;
                }
                Some(gossip_option::TAXI) => {
                    for message in open_taxi_outbound(store, player_guid, npc)? {
                        send(tx, message)?;
                    }
                }
                Some(gossip_option::BANKER) => super::send_show_bank(tx, npc)?,
                Some(gossip_option::TABARDDESIGNER) => {
                    send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?;
                    send(tx, super::guild::tabard_designer_window(npc))?;
                }
                Some(gossip_option::PETITIONER) => {
                    send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?;
                    send(tx, super::guild::petition_showlist(npc))?;
                }
                // Plain-GOSSIP/submenu-link, the trailing Farewell, or a click with no live snapshot
                // behind it — close the window.
                _ => send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?,
            }
        }
        // Item template resolution (items slice-1): the client queries an item it has encountered
        // (it holds the object) for its name/tooltip/icon. Always reply — `build_item_query_response`
        // emits a NotFound (`found: None`) for an unknown entry so the client stops re-asking.
        ClientOpcodeMessage::CMSG_ITEM_QUERY_SINGLE(q) => {
            let resp = Box::new(codec::build_item_query_response(
                q.item,
                store.item_template(q.item)?.as_ref(),
            ));
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(resp)),
            )?;
        }
        // Social tier: say/yell/`/e` -> send_chat (insert a broadcast game_chat_event the gateway
        // fans back as SMSG_MESSAGECHAT to listeners in range). Whisper, party, raid, guild,
        // officer chat and `/afk` `/dnd` never reach this arm: `dispatch_chat_action` consumes
        // them. No reply on success (the speaker sees their own line via the relay). A say, yell
        // or `/e` line in a language the speaker's race does not know answers "You don't know that
        // language"; every other Refusal is silent, matching vanilla.
        //
        // GM playtest dot-commands: a Say line starting with `.` diverts BEFORE
        // `send_chat` — never broadcast, never inserted as a `game_chat_event` row — straight to the
        // module's one generic `gm_command` reducer. A reducer `Err` (bad gm_level, unknown command,
        // bad args) is relayed back to the SENDER ONLY as a system chat line (`SMSG_MESSAGECHAT`
        // System); success has no reply (the command's own effect — a teleport, a stat change — is
        // its own feedback).
        ClientOpcodeMessage::CMSG_MESSAGECHAT(c) => {
            let CMSG_MESSAGECHAT {
                chat_type,
                language,
                message,
            } = *c;
            let lang = language.as_int() as u8;
            match chat_type {
                CMSG_MESSAGECHAT_ChatType::Say if super::is_guild_dot_command(&message) => {
                    let player = super::GuildActionPlayer {
                        account_id: conn.account_id,
                        self_guid: social::self_guid(conn),
                    };
                    if let Some(line) = super::run_guild_dot_command(store, player, &message)? {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                                codec::build_gm_system_message(line),
                            ))),
                        )?;
                    }
                }
                CMSG_MESSAGECHAT_ChatType::Say if message.starts_with('.') => {
                    if let Err(e) = store.gm_command(&conn.account_name, self_guid, message) {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                                codec::build_gm_system_message(e.to_string()),
                            ))),
                        )?;
                    }
                }
                CMSG_MESSAGECHAT_ChatType::Say => {
                    speak_nearby(tx, store, conn, broadcast_chat::SAY, lang, message)?;
                }
                CMSG_MESSAGECHAT_ChatType::Yell => {
                    speak_nearby(tx, store, conn, broadcast_chat::YELL, lang, message)?;
                }
                // `/e` custom emote: same broadcast path as Say/Yell, EMOTE type.
                CMSG_MESSAGECHAT_ChatType::Emote => {
                    speak_nearby(tx, store, conn, broadcast_chat::EMOTE, lang, message)?;
                }
                // Whisper, party, raid, channel, guild and officer lines and `/afk` `/dnd` never
                // get here: `dispatch_chat_action` consumes them.
                _ => {}
            }
        }
        // Social tier: a text emote (/dance, /wave, …) → send_emote (insert a broadcast
        // game_emote_event the gateway fans back as SMSG_TEXT_EMOTE + SMSG_EMOTE). The client supplies
        // the social-emote id, the animation, and its selected target (0 guid = untargeted); the
        // gateway resolves the target guid to a name on relay. Failure is dropped.
        ClientOpcodeMessage::CMSG_TEXT_EMOTE(c) => {
            let _ = store.send_emote(
                conn.account_id,
                self_guid,
                c.text_emote.as_int(),
                c.emote,
                c.target.guid(),
            );
        }
        // /roll is a Group Broadcast: the party authority draws the result and sends it to every
        // group member on any shard, or to the roller alone when ungrouped.
        ClientOpcodeMessage::MSG_RANDOM_ROLL(r) => {
            let op = party::Op::RandomRoll {
                min: r.minimum,
                max: r.maximum,
            };
            social::run_group_broadcast(store, conn, op);
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
