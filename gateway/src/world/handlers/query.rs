//! Query / social family: name / creature / item lookups + the gossip / npc-text round-trips,
//! plus who/friend/ignore. Pure code-motion out of `world/mod.rs`.

use super::super::*;

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
            let (taken, rewarded) = store.quest_status(player_guid, opt.cond_value1);
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

/// Query / social family: name / creature / item lookups + the gossip / npc-text round-trips, plus
/// the social tier (say / yell / whisper chat + text emotes) — grouped as the stateless
/// request→reply / broadcast opcodes.
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
            match party::character_anywhere(store, guid)? {
                Some(c) => send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_NAME_QUERY_RESPONSE(Box::new(
                        codec::build_name_query_response(&c)?,
                    ))),
                )?,
                None => log::debug!("world: name query for unknown guid {guid}"),
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
                WorldState::InWorld(iw) => {
                    codec::quest_menu_items(&store.quest_giver_evals(npc, iw.self_guid)?)
                }
                WorldState::CharSelect => Vec::new(),
            };
            // A vendor that ALSO has the gossip bit gets a "browse goods" menu entry (rank-vendor #6);
            // having stock is the is-vendor signal, so no npc_flags read is needed. An innkeeper gets a
            // "Make this inn your home." entry (hearthstone bind) — that one DOES need the npc_flags
            // read. Both are APPENDED to the imported options rather than replaced by them: a dump menu
            // that omits the row would otherwise strand the NPC's stock or its bind.
            let is_vendor = !store.vendor_items(npc)?.is_empty();
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
        // innkeeper → bind_home, trainer → SMSG_TRAINER_LIST, banker → SMSG_SHOW_BANK, everything else
        // including the trailing Farewell → SMSG_GOSSIP_COMPLETE. Submenu navigation is deferred
        // (`action_menu_id` stays inert).
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
            let _ = store.gossip_select(conn.account_id, self_guid, npc, c.gossip_list_id, option_row_id);
            use lyracore_shared::constants::gossip_option;
            match clicked.map(|(_, action)| action) {
                Some(gossip_option::VENDOR) => {
                    let items = store.vendor_items(npc)?;
                    let (opcode, body) = codec::build_list_inventory_raw(npc, &items);
                    send(tx, Outbound::Raw { opcode, body })?;
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
                Some(gossip_option::BANKER) => super::send_show_bank(tx, npc)?,
                // TAXI/plain-GOSSIP/submenu-link, the trailing Farewell, or a click with no live
                // snapshot behind it — close the window.
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
        // Social tier: say/yell -> send_chat (insert a broadcast game_chat_event the gateway fans back
        // as SMSG_MESSAGECHAT on every connection's subscription); whisper -> send_whisper (private,
        // per-recipient); party -> party_chat (per-recipient, RLS-scoped to the
        // caller's CURRENT group). Guild/channel still need systems that don't exist yet and are
        // dropped. No reply on say/yell/party success (the speaker sees their own line via the
        // relay — party echoes back through the SAME per-recipient event a real member would get); a
        // rejected say/yell/whisper-target line is silently dropped, matching vanilla; a rejected
        // party line replies only for "not in a group" (see the Party arm below).
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
                CMSG_MESSAGECHAT_ChatType::Say if message.starts_with('.') => {
                    if let Err(e) = store.gm_command(conn.account_id, self_guid, message) {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                                codec::build_gm_system_message(e.to_string()),
                            ))),
                        )?;
                    }
                }
                CMSG_MESSAGECHAT_ChatType::Say => {
                    let _ = store.send_chat(conn.account_id, self_guid, 0, lang, message);
                }
                CMSG_MESSAGECHAT_ChatType::Yell => {
                    let _ = store.send_chat(conn.account_id, self_guid, 1, lang, message);
                }
                // Channel: route to the membership-validated reducer; the relay fans it out
                // to members. A rejection (not joined / dead) is per-action — log + drop, vanilla
                // shows nothing for a failed channel line.
                CMSG_MESSAGECHAT_ChatType::Channel { channel } => {
                    if let Err(e) = store.send_channel_message(conn.account_id, self_guid, channel, message) {
                        log::debug!(
                            "world: channel message rejected (account {}): {e}",
                            conn.account_id
                        );
                    }
                }
                // Whisper: private delivery to a named player (+ a "To X:" echo to the sender). The
                // module resolves the name (case-insensitive) → recipient identity and RLS-scopes both
                // rows. A rejected whisper (no such online player) → SMSG_CHAT_PLAYER_NOT_FOUND so the
                // sender sees "No player named X is online" instead of a silent drop. (In practice the
                // only realistic failure is an unknown/offline target — a logged-in player is always
                // "in world" and the client never sends an empty whisper.)
                CMSG_MESSAGECHAT_ChatType::Whisper { target_player } => {
                    // The whisper goes through `world::whisper`, which decides
                    // WHICH DATABASE carries it — realm-core when the gateway is multi-database (the
                    // only plane on which a target standing on another shard can be named at all),
                    // the player's own shard otherwise. The reply is unchanged on both planes: any
                    // refusal is SMSG_CHAT_PLAYER_NOT_FOUND carrying the name the player typed.
                    //
                    // `social::self_guid` — never a literal: on the realm plane the sender's guid is
                    // an ARGUMENT to the operator-gated reducer, so the guid this socket
                    // authenticated with IS the authorization (a whisper attributed to somebody else
                    // is impersonation — an earlier version of this code made exactly that mistake).
                    let sender = social::self_guid(conn);
                    if whisper::run(store, conn.account_id, sender, &target_player, message)
                        .is_err()
                    {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(
                                Box::new(SMSG_CHAT_PLAYER_NOT_FOUND {
                                    name: target_player,
                                }),
                            )),
                        )?;
                    }
                }
                // Party (`/p`): routes to the caller's CURRENT group members via
                // `party_chat` (the module validates membership + bounds; the gateway never reads
                // group state itself). Not in a group → the standard `SMSG_PARTY_COMMAND_RESULT`
                // "You aren't in a party" line (the SAME mapping `group_leave`/`group_uninvite`
                // already use for this exact reducer error — see `social::party_result_for`'s
                // needle list). Any OTHER rejection (not in world / empty message) is silently
                // dropped, matching say/yell — the client never sends an empty line anyway.
                CMSG_MESSAGECHAT_ChatType::Party => {
                    if let Err(e) = store.party_chat(conn.account_id, self_guid, message) {
                        if e.to_string()
                            .contains(lyracore_shared::group::err::NOT_IN_GROUP)
                        {
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(
                                    Box::new(codec::build_party_command_result(
                                        PartyOperation::Leave,
                                        String::new(),
                                        PartyResult::NotInGroup,
                                    )),
                                )),
                            )?;
                        }
                    }
                }
                _ => {} // guild/channel/etc. need systems that don't exist yet
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
        // /roll: the client sends MSG_RANDOM_ROLL_Client (CMSG direction) with its min/max. We pick
        // a server-side result and broadcast MSG_RANDOM_ROLL_Server to all nearby players via the
        // game_roll_event broadcast table. Failure (not in world) is silently dropped.
        ClientOpcodeMessage::MSG_RANDOM_ROLL(r) => {
            let _ = store.send_roll(conn.account_id, self_guid, r.minimum, r.maximum);
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
