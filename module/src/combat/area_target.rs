//! The faction, reputation and PvP checks of vmangos `IsValidAttackTarget` for a ground area.

use spacetimedb::ReducerContext;

use crate::pvp::Combatant;
use crate::{game_faction_template, WorldEntity};

// vmangos ReputationRank.
const REP_HOSTILE: u8 = 1;
const REP_NEUTRAL: u8 = 3;
const REP_FRIENDLY: u8 = 4;

/// Whether a ground area that `caster` placed harms `target`.
pub(crate) fn area_may_harm(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    target: &WorldEntity,
) -> bool {
    if target.is_flying() {
        return false;
    }
    let by = crate::pvp::affecting_character(ctx, caster);
    let of = crate::pvp::affecting_character(ctx, target);
    let dueling = matches!(
        (by, of),
        (Some(by), Some(of)) if crate::duel::active_opponents(ctx, by.guid, of.guid)
    );
    let to_target = reaction(ctx, (caster, by), (target, of), dueling);
    let to_caster = reaction(ctx, (target, of), (caster, by), dueling);
    attack_allowed(by, of, to_target, to_caster, dueling)
}

fn reaction(
    ctx: &ReducerContext,
    (from, from_side): (&WorldEntity, Option<Combatant>),
    (to, to_side): (&WorldEntity, Option<Combatant>),
    dueling: bool,
) -> u8 {
    match (from_side, to_side) {
        (Some(a), Some(b)) if a.guid == b.guid => return REP_FRIENDLY,
        (Some(_), Some(_)) if dueling => return REP_HOSTILE,
        (Some(character), None) => {
            if let Some((_, at_war)) = reputation_with(ctx, character.guid, to) {
                return toward_reputation_faction(at_war);
            }
        }
        (None, Some(character)) => {
            if let Some((rank, at_war)) = reputation_with(ctx, character.guid, from) {
                return from_reputation_faction(rank, at_war);
            }
        }
        _ => {}
    }
    template_reaction(
        crate::faction::is_hostile(ctx, from.faction_template, to.faction_template),
        crate::faction::is_friendly(ctx, from.faction_template, to.faction_template)
            || crate::faction::is_friendly(ctx, to.faction_template, from.faction_template),
    )
}

/// The Character's rank with the unit's faction and whether it is at war, for a reputation faction.
fn reputation_with(
    ctx: &ReducerContext,
    character_guid: u64,
    unit: &WorldEntity,
) -> Option<(u8, bool)> {
    let faction = ctx
        .db
        .game_faction_template()
        .id()
        .find(unit.faction_template)?
        .faction;
    crate::reputation::rank_and_war(ctx, character_guid, faction)
}

/// A Character sees a reputation faction as hostile only at war.
fn toward_reputation_faction(at_war: bool) -> u8 {
    if at_war {
        REP_HOSTILE
    } else {
        REP_FRIENDLY
    }
}

/// A reputation faction sees a Character by rank, at most neutral when the Character is at war.
fn from_reputation_faction(rank: u8, at_war: bool) -> u8 {
    if at_war {
        rank.min(REP_NEUTRAL)
    } else {
        rank
    }
}

fn template_reaction(hostile: bool, friendly: bool) -> u8 {
    if hostile {
        REP_HOSTILE
    } else if friendly {
        REP_FRIENDLY
    } else {
        REP_NEUTRAL
    }
}

/// vmangos `IsValidAttackTarget` on both reactions, plus the patch 1.7 rule for Characters.
fn attack_allowed(
    by: Option<Combatant>,
    of: Option<Combatant>,
    to_target: u8,
    to_caster: u8,
    dueling: bool,
) -> bool {
    match (by, of) {
        (None, None) => to_target <= REP_HOSTILE || to_caster <= REP_HOSTILE,
        _ if to_target > REP_NEUTRAL || to_caster > REP_NEUTRAL => false,
        (Some(by), Some(of)) => dueling || (by.flagged && of.flagged),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASTER: u64 = 1;
    const OTHER: u64 = 2;
    const REP_HONORED: u8 = 5;

    fn character(guid: u64, flagged: bool) -> Option<Combatant> {
        Some(Combatant { guid, flagged })
    }

    fn neutral() -> u8 {
        template_reaction(false, false)
    }

    fn hostile() -> u8 {
        template_reaction(true, false)
    }

    #[test]
    fn a_characters_area_spares_a_neutral_town_guard_when_not_at_war() {
        // Booty Bay and Gadgetzan guards: a reputation faction at Neutral.
        let caster = character(CASTER, false);
        assert!(!attack_allowed(
            caster,
            None,
            toward_reputation_faction(false),
            from_reputation_faction(REP_NEUTRAL, false),
            false
        ));
    }

    #[test]
    fn a_characters_area_harms_a_reputation_faction_it_is_at_war_with() {
        let caster = character(CASTER, false);
        assert!(attack_allowed(
            caster,
            None,
            toward_reputation_faction(true),
            from_reputation_faction(REP_HONORED, true),
            false
        ));
    }

    #[test]
    fn a_characters_area_harms_a_neutral_creature_without_reputation() {
        assert!(attack_allowed(
            character(CASTER, false),
            None,
            neutral(),
            neutral(),
            false
        ));
    }

    #[test]
    fn a_creatures_area_spares_a_creature_neutral_to_it() {
        // A critter beside a hostile creature's Poison Cloud, or a unit without a faction template.
        assert!(!attack_allowed(None, None, neutral(), neutral(), false));
    }

    #[test]
    fn a_creatures_area_harms_a_creature_hostile_in_either_direction() {
        assert!(attack_allowed(None, None, hostile(), neutral(), false));
        assert!(attack_allowed(None, None, neutral(), hostile(), false));
    }

    #[test]
    fn an_area_never_harms_a_friendly_unit() {
        let friendly = template_reaction(false, true);
        assert!(!attack_allowed(
            None,
            character(OTHER, true),
            neutral(),
            friendly,
            false
        ));
    }

    #[test]
    fn an_unflagged_characters_area_spares_a_flagged_enemy_character() {
        assert!(!attack_allowed(
            character(CASTER, false),
            character(OTHER, true),
            hostile(),
            hostile(),
            false
        ));
    }

    #[test]
    fn a_flagged_characters_area_harms_a_flagged_enemy_character() {
        assert!(attack_allowed(
            character(CASTER, true),
            character(OTHER, true),
            hostile(),
            hostile(),
            false
        ));
    }

    #[test]
    fn an_unflagged_characters_area_harms_its_duel_opponent() {
        assert!(attack_allowed(
            character(CASTER, false),
            character(OTHER, false),
            REP_HOSTILE,
            REP_HOSTILE,
            true
        ));
    }
}
