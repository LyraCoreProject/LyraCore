use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::{
    faction::Faction, game_character, game_faction, game_faction_template, game_world_entity,
};

#[table(accessor = game_player_reputation, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct PlayerReputation {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub faction_id: u32,
    pub standing: i32,
    // Client reputation slots are 0..63 and differ from Faction.dbc ids.
    #[default(0)]
    pub reputation_index: i32,
    #[default(false)]
    pub at_war: bool,
}

impl PlayerReputation {
    pub(crate) fn delete(ctx: &ReducerContext, id: u64) {
        if let Some(mut row) = ctx.db.game_player_reputation().id().find(id) {
            row.standing = ctx
                .db
                .game_faction()
                .faction_id()
                .find(row.faction_id)
                .map_or(0, |faction| {
                    clamped_standing(base_standing(ctx, row.character_guid, &faction))
                });
            row.at_war = false;
            crate::native::reputation::changed(ctx, None, &row);
            ctx.db.game_player_reputation().id().delete(id);
        }
    }

    pub(crate) fn insert(ctx: &ReducerContext, row: Self) {
        crate::native::reputation::changed(ctx, None, &row);
        ctx.db.game_player_reputation().insert(row);
    }

    fn update(self, ctx: &ReducerContext) {
        let previous = ctx.db.game_player_reputation().id().find(self.id);
        crate::native::reputation::changed(ctx, previous.as_ref(), &self);
        ctx.db.game_player_reputation().id().update(self);
    }
}

