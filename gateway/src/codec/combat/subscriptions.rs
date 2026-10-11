//! Table Relays for this Protocol Family.
use lyracore_shared::spell::taxonomy::{A_MOD_RESISTANCE, A_MOD_SPEED, SPEED_SWIM};

use crate::codec::entity::subscriptions::build_peer_create;
use crate::codec::{self};
use crate::stdb::bindings::*;
use crate::stdb::world_index::{CellKey, EntityLayer, ShardId};
use crate::stdb::world_view::*;
use crate::stdb::Coordinator;
use crate::world::Outbound;
use lyracore_shared::spatial::BOX_HALF_SPAN;
use spacetimedb_sdk::Table;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

/// Stealth presence marker (taxonomy `A_STEALTH`): a unit carrying ≥1 such aura is stealthed and must
/// be hidden from OTHER players' clients. Mirrors `module::spell::taxonomy::A_STEALTH = 0xA8`.
pub(crate) use lyracore_shared::spell::taxonomy::A_STEALTH;

/// The visibility action a single A_STEALTH aura insert/delete implies, decided from the POST-change
/// count of A_STEALTH auras on the stealther's guid. A Stealth cast applies TWO A_STEALTH auras (and a
/// break removes both); since the SDK applies the whole row-delta before firing callbacks, both inserts
/// fire seeing count 2 and both deletes seeing count 0 — so we HIDE on ANY insert while stealthed
/// (count >= 1) and REVEAL when the count reaches 0. Pure + unit-tested; the exactly-once guarantee
/// against an already-hidden/visible peer is the per-viewer `created` set at the call site, not a boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StealthAction {
    Hide,
    Reveal,
    None,
}

/// Hide on a stealth insert and reveal when the last marker is removed. The aura index supplies
/// the count after this callback's change; the viewer's `created` set makes repeated actions harmless.
pub(crate) fn stealth_action(is_insert: bool, post_change_count: usize) -> StealthAction {
    match (is_insert, post_change_count) {
        (true, n) if n >= 1 => StealthAction::Hide,
        (false, 0) => StealthAction::Reveal,
        _ => StealthAction::None,
    }
}

/// Relay a visible melee swing or ranged launch. The Module emits ranged damage at impact
/// through `SpellCastEvent`, which `cast_event_outbound` relays separately.
pub(crate) fn combat_event_outbound(
    created: &Mutex<HashSet<u64>>,
    row: &CombatEvent,
) -> Vec<Outbound> {
    if !created.lock().unwrap().contains(&row.attacker_guid) {
        return Vec::new();
    }
    let mut out = Vec::new();
    // A ranged launch uses SPELL_GO. ATTACKERSTATEUPDATE would animate a melee swing.
    if row.ranged_spell_id != 0 {
        // Auto Shot stamps an ammo display id → the AMMO flag fires the arrow graphic (24 =
        // INVTYPE_AMMO). Wand Shoot (ammo_display_id 0) → no flag → the bolt. Deliberate
        // simplification: inv-type 24 is hardcoded.
        let ammo = if row.ammo_display_id != 0 {
            Some((row.ammo_display_id, 24))
        } else {
            None
        };
        let miss = row.hit_info == 2; // module HIT_MISS
        let go = codec::build_spell_go_outcome(
            row.attacker_guid,
            row.ranged_spell_id,
            row.target_guid,
            ammo,
            miss,
        );
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
            go,
        ))));
    } else if !row.spell_swing {
        // A fired on-next-swing spell (Heroic Strike/Cleave) REPLACES the white hit — the
        // whole swing rides the spell's cast-event row (GO + yellow named damage log), so this
        // event sends NO white ATTACKERSTATEUPDATE. killing_blow/ATTACKSTOP below still applies.
        let m = codec::build_attacker_state_update(
            row.attacker_guid,
            row.target_guid,
            row.damage,
            row.hit_info,
            row.blocked_amount,
            0,
        );
        out.push(Outbound::One(
            ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(m)),
        ));
    }
    // C2: on a MELEE killing blow, tell the attacker to leave combat stance. The target itself
    // vanishes via the game_world_entity on_delete → SMSG_DESTROY_OBJECT relay. A RANGED kill
    // sends no ATTACKSTOP (the client was never in melee-attack state; vanilla sends none) —
    // its stop signal is the SMSG_CANCEL_AUTO_REPEAT from the engagement-row delete relay
    // (kill_creature's disengage frees the attacker's row).
    if row.killing_blow && row.ranged_spell_id == 0 {
        let stop = codec::build_attack_stop(row.attacker_guid, row.target_guid);
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(
            Box::new(stop),
        )));
    }
    out
}

/// Combat-stance engage leg: SMSG_ATTACKSTART for a melee engagement —
/// the one body both legs run. Ranged rows are NOT melee combat (Auto Shot: ATTACKSTART would animate a
/// melee swing between shots); the `created` gate skips an out-of-scope attacker like the swing
/// relay.
pub(crate) fn melee_engage_outbound(
    created: &Mutex<HashSet<u64>>,
    row: &MeleeAttack,
) -> Vec<Outbound> {
    if row.ranged_spell_id != 0 {
        return Vec::new();
    }
    if !created.lock().unwrap().contains(&row.attacker_guid) {
        return Vec::new();
    }
    let m = codec::build_attack_start(row.attacker_guid, row.target_guid);
    vec![Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(
        Box::new(m),
    ))]
}

/// Combat-stance disengage leg. A RANGED row's delete is the ONE
/// server-initiated auto-repeat teardown choke point: the OWNING player (and only them) gets the
/// 0-byte SMSG_CANCEL_AUTO_REPEAT so its toggle drops in lockstep (Auto Shot); a melee row's delete is
/// SMSG_ATTACKSTOP for everyone (a non-kill disengage leaves stance too).
pub(crate) fn melee_disengage_outbound(self_guid: u64, row: &MeleeAttack) -> Vec<Outbound> {
    if row.ranged_spell_id != 0 {
        if row.attacker_guid == self_guid {
            return vec![Outbound::One(ServerOpcodeMessage::SMSG_CANCEL_AUTO_REPEAT)];
        }
        return Vec::new();
    }
    let m = codec::build_attack_stop(row.attacker_guid, row.target_guid);
    vec![Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(
        Box::new(m),
    ))]
}

