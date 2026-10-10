//! Character selection, world entry, and connection-level ping and Realm Clock replies.

use super::super::*;
use super::vendor::build_buyback_view_replay;

/// Character select, and the per-Character reads world entry builds the self CREATE from.
pub(crate) trait CharacterStore: Send + Sync {
    /// The account's characters for the character-select screen. In production this
    /// reads the per-player `game_character` subscription (RLS-restricted to the owner).
    fn characters(&self, account_id: u64) -> Result<Vec<codec::CharacterView>>;

    /// Create a character for the account (`CMSG_CHAR_CREATE`). Returns the game outcome
    /// (success / name-in-use / failed); `Err` only for a Transport Loss.
    fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: codec::Appearance,
    ) -> Result<codec::CharCreateOutcome>;

    /// Delete `character` for the account (`CMSG_CHAR_DELETE`). Returns the game outcome
    /// (success/failed); `Err` only for a Transport Loss. Ownership is enforced module-side (the
    /// character must belong to `account_id`).
    fn delete_character(
        &self,
        account_id: u64,
        character: Actor,
    ) -> Result<codec::CharDeleteOutcome>;

    /// Look up a character by guid (any owner) to answer `CMSG_NAME_QUERY` — the queried guid is
    /// usually a peer, so this is not account-scoped.
    fn character_by_guid(&self, guid: u64) -> Result<Option<codec::CharacterView>>;

    /// Does any World Shard hold this Character? `false` means every configured Shard was readable
    /// and had no row. An incomplete, unhealthy, or changing Shard set must return `Err`.
    fn character_exists_on_any_world_shard(&self, guid: u64) -> Result<bool>;

    /// The character's learned skill lines as `(skill_line, current, max_rank)` — feeds the self
    /// CREATE's SkillInfo block. Empty when no `game_player_skill` rows exist.
    fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>>;

    /// The EFFECTIVE armor for `guid` (base + worn gear armor) for the self-login CREATE's
    /// `UNIT_FIELD_RESISTANCES[0]` — so the character sheet shows real worn armor on relog. Auras aren't
    /// folded here (they self-correct via the on_aura relay). Mirrors the module's combat `effective_armor`.
    fn effective_armor(&self, guid: u64) -> u32;

    fn effective_magic_resistances(&self, guid: u64) -> [u32; 6];

    /// The character's active spell-modifier auras as raw (family_mask, op, amount, is_pct) rows —
    /// the SMSG_SET_FLAT/PCT_SPELL_MODIFIER mirror source.
    fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)>;

    /// The player's LEARNED spells (`game_player_spell`, beyond the class kit) — chained into the
    /// login SMSG_INITIAL_SPELLS so a taught ability (e.g. Auto Shot) reaches the client spellbook.
    fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>>;

    /// The player's persisted reputation standings (`game_player_reputation`) as `(reputation_index,
    /// standing)` pairs — folded into the login `SMSG_INITIALIZE_FACTIONS` so a relog shows
    /// the real standing instead of the all-neutral stub.
    fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>>;

    /// The player's IMPORTED action-bar rows (`game_player_action`) as `(button,
    /// action, action_type)` triples — empty pre-import (the common case today), in which case the
    /// login codec falls back to synthesizing the bar from the spellbook (byte-identical to before
    /// this method existed).
    fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>>;
}

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
fn abort_pending_transfer<St: TransferStore + ?Sized>(
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

/// Rebuild the Character's entity and subscriptions, then send its entry batch.
/// The bound Store retains Account ownership across a map change. Entry after a map change omits
/// `SMSG_LOGIN_VERIFY_WORLD`, which would tell the client to load the map again.
fn enter_world(
    tx: &SessionTx,
    store: &dyn WorldStore,
    conn: &mut WorldConn,
    character: Actor,
    entry: codec::WorldEntry,
) -> Result<()> {
    conn.state = WorldState::CharSelect;

    let character_guid = character.guid();
    let mut entity = store.player_login(conn.account_id, character, entry)?;
    // Character sheet (UNIT_FIELD_RESISTANCES[0]): override the BASE armor `player_login` set with
    // the EFFECTIVE armor (base + worn gear) so the Armor readout is correct at relog. Armor auras
    // self-correct via the on_aura relay; combat mitigation is unchanged (the module still folds its
    // own effective_armor on demand — this only feeds the display descriptor).
    entity.effective_armor = store.effective_armor(character_guid);
    entity.magic_resistances = store.effective_magic_resistances(character_guid);
    (entity.guild_id, entity.guild_rank) = super::guild_projection(store, character_guid);
    log::info!(
        "world: entering world guid={character_guid} -> entity at map {} ({:.1},{:.1},{:.1}); subscribing + sending login sequence + self-spawn",
        entity.map_id, entity.x, entity.y, entity.z
    );
    // Items: the character's owned items. Each becomes an item CREATE_OBJECT sent
    // BEFORE the player self-spawn (so the inventory-slot guid resolves to an object the
    // client already has), and the (slot, guid) pairs seed the player's PLAYER_FIELD_INV_SLOT
    // descriptors. Empty for a character that owns nothing — login is otherwise unchanged.
    let items = store.player_items(character_guid).unwrap_or_default();
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
    let quests_enabled = crate::config::quest_log_fields_enabled();
    let quests = if quests_enabled {
        store.player_quest_log(character_guid)?
    } else {
        Vec::new()
    };
    batch.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
        codec::build_self_create_object(&entity, &items, &skills, &quests)?,
    )));
    // The environment, closing the same contiguous batch: the client must land with the right sky
    // rather than the last one it rendered, so this is Instant and is sent on EVERY world entry —
    // fresh login, reconnect and cross-map world-port alike. The zone is the Module's answer,
    // resolved from terrain when it built the live entity. A zone with no weather is fine weather
    // and still sends its packet: an arriving client that is told nothing keeps whatever it had.
    batch.push(zone_weather_message(
        store,
        entity.zone_id,
        WeatherChangeType::Instant,
    ));
    send(tx, Outbound::Batch(batch))?;
    // A typed batch cannot hold a raw update, so Random Property enchant ids follow the batch.
    for (opcode, body) in items.iter().filter_map(codec::build_random_property_values) {
        send(tx, Outbound::Raw { opcode, body })?;
    }
    for item in &items {
        if let Some((bag_slot, position)) = codec::bag_content_parts(item.slot) {
            if let Some(bag) = items.iter().find(|bag| bag.slot == bag_slot) {
                let (opcode, body) =
                    codec::build_container_slot_values(bag.guid, position, item.guid);
                send(tx, Outbound::Raw { opcode, body })?;
            }
        }
    }
    // Subscribe AFTER the self-spawn batch is on the wire — so the AOI initial-apply creates for
    // entities ALREADY in view (notably a questgiver you spawn right next to) arrive AFTER the
    // client is in-world. Spawning ON a questgiver otherwise left it targetable but with no '!' /
    // right-click: its CREATE raced the login sequence and the client registered it as a plain
    // unit, never polling its quest status. The streaming path (a peer entering view later) was
    // always fine — this makes the login case match it. (Missing a peer that
    // inserts in the µs window between this send and the subscribe is negligible.) The dedup set
    // is seeded with self_guid so the player's own row (delivered on initial apply) is skipped.
    let subs =
        store.subscribe_player_events(conn.account_id, character_guid, &entity, tx.clone())?;
    if quests_enabled {
        // A quest can change before viewer registration. Read again on the writer, where live
        // quest Relays also read, so reconciliation cannot overwrite newer queued progress.
        let store = conn.store.current();
        let close = tx.clone();
        send(
            tx,
            Outbound::Job(Box::new(move || {
                match store.player_quest_log(character_guid) {
                    Ok(current) if current != quests => {
                        let mask = codec::update_mask::full_quest_log_mask(&current);
                        let (opcode, body) = codec::build_values_update_raw(character_guid, &mask);
                        vec![Outbound::Raw { opcode, body }]
                    }
                    Ok(_) => Vec::new(),
                    Err(error) => {
                        log::error!("world: quest log reconciliation failed for guid {character_guid}: {error:#}");
                        close.close();
                        Vec::new()
                    }
                }
            })),
        )?;
    }
    // Replay the buyback-tab view (the ring persists across sessions; without this the tab
    // is empty until the first in-session sell).
    for message in build_buyback_view_replay(store, character_guid) {
        send(tx, message)?;
    }
    // Replay private System Messages emitted inside `player_login` (a Package `on_login` hook):
    // their insert relayed before this session was registered, so nobody was addressable and the
    // rows are still parked in the shard cache. (A message landing in the µs window between the
    // registration above and this replay can arrive twice; a duplicate line beats a swallowed one.)
    for message in store.pending_system_messages(character_guid) {
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                codec::build_gm_system_message(message),
            ))),
        )?;
    }
    // The guild step needs the registered viewer: its SIGNED_ON reaches the other members, and
    // its Guild Projection re-send covers a membership change that landed after the CREATE read.
    // A sign-on is owed a sign-off from here on, even if this entry or a later world-port fails.
    if entry == codec::WorldEntry::FreshLogin && super::guild_sign_on(store, character_guid) {
        conn.guild_signed_on = Some(character_guid);
    }
    let created_projection = (entity.guild_id, entity.guild_rank);
    for message in super::guild_world_entry(store, character_guid, entry, created_projection) {
        send(tx, message)?;
    }
    // Put realm-core's party roster onto the shard this character just entered
    // and re-render the party frame. THIS is what carries a party across a shard boundary now that
    // the old interim blob mirror is gone — and it runs on every world entry, so a party formed while the
    // player was on the loading screen lands too. A no-op on a single-database gateway.
    //
    // Failures are logged, not propagated: a party frame that renders late is a cosmetic defect,
    // and failing the login over it would be strictly worse for the player.
    if let Err(e) = party::on_world_entry(tx, store, character) {
        log::warn!("world: party sync at world entry failed for guid {character_guid}: {e:#}");
    }
    // A Fee Hold that an earlier session or a Transfer left behind is finished here, on the Home
    // Shard that holds the Character now. The purse change reaches the client through its entity.
    crate::world::guild_fee::redrive(store, character_guid);
    // A Guild Charter left in the bags after its Petition closed is destroyed once the Fee Hold is
    // finished, so it no longer blocks the next Charter purchase.
    let charters: Vec<u64> = items
        .iter()
        .filter(|item| item.entry == lyracore_shared::guild::GUILD_CHARTER_ENTRY)
        .map(|item| item.guid)
        .collect();
    super::destroy_inert_charters(store, character_guid, &charters);
    // A Reward Letter or a send that an earlier session left as Escrow on this Home Shard is
    // delivered here, so a Gateway restart after a turn-in loses no letter.
    crate::world::mail::redrive(store, character_guid);
    // A Member Stats tick can run between the registration above and that party frame, for a party
    // the client does not know yet. Forget it behind the frame so the next tick sends every field.
    if let Some(record) = subs.member_stats_record().cloned() {
        let forget = move || {
            record.forget_all();
            Vec::new()
        };
        send(tx, Outbound::Job(Box::new(forget)))?;
    }
    // Enter the world: CharSelect → InWorld (a reused connection has no open loot/attack — a world-port
    // re-entry likewise starts clean, since whatever the player was attacking/looting on the old map is
    // meaningless on the new one).
    conn.state = WorldState::InWorld(InWorld {
        self_guid: character_guid,
        subs,
        attacking_target: None,
        open_loot: OpenLootState::default(),
        ranged_repeat: false,
    });
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
pub(crate) fn handle_char<
    St: CharacterStore + GuildActionStore + SessionStore + TransferStore + ?Sized,
