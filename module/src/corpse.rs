use spacetimedb::{table, ReducerContext, Table, Timestamp};

use lyracore_shared::packing::unpack4;

pub const HIGHGUID_CORPSE: u64 = 0xF101;

pub(crate) fn corpse_guid_for(owner_guid: u64) -> u64 {
    (HIGHGUID_CORPSE << 48) | (owner_guid & 0x0000_FFFF_FFFF_FFFF)
}

const RECLAIM_RADIUS_SQ: f32 = 1521.0;

pub const CORPSE_RECLAIM_DELAY_SECS: [i64; 3] = [30, 60, 120];

const STREAK_RUNG_MICROS: i64 = 300 * 1_000_000;

const MAX_STREAK_RUNGS: i64 = CORPSE_RECLAIM_DELAY_SECS.len() as i64;

// Corpse and bone lifetimes use chosen values; vanilla decay timing is unverified.
pub(crate) const CORPSE_DECAY_MICROS: i64 = 300 * 1_000_000;

pub(crate) const BONES_DECAY_MICROS: i64 = 300 * 1_000_000;

pub(crate) fn escalated_reclaim(streak_deadline_micros: i64, now_micros: i64) -> (i64, i64) {
    let credit_remaining = streak_deadline_micros - now_micros;
    let deadline = if credit_remaining <= 0 {
        now_micros + STREAK_RUNG_MICROS
    } else if credit_remaining / STREAK_RUNG_MICROS < MAX_STREAK_RUNGS {
        streak_deadline_micros + STREAK_RUNG_MICROS
    } else {
        now_micros + MAX_STREAK_RUNGS * STREAK_RUNG_MICROS
    };
    let rung = ((deadline - now_micros + STREAK_RUNG_MICROS - 1) / STREAK_RUNG_MICROS)
        .clamp(1, MAX_STREAK_RUNGS);
    (
        deadline,
        CORPSE_RECLAIM_DELAY_SECS[rung as usize - 1] * 1_000_000,
    )
}

// CORPSE_FIELD_BYTES_1 packs race<<8, gender<<16 and skin<<24, unlike PLAYER_BYTES.
pub(crate) fn corpse_appearance_bytes(
    unit_bytes_0: u32,
    player_bytes: u32,
    player_bytes_2: u32,
) -> (u32, u32) {
    let (race, _, gender, _) = unpack4(unit_bytes_0);
    let (skin, face, hairstyle, haircolor) = unpack4(player_bytes);
    let (facialhair, _, _, _) = unpack4(player_bytes_2);
    let (race, gender, skin) = (race as u32, gender as u32, skin as u32);
    let (face, hairstyle, haircolor, facialhair) = (
        face as u32,
        hairstyle as u32,
        haircolor as u32,
        facialhair as u32,
    );
    let bytes_1 = (race << 8) | (gender << 16) | (skin << 24);
    let bytes_2 = face | (hairstyle << 8) | (haircolor << 16) | (facialhair << 24);
    (bytes_1, bytes_2)
}

#[table(accessor = game_corpse, public,
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell])),
    index(accessor = by_instance, btree(columns = [instance_id])),
    index(accessor = by_created_at, btree(columns = [created_at]))
)]
pub struct Corpse {
    #[primary_key]
    pub guid: u64,
    pub owner_guid: u64,
    pub map_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub display_id: u32,
    pub bytes_1: u32,
    pub bytes_2: u32,
    pub created_at: Timestamp,

    #[default(30_000_000i64)]
    pub reclaim_delay_micros: i64,

    #[default(false)]
    pub is_bones: bool,

    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i64)]
    pub cell: i64,
}

impl Corpse {
    pub(crate) fn position_cell(&self) -> i64 {
        let (x, y) = lyracore_shared::spatial::grid_cell(self.x, self.y);
        lyracore_shared::spatial::grid_cell_id(x, y)
    }

    pub(crate) fn insert(mut self, ctx: &ReducerContext) -> Self {
        self.cell = self.position_cell();
        let row = ctx.db.game_corpse().insert(self);
        crate::aoi::corpse_changed(ctx, &row);
        crate::native::corpse_created(ctx, &row);
        row
    }

