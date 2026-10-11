//! Table Relays for this Protocol Family.

use crate::codec::combat::subscriptions::aura_sync;
use crate::codec::guild::subscriptions::guild_projection_of;
use crate::codec::movement::subscriptions::append_resident_creature_after_create;
use crate::codec::movement::subscriptions::unix_now_micros;
use crate::codec::taxi::subscriptions::append_resident_taxi_after_create;
use crate::codec::weather::subscriptions::zone_crossed;
use crate::codec::{self, CreateKind};
use crate::stdb::bindings::*;
use crate::stdb::views::{entity_view, hunter_pet_protocol_view};
use crate::stdb::world_index::{CellKey, EntityLayer, SessionId, ShardId};
use crate::stdb::world_view::*;
use crate::stdb::Coordinator;
use crate::world::Outbound;
use anyhow::Result;
use lyracore_shared::spatial::BOX_HALF_SPAN;
use spacetimedb_sdk::Table;
use std::collections::HashSet;
use std::sync::Arc;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

/// Read a peer's equipment and current aura state for AOI entry or stealth reveal.
/// An encoding failure leaves the caller responsible for removing its `created` entry.
pub(crate) fn build_peer_create(
    coord: &Coordinator,
    world: &WorldView,
    shard: crate::stdb::world_index::ShardId,
    viewer_guid: u64,
    row: &WorldEntity,
) -> Option<Vec<Outbound>> {
    let inv: Vec<codec::ItemInstanceView> =
        if row.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0 {
            coord
                .player_items(row.guid)
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.slot <= 18)
                .collect()
        } else {
            Vec::new()
        };
    let view = peer_entity_view(
        row,
        projected_dynamic_flags(&coord.0.coord().conn.db, viewer_guid, row),
        |guid| guild_projection_of(coord, guid),
    );
    let auras = world.auras.on_target(shard, row.guid);
    let movement = coord
        .0
        .coord()
        .conn
        .db
        .game_entity_motion()
        .guid()
        .find(&row.guid)
        .filter(|motion| motion.map_id == row.map_id && motion.instance_id == row.instance_id)
        .filter(|motion| {
            codec::bytes_to_movement_info(&motion.movement_info).is_ok_and(|info| {
                info.timestamp == row.last_move_ms
                    && info.position.x == row.x
                    && info.position.y == row.y
                    && info.position.z == row.z
                    && info.orientation == row.orientation
            })
        });
    match peer_create_outbound(
        &view,
        &inv,
        &auras,
        movement
            .as_ref()
            .map(|motion| motion.movement_info.as_slice()),
    ) {
        Ok(out) => Some(out),
        Err(e) => {
            log::warn!("peer create encode failed for guid {}: {e}", row.guid);
            None
        }
    }
}

/// The view a peer CREATE encodes: the stored row, the viewer-relative dynamic flags, and for a
/// player the Guild Projection `guild_of` reads.
pub(crate) fn peer_entity_view(
    row: &WorldEntity,
    dynamic_flags: u32,
    guild_of: impl FnOnce(u64) -> (u32, u32),
) -> codec::EntityView {
    let mut view = entity_view(row.clone(), 0);
    view.dynamic_flags = dynamic_flags;
    if row.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0 {
        (view.guild_id, view.guild_rank) = guild_of(row.guid);
    }
    view
}

/// A peer's current auras follow its CREATE. Their rows may have arrived before the viewer
/// could see it, so another aura callback cannot be relied on to supply them.
pub(crate) fn peer_create_outbound(
    view: &codec::EntityView,
    inventory: &[codec::ItemInstanceView],
    auras: &[Aura],
    movement: Option<&[u8]>,
) -> Result<Vec<Outbound>> {
    use lyracore_shared::constants::speeds;

    let speed_auras = auras
        .iter()
        .filter(|aura| aura.target_guid == view.guid)
        .map(|aura| lyracore_shared::aura::AuraAmount {
            kind: aura.eff_kind,
            parameter: aura.eff_p0,
            amount: aura.amount,
            stacks: aura.stacks,
        });
    let mut create = codec::build_create_object(view, CreateKind::Peer, inventory, &[])?;
    for object in &mut create.objects {
        if let wow_world_messages::vanilla::Object::CreateObject2 { movement2, .. } = object {
            if let Some(mut living) = movement2.update_flag.get_living().cloned() {
                if let wow_world_messages::vanilla::MovementBlock_UpdateFlag_Living::Living {
                    running_speed,
                    swimming_speed,
                    ..
                } = &mut living
                {
                    if speed_auras.clone().any(|aura| {
                        lyracore_shared::aura::moves_run_speed(aura.kind, aura.parameter)
                    }) {
                        *running_speed =
                            lyracore_shared::aura::run_speed(speeds::RUN, speed_auras.clone());
                    }
                    *swimming_speed =
                        lyracore_shared::aura::swim_speed(speeds::SWIM, speed_auras.clone());
                }
                movement2.update_flag = movement2.update_flag.clone().set_living(living);
            }
        }
    }
    let created = match movement {
        Some(movement) => Outbound::Raw {
            opcode: 0x00a9,
            body: codec::create_with_movement(&create, movement)?,
        },
        None => Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(create))),
    };
    let mut out = vec![created];
    if !auras.is_empty() {
        out.push(aura_sync(auras.iter().cloned(), view.guid));
    }
    Ok(out)
}

