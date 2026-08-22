//! The engagement model (#382 split of the former monolithic `combat/mod.rs`, on top of #370's shared
//! damage pipeline): `enter_combat`/`disengage`/`clear_target`/`break_own_attacks` + the `is_engaged`/
//! `melee_combatant_guids` queries over `game_melee_attack` (the single source of truth for who fights
//! whom — see the banner below for the both-directions rule those two helpers encode), the engagement
//! tables (`MeleeAttack`/`CombatEvent`/`MeleeSchedule`/`RangedImpactSchedule`), and the
//! `start_attack`/`start_ranged_attack`/`stop_attack` reducers that arm/disarm them. The tick-driven
//! swing resolution (`tick_melee` and everything it calls) lives in `swing.rs`. `mod.rs` re-exports
//! this module (`pub use engage::*`) so every `crate::combat::<sym>` path resolves regardless of which
//! submodule actually defines it.

use spacetimedb::{table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{game_faction_template, game_world_entity, WorldEntity};

// Tables' pure formulas/consts and the sibling submodules' re-exports (`roll_swing`, `kill_creature`,
// ...) are all pulled in from `mod.rs` (`pub use tables::*` + `pub use folds::*`/`death::*`/`swing::*`)
// — this ALSO brings `tick_melee`/`ranged_impact` into scope for the `MeleeSchedule`/
// `RangedImpactSchedule` tables' `scheduled(..)` macros below to resolve, since those two reducers are
// defined in `swing.rs` (mirrors `spell::tables`'s identical cross-file `scheduled(..)` pattern).
use super::*;

// --- Engagement queries over `game_melee_attack` (the single source of truth for who fights whom).
// `attacker_guid` is the PK; an engagement "touches" a unit when it is on EITHER side. These three
// helpers are the ONE place the both-directions rule is encoded, so leash evade, killing blow,
// logout teardown, the patrol-hold gate, and regen all share it instead of re-deriving it inline.

/// How long (ms) a unit stays flagged IN COMBAT after its last hostile action — vanilla's ~5-6s
/// combat-drop. `enter_combat` stamps `now + this`; the tick's combat-drop pass clears the flag past it.
pub(crate) const COMBAT_DROP_MS: u64 = 6000;

/// Put `guid` into combat: stamp `combat_until_ms = now + COMBAT_DROP_MS` and set `UNIT_FLAG_IN_COMBAT`
/// on its `unit_flags`, so OBSERVERS see the combat state — including a pure caster, whom the
/// auto-attack / `game_melee_attack` stance (`SMSG_ATTACKSTART`) can't cover. Idempotent; skips
/// dead/missing. Re-stamps only when newly entering OR the deadline is within half the window, so a
/// sustained fight doesn't write + fire a relay callback every 500ms tick. The flag is CLEARED by the
/// tick's combat-drop pass (`creatures::tick`), not here — so it lingers ~COMBAT_DROP_MS after the last
/// action, like vanilla (NOT instantly on `disengage`). [entity]
pub(crate) fn enter_combat(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(guid) else {
        return;
    };
    if e.dead {
        return;
    }
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    let already = e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0;
    if already && e.combat_until_ms > now_ms + COMBAT_DROP_MS / 2 {
        return; // already comfortably in combat — skip the redundant write + relay callback
    }
    e.combat_until_ms = now_ms + COMBAT_DROP_MS;
    e.unit_flags |= lyracore_shared::constants::unit_flags::IN_COMBAT;
    entities.guid().update(e);
}

/// Arm a creature's first outgoing melee engagement and dispatch its aggro edge once.
pub(crate) fn arm_creature_engagement(
    ctx: &ReducerContext,
    creature_guid: u64,
    target_guid: u64,
    assist: bool,
) -> bool {
    let melee = ctx.db.game_melee_attack();
    if melee.attacker_guid().find(creature_guid).is_some() {
        return false;
    }
    melee.insert(MeleeAttack {
        attacker_guid: creature_guid,
        target_guid,
        last_swing_ms: 0,
        ranged_spell_id: 0,
        last_offhand_swing_ms: 0,
        rout_ends_ms: 0,
        pursuit_ends_ms: 0,
        leash_x: 0.0,
        leash_y: 0.0,
    });
    let entities = ctx.db.game_world_entity();
    if let Some(mut creature) = entities.guid().find(creature_guid) {
        if creature.target_guid != target_guid {
            creature.target_guid = target_guid;
            entities.guid().update(creature);
        }
    }
    crate::hooks::fire_on_aggro(
        ctx,
        &crate::hooks::AggroPayload {
            creature_guid,
            target_guid,
            assist,
        },
    );
    true
}

/// Refresh the pursuit leash on the CREATURE side of a damage exchange: deadline to
/// `now + PURSUIT_WINDOW_MS`, remembered position to wherever that creature stands. Fed both guids, so
/// the creature's own damage refreshes it like the player's.
///
/// Called from the damage chokepoint (`apply_hit`), never from `enter_combat`: the creature tick
/// re-stamps combat flags through that helper every 500ms, which would defer the deadline forever.
/// [entity]
pub(crate) fn refresh_leash(ctx: &ReducerContext, guid_a: u64, guid_b: u64) {
    let melee = ctx.db.game_melee_attack();
    let entities = ctx.db.game_world_entity();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    for guid in [guid_a, guid_b] {
        // Only a CREATURE's own outgoing row is ever read by the leash pass; a player's is not.
        let (Some(mut row), Some(e)) =
            (melee.attacker_guid().find(guid), entities.guid().find(guid))
        else {
            continue;
        };
        if e.is_player() {
            continue;
        }
        row.pursuit_ends_ms = crate::creatures::pursuit_deadline_ms(now_ms);
        row.leash_x = e.x;
        row.leash_y = e.y;
        melee.attacker_guid().update(row);
    }
}

/// Free every melee engagement touching `guid` — its own outgoing attack AND any attack on it.
/// The canonical "leave combat" teardown. Collect-then-delete (never mutate while iterating).
pub(crate) fn disengage(ctx: &ReducerContext, guid: u64) {
    let melee = ctx.db.game_melee_attack();
    // Capture the CREATURES attacking `guid` (their target_guid points at `guid`) BEFORE deleting their
    // rows, so we can also drop their stale selection below. Without this a creature whose melee row is
    // gone keeps facing/tracking the dead/ghost player (the row stops the swing, but target_guid lingers
    // and the client renders the mob still selecting the corpse). Generic over any attacker of any unit.
    // Perf catalog 1.15: `by_target` turns these three full scans into own-row probes.
    let attackers_of_guid: Vec<u64> = melee
        .by_target()
        .filter(&guid)
        .map(|a| a.attacker_guid)
        .collect();
    // `touching` = the outgoing row (keyed by PK) plus every incoming one. Deletes are by PK, so the
    // visit order is irrelevant; the SET is identical to the old scan's.
    for a in melee
        .attacker_guid()
        .find(guid)
        .map(|a| a.attacker_guid)
        .into_iter()
        .chain(attackers_of_guid.iter().copied())
    {
        melee.attacker_guid().delete(a);
    }
    // Threat is part of being in combat: leaving it clears `guid` from the threat tables — its OWN table
    // (a creature that evaded/fled/died forgets everyone) AND its entries on every other creature's table
    // (a player who died/logged out is forgotten by every mob). One symmetric clear, the canonical place.
    crate::threat::clear_for_unit(ctx, guid);
    clear_target(ctx, guid);
    // Drop the stale target on every creature that was attacking `guid`. `clear_target` skips players and
    // is a no-op when target_guid == 0, so this only touches creatures that actually pointed at the dying
    // unit — baseline-safe (a creature attacking someone else is untouched) and covers a whole pack.
    for a in &attackers_of_guid {
        clear_target(ctx, *a);
    }
    // 249: leaving combat is IMMEDIATE for anyone this disengage left with no engagement at all —
    // vanilla drops the player's combat the moment the mob evades, not ~COMBAT_DROP_MS later (the
    // decay window still covers DoT/spell-only combat via the cycle's combat exit, unchanged).
    let mut freed = attackers_of_guid;
    freed.push(guid);
    let entities = ctx.db.game_world_entity();
    for g in freed {
        if is_engaged(ctx, g) {
            continue;
        }
        // The fight is over for this unit however it ended — evade, the player dying, a logout, a
        // map change. This is the ONLY place that sees all four, so the EventAI engagement resets
        // here rather than in the cycle's combat-drop pass, which sees only a decayed flag.
        crate::creatures::reset_engagement(ctx, g);
        if let Some(mut e) = entities.guid().find(g) {
            if e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                e.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;
                e.combat_until_ms = 0;
                entities.guid().update(e);
            }
        }
    }
}

