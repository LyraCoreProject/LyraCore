use lyracore_shared::constants::go_type;
use lyracore_shared::spell::taxonomy::E_SUMMON_PLAYER;
use spacetimedb::{
    log, reducer, table, ReducerContext, ScheduleAt, SpacetimeType, Table, TimeDuration, Timestamp,
};

use crate::gameobject::{
    game_gameobject, game_gameobject_ritual, game_gameobject_template, GameObject,
    GameObjectRitual, RITUAL_BAND,
};
use crate::spell::{CastRefusalReason, SpellCastEventKind};
use crate::{game_spell, game_spell_effect, game_world_entity, SpellCastEvent, WorldEntity};

// vmangos MAX_PLAYER_SUMMON_DELAY.
pub(crate) const SUMMON_OFFER_MICROS: i64 = 120_000_000;
// vmangos GetClosePoint: object radius plus target radius, both DEFAULT_WORLD_OBJECT_SIZE here.
const LANDING_DISTANCE_YD: f32 = 0.388_888_9 * 2.0;
// Module GameObject states are the inverse of the client's READY and ACTIVE states.
const PORTAL_READY: u8 = 0;
const PORTAL_ACTIVE: u8 = 1;

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RitualPhase {
    /// The owner channels and the object waits for its participants.
    Gathering,
    /// Enough participants stay; the owner casts the ritual's spell.
    Summoning,
    /// The spell was refused; the active object stays until the owner stops channelling.
    Spent,
}

#[table(accessor = game_summoning_ritual)]
pub struct SummoningRitual {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub go_guid: u64,
    pub entry: u32,
    pub owner_guid: u64,
    pub spell_id: u32,
    pub summon_target_guid: u64,
    pub phase: RitualPhase,
    pub expires_at: Timestamp,
}

#[table(
    accessor = game_summoning_ritual_participant,
    index(accessor = by_ritual, btree(columns = [ritual_id]))
)]
pub struct RitualParticipant {
    #[primary_key]
    pub character_guid: u64,
    pub ritual_id: u64,
}

#[table(
    accessor = game_summoning_ritual_schedule,
    scheduled(advance_summoning_ritual),
    index(accessor = by_ritual, btree(columns = [ritual_id]))
)]
pub struct SummoningRitualSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub ritual_id: u64,
}

#[table(accessor = game_summon_offer)]
pub struct SummonOffer {
    #[primary_key]
    pub target_guid: u64,
    pub summoner_guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub expires_at: Timestamp,
}

crate::outside_table_catalog!(
    game_summoning_ritual,
    game_summoning_ritual_participant,
    game_summoning_ritual_schedule,
    game_summon_offer
);

fn after(ctx: &ReducerContext, micros: i64) -> Timestamp {
    let now = crate::time::now(ctx);
    now.checked_add(TimeDuration::from_micros(micros))
        .unwrap_or(now)
}

fn schedule(ctx: &ReducerContext, ritual_id: u64, due: Timestamp) {
    let schedules = ctx.db.game_summoning_ritual_schedule();
    for row in schedules.by_ritual().filter(ritual_id).collect::<Vec<_>>() {
        schedules.scheduled_id().delete(row.scheduled_id);
    }
    schedules.insert(SummoningRitualSchedule {
        scheduled_id: 0,
        scheduled_at: crate::time::schedule_time(ctx, due),
        ritual_id,
    });
}

fn same_group(ctx: &ReducerContext, a: u64, b: u64) -> bool {
    match (
        crate::group::group_of(ctx, a),
        crate::group::group_of(ctx, b),
    ) {
        (Some(a), Some(b)) => a.group_id == b.group_id,
        _ => false,
    }
}

fn summons_player(ctx: &ReducerContext, spell_id: u32) -> bool {
    ctx.db
        .game_spell_effect()
        .by_spell()
        .filter(spell_id)
        .any(|effect| effect.kind == E_SUMMON_PLAYER)
}

fn spell_duration_micros(ctx: &ReducerContext, spell_id: u32) -> Option<i64> {
    let duration_ms = ctx.db.game_spell().spell_id().find(spell_id)?.duration_ms;
    // DBC duration -1 imports as u32::MAX; it and 0 leave the channel to end some other way.
    (duration_ms != u32::MAX && duration_ms != 0).then_some(i64::from(duration_ms) * 1_000)
}

