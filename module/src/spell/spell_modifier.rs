use lyracore_shared::spell::taxonomy::{
    A_SPELLMOD_FLAT, A_SPELLMOD_PCT, SPELLMOD_OP_CASTING_TIME, SPELLMOD_OP_COST,
    SPELLMOD_OP_CRITICAL_CHANCE, SPELL_FAMILY_POTION,
};
use spacetimedb::{ReducerContext, Timestamp};

use super::proc::{DAMAGE_CLASS_MAGIC, DAMAGE_CLASS_MELEE, DAMAGE_CLASS_RANGED};
use super::tables::SPELL_ATTR_EX3_IGNORE_CASTER_MODIFIERS;
use super::{apply_spell_mod, auras_on, can_crit, shed_auras};
use crate::modifiers::AuraAmount;
use crate::{game_spell, Aura, Spell, SpellEffect};

// vmangos Player::ApplySpellMod leaves an instant cast-time modifier off a cast this long or longer.
const INSTANT_CAST_LIMIT_MS: u32 = 10_000;

fn is_spell_modifier(row: &Aura) -> bool {
    matches!(row.eff_kind, A_SPELLMOD_FLAT | A_SPELLMOD_PCT)
}

// vmangos SpellModifier::IsAffectedOnSpell; ApplySpellMod skips an IGNORE_CASTER_MODIFIERS spell.
fn affecting(ctx: &ReducerContext, caster_guid: u64, hdr: &Spell) -> Vec<Aura> {
    if hdr.family_name == 0
        || hdr.family_flags == 0
        || hdr.attributes_ex_3 & SPELL_ATTR_EX3_IGNORE_CASTER_MODIFIERS != 0
    {
        return Vec::new();
    }
    let spells = ctx.db.game_spell();
    auras_on(ctx, caster_guid)
        .filter(|row| {
            is_spell_modifier(row)
                && hdr.family_flags & (row.eff_p1 as u32 as u64) != 0
                && spells
                    .spell_id()
                    .find(row.spell_id)
                    .is_some_and(|source| source.family_name == hdr.family_name)
        })
        .collect()
}

fn sums(rows: &[Aura]) -> (i32, i32) {
    rows.iter().fold((0, 0), |(flat, pct), row| {
        let amount = AuraAmount::from(row).total();
        if row.eff_kind == A_SPELLMOD_FLAT {
            (flat + amount, pct)
        } else {
            (flat, pct + amount)
        }
    })
}

/// The summed flat and percent `op` modifiers of the caster that affect `hdr`.
pub(crate) fn spell_mod(
    ctx: &ReducerContext,
    caster_guid: u64,
    op: i32,
    hdr: &Spell,
) -> (i32, i32) {
    let rows: Vec<Aura> = affecting(ctx, caster_guid, hdr)
        .into_iter()
        .filter(|row| row.eff_p0 == op)
        .collect();
    sums(&rows)
}

fn makes_instant(row: &Aura) -> bool {
    row.eff_p0 == SPELLMOD_OP_CASTING_TIME
        && row.eff_kind == A_SPELLMOD_PCT
        && AuraAmount::from(row).total() <= -100
}

// Read docs/schema.md#spell-modifiers for the base-value rules.
fn changes(row: &Aura, base: u32) -> bool {
    if row.eff_p0 == SPELLMOD_OP_CASTING_TIME && base == 0 {
        return false;
    }
    row.eff_kind == A_SPELLMOD_FLAT
        || (base != 0 && !(makes_instant(row) && base >= INSTANT_CAST_LIMIT_MS))
}

/// vmangos Player::ApplySpellMod: `base` under the caster's `op` modifiers that affect `hdr`.
pub(crate) fn spell_modified_value(
    ctx: &ReducerContext,
    caster_guid: u64,
    op: i32,
    hdr: &Spell,
    base: u32,
) -> u32 {
    let rows: Vec<Aura> = affecting(ctx, caster_guid, hdr)
        .into_iter()
        .filter(|row| row.eff_p0 == op && changes(row, base))
        .collect();
    if rows.iter().any(makes_instant) {
        return 0;
    }
    let (flat, pct) = sums(&rows);
    apply_spell_mod(base, flat, pct)
}

/// The power `hdr` costs the caster after its cost modifiers.
pub(crate) fn spell_cost(ctx: &ReducerContext, caster_guid: u64, hdr: &Spell) -> u32 {
    spell_modified_value(ctx, caster_guid, SPELLMOD_OP_COST, hdr, hdr.cost)
}

/// vmangos Player::ApplySpellMod on a crit `chance` in percent; percent modifiers skip a 0 chance.
pub(crate) fn crit_chance_under(chance: f32, (flat, pct): (i32, i32)) -> f32 {
    let pct = if chance == 0.0 { 0 } else { pct };
    let diff = (chance + flat as f32) * pct as f32 / 100.0 + flat as f32;
    (chance + diff).max(0.0)
}

