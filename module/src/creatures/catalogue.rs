use spacetimedb::{table, ReducerContext};

use crate::CreatureTemplate;

#[table(accessor = game_creature_defense, public)]
pub struct CreatureDefense {
    #[primary_key]
    pub entry: u32,
    pub no_parry: bool,
    pub no_block: bool,
    pub no_defense: bool,
}

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

/// One of a template's virtual equipment choices, by item entry; one is chosen by weight at spawn,
/// and a choice of weight 0 never is.
#[table(accessor = game_creature_equipment, public, index(accessor = by_entry, btree(columns = [entry])))]
pub struct CreatureEquipment {
    #[primary_key]
    pub id: u64,
    pub entry: u32,
    pub weight: u32,
    pub main_hand: u32,
    pub off_hand: u32,
    pub ranged: u32,
}

/// A display's combat reach in yards, measured at `display_scale`, its CreatureDisplayInfo.dbc scale.
#[table(accessor = game_display_combat_reach, public)]
pub struct DisplayCombatReach {
    #[primary_key]
    pub display_id: u32,
    pub combat_reach: f32,
    pub display_scale: f32,
}

crate::outside_table_catalog!(
    game_creature_defense,
    game_creature_level_stats,
    game_creature_display,
    game_creature_equipment,
    game_display_combat_reach
);

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

/// What a template's equipment gives a creature, as vmangos `Creature::LoadEquipment` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TemplateEquipment {
    NoEquipment,
    /// The template has choices but their weights total 0, so the current slots stay.
    Unpicked,
    Items([u32; 3]),
}

/// vmangos `EquipmentTemplate::ChooseEquipmentEntry`: a choice of template `entry`'s equipment,
/// by weight.
pub(crate) fn equipment(ctx: &ReducerContext, entry: u32) -> TemplateEquipment {
    let mut choices: Vec<_> = ctx
        .db
        .game_creature_equipment()
        .by_entry()
        .filter(entry)
        .collect();
    if choices.is_empty() {
        return TemplateEquipment::NoEquipment;
    }
    choices.sort_by_key(|choice| choice.id);
    let total: u64 = choices.iter().map(|choice| u64::from(choice.weight)).sum();
    if total == 0 {
        return TemplateEquipment::Unpicked;
    }
    let mut roll = u64::from(ctx.random::<u32>()) % total;
    for choice in choices {
        if roll < u64::from(choice.weight) {
            return TemplateEquipment::Items([choice.main_hand, choice.off_hand, choice.ranged]);
        }
        roll -= u64::from(choice.weight);
    }
    TemplateEquipment::Unpicked
}

/// The template's display rows when the first one repeats the template's own display and scale.
fn template_displays(ctx: &ReducerContext, template: &CreatureTemplate) -> Vec<CreatureDisplay> {
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
        displays.clear();
    }
    displays
}

pub(crate) fn display(ctx: &ReducerContext, template: &CreatureTemplate, roll: u32) -> (u32, f32) {
    let fallback = (template.display_id, template.scale);
    select_display(&template_displays(ctx, template), roll)
        .map_or(fallback, |d| (d.display_id, d.scale))
}

/// The scale of a creature of `template` whose native model is `display_id`, as `display` chose it
/// at spawn; scale 0 reads as 1, as vmangos `CheckValidScale` does.
pub(crate) fn native_scale(
    ctx: &ReducerContext,
    template: &CreatureTemplate,
    display_id: u32,
) -> f32 {
    scale_of_display(&template_displays(ctx, template), template, display_id)
}

fn scale_of_display(
    displays: &[CreatureDisplay],
    template: &CreatureTemplate,
    display_id: u32,
) -> f32 {
    let scale = displays
        .iter()
        .find(|d| d.display_id == display_id)
        .map_or(template.scale, |d| d.scale);
    if scale > 0.0 {
        scale
    } else {
        1.0
    }
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

    fn big_template() -> CreatureTemplate {
        CreatureTemplate {
            display_id: 100,
            scale: 2.0,
            ..CreatureTemplate::blank(10, "Giant")
        }
    }

    #[test]
    fn a_native_scale_without_matching_display_rows_is_the_templates() {
        assert_eq!(scale_of_display(&[], &big_template(), 100), 2.0);
        assert_eq!(scale_of_display(&[], &big_template(), 777), 2.0);
        let unscaled = CreatureTemplate {
            scale: 0.0,
            ..big_template()
        };
        assert_eq!(scale_of_display(&[], &unscaled, 100), 1.0);
    }

    #[test]
    fn a_native_scale_with_display_rows_is_the_scale_of_its_model() {
        let row = |id, display_id, scale| CreatureDisplay {
            id,
            entry: 10,
            display_id,
            scale,
            weight: 1,
            gender: 0,
            bounding_radius: 0.389,
        };
        let rows = [row(1, 100, 2.0), row(2, 200, 0.5)];
        assert_eq!(scale_of_display(&rows, &big_template(), 200), 0.5);
        assert_eq!(scale_of_display(&rows, &big_template(), 100), 2.0);
    }
}
