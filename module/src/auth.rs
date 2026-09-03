//! Account / auth tables (private — never delivered to clients) and the two gateway-coordination
//! reducers that provision SRP6 credentials and establish a session. [session]

use spacetimedb::{reducer, table, Identity, ReducerContext, Table, TimeDuration, Timestamp};

use crate::items::game_item_instance; // monotonic-guid: don't reuse a guid that still owns items
use crate::{game_char_base_info, game_character, game_start_position, Character};

// ===========================================================================================
//  Account / auth tables [session] — PRIVATE
// ===========================================================================================

/// SRP6 verifier material — never the password. Private. [session]
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
    /// Account-owned Alpha Test Tools authority. Existing rows enroll on the additive migration;
    /// new rows copy [`AlphaTestToolsEnrollment::enabled`].
    #[default(true)]
    pub alpha_test_tools: bool,
}

/// Whether newly provisioned Accounts receive Alpha Test Tools. Private and authoritative on
/// Realm-core. A missing row means enabled so an additive publish behaves like a fresh database.
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

/// Ensure a SHADOW account row exists for `account_id` on THIS database.
///
/// Accounts are realm-scoped: SRP6 lives on the logon tier and writes `game_account` on the realm
/// (default) database only. An instance/continent shard therefore has no account row at all — and
/// `gw::gw_player_login` resolves the account by id, so a character that arrives via
/// `transfer::import_character_blob` could not log in.
///
/// This row is an INDEX ENTRY, not a credential: `salt`/`verifier` are empty, so it can never
/// satisfy an SRP proof even if the logon tier were ever pointed at this database. `identity` is
/// left unbound; the gateway calls `establish_session` here at every world entry, which is the only
/// field the row exists to carry.
///
/// Deliberate simplification: the username is synthetic (`#<id>`, which no real account name can
/// collide with — account names are uppercased alphanumerics) purely to satisfy the `#[unique]`
/// constraint. Upgrade path: realm-core owns accounts for every shard and this function is
/// deleted.
/// Idempotent — a second call is a no-op, and it NEVER overwrites a real account row. The explicit
/// id does not advance the table's `auto_inc` sequence (danger-zones §2), which is safe only
/// because nothing ever calls `provision_account` on a non-realm shard — if that ever changes, this
/// is where the collision comes from.
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

/// Gateway coordination: shared session key K. Private. [session]
#[table(accessor = game_session)]
pub struct Session {
    #[primary_key]
    pub account_id: u64,
    pub session_key: Vec<u8>, // 40 bytes
    pub identity: Option<Identity>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
}

/// The single trusted OPERATOR identity — the gateway's coordinator connection + the deploy CLI, which
/// both authenticate with the owner token (verified: that identity is NOT `ctx.database_identity()`, the
/// module's own identity, so we capture it rather than derive it). `require_operator` gates the
/// privileged owner-fired reducers against this, defending them from a direct anonymous SpacetimeDB
/// connection that bypasses the gateway. Set once via `claim_operator` (TOFU). Private. [session]
#[table(accessor = game_operator)]
pub struct Operator {
    #[primary_key]
    pub id: u8, // singleton: always 0
    pub identity: Identity,
}

#[derive(Debug, Eq, PartialEq)]
enum OperatorClaimAction {
    Insert,
    AlreadyOwned,
}

/// Decide whether a claim inserts the singleton, succeeds as an idempotent retry, or must refuse a
/// different identity. Kept context-free so all three authorization outcomes are unit-testable.
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

/// Trust-on-first-use claim of the operator identity: the deploy admin calls this right after the
/// first publish over the owner token, so `ctx.sender()` is the coordinator/CLI identity. A retry by
/// that same identity is an idempotent success; a different identity is refused. Persists across
/// additive republishes; only a `-c` reprovision clears it (re-claim then). NOT gated — it's the
/// bootstrap; it's safe because it can only ever be set when unclaimed or confirmed by the owner.
#[reducer]
pub fn claim_operator(ctx: &ReducerContext) -> Result<(), String> {
    let ops = ctx.db.game_operator();
    // `find(0)`, not `find(&0)` — main's clippy sweep dropped the needless borrow.
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

    #[test]
    fn reducer_routes_existing_and_new_claims_through_the_tested_decision() {
        let got = crate::test_scan::shape_of(
            include_str!("auth.rs"),
            "pub fn claim_operator(ctx: &ReducerContext) -> Result<(), String> {",
        );
        let want = "{
            let ops = ctx.db.game_operator();
            let claimed_identity = ops.id().find(0).map(|operator| operator.identity);
            if operator_claim_action(claimed_identity, ctx.sender())? == OperatorClaimAction::Insert {
                ops.insert(Operator { id: 0, identity: ctx.sender(), });
            }
            Ok(())
        }"
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
        assert_eq!(
            got, want,
            "`claim_operator` must route every claim through the tested decision and insert only \
             on its Insert outcome"
        );
    }
}

