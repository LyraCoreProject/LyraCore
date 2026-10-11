//! World Session entry keeps queue, viewer registration, and Transfer operations in order.

use super::handlers::build_buyback_view_replay;
use super::*;

pub(super) fn login(
    tx: &SessionTx,
    store: &dyn WorldStore,
    conn: &mut WorldConn,
    character: Actor,
) -> Result<()> {
    if conn.session_claim.is_some() {
        return Err(anyhow!("ACCOUNT_IN_USE"));
    }
    let character_guid = character.guid();
    let token = store.claim_session(conn.account_id, character_guid)?;
    conn.session_claim = Some(token);
    if let Some(bound) = store.bind_session(token)? {
        conn.store.pin(bound);
    }
    // Route before login and viewer registration so both use the Character's Home Shard.
    conn.route_home(character_guid)?;
    let home = conn.store.current();
    enter_world(tx, &*home, conn, character, codec::WorldEntry::FreshLogin)
}

pub(super) fn world_port_ack(
    tx: &SessionTx,
    store: &dyn WorldStore,
    conn: &mut WorldConn,
) -> Result<()> {
    let Some(character) = conn.protocol.actor() else {
        return Ok(());
    };
    let character_guid = character.guid();
    if store.entity_in_world(character_guid) {
        log::debug!("world: spurious WORLDPORT_ACK ignored (guid {character_guid} still in world)");
        return Ok(());
    }
    // Transfer cleanup must not publish source-shard changes to this viewer. Retain InWorld
    // until entry succeeds so a failed Transfer still follows the existing logout policy.
    if let WorldState::InWorld(iw) = &mut conn.state {
        iw.subs.unregister_viewer();
    }
    let mut ported = conn.route_home(character_guid);
    if ported.is_ok() {
        let home = conn.store.current();
        ported = enter_world(tx, &*home, conn, character, codec::WorldEntry::WorldPort);
    }
    if let Err(error) = ported {
        abort_pending_transfer(tx, store, character_guid, &error);
        return Err(error);
    }
    Ok(())
}

/// Queue the Transfer abort when possible, preserving the original failure.
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
        "world: world-port for guid {character_guid} cannot complete ({cause:#}); aborting the \
         client's transfer to map {dest_map:?}. The character is unharmed: the escrow is idempotent \
         and the next login re-drives it."
    );
    let Some(map_id) = dest_map else {
        log::warn!(
            "world: no durable destination for guid {character_guid}; the client gets no \
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

/// Queue self creation before registering the viewer; initial AOI Relays must follow it.
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
    entity.effective_armor = store.effective_armor(character_guid);
    entity.magic_resistances = store.effective_magic_resistances(character_guid);
    (entity.guild_id, entity.guild_rank) = handlers::guild_projection(store, character_guid);
    log::info!(
        "world: entering world guid={character_guid} -> entity at map {} ({:.1},{:.1},{:.1}); subscribing + sending login sequence + self-spawn",
        entity.map_id, entity.x, entity.y, entity.z
    );
    let items = store.player_items(character_guid).unwrap_or_default();
    let learned = store
        .player_learned_spells(character_guid)
        .unwrap_or_default();
    let skills = store.player_skills(character_guid).unwrap_or_default();
    let reputations = store.player_reputations(character_guid).unwrap_or_default();
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
    batch.push(zone_weather_message(
        store,
        entity.zone_id,
        WeatherChangeType::Instant,
    ));
    // The login sequence and self creation stay contiguous on the writer.
    send(tx, Outbound::Batch(batch))?;
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
    // Registration may queue initial AOI objects immediately.
    let subs =
        store.subscribe_player_events(conn.account_id, character_guid, &entity, tx.clone())?;
    if quests_enabled {
        // Read on the writer so reconciliation cannot overwrite newer queued quest progress.
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
    for message in build_buyback_view_replay(store, character_guid) {
        send(tx, message)?;
    }
    for message in store.pending_system_messages(character_guid) {
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                codec::build_gm_system_message(message),
            ))),
        )?;
    }
    // Record successful sign-on so cleanup owes a sign-off even if later entry work fails.
    if entry == codec::WorldEntry::FreshLogin && handlers::guild_sign_on(store, character_guid) {
        conn.guild_signed_on = Some(character_guid);
    }
    let created_projection = (entity.guild_id, entity.guild_rank);
    for message in handlers::guild_world_entry(store, character_guid, entry, created_projection) {
        send(tx, message)?;
    }
    if let Err(e) = party::on_world_entry(tx, store, character) {
        log::warn!("world: party sync at world entry failed for guid {character_guid}: {e:#}");
    }
    crate::world::guild_fee::redrive(store, character_guid);
    let charters: Vec<u64> = items
        .iter()
        .filter(|item| item.entry == lyracore_shared::guild::GUILD_CHARTER_ENTRY)
        .map(|item| item.guid)
        .collect();
    handlers::destroy_inert_charters(store, character_guid, &charters);
    crate::world::mail::redrive(store, character_guid);
    // Forget snapshots after the party frame so the next tick supplies every field.
    if let Some(record) = subs.member_stats_record().cloned() {
        let forget = move || {
            record.forget_all();
            Vec::new()
        };
        send(tx, Outbound::Job(Box::new(forget)))?;
    }
    conn.state = WorldState::InWorld(InWorld {
        self_guid: character_guid,
        subs,
        attacking_target: None,
        open_loot: OpenLootState::default(),
        ranged_repeat: false,
    });
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
    if store.talent_points_spent(character_guid) > 0 {
        let (_, _, remaining) = store.talent_pane_sync(character_guid, 0);
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_talent_points_values(character_guid, remaining),
            ))),
        )?;
    }
    for m in codec::build_spell_modifier_msgs(&store.spell_modifiers(character_guid)) {
        send(tx, Outbound::One(m))?;
    }
    Ok(())
}
