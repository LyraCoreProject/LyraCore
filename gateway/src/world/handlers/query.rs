//! Query / social family: name / creature / item lookups + the gossip / npc-text round-trips,
//! plus who/friend/ignore. Pure code-motion out of `world/mod.rs`.

use super::super::*;
use super::quest;
use super::taxi::open_taxi_outbound;
use super::trainer::settle_per_action;
use super::vendor::{vendor_has_stock, vendor_open_outbound};
use crate::stdb::{classify, DurableFailure};
use lyracore_shared::chat::broadcast_chat;

/// What the GM reads when a dot-command arrives before the session has a Character.
const NO_CHARACTER_FOR_GM_COMMAND: &str = "gm_command: actor_guid unresolved";

/// NPC and gameobject templates, gossip, area triggers and inspect.
pub(crate) trait NpcStore: Send + Sync {
    /// Look up a creature template by entry to answer `CMSG_CREATURE_QUERY` (Tier 2 / NPCs).
    fn creature_template(&self, entry: u32) -> Result<Option<codec::CreatureView>>;

    /// Resolve the public name of a live pet for an in-world requester.
    fn pet_name(
        &self,
        requester: Actor,
        pet_number: u32,
        pet_guid: u64,
    ) -> Result<Option<codec::PetNameView>>;

    /// Look up a gameobject template by entry to answer `CMSG_GAMEOBJECT_QUERY`.
    fn gameobject_template(&self, entry: u32) -> Result<Option<codec::GameObjectTemplateView>>;

    /// The `type_id` of a spawned GameObject by its live guid. `CMSG_GAMEOBJ_USE` sends questgivers
    /// to the quest window, chests to the loot-window lifecycle, and every other type to the general
    /// use path.
    fn gameobject_type(&self, go_guid: u64) -> Result<Option<u8>>;

    /// Enter an area trigger (`CMSG_AREATRIGGER`): credit any active "explore" quest tied to it.
    fn enter_areatrigger(&self, actor: Actor, trigger_id: u32) -> Result<()>;

    /// Standing-derived reaction gate: does this NPC refuse `player_guid` its
    /// interaction WINDOW? Rep-bar factions refuse at Unfriendly-or-below standing; bar-less
    /// factions fall back to the FactionTemplate hostility masks. Fail-open on missing data.
    /// Gossip, trainer and banker ask here; the migrated families ask the same read through their
    /// own traits (`QuestActionStore::giver_refuses_interaction`,
    /// `VendorActionStore::vendor_refuses_interaction`).
    fn npc_refuses_interaction(&self, npc_guid: u64, player_guid: u64) -> Result<bool>;

    /// Bind the caller's hearthstone home to their current position (innkeeper gossip "Make this inn
    /// your home."). No args — the module resolves the caller via `ctx.sender`.
    fn bind_home(&self, actor: Actor) -> Result<()>;

    /// Does the NPC at `guid` carry the innkeeper flag? Gates the "Make this inn your home." gossip
    /// option + the bind select.
    fn npc_is_innkeeper(&self, guid: u64) -> Result<bool>;

    /// Resolve the `title_text_id` to embed in `SMSG_GOSSIP_MESSAGE` for the NPC at `guid`.
    /// Looks up `game_gossip_menu` by creature entry; falls back to 1 (generic greeting).
    fn npc_gossip_text_id(&self, npc_guid: u64) -> u32;

    /// Look up the full weighted greeting (all 8 `npc_text` slots) for a `text_id`.
    /// Returns `None` when no imported `game_npc_text` row exists (the gateway falls back to the
    /// generic greeting string).
    fn npc_text_for_id(&self, text_id: u32) -> Option<codec::NpcTextView>;

    /// The imported gossip menu options for the NPC at `guid`, sorted by
    /// `option_index`, RAW/unfiltered by condition. Empty when nothing is imported for this creature
    /// (the gateway falls back to the flag-derived vendor/innkeeper synthesis).
    fn gossip_options(&self, npc_guid: u64) -> Result<Vec<codec::GossipOptionView>>;

