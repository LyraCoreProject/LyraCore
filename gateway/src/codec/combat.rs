//! Combat wire mapping: the spell-cast visual (`SMSG_SPELL_GO`), the attack start/stop stance
//! messages, and the per-swing `SMSG_ATTACKERSTATEUPDATE` (with its VictimState constants).
//! Pure code-motion out of `mod.rs`.

use super::*;

/// `SMSG_SPELL_COOLDOWN` — start the client's cooldown swipe on `spell_id` for `cooldown_ms`. Sent ONLY for
/// spells that actually HAVE a cooldown (Mortal Strike 6s, Judgement 10s, …). mangos does NOT send a cooldown
/// packet per cast: the 1.5s GCD is client-managed from Spell.dbc (StartRecoveryTime), and the SMSG_SPELL_GO
/// is what releases the client's pending-cast state. A prior theory had the gateway send this (cooldown 0)
/// after EVERY cast to "close the pending lock" — that instead STUCK the action button ("Another action is in
/// progress" on every recast). The relay now gates it on cooldown_ms > 0. `guid` is a plain 8-byte guid. [combat]
pub fn build_spell_cooldown(
    caster_guid: u64,
    spell_id: u32,
    cooldown_ms: u32,
) -> SMSG_SPELL_COOLDOWN {
    SMSG_SPELL_COOLDOWN {
        guid: Guid::new(caster_guid),
        cooldowns: vec![SpellCooldownStatus {
            id: spell_id,
            cooldown_time: std::time::Duration::from_millis(cooldown_ms as u64),
        }],
    }
}

// The argument list IS the packet's field list (SMSG_SPELLNONMELEEDAMAGELOG); a struct here would only move the same fields one level down.
#[allow(clippy::too_many_arguments)]
/// `SMSG_SPELLNONMELEEDAMAGELOG` — the floating spell damage number over the target (vanilla "Shadow Bolt
/// hits X for N Shadow damage"). Sent on the cast-GO when the cast dealt damage. `school_index` is the
/// SpellSchool INDEX (0=normal/physical..6=arcane) the module already derived from the spell header's
/// school bitmask, so this builder stays dumb (no mask math) — an out-of-range index falls back to Normal.
/// `is_crit`/`resisted`/`absorbed` now ride from the cast-event row: a crit sets hit_info=CriticalHit
/// (the 5875 client renders the number as a crit), and resisted/absorbed populate the "(N resisted)"/
/// "(N absorbed)" suffixes. `blocked` stays 0 (spells aren't blocked in vanilla). Single-target —
/// multi-target / DoT per-tick logging is a follow-up. `target`/`attacker` are PackedGuid (matches the
/// other combat builders).
pub fn build_spell_non_melee_damage_log(
    target_guid: u64,
    caster_guid: u64,
    spell_id: u32,
    damage: u32,
    school_index: u8,
    is_crit: bool,
    resisted: u32,
    absorbed: u32,
) -> SMSG_SPELLNONMELEEDAMAGELOG {
    SMSG_SPELLNONMELEEDAMAGELOG {
        target: Guid::new(target_guid),
        attacker: Guid::new(caster_guid),
        spell: spell_id,
        damage,
        school: SpellSchool::try_from(school_index).unwrap_or(SpellSchool::Normal),
        absorbed_damage: absorbed,
        resisted,
        periodic_log: false,
        unused: 0,
        blocked: 0,
        // GTKER FIELD NAMES LIE (3rd confirmed instance): gtker typed this field with the MELEE
        // HitInfo enum, but the 5875 client reads it as the SPELL hit-type flags (cmangos
        // SendSpellNonMeleeDamageLog: SPELL_HIT_TYPE_CRIT = 0x2). HitInfo::CriticalHit encodes 0x80 —
        // the melee crit bit — which the client ignores here (live find: a 22-damage Heroic Strike
        // crit rendered plain). The variant that happens to encode the needed 0x2 is AffectsVictim.
        hit_info: if is_crit {
            HitInfo::AffectsVictim
        } else {
            HitInfo::NormalSwing
        },
        extend_flag: 0,
    }
}

