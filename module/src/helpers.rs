use crate::{game_character, game_operator, game_world_entity, Character, WorldEntity};
use spacetimedb::{Identity, ReducerContext, Table};

pub(crate) fn require_operator(ctx: &ReducerContext) -> Result<(), String> {
    match ctx.db.game_operator().id().find(0) {
        Some(op) if op.identity == ctx.sender() => Ok(()),
        Some(_) => Err("operator only".to_string()),
        None => Err("operator not claimed".to_string()),
    }
}

pub(crate) fn entity_by_owner(ctx: &ReducerContext, owner: Identity) -> Option<WorldEntity> {
    gate_by_guid(
        ctx,
        ctx.db.game_world_entity().by_owner().filter(&owner).next(),
        |e| e.guid,
    )
}

pub(crate) fn acting_entity_by_guid(ctx: &ReducerContext, guid: u64) -> Option<WorldEntity> {
    gate_by_guid(ctx, ctx.db.game_world_entity().guid().find(guid), |e| {
        e.guid
    })
}

pub(crate) fn character_by_guid(ctx: &ReducerContext, guid: u64) -> Option<Character> {
    gate_by_guid(ctx, ctx.db.game_character().guid().find(guid), |c| c.guid)
}

pub(crate) fn character_by_name(ctx: &ReducerContext, name: &str) -> Option<Character> {
    characters_by_name(ctx, name).next()
}

pub(crate) fn characters_by_name<'a>(
    ctx: &'a ReducerContext,
    name: &'a str,
) -> impl Iterator<Item = Character> + 'a {
    ctx.db
        .game_character()
        .iter()
        .filter(move |character| character.name.eq_ignore_ascii_case(name))
        .filter_map(move |character| gate_by_guid(ctx, Some(character), |character| character.guid))
}

fn gate_by_guid<T>(
    ctx: &ReducerContext,
    candidate: Option<T>,
    guid_of: impl Fn(&T) -> u64,
) -> Option<T> {
    let in_transit = candidate
        .as_ref()
        .is_some_and(|row| crate::transfer::is_in_transit(ctx, guid_of(row)));
    gate_in_transit(candidate, in_transit)
}

// Transfer-fenced Characters are absent from ordinary gameplay reads.
pub(crate) fn gate_in_transit<T>(candidate: Option<T>, in_transit: bool) -> Option<T> {
    if in_transit {
        None
    } else {
        candidate
    }
}

pub(crate) fn dist_sq(a: &WorldEntity, b: &WorldEntity) -> f32 {
    let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
    dx * dx + dy * dy + dz * dz
}

const PLAYER_INTERACTION_RANGE_SQ: f32 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlayerInteractionDenied {
    ActorDead,
    NoTarget,
    TargetNotPlayer,
    TargetDead,
    DifferentPartition,
    OutOfRange,
}

pub(crate) fn player_interaction_gate(
    actor: &WorldEntity,
    target: Option<&WorldEntity>,
) -> Result<(), PlayerInteractionDenied> {
    if actor.dead {
        return Err(PlayerInteractionDenied::ActorDead);
    }
    let Some(target) = target else {
        return Err(PlayerInteractionDenied::NoTarget);
    };
    if !target.is_player() {
        return Err(PlayerInteractionDenied::TargetNotPlayer);
    }
    if target.dead {
        return Err(PlayerInteractionDenied::TargetDead);
    }
    if !in_same_partition(target, actor.map_id, actor.instance_id) {
        return Err(PlayerInteractionDenied::DifferentPartition);
    }
    if dist_sq(actor, target) > PLAYER_INTERACTION_RANGE_SQ {
        return Err(PlayerInteractionDenied::OutOfRange);
    }
    Ok(())
}

pub(crate) fn entities_near(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    radius: f32,
) -> Vec<WorldEntity> {
    let (gx0, gx1, gy0, gy1) = lyracore_shared::spatial::covering_cell_box(x, y, radius);
    let entities = ctx.db.game_world_entity();
    let mut out = Vec::new();
    for gx in gx0..=gx1 {
        for gy in gy0..=gy1 {
            out.extend(
                entities
                    .by_grid()
                    .filter((map_id, instance_id, gx, gy))
                    .filter(|e| in_same_partition(e, map_id, instance_id)),
            );
        }
    }
    out
}

