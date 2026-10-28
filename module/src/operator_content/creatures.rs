use std::collections::BTreeMap;

use lyracore_package_delta::ids::{
    is_fixture_reserved_creature_id, is_fixture_reserved_loot_id, is_operator_template_id,
    is_package_cast_id, is_package_creature_id, is_package_loot_id, FIXTURE_RESERVED_ID_CEIL,
    MAX_CREATURE_GUID_COMPONENT, OPERATOR_TEMPLATE_ID_CEIL, OPERATOR_TEMPLATE_ID_FLOOR,
    PACKAGE_CAST_ID_CEIL, PACKAGE_LOOT_ID_CEIL,
};
use lyracore_protocol::messages::vanilla::{Class, CreatureFamily};
use spacetimedb::{reducer, ReducerContext, SpacetimeType, Table};

use crate::creatures::catalogue::{game_creature_display, CreatureDisplay as DisplayRow};
use crate::creatures::{
    game_creature_cast, game_creature_spawn, game_creature_spell, game_creature_template,
    CreatureCast, CreatureSpell as SpellRow, CreatureTemplate,
};
use crate::faction::game_faction_template;
use crate::items::game_item_template;
use crate::loot::{
    game_creature_loot, game_pickpocket_loot, game_skinning_loot, CreatureLoot, GamePickpocketLoot,
    GameSkinningLoot,
};
use crate::{game_spell, game_world_entity};

const MAX_EDITS: usize = 64;
const MAX_LIST_ROWS: usize = 128;
const MAX_BATCH_ROWS: usize = 512;
// See docs/operator-content-api.md for the measured respawn budget.
const MAX_RESPAWNS: usize = 2_000;
const MAX_TEXT_BYTES: usize = 255;

// Exhaustive construction makes a new template column a compile failure here.
macro_rules! creature_fields {
    ($($field:ident: $ty:ty),* $(,)?) => {
        #[derive(SpacetimeType, Clone, Default)]
        pub struct CreatureFields { $(pub $field: $ty,)* }

        impl CreatureFields {
            fn into_template(self, entry: u32) -> CreatureTemplate {
                CreatureTemplate { entry, $($field: self.$field,)* }
            }

            fn of_template(row: &CreatureTemplate) -> Self {
                Self { $($field: row.$field.clone(),)* }
            }

            fn changes(&self, old: Option<&CreatureTemplate>) -> String {
                let mut changes = Vec::new();
                $(if old.is_none_or(|old| old.$field != self.$field) {
                    changes.push(format!("{} {:?} -> {:?}",
                        stringify!($field), old.map(|old| &old.$field), self.$field));
                })*
                changes.join(", ")
            }
        }
    };
}

creature_fields! {
    name: String,
    subname: String,
    display_id: u32,
    level: u32,
    health: u32,
    faction_template: u32,
    npc_flags: u32,
    unit_flags: u32,
    creature_type: u8,
    creature_family: u8,
    type_flags: u32,
    rank: u8,
    scale: f32,
    base_attack_time_ms: u32,
    money_min: u32,
    money_max: u32,
    max_level: u32,
    max_level_health: u32,
    aggro_range: u32,
    damage_min: u32,
    damage_max: u32,
    armor: u32,
    pickpocket_loot_id: u32,
    skin_loot_id: u32,
    trainer_type: u8,
    trainer_class: u8,
}

#[derive(SpacetimeType, Clone)]
pub struct CreatureCreate {
    pub fields: CreatureFields,
}

#[derive(SpacetimeType, Clone)]
pub struct CreatureUpdate {
    pub entry: u32,
    pub fields: CreatureFields,
}

#[derive(SpacetimeType, Clone, Debug)]
#[sats(name = "OperatorCreatureDisplay")]
pub struct CreatureDisplay {
    pub display_id: u32,
    pub scale: f32,
    pub weight: u32,
    pub gender: u8,
    pub bounding_radius: f32,
}

#[derive(SpacetimeType, Clone)]
pub struct CreatureDisplays {
    pub entry: u32,
    pub displays: Vec<CreatureDisplay>,
}

