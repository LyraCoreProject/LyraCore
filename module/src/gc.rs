//! Scheduled garbage collection: the event reaper that GCs short-lived broadcast/relay event rows
//! (movement relays + creature-move legs) once their TTL elapses. [server]

use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table};

use crate::{
    game_addon_message, game_bot_invite_intent, game_channel_event, game_chat_event,
    game_combat_event, game_duel_event, game_emote_event, game_group_event, game_group_invite,
    game_levelup_event, game_movement_violation, game_roll_event, game_spell_cast_event,
    game_spell_impact_event, game_teleport_event, game_trade_event, game_trade_session,
    game_whisper_event, game_xp_event, EVENT_TTL_MICROS, INVITE_TTL_MICROS,
};
use crate::breath_relay::game_breath_relay_event;
// `rest` isn't re-exported at crate scope (`mod rest;`, no `pub use rest::*;` in lib.rs) — every
// other event table's accessor trait rides that glob, so this is the one accessor here needing its
// own import.
use crate::rest::game_rest_state_event;
// `game_movement_event` / `game_creature_move_event` are deliberately NOT imported: nothing writes
// either table any more, so the reaper no longer touches them (see the notes in `reap_events`).

/// Drives the movement-event GC tick. [server]
#[table(accessor = game_event_reaper_schedule, scheduled(reap_movement_events))]
pub struct EventReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

// NO per-firing delete cap here — perf catalog 1.5 tier 1 is PARKED, not landed. A flat cap
// (a `.take(N)` on the stale list) bounds the transaction but SILENTLY DROPS the reaper's drain
// guarantee: the reaper fires 1×/s over a 1s TTL, so it drains only while production ≤ N rows/s
// per table. The catalog's own arithmetic puts movement-event production at ~20k rows/s at C≈100 —
// four times a 5k cap — i.e. the cap starts leaking at exactly the crowd size the rest of this
// catalog exists to reach, and `game_movement_event` (public, per-player subscribed) then grows
// without bound. Bounding the stall needs a mechanism that still drains (catch-up firings, or the
// tier-3 restructure that stops modeling movement as insert+delete rows); a bare cap is a
// behaviour change dressed as a perf win.