/// A spell or heal crit `chance` in percent under the caster's crit chance modifiers on `hdr`.
pub(crate) fn modified_crit_chance(
    ctx: &ReducerContext,
    caster_guid: u64,
    hdr: &Spell,
    chance: f32,
) -> f32 {
    crit_chance_under(
        chance,
        spell_mod(ctx, caster_guid, SPELLMOD_OP_CRITICAL_CHANCE, hdr),
    )
}

/// Whether vmangos Unit::GetSpellCritChance reaches the crit chance modifiers for `hdr`.
pub(crate) fn rolls_modified_crit(hdr: &Spell, effects: &[SpellEffect]) -> bool {
    can_crit(hdr, effects)
        && (hdr.family_name == SPELL_FAMILY_POTION
            || matches!(
                hdr.damage_class,
                DAMAGE_CLASS_MAGIC | DAMAGE_CLASS_MELEE | DAMAGE_CLASS_RANGED
            ))
}

/// When a resolving cast fixed its cast time, whether it computes a power cost, and whether a
/// crit roll reads its crit chance modifiers.
#[derive(Clone, Copy)]
pub(crate) struct ModifiedCast {
    pub began_at: Timestamp,
    pub pays_cost: bool,
    pub rolls_crit: bool,
}

/// The charged spell modifier rows a resolving cast uses; [`Self::spend`] alone spends them.
pub(crate) struct ModifierCharges {
    carrier_guid: u64,
    used: Vec<Aura>,
}

impl ModifierCharges {
    pub(crate) fn used_by(
        ctx: &ReducerContext,
        caster_guid: u64,
        hdr: &Spell,
        cast: ModifiedCast,
    ) -> Self {
        let spells = ctx.db.game_spell();
        // vmangos Aura::HandleAddModifier gives a stacking spell's modifier no charges.
        let chargeable: Vec<Aura> = affecting(ctx, caster_guid, hdr)
            .into_iter()
            .filter(|row| {
                spells
                    .spell_id()
                    .find(row.spell_id)
                    .is_some_and(|source| source.max_stacks <= 1)
            })
            .collect();
        Self {
            carrier_guid: caster_guid,
            used: used_rows(chargeable, hdr, cast),
        }
    }

    /// vmangos Player::RemoveSpellMods: each Aura Holder loses one charge, and its last removes it.
    pub(crate) fn spend(self, ctx: &ReducerContext) {
        let mut spent: Vec<(u32, u64)> = Vec::new();
        for used in &self.used {
            let holder = (used.spell_id, used.caster_guid);
            if spent.contains(&holder) {
                continue;
            }
            spent.push(holder);
            let rows: Vec<Aura> = auras_on(ctx, self.carrier_guid)
                .filter(|row| (row.spell_id, row.caster_guid) == holder)
                .collect();
            // A Holder refreshed by the cast's own effects keeps its fresh charges.
            if rows.iter().any(|row| row.applied_at != used.applied_at) {
                continue;
            }
            let charges = rows.iter().map(|row| row.proc_charges).max().unwrap_or(0);
            if charges <= 1 {
                shed_auras(ctx, rows);
            } else {
                for mut row in rows {
                    row.proc_charges = charges - 1;
                    row.update(ctx);
                }
            }
        }
    }
}