/// The cast *begin* (`SMSG_SPELL_START`), paired with `build_spell_go` and sent FIRST. The 5875 client
/// opens its cast state on CMSG_CAST_SPELL and only fully closes it when it sees the START→GO pair the
/// real cores always send — a lone `SMSG_SPELL_GO` leaves `m_currentSpell` set, so the action button
/// stays lit and every further ability fails with "Another action is in progress." `timer = 0` (the
/// gateway relays this when the cast RESOLVES, so for both instant and just-finished timed casts the
/// remaining cast time is 0). Self-cast: `cast_item == caster`, empty targets/flags.
///
/// `target`/`ammo` exist for the RANGED auto-repeat activation (097): vanilla's SendSpellStart writes
/// the real unit target (not SELF) and, for a ranged spell, CAST_FLAG_AMMO (0x20) + the ammo block —
/// that block is what nocks the arrow/bullet on the 5875 client. All other casts pass `0, None`
/// (byte-identical to before). [combat]
pub fn build_spell_start(
    caster_guid: u64,
    spell_id: u32,
    timer: u32,
    target: u64,
    ammo: Option<(u32, u32)>,
) -> SMSG_SPELL_START {
    // [083] CAST_FLAG_UNKNOWN2 (0x02) — base flag every mangos/vmangos SendSpellStart sets; a ranged
    // auto-repeat ORs in CAST_FLAG_AMMO (0x20) + the ammo block (mirrors build_spell_go's ammo path).
    let flags = match ammo {
        Some((ammo_display_id, ammo_inventory_type)) => SMSG_SPELL_START_CastFlags::new(
            0x0002 | 0x0020,
            Some(SMSG_SPELL_START_CastFlags_Ammo {
                ammo_display_id,
                ammo_inventory_type,
            }),
        ),
        None => SMSG_SPELL_START_CastFlags::new_unknown2(),
    };
    let targets = if target == 0 || target == caster_guid {
        SpellCastTargets::default()
    } else {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
                SpellCastTargets_SpellCastTargetFlags_Unit {
                    unit_target: Guid::new(target),
                },
            ),
        }
    };
    SMSG_SPELL_START {
        cast_item: Guid::new(caster_guid),
        caster: Guid::new(caster_guid),
        spell: spell_id,
        flags,
        timer, // cast-bar duration (ms): >0 on a timed cast-START, 0 on an instant START
        targets,
    }
}

/// The cast-INTERRUPT signal (`SMSG_SPELL_FAILURE`, opcode 0x0133, 13-byte body). Relayed to the caster
/// when its in-progress timed cast is cancelled by direct damage (or CC) mid-cast — no `SMSG_SPELL_GO`
/// follows (the cast never resolves), so the client must be told explicitly to tear down its filling cast
/// bar. `result = Interrupted` (byte 0x23) is the vanilla "your cast was interrupted" reason. [combat]
pub fn build_spell_failure(caster_guid: u64, spell_id: u32) -> SMSG_SPELL_FAILURE {
    SMSG_SPELL_FAILURE {
        guid: Guid::new(caster_guid),
        spell: spell_id,
        result: SpellCastResult::Interrupted,
    }
}

/// `SMSG_SPELL_DELAYED` (opcode 0x01E2, 12-byte body: guid + u32 delay) — vanilla 1.12's cast-bar
/// PUSHBACK signal (work-item 039). Sent when a landed DIRECT hit slides `caster_guid`'s in-progress
/// timed cast's fire time by `delay_ms` (the module caps this at `CAST_PUSHBACK_MAX` pushbacks per
/// cast — see `module/src/spell/cast.rs::pushback_cast`); the client visibly slides its cast bar by the
/// same amount rather than tearing it down (unlike `SMSG_SPELL_FAILURE`/`build_spell_failure`, which
/// tears the bar down on an actual cancel/Kick). Broadcast like `build_spell_start`/`build_spell_go`
/// (the cast bar is caster-visible unit state, not a private failure message) — `guid` is a plain
/// 8-byte guid. [combat]
pub fn build_spell_delayed(caster_guid: u64, delay_ms: u32) -> SMSG_SPELL_DELAYED {
    SMSG_SPELL_DELAYED {
        guid: Guid::new(caster_guid),
        delay_time: delay_ms,
    }
}