/// Places an `E_SUMMON_OBJECT` object; only a ritual that summons a player has a cast path.
pub(crate) fn place(ctx: &ReducerContext, caster_guid: u64, spell_id: u32, entry: u32) {
    let is_ritual = ctx
        .db
        .game_gameobject_template()
        .entry()
        .find(entry)
        .is_some_and(|template| template.type_id == go_type::SUMMONING_RITUAL);
    let summons = ctx
        .db
        .game_gameobject_ritual()
        .entry()
        .find(entry)
        .is_some_and(|data| summons_player(ctx, data.spell_id));
    let caster = ctx.db.game_world_entity().guid().find(caster_guid);
    let (true, true, Some(caster)) = (is_ritual, summons, caster.filter(WorldEntity::is_player))
    else {
        log::info!("spell {spell_id} places GameObject {entry}, which has no cast path (no-op)");
        return;
    };
    leave(ctx, caster_guid);
    let rituals = ctx.db.game_summoning_ritual();
    let duration = spell_duration_micros(ctx, spell_id);
    let mut ritual = rituals.insert(SummoningRitual {
        id: 0,
        go_guid: 0,
        entry,
        owner_guid: caster_guid,
        spell_id,
        // vmangos EffectTransmitted takes the caster's selection as the summon target.
        summon_target_guid: caster.target_guid,
        phase: RitualPhase::Gathering,
        expires_at: duration.map_or_else(|| crate::time::timer_never(ctx), |us| after(ctx, us)),
    });
    let Some(go_guid) = crate::gameobject::banded_object_guid(RITUAL_BAND, caster_guid, ritual.id)
    else {
        log::warn!("Summoning Ritual {} has no free object guid", ritual.id);
        rituals.id().delete(ritual.id);
        return;
    };
    let portal = GameObject {
        guid: go_guid,
        template_entry: entry,
        map_id: caster.map_id,
        x: caster.x,
        y: caster.y,
        z: caster.z,
        orientation: caster.orientation,
        state: PORTAL_READY,
        created_at: crate::time::now(ctx),
        respawn_at_micros: 0,
        instance_id: caster.instance_id,
        grid_x: 0,
        grid_y: 0,
        cell: 0,
        rotation_0: 0.0,
        rotation_1: 0.0,
        rotation_2: 0.0,
        rotation_3: 0.0,
        respawn_min_secs: 0,
        respawn_max_secs: 0,
        despawned: false,
    };
    if let Err(error) = portal.try_insert(ctx) {
        log::warn!(
            "Summoning Ritual {} object did not insert: {error}",
            ritual.id
        );
        rituals.id().delete(ritual.id);
        return;
    }
    ritual.go_guid = go_guid;
    let ritual = rituals.id().update(ritual);
    schedule(ctx, ritual.id, ritual.expires_at);
    join(ctx, &ritual, caster_guid, spell_id);
}

fn join(ctx: &ReducerContext, ritual: &SummoningRitual, character_guid: u64, channel_spell: u32) {
    ctx.db
        .game_summoning_ritual_participant()
        .insert(RitualParticipant {
            character_guid,
            ritual_id: ritual.id,
        });
    if channel_spell == 0 {
        return;
    }
    let duration_ms = spell_duration_micros(ctx, channel_spell).map_or(u32::MAX, |us| {
        (us / 1_000).clamp(0, i64::from(u32::MAX)) as u32
    });
    crate::native::aura::channel_started(ctx, character_guid, channel_spell, duration_ms);
}

fn participants(ctx: &ReducerContext, ritual_id: u64) -> Vec<RitualParticipant> {
    ctx.db
        .game_summoning_ritual_participant()
        .by_ritual()
        .filter(ritual_id)
        .collect()
}

