//! Char / world-entry family: character enum + creation (character-select), then enter-world
//! (login and cross-map world-port). Pure code-motion out of `world/mod.rs`.

use super::super::*;
use super::quest::{quest_log_update, QuestActionStore};
use super::vendor::build_buyback_view_replay;

/// Tell a client whose world-port cannot complete that it is off, so its loading screen ends with an
/// error instead of never ending. Best-effort and infallible by design: it runs on
/// a path that is already failing, and every one of its own failure modes (an unmapped destination
/// map, a dead socket) is strictly less bad than the hang it replaces, so none of them may mask the
/// original error the caller is about to propagate.
///
/// The destination map comes from the character's own durable row — the same row
/// `world::teleport_player` wrote the destination into before it despawned the entity, i.e. the map
/// the client is loading right now. `TransferAbortReason::NotFound` is the closest vanilla reason to
/// "the shard that owns this instance would not take you"; the operator-facing detail is the log line.
fn abort_pending_transfer<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    character_guid: u64,
    cause: &anyhow::Error,
) {
    let dest_map = store
        .character_destination(character_guid)
        .map(|p| p.dest_map_id);
    log::error!(
        "world: world-port for guid {character_guid} cannot complete ({cause:#}) — aborting the \
         client's transfer to map {dest_map:?}. The character is unharmed: the escrow is idempotent \
         and the next login re-drives it."
    );
    let Some(map_id) = dest_map else {
        log::warn!(
            "world: no durable destination for guid {character_guid} — the client gets no \
             SMSG_TRANSFER_ABORTED and will need to reconnect"
        );
        return;
    };
    use wow_world_messages::vanilla::TransferAbortReason;
    match codec::build_transfer_aborted(map_id, TransferAbortReason::NotFound) {
        Ok(msg) => {
            let _ = send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_TRANSFER_ABORTED(msg)),
            );
        }
        Err(e) => {
            log::warn!("world: could not build SMSG_TRANSFER_ABORTED for map {map_id}: {e:#}")
        }
    }
}

/// Build + send the player's quest-log descriptor fields as a raw VALUES update (Phase 2) — the
/// world-entry copy of the block, sent right after the self CREATE. A no-op when the gate is off
/// or the player has no active quests (`quest_log_update` returns an empty batch, so the CREATE
/// packet's already-zeroed fields stand). The in-session relay on accept / progress / turn-in is
/// `stdb::subscriptions`'s `quest_log_sync`, which renders the same descriptor from the same
/// builders.
fn send_quest_log<St: QuestActionStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    player_guid: u64,
) -> Result<()> {
    if !crate::config::quest_log_fields_enabled() {
        return Ok(());
    }
    // A read failure is treated as an empty log, not a login failure — the same defensive
    // `unwrap_or_default` every other read in `enter_world` uses; this one is display-only too.
    for message in quest_log_update(store, player_guid).unwrap_or_default() {
        send(tx, message)?;
    }
    Ok(())
}

/// The guild's message of the day, as the one `SMSG_GUILD_EVENT(Motd)` vanilla shows at world
/// entry. Empty batch for a guildless character and for a guild whose MOTD is unset.
///
/// A read failure yields an empty batch rather than failing the login: the greeting is display-only,
/// and the same MOTD arrives live the next time the master sets it.
fn guild_motd_greeting<St: super::GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
) -> Vec<Outbound> {
    let motd = store
        .guild_of(character_guid)
        .ok()
        .flatten()
        .and_then(|guild_id| store.guild_view(guild_id).ok().flatten())
        .map(|view| view.motd)
        .unwrap_or_default();
    if motd.is_empty() {
        return Vec::new();
    }
    vec![Outbound::One(ServerOpcodeMessage::SMSG_GUILD_EVENT(
        Box::new(codec::build_guild_event(
            wow_world_messages::vanilla::GuildEvent::Motd,
            std::slice::from_ref(&motd),
        )),
    ))]
}

