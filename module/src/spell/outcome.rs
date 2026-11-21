use super::*;
use crate::{game_pending_cast, PendingCast};
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

macro_rules! cast_refusal_reasons {
    ($($reason:ident = $code:literal,)+) => {
        /// What the caster's cast result reports; Package API kinds stay in [`CastRefusalKind`].
        /// Each discriminant is its `SpellCastEvent::failure_reason` code; codes are append-only.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u8)]
        pub(crate) enum CastRefusalReason {
            $($reason = $code,)+
        }

        impl CastRefusalReason {
            /// Every reason, built from the same list as the enum so none is missing.
            const ALL: &'static [Self] = &[$(Self::$reason,)+];
        }
    };
}

cast_refusal_reasons! {
    NoPower = 1,
    Interrupted = 2,
    Unreported = 3,
    CasterAuraState = 4,
    TargetAuraState = 5,
    NotReady = 6,
    OutOfRange = 7,
    TooClose = 8,
    NotInFront = 9,
    NoLineOfSight = 10,
    NoAmmo = 11,
    WrongStance = 12,
    NotShapeshift = 13,
    NotBehind = 14,
    NotStealthed = 15,
    NoComboPoints = 16,
    SchoolLocked = 17,
    ReactWindowClosed = 18,
    Stunned = 19,
    LevelTooLow = 20,
    CasterDead = 21,
    TargetDead = 22,
    BadTargets = 23,
    MountWhileDead = 24,
    AffectingCombat = 25,
    MountIndoors = 26,
    MountInDeepWater = 27,
    TargetLevelTooLow = 28,
    ItemGone = 29,
}

impl CastRefusalReason {
    pub(crate) const fn failure_code(self) -> u8 {
        self as u8
    }

    /// The reason a `SpellCastEvent::failure_reason` code names; zero names none.
    pub(crate) fn from_failure_code(code: u8) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|reason| reason.failure_code() == code)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CastGateRefusal {
    pub(crate) reason: CastRefusalReason,
    pub(crate) refusal: CastRefusal,
    /// The equip error the caster also gets, as vmangos `Spell::CheckItems` sends one.
    pub(crate) item_refusal: Option<lyracore_shared::item::ItemRefusal>,
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
            item_refusal: None,
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

#[allow(dead_code, reason = "Packages call it as actor::cast_readiness")]
pub(crate) fn cast_readiness(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell_id: u32,
    target_guid: u64,
) -> Result<(), CastRefusal> {
    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    ready_spell(ctx, &caster, spell_id, target_guid).map(|_| ())
}

fn ready_spell(
    ctx: &ReducerContext,
    caster: &crate::WorldEntity,
    spell_id: u32,
    target_guid: u64,
) -> Result<LoadedSpell, CastRefusal> {
    if caster.is_player() && !knows_spell(ctx, caster.guid, spell_id) {
        return Err(CastRefusal::new(
            CastRefusalKind::UnlearnedSpell,
            format!("spell {spell_id} is not in the caster's spellbook"),
        ));
    }
    let spell = LoadedSpell::load(ctx, spell_id)?;
    if spell.header.cast_flags & CAST_FLAG_CHANNELED != 0 {
        return Err(CastRefusal::new(
            CastRefusalKind::UnsupportedChannel,
            "bot cast requests do not yet retain channels".to_string(),
        ));
    }
    check_cast_start_gates(
        ctx,
        caster,
        &spell,
        target_guid,
        CreatureSpellCasterAdmission::Living,
    )?;
    Ok(spell)
}

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
    let spell = ready_spell(ctx, &caster, spell_id, target_guid)?;
    begin_loaded_cast(
        ctx,
        &CastRequest::new(caster_guid, spell_id, caster.level as u8, target_guid),
        &spell,
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

#[cfg(test)]
mod failure_code_tests {
    use super::CastRefusalReason;

    #[test]
    fn every_reason_reads_back_from_its_own_nonzero_failure_code() {
        for &reason in CastRefusalReason::ALL {
            assert_ne!(reason.failure_code(), 0, "{reason:?}");
            assert_eq!(
                CastRefusalReason::from_failure_code(reason.failure_code()),
                Some(reason)
            );
        }
        assert_eq!(CastRefusalReason::from_failure_code(0), None);
    }

    #[test]
    fn the_codes_queued_strikes_already_write_keep_their_meaning() {
        assert_eq!(crate::spell::CAST_FAIL_NO_POWER, 1);
        assert_eq!(crate::spell::CAST_FAIL_INTERRUPTED, 2);
    }
}