/// Raw body for `SMSG_CAST_RESULT` (0x0130) with result = OKAY (0x00). vmangos sends this
/// after `SMSG_SPELL_COOLDOWN` and BEFORE `SMSG_SPELL_GO` for every successful non-triggered
/// player cast; without it the 5875 client's m_currentSpells is never cleared, leaving the action
/// button locked ("Another action is in progress"). Body = spell_id(u32 LE) + 0x00(u8) = 5 bytes.
/// Note: the gtker SMSG_CAST_RESULT wowm has the Success/Failure condition inverted (Success would
/// add a reason byte), so this is emitted as `Outbound::Raw { opcode: 0x0130, body }`. [combat]
/// Raw body for `SMSG_CAST_RESULT` (0x0130) with result = FAILED: spell_id(u32 LE) + 0x02
/// (CAST_FAILED) + the 1.12 `CastFailureReason` byte — the client prints the red error line
/// ("Not enough rage", "You must be behind your target", ...). Raw for the same reason as
/// `build_cast_result_ok` below (gtker's typed SMSG_CAST_RESULT inverts the status semantics). [040]
pub fn build_cast_result_failed(spell_id: u32, reason: u8) -> Vec<u8> {
    let mut v = spell_id.to_le_bytes().to_vec();
    v.push(0x02);
    v.push(reason);
    v
}

/// Map a module cast-rejection message to the closest 1.12 `CastFailureReason` (040). The module's
/// rejections are human strings (not tagged codes), so this is a SUBSTRING map over the stable
/// phrases in `resolve_cast_at`'s gates — if a gate's wording changes, the worst case is the
/// generic fallback (0x17 DontReport: red-line-free silent fail, what mangos uses for
/// "don't confuse the player"). Values from gtker's vanilla CastFailureReason (client-verified ids).
pub fn cast_failure_reason_for(es: &str) -> u8 {
    const NO_POWER: u8 = 0x4D;
    const NOT_READY: u8 = 0x3C;
    const OUT_OF_RANGE: u8 = 0x59;
    const TOO_CLOSE: u8 = 0x76;
    const UNIT_NOT_INFRONT: u8 = 0x7C;
    const LINE_OF_SIGHT: u8 = 0x2A;
    const NO_AMMO: u8 = 0x24;
    const NOT_SHAPESHIFT: u8 = 0x3D;
    const NOT_BEHIND: u8 = 0x33;
    const ONLY_STEALTHED: u8 = 0x57;
    const INTERRUPTED: u8 = 0x23;
    const CASTER_AURASTATE: u8 = 0x12; // "You can't do that yet" — the react-window (Overpower/Revenge) fit
    const STUNNED: u8 = 0x64;
    const LEVEL_REQUIREMENT: u8 = 0x29;
    const TARGETS_DEAD: u8 = 0x65;
    const BAD_TARGETS: u8 = 0x0A;
    const DONT_REPORT: u8 = 0x17;
    if es.contains("not enough power") {
        NO_POWER
    } else if es.contains("cooldown") {
        NOT_READY
    } else if es.contains("out of range") || es.contains("another map") {
        OUT_OF_RANGE
    } else if es.contains("too close") {
        TOO_CLOSE
    } else if es.contains("in front") {
        UNIT_NOT_INFRONT
    } else if es.contains("line of sight") {
        LINE_OF_SIGHT
    } else if es.contains("no ammo") {
        NO_AMMO
    } else if es.contains("current stance") {
        NOT_SHAPESHIFT
    } else if es.contains("BEHIND the target") {
        NOT_BEHIND
    } else if es.contains("requires stealth") {
        ONLY_STEALTHED
    } else if es.contains("locked (interrupted)") {
        INTERRUPTED
    } else if es.contains("Overpower window") || es.contains("Revenge window") {
        CASTER_AURASTATE
    } else if es.contains("cannot act (stun") {
        STUNNED
    } else if es.contains("requires level") {
        LEVEL_REQUIREMENT
    } else if es.contains("is dead") {
        TARGETS_DEAD
    } else if es.contains("cannot be cast on this unit") || es.contains("only target") {
        BAD_TARGETS
    } else {
        DONT_REPORT
    }
}