>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    match msg {
        ClientOpcodeMessage::CMSG_PING(ping) => {
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_PONG(
                    wow_world_messages::vanilla::SMSG_PONG {
                        sequence_id: ping.sequence_id,
                    },
                )),
            )?;
        }
        ClientOpcodeMessage::CMSG_QUERY_TIME => {
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_QUERY_TIME_RESPONSE(
                    wow_world_messages::vanilla::SMSG_QUERY_TIME_RESPONSE {
                        time: u32::try_from(seconds)?,
                    },
                )),
            )?;
        }
        // Character-select screen.
        ClientOpcodeMessage::CMSG_CHAR_ENUM => {
            let characters = store.characters(conn.account_id)?;
            let enum_msg = codec::build_char_enum(&characters)?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_ENUM(Box::new(enum_msg))),
            )?;
        }
        // Character creation. Create the row, reply SMSG_CHAR_CREATE; on success the client
        // re-sends CMSG_CHAR_ENUM and the new character appears. A Refusal is NOT session-fatal —
        // report it as a result. A Transport Loss ends the session.
        ClientOpcodeMessage::CMSG_CHAR_CREATE(c) => {
            let appearance = codec::Appearance {
                skin: c.skin_color,
                face: c.face,
                hair_style: c.hair_style,
                hair_color: c.hair_color,
                facial_hair: c.facial_hair,
            };
            let outcome = store.create_character(
                conn.account_id,
                c.name.as_str(),
                c.race.as_int(),
                c.class.as_int(),
                c.gender.as_int(),
                appearance,
            )?;
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_CREATE(
                    codec::build_char_create_response(outcome),
                )),
            )?;
        }
        // Character deletion. Per the wire doc SMSG_CHAR_DELETE alone updates the
        // character-select screen — no re-sent CMSG_CHAR_ENUM needed. Ownership is enforced module-
        // side; a Refusal is NOT session-fatal, same treatment as CMSG_CHAR_CREATE above. Guid 0
        // names no Character and fails like a Refusal.
        //
        // A Guild Leader is not deleted: 1.12 answers CHAR_DELETE_FAILED, the same 0x3A mangos sends
        // as FAILED_GUILD_LEADER (`cm:CharacterHandler.cpp:540-546`). Realm-core holds the Guild and
        // the Home Shard cannot read it, so the Gateway asks first. An unreadable answer deletes
        // nothing either.
        ClientOpcodeMessage::CMSG_CHAR_DELETE(d) => {
            let character_guid = d.guid.guid();
            let outcome = match (
                Actor::new(character_guid),
                super::leads_a_guild(store, character_guid),
            ) {
                (None, _) => codec::CharDeleteOutcome::Failed,
                (Some(character), Ok(false)) => {
                    store.delete_character(conn.account_id, character)?
                }
                (Some(_), Ok(true)) => {
                    log::info!("world: Guild Leader {character_guid} is not deleted");
                    codec::CharDeleteOutcome::Failed
                }
                (Some(_), Err(error)) => {
                    log::warn!(
                        "world: Guild Leader check for {character_guid} failed, not deleted: \
                         {error:#}"
                    );
                    codec::CharDeleteOutcome::Failed
                }
            };
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_CHAR_DELETE(
                    codec::build_char_delete_response(outcome),
                )),
            )?;
        }
        // Enter world -> register peer subscriptions, then login sequence + self
        // CREATE_OBJECT2 as one contiguous batch (so an async peer event can't splice into it).
        ClientOpcodeMessage::CMSG_PLAYER_LOGIN(p) => {
            let character_guid = p.guid.guid();
            if conn.session_claim.is_some() {
                return Err(anyhow::anyhow!("ACCOUNT_IN_USE"));
            }
            let Some(character) = Actor::new(character_guid) else {
                return Err(anyhow::anyhow!("CMSG_PLAYER_LOGIN names no Character"));
            };
            let token = store.claim_session(conn.account_id, character_guid)?;
            conn.session_claim = Some(token);
            if let Some(bound) = store.bind_session(token)? {
                conn.store.pin(bound);
            }
            // Multi-shard routing: pin this session to the shard that owns the character's
            // location BEFORE `player_login` runs, so the login reducer and viewer registration
            // land on the home shard — and so does every message after this one
            // (`run_world_session` reads `conn.store` per frame).
            // A single-entry shard map never pins anything → `enter_world` runs on `store`.
            conn.route_home(character_guid)?;
            let home = conn.store.current();
            enter_world(tx, &*home, conn, character, codec::WorldEntry::FreshLogin)?;
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
        // client ack in this dispatch. The bound Store retains the same Account ownership.
        ClientOpcodeMessage::MSG_MOVE_WORLDPORT_ACK => {
            let resume = match &conn.state {
                WorldState::InWorld(iw) => Actor::new(iw.self_guid),
                WorldState::CharSelect => None,
            };
            if let Some(character) = resume {
                let character_guid = character.guid();
                // Gate on a REAL pending transfer: cross-map teleport
                // despawns the entity until this ack; a live entity means no transfer is in
                // flight and the ack is spurious — ignore it instead of re-entering the world.
                // `store` is ALREADY the home-shard handle — the read loop routes every frame
                // through `conn.store` — so this reads the cache the entity actually lives in.
                if store.entity_in_world(character_guid) {
                    log::debug!("world: spurious WORLDPORT_ACK ignored (guid {character_guid} still in world)");
                } else {
                    // Stop source-shard owner dispatch before `route_home` can drive a transfer.
                    // `finish_transfer` cascade-deletes quest and item rows, and their shared
                    // callbacks must find no source viewer to enqueue for. A source delta the pump
                    // applies only after destination registration is dropped by the shard-scoped
                    // owner lookup instead. Keep `InWorld` intact:
                    // if routing fails, the existing abort path terminates the socket and
                    // `leave_world` retains its logout/error policy. The guard's later Drop is
                    // idempotent, and destination entry retains the same Character and Account ownership.
                    if let WorldState::InWorld(iw) = &mut conn.state {
                        iw.subs.unregister_viewer();
                    }
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

                    let mut ported = conn.route_home(character_guid);
                    if ported.is_ok() {
                        let home = conn.store.current();
                        ported =
                            enter_world(tx, &*home, conn, character, codec::WorldEntry::WorldPort);
                    }
                    if let Err(e) = ported {
                        abort_pending_transfer(tx, store, character_guid, &e);
                        return Err(e);
                    }
                }
            }
        }
        // Graceful in-game Logout/Exit. Deny if in combat (vanilla behaviour); otherwise
        // ack instantly + complete, remove the entity (observers see DESTROY), drop the peer
        // subscriptions, and return to character-select with the connection still open.
        ClientOpcodeMessage::CMSG_LOGOUT_CANCEL => {
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_LOGOUT_CANCEL_ACK),
            )?;
        }
        ClientOpcodeMessage::CMSG_LOGOUT_REQUEST | ClientOpcodeMessage::CMSG_PLAYER_LOGOUT => {
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
            conn.leave_world()?;
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
