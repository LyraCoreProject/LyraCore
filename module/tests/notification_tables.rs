use lyracore_module::{
    game_chat_event, game_chat_event_live, game_combat_event, game_combat_event_live,
    game_emote_event, game_emote_event_live, game_spell_cast_event, game_spell_cast_event_live,
    game_spell_impact_event, game_spell_impact_event_live,
};
use spacetimedb::{Local, Table};

fn additive_event<Legacy: Table, Live: Table<Row = Legacy::Row>>(
    _legacy: fn(&Local) -> Legacy,
    _live: fn(&Local) -> Live,
) {
    assert!(
        !Legacy::IS_EVENT,
        "{} must stay ordinary",
        Legacy::TABLE_NAME
    );
    assert!(Live::IS_EVENT, "{} must be transient", Live::TABLE_NAME);
    assert_eq!(Live::TABLE_NAME, format!("{}_live", Legacy::TABLE_NAME));
}

#[test]
fn native_notifications_keep_the_existing_tables_and_row_types() {
    additive_event(Local::game_combat_event, Local::game_combat_event_live);
    additive_event(
        Local::game_spell_cast_event,
        Local::game_spell_cast_event_live,
    );
    additive_event(
        Local::game_spell_impact_event,
        Local::game_spell_impact_event_live,
    );
    additive_event(Local::game_chat_event, Local::game_chat_event_live);
    additive_event(Local::game_emote_event, Local::game_emote_event_live);
}
