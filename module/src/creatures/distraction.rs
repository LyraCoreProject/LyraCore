//! The Distraction: a Creature faces one ground point and holds its idle movement until the expiry.
//!
//! Only the creature behavior cycle reads it, through `IdleSink`: patrol and wander hold while it is
//! active, and after the expiry the cycle plays one facing back to the spawn orientation and ends it.
//! A new Engagement, death and despawn end it at once.

use lyracore_shared::constants::unit_flags;
use spacetimedb::{table, ReducerContext, Table};

use super::tick;
use crate::{game_creature_spawn, game_creature_spline, game_world_entity};

#[table(accessor = game_creature_distraction)]
pub struct CreatureDistraction {
    #[primary_key]
    pub creature_guid: u64,
    /// Epoch milliseconds. Not a `Timestamp`, so a durable test can move it with plain SQL.
    pub ends_ms: u64,
}

/// The facts that decide whether a unit reacts to a Distract.
#[derive(Clone, Copy)]
struct Candidate {
    player: bool,
    dead: bool,
    in_combat: bool,
    engaged: bool,
    action_blocked: bool,
}

impl Candidate {
    /// Only an idle Creature that can act turns. A fight or crowd control already owns its movement.
    fn may_be_distracted(self) -> bool {
        !(self.player || self.dead || self.in_combat || self.engaged || self.action_blocked)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Phase {
    Active,
    Expired,
}

fn phase(ends_ms: u64, now_ms: u64) -> Phase {
    if now_ms < ends_ms {
        Phase::Active
    } else {
        Phase::Expired
    }
}

/// The orientation that faces `to` from `from`, in the client's radians.
fn heading(from: (f32, f32), to: (f32, f32)) -> f32 {
    (to.1 - from.1).atan2(to.0 - from.0)
}

/// The effect amount is the length in seconds. A non-positive amount distracts nobody.
fn length_ms(seconds: i32) -> Option<u64> {
    u64::try_from(seconds)
        .ok()
        .filter(|&s| s > 0)
        .map(|s| s * 1_000)
}

fn now_ms(ctx: &ReducerContext) -> u64 {
    (ctx.timestamp.to_micros_since_unix_epoch() / 1_000) as u64
}

/// Start or refresh a Distraction on `creature_guid`: stop it where the client renders it, face
/// `dest`, and hold for `seconds`. An ineligible unit is left unchanged.
pub(crate) fn distract(
    ctx: &ReducerContext,
    creature_guid: u64,
    dest: (f32, f32, f32),
    seconds: i32,
) {
    let Some(length_ms) = length_ms(seconds) else {
        return;
    };
    let entities = ctx.db.game_world_entity();
    let Some(mut creature) = entities.guid().find(creature_guid) else {
        return;
    };
    let candidate = Candidate {
        player: creature.is_player(),
        dead: creature.dead,
        in_combat: creature.unit_flags & unit_flags::IN_COMBAT != 0,
        engaged: crate::combat::is_engaged(ctx, creature_guid),
        action_blocked: crate::spell::is_action_blocked(ctx, creature_guid),
    };
    if !candidate.may_be_distracted() {
        return;
    }
    tick::stop_where_rendered(ctx, &mut creature);
    // A stopped leg is over; the patrol must not wait for its old ETA after the expiry.
    creature.leg_ends_ms = 0;
    creature.orientation = heading((creature.x, creature.y), (dest.0, dest.1));
    let replaced = ctx
        .db
        .game_creature_spline()
        .guid()
        .find(creature_guid)
        .map_or(0, |leg| leg.spline_id);
    tick::emit_facing_spline(
        ctx,
        creature_guid,
        (creature.x, creature.y, creature.z),
        creature.orientation,
        tick::next_spline_id(ctx.timestamp.to_micros_since_unix_epoch() as u64, replaced),
        creature.map_id,
        creature.instance_id,
        (creature.grid_x, creature.grid_y),
    );
    entities.guid().update(creature);
    let row = CreatureDistraction {
        creature_guid,
        ends_ms: now_ms(ctx) + length_ms,
    };
    let table = ctx.db.game_creature_distraction();
    if table.creature_guid().find(creature_guid).is_some() {
        table.creature_guid().update(row);
    } else {
        table.insert(row);
    }
}

/// Is `guid` distracted now? Patrol and wander hold while this is true.
pub(crate) fn active(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_creature_distraction()
        .creature_guid()
        .find(guid)
        .is_some_and(|row| phase(row.ends_ms, now_ms(ctx)) == Phase::Active)
}

/// The orientation to turn back to once the Distraction has expired: the spawn orientation, or the
/// current one for a Creature without a spawn row. `None` while it is active or absent.
pub(crate) fn expired_facing(ctx: &ReducerContext, guid: u64) -> Option<f32> {
    let row = ctx
        .db
        .game_creature_distraction()
        .creature_guid()
        .find(guid)?;
    if phase(row.ends_ms, now_ms(ctx)) == Phase::Active {
        return None;
    }
    ctx.db
        .game_creature_spawn()
        .guid()
        .find(guid)
        .map(|spawn| spawn.orientation)
        .or_else(|| {
            ctx.db
                .game_world_entity()
                .guid()
                .find(guid)
                .map(|creature| creature.orientation)
        })
}

/// End an expired Distraction after its turn back has played. An active one is kept.
pub(crate) fn end_expired(ctx: &ReducerContext, guid: u64) {
    if expired_facing(ctx, guid).is_some() {
        clear(ctx, guid);
    }
}

/// End the Distraction now: a new Engagement, death or despawn.
pub(crate) fn clear(ctx: &ReducerContext, guid: u64) {
    ctx.db
        .game_creature_distraction()
        .creature_guid()
        .delete(guid);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn heading_faces_the_ground_point() {
        assert!((heading((0.0, 0.0), (10.0, 0.0)) - 0.0).abs() < 1e-6);
        assert!((heading((0.0, 0.0), (0.0, 10.0)) - PI / 2.0).abs() < 1e-6);
        assert!((heading((0.0, 0.0), (-10.0, 0.0)) - PI).abs() < 1e-6);
    }

    #[test]
    fn the_effect_amount_is_the_length_in_seconds() {
        assert_eq!(length_ms(10), Some(10_000));
        assert_eq!(length_ms(0), None);
        assert_eq!(length_ms(-5), None);
    }

    #[test]
    fn only_an_idle_creature_that_can_act_is_distracted() {
        let idle = Candidate {
            player: false,
            dead: false,
            in_combat: false,
            engaged: false,
            action_blocked: false,
        };
        assert!(idle.may_be_distracted());
        for ignored in [
            Candidate {
                player: true,
                ..idle
            },
            Candidate { dead: true, ..idle },
            Candidate {
                in_combat: true,
                ..idle
            },
            Candidate {
                engaged: true,
                ..idle
            },
            Candidate {
                action_blocked: true,
                ..idle
            },
        ] {
            assert!(!ignored.may_be_distracted());
        }
    }

    #[test]
    fn a_distraction_is_active_until_its_expiry() {
        assert_eq!(phase(10_000, 9_999), Phase::Active);
        assert_eq!(phase(10_000, 10_000), Phase::Expired);
        assert_eq!(phase(10_000, 12_000), Phase::Expired);
    }
}