/// Spell cast visuals: the one body both paths run. It handles START and GO sequencing, interrupt
/// teardown, pushback, Proc logs, damage and heal logs, and cooldown. Every caster-private branch
/// keys on `self_guid`.
pub(crate) fn cast_event_outbound(self_guid: u64, row: &SpellCastEvent) -> Vec<Outbound> {
    let mut out = Vec::new();
    let Some(kind) = SpellCastEventKind::decode(row) else {
        return out;
    };
    // INTERRUPT signal: the victim's mid-cast timed spell was cancelled.
    // Relay SMSG_SPELL_FAILURE{spell, Interrupted} to the caster so the client tears down its cast
    // bar. No SMSG_SPELL_GO follows because the cast never resolved.
    if kind == SpellCastEventKind::Interrupt {
        // SELF-ONLY for a PLAYER caster: game_spell_cast_event is a global public subscription,
        // so this closure fires for EVERY player. SMSG_SPELL_FAILURE is a caster-private cast-bar
        // teardown (unlike the START/GO broadcast visuals), so relay it ONLY to the caster — else
        // bystanders get stray "Interrupted" feedback and a bystander mid-casting the same spell
        // risks a spurious teardown.
        // A creature or pet caster (0xF130/0xF140 high-guid) has no self — broadcast so every observer's
        // mob cast bar tears down on a Kick/Counterspell (the packet carries the mob's guid, so
        // a bystander's own bar is untouched; mirrors the START broadcast that drew the bar).
        let is_creature = matches!(row.caster_guid >> 48, 0xF130 | 0xF140);
        if is_creature || row.caster_guid == self_guid {
            let m = codec::build_spell_failure(row.caster_guid, row.spell_id);
            out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_FAILURE(
                Box::new(m),
            )));
        }
        // A row that names a concrete reason (an on-next-swing strike that could not pay its cost at
        // the swing) follows the teardown with the failed cast result, in vanilla's order
        // (SendInterrupted then SendCastResult). The client needs BOTH: the teardown closes the cast
        // bar, the cast result releases the ability it holds as its current melee spell and prints
        // the red "Not enough rage" line. Caster-private, like the teardown above — the raw encoder,
        // because the typed SMSG_CAST_RESULT inverts the success/failure semantics.
        if row.caster_guid == self_guid {
            if let Some(reason) = codec::cast_failure_reason_for_code(row.failure_reason) {
                out.push(Outbound::Raw {
                    opcode: 0x0130,
                    body: codec::build_cast_result_failed(row.spell_id, reason),
                });
            }
        }
        return out;
    }
    // PUSHBACK signal: a direct hit moved the fire time of an in-progress timed cast. Broadcast
    // SMSG_SPELL_DELAYED so anyone watching the caster's cast bar sees the same shift.
    if kind == SpellCastEventKind::Pushback {
        let m = codec::build_spell_delayed(row.caster_guid, row.delay_ms);
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_DELAYED(
            Box::new(m),
        )));
        return out;
    }
    // Proc and ranged-impact rows report damage without another cast animation or cooldown.
    if kind == SpellCastEventKind::ProcLog {
        if row.damage > 0 {
            let log = codec::build_spell_non_melee_damage_log(
                row.target_guid,
                row.caster_guid,
                row.spell_id,
                row.damage,
                row.school,
                row.is_crit,
                row.resisted,
                row.absorbed,
            );
            out.push(Outbound::One(
                ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
            ));
        }
        return out;
    }
    if kind == SpellCastEventKind::Start {
        // Cast-START (a timed spell): SMSG_SPELL_START with the cast-bar duration so observers
        // see the bar FILL. The GO/COOLDOWN follow on the cast-GO COMPLETION event.
        let start =
            codec::build_spell_start(row.caster_guid, row.spell_id, row.cast_time_ms, 0, None);
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
            Box::new(start),
        )));
        return out;
    }
    // Cast-GO. Legacy rows reach this branch when cast_time_ms is zero. Sequence:
    //   - GENUINE INSTANT (is_completion=false): START(0)+GO — SendSpellStart fires for EVERY
    //     non-triggered cast (timer 0 for an instant) to register the pending cast, then cast() →
    //     SendSpellGo finalizes it. START flags = 0x02, GO flags = 0x0100 (set in the builders).
    //   - TIMED COMPLETION (is_completion=true): GO ALONE — the begin already sent START(cast_time);
    //     a 2nd START(0) reset the bar to zero-length ("stuck on full"). The SMSG_SPELL_GO is the
    //     client's cast finalizer (it matches by caster guid + spell id to release the pending cast).
    // The CASTER's OWN instant cast (cast_time 0 → this GO branch, !is_completion) already got
    // START+GO SYNCHRONOUSLY from the CMSG_CAST_SPELL handler (so they precede the aura effects the
    // SDK's alphabetical callback order would otherwise send first). Skip the duplicate to the caster
    // — but still relay it to OBSERVERS (caster != self) and for TIMED COMPLETIONS (is_completion,
    // which the gateway did NOT sync-send). NOTE: a player's TRIGGERED instant cast (channel-tick
    // missile / on-hit trigger) also matches !is_completion && caster==self and is suppressed here —
    // it did not get a synchronous send, so it loses its caster-side visual until the client_initiated
    // flag lands (tracked follow-up). Acceptable for the slice; observers still see it.
    // Suppress ONLY what the CMSG handler actually sent synchronously — the row says so
    // (client_initiated rides from the cast_spell reducer path alone). The old shape
    // (!is_completion && self) also swallowed the caster's TRIGGERED instants — channel-tick
    // missiles (Arcane Missiles/Drain Life), on-hit trigger_spell procs — which never had a
    // synchronous send, so their caster saw no visual while observers did.
    let caster_synced_instant = row.client_initiated && row.caster_guid == self_guid;
    if !caster_synced_instant {
        if !row.is_completion {
            let start = codec::build_spell_start(row.caster_guid, row.spell_id, 0, 0, None);
            out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
                Box::new(start),
            )));
        }
        // CAST_RESULT(OK) is caster-only: clears the pending spell state so the subsequent
        // GO can release m_currentSpells. Only for timed completions — the instant-cast
        // caster got it synchronously from the CMSG_CAST_SPELL handler.
        if row.is_completion && row.caster_guid == self_guid {
            out.push(Outbound::Raw {
                opcode: 0x0130,
                body: codec::build_cast_result_ok(row.spell_id),
            });
        }
        let mut go = codec::build_spell_go(row.caster_guid, row.spell_id, row.target_guid, None);
        // A 0-damage on-next-swing FIRE that rode a missed/dodged/parried swing reports
        // the outcome in the GO's miss list — the client prints the yellow "Your Heroic
        // Strike missed/was dodged/was parried" line (the white MISS was suppressed via
        // spell_swing). swing_hit_info uses the CombatEvent codes (2 miss, 3 dodge, 4 parry);
        // every non-swing row carries 0, keeping this branch dead for ordinary casts.
        if row.is_completion && row.damage == 0 {
            use wow_world_messages::vanilla::{SpellMiss, SpellMissInfo};
            let miss_info = match row.swing_hit_info {
                2 => Some(SpellMissInfo::Miss),
                3 => Some(SpellMissInfo::Dodge),
                4 => Some(SpellMissInfo::Parry),
                _ => None,
            };
            if let Some(mi) = miss_info {
                go.hits.clear();
                go.misses = vec![SpellMiss {
                    target: wow_world_messages::Guid::new(row.target_guid),
                    miss_info: mi,
                }];
            }
        }
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
            go,
        ))));
    }

    if row.damage > 0 {
        let log = codec::build_spell_non_melee_damage_log(
            row.target_guid,
            row.caster_guid,
            row.spell_id,
            row.damage,
            row.school,
            row.is_crit,
            row.resisted,
            row.absorbed,
        );
        out.push(Outbound::One(
            ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
        ));
    }
    // The green floating heal number + combat-log line — SMSG_SPELLHEALLOG whenever
    // this cast restored health (module sums EFFECTIVE heal onto the row; overheal-only
    // casts carry 0 and stay silent, matching the damage gate's shape).
    if row.healed > 0 {
        use wow_world_messages::vanilla::SMSG_SPELLHEALLOG;
        use wow_world_messages::Guid;
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELLHEALLOG(
            Box::new(SMSG_SPELLHEALLOG {
                victim: Guid::new(row.target_guid),
                caster: Guid::new(row.caster_guid),
                id: row.spell_id,
                damage: row.healed,
                critical: row.is_crit,
            }),
        )));
    }

    // SMSG_SPELL_GO releases pending casts. A zero-duration cooldown can leave the button stuck.
    if row.cooldown_ms > 0 {
        let cd = codec::build_spell_cooldown(row.caster_guid, row.spell_id, row.cooldown_ms);
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(
            Box::new(cd),
        )));
    }
    out
}