struct LootTagProjectionRows<'a> {
    tap: Option<&'a CreatureQuestTap>,
    tag_group: Option<&'a CreatureLootTagGroup>,
    tag_members: &'a [CreatureQuestTapMember],
    current_members: &'a [GroupMember],
    corpse_eligible: &'a [CorpseLootEligible],
}

/// Render the stored entity flags for one viewer without changing the durable row.
fn viewer_relative_dynamic_flags(
    viewer_guid: u64,
    entity: &WorldEntity,
    rows: LootTagProjectionRows<'_>,
) -> u32 {
    use lyracore_shared::constants::unit_dynamic_flags::{LOOTABLE, TAPPED_BY_PLAYER};

    if entity.dead {
        return if rows
            .corpse_eligible
            .iter()
            .any(|row| row.corpse_guid == entity.guid && row.eligible_guid == viewer_guid)
        {
            entity.dynamic_flags
        } else {
            entity.dynamic_flags & !LOOTABLE
        };
    }

    let Some(tap) = rows.tap.filter(|tap| tap.creature_guid == entity.guid) else {
        return entity.dynamic_flags;
    };
    let entitled = match rows
        .tag_group
        .filter(|group| group.creature_guid == entity.guid)
    {
        Some(group) => {
            rows.tag_members.iter().any(|member| {
                member.creature_guid == entity.guid && member.character_guid == viewer_guid
            }) && rows.current_members.iter().any(|member| {
                member.character_guid == viewer_guid && member.group_id == group.group_id
            })
        }
        None => viewer_guid == tap.character_guid,
    };
    if entitled {
        entity.dynamic_flags | TAPPED_BY_PLAYER
    } else {
        entity.dynamic_flags & !TAPPED_BY_PLAYER
    }
}

pub(crate) fn projected_dynamic_flags(
    db: &RemoteTables,
    viewer_guid: u64,
    entity: &WorldEntity,
) -> u32 {
    let tap = db
        .game_creature_quest_tap()
        .creature_guid()
        .find(&entity.guid);
    let tag_group = db
        .game_creature_loot_tag_group()
        .creature_guid()
        .find(&entity.guid);
    let tag_members: Vec<_> = db
        .game_creature_quest_tap_member()
        .iter()
        .filter(|member| member.creature_guid == entity.guid)
        .collect();
    let current_members: Vec<_> = db
        .game_group_member()
        .iter()
        .filter(|member| member.character_guid == viewer_guid)
        .collect();
    let corpse_eligible: Vec<_> = db
        .game_corpse_loot_eligible()
        .iter()
        .filter(|eligible| eligible.corpse_guid == entity.guid)
        .collect();
    viewer_relative_dynamic_flags(
        viewer_guid,
        entity,
        LootTagProjectionRows {
            tap: tap.as_ref(),
            tag_group: tag_group.as_ref(),
            tag_members: &tag_members,
            current_members: &current_members,
            corpse_eligible: &corpse_eligible,
        },
    )
}

/// May an instance-tagged CORPSE/GAMEOBJECT row be CREATE-relayed to this
/// viewer? Instance equality against a KNOWN viewer instance — `None` (viewer entity not resident,
/// see `viewer_instance`) relays nothing: suppress-then-sweep is strictly safer than the old
/// default-to-0 (which leaked instance-0 rows to a viewer whose real instance wasn't known yet).
/// Kept map-agnostic deliberately: instance-0 rows keep today's relay behavior byte-identical for
/// every resident open-world viewer, while a per-instance copy (or an in-instance corpse) reaches
/// ONLY viewers inside that instance. DELETE relays stay ungated (the `on_melee_delete` precedent —
/// SMSG_DESTROY_OBJECT for a never-created guid is a client no-op, and gating deletes risks
/// leaking a stale object on an instance transition). Pure — unit-tested below.
pub(crate) fn instance_relay_gate(row_instance_id: u64, viewer_instance_id: Option<u64>) -> bool {
    viewer_instance_id == Some(row_instance_id)
}

pub(crate) fn peer_create_gate(
    row_guid: u64,
    self_guid: u64,
    row_instance_id: u64,
    viewer_instance_id: u64,
    row_npc_flags: u32,
    viewer_is_ghost: bool,
    row_is_stealthed: bool,
) -> bool {
    const SPIRITHEALER_NPC_FLAG: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER; // 0x20
    if row_guid == self_guid {
        return false;
    }
    if row_instance_id != viewer_instance_id {
        return false;
    }
    if row_npc_flags & SPIRITHEALER_NPC_FLAG != 0 && !viewer_is_ghost {
        return false;
    }
    if row_is_stealthed {
        return false;
    }
    true
}