pub(crate) fn in_same_partition(e: &WorldEntity, map_id: u32, instance_id: u64) -> bool {
    e.map_id == map_id && e.instance_id == instance_id
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn nearest_entity(
    ctx: &ReducerContext,
    origin: &WorldEntity,
    mut pred: impl FnMut(&WorldEntity) -> bool,
) -> Option<WorldEntity> {
    let mut best: Option<(WorldEntity, f32)> = None;
    for e in ctx.db.game_world_entity().by_map().filter(&origin.map_id) {
        if !in_same_partition(&e, origin.map_id, origin.instance_id) || !pred(&e) {
            continue;
        }
        let d = (e.x - origin.x).powi(2) + (e.y - origin.y).powi(2);
        if best.as_ref().is_none_or(|(_, bd)| d < *bd) {
            best = Some((e, d));
        }
    }
    best.map(|(e, _)| e)
}

pub(crate) fn event_recipient_identity(bound: Option<Identity>) -> Identity {
    bound.unwrap_or(Identity::ZERO)
}

pub(crate) fn live_entity(ctx: &ReducerContext, guid: u64) -> Result<WorldEntity, String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .ok_or_else(|| format!("no live entity for guid {guid}"))
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn require_character(ctx: &ReducerContext, guid: u64) -> Result<Character, String> {
    character_by_guid(ctx, guid).ok_or_else(|| format!("no character {guid}"))
}

pub(crate) fn grid_of(ctx: &ReducerContext, guid: u64) -> (u32, u64, i32, i32) {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .map(|e| (e.map_id, e.instance_id, e.grid_x, e.grid_y))
        .unwrap_or((0, 0, 0, 0))
}

pub(crate) fn entity_addr(e: &WorldEntity) -> (u32, u64, i32, i32) {
    (e.map_id, e.instance_id, e.grid_x, e.grid_y)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn entity(
        guid: u64,
        map_id: u32,
        instance_id: u64,
        grid_x: i32,
        grid_y: i32,
    ) -> WorldEntity {
        WorldEntity {
            guid,
            owner_identity: Identity::ZERO,
            account_id: 0,
            map_id,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            grid_x,
            grid_y,
            cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
            last_move_ms: 0,
            type_mask: 0,
            entry: 0,
            scale_x: 1.0,
            health: 1,
            max_health: 1,
            power: 0,
            max_power: 0,
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
            next_level_xp: 0,
            target_guid: 0,
            money: 0,
            unit_bytes_1: 0,
            unit_bytes_2: 0,
            strength: 0,
            agility: 0,
            stamina: 0,
            intellect: 0,
            spirit: 0,
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
            instance_id,
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
            regeneration_pending: false,
            combat_active: false,
            simulation_indexed: false,
        }
    }

    #[test]
    fn entities_near_partitions_distinct_instances_on_the_same_cell_into_disjoint_sets() {
        let same_map = 1u32;
        let same_cell = (3i32, -4i32);
        let population = [
            entity(100, same_map, 0, same_cell.0, same_cell.1),
            entity(101, same_map, 0, same_cell.0, same_cell.1),
            entity(200, same_map, 7, same_cell.0, same_cell.1),
            entity(201, same_map, 9, same_cell.0, same_cell.1),
            entity(300, same_map + 1, 0, same_cell.0, same_cell.1),
        ];

        let instance_0: Vec<u64> = population
            .iter()
            .filter(|e| in_same_partition(e, same_map, 0))
            .map(|e| e.guid)
            .collect();
        let instance_7: Vec<u64> = population
            .iter()
            .filter(|e| in_same_partition(e, same_map, 7))
            .map(|e| e.guid)
            .collect();
        let instance_9: Vec<u64> = population
            .iter()
            .filter(|e| in_same_partition(e, same_map, 9))
            .map(|e| e.guid)
            .collect();

        assert_eq!(instance_0, vec![100, 101]);
        assert_eq!(instance_7, vec![200]);
        assert_eq!(instance_9, vec![201]);

        for a in &instance_0 {
            assert!(!instance_7.contains(a) && !instance_9.contains(a));
        }
        for a in &instance_7 {
            assert!(!instance_0.contains(a) && !instance_9.contains(a));
        }
    }

    #[test]
    fn a_recipient_without_a_character_row_addresses_the_event_to_nobody() {
        let bound = Identity::from_byte_array([7u8; 32]);
        assert_eq!(event_recipient_identity(Some(bound)), bound);
        assert_eq!(event_recipient_identity(None), Identity::ZERO);
        assert_ne!(
            Identity::ZERO,
            bound,
            "ZERO must not be a reachable client identity"
        );
    }

    #[test]
    fn gate_in_transit_refuses_an_in_transit_candidate_and_returns_a_normal_one() {
        assert_eq!(
            gate_in_transit(Some("normal_character"), false),
            Some("normal_character"),
            "not in transit: the candidate must pass through untouched"
        );
        assert_eq!(
            gate_in_transit(Some("in_transit_character"), true),
            None,
            "in transit: the candidate must be refused (read as absent), never handed back"
        );
        assert_eq!(gate_in_transit(None::<&str>, false), None);
        assert_eq!(gate_in_transit(None::<&str>, true), None);
    }

    #[test]
    fn entity_addr_matches_grid_of_field_order() {
        let e = entity(1, 7, 42, -3, 11);
        assert_eq!(entity_addr(&e), (7, 42, -3, 11));
    }

    fn player_at(guid: u64, x: f32) -> WorldEntity {
        let mut e = entity(guid, 0, 0, 0, 0);
        e.type_mask = lyracore_shared::constants::type_mask::PLAYER_BIT;
        e.x = x;
        e
    }

    #[test]
    fn a_living_player_next_to_a_living_player_passes_and_an_absent_partner_is_no_target() {
        let actor = player_at(1, 0.0);
        let partner = player_at(2, 3.0);
        assert_eq!(player_interaction_gate(&actor, Some(&partner)), Ok(()));
        assert_eq!(
            player_interaction_gate(&actor, None),
            Err(PlayerInteractionDenied::NoTarget)
        );
    }

    #[test]
    fn the_gate_names_the_specific_refusal_and_holds_the_range_boundary_and_precedence() {
        let actor = player_at(1, 0.0);

        let mut dead_actor = player_at(1, 0.0);
        dead_actor.dead = true;
        let partner = player_at(2, 3.0);
        assert_eq!(
            player_interaction_gate(&dead_actor, Some(&partner)),
            Err(PlayerInteractionDenied::ActorDead)
        );
        assert_eq!(
            player_interaction_gate(&dead_actor, None),
            Err(PlayerInteractionDenied::ActorDead)
        );

        let mut creature = player_at(2, 3.0);
        creature.type_mask = 0;
        assert_eq!(
            player_interaction_gate(&actor, Some(&creature)),
            Err(PlayerInteractionDenied::TargetNotPlayer)
        );

        let mut dead_partner = player_at(2, 3.0);
        dead_partner.dead = true;
        assert_eq!(
            player_interaction_gate(&actor, Some(&dead_partner)),
            Err(PlayerInteractionDenied::TargetDead)
        );

        let mut other_map = player_at(2, 3.0);
        other_map.map_id = 1;
        assert_eq!(
            player_interaction_gate(&actor, Some(&other_map)),
            Err(PlayerInteractionDenied::DifferentPartition)
        );
        let mut other_instance = player_at(2, 3.0);
        other_instance.instance_id = 7;
        assert_eq!(
            player_interaction_gate(&actor, Some(&other_instance)),
            Err(PlayerInteractionDenied::DifferentPartition)
        );

        let at_boundary = player_at(2, PLAYER_INTERACTION_RANGE_SQ.sqrt());
        assert_eq!(player_interaction_gate(&actor, Some(&at_boundary)), Ok(()));
        let mut past_boundary = player_at(2, PLAYER_INTERACTION_RANGE_SQ.sqrt());
        past_boundary.z = 1.0;
        assert_eq!(
            player_interaction_gate(&actor, Some(&past_boundary)),
            Err(PlayerInteractionDenied::OutOfRange)
        );
    }
}