/// A participant uses the ritual object, as vmangos `GameObject::Use` for a summoning ritual.
pub(crate) fn assist(
    ctx: &ReducerContext,
    user_guid: u64,
    mut portal: GameObject,
) -> Result<(), String> {
    let ritual = ctx
        .db
        .game_summoning_ritual()
        .go_guid()
        .find(portal.guid)
        .ok_or("this Summoning Ritual has ended")?;
    let data = ctx
        .db
        .game_gameobject_ritual()
        .entry()
        .find(ritual.entry)
        .ok_or("this GameObject has no Summoning Ritual data")?;
    if user_guid == ritual.owner_guid {
        return Err("the ritual's owner already channels it".into());
    }
    if data.casters_grouped && !same_group(ctx, user_guid, ritual.owner_guid) {
        return Err("only the owner's party or raid can join the ritual".into());
    }
    let participants = ctx.db.game_summoning_ritual_participant();
    if participants
        .character_guid()
        .find(ritual.owner_guid)
        .is_none_or(|owner| owner.ritual_id != ritual.id)
    {
        return Err("the ritual's owner no longer channels it".into());
    }
    if participants
        .character_guid()
        .find(user_guid)
        .is_some_and(|row| row.ritual_id == ritual.id)
    {
        return Ok(());
    }
    crate::spell::break_channel(ctx, user_guid);
    join(ctx, &ritual, user_guid, data.anim_spell_id);
    let count = self::participants(ctx, ritual.id).len();
    if count < data.required_participants as usize
        || data.spell_id == 0
        || ritual.phase != RitualPhase::Gathering
    {
        return Ok(());
    }
    portal.state = PORTAL_ACTIVE;
    portal.update(ctx);
    let Some(owner) = ctx.db.game_world_entity().guid().find(ritual.owner_guid) else {
        return Ok(());
    };
    // vmangos Spell::prepare refuses the ritual spell before its cast bar starts.
    if let Err(reason) = summon_refusal(ctx, &owner, ritual.summon_target_guid) {
        crate::native::summon::cast_refused(ctx, owner.guid, data.spell_id, reason);
        ctx.db.game_summoning_ritual().id().update(SummoningRitual {
            phase: RitualPhase::Spent,
            ..ritual
        });
        return Ok(());
    }
    let cast_ms = ctx
        .db
        .game_spell()
        .spell_id()
        .find(data.spell_id)
        .map_or(0, |spell| spell.cast_time_ms);
    let ritual = ctx.db.game_summoning_ritual().id().update(SummoningRitual {
        phase: RitualPhase::Summoning,
        ..ritual
    });
    schedule(ctx, ritual.id, after(ctx, i64::from(cast_ms) * 1_000));
    SpellCastEvent::insert(
        ctx,
        SpellCastEvent {
            target_guid: ritual.summon_target_guid,
            cast_time_ms: cast_ms,
            ..SpellCastEvent::signal_at(ctx, &owner, data.spell_id, SpellCastEventKind::Start)
        },
    );
    Ok(())
}

/// The spell a Character channels for its Summoning Ritual: the owner's spell or the object's.
pub(crate) fn channel_spell(ctx: &ReducerContext, character_guid: u64) -> Option<u32> {
    let row = ctx
        .db
        .game_summoning_ritual_participant()
        .character_guid()
        .find(character_guid)?;
    let ritual = ctx.db.game_summoning_ritual().id().find(row.ritual_id)?;
    if ritual.owner_guid == character_guid {
        return Some(ritual.spell_id);
    }
    ctx.db
        .game_gameobject_ritual()
        .entry()
        .find(ritual.entry)
        .map(|data| data.anim_spell_id)
        .filter(|&spell_id| spell_id != 0)
}

/// Ends the Character's ritual channel, as vmangos `RemoveUniqueUse`; returns whether it had one.
pub(crate) fn leave(ctx: &ReducerContext, character_guid: u64) -> bool {
    let participants = ctx.db.game_summoning_ritual_participant();
    let Some(row) = participants.character_guid().find(character_guid) else {
        return false;
    };
    participants.character_guid().delete(character_guid);
    crate::native::aura::channel_ended(ctx, character_guid);
    let Some(ritual) = ctx.db.game_summoning_ritual().id().find(row.ritual_id) else {
        return true;
    };
    let data = ctx.db.game_gameobject_ritual().entry().find(ritual.entry);
    let required = data.as_ref().map_or(0, |data| data.required_participants) as usize;
    let summoning = ritual.phase == RitualPhase::Summoning;
    if character_guid != ritual.owner_guid
        && !(summoning && self::participants(ctx, ritual.id).len() < required)
    {
        return true;
    }
    let owner = ctx.db.game_world_entity().guid().find(ritual.owner_guid);
    if let (true, Some(owner)) = (summoning, owner) {
        let spell_id = data.as_ref().map_or(0, |data| data.spell_id);
        SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                is_interrupted: true,
                failure_reason: CastRefusalReason::Interrupted.failure_code(),
                ..SpellCastEvent::signal_at(ctx, &owner, spell_id, SpellCastEventKind::Interrupt)
            },
        );
    }
    if data.is_some_and(|data| data.persistent) {
        rest(ctx, ritual);
    } else {
        end(ctx, ritual);
    }
    true
}

