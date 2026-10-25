use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::{game_aura, game_spell};

#[table(
    accessor = game_player_spell,
    public,
    index(accessor = by_character, btree(columns = [character_guid])),
    index(accessor = by_character_spell, btree(columns = [character_guid, spell_id]))
)]
pub struct PlayerSpell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub spell_id: u32,
}

crate::character_owned!(delete, fn sweep_delete_game_player_spell(ctx, character_guid) {
    let spells = ctx.db.game_player_spell();
    for r in spells.by_character().filter(&character_guid).collect::<Vec<_>>() {
        spells.id().delete(r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_player_spell(ctx, character_guid, io) {
    table = game_player_spell,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_player_spell(ctx, character_guid, identity) {
    let spells = ctx.db.game_player_spell();
    for mut r in spells.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            spells.id().update(r);
        }
    }
});

// Zero race or class matches every value in that field.
#[table(accessor = game_createinfo_spell)]
pub struct CreateinfoSpell {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub race: u8,
    pub class: u8,
    pub spell_id: u32,
}

pub(crate) fn grant_createinfo_spells(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    race: u8,
    class: u8,
) {
    for row in ctx.db.game_createinfo_spell().iter() {
        if createinfo_row_matches(row.race, row.class, race, class) {
            learn_spell(ctx, guid, owner, row.spell_id);
        }
    }
}

pub(crate) fn createinfo_row_matches(row_race: u8, row_class: u8, race: u8, class: u8) -> bool {
    (row_race == 0 || row_race == race) && (row_class == 0 || row_class == class)
}

pub(crate) fn knows_spell(ctx: &ReducerContext, guid: u64, spell_id: u32) -> bool {
    ctx.db
        .game_player_spell()
        .by_character_spell()
        .filter((guid, spell_id))
        .next()
        .is_some()
}

pub(crate) fn apply_racial_passives(ctx: &ReducerContext, guid: u64, race: u8, level: u8) {
    let racials: Vec<u32> = ctx
        .db
        .game_createinfo_spell()
        .iter()
        .filter(|r| r.class == 0 && (r.race == 0 || r.race == race))
        .map(|r| r.spell_id)
        .collect();
    for spell_id in racials {
        let Some(hdr) = ctx.db.game_spell().spell_id().find(spell_id) else {
            continue;
        };
        if hdr.attributes & lyracore_shared::spell::taxonomy::SPELL_ATTR_PASSIVE != 0 {
            crate::spell::apply_spell_auras(ctx, spell_id, guid, level, 1);
        }
    }
}

pub(crate) fn learn_spell(ctx: &ReducerContext, guid: u64, owner: Identity, spell_id: u32) {
    if knows_spell(ctx, guid, spell_id) {
        return;
    }
    ctx.db.game_player_spell().insert(PlayerSpell {
        id: 0,
        character_guid: guid,
        owner_identity: owner,
        spell_id,
    });
    crate::native::spell_learned(ctx, guid, spell_id);
    crate::skill::grant_lockpicking_on_learn(ctx, guid, owner, spell_id);
}

pub(crate) fn forget_spell(ctx: &ReducerContext, guid: u64, spell_id: u32) {
    let spells = ctx.db.game_player_spell();
    for row in spells
        .by_character_spell()
        .filter((guid, spell_id))
        .collect::<Vec<_>>()
    {
        spells.id().delete(row.id);
        crate::native::spell_forgotten(ctx, guid, spell_id);
    }
}

// The caller must recompute vitals once after stripping its batch of spells.
pub(crate) fn strip_spell_auras(ctx: &ReducerContext, guid: u64, spell_id: u32) -> bool {
    let auras = ctx.db.game_aura();
    let redisplay = auras
        .by_target()
        .filter(&guid)
        .any(|a| a.spell_id == spell_id && a.eff_kind == super::A_TRANSFORM);
    let to_remove: Vec<(u64, bool, bool)> = auras
        .by_target()
        .filter(&guid)
        .filter(|a| a.spell_id == spell_id)
        .map(|a| {
            (
                a.id,
                crate::spell::aura_moves_vitals(a.eff_kind, a.eff_p0),
                crate::mount::mount_aura_moves_mount(a.eff_kind, a.eff_p0),
            )
        })
        .collect();
    let mut moved_vitals = false;
    let mut moved_mount = false;
    for (id, mv, mm) in to_remove {
        crate::Aura::delete(ctx, id);
        moved_vitals |= mv;
        moved_mount |= mm;
    }
    if moved_mount {
        crate::mount::recompute_mount(ctx, guid);
    }
    if redisplay {
        super::recompute_display(ctx, guid);
    }
    moved_vitals
}

#[table(accessor = game_spell_chain, public, index(accessor = by_first, btree(columns = [first_spell])))]
pub struct SpellChain {
    #[primary_key]
    pub spell_id: u32,
    pub prev_spell: u32,
    pub first_spell: u32,
    pub rank: u8,
    pub req_spell: u32,
}

#[table(accessor = game_spell_learn, index(accessor = by_parent, btree(columns = [parent_spell])))]
pub struct SpellLearn {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub parent_spell: u32,
    pub learn_spell: u32,
}

pub(crate) fn learn_spell_with_dependents(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    spell_id: u32,
) {
    learn_spell(ctx, guid, owner, spell_id);
    let rows: Vec<(u32, u32)> = ctx
        .db
        .game_spell_learn()
        .by_parent()
        .filter(&spell_id)
        .map(|r| (r.parent_spell, r.learn_spell))
        .collect();
    for dep in dependent_spell_ids(&rows, spell_id) {
        learn_spell(ctx, guid, owner, dep);
    }
}

pub(crate) fn dependent_spell_ids(rows: &[(u32, u32)], parent_spell: u32) -> Vec<u32> {
    rows.iter()
        .filter(|&&(parent, _)| parent == parent_spell)
        .map(|&(_, learn)| learn)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{createinfo_row_matches, dependent_spell_ids};
    use crate::seed::CREATEINFO_KIT;

    #[test]
    fn dependent_spell_ids_filters_by_parent_and_is_empty_for_no_match_or_empty_table() {
        let rows = vec![(100u32, 200u32), (100, 201), (300, 400)];
        assert_eq!(dependent_spell_ids(&rows, 100), vec![200, 201]);
        assert_eq!(dependent_spell_ids(&rows, 300), vec![400]);
        assert_eq!(
            dependent_spell_ids(&rows, 999),
            Vec::<u32>::new(),
            "no dependents for an unrelated parent"
        );
        assert_eq!(
            dependent_spell_ids(&[], 100),
            Vec::<u32>::new(),
            "empty table -> no dependents, never a panic"
        );
    }

    #[test]
    fn an_armor_proficiency_wrapper_teaches_its_passive_as_a_dependent() {
        use lyracore_shared::constants::armor_proficiency::*;

        let rows = vec![
            (PLATE_TRAINER_SPELL_ID, PLATE_PASSIVE_SPELL_ID),
            (MAIL_TRAINER_SPELL_ID, MAIL_PASSIVE_SPELL_ID),
        ];
        assert_eq!(
            dependent_spell_ids(&rows, PLATE_TRAINER_SPELL_ID),
            vec![PLATE_PASSIVE_SPELL_ID]
        );
        assert_eq!(
            dependent_spell_ids(&rows, MAIL_TRAINER_SPELL_ID),
            vec![MAIL_PASSIVE_SPELL_ID]
        );
        assert_eq!(
            dependent_spell_ids(&rows, PLATE_PASSIVE_SPELL_ID),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn createinfo_row_matches_exact_race_and_class() {
        assert!(createinfo_row_matches(1, 1, 1, 1));
        assert!(!createinfo_row_matches(1, 1, 1, 4));
        assert!(!createinfo_row_matches(1, 1, 3, 1));
    }

    #[test]
    fn createinfo_row_matches_race_wildcard_matches_every_race() {
        assert!(createinfo_row_matches(0, 1, 1, 1));
        assert!(createinfo_row_matches(0, 1, 3, 1));
        assert!(createinfo_row_matches(0, 1, 7, 1));
        assert!(!createinfo_row_matches(0, 1, 1, 4));
    }

    #[test]
    fn createinfo_row_matches_class_wildcard_matches_every_class() {
        assert!(createinfo_row_matches(1, 0, 1, 1));
        assert!(createinfo_row_matches(1, 0, 1, 8));
        assert!(!createinfo_row_matches(1, 0, 3, 1));
    }

    #[test]
    fn createinfo_row_matches_double_wildcard_matches_everyone() {
        assert!(createinfo_row_matches(0, 0, 1, 1));
        assert!(createinfo_row_matches(0, 0, 7, 4));
        assert!(createinfo_row_matches(0, 0, 255, 255));
    }

    #[test]
    fn createinfo_kit_holds_starting_spells_and_nothing_trainer_taught() {
        let has = |race: u8, class: u8, id: u32| {
            CREATEINFO_KIT
                .iter()
                .any(|&(r, c, s)| r == race && c == class && s == id)
        };
        assert!(has(0, 1, 78));
        assert!(has(0, 1, 2457));
        assert!(has(1, 0, 20598));
        let ids: Vec<u32> = CREATEINFO_KIT.iter().map(|&(_, _, s)| s).collect();
        assert!(!ids.contains(&6673));
        assert!(!ids.contains(&871));
        assert!(!ids.contains(&51300));
        assert!(!ids.contains(&51000));
    }
}