/// The full aura-array VALUES block for `target_guid`. `auras` is that unit's current aura set
/// ([`crate::stdb::world_view::AuraIndex::on_target`]); the filter only guards a caller that
/// hands over a wider set.
pub(crate) fn aura_sync(
    auras: impl Iterator<Item = crate::stdb::bindings::Aura>,
    target_guid: u64,
) -> Outbound {
    let slots: Vec<codec::update_mask::AuraSlot> = auras
        .filter(|a| a.target_guid == target_guid)
        .map(|a| codec::update_mask::AuraSlot {
            slot: a.slot,
            spell_id: a.spell_id,
            flags: a.flags,
            level: a.level,
        })
        .collect();
    let mask = codec::update_mask::full_aura_mask(&slots);
    let (opcode, body) = codec::build_values_update_raw(target_guid, &mask);
    Outbound::Raw { opcode, body }
}

// A self resistance aura applies or expires after the entity CREATE. Recompute all seven effective
// resistances from the coordinator's post-change cache and push the complete paperdoll group.
pub(crate) fn armor_packet(
    coord: &Coordinator,
    changed: &Aura,
    self_guid: u64,
) -> Option<Outbound> {
    const RESISTANCE_MASK: u32 = 0x7f;
    if changed.target_guid != self_guid
        || changed.eff_kind != A_MOD_RESISTANCE
        || (changed.eff_p0 as u32 & RESISTANCE_MASK) == 0
    {
        return None;
    }
    // The coordinator owns the entity, aura, item-instance and item-catalogue caches needed by this fold.
    let guard = coord.0.coord();
    let db = &guard.conn.db;
    let eff = crate::stdb::armor::effective_resistances(db, self_guid);
    // Carry the positive AURA portion alongside the total so the paperdoll renders the
    // green "(+N)" (Devotion Aura showed as plain white armor). Raw path — the positive
    // field has no gtker setter. Login self-corrects through this same relay (the SDK
    // replays aura rows after subscription-apply).
    let pos = crate::stdb::armor::aura_armor_positive(db, self_guid);
    Some(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(codec::build_armor_values(self_guid, eff, pos)),
    )))
}

// The Module has already persisted sheet stats in the aura transaction. Re-push that row
// only for aura kinds that affect it; the paperdoll is private to its owner.
pub(crate) fn sheet_packet(
    coord: &Coordinator,
    changed: &Aura,
    self_guid: u64,
) -> Option<Outbound> {
    let moves_sheet = lyracore_shared::aura::moves_sheet(changed.eff_kind, changed.eff_p0);
    if changed.target_guid != self_guid || !moves_sheet {
        return None;
    }
    let guard = coord.0.coord();
    let db = &guard.conn.db;
    let st = crate::stdb::armor::sheet_stats(db, self_guid);
    drop(guard);
    st.map(|st| {
        Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_sheet_stats_values(self_guid, &st),
        )))
    })
}

/// Hide a peer on stealth gain; reveal it with current equipment and auras on stealth loss.
/// The AOI and `created` checks prevent distant or duplicate CREATEs. Self never hides from itself.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stealth_visibility(
    post_change_stealth_count: usize,
    view: &WorldView,
    shard: crate::stdb::world_index::ShardId,
    session: u64,
    coord: &Coordinator,
    created: &Arc<Mutex<HashSet<u64>>>,
    changed: &Aura,
    self_guid: u64,
    is_insert: bool,
) -> Vec<Outbound> {
    if changed.eff_kind != A_STEALTH || changed.target_guid == self_guid {
        return Vec::new();
    }
    match stealth_action(is_insert, post_change_stealth_count) {
        StealthAction::Hide => {
            // Evict so a later REVEAL (and ordinary AOI re-entry) re-CREATEs; only DESTROY if the
            // viewer actually had it (idempotent — a never-created peer needs no DESTROY).
            if created.lock().unwrap().remove(&changed.target_guid) {
                vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                    codec::build_destroy_object(changed.target_guid),
                ))]
            } else {
                Vec::new()
            }
        }
        StealthAction::Reveal => {
            // Re-CREATE only if not currently shown (idempotent) AND the peer is in this viewer's
            // scope (its row is in the cache). Insert into `created` BEFORE building so the dedup
            // is consistent; if the row is gone (out of scope), undo the insert and emit nothing.
            if !created.lock().unwrap().insert(changed.target_guid) {
                return Vec::new();
            }

            if !view
                .spatial
                .can_see(EntityLayer::WorldEntity, session, changed.target_guid)
            {
                created.lock().unwrap().remove(&changed.target_guid);
                return Vec::new();
            }
            match coord
                .0
                .coord()
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&changed.target_guid)
            {
                Some(row) => match build_peer_create(coord, view, shard, self_guid, &row) {
                    Some(out) => out,
                    None => {
                        // Encode failure: roll the dedup entry back like `offer_peer_create`
                        // does, else this guid is permanently suppressed (marked created with
                        // no CREATE ever sent) — the same latent re-entry bug fixed on the
                        // insert/update path.
                        created.lock().unwrap().remove(&changed.target_guid);
                        Vec::new()
                    }
                },
                None => {
                    created.lock().unwrap().remove(&changed.target_guid);
                    Vec::new()
                }
            }
        }
        StealthAction::None => Vec::new(),
    }
}

