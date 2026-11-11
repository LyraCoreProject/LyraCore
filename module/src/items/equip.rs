use spacetimedb::ReducerContext;

use super::{ItemInstance, ItemTemplate};
use crate::{
    game_aura, game_item_instance, game_item_template, game_spell, game_spell_cd, game_world_entity,
};

const NO_EQUIP_COOLDOWN: u32 = 0x80;
const SPELLMOD_COOLDOWN: i32 = 11;

pub(crate) fn spell_ready(ctx: &ReducerContext, caster: u64, spell: u32, category: u32) -> bool {
    let now = crate::time::now(ctx);
    let micros = now.to_micros_since_unix_epoch();
    !ctx.db
        .game_spell_cd()
        .by_caster()
        .filter(caster)
        .any(|cooldown| {
            (cooldown.spell_id == spell
                && (now < cooldown.ready_at || cooldown.category_ready_micros > micros))
                || (category != 0
                    && cooldown.category == category
                    && cooldown.category_ready_micros > micros)
        })
}

pub(super) fn use_category(item: &ItemTemplate, spell: u32) -> u32 {
    spells(item)
        .into_iter()
        .zip([
            item.spellcategory_1,
            item.spellcategory_2,
            item.spellcategory_3,
            item.spellcategory_4,
            item.spellcategory_5,
        ])
        .find(|((id, trigger), _)| *id == spell && *trigger == 0)
        .map_or(0, |(_, category)| category)
}

pub(super) fn spells(item: &ItemTemplate) -> [(u32, u8); 5] {
    [
        (item.spellid_1, item.spelltrigger_1),
        (item.spellid_2, item.spelltrigger_2),
        (item.spellid_3, item.spelltrigger_3),
        (item.spellid_4, item.spelltrigger_4),
        (item.spellid_5, item.spelltrigger_5),
    ]
}

fn equipped(item: &ItemInstance) -> bool {
    item.owner_guid != 0 && item.slot < 23
}

pub(super) fn changed(
    ctx: &ReducerContext,
    previous: Option<&ItemInstance>,
    current: Option<&ItemInstance>,
) {
    if previous.zip(current).is_some_and(|(old, new)| {
        old.owner_guid == new.owner_guid
            && old.slot == new.slot
            && old.entry == new.entry
            && (old.durability == 0) == (new.durability == 0)
    }) {
        return;
    }
    if let Some(previous) = previous.filter(|item| equipped(item)) {
        if current.is_some_and(|item| {
            equipped(item) && item.owner_guid == previous.owner_guid && item.slot != previous.slot
        }) {
            let removed = ctx
                .db
                .game_aura()
                .by_target()
                .filter(previous.owner_guid)
                .filter(|aura| aura.item_guid(ctx) == previous.guid)
                .collect();
            crate::spell::shed_auras(ctx, removed);
        }
        reconcile_item(ctx, previous.owner_guid, Some(previous.guid), false);
    }
    if let Some(current) = current.filter(|item| equipped(item)) {
        if previous.is_none_or(|old| !equipped(old) || old.owner_guid != current.owner_guid) {
            reconcile_item(ctx, current.owner_guid, Some(current.guid), false);
        }
        if previous
            .is_none_or(|old| old.owner_guid != current.owner_guid || old.slot != current.slot)
        {
            cooldown(ctx, current);
        }
    }
}

pub(crate) fn reconcile(ctx: &ReducerContext, guid: u64) {
    reconcile_item(ctx, guid, None, false);
}

pub(crate) fn repair(ctx: &ReducerContext, guid: u64) {
    let stale = ctx
        .db
        .game_aura()
        .by_target()
        .filter(guid)
        .filter(|aura| aura.item_guid(ctx) != 0 && !aura.item_definition_matches(ctx))
        .collect();
    crate::spell::shed_auras(ctx, stale);
    restore(ctx, guid);
}

pub(crate) fn restore(ctx: &ReducerContext, guid: u64) {
    reconcile_item(ctx, guid, None, true);
}