#[derive(SpacetimeType, Clone, Debug)]
pub struct LootRow {
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
    pub quest_only: bool,
}

#[derive(SpacetimeType, Clone)]
pub struct CreatureLootRows {
    pub creature_entry: u32,
    pub rows: Vec<LootRow>,
}

#[derive(SpacetimeType, Clone, Debug)]
pub struct SkinRow {
    pub item_entry: u32,
    pub chance_bp: u32,
    pub count: u32,
    pub group_id: u32,
}

#[derive(SpacetimeType, Clone)]
pub struct SkinningLootRows {
    pub skin_loot_id: u32,
    pub rows: Vec<SkinRow>,
}

#[derive(SpacetimeType, Clone, Debug)]
#[sats(name = "OperatorCreatureSpell")]
pub struct CreatureSpell {
    pub spell_id: u32,
    pub priority: u8,
    pub condition: u8,
    pub condition_value: u8,
}

#[derive(SpacetimeType, Clone)]
pub struct CreatureSpells {
    pub creature_entry: u32,
    pub spells: Vec<CreatureSpell>,
}

#[derive(SpacetimeType, Clone)]
pub struct CreatureCastEdit {
    pub creature_entry: u32,
    pub spell_id: Option<u32>,
}

#[derive(SpacetimeType, Clone)]
pub enum CreatureEdit {
    CreateCreature(CreatureCreate),
    UpdateCreature(CreatureUpdate),
    SetCreatureDisplays(CreatureDisplays),
    SetCreatureLoot(CreatureLootRows),
    SetPickpocketLoot(CreatureLootRows),
    SetSkinningLoot(SkinningLootRows),
    SetCreatureSpells(CreatureSpells),
    SetCreatureCast(CreatureCastEdit),
}

/// Applies edits in order; a Refusal rolls back rows, despawns and queued packets.
#[reducer]
pub fn operator_apply_creature_edits(
    ctx: &ReducerContext,
    edits: Vec<CreatureEdit>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    check_batch(&edits)?;
    let mut ids = CreatureIds::default();
    let mut changed = BTreeMap::new();
    let mut changes = Vec::with_capacity(edits.len());
    for (index, edit) in edits.into_iter().enumerate() {
        let (change, entry) = apply(ctx, &mut ids, edit)
            .map_err(|reason| format!("creature edit {index}: {reason}"))?;
        if let Some(entry) = entry {
            changed.insert(entry, index);
        }
        changes.push(change);
    }
    respawn_changed(ctx, &changed)?;
    for (entry, index) in changed {
        crate::native::broadcast_creature_query(ctx, entry)
            .map_err(|reason| format!("creature edit {index}: {reason}"))?;
    }
    for change in changes {
        super::log_change(ctx, &change);
    }
    Ok(())
}

