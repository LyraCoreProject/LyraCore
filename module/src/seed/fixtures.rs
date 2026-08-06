//! Test/mock-seed FIXTURES — synthetic spells, items, NPCs, and quests (5xxxx ids) that keep
//! engine mechanics headlessly verifiable on a no-import sandbox (no Spell.dbc — the licensing
//! firewall keeps real client data out of the repo). This is a DIFFERENT fixture family from
//! `seed.rs`'s own map-0 (Northshire) demo content (see that file's header, layer 2): these are
//! synthetic engine-mechanic probes with no map presence, kept in their own file so the kit can
//! grow without dragging `init` itself past readability; `seed.rs`'s map-0 fixtures stay inline
//! because they compose directly with the production seed's spawn/gameobject calls.
//! Every fn here is IDEMPOTENT (insert-if-absent / upsert) and is shared by `init` and its
//! feature-gated `debug_seed_*` re-runner reducer (init does NOT re-run on an auto-migrate
//! publish, so a dev DB re-seeds via the debug reducer).

use spacetimedb::{ReducerContext, Table};

use crate::{
    game_creature_template, game_faction, game_item_template, game_spell, game_spell_effect,
    CreatureTemplate, Faction, ItemTemplate, Spell, SpellEffect,
};

/// Seed Weakened Soul (6788) + the Test PW:Shield fixture (50072) — the generic linked-debuff mechanic.
/// IDEMPOTENT (inserts only rows that are absent), mirroring `talent::seed_talents`, so
/// it is safe to call from `init` (fresh install) AND from `debug_seed_pw_shield_fixture` on an
/// already-migrated dev DB (where `init` did not re-run).
///
/// Weakened Soul (REAL vanilla id 6788) is the hardcoded Power Word: Shield lockout debuff. Its real
/// Spell.dbc shape (CONFIRMED via a DBC dry-run for work-item 122 — this is NOT the effectless marker
/// earlier believed) is a single `A_IMMUNITY` (0xB1) aura with MiscValue 19 (MECHANIC_SHIELD): vanilla's
/// actual "immune to the shield mechanic" (i.e. can't be re-shielded) effect. 15s duration, holy school
/// (school_mask 2), dispel_type 0 — mirroring the importer's DBC output so a seed-only dev DB matches a
/// full-import DB byte-for-byte. Applied generically by `spell::apply_linked_debuff` whenever an aura
/// effect's `p1` names it; the PW:Shield lockout gate keys on `has_aura(target, 6788)` (presence of ANY
/// 6788 aura), so this aura fires the refusal exactly like the old marker did — now DBC-faithful.
///
/// Test Power Word: Shield (50072) is a PW:Shield-shaped fixture (a single A_ABSORB effect, matching the
/// real live spell 17's DBC shape) that links Weakened Soul (6788) via `p1` —
/// headlessly exercises the generic linked-debuff apply + refusal gate without needing a live client
/// Spell.dbc (none is available in every dev environment; the licensing firewall keeps it out of the
/// repo). Ally-targeted, holy school, absorbs 50 damage. `debug_cast_at(caster_guid, 50072, target_guid)`:
/// places the shield + Weakened Soul; a second cast at the same target within ~15s returns Err (the
/// linked-debuff refusal gate in resolve_cast_at); once Weakened Soul expires, a re-cast succeeds again.
///
/// The REAL live spell 17's imported `A_ABSORB` effect also carries `p1 = 6788` via a by-NAME override in
/// `importer/src/spell.rs` (`power_word_shield_p1_override`, mirroring the `synthetic_seal_effect`
/// precedent) — an operator who re-runs the importer against their own Spell.dbc gets the real spell 17
/// wired through the exact same generic mechanic as this fixture, no engine spell-id references needed.
/// This dev sandbox has no Spell.dbc (licensing firewall), so 50072 remains here purely so the mechanic is
/// headlessly exercisable without one.
pub(crate) fn seed_pw_shield_fixture(ctx: &ReducerContext) {
    // UPSERT (not insert-only): a `debug_seed_pw_shield_fixture` re-run on a dev DB that already has a
    // stale/earlier-shape fixture row corrects it in place, instead of silently keeping the stale data —
    // safe because these are TEST fixture ids (6788 is real-but-otherwise-unused; 50072 is a reserved
    // synthetic id), never touched by player state.
    let ws_hdr = Spell {
        spell_id: 6788,
        name: "Weakened Soul".to_string(),
        power_type: 0,
        cost: 0,
        family_name: 0,
        family_flags: 0,
        cast_time_ms: 0,
        gcd_ms: 1500,
        cooldown_ms: 0,
        range_yd: 0,
        duration_ms: 15000,
        school_mask: 2,
        dispel_type: 0,
        mechanic: 0,
        max_stacks: 0,
        aura_interrupt: 0,
        attributes: 0,
        spell_level: 0,
        max_level: 0,
        is_negative: false,
        cast_flags: 0,
        stances: 0,
    };
    if ctx.db.game_spell().spell_id().find(6788u32).is_some() {
        ctx.db.game_spell().spell_id().update(ws_hdr);
    } else {
        ctx.db.game_spell().insert(ws_hdr);
    }
    // Drop any stale-shape 6788 effect rows (an earlier fixture seeded an inert A_FLAG at index 0), then
    // insert the DBC-faithful A_IMMUNITY at its real effect index (1). Mirrors the live import shape.
    for e in ctx.db.game_spell_effect().by_spell().filter(&6788u32) {
        ctx.db.game_spell_effect().id().delete(e.id);
    }
    upsert_effect(
        ctx,
        SpellEffect {
            id: (6788u64 << 2) | 1,
            spell_id: 6788,
            effect_index: 1,
            kind: 0xB1, // A_IMMUNITY
            base_points: 1,
            die_sides: 0,
            per_level: 0.0,
            period_ms: 0,
            target: 2, // T_TARGET_ENEMY (DBC target 2)
            radius_yd: 0.0,
            chain_targets: 0,
            trigger_spell: 0,
            effect_mechanic: 0,
            p0: 19,
            p0_kind: 3, // MECHANIC_SHIELD, P_MECHANIC
            p1: 0,
            script_id: 0,
            enters_combat: false,
        },
    );
    if let Some(mut s) = ctx.db.game_spell().spell_id().find(50072u32) {
        s.duration_ms = 30000; // must be nonzero — 0ms would reap the A_ABSORB instantly
        ctx.db.game_spell().spell_id().update(s);
    } else {
        ctx.db.game_spell().insert(Spell {
            spell_id: 50072,
            name: "Test Power Word: Shield".to_string(),
            power_type: 0,
            cost: 0,
            family_name: 0,
            family_flags: 0,
            cast_time_ms: 0,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 30,
            duration_ms: 30000, // 30s shield (vanilla R1 is longer; a real duration is required — A_ABSORB is an aura, 0ms would reap it instantly)
            school_mask: 2,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: false,
            cast_flags: 0,
            stances: 0,
        });
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find(50072u64 << 2)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: (50072u64 << 2),
                spell_id: 50072,
                effect_index: 0,
                kind: 0xA2, // A_ABSORB
                base_points: 50,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target: 2, // T_TARGET_ALLY
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 2,
                p0_kind: 2, // holy school mask, P_SCHOOL_MASK
                p1: 6788,   // links Weakened Soul — the linked-debuff mechanic under test
                script_id: 0,
                enters_combat: false,
            },
        );
    }
}

