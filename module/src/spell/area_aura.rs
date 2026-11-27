//! vmangos `AreaAura::Update` for party and pet area Auras: see docs/schema.md#area-auras.

use lyracore_shared::spell::taxonomy::{CAST_FLAG_AREA_AURA, CAST_FLAG_PET_AREA_AURA};
use spacetimedb::{table, ReducerContext, Table};

use crate::combat::Relation;
use crate::spell::cast::{aura_points, place_aura, AuraPlacement};
use crate::spell::tables::SPELL_ATTR_EX3_ONLY_ON_PLAYER;
use crate::spell::SPELL_ATTR_PASSIVE;
use crate::{
    game_aura, game_spell, game_spell_chain, game_spell_effect, game_world_entity, Aura, Spell,
    WorldEntity,
};

#[table(accessor = game_area_aura_source)]
pub struct AreaAuraSource {
    #[primary_key]
    pub aura_id: u64,
}

#[table(
    accessor = game_area_aura_copy,
    index(accessor = by_source, btree(columns = [source_aura_id]))
)]
pub struct AreaAuraCopy {
    #[primary_key]
    pub aura_id: u64,
    pub source_aura_id: u64,
}

crate::outside_table_catalog!(game_area_aura_source, game_area_aura_copy);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    Party,
    Pet,
}

fn reach_of(spell: &Spell) -> Option<Reach> {
    if spell.cast_flags & CAST_FLAG_PET_AREA_AURA != 0 {
        Some(Reach::Pet)
    } else if spell.cast_flags & CAST_FLAG_AREA_AURA != 0 {
        Some(Reach::Party)
    } else {
        None
    }
}

/// vmangos gives a totem's own party Aura `SPELL_AURA_NONE`: it holds the Aura with no amount.
pub(crate) fn own_points(
    ctx: &ReducerContext,
    spell: &Spell,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
) -> i32 {
    if caster_guid == target_guid && holds_inert(ctx, spell, caster_guid) {
        0
    } else {
        points
    }
}

fn holds_inert(ctx: &ReducerContext, spell: &Spell, caster_guid: u64) -> bool {
    reach_of(spell) == Some(Reach::Party)
        && ctx
            .db
            .game_world_entity()
            .guid()
            .find(caster_guid)
            .is_some_and(|caster| crate::combat::is_totem(ctx, &caster))
}

/// Records a caster's own area Aura as a source; Aura inserts, refreshes and world entry call it.
pub(crate) fn record_source(ctx: &ReducerContext, aura: &Aura) {
    let source = aura.caster_guid == aura.target_guid
        && ctx
            .db
            .game_spell()
            .spell_id()
            .find(aura.spell_id)
            .as_ref()
            .and_then(reach_of)
            .is_some();
    let sources = ctx.db.game_area_aura_source();
    if source && sources.aura_id().find(aura.id).is_none() {
        sources.insert(AreaAuraSource { aura_id: aura.id });
    }
}

/// Forgets a deleted copy, and removes every copy of a deleted source.
pub(crate) fn deleted(ctx: &ReducerContext, aura: &Aura) {
    ctx.db.game_area_aura_copy().aura_id().delete(aura.id);
    if ctx.db.game_area_aura_source().aura_id().delete(aura.id) {
        crate::spell::shed_auras(ctx, copies_of(ctx, aura.id));
    }
}

/// Whether an Aura is a copy that its source spread; a Character Transfer leaves copies behind.
pub(crate) fn is_copy(ctx: &ReducerContext, aura_id: u64) -> bool {
    ctx.db
        .game_area_aura_copy()
        .aura_id()
        .find(aura_id)
        .is_some()
}

/// Records the area Auras a unit holds of its own; every unit entering the world calls it.
pub(crate) fn entered_world(ctx: &ReducerContext, unit_guid: u64) {
    let held: Vec<Aura> = ctx
        .db
        .game_aura()
        .by_target()
        .filter(unit_guid)
        .filter(|aura| aura.caster_guid == unit_guid)
        .collect();
    for aura in &held {
        record_source(ctx, aura);
    }
}