crate::character_owned!(delete, fn sweep_delete_game_player_reputation(ctx, character_guid) {
    let reps = ctx.db.game_player_reputation();
    for r in reps.by_character().filter(&character_guid).collect::<Vec<_>>() {
        PlayerReputation::delete(ctx, r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_player_reputation(ctx, character_guid, io) {
    table = game_player_reputation,
    by = by_character,
    remint = id,
});

crate::character_owned!(restamp, fn sweep_restamp_game_player_reputation(ctx, character_guid, identity) {
    let reps = ctx.db.game_player_reputation();
    for mut r in reps.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            r.update(ctx);
        }
    }
});

// vmangos ReputationMgr Reputation_Bottom and Reputation_Cap.
const REP_MIN: i32 = -42000;
const REP_MAX: i32 = 42999;

pub(crate) fn clamped_standing(standing: i32) -> i32 {
    standing.clamp(REP_MIN, REP_MAX)
}

/// vmangos `GetBaseReputation` for the Character's race and class; 0 when no slot admits it.
pub(crate) fn base_standing(ctx: &ReducerContext, character_guid: u64, faction: &Faction) -> i32 {
    let (race, class) = ctx
        .db
        .game_character()
        .guid()
        .find(character_guid)
        .map_or((0, 0), |character| (character.race, character.class));
    faction.base_standing_for(race, class)
}

/// vmangos `FactionState::Standing`, the part above the base that the client is sent.
pub(crate) fn earned_standing(ctx: &ReducerContext, row: &PlayerReputation) -> i32 {
    let base = ctx
        .db
        .game_faction()
        .faction_id()
        .find(row.faction_id)
        .map_or(0, |faction| {
            base_standing(ctx, row.character_guid, &faction)
        });
    row.standing.saturating_sub(base)
}

/// Stored standing, else the Character's base standing; `None` unindexed.
pub(crate) fn reputation(
    ctx: &ReducerContext,
    character_guid: u64,
    faction_id: u32,
) -> Option<i32> {
    let faction = ctx
        .db
        .game_faction()
        .faction_id()
        .find(faction_id)
        .filter(|faction| faction.reputation_index >= 0)?;
    Some(
        ctx.db
            .game_player_reputation()
            .by_character()
            .filter(&character_guid)
            .find(|row| row.faction_id == faction_id)
            .map_or_else(
                || base_standing(ctx, character_guid, &faction),
                |row| row.standing,
            ),
    )
}

pub(crate) fn reputation_rank(standing: i32) -> u8 {
    lyracore_shared::faction::reputation_rank(standing)
}

pub(crate) fn reputation_discount_pct(standing: i32) -> u32 {
    if reputation_rank(standing) >= 5 {
        10
    } else {
        0
    }
}

pub(crate) fn vendor_discount_pct(
    ctx: &ReducerContext,
    player_guid: u64,
    faction_template_id: u32,
) -> u32 {
    ctx.db
        .game_faction_template()
        .id()
        .find(faction_template_id)
        .and_then(|tmpl| reputation(ctx, player_guid, tmpl.faction))
        .map_or(0, reputation_discount_pct)
}

pub(crate) fn npc_refuses_interaction(
    ctx: &ReducerContext,
    npc: &crate::WorldEntity,
    player: &crate::WorldEntity,
) -> bool {
    let templates = ctx.db.game_faction_template();
    let Some(npc_ft) = templates.id().find(npc.faction_template) else {
        return false;
    };
    let reputation = reputation(ctx, player.guid, npc_ft.faction).map(|standing| {
        let at_war = ctx
            .db
            .game_player_reputation()
            .by_character()
            .filter(&player.guid)
            .any(|row| row.faction_id == npc_ft.faction && row.at_war);
        (standing, at_war)
    });
    let template_hostile = templates
        .id()
        .find(player.faction_template)
        .is_some_and(|player_ft| crate::faction::compute_hostile(&npc_ft, &player_ft));
    lyracore_shared::faction::refuses_interaction(reputation, template_hostile)
}

pub(crate) fn grant_reputation(
    ctx: &ReducerContext,
    player_guid: u64,
    faction_id: u32,
    amount: i32,
) {
    let (base, rep_index) = match ctx.db.game_faction().faction_id().find(faction_id) {
        Some(f) if f.reputation_index >= 0 => {
            (base_standing(ctx, player_guid, &f), f.reputation_index)
        }
        _ => return,
    };
    let reps = ctx.db.game_player_reputation();
    if let Some(mut row) = reps
        .by_character()
        .filter(&player_guid)
        .find(|r| r.faction_id == faction_id)
    {
        row.standing = clamped_standing(row.standing.saturating_add(amount));
        row.reputation_index = rep_index;
        row.update(ctx);
        return;
    }
    let Some(owner) = ctx
        .db
        .game_world_entity()
        .guid()
        .find(player_guid)
        .map(|e| e.owner_identity)
        .or_else(|| {
            ctx.db
                .game_character()
                .guid()
                .find(player_guid)
                .map(|c| c.owner_identity)
        })
    else {
        return;
    };
    PlayerReputation::insert(
        ctx,
        PlayerReputation {
            id: 0,
            character_guid: player_guid,
            owner_identity: owner,
            faction_id,
            standing: clamped_standing(base.saturating_add(amount)),
            reputation_index: rep_index,
            at_war: false,
        },
    );
}

pub(crate) fn apply_set_faction_at_war(
    ctx: &ReducerContext,
    player: crate::WorldEntity,
    reputation_index: u32,
    at_war: bool,
) -> Result<(), String> {
    if reputation_index >= 64 {
        return Err(format!("invalid reputation index {reputation_index}"));
    }
    let Some(faction) = ctx
        .db
        .game_faction()
        .iter()
        .find(|f| f.reputation_index == reputation_index as i32)
    else {
        return Err(format!("no faction at reputation index {reputation_index}"));
    };
    let reps = ctx.db.game_player_reputation();
    if let Some(mut row) = reps
        .by_character()
        .filter(&player.guid)
        .find(|r| r.faction_id == faction.faction_id)
    {
        row.at_war = at_war;
        row.reputation_index = reputation_index as i32;
        row.update(ctx);
        return Ok(());
    }
    PlayerReputation::insert(
        ctx,
        PlayerReputation {
            id: 0,
            character_guid: player.guid,
            owner_identity: player.owner_identity,
            faction_id: faction.faction_id,
            standing: clamped_standing(base_standing(ctx, player.guid, &faction)),
            reputation_index: reputation_index as i32,
            at_war,
        },
    );
    Ok(())
}

pub(crate) fn apply_set_watched_faction(
    ctx: &ReducerContext,
    character_guid: u64,
    reputation_index: i32,
) -> Result<(), String> {
    if !(-1..=63).contains(&reputation_index) {
        return Err(format!(
            "invalid watched reputation index {reputation_index}"
        ));
    }
    let characters = ctx.db.game_character();
    let mut character = crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or_else(|| "Character not found".to_string())?;
    character.watched_faction_index = reputation_index;
    characters.guid().update(character);
    crate::native::reputation::watched_faction(ctx, character_guid, reputation_index);
    Ok(())
}

#[cfg(test)]
mod rep_tests {
    use super::{clamped_standing, reputation_discount_pct, reputation_rank};

    #[test]
    fn standing_reaches_the_vmangos_cap_of_42999() {
        assert_eq!(clamped_standing(42_999), 42_999);
        assert_eq!(clamped_standing(43_000), 42_999);
        assert_eq!(clamped_standing(-42_001), -42_000);
        assert_eq!(reputation_rank(42_999), 7);
    }

    #[test]
    fn rank_thresholds_match_vanilla() {
        assert_eq!(reputation_rank(-42000), 0);
        assert_eq!(reputation_rank(-1), 2);
        assert_eq!(reputation_rank(0), 3);
        assert_eq!(reputation_rank(3000), 4);
        assert_eq!(reputation_rank(9000), 5);
        assert_eq!(reputation_rank(21000), 6);
        assert_eq!(reputation_rank(42000), 7);
    }

    #[test]
    fn discount_is_ten_percent_from_honored() {
        assert_eq!(reputation_discount_pct(0), 0);
        assert_eq!(reputation_discount_pct(-10000), 0);
        assert_eq!(reputation_discount_pct(3000), 0);
        assert_eq!(reputation_discount_pct(8999), 0);
        assert_eq!(reputation_discount_pct(9000), 10);
        assert_eq!(reputation_discount_pct(21000), 10);
        assert_eq!(reputation_discount_pct(42000), 10);
    }
}