fn reconcile_item(ctx: &ReducerContext, guid: u64, changed_item: Option<u64>, restoring: bool) {
    let Some(entity) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let mut desired = std::collections::BTreeMap::new();
    let items: Vec<_> = if let Some(item) = changed_item {
        ctx.db
            .game_item_instance()
            .guid()
            .find(item)
            .into_iter()
            .filter(|item| item.owner_guid == guid)
            .collect()
    } else {
        ctx.db
            .game_item_instance()
            .by_owner_guid()
            .filter(guid)
            .collect()
    };
    for item in items.into_iter().filter(equipped) {
        let Some(template) = ctx.db.game_item_template().entry().find(item.entry) else {
            continue;
        };
        if super::item_is_broken(&template, &item) {
            continue;
        }
        for (spell, trigger) in spells(&template) {
            if spell != 0 && trigger == 1 {
                if let Some(header) = ctx.db.game_spell().spell_id().find(spell) {
                    if (!entity.dead || header.equip_passive())
                        && header.allows_equip_form(entity.form())
                    {
                        desired.insert((item.guid, spell), header);
                    }
                }
            }
        }
    }
    let active: Vec<_> = ctx.db.game_aura().by_target().filter(guid).collect();
    let removed = active
        .into_iter()
        .filter(|aura| {
            let item = aura.item_guid(ctx);
            item != 0
                && changed_item.is_none_or(|changed| item == changed)
                && !desired.contains_key(&(item, aura.spell_id))
        })
        .collect();
    crate::spell::shed_auras(ctx, removed);
    for ((item, _), header) in desired {
        if !entity.dead || (restoring && header.equip_passive()) {
            crate::spell::apply_item_auras(ctx, &header, guid, entity.level as u8, item);
        }
    }
}

pub(crate) fn clear(ctx: &ReducerContext, guid: u64) {
    let auras: Vec<_> = ctx
        .db
        .game_aura()
        .by_target()
        .filter(guid)
        .filter(|aura| aura.item_guid(ctx) != 0)
        .collect();
    crate::spell::shed_auras(ctx, auras);
}

fn cooldown(ctx: &ReducerContext, item: &ItemInstance) {
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(item.owner_guid)
        .is_none()
    {
        return;
    }
    let Some(template) = ctx.db.game_item_template().entry().find(item.entry) else {
        return;
    };
    if template.item_flags & NO_EQUIP_COOLDOWN != 0 {
        return;
    }
    for (spell_id, trigger) in spells(&template) {
        if spell_id == 0 || trigger != 0 {
            continue;
        }
        let Some(spell) = ctx.db.game_spell().spell_id().find(spell_id) else {
            continue;
        };
        if spell_ready(ctx, item.owner_guid, spell_id, 0) {
            let (flat, percent) =
                crate::spell::spell_mod(ctx, item.owner_guid, SPELLMOD_COOLDOWN, &spell);
            let duration = crate::spell::apply_spell_mod(30_000, flat, percent);
            if duration != 0 || spell.category_cooldown_ms != 0 {
                crate::spell::record_spell_cooldown(ctx, item.owner_guid, spell_id, duration);
            }
            if spell.category != 0 && spell.category_cooldown_ms != 0 {
                let ready = crate::time::now(ctx)
                    .to_micros_since_unix_epoch()
                    .saturating_add(i64::from(spell.category_cooldown_ms) * 1000);
                let cooldowns = ctx.db.game_spell_cd();
                let row = cooldowns
                    .by_caster()
                    .filter(item.owner_guid)
                    .find(|row| {
                        row.category == spell.category
                            && row.category_ready_micros
                                > crate::time::now(ctx).to_micros_since_unix_epoch()
                    })
                    .or_else(|| {
                        cooldowns
                            .by_caster()
                            .filter(item.owner_guid)
                            .find(|row| row.spell_id == spell_id)
                    });
                if let Some(mut row) = row {
                    row.category = spell.category;
                    row.category_ready_micros = ready;
                    cooldowns.id().update(row);
                }
            }
        }
        crate::native::item_equip_cooldown(ctx, item.owner_guid, item.guid, spell_id);
    }
}
