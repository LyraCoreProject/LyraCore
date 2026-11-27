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

crate::outside_table_catalog!(game_area_aura_source);

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

/// Records a caster's own area Aura as a source; every new Aura row passes through here.
pub(crate) fn inserted(ctx: &ReducerContext, aura: &Aura) {
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

/// Removes every copy of a source Aura whose row was deleted.
pub(crate) fn deleted(ctx: &ReducerContext, aura: &Aura) {
    if ctx.db.game_area_aura_source().aura_id().delete(aura.id) {
        crate::spell::shed_auras(ctx, copies_of(ctx, aura));
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
        inserted(ctx, aura);
    }
    ctx.db.game_area_aura_source().count() - before
}

/// One vmangos `AreaAura::Update` pass over every source.
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

fn keep_in_step(ctx: &ReducerContext, source: &Aura) {
    let spell = ctx.db.game_spell().spell_id().find(source.spell_id);
    let reach = spell.as_ref().and_then(reach_of);
    let effect = ctx.db.game_spell_effect().id().find(source.effect_id);
    let caster = ctx.db.game_world_entity().guid().find(source.caster_guid);
    let (Some(spell), Some(reach), Some(effect), Some(caster)) = (spell, reach, effect, caster)
    else {
        crate::spell::shed_auras(ctx, copies_of(ctx, source));
        return;
    };
    let area = Area {
        caster,
        reach,
        radius_yd: effect.radius_yd,
    };
    let leaving = copies_of(ctx, source)
        .into_iter()
        .filter(|copy| !keeps_copy(ctx, &area, copy.target_guid))
        .collect();
    crate::spell::shed_auras(ctx, leaving);
    for unit in gains(ctx, &area) {
        give_copy(ctx, source, &spell, &unit);
    }
}

/// The copies on other units that a source Aura spread; each keeps the caster's guid.
fn copies_of(ctx: &ReducerContext, source: &Aura) -> Vec<Aura> {
    let chain = crate::spell::first_rank(ctx, source.spell_id);
    let index = effect_index(source.effect_id);
    ctx.db
        .game_aura()
        .by_caster()
        .filter(source.caster_guid)
        .filter(|aura| {
            aura.target_guid != source.caster_guid
                && effect_index(aura.effect_id) == index
                && crate::spell::first_rank(ctx, aura.spell_id) == chain
        })
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

/// vmangos skips a duelling member, an unfriendly one, and a flagged one when the caster is not.
fn member_gains(ctx: &ReducerContext, area: &Area, owner_guid: u64, member: &WorldEntity) -> bool {
    let caster_flagged =
        crate::pvp::affecting_character(ctx, &area.caster).is_some_and(|by| by.flagged);
    (member.guid == owner_guid || crate::duel::duel_involving(ctx, member.guid).is_none())
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

fn give_copy(ctx: &ReducerContext, source: &Aura, spell: &Spell, unit: &WorldEntity) {
    let Some(rank) = rank_for_level(ctx, spell, unit.level) else {
        return;
    };
    if rank.attributes_ex_3 & SPELL_ATTR_EX3_ONLY_ON_PLAYER != 0 && !unit.is_player() {
        return;
    }
    let index = effect_index(source.effect_id);
    if holds_as_strong(ctx, unit.guid, &rank, index) {
        return;
    }
    let id = lyracore_package_delta::packed_spell_effect_id(rank.spell_id, index);
    let Some(effect) = ctx.db.game_spell_effect().id().find(id) else {
        return;
    };
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
    place_aura(
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
        },
    );
}

/// Whether the unit holds its own Aura of this chain's effect, or a copy of at least this rank.
fn holds_as_strong(ctx: &ReducerContext, unit_guid: u64, rank: &Spell, index: u8) -> bool {
    let chain = crate::spell::first_rank(ctx, rank.spell_id);
    let incoming = crate::spell::stacking::rank_of(ctx, rank.spell_id);
    ctx.db
        .game_aura()
        .by_target()
        .filter(unit_guid)
        .filter(|aura| {
            effect_index(aura.effect_id) == index
                && crate::spell::first_rank(ctx, aura.spell_id) == chain
        })
        .any(|aura| {
            aura.caster_guid == unit_guid
                || crate::spell::stacking::rank_of(ctx, aura.spell_id) >= incoming
        })
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
