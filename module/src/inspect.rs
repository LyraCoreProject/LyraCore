use spacetimedb::{ReducerContext, Table};

use crate::faction::game_faction_template;
use crate::npc_interaction::INTERACTION_RANGE_SQ;

pub(crate) fn can_inspect(
    target_is_player: bool,
    same_map: bool,
    dist_sq: f32,
    friendly: bool,
) -> Result<(), String> {
    if !target_is_player {
        return Err("inspect target is not a player".to_string());
    }
    if !same_map {
        return Err("inspect target on another map".to_string());
    }
    if dist_sq > INTERACTION_RANGE_SQ {
        return Err("inspect target out of range".to_string());
    }
    if !friendly {
        return Err("inspect target is not friendly".to_string());
    }
    Ok(())
}

pub(crate) fn apply_inspect(
    ctx: &ReducerContext,
    inspector: crate::WorldEntity,
    target_guid: u64,
) -> Result<(), crate::Refusal> {
    let target = crate::helpers::live_entity(ctx, target_guid)
        .map_err(|_| "no such inspect target".to_string())?;
    let dist_sq = crate::helpers::dist_sq(&inspector, &target);
    let friendly = ctx.db.game_faction_template().count() == 0
        || crate::faction::is_friendly(ctx, inspector.faction_template, target.faction_template);
    Ok(can_inspect(
        target.is_player(),
        target.map_id == inspector.map_id && target.instance_id == inspector.instance_id,
        dist_sq,
        friendly,
    )?)
}

#[cfg(test)]
mod tests {
    use super::{can_inspect, INTERACTION_RANGE_SQ};

    #[test]
    fn inspect_gate_admits_only_an_in_range_friendly_player() {
        assert!(can_inspect(true, true, 0.0, true).is_ok());
        assert!(can_inspect(true, true, INTERACTION_RANGE_SQ, true).is_ok());

        assert_eq!(
            can_inspect(false, true, 0.0, true).unwrap_err(),
            "inspect target is not a player"
        );
        assert_eq!(
            can_inspect(true, false, 0.0, true).unwrap_err(),
            "inspect target on another map"
        );
        assert_eq!(
            can_inspect(true, true, INTERACTION_RANGE_SQ + 1.0, true).unwrap_err(),
            "inspect target out of range"
        );
        assert_eq!(
            can_inspect(true, true, 0.0, false).unwrap_err(),
            "inspect target is not friendly"
        );
    }
}