/// Publish repair: records the area Auras placed before sources were recorded. Returns the count.
pub(crate) fn record_existing_sources(ctx: &ReducerContext) -> u64 {
    let before = ctx.db.game_area_aura_source().count();
    let held: Vec<Aura> = ctx
        .db
        .game_aura()
        .iter()
        .filter(|aura| aura.caster_guid == aura.target_guid)
        .collect();
    for aura in &held {
        record_source(ctx, aura);
    }
    ctx.db.game_area_aura_source().count() - before
}

/// One vmangos `AreaAura::Update` pass over every source, then over copies whose source is gone.
pub(crate) fn spread(ctx: &ReducerContext) {
    let sources: Vec<u64> = ctx
        .db
        .game_area_aura_source()
        .iter()
        .map(|source| source.aura_id)
        .collect();
    for aura_id in sources {
        match ctx.db.game_aura().id().find(aura_id) {
            Some(source) => keep_in_step(ctx, &source),
            None => {
                ctx.db.game_area_aura_source().aura_id().delete(aura_id);
            }
        }
    }
    let orphans: Vec<u64> = ctx
        .db
        .game_area_aura_copy()
        .iter()
        .filter(|copy| {
            ctx.db
                .game_area_aura_source()
                .aura_id()
                .find(copy.source_aura_id)
                .is_none()
        })
        .map(|copy| copy.aura_id)
        .collect();
    for aura_id in orphans {
        match ctx.db.game_aura().id().find(aura_id) {
            Some(copy) => crate::spell::shed_auras(ctx, vec![copy]),
            None => {
                ctx.db.game_area_aura_copy().aura_id().delete(aura_id);
            }
        }
    }
}

struct Area {
    caster: WorldEntity,
    reach: Reach,
    radius_yd: f32,
}

impl Area {
    fn within(&self, unit: &WorldEntity) -> bool {
        crate::helpers::in_same_partition(unit, self.caster.map_id, self.caster.instance_id)
            && crate::helpers::dist_sq(&self.caster, unit) <= self.radius_yd * self.radius_yd
    }

    fn friendly_to(&self, ctx: &ReducerContext, unit: &WorldEntity) -> bool {
        matches!(
            crate::combat::relation(ctx, &self.caster, unit),
            Relation::Friendly | Relation::SelfUnit
        )
    }
}

// A source whose caster left the world stops being one until the caster enters it again.
fn keep_in_step(ctx: &ReducerContext, source: &Aura) {
    let caster = ctx.db.game_world_entity().guid().find(source.caster_guid);
    let spell = ctx.db.game_spell().spell_id().find(source.spell_id);
    let reach = spell.as_ref().and_then(reach_of);
    let effect = ctx.db.game_spell_effect().id().find(source.effect_id);
    let (Some(caster), Some(spell), Some(reach), Some(effect)) = (caster, spell, reach, effect)
    else {
        ctx.db.game_area_aura_source().aura_id().delete(source.id);
        crate::spell::shed_auras(ctx, copies_of(ctx, source.id));
        return;
    };
    let area = Area {
        caster,
        reach,
        radius_yd: effect.radius_yd,
    };
    let (kept, leaving): (Vec<Aura>, Vec<Aura>) = copies_of(ctx, source.id)
        .into_iter()
        .partition(|copy| keeps_copy(ctx, &area, copy.target_guid));
    crate::spell::shed_auras(ctx, leaving);
    let holders: std::collections::BTreeSet<u64> =
        kept.iter().map(|copy| copy.target_guid).collect();
    for mut copy in kept {
        if copy.expires_at != source.expires_at {
            copy.expires_at = source.expires_at;
            copy.update(ctx);
        }
    }
    for unit in gains(ctx, &area) {
        if !holders.contains(&unit.guid) {
            give_copy(ctx, source, &spell, reach, &unit);
        }
    }
}

/// The copies on other units that a source Aura spread; each keeps the caster's guid.
fn copies_of(ctx: &ReducerContext, source_aura_id: u64) -> Vec<Aura> {
    let auras = ctx.db.game_aura();
    ctx.db
        .game_area_aura_copy()
        .by_source()
        .filter(source_aura_id)
        .filter_map(|copy| auras.id().find(copy.aura_id))
        .collect()
}

// `packed_spell_effect_id` keeps the effect index in the two low bits.
fn effect_index(effect_id: u64) -> u8 {
    (effect_id & 0b11) as u8
}