/// Enter (or RE-enter) the world as `character_guid`: rebuild the live entity, subscribe
/// to its per-player views (a FRESH `created` dedup set every call — the full AOI reset a cross-map
/// re-entry needs), and send the login sequence + self CREATE_OBJECT as one contiguous batch. Shared by
/// `CMSG_PLAYER_LOGIN` (fresh world entry) and `MSG_MOVE_WORLDPORT_ACK` (cross-map re-entry after
/// `teleport_player` despawned the old entity) — see their call sites' doc comments for why reusing this
/// exact path is correct for both. `session_epoch` is the caller's to manage: a fresh login claims a new
/// one; a world-port reuses the existing one (the session itself hasn't changed). `entry` is the ONE
/// packet-level difference between the two callers: a world-port re-entry omits
/// `SMSG_LOGIN_VERIFY_WORLD` (see `codec::WorldEntry`).
///
/// Drops any PREVIOUS `InWorld` state FIRST (a world-port re-entry has one, scoped to the OLD map/AOI
/// box; a fresh login doesn't) — the old `PlayerSubscriptions`' RAII `Drop` unregisters every callback +
/// tears down the old AOI tracker before this registers new ones, so nothing double-fires and nothing
/// leaks a stale grid subscription pointed at a map the player already left.
fn enter_world<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    character_guid: u64,
    session_epoch: u64,
    entry: codec::WorldEntry,
) -> Result<()> {
    conn.state = WorldState::CharSelect;

    let mut entity = store.player_login(conn.account_id, character_guid)?;
    // Character sheet (UNIT_FIELD_RESISTANCES[0]): override the BASE armor `player_login` set with
    // the EFFECTIVE armor (base + worn gear) so the Armor readout is correct at relog. Armor auras
    // self-correct via the on_aura relay; combat mitigation is unchanged (the module still folds its
    // own effective_armor on demand — this only feeds the display descriptor).
    entity.effective_armor = store.effective_armor(character_guid);
    log::info!(
        "world: entering world guid={character_guid} -> entity at map {} ({:.1},{:.1},{:.1}); subscribing + sending login sequence + self-spawn",
        entity.map_id, entity.x, entity.y, entity.z
    );
    // Items slice-1: the character's owned items. Each becomes an item CREATE_OBJECT sent
    // BEFORE the player self-spawn (so the inventory-slot guid resolves to an object the
    // client already has), and the (slot, guid) pairs seed the player's PLAYER_FIELD_INV_SLOT
    // descriptors. Empty for a character that owns nothing — login is otherwise unchanged.
    let items = store.player_items(character_guid).unwrap_or_default();
    let inventory: Vec<(u8, u64, u32)> = items.iter().map(|i| (i.slot, i.guid, i.entry)).collect();
    let learned = store
        .player_learned_spells(character_guid)
        .unwrap_or_default();
    let skills = store.player_skills(character_guid).unwrap_or_default();
    let reputations = store.player_reputations(character_guid).unwrap_or_default();
    // Imported action-bar rows (empty pre-import — the login codec falls back
    // to synthesizing the bar from `learned` in that case, byte-identical to before).
    let player_actions = store.player_actions(character_guid).unwrap_or_default();
    let mut batch =
        codec::login_sequence_messages(&entity, &learned, &reputations, &player_actions, entry)?;
    for item in &items {
        batch.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_item_create_object(item),
        )));
    }
    batch.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
        codec::build_create_object(&entity, codec::CreateKind::SelfPlayer, &inventory, &skills)?,
    )));
    send(tx, Outbound::Batch(batch))?;
    // Subscribe AFTER the self-spawn batch is on the wire — so the AOI initial-apply creates for
    // entities ALREADY in view (notably a questgiver you spawn right next to) arrive AFTER the
    // client is in-world. Spawning ON a questgiver otherwise left it targetable but with no '!' /
    // right-click: its CREATE raced the login sequence and the client registered it as a plain
    // unit, never polling its quest status. The streaming path (a peer entering view later) was
    // always fine — this makes the login case match it. (Missing a peer that
    // inserts in the µs window between this send and the subscribe is negligible.) The dedup set
    // is seeded with self_guid so the player's own row (delivered on initial apply) is skipped.
    let subs = store.subscribe_player_events(
        conn.account_id,
        character_guid,
        entity.instance_id,
        entity.map_id,
        entity.x,
        entity.y,
        tx.clone(),
    )?;
    // Replay the buyback-tab view (the ring persists across sessions; without this the tab
    // is empty until the first in-session sell).
    for message in build_buyback_view_replay(store, character_guid) {
        send(tx, message)?;
    }
    // Put realm-core's party roster onto the shard this character just entered
    // and re-render the party frame. THIS is what carries a party across a shard boundary now that
    // the old interim blob mirror is gone — and it runs on every world entry, so a party formed while the
    // player was on the loading screen lands too. A no-op on a single-database gateway.
    //
    // Failures are logged, not propagated: a party frame that renders late is a cosmetic defect,
    // and failing the login over it would be strictly worse for the player.
    if let Err(e) = party::on_world_entry(tx, store, character_guid) {
        log::warn!("world: party sync at world entry failed for guid {character_guid}: {e:#}");
    }
    // The same push for the character's own guild columns — the only guild state a world shard
    // holds. Also a no-op on a single-database gateway, and logged rather than propagated for the
    // same reason: a stale guild id costs a name plate, and failing the login over it is worse.
    if let Err(e) = crate::world::guild::on_world_entry(store, character_guid) {
        log::warn!("world: guild sync at world entry failed for guid {character_guid}: {e:#}");
    }
    // ...and tell the rest of their guild they are here. Best-effort inside, like everything else
    // on this path: a missing status line is not worth a failed login.
    crate::world::guild::broadcast_presence(store, character_guid, true);
    // The guild's message of the day, which vanilla shows once, at world entry. Read through the
    // routing layer so a sharded gateway reads realm-core, and skipped when the guild has no MOTD:
    // an empty line in the chat frame is noise, not a greeting.
    for message in guild_motd_greeting(store, character_guid) {
        send(tx, message)?;
    }
    // Enter the world: CharSelect → InWorld (a reused connection has no open loot/attack — a world-port
    // re-entry likewise starts clean, since whatever the player was attacking/looting on the old map is
    // meaningless on the new one).
    conn.state = WorldState::InWorld(InWorld {
        self_guid: character_guid,
        subs,
        session_epoch,
        attacking_target: None,
        looting_target: None,
        ranged_repeat: false,
    });
    // Phase 2: the quest-log window. Sent as a separate raw VALUES update AFTER the CREATE
    // (gtker's CREATE can't carry these walled fields), gated behind LYRACORE_QUEST_LOG until verified.
    send_quest_log(tx, store, character_guid)?;
    // If the player carries ammo (a Projectile item, class 6), tell the client it's loaded
    // (PLAYER_AMMO_ID) so Auto Shot is usable. Deliberate simplification: login-time only — no
    // live re-send on pickup/runout (the next login re-syncs; the shot itself gates on the bag
    // having ammo).
    if let Some(ammo_entry) = items.iter().map(|i| i.entry).find(|&e| {
        store
            .item_template(e)
            .ok()
            .flatten()
            .is_some_and(|t| t.class == 6)
    }) {
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_player_ammo_id_values(character_guid, ammo_entry),
            ))),
        )?;
    }
    // Talent-pane points correction: the CREATE's PLAYER_CHARACTER_POINTS1 counts points EARNED
    // (level−9; codec/entity.rs), so a character with SPENT points over-reports until a pick.
    // Push the true remaining once, post-CREATE (same partial-VALUES mechanism as the live pick).
    // Skipped for spent == 0 → a fresh character's login stays byte-identical.
    if store.talent_points_spent(character_guid) > 0 {
        let (_, _, remaining) = store.talent_pane_sync(character_guid, 0);
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_talent_points_values(character_guid, remaining),
            ))),
        )?;
    }
    // Spell-modifier mirror: tell the client which of its spells the learned passives modify
    // (Improved Fireball's cast-time cut etc.) so ITS cast bars/tooltips match the server's folded
    // timings. One packet per (op, mask-bit) total, the mangos convention; none learned → nothing.
    for m in codec::build_spell_modifier_msgs(&store.spell_modifiers(character_guid)) {
        send(tx, Outbound::One(m))?;
    }
    Ok(())
}