/// Hand-seed the Soul Shard item template (real vanilla item 6265). The .import ETL
/// doesn't reliably carry it, so — mirroring `seed_pw_shield_fixture`'s precedent for a mechanic whose
/// live-DBC row isn't available in every dev environment — it's authored here. A plain, non-equippable,
/// non-sellable trade good (vanilla: Soul Shard cannot be sold to a vendor; `sell_price: 0` encodes
/// that), stacking to 20 like the real item. IDEMPOTENT (inserts only if absent), so it's safe from both
/// `init` (fresh install) and `debug_seed_soul_shard_item` (an already-migrated dev DB where `init` did
/// not re-run).
pub(crate) fn seed_soul_shard_item(ctx: &ReducerContext) {
    const SOUL_SHARD: u32 = crate::combat::SOUL_SHARD_ENTRY;
    if ctx
        .db
        .game_item_template()
        .entry()
        .find(SOUL_SHARD)
        .is_some()
    {
        return;
    }
    ctx.db.game_item_template().insert(ItemTemplate {
        entry: SOUL_SHARD,
        class: 7,    // Trade Goods
        subclass: 0, // Trade Goods (generic)
        name: "Soul Shard".to_string(),
        display_id: 1542, // placeholder icon (5875 fixture, like the other hand-authored items above)
        quality: 1,       // Common (white)
        inventory_type: 0, // not equippable
        item_level: 1,
        required_level: 1,
        max_durability: 0,
        buy_price: 0,
        sell_price: 0, // vendors refuse Soul Shards in real vanilla
        max_stack: 20,
        damage_min: 0.0,
        damage_max: 0.0,
        delay_ms: 0,
        stat_strength: 0,
        stat_agility: 0,
        stat_stamina: 0,
        stat_intellect: 0,
        stat_spirit: 0,
        stat_crit: 0,
        stat_hit: 0,
        stat_armor: 0,
        block_value: 0,
        restores_power: false,
        spellid_1: 0,
        spelltrigger_1: 0,
        spellid_2: 0,
        spelltrigger_2: 0,
        container_slots: 0,
        sheath: 0,
        bonding: crate::items::bonding::BIND_ON_PICKUP, // real vanilla Soul Shard: unsellable + BoP
        holy_res: 0,
        fire_res: 0,
        nature_res: 0,
        frost_res: 0,
        shadow_res: 0,
        arcane_res: 0,
        spellid_3: 0,
        spelltrigger_3: 0,
        spellid_4: 0,
        spelltrigger_4: 0,
        spellid_5: 0,
        spelltrigger_5: 0,
        required_skill: 0,
        required_skill_rank: 0,
        required_reputation_faction: 0,
        required_reputation_rank: 0,
        max_count: 0,
        item_flags: 0,
        page_text: 0,
        start_quest: 0,
        bag_family: 0,
        buy_count: 1,
    });
}

/// Mock-seed Drain Soul (real vanilla spell 1120) as a channel headlessly exercisable
/// without a live Spell.dbc (licensing firewall, same precedent as `seed_pw_shield_fixture`'s Test
/// PW:Shield). A single `A_PERIODIC_DAMAGE` effect on the enemy target — the real spell's periodic
/// shadow-damage tick; the real vanilla script effect (`ChannelDeathItem`, the shard-on-kill grant) has
/// no Rust hook of its own here — it's implemented directly in `combat::kill_creature` (an aura naming
/// this spell id, cast by the killer, on the dying creature). 3s cast, 15s duration / 5 ticks (3s each),
/// shadow school. IDEMPOTENT (inserts only if absent).
pub(crate) fn seed_drain_soul_fixture(ctx: &ReducerContext) {
    const DRAIN_SOUL: u32 = crate::combat::DRAIN_SOUL_SPELL_ID;
    if ctx.db.game_spell().spell_id().find(DRAIN_SOUL).is_none() {
        ctx.db.game_spell().insert(Spell {
            spell_id: DRAIN_SOUL,
            name: "Drain Soul".to_string(),
            power_type: 0,
            cost: 0,
            family_name: 0,
            family_flags: 0,
            cast_time_ms: 0,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 30,
            duration_ms: 15000,
            school_mask: 32,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: true,
            cast_flags: 0,
            stances: 0,
        });
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find((DRAIN_SOUL as u64) << 2)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: (DRAIN_SOUL as u64) << 2,
                spell_id: DRAIN_SOUL,
                effect_index: 0,
                kind: 0x90, // A_PERIODIC_DAMAGE
                base_points: 45,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 3000,
                target: 1, // T_TARGET_ENEMY
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 32,
                p0_kind: 2, // shadow school mask, P_SCHOOL_MASK
                p1: 0,
                script_id: 0,
                enters_combat: false,
            },
        );
    }
}

