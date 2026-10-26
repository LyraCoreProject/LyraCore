use lyracore_package_delta::packed_race_class_id;
use spacetimedb::{
    reducer, table, Identity, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp,
};

use crate::items::game_item_instance;
use crate::{
    game_char_base_info, game_character, game_start_position, game_world_entity, Character,
};

#[table(accessor = game_account)]
pub struct Account {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub username: String,
    pub salt: Vec<u8>,     // 32 bytes
    pub verifier: Vec<u8>, // 32 bytes
    pub identity: Option<Identity>,
    pub banned: bool,
    #[default(true)]
    pub alpha_test_tools: bool,
}

#[table(accessor = game_alpha_test_tools_enrollment)]
pub struct AlphaTestToolsEnrollment {
    #[primary_key]
    pub id: u8,
    pub enabled: bool,
}

const ALPHA_TEST_TOOLS_ENROLLMENT_ID: u8 = 0;

fn alpha_test_tools_enrollment(enabled: Option<bool>) -> bool {
    enabled.unwrap_or(true)
}

fn provisioned_alpha_test_tools(existing: Option<bool>, enrollment: bool) -> bool {
    existing.unwrap_or(enrollment)
}

fn changed_alpha_test_tools(existing: Option<bool>, enabled: bool) -> Result<bool, &'static str> {
    existing.map(|_| enabled).ok_or("unknown account")
}

fn realm_account_name(account_name: &str) -> String {
    account_name.to_ascii_uppercase()
}

fn current_alpha_test_tools_enrollment(ctx: &ReducerContext) -> bool {
    alpha_test_tools_enrollment(
        ctx.db
            .game_alpha_test_tools_enrollment()
            .id()
            .find(ALPHA_TEST_TOOLS_ENROLLMENT_ID)
            .map(|row| row.enabled),
    )
}

pub(crate) fn ensure_shadow_account(ctx: &ReducerContext, account_id: u64) {
    let accounts = ctx.db.game_account();
    if accounts.id().find(account_id).is_some() {
        return;
    }
    accounts.insert(Account {
        id: account_id,
        username: format!("#{account_id}"),
        salt: Vec::new(),
        verifier: Vec::new(),
        identity: None,
        banned: false,
        alpha_test_tools: false,
    });
}

#[table(accessor = game_session)]
pub struct Session {
    #[primary_key]
    pub account_id: u64,
    pub session_key: Vec<u8>, // 40 bytes
    pub identity: Option<Identity>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

pub(crate) const SESSION_TTL_MICROS: i64 = 3_600_000_000;
pub(crate) const SESSION_REAP_MICROS: i64 = 300_000_000;

#[table(accessor = game_session_reaper_schedule, scheduled(reap_sessions))]
pub struct SessionReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

pub(crate) fn session_expired(now_micros: i64, expires_at_micros: i64) -> bool {
    expires_at_micros <= now_micros
}

pub(crate) fn expired_sessions(
    sessions: impl Iterator<Item = (u64, i64)>,
    now_micros: i64,
) -> Vec<u64> {
    sessions
        .filter(|(_, expires_at_micros)| session_expired(now_micros, *expires_at_micros))
        .map(|(account_id, _)| account_id)
        .collect()
}

#[reducer]
pub fn reap_sessions(ctx: &ReducerContext, _schedule: SessionReaperSchedule) -> Result<(), String> {
    if ctx.sender() != ctx.database_identity() {
        return Err("scheduler only".to_string());
    }
    let sessions = ctx.db.game_session();
    let expired = expired_sessions(
        sessions
            .iter()
            .map(|s| (s.account_id, s.expires_at.to_micros_since_unix_epoch())),
        ctx.timestamp.to_micros_since_unix_epoch(),
    );
    let active = crate::native::active_session_accounts(ctx);
    for account_id in expired {
        if !active.contains(&account_id) {
            sessions.account_id().delete(account_id);
        }
    }
    Ok(())
}

fn ensure_session_reaper_armed(ctx: &ReducerContext) {
    let schedule = ctx.db.game_session_reaper_schedule();
    if schedule.iter().next().is_some() {
        return;
    }
    schedule.insert(SessionReaperSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(SESSION_REAP_MICROS)),
    });
}