    pub(crate) fn update(mut self, ctx: &ReducerContext) -> Self {
        self.cell = self.position_cell();
        let previous = ctx.db.game_corpse().guid().find(self.guid);
        let row = ctx.db.game_corpse().guid().update(self);
        crate::aoi::corpse_changed(ctx, &row);
        if previous.is_some_and(|old| old.is_bones != row.is_bones) {
            crate::native::corpse_changed(ctx, &row);
        }
        row
    }

    pub(crate) fn delete(ctx: &ReducerContext, guid: u64) {
        if let Some(corpse) = ctx.db.game_corpse().guid().find(guid) {
            ctx.db.game_corpse().guid().delete(guid);
            crate::native::corpse_removed(ctx, corpse.owner_guid);
        }
        crate::aoi::remove_subject(ctx, crate::aoi::CORPSE, guid);
    }
}

pub(crate) fn apply_reclaim_corpse(
    ctx: &ReducerContext,
    mut player: crate::WorldEntity,
) -> Result<(), crate::Refusal> {
    use lyracore_shared::constants::player_flags;
    let corpses = ctx.db.game_corpse();

    if !player.dead || player.player_flags & player_flags::GHOST == 0 {
        return Err("caller is not a ghost".into());
    }
    // The request GUID is advisory; reclaim always selects the Actor's own corpse.
    let corpse_guid = corpse_guid_for(player.guid);
    let corpse = corpses
        .guid()
        .find(corpse_guid)
        .ok_or_else(|| "no such corpse".to_string())?;
    if corpse.map_id != player.map_id {
        return Err("corpse on another map".into());
    }
    if corpse.instance_id != player.instance_id {
        return Err("corpse in another instance".into());
    }
    if corpse.is_bones {
        return Err("corpse has decayed to bones".into());
    }
    let (dx, dy, dz) = (
        corpse.x - player.x,
        corpse.y - player.y,
        corpse.z - player.z,
    );
    if dx * dx + dy * dy + dz * dz > RECLAIM_RADIUS_SQ {
        return Err("too far from corpse".into());
    }
    let elapsed =
        ctx.timestamp.to_micros_since_unix_epoch() - corpse.created_at.to_micros_since_unix_epoch();
    if elapsed < corpse.reclaim_delay_micros {
        return Err("corpse reclaim delay not elapsed".into());
    }

    player.health = (player.max_health / 2).max(1);
    let player_guid = player.guid;
    player.update(ctx);
    crate::ghost::finish_resurrection(ctx, player_guid);
    Ok(())
}