/// Remove only the two attack rows belonging to a Duel. Other combat remains intact; each
/// participant's combat flag clears only when no other engagement still touches them.
pub(crate) fn stop_duel_combat(ctx: &ReducerContext, first_guid: u64, second_guid: u64) {
    let melee = ctx.db.game_melee_attack();
    for attacker_guid in [first_guid, second_guid] {
        if melee
            .attacker_guid()
            .find(attacker_guid)
            .is_some_and(|attack| {
                (attacker_guid == first_guid && attack.target_guid == second_guid)
                    || (attacker_guid == second_guid && attack.target_guid == first_guid)
            })
        {
            melee.attacker_guid().delete(attacker_guid);
        }
    }
    let entities = ctx.db.game_world_entity();
    for guid in [first_guid, second_guid] {
        if is_engaged(ctx, guid) {
            continue;
        }
        if let Some(mut entity) = entities.guid().find(guid) {
            entity.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;
            entity.combat_until_ms = 0;
            entities.guid().update(entity);
        }
    }
}

/// Zero a CREATURE's `target_guid` when it leaves combat (evade / flee / death) so the client stops
/// showing a stale target on a disengaged mob. Skips PLAYERS: a player's `target_guid` is their
/// SELECTION (`CMSG_SET_SELECTION`), which persists out of combat. No-op when already 0 (common path).
pub(crate) fn clear_target(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(guid) {
        if !e.is_player() && e.target_guid != 0 {
            e.target_guid = 0;
            entities.guid().update(e);
        }
    }
}

