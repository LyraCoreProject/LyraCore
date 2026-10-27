use spacetimedb::{table, ReducerContext};

use super::CreatureTemplate;

#[table(accessor = game_creature_level_stats, public, index(accessor = by_entry, btree(columns = [entry])))]
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

#[table(accessor = game_creature_display, public, index(accessor = by_entry, btree(columns = [entry])), index(accessor = by_display, btree(columns = [display_id])))]
pub struct CreatureDisplay {
    #[primary_key]
    pub id: u64,
    pub entry: u32,
    pub display_id: u32,
    pub scale: f32,
    pub weight: u32,
    #[default(0u8)]
    pub gender: u8,
    #[default(0.389f32)]
    pub bounding_radius: f32,
}

pub(crate) fn display_gender(ctx: &ReducerContext, display_id: u32) -> u8 {
    ctx.db
        .game_creature_display()
        .by_display()
        .filter(display_id)
        .next()
        .map_or(0, |display| display.gender)
}

pub(crate) fn level_stats(
    ctx: &ReducerContext,
    template: &CreatureTemplate,
    level: u32,
) -> Option<CreatureLevelStats> {
    let table = ctx.db.game_creature_level_stats();
    // Presence preserves the imported stat convention through Package template edits.
    table.by_entry().filter(template.entry).next()?;
    if level < template.level || level > template.max_level.max(template.level) {
        return None;
    }
    let key = |level| (u64::from(template.entry) << 8) | u64::from(level);
    if level <= 255 && template.level <= 255 && template.max_level <= 255 {
        if let (Some(base), Some(last)) = (
            table.id().find(key(template.level)),
            table.id().find(key(template.max_level)),
        ) {
            if base.health == template.health
                && last.health == template.max_level_health
                && base.damage_min == template.damage_min
                && base.damage_max == template.damage_max
                && base.armor == template.armor
            {
                return table.id().find(key(level));
            }
        }
    }
    let (_, health) = super::rolled_creature_stats(
        template.level,
        template.max_level,
        template.health,
        template.max_level_health,
        level.saturating_sub(template.level),
    );
    Some(CreatureLevelStats {
        id: key(level),
        entry: template.entry,
        level,
        health,
        damage_min: template.damage_min,
        damage_max: template.damage_max,
        armor: template.armor,
    })
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
                gender: 0,
                bounding_radius: 0.389,
            },
            CreatureDisplay {
                id: 2,
                entry: 10,
                display_id: 200,
                scale: 2.0,
                weight: 3,
                gender: 1,
                bounding_radius: 0.389,
            },
            CreatureDisplay {
                id: 3,
                entry: 10,
                display_id: 300,
                scale: 1.0,
                weight: 0,
                gender: 2,
                bounding_radius: 0.389,
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