/// Mock-seed Mana Burn (real vanilla spell 8129) as a single `E_POWER_BURN` effect,
/// headlessly exercisable without a live Spell.dbc import (same precedent as `seed_pw_shield_fixture`'s
/// Test PW:Shield / `seed_drain_soul_fixture`). `base_points 100` = drain up to 100 mana (floor-at-
/// available); `p1 50` = the vanilla EffectMultipleValue 0.5 in basis-points, so a full 100-mana drain
/// deals exactly 50 damage. `p0 0 / p0_kind 4 (P_POWER_TYPE)` documents the MANA gate for data parity
/// with the real importer mapping, though the module's E_POWER_BURN handler reads the target's power
/// type straight off `unit_bytes_0` (never p0). Shadow school (32), enemy target. IDEMPOTENT (inserts
/// only if absent).
pub(crate) fn seed_mana_burn_fixture(ctx: &ReducerContext) {
    const MANA_BURN: u32 = 8129;
    if ctx.db.game_spell().spell_id().find(MANA_BURN).is_none() {
        ctx.db.game_spell().insert(Spell {
            spell_id: MANA_BURN,
            name: "Mana Burn".to_string(),
            power_type: 0,
            cost: 0,
            family_name: 0,
            family_flags: 0,
            cast_time_ms: 1500,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 30,
            duration_ms: 0,
            school_mask: 32,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: true,
            cast_flags: 0,
            stances: 0,
        });
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find((MANA_BURN as u64) << 2)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: (MANA_BURN as u64) << 2,
                spell_id: MANA_BURN,
                effect_index: 0,
                kind: 0x19, // E_POWER_BURN
                base_points: 100,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target: 1, // T_TARGET_ENEMY
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 0,
                p0_kind: 4, // MANA, P_POWER_TYPE (documentation only — the handler reads unit_bytes_0)
                p1: 50,
                script_id: 0,
                enters_combat: false, // 50bp = vanilla EffectMultipleValue 0.5
            },
        );
    }
}

/// Mock-seed Stealth (real vanilla spell 1784) as a self-targeted `A_STEALTH` presence
/// marker, headlessly exercisable without a live Spell.dbc import (same precedent as
/// `seed_pw_shield_fixture`'s Test PW:Shield / `seed_drain_soul_fixture`). A single `A_FLAG`-shaped
/// effect carrying `A_STEALTH` as its `kind` — matches the importer's real mapping (`importer/src/
/// spell.rs`: `"Stealth" => A_STEALTH`). `duration_ms: 0` because A_STEALTH is permanent-until-broken
/// (never timer-reaped; see `spell::taxonomy::A_STEALTH` / `scheduler.rs`'s reap-skip). IDEMPOTENT
/// (inserts only if absent).
///
/// Issue #85 audit: until this call was wired into `init` below, this fixture was reachable ONLY via
/// `debug_seed_stealth_fixture` — the exact bug class #85 fixed for the item/faction fixtures, just
/// for `game_spell`/`game_spell_effect` (the `spells` catalogue-fingerprint family) instead. It was
/// masked live only because 1784 is a REAL vanilla id the Spell.dbc importer already seeds on every
/// shard that has imported, so the insert-if-absent guards below silently no-op post-import — but a
/// freshly-published, not-yet-imported shard that had only this debug reducer run against it would
/// diverge from a sibling that didn't, same as the items/faction case. Now called from `init` too, so
/// every fresh shard agrees unconditionally regardless of import order.
pub(crate) fn seed_stealth_fixture(ctx: &ReducerContext) {
    const STEALTH: u32 = 1784;
    if ctx.db.game_spell().spell_id().find(STEALTH).is_none() {
        ctx.db.game_spell().insert(Spell {
            spell_id: STEALTH,
            name: "Stealth".to_string(),
            power_type: 3,
            cost: 0,
            family_name: 0,
            family_flags: 0,
            cast_time_ms: 0,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 0,
            duration_ms: 0,
            school_mask: 1,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: false,
            cast_flags: 0,
            stances: 0,
        });
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find((STEALTH as u64) << 2)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: (STEALTH as u64) << 2,
                spell_id: STEALTH,
                effect_index: 0,
                kind: crate::spell::A_STEALTH,
                base_points: 0,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target: 0, // T_SELF
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 0,
                p0_kind: 7, // P_FLAG
                p1: 0,
                script_id: 0,
                enters_combat: false,
            },
        );
    }
}

/// Mock-seed Chilled (real vanilla spell 6136) + Frost Armor (real vanilla spell 168) —
/// the reactive proc-on-being-hit-in-melee primitive, headlessly exercisable without a live Spell.dbc
/// import (same precedent as `seed_pw_shield_fixture`/`seed_drain_soul_fixture`/`seed_stealth_fixture`).
///
/// Chilled (6136) is the move-slow the proc applies to a melee attacker: ONE `A_MOD_SPEED` effect, p0 =
/// `SPEED_MOVE` (p0_kind `P_SPEED_KIND`), amount −30 (signed percent, matching vanilla Chilled's slow),
/// frost school, 5s duration. It is loaded through `apply_linked_debuff` (the same "apply spell X's aura
/// effects onto Y" machinery PW:Shield's Weakened Soul link already uses) — its OWN `target` field is
/// irrelevant to that path (the caller supplies the target explicitly), so it's left `T_TARGET_ENEMY` for
/// self-documentation / any future direct-cast use.
///
/// Frost Armor (168) mirrors the real DBC shape the importer maps (`importer/src/spell.rs`): eff0 is the
/// `+armor` self-buff (`A_MOD_RESISTANCE`, p0 = `RESIST_ARMOR` bit); eff1 is the reactive chill, classified
/// as `A_PROC_ON_HIT` with `trigger_spell = 6136` — `break_auras_on_damage`'s proc-on-hit scan reads it off
/// any melee-hit unit carrying this aura and applies Chilled onto the ATTACKER. Permanent self-buff
/// (`duration_ms = u32::MAX`, the importer's infinite-aura sentinel — matches vanilla armor spells' -1 DBC
/// duration). IDEMPOTENT (inserts only if absent), mirroring the other mock-seed fixtures.
pub(crate) fn seed_frost_armor_fixture(ctx: &ReducerContext) {
    const CHILLED: u32 = 6136;
    const FROST_ARMOR: u32 = 168;
    if ctx.db.game_spell().spell_id().find(CHILLED).is_none() {
        ctx.db.game_spell().insert(Spell {
            spell_id: CHILLED,
            name: "Chilled".to_string(),
            power_type: 0,
            cost: 0,
            family_name: 0,
            family_flags: 0,
            cast_time_ms: 0,
            gcd_ms: 0,
            cooldown_ms: 0,
            range_yd: 0,
            duration_ms: 5000,
            school_mask: 16,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: true,
            cast_flags: 0,
            stances: 0,
        });
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find((CHILLED as u64) << 2)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: (CHILLED as u64) << 2,
                spell_id: CHILLED,
                effect_index: 0,
                kind: crate::spell::A_MOD_SPEED,
                base_points: -30,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target: 1, // T_TARGET_ENEMY
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 0,
                p0_kind: 6, // SPEED_MOVE, P_SPEED_KIND
                p1: 0,
                script_id: 0,
                enters_combat: false,
            },
        );
    }
    if ctx.db.game_spell().spell_id().find(FROST_ARMOR).is_none() {
        ctx.db.game_spell().insert(Spell {
            spell_id: FROST_ARMOR,
            name: "Frost Armor".to_string(),
            power_type: 0,
            cost: 0,
            family_name: 0,
            family_flags: 0,
            cast_time_ms: 0,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 0,
            duration_ms: u32::MAX, // permanent until replaced/dispelled
            school_mask: 16,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: false,
            cast_flags: 0,
            stances: 0,
        });
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find((FROST_ARMOR as u64) << 2)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: (FROST_ARMOR as u64) << 2,
                spell_id: FROST_ARMOR,
                effect_index: 0,
                kind: crate::spell::A_MOD_RESISTANCE,
                base_points: 150,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target: 0, // T_SELF
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 1,
                p0_kind: 2, // RESIST_ARMOR bit, P_SCHOOL_MASK
                p1: 0,
                script_id: 0,
                enters_combat: false,
            },
        );
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find(((FROST_ARMOR as u64) << 2) | 1)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: ((FROST_ARMOR as u64) << 2) | 1,
                spell_id: FROST_ARMOR,
                effect_index: 1,
                kind: crate::spell::A_PROC_ON_HIT,
                base_points: 0,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target: 0, // T_SELF
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: CHILLED,
                effect_mechanic: 0,
                p0: 0,
                p0_kind: 0, // P_NONE
                p1: 0,
                script_id: 0,
                enters_combat: false,
            },
        );
    }
}