/// Drop only `guid`'s OWN outgoing attack (and clear its threat), LEAVING any attack *on* it intact.
/// Used when a creature FLEES at low HP: it stops swinging and forgets its foes, but whoever is hitting
/// it stays engaged so they can run it down (vanilla "the mob flees, you chase the kill"). Contrast
/// [`disengage`], which frees BOTH directions — using that here deleted the player's row too, dropping
/// them out of combat one swing short of the kill (the "combat just stops at low HP" bug).
pub(crate) fn break_own_attacks(ctx: &ReducerContext, guid: u64) {
    // `attacker_guid` is the PK → at most one outgoing row for `guid`.
    ctx.db.game_melee_attack().attacker_guid().delete(guid);
    crate::threat::clear_for_unit(ctx, guid);
    clear_target(ctx, guid);
}

/// Is `guid` in any melee engagement, attacking or attacked?
pub(crate) fn is_engaged(ctx: &ReducerContext, guid: u64) -> bool {
    let melee = ctx.db.game_melee_attack();
    melee.attacker_guid().find(guid).is_some() || melee.by_target().filter(&guid).next().is_some()
}

fn may_harm_decision(same_unit: bool, active_duel: bool, friendly: bool) -> bool {
    same_unit || active_duel || !friendly
}

fn may_help_decision(same_unit: bool, active_duel: bool, hostile: bool) -> bool {
    same_unit || (!active_duel && !hostile)
}

/// Current harmful-target authorization. An active Duel overrides friendship only for its exact
/// pair; neutral and hostile targets retain the ordinary faction behavior.
pub(crate) fn may_harm(ctx: &ReducerContext, attacker: &WorldEntity, target: &WorldEntity) -> bool {
    may_harm_decision(
        attacker.guid == target.guid,
        crate::duel::active_opponents(ctx, attacker.guid, target.guid),
        crate::faction::is_friendly(ctx, attacker.faction_template, target.faction_template),
    )
}

/// Current helpful-target authorization. Active Duel opponents are hostile to one another even
/// when their ordinary faction relation is friendly.
pub(crate) fn may_help(ctx: &ReducerContext, helper: &WorldEntity, target: &WorldEntity) -> bool {
    may_help_decision(
        helper.guid == target.guid,
        crate::duel::active_opponents(ctx, helper.guid, target.guid),
        crate::faction::is_hostile(ctx, helper.faction_template, target.faction_template),
    )
}

/// Whether an enemy-only area selector should include `target`. Unlike direct attacks, ordinary
/// neutral units are not selected automatically; an active Duel opponent is the narrow exception.
pub(crate) fn is_hostile_target(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
) -> bool {
    attacker.guid != target.guid
        && (crate::duel::active_opponents(ctx, attacker.guid, target.guid)
            || crate::faction::is_hostile(ctx, attacker.faction_template, target.faction_template))
}