/// Channel timers belong to the caster, including channels whose auras sit on an enemy.
pub(crate) fn spell_channel_outbound(
    coord: &Coordinator,
    self_guid: u64,
    row: &Aura,
    removed: bool,
) -> Option<Outbound> {
    use wow_world_messages::vanilla::{MSG_CHANNEL_START_Server, MSG_CHANNEL_UPDATE_Server};
    if row.caster_guid != self_guid {
        return None;
    }
    let guard = coord.0.coord();
    let db = &guard.conn.db;
    let is_channel = |aura: &Aura| {
        let flags = db
            .game_spell()
            .spell_id()
            .find(&aura.spell_id)
            .map_or(0, |spell| spell.cast_flags);
        lyracore_shared::spell::is_channel_aura(aura.eff_kind, flags)
    };
    if !is_channel(row) {
        return None;
    }
    let active = db
        .game_aura()
        .iter()
        .filter(|aura| aura.caster_guid == self_guid && is_channel(aura))
        .min_by_key(|aura| aura.id);
    let packet = if removed {
        if active.is_some() {
            return None;
        }
        ServerOpcodeMessage::MSG_CHANNEL_UPDATE(MSG_CHANNEL_UPDATE_Server { time: 0 })
    } else {
        if active.as_ref().is_none_or(|active| active.id != row.id) {
            return None;
        }
        let duration = ((row.expires_at.to_micros_since_unix_epoch()
            - row.applied_at.to_micros_since_unix_epoch())
            / 1000)
            .clamp(0, u32::MAX as i64) as u32;
        ServerOpcodeMessage::MSG_CHANNEL_START(MSG_CHANNEL_START_Server {
            spell: row.spell_id,
            duration,
        })
    };
    Some(Outbound::One(packet))
}