/// A unit's own Character, or its owner's: vmangos `GetCharmerOrOwnerPlayerOrPlayerItself`.
fn character_of(ctx: &ReducerContext, unit: &WorldEntity) -> Option<u64> {
    crate::pvp::affecting_character(ctx, unit).map(|character| character.guid)
}

/// The Group and Subgroup a Character is in.
fn subgroup_of(ctx: &ReducerContext, character_guid: u64) -> Option<(u64, u8)> {
    crate::group::group_of(ctx, character_guid).map(|member| {
        (
            member.group_id,
            crate::group::raid_slot_of(&member).subgroup(),
        )
    })
}

fn gains(ctx: &ReducerContext, area: &Area) -> Vec<WorldEntity> {
    let caster = &area.caster;
    // vmangos `GetCharmerOrOwner`, else the caster itself.
    let owner = Some(caster.owner_guid)
        .filter(|&guid| guid != 0)
        .and_then(|guid| ctx.db.game_world_entity().guid().find(guid));
    let (owner_guid, owner_is_player) = owner
        .as_ref()
        .map_or((caster.guid, caster.is_player()), |owner| {
            (owner.guid, owner.is_player())
        });
    let mut units: Vec<WorldEntity> = match area.reach {
        Reach::Pet => owner.into_iter().collect(),
        Reach::Party => match party_members(ctx, owner_guid).filter(|_| owner_is_player) {
            Some(members) => members
                .into_iter()
                .filter(|member| member_gains(ctx, area, owner_guid, member))
                .flat_map(|member| {
                    let pet = crate::creatures::pet_of(ctx, member.guid);
                    std::iter::once(member).chain(pet)
                })
                .collect(),
            None => owner
                .into_iter()
                .chain(crate::creatures::pet_of(ctx, caster.guid))
                .collect(),
        },
    };
    units.retain(|unit| !unit.dead && area.within(unit));
    units
}

/// The grouped Character's Subgroup on this Shard.
fn party_members(ctx: &ReducerContext, character_guid: u64) -> Option<Vec<WorldEntity>> {
    let (group_id, subgroup) = subgroup_of(ctx, character_guid)?;
    let entities = ctx.db.game_world_entity();
    Some(
        crate::group::members_of(ctx, group_id)
            .into_iter()
            .filter(|member| crate::group::raid_slot_of(member).subgroup() == subgroup)
            .filter_map(|member| entities.guid().find(member.character_guid))
            .collect(),
    )
}

/// vmangos skips a dead, duelling or unfriendly member, and a flagged one under an unflagged caster.
fn member_gains(ctx: &ReducerContext, area: &Area, owner_guid: u64, member: &WorldEntity) -> bool {
    let caster_flagged =
        crate::pvp::affecting_character(ctx, &area.caster).is_some_and(|by| by.flagged);
    !member.dead
        && (member.guid == owner_guid || crate::duel::duel_involving(ctx, member.guid).is_none())
        && area.friendly_to(ctx, member)
        && (caster_flagged || !crate::pvp::is_flagged(member))
}

/// vmangos checks a copy only for range, friendliness and Subgroup, not for Duels or PvP flags.
fn keeps_copy(ctx: &ReducerContext, area: &Area, target_guid: u64) -> bool {
    let Some(target) = ctx.db.game_world_entity().guid().find(target_guid) else {
        return false;
    };
    if !area.within(&target) || !area.friendly_to(ctx, &target) {
        return false;
    }
    let caster = &area.caster;
    let owner_or_pet = (caster.owner_guid != 0 && target.guid == caster.owner_guid)
        || target.owner_guid == caster.guid;
    match area.reach {
        Reach::Pet => caster.owner_guid != 0 && target.guid == caster.owner_guid,
        Reach::Party if owner_or_pet => true,
        Reach::Party => {
            let caster_subgroup = character_of(ctx, caster).and_then(|guid| subgroup_of(ctx, guid));
            caster_subgroup.is_some()
                && character_of(ctx, &target).and_then(|guid| subgroup_of(ctx, guid))
                    == caster_subgroup
        }
    }
}

