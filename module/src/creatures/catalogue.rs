use spacetimedb::{table, ReducerContext};

use super::CreatureTemplate;

#[table(accessor = game_creature_level_stats, public)]
pub struct CreatureLevelStats {
    #[primary_key]
    pub id: u64,
    pub entry: u32,
    pub level: u32,
    pub health: u32,
    pub damage_min: u32,
    pub damage_max: u32,
    pub armor: u32,
}

#[table(accessor = game_creature_display, public, index(accessor = by_entry, btree(columns = [entry])))]
pub struct CreatureDisplay {
    #[primary_key]
    pub id: u64,
    pub entry: u32,
    pub display_id: u32,
    pub scale: f32,
    pub weight: u32,
}

pub(crate) fn level_stats(
    ctx: &ReducerContext,
    template: &CreatureTemplate,
    level: u32,
) -> Option<CreatureLevelStats> {
    if level > 255 || template.level > 255 || template.max_level > 255 {
        return None;
    }
    let table = ctx.db.game_creature_level_stats();
    let key = |level| (u64::from(template.entry) << 8) | u64::from(level);
    let base = table.id().find(key(template.level))?;
    let last = table.id().find(key(template.max_level))?;
    // A Package's template stat changes take precedence over imported level rows.
    if base.health != template.health
        || last.health != template.max_level_health
        || base.damage_min != template.damage_min
        || base.damage_max != template.damage_max
        || base.armor != template.armor
    {
        return None;
    }
    table.id().find(key(level))
}

fn select_display(displays: &[CreatureDisplay], roll: u32) -> Option<&CreatureDisplay> {
    let total: u64 = displays.iter().map(|d| u64::from(d.weight)).sum();
    if total == 0 {
        return None;
    }
    let mut roll = u64::from(roll) % total;
    for display in displays {
        if roll < u64::from(display.weight) {
            return Some(display);
        }
        roll -= u64::from(display.weight);
    }
    None
}

pub(crate) fn display(ctx: &ReducerContext, template: &CreatureTemplate, roll: u32) -> (u32, f32) {
    let fallback = (template.display_id, template.scale);
    let mut displays: Vec<_> = ctx
        .db
        .game_creature_display()
        .by_entry()
        .filter(template.entry)
        .collect();
    displays.sort_by_key(|d| d.id);
    if displays
        .first()
        .is_none_or(|d| d.display_id != template.display_id || d.scale != template.scale)
    {
        return fallback;
    }
    select_display(&displays, roll).map_or(fallback, |d| (d.display_id, d.scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_weights_keep_the_selected_models_scale() {
        let rows = [
            CreatureDisplay {
                id: 1,
                entry: 10,
                display_id: 100,
                scale: 0.5,
                weight: 1,
            },
            CreatureDisplay {
                id: 2,
                entry: 10,
                display_id: 200,
                scale: 2.0,
                weight: 3,
            },
            CreatureDisplay {
                id: 3,
                entry: 10,
                display_id: 300,
                scale: 1.0,
                weight: 0,
            },
        ];
        let outcomes: Vec<_> = (0..4)
            .map(|roll| {
                let d = select_display(&rows, roll).unwrap();
                (d.display_id, d.scale)
            })
            .collect();
        assert_eq!(outcomes, [(100, 0.5), (200, 2.0), (200, 2.0), (200, 2.0)]);
    }
}
