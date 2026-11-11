use spacetimedb::ReducerContext;

use crate::conformance_rolls::{draw, RollStream};
use crate::{game_creature_template, game_world_entity, Spell, WorldEntity};

use super::folds::{defender_block_value, effective_crit_bp, effective_ranged_crit_bp};
use super::tables::{crushing_chance_bp, dodge_chance_bp, BLOCK_BP, PARRY_BP};

const BASE_MISS_BP: u32 = 500;
const DUAL_WIELD_MISS_BP: u32 = 2400;
const MAX_MISS_BP: f32 = 6000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttackOutcome {
    Normal,
    Crit,
    Miss,
    Dodge,
    Parry,
    Glancing,
    Crushing,
    Block,
}

impl AttackOutcome {
    /// The outcome code that `CombatEvent.hit_info` and `SpellCastEvent.swing_hit_info` store.
    pub(crate) fn wire(self) -> u8 {
        match self {
            Self::Normal => 0,
            Self::Crit => 1,
            Self::Miss => 2,
            Self::Dodge => 3,
            Self::Parry => 4,
            Self::Glancing => 5,
            Self::Crushing => 6,
            Self::Block => 7,
        }
    }

    pub(crate) fn is_avoided(self) -> bool {
        match self {
            Self::Miss | Self::Dodge | Self::Parry => true,
            Self::Normal | Self::Crit | Self::Glancing | Self::Crushing | Self::Block => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttackType {
    Melee,
    Ranged,
}

#[derive(Clone, Copy)]
pub(crate) struct DeliveredSpell {
    pub(crate) next_melee_swing: bool,
    pub(crate) always_hit: bool,
    pub(crate) cannot_crit: bool,
}

impl DeliveredSpell {
    pub(crate) fn queued_strike(spell: Option<&Spell>) -> Self {
        Self::of(spell, true)
    }

    pub(crate) fn cast(spell: &Spell) -> Self {
        Self::of(Some(spell), false)
    }

    fn of(spell: Option<&Spell>, next_melee_swing: bool) -> Self {
        let flags = spell.map_or(0, |spell| spell.cast_flags);
        Self {
            next_melee_swing,
            always_hit: flags & crate::spell::CAST_FLAG_ALWAYS_HIT != 0,
            cannot_crit: flags & crate::spell::CAST_FLAG_CANNOT_CRIT != 0,
        }
    }
}

pub(crate) struct Skills {
    pub(crate) weapon: i32,
    pub(crate) maximum: i32,
    pub(crate) defense: i32,
    pub(crate) defense_maximum: i32,
    pub(crate) target_level: u32,
    pub(crate) target_is_character: bool,
}

/// Basis-point chances before skill, facing, stand state and attack facts adjust them.
#[derive(Clone, Copy)]
pub(crate) struct BaseChances {
    pub(crate) miss: u32,
    pub(crate) dodge: u32,
    pub(crate) parry: u32,
    pub(crate) block: u32,
    pub(crate) crit: u32,
    pub(crate) crushing: u32,
}

pub(crate) struct AttackFacts {
    pub(crate) skills: Skills,
    pub(crate) base: BaseChances,
    pub(crate) hit_bonus: i32,
    pub(crate) attack_type: AttackType,
    pub(crate) spell: Option<DeliveredSpell>,
    pub(crate) attacker_can_glance: bool,
    pub(crate) target_faces_attacker: bool,
    pub(crate) target_standing: bool,
    pub(crate) no_parry: bool,
    pub(crate) no_block: bool,
}

/// Basis-point bands walked in field order; `partial_block` is rolled after the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AttackTable {
    pub(crate) miss: u32,
    pub(crate) dodge: u32,
    pub(crate) parry: u32,
    pub(crate) glancing: u32,
    pub(crate) block: u32,
    pub(crate) crit: u32,
    pub(crate) crushing: u32,
    pub(crate) partial_block: u32,
}

impl AttackTable {
    pub(crate) fn outcome(&self, roll: u32) -> AttackOutcome {
        let bands = [
            (self.miss, AttackOutcome::Miss),
            (self.dodge, AttackOutcome::Dodge),
            (self.parry, AttackOutcome::Parry),
            (self.glancing, AttackOutcome::Glancing),
            (self.block, AttackOutcome::Block),
            (self.crit, AttackOutcome::Crit),
            (self.crushing, AttackOutcome::Crushing),
        ];
        let mut floor = 0;
        for (width, outcome) in bands {
            floor += width;
            if roll < floor {
                return outcome;
            }
        }
        AttackOutcome::Normal
    }
}

// vmangos RollMeleeOutcomeAgainst; docs/adr/0007-statistical-roll-conformance.md cites its lines.
pub(crate) fn attack_table(facts: &AttackFacts) -> AttackTable {
    let skills = &facts.skills;
    let base = facts.base;
    let melee = facts.attack_type == AttackType::Melee;
    let swing = facts.spell.is_none_or(|spell| spell.next_melee_swing);
    let faced = facts.target_faces_attacker;
    let can_block = melee && faced && !facts.no_block;

    let full = skills.weapon - skills.defense;
    let difference = skills.weapon - skills.defense_maximum;
    let capped = skills.weapon.min(skills.maximum) - skills.defense_maximum;
    let low_level = if !skills.target_is_character && skills.target_level < 10 {
        skills.target_level as f32 / 10.0
    } else {
        1.0
    };
    let avoidance_per_skill = if skills.target_is_character { 4 } else { 10 };
    let miss_per_skill = if skills.target_is_character {
        4
    } else if full < -10 {
        20
    } else {
        10
    };
    let hit_bonus = if full < -10 && facts.hit_bonus > 0 {
        facts.hit_bonus - 100
    } else {
        facts.hit_bonus
    };
    let miss = (((base.miss as i32 - full * miss_per_skill) as f32 * low_level) - hit_bonus as f32)
        .clamp(0.0, MAX_MISS_BP) as u32;
    let dodge =
        ((base.dodge as i32 - difference * avoidance_per_skill) as f32 * low_level).max(0.0) as u32;
    let parry = if base.parry > 0 {
        let parry_bonus = if skills.target_is_character {
            difference * 4
        } else {
            capped * if capped < -10 { 60 } else { 20 }
        };
        ((base.parry as i32 - parry_bonus) as f32 * low_level).max(0.0) as u32
    } else {
        0
    };
    let glancing =
        (1000 + (skills.defense - skills.weapon.min(skills.maximum)) * 200).clamp(0, 4000) as u32;
    let block = if base.block > 0 {
        let block = base.block as i32 - difference * avoidance_per_skill;
        let block = if skills.target_is_character {
            block
        } else {
            block.min(500)
        };
        (block as f32 * low_level).max(0.0) as u32
    } else {
        0
    };
    let crit_bonus = if skills.target_is_character || full > 0 {
        full * 4
    } else {
        (skills.weapon.min(skills.maximum) - skills.defense) * 20
    };
    let crit = (base.crit as i32 + crit_bonus).max(0) as u32;

    let band = |open: bool, chance: u32| if open { chance } else { 0 };
    AttackTable {
        miss: band(
            facts.target_standing && !facts.spell.is_some_and(|spell| spell.always_hit),
            miss,
        ),
        dodge: band(melee && (faced || !skills.target_is_character), dodge),
        parry: band(melee && faced && !facts.no_parry, parry),
        glancing: band(melee && swing && facts.attacker_can_glance, glancing),
        block: band(can_block && facts.spell.is_none(), block),
        crit: band(!facts.spell.is_some_and(|spell| spell.cannot_crit), crit),
        crushing: band(melee && swing, base.crushing),
        partial_block: band(can_block && facts.spell.is_some(), block),
    }
}

/// One weapon outcome: a swing draws from `RollStream::Swing`, a cast Spell from `Caster`.
pub(crate) fn roll_attack(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    attack_type: AttackType,
    spell: Option<DeliveredSpell>,
) -> (AttackOutcome, AttackTable) {
    let table = attack_table_for(ctx, attacker, target, attack_type, spell);
    let stream = match spell {
        Some(spell) if !spell.next_melee_swing => RollStream::Caster(attacker.guid),
        _ => RollStream::Swing(attacker.guid),
    };
    (table.outcome(draw(ctx, stream) % 10_000), table)
}

pub(crate) fn attack_table_for(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    attack_type: AttackType,
    spell: Option<DeliveredSpell>,
) -> AttackTable {
    use crate::creatures::catalogue::game_creature_defense;

    let target_is_character = target.is_player();
    let (weapon, crit) = match attack_type {
        AttackType::Melee => (
            crate::skill::effective_weapon_skill(ctx, attacker),
            effective_crit_bp(ctx, attacker),
        ),
        AttackType::Ranged => (
            crate::skill::effective_ranged_weapon_skill(ctx, attacker),
            effective_ranged_crit_bp(ctx, attacker),
        ),
    };
    let flags = (!target_is_character)
        .then(|| ctx.db.game_creature_defense().entry().find(target.entry))
        .flatten();
    let (dodge, parry, block) = defense_chances(ctx, target);
    let dual_wield_penalty = attack_type == AttackType::Melee
        && spell.is_none()
        && attacker.is_player()
        && attacker.next_swing_spell == 0
        && super::folds::equipped_offhand_weapon_damage(ctx, attacker.guid).is_some();
    attack_table(&AttackFacts {
        skills: Skills {
            weapon: weapon as i32,
            maximum: crate::skill::skill_cap_for_level(attacker.level) as i32,
            defense: defense_skill(
                ctx,
                attacker,
                target,
                flags.as_ref().is_some_and(|flags| flags.no_defense),
            ),
            defense_maximum: crate::skill::skill_cap_for_level(target.level) as i32,
            target_level: target.level,
            target_is_character,
        },
        base: BaseChances {
            miss: if dual_wield_penalty {
                DUAL_WIELD_MISS_BP
            } else {
                BASE_MISS_BP
            },
            dodge,
            parry,
            block,
            crit,
            crushing: if !attacker.is_player() && target_is_character {
                crushing_chance_bp(attacker.level, target.level)
            } else {
                0
            },
        },
        hit_bonus: crate::modifiers::combat_field_bonus(
            ctx,
            attacker.guid,
            crate::spell::COMBAT_HIT,
        ) + crate::items::equipped_stat_bonus(
            ctx,
            attacker.guid,
            crate::items::EquipStat::Hit,
        ),
        attack_type,
        spell,
        attacker_can_glance: character_controls(ctx, attacker) && !character_controls(ctx, target),
        target_faces_attacker: crate::geometry::is_facing(
            target.x,
            target.y,
            target.orientation,
            attacker.x,
            attacker.y,
        ),
        target_standing: target.unit_bytes_1 & crate::world::STAND_STATE_MASK
            == crate::world::STAND_STATE_STAND,
        no_parry: flags.as_ref().is_some_and(|flags| flags.no_parry),
        no_block: flags.as_ref().is_some_and(|flags| flags.no_block),
    })
}

fn character_controls(ctx: &ReducerContext, unit: &WorldEntity) -> bool {
    unit.is_player()
        || unit.owner_guid != 0
            && ctx
                .db
                .game_world_entity()
                .guid()
                .find(unit.owner_guid)
                .is_some_and(|owner| owner.is_player())
}

fn fold_band(
    ctx: &ReducerContext,
    target_guid: u64,
    base: u32,
    field: u8,
    gear: crate::items::EquipStat,
) -> u32 {
    (i64::from(base)
        + i64::from(crate::modifiers::combat_field_bonus(
            ctx,
            target_guid,
            field,
        ))
        + i64::from(crate::items::equipped_stat_bonus(ctx, target_guid, gear)))
    .clamp(0, 10_000) as u32
}

// vmangos GetUnitDodgeChance, GetUnitParryChance and GetUnitBlockChance.
fn defense_chances(ctx: &ReducerContext, target: &WorldEntity) -> (u32, u32, u32) {
    use crate::items::EquipStat;
    use crate::spell::{COMBAT_BLOCK, COMBAT_DODGE, COMBAT_PARRY};

    let target_is_character = target.is_player();
    let totem = !target_is_character
        && ctx
            .db
            .game_creature_template()
            .entry()
            .find(target.entry)
            .is_some_and(|template| {
                template.creature_type == lyracore_shared::spell::taxonomy::TOTEM_TYPE
            });
    if totem
        || crate::spell::is_stunned(ctx, target.guid)
        || crate::spell::is_non_melee_spell_casting(ctx, target.guid)
    {
        return (0, 0, 0);
    }
    let base_dodge = if target_is_character {
        dodge_chance_bp(crate::modifiers::effective_stat(
            ctx,
            target,
            crate::spell::STAT_AGI,
        ))
    } else {
        500
    };
    let dodge = fold_band(ctx, target.guid, base_dodge, COMBAT_DODGE, EquipStat::Dodge);
    let parry = fold_band(ctx, target.guid, PARRY_BP, COMBAT_PARRY, EquipStat::Parry);
    let can_block = !target_is_character || defender_block_value(ctx, target) > 0;
    let block = fold_band(ctx, target.guid, BLOCK_BP, COMBAT_BLOCK, EquipStat::Block);
    let defense_bonus = if target_is_character {
        (i64::from(crate::skill::effective_defense_skill(ctx, target))
            - i64::from(crate::skill::skill_cap_for_level(target.level)))
            * 4
    } else {
        0
    };
    let with_defense = |chance| (i64::from(chance) + defense_bonus).clamp(0, 10_000) as u32;
    (
        with_defense(dodge),
        with_defense(parry),
        if can_block { with_defense(block) } else { 0 },
    )
}

fn defense_skill(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    no_defense: bool,
) -> i32 {
    use crate::skill::game_player_skill;
    let maximum = crate::skill::skill_cap_for_level(target.level) as i32;
    if no_defense {
        0
    } else if target.is_player() && attacker.is_player() {
        let maximum = ctx
            .db
            .game_player_skill()
            .by_character()
            .filter(target.guid)
            .find(|skill| skill.skill_line == crate::skill::skill_line::DEFENSE)
            .map_or(maximum, |skill| i32::from(skill.max_rank));
        let aura =
            crate::modifiers::combat_field_bonus(ctx, target.guid, crate::spell::COMBAT_DEFENSE);
        let gear =
            crate::items::equipped_stat_bonus(ctx, target.guid, crate::items::EquipStat::Defense);
        (i64::from(maximum) + i64::from(aura) + i64::from(gear)).clamp(0, i64::from(i32::MAX))
            as i32
    } else if target.is_player() {
        crate::skill::effective_defense_skill(ctx, target) as i32
    } else {
        maximum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUEUED: DeliveredSpell = DeliveredSpell {
        next_melee_swing: true,
        always_hit: false,
        cannot_crit: false,
    };
    const CAST: DeliveredSpell = DeliveredSpell {
        next_melee_swing: false,
        always_hit: false,
        cannot_crit: false,
    };

    fn facts(weapon: i32, maximum: i32, defense: i32, level: u32, hit: i32) -> AttackFacts {
        AttackFacts {
            skills: Skills {
                weapon,
                maximum,
                defense,
                defense_maximum: level as i32 * 5,
                target_level: level,
                target_is_character: false,
            },
            base: BaseChances {
                miss: 500,
                dodge: 500,
                parry: 500,
                block: 500,
                crit: 500,
                crushing: 0,
            },
            hit_bonus: hit,
            attack_type: AttackType::Melee,
            spell: None,
            attacker_can_glance: true,
            target_faces_attacker: true,
            target_standing: true,
            no_parry: false,
            no_block: false,
        }
    }

    fn level_60() -> AttackFacts {
        facts(300, 300, 300, 60, 0)
    }

    fn level_63() -> AttackFacts {
        facts(300, 300, 315, 63, 0)
    }

    fn bands(t: AttackTable) -> [u32; 8] {
        [
            t.miss,
            t.dodge,
            t.parry,
            t.glancing,
            t.block,
            t.crit,
            t.crushing,
            t.partial_block,
        ]
    }

    #[test]
    fn the_table_walks_miss_dodge_parry_glancing_block_crit_crushing_then_hit() {
        let table = AttackTable {
            miss: 100,
            dodge: 100,
            parry: 100,
            glancing: 100,
            block: 100,
            crit: 100,
            crushing: 100,
            partial_block: 5_000,
        };
        let walked = [0, 99, 100, 200, 300, 400, 500, 600, 700, 9_999].map(|r| table.outcome(r));
        assert_eq!(
            walked,
            [
                AttackOutcome::Miss,
                AttackOutcome::Miss,
                AttackOutcome::Dodge,
                AttackOutcome::Parry,
                AttackOutcome::Glancing,
                AttackOutcome::Block,
                AttackOutcome::Crit,
                AttackOutcome::Crushing,
                AttackOutcome::Normal,
                AttackOutcome::Normal,
            ]
        );
    }

    #[test]
    fn white_melee_intervals_follow_skill_caps_and_low_level_rules() {
        assert_eq!(
            bands(attack_table(&level_60())),
            [500, 500, 500, 1000, 500, 500, 0, 0]
        );
        assert_eq!(
            bands(attack_table(&level_63())),
            [800, 650, 1400, 4000, 500, 200, 0, 0]
        );
        let trained = attack_table(&facts(305, 300, 315, 63, 100));
        assert_eq!(
            [trained.miss, trained.parry, trained.glancing, trained.crit],
            [500, 1400, 4000, 200]
        );
        let low = attack_table(&facts(25, 25, 25, 5, 0));
        assert_eq!(
            [low.miss, low.dodge, low.parry, low.glancing, low.block],
            [250, 250, 250, 1000, 250]
        );
        assert_eq!(
            bands(attack_table(&facts(310, 300, 300, 60, 0))),
            [400, 400, 500, 1000, 400, 540, 0, 0]
        );
    }

    #[test]
    fn character_defenders_use_four_basis_points_per_skill() {
        let mut duel = facts(315, 315, 300, 60, 0);
        duel.skills.defense_maximum = 300;
        duel.skills.target_is_character = true;
        duel.attacker_can_glance = false;
        assert_eq!(
            bands(attack_table(&duel)),
            [440, 440, 440, 0, 440, 560, 0, 0]
        );
    }

    #[test]
    fn a_queued_strike_keeps_glancing_and_crushing_and_rolls_its_block_after_the_table() {
        let mut strike = level_63();
        strike.base.crushing = 1500;
        strike.spell = Some(QUEUED);
        assert_eq!(
            bands(attack_table(&strike)),
            [800, 650, 1400, 4000, 0, 200, 1500, 500]
        );
    }

    #[test]
    fn a_cast_melee_spell_neither_glances_nor_crushes_nor_blocks_in_the_table() {
        let mut cast = level_63();
        cast.base.crushing = 1500;
        cast.spell = Some(CAST);
        assert_eq!(
            bands(attack_table(&cast)),
            [800, 650, 1400, 0, 0, 200, 0, 500]
        );
    }

    #[test]
    fn an_always_hit_spell_cannot_miss_but_can_still_be_dodged() {
        let mut cast = level_63();
        cast.spell = Some(DeliveredSpell {
            always_hit: true,
            ..CAST
        });
        let table = attack_table(&cast);
        assert_eq!([table.miss, table.dodge], [0, 650]);
    }

    #[test]
    fn a_cannot_crit_spell_has_no_crit_band() {
        let mut strike = level_60();
        strike.spell = Some(DeliveredSpell {
            cannot_crit: true,
            ..QUEUED
        });
        assert_eq!(attack_table(&strike).crit, 0);
    }

    #[test]
    fn a_ranged_attack_rolls_only_miss_and_crit() {
        let mut shot = level_63();
        shot.base.crushing = 1500;
        shot.attack_type = AttackType::Ranged;
        assert_eq!(bands(attack_table(&shot)), [800, 0, 0, 0, 0, 200, 0, 0]);
        shot.spell = Some(CAST);
        assert_eq!(bands(attack_table(&shot)), [800, 0, 0, 0, 0, 200, 0, 0]);
    }

    #[test]
    fn behind_a_creature_only_dodge_remains_and_behind_a_character_none_does() {
        let mut behind = level_60();
        behind.target_faces_attacker = false;
        let table = attack_table(&behind);
        assert_eq!([table.dodge, table.parry, table.block], [500, 0, 0]);
        behind.spell = Some(QUEUED);
        assert_eq!(attack_table(&behind).partial_block, 0);
        behind.skills.target_is_character = true;
        assert_eq!(attack_table(&behind).dodge, 0);
    }

    #[test]
    fn a_target_that_is_not_standing_is_never_missed() {
        for spell in [None, Some(QUEUED), Some(CAST)] {
            let mut sitting = level_63();
            sitting.target_standing = false;
            sitting.spell = spell;
            assert_eq!(attack_table(&sitting).miss, 0);
        }
    }

    #[test]
    fn creature_flags_remove_parry_and_block() {
        let mut flagged = level_60();
        flagged.no_parry = true;
        flagged.no_block = true;
        let white = attack_table(&flagged);
        assert_eq!([white.parry, white.block], [0, 0]);
        flagged.spell = Some(QUEUED);
        assert_eq!(attack_table(&flagged).partial_block, 0);
    }

    #[test]
    fn the_wire_code_is_the_stored_outcome_value() {
        let outcomes = [
            AttackOutcome::Normal,
            AttackOutcome::Crit,
            AttackOutcome::Miss,
            AttackOutcome::Dodge,
            AttackOutcome::Parry,
            AttackOutcome::Glancing,
            AttackOutcome::Crushing,
            AttackOutcome::Block,
        ];
        assert_eq!(outcomes.map(AttackOutcome::wire), [0, 1, 2, 3, 4, 5, 6, 7]);
    }
}