// ===========================================================================================
//  Session establishment (gateway coordination)
// ===========================================================================================

/// Provision (or update) an account's SRP6 credentials. Called by the trusted gateway, which
/// computes the salt/verifier with `wow_srp`. Privileged in production (restrict to the
/// gateway identity); permissive here for the slice.
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
        // Existing account → rotate its credentials (e.g. give the seeded TEST a real password).
        Some(mut acc) => {
            acc.salt = salt;
            acc.verifier = verifier;
            accounts.id().update(acc);
        }
        // New account → create it (e.g. TEST2 through `gateway provision TEST2 --password-stdin`
        // for two-client tests). No bound identity yet (a login binds it via establish_session);
        // not banned. Upsert so provisioning a new username works.
        None => {
            accounts.insert(Account {
                id: 0, // auto_inc
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

/// Set whether genuinely new Accounts receive Alpha Test Tools. Existing Accounts are unchanged.
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

/// Grant Alpha Test Tools to one Account by realm-wide, uppercased Account name.
#[reducer]
pub fn grant_alpha_test_tools(ctx: &ReducerContext, account_name: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    set_account_alpha_test_tools(ctx, &account_name, true)
}

/// Revoke Alpha Test Tools from one Account by realm-wide, uppercased Account name.
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

    #[test]
    fn operator_controls_are_gated_before_their_state_change() {
        let src = include_str!("auth.rs");
        for reducer in [
            "pub fn set_alpha_test_tools_enrollment(",
            "pub fn grant_alpha_test_tools(",
            "pub fn revoke_alpha_test_tools(",
        ] {
            let body = crate::test_scan::code_of(src, reducer);
            assert!(
                body.contains("crate::helpers::require_operator(ctx)?;"),
                "{reducer} must refuse a caller who is not the Operator"
            );
        }
    }

    #[test]
    fn account_authority_is_an_end_appended_additive_default() {
        let src = include_str!("auth.rs");
        let account = &src[src.find("pub struct Account {").expect("Account row")..];
        let account = &account[..account.find("\n}").expect("Account row end")];
        assert!(
            account.ends_with("#[default(true)]\n    pub alpha_test_tools: bool,"),
            "existing Account rows must receive Alpha Test Tools during an additive publish"
        );
    }
}

/// Logon writes the shared session key K and binds the per-account identity.
/// In production, restrict the caller to the gateway identity.
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

    // No per-character owner_identity restamp here: it existed so per-owner RLS would admit the
    // player's own connection, which no longer exists — `apply_player_login` stamps owner rows
    // via `restamp_owned_data`, and the gateway predicates are the visibility layer.

    let now = ctx.timestamp;
    let expires = now
        .checked_add(TimeDuration::from_micros(3_600_000_000))
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
    Ok(())
}

// ===========================================================================================
//  Realm-safe guid allocation
// ===========================================================================================

/// Durable high-water mark for `game_character.guid`. Singleton row (`id` is always 0). Private —
/// nothing outside this crate reads it.
///
/// Why this exists: the old allocator was a per-transaction scan of THIS database's own
/// `game_character`/`game_item_instance` rows, which only ever sees what's still HERE. The
/// escrowed transfer's `finish_transfer` deletes the departed character's row on the source
/// database (and, via the `character_owned!` sweep, its item rows too) — so the instant a
/// character transfers away, its guid looked free again on the database it left, and the next
/// `create_character` there could re-issue it while the transferred character is still alive on
/// the destination. Two characters, one guid, two databases.
///
/// This mark is monotonic: `next_character_guid` bumps it by one on every local create, and
/// `crate::transfer::apply_import_blob` bumps it (via `bump_guid_high_water`) to at least the
/// guid of every character this database RECEIVES from another one. Neither path ever lowers it,
/// so a guid this database has ever handed out — locally or by import — can never be handed out
/// here again, even after the character that held it is long deleted.
///
/// `create_character` is unrouted (§ premise check): the gateway always calls it
/// against the account's home/default database, never a routed shard, so a per-database mark is
/// sufficient — no cross-database plumbing on the creation path itself.
#[table(accessor = game_guid_allocator)]
pub struct GuidAllocator {
    #[primary_key]
    pub id: u32, // singleton: always 0 (matches game_config's convention, config.rs:50)
    pub high_water: u64,
}

/// Never let the high-water mark move backwards. This one line IS the whole guarantee: as long as
/// every write to `GuidAllocator.high_water` goes through it, a deleted character's guid can never
/// come back into circulation. Pure — no `ReducerContext` — so it's unit-testable directly (the
/// module crate has no ctx test harness by design; playbook §7).
fn ratchet_high_water(current: u64, floor: u64) -> u64 {
    current.max(floor)
}

/// The ORIGINAL per-database scan (the precursor — see `create_character`'s old comment),
/// generalised over guid iterators so it needs no `ReducerContext` either. Used ONLY to seed a
/// `game_guid_allocator` row that has never been touched — an auto-migrated database that already
/// has characters seeds from their real max instead of resetting to 0 and re-issuing them.
fn legacy_guid_seed(
    char_guids: impl Iterator<Item = u64>,
    owned_guids: impl Iterator<Item = u64>,
) -> u64 {
    char_guids
        .max()
        .unwrap_or(0)
        .max(owned_guids.max().unwrap_or(0))
}

/// The whole decision behind `next_character_guid`, pure: advance the seed by exactly one — no
/// ratchet needed here, since the seed IS the current mark (or the legacy scan's max), so "advance"
/// can never be a floor call in disguise. Mutation target: this is where a `next_character_guid`
/// that stops allocating (e.g. swapping `seed + 1` for `seed`) would live if it weren't pulled out
/// here — pulling it out is what lets a ctx-free test catch that mutation at all (playbook §7).
fn allocate_next_guid(seed: u64) -> u64 {
    seed + 1
}

/// Read the singleton row, whole. `None` means "never written" — every caller here treats that as
/// "run the legacy seed", never as "the mark is 0". Callers that need just the mark derive it with
/// `.map(|r| r.high_water)` rather than this function returning a second, redundant `Option<u64>`.
fn read_high_water(ctx: &ReducerContext) -> Option<GuidAllocator> {
    ctx.db.game_guid_allocator().id().find(0)
}

/// The ONLY raw `game_character`/`game_item_instance` scan site in this file (both
/// `next_character_guid` and `bump_guid_high_water` below route through it via
/// `Option::unwrap_or_else`, so it only actually runs on a mark's first-ever touch) — deliberately
/// factored out so it stays ONE occurrence rather than two, which is what
/// `character_fence_tripwire`'s per-file raw-lookup budget (module/src/tripwires.rs's auth.rs
/// WHITELIST entry) counts.
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

/// Allocate the next character guid, durably. Replaces `create_character`'s old inline scan. Thin
/// on purpose: the only decision here — [`allocate_next_guid`] — is pure and unit-tested; this
/// wrapper is just the read/compute/write around it. Pinned by [`guid_allocator_tests::next_character_guid_routes_through_allocate_next_guid`]
/// (ctx glue can't be unit-tested directly — playbook §7).
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

/// Ratchet the mark up to at least `guid`. Two call sites: `transfer::apply_import_blob`
/// right after it materialises an imported character (AC#3 — so THIS database can never later
/// re-issue a guid it just received from another one), and `world::cascade_delete_character` right
/// before it removes a `game_character` row on ANY delete path (defect 1 — so a database that has
/// never yet seeded its allocator can't have a guid freed out from under a scan that would otherwise
/// miss it). A no-op once the mark is already seeded and ahead. Pinned by
/// [`guid_allocator_tests::bump_guid_high_water_routes_through_ratchet_high_water`].
pub(crate) fn bump_guid_high_water(ctx: &ReducerContext, guid: u64) {
    let existing = read_high_water(ctx);
    let seed = existing
        .as_ref()
        .map(|r| r.high_water)
        .unwrap_or_else(|| legacy_guid_seed_now(ctx));
    let mark = ratchet_high_water(seed, guid);
    if mark == seed && existing.is_some() {
        return; // already seeded, already ahead — avoid a no-op write
    }
    write_high_water(ctx, existing, mark);
}

/// **The guid range THIS database was assigned, and its licence to mint.**
///
/// `game_character.guid` is a per-database `#[auto_inc]`-style mark, so two databases that both
/// MINT characters mint colliding guids — and `transfer_id` **is** the character guid, so a
/// collision means two characters sharing an escrow ledger row. It was demonstrated live: guid 59
/// was one character on `lyracore` and a different one on `lyracore-world-1`, and routing
/// sent the second player to the first's shard. The receiving half already exists
/// (`apply_import_blob` and `cascade_delete_character` ratchet the mark), so a shard can never
/// re-issue a guid it was *given*; this table is the other half: a shard that mints its own starts
/// from a floor nobody else will reach.
///
/// Convention (`docs/design/`): shard *n* gets floor `n * 1_000_000_000`. Decimal and small on
/// purpose — it stays readable in logs and SQL, leaves a billion characters per shard, and stays far
/// below both `2^53` (above which `spacetime call` mangles u64 arguments — danger-zones) and the
/// high-bit type markers real entity guids carry.
///
/// the first cut (since deleted) gave each database its floor by hand through a
/// standalone reducer that only ratcheted `game_guid_allocator.high_water` — a collision became
/// *impossible*, but the guarantee rested on an operator remembering to run it on every new shard;
/// a shard provisioned without it minted from zero into `lyracore`'s range, silently at creation,
/// and surfaced later as a mis-routed login or two characters sharing an escrow row (`transfer_id`
/// IS the character guid).
///
/// So the floor is no longer something a database can lack: the presence of this row is what
/// permits [`next_character_guid`] to run at all, and only [`install_guid_range`] writes it, from a
/// range realm-core assigned. A shard that never claimed one refuses to create characters — loudly,
/// at the point of creation — rather than minting guids that belong to someone else.
///
/// A range is never *reassigned*: guids already minted from it are referenced by rows that outlive
/// the character (escrow ledgers, party rosters, the shard index, mail), so moving a shard's range
/// would strand them.
#[table(accessor = game_guid_range)]
pub struct GuidRange {
    #[primary_key]
    pub id: u32, // singleton: always 0 (game_config's convention)
    pub base: u64,
    pub size: u64,
}

/// May this database mint a character guid? Pure — the whole rule, so it is testable without a
/// `ReducerContext` and so the refusal message has one source.
pub(crate) fn may_mint(range: Option<(u64, u64)>, mark: u64) -> Result<(), String> {
    let Some((base, size)) = range else {
        return Err("NO_GUID_RANGE".to_string());
    };
    if mark >= base.saturating_add(size) {
        return Err("GUID_RANGE_EXHAUSTED".to_string());
    }
    Ok(())
}

/// [`may_mint`]'s ctx glue: read this database's range + mark and apply the rule. Called by
/// `create_character` BEFORE anything is written, so a refusal leaves nothing behind.
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

/// Is `guid` inside `range`? Pure, same reason as [`may_mint`] — testable without a
/// `ReducerContext`. `transfer::apply_import_blob` calls this to decide whether an
/// ARRIVING character's guid may ratchet THIS database's `game_guid_allocator` — ranges are
/// disjoint by construction, so a guid outside this
/// database's own range belongs to another shard and can never collide with anything this shard
/// mints; ratcheting past it anyway is pure self-harm (it walks this shard's own mark toward, or
/// past, its own range end for a guid it will never be asked to re-mint — the live
/// incident).
///
/// `None` (no range installed yet) is conservatively `false`, i.e. "not inside" — never a bump.
/// Same reasoning as `may_mint`'s `NO_GUID_RANGE`: a database with no range cannot mint locally
/// either, so there is nothing local yet for a foreign arrival to threaten, and treating an
/// unranged guid as "inside" would let an import inflate the mark before a range even exists to
/// check it against (poisoning the eventual `install_guid_range` — see its own "already minted
/// up to N" guard).
pub(crate) fn in_guid_range(range: Option<(u64, u64)>, guid: u64) -> bool {
    let Some((base, size)) = range else {
        return false;
    };
    guid >= base && guid < base.saturating_add(size)
}

/// Install the range realm-core assigned to this database. Idempotent — re-installing the
/// same range is how the gateway confirms one is held — and it never *moves* a range: a different
/// base is an error, not an overwrite.
///
/// Ratchets the allocator up to `base` through the same `bump_guid_high_water` every other caller
/// uses, so installing can only ever move the mark UP. Refuses a range this database has already
/// minted past, which is the loud failure that replaces the silent one: it means the range being
/// assigned is not the range in use.
#[reducer]
pub fn install_guid_range(ctx: &ReducerContext, base: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    // One owner for the size (`realm_core::GUID_RANGE_SIZE`, which is also what carves the slots),
    // so the gateway never carries a second copy of it that could drift from the registry's.
    let size = crate::realm_core::GUID_RANGE_SIZE;
    let ranges = ctx.db.game_guid_range();
    if let Some(row) = ranges.id().find(0) {
        if row.base == base && row.size == size {
            return Ok(()); // already held — the gateway's confirm path
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

    /// Two shards that both mint characters must never mint the same guid, and the
    /// mechanism is a per-shard floor. The property that makes a floor SAFE to apply to a live
    /// database — and safe to re-apply — is that it can only ratchet UP: a floor below the current
    /// mark must not rewind the allocator onto guids that are already issued. `install_guid_range`
    /// (below) is the only caller left that applies one (deleted the standalone
    /// `set_guid_floor` reducer — dead once every shard's floor got a licensed, non-optional
    /// path instead), and it reaches the mark through this same function, so this pin still covers
    /// it. Mutation target: swapping `ratchet_high_water(seed, floor)` for a plain assignment inside
    /// `bump_guid_high_water` makes the second assertion re-issue guid 40.
    #[test]
    fn a_guid_floor_only_ever_ratchets_upwards() {
        // A fresh shard given a floor: the mark jumps to it, so the next guid is above it.
        assert_eq!(ratchet_high_water(0, 1_000_000_000), 1_000_000_000);
        // A shard that has ALREADY issued guids past the floor keeps its mark — re-running the
        // provisioning step, or applying a floor to the default shard by mistake, must not hand out
        // a guid that is already alive.
        assert_eq!(
            ratchet_high_water(2_000_000_050, 1_000_000_000),
            2_000_000_050,
            "a floor below the current mark rewound the allocator onto live guids"
        );
    }

    /// The floor above is monotonic but *optional* — a shard nobody ran it on mints
    /// from zero. This is the rule that makes it non-optional, and the two refusals are the whole
    /// point: no range at all, and a mark that has escaped the range it was assigned.
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
        // A licensed shard mints anywhere inside its range, including at the very base.
        assert!(may_mint(Some((1_000_000_000, 1_000_000_000)), 1_000_000_000).is_ok());
        assert!(may_mint(Some((1_000_000_000, 1_000_000_000)), 1_999_999_998).is_ok());
        // …and stops at the boundary rather than walking into the next shard's guids.
        assert_eq!(
            may_mint(Some((1_000_000_000, 1_000_000_000)), 2_000_000_000).unwrap_err(),
            "GUID_RANGE_EXHAUSTED"
        );
    }

    /// `in_guid_range` is the pure decision `transfer::apply_import_blob` calls to
    /// decide whether an ARRIVING character's guid may ratchet this database's allocator.
    /// Boundary-checked the same way as `may_mint` just above (base included, end excluded), and,
    /// like it, conservative when there is no range installed at all.
    #[test]
    fn in_guid_range_only_admits_guids_inside_the_installed_range() {
        use super::in_guid_range;
        // No range installed: nothing is "inside" it — a database that cannot mint locally
        // either has nothing local yet for a foreign arrival to threaten.
        assert!(!in_guid_range(None, 0));
        assert!(!in_guid_range(None, 4242));
        // Inside a real range, base included, end excluded — same boundary shape as `may_mint`.
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
        // Anything below base, or from another shard's slot entirely, is foreign.
        assert!(!in_guid_range(range, 0));
        assert!(!in_guid_range(range, 3_000_000_000));
    }

    /// …and the WIRING (playbook §8 — the repo's dominant test failure is pinning the helper and
    /// not the call site). `create_character` must consult the rule BEFORE it writes anything;
    /// deleting that one line brings back exactly the silent minting this rule exists to end, with every
    /// other test in this file still green.
    #[test]
    fn create_character_refuses_before_it_writes_anything() {
        // The trailing newline matters: this test module sits ABOVE the reducer in the file, so a
        // bare `"pub fn create_character("` needle finds THIS line's own string literal first and
        // scans the test instead of the reducer. The real signature breaks its line after the `(`.
        let body = crate::test_scan::code_of(include_str!("auth.rs"), "pub fn create_character(\n");
        assert!(
            body.contains("require_guid_range(ctx)?"),
            "`create_character` no longer checks this database's guid range, so a shard that was \
             never assigned one mints from zero into another shard's guids — silently. Body \
             was:\n{body}"
        );
        let check_at = body
            .find("require_guid_range(ctx)?")
            .expect("checked above");
        let mint_at = body.find("next_character_guid(ctx)").unwrap_or(usize::MAX);
        assert!(
            check_at < mint_at,
            "the range check must come BEFORE the guid is minted, not after"
        );
    }

    /// Mutation target: flip `.max` to `.min` (or drop the `floor` arm entirely) and this goes red.
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

    /// Mutation target: swap the `.max` between the two iterators, or drop either arm, and this
    /// goes red — it's the exact scan `create_character` used to run inline.
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

    /// Mutation target: reproduces the reviewer's exact find — `next_character_guid` used to inline
    /// `ratchet_high_water(current, current + 1)` with no ctx-free seam to catch a mutation that
    /// swapped it for plain `current` (never advances). That decision now lives in
    /// `allocate_next_guid`, so this test alone catches it.
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

    /// Defect 1, COMPOSED end-to-end through the pure functions (no ctx, so this can run
    /// as a plain unit test): a database with characters up to 49 has JUST cascade-deleted guid 50
    /// (a departed transfer) — `legacy_guid_seed` therefore only sees the survivors, exactly the
    /// scan `create_character` used to run inline. Flooring the mark at 50 (what
    /// `cascade_delete_character`'s new `bump_guid_high_water` call does) must make the NEXT
    /// allocation land on 51, never re-issue the departed 50. This is the outcome the individual
    /// `ratchet_high_water`/`allocate_next_guid` tests above don't quite prove on their own — this
    /// one composes them the way production actually does, guid-for-guid.
    #[test]
    fn defect_1_transfer_then_create_no_longer_reissues_the_departed_guid() {
        let chars_after_transfer = [10u64, 30, 49]; // guid 50 already gone when this runs
        let seed = legacy_guid_seed(chars_after_transfer.into_iter(), std::iter::empty());
        let allocator_mark_after_delete = ratchet_high_water(seed, 50);
        assert_eq!(allocator_mark_after_delete, 50);
        let reissued_guid = allocate_next_guid(allocator_mark_after_delete);
        assert_ne!(reissued_guid, 50);
        assert_eq!(reissued_guid, 51);
    }

    /// `body_of`/`code_of`/`shape_of` are the shared scan primitives in [`crate::test_scan`]
    /// (this used to be six near-identical copies, drifted apart).
    use crate::test_scan::shape_of;

    /// Collapse whitespace the same way [`shape_of`] does, for a hand-written "what it SHOULD be"
    /// literal — so the `want` side of an equality assertion never has to be hand-space-counted
    /// (a likely source of a flaky/wrong pin) and can just be written as normal-looking Rust.
    fn norm(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// `next_character_guid` cannot itself be unit-tested (it takes `&ReducerContext`, and this
    /// crate has no ctx harness by design — playbook §7), so its glue is pinned by EXACT SHAPE
    /// instead of `.contains()` — the round-1 review reproduced FIVE ways to keep a `.contains()`
    /// needle present while gutting the function (dead comment, shadowed variable, wrong return
    /// value...); equality against the whole body closes all of them, because every one of those
    /// mutations changes the body's TEXT, not just whether a substring survives in it.
    #[test]
    fn next_character_guid_is_exactly_the_pinned_shape() {
        let got = shape_of(
            include_str!("auth.rs"),
            "pub(crate) fn next_character_guid(",
        );
        let want = norm(
            "{
                let existing = read_high_water(ctx);
                let seed = existing
                    .as_ref()
                    .map(|r| r.high_water)
                    .unwrap_or_else(|| legacy_guid_seed_now(ctx));
                let next = allocate_next_guid(seed);
                write_high_water(ctx, existing, next);
                next
            }",
        );
        assert_eq!(
            got, want,
            "next_character_guid's body no longer matches the pinned shape — it must seed from \
             the durable mark first, route through the tested `allocate_next_guid`, persist the \
             result, and return the NEW mark (not the seed). Got:\n{got}"
        );
    }

    /// Same reasoning as above, for the other entry point: `bump_guid_high_water` must actually
    /// ratchet at `guid` and persist the result, or the defect 1/AC#3 fix (the two call
    /// sites in `world::cascade_delete_character` and `transfer::apply_import_blob`) does nothing.
    #[test]
    fn bump_guid_high_water_is_exactly_the_pinned_shape() {
        let got = shape_of(
            include_str!("auth.rs"),
            "pub(crate) fn bump_guid_high_water(",
        );
        let want = norm(
            "{
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
            }",
        );
        assert_eq!(
            got, want,
            "bump_guid_high_water's body no longer matches the pinned shape — it must seed from \
             the durable mark first, floor it at `guid` via the tested `ratchet_high_water`, and \
             persist the result (skipping the write ONLY on the already-seeded, already-ahead \
             no-op). Got:\n{got}"
        );
    }
}

// ===========================================================================================
//  Character creation (CMSG_CHAR_CREATE)
// ===========================================================================================

/// Create a new character for `account_id` (called by the trusted gateway on `CMSG_CHAR_CREATE`,
/// like `provision_account`/`establish_session`; restrict to the gateway identity in production).
/// Returns a distinguished `Err("NAME_IN_USE")` for a taken name (the gateway maps it to
/// `CharCreateNameInUse`); any other `Err` maps to a generic failure. The new character spawns at
/// the seeded start position for its (race, class), falling back to Human Warrior's if that combo
/// has none seeded (we have no DBC; the slice only seeds Human Warrior).
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
    // Minting is licensed by a range realm-core assigned, and an unlicensed database refuses
    // rather than minting from zero into another shard's guids. FIRST — before any write, and
    // before the cheaper validations, so the refusal is unambiguous in the log.
    require_guid_range(ctx)?;
    let chars = ctx.db.game_character();
    // `name` is unique; check first so we can return a clean, distinguished result.
    if chars.name().find(&name).is_some() {
        return Err("NAME_IN_USE".to_string());
    }
    // Per-realm character cap (vanilla: 10). The 5875 client can't render a char list with >10 entries
    // (it rejects SMSG_CHAR_ENUM → "Error retrieving character list"), so refuse the 11th here with a
    // distinguished error the gateway maps to CHAR_CREATE_SERVER_LIMIT ("cannot create any more").
    // Routed through the `by_account` index — this used to be a full-table scan.
    if chars.by_account().filter(&account_id).count() >= 10 {
        return Err("SERVER_LIMIT".to_string());
    }
    // Reject an illegal (race, class) (e.g. Human Shaman) — server-side defense-in-depth from the
    // imported CharBaseInfo.dbc (the client already gates the UI). Skipped when the table is empty
    // (unloaded), so creation still works pre-import. [importer P1]
    let cbi = ctx.db.game_char_base_info();
    if cbi.count() > 0
        && cbi
            .race_class()
            .find(((race as u16) << 8) | class as u16)
            .is_none()
    {
        return Err("INVALID_RACE_CLASS".to_string());
    }
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or_else(|| format!("no such account id: {account_id}"))?;

    // Start position for this (race, class); fall back to Human(1)/Warrior(1) if unseeded.
    let positions = ctx.db.game_start_position();
    let race_class = ((race as u16) << 8) | class as u16;
    let human_warrior = (1u16 << 8) | 1u16;
    let sp = positions
        .race_class()
        .find(race_class)
        .or_else(|| positions.race_class().find(human_warrior))
        .ok_or_else(|| "no start position seeded".to_string())?;

    // Realm-safe GUID allocation: a durable high-water mark, not a per-transaction scan
    // of THIS database alone. The old scan (still the seed a never-touched mark bootstraps from —
    // see `legacy_guid_seed`) looked free the instant a cross-database transfer deleted the departed
    // character's row (and its items) here, and the next creation could re-issue that guid while the
    // character was still alive on the destination database. `next_character_guid` never re-derives
    // the mark from a scan once it's seeded, and never lets it move backwards.
    let next_guid = next_character_guid(ctx);

    let row = Character {
        guid: next_guid,
        account_id,
        // Bind to the account's identity (set at establish_session) so the per-owner RLS lets the
        // player see it immediately; ZERO if somehow unbound (gateway reads via the owner conn anyway).
        owner_identity: account.identity.unwrap_or(Identity::ZERO),
        name: name.clone(),
        race,
        class,
        gender,
        skin,
        face,
        hair_style,
        hair_color,
        facial_hair,
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
        money: 0,     // new characters start broke
        rested_xp: 0, // rank 30: no rested XP until the first logout/login cycle
        last_logout_micros: 0,
        // Hearthstone home = the start position — a fresh character recalls to where it began until it
        // binds at an innkeeper (`bind_home`).
        home_map: sp.map_id,
        home_zone: sp.zone_id,
        home_x: sp.x,
        home_y: sp.y,
        home_z: sp.z,
        played_total_secs: 0,    // fresh character, nothing played yet
        session_start_micros: 0, // no live session until player_login stamps it
        health: 0, // sentinel: no persisted value yet — build_player_entity spawns at full health
        power: 0, // sentinel: no persisted value yet — build_player_entity spawns at starting power
        respec_count: 0,
        death_expire_micros: 0, // never died // fresh character, never respec'd
        pending_instance_id: 0, // open world
        gm_level: 0,            // no GM access until granted via set_gm_level
        pending_ghost: false,   // fresh characters are alive (work-item 226)
        resting: false,         // 196: not in an inn until they walk into one
        rested_since_micros: 0, // 196: live-accrual clock idle
        pending_godmode: false, // 289: GM playtest carry — off until `.god` + a map change
        pending_run_speed_mult_bp: crate::world::RUN_SPEED_BP_1X, // 289: 1× until `.speed` + a map change
        bank_bag_slots: 0,                                        // none bought yet
    };

    // `try_insert` rather than `insert`: if a same-name character was created between the find()
    // above and here (the race the early check can't fully close), the `#[unique] name` constraint
    // makes this fail cleanly instead of panicking. `name` is game_character's only unique
    // constraint and `guid` is max+1, so a failure here means the name is now taken → NAME_IN_USE.
    if chars.try_insert(row).is_err() {
        return if chars.name().find(&name).is_some() {
            Err("NAME_IN_USE".to_string())
        } else {
            Err("character create failed".to_string())
        };
    }
    // Same transaction as the insert: the createinfo kit becomes the character's spellbook rows —
    // the one castability source (`knows_spell`) and the gateway's SMSG_INITIAL_SPELLS both read them.
    let owner = account.identity.unwrap_or(Identity::ZERO);
    crate::spell::spellbook::grant_createinfo_spells(ctx, next_guid, owner, race, class);
    // Action-bar rows (work-item 212): copies `game_createinfo_action` the same way the spell grant
    // above copies `game_createinfo_spell` — a no-op pre-import (table empty), so the gateway's login
    // synth covers it until the dump is imported.
    crate::action_bar::grant_createinfo_actions(ctx, next_guid, owner, race, class);
    // And the starter LOADOUT (outfit + hearthstone) — at creation, not first login, so the
    // char-select screen renders the gear (SMSG_CHAR_ENUM reads item rows; a never-logged-in
    // character used to have none and showed a naked model). Identity staleness is a non-issue
    // here: char-select reads ride the gateway's PRIVILEGED coordinator (RLS-bypassed), and any
    // ZERO/stale owner stamp is corrected by player_login's restamp_owned_data sweep BEFORE the
    // first player-scoped (RLS) read of these rows can happen.
    crate::items::grant_starter_item(ctx, next_guid, owner);
    Ok(())
}