#[reducer]
pub fn reap_movement_events(ctx: &ReducerContext, _schedule: EventReaperSchedule) {
    // Only the scheduler should invoke this.
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let cutoff = ctx.timestamp.to_micros_since_unix_epoch() - EVENT_TTL_MICROS;

    // All three event tables share the same shape (`id: u64` PK + `created_at: Timestamp`) and TTL,
    // so one macro reaps each: collect stale ids (created_at < cutoff), then delete by id. A generic
    // `fn` is awkward here — each table's `.id()` is a distinct generated accessor type — so a local
    // macro keyed on the table accessor is the direct fit.
    macro_rules! reap {
        ($accessor:ident) => {{
            let t = ctx.db.$accessor();
            let stale: Vec<u64> = t
                .iter()
                .filter(|e| e.created_at.to_micros_since_unix_epoch() < cutoff)
                .map(|e| e.id)
                .collect();
            for id in stale {
                t.id().delete(&id);
            }
        }};
    }
    // `game_movement_event` is GONE (dropped — #350): peer movement rides the per-mover
    // `game_entity_motion` row, which is updated in place and needs no reaping at all (perf catalog
    // 2.1). This reap was the second half of the O(C²) cost: at 200 co-located players it deleted
    // 67,753 rows/s to match the 70,568 the writer had just inserted.
    // `game_creature_move_event` is NO LONGER REAPED because nothing writes it any more — creature
    // legs ride the per-creature `game_creature_spline` row, which is updated in place and needs no
    // reaping (same move as perf catalog 2.1 made for player movement). This reap was deleting
    // 121.6 rows/s at 100 dispersed players to match the inserts. The table stays in the schema
    // (dropping one is a separate destructive migration) and stays empty.
    reap!(game_combat_event); // melee swing logs
    reap!(game_xp_event); // per-kill XP awards
    reap!(game_levelup_event); // level-up dings
    reap!(game_spell_cast_event); // spell-cast visuals (aura tracer)
    reap!(game_spell_impact_event); // deferred projectile-impact damage logs
    reap!(game_chat_event); // say/yell broadcast lines
    reap!(game_channel_event); // channel lines (065)
    reap!(game_emote_event); // emote broadcast (text + animation)
    reap!(game_whisper_event); // private whisper lines (RLS-scoped)
    reap!(game_addon_message); // addon-bridge UI messages (184, RLS-scoped)
    reap!(game_roll_event); // /roll broadcast results
    reap!(game_group_event); // group invite/roster notifications (RLS-scoped)
    reap!(game_trade_event); // trade-status relay rows (#120, RLS-scoped)
    reap!(game_duel_event); // Duel lifecycle relay rows (RLS-scoped)
    reap!(game_bot_invite_intent); // bot-decided invites awaiting gateway pickup (issue #54)
    reap!(game_movement_violation); // recent anti-cheat diagnostics (issue #211)
                                    // Rest-area zzz/blue-bar relay rows (196). Caught missing by the #379 gc_reap_tripwire: this
                                    // table carries the same `id: u64` + `created_at: Timestamp` TTL shape as every table above but
                                    // had no reap line — every inn threshold crossing for the lifetime of a character left one more
                                    // row behind. The durable rest state (`Character.resting`/`rested_xp`) lives elsewhere; this row
                                    // is only the one-shot PLAYER_BYTES_2 relay.
    reap!(game_rest_state_event);
    reap!(game_breath_relay_event); // breath timer edges + drowning damage relay (#141)

    // Never-answered pending invites. Same id+created_at shape as the event tables, but on
    // the longer INVITE_TTL (a human is looking at the invite dialog). After that the row is dead
    // weight: accepting a reaped invite fails as "no pending invite", exactly like accepting one
    // that was never sent.
    {
        let t = ctx.db.game_group_invite();
        let invite_cutoff = ctx.timestamp.to_micros_since_unix_epoch() - INVITE_TTL_MICROS;
        let stale: Vec<u64> = t
            .iter()
            .filter(|e| e.created_at.to_micros_since_unix_epoch() < invite_cutoff)
            .map(|e| e.id)
            .collect();
        for id in stale {
            t.id().delete(id);
        }
    }

    // Idle Trade Sessions (#123): `created_at` is bumped by every trade action, so the pure
    // policy (`trade::session_is_stale`) measures idleness — a live negotiation is never reaped.
    // Torn down THROUGH `cancel_trade_for`, never a bare delete, so both clients hear
    // `TradeCanceled` instead of keeping stale windows open.
    {
        let sessions = ctx.db.game_trade_session();
        let now = ctx.timestamp.to_micros_since_unix_epoch();
        let stale: Vec<u64> = sessions
            .iter()
            .filter(|s| crate::trade::session_is_stale(s.created_at.to_micros_since_unix_epoch(), now))
            .map(|s| s.initiator_guid)
            .collect();
        for guid in stale {
            crate::trade::cancel_trade_for(ctx, guid);
        }
    }

    // CC diminishing-returns windows (work-item 192): rows whose 15s post-removal window has
    // elapsed are dead state — the next same-category CC starts fresh at level 1 anyway.
    crate::spell::stacking::sweep_dr_state(ctx);

    // Need/greed roll deadlines (work-item 187): resolves any roll whose 60s window lapsed with
    // votes outstanding (absent members auto-pass) — the deadline half of resolve-exactly-once.
    crate::loot::sweep_loot_rolls(ctx);

    // Teleport handshake relays. Same TTL, but the time column is a defaulted u64 (`created_micros`,
    // not a `Timestamp` — so the column-add auto-migrated), so it can't use the `reap!` macro.
    {
        let t = ctx.db.game_teleport_event();
        let stale: Vec<u64> = t
            .iter()
            .filter(|e| (e.created_micros as i64) < cutoff)
            .map(|e| e.id)
            .collect();
        for id in stale {
            t.id().delete(id);
        }
    }

    // Player corpses: unreclaimed body → bones → despawn, on its own decay policy (own the bones,
    // not this reaper). See `corpse::sweep_corpse_decay`'s doc for the timers and the relay shape.
    crate::corpse::sweep_corpse_decay(ctx);
}

#[cfg(test)]
mod tests {
    use crate::test_scan::code_of;

    #[test]
    fn movement_violation_rows_are_ttl_reaped() {
        let body = code_of(include_str!("gc.rs"), "pub fn reap_movement_events(");
        let reaper_call = ["reap!(", "game_movement_violation", ");"].concat();
        assert!(
            body.contains(&reaper_call),
            "the shared event reaper must delete expired movement-violation diagnostics"
        );
    }

    #[test]
    fn lapsed_cc_diminishing_return_rows_are_reaped() {
        let body = code_of(include_str!("gc.rs"), "pub fn reap_movement_events(");
        assert!(
            body.contains("stacking::sweep_dr_state(ctx)"),
            "the shared event reaper must drop lapsed crowd-control diminishing-return rows"
        );
    }
}
