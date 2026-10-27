use spacetimedb::{log, table, ReducerContext, Table};

use crate::game_world_entity;

use super::*;

#[table(accessor = game_spell_group, index(accessor = by_group, btree(columns = [group_id])), index(accessor = by_spell, btree(columns = [spell_id])))]
pub struct SpellGroup {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub group_id: u32,
    pub spell_id: u32,
}

#[table(accessor = game_spell_group_rule)]
pub struct SpellGroupRule {
    #[primary_key]
    pub group_id: u32,
    pub rule: u8,
    // Rank numbers from different spell chains do not measure comparable strength.
    #[default(true)]
    pub rank_is_comparable: bool,
}

#[allow(dead_code)]
pub(crate) const RULE_STACKS: u8 = 0;
pub(crate) const RULE_EXCLUSIVE: u8 = 1;
pub(crate) const RULE_EXCLUSIVE_PER_CASTER: u8 = 2;
pub(crate) const RULE_EXCLUSIVE_STRONGER: u8 = 3;

// Apply-time expiry keeps recasts in the window; aura removal resets it from the removal time.
#[table(accessor = game_dr_state, index(accessor = by_target, btree(columns = [target_guid])))]
pub struct DrState {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub target_guid: u64,
    pub category: u8,
    pub level: u8,
    pub window_expires_micros: i64,
}

// DR category numbers follow cmangos, not the M_* mechanic constants.
pub(crate) const DR_CAT_STUN: u8 = 1;
pub(crate) const DR_CAT_FEAR: u8 = 2;
pub(crate) const DR_CAT_ROOT: u8 = 4;
pub(crate) const DR_CAT_POLY: u8 = 6;