pub fn build_cast_result_ok(spell_id: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(5);
    v.extend_from_slice(&spell_id.to_le_bytes());
    v.push(0x00); // SPELL_RESULT_STATUS_OKAY
    v
}

/// The cast visual (`SMSG_SPELL_GO`). `hit_target == 0` → a self-cast (hits = [caster]): the cast-clear
/// after a `CMSG_CAST_SPELL`, and the aura-tracer relay. `hit_target != 0` → a shot/bolt landing on that
/// target (hits = [target]): the per-cast projectile the client renders. `ammo` = `Some((display_id,
/// inv_type))` sets the AMMO flag so an Auto Shot fires the ARROW graphic (#10); `None` → no flag (wand
/// bolt / generic spell missile). `cast_item` == `caster` (cmangos convention; 0 is wrong), no misses.
///
/// PROJECTILE FIX: the 5875 client flies a spell's travelling missile (SpellVisual→SpellMissile, e.g.
/// Shadow Bolt) from `caster → m_targets.unit_target` — it reads the trajectory from the `targets`
/// (`SpellCastTargets`) block, NOT from `hits[]` (which only drives the combat log / impact resolution).
/// An empty `SpellCastTargets::default()` is TARGET_FLAG_SELF with no unit target, so a generic spell
/// bolt has no flight destination and the client renders the impact in place ("instant damage, no bolt").
/// When `hit_target != 0` we therefore set TARGET_FLAG_UNIT + the packed target guid so the missile
/// flies at the mob. (Auto Shot/wand looked right only because the AMMO path renders from the caster's
/// ranged target/facing; a no-ammo spell bolt needs this unit target.) Self-cast → default unchanged.
pub fn build_spell_go(
    caster_guid: u64,
    spell_id: u32,
    hit_target: u64,
    ammo: Option<(u32, u32)>,
) -> SMSG_SPELL_GO {
    build_spell_go_outcome(caster_guid, spell_id, hit_target, ammo, false)
}

/// The GO for a GROUND-AREA spell (118, Consecration): an EMPTY hit list — the spell hits the
/// ground, nothing "impacts" a unit at cast time (the area's ticks deal the damage; the swirl is
/// the DynamicObject). The generic self-cast fallback put the CASTER in `hits[]` and the 5875
/// client played the impact animation ON the paladin (user bug). Targets stay SELF (the area is
/// caster-anchored this slice; a dest-clicked area rides cast_spell_at's dest block separately).
pub fn build_spell_go_area(caster_guid: u64, spell_id: u32) -> SMSG_SPELL_GO {
    SMSG_SPELL_GO {
        cast_item: Guid::new(caster_guid),
        caster: Guid::new(caster_guid),
        spell: spell_id,
        flags: SMSG_SPELL_GO_CastFlags::new_unknown9(),
        hits: vec![],
        misses: vec![],
        targets: SpellCastTargets::default(),
    }
}