/// Mock-seed Demon Skin (real vanilla spell 696, rank 2) — the COMBAT-INDEPENDENT
/// health-per-5 periodic-tick primitive, headlessly exercisable without a live Spell.dbc import (same
/// precedent as `seed_frost_armor_fixture`/`seed_pw_shield_fixture`/`seed_drain_soul_fixture`).
///
/// Observed vanilla behaviour (cross-checked against the reference cores — a behaviour citation, not a
/// port): aura 84 `SPELL_AURA_MOD_REGEN` ticks on a forced 5000ms period regardless of the DBC's own
/// EffectAmplitude, and heals a LIVING target with no combat gate at all — i.e. it ticks the SAME
/// in-combat or out, unlike the natural spirit-regen pass (out-of-combat-only)
/// or its during-combat-percent cousin `ModRegenDuringCombat`/`A_COMBAT_HEALTH_REGEN_PCT` (implemented
/// separately for Troll Regeneration — a DIFFERENT mechanic, not conflated here). This is
/// exactly the same primitive the engine already runs for Renew/bandages/food (`A_PERIODIC_HEAL`, folded
/// through `tick_auras` with no combat gate), so Demon Skin's eff2 is mock-seeded straight onto it: 5
/// health every 5000ms, matching wowhead classic's "restores 5 Health per 5 sec." tooltip for rank 2.
///
/// eff0 is the existing `+armor` self-buff (`A_MOD_RESISTANCE`, p0 = `RESIST_ARMOR` bit, +120); eff1 is
/// the `A_PERIODIC_HEAL` regen tick. Permanent-for-30-min per the tooltip (`duration_ms = 1_800_000`).
/// IDEMPOTENT (inserts only if absent), mirroring the other mock-seed fixtures.
pub(crate) fn seed_demon_skin_fixture(ctx: &ReducerContext) {
    const DEMON_SKIN: u32 = 696;
    if ctx.db.game_spell().spell_id().find(DEMON_SKIN).is_none() {
        ctx.db.game_spell().insert(Spell {
            spell_id: DEMON_SKIN,
            name: "Demon Skin".to_string(),
            power_type: 0,
            cost: 0,
            family_name: 0,
            family_flags: 0,
            cast_time_ms: 0,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 0,
            duration_ms: 1_800_000, // 30 min
            school_mask: 1,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: false,
            cast_flags: 0,
            stances: 0,
        });
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find((DEMON_SKIN as u64) << 2)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: (DEMON_SKIN as u64) << 2,
                spell_id: DEMON_SKIN,
                effect_index: 0,
                kind: crate::spell::A_MOD_RESISTANCE,
                base_points: 120,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target: 0, // T_SELF
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 1,
                p0_kind: 2, // RESIST_ARMOR bit, P_SCHOOL_MASK
                p1: 0,
                script_id: 0,
                enters_combat: false,
            },
        );
    }
    if ctx
        .db
        .game_spell_effect()
        .id()
        .find(((DEMON_SKIN as u64) << 2) | 1)
        .is_none()
    {
        upsert_effect(
            ctx,
            SpellEffect {
                id: ((DEMON_SKIN as u64) << 2) | 1,
                spell_id: DEMON_SKIN,
                effect_index: 1,
                kind: crate::spell::A_PERIODIC_HEAL,
                base_points: 5,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 5000,
                target: 0, // T_SELF
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 0,
                p0_kind: 0, // P_NONE
                p1: 0,
                script_id: 0,
                enters_combat: false,
            },
        );
    }
}

