//! Imported mail catalogue: letter bodies, the single item a letter may carry, and which quests send
//! one at turn-in. Sourced by the importer from `MailTemplate.dbc`, cmangos `mail_loot_template`, and
//! two `quest_template` columns plus the `SCRIPT_COMMAND_SEND_MAIL` `dbscripts_on_quest_end` rows.
//!
//! NOT public, the `game_import_meta` precedent (`import_meta.rs`): only the Module reads these
//! tables (`mail_reward` files a Reward Letter from them at turn-in), so the Gateway subscribes to
//! none. Replicated to every shard like other imported catalogues, because a quest can turn in on
//! any Home Shard.

use spacetimedb::table;

/// One imported letter body (`MailTemplate.dbc`). The client shows this text for a mail that names
/// `id` as its template; vanilla sends the id with an empty subject and lets the client render the
/// stored body, which carries `$B` (line break) and `$n` (recipient name) placeholders verbatim.
#[table(accessor = game_mail_template)]
pub struct MailTemplate {
    #[primary_key]
    pub id: u32,
    pub body: String,
}

/// The single item one mail template's letter attaches (cmangos `mail_loot_template`: chance 100,
/// group 0, count 1, no condition — a mail carries at most one item). A template with no row here is
/// a text- or money-only letter.
#[table(accessor = game_mail_loot)]
pub struct MailLoot {
    #[primary_key]
    pub mail_template_id: u32,
    pub item_entry: u32,
    pub count: u32,
}

/// One quest's reward mail, sent at turn-in. Sourced from either `quest_template.RewMailTemplateId`
/// (the giver sends it, no money) or a `dbscripts_on_quest_end` `SCRIPT_COMMAND_SEND_MAIL` row (an
/// alternative sender and/or money); the importer refuses a quest that carries both.
/// `sender_creature_entry == 0` means the turn-in giver sends the letter.
#[table(accessor = game_quest_reward_mail)]
pub struct QuestRewardMail {
    #[primary_key]
    pub quest_entry: u32,
    pub mail_template_id: u32,
    pub delay_secs: u32,
    pub sender_creature_entry: u32,
    pub money: u32,
}