/// Every guid currently in combat, for bulk in-combat gates (e.g. regen).
///
/// Includes both sides of every live melee engagement AND any entity whose
/// `UNIT_FLAG_IN_COMBAT` bit is set (covers pure casters, warriors with Bloodrage,
/// and anything else `enter_combat()` flagged without a melee row).  The two sets
/// overlap in practice; callers use `.contains()` so duplicates are harmless.
/// The MELEE half of the combatant set — both sides of every live `game_melee_attack` row. The other
/// half is every entity carrying `UNIT_FLAG_IN_COMBAT` (pure casters, Bloodrage warriors — anything
/// `enter_combat()` flagged without a melee row); perf catalog 1.6 moved that half to the CALLER,
/// because the only caller (the cycle's regeneration candidate read) gets that half from the flag
/// bits the tick's active-cell sweep already harvested, instead of paying a second full table scan
/// for it. A future caller must harvest the flag half itself the same way.
pub(crate) fn melee_combatant_guids(ctx: &ReducerContext) -> Vec<u64> {
    ctx.db
        .game_melee_attack()
        .iter()
        .flat_map(|a| [a.attacker_guid, a.target_guid])
        .collect()
}

// ===========================================================================================
//  Combat tables [entity]/[event]
// ===========================================================================================

/// An active melee auto-attack: `attacker_guid` swings `target_guid`. Keyed by attacker (one
/// target at a time). [entity]
#[table(
    accessor = game_melee_attack,
    public,
    // Perf catalog 1.15: every "who is attacking X" question (disengage, kill_creature's
    // still_engaged, is_engaged, combatant_guids, heal threat) used to full-scan this table.
    index(accessor = by_target, btree(columns = [target_guid]))
)]
pub struct MeleeAttack {
    #[primary_key]
    pub attacker_guid: u64,
    pub target_guid: u64,
    pub last_swing_ms: u32, // 0 = never swung (the next tick swings immediately)
    /// 0 = melee auto-attack (the original behavior); 75 (Auto Shot) / 5019 (wand Shoot) = a RANGED
    /// auto-attack — the swing tick then uses the equipped ranged weapon (slot 17), its delay as the
    /// swing interval, a longer range, and a reduced attack table (no parry/block). One engagement per
    /// attacker, so a unit is either meleeing OR shooting. END-appended + `#[default(0)]` → adding it
    /// auto-migrates existing rows (no `-c` wipe). [entity]
    #[default(0)]
    pub ranged_spell_id: u32,
    /// Work-item 037 (dual wield): the OFF-HAND swing's own clock, independent of `last_swing_ms` — an
    /// off-hand weapon has its own `delay_ms`, so it swings on a different cadence than the main hand.
    /// 0 = never swung (the next eligible tick swings immediately), same sentinel as `last_swing_ms`.
    /// Only consulted for a MELEE engagement (`ranged_spell_id == 0`) with a live off-hand weapon
    /// (`equipped_offhand_weapon_damage`) — a ranged engagement or a bare/shielded off-hand never
    /// advances this field. END-appended + `#[default(0)]` → adding it auto-migrates existing rows (no
    /// `-c` wipe). [entity]
    #[default(0)]
    pub last_offhand_swing_ms: u32,
    /// The low-HP ROUT clock for this engagement: 0 = no rout has started; otherwise the wall-clock ms
    /// at which the rout window closes. A value in the past means the rout is over AND spent, which is
    /// what makes a rout once-per-engagement — the row dies on disengage, so the next fight rearms it
    /// with no cleanup path. Read through `ai::rout_window_open` / `ai::may_start_rout`.
    /// END-appended + `#[default(0)]` → adding it auto-migrates existing rows (no `-c` wipe). [entity]
    #[default(0)]
    pub rout_ends_ms: u32,
    /// The wall-clock ms past which the leash pass may evade this creature, re-stamped by every damage
    /// exchange in either direction (`refresh_leash`). 0 = never refreshed, which never evades — the
    /// leash pass seeds it instead, so a pull whose first shot has not landed cannot be evaded out from
    /// under. Read through `ai::should_evade`.
    /// END-appended + `#[default(0)]` → adding it auto-migrates existing rows (no `-c` wipe). [entity]
    #[default(0)]
    pub pursuit_ends_ms: u32,
    /// Where the CREATURE stood at that refresh — the point the TARGET's distance is measured from.
    /// Meaningless while `pursuit_ends_ms` is 0. 2D, like the wander/return-home math (z varies on
    /// slopes). END-appended + `#[default(0.0)]` → auto-migrates existing rows. [entity]
    #[default(0.0)]
    pub leash_x: f32,
    #[default(0.0)]
    pub leash_y: f32,
}