/// Char / world-entry family (§4/§5): character enum + creation (character-select), then enter world
/// (`CMSG_PLAYER_LOGIN`) + graceful logout — the session-lifecycle opcodes.
pub(crate) fn handle_char<St: WorldStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        // Phase 3 (§4): character-select screen.
        ClientOpcodeMessage::CMSG_CHAR_ENUM => {
            let characters = store.characters(conn.account_id)?;
            let enum_msg = codec::build_char_enum(&characters)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_ENUM(Box::new(enum_msg))),
            )?;
        }
        // Character creation. Create the row, reply SMSG_CHAR_CREATE; on success the client
        // re-sends CMSG_CHAR_ENUM and the new character appears. A creation failure is NOT
        // session-fatal — report it as a result, never drop the connection.
        ClientOpcodeMessage::CMSG_CHAR_CREATE(c) => {
            let appearance = codec::Appearance {
                skin: c.skin_color,
                face: c.face,
                hair_style: c.hair_style,
                hair_color: c.hair_color,
                facial_hair: c.facial_hair,
            };
            let outcome = store
                .create_character(
                    conn.account_id,
                    c.name.as_str(),
                    c.race.as_int(),
                    c.class.as_int(),
                    c.gender.as_int(),
                    appearance,
                )
                .unwrap_or(codec::CharCreateOutcome::Failed);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_CREATE(
                    codec::build_char_create_response(outcome),
                )),
            )?;
        }
        // Character deletion. Per the wire doc SMSG_CHAR_DELETE alone updates the
        // character-select screen — no re-sent CMSG_CHAR_ENUM needed. Ownership is enforced module-
        // side; a failure is NOT session-fatal, same treatment as CMSG_CHAR_CREATE above.
        ClientOpcodeMessage::CMSG_CHAR_DELETE(d) => {
            let outcome = store
                .delete_character(conn.account_id, d.guid.guid())
                .unwrap_or(codec::CharDeleteOutcome::Failed);
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_DELETE(
                    codec::build_char_delete_response(outcome),
                )),
            )?;
        }
        // Phase 4 (§5): enter world -> register peer subscriptions, then login sequence + self
        // CREATE_OBJECT2 as one contiguous batch (so an async peer event can't splice into it).
        ClientOpcodeMessage::CMSG_PLAYER_LOGIN(p) => {
            let character_guid = p.guid.guid();
            // Claim this account's in-world session: become the current owner of the live entity so a
            // stale earlier socket's teardown can't delete it out from under us. A world-port
            // re-entry (below) reuses the EXISTING epoch instead — the session itself hasn't changed,
            // only the map.
            let session_epoch = store.claim_session(conn.account_id);
            // Multi-shard routing: pin this session to the shard that owns the character's
            // location BEFORE `player_login` runs, so the login reducer, the per-player connection
            // it opens, and the AOI subscriptions all land on the home shard — and so does every
            // message after this one (`run_world_session` re-reads `conn.home` per frame).
            // A single-entry shard map never pins anything → `enter_world` runs on `store`, as it
            // always did.
            conn.route_home(store, character_guid)?;
            on_home_shard!(conn, store, |st| enter_world(
                tx,
                st,
                conn,
                character_guid,
                session_epoch,
                codec::WorldEntry::FreshLogin
            ))?;
        }
        // Cross-map teleport: the client's ack that it finished loading the map named
        // by our `SMSG_NEW_WORLD` (sent from the `on_teleport` relay when `teleport_player` despawned
        // the live entity for a cross-map hop). Per gtker's own doc comment on this opcode — "The server
        // should reply with what it normally does to log players into the world" — so this reuses the
        // EXACT same `enter_world` path CMSG_PLAYER_LOGIN uses: rebuild the entity (`player_login` is
        // idempotent here — the ghost-relog branch no-ops because the entity is ALREADY gone), tear down
        // the OLD map's subscriptions and register fresh ones at the new map/position (a brand new
        // `created` dedup set — the full AOI reset a cross-map re-entry requires, the same "initial-subscribe"
        // precedent already established), and re-send the (now new-map) login sequence + self CREATE_OBJECT —
        // minus SMSG_LOGIN_VERIFY_WORLD (`WorldEntry::WorldPort`): a verify-world resend would command a
        // second load of the map the ack says is already loaded.
        // A spurious/late ack while not mid-transfer (e.g. a double-send) is a no-op — CharSelect has no
        // `self_guid` to re-enter with, so it's silently accepted-and-ignored like every other unsolicited
        // client ack in this dispatch. The session epoch is REUSED (not re-claimed) — nothing about
        // session ownership changed, only the entity/map.
        ClientOpcodeMessage::MSG_MOVE_WORLDPORT_ACK => {
            let resume = match &conn.state {
                WorldState::InWorld(iw) => Some((iw.self_guid, iw.session_epoch)),
                WorldState::CharSelect => None,
            };
            if let Some((character_guid, session_epoch)) = resume {
                // Gate on a REAL pending transfer: cross-map teleport
                // despawns the entity until this ack; a live entity means no transfer is in
                // flight and the ack is spurious — ignore it instead of re-entering the world.
                // `store` is ALREADY the home-shard handle — the read loop routes every frame
                // through `on_home_shard!` — so this reads the cache the entity actually lives in.
                if store.entity_in_world(character_guid) {
                    log::debug!("world: spurious WORLDPORT_ACK ignored (guid {character_guid} still in world)");
                } else {
                    // A world-port changes the map, which can change the owning shard —
                    // re-resolve before re-entering, exactly as a fresh login does. This is also
                    // where the escrowed cross-database transfer actually RUNS.
                    //
                    // FAIL LOUDLY, NEVER HANG. The client is on a loading screen it
                    // entered because we sent it `SMSG_TRANSFER_PENDING`, and the only thing that
                    // ends that screen is us finishing the world entry. Propagating the error here
                    // closes the socket mid-load, which the 1.12 client renders as an infinite
                    // loading bar — the player is stranded with no message and no recourse, which
                    // is strictly worse than any error. So: tell the client the transfer is off
                    // (`SMSG_TRANSFER_ABORTED`), THEN end the session. Nothing durable is lost —
                    // the escrow is idempotent and the next login re-drives it from the same rows.
                    //
                    // The guard covers the WHOLE world-port, not just its routing step (adversarial
                    // review): re-entry can fail on its own — `player_login` refused by the
                    // stranding guard, a subscription that would not register — with the client on
                    // exactly the same loading screen, and that window is the wider of the two.
                    let mut ported = conn.route_home(store, character_guid);
                    if ported.is_ok() {
                        ported = on_home_shard!(conn, store, |st| enter_world(
                            tx,
                            st,
                            conn,
                            character_guid,
                            session_epoch,
                            codec::WorldEntry::WorldPort
                        ));
                    }
                    if let Err(e) = ported {
                        abort_pending_transfer(tx, store, character_guid, &e);
                        return Err(e);
                    }
                }
            }
        }
        // Phase 7: graceful in-game Logout/Exit. Deny if in combat (vanilla behaviour); otherwise
        // ack instantly + complete, remove the entity (observers see DESTROY), drop the peer
        // subscriptions, and return to character-select with the connection still open.
        ClientOpcodeMessage::CMSG_LOGOUT_REQUEST => {
            // In-combat gate: deny logout while combat_until_ms is still in the future. We read the
            // wall-clock here (the gateway is a normal Rust process) and compare against the entity
            // row's ms-epoch timestamp written by `enter_combat`. 0 = never in combat → allowed.
            if let WorldState::InWorld(iw) = &conn.state {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                if store.player_combat_until_ms(iw.self_guid) > now_ms {
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(
                            codec::logout_denied_in_combat(),
                        )),
                    )?;
                    return Ok(None);
                }
            }
            send(tx, Outbound::Batch(codec::logout_sequence()))?;
            // Leave the world: InWorld → CharSelect drops the relay subs; delete the entity only if
            // we still own it — a newer login on this account supersedes us, and deleting then would
            // vanish them. A `logout` failure here is session-fatal (propagated), as before.
            conn.leave_world(store)?;
        }
        // /played: read the durable total + the live session stamp off the
        // character row and fold them in `build_played_time` so an online player's total keeps
        // ticking without a periodic write. A no-op (no reply) if somehow not in-world or the row
        // vanished — never session-fatal for a display-only command.
        ClientOpcodeMessage::CMSG_PLAYED_TIME => {
            if let WorldState::InWorld(iw) = &conn.state {
                if let Some(c) = store.character_by_guid(iw.self_guid)? {
                    let now_micros = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_PLAYED_TIME(
                            codec::build_played_time(
                                c.played_total_secs,
                                c.session_start_micros,
                                now_micros,
                            ),
                        )),
                    )?;
                }
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