// The Owner Token caller differs from ctx.database_identity(), so capture it explicitly.
#[table(accessor = game_operator)]
pub struct Operator {
    #[primary_key]
    pub id: u8,
    pub identity: Identity,
}

#[derive(Debug, Eq, PartialEq)]
enum OperatorClaimAction {
    Insert,
    AlreadyOwned,
}

fn operator_claim_action(
    claimed_identity: Option<Identity>,
    sender: Identity,
) -> Result<OperatorClaimAction, String> {
    match claimed_identity {
        None => Ok(OperatorClaimAction::Insert),
        Some(identity) if identity == sender => Ok(OperatorClaimAction::AlreadyOwned),
        Some(_) => Err("operator already claimed".to_string()),
    }
}

#[reducer]
pub fn claim_operator(ctx: &ReducerContext) -> Result<(), String> {
    let ops = ctx.db.game_operator();
    let claimed_identity = ops.id().find(0).map(|operator| operator.identity);
    if operator_claim_action(claimed_identity, ctx.sender())? == OperatorClaimAction::Insert {
        ops.insert(Operator {
            id: 0,
            identity: ctx.sender(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod operator_claim_tests {
    use super::{operator_claim_action, OperatorClaimAction};
    use spacetimedb::Identity;

    fn identity(byte: u8) -> Identity {
        Identity::from_byte_array([byte; 32])
    }

    #[test]
    fn an_unclaimed_database_inserts_the_sender() {
        assert_eq!(
            operator_claim_action(None, identity(1)),
            Ok(OperatorClaimAction::Insert)
        );
    }

    #[test]
    fn a_retry_by_the_claimed_identity_is_an_idempotent_success() {
        let owner = identity(1);
        assert_eq!(
            operator_claim_action(Some(owner), owner),
            Ok(OperatorClaimAction::AlreadyOwned)
        );
    }

    #[test]
    fn a_different_identity_remains_refused() {
        assert_eq!(
            operator_claim_action(Some(identity(1)), identity(2)),
            Err("operator already claimed".to_string())
        );
    }
}

#[reducer]
pub fn provision_account(
    ctx: &ReducerContext,
    username: String,
    salt: Vec<u8>,
    verifier: Vec<u8>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let accounts = ctx.db.game_account();
    match accounts.username().find(&username) {
        Some(mut acc) => {
            acc.salt = salt;
            acc.verifier = verifier;
            accounts.id().update(acc);
        }
        None => {
            accounts.insert(Account {
                id: 0,
                username,
                salt,
                verifier,
                identity: None,
                banned: false,
                alpha_test_tools: provisioned_alpha_test_tools(
                    None,
                    current_alpha_test_tools_enrollment(ctx),
                ),
            });
        }
    }
    Ok(())
}

#[reducer]
pub fn set_alpha_test_tools_enrollment(ctx: &ReducerContext, enabled: bool) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let enrollment = ctx.db.game_alpha_test_tools_enrollment();
    let row = AlphaTestToolsEnrollment {
        id: ALPHA_TEST_TOOLS_ENROLLMENT_ID,
        enabled,
    };
    if enrollment
        .id()
        .find(ALPHA_TEST_TOOLS_ENROLLMENT_ID)
        .is_some()
    {
        enrollment.id().update(row);
    } else {
        enrollment.insert(row);
    }
    Ok(())
}

fn set_account_alpha_test_tools(
    ctx: &ReducerContext,
    account_name: &str,
    enabled: bool,
) -> Result<(), String> {
    let account_name = realm_account_name(account_name);
    let accounts = ctx.db.game_account();
    let existing = accounts.username().find(&account_name);
    let alpha_test_tools = changed_alpha_test_tools(
        existing.as_ref().map(|account| account.alpha_test_tools),
        enabled,
    )
    .map_err(|_| format!("no Account named {account_name}"))?;
    let mut account = existing.expect("the authority decision requires an existing Account");
    account.alpha_test_tools = alpha_test_tools;
    accounts.id().update(account);
    Ok(())
}

#[reducer]
pub fn grant_alpha_test_tools(ctx: &ReducerContext, account_name: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    set_account_alpha_test_tools(ctx, &account_name, true)
}

#[reducer]
pub fn revoke_alpha_test_tools(ctx: &ReducerContext, account_name: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    set_account_alpha_test_tools(ctx, &account_name, false)
}

#[cfg(test)]
mod alpha_test_tools_tests {
    use super::{
        alpha_test_tools_enrollment, changed_alpha_test_tools, provisioned_alpha_test_tools,
        realm_account_name,
    };

    #[test]
    fn enrollment_starts_enabled_and_tracks_operator_changes() {
        assert!(alpha_test_tools_enrollment(None));
        assert!(alpha_test_tools_enrollment(Some(true)));
        assert!(!alpha_test_tools_enrollment(Some(false)));
    }

    #[test]
    fn new_accounts_copy_enrollment_while_existing_accounts_keep_their_authority() {
        assert!(provisioned_alpha_test_tools(None, true));
        assert!(!provisioned_alpha_test_tools(None, false));
        assert!(provisioned_alpha_test_tools(Some(true), false));
        assert!(!provisioned_alpha_test_tools(Some(false), true));
    }

    #[test]
    fn explicit_grant_and_revoke_change_only_a_known_account() {
        assert_eq!(changed_alpha_test_tools(Some(false), true), Ok(true));
        assert_eq!(changed_alpha_test_tools(Some(true), false), Ok(false));
        assert_eq!(changed_alpha_test_tools(None, true), Err("unknown account"));
        assert_eq!(realm_account_name("TeSt"), "TEST");
    }
}

#[reducer]
pub fn establish_session(
    ctx: &ReducerContext,
    account_id: u64,
    session_key: Vec<u8>,
    bound_identity: Identity,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let accounts = ctx.db.game_account();
    let mut acc = accounts
        .id()
        .find(account_id)
        .ok_or_else(|| format!("no such account id: {account_id}"))?;
    acc.identity = Some(bound_identity);
    accounts.id().update(acc);

    let now = ctx.timestamp;
    let expires = now
        .checked_add(TimeDuration::from_micros(SESSION_TTL_MICROS))
        .unwrap_or(now);
    let sessions = ctx.db.game_session();
    let row = Session {
        account_id,
        session_key,
        identity: Some(bound_identity),
        created_at: now,
        expires_at: expires,
    };
    if sessions.account_id().find(account_id).is_some() {
        sessions.account_id().update(row);
    } else {
        sessions.insert(row);
    }
    ensure_session_reaper_armed(ctx);
    Ok(())
}

#[cfg(test)]
mod session_expiry_tests {
    use super::{expired_sessions, session_expired};

    #[test]
    fn a_session_expires_at_its_deadline_and_not_before() {
        assert!(!session_expired(999, 1_000));
        assert!(session_expired(1_000, 1_000));
        assert!(session_expired(1_001, 1_000));
    }

    #[test]
    fn the_reaper_selects_only_the_sessions_whose_window_closed() {
        let rows = [(1, 500), (2, 1_000), (3, 1_500)].into_iter();
        assert_eq!(expired_sessions(rows, 1_000), vec![1, 2]);
    }
}

// Deletion and Transfer never lower the issued GUID floor; surviving rows cannot recover it.
#[table(accessor = game_guid_allocator)]
pub struct GuidAllocator {
    #[primary_key]
    pub id: u32,
    pub high_water: u64,
}

fn ratchet_high_water(current: u64, floor: u64) -> u64 {
    current.max(floor)
}

fn legacy_guid_seed(
    char_guids: impl Iterator<Item = u64>,
    owned_guids: impl Iterator<Item = u64>,
) -> u64 {
    char_guids
        .max()
        .unwrap_or(0)
        .max(owned_guids.max().unwrap_or(0))
}

fn allocate_next_guid(seed: u64) -> u64 {
    seed + 1
}

fn read_high_water(ctx: &ReducerContext) -> Option<GuidAllocator> {
    ctx.db.game_guid_allocator().id().find(0)
}

fn legacy_guid_seed_now(ctx: &ReducerContext) -> u64 {
    legacy_guid_seed(
        ctx.db.game_character().iter().map(|c| c.guid),
        ctx.db.game_item_instance().iter().map(|i| i.owner_guid),
    )
}

fn write_high_water(ctx: &ReducerContext, existing: Option<GuidAllocator>, mark: u64) {
    match existing {
        Some(row) => {
            ctx.db.game_guid_allocator().id().update(GuidAllocator {
                high_water: mark,
                ..row
            });
        }
        None => {
            ctx.db.game_guid_allocator().insert(GuidAllocator {
                id: 0,
                high_water: mark,
            });
        }
    }
}

pub(crate) fn next_character_guid(ctx: &ReducerContext) -> u64 {
    let existing = read_high_water(ctx);
    let seed = existing
        .as_ref()
        .map(|r| r.high_water)
        .unwrap_or_else(|| legacy_guid_seed_now(ctx));
    let next = allocate_next_guid(seed);
    write_high_water(ctx, existing, next);
    next
}

pub(crate) fn reserve_guids(
    ctx: &ReducerContext,
    count: u64,
    max_guid: u64,
) -> Result<std::ops::RangeInclusive<u64>, String> {
    let existing = read_high_water(ctx);
    let mark = existing
        .as_ref()
        .map(|row| row.high_water)
        .unwrap_or_else(|| legacy_guid_seed_now(ctx));
    let range = ctx
        .db
        .game_guid_range()
        .id()
        .find(0)
        .map(|row| (row.base, row.size));
    let last = guid_batch_end(range, mark, count, max_guid)?;
    write_high_water(ctx, existing, last);
    Ok(mark + 1..=last)
}

fn guid_batch_end(
    range: Option<(u64, u64)>,
    mark: u64,
    count: u64,
    max_guid: u64,
) -> Result<u64, String> {
    let (base, size) = range.ok_or_else(|| "NO_GUID_RANGE".to_owned())?;
    let end = base
        .checked_add(size)
        .ok_or_else(|| "GUID_RANGE_EXHAUSTED".to_owned())?;
    let last = mark
        .checked_add(count)
        .ok_or_else(|| "GUID_RANGE_EXHAUSTED".to_owned())?;
    if count == 0 || mark < base || last >= end {
        return Err("GUID_RANGE_EXHAUSTED".to_owned());
    }
    if last > max_guid {
        return Err("ITEM_GUID_EXHAUSTED".to_owned());
    }
    Ok(last)
}

// Foreign guids cannot advance an installed range; retain the legacy floor before installation.
pub(crate) fn bump_guid_high_water(ctx: &ReducerContext, guid: u64) {
    let range = ctx
        .db
        .game_guid_range()
        .id()
        .find(0)
        .map(|row| (row.base, row.size));
    if range.is_some() && !in_guid_range(range, guid) {
        return;
    }
    let existing = read_high_water(ctx);
    let seed = existing
        .as_ref()
        .map(|r| r.high_water)
        .unwrap_or_else(|| legacy_guid_seed_now(ctx));
    let mark = ratchet_high_water(seed, guid);
    if mark == seed && existing.is_some() {
        return;
    }
    write_high_water(ctx, existing, mark);
}

// Reassignment could reissue identities still held in other Shards or Escrow.
#[table(accessor = game_guid_range)]
pub struct GuidRange {
    #[primary_key]
    pub id: u32,
    pub base: u64,
    pub size: u64,
}

pub(crate) fn may_mint(range: Option<(u64, u64)>, mark: u64) -> Result<(), String> {
    guid_batch_end(range, mark, 1, u64::MAX).map(|_| ())
}

pub(crate) fn require_guid_range(ctx: &ReducerContext) -> Result<(), String> {
    let range = ctx
        .db
        .game_guid_range()
        .id()
        .find(0)
        .map(|r| (r.base, r.size));
    let mark = read_high_water(ctx).map(|r| r.high_water);
    may_mint(range, mark.unwrap_or(0))
}

pub(crate) fn in_guid_range(range: Option<(u64, u64)>, guid: u64) -> bool {
    let Some((base, size)) = range else {
        return false;
    };
    guid >= base && guid < base.saturating_add(size)
}

#[reducer]
pub fn install_guid_range(ctx: &ReducerContext, base: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let size = crate::realm_core::GUID_RANGE_SIZE;
    let ranges = ctx.db.game_guid_range();
    if let Some(row) = ranges.id().find(0) {
        if row.base == base && row.size == size {
            return Ok(());
        }
        return Err(format!(
            "install_guid_range: this database already holds [{}, {}) and a range is never \
             reassigned — guids already minted from it are referenced by rows that outlive the \
             character (refusing [{base}, {}))",
            row.base,
            row.base.saturating_add(row.size),
            base.saturating_add(size)
        ));
    }
    let mark = read_high_water(ctx)
        .map(|r| r.high_water)
        .unwrap_or_else(|| legacy_guid_seed_now(ctx));
    if mark >= base.saturating_add(size) {
        return Err(format!(
            "install_guid_range: this database has already minted up to {mark}, which is outside \
             the assigned range [{base}, {}) — assigning it would re-issue live guids",
            base.saturating_add(size)
        ));
    }
    ranges.insert(GuidRange { id: 0, base, size });
    bump_guid_high_water(ctx, base);
    spacetimedb::log::info!(
        "install_guid_range: this database mints from [{base}, {}); mark was {mark}",
        base.saturating_add(size)
    );
    Ok(())
}

#[cfg(test)]
mod guid_allocator_tests {
    use super::{allocate_next_guid, legacy_guid_seed, ratchet_high_water};

    #[test]
    fn a_guid_floor_only_ever_ratchets_upwards() {
        assert_eq!(ratchet_high_water(0, 1_000_000_000), 1_000_000_000);
        assert_eq!(
            ratchet_high_water(2_000_000_050, 1_000_000_000),
            2_000_000_050,
            "a floor below the current mark rewound the allocator onto live guids"
        );
    }

    #[test]
    fn guid_batches_share_the_character_boundary_and_refuse_overflow() {
        use super::guid_batch_end;
        assert_eq!(guid_batch_end(Some((100, 10)), 106, 3, u64::MAX), Ok(109));
        assert_eq!(
            guid_batch_end(Some((100, 10)), 109, 1, u64::MAX).unwrap_err(),
            "GUID_RANGE_EXHAUSTED"
        );
        assert!(guid_batch_end(Some((100, 10)), 106, 4, u64::MAX).is_err());
        assert!(guid_batch_end(Some((u64::MAX - 1, 2)), u64::MAX - 1, 1, u64::MAX).is_err());
        assert!(guid_batch_end(Some((100, 10)), 99, 1, u64::MAX).is_err());
        assert_eq!(
            guid_batch_end(Some((100, 10)), 106, 3, 108).unwrap_err(),
            "ITEM_GUID_EXHAUSTED"
        );
        assert_eq!(
            guid_batch_end(None, 0, 1, u64::MAX).unwrap_err(),
            "NO_GUID_RANGE"
        );
    }

    #[test]
    fn a_database_without_an_assigned_range_refuses_to_mint() {
        use super::may_mint;
        assert_eq!(may_mint(None, 0).unwrap_err(), "NO_GUID_RANGE");
        assert_eq!(
            may_mint(None, 5_000).unwrap_err(),
            "NO_GUID_RANGE",
            "having minted before is not a licence — that IS the shard that mints into someone \
             else's range"
        );
        assert!(may_mint(Some((1_000_000_000, 1_000_000_000)), 1_000_000_000).is_ok());
        assert!(may_mint(Some((1_000_000_000, 1_000_000_000)), 1_999_999_998).is_ok());
        assert!(may_mint(Some((1_000_000_000, 1_000_000_000)), 1_999_999_999).is_err());
        assert_eq!(
            may_mint(Some((1_000_000_000, 1_000_000_000)), 2_000_000_000).unwrap_err(),
            "GUID_RANGE_EXHAUSTED"
        );
    }

    #[test]
    fn in_guid_range_only_admits_guids_inside_the_installed_range() {
        use super::in_guid_range;
        assert!(!in_guid_range(None, 0));
        assert!(!in_guid_range(None, 4242));
        let range = Some((1_000_000_000, 1_000_000_000));
        assert!(
            in_guid_range(range, 1_000_000_000),
            "the base itself is inside its own range"
        );
        assert!(
            in_guid_range(range, 1_999_999_999),
            "the last guid before the end is inside"
        );
        assert!(
            !in_guid_range(range, 2_000_000_000),
            "the end itself belongs to the NEXT range"
        );
        assert!(!in_guid_range(range, 0));
        assert!(!in_guid_range(range, 3_000_000_000));
    }

    #[test]
    fn ratchet_high_water_never_moves_backwards() {
        assert_eq!(
            ratchet_high_water(5, 3),
            5,
            "a lower floor must not pull the mark down"
        );
        assert_eq!(
            ratchet_high_water(5, 9),
            9,
            "a higher floor must push the mark up"
        );
        assert_eq!(ratchet_high_water(0, 0), 0);
    }

    #[test]
    fn legacy_guid_seed_is_the_max_of_both_sources() {
        assert_eq!(
            legacy_guid_seed([1u64, 7, 3].into_iter(), [2u64, 4].into_iter()),
            7
        );
        assert_eq!(
            legacy_guid_seed([1u64, 2].into_iter(), [9u64, 3].into_iter()),
            9
        );
        assert_eq!(legacy_guid_seed(std::iter::empty(), std::iter::empty()), 0);
    }

    #[test]
    fn allocate_next_guid_always_advances_past_the_seed() {
        assert_eq!(allocate_next_guid(50), 51);
        assert_eq!(allocate_next_guid(0), 1);
        assert_ne!(
            allocate_next_guid(7),
            7,
            "must never hand out the seed itself"
        );
    }

    #[test]
    fn defect_1_transfer_then_create_no_longer_reissues_the_departed_guid() {
        let chars_after_transfer = [10u64, 30, 49];
        let seed = legacy_guid_seed(chars_after_transfer.into_iter(), std::iter::empty());
        let allocator_mark_after_delete = ratchet_high_water(seed, 50);
        assert_eq!(allocator_mark_after_delete, 50);
        let reissued_guid = allocate_next_guid(allocator_mark_after_delete);
        assert_ne!(reissued_guid, 50);
        assert_eq!(reissued_guid, 51);
    }
}

// The 5875 client refuses a Character list longer than this limit.
pub(crate) const CHARACTERS_PER_ACCOUNT: usize = 10;

#[derive(Clone, Copy, Default)]
pub(crate) struct Appearance {
    pub gender: u8,
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn create_character(
    ctx: &ReducerContext,
    account_id: u64,
    name: String,
    race: u8,
    class: u8,
    gender: u8,
    skin: u8,
    face: u8,
    hair_style: u8,
    hair_color: u8,
    facial_hair: u8,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let appearance = Appearance {
        gender,
        skin,
        face,
        hair_style,
        hair_color,
        facial_hair,
    };
    insert_new_character(ctx, account_id, name, race, class, appearance)
        .map(|_| ())
        .map_err(String::from)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CreateCharacterRefusal {
    NameInUse,
    InvalidRaceClass,
    AccountFull,
    Refused(crate::Refusal),
}

impl From<String> for CreateCharacterRefusal {
    fn from(cause: String) -> Self {
        Self::Refused(cause.into())
    }
}

// The create_character reducer and the Package API return this text.
impl std::fmt::Display for CreateCharacterRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NameInUse => "NAME_IN_USE".fmt(f),
            Self::InvalidRaceClass => "INVALID_RACE_CLASS".fmt(f),
            Self::AccountFull => "SERVER_LIMIT".fmt(f),
            Self::Refused(refusal) => refusal.fmt(f),
        }
    }
}

impl From<CreateCharacterRefusal> for String {
    fn from(refusal: CreateCharacterRefusal) -> Self {
        refusal.to_string()
    }
}

pub(crate) fn check_new_character(
    ctx: &ReducerContext,
    name: &str,
    race: u8,
    class: u8,
) -> Result<(), CreateCharacterRefusal> {
    require_guid_range(ctx)?;
    if ctx
        .db
        .game_character()
        .name()
        .find(name.to_string())
        .is_some()
    {
        return Err(CreateCharacterRefusal::NameInUse);
    }
    let cbi = ctx.db.game_char_base_info();
    if cbi.count() > 0
        && cbi
            .race_class()
            .find(packed_race_class_id(race, class) as u16)
            .is_none()
    {
        return Err(CreateCharacterRefusal::InvalidRaceClass);
    }
    Ok(())
}

pub(crate) fn account_has_room(ctx: &ReducerContext, account_id: u64) -> bool {
    ctx.db
        .game_character()
        .by_account()
        .filter(&account_id)
        .count()
        < CHARACTERS_PER_ACCOUNT
}

pub(crate) fn insert_new_character(
    ctx: &ReducerContext,
    account_id: u64,
    name: String,
    race: u8,
    class: u8,
    appearance: Appearance,
) -> Result<u64, CreateCharacterRefusal> {
    check_new_character(ctx, &name, race, class)?;
    if !account_has_room(ctx, account_id) {
        return Err(CreateCharacterRefusal::AccountFull);
    }
    let chars = ctx.db.game_character();
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or_else(|| format!("no such account id: {account_id}"))?;

    let positions = ctx.db.game_start_position();
    let race_class = packed_race_class_id(race, class) as u16;
    let human_warrior = packed_race_class_id(1, 1) as u16;
    let sp = positions
        .race_class()
        .find(race_class)
        .or_else(|| positions.race_class().find(human_warrior))
        .ok_or_else(|| "no start position seeded".to_string())?;

    let next_guid = next_character_guid(ctx);

    let row = Character {
        guid: next_guid,
        account_id,
        owner_identity: account.identity.unwrap_or(Identity::ZERO),
        name: name.clone(),
        race,
        class,
        gender: appearance.gender,
        skin: appearance.skin,
        face: appearance.face,
        hair_style: appearance.hair_style,
        hair_color: appearance.hair_color,
        facial_hair: appearance.facial_hair,
        level: 1,
        xp: 0,
        next_level_xp: crate::xp::xp_to_next_level(1),
        map_id: sp.map_id,
        zone_id: sp.zone_id,
        x: sp.x,
        y: sp.y,
        z: sp.z,
        orientation: sp.orientation,
        first_login: true,
        online: false,
        money: 0,
        rested_xp: 0,
        last_logout_micros: 0,
        home_map: sp.map_id,
        home_zone: sp.zone_id,
        home_x: sp.x,
        home_y: sp.y,
        home_z: sp.z,
        played_total_secs: 0,
        session_start_micros: 0,
        health: 0,
        power: 0,
        respec_count: 0,
        death_expire_micros: 0,
        pending_instance_id: 0,
        gm_level: 0,
        pending_ghost: false,
        resting: false,
        rested_since_micros: 0,
        pending_godmode: false,
        pending_run_speed_mult_bp: crate::world::RUN_SPEED_BP_1X,
        bank_bag_slots: 0,
        watched_faction_index: -1,
    };

    if chars.try_insert(row).is_err() {
        return if chars.name().find(&name).is_some() {
            Err(CreateCharacterRefusal::NameInUse)
        } else {
            Err("character create failed".to_string().into())
        };
    }
    let owner = account.identity.unwrap_or(Identity::ZERO);
    crate::spell::spellbook::grant_createinfo_spells(ctx, next_guid, owner, race, class);
    crate::action_bar::grant_createinfo_actions(ctx, next_guid, owner, race, class);

    crate::items::grant_starter_item(ctx, next_guid, owner)?;
    Ok(next_guid)
}

#[reducer]
pub fn delete_character(
    ctx: &ReducerContext,
    account_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    apply_delete_character(ctx, account_id, character_guid)
}

pub(crate) fn apply_delete_character(
    ctx: &ReducerContext,
    account_id: u64,
    character_guid: u64,
) -> Result<(), String> {
    let c = ctx
        .db
        .game_character()
        .guid()
        .find(character_guid)
        .ok_or_else(|| "NO_SUCH_CHAR".to_string())?;
    if c.account_id != account_id {
        return Err("NOT_OWNER".to_string());
    }
    if crate::helpers::character_by_guid(ctx, character_guid).is_none() {
        return Err("CHAR_IN_TRANSIT".to_string());
    }
    if crate::auction::character_has_auction_value(ctx, character_guid) {
        return Err("CHAR_HAS_AUCTION_VALUE".to_string());
    }
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .is_some()
    {
        return Err("CHAR_IN_WORLD".into());
    }
    crate::mail_text::drop_character_letters(ctx, character_guid);
    crate::world::cascade_delete_character(ctx, character_guid);
    Ok(())
}
