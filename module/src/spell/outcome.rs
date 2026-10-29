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
    UnlearnedSpell,
    NoLineOfSight,
    BadTargets,
    TargetLevelTooLow,
}

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

/// What the casting client's cast result reports; Package API kinds stay in [`CastRefusalKind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastRefusalReason {
    Unreported,
    NoPower,
    NotReady,
    OutOfRange,
    TooClose,
    NotInFront,
    NoLineOfSight,
    NoAmmo,
    WrongStance,
    NotBehind,
    NotStealthed,
    NoComboPoints,
    SchoolLocked,
    ReactWindowClosed,
    Stunned,
    LevelTooLow,
    CasterDead,
    TargetDead,
    BadTargets,
    MountWhileDead,
    MountInCombat,
    MountIndoors,
    MountInDeepWater,
    TargetLevelTooLow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CastGateRefusal {
    pub(crate) reason: CastRefusalReason,
    pub(crate) refusal: CastRefusal,
}

impl CastGateRefusal {
    pub(crate) fn new(
        kind: CastRefusalKind,
        reason: CastRefusalReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            reason,
            refusal: CastRefusal::new(kind, message.into()),
        }
    }

    pub(crate) fn other(reason: CastRefusalReason, message: impl Into<String>) -> Self {
        Self::new(CastRefusalKind::Other, reason, message)
    }
}

impl From<String> for CastGateRefusal {
    fn from(message: String) -> Self {
        Self::other(CastRefusalReason::Unreported, message)
    }
}

impl From<crate::Refusal> for CastGateRefusal {
    fn from(refusal: crate::Refusal) -> Self {
        String::from(refusal).into()
    }
}

impl From<CastGateRefusal> for CastRefusal {
    fn from(refusal: CastGateRefusal) -> Self {
        refusal.refusal
    }
}

impl From<CastGateRefusal> for String {
    fn from(refusal: CastGateRefusal) -> Self {
        refusal.refusal.message
    }
}

impl std::fmt::Display for CastGateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.refusal.fmt(f)
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct CastHandle {
    pub scheduled_id: u64,
    pub spell_id: u32,
    pub target_guid: u64,
    pub due_micros: i64,
}

impl CastHandle {
    pub(crate) fn from_row(ctx: &ReducerContext, cast: PendingCast) -> Self {
        let due_micros = match cast.scheduled_at {
            ScheduleAt::Time(at) => {
                crate::time::scheduled_time(ctx, at).to_micros_since_unix_epoch()
            }
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

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn cast_readiness(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), CastRefusal> {
    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    if caster.is_player() && !knows_spell(ctx, caster_guid, spell_id) {
        return Err(CastRefusal::new(
            CastRefusalKind::UnlearnedSpell,
            format!("spell {spell_id} is not in the caster's spellbook"),
        ));
    }
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
    check_cast_start_gates(
        ctx,
        &caster,
        spell_id,
        target_guid,
        CreatureSpellCasterAdmission::Living,
    )
    .map_err(Into::into)
}

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
    cast_readiness(ctx, caster_guid, spell_id, target_guid)?;
    begin_cast_with_admission(
        ctx,
        caster_guid,
        spell_id,
        caster.level as u8,
        target_guid,
        false,
        None,
        CreatureSpellCasterAdmission::Living,
    )
    .map_err(Into::into)
}

pub(crate) fn pending_cast(ctx: &ReducerContext, caster_guid: u64) -> Option<CastHandle> {
    ctx.db
        .game_pending_cast()
        .by_caster()
        .filter(caster_guid)
        .next()
        .map(|cast| CastHandle::from_row(ctx, cast))
}