/// A logged melee swing to relay as `SMSG_ATTACKERSTATEUPDATE`. Broadcast (public, no RLS), like
/// `game_creature_move_event` — the gateway fans it to in-world clients. [event]
#[table(
    accessor = game_combat_event,
    public,
    // perf catalog 2.3: AOI-box scoping instead of a global `SELECT *`.
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct CombatEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub attacker_guid: u64,
    pub target_guid: u64,
    pub damage: u32,
    pub hit_info: u8, // 0 normal, 1 crit, 2 miss, 3 dodge, 4 parry, 5 glancing, 6 crushing, 7 block
    pub killing_blow: bool, // the swing that killed the target → gateway also sends SMSG_ATTACKSTOP
    pub created_at: Timestamp,
    /// The flat shield `block_value` this swing absorbed (HIT_BLOCK only; 0 otherwise). Drives the
    /// SMSG_ATTACKERSTATEUPDATE `blocked_amount` wire field so the client shows the "Block N" text.
    /// END-appended + `#[default(0)]` → adding it auto-migrates existing rows (no `-c` wipe). [event]
    #[default(0)]
    pub blocked_amount: u32,
    /// 0 for a melee swing; the ranged spell (75 Auto Shot / 5019 Shoot) for a ranged shot. The gateway
    /// sets the SMSG_ATTACKERSTATEUPDATE `spell_id` field from this so the shot is attributed to the
    /// ranged ability. END-appended + `#[default(0)]` → auto-migrates. [event]
    #[default(0)]
    pub ranged_spell_id: u32,
    /// The `display_id` of the ammo (arrow/bullet) this shot consumed — Auto Shot (75) only; 0 for melee,
    /// wand Shoot, and out-of-data shots. The gateway sets the SMSG_SPELL_GO AMMO flag + this display so
    /// the client renders the arrow projectile. END-appended + `#[default(0)]` → auto-migrates. [event]
    #[default(0)]
    pub ammo_display_id: u32,
    /// True when a queued on-next-swing spell (Heroic Strike/Cleave) FIRED on this landed swing (114):
    /// vanilla REPLACES the white hit — the whole swing is the spell (one yellow named line, carried by
    /// the SpellCastEvent the swing inserts). The gateway then SKIPS the SMSG_ATTACKERSTATEUPDATE for
    /// this row (killing_blow/ATTACKSTOP still honored); `damage` keeps the true total for QA readers.
    /// END-appended + `#[default(false)]` → auto-migrates. [event]
    #[default(false)]
    pub spell_swing: bool,
    /// Projectile travel time (ms) for a RANGED shot — dist / Spell.dbc speed (Auto Shot 40 yd/s,
    /// wand 20 yd/s). 0 = melee (instant). The shot's DAMAGE lands at fire + this: the module's
    /// `ranged_impact` schedule applies health/kill then, and the gateway delays the
    /// SMSG_SPELLNONMELEEDAMAGELOG by the same amount — so the number lands WITH the arrow, not at
    /// the muzzle (user bug: "damage lands earlier than the projectile"). The SMSG_SPELL_GO
    /// (arrow launch) still relays immediately. END-appended + `#[default(0)]` → auto-migrates. [event]
    #[default(0)]
    pub impact_delay_ms: u32,
    // --- AOI columns (perf catalog 2.3), END-appended + TYPED defaults (a bare `0` on a u64
    // encodes as 4 bytes and fails the publish). Stamped from the actor via `helpers::grid_of`;
    // (0,0,0,0) means "no live actor", which matches no box and is correctly never delivered.
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
}

impl CombatEvent {
    /// A baseline `CombatEvent` row for `attacker`/`target_guid`, stamped from the attacker's
    /// already-fetched [`WorldEntity`] — zero `game_world_entity` lookups. `id`=0,
    /// `created_at`=`ctx.timestamp`, the AOI address (`map_id`/`instance_id`/`grid_x`/`grid_y`) copied
    /// straight off `attacker`, every other field at its neutral zero/false. Replaces the field-literal
    /// plus the four-call `grid_of` copy-paste this used to require at every call site (perf catalog
    /// audit, 2026-08-06) — every current call site already has the attacker entity in hand (the swing
    /// tick fetches it once up front), so this is the only constructor `CombatEvent` needs; a guid-only
    /// `grid_of`-backed variant can be added the day a call site without the entity shows up. A call
    /// site overrides only the handful of fields that carry real signal via struct-update syntax.
    pub(crate) fn signal_at(
        ctx: &ReducerContext,
        attacker: &WorldEntity,
        target_guid: u64,
    ) -> Self {
        let (map_id, instance_id, grid_x, grid_y) = crate::helpers::entity_addr(attacker);
        Self {
            id: 0,
            attacker_guid: attacker.guid,
            target_guid,
            damage: 0,
            hit_info: 0,
            killing_blow: false,
            created_at: ctx.timestamp,
            blocked_amount: 0,
            ranged_spell_id: 0,
            ammo_display_id: 0,
            spell_swing: false,
            impact_delay_ms: 0,
            map_id,
            instance_id,
            grid_x,
            grid_y,
        }
    }
}