#[must_use]
pub(crate) fn offer_peer_create_for(
    coord: &Coordinator,
    view: &WorldView,
    shard: crate::stdb::world_index::ShardId,
    viewer: &Viewer,
    row: &WorldEntity,
) -> Vec<Outbound> {
    const SPIRITHEALER_NPC_FLAG: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER; // 0x20
    if row.guid == viewer.self_guid || row.instance_id != viewer.instance_id {
        return Vec::new();
    }
    let viewer_is_ghost = viewer.gates.is_ghost();
    // A spirit healer viewed by a living player is refused regardless of stealth.
    if row.npc_flags & SPIRITHEALER_NPC_FLAG != 0 && !viewer_is_ghost {
        return Vec::new();
    }
    let row_is_stealthed = view.auras.is_stealthed(shard, row.guid);
    let guard = coord.0.coord();
    let db = &guard.conn.db;
    if !peer_create_gate(
        row.guid,
        viewer.self_guid,
        row.instance_id,
        viewer.instance_id,
        row.npc_flags,
        viewer_is_ghost,
        row_is_stealthed,
    ) {
        return Vec::new();
    }
    if !viewer.created.lock().unwrap().insert(row.guid) {
        return Vec::new();
    }
    let Some(mut out) = build_peer_create(coord, view, shard, viewer.self_guid, row) else {
        viewer.created.lock().unwrap().remove(&row.guid);
        return Vec::new();
    };
    if let Some(equipment) = db.game_encounter_equip().creature_guid().find(&row.guid) {
        append_encounter_equip_after_create(&mut out, viewer, &equipment);
    }
    // Pet action bar: this viewer's OWN summoned pet just appeared — bind it client-side
    // (UNIT_FIELD_SUMMON + the pet action bar).
    if row.owner_guid == viewer.self_guid {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_owner_summon_values(viewer.self_guid, row.guid)),
        )));
        if let Some(pet) = db
            .game_hunter_pet_protocol()
            .iter()
            .find(|pet| pet.live_pet_guid == row.guid && pet.owner_guid == viewer.self_guid)
        {
            out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(codec::build_hunter_pet_values(&hunter_pet_protocol_view(
                    pet,
                ))),
            )));
        }
        let spells: Vec<u32> = db
            .game_creature_cast()
            .iter()
            .filter(|c| c.creature_entry == row.entry)
            .map(|c| c.spell_id)
            .collect();
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_PET_SPELLS(
            Box::new(codec::build_pet_spells(row.guid, &spells)),
        )));
    }
    let resident_creature_spline = db.game_creature_spline().guid().find(&row.guid);
    append_resident_creature_after_create(
        &mut out,
        &viewer.created,
        viewer.instance_id,
        resident_creature_spline.as_ref(),
        unix_now_micros(),
    );
    if let Some(spline) = db
        .game_taxi_passenger_spline()
        .character_guid()
        .find(&row.guid)
    {
        append_resident_taxi_after_create(&mut out, &viewer.created, viewer.self_guid, &spline);
    }
    out
}

/// Relay one durable creature virtual-item projection to a viewer that already holds the creature.
/// A delete clears all three display slots. The raw builder is the crash-safe sparse VALUES path.
pub(crate) fn encounter_equip_outbound(
    viewer: &Viewer,
    row: &EncounterEquip,
    cleared: bool,
) -> Vec<Outbound> {
    if !viewer.created.lock().unwrap().contains(&row.creature_guid) {
        return Vec::new();
    }
    let (main_hand, off_hand, ranged) = if cleared {
        (0, 0, 0)
    } else {
        (row.main_hand, row.off_hand, row.ranged)
    };
    let (opcode, body) =
        codec::build_virtual_item_values(row.creature_guid, main_hand, off_hand, ranged);
    vec![Outbound::Raw { opcode, body }]
}

pub(crate) fn append_encounter_equip_after_create(
    out: &mut Vec<Outbound>,
    viewer: &Viewer,
    row: &EncounterEquip,
) {
    out.extend(encounter_equip_outbound(viewer, row, false));
}

#[must_use]
pub(crate) fn relay_entity_update(
    coord: &Coordinator,
    view: &WorldView,
    shard: crate::stdb::world_index::ShardId,
    viewer: &Viewer,
    old: &WorldEntity,
    new: &WorldEntity,
) -> Vec<Outbound> {
    const SPIRITHEALER_NPC_FLAG: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER;
    const GHOST_PLAYER_FLAG: u32 = lyracore_shared::constants::player_flags::GHOST;
    if new.guid != viewer.self_guid {
        let shown = viewer.created.lock().unwrap().contains(&new.guid);
        if is_update_reentry(new.guid, viewer.self_guid, shown) {
            return offer_peer_create_for(coord, view, shard, viewer, new);
        }
    }
    let dynamic_flags = {
        let guard = coord.0.coord();
        projected_dynamic_flags(&guard.conn.db, viewer.self_guid, new)
    };
    let mut out: Vec<Outbound> =
        entity_update_to_outbound_with_dynamic_flags(old, new, dynamic_flags)
            .into_iter()
            .map(Outbound::One)
            .collect();
    if new.guid == viewer.self_guid {
        if let Some(is_ghost) =
            ghost_transition(old.player_flags, new.player_flags, GHOST_PLAYER_FLAG)
        {
            viewer
                .gates
                .is_ghost
                .store(is_ghost, std::sync::atomic::Ordering::Relaxed);
            // Every spirit healer currently IN VIEW (the index answers that; before the
            // shared-connection model this scanned the AOI-scoped connection cache, which was the
            // same set). Idempotent through
            // `created`, exactly like the stealth Reveal/Hide arms.
            let guard = coord.0.coord();
            let healers: Vec<WorldEntity> = guard
                .conn
                .db
                .game_world_entity()
                .iter()
                .filter(|e| {
                    e.npc_flags & SPIRITHEALER_NPC_FLAG != 0 && e.instance_id == new.instance_id
                })
                .collect();
            drop(guard);
            for h in healers {
                if is_ghost {
                    out.extend(offer_peer_create_for(coord, view, shard, viewer, &h));
                } else if viewer.created.lock().unwrap().remove(&h.guid) {
                    out.push(Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                        codec::build_destroy_object(h.guid),
                    )));
                }
            }
        }
    }
    out
}

