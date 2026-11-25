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

fn race_and_class(ctx: &ReducerContext, character_guid: u64) -> (u8, u8) {
    crate::helpers::character_by_guid(ctx, character_guid)
        .map_or((0, 0), |character| (character.race, character.class))
}

/// vmangos `GetBaseReputation` for the Character's race and class; 0 when no slot admits it.
pub(crate) fn base_standing(ctx: &ReducerContext, character_guid: u64, faction: &Faction) -> i32 {
    let (race, class) = race_and_class(ctx, character_guid);
    faction.base_standing_for(race, class)
}

// vmangos ReputationMgr.h FactionFlags.
const VISIBLE: u32 = 0x01;
const AT_WAR: u32 = 0x02;
const HIDDEN: u32 = 0x04;
const INVISIBLE_FORCED: u32 = 0x08;
const PEACE_FORCED: u32 = 0x10;

// vmangos ReputationRank REP_HATED, REP_HOSTILE and REP_UNFRIENDLY.
const HATED: u8 = 0;
const HOSTILE: u8 = 1;
const UNFRIENDLY: u8 = 2;

struct StoredReputation {
    standing: i32,
    earned: i32,
    at_war: bool,
}

/// vmangos `SetAtWar(faction, true)`: peace forced refuses war above an earned Hated.
fn declare_war(flags: &mut u32, earned: i32) {
    if *flags & PEACE_FORCED == 0 || reputation_rank(earned) == HATED {
        *flags |= AT_WAR;
    }
}

/// vmangos `FactionState::Flags` after `LoadFromDB`.
fn faction_flags(defaults: u32, base: i32, stored: Option<StoredReputation>) -> u32 {
    let mut flags = defaults;
    let Some(stored) = stored else {
        if reputation_rank(base) <= HOSTILE {
            declare_war(&mut flags, 0);
        }
        return flags;
    };
    if flags & (HIDDEN | INVISIBLE_FORCED) == 0 {
        flags |= VISIBLE;
    }
    if stored.at_war {
        declare_war(&mut flags, stored.earned);
    } else if flags & VISIBLE != 0 {
        flags &= !AT_WAR;
    }
    if reputation_rank(stored.standing) <= HOSTILE {
        declare_war(&mut flags, stored.earned);
    }
    flags
}

/// A faction's base standing and default flags for one Character's race and class.
struct CharacterFaction {
    base: i32,
    defaults: u32,
}

impl CharacterFaction {
    fn new(faction: &Faction, race: u8, class: u8) -> Self {
        Self {
            base: faction.base_standing_for(race, class),
            defaults: faction.default_flags_for(race, class),
        }
    }

    fn read(ctx: &ReducerContext, character_guid: u64, faction: &Faction) -> Self {
        let (race, class) = race_and_class(ctx, character_guid);
        Self::new(faction, race, class)
    }

    fn flags(&self, row: Option<&PlayerReputation>) -> u32 {
        faction_flags(
            self.defaults,
            self.base,
            row.map(|row| StoredReputation {
                standing: row.standing,
                earned: row.standing.saturating_sub(self.base),
                at_war: row.at_war,
            }),
        )
    }
}

fn stored_row(
    ctx: &ReducerContext,
    character_guid: u64,
    faction_id: u32,
) -> Option<PlayerReputation> {
    ctx.db
        .game_player_reputation()
        .by_character()
        .filter(&character_guid)
        .find(|row| row.faction_id == faction_id)
}

/// vmangos `SendInitialReputations`: each reputation slot's index, earned standing and flags.
pub(crate) fn initial_factions(ctx: &ReducerContext, character_guid: u64) -> Vec<(i32, i32, u8)> {
    let (race, class) = race_and_class(ctx, character_guid);
    let rows = ctx
        .db
        .game_player_reputation()
        .by_character()
        .filter(&character_guid)
        .collect::<Vec<_>>();
    ctx.db
        .game_faction()
        .iter()
        .filter(|faction| (0..64).contains(&faction.reputation_index))
        .map(|faction| {
            let state = CharacterFaction::new(&faction, race, class);
            let row = rows.iter().find(|row| row.faction_id == faction.faction_id);
            let earned = row.map_or(0, |row| row.standing.saturating_sub(state.base));
            // The client's flag byte holds every vmangos FactionFlags bit.
            (faction.reputation_index, earned, state.flags(row) as u8)
        })
        .collect()
}

/// vmangos `FactionState::Standing`, the standing relative to the base, which the client is sent.
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
    Some(stored_row(ctx, character_guid, faction_id).map_or_else(
        || base_standing(ctx, character_guid, &faction),
        |row| row.standing,
    ))
}