/// `build_spell_go` with an explicit MISS outcome (097): a MISSED ranged auto-shot puts the target in
/// the GO's `misses` list (SpellMissInfo::Miss) instead of `hits` — the 5875 client renders the white
/// "Miss" over the target from this list (vanilla shape; a missed shot sends NO damage log). Every
/// other call site keeps the 4-arg `build_spell_go` (miss = false, byte-identical).
pub fn build_spell_go_outcome(
    caster_guid: u64,
    spell_id: u32,
    hit_target: u64,
    ammo: Option<(u32, u32)>,
    miss: bool,
) -> SMSG_SPELL_GO {
    // [083] A self-cast arrives as hit_target 0 OR the caster's own guid (cast_spell resolves a self-cast
    // to target_guid = caster). Both write TARGET_FLAG_SELF (the default), NOT TARGET_FLAG_UNIT → caster.
    let is_self_cast = hit_target == 0 || hit_target == caster_guid;
    let hit = if is_self_cast {
        caster_guid
    } else {
        hit_target
    };
    // [083] CAST_FLAG_UNKNOWN9 (0x0100) base — every mangos/vmangos SendSpellGo sets it; a ranged shot
    // ORs in CAST_FLAG_AMMO (0x20) + the ammo block.
    let flags = match ammo {
        Some((ammo_display_id, ammo_inventory_type)) => SMSG_SPELL_GO_CastFlags::new(
            0x0100 | 0x0020,
            Some(SMSG_SPELL_GO_CastFlags_Ammo {
                ammo_display_id,
                ammo_inventory_type,
            }),
        ),
        None => SMSG_SPELL_GO_CastFlags::new_unknown9(),
    };
    // A real OTHER target rides as TARGET_FLAG_UNIT (caster → unit_target so the missile flies at it); a
    // self-cast keeps the default SELF target block.
    let targets = if is_self_cast {
        SpellCastTargets::default()
    } else {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
                SpellCastTargets_SpellCastTargetFlags_Unit {
                    unit_target: Guid::new(hit_target),
                },
            ),
        }
    };
    let (hits, misses) = if miss {
        (
            vec![],
            vec![SpellMiss {
                target: Guid::new(hit),
                miss_info: SpellMissInfo::Miss,
            }],
        )
    } else {
        (vec![Guid::new(hit)], vec![])
    };
    SMSG_SPELL_GO {
        cast_item: Guid::new(caster_guid),
        caster: Guid::new(caster_guid),
        spell: spell_id,
        flags,
        hits,
        misses,
        targets,
    }
}

/// Build `SMSG_ATTACKSTART` (Tier 3 combat C1) — tells observers `attacker` is now auto-attacking
/// `victim`, which puts the attacker into combat stance. Both guids are the values the client
/// already knows the units by (player = raw low guid, high-part 0; creature = full unit guid), so
/// `Guid::new` mirrors `build_create_object`.
pub fn build_attack_start(attacker_guid: u64, victim_guid: u64) -> SMSG_ATTACKSTART {
    SMSG_ATTACKSTART {
        attacker: Guid::new(attacker_guid),
        victim: Guid::new(victim_guid),
    }
}

/// Build `SMSG_ATTACKSTOP` (Tier 3 combat C1) — leaves combat stance. `unknown1 = 0` per the
/// mangos cores.
pub fn build_attack_stop(attacker_guid: u64, victim_guid: u64) -> SMSG_ATTACKSTOP {
    SMSG_ATTACKSTOP {
        player: Guid::new(attacker_guid),
        enemy: Guid::new(victim_guid),
        unknown1: 0,
    }
}