/// Mock-seed COMBAT-REGEN fixture: Test Regeneration (50137) — the one
/// `A_COMBAT_HEALTH_REGEN_PCT` (0xA9) source on a no-import sandbox. Demon Skin 696's regen effect
/// is `A_PERIODIC_HEAL`, not `A_COMBAT_HEALTH_REGEN_PCT`, so without a dedicated source the combat-
/// regen integration probe (test-combat-regen.sh) finds ZERO kind-169 rows on a fresh node and its
/// `HAS_COMBAT_REGEN_EFFECT` gate skips it forever. This fixture (operator-pick over the Troll
/// racial import — no DBC dependency) keeps the combat-regen-gate mechanic headlessly
/// exercisable: self-buff, 5 min, allows 5% of health_regen_per_tick THROUGH combat.
/// IDEMPOTENT (insert-if-absent), same precedent as every other fixture here.
pub(crate) fn seed_regen_fixture(ctx: &ReducerContext) {
    if ctx.db.game_spell().spell_id().find(50137u32).is_none() {
        ctx.db.game_spell().insert(Spell {
            spell_id: 50137,
            name: "Test Regeneration".to_string(),
            power_type: 0,
            cost: 0,
            family_name: 0,
            family_flags: 0,
            cast_time_ms: 0,
            gcd_ms: 1500,
            cooldown_ms: 0,
            range_yd: 0,
            duration_ms: 300_000,
            school_mask: 8,
            dispel_type: 0,
            mechanic: 0,
            max_stacks: 0,
            aura_interrupt: 0,
            attributes: 0,
            spell_level: 0,
            max_level: 0,
            is_negative: false,
            cast_flags: 0,
            stances: 0,
        });
    }
    let eff_id = (50137u64) << 2;
    if ctx.db.game_spell_effect().id().find(eff_id).is_none() {
        upsert_effect(
            ctx,
            SpellEffect {
                id: eff_id,
                spell_id: 50137,
                effect_index: 0,
                kind: 0xA9, // A_COMBAT_HEALTH_REGEN_PCT
                base_points: 5,
                die_sides: 0,
                per_level: 0.0,
                period_ms: 0,
                target: 0, // self
                radius_yd: 0.0,
                chain_targets: 0,
                trigger_spell: 0,
                effect_mechanic: 0,
                p0: 0,
                p0_kind: 0,
                p1: 0,
                script_id: 0,
                enters_combat: false,
            },
        );
    }
}

/// Scenario-runner mock-seed: everything the four wire scenarios need on a
/// no-import sandbox, insert-if-absent like every other fixture here. Same precedent as
/// `seed_pw_shield_fixture` — call via `debug_seed_scenario_fixtures` post-publish.
///
/// - faction 79 with a real reputation bar (rep index 5) so a quest rep reward lands in
///   `game_player_reputation` (grant_reputation skips bar-less factions).
/// - quest 50900 "Wolf Cull": kill 2x Test Wolf (51000), rewards 150c + 90 XP + 2x Tough Jerky (52)
///   + 250 rep with faction 79. REPEATABLE so suite runs stay green without deleting the log row.
/// - questgiver NPC template 51003 (starts + ends 50900).
/// - vendor/repairer NPC template 51004 selling Tempered Blade (50) + Tough Jerky (52).
/// - trainer offering on the seeded Profession Trainer (51001): Lesser Heal (2050, a seeded 1.5s
///   heal) for 100c at level 1 — the train-and-cast scenario's purchase.
/// - Weapon Master NPC template 51005 ("Woo Ping", work-item 202): a second GOSSIP|TRAINER creature
///   (mirrors the 51004 vendor block) offering 1H Axe (skill line 44, marker 50130, required_level 1,
///   100c) and Polearm (skill line 229, marker 50131, required_level 60, 100c — the level-refusal
///   fixture). Both rows carry `learn_skill_line` set to a COMBAT line, so `apply_trainer_buy` routes
///   them onto the weapon fork (level-derived cap, presence-known) instead of the profession fork;
///   `learn_skill_cap` is irrelevant/ignored on that fork (kept at 0, never read).
// Sole consumer today is the feature-gated harness reducer; see `grant_quest_unchecked` for the
// lint convention (silenced ONLY in default builds).
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
///
/// Reserved fixture ITEM entries (2026-07-16): the scenarios used the mock-seed items 50 (Tempered
/// Blade) and 52 (Tough Jerky), but the world ETL replaces those low entries with whatever real
/// imported items happen to occupy them — the vendor scenario bought a few-copper item where it
/// asserted a 1200c sword, and the quest rewarded something else entirely. Same reserved-id fix as
/// the 509xxxx quest/vendor rows: fixture entries the import never touches.
pub(crate) const FIXTURE_BLADE: u32 = 5090050;
pub(crate) const FIXTURE_JERKY: u32 = 5090052;

/// Insert the two reserved fixture item templates (insert-if-absent) — byte-copies of the
/// mock-seed's Tempered Blade (50) / Tough Jerky (52) under the reserved entries above.
fn seed_fixture_items(ctx: &ReducerContext) {
    let items = ctx.db.game_item_template();
    if items.entry().find(FIXTURE_BLADE).is_none() {
        items.insert(crate::items::ItemTemplate {
            entry: FIXTURE_BLADE,
            class: 2,    // Weapon
            subclass: 7, // Sword (one-hand)
            name: "Tempered Blade".to_string(),
            display_id: 1542,
            quality: 2,         // Uncommon (green)
            inventory_type: 21, // main-hand
            item_level: 12,
            required_level: 1,
            max_durability: 70,
            buy_price: 1200,
            sell_price: 240,
            max_stack: 1,
            damage_min: 8.0,
            damage_max: 12.0,
            delay_ms: 2600,
            bonding: crate::items::bonding::BIND_ON_EQUIP,
            stat_strength: 0,
            stat_agility: 0,
            stat_stamina: 0,
            stat_intellect: 0,
            stat_spirit: 0,
            stat_crit: 0,
            stat_hit: 0,
            stat_armor: 0,
            block_value: 0,
            restores_power: false,
            spellid_1: 0,
            spelltrigger_1: 0,
            spellid_2: 0,
            spelltrigger_2: 0,
            container_slots: 0,
            sheath: 0,
            holy_res: 0,
            fire_res: 0,
            nature_res: 0,
            frost_res: 0,
            shadow_res: 0,
            arcane_res: 0,
            spellid_3: 0,
            spelltrigger_3: 0,
            spellid_4: 0,
            spelltrigger_4: 0,
            spellid_5: 0,
            spelltrigger_5: 0,
            required_skill: 0,
            required_skill_rank: 0,
            required_reputation_faction: 0,
            required_reputation_rank: 0,
            max_count: 0,
            item_flags: 0,
            page_text: 0,
            start_quest: 0,
            bag_family: 0,
            buy_count: 1,
        });
    }
    if items.entry().find(FIXTURE_JERKY).is_none() {
        items.insert(crate::items::ItemTemplate {
            entry: FIXTURE_JERKY,
            class: 0,    // Consumable
            subclass: 0, // Food & Drink
            name: "Tough Jerky".to_string(),
            display_id: 1542,
            quality: 0,
            inventory_type: 0,
            item_level: 1,
            required_level: 1,
            max_durability: 0,
            buy_price: 10,
            sell_price: 2,
            max_stack: 20,
            damage_min: 0.0,
            damage_max: 0.0,
            delay_ms: 0,
            bonding: crate::items::bonding::NONE,
            stat_strength: 0,
            stat_agility: 0,
            stat_stamina: 0,
            stat_intellect: 0,
            stat_spirit: 0,
            stat_crit: 0,
            stat_hit: 0,
            stat_armor: 0,
            block_value: 0,
            restores_power: false,
            spellid_1: 0,
            spelltrigger_1: 0,
            spellid_2: 0,
            spelltrigger_2: 0,
            container_slots: 0,
            sheath: 0,
            holy_res: 0,
            fire_res: 0,
            nature_res: 0,
            frost_res: 0,
            shadow_res: 0,
            arcane_res: 0,
            spellid_3: 0,
            spelltrigger_3: 0,
            spellid_4: 0,
            spelltrigger_4: 0,
            spellid_5: 0,
            spelltrigger_5: 0,
            required_skill: 0,
            required_skill_rank: 0,
            required_reputation_faction: 0,
            required_reputation_rank: 0,
            max_count: 0,
            item_flags: 0,
            page_text: 0,
            start_quest: 0,
            bag_family: 0,
            buy_count: 1,
        });
    }
}