// The 15-second window starts when crowd control ends.
pub(crate) const DR_WINDOW_MICROS: i64 = 15_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AuraSummary {
    pub aura_id: u64,
    pub caster_guid: u64,
    pub strength: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupRule {
    Stacks,
    Exclusive,
    ExclusivePerCaster,
    ExclusiveStronger,
}

pub(crate) fn group_rule_from_u8(rule: u8) -> GroupRule {
    match rule {
        RULE_EXCLUSIVE => GroupRule::Exclusive,
        RULE_EXCLUSIVE_PER_CASTER => GroupRule::ExclusivePerCaster,
        RULE_EXCLUSIVE_STRONGER => GroupRule::ExclusiveStronger,
        _ => GroupRule::Stacks,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DrWindow {
    pub level: u8,
    pub window_expires_micros: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ApplyDecision {
    Apply {
        duration_scale_bp: u32,
        evict: Vec<u64>,
    },
    Refuse {
        reason: &'static str,
    },
}

pub(crate) fn resolve_group_conflict(
    caster_guid: u64,
    strength: i32,
    rule: GroupRule,
    existing: &[AuraSummary],
) -> ApplyDecision {
    match rule {
        GroupRule::Stacks => ApplyDecision::Apply {
            duration_scale_bp: 10_000,
            evict: Vec::new(),
        },
        GroupRule::Exclusive => ApplyDecision::Apply {
            duration_scale_bp: 10_000,
            evict: existing.iter().map(|a| a.aura_id).collect(),
        },
        GroupRule::ExclusivePerCaster => ApplyDecision::Apply {
            duration_scale_bp: 10_000,
            evict: existing
                .iter()
                .filter(|a| a.caster_guid == caster_guid)
                .map(|a| a.aura_id)
                .collect(),
        },
        GroupRule::ExclusiveStronger => {
            if let Some(strongest) = existing.iter().map(|a| a.strength).max() {
                if strongest > strength {
                    return ApplyDecision::Refuse { reason: "weaker" };
                }
            }
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: existing.iter().map(|a| a.aura_id).collect(),
            }
        }
    }
}

pub(crate) fn group_strength_satisfied(strength: i32, existing: &[AuraSummary]) -> bool {
    existing.iter().any(|aura| aura.strength >= strength)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuffGroupStatus {
    NoGroup,
    Missing,
    Satisfied,
    Unavailable,
}

pub(crate) fn buff_group_status(
    ctx: &ReducerContext,
    spell_id: u32,
    caster_level: u8,
    auras: &[Aura],
    family_limit: usize,
) -> BuffGroupStatus {
    let groups = ctx.db.game_spell_group();
    let memberships: Vec<_> = groups.by_spell().filter(&spell_id).take(2).collect();
    let [membership] = memberships.as_slice() else {
        return if memberships.is_empty() {
            BuffGroupStatus::NoGroup
        } else {
            BuffGroupStatus::Unavailable
        };
    };
    let group_id = membership.group_id;
    let Some(rule) = ctx.db.game_spell_group_rule().group_id().find(group_id) else {
        return BuffGroupStatus::Unavailable;
    };
    if group_rule_from_u8(rule.rule) != GroupRule::ExclusiveStronger {
        return BuffGroupStatus::Missing;
    }
    let Some(header) = ctx.db.game_spell().spell_id().find(spell_id) else {
        return BuffGroupStatus::Unavailable;
    };
    let effects: Vec<_> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .take(4)
        .collect();
    if effects.len() > 3 {
        return BuffGroupStatus::Unavailable;
    }
    let aura_effects: Vec<_> = effects
        .into_iter()
        .filter(|effect| effect.kind & KIND_AURA_BIT != 0)
        .collect();
    if aura_effects.is_empty() {
        return BuffGroupStatus::Unavailable;
    }
    let incoming = aura_effects
        .into_iter()
        .map(|effect| {
            compute_group_strength(
                rule.rank_is_comparable,
                rank_of(ctx, spell_id),
                effect_amount(
                    effect.base_points,
                    effect.die_sides,
                    effect.per_level,
                    u32::from(caster_level),
                    header.spell_level,
                    header.max_level,
                    0,
                )
                .saturating_abs(),
            )
        })
        .max()
        .unwrap_or(0);
    let family_rows: Vec<_> = groups
        .by_group()
        .filter(&group_id)
        .take(family_limit.saturating_add(1))
        .collect();
    if family_rows.len() > family_limit {
        return BuffGroupStatus::Unavailable;
    }
    let family: std::collections::HashSet<_> =
        family_rows.into_iter().map(|row| row.spell_id).collect();
    let existing: Vec<_> = auras
        .iter()
        .filter(|aura| family.contains(&aura.spell_id))
        .map(|aura| AuraSummary {
            aura_id: aura.id,
            caster_guid: aura.caster_guid,
            strength: stacked_group_strength(
                rule.rank_is_comparable,
                rank_of(ctx, aura.spell_id),
                aura.amount,
                aura.stacks,
            ),
        })
        .collect();
    if group_strength_satisfied(incoming, &existing) {
        BuffGroupStatus::Satisfied
    } else {
        BuffGroupStatus::Missing
    }
}

pub(crate) fn resolve_dr(prior: Option<DrWindow>, now_micros: i64) -> ApplyDecision {
    match dr_scale_bp(dr_pre_level(prior, now_micros)) {
        Some(bp) => ApplyDecision::Apply {
            duration_scale_bp: bp,
            evict: Vec::new(),
        },
        None => ApplyDecision::Refuse { reason: "immune" },
    }
}

pub(crate) fn dr_pre_level(prior: Option<DrWindow>, now_micros: i64) -> u8 {
    match prior {
        Some(w) if now_micros <= w.window_expires_micros => w.level,
        _ => 0,
    }
}

pub(crate) fn dr_bump(
    prior: Option<DrWindow>,
    now_micros: i64,
    expires_at_micros: i64,
) -> DrWindow {
    DrWindow {
        level: (dr_pre_level(prior, now_micros) + 1).min(3),
        window_expires_micros: dr_window_expires(expires_at_micros),
    }
}

pub(crate) fn dr_window_expires(control_ends_micros: i64) -> i64 {
    control_ends_micros + DR_WINDOW_MICROS
}

pub(crate) fn dr_scaled_duration_ms(duration_ms: u32, duration_scale_bp: u32) -> u32 {
    ((duration_ms as u64) * (duration_scale_bp as u64) / 10_000) as u32
}

pub(crate) fn dr_scale_bp(pre_level: u8) -> Option<u32> {
    match pre_level {
        0 => Some(10_000),
        1 => Some(5_000),
        2 => Some(2_500),
        _ => None,
    }
}

pub(crate) fn compute_strength(rank: u8, magnitude: i32) -> i32 {
    (rank as i32) * 100_000 + magnitude.clamp(0, 99_999)
}

pub(crate) fn compute_group_strength(rank_is_comparable: bool, rank: u8, magnitude: i32) -> i32 {
    compute_strength(if rank_is_comparable { rank } else { 0 }, magnitude)
}

fn stacked_group_strength(rank_is_comparable: bool, rank: u8, amount: i32, stacks: u8) -> i32 {
    compute_group_strength(
        rank_is_comparable,
        rank,
        amount
            .saturating_abs()
            .saturating_mul(i32::from(stacks.max(1))),
    )
}

pub(crate) fn rank_of(ctx: &ReducerContext, spell_id: u32) -> u8 {
    ctx.db
        .game_spell_chain()
        .spell_id()
        .find(spell_id)
        .map(|c| c.rank)
        .unwrap_or(0)
}

pub(crate) fn dr_category_for_effect(
    ctx: &ReducerContext,
    eff_kind: u8,
    eff_p0_kind: u8,
    eff_p0: i32,
    target_guid: u64,
) -> Option<u8> {
    if eff_kind != A_CONTROL || eff_p0_kind != P_MECHANIC {
        return None;
    }
    let is_player = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .map(|t| t.is_player())
        .unwrap_or(false);
    if !is_player {
        return None;
    }
    dr_category_for_mechanic(eff_p0)
}

const fn dr_category_for_mechanic(mechanic: i32) -> Option<u8> {
    match mechanic {
        M_STUN => Some(DR_CAT_STUN),
        M_ROOT => Some(DR_CAT_ROOT),
        M_FEAR => Some(DR_CAT_FEAR),
        M_POLY => Some(DR_CAT_POLY),
        _ => None,
    }
}

pub(crate) fn apply_group_conflict(
    ctx: &ReducerContext,
    spell_id: u32,
    caster_guid: u64,
    target_guid: u64,
    incoming_rank: u8,
    incoming_magnitude: i32,
) -> bool {
    let groups = ctx.db.game_spell_group();
    let Some(group_id) = groups
        .by_spell()
        .filter(&spell_id)
        .map(|g| g.group_id)
        .next()
    else {
        return true;
    };
    let rule_row = ctx
        .db
        .game_spell_group_rule()
        .group_id()
        .find(group_id)
        .map(|r| (group_rule_from_u8(r.rule), r.rank_is_comparable))
        .unwrap_or((GroupRule::Stacks, true));
    let (rule, rank_is_comparable) = rule_row;
    if rule == GroupRule::Stacks {
        return true;
    }
    let member_ids: std::collections::HashSet<u32> = groups
        .by_group()
        .filter(&group_id)
        .map(|g| g.spell_id)
        .collect();
    let auras = ctx.db.game_aura();
    let members: Vec<AuraSummary> = auras
        .by_target()
        .filter(&target_guid)
        // Earlier effects of the same cast must not conflict with its later effects.
        .filter(|a| a.spell_id != spell_id && member_ids.contains(&a.spell_id))
        .map(|a| AuraSummary {
            aura_id: a.id,
            caster_guid: a.caster_guid,
            strength: stacked_group_strength(
                rank_is_comparable,
                rank_of(ctx, a.spell_id),
                a.amount,
                a.stacks,
            ),
        })
        .collect();
    let strength = compute_group_strength(rank_is_comparable, incoming_rank, incoming_magnitude);
    match resolve_group_conflict(caster_guid, strength, rule, &members) {
        ApplyDecision::Refuse { reason } => {
            log::info!(
                "stacking group {group_id}: spell {spell_id} onto {target_guid} refused ({reason})"
            );
            false
        }
        ApplyDecision::Apply { evict, .. } => {
            let shed = evict
                .into_iter()
                .filter_map(|id| auras.id().find(id))
                .collect();
            crate::spell::shed_auras(ctx, target_guid, shed);
            true
        }
    }
}

pub(crate) fn resolve_dr_for_target(
    ctx: &ReducerContext,
    target_guid: u64,
    category: u8,
    now_micros: i64,
) -> ApplyDecision {
    let prior = ctx
        .db
        .game_dr_state()
        .by_target()
        .filter(&target_guid)
        .find(|r| r.category == category)
        .map(|r| DrWindow {
            level: r.level,
            window_expires_micros: r.window_expires_micros,
        });
    resolve_dr(prior, now_micros)
}

pub(crate) fn bump_dr_level(
    ctx: &ReducerContext,
    target_guid: u64,
    category: u8,
    now_micros: i64,
    expires_at_micros: i64,
) {
    let table = ctx.db.game_dr_state();
    let existing = table
        .by_target()
        .filter(&target_guid)
        .find(|r| r.category == category);
    let prior = existing.as_ref().map(|r| DrWindow {
        level: r.level,
        window_expires_micros: r.window_expires_micros,
    });
    let bumped = dr_bump(prior, now_micros, expires_at_micros);
    let new_level = bumped.level;
    let new_window = bumped.window_expires_micros;
    match existing {
        Some(mut row) => {
            row.level = new_level;
            row.window_expires_micros = new_window;
            table.id().update(row);
        }
        None => {
            table.insert(DrState {
                id: 0,
                target_guid,
                category,
                level: new_level,
                window_expires_micros: new_window,
            });
        }
    }
}

pub(crate) fn dr_window_on_removal(
    ctx: &ReducerContext,
    target_guid: u64,
    category: u8,
    removed_at_micros: i64,
) {
    let table = ctx.db.game_dr_state();
    if let Some(mut row) = table
        .by_target()
        .filter(&target_guid)
        .find(|r| r.category == category)
    {
        row.window_expires_micros = dr_window_expires(removed_at_micros);
        table.id().update(row);
    }
}

pub(crate) fn sweep_dr_state(ctx: &ReducerContext) {
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let table = ctx.db.game_dr_state();
    let stale: Vec<u64> = table
        .iter()
        .filter(|r| r.window_expires_micros < now_micros)
        .map(|r| r.id)
        .collect();
    for id in stale {
        table.id().delete(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fortitude_rank_beats_lower_rank_any_caster_exclusive_stronger() {
        let rank1_b = AuraSummary {
            aura_id: 42,
            caster_guid: 200,
            strength: compute_strength(1, 20),
        };
        assert_eq!(
            resolve_group_conflict(
                100,
                compute_strength(2, 34),
                GroupRule::ExclusiveStronger,
                &[rank1_b],
            ),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![42]
            }
        );

        let rank2_b = AuraSummary {
            aura_id: 43,
            caster_guid: 200,
            strength: compute_strength(2, 34),
        };
        assert_eq!(
            resolve_group_conflict(
                100,
                compute_strength(1, 20),
                GroupRule::ExclusiveStronger,
                &[rank2_b],
            ),
            ApplyDecision::Refuse { reason: "weaker" }
        );
    }

    #[test]
    fn blessings_are_exclusive_per_caster_not_across_casters() {
        let might_a = AuraSummary {
            aura_id: 1,
            caster_guid: 500,
            strength: 0,
        };
        assert_eq!(
            resolve_group_conflict(500, 0, GroupRule::ExclusivePerCaster, &[might_a]),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![1]
            }
        );
        assert_eq!(
            resolve_group_conflict(600, 0, GroupRule::ExclusivePerCaster, &[might_a]),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
    }

    #[test]
    fn armor_debuff_strength_folds_sunders_active_stack_count() {
        const EXPOSE_5_COMBO: i32 = 2050;
        let armor_strength =
            |rank, amount, stacks| stacked_group_strength(false, rank, amount, stacks);

        let sunder_1_stack = AuraSummary {
            aura_id: 8,
            caster_guid: 2,
            strength: armor_strength(5, -450, 1),
        };
        assert_eq!(sunder_1_stack.strength, 450);
        assert_eq!(
            resolve_group_conflict(
                1,
                armor_strength(5, -EXPOSE_5_COMBO, 1),
                GroupRule::ExclusiveStronger,
                &[sunder_1_stack],
            ),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![8]
            }
        );

        let sunder_5_stack = AuraSummary {
            aura_id: 8,
            caster_guid: 2,
            strength: armor_strength(5, -450, 5),
        };
        assert_eq!(sunder_5_stack.strength, 2250);
        assert_eq!(
            resolve_group_conflict(
                1,
                armor_strength(5, -EXPOSE_5_COMBO, 1),
                GroupRule::ExclusiveStronger,
                &[sunder_5_stack],
            ),
            ApplyDecision::Refuse { reason: "weaker" }
        );

        let expose_existing = AuraSummary {
            aura_id: 7,
            caster_guid: 1,
            strength: armor_strength(5, -EXPOSE_5_COMBO, 1),
        };
        assert_eq!(
            resolve_group_conflict(
                2,
                armor_strength(5, -450, 1),
                GroupRule::ExclusiveStronger,
                &[expose_existing],
            ),
            ApplyDecision::Refuse { reason: "weaker" }
        );
    }

    #[test]
    fn fortitude_family_replaces_and_refuses_by_magnitude() {
        const PW_FORT_R5: i32 = 43;
        const PW_FORT_R6: i32 = 54;
        let rank5 = AuraSummary {
            aura_id: 42,
            caster_guid: 200,
            strength: compute_group_strength(false, 5, PW_FORT_R5),
        };
        assert_eq!(
            resolve_group_conflict(
                100,
                compute_group_strength(false, 6, PW_FORT_R6),
                GroupRule::ExclusiveStronger,
                &[rank5],
            ),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![42]
            }
        );

        let rank6 = AuraSummary {
            aura_id: 43,
            caster_guid: 200,
            strength: compute_group_strength(false, 6, PW_FORT_R6),
        };
        assert_eq!(
            resolve_group_conflict(
                100,
                compute_group_strength(false, 5, PW_FORT_R5),
                GroupRule::ExclusiveStronger,
                &[rank6],
            ),
            ApplyDecision::Refuse { reason: "weaker" }
        );

        assert_eq!(
            resolve_group_conflict(
                100,
                compute_group_strength(false, 2, PW_FORT_R6),
                GroupRule::ExclusiveStronger,
                &[rank6],
            ),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![43]
            }
        );
    }

    #[test]
    fn stacks_rule_is_a_full_duration_noop() {
        let other = AuraSummary {
            aura_id: 99,
            caster_guid: 1,
            strength: 1_000_000,
        };
        assert_eq!(
            resolve_group_conflict(2, 0, GroupRule::Stacks, &[other]),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
    }

    #[test]
    #[allow(clippy::erasing_op)]
    fn poly_dr_timeline_matches_the_verbatim_vector() {
        const SECOND: i64 = 1_000_000;

        let t0 = 0 * SECOND;
        let decision0 = resolve_dr(None, t0);
        assert_eq!(
            decision0,
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
        assert_eq!(dr_pre_level(None, t0), 0);

        let row_after_removal_1 = DrWindow {
            level: 1,
            window_expires_micros: 25 * SECOND,
        };

        let t12 = 12 * SECOND;
        let decision12 = resolve_dr(Some(row_after_removal_1), t12);
        assert_eq!(
            decision12,
            ApplyDecision::Apply {
                duration_scale_bp: 5_000,
                evict: vec![]
            }
        );
        assert_eq!(dr_pre_level(Some(row_after_removal_1), t12), 1);
        let row_after_apply_2 = DrWindow {
            level: 2,
            window_expires_micros: 32 * SECOND,
        };

        let t16 = 16 * SECOND;
        let decision16 = resolve_dr(Some(row_after_apply_2), t16);
        assert_eq!(
            decision16,
            ApplyDecision::Apply {
                duration_scale_bp: 2_500,
                evict: vec![]
            }
        );
        assert_eq!(dr_pre_level(Some(row_after_apply_2), t16), 2);
        let row_after_removal_3 = DrWindow {
            level: 3,
            window_expires_micros: dr_window_expires(18 * SECOND + SECOND / 2),
        };
        assert_eq!(row_after_removal_3.window_expires_micros, 33_500_000);

        let t20 = 20 * SECOND;
        let decision20 = resolve_dr(Some(row_after_removal_3), t20);
        assert_eq!(decision20, ApplyDecision::Refuse { reason: "immune" });
        assert_eq!(dr_pre_level(Some(row_after_removal_3), t20), 3);

        let t40_1 = 40 * SECOND + SECOND / 10;
        let decision40 = resolve_dr(Some(row_after_removal_3), t40_1);
        assert_eq!(
            decision40,
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
        assert_eq!(dr_pre_level(Some(row_after_removal_3), t40_1), 0);
    }

    struct DrProbe {
        row: Option<DrWindow>,
    }

    impl DrProbe {
        fn apply(&mut self, now: i64, base_ms: u32) -> Option<i64> {
            match resolve_dr(self.row, now) {
                ApplyDecision::Refuse { reason } => {
                    assert_eq!(reason, "immune");
                    None
                }
                ApplyDecision::Apply {
                    duration_scale_bp,
                    evict,
                } => {
                    assert!(evict.is_empty(), "a DR decision never evicts another aura");
                    let expires =
                        now + i64::from(dr_scaled_duration_ms(base_ms, duration_scale_bp)) * 1_000;
                    self.row = Some(dr_bump(self.row, now, expires));
                    Some(expires)
                }
            }
        }

        fn remove(&mut self, at: i64) {
            let row = self
                .row
                .as_mut()
                .expect("a removed CC aura always has a DR row");
            row.window_expires_micros = dr_window_expires(at);
        }
    }

    #[test]
    fn player_target_dr_timeline_end_to_end_on_a_twenty_second_control() {
        const S: i64 = 1_000_000;
        const POLY_MS: u32 = 20_000;
        let mut probe = DrProbe { row: None };

        let expiry_1 = probe
            .apply(0, POLY_MS)
            .expect("a fresh target is never immune");
        assert_eq!(expiry_1, 20 * S);
        let row_1 = probe.row.expect("a successful application persists a row");
        assert_eq!(row_1.level, 1);
        assert_eq!(row_1.window_expires_micros, 35 * S);

        let expiry_2 = probe.apply(3 * S, POLY_MS).expect("level 1 still applies");
        assert_eq!(expiry_2, 13 * S);
        let row_2 = probe.row.expect("row persists");
        assert_eq!(row_2.level, 2);
        assert_eq!(row_2.window_expires_micros, 28 * S);

        let provisional = row_2.window_expires_micros;
        probe.remove(5 * S);
        let row_3 = probe.row.expect("row persists");
        assert_eq!(row_3.window_expires_micros, 20 * S);
        assert!(row_3.window_expires_micros < provisional);
        assert_eq!(row_3.level, 2);

        let expiry_3 = probe.apply(8 * S, POLY_MS).expect("level 2 still applies");
        assert_eq!(expiry_3, 13 * S);
        assert_eq!(probe.row.expect("row persists").level, 3);

        let before_immune = probe.row;
        assert_eq!(probe.apply(10 * S, POLY_MS), None);
        assert_eq!(probe.row, before_immune);

        let removed_at = 13 * S + S / 5;
        assert!(removed_at >= expiry_3);
        probe.remove(removed_at);
        let row_4 = probe.row.expect("row persists");
        assert_eq!(row_4.window_expires_micros, 28_200_000);
        assert_eq!(row_4.level, 3);

        assert_eq!(probe.apply(row_4.window_expires_micros, POLY_MS), None);

        let restart = row_4.window_expires_micros + 1;
        let expiry_4 = probe
            .apply(restart, POLY_MS)
            .expect("a lapsed window is a fresh target");
        assert_eq!(expiry_4 - restart, 20 * S);
        assert_eq!(probe.row.expect("row persists").level, 1);
    }

    #[test]
    fn creature_target_skips_dr_entirely_full_duration_both_casts() {
        assert_eq!(
            resolve_dr(None, 0),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
        assert_eq!(
            resolve_dr(None, 999_999_999),
            ApplyDecision::Apply {
                duration_scale_bp: 10_000,
                evict: vec![]
            }
        );
    }

    #[test]
    fn dr_scale_ladder_and_window_lapse_reset() {
        assert_eq!(dr_scale_bp(0), Some(10_000));
        assert_eq!(dr_scale_bp(1), Some(5_000));
        assert_eq!(dr_scale_bp(2), Some(2_500));
        assert_eq!(dr_scale_bp(3), None);
        assert_eq!(dr_scale_bp(4), None);
        assert_eq!(dr_pre_level(None, 0), 0);
        assert_eq!(dr_pre_level(None, 1_000_000_000), 0);
        let w = DrWindow {
            level: 2,
            window_expires_micros: 100,
        };
        assert_eq!(dr_pre_level(Some(w), 100), 2);
        assert_eq!(dr_pre_level(Some(w), 101), 0);
    }

    #[test]
    fn dr_category_mapping_covers_every_current_mechanic() {
        assert_eq!(dr_category_for_mechanic(M_STUN), Some(DR_CAT_STUN));
        assert_eq!(dr_category_for_mechanic(M_ROOT), Some(DR_CAT_ROOT));
        assert_eq!(dr_category_for_mechanic(M_FEAR), Some(DR_CAT_FEAR));
        assert_eq!(dr_category_for_mechanic(M_POLY), Some(DR_CAT_POLY));
        assert_eq!(dr_category_for_mechanic(0), None);
        assert_eq!(dr_category_for_mechanic(999), None);
    }

    #[test]
    fn compute_strength_ranks_first_then_magnitude() {
        assert!(compute_strength(2, 1) > compute_strength(1, 99_999));
        assert!(compute_strength(0, 200) > compute_strength(0, 100));
        assert_eq!(compute_strength(0, -50), 0);
    }

    #[test]
    fn heterogeneous_groups_ignore_unrelated_spell_chain_ranks() {
        assert!(compute_group_strength(false, 1, 2_050) > compute_group_strength(false, 5, 450));
        assert!(compute_group_strength(true, 2, 1) > compute_group_strength(true, 1, 99_999));
    }

    #[test]
    fn group_rule_decode_and_safe_fallback() {
        assert_eq!(group_rule_from_u8(RULE_STACKS), GroupRule::Stacks);
        assert_eq!(group_rule_from_u8(RULE_EXCLUSIVE), GroupRule::Exclusive);
        assert_eq!(
            group_rule_from_u8(RULE_EXCLUSIVE_PER_CASTER),
            GroupRule::ExclusivePerCaster
        );
        assert_eq!(
            group_rule_from_u8(RULE_EXCLUSIVE_STRONGER),
            GroupRule::ExclusiveStronger
        );
        assert_eq!(group_rule_from_u8(255), GroupRule::Stacks);
    }
}