/// Aura insert leg: array sync, self-only duration/run-speed/armor/sheet packets, and the
/// stealth HIDE transition. The target's current aura set comes from the gateway's aura index,
/// post-change like the cache it mirrors; `stealth_count` was taken on the pump.
#[allow(clippy::too_many_arguments)]
pub(crate) fn aura_insert_outbound(
    coord: &Coordinator,
    view: &WorldView,
    shard: crate::stdb::world_index::ShardId,
    session: u64,
    created: &Arc<Mutex<HashSet<u64>>>,
    self_guid: u64,
    row: &Aura,
    stealth_count: usize,
) -> Vec<Outbound> {
    let mut out = Vec::new();
    out.extend(spell_channel_outbound(coord, self_guid, row, false));
    // A stealth-hidden peer (not in `created`) must get NO per-peer relay: a partial VALUES on
    // a DESTROYed object is a client crash/desync vector.
    let visible =
        row.target_guid == self_guid || created.lock().unwrap().contains(&row.target_guid);
    let current = visible.then(|| view.auras.on_target(shard, row.target_guid));
    if let Some(current) = &current {
        out.push(aura_sync(current.iter().cloned(), row.target_guid));
    }
    if let Some(o) = aura_duration_packet(row, self_guid) {
        out.push(o);
    }
    if let Some(current) = current {
        if let Some(o) = swim_speed_packet(current.iter().cloned(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = run_speed_packet(current.into_iter(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = armor_packet(coord, row, self_guid) {
            out.push(o);
        }
        if let Some(o) = sheet_packet(coord, row, self_guid) {
            out.push(o);
        }
    }
    out.extend(stealth_visibility(
        stealth_count,
        view,
        shard,
        session,
        coord,
        created,
        row,
        self_guid,
        true,
    ));
    out
}

/// Aura update leg (no stealth transition on an update).
pub(crate) fn aura_update_outbound(
    coord: &Coordinator,
    view: &WorldView,
    shard: crate::stdb::world_index::ShardId,
    created: &Arc<Mutex<HashSet<u64>>>,
    self_guid: u64,
    row: &Aura,
    expires_changed: bool,
) -> Vec<Outbound> {
    let mut out = Vec::new();
    let visible =
        row.target_guid == self_guid || created.lock().unwrap().contains(&row.target_guid);
    let current = visible.then(|| view.auras.on_target(shard, row.target_guid));
    if let Some(current) = &current {
        out.push(aura_sync(current.iter().cloned(), row.target_guid));
    }
    // Re-send the timer only when the duration window actually changed (a refresh).
    if expires_changed {
        if let Some(o) = aura_duration_packet(row, self_guid) {
            out.push(o);
        }
    }
    if let Some(current) = current {
        if let Some(o) = swim_speed_packet(current.iter().cloned(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = run_speed_packet(current.into_iter(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = armor_packet(coord, row, self_guid) {
            out.push(o);
        }
        if let Some(o) = sheet_packet(coord, row, self_guid) {
            out.push(o);
        }
    }
    out
}

/// Aura delete leg (the REVEAL half of stealth). The aura index is post-delete, so the
/// sync/speed folds read the remaining set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn aura_delete_outbound(
    coord: &Coordinator,
    view: &WorldView,
    shard: crate::stdb::world_index::ShardId,
    session: u64,
    created: &Arc<Mutex<HashSet<u64>>>,
    self_guid: u64,
    row: &Aura,
    stealth_count: usize,
) -> Vec<Outbound> {
    let mut out = Vec::new();
    out.extend(spell_channel_outbound(coord, self_guid, row, true));
    let visible =
        row.target_guid == self_guid || created.lock().unwrap().contains(&row.target_guid);
    if visible {
        let current = view.auras.on_target(shard, row.target_guid);
        out.push(aura_sync(current.iter().cloned(), row.target_guid));
        if let Some(o) = swim_speed_packet(current.iter().cloned(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = run_speed_packet(current.into_iter(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = armor_packet(coord, row, self_guid) {
            out.push(o);
        }
        if let Some(o) = sheet_packet(coord, row, self_guid) {
            out.push(o);
        }
    }
    out.extend(stealth_visibility(
        stealth_count,
        view,
        shard,
        session,
        coord,
        created,
        row,
        self_guid,
        false,
    ));
    out
}

/// Reconnect sends one current aura array per target and observer. Owner timers remain per aura,
/// while speed, armor, and sheet values refresh once even when several auras changed offline.
pub(crate) fn aura_snapshot_outbound(
    coord: &Coordinator,
    view: &WorldView,
    shard: crate::stdb::world_index::ShardId,
    viewer: &Viewer,
    target_guid: u64,
    previous: &[Aura],
) -> Vec<Outbound> {
    let current = view.auras.on_target(shard, target_guid);
    let stealth_count = current
        .iter()
        .filter(|row| row.eff_kind == A_STEALTH)
        .count();
    let mut out = Vec::new();
    if let Some(stealth) = current
        .iter()
        .chain(previous)
        .find(|row| row.eff_kind == A_STEALTH)
    {
        out.extend(stealth_visibility(
            stealth_count,
            view,
            shard,
            viewer.session,
            coord,
            &viewer.created,
            stealth,
            viewer.self_guid,
            stealth_count > 0,
        ));
        // Reveal includes the aura array; hide must not send VALUES after DESTROY.
        if !out.is_empty() {
            return out;
        }
    }
    if target_guid != viewer.self_guid && !viewer.created.lock().unwrap().contains(&target_guid) {
        return out;
    }
    out.push(aura_sync(current.iter().cloned(), target_guid));
    if target_guid != viewer.self_guid {
        return out;
    }
    out.extend(
        current
            .iter()
            .filter_map(|row| aura_duration_packet(row, viewer.self_guid)),
    );
    out.extend(
        current
            .iter()
            .chain(previous)
            .find_map(|row| run_speed_packet(current.iter().cloned(), row, viewer.self_guid)),
    );
    out.extend(
        current
            .iter()
            .chain(previous)
            .find_map(|row| swim_speed_packet(current.iter().cloned(), row, viewer.self_guid)),
    );
    out.extend(
        current
            .iter()
            .chain(previous)
            .find_map(|row| armor_packet(coord, row, viewer.self_guid)),
    );
    out.extend(
        current
            .iter()
            .chain(previous)
            .find_map(|row| sheet_packet(coord, row, viewer.self_guid)),
    );
    out
}

/// Projectile impact: the floating damage number for a projectile that
/// finished its travel — never a START/GO. Pure over the row; broadcast.
pub(crate) fn impact_event_outbound(row: &SpellImpactEvent) -> Vec<Outbound> {
    if row.damage == 0 {
        return Vec::new(); // a fully-absorbed impact (or a vanished target) logs nothing
    }
    let log = codec::build_spell_non_melee_damage_log(
        row.target_guid,
        row.caster_guid,
        row.spell_id,
        row.damage,
        row.school,
        row.is_crit,
        row.resisted,
        row.absorbed,
    );
    vec![Outbound::One(
        ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
    )]
}

/// Dynamic objects: the shared-dispatch twin of the per-player
/// `on_dynobj_insert` relay. The instance gate already ran in `world_view::dynobj_appeared`.
pub(crate) fn relay_dynobj_create(row: &DynamicObject) -> Vec<Outbound> {
    let m = codec::build_dynamicobject_create_object(
        row.guid,
        row.caster_guid,
        row.spell_id,
        row.x,
        row.y,
        row.z,
        row.radius_yd,
    );
    vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(m),
    ))]
}

/// A bare SMSG_DESTROY_OBJECT — the delete leg of the dynobj/corpse families, shared because the
/// packet is nothing but the guid.
pub(crate) fn relay_destroy_object(guid: u64) -> Vec<Outbound> {
    vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
        codec::build_destroy_object(guid),
    ))]
}

/// SMSG_UPDATE_AURA_DURATION (buff timer): the 1.12 UpdateMask aura array carries NO duration,
/// so without this the client shows "0 seconds"/flashing icon. Only for THIS player's own auras
/// (the slot indexes the player's aura array) with a finite window; the duration is the full
/// applied span (expires - applied), which is what the client shows on apply/refresh. Pure →
/// unit-tested (hoisted verbatim out of `subscribe_player_events`, no logic change).
pub(crate) fn aura_duration_packet(row: &Aura, self_guid: u64) -> Option<Outbound> {
    if row.target_guid != self_guid {
        return None;
    }
    // i64::MAX is the infinite-duration sentinel; no timer packet for toggle auras.
    if row.expires_at.to_micros_since_unix_epoch() == i64::MAX {
        return None;
    }
    let ms = (row.expires_at.to_micros_since_unix_epoch()
        - row.applied_at.to_micros_since_unix_epoch())
        / 1000;
    if ms <= 0 {
        return None;
    }
    Some(Outbound::One(
        ServerOpcodeMessage::SMSG_UPDATE_AURA_DURATION(codec::build_aura_duration(
            row.slot, ms as u32,
        )),
    ))
}

/// A movement aura change refreshes the owner's speed from its current movement and mount auras.
/// The complete fold also applies stacks and the Module's minimum movement speed.
pub(crate) fn run_speed_packet(
    auras: impl Iterator<Item = crate::stdb::bindings::Aura>,
    changed: &Aura,
    self_guid: u64,
) -> Option<Outbound> {
    const BASE_RUN_SPEED: f32 = 7.0; // vanilla base run yd/s
    if changed.target_guid != self_guid
        || !lyracore_shared::aura::moves_run_speed(changed.eff_kind, changed.eff_p0)
    {
        return None;
    }
    let speed = lyracore_shared::aura::run_speed(
        BASE_RUN_SPEED,
        auras
            .filter(|aura| aura.target_guid == self_guid)
            .map(|aura| lyracore_shared::aura::AuraAmount {
                kind: aura.eff_kind,
                parameter: aura.eff_p0,
                amount: aura.amount,
                stacks: aura.stacks,
            }),
    );
    Some(Outbound::One(
        ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(Box::new(codec::build_force_run_speed(
            self_guid, speed,
        ))),
    ))
}

/// A swimming aura change refreshes the owner from the remaining aura set.
pub(crate) fn swim_speed_packet(
    auras: impl Iterator<Item = crate::stdb::bindings::Aura>,
    changed: &Aura,
    self_guid: u64,
) -> Option<Outbound> {
    if changed.target_guid != self_guid
        || changed.eff_kind != A_MOD_SPEED
        || changed.eff_p0 != i32::from(SPEED_SWIM)
    {
        return None;
    }
    let speed = lyracore_shared::aura::swim_speed(
        lyracore_shared::constants::speeds::SWIM,
        auras
            .filter(|aura| aura.target_guid == self_guid)
            .map(|aura| lyracore_shared::aura::AuraAmount {
                kind: aura.eff_kind,
                parameter: aura.eff_p0,
                amount: aura.amount,
                stacks: aura.stacks,
            }),
    );
    Some(Outbound::One(
        ServerOpcodeMessage::SMSG_FORCE_SWIM_SPEED_CHANGE(Box::new(codec::build_force_swim_speed(
            self_guid, speed,
        ))),
    ))
}

/// A ground-area dynamic object appeared (Consecration's swirl etc.) → CREATE for every
/// same-instance viewer on the row's shard — the same `instance_relay_gate` the per-player leg
/// runs. Short-lived, so no login/AOI resident sweep (a mid-area login misses the visual),
/// exactly as before.
pub(crate) fn dynobj_appeared(view: &WorldView, shard: ShardId, row: &DynamicObject) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        if !crate::codec::entity::subscriptions::instance_relay_gate(
            row.instance_id,
            Some(viewer.instance_id),
        ) {
            continue;
        }
        let row = row.clone();
        enqueue(viewer.clone(), move |_| {
            crate::codec::combat::subscriptions::relay_dynobj_create(&row)
        });
    }
}

/// A dynamic object's area ended → DESTROY to every viewer on the shard. Deliberately NOT
/// instance-gated: the per-player delete leg never was (a DESTROY for a guid the client doesn't
/// know is a no-op), and the A/B contract is byte-for-byte equality.
pub(crate) fn dynobj_vanished(view: &WorldView, shard: ShardId, row: &DynamicObject) {
    let guid = row.guid;
    for viewer in viewers_on_shard(view, shard) {
        enqueue(viewer.clone(), move |_| {
            crate::codec::combat::subscriptions::relay_destroy_object(guid)
        });
    }
}

pub(crate) fn combat_audience(
    view: &WorldView,
    shard: ShardId,
    row: &CombatEvent,
) -> Vec<Arc<Viewer>> {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    view.cell_audience(
        shard,
        Some(key),
        BOX_HALF_SPAN,
        &[row.attacker_guid, row.target_guid],
    )
}

/// Queue a ranged launch or melee swing for each viewer. The job checks its created set.
pub(crate) fn combat_event_appeared(view: &WorldView, shard: ShardId, row: &CombatEvent) {
    let row = Arc::new(row.clone());
    for viewer in combat_audience(view, shard, &row) {
        let row = row.clone();
        enqueue(viewer.clone(), move |viewer| {
            crate::codec::combat::subscriptions::combat_event_outbound(&viewer.created, &row)
        });
    }
}

/// An engagement row names no cell; the attacker's indexed cell anchors it.
pub(crate) fn melee_audience(
    view: &WorldView,
    shard: ShardId,
    row: &MeleeAttack,
) -> Vec<Arc<Viewer>> {
    let key = view
        .spatial
        .entity_cell_on_shard(EntityLayer::WorldEntity, row.attacker_guid, shard);
    view.cell_audience(
        shard,
        key,
        BOX_HALF_SPAN,
        &[row.attacker_guid, row.target_guid],
    )
}

/// A melee/ranged engagement began → SMSG_ATTACKSTART per viewer (`melee_engage_outbound` runs
/// the ranged skip + created gate in the job).
pub(crate) fn melee_engaged(view: &WorldView, shard: ShardId, row: &MeleeAttack) {
    let row = Arc::new(row.clone());
    for viewer in melee_audience(view, shard, &row) {
        let row = row.clone();
        let created = viewer.created.clone();
        enqueue(viewer.clone(), move |_| {
            crate::codec::combat::subscriptions::melee_engage_outbound(&created, &row)
        });
    }
}

/// An engagement row was removed → ATTACKSTOP / the owner-only CANCEL_AUTO_REPEAT
/// (`melee_disengage_outbound`, keyed on each viewer's own guid).
pub(crate) fn melee_disengaged(view: &WorldView, shard: ShardId, row: &MeleeAttack) {
    let row = Arc::new(row.clone());
    for viewer in melee_audience(view, shard, &row) {
        let row = row.clone();
        let self_guid = viewer.self_guid;
        enqueue(viewer.clone(), move |_| {
            crate::codec::combat::subscriptions::melee_disengage_outbound(self_guid, &row)
        });
    }
}

/// Aura descriptors follow the target's AOI; channel timers also reach the caster.
pub(crate) fn aura_audience(view: &WorldView, shard: ShardId, row: &Aura) -> Vec<Arc<Viewer>> {
    let key = view
        .spatial
        .entity_cell_on_shard(EntityLayer::WorldEntity, row.target_guid, shard);
    view.cell_audience(
        shard,
        key,
        BOX_HALF_SPAN,
        &[row.target_guid, row.caster_guid],
    )
}

/// An aura appeared → per viewer: array sync + self-only packets + the stealth HIDE transition
/// (`aura_insert_outbound`, all gates inside the job against that viewer's own state).
pub(crate) fn aura_applied(
    view: &Arc<WorldView>,
    coord: &Coordinator,
    shard: ShardId,
    row: &Aura,
    stealth_count: usize,
) {
    let row = Arc::new(row.clone());
    for viewer in aura_audience(view, shard, &row) {
        let (row, coord, view) = (row.clone(), coord.clone(), view.clone());
        enqueue(viewer.clone(), move |viewer| {
            crate::codec::combat::subscriptions::aura_insert_outbound(
                &coord,
                &view,
                shard,
                viewer.session,
                &viewer.created,
                viewer.self_guid,
                &row,
                stealth_count,
            )
        });
    }
}

/// An aura changed → per viewer: array sync + refresh-only duration + self-only packets.
pub(crate) fn aura_updated(
    view: &Arc<WorldView>,
    coord: &Coordinator,
    shard: ShardId,
    row: &Aura,
    expires_changed: bool,
) {
    let row = Arc::new(row.clone());
    for viewer in aura_audience(view, shard, &row) {
        let (row, coord, view) = (row.clone(), coord.clone(), view.clone());
        enqueue(viewer.clone(), move |viewer| {
            crate::codec::combat::subscriptions::aura_update_outbound(
                &coord,
                &view,
                shard,
                &viewer.created,
                viewer.self_guid,
                &row,
                expires_changed,
            )
        });
    }
}

/// An aura expired/was removed → per viewer: array sync + self-only packets + the stealth REVEAL
/// transition.
pub(crate) fn aura_removed(
    view: &Arc<WorldView>,
    coord: &Coordinator,
    shard: ShardId,
    row: &Aura,
    stealth_count: usize,
) {
    let row = Arc::new(row.clone());
    for viewer in aura_audience(view, shard, &row) {
        let (row, coord, view) = (row.clone(), coord.clone(), view.clone());
        enqueue(viewer.clone(), move |viewer| {
            crate::codec::combat::subscriptions::aura_delete_outbound(
                &coord,
                &view,
                shard,
                viewer.session,
                &viewer.created,
                viewer.self_guid,
                &row,
                stealth_count,
            )
        });
    }
}

pub(crate) fn cast_audience(
    view: &WorldView,
    shard: ShardId,
    row: &SpellCastEvent,
) -> Vec<Arc<Viewer>> {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    view.cell_audience(
        shard,
        Some(key),
        BOX_HALF_SPAN,
        &[row.caster_guid, row.target_guid],
    )
}

/// A cast event landed → the full cast-lock sequence per viewer (`cast_event_outbound`: the
/// caster-private branches key on each viewer's own guid inside the job).
pub(crate) fn cast_event_appeared(view: &WorldView, shard: ShardId, row: &SpellCastEvent) {
    let row = Arc::new(row.clone());
    for viewer in cast_audience(view, shard, &row) {
        let row = row.clone();
        let self_guid = viewer.self_guid;
        enqueue(viewer.clone(), move |_| {
            crate::codec::combat::subscriptions::cast_event_outbound(self_guid, &row)
        });
    }
}

pub(crate) fn impact_audience(
    view: &WorldView,
    shard: ShardId,
    row: &SpellImpactEvent,
) -> Vec<Arc<Viewer>> {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    view.cell_audience(
        shard,
        Some(key),
        BOX_HALF_SPAN,
        &[row.caster_guid, row.target_guid],
    )
}

/// A projectile impact landed → the floating damage number per viewer that can see the caster.
pub(crate) fn impact_appeared(view: &WorldView, shard: ShardId, row: &SpellImpactEvent) {
    let row = Arc::new(row.clone());
    for viewer in impact_audience(view, shard, &row) {
        let row = row.clone();
        enqueue(viewer.clone(), move |_| {
            crate::codec::combat::subscriptions::impact_event_outbound(&row)
        });
    }
}

crate::stdb::relays::declare_relays! {
    game_dynamic_object [false] {
        insert(Snapshot) => |context, _event, row| dynobj_appeared(&context.view, context.shard, row);
        delete(Snapshot) => |context, _event, row| dynobj_vanished(&context.view, context.shard, row);
    }
    game_combat_event [false] {
        insert(Snapshot) => |context, _event, row| combat_event_appeared(&context.view, context.shard, row);
    }
    game_melee_attack [false] {
        insert(Snapshot) => |context, _event, row| melee_engaged(&context.view, context.shard, row);
        delete(Snapshot) => |context, _event, row| melee_disengaged(&context.view, context.shard, row);
    }
    game_spell_cast_event [false] {
        insert(Snapshot) => |context, _event, row| cast_event_appeared(&context.view, context.shard, row);
    }
    game_spell_impact_event [false] {
        insert(Snapshot) => |context, _event, row| impact_appeared(&context.view, context.shard, row);
    }
    game_aura [false] {
        insert(Snapshot) => |context, _event, row| { context.view.auras.upsert(context.shard, row); let count = context.view.auras.stealth_count(context.shard, row.target_guid); aura_applied(&context.view, &context.coord, context.shard, row, count); };
        update(Snapshot) => |context, _event, old, row| { context.view.auras.upsert(context.shard, row); aura_updated(&context.view, &context.coord, context.shard, row, old.expires_at != row.expires_at); };
        delete(Snapshot) => |context, _event, row| { context.view.auras.remove(context.shard, row); let count = context.view.auras.stealth_count(context.shard, row.target_guid); aura_removed(&context.view, &context.coord, context.shard, row, count); };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SpellCastEventKind {
    Start,
    Go,
    Interrupt,
    Pushback,
    ProcLog,
}

impl SpellCastEventKind {
    /// Decode the append-only Module code. Kind zero is a row from the old schema and keeps the
    /// former field-based order. An unknown nonzero kind is newer or damaged data, so guessing a
    /// client packet would be unsafe.
    pub(crate) fn decode(row: &SpellCastEvent) -> Option<Self> {
        match row.kind {
            0 if row.is_interrupted => Some(Self::Interrupt),
            0 if row.delay_ms > 0 => Some(Self::Pushback),
            0 if row.is_proc_log => Some(Self::ProcLog),
            0 if row.cast_time_ms > 0 => Some(Self::Start),
            0 => Some(Self::Go),
            1 => Some(Self::Start),
            2 => Some(Self::Go),
            3 => Some(Self::Interrupt),
            4 => Some(Self::Pushback),
            5 => Some(Self::ProcLog),
            unknown => {
                log::warn!(
                    "spell cast relay: unknown kind {unknown} (event {}, caster {}, spell {})",
                    row.id,
                    row.caster_guid,
                    row.spell_id
                );
                None
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lyracore_shared::spell::taxonomy::SPEED_MOVE;
    /// A minimal `Aura` row for the packet-builder tests: `target_guid`/`eff_kind`/`eff_p0`/`amount`
    /// are the fields those builders branch on; `applied_at`/`expires_at` (raw micros) drive the
    /// duration math. Everything else is a fixed, inert value.
    pub(crate) fn aura(
        target_guid: u64,
        eff_kind: u8,
        eff_p0: i32,
        amount: i32,
        applied_at_micros: i64,
        expires_at_micros: i64,
    ) -> Aura {
        Aura {
            id: 1,
            target_guid,
            caster_guid: target_guid,
            spell_id: 6673,
            slot: 3,
            level: 1,
            flags: 0,
            applied_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(applied_at_micros),
            expires_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(expires_at_micros),
            effect_id: 1,
            eff_kind,
            amount,
            eff_p0,
            eff_p0_kind: 0,
            eff_p1: 0,
            period_ms: 0,
            amount_remaining: 0,
            stacks: 1,
            next_tick_micros: 0,
            channel_target: 0,
            enters_combat: false,
            proc_flags: 0,
            proc_chance: 0,
            proc_ppm: 0.0,
            proc_ex: 0,
            proc_school_mask: 0,
            proc_family_name: 0,
            proc_family_flags: 0,
            proc_charges: 0,
            proc_icd_ms: 0,
            proc_ready_micros: 0,
        }
    }

    #[test]
    fn aura_duration_packet_pins_the_remaining_window_for_self_only() {
        let self_guid = 1u64;
        // A 5-second remaining window on the player's OWN aura, in slot 3.
        let mine = aura(self_guid, 0xA1, 1, 10, 0, 5_000_000);
        let out = aura_duration_packet(&mine, self_guid);
        let m = match out {
            Some(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_AURA_DURATION(m))) => m,
            _ => panic!("expected Some(Outbound::One(SMSG_UPDATE_AURA_DURATION))"),
        };
        assert_eq!(m, codec::build_aura_duration(3, 5000));
        // A peer's aura (not self) never gets a duration packet — the slot only indexes the
        // viewer's OWN aura array.
        assert!(aura_duration_packet(&mine, self_guid + 1).is_none());
        // The infinite-duration sentinel (a toggle aura) never gets a timer packet.
        let toggle = aura(self_guid, 0xA1, 1, 10, 0, i64::MAX);
        assert!(aura_duration_packet(&toggle, self_guid).is_none());
        // A non-positive remaining window (expires_at <= applied_at) emits nothing.
        let expired = aura(self_guid, 0xA1, 1, 10, 5_000_000, 5_000_000);
        assert!(aura_duration_packet(&expired, self_guid).is_none());
    }

    #[test]
    fn run_speed_packet_uses_the_strongest_move_bonus_on_self_only() {
        let self_guid = 1u64;
        // Ghost and Wisp Spirit coexist, but only the stronger run bonus applies.
        let a1 = aura(self_guid, 0xA4, 0, 25, 0, i64::MAX);
        let a2 = aura(self_guid, 0xA4, 0, 50, 0, i64::MAX);
        let current = vec![a1.clone(), a2.clone()];
        let out = run_speed_packet(current.clone().into_iter(), &a1, self_guid);
        let m = match out {
            Some(Outbound::One(ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(m))) => m,
            _ => panic!("expected Some(Outbound::One(SMSG_FORCE_RUN_SPEED_CHANGE))"),
        };
        assert_eq!(*m, codec::build_force_run_speed(self_guid, 10.5));
        // A non-move-speed change on self spams nothing (ordinary buffs/debuffs don't trigger this).
        let unrelated = aura(self_guid, 0xA1, 1, 10, 0, i64::MAX);
        assert!(run_speed_packet(current.clone().into_iter(), &unrelated, self_guid).is_none());
        // A move-speed change on a DIFFERENT guid (a peer) never triggers self's packet.
        let peer_changed = aura(self_guid + 1, 0xA4, 0, 25, 0, i64::MAX);
        assert!(run_speed_packet(current.into_iter(), &peer_changed, self_guid).is_none());
    }

    #[test]
    fn run_speed_packet_refreshes_always_and_not_stack_bonuses_after_removal() {
        use lyracore_shared::spell::taxonomy::{SPEED_MOVE_ALWAYS, SPEED_MOVE_NOT_STACK};
        let guid = 7;
        for speed_kind in [SPEED_MOVE_ALWAYS, SPEED_MOVE_NOT_STACK] {
            let changed = aura(guid, A_MOD_SPEED, i32::from(speed_kind), 50, 0, i64::MAX);
            for (current, expected) in [(vec![changed.clone()], 10.5), (vec![], 7.0)] {
                let Some(Outbound::One(ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(packet))) =
                    run_speed_packet(current.into_iter(), &changed, guid)
                else {
                    panic!("run speed packet");
                };
                assert_eq!(*packet, codec::build_force_run_speed(guid, expected));
            }
        }
    }

    #[test]
    fn swim_speed_packet_updates_and_restores_the_owner_speed() {
        let guid = 7;
        let ghost = aura(guid, A_MOD_SPEED, i32::from(SPEED_SWIM), 25, 0, i64::MAX);
        let wisp = aura(guid, A_MOD_SPEED, i32::from(SPEED_SWIM), 50, 0, i64::MAX);
        let base = lyracore_shared::constants::speeds::SWIM;
        for (current, expected) in [
            (vec![ghost.clone(), wisp.clone()], base * 1.5),
            (vec![ghost.clone()], base * 1.25),
            (vec![], base),
        ] {
            let Some(Outbound::One(ServerOpcodeMessage::SMSG_FORCE_SWIM_SPEED_CHANGE(packet))) =
                swim_speed_packet(current.into_iter(), &wisp, guid)
            else {
                panic!("swim speed packet");
            };
            assert_eq!(packet.guid.guid(), guid);
            assert_eq!(packet.move_event, 0);
            assert!((packet.speed - expected).abs() < 0.000001);
        }
        assert!(swim_speed_packet([ghost.clone()].into_iter(), &ghost, guid + 1).is_none());
        let cast = aura(guid, A_MOD_SPEED, 2, 50, 0, i64::MAX);
        assert!(swim_speed_packet([cast.clone()].into_iter(), &cast, guid).is_none());
    }
    #[test]
    fn mounted_snare_apply_and_remove_preserve_the_mount_bonus() {
        use lyracore_shared::spell::taxonomy::{A_MOUNTED, SPEED_MOUNTED};
        let rider = 7;
        let mounted = aura(rider, A_MOUNTED, 1147, 0, 0, i64::MAX);
        let bonus = aura(
            rider,
            A_MOD_SPEED,
            i32::from(SPEED_MOUNTED),
            60,
            0,
            i64::MAX,
        );
        let snare = aura(rider, A_MOD_SPEED, i32::from(SPEED_MOVE), -30, 0, i64::MAX);
        for (current, expected) in [
            (vec![mounted.clone(), bonus.clone(), snare.clone()], 9.1),
            (vec![mounted.clone(), bonus.clone()], 11.2),
            (vec![bonus.clone(), snare.clone()], 4.9),
        ] {
            let Some(Outbound::One(ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(packet))) =
                run_speed_packet(current.into_iter(), &snare, rider)
            else {
                panic!("speed packet");
            };
            assert_eq!(*packet, codec::build_force_run_speed(rider, expected));
        }
    }

    #[test]
    fn stacked_snare_packet_uses_the_ten_percent_floor() {
        let mut snare = aura(7, A_MOD_SPEED, i32::from(SPEED_MOVE), -30, 0, i64::MAX);
        snare.stacks = 4;
        let Some(Outbound::One(ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(packet))) =
            run_speed_packet([snare.clone()].into_iter(), &snare, 7)
        else {
            panic!("speed packet");
        };
        assert_eq!(*packet, codec::build_force_run_speed(7, 0.7));
    }
}