/// Reserved fixture FACTION entry (2026-07-16): SYNTHETIC id — was 79, a REAL Faction.dbc id, so on
/// an imported node the insert-if-absent no-op'd against the real row (reputation_index -1, no bar)
/// and the quest's rep reward silently vanished (grant_reputation skips bar-less factions →
/// scenario-quest's "+250 rep" assert failed). 50900 collides with nothing the DBC ships (ids top
/// out ~1000).
pub(crate) const FIXTURE_FACTION: u32 = 50900;

/// Seed the reserved-id CATALOGUE rows the scenario fixtures reference: the two fixture items
/// (`FIXTURE_BLADE`/`FIXTURE_JERKY`) and the fixture faction above. Split out from
/// `seed_scenario_fixtures` (issue #85) and called from `init` too — same precedent as
/// `seed_pw_shield_fixture` — because these rows land in tables the cross-shard catalogue parity
/// check (#82) fingerprints whole (`game_item_template`, `game_faction`): before this, only a shard
/// that had `debug_seed_scenario_fixtures` run against it (historically the wire-suite's target,
/// lyracore) carried them, so its `items`/`dbc_reference` fingerprints permanently disagreed
/// with siblings that never ran the harness reducer — a false catalogue-skew signal, not a real one.
/// Calling this from `init` makes every freshly published shard agree unconditionally, matching how
/// `seed_pw_shield_fixture` already keeps `spells` in agreement. Idempotent (insert-if-absent), so
/// the repeat call from `debug_seed_scenario_fixtures` below is a no-op once `init` has run.
pub(crate) fn seed_fixture_catalogue(ctx: &ReducerContext) {
    seed_fixture_items(ctx);
    if ctx
        .db
        .game_faction()
        .faction_id()
        .find(FIXTURE_FACTION)
        .is_none()
    {
        ctx.db.game_faction().insert(Faction {
            faction_id: FIXTURE_FACTION,
            // Slot 60: the real import claims indices 0..=54 of the client's 64-entry rep array
            // (danger-zones §1.4) — 60 stays clear of both the import and the array bound.
            reputation_index: 60,
            base_standing: 0,
        });
    }
}