fn check_batch(edits: &[CreatureEdit]) -> Result<(), String> {
    if edits.len() > MAX_EDITS {
        return Err(format!(
            "creature edit {MAX_EDITS}: a batch holds at most {MAX_EDITS} edits"
        ));
    }
    let mut total = 0;
    for (index, edit) in edits.iter().enumerate() {
        let rows = match edit {
            CreatureEdit::SetCreatureDisplays(edit) => edit.displays.len(),
            CreatureEdit::SetCreatureLoot(edit) | CreatureEdit::SetPickpocketLoot(edit) => {
                edit.rows.len()
            }
            CreatureEdit::SetSkinningLoot(edit) => edit.rows.len(),
            CreatureEdit::SetCreatureSpells(edit) => edit.spells.len(),
            _ => 0,
        };
        if rows > MAX_LIST_ROWS {
            return Err(format!(
                "creature edit {index}: a list holds at most {MAX_LIST_ROWS} rows"
            ));
        }
        total += rows;
        if total > MAX_BATCH_ROWS {
            return Err(format!(
                "creature edit {index}: a batch holds at most {MAX_BATCH_ROWS} list rows"
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum RowBand {
    Display,
    CreatureLoot,
    Loot,
    Cast,
}

impl RowBand {
    fn protected(self, id: u64) -> bool {
        is_fixture_reserved_loot_id(id)
            || match self {
                Self::Display | Self::CreatureLoot => false,
                Self::Loot => is_package_loot_id(id),
                Self::Cast => is_package_cast_id(id),
            }
    }

    fn next(self, last: u64) -> Result<u64, String> {
        let mut next = last.checked_add(1).ok_or("row id exhausted")?;
        if is_fixture_reserved_loot_id(next) {
            next = u64::from(FIXTURE_RESERVED_ID_CEIL) + 1;
        }
        match self {
            Self::Loot if is_package_loot_id(next) => next = PACKAGE_LOOT_ID_CEIL + 1,
            Self::Cast if is_package_cast_id(next) => next = PACKAGE_CAST_ID_CEIL + 1,
            _ => {}
        }
        Ok(next)
    }
}

#[derive(Default)]
struct CreatureIds {
    template: Option<u32>,
    display: Option<u64>,
    loot: Option<u64>,
    pickpocket: Option<u64>,
    skinning: Option<u64>,
    spell: Option<u64>,
}

fn next_row_id(
    cursor: &mut Option<u64>,
    band: RowBand,
    existing: impl FnOnce() -> Vec<u64>,
) -> Result<u64, String> {
    let last = *cursor.get_or_insert_with(|| {
        existing()
            .into_iter()
            .filter(|id| !band.protected(*id))
            .max()
            .unwrap_or(0)
    });
    let next = band.next(last)?;
    *cursor = Some(next);
    Ok(next)
}

fn next_template_id(last: u32) -> Result<u32, String> {
    let next = last.saturating_add(1);
    if next > OPERATOR_TEMPLATE_ID_CEIL {
        return Err("the Operator Template Range is exhausted".into());
    }
    Ok(next)
}

fn editable_creature(ctx: &ReducerContext, entry: u32) -> Result<CreatureTemplate, String> {
    if entry == 0 || entry > MAX_CREATURE_GUID_COMPONENT {
        return Err(format!("creature entry {entry} must be in 1..=16777215"));
    }
    if is_package_creature_id(entry) {
        return Err(format!(
            "creature {entry} belongs to the Package Creature Range"
        ));
    }
    if is_fixture_reserved_creature_id(entry) {
        return Err(format!("creature {entry} is Fixture-Reserved"));
    }
    ctx.db
        .game_creature_template()
        .entry()
        .find(entry)
        .ok_or_else(|| format!("creature template {entry} does not exist"))
}

fn apply(
    ctx: &ReducerContext,
    ids: &mut CreatureIds,
    edit: CreatureEdit,
) -> Result<(String, Option<u32>), String> {
    match edit {
        CreatureEdit::CreateCreature(create) => {
            check_fields(ctx, &create.fields, None)?;
            let last = *ids.template.get_or_insert_with(|| {
                ctx.db
                    .game_creature_template()
                    .iter()
                    .map(|row| row.entry)
                    .filter(|entry| is_operator_template_id(*entry))
                    .max()
                    .unwrap_or(OPERATOR_TEMPLATE_ID_FLOOR - 1)
            });
            let entry = next_template_id(last)?;
            ids.template = Some(entry);
            let change = create.fields.changes(None);
            ctx.db
                .game_creature_template()
                .try_insert(create.fields.into_template(entry))
                .map_err(|reason| format!("creature {entry} cannot be inserted: {reason}"))?;
            Ok((format!("create creature {entry}: {change}"), Some(entry)))
        }
        CreatureEdit::UpdateCreature(update) => {
            let old = editable_creature(ctx, update.entry)?;
            check_fields(
                ctx,
                &update.fields,
                Some(&CreatureFields::of_template(&old)),
            )?;
            let change = update.fields.changes(Some(&old));
            ctx.db
                .game_creature_template()
                .entry()
                .update(update.fields.into_template(update.entry));
            Ok((
                format!("update creature {}: {change}", update.entry),
                Some(update.entry),
            ))
        }
        CreatureEdit::SetCreatureDisplays(edit) => replace_displays(ctx, ids, edit),
        CreatureEdit::SetCreatureLoot(edit) => {
            replace_creature_loot(ctx, ids, edit).map(|log| (log, None))
        }
        CreatureEdit::SetPickpocketLoot(edit) => {
            replace_pickpocket_loot(ctx, ids, edit).map(|log| (log, None))
        }
        CreatureEdit::SetSkinningLoot(edit) => {
            replace_skinning_loot(ctx, ids, edit).map(|log| (log, None))
        }
        CreatureEdit::SetCreatureSpells(edit) => {
            replace_spells(ctx, ids, edit).map(|log| (log, None))
        }
        CreatureEdit::SetCreatureCast(edit) => {
            editable_creature(ctx, edit.creature_entry)?;
            if is_package_cast_id(u64::from(edit.creature_entry)) {
                return Err("cast belongs to the Package Cast Range".into());
            }
            if let Some(spell) = edit.spell_id {
                check_spell(ctx, spell)?;
            }
            let table = ctx.db.game_creature_cast();
            let old = table
                .creature_entry()
                .find(edit.creature_entry)
                .map(|row| row.spell_id);
            table.creature_entry().delete(edit.creature_entry);
            if let Some(spell_id) = edit.spell_id {
                table
                    .try_insert(CreatureCast {
                        creature_entry: edit.creature_entry,
                        spell_id,
                    })
                    .map_err(|reason| format!("cast cannot be inserted: {reason}"))?;
            }
            Ok((
                format!(
                    "creature {} cast {old:?} -> {:?}",
                    edit.creature_entry, edit.spell_id
                ),
                None,
            ))
        }
    }
}

fn check_fields(
    ctx: &ReducerContext,
    fields: &CreatureFields,
    old: Option<&CreatureFields>,
) -> Result<(), String> {
    if let Some(reason) = violations(fields, old).into_iter().next() {
        return Err(reason);
    }
    if old.is_none_or(|old| old.faction_template != fields.faction_template)
        && ctx
            .db
            .game_faction_template()
            .id()
            .find(fields.faction_template)
            .is_none()
    {
        return Err(format!(
            "faction template {} does not exist",
            fields.faction_template
        ));
    }
    Ok(())
}

fn violations(fields: &CreatureFields, old: Option<&CreatureFields>) -> Vec<String> {
    let mut out = Vec::new();
    macro_rules! check {
        ($valid:expr, $message:expr, $($field:ident),+) => {
            if old.is_none_or(|old| $(old.$field != fields.$field)||+) && !($valid) {
                out.push($message.to_string());
            }
        };
    }
    check!(
        !fields.name.trim().is_empty()
            && fields.name.len() <= MAX_TEXT_BYTES
            && !fields.name.contains('\0'),
        "name must contain text, no NUL, and at most 255 UTF-8 bytes",
        name
    );
    check!(
        fields.subname.len() <= MAX_TEXT_BYTES && !fields.subname.contains('\0'),
        "subname must contain no NUL and at most 255 UTF-8 bytes",
        subname
    );
    check!(
        fields.display_id != 0,
        "display_id must be nonzero",
        display_id
    );
    if old.is_none_or(|old| old.scale.to_bits() != fields.scale.to_bits())
        && (!fields.scale.is_finite() || fields.scale <= 0.0)
    {
        out.push("scale must be finite and positive".into());
    }
    check!(
        fields.level > 0 && fields.level <= fields.max_level && fields.max_level <= 255,
        "levels must satisfy 1 <= level <= max_level <= 255",
        level,
        max_level
    );
    check!(fields.health > 0, "health must be positive", health);
    check!(
        fields.max_level_health > 0,
        "max_level_health must be positive",
        max_level_health
    );
    check!(
        fields.damage_min <= fields.damage_max,
        "damage_min must not exceed damage_max",
        damage_min,
        damage_max
    );
    check!(
        fields.money_min <= fields.money_max,
        "money_min must not exceed money_max",
        money_min,
        money_max
    );
    check!(
        (1..=10).contains(&fields.creature_type),
        "creature_type must be in 1..=10",
        creature_type
    );
    check!(
        CreatureFamily::try_from(fields.creature_family).is_ok(),
        "unsupported creature_family",
        creature_family
    );
    check!(fields.rank <= 4, "rank must be in 0..=4", rank);
    check!(
        fields.trainer_type <= 3,
        "trainer_type must be in 0..=3",
        trainer_type
    );
    check!(
        fields.trainer_class == 0 || Class::try_from(fields.trainer_class).is_ok(),
        "unsupported trainer_class",
        trainer_class
    );
    check!(
        fields.base_attack_time_ms > 0,
        "base_attack_time_ms must be positive",
        base_attack_time_ms
    );
    out
}

fn respawn_changed(ctx: &ReducerContext, changed: &BTreeMap<u32, usize>) -> Result<(), String> {
    let mut spawns = Vec::new();
    for (&entry, index) in changed {
        for entity in ctx.db.game_world_entity().by_entry().filter(entry) {
            if entity.instance_id != 0 || entity.owner_guid != 0 {
                continue;
            }
            let Some(spawn) = ctx.db.game_creature_spawn().guid().find(entity.guid) else {
                continue;
            };
            if spawn.instance_id != 0 || spawn.source_guid != 0 {
                continue;
            }
            if is_package_creature_id((spawn.guid & u64::from(MAX_CREATURE_GUID_COMPONENT)) as u32)
            {
                continue;
            }
            spawns.push(spawn);
            if spawns.len() > MAX_RESPAWNS {
                return Err(format!(
                    "creature edit {index}: entry {entry} exceeds the batch limit; a batch may respawn at most {MAX_RESPAWNS} creatures"
                ));
            }
        }
    }
    for mut spawn in spawns {
        crate::creatures::despawn_creature_entity(ctx, spawn.guid);
        super::rearm_respawn(ctx, &mut spawn);
        ctx.db.game_creature_spawn().guid().update(spawn);
    }
    Ok(())
}

fn check_displays(displays: &[CreatureDisplay]) -> Result<(), String> {
    for display in displays {
        if display.display_id == 0
            || !display.scale.is_finite()
            || display.scale <= 0.0
            || display.gender > 2
            || !display.bounding_radius.is_finite()
            || display.bounding_radius < 0.0
        {
            return Err("displays need a nonzero id, finite positive scale, gender 0..=2 and finite nonnegative radius".into());
        }
    }
    let total: u64 = displays
        .iter()
        .map(|display| u64::from(display.weight))
        .sum();
    if !displays.is_empty() && (total == 0 || total > u64::from(u32::MAX)) {
        return Err("a nonempty display list needs total weight in 1..=4294967295".into());
    }
    Ok(())
}

fn replace_displays(
    ctx: &ReducerContext,
    ids: &mut CreatureIds,
    edit: CreatureDisplays,
) -> Result<(String, Option<u32>), String> {
    let mut template = editable_creature(ctx, edit.entry)?;
    check_displays(&edit.displays)?;
    let table = ctx.db.game_creature_display();
    let old: Vec<_> = table.by_entry().filter(edit.entry).collect();
    if old.iter().any(|row| RowBand::Display.protected(row.id)) {
        return Err("the list contains Fixture-Reserved rows".into());
    }
    let mut rows = Vec::new();
    for display in &edit.displays {
        rows.push(DisplayRow {
            id: next_row_id(&mut ids.display, RowBand::Display, || {
                table.iter().map(|row| row.id).collect()
            })?,
            entry: edit.entry,
            display_id: display.display_id,
            scale: display.scale,
            weight: display.weight,
            gender: display.gender,
            bounding_radius: display.bounding_radius,
        });
    }
    let old_model = (template.display_id, template.scale);
    if let Some(first) = edit.displays.first() {
        template.display_id = first.display_id;
        template.scale = first.scale;
    }
    let new_model = (template.display_id, template.scale);
    ctx.db.game_creature_template().entry().update(template);
    for row in &old {
        table.id().delete(row.id);
    }
    for row in rows {
        table
            .try_insert(row)
            .map_err(|reason| format!("display cannot be inserted: {reason}"))?;
    }
    let old: Vec<_> = old
        .into_iter()
        .map(|row| CreatureDisplay {
            display_id: row.display_id,
            scale: row.scale,
            weight: row.weight,
            gender: row.gender,
            bounding_radius: row.bounding_radius,
        })
        .collect();
    Ok((format!("creature {} displays {old:?} -> {:?}; template display/scale {old_model:?} -> {new_model:?}", edit.entry, edit.displays), Some(edit.entry)))
}

fn check_loot(
    ctx: &ReducerContext,
    rows: impl Iterator<Item = (u32, u32, u32, u32)>,
) -> Result<(), String> {
    let mut groups = BTreeMap::<u32, u32>::new();
    for (item, chance, count, group) in rows {
        if ctx.db.game_item_template().entry().find(item).is_none() {
            return Err(format!("item {item} does not exist"));
        }
        if chance > 10_000 || count == 0 {
            return Err("loot needs chance_bp <= 10000 and a positive count".into());
        }
        if group != 0 {
            let total = groups.entry(group).or_default();
            *total += chance;
            if *total > 10_000 {
                return Err(format!("loot group {group} exceeds 10000 basis points"));
            }
        }
    }
    Ok(())
}

// Allocate before deleting: imported explicit ids do not advance auto_inc.
macro_rules! replace_rows {
    ($table:ident, $old:ident, $rows:ident, $cursor:expr, $band:expr, $build:expr) => {{
        if $old.iter().any(|row| $band.protected(row.id)) {
            return Err("the list contains Package or Fixture-Reserved rows".into());
        }
        let mut replacements = Vec::new();
        for row in &$rows {
            let id = next_row_id($cursor, $band, || $table.iter().map(|row| row.id).collect())?;
            replacements.push($build(id, row));
        }
        for row in &$old {
            $table.id().delete(row.id);
        }
        for row in replacements {
            $table
                .try_insert(row)
                .map_err(|reason| format!("list row cannot be inserted: {reason}"))?;
        }
    }};
}

fn replace_creature_loot(
    ctx: &ReducerContext,
    ids: &mut CreatureIds,
    edit: CreatureLootRows,
) -> Result<String, String> {
    let entry = edit.creature_entry;
    editable_creature(ctx, entry)?;
    let rows = edit.rows;
    check_loot(
        ctx,
        rows.iter()
            .map(|row| (row.item_entry, row.chance_bp, row.count, row.group_id)),
    )?;
    let table = ctx.db.game_creature_loot();
    let old: Vec<_> = table.by_creature().filter(entry).collect();
    replace_rows!(
        table,
        old,
        rows,
        &mut ids.loot,
        RowBand::CreatureLoot,
        |id, row: &LootRow| CreatureLoot {
            id,
            creature_entry: entry,
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            count: row.count,
            group_id: row.group_id,
            quest_only: row.quest_only,
        }
    );
    let old: Vec<_> = old
        .into_iter()
        .map(|row| LootRow {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            count: row.count,
            group_id: row.group_id,
            quest_only: row.quest_only,
        })
        .collect();
    Ok(format!("creature {entry} loot {old:?} -> {rows:?}"))
}

fn replace_pickpocket_loot(
    ctx: &ReducerContext,
    ids: &mut CreatureIds,
    edit: CreatureLootRows,
) -> Result<String, String> {
    let entry = edit.creature_entry;
    editable_creature(ctx, entry)?;
    let rows = edit.rows;
    check_loot(
        ctx,
        rows.iter()
            .map(|row| (row.item_entry, row.chance_bp, row.count, row.group_id)),
    )?;
    let table = ctx.db.game_pickpocket_loot();
    let old: Vec<_> = table.by_creature().filter(entry).collect();
    replace_rows!(
        table,
        old,
        rows,
        &mut ids.pickpocket,
        RowBand::Loot,
        |id, row: &LootRow| GamePickpocketLoot {
            id,
            creature_entry: entry,
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            count: row.count,
            group_id: row.group_id,
            quest_only: row.quest_only,
        }
    );
    let old: Vec<_> = old
        .into_iter()
        .map(|row| LootRow {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            count: row.count,
            group_id: row.group_id,
            quest_only: row.quest_only,
        })
        .collect();
    Ok(format!(
        "creature {entry} pickpocket loot {old:?} -> {rows:?}"
    ))
}

fn replace_skinning_loot(
    ctx: &ReducerContext,
    ids: &mut CreatureIds,
    edit: SkinningLootRows,
) -> Result<String, String> {
    let skin = edit.skin_loot_id;
    if skin == 0 {
        return Err("skin_loot_id must be nonzero".into());
    }
    let rows = edit.rows;
    check_loot(
        ctx,
        rows.iter()
            .map(|row| (row.item_entry, row.chance_bp, row.count, row.group_id)),
    )?;
    let table = ctx.db.game_skinning_loot();
    let old: Vec<_> = table.by_skin().filter(skin).collect();
    replace_rows!(
        table,
        old,
        rows,
        &mut ids.skinning,
        RowBand::Loot,
        |id, row: &SkinRow| GameSkinningLoot {
            id,
            skin_loot_id: skin,
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            count: row.count,
            group_id: row.group_id,
        }
    );
    let old: Vec<_> = old
        .into_iter()
        .map(|row| SkinRow {
            item_entry: row.item_entry,
            chance_bp: row.chance_bp,
            count: row.count,
            group_id: row.group_id,
        })
        .collect();
    Ok(format!("skin loot {skin} {old:?} -> {rows:?}"))
}

fn check_spell(ctx: &ReducerContext, spell: u32) -> Result<(), String> {
    if ctx.db.game_spell().spell_id().find(spell).is_none() {
        return Err(format!("spell {spell} does not exist"));
    }
    Ok(())
}

fn replace_spells(
    ctx: &ReducerContext,
    ids: &mut CreatureIds,
    edit: CreatureSpells,
) -> Result<String, String> {
    let entry = edit.creature_entry;
    editable_creature(ctx, entry)?;
    let rows = edit.spells;
    for row in &rows {
        check_spell(ctx, row.spell_id)?;
        if row.condition > 3 || (row.condition == 1 && row.condition_value > 100) {
            return Err("spell conditions must be 0..=3; health percent must be 0..=100".into());
        }
    }
    let table = ctx.db.game_creature_spell();
    let old: Vec<_> = table.by_entry().filter(entry).collect();
    replace_rows!(
        table,
        old,
        rows,
        &mut ids.spell,
        RowBand::Cast,
        |id, row: &CreatureSpell| SpellRow {
            id,
            creature_entry: entry,
            spell_id: row.spell_id,
            priority: row.priority,
            condition: row.condition,
            condition_value: row.condition_value,
        }
    );
    let old: Vec<_> = old
        .into_iter()
        .map(|row| CreatureSpell {
            spell_id: row.spell_id,
            priority: row.priority,
            condition: row.condition,
            condition_value: row.condition_value,
        })
        .collect();
    Ok(format!("creature {entry} spells {old:?} -> {rows:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> CreatureFields {
        CreatureFields {
            name: "Creature".into(),
            display_id: 100,
            level: 1,
            max_level: 1,
            health: 10,
            max_level_health: 10,
            scale: 1.0,
            creature_type: 1,
            base_attack_time_ms: 2000,
            ..Default::default()
        }
    }

    #[test]
    fn allocation_starts_in_range_and_refuses_exhaustion() {
        assert_eq!(
            next_template_id(OPERATOR_TEMPLATE_ID_FLOOR - 1),
            Ok(5_200_500)
        );
        assert_eq!(next_template_id(5_200_541), Ok(5_200_542));
        assert!(next_template_id(OPERATOR_TEMPLATE_ID_CEIL).is_err());
        assert!(RowBand::Loot.next(u64::MAX).is_err());
    }

    #[test]
    fn row_allocation_skips_owned_bands_and_ignores_their_high_water() {
        assert_eq!(RowBand::Loot.next(5_089_999), Ok(5_100_000));
        assert_eq!(RowBand::Loot.next(8_999_999), Ok(10_000_000));
        assert_eq!(RowBand::Cast.next(9_999_999), Ok(11_000_000));
        assert_eq!(RowBand::CreatureLoot.next(8_999_999), Ok(9_000_000));
        let mut cursor = None;
        assert_eq!(
            next_row_id(&mut cursor, RowBand::Loot, || vec![
                42, 5_090_000, 9_999_999
            ]),
            Ok(43)
        );
        assert_eq!(next_row_id(&mut cursor, RowBand::Loot, Vec::new), Ok(44));
    }

    #[test]
    fn template_gates_check_new_values_and_preserve_unchanged_imported_values() {
        assert!(violations(&fields(), None).is_empty());
        let invalid = [
            CreatureFields {
                name: "bad\0name".into(),
                ..fields()
            },
            CreatureFields {
                subname: "x".repeat(256),
                ..fields()
            },
            CreatureFields {
                scale: f32::NAN,
                ..fields()
            },
            CreatureFields {
                display_id: 0,
                ..fields()
            },
            CreatureFields {
                max_level: 0,
                ..fields()
            },
            CreatureFields {
                level: 256,
                max_level: 256,
                ..fields()
            },
            CreatureFields {
                health: 0,
                ..fields()
            },
            CreatureFields {
                max_level_health: 0,
                ..fields()
            },
            CreatureFields {
                damage_min: 2,
                damage_max: 1,
                ..fields()
            },
            CreatureFields {
                money_min: 2,
                money_max: 1,
                ..fields()
            },
            CreatureFields {
                creature_type: 11,
                ..fields()
            },
            CreatureFields {
                creature_family: 255,
                ..fields()
            },
            CreatureFields {
                rank: 5,
                ..fields()
            },
            CreatureFields {
                trainer_type: 4,
                ..fields()
            },
            CreatureFields {
                trainer_class: 10,
                ..fields()
            },
            CreatureFields {
                base_attack_time_ms: 0,
                ..fields()
            },
        ];
        for fields in invalid {
            assert!(!violations(&fields, None).is_empty());
            let renamed = CreatureFields {
                name: "Renamed".into(),
                ..fields.clone()
            };
            assert!(violations(&renamed, Some(&fields)).is_empty());
        }
        let imported = CreatureFields {
            creature_type: 11,
            ..fields()
        };
        let changed_invalid = CreatureFields {
            creature_type: 12,
            ..imported.clone()
        };
        assert!(!violations(&changed_invalid, Some(&imported)).is_empty());
    }

    #[test]
    fn displays_need_reachable_weights_and_finite_model_dimensions() {
        let display = CreatureDisplay {
            display_id: 100,
            scale: 1.0,
            weight: 1,
            gender: 0,
            bounding_radius: 0.389,
        };
        assert!(check_displays(&[]).is_ok());
        assert!(check_displays(std::slice::from_ref(&display)).is_ok());
        for invalid in [
            CreatureDisplay {
                display_id: 0,
                ..display.clone()
            },
            CreatureDisplay {
                scale: f32::INFINITY,
                ..display.clone()
            },
            CreatureDisplay {
                bounding_radius: -0.1,
                ..display.clone()
            },
            CreatureDisplay {
                gender: 3,
                ..display.clone()
            },
            CreatureDisplay {
                weight: 0,
                ..display.clone()
            },
        ] {
            assert!(check_displays(&[invalid]).is_err());
        }
        assert!(check_displays(&[
            CreatureDisplay {
                weight: u32::MAX,
                ..display.clone()
            },
            display,
        ])
        .is_err());
    }

    #[test]
    fn log_names_only_changed_columns_and_escapes_line_breaks() {
        let old = fields().into_template(42);
        let renamed = CreatureFields {
            name: "New\nname".into(),
            ..fields()
        };
        assert_eq!(
            renamed.changes(Some(&old)),
            "name Some(\"Creature\") -> \"New\\nname\""
        );
    }
}