/// Drives the melee swing tick. [server]
#[table(accessor = game_melee_schedule, scheduled(tick_melee))]
pub struct MeleeSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

/// One in-flight RANGED projectile (097): scheduled at fire + travel time; `ranged_impact` then
/// applies the frozen post-mitigation damage (health/lethal/threat/rage/skill/combat-flag) so the
/// server-side hit lands when the client's arrow does. Damage is FROZEN at launch (rolled + folded
/// there — vanilla folds absorb at impact, but freezing keeps the delayed damage LOG equal to what
/// actually lands; the ≤1s divergence window is noise). Module-private (not gateway-subscribed).
#[table(accessor = game_ranged_impact_schedule, scheduled(ranged_impact))]
pub struct RangedImpactSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub attacker_guid: u64,
    pub target_guid: u64,
    /// Post-mitigation damage frozen at launch (0 = a fully-absorbed shot: nothing to apply).
    pub damage: u32,
}

// ===========================================================================================
//  Reducers
// ===========================================================================================

/// The TARGET-side half of the attack-command gate, shared verbatim by `apply_start_attack` and
/// `apply_start_ranged_attack` (#370 — it was a copy-paste, and a copy of a gate is a gate that
/// eventually only half-holds): the target must EXIST, be on the same map + instance, not be a
/// CORPSE, and not be FRIENDLY. Returns the resolved target row so the caller doesn't fetch it twice.
///
/// The error strings are LOAD-BEARING — the gateway substring-maps them onto the 1.12 client
/// responses (`ERR_ATTACK_TARGET_DEAD` → SMSG_ATTACKSWING_DEADTARGET, `ERR_ATTACK_FRIENDLY` →
/// SMSG_ATTACKSWING_CANT_ATTACK), which is how the client leaves combat stance. The CC and
/// attack-self checks stay with the callers: they run BEFORE this block, and the ranged path
/// additionally wedges its "a ranged weapon must be equipped" check between them, so the order in
/// which a doubly-invalid command reports its reason is preserved exactly.
fn validate_attack_target(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target_guid: u64,
) -> Result<WorldEntity, String> {
    let target =
        crate::helpers::live_entity(ctx, target_guid).map_err(|_| "no such target".to_string())?;
    if target.map_id != attacker.map_id || target.instance_id != attacker.instance_id {
        return Err("target on another map".to_string());
    }
    if target.dead {
        // Can't attack a corpse during decay. The gateway maps this exact error to
        // SMSG_ATTACKSWING_DEADTARGET so the client leaves combat stance (shared constant).
        return Err(lyracore_shared::ERR_ATTACK_TARGET_DEAD.to_string());
    }
    // Faction gate: reject a swing only at a FRIENDLY target. Hostile (red) AND neutral (yellow —
    // e.g. Elwynn wolves, which are huntable) stay attackable, matching vanilla; only friendly (green)
    // units are protected. The gateway maps this to SMSG_ATTACKSWING_CANT_ATTACK so the client leaves
    // stance. SKIPPED when faction data isn't loaded (table empty) so missing data never blocks combat.
    if ctx.db.game_faction_template().count() > 0 && !may_harm(ctx, attacker, &target) {
        return Err(lyracore_shared::ERR_ATTACK_FRIENDLY.to_string());
    }
    Ok(target)
}