// Read docs/schema.md#spell-modifiers for which rows a cast uses.
fn used_rows(rows: Vec<Aura>, hdr: &Spell, cast: ModifiedCast) -> Vec<Aura> {
    let fixed_cast_time = |row: &Aura| row.applied_at <= cast.began_at;
    let instant = rows
        .iter()
        .any(|row| makes_instant(row) && fixed_cast_time(row));
    rows.into_iter()
        .filter(|row| {
            row.proc_charges > 0
                && match row.eff_p0 {
                    SPELLMOD_OP_CASTING_TIME => {
                        fixed_cast_time(row)
                            && changes(row, hdr.cast_time_ms)
                            && !(instant && row.eff_kind == A_SPELLMOD_FLAT)
                    }
                    SPELLMOD_OP_COST => cast.pays_cost && changes(row, hdr.cost),
                    SPELLMOD_OP_CRITICAL_CHANCE => cast.rolls_crit,
                    _ => true,
                }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEGAN_MICROS: i64 = 1_000_000;

    fn modifier(kind: u8, op: i32, amount: i32, charges: u8) -> Aura {
        Aura {
            id: 0,
            target_guid: 1,
            caster_guid: 1,
            spell_id: 1,
            slot: 0,
            level: 60,
            flags: 0,
            applied_at: Timestamp::from_micros_since_unix_epoch(BEGAN_MICROS),
            expires_at: Timestamp::UNIX_EPOCH,
            effect_id: 0,
            eff_kind: kind,
            amount,
            eff_p0: op,
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
            proc_charges: charges,
            proc_icd_ms: 0,
            proc_ready_micros: 0,
        }
    }

    fn spell(cost: u32, cast_time_ms: u32) -> Spell {
        Spell {
            cost,
            cast_time_ms,
            ..Spell::blank(1, "Modified fixture")
        }
    }

    fn cast() -> ModifiedCast {
        ModifiedCast {
            began_at: Timestamp::from_micros_since_unix_epoch(BEGAN_MICROS),
            pays_cost: true,
            rolls_crit: true,
        }
    }

    fn uses(rows: Vec<Aura>, hdr: &Spell, cast: ModifiedCast) -> usize {
        used_rows(rows, hdr, cast).len()
    }

    #[test]
    fn a_crit_chance_modifier_adds_its_flat_percent_then_scales_by_its_percent() {
        assert_eq!(crit_chance_under(4.5, (100, 0)), 104.5);
        assert_eq!(crit_chance_under(10.0, (5, 50)), 22.5);
        assert_eq!(crit_chance_under(0.0, (5, 50)), 5.0);
        assert_eq!(crit_chance_under(3.0, (-10, 0)), 0.0);
    }

    #[test]
    fn a_crit_chance_modifier_is_used_only_by_a_cast_whose_crit_roll_reads_it() {
        let cold_blood = || {
            vec![modifier(
                A_SPELLMOD_FLAT,
                SPELLMOD_OP_CRITICAL_CHANCE,
                100,
                1,
            )]
        };
        assert_eq!(uses(cold_blood(), &spell(0, 0), cast()), 1);
        let no_crit_roll = ModifiedCast {
            rolls_crit: false,
            ..cast()
        };
        assert_eq!(uses(cold_blood(), &spell(0, 0), no_crit_roll), 0);
    }

    #[test]
    fn a_spell_rolls_a_modified_crit_when_it_can_crit_with_a_damage_class_or_as_a_potion() {
        use crate::spell::{E_HEAL, E_TRIGGER};
        let of_class = |damage_class| Spell {
            damage_class,
            ..Spell::blank(1, "Crit fixture")
        };
        let effect = |kind| SpellEffect {
            kind,
            ..SpellEffect::blank(1, 0)
        };
        let heal = [effect(E_HEAL)];
        assert!(rolls_modified_crit(&of_class(DAMAGE_CLASS_MAGIC), &heal));
        assert!(rolls_modified_crit(&of_class(DAMAGE_CLASS_MELEE), &heal));
        assert!(!rolls_modified_crit(&of_class(0), &heal));
        let potion = Spell {
            family_name: SPELL_FAMILY_POTION,
            ..of_class(0)
        };
        assert!(rolls_modified_crit(&potion, &heal));
        let dummy = [effect(E_TRIGGER)];
        assert!(!rolls_modified_crit(&of_class(DAMAGE_CLASS_MAGIC), &dummy));
    }

    #[test]
    fn a_percent_cost_modifier_is_kept_by_a_free_spell_and_used_by_a_costly_one() {
        let clearcasting = || vec![modifier(A_SPELLMOD_PCT, SPELLMOD_OP_COST, -100, 1)];
        assert_eq!(uses(clearcasting(), &spell(0, 0), cast()), 0);
        assert_eq!(uses(clearcasting(), &spell(10, 0), cast()), 1);
        let item_cast = ModifiedCast {
            pays_cost: false,
            ..cast()
        };
        assert_eq!(uses(clearcasting(), &spell(10, 0), item_cast), 0);
    }

    #[test]
    fn an_uncharged_modifier_is_never_used() {
        let talent = vec![modifier(A_SPELLMOD_FLAT, SPELLMOD_OP_COST, -5, 0)];
        assert_eq!(uses(talent, &spell(10, 0), cast()), 0);
    }

    #[test]
    fn a_cast_time_modifier_gained_after_the_cast_began_is_kept() {
        let mut natures_grace = modifier(A_SPELLMOD_FLAT, SPELLMOD_OP_CASTING_TIME, -500, 1);
        natures_grace.applied_at = Timestamp::from_micros_since_unix_epoch(BEGAN_MICROS + 1);
        assert_eq!(uses(vec![natures_grace], &spell(0, 2000), cast()), 0);
    }

    #[test]
    fn a_cast_time_modifier_is_kept_by_a_spell_without_a_cast_time() {
        let natures_grace = vec![modifier(A_SPELLMOD_FLAT, SPELLMOD_OP_CASTING_TIME, -500, 1)];
        assert_eq!(uses(natures_grace, &spell(0, 0), cast()), 0);
    }

    #[test]
    fn an_instant_cast_modifier_is_kept_by_a_cast_of_ten_seconds() {
        let presence_of_mind = || vec![modifier(A_SPELLMOD_PCT, SPELLMOD_OP_CASTING_TIME, -100, 1)];
        assert_eq!(uses(presence_of_mind(), &spell(0, 10_000), cast()), 0);
        assert_eq!(uses(presence_of_mind(), &spell(0, 9_999), cast()), 1);
    }

    #[test]
    fn natures_grace_is_kept_by_a_cast_natures_swiftness_made_instant() {
        let rows = vec![
            modifier(A_SPELLMOD_PCT, SPELLMOD_OP_CASTING_TIME, -100, 1),
            Aura {
                spell_id: 2,
                ..modifier(A_SPELLMOD_FLAT, SPELLMOD_OP_CASTING_TIME, -500, 1)
            },
        ];
        let used = used_rows(rows, &spell(0, 2000), cast());
        assert_eq!(used.iter().map(|row| row.spell_id).collect::<Vec<_>>(), [1]);
    }
}
