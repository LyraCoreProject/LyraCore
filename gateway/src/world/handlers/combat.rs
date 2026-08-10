//! Combat family (N3/C1 + aura tracer): selection, melee swing/stop, spell cast. Pure
//! code-motion out of `world/mod.rs`.

use super::super::*;

/// The `(display_id, inv_type)` ammo block for the auto-shot activation `SMSG_SPELL_START`:
/// `Some` only when the ranged slot (17) holds a LAUNCHER (weapon subclass 2/3/18 — bow/gun/crossbow)
/// and a class-6 Projectile stack is in the bags; a wand fires its own bolt (no ammo block), mirroring
/// the swing tick's per-shot rule + the GO relay. Deliberate simplification: inv-type 24
/// (INVTYPE_AMMO) is hardcoded like the GO relay; no-ammo launchers get None (the first shot tick
/// then tears the loop down → cancel).
fn ranged_ammo_display<St: WorldStore + ?Sized>(store: &St, self_guid: u64) -> Option<(u32, u32)> {
    let items = store.player_items(self_guid).ok()?;
    let launcher = items
        .iter()
        .find(|i| i.slot == 17)
        .and_then(|i| store.item_template(i.entry).ok().flatten())?;
    if launcher.class != 2 || !matches!(launcher.subclass, 2 | 3 | 18) {
        return None;
    }
    // min_by_key(slot) + stack_count > 0 mirrors the module's per-shot `find_ammo` pick, so the
    // nocked projectile on the START matches the one each SPELL_GO fires (review find — the client
    // cache iterates unsorted, and a dead stack must not be nocked).
    items
        .iter()
        .filter(|i| i.stack_count > 0)
        .filter(|i| {
            store
                .item_template(i.entry)
                .ok()
                .flatten()
                .is_some_and(|t| t.class == 6)
        })
        .min_by_key(|i| i.slot)
        .and_then(|i| store.item_template(i.entry).ok().flatten())
        .map(|t| (t.display_id, 24))
}