fn give_copy(ctx: &ReducerContext, source: &Aura, spell: &Spell, reach: Reach, unit: &WorldEntity) {
    let rank = match reach {
        Reach::Party => rank_for_level(ctx, spell, unit.level),
        Reach::Pet => Some(spell.clone()),
    };
    let Some(rank) = rank else {
        return;
    };
    if rank.attributes_ex_3 & SPELL_ATTR_EX3_ONLY_ON_PLAYER != 0 && !unit.is_player() {
        return;
    }
    let index = effect_index(source.effect_id);
    let held = family_on(ctx, unit.guid, &rank, index);
    let incoming = strength(ctx, &rank);
    if held
        .iter()
        .any(|(aura, strength)| aura.caster_guid == unit.guid || *strength >= incoming)
    {
        return;
    }
    let id = lyracore_package_delta::packed_spell_effect_id(rank.spell_id, index);
    let Some(effect) = ctx.db.game_spell_effect().id().find(id) else {
        return;
    };
    let weaker = held
        .into_iter()
        .map(|(aura, _)| aura)
        .filter(|aura| is_copy(ctx, aura.id))
        .collect();
    crate::spell::shed_auras(ctx, weaker);
    // vmangos recalculates a lower rank's base points; a totem's own Aura holds none to pass on.
    let points = if rank.spell_id == spell.spell_id && !holds_inert(ctx, spell, source.caster_guid)
    {
        source.amount
    } else {
        aura_points(ctx, &effect, &rank, source.caster_guid, effect.base_points)
    };
    // A channelled copy leaves its channel checks to the caster's own Aura; the pass checks range.
    let cast_target_guid = if rank.channelled() {
        source.caster_guid
    } else {
        unit.guid
    };
    let placed = place_aura(
        ctx,
        &effect,
        &rank,
        &AuraPlacement {
            caster_guid: source.caster_guid,
            target_guid: unit.guid,
            cast_target_guid,
            level: source.level,
            points,
            source_item: 0,
            expires: source.expires_at,
            relays_overflow: false,
        },
    );
    let copy = ctx
        .db
        .game_aura()
        .by_target()
        .filter(unit.guid)
        .find(|aura| aura.caster_guid == source.caster_guid && aura.effect_id == effect.id);
    if let Some(copy) = copy.filter(|_| placed) {
        let copies = ctx.db.game_area_aura_copy();
        if copies.aura_id().find(copy.id).is_none() {
            copies.insert(AreaAuraCopy {
                aura_id: copy.id,
                source_aura_id: source.id,
            });
        }
    }
}

/// The unit's Auras of the rank's family (docs/schema.md#area-auras) at one effect index.
fn family_on(
    ctx: &ReducerContext,
    unit_guid: u64,
    rank: &Spell,
    index: u8,
) -> Vec<(Aura, (u8, u8))> {
    let chain = crate::spell::first_rank(ctx, rank.spell_id);
    let spells = ctx.db.game_spell();
    ctx.db
        .game_aura()
        .by_target()
        .filter(unit_guid)
        .filter(|aura| effect_index(aura.effect_id) == index)
        .filter_map(|aura| {
            let spell = spells.spell_id().find(aura.spell_id)?;
            let related = crate::spell::first_rank(ctx, aura.spell_id) == chain
                || (!rank.name.is_empty() && spell.name == rank.name && reach_of(&spell).is_some());
            related.then(|| (aura, strength(ctx, &spell)))
        })
        .collect()
}

/// Orders a family: chain rank, then spell level.
fn strength(ctx: &ReducerContext, spell: &Spell) -> (u8, u8) {
    (
        crate::spell::stacking::rank_of(ctx, spell.spell_id),
        spell.spell_level,
    )
}

/// vmangos `SelectAuraRankForLevel`: the highest rank at most ten levels above `level`, or none.
fn rank_for_level(ctx: &ReducerContext, spell: &Spell, level: u32) -> Option<Spell> {
    let fits = |rank: &Spell| level + 10 >= u32::from(rank.spell_level);
    let chains = ctx.db.game_spell_chain();
    if fits(spell)
        || spell.attributes & SPELL_ATTR_PASSIVE != 0
        || spell.negative()
        || chains.spell_id().find(spell.spell_id).is_none()
    {
        return Some(spell.clone());
    }
    let mut rank_id = spell.spell_id;
    while rank_id != 0 {
        let rank = ctx.db.game_spell().spell_id().find(rank_id)?;
        if fits(&rank) {
            return Some(rank);
        }
        rank_id = chains
            .spell_id()
            .find(rank_id)
            .map_or(0, |chain| chain.prev_spell);
    }
    None
}