/// A persistent ritual object returns to waiting for participants.
fn rest(ctx: &ReducerContext, ritual: SummoningRitual) {
    if let Some(mut portal) = ctx.db.game_gameobject().guid().find(ritual.go_guid) {
        portal.state = PORTAL_READY;
        portal.update(ctx);
    }
    let ritual = ctx.db.game_summoning_ritual().id().update(SummoningRitual {
        phase: RitualPhase::Gathering,
        ..ritual
    });
    schedule(ctx, ritual.id, ritual.expires_at);
}

/// Removes the ritual object and ends every participant's channel.
fn end(ctx: &ReducerContext, ritual: SummoningRitual) {
    for row in participants(ctx, ritual.id) {
        ctx.db
            .game_summoning_ritual_participant()
            .character_guid()
            .delete(row.character_guid);
        crate::native::aura::channel_ended(ctx, row.character_guid);
    }
    let schedules = ctx.db.game_summoning_ritual_schedule();
    for row in schedules.by_ritual().filter(ritual.id).collect::<Vec<_>>() {
        schedules.scheduled_id().delete(row.scheduled_id);
    }
    ctx.db.game_summoning_ritual().id().delete(ritual.id);
    GameObject::delete(ctx, ritual.go_guid);
}

#[reducer]
pub fn advance_summoning_ritual(ctx: &ReducerContext, row: SummoningRitualSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    ctx.db
        .game_summoning_ritual_schedule()
        .scheduled_id()
        .delete(row.scheduled_id);
    let Some(ritual) = ctx.db.game_summoning_ritual().id().find(row.ritual_id) else {
        return;
    };
    let data = ctx.db.game_gameobject_ritual().entry().find(ritual.entry);
    match (ritual.phase, data) {
        (RitualPhase::Summoning, Some(data)) => finish(ctx, ritual, &data),
        _ => end(ctx, ritual),
    }
}

/// The ritual spell completes, as vmangos `Spell::cast` and `GameObject::FinishRitual`.
fn finish(ctx: &ReducerContext, ritual: SummoningRitual, data: &GameObjectRitual) {
    let entities = ctx.db.game_world_entity();
    let portal = ctx.db.game_gameobject().guid().find(ritual.go_guid);
    let (Some(owner), Some(portal), true) = (
        entities.guid().find(ritual.owner_guid),
        portal,
        summons_player(ctx, data.spell_id),
    ) else {
        end(ctx, ritual);
        return;
    };
    if let Err(reason) = summon_refusal(ctx, &owner, ritual.summon_target_guid) {
        SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                is_interrupted: true,
                failure_reason: reason.failure_code(),
                ..SpellCastEvent::signal_at(
                    ctx,
                    &owner,
                    data.spell_id,
                    SpellCastEventKind::Interrupt,
                )
            },
        );
        let ritual = ctx.db.game_summoning_ritual().id().update(SummoningRitual {
            phase: RitualPhase::Spent,
            ..ritual
        });
        schedule(ctx, ritual.id, ritual.expires_at);
        return;
    }
    SpellCastEvent::insert(
        ctx,
        SpellCastEvent {
            target_guid: ritual.summon_target_guid,
            is_completion: true,
            ..SpellCastEvent::signal_at(ctx, &owner, data.spell_id, SpellCastEventKind::Go)
        },
    );
    let beside = landing_beside(portal.x, portal.y, portal.orientation);
    let landing = on_floor(ctx, beside, portal.map_id, portal.instance_id, portal.z);
    offer(ctx, &owner, ritual.summon_target_guid, landing);
    if let Some(spell) = ctx.db.game_spell().spell_id().find(ritual.spell_id) {
        crate::spell::start_spell_cooldown(ctx, owner.guid, spell.spell_id, spell.cooldown_ms);
    }
    if data.persistent {
        let ritual = ctx.db.game_summoning_ritual().id().update(SummoningRitual {
            phase: RitualPhase::Spent,
            ..ritual
        });
        schedule(ctx, ritual.id, ritual.expires_at);
    } else {
        end(ctx, ritual);
    }
}

fn landing_beside(x: f32, y: f32, orientation: f32) -> (f32, f32) {
    (
        x + LANDING_DISTANCE_YD * orientation.cos(),
        y + LANDING_DISTANCE_YD * orientation.sin(),
    )
}

fn on_floor(
    ctx: &ReducerContext,
    (x, y): (f32, f32),
    map_id: u32,
    instance_id: u64,
    z: f32,
) -> (f32, f32, f32) {
    (
        x,
        y,
        crate::terrain::snap_z(ctx, map_id, instance_id, x, y, z),
    )
}

