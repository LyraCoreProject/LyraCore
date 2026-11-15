use spacetimedb::{table, Identity, ReducerContext, SpacetimeType, Table};

use super::directory::{self, game_character_directory, CharacterProfile};
use crate::character::CreateCharacterRefusal;

#[table(accessor = game_character_name,
    index(accessor = by_guid, btree(columns = [guid])),
    index(accessor = by_account, btree(columns = [account_id])),
    index(accessor = by_selection, btree(columns = [selection_id])))]
pub struct CharacterName {
    #[primary_key]
    pub name: String,
    pub account_id: u64,
    pub world: Identity,
    pub selection_id: u64,
    pub guid: u64,
}

#[table(accessor = game_character_names_ready)]
pub struct CharacterNamesReady {
    #[primary_key]
    pub world: Identity,
}

#[derive(SpacetimeType, Clone, Copy)]
pub(super) struct NameReport {
    pub revision: u64,
    pub index: u32,
    pub total: u32,
}

#[table(accessor = game_character_name_report)]
pub struct CharacterNameReport {
    #[primary_key]
    pub world: Identity,
    pub revision: u64,
    pub total: u32,
    pub received: Vec<u32>,
}

crate::outside_table_catalog!(
    game_character_name,
    game_character_names_ready,
    game_character_name_report
);

pub(super) fn reported(ctx: &ReducerContext, report: NameReport) -> Result<(), String> {
    if report.total == 0 || report.index >= report.total {
        return Err("invalid Character name report batch".into());
    }
    let ready = ctx.db.game_character_names_ready();
    if ready.world().find(ctx.sender()).is_some() {
        return Ok(());
    }
    let reports = ctx.db.game_character_name_report();
    let old = reports.world().find(ctx.sender());
    if old
        .as_ref()
        .is_some_and(|old| old.revision > report.revision)
    {
        return Ok(());
    }
    let mut progress =
        old.filter(|old| old.revision == report.revision)
            .unwrap_or(CharacterNameReport {
                world: ctx.sender(),
                revision: report.revision,
                total: report.total,
                received: Vec::new(),
            });
    if progress.total != report.total {
        return Err("Character name report batch count changed".into());
    }
    if !progress.received.contains(&report.index) {
        progress.received.push(report.index);
    }
    reports.world().delete(ctx.sender());
    if progress.received.len() == report.total as usize {
        ready.insert(CharacterNamesReady {
            world: ctx.sender(),
        });
    } else {
        reports.insert(progress);
    }
    Ok(())
}

pub(super) fn reserve(
    ctx: &ReducerContext,
    name: &str,
    account_id: u64,
    world: Identity,
    selection_id: u64,
) -> Result<(), CreateCharacterRefusal> {
    if directory::worlds(ctx).iter().any(|world| {
        ctx.db
            .game_character_names_ready()
            .world()
            .find(*world)
            .is_none()
    }) {
        return Err("World Shard names have not been reported"
            .to_string()
            .into());
    }
    let names = ctx.db.game_character_name();
    let name = name.to_lowercase();
    if names.name().find(&name).is_some() {
        return Err(CreateCharacterRefusal::NameInUse);
    }
    if !account_has_room(ctx, account_id) {
        return Err(CreateCharacterRefusal::AccountFull);
    }
    names.insert(CharacterName {
        name,
        account_id,
        world,
        selection_id,
        guid: 0,
    });
    Ok(())
}

pub(crate) fn account_has_room(ctx: &ReducerContext, account_id: u64) -> bool {
    let names = ctx.db.game_character_name();
    let pending = names
        .by_account()
        .filter(account_id)
        .filter(|name| name.guid == 0)
        .count();
    let existing = ctx
        .db
        .game_character_directory()
        .by_account()
        .filter(account_id)
        .count();
    existing + pending < crate::character::CHARACTERS_PER_ACCOUNT
}

pub(super) fn release_package(ctx: &ReducerContext, name: &str, account_id: u64, world: Identity) {
    let names = ctx.db.game_character_name();
    let name = name.to_lowercase();
    if names.name().find(&name).is_some_and(|row| {
        row.guid == 0 && row.selection_id == 0 && row.account_id == account_id && row.world == world
    }) {
        names.name().delete(name);
    }
}

pub(super) fn observe(ctx: &ReducerContext, profile: &CharacterProfile) -> Result<(), String> {
    if profile.deleted {
        deleted(ctx, profile.guid);
        return Ok(());
    }
    let names = ctx.db.game_character_name();
    let name = profile.name.to_lowercase();
    if let Some(old) = names.name().find(&name) {
        let reserved =
            old.guid == 0 && old.account_id == profile.account_id && old.world == profile.world;
        if old.guid != profile.guid && !reserved {
            return Err(format!(
                "Character name conflicts with another reservation: {}",
                profile.name
            ));
        }
    }
    deleted(ctx, profile.guid);
    names.name().delete(&name);
    names.insert(CharacterName {
        name,
        account_id: profile.account_id,
        world: profile.world,
        selection_id: 0,
        guid: profile.guid,
    });
    Ok(())
}

pub(super) fn created(
    ctx: &ReducerContext,
    selection_id: u64,
    account_id: u64,
    changes: &[u8],
) -> Result<(), String> {
    if selection_id == 0 {
        return Ok(());
    }
    let names = ctx.db.game_character_name();
    let Some(mut name) = names.by_selection().filter(selection_id).next() else {
        return Ok(());
    };
    if name.world != ctx.sender() || name.account_id != account_id {
        return Err(
            "Character creation reservation belongs to another World Shard or Account".into(),
        );
    }
    let changes: Vec<(u64, bool)> =
        spacetimedb::sats::bsatn::from_slice(changes).map_err(|error| error.to_string())?;
    let [(guid, true)] = changes.as_slice() else {
        return Err("Character creation must report one new Character".into());
    };
    if ctx
        .db
        .game_character_directory()
        .guid()
        .find(*guid)
        .is_none()
    {
        names.name().delete(name.name);
        return Ok(());
    }
    name.guid = *guid;
    name.selection_id = 0;
    names.name().update(name);
    Ok(())
}

pub(super) fn refused(ctx: &ReducerContext, selection_id: u64, world: Identity) {
    if selection_id == 0 {
        return;
    }
    let names = ctx.db.game_character_name();
    let released: Vec<_> = names
        .by_selection()
        .filter(selection_id)
        .filter(|name| name.guid == 0 && name.world == world)
        .map(|name| name.name)
        .collect();
    for name in released {
        names.name().delete(name);
    }
}

pub(super) fn deleted(ctx: &ReducerContext, guid: u64) {
    let names = ctx.db.game_character_name();
    let released: Vec<_> = names.by_guid().filter(guid).map(|name| name.name).collect();
    for name in released {
        names.name().delete(name);
    }
}
