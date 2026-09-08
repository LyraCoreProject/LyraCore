//! Cast requests retain the scheduled cast identity through completion and cancellation.

use super::*;
use spacetimedb::{ReducerContext, ScheduleAt};

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastRefusalKind {
    Other,
    CannotAct,
    Dead,
    Cooldown,
    OtherPartition,
    OutOfRange,
    InsufficientPower,
    UnsupportedChannel,
}

/// The owning Gate supplies the reason. `Other` preserves a Gate with no caller-specific policy.
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct CastRefusal {
    pub kind: CastRefusalKind,
    pub message: String,
}

impl CastRefusal {
    pub fn new(kind: CastRefusalKind, message: String) -> Self {
        Self { kind, message }
    }
}

impl From<String> for CastRefusal {
    fn from(message: String) -> Self {
        Self::new(CastRefusalKind::Other, message)
    }
}

impl From<CastRefusal> for String {
    fn from(refusal: CastRefusal) -> Self {
        refusal.message
    }
}

impl std::fmt::Display for CastRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct CastHandle {
    pub scheduled_id: u64,
    pub spell_id: u32,
    pub target_guid: u64,
    pub due_micros: i64,
}

impl From<PendingCast> for CastHandle {
    fn from(cast: PendingCast) -> Self {
        let due_micros = match cast.scheduled_at {
            ScheduleAt::Time(at) => at.to_micros_since_unix_epoch(),
            ScheduleAt::Interval(_) => i64::MAX,
        };
        Self {
            scheduled_id: cast.scheduled_id,
            spell_id: cast.spell_id,
            target_guid: cast.target_guid,
            due_micros,
        }
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum CastStart {
    /// Effects dispatched. A projectile may still be in flight.
    Resolved,
    Started(CastHandle),
    Waiting(CastHandle),
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum CastFinish {
    Resolved,
    Refused(CastRefusal),
    Cancelled,
    Expired,
}

/// A request never replaces a pending cast. Explicit cancellation is a separate operation.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn request_cast(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<CastStart, CastRefusal> {
    if let Some(cast) = pending_cast(ctx, caster_guid) {
        return Ok(CastStart::Waiting(cast));
    }
    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    if ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .is_some_and(|spell| spell.cast_flags & SPELL_ATTR_CHANNELED != 0)
    {
        return Err(CastRefusal::new(
            CastRefusalKind::UnsupportedChannel,
            "bot cast requests do not yet retain channels".to_string(),
        ));
    }
    begin_cast_with_admission(
        ctx,
        caster_guid,
        spell_id,
        caster.level as u8,
        target_guid,
        false,
        None,
        CreatureSpellCasterAdmission::Living,
    )?;
    Ok(match pending_cast(ctx, caster_guid) {
        Some(cast) => CastStart::Started(cast),
        None => CastStart::Resolved,
    })
}

pub(crate) fn pending_cast(ctx: &ReducerContext, caster_guid: u64) -> Option<CastHandle> {
    ctx.db
        .game_pending_cast()
        .by_caster()
        .filter(caster_guid)
        .next()
        .map(Into::into)
}