/// Combat family (N3/C1 + aura tracer): selection, melee swing/stop, spell cast. ⚠️ Holds the two
/// session-fatal `is_desync_error` early-exits on CMSG_ATTACKSWING/CMSG_ATTACKSTOP — preserved
/// verbatim (a desync = the player's own entity is gone → tear the session down for a clean relog,
/// unlike the transient per-swing failures that stay logged + alive).
pub(crate) fn handle_combat<St: WorldStore + ?Sized>(
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
        // Targeting (N3): record the player's selection server-side (foundation for combat).
        ClientOpcodeMessage::CMSG_SET_SELECTION(s) => {
            store.set_target(conn.account_id, self_guid, s.target.guid())?
        }
        // Pet command bar (CMSG_PET_ACTION): pass the raw packed `data` + target through; the module
        // decodes stay/follow/attack/dismiss + passive/defensive/aggressive and validates ownership. A
        // transient reject (no pet, dead/invalid target) must NOT drop the session — log + ignore, like
        // the start_attack path (do NOT route through is_desync_error).
        ClientOpcodeMessage::CMSG_PET_ACTION(p) => {
            if let Err(e) = store.pet_command(conn.account_id, self_guid, p.data, p.target.guid()) {
                log::debug!(
                    "world: pet_command ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // The client's ack to our `SMSG_FORCE_RUN_SPEED_CHANGE` (`.speed`). We don't
        // gate on the reply (the movement counter/new_speed aren't cross-checked) — explicitly
        // consumed here (rather than falling through to the dispatch tail's `log::debug!` "ignoring"
        // line) so a `.speed` never spams the log or risks a future desync-classifier false-positive.
        ClientOpcodeMessage::CMSG_FORCE_RUN_SPEED_CHANGE_ACK(_) => {}
        // Combat (C1): begin melee auto-attack. Arm the server-side engagement (the swing tick
        // applies damage), and ack with SMSG_ATTACKSTART so the client enters combat stance and
        // plays the swing animation. The per-swing damage text comes from the relayed
        // SMSG_ATTACKERSTATEUPDATE; the health bar from the on_update VALUES relay.
        ClientOpcodeMessage::CMSG_ATTACKSWING(s) => {
            let target_guid = s.guid.guid();
            let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
            log::info!("world[autoshot]: CMSG_ATTACKSWING target={target_guid} ranged_repeat_active={was_repeat} (account {})", conn.account_id);
            // A failed start_attack (target already dead/despawned, out of range, not in world) is
            // an EXPECTED transient condition, not session-fatal — e.g. the client swings at the
            // Chicken on the same frame it dies. Log + ignore so the player isn't disconnected; only
            // arm + ack the stance when the engagement actually started.
            match store.start_attack(conn.account_id, self_guid, target_guid) {
                Ok(()) => {
                    if let WorldState::InWorld(iw) = &mut conn.state {
                        iw.attacking_target = Some(target_guid);
                        // Switching to melee overwrites the shared game_melee_attack row to a melee
                        // engagement, so a later CMSG_ATTACKSTOP should now be honored.
                        iw.ranged_repeat = false;
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(Box::new(
                                codec::build_attack_start(iw.self_guid, target_guid),
                            ))),
                        )?;
                    }
                }
                Err(e) => {
                    // A swing at a corpse → reply SMSG_ATTACKSWING_DEADTARGET so the client leaves
                    // combat stance and shows "can't attack — target is dead" (it otherwise hangs in
                    // stance with no swings, since the server refuses to arm the engagement). A friendly
                    // target → SMSG_ATTACKSWING_CANT_ATTACK. These are TRANSIENT per-swing failures.
                    if e.to_string()
                        .contains(lyracore_shared::ERR_ATTACK_TARGET_DEAD)
                    {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSWING_DEADTARGET),
                        )?;
                    } else if e.to_string().contains(lyracore_shared::ERR_ATTACK_FRIENDLY) {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSWING_CANT_ATTACK),
                        )?;
                    } else if is_desync_error(&e) {
                        // The player's OWN entity is gone — a desync, NOT a transient swing failure (the
                        // gateway's view went stale, e.g. a schema migration dropped its subscription). No
                        // action can be served, so the player would otherwise hang in combat stance with
                        // no recovery. Propagate as session-fatal → clean socket teardown → the client
                        // shows "Disconnected" and relog re-materializes from durable state.
                        return Err(e.context(
                            "player desync (entity missing) on attackswing — closing session for a clean relog",
                        ));
                    }
                    // Other failures (out of range, retarget races) are transient → log + ignore.
                    log::debug!(
                        "world: start_attack ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        // Combat (C1): stop auto-attacking; leave combat stance. Best-effort — a stop_attack
        // failure must not drop the session, and the client is always told to leave stance.
        ClientOpcodeMessage::CMSG_ATTACKSTOP => {
            // While a ranged auto-repeat is armed, the client sends CMSG_ATTACKSTOP as part of
            // switching out of melee stance — but melee + ranged share one game_melee_attack row,
            // so honoring it would delete the auto-shot engagement (one-shot-then-stops). The
            // ranged loop is torn down only by CMSG_CANCEL_AUTO_REPEAT_SPELL; ignore the melee stop.
            let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
            log::info!(
                "world[autoshot]: CMSG_ATTACKSTOP ranged_repeat_active={was_repeat} (account {})",
                conn.account_id
            );
            if was_repeat {
                return Ok(None);
            }
            if let Err(e) = store.stop_attack(conn.account_id, self_guid) {
                // A desync (entity gone) is session-fatal — recover via a clean disconnect, not a
                // silent hang. A transient stop_attack failure stays logged + ignored.
                if is_desync_error(&e) {
                    return Err(e.context(
                        "player desync (entity missing) on attackstop — closing session for a clean relog",
                    ));
                }
                log::debug!(
                    "world: stop_attack ignored (account {}): {e}",
                    conn.account_id
                );
            }
            // `attacking_target` may name a creature the server already killed (the kill sends its
            // own SMSG_ATTACKSTOP and can't reach this thread to clear it); re-sending stop for a
            // now-dead guid is harmless (the client no longer has that unit).
            if let WorldState::InWorld(iw) = &mut conn.state {
                if let Some(target_guid) = iw.attacking_target.take() {
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(Box::new(
                            codec::build_attack_stop(iw.self_guid, target_guid),
                        ))),
                    )?;
                }
            }
        }
        // Aura+spell tracer: cast a spell. On success the module applies the aura + emits the cast
        // event (relayed as SMSG_CAST_RESULT(OK)+SMSG_SPELL_GO + the buff-icon VALUES). On
        // rejection (unknown spell / not in world), reply SMSG_CAST_RESULT::Failure — a SILENT
        // cast-bar reset (NOT a red error; `Success { reason }` is the red-error variant). Self-cast:
        // ignore c.targets. Today CMSG_CAST_SPELL falls into `other =>` and is ignored, so this only
        // enriches safe behavior.
        ClientOpcodeMessage::CMSG_CAST_SPELL(c) => {
            // Ranged auto-attack: Auto Shot + wand Shoot are AUTO-REPEAT ranged attacks (the
            // RANGED_AUTO_REPEAT cast_flags bit — set by the importer from the DBC AttributesEx2 AUTOREPEAT
            // bit, NOT a hardcoded id list), not one-shot casts. Intercept them BEFORE the
            // normal cast path: clear the client cast state (SPELL_START→SPELL_GO, else the action button
            // locks with "Another action is in progress"), then arm the server-side ranged swing loop on
            // the cast's unit target. The loop fires on the ranged-weapon timer until
            // CMSG_CANCEL_AUTO_REPEAT_SPELL / CMSG_ATTACKSTOP.
            if store.spell_is_ranged_auto_repeat(c.spell) {
                // The shot's target rides the cast's SpellCastTargets (UNIT flag). Deliberate
                // simplification: no current-selection fallback — Auto Shot/Shoot are cast ON a
                // target, so the client always includes it.
                let target = c
                    .targets
                    .target_flags
                    .get_unit()
                    .map(|u| u.unit_target.guid())
                    .unwrap_or(0);
                let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
                log::info!(
                    "world[autoshot]: AUTO-REPEAT activate spell={} target={} already_repeating={} (account {})",
                    c.spell, target, was_repeat, conn.account_id
                );
                // Arm the server loop FIRST; a rejected activation answers ONLY the raw
                // SMSG_CAST_RESULT(reason) — the 5875 client drops its auto-repeat toggle on a failure
                // result, keeping the client/server toggle in lockstep (vanilla likewise rejects a
                // failed castability check BEFORE sending SPELL_START). The old shape (START first, then a
                // bare typed Failure on rejection) left the client toggled ON over a dead server loop:
                // the NEXT press then sent CMSG_CANCEL_AUTO_REPEAT_SPELL instead of a cast — the
                // "pressing Auto Shot does nothing until I move" bug.
                match store.start_ranged_attack(conn.account_id, self_guid, target, c.spell) {
                    Err(e) => {
                        log::info!("world[autoshot]: start_ranged_attack REJECTED spell={} target={} (account {}): {e}", c.spell, target, conn.account_id);
                        let reason = codec::cast_failure_reason_for(&e.to_string());
                        send(
                            tx,
                            Outbound::Raw {
                                opcode: 0x0130,
                                body: codec::build_cast_result_failed(c.spell, reason),
                            },
                        )?;
                        // A rejected RE-activation (retarget at an invalid new target) drops the
                        // client's toggle on the failure result — tear down the still-firing OLD
                        // loop too, or the server keeps shooting a target the client thinks it
                        // stopped (review find). Fresh activations (was_repeat false) skip the no-op.
                        if was_repeat {
                            if let WorldState::InWorld(iw) = &mut conn.state {
                                iw.ranged_repeat = false;
                            }
                            if let Err(e) = store.stop_attack(conn.account_id, self_guid) {
                                log::debug!(
                                    "world: reject-teardown stop_attack ignored (account {}): {e}",
                                    conn.account_id
                                );
                            }
                        }
                    }
                    Ok(()) => {
                        if let WorldState::InWorld(iw) = &mut conn.state {
                            iw.ranged_repeat = true;
                            // Activation ack = SMSG_SPELL_START alone: timer 0 (the 0.5s
                            // wind-up is an ATTACK-TIMER, not a cast bar — vmangos GetCastTime skips the
                            // ranged +500ms for auto-repeat; the client animates its own wind-up),
                            // CAST_FLAG_AMMO + ammo block (nocks the arrow — the between-shots aim pose
                            // rides the client's local auto-repeat state), and the real unit target.
                            // No CAST_RESULT(OK) and no GO: the activation cast is parked in the
                            // client's AUTOREPEAT slot and never resolves; each shot's GO comes from
                            // the swing-tick combat-event relay (subscriptions.rs).
                            let ammo = ranged_ammo_display(store, iw.self_guid);
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                                    codec::build_spell_start(
                                        iw.self_guid,
                                        c.spell,
                                        0,
                                        target,
                                        ammo,
                                    ),
                                ))),
                            )?;
                        }
                    }
                }
            } else if let Some(route) = store.enchant_route(c.spell) {
                // Enchant/disenchant spells target an item instance by GUID (routed here by EFFECT KIND,
                // not a spell-id list — a new enchant is a data row). Resolve the GUID → bag slot, then
                // dispatch to the module reducer (disenchant or enchant_item_on_slot, with enchant_id
                // carried in the effect data). These reducers don't emit game_spell_cast_event, so we
                // send START+GO manually to clear the client cast bar.
                use wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags_Item as ItTgt;
                let item_guid = match c.targets.target_flags.get_item() {
                    Some(ItTgt::Item { item }) => item.guid(),
                    _ => 0,
                };
                let result = if item_guid != 0 {
                    match store.item_slot_by_guid(conn.account_id, item_guid) {
                        Some(slot) => match route {
                            EnchantRoute::Disenchant => {
                                store.disenchant_item(conn.account_id, self_guid, slot)
                            }
                            EnchantRoute::Enchant(enchant_id) => {
                                store.enchant_item_on_slot(conn.account_id, self_guid, slot, enchant_id)
                            }
                        },
                        None => Err(anyhow!("enchant: item {item_guid} not in player bag")),
                    }
                } else {
                    Err(anyhow!("enchant: no item target in cast"))
                };
                if let Err(e) = result {
                    log::debug!(
                        "world: enchanting failed (account {}): {e}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_CAST_RESULT(Box::new(
                            SMSG_CAST_RESULT {
                                spell: c.spell,
                                result: SMSG_CAST_RESULT_SimpleSpellCastResult::Failure,
                            },
                        ))),
                    )?;
                } else if let WorldState::InWorld(iw) = &conn.state {
                    let caster = iw.self_guid;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                            codec::build_spell_start(caster, c.spell, 0, 0, None),
                        ))),
                    )?;
                    send(
                        tx,
                        Outbound::Raw {
                            opcode: 0x0130,
                            body: codec::build_cast_result_ok(c.spell),
                        },
                    )?;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
                            codec::build_spell_go(caster, c.spell, 0, None),
                        ))),
                    )?;
                }
            } else if store.spell_is_fishing(c.spell) {
                // Fishing: instant-resolve — route to the `fish` reducer (lenient alpha gate:
                // auto-learns the skill, grants the catch straight to the bag; the bobber/channel
                // flow is the deferred follow-up). Same manual START→OK→GO clear as the enchant
                // path (the fish reducer emits no game_spell_cast_event). Kind-routed via the
                // synthesized E_FISH effect row — a new fishing tier is a data row.
                match store.fish(conn.account_id, social::self_guid(conn).unwrap_or(0)) {
                    Err(e) => {
                        log::debug!("world: fish failed (account {}): {e}", conn.account_id);
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_CAST_RESULT(Box::new(
                                SMSG_CAST_RESULT {
                                    spell: c.spell,
                                    result: SMSG_CAST_RESULT_SimpleSpellCastResult::Failure,
                                },
                            ))),
                        )?;
                    }
                    Ok(()) => {
                        if let WorldState::InWorld(iw) = &conn.state {
                            let caster = iw.self_guid;
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                                    codec::build_spell_start(caster, c.spell, 0, 0, None),
                                ))),
                            )?;
                            send(
                                tx,
                                Outbound::Raw {
                                    opcode: 0x0130,
                                    body: codec::build_cast_result_ok(c.spell),
                                },
                            )?;
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
                                    codec::build_spell_go(caster, c.spell, 0, None),
                                ))),
                            )?;
                        }
                    }
                }
            } else if store.spell_is_open_lock(c.spell) {
                // Pick Lock: the cast targets a locked GameObject by GUID (routed here by EFFECT
                // KIND, not a spell-id list — a new open-lock spell is a data row). Decode the GO guid off
                // the cast's SpellCastTargets (GAMEOBJECT flag), call the `pick_lock` reducer, then send
                // START+OK+GO manually to clear the client cast bar (the reducer emits no cast event) —
                // the identical handshake as the enchant/fish arms.
                use wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags_Gameobject as GoTgt;
                let go_guid = match c.targets.target_flags.get_gameobject() {
                    Some(GoTgt::Gameobject { gameobject }) => gameobject.guid(),
                    Some(GoTgt::ObjectUnk { object_unk }) => object_unk.guid(),
                    None => 0,
                };
                let result = if go_guid != 0 {
                    store.pick_lock(conn.account_id, self_guid, go_guid)
                } else {
                    Err(anyhow!("pick_lock: no gameobject target in cast"))
                };
                if let Err(e) = result {
                    log::debug!("world: pick_lock failed (account {}): {e}", conn.account_id);
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_CAST_RESULT(Box::new(
                            SMSG_CAST_RESULT {
                                spell: c.spell,
                                result: SMSG_CAST_RESULT_SimpleSpellCastResult::Failure,
                            },
                        ))),
                    )?;
                } else if let WorldState::InWorld(iw) = &conn.state {
                    let caster = iw.self_guid;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                            codec::build_spell_start(caster, c.spell, 0, 0, None),
                        ))),
                    )?;
                    send(
                        tx,
                        Outbound::Raw {
                            opcode: 0x0130,
                            body: codec::build_cast_result_ok(c.spell),
                        },
                    )?;
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
                            codec::build_spell_go(caster, c.spell, 0, None),
                        ))),
                    )?;
                }
            } else {
                // Thread the client's selected unit target (mirrors the ranged path above) so target-keyed
                // effects — combo finishers, enemy spells — see the real target, not the caster. No unit
                // target (self-buffs) → 0 → the module substitutes the caster.
                let target = c
                    .targets
                    .target_flags
                    .get_unit()
                    .map(|u| u.unit_target.guid())
                    .unwrap_or(0);
                // ROOT-CAUSE FIX: for an INSTANT cast, send the caster's START+GO SYNCHRONOUSLY here
                // (as Auto-Shot/Enchant already do), BEFORE dispatching cast_spell. The async relay would
                // deliver START/GO via the game_spell_cast_event subscription callback, which the SpacetimeDB
                // SDK fires AFTER game_aura's (fixed alphabetical callback order, bindings/mod.rs) — so the
                // applied buff reaches the client before START/GO and wedges its cast slot ("Another action in
                // progress" until Esc). Sending them inline guarantees START -> GO -> effects. Timed casts keep
                // the relay path (begin_cast sends START(cast_time); the completion sends GO). The relay
                // suppresses its now-duplicate START/GO to the caster for this instant cast. Unknown spell ->
                // treat as instant (safe: a stray START/GO is harmless; a missing one wedges).
                let instant = store
                    .spell_cast_time(c.spell)
                    .map(|t| t == 0)
                    .unwrap_or(true);
                // An on-next-swing spell (Heroic Strike/Cleave) sends NOTHING here — the client
                // lights the button locally on the press and holds the pending cast; the module's
                // swing-fire emits the CAST_RESULT(OK)+GO (is_completion row) that releases it. The
                // sync START(0)+GO below is exactly what un-lit the button and "resolved" the cast
                // at queue time (the button-lock bug).
                let queues_swing = instant && store.spell_queues_next_swing(c.spell);
                if instant && !queues_swing {
                    if let WorldState::InWorld(iw) = &conn.state {
                        let caster = iw.self_guid;
                        // vmangos sequence: START(0) → CAST_RESULT(OK) → GO.
                        // SMSG_CAST_RESULT(spell_id, 0x00) is the missing server ACK; the 5875 client
                        // requires it before GO to transition m_currentSpells to a clearable state.
                        // Raw 5-byte body (gtker's Success struct would add an erroneous reason byte).
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                                codec::build_spell_start(caster, c.spell, 0, 0, None),
                            ))),
                        )?;
                        send(
                            tx,
                            Outbound::Raw {
                                opcode: 0x0130,
                                body: codec::build_cast_result_ok(c.spell),
                            },
                        )?;
                        // A GROUND-AREA spell (Consecration) hits the ground, not a unit — an
                        // EMPTY hit list (the self-cast fallback put the CASTER in hits[] and the
                        // client played the impact animation ON the paladin).
                        let go = if store.spell_is_ground_area(c.spell) {
                            codec::build_spell_go_area(caster, c.spell)
                        } else {
                            codec::build_spell_go(caster, c.spell, target, None)
                        };
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(go))),
                        )?;
                    }
                }
                // A GROUND-TARGETED cast rides a DEST_LOCATION target block (the player
                // clicked the ground — Flamestrike/Blizzard/Rain of Fire). Route it to cast_spell_at with
                // the click coords so the module anchors the AoE/patch there; a normal cast (no dest) keeps
                // the cast_spell path. The START/GO handshake above is unchanged (keyed on `instant`) — a
                // timed ground cast just relays START now + GO at completion like any other timed spell.
                let dest = c
                    .targets
                    .target_flags
                    .get_dest_location()
                    .map(|d| (d.destination.x, d.destination.y, d.destination.z));
                let cast_result = match dest {
                    Some((x, y, z)) => {
                        store.cast_spell_at(conn.account_id, self_guid, c.spell, target, x, y, z)
                    }
                    None => store.cast_spell(conn.account_id, self_guid, c.spell, target),
                };
                if let Err(e) = cast_result {
                    // Carry the REASON so the client prints the red error line ("Not enough
                    // rage", "You must be behind your target") — the bare typed Failure only reset
                    // the button silently, leaving server-only gates (behind/stealth/stance/react)
                    // invisible. Raw body: spell + 0x02 + CastFailureReason.
                    let reason = codec::cast_failure_reason_for(&e.to_string());
                    send(
                        tx,
                        Outbound::Raw {
                            opcode: 0x0130,
                            body: codec::build_cast_result_failed(c.spell, reason),
                        },
                    )?;
                }
            }
        }
        // Stop the ranged auto-repeat loop (the client toggled off / auto-switched to melee).
        // `stop_attack` ONLY when a ranged loop is actually armed: the client's
        // melee-press sends CMSG_ATTACKSWING *then* this cancel back-to-back (live-logged), and the
        // swing handler has already overwritten the shared engagement row to MELEE + cleared
        // `ranged_repeat` — an unconditional stop here deleted that just-armed melee row (the
        // "press melee attack twice" bug). Same observable rule the reference cores follow — a no-op when nothing is
        // armed. NO inline ack either — the SMSG_CANCEL_AUTO_REPEAT the client needs on a real
        // teardown is sent by the game_melee_attack on_delete relay (the one choke point), and real
        // cores never ack a client-initiated cancel from the handler (cmangos: echo-loop warning).
        ClientOpcodeMessage::CMSG_CANCEL_AUTO_REPEAT_SPELL => {
            let was_repeat = matches!(&conn.state, WorldState::InWorld(iw) if iw.ranged_repeat);
            log::info!("world[autoshot]: CMSG_CANCEL_AUTO_REPEAT_SPELL ranged_repeat_active={was_repeat} (account {})", conn.account_id);
            if let WorldState::InWorld(iw) = &mut conn.state {
                iw.ranged_repeat = false;
            }
            if was_repeat {
                if let Err(e) = store.stop_attack(conn.account_id, self_guid) {
                    log::debug!(
                        "world: cancel_auto_repeat stop_attack ignored (account {}): {e}",
                        conn.account_id
                    );
                }
            }
        }
        // Cancel a buff: the player right-clicked its icon (CMSG_CANCEL_AURA). Best-effort — remove the
        // caller's own aura by spell id; the aura on_delete relay then re-syncs the buff bar. A failure
        // (no such aura / not in world) is per-action — log + ignore, never tear the session down.
        // Chat channels: the client auto-sends JOIN for General/Trade/LocalDefense on
        // zone-in; ack with SMSG_CHANNEL_NOTIFY(YouJoined) so the tab arms (the client won't
        // accept channel lines for a channel it never got the join notice for). Re-joins are
        // idempotent (the module dedupes; vanilla re-acks). Passwords are ignored (no private
        // channels this slice).
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(c) => {
            if let Err(e) = store.join_channel(conn.account_id, self_guid, c.channel_name.clone()) {
                log::debug!(
                    "world: join_channel failed (account {}): {e}",
                    conn.account_id
                );
            } else {
                use wow_world_messages::vanilla::{ChatNotify, SMSG_CHANNEL_NOTIFY};
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_NOTIFY(Box::new(
                        SMSG_CHANNEL_NOTIFY {
                            notify_type: ChatNotify::YouJoinedNotice,
                            channel_name: c.channel_name,
                        },
                    ))),
                )?;
            }
        }
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(c) => {
            if let Err(e) = store.leave_channel(conn.account_id, self_guid, c.channel_name.clone()) {
                log::debug!(
                    "world: leave_channel failed (account {}): {e}",
                    conn.account_id
                );
            } else {
                use wow_world_messages::vanilla::{ChatNotify, SMSG_CHANNEL_NOTIFY};
                send(
                    tx,
                    Outbound::One(ServerOpcodeMessage::SMSG_CHANNEL_NOTIFY(Box::new(
                        SMSG_CHANNEL_NOTIFY {
                            notify_type: ChatNotify::YouLeftNotice,
                            channel_name: c.channel_name,
                        },
                    ))),
                )?;
            }
        }
        ClientOpcodeMessage::CMSG_CANCEL_AURA(c) => {
            if let Err(e) = store.cancel_aura(conn.account_id, self_guid, c.id) {
                log::debug!(
                    "world: cancel_aura ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        // Cancel an in-progress cast (CMSG_CANCEL_CAST — Esc / moved / recast). Clear the caller's pending
        // cast server-side so a scheduled completion can't fire a phantom SMSG_SPELL_GO that wedges the
        // client's cast state ("Another action is in progress"). Best-effort — a failure (nothing pending
        // / not in world) is per-action: log + ignore. The client's spell id (_c) is not needed.
        ClientOpcodeMessage::CMSG_CANCEL_CAST(_c) => {
            if let Err(e) = store.cancel_cast(conn.account_id, social::self_guid(conn).unwrap_or(0)) {
                log::debug!(
                    "world: cancel_cast ignored (account {}): {e}",
                    conn.account_id
                );
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}