/// Build `SMSG_ATTACKERSTATEUPDATE` (Tier 3 combat C1) for one physical normal melee swing of
/// `damage`. This drives the swing animation + floating damage text; the victim's health bar is
/// moved separately by the `game_world_entity` `on_update` → VALUES relay. Single physical
/// `DamageInfo` (school mask 0; `damage_float == damage_uint`), no block/absorb/resist.
pub fn build_attacker_state_update(
    attacker_guid: u64,
    target_guid: u64,
    damage: u32,
    hit_info: u8,
    blocked_amount: u32,
    spell_id: u32,
) -> SMSG_ATTACKERSTATEUPDATE {
    // The 5875 client picks the floating combat text from `damage_state` (the VictimState) and the
    // HitInfo flags TOGETHER — NOT from the damage value. mangos-zero/vmangos
    // (Unit::CalculateMeleeDamage) set, for a player/creature BASE_ATTACK auto-swing:
    //   landed NORMAL: HitInfo = AFFECTS_VICTIM (0x02),               VictimState = NORMAL (1)
    //   landed CRIT:   HitInfo = AFFECTS_VICTIM|CRITICAL_HIT (0x82),  VictimState = NORMAL (1)
    //   MISS:          HitInfo = MISS (0x10) [AFFECTS_VICTIM cleared],VictimState = UNAFFECTED (0)
    // Our old packet sent VictimState=0 (UNAFFECTED) for landed hits — UNAFFECTED is precisely the
    // value mangos sends on a MISS, so the client renders "MISS" even though DamageInfo carried real
    // damage. The load-bearing field for the text is `damage_state`; AFFECTS_VICTIM (0x02) is what
    // plays the victim's being-hit animation (vmangos: "No animation on victim" clears it on miss).
    //
    // gtker's `HitInfo` is a CLOSED single-value enum written via `as_int()`, so it cannot carry the
    // OR'd 0x82 / 0x12 mangos uses — there is no raw/From<u32> write path. We therefore send the
    // dominant single bit through the enum and keep the (now-correct) VictimState in `damage_state`:
    //   normal -> HitInfo::AffectsVictim (0x02), VictimState 1   (hit + animation, no degradation)
    //   crit   -> HitInfo::CriticalHit   (0x80), VictimState 1   (loses 0x02 anim bit; crit text +
    //                                                             VictimState=1 still render as a hit)
    //   miss   -> HitInfo::Miss          (0x10), VictimState 0   (matches mangos exactly)
    // Dodge/parry are zero-damage avoidance outcomes. In vanilla they ride the VictimState
    // (`damage_state`), NOT a HitInfo flag (gtker's HitInfo has no Dodge/Parry variant — the client
    // renders the "Dodge"/"Parry" text from VictimState 2/3). HitInfo stays NormalSwing (0x0) so the
    // AFFECTS_VICTIM hit-animation bit is cleared — no being-hit animation plays on an avoided swing.
    let (hit_info, damage_state) = match hit_info {
        1 => (HitInfo::CriticalHit, u32::from(VICTIMSTATE_NORMAL)),
        2 => (HitInfo::Miss, u32::from(VICTIMSTATE_UNAFFECTED)),
        3 => (HitInfo::NormalSwing, u32::from(VICTIMSTATE_DODGE)),
        4 => (HitInfo::NormalSwing, u32::from(VICTIMSTATE_PARRY)),
        // Glancing: a landed hit (VictimState NORMAL) carrying reduced damage; HitInfo::Glancing
        // (0x4000) drives the "Glancing" text. Affects-victim animation still plays (it connected).
        5 => (HitInfo::Glancing, u32::from(VICTIMSTATE_NORMAL)),
        // Block: a LANDED hit a shield reduced by `blocked_amount`. Like dodge/parry, vanilla renders the
        // "Block" floating text from the VictimState (BLOCK=4) + the `blocked_amount` field, NOT a HitInfo
        // flag (gtker's HitInfo has no Block variant). It still affects the victim (the swing connected),
        // so it carries AffectsVictim — the being-hit animation plays, unlike avoidance.
        7 => (HitInfo::AffectsVictim, u32::from(VICTIMSTATE_BLOCK)),
        _ => (HitInfo::AffectsVictim, u32::from(VICTIMSTATE_NORMAL)),
    };
    SMSG_ATTACKERSTATEUPDATE {
        hit_info,
        attacker: Guid::new(attacker_guid),
        target: Guid::new(target_guid),
        total_damage: damage,
        damages: vec![DamageInfo {
            spell_school_mask: 0, // physical
            damage_float: damage as f32,
            damage_uint: damage,
            absorb: 0,
            resist: 0,
        }],
        damage_state,
        unknown1: 0,
        // 0 for a melee swing; the ranged spell (75 Auto Shot / 5019 Shoot) for a ranged shot (#10), so
        // the client attributes the shot to the ability. HitInfo is identical to melee — vanilla has no
        // ranged-specific HitInfo flag.
        spell_id,
        blocked_amount,
    }
}