/// Re-send the projected flags for visible live Loot Tags after the shard membership mirror
/// changes. Membership does not rewrite a creature's `WorldEntity`, so its normal VALUES relay has
/// nothing to carry.
pub(crate) fn loot_tag_flags_after_membership_change(
    coord: &Coordinator,
    viewer: &Viewer,
) -> Vec<Outbound> {
    use lyracore_shared::constants::unit_dynamic_flags::TAPPED;

    let shown = viewer.created.lock().unwrap().clone();
    let guard = coord.0.coord();
    shown
        .iter()
        .filter_map(|guid| guard.conn.db.game_world_entity().guid().find(guid))
        .filter(|entity| !entity.dead && entity.dynamic_flags & TAPPED != 0)
        .map(|entity| {
            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_dynamic_flags_values(
                    entity.guid,
                    projected_dynamic_flags(&guard.conn.db, viewer.self_guid, &entity),
                ),
            )))
        })
        .collect()
}

#[must_use]
pub(crate) fn relay_peer_destroy(viewer: &Viewer, guid: u64, owner_guid: u64) -> Vec<Outbound> {
    if guid == viewer.self_guid {
        return Vec::new();
    }
    if !viewer.created.lock().unwrap().remove(&guid) {
        return Vec::new();
    }
    let mut out = vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
        codec::build_destroy_object(guid),
    ))];
    // Pet action bar: this viewer's own pet despawned — clear UNIT_FIELD_SUMMON and send the empty
    // SMSG_PET_SPELLS (the vanilla "remove pet bar" form).
    if owner_guid == viewer.self_guid {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_owner_summon_values(viewer.self_guid, 0)),
        )));
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_PET_SPELLS(
            Box::new(codec::build_pet_spells_clear()),
        )));
    }
    out
}

/// The `on_update` dispatch decision: does this update need to be treated as a
/// re-entry (offer a fresh CREATE via `offer_peer_create`) instead of a field diff? True exactly when
/// the guid isn't self AND isn't already in the viewer's `created` set — i.e. either this is the first
/// time this viewer has seen the row, or it left scope (evicted by `on_delete`) and is now returning as
/// an UPDATE of the SDK's still-cached row instead of a fresh `on_insert`. Pure over the dedup-set
/// membership so the branch is unit-testable without a live SDK connection.
pub(crate) fn is_update_reentry(guid: u64, self_guid: u64, currently_shown: bool) -> bool {
    guid != self_guid && !currently_shown
}

/// Edge-detect for the spirit-healer reveal: did the GHOST player-flag (mask) cross 0↔1 between `old`
/// and `new`? Returns `Some(true)` on an alive→ghost crossing (REVEAL the in-scope spirit healers),
/// `Some(false)` on ghost→alive (DESTROY them), `None` when the bit didn't change (the common heartbeat /
/// any other field update — no reveal work). Pure so the on_update closure stays a thin relay and this
/// stays unit-testable (the closure itself needs a live EventContext to enumerate healers).
pub(crate) fn ghost_transition(old_flags: u32, new_flags: u32, ghost_mask: u32) -> Option<bool> {
    let was = old_flags & ghost_mask != 0;
    let is = new_flags & ghost_mask != 0;
    if was == is {
        None
    } else {
        Some(is)
    }
}

/// Diff an observed `game_world_entity` row (`old` → `new`) into the partial-VALUES `SMSG_UPDATE_OBJECT`
/// packets that replicate the change to this player's client. Pure (no I/O) so it is unit-testable — the
/// `on_update` relay callback just sends whatever this returns, in order. The level-up branch is
/// MUTUALLY EXCLUSIVE with the individual field diffs: a ding pushes ONE combined VALUES block (level +
/// vitals + xp together) instead of the separate health/xp packets, so the panel + bars flip in lockstep.
/// Player-only fields (xp / coinage / ghost / power) are gated on the PLAYER bit; health and dynamic-flags
/// apply to any unit (creatures' health bars + loot sparkle). Order is preserved from the original relay.
#[cfg(test)]
pub(crate) fn entity_update_to_outbound(
    old: &WorldEntity,
    new: &WorldEntity,
) -> Vec<ServerOpcodeMessage> {
    entity_update_to_outbound_with_dynamic_flags(old, new, new.dynamic_flags)
}