/// Shared core: arm (or retarget) `attacker_guid`'s MELEE auto-attack on `target_guid` — the
/// explicit-guid body behind the `start_attack` reducer and `actor::attack`.
pub(crate) fn apply_start_attack(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
) -> Result<(), String> {
    let attacker = crate::helpers::live_entity(ctx, attacker_guid)
        .map_err(|_| "attacker not in world".to_string())?;
    // Crowd control: an ACTION-blocked attacker (stunned/polymorphed/feared) cannot ENTER combat —
    // arming an engagement is itself an action. This is the player-command twin of the per-swing gate in
    // `tick_melee` (without it a CC'd player could insert a `game_melee_attack` row whose swings are then
    // all blocked — a hollow "in combat but can't swing" state). Mirrors the cast chokepoint
    // (`resolve_cast_at`) and the creature aggro/cast action passes. Baseline-safe: `false` without a CC
    // aura → an un-CC'd attack command arms exactly as before. (The CC error message deliberately does NOT
    // match the gateway's desync classifier, so it just rejects the command rather than dropping the
    // session.)
    if crate::spell::is_action_blocked(ctx, attacker.guid) {
        return Err(format!(
            "attacker {} cannot act (stun/poly/fear)",
            attacker.guid
        ));
    }
    if target_guid == attacker.guid {
        return Err("cannot attack self".to_string());
    }
    validate_attack_target(ctx, &attacker, target_guid)?;

    // LAND-MOUNT dismount (22): an ACCEPTED attack start drops the attacker's mount BEFORE the
    // engagement is armed. Every rejection above returned already, so an invalid attack packet leaves
    // the mount up. No-op for an unmounted attacker, and idempotent on a re-target.
    crate::mount::dismount(ctx, attacker.guid);

    // Arm the engagement (the tick gates each swing on range + timer). Re-arming retargets.
    let melee = ctx.db.game_melee_attack();
    let row = MeleeAttack {
        attacker_guid: attacker.guid,
        target_guid,
        last_swing_ms: 0,
        ranged_spell_id: 0, // melee auto-attack
        last_offhand_swing_ms: 0,
        rout_ends_ms: 0,
        pursuit_ends_ms: 0,
        leash_x: 0.0,
        leash_y: 0.0,
    };
    if melee.attacker_guid().find(attacker.guid).is_some() {
        melee.attacker_guid().update(row);
    } else {
        melee.insert(row);
    }
    Ok(())
}

/// Shared core: arm `attacker_guid`'s RANGED auto-attack on `target_guid` with `spell_id`. Same gates as
/// `start_attack` (CC / self / target exists / same map / not a corpse / not friendly) PLUS a ranged
/// weapon must be equipped (slot 17) — Auto Shot/Shoot are impossible bare-handed. Arms/retargets the one
/// engagement row with `ranged_spell_id = spell_id`; the swing tick then runs the ranged branch.
pub(crate) fn apply_start_ranged_attack(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    spell_id: u32,
) -> Result<(), String> {
    let attacker = crate::helpers::live_entity(ctx, attacker_guid)
        .map_err(|_| "attacker not in world".to_string())?;
    if crate::spell::is_action_blocked(ctx, attacker.guid) {
        return Err(format!(
            "attacker {} cannot act (stun/poly/fear)",
            attacker.guid
        ));
    }
    if target_guid == attacker.guid {
        return Err("cannot attack self".to_string());
    }
    // The equipped ranged weapon, read ONCE for the whole reducer (it used to be fetched three times:
    // here, for the ammo gate, and again for the wind-up seed) — Auto Shot / Shoot are impossible
    // bare-handed. Checked BEFORE the shared target gate, preserving the order in which a command that
    // is invalid on both counts reports its reason.
    let ranged_weapon = equipped_ranged_weapon(ctx, attacker.guid)
        .ok_or_else(|| "no ranged weapon equipped".to_string())?;
    let target = validate_attack_target(ctx, &attacker, target_guid)?;
    // Activation CheckCast (097/vanilla): vmangos REJECTS the auto-repeat activation for any hard
    // cast failure — the gateway relays the reason as SMSG_CAST_RESULT and the client drops its
    // toggle, so the client's auto-repeat state never outlives a loop that could not start. Without
    // these gates the row armed silently, every shot was suppressed by the same checks in the swing
    // tick, and the client sat toggled-on over a dead loop. Error strings are load-bearing: the
    // gateway's `cast_failure_reason_for` substring-maps them to the 1.12 red-error reasons
    // ("out of range" → 0x59, "too close" → 0x76, "in front" → 0x7C, "line of sight" → 0x2A).
    {
        let (dx, dy, dz) = (
            target.x - attacker.x,
            target.y - attacker.y,
            target.z - attacker.z,
        );
        let dist_sq = dx * dx + dy * dy + dz * dz;
        if dist_sq > RANGED_RANGE_SQ {
            return Err("ranged target out of range".to_string());
        }
        if dist_sq < MELEE_RANGE_SQ {
            return Err("ranged target too close".to_string());
        }
        if !crate::spell::is_facing(
            attacker.x,
            attacker.y,
            attacker.orientation,
            target.x,
            target.y,
        ) {
            return Err("target must be in front of you".to_string());
        }
        if !crate::nav::has_los(
            ctx,
            attacker.map_id,
            (attacker.x, attacker.y, attacker.z),
            (target.x, target.y, target.z),
        ) {
            return Err("target not in line of sight".to_string());
        }
        // A launcher (bow/gun/crossbow) with an empty quiver rejects at activation — vanilla's
        // SPELL_FAILED_NO_AMMO red error — instead of arming, sending a clean START, and silently
        // cancelling ~500ms later when the first shot's find_ammo comes up empty (review find).
        // Wands consume nothing. The mid-loop run-out keeps the swing tick's teardown+cancel.
        use crate::items::weapon_subclass as ws;
        if matches!(ranged_weapon.3, ws::BOW | ws::GUN | ws::CROSSBOW)
            && find_ammo(ctx, attacker.guid).is_none()
        {
            return Err("no ammo for ranged weapon".to_string());
        }
    }
    // LAND-MOUNT dismount (22): the ranged twin of the melee hook — after the last activation gate,
    // before the engagement is armed, so a refused activation leaves the mount up.
    crate::mount::dismount(ctx, attacker.guid);
    // Initial-shot wind-up (097): a ranged auto-attack must NOT fire instantly on activation — vanilla's
    // Auto Shot has a ~0.5s cast before the first shot (the user: "we shoot right away, no waiting for the
    // attack timer"). `last_swing_ms == 0` would make the swing tick fire THIS tick; instead seed it so the
    // first shot is `RANGED_INITIAL_SHOT_MS` out. The swing gate fires when `now - last_swing >= delay`, so
    // set `last_swing = now - (delay - initial)` → the first shot lands after `initial` ms, then subsequent
    // shots on the full weapon `delay`. A fast weapon (delay < initial) → seed `now` → waits its full delay.
    // Melee keeps `last_swing_ms: 0` (swings immediately on engage — correct for melee).
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let ranged_delay = ranged_weapon.2;
    let first_swing_seed = now_ms
        .saturating_sub(ranged_delay.saturating_sub(RANGED_INITIAL_SHOT_MS))
        .max(1); // .max(1): never the 0 "swing now" sentinel
    let melee = ctx.db.game_melee_attack();
    let row = MeleeAttack {
        attacker_guid: attacker.guid,
        target_guid,
        last_swing_ms: first_swing_seed,
        ranged_spell_id: spell_id,
        last_offhand_swing_ms: 0,
        rout_ends_ms: 0,
        pursuit_ends_ms: 0,
        leash_x: 0.0,
        leash_y: 0.0,
    };
    if melee.attacker_guid().find(attacker.guid).is_some() {
        melee.attacker_guid().update(row);
    } else {
        melee.insert(row);
    }
    Ok(())
}