/// VictimState (the `damage_state` u32 in vanilla SMSG_ATTACKERSTATEUPDATE). The 5875 client reads
/// this to decide whether the swing affected the victim; `UNAFFECTED` makes it render "MISS".
/// (mangos: `VICTIMSTATE_UNAFFECTED = 0`, `VICTIMSTATE_NORMAL = 1`, `DODGE = 2`, `PARRY = 3`.) The
/// 5875 client renders the "Dodge"/"Parry" floating text from these (HitInfo carries no such flag).
const VICTIMSTATE_UNAFFECTED: u8 = 0;
const VICTIMSTATE_NORMAL: u8 = 1;
const VICTIMSTATE_DODGE: u8 = 2;
const VICTIMSTATE_PARRY: u8 = 3;
const VICTIMSTATE_BLOCK: u8 = 4; // a shielded defender — drives the "Block" text with `blocked_amount`

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spell_start_carries_cast_bar_timer() {
        // Fix 2 (peer cast bar): a timed cast-START relays its duration so observers see the bar fill;
        // an instant cast / cast-clear sends 0.
        assert_eq!(build_spell_start(0xF1, 133, 3500, 0, None).timer, 3500);
        assert_eq!(build_spell_start(0xF1, 133, 0, 0, None).timer, 0);
        // 097: the ranged-activation START carries the AMMO flag + block and the real unit target.
        let s = build_spell_start(0xF1, 75, 0, 0xBEEF, Some((5996, 24)));
        assert_eq!(
            s.flags
                .get_ammo()
                .map(|a| (a.ammo_display_id, a.ammo_inventory_type)),
            Some((5996, 24))
        );
        assert!(s.targets.target_flags.get_unit().is_some());
        // 097: a MISSED shot's GO carries the target in `misses`, not `hits`.
        let go = build_spell_go_outcome(0xF1, 75, 0xBEEF, Some((5996, 24)), true);
        assert!(go.hits.is_empty());
        assert_eq!(go.misses.len(), 1);
    }

    #[test]
    fn spell_go_aims_at_the_target_not_self() {
        // Fix 1 (peer cast direction): the missile flies at the primary target (hits = [target]).
        let caster = 0xF130_0000_0000_0001u64;
        let target = 0xF130_0000_0000_00AAu64;
        assert_eq!(
            build_spell_go(caster, 133, target, None).hits,
            vec![Guid::new(target)]
        );
        // hit_target 0 → self-cast falls back to the caster (unchanged).
        assert_eq!(
            build_spell_go(caster, 133, 0, None).hits,
            vec![Guid::new(caster)]
        );
    }

    #[test]
    fn spell_go_targets_carry_unit_for_the_missile_trajectory() {
        // PROJECTILE FIX: a real target must ride SMSG_SPELL_GO.targets as TARGET_FLAG_UNIT so the
        // 5875 client flies the spell missile (caster → unit_target). hits[] alone is not enough.
        let caster = 0xF130_0000_0000_0001u64;
        let target = 0xF130_0000_0000_00AAu64;
        let go = build_spell_go(caster, 686, target, None);
        let unit = go.targets.target_flags.get_unit().expect("UNIT flag set");
        assert_eq!(unit.unit_target, Guid::new(target));
        // Self-cast (hit_target 0) keeps the default SELF target block — no unit target.
        let self_go = build_spell_go(caster, 686, 0, None);
        assert!(self_go.targets.target_flags.get_unit().is_none());
    }

    #[test]
    fn spell_failure_carries_interrupted_for_the_caster() {
        // Cast-interrupt signal: SMSG_SPELL_FAILURE addressed to the caster's guid, naming the cancelled
        // spell, with result=Interrupted (the cast-bar teardown). guid is the plain 8-byte caster guid.
        let caster = 0xF130_0000_0000_0005u64;
        let f = build_spell_failure(caster, 686);
        assert_eq!(f.guid, Guid::new(caster));
        assert_eq!(f.spell, 686);
        assert_eq!(f.result, SpellCastResult::Interrupted);
    }

    #[test]
    fn spell_delayed_carries_the_caster_guid_and_delay_ms() {
        // Cast-pushback signal (work-item 039): SMSG_SPELL_DELAYED addressed to the caster's guid, with
        // the delay in ms so the client's cast bar visibly slides. guid is the plain 8-byte caster guid.
        let caster = 0xF130_0000_0000_0007u64;
        let d = build_spell_delayed(caster, 500);
        assert_eq!(d.guid, Guid::new(caster));
        assert_eq!(d.delay_time, 500);
    }

    #[test]
    fn cast_result_ok_body_is_spell_id_plus_status_byte() {
        let body = build_cast_result_ok(686);
        assert_eq!(body.len(), 5, "spell_id(u32 LE) + status(u8)");
        assert_eq!(u32::from_le_bytes(body[0..4].try_into().unwrap()), 686);
        assert_eq!(body[4], 0x00, "SPELL_RESULT_STATUS_OKAY");
    }

    #[test]
    fn spell_cooldown_carries_the_spell_and_duration() {
        let msg = build_spell_cooldown(0xF1, 6673, 6000);
        assert_eq!(msg.guid.guid(), 0xF1);
        assert_eq!(
            msg.cooldowns.len(),
            1,
            "only the just-cast spell gets a cooldown entry"
        );
        assert_eq!(msg.cooldowns[0].id, 6673);
        assert_eq!(
            msg.cooldowns[0].cooldown_time,
            std::time::Duration::from_millis(6000)
        );
    }
}