// Sole consumer is `debug::debug_seed_scenario_fixtures`, so a build WITHOUT `debug_reducers` (a
// production publish, or a `cargo clippy` that does not unify the module's features) sees this as
// dead. Same convention as `FIXTURE_BLADE` above: silenced ONLY in the builds where it really is
// unreachable, never unconditionally.
#[cfg_attr(not(feature = "debug_reducers"), allow(dead_code))]
pub(crate) fn seed_scenario_fixtures(ctx: &ReducerContext) {
    use crate::quest::quest_role;

    use crate::{
        game_creature_quest, game_npc_vendor, game_quest_objective, game_quest_reward_item,
        game_quest_template, game_quest_text, game_trainer_spell,
    };

    // Reserved fixture items + faction first — the quest reward/rep/vendor stock below reference
    // them. Also called unconditionally from `init` now (see `seed_fixture_catalogue`'s doc); this
    // call stays so an already-migrated dev DB that only ever runs the debug reducer still gets them.
    seed_fixture_catalogue(ctx);

    const QUEST: u32 = 50900;
    const QUESTGIVER: u32 = 51003;
    const VENDOR: u32 = 51004;
    const WOLF: u32 = 51000;
    if ctx.db.game_quest_template().entry().find(QUEST).is_none() {
        ctx.db.game_quest_template().insert(crate::QuestTemplate {
            entry: QUEST,
            min_level: 0,
            quest_level: 2,
            title: "Wolf Cull".to_string(),
            reward_money: 150,
            reward_xp: 90,
            prev_quest_id: 0,
            required_races: 0,
            required_classes: 0,
            zone_or_sort: 12,
            rew_rep_faction_1: FIXTURE_FACTION,
            rew_rep_value_1: 250,
            rew_rep_faction_2: 0,
            rew_rep_value_2: 0,
            src_item: 0,
            src_item_count: 0,
            repeatable: true,
            next_quest_id: 0,
            limit_time: 0,
            reward_money_max_level: 0, // fixture sets reward_xp explicitly, so this is unused here
        });
        ctx.db.game_quest_text().insert(crate::QuestText {
            quest_entry: QUEST,
            details: "The test wolves multiply. Cull two of them.".to_string(),
            objectives: "Kill 2 Test Wolves.".to_string(),
            offer_reward_text: "The pack thins. Well done.".to_string(),
            request_items_text: "Are the wolves culled?".to_string(),
        });
        // EXPLICIT reserved ids (not the auto_inc 0 sentinel): the world ETL imports these quest
        // tables with explicit dump ids, leaving the table's sequence allocator BEHIND the data —
        // an id-0 insert then allocates an id that already exists and PANICS (errno 12; the
        // fixture-seed rollback found live 2026-07-15). Fixed ids in the 509xx fixture range are
        // idempotent with the delete below and can never collide with dump rows.
        ctx.db.game_quest_objective().id().delete(5090000u64);
        ctx.db.game_quest_objective().insert(crate::QuestObjective {
            id: 5090000,
            quest_entry: QUEST,
            obj_index: 0,
            kind: crate::quest::objective_kind::KILL_CREATURE,
            target_entry: WOLF,
            required_count: 2,
        });
        ctx.db.game_quest_reward_item().id().delete(5090001u64);
        ctx.db
            .game_quest_reward_item()
            .insert(crate::QuestRewardItem {
                id: 5090001,
                quest_entry: QUEST,
                item_entry: FIXTURE_JERKY, // reserved fixture Tough Jerky (see seed_fixture_items)
                count: 2,
            });
        ctx.db.game_creature_quest().id().delete(5090002u64);
        ctx.db.game_creature_quest().insert(crate::CreatureQuest {
            id: 5090002,
            creature_entry: QUESTGIVER,
            quest_entry: QUEST,
            role: quest_role::START,
        });
        ctx.db.game_creature_quest().id().delete(5090003u64);
        ctx.db.game_creature_quest().insert(crate::CreatureQuest {
            id: 5090003,
            creature_entry: QUESTGIVER,
            quest_entry: QUEST,
            role: quest_role::END,
        });
    }

    // The quest-loop's LOOT step needs a coin window: give the init-seeded Test Wolf pocket change
    // if it has none yet (converges to the same values; kill-time money rolls read the template).
    let templates = ctx.db.game_creature_template();
    if let Some(mut wolf) = templates.entry().find(WOLF) {
        if wolf.money_max == 0 {
            wolf.money_min = 25;
            wolf.money_max = 50;
            templates.entry().update(wolf);
        }
    }
    // 060/187 recurring trap: the world ETL truncates game_creature_template and reloads from the
    // dump — the INIT-seeded fixture templates (Test Wolf 51000, Profession Trainer 51001) vanish
    // on every re-import, breaking the wire scenarios until someone reseeds by hand. Re-seed them
    // HERE (this reducer is the operator's idempotent post-import fixture restore).
    if templates.entry().find(WOLF).is_none() {
        templates.insert(CreatureTemplate {
            entry: WOLF,
            name: "Test Wolf".to_string(),
            subname: String::new(),
            display_id: 720,
            level: 1,
            health: 60,
            faction_template: 14, // Monster (hostile - a usable kill target)
            npc_flags: 0,
            unit_flags: 0,
            creature_type: 1,   // BEAST (skinnable)
            creature_family: 1, // Wolf
            type_flags: 0x100,  // SKINNABLE
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 25,
            money_max: 50,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0, // passive: engages only when attacked
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0,
            skin_loot_id: 0,
        });
    }
    const PROFESSION_TRAINER: u32 = 51001;
    if templates.entry().find(PROFESSION_TRAINER).is_none() {
        templates.insert(CreatureTemplate {
            entry: PROFESSION_TRAINER,
            name: "Profession Trainer".to_string(),
            subname: "Fixture".to_string(),
            display_id: 3167,
            level: 10,
            health: 100,
            faction_template: 35, // FRIENDLY trainer
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
                | lyracore_shared::constants::npc_flags::TRAINER,
            unit_flags: 0,
            creature_type: 7, // Humanoid
            creature_family: 0,
            type_flags: 0,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 0,
            money_max: 0,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0,
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0,
            skin_loot_id: 0,
        });
    }
    // "Test Wolf Elder" (51002) — the BOT-SUITE fight fixture (266). The playerbots tests level
    // their bots to clear the cast level-gate (Taunt 355 = spell_level 10), which greys the L1
    // Test Wolf 51000 (aggro_radius returns 0 at a >=8 level gap) — so its wolves stop aggroing and
    // the tank has nothing to Taunt. This one is level 9: non-grey to a level-10 bot (20-level gap
    // rule), so it proximity-aggros and pays real kill XP. SEPARATE id keeps the scenario_quest
    // "grey L1 wolf pays 0 kill XP" fixture (51000) untouched — the done-when's hard constraint.
    const WOLF_ELDER: u32 = 51002;
    if templates.entry().find(WOLF_ELDER).is_none() {
        templates.insert(CreatureTemplate {
            entry: WOLF_ELDER,
            name: "Test Wolf Elder".to_string(),
            subname: String::new(),
            display_id: 720,
            level: 8, // diff 2 vs an L10 bot — inside the goals.rs GRIND ±3 band, non-grey
            health: 300, // survives a 1s top-up window vs an L10 trio's burst; solo-killable in ~15s
            faction_template: 14, // Monster (hostile — same as 51000)
            npc_flags: 0,
            unit_flags: 0,
            creature_type: 1, // BEAST (flee_eligible false — never routs mid-fight)
            creature_family: 1, // Wolf
            type_flags: 0x100, // SKINNABLE
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 25,
            money_max: 50,
            max_level: 0,
            max_level_health: 0,
            // EXPLICIT 20yd (aggro_radius returns an override verbatim, beating the grey rule) —
            // so proximity aggro survives ANY future bot level, not just level 10.
            aggro_range: 20,
            damage_min: 2, // low on purpose: a solo bot-goals bot survives 3 grind kills healer-less
            damage_max: 4,
            armor: 0,
            pickpocket_loot_id: 0,
            skin_loot_id: 0,
        });
    }
    if templates.entry().find(QUESTGIVER).is_none() {
        templates.insert(CreatureTemplate {
            entry: QUESTGIVER,
            name: "Scenario Questgiver".to_string(),
            subname: "Wolf Cull".to_string(),
            display_id: 3167,
            level: 10,
            health: 500,
            faction_template: 35, // FRIENDLY
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP | 0x2, // 0x2 = UNIT_NPC_FLAG_QUESTGIVER (1.12)
            unit_flags: 0,
            creature_type: 7, // Humanoid
            creature_family: 0,
            type_flags: 0,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 0,
            money_max: 0,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0,
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0, // not imported — a Humanoid questgiver has no pickpocket table
            skin_loot_id: 0,       // not imported — a Humanoid questgiver isn't skinnable anyway
        });
    }
    if templates.entry().find(VENDOR).is_none() {
        templates.insert(CreatureTemplate {
            entry: VENDOR,
            name: "Scenario Vendor".to_string(),
            subname: "Blades & Repairs".to_string(),
            display_id: 3167,
            level: 10,
            health: 500,
            faction_template: 35,
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
                | lyracore_shared::constants::npc_flags::VENDOR
                | lyracore_shared::constants::npc_flags::REPAIR,
            unit_flags: 0,
            creature_type: 7,
            creature_family: 0,
            type_flags: 0,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 0,
            money_max: 0,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0,
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0, // not imported — a Humanoid vendor has no pickpocket table
            skin_loot_id: 0,       // not imported — a Humanoid vendor isn't skinnable anyway
        });
    }
    let vendor_rows = ctx.db.game_npc_vendor();
    // Explicit reserved ids (same errno-12 sequence-desync fix as the quest rows above: the ETL
    // imports vendor/trainer rows with explicit ids, leaving the sequence behind the data).
    if !vendor_rows
        .by_vendor()
        .filter(&VENDOR)
        .any(|r| r.item_entry == FIXTURE_BLADE)
    {
        vendor_rows.id().delete(5090010u64);
        vendor_rows.insert(crate::NpcVendor {
            id: 5090010,
            creature_entry: VENDOR,
            item_entry: FIXTURE_BLADE,
            slot: 0,
            max_count: 0,
        });
    }
    if !vendor_rows
        .by_vendor()
        .filter(&VENDOR)
        .any(|r| r.item_entry == FIXTURE_JERKY)
    {
        vendor_rows.id().delete(5090011u64);
        vendor_rows.insert(crate::NpcVendor {
            id: 5090011,
            creature_entry: VENDOR,
            item_entry: FIXTURE_JERKY,
            slot: 1,
            max_count: 0,
        });
    }

    const TRAINER: u32 = 51001; // the init-seeded Profession Trainer (already GOSSIP|TRAINER)
    const LESSER_HEAL: u32 = 2050;
    let offerings = ctx.db.game_trainer_spell();
    if !offerings
        .by_trainer()
        .filter(&TRAINER)
        .any(|r| r.spell_id == LESSER_HEAL)
    {
        offerings.id().delete(5090012u64);
        offerings.insert(crate::TrainerSpell {
            id: 5090012,
            trainer_entry: TRAINER,
            spell_id: LESSER_HEAL,
            cost: 100,
            required_level: 1,
            learn_skill_line: 0,
            learn_skill_cap: 75,
        });
    }

    // --- WEAPON MASTER (work-item 202): "Woo Ping" (51005) sells weapon proficiencies for gold —
    // the vanilla weapon-master shape (a trainer-list row whose `learn_skill_line` names a weapon line
    // instead of a spell/profession). Mirrors the 51004 vendor block: GOSSIP|TRAINER, faction 35
    // (FRIENDLY, never a kill target).
    const WEAPON_MASTER: u32 = 51005;
    // Marker spell ids for the weapon-learn offerings — NEVER resolved as real spells (no `game_spell`
    // header/effects), same convention as the profession markers in `skill.rs`; MUST match
    // `skill::LEARN_AXE_1H_SPELL_ID` / `skill::LEARN_POLEARM_SPELL_ID` (the debug reducer's twin lookup).
    const LEARN_AXE_1H: u32 = 50130; // -> learn_skill_line = AXE_1H (44)
    const LEARN_POLEARM: u32 = 50131; // -> learn_skill_line = POLEARM (229)
    if templates.entry().find(WEAPON_MASTER).is_none() {
        templates.insert(CreatureTemplate {
            entry: WEAPON_MASTER,
            name: "Woo Ping".to_string(),
            subname: "Weapon Master".to_string(),
            display_id: 3167,
            level: 30,
            health: 1500,
            faction_template: 35, // FRIENDLY (a trainer, not a kill target)
            npc_flags: lyracore_shared::constants::npc_flags::GOSSIP
                | lyracore_shared::constants::npc_flags::TRAINER,
            unit_flags: 0,
            creature_type: 7, // Humanoid
            creature_family: 0,
            type_flags: 0,
            rank: 0,
            scale: 1.0,
            base_attack_time_ms: 2000,
            money_min: 0,
            money_max: 0,
            max_level: 0,
            max_level_health: 0,
            aggro_range: 0, // never aggros (friendly trainer)
            damage_min: 0,
            damage_max: 0,
            armor: 0,
            pickpocket_loot_id: 0, // not imported — a friendly weapon master has no pickpocket table
            skin_loot_id: 0,       // not imported — a Humanoid trainer isn't skinnable anyway
        });
    }
    if !offerings
        .by_trainer()
        .filter(&WEAPON_MASTER)
        .any(|r| r.spell_id == LEARN_AXE_1H)
    {
        offerings.id().delete(5090013u64);
        offerings.insert(crate::TrainerSpell {
            id: 5090013,
            trainer_entry: WEAPON_MASTER,
            spell_id: LEARN_AXE_1H,
            cost: 100,
            required_level: 1,
            learn_skill_line: crate::skill::skill_line::AXE_1H,
            learn_skill_cap: 0, // ignored on the weapon fork (cap is level-derived)
        });
    }
    if !offerings
        .by_trainer()
        .filter(&WEAPON_MASTER)
        .any(|r| r.spell_id == LEARN_POLEARM)
    {
        offerings.id().delete(5090014u64);
        offerings.insert(crate::TrainerSpell {
            id: 5090014,
            trainer_entry: WEAPON_MASTER,
            spell_id: LEARN_POLEARM,
            cost: 100,
            required_level: 60, // the level-refusal fixture (Ginger's default level is well below 60)
            learn_skill_line: crate::skill::skill_line::POLEARM,
            learn_skill_cap: 0,
        });
    }
}

/// Idempotent fixture effect write: `SpellEffect.id` is a DETERMINISTIC PK
/// `(spell_id<<2)|effect_index` (NOT auto_inc), so a plain `insert` PANICS (errno 12,
/// unique-exists) whenever the curated importer has already written the same effect row — a
/// re-imported kit + a fixture re-seed collided live 2026-07-15. Delete-then-insert keeps the
/// fixture authoritative for its own rows without tripping the constraint.
fn upsert_effect(ctx: &spacetimedb::ReducerContext, row: SpellEffect) {
    ctx.db.game_spell_effect().id().delete(row.id);
    ctx.db.game_spell_effect().insert(row);
}