pub(crate) fn sweep_corpse_decay(ctx: &ReducerContext) {
    let t = ctx.db.game_corpse();
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let to_bones: Vec<u64> = t
        .by_created_at()
        .filter(..=Timestamp::from_micros_since_unix_epoch(now - CORPSE_DECAY_MICROS))
        .filter(|c| !c.is_bones)
        .map(|c| c.guid)
        .collect();
    for guid in to_bones {
        if let Some(mut c) = t.guid().find(guid) {
            c.is_bones = true;
            c.update(ctx);
        }
    }
    let bones_gone_micros = CORPSE_DECAY_MICROS + BONES_DECAY_MICROS;
    let to_despawn: Vec<u64> = t
        .by_created_at()
        .filter(..=Timestamp::from_micros_since_unix_epoch(now - bones_gone_micros))
        .filter(|c| c.is_bones)
        .map(|c| c.guid)
        .collect();
    for guid in to_despawn {
        Corpse::delete(ctx, guid);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        corpse_appearance_bytes, corpse_guid_for, escalated_reclaim, HIGHGUID_CORPSE,
        MAX_STREAK_RUNGS, STREAK_RUNG_MICROS,
    };

    #[test]
    fn corpse_guid_puts_highguid_in_the_top_16_and_keeps_the_owner_low_48() {
        assert_eq!(corpse_guid_for(42), 0xF101_0000_0000_002A);
        assert_eq!(corpse_guid_for(42) >> 48, HIGHGUID_CORPSE);
        assert_eq!(corpse_guid_for(42) & 0x0000_FFFF_FFFF_FFFF, 42);
        let owner = (0xABCD_u64 << 48) | 7;
        assert_eq!(corpse_guid_for(owner), 0xF101_0000_0000_0007);
        assert_eq!(
            corpse_guid_for(0x0000_FFFF_FFFF_FFFF),
            (HIGHGUID_CORPSE << 48) | 0x0000_FFFF_FFFF_FFFF
        );
    }

    #[test]
    #[allow(clippy::identity_op)]
    fn corpse_bytes_put_race_gender_in_the_right_slots() {
        let unit_bytes_0 = 1 | (1 << 8) | (0 << 16) | (1 << 24);
        let player_bytes = 5 | (6 << 8) | (7 << 16) | (8 << 24);
        let player_bytes_2 = 9;
        let (b1, b2) = corpse_appearance_bytes(unit_bytes_0, player_bytes, player_bytes_2);
        assert_eq!(b1 & 0xFF, 0);
        assert_eq!((b1 >> 8) & 0xFF, 1, "byte1 must be race");
        assert_eq!((b1 >> 16) & 0xFF, 0, "byte2 must be gender");
        assert_eq!((b1 >> 24) & 0xFF, 5, "byte3 must be skin");
        assert_eq!(b2 & 0xFF, 6);
        assert_eq!((b2 >> 8) & 0xFF, 7);
        assert_eq!((b2 >> 16) & 0xFF, 8);
        assert_eq!((b2 >> 24) & 0xFF, 9);
    }

    #[test]
    fn escalated_reclaim_climbs_the_ladder_then_caps_then_resets_past_expiry() {
        let now: i64 = 1_800_000_000_000_000;
        let step = STREAK_RUNG_MICROS;

        let (expire1, delay1) = escalated_reclaim(0, now);
        assert_eq!(delay1, 30_000_000, "first death is the 30s base");
        assert_eq!(expire1, now + step);

        let now2 = now + 1_000_000;
        let (expire2, delay2) = escalated_reclaim(expire1, now2);
        assert_eq!(delay2, 60_000_000, "second quick death escalates to 60s");

        let now3 = now2 + 1_000_000;
        let (expire3, delay3) = escalated_reclaim(expire2, now3);
        assert_eq!(
            delay3, 120_000_000,
            "third quick death reaches the 120s cap"
        );

        let now4 = now3 + 1_000_000;
        let (_expire4, delay4) = escalated_reclaim(expire3, now4);
        assert_eq!(
            delay4, 120_000_000,
            "a fourth quick death stays capped at 120s, not beyond"
        );

        let now_later = expire3 + step + 1;
        let (expire_reset, delay_reset) = escalated_reclaim(expire3, now_later);
        assert_eq!(
            delay_reset, 30_000_000,
            "decay past expiry resets to the 30s base"
        );
        assert_eq!(expire_reset, now_later + step);
    }

    #[test]
    fn escalated_reclaim_saturates_and_never_leaves_the_ladder() {
        let step = STREAK_RUNG_MICROS;
        let ladder_top =
            super::CORPSE_RECLAIM_DELAY_SECS[super::CORPSE_RECLAIM_DELAY_SECS.len() - 1];
        let mut deadline = 0i64;
        let mut t: i64 = 1_800_000_000_000_000;
        for i in 0..500i64 {
            t += (i * 37_000_000) % (step * 2);
            let (next, delay) = escalated_reclaim(deadline, t);
            assert!(
                super::CORPSE_RECLAIM_DELAY_SECS.contains(&(delay / 1_000_000)),
                "death {i}: delay {delay} is not a ladder value"
            );
            assert!(
                delay <= ladder_top * 1_000_000,
                "death {i}: delay ran past the ceiling"
            );
            assert!(
                next > t,
                "death {i}: the streak deadline must sit ahead of the death"
            );
            assert!(
                next - t <= (MAX_STREAK_RUNGS + 1) * step,
                "death {i}: the deadline grew past the re-pin bound"
            );
            deadline = next;
        }
        let (_, after_a_long_quiet) = escalated_reclaim(deadline, deadline + 1);
        assert_eq!(
            after_a_long_quiet, 30_000_000,
            "a lapsed streak is back to the base delay"
        );
    }
}