    /// Validate a `CMSG_INSPECT` request: `target_guid` must be a real in-world player, on the
    /// caller's map, in range, and friendly. `Ok(())` → the gateway replies `SMSG_INSPECT(target_guid)`;
    /// `Err` (out of range / hostile / no such target) → silently ignored, matching the other
    /// stateless-gate reducers (`enter_areatrigger`, `use_gameobject`).
    fn inspect(&self, actor: Actor, target_guid: u64) -> Result<()>;

    /// NOTIFY-ONLY module chokepoint for a gossip-option click — fired best-effort
    /// before the gateway's own gossip handling; failure never blocks the gossip reply.
    fn gossip_select(
        &self,
        actor: Actor,
        npc_guid: u64,
        option_id: u32,
        option_row_id: u32,
    ) -> Result<()>;
}

/// The NPC's imported gossip options, condition-filtered against `player_guid`'s quest
/// state — the SINGLE chokepoint both `CMSG_GOSSIP_HELLO` (render) and `CMSG_GOSSIP_SELECT_OPTION`
/// (re-derive the click) call, so the two can never disagree about which options are visible (the
/// "HELLO/SELECT_OPTION alignment" trap: a click's `gossip_list_id` indexes into whatever list HELLO
/// actually sent, so SELECT must reproduce that exact list, not just re-read the raw unfiltered rows).
/// Preserves `option_index` order (already sorted by the store read).
fn filtered_gossip_options<
    St: CharacterStore + NpcStore + QuestActionStore + TrainerStore + ?Sized,
>(
    store: &St,
    npc_guid: u64,
    player_guid: u64,
) -> Result<Vec<codec::GossipOptionView>> {
    use lyracore_shared::constants::{gossip_option, MIN_TALENT_LEVEL};
    // The unlearn-talents row has no condition of its own (cmangos gates it in C++ code at
    // GossipHello, not via a `conditions` row — see `gossip_option::UNLEARNTALENTS`'s doc), so it
    // needs its own level check here rather than falling through `option_condition_holds`. Below
    // level 10 a character literally cannot have a talent point, so the option would be inert even
    // if shown.
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
/// returns on the Relay; a Refusal gets the answer every chat line shares, and only a Transport
/// Loss ends the World Session. A speaker with no Character yet is silent.
fn speak_nearby<St: SpeechStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &WorldConn,
    actor: Option<Actor>,
    chat_type: u8,
    language: u8,
    message: String,
) -> Result<()> {
    let Some(actor) = actor else {
        log::debug!(
            "world: broadcast chat type {chat_type} dropped (account {}): no Character",
            conn.account_id
        );
        return Ok(());
    };
    let player = super::ChatActionPlayer {
        account_id: conn.account_id,
        self_guid: social::self_guid(conn),
    };
    let refusal = match store.send_chat(actor, chat_type, language, message) {
        Ok(ChatOutcome::Delivered) => None,
        Ok(ChatOutcome::Refused(refusal)) => Some(refusal),
        Err(error) => match classify(&error) {
            DurableFailure::Refusal { reason } => {
                log::debug!(
                    "world: broadcast chat type {chat_type} dropped (account {}): {reason}",
                    player.account_id
                );
                None
            }
            DurableFailure::TransportLoss => return Err(error),
        },
    };
    for message in super::chat::refusal_outbound(player, refusal) {
        send(tx, message)?;
    }
    Ok(())
}