/// Shared core: disarm `attacker_guid`'s outgoing auto-attack row (melee or ranged) — the
/// explicit-guid body behind the `stop_attack` reducer and `actor::stop_attack`.
/// Unconditional; a no-op when nothing is armed.
pub(crate) fn stop_attack_for(ctx: &ReducerContext, attacker_guid: u64) {
    ctx.db
        .game_melee_attack()
        .attacker_guid()
        .delete(attacker_guid);
}

#[cfg(test)]
mod duel_relation_tests {
    use super::{may_harm_decision, may_help_decision};

    #[test]
    fn active_duel_is_the_only_friendly_fire_exception() {
        assert!(!may_harm_decision(false, false, true));
        assert!(may_harm_decision(false, true, true));
        assert!(may_harm_decision(false, false, false));
        assert!(may_harm_decision(true, false, true));
    }

    #[test]
    fn active_opponents_stop_being_helpful_targets() {
        assert!(may_help_decision(false, false, false));
        assert!(!may_help_decision(false, true, false));
        assert!(!may_help_decision(false, false, true));
        assert!(may_help_decision(true, false, true));
    }
}

#[cfg(test)]
mod engagement_reset_tripwire {
    use crate::test_scan::code_of;

    /// `disengage` is the ONLY place that sees every way a fight ends: an evade, the player dying,
    /// a logout, a map change. Each of those clears `IN_COMBAT` here, so the cycle's combat-drop
    /// pass never reaches them and its `leave_combat` cannot be the EventAI engagement reset on its
    /// own. Dropping this call fails silently and in the player's favour nowhere: the next pull
    /// finds once-only aggro rules still spent and timed rules due on their first tick.
    #[test]
    fn freeing_a_unit_starts_its_next_eventai_engagement() {
        let body = code_of(
            include_str!("engage.rs"),
            "pub(crate) fn disengage(ctx: &ReducerContext, guid: u64) {",
        );
        assert!(
            body.contains("crate::creatures::reset_engagement(ctx, g)"),
            "`disengage` no longer resets the EventAI engagement of the units it freed"
        );
    }
}