/// vmangos `Spell::CheckCast` for SPELL_EFFECT_SUMMON_PLAYER.
pub(crate) fn summon_refusal(
    ctx: &ReducerContext,
    summoner: &WorldEntity,
    target_guid: u64,
) -> Result<(), CastRefusalReason> {
    if !summoner.is_player() {
        return Err(CastRefusalReason::BadTargets);
    }
    let target = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .filter(|target| target.is_player() && target.guid != summoner.guid)
        .ok_or(CastRefusalReason::BadTargets)?;
    if !same_group(ctx, summoner.guid, target.guid) {
        return Err(CastRefusalReason::BadTargets);
    }
    if crate::instance::is_dungeon_map(summoner.map_id)
        && (target.map_id, target.instance_id) != (summoner.map_id, summoner.instance_id)
    {
        return Err(CastRefusalReason::TargetNotInInstance);
    }
    Ok(())
}

/// An `E_SUMMON_PLAYER` effect cast outside a ritual offers a landing beside its caster.
pub(crate) fn summon_to_caster(ctx: &ReducerContext, caster_guid: u64, target_guid: u64) {
    let Some(caster) = ctx.db.game_world_entity().guid().find(caster_guid) else {
        return;
    };
    if let Err(reason) = summon_refusal(ctx, &caster, target_guid) {
        log::info!("summon of {target_guid} by {caster_guid} refused: {reason:?}");
        return;
    }
    let beside = landing_beside(caster.x, caster.y, caster.orientation);
    let landing = on_floor(ctx, beside, caster.map_id, caster.instance_id, caster.z);
    offer(ctx, &caster, target_guid, landing);
}

fn offer(
    ctx: &ReducerContext,
    summoner: &WorldEntity,
    target_guid: u64,
    (x, y, z): (f32, f32, f32),
) {
    let offers = ctx.db.game_summon_offer();
    offers.target_guid().delete(target_guid);
    let offer = offers.insert(SummonOffer {
        target_guid,
        summoner_guid: summoner.guid,
        map_id: summoner.map_id,
        instance_id: summoner.instance_id,
        x,
        y,
        z,
        expires_at: after(ctx, SUMMON_OFFER_MICROS),
    });
    crate::native::summon::offered(ctx, &offer, summoner.zone_id);
}

/// The target accepts `summoner_guid`'s Summon Offer, as vmangos `HandleSummonResponseOpcode`.
pub(crate) fn accept(
    ctx: &ReducerContext,
    target_guid: u64,
    summoner_guid: u64,
) -> Result<(), String> {
    let offers = ctx.db.game_summon_offer();
    let offer = offers
        .target_guid()
        .find(target_guid)
        .filter(|offer| offer.summoner_guid == summoner_guid)
        .ok_or("no Summon Offer from that Character")?;
    // vmangos SummonIfPossible ignores a lapsed offer; deleting it must not roll back.
    if offer.expires_at < crate::time::now(ctx) {
        offers.target_guid().delete(target_guid);
        return Ok(());
    }
    let target = crate::helpers::live_entity(ctx, target_guid)?;
    if target.dead || target.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
        return Err("a dead Character or one in combat cannot accept a summon".into());
    }
    offers.target_guid().delete(target_guid);
    crate::world::teleport(
        ctx,
        target_guid,
        offer.map_id,
        offer.instance_id,
        [offer.x, offer.y, offer.z, target.orientation],
    )
    .map_err(String::from)
}

/// Drops the Character's ritual channel and Summon Offer when it leaves its map or Instance.
pub(crate) fn character_left(ctx: &ReducerContext, character_guid: u64) {
    leave(ctx, character_guid);
    ctx.db
        .game_summon_offer()
        .target_guid()
        .delete(character_guid);
}

#[cfg(test)]
mod tests {
    use super::landing_beside;

    #[test]
    fn a_summoned_character_lands_beside_the_object_along_its_facing() {
        let (x, y) = landing_beside(10.0, 20.0, 0.0);
        assert!((x - 10.777_778).abs() < 1e-4 && (y - 20.0).abs() < 1e-4);
        let (x, y) = landing_beside(10.0, 20.0, std::f32::consts::FRAC_PI_2);
        assert!((x - 10.0).abs() < 1e-4 && (y - 20.777_778).abs() < 1e-4);
    }
}