/// Delete `character_guid` for `account_id` — the player `CMSG_CHAR_DELETE` path. Operator-gated (the
/// gateway coordinator calls it) AND ownership-checked: the character must belong to `account_id`, so
/// one account can't delete another's. Cascades via [`crate::world::cascade_delete_character`] (items,
/// learned spells/skills/talents, quest log, buyback, corpse, world entity) so no orphaned rows survive
/// for a reused guid to inherit (the Warrior-wore-Warlock-gear bug).
#[reducer]
pub fn delete_character(
    ctx: &ReducerContext,
    account_id: u64,
    character_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let c = ctx
        .db
        .game_character()
        .guid()
        .find(character_guid)
        .ok_or_else(|| "NO_SUCH_CHAR".to_string())?;
    if c.account_id != account_id {
        return Err("NOT_OWNER".to_string());
    }
    // IN-TRANSIT FENCE. `cascade_delete_character` runs the `character_owned!` delete
    // sweeps, which include `sweep_delete_game_transfer_out` — so a delete landing mid-transfer wipes
    // the character AND both escrow rows in one transaction, voiding a transfer another shard is
    // still driving. Cross-database that is the annihilation case: the destination's in-row and its
    // arrival copy survive the source's delete, and its reaper (no out-row ⇒ `Hold`) never settles
    // them. The escrow must be resolved (finished or reaped) before the character can be destroyed.
    //
    // Routed through the shared gate rather than a bespoke `is_in_transit` call, but the
    // DISTINGUISHED error is kept: the raw `find` above is what lets NO_SUCH_CHAR / NOT_OWNER /
    // CHAR_IN_TRANSIT stay three different answers, which `character_by_guid` alone would flatten to
    // NO_SUCH_CHAR. The gateway maps every non-OK delete the same way, so no seam either way.
    if crate::helpers::character_by_guid(ctx, character_guid).is_none() {
        return Err("CHAR_IN_TRANSIT".to_string());
    }
    if crate::auction::character_has_auction_value(ctx, character_guid) {
        return Err("CHAR_HAS_AUCTION_VALUE".to_string());
    }
    crate::world::cascade_delete_character(ctx, character_guid);
    Ok(())
}
