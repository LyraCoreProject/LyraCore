use spacetimedb::{table, Identity, ReducerContext, Table};

use crate::spell::spellbook::createinfo_row_matches;

#[table(accessor = game_createinfo_action, public)]
pub struct CreateinfoAction {
    #[primary_key]
    #[auto_inc]
    pub row_id: u64,
    pub race: u8,
    pub class: u8,
    pub button: u8,
    pub action: u32,
    pub action_type: u8,
}

#[table(accessor = game_player_action, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct PlayerAction {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub button: u8,
    pub action: u32,
    pub action_type: u8,
}

crate::character_owned!(delete, fn sweep_delete_game_player_action(ctx, character_guid) {
    let actions = ctx.db.game_player_action();
    for r in actions.by_character().filter(&character_guid).collect::<Vec<_>>() {
        actions.id().delete(r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_player_action(ctx, character_guid, io) {
    table = game_player_action,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_player_action(ctx, character_guid, identity) {
    let actions = ctx.db.game_player_action();
    for mut r in actions.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            actions.id().update(r);
        }
    }
});

pub(crate) fn matching_actions<'a>(
    rows: impl Iterator<Item = &'a CreateinfoAction>,
    race: u8,
    class: u8,
) -> Vec<(u8, u32, u8)> {
    let mut seen_buttons = [false; 256];
    rows.filter(|r| createinfo_row_matches(r.race, r.class, race, class))
        .filter(|r| !std::mem::replace(&mut seen_buttons[r.button as usize], true))
        .map(|r| (r.button, r.action, r.action_type))
        .collect()
}

pub(crate) fn grant_createinfo_actions(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    race: u8,
    class: u8,
) {
    let actions = ctx.db.game_player_action();
    let rows: Vec<CreateinfoAction> = ctx.db.game_createinfo_action().iter().collect();
    for (button, action, action_type) in matching_actions(rows.iter(), race, class) {
        if actions
            .by_character()
            .filter(&guid)
            .any(|a| a.button == button)
        {
            continue;
        }
        actions.insert(PlayerAction {
            id: 0,
            character_guid: guid,
            owner_identity: owner,
            button,
            action,
            action_type,
        });
    }
}

pub(crate) fn apply_set_action_button(
    ctx: &ReducerContext,
    character: crate::WorldEntity,
    button: u8,
    action: u32,
    action_type: u8,
) -> Result<(), crate::Refusal> {
    let actions = ctx.db.game_player_action();
    let existing = actions
        .by_character()
        .filter(&character.guid)
        .find(|r| r.button == button);
    if action == 0 {
        if let Some(r) = existing {
            actions.id().delete(r.id);
        }
        return Ok(());
    }
    match existing {
        Some(mut r) => {
            r.action = action;
            r.action_type = action_type;
            actions.id().update(r);
        }
        None => {
            actions.insert(PlayerAction {
                id: 0,
                character_guid: character.guid,
                owner_identity: character.owner_identity,
                button,
                action,
                action_type,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        row_id: u64,
        race: u8,
        class: u8,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> CreateinfoAction {
        CreateinfoAction {
            row_id,
            race,
            class,
            button,
            action,
            action_type,
        }
    }

    #[test]
    fn createinfo_row_matches_is_reused_verbatim_for_actions() {
        assert!(createinfo_row_matches(1, 1, 1, 1));
        assert!(!createinfo_row_matches(1, 1, 1, 4));
        assert!(createinfo_row_matches(0, 1, 3, 1));
    }

    #[test]
    fn matching_actions_on_an_empty_table_creates_nothing() {
        let rows: Vec<CreateinfoAction> = Vec::new();
        assert!(matching_actions(rows.iter(), 1, 1).is_empty());
    }

    #[test]
    fn matching_actions_copies_only_the_matching_race_and_class() {
        let rows = [
            row(1, 1, 1, 0, 6603, 0),
            row(2, 1, 1, 1, 78, 0),
            row(3, 3, 1, 0, 6603, 0),
            row(4, 1, 5, 0, 6603, 0),
        ];
        let granted = matching_actions(rows.iter(), 1, 1);
        assert_eq!(granted, vec![(0, 6603, 0), (1, 78, 0)]);
    }

    #[test]
    fn matching_actions_grants_at_most_one_row_per_button() {
        let rows = [
            row(1, 1, 1, 0, 6603, 0),
            row(2, 0, 1, 0, 78, 0),
            row(3, 1, 1, 1, 78, 0),
        ];
        assert_eq!(
            matching_actions(rows.iter(), 1, 1),
            vec![(0, 6603, 0), (1, 78, 0)]
        );
    }

    #[test]
    fn matching_actions_respects_wildcards_like_the_spell_kit() {
        let rows = [row(1, 0, 1, 0, 6603, 0)];
        assert_eq!(matching_actions(rows.iter(), 1, 1), vec![(0, 6603, 0)]);
        assert_eq!(matching_actions(rows.iter(), 3, 1), vec![(0, 6603, 0)]);
        assert!(matching_actions(rows.iter(), 1, 4).is_empty());
    }
}