pub(crate) fn entity_update_to_outbound_with_dynamic_flags(
    old: &WorldEntity,
    new: &WorldEntity,
    dynamic_flags: u32,
) -> Vec<ServerOpcodeMessage> {
    let mut out = Vec::new();
    let is_player = new.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0;
    if is_player && old.level != new.level {
        // Level-up ding: level + maxhealth + health + xp in ONE VALUES block (the panel + health/XP bars
        // flip together). SMSG_LEVELUP_INFO (relayed separately) is the cosmetic popup only.
        let m = codec::build_levelup_values(
            new.guid,
            new.level,
            new.health,
            new.max_health,
            new.xp,
            new.next_level_xp,
            (new.unit_bytes_0 >> 24) as u8,
            new.max_power,
        );
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    } else {
        if old.health != new.health {
            let m = codec::build_health_values(new.guid, new.health);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Dynamic-flags VALUES relay: the killing blow sets UNIT_DYNFLAG_LOOTABLE (0x1) on a corpse that
        // rolled money; the loot reducer clears it. NOTE: UNIT_DYNFLAG_DEAD (0x20) is NEVER set
        // (it is feign-death in vanilla — see combat + lyracore-shared constants).
        if old.dynamic_flags != new.dynamic_flags {
            let m = codec::build_dynamic_flags_values(new.guid, dynamic_flags);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Target relay (UNIT_FIELD_TARGET): observers see who this unit selected — the target ring /
        // target-of-target / assist. Any unit (a creature's aggro target relays too), not player-gated.
        if old.target_guid != new.target_guid {
            let m = codec::build_target_values(new.guid, new.target_guid);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Max-vitals relay: a non-level-up max_health/max_power change (+Stamina/+Intellect gear or a
        // Fortitude/Mark aura) moves the bar DENOMINATOR live for observers. Any unit.
        if old.max_health != new.max_health || old.max_power != new.max_power {
            let power_b = (new.unit_bytes_0 >> 24) as u8;
            let m =
                codec::build_max_vitals_values(new.guid, new.max_health, power_b, new.max_power);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Taxi presentation changes mount display + the TAXI_FLIGHT unit flag in one mask (activation
        // and landing set both atomically), so the client never sees an intermediate
        // mounted-but-controllable or unmounted-but-in-flight state — detected by the flight bit itself
        // toggling, not just "unit_flags changed", so an unrelated flag change alongside a land mount
        // doesn't get mistaken for a taxi transition. A LAND mount/dismount changes `mount_display_id`
        // ALONE (the flight bit is untouched) and relays through the standalone single-field builder
        // instead, never alongside the coupled one for the same change.
        let taxi_flight_toggled = (old.unit_flags ^ new.unit_flags)
            & lyracore_shared::constants::unit_flags::TAXI_FLIGHT
            != 0;
        if old.mount_display_id != new.mount_display_id && taxi_flight_toggled {
            let m = codec::build_taxi_presentation_values(
                new.guid,
                new.mount_display_id,
                new.unit_flags,
            );
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        } else {
            if old.mount_display_id != new.mount_display_id {
                let m = codec::build_mount_display_values(new.guid, new.mount_display_id);
                out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
            }
            if old.unit_flags != new.unit_flags {
                let m = codec::build_unit_flags_values(new.guid, new.unit_flags);
                out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
            }
        }
        // Sheath relay: UNIT_FIELD_BYTES_2 byte 0 flipping as a unit draws or stows its weapon.
        // Any unit, not player-gated — a creature drawing on engage is the same wire field. Without
        // this arm the server knows the state and no observer ever hears about it.
        if old.unit_bytes_2 != new.unit_bytes_2 {
            let m = codec::build_sheath_values(new.guid, new.unit_bytes_2);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Live XP-bar update: players only (creatures never change xp; player-only fields).
        let xp_changed = old.xp != new.xp || old.next_level_xp != new.next_level_xp;
        if xp_changed && is_player {
            let m = codec::build_player_xp_values(new.guid, new.xp, new.next_level_xp);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Live purse update: PLAYER_FIELD_COINAGE on money change (looting). Players only.
        if old.money != new.money && is_player {
            let m = codec::build_coinage_values(new.guid, new.money);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Live bank bag slot count: PLAYER_BYTES_2 byte 2 on a purchase, so the bank window grows a
        // slot without a relog. Scoped to byte 2 only — byte 3 (rest state) already relays through
        // its own dedicated `game_rest_state_event` path and must not double-fire here.
        const BANK_SLOT_BYTE: u32 = 0x00FF_0000;
        if is_player
            && (old.player_bytes_2 & BANK_SLOT_BYTE) != (new.player_bytes_2 & BANK_SLOT_BYTE)
        {
            let m = codec::build_bank_bag_slots_values(new.guid, new.player_bytes_2);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // PLAYER_FLAGS and the UNIT_FIELD_BYTES_1 vis bit: the ghost transition on Release Spirit
        // and reclaim, and the AFK and DND bits of an Away Status. Players only.
        if is_player
            && (old.player_flags != new.player_flags || old.unit_bytes_1 != new.unit_bytes_1)
        {
            let m = codec::build_player_flags_values(new.guid, new.player_flags, new.unit_bytes_1);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Live power update: rage/energy/mana changing in combat. The power-type byte of unit_bytes_0 picks
        // the right UNIT_FIELD_POWER_N. Players only — else the rage bar stays at 0 and rage spells grey out.
        if is_player && old.power != new.power {
            let power_b = (new.unit_bytes_0 >> 24) as u8;
            let m = codec::build_power_values(new.guid, power_b, new.power);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // GM playtest `.speed`: a `run_speed_mult_bp` change relays
        // SMSG_FORCE_RUN_SPEED_CHANGE — player movement is client-authoritative, so the server-side
        // field alone never speeds the client up. Mirrors the existing `A_MOD_SPEED(MOVE)` aura relay
        // (`run_speed_packet`, same `BASE_RUN_SPEED` constant) but keyed off THIS field, any unit (not
        // player-gated — a GM could in principle god/speed-tag a creature down the line, and the relay
        // is inert for anyone not watching this guid).
        if old.run_speed_mult_bp != new.run_speed_mult_bp {
            const BASE_RUN_SPEED: f32 = 7.0; // vanilla base run yd/s — same constant as `run_speed_packet`
            let speed = BASE_RUN_SPEED * (new.run_speed_mult_bp as f32 / 10_000.0);
            out.push(ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(Box::new(
                codec::build_force_run_speed(new.guid, speed),
            )));
        }
    }
    if old.display_id != new.display_id {
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_display_values(new.guid, new.display_id),
        )));
    }
    out
}

/// A row's cell, taken from the grid columns the WRITER stamped — not recomputed from `x`/`y`.
///
/// The subscription predicate this replaces filtered on `grid_x`/`grid_y`, so reading the same
/// columns is what makes the index and the deleted SQL agree by construction rather than by both
/// happening to call `grid_cell` the same way.
pub(crate) fn entity_key(row: &WorldEntity) -> CellKey {
    CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y)
}

/// A row entered the world (or this shard). Index it, then offer a CREATE to everyone who can see
/// its cell.
pub(crate) fn entity_appeared(view: &Arc<WorldView>, shard: ShardId, row: &WorldEntity) {
    let key = entity_key(row);
    view.spatial.upsert_entity(
        EntityLayer::WorldEntity,
        row.guid,
        key,
        shard,
        row.owner_guid,
    );
    let row = Arc::new(row.clone());
    for session in view.world_entity_recipients(shard, row.guid, key) {
        offer_create_job(view, shard, session, &row);
    }
}

/// Create or update for the new cell's viewers, destroy for viewers left behind,
/// and leave all other viewers untouched.
pub(crate) fn entity_changed(
    view: &Arc<WorldView>,
    shard: ShardId,
    old: &WorldEntity,
    new: &WorldEntity,
) {
    let key = entity_key(new);
    let previous = view.spatial.upsert_entity(
        EntityLayer::WorldEntity,
        new.guid,
        key,
        shard,
        new.owner_guid,
    );
    let new_recipients = view.world_entity_recipients(shard, new.guid, key);
    if let Some(old_key) = previous {
        if old_key != key {
            let still: HashSet<SessionId> = new_recipients.iter().copied().collect();
            for viewer in view.cell_audience(shard, Some(old_key), BOX_HALF_SPAN, &[]) {
                if !still.contains(&viewer.session) {
                    destroy_job(view, viewer.session, new.guid, new.owner_guid);
                }
            }
        }
    }
    let old = Arc::new(old.clone());
    let new = Arc::new(new.clone());
    for session in new_recipients {
        update_job(view, shard, session, &old, &new);
    }
}

/// The row is gone from the database. Everyone who could see it gets a DESTROY.
pub(crate) fn entity_vanished(view: &WorldView, shard: ShardId, guid: u64, owner_guid: u64) {
    let Some((key, _)) = view
        .spatial
        .remove_entity(EntityLayer::WorldEntity, guid, shard)
    else {
        return;
    };
    for session in view.world_entity_recipients(shard, guid, key) {
        destroy_job(view, session, guid, owner_guid);
    }
}

pub(crate) fn encounter_equip_changed(
    view: &WorldView,
    shard: ShardId,
    row: &EncounterEquip,
    cleared: bool,
) {
    let Some(coord) = view.shard(shard) else {
        return;
    };
    let Some(entity) = coord
        .0
        .coord()
        .conn
        .db
        .game_world_entity()
        .guid()
        .find(&row.creature_guid)
    else {
        return;
    };
    let key = entity_key(&entity);
    let row = Arc::new(row.clone());
    for session in view.world_entity_recipients(shard, row.creature_guid, key) {
        let Some(viewer) = view.viewer(session) else {
            continue;
        };
        let row = row.clone();
        enqueue(viewer.clone(), move |viewer| {
            crate::codec::entity::subscriptions::encounter_equip_outbound(&viewer, &row, cleared)
        });
    }
}

pub(crate) fn offer_create_job(
    view: &Arc<WorldView>,
    shard: ShardId,
    session: SessionId,
    row: &Arc<WorldEntity>,
) {
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    let Some(coord) = view.shard(shard) else {
        return;
    };
    let (view, row) = (view.clone(), row.clone());
    enqueue(viewer.clone(), move |viewer| {
        crate::codec::entity::subscriptions::offer_peer_create_for(
            &coord, &view, shard, &viewer, &row,
        )
    });
}

pub(crate) fn update_job(
    view: &Arc<WorldView>,
    shard: ShardId,
    session: SessionId,
    old: &Arc<WorldEntity>,
    new: &Arc<WorldEntity>,
) {
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    let Some(coord) = view.shard(shard) else {
        return;
    };
    let (view, old, new) = (view.clone(), old.clone(), new.clone());
    enqueue(viewer.clone(), move |viewer| {
        crate::codec::entity::subscriptions::relay_entity_update(
            &coord, &view, shard, &viewer, &old, &new,
        )
    });
}

pub(crate) fn destroy_job(view: &WorldView, session: SessionId, guid: u64, owner_guid: u64) {
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    enqueue(viewer.clone(), move |viewer| {
        crate::codec::entity::subscriptions::relay_peer_destroy(&viewer, guid, owner_guid)
    });
}

/// A standing row is delivered only to its owner. The wire packet uses the Faction.dbc
/// ReputationListID, never the game's faction id.
pub(crate) fn reputation_appeared(view: &WorldView, shard: ShardId, row: &PlayerReputation) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.character_guid)) else {
        return;
    };
    let row = row.clone();
    enqueue(viewer.clone(), move |_| reputation_outbound(&row));
}

pub(crate) fn reputation_outbound(row: &PlayerReputation) -> Vec<Outbound> {
    if row.reputation_index < 0 {
        return Vec::new();
    }
    crate::codec::build_set_faction_standing_raw(row.reputation_index as u32, row.standing)
        .map(|(opcode, body)| vec![Outbound::Raw { opcode, body }])
        .unwrap_or_default()
}

crate::stdb::relays::declare_relays! {
    game_player_reputation [false] {
        insert(Live) => |context, _event, row| reputation_appeared(&context.view, context.shard, row);
        update(Snapshot) => |context, _event, _old, row| reputation_appeared(&context.view, context.shard, row);
    }
    game_world_entity [false] {
        insert(Snapshot) => |context, _event, row| entity_appeared(&context.view, context.shard, row);
        update(Snapshot) => |context, _event, old, new| {
                entity_changed(&context.view, context.shard, old, new);
                zone_crossed(&context.view, &context.coord, context.shard, new.guid, old.zone_id, new.zone_id);
            };
        delete(Snapshot) => |context, _event, row| entity_vanished(&context.view, context.shard, row.guid, row.owner_guid);
    }
    game_encounter_equip [false] {
        insert(Snapshot) => |context, _event, row| encounter_equip_changed(&context.view, context.shard, row, false);
        update(Snapshot) => |context, _event, _old, row| encounter_equip_changed(&context.view, context.shard, row, false);
        delete(Snapshot) => |context, _event, row| encounter_equip_changed(&context.view, context.shard, row, true);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    /// A live-entity row with no pending change; tests clone it and mutate one field at a time.
    /// `type_mask` carries the PLAYER bit by default (most relayed fields are player-only).
    pub(crate) fn player_entity() -> WorldEntity {
        WorldEntity {
            guid: 1,
            owner_identity: spacetimedb_sdk::Identity::from_byte_array([0u8; 32]),
            account_id: 1,
            map_id: 0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            grid_x: 0,
            grid_y: 0,
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
            last_move_ms: 0,
            type_mask: lyracore_shared::constants::type_mask::PLAYER_BIT,
            entry: 0,
            scale_x: 1.0,
            health: 100,
            max_health: 100,
            power: 100,
            max_power: 100,
            level: 1,
            faction_template: 0,
            unit_bytes_0: 0,
            display_id: 0,
            native_display_id: 0,
            unit_flags: 0,
            base_attack_time_ms: 2000,
            dynamic_flags: 0,
            dead: false,
            player_bytes: 0,
            player_bytes_2: 0,
            player_bytes_3: 0,
            player_flags: 0,
            xp: 0,
            next_level_xp: 100,
            target_guid: 0,
            money: 0,
            unit_bytes_1: 0,
            unit_bytes_2: 0,
            strength: 1,
            agility: 1,
            stamina: 1,
            intellect: 1,
            spirit: 1,
            npc_flags: 0,
            armor: 0,
            leg_ends_ms: 0,
            wp_target: 0,
            movement_flags: 0,
            combat_until_ms: 0,
            pickpocketed: false,
            next_swing_spell: 0,
            overpower_until_ms: 0,
            revenge_until_ms: 0,
            stance: 0,
            owner_guid: 0,
            skinned: false,
            mana_regen_paused_until_ms: 0,
            death_expire_micros: 0,
            instance_id: 0,
            run_speed_mult_bp: 10_000,
            godmode: false,
            resting: false,
            sheet_str_bonus: 0,
            sheet_agi_bonus: 0,
            sheet_sta_bonus: 0,
            sheet_int_bonus: 0,
            sheet_spi_bonus: 0,
            sheet_ap_base: 0,
            sheet_ap_mods: 0,
            sheet_dmg_min: 0,
            sheet_dmg_max: 0,
            sheet_crit_bp: 0,
            bank_bag_slots: 0,
            mount_display_id: 0,
            zone_id: 0,
            sheet_ranged_ap: 0,
            sheet_ranged_dmg_min: 0,
            sheet_ranged_dmg_max: 0,
        }
    }

    pub(crate) fn creature_entity() -> WorldEntity {
        let mut entity = player_entity();
        entity.guid = 99;
        entity.type_mask = lyracore_shared::constants::type_mask::CREATURE;
        entity.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::TAPPED;
        entity
    }

    fn tap(character_guid: u64) -> CreatureQuestTap {
        CreatureQuestTap {
            creature_guid: 99,
            character_guid,
        }
    }

    fn tag_member(character_guid: u64) -> CreatureQuestTapMember {
        CreatureQuestTapMember {
            id: character_guid,
            creature_guid: 99,
            character_guid,
        }
    }

    fn current_member(character_guid: u64, group_id: u64) -> GroupMember {
        GroupMember {
            id: character_guid,
            group_id,
            character_guid,
            owner_identity: spacetimedb_sdk::Identity::from_byte_array([0; 32]),
            raid_slot: 0,
        }
    }

    fn project_flags(
        viewer_guid: u64,
        entity: &WorldEntity,
        tap: Option<&CreatureQuestTap>,
        tag_group: Option<&CreatureLootTagGroup>,
        tag_members: &[CreatureQuestTapMember],
        current_members: &[GroupMember],
        corpse_eligible: &[CorpseLootEligible],
    ) -> u32 {
        viewer_relative_dynamic_flags(
            viewer_guid,
            entity,
            LootTagProjectionRows {
                tap,
                tag_group,
                tag_members,
                current_members,
                corpse_eligible,
            },
        )
    }

    #[test]
    fn solo_tagger_sees_tapped_by_player() {
        let entity = creature_entity();
        let tap = tap(7);
        assert_eq!(
            project_flags(7, &entity, Some(&tap), None, &[], &[], &[]),
            entity.dynamic_flags | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER
        );
    }

    #[test]
    fn snapshot_group_member_sees_tapped_by_player() {
        let entity = creature_entity();
        let tap = tap(7);
        let group = CreatureLootTagGroup {
            creature_guid: entity.guid,
            group_id: 42,
        };
        let members = [tag_member(7), tag_member(8)];
        let current = [current_member(8, 42)];
        assert_eq!(
            project_flags(
                8,
                &entity,
                Some(&tap),
                Some(&group),
                &members,
                &current,
                &[]
            ),
            entity.dynamic_flags | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER
        );
    }

    #[test]
    fn stranger_sees_only_stored_tapped_flag() {
        let entity = creature_entity();
        let tap = tap(7);
        assert_eq!(
            project_flags(8, &entity, Some(&tap), None, &[], &[], &[]),
            entity.dynamic_flags
        );
    }

    #[test]
    fn later_group_joiner_sees_only_stored_tapped_flag() {
        let entity = creature_entity();
        let tap = tap(7);
        let group = CreatureLootTagGroup {
            creature_guid: entity.guid,
            group_id: 42,
        };
        let members = [tag_member(7)];
        let current = [current_member(8, 42)];
        assert_eq!(
            project_flags(
                8,
                &entity,
                Some(&tap),
                Some(&group),
                &members,
                &current,
                &[]
            ),
            entity.dynamic_flags
        );
    }

    #[test]
    fn group_leaver_loses_tapped_by_player() {
        let entity = creature_entity();
        let tap = tap(7);
        let group = CreatureLootTagGroup {
            creature_guid: entity.guid,
            group_id: 42,
        };
        let members = [tag_member(7), tag_member(8)];
        let current = [current_member(8, 99)];
        assert_eq!(
            project_flags(
                8,
                &entity,
                Some(&tap),
                Some(&group),
                &members,
                &current,
                &[]
            ),
            entity.dynamic_flags
        );
    }

    #[test]
    fn eligible_corpse_retains_lootable() {
        let mut entity = creature_entity();
        entity.dead = true;
        entity.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
        let eligible = [CorpseLootEligible {
            id: 1,
            corpse_guid: entity.guid,
            eligible_guid: 7,
        }];
        assert_eq!(
            project_flags(7, &entity, None, None, &[], &[], &eligible),
            entity.dynamic_flags
        );
    }

    #[test]
    fn foreign_viewer_does_not_see_corpse_lootable() {
        let mut entity = creature_entity();
        entity.dead = true;
        entity.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
        let eligible = [CorpseLootEligible {
            id: 1,
            corpse_guid: entity.guid,
            eligible_guid: 7,
        }];
        assert_eq!(
            project_flags(8, &entity, None, None, &[], &[], &eligible),
            0
        );
    }

    #[test]
    fn empty_corpse_eligibility_does_not_show_lootable() {
        let mut entity = creature_entity();
        entity.dead = true;
        entity.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
        assert_eq!(project_flags(7, &entity, None, None, &[], &[], &[]), 0);
    }

    #[test]
    fn untagged_entity_flags_are_unchanged() {
        let mut entity = creature_entity();
        entity.dynamic_flags = 0x42;
        assert_eq!(project_flags(7, &entity, None, None, &[], &[], &[]), 0x42);
    }
}