/// Query / social family: name / creature / item lookups + the gossip / npc-text round-trips, plus
/// the social tier (say / yell / `/e` chat + text emotes), grouped as the stateless
/// request→reply / broadcast opcodes.
#[allow(clippy::too_many_lines)] // One arm per query and social opcode.
pub(crate) fn handle_query<
    St: CastStore
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
>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    // `None` until the session has a Character in the world.
    let actor = match &conn.state {
        WorldState::InWorld(iw) => Actor::new(iw.self_guid),
        _ => None,
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
            let pet = match actor {
                Some(actor) => store.pet_name(actor, q.pet_number, pet_guid)?,
                None => None,
            };
            match pet {
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
            if let Some(actor) = actor {
                match store.inspect(actor, target_guid) {
                    Ok(()) => send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_INSPECT(
                            codec::build_inspect_response(target_guid),
                        )),
                    )?,
                    Err(error) => settle_per_action("inspect", conn.account_id, Err(error))?,
                }
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
            let player_guid = actor.map_or(0, Actor::guid);
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
            // A vendor that ALSO has the gossip bit gets a "browse goods" menu entry (rank-vendor);
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
            let player_guid = actor.map_or(0, Actor::guid);
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
            if let Some(actor) = actor {
                settle_per_action(
                    "gossip_select",
                    conn.account_id,
                    store.gossip_select(actor, npc, c.gossip_list_id, option_row_id),
                )?;
            }
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
                    if let Some(actor) = actor {
                        settle_per_action("bind_home", conn.account_id, store.bind_home(actor))?;
                    }
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
                    // Respec. Errors (out of range / not enough gold) are per-action,
                    // the window closes either way, same as bind_home above.
                    if let Some(actor) = actor {
                        settle_per_action(
                            "reset_talents",
                            conn.account_id,
                            store.reset_talents(actor, npc),
                        )?;
                    }
                    send(tx, Outbound::One(ServerOpcodeMessage::SMSG_GOSSIP_COMPLETE))?;
                }
                Some(gossip_option::TAXI) => {
                    for message in open_taxi_outbound(store, actor, npc)? {
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
        // Item template resolution: the client queries an item it has encountered
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
                // The GM reads the Module's Refusal text verbatim. Any other failure ends the
                // World Session.
                CMSG_MESSAGECHAT_ChatType::Say if message.starts_with('.') => {
                    let reply = match actor {
                        Some(actor) => match store.gm_command(&conn.account_name, actor, message) {
                            Ok(()) => None,
                            Err(error) => match classify(&error) {
                                DurableFailure::Refusal { reason } => Some(reason.to_string()),
                                DurableFailure::TransportLoss => return Err(error),
                            },
                        },
                        None => Some(NO_CHARACTER_FOR_GM_COMMAND.to_string()),
                    };
                    if let Some(line) = reply {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                                codec::build_gm_system_message(line),
                            ))),
                        )?;
                    }
                }
                CMSG_MESSAGECHAT_ChatType::Say => {
                    speak_nearby(tx, store, conn, actor, broadcast_chat::SAY, lang, message)?;
                }
                CMSG_MESSAGECHAT_ChatType::Yell => {
                    speak_nearby(tx, store, conn, actor, broadcast_chat::YELL, lang, message)?;
                }
                // `/e` custom emote: same broadcast path as Say/Yell, EMOTE type.
                CMSG_MESSAGECHAT_ChatType::Emote => {
                    speak_nearby(tx, store, conn, actor, broadcast_chat::EMOTE, lang, message)?;
                }
                // Whisper, party, raid, channel, guild and officer lines and `/afk` `/dnd` never
                // get here: `dispatch_chat_action` consumes them.
                _ => {}
            }
        }
        // Social tier: a text emote (/dance, /wave, …) → send_emote (insert a broadcast
        // game_emote_event the gateway fans back as SMSG_TEXT_EMOTE + SMSG_EMOTE). The client supplies
        // the social-emote id, the animation, and its selected target (0 guid = untargeted); the
        // gateway resolves the target guid to a name on relay. A Refusal is dropped.
        ClientOpcodeMessage::CMSG_TEXT_EMOTE(c) => {
            if let Some(actor) = actor {
                settle_per_action(
                    "send_emote",
                    conn.account_id,
                    store.send_emote(actor, c.text_emote.as_int(), c.emote, c.target.guid()),
                )?;
            }
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