/// The Character's rank with a reputation faction and whether it is at war; `None` without one.
pub(crate) fn rank_and_war(
    ctx: &ReducerContext,
    character_guid: u64,
    faction_id: u32,
) -> Option<(u8, bool)> {
    let faction = ctx
        .db
        .game_faction()
        .faction_id()
        .find(faction_id)
        .filter(|faction| faction.reputation_index >= 0)?;
    let state = CharacterFaction::read(ctx, character_guid, &faction);
    let row = stored_row(ctx, character_guid, faction_id);
    let standing = row.as_ref().map_or(state.base, |row| row.standing);
    Some((
        reputation_rank(standing),
        state.flags(row.as_ref()) & AT_WAR != 0,
    ))
}

const BASE_STANDING_REPAIR_FAMILY: &str = "repair_base_standing";

/// One-shot after the first `dbc` stamp with masks; see danger-zones "Base reputation migration".
pub(crate) fn repair_base_standing(ctx: &ReducerContext) {
    if crate::import_meta::is_imported(ctx, BASE_STANDING_REPAIR_FAMILY)
        || !ctx
            .db
            .game_faction()
            .iter()
            .any(|faction| faction.reputation_index >= 0 && faction.has_slot_masks())
    {
        return;
    }
    let rows = ctx.db.game_player_reputation().iter().collect::<Vec<_>>();
    if rows
        .iter()
        .any(|row| crate::transfer::is_in_transit(ctx, row.character_guid))
    {
        spacetimedb::log::warn!(
            "repair_base_standing: a Character is in transit; the next dbc stamp runs the repair"
        );
        return;
    }
    let mut moved = 0u64;
    for mut row in rows {
        let Some(faction) = ctx.db.game_faction().faction_id().find(row.faction_id) else {
            continue;
        };
        let standing = clamped_standing(
            row.standing
                .saturating_sub(faction.base_standing)
                .saturating_add(base_standing(ctx, row.character_guid, &faction)),
        );
        if standing != row.standing {
            row.standing = standing;
            row.update(ctx);
            moved += 1;
        }
    }
    crate::import_meta::stamp(ctx, BASE_STANDING_REPAIR_FAMILY, "", "", moved);
    spacetimedb::log::info!("repair_base_standing: moved {moved} reputation row(s) to their slot");
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

// A reputation rank at or below `limit`, else hostile templates; at war never refuses alone.
fn rank_refuses(reputation_standing: Option<i32>, template_hostile: bool, limit: u8) -> bool {
    reputation_standing.map_or(template_hostile, |standing| {
        reputation_rank(standing) <= limit
    })
}

fn npc_reaction_refuses(
    ctx: &ReducerContext,
    npc: &crate::WorldEntity,
    player: &crate::WorldEntity,
    limit: u8,
) -> bool {
    let templates = ctx.db.game_faction_template();
    let Some(npc_template) = templates.id().find(npc.faction_template) else {
        return false;
    };
    rank_refuses(
        reputation(ctx, player.guid, npc_template.faction),
        templates
            .id()
            .find(player.faction_template)
            .is_some_and(|player_template| {
                crate::faction::compute_hostile(&npc_template, &player_template)
            }),
        limit,
    )
}

/// vmangos `Unit::IsHostileTo` for the creature: Hostile or Hated, else hostile templates.
pub(crate) fn npc_is_hostile(
    ctx: &ReducerContext,
    npc: &crate::WorldEntity,
    player: &crate::WorldEntity,
) -> bool {
    npc_reaction_refuses(ctx, npc, player, HOSTILE)
}

/// vmangos `CanInteractWithNPC`: hostile, or a reputation faction at Unfriendly or below.
pub(crate) fn npc_refuses_service(
    ctx: &ReducerContext,
    npc: &crate::WorldEntity,
    player: &crate::WorldEntity,
) -> bool {
    npc_reaction_refuses(ctx, npc, player, UNFRIENDLY)
}

pub(crate) fn grant_reputation(
    ctx: &ReducerContext,
    player_guid: u64,
    faction_id: u32,
    amount: i32,
) {
    let Some(faction) = ctx
        .db
        .game_faction()
        .faction_id()
        .find(faction_id)
        .filter(|faction| faction.reputation_index >= 0)
    else {
        return;
    };
    let state = CharacterFaction::read(ctx, player_guid, &faction);
    let (mut row, existing) = match stored_row(ctx, player_guid, faction_id) {
        Some(row) => (row, true),
        None => match new_row(ctx, player_guid, &faction, &state) {
            Some(row) => (row, false),
            None => return,
        },
    };
    row.standing = clamped_standing(row.standing.saturating_add(amount));
    row.reputation_index = faction.reputation_index;
    // vmangos SetOneFactionReputation declares war at Hostile or below.
    row.at_war = state.flags(Some(&row)) & AT_WAR != 0;
    if existing {
        row.update(ctx);
    } else {
        PlayerReputation::insert(ctx, row);
    }
}

fn new_row(
    ctx: &ReducerContext,
    character_guid: u64,
    faction: &Faction,
    state: &CharacterFaction,
) -> Option<PlayerReputation> {
    let owner_identity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .map(|e| e.owner_identity)
        .or_else(|| {
            ctx.db
                .game_character()
                .guid()
                .find(character_guid)
                .map(|c| c.owner_identity)
        })?;
    Some(PlayerReputation {
        id: 0,
        character_guid,
        owner_identity,
        faction_id: faction.faction_id,
        standing: clamped_standing(state.base),
        reputation_index: faction.reputation_index,
        at_war: state.defaults & AT_WAR != 0,
    })
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
    let state = CharacterFaction::read(ctx, player.guid, &faction);
    let row = stored_row(ctx, player.guid, faction.faction_id);
    let flags = state.flags(row.as_ref());
    let mut declared = flags;
    let standing = row.as_ref().map_or(state.base, |row| row.standing);
    declare_war(&mut declared, standing.saturating_sub(state.base));
    // vmangos ignores the request in combat, for a hidden faction, under peace forced or unchanged.
    if player.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0
        || flags & (HIDDEN | INVISIBLE_FORCED) != 0
        || (flags & AT_WAR != 0) == at_war
        || (at_war && declared & AT_WAR == 0)
    {
        return Ok(());
    }
    if let Some(mut row) = row {
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
            standing: clamped_standing(state.base),
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
    use super::{
        clamped_standing, faction_flags, rank_refuses, reputation_discount_pct, reputation_rank,
        StoredReputation, AT_WAR, HIDDEN, HOSTILE, INVISIBLE_FORCED, PEACE_FORCED, UNFRIENDLY,
        VISIBLE,
    };

    fn stored(standing: i32, at_war: bool) -> Option<StoredReputation> {
        Some(StoredReputation {
            standing,
            earned: standing,
            at_war,
        })
    }

    #[test]
    fn a_faction_without_a_row_keeps_its_default_flags() {
        assert_eq!(
            faction_flags(HIDDEN | PEACE_FORCED, 0, None),
            HIDDEN | PEACE_FORCED
        );
        assert_eq!(faction_flags(AT_WAR, 0, None), AT_WAR);
    }

    #[test]
    fn a_faction_without_a_row_is_at_war_from_a_hostile_base_unless_peace_forced() {
        assert_eq!(faction_flags(0, -42_000, None), AT_WAR);
        assert_eq!(faction_flags(0, -3001, None), AT_WAR);
        assert_eq!(faction_flags(0, -3000, None), 0);
        assert_eq!(faction_flags(PEACE_FORCED, -42_000, None), PEACE_FORCED);
    }

    #[test]
    fn a_stored_row_is_visible_unless_its_faction_is_hidden() {
        assert_eq!(faction_flags(0, 0, stored(500, false)), VISIBLE);
        assert_eq!(faction_flags(HIDDEN, 0, stored(500, false)), HIDDEN);
        assert_eq!(
            faction_flags(INVISIBLE_FORCED | AT_WAR, 0, stored(500, false)),
            INVISIBLE_FORCED | AT_WAR
        );
    }

    #[test]
    fn peace_forced_refuses_war_above_an_earned_hated() {
        assert_eq!(
            faction_flags(PEACE_FORCED, 0, stored(-6000, true)),
            PEACE_FORCED | VISIBLE
        );
        assert_eq!(
            faction_flags(PEACE_FORCED, 0, stored(-6001, true)),
            PEACE_FORCED | VISIBLE | AT_WAR
        );
        assert_eq!(faction_flags(0, 0, stored(42_000, true)), VISIBLE | AT_WAR);
    }

    #[test]
    fn a_hostile_rank_declares_war_and_a_visible_row_clears_a_default_war() {
        assert_eq!(faction_flags(0, 0, stored(-3001, false)), VISIBLE | AT_WAR);
        assert_eq!(faction_flags(0, 0, stored(-3000, false)), VISIBLE);
        assert_eq!(faction_flags(AT_WAR, 0, stored(0, false)), VISIBLE);
    }

    #[test]
    fn hostility_refuses_from_hostile_and_service_from_unfriendly() {
        assert!(rank_refuses(Some(-3001), false, HOSTILE));
        assert!(rank_refuses(Some(-42000), false, HOSTILE));
        assert!(!rank_refuses(Some(-3000), true, HOSTILE), "Unfriendly");
        assert!(rank_refuses(Some(-1), false, UNFRIENDLY), "Unfriendly");
        assert!(!rank_refuses(Some(0), true, UNFRIENDLY), "Neutral");
    }

    #[test]
    fn without_a_reputation_faction_the_templates_decide() {
        for limit in [HOSTILE, UNFRIENDLY] {
            assert!(rank_refuses(None, true, limit));
            assert!(!rank_refuses(None, false, limit));
        }
    }

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