/// The client-side SPELL-MODIFIER mirror (264): aggregate raw modifier rows `(family_mask, op,
/// amount, is_pct)` into one `SMSG_SET_FLAT/PCT_SPELL_MODIFIER` per SET BIT of each op's combined
/// mask — `eff` is the mask BIT INDEX (0..31) and `value` the TOTAL for that (op, bit), the mangos
/// SendSpellMod convention. The 5875 client then shortens its own cast bars/tooltips for the
/// affected spells; the server stays authoritative regardless (display parity only).
pub fn build_spell_modifier_msgs(rows: &[(u32, u8, i32, bool)]) -> Vec<ServerOpcodeMessage> {
    use std::collections::BTreeMap;
    use wow_world_messages::vanilla::{SMSG_SET_FLAT_SPELL_MODIFIER, SMSG_SET_PCT_SPELL_MODIFIER};
    // (is_pct, op, bit) -> summed value. BTreeMap for a deterministic packet order.
    let mut totals: BTreeMap<(bool, u8, u8), i64> = BTreeMap::new();
    for &(mask, op, amount, pct) in rows {
        for bit in 0..32u8 {
            if mask & (1 << bit) != 0 {
                *totals.entry((pct, op, bit)).or_default() += amount as i64;
            }
        }
    }
    totals
        .into_iter()
        .map(|((pct, op, bit), value)| {
            let value = value as i32 as u32; // signed totals bit-cast onto the wire (−200 → 0xFFFFFF38)
            if pct {
                ServerOpcodeMessage::SMSG_SET_PCT_SPELL_MODIFIER(SMSG_SET_PCT_SPELL_MODIFIER {
                    eff: bit,
                    op,
                    value,
                })
            } else {
                ServerOpcodeMessage::SMSG_SET_FLAT_SPELL_MODIFIER(SMSG_SET_FLAT_SPELL_MODIFIER {
                    eff: bit,
                    op,
                    value,
                })
            }
        })
        .collect()
}

#[cfg(test)]
mod spellmod_tests {
    use super::*;

    #[test]
    fn spell_modifier_msgs_aggregate_per_op_and_bit() {
        // Two FLAT cast-time mods sharing bit 0 sum; a PCT damage mod rides its own packet kind.
        let rows = vec![
            (0b1u32, 10u8, -100i32, false),
            (0b1, 10, -100, false),
            (0b11, 0, 5, true),
        ];
        let msgs = build_spell_modifier_msgs(&rows);
        assert_eq!(msgs.len(), 3); // flat(op10,bit0) + pct(op0,bit0) + pct(op0,bit1)
        match &msgs[0] {
            ServerOpcodeMessage::SMSG_SET_FLAT_SPELL_MODIFIER(m) => {
                assert_eq!((m.eff, m.op, m.value as i32), (0, 10, -200));
            }
            other => panic!("expected FLAT first, got {other:?}"),
        }
    }
}
