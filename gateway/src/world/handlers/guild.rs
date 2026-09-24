//! Guild family: the guild window reads, CMSG_GUILD_CREATE, the `.guild create` dot-command, the
//! tabard designer, and the guild steps of world entry and exit. Every guild fact lives on
//! Realm-core, so every read and Durable Request here goes there. The Gateway adds only the
//! Character facts Realm-core cannot read: name, team, Realm Account, whether the Character is live,
//! and the actor's GM level from its Home Shard. A guild fee moves through `guild_fee`.

use super::super::guild_fee::{self, GuildFeeStore};
use super::super::*;
use lyracore_shared::guild::{event_kind, has_right, rights, GuildRefusal, LEADER_RANK};
use wow_world_messages::vanilla::{
    GuildCommand, GuildCommandResult, GuildEmblemResult, MSG_SAVE_GUILD_EMBLEM_Client,
    MSG_SAVE_GUILD_EMBLEM_Server, MSG_TABARDVENDOR_ACTIVATE,
};
use wow_world_messages::Guid;

/// Character facts a guild Gate or render needs, read from whichever World Shard holds the
/// Character.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CharacterFacts {
    pub(crate) guid: u64,
    pub(crate) name: String,
    pub(crate) race: u8,
    pub(crate) class: u8,
    pub(crate) level: u8,
    pub(crate) zone_id: u32,
    pub(crate) last_logout_micros: u64,
    /// A live entity on any World Shard, bots included.
    pub(crate) online: bool,
    /// 0 when no World Shard retains the Character's Realm Account.
    pub(crate) realm_account_id: u64,
}

/// One guild Durable Request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum GuildRequest {
    /// A GM founds a Guild led by `leader_guid`.
    GmCreate {
        leader_guid: u64,
        leader_name: String,
        leader_team: u32,
        leader_realm_account: u64,
        gm_level: u8,
        name: String,
    },
    /// The actor entered the world; `actor_name` refreshes its member name snapshot.
    SignOn { actor_name: String },
    /// The actor left the world.
    SignOff,
}

/// What Realm-core did with a guild Durable Request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GuildOutcome {
    Ran,
    Refused(GuildRefusal),
}

/// Guild reads and requests, in the seam's own vocabulary. Guild reads and `guild_op` go to
/// Realm-core; Character facts are realm-wide reads over the World Shards. Guild fees go through
/// [`GuildFeeStore`].
pub(crate) trait GuildActionStore: GuildFeeStore + Send + Sync {
    /// The membership row of `character_guid`, if it is in a Guild.
    fn guild_member(&self, character_guid: u64) -> Result<Option<codec::GuildMemberView>>;
    /// One Guild with its Guild Ranks.
    fn guild(&self, guild_id: u32) -> Result<Option<codec::GuildView>>;
    /// Every member row of one Guild.
    fn guild_members(&self, guild_id: u32) -> Result<Vec<codec::GuildMemberView>>;
    /// Facts for one Character from whichever World Shard holds it.
    fn guild_character_facts(&self, character_guid: u64) -> Result<Option<CharacterFacts>>;
    /// Every Character guid that carries `name`, across every World Shard.
    fn guild_characters_named(&self, name: &str) -> Result<Vec<u64>>;
    /// The actor's GM level, read from THIS handle: the actor's Home Shard. A copy an interrupted
    /// Transfer left on another shard is not authority.
    fn guild_gm_level(&self, actor_guid: u64) -> Result<u8>;
    /// The unit the actor has selected, 0 for none.
    fn guild_selected_target(&self, actor_guid: u64) -> u64;
    /// Run one guild op on Realm-core as `actor_guid`.
    fn guild_op(&self, actor_guid: u64, request: GuildRequest) -> Result<GuildOutcome>;
    /// Does the NPC refuse the actor by faction? The one Gate a read path answers here.
    fn guild_npc_refuses(&self, npc_guid: u64, actor_guid: u64) -> Result<bool>;
}

impl GuildActionStore for crate::stdb::Coordinator {
    fn guild_member(&self, character_guid: u64) -> Result<Option<codec::GuildMemberView>> {
        Ok(self.realm_core()?.guild_member_row(character_guid))
    }

    fn guild(&self, guild_id: u32) -> Result<Option<codec::GuildView>> {
        Ok(self.realm_core()?.guild_row(guild_id))
    }

    fn guild_members(&self, guild_id: u32) -> Result<Vec<codec::GuildMemberView>> {
        Ok(self.realm_core()?.guild_member_rows(guild_id))
    }

    fn guild_character_facts(&self, character_guid: u64) -> Result<Option<CharacterFacts>> {
        Ok(crate::stdb::Coordinator::guild_character_facts(
            self,
            character_guid,
        ))
    }

    fn guild_characters_named(&self, name: &str) -> Result<Vec<u64>> {
        party::resolve_all_by_name(self, name)
    }

    fn guild_gm_level(&self, actor_guid: u64) -> Result<u8> {
        Ok(crate::stdb::Coordinator::home_gm_level(self, actor_guid))
    }

    fn guild_selected_target(&self, actor_guid: u64) -> u64 {
        crate::stdb::Coordinator::selected_target(self, actor_guid)
    }

    fn guild_op(&self, actor_guid: u64, request: GuildRequest) -> Result<GuildOutcome> {
        self.realm_core()?.realm_guild_op(actor_guid, request)
    }

    fn guild_npc_refuses(&self, npc_guid: u64, actor_guid: u64) -> Result<bool> {
        crate::stdb::Coordinator::npc_refuses_interaction(self, npc_guid, actor_guid)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GuildActionPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
}

pub(crate) enum GuildActionOutcome {
    Handled { outbound: Vec<Outbound> },
    PassThrough(ClientOpcodeMessage),
}

/// A lost reducer transport ends the session. Every other failure is a guild outcome the client
/// hears as silence.
fn is_fatal(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
}

pub(crate) fn dispatch_guild_action<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    msg: ClientOpcodeMessage,
) -> Result<GuildActionOutcome> {
    let outbound = match msg {
        ClientOpcodeMessage::CMSG_GUILD_QUERY(query) => query_outbound(store, query.guild_id),
        ClientOpcodeMessage::CMSG_GUILD_ROSTER => roster_outbound(store, player),
        ClientOpcodeMessage::CMSG_GUILD_INFO => info_outbound(store, player),
        ClientOpcodeMessage::CMSG_GUILD_CREATE(create) => {
            create_outbound(store, player, create.guild_name)
        }
        ClientOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(activate) => {
            tabard_window_outbound(store, player, activate.guid.guid())
        }
        ClientOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(save) => {
            save_emblem_outbound(store, player, &save)
        }
        other => return Ok(GuildActionOutcome::PassThrough(other)),
    };
    match outbound {
        Ok(outbound) => Ok(GuildActionOutcome::Handled { outbound }),
        Err(error) if is_fatal(&error) => Err(error),
        Err(error) => {
            log::debug!(
                "world: guild request dropped (account {}): {error:#}",
                player.account_id
            );
            Ok(GuildActionOutcome::Handled {
                outbound: Vec::new(),
            })
        }
    }
}

fn command_result(command: GuildCommand, name: String, result: GuildCommandResult) -> Outbound {
    Outbound::One(ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(Box::new(
        codec::build_guild_command_result(command, name, result),
    )))
}

/// What mangos answers a guild request from outside any Guild (`cm:GuildHandler.cpp:43,248`).
fn not_in_guild() -> Outbound {
    command_result(
        GuildCommand::Create,
        String::new(),
        GuildCommandResult::GuildPlayerNotInGuild,
    )
}

/// Any Guild answers, in any state: the character list asks for the names of its Characters'
/// Guilds before world entry (`cm:Opcodes.cpp:112` STATUS_AUTHED).
fn query_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    guild_id: u32,
) -> Result<Vec<Outbound>> {
    Ok(vec![match store.guild(guild_id)? {
        Some(guild) => Outbound::One(ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
            codec::build_guild_query_response(&guild),
        ))),
        None => not_in_guild(),
    }])
}

/// The actor's membership and its Guild, when both exist.
fn actor_guild<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Option<(codec::GuildMemberView, codec::GuildView)>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(None);
    };
    let Some(member) = store.guild_member(actor_guid)? else {
        return Ok(None);
    };
    Ok(store.guild(member.guild_id)?.map(|guild| (member, guild)))
}

/// Outside a Guild the client gets no reply (`cm:GuildHandler.cpp:265-266`).
fn roster_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some((viewer, guild)) = actor_guild(store, player)? else {
        return Ok(Vec::new());
    };
    let members = store.guild_members(guild.guild_id)?;
    let sees_officer_notes = has_right(guild.rank_rights(viewer.rank_id), rights::VIEWOFFNOTE);
    let now = now_micros();
    // Lazy, so the size cap also stops the Character reads.
    let lines = members
        .into_iter()
        .map(|member| roster_line(store, member, sees_officer_notes, now));
    Ok(vec![Outbound::One(ServerOpcodeMessage::SMSG_GUILD_ROSTER(
        Box::new(codec::build_guild_roster(&guild, lines)),
    ))])
}

/// One roster line. A member whose World Shard cannot answer still lists, offline, under its name
/// snapshot.
fn roster_line<St: GuildActionStore + ?Sized>(
    store: &St,
    member: codec::GuildMemberView,
    sees_officer_notes: bool,
    now_micros: u64,
) -> codec::GuildRosterLine {
    let facts = store
        .guild_character_facts(member.character_guid)
        .ok()
        .flatten();
    let officer_note = if sees_officer_notes {
        member.officer_note
    } else {
        String::new()
    };
    let Some(facts) = facts else {
        return codec::GuildRosterLine {
            guid: member.character_guid,
            name: member.name,
            rank_id: member.rank_id,
            days_offline: Some(0.0),
            public_note: member.public_note,
            officer_note,
            ..codec::GuildRosterLine::default()
        };
    };
    let days_offline = (!facts.online).then(|| {
        if facts.last_logout_micros == 0 {
            0.0
        } else {
            now_micros.saturating_sub(facts.last_logout_micros) as f32 / 86_400_000_000.0
        }
    });
    codec::GuildRosterLine {
        guid: member.character_guid,
        name: facts.name,
        rank_id: member.rank_id,
        level: facts.level,
        class: facts.class,
        zone_id: facts.zone_id,
        days_offline,
        public_note: member.public_note,
        officer_note,
    }
}

fn info_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
) -> Result<Vec<Outbound>> {
    let Some((_, guild)) = actor_guild(store, player)? else {
        return Ok(vec![not_in_guild()]);
    };
    let members = store.guild_members(guild.guild_id)?;
    let member_count = u32::try_from(members.len()).unwrap_or(u32::MAX);
    Ok(vec![Outbound::One(ServerOpcodeMessage::SMSG_GUILD_INFO(
        Box::new(codec::build_guild_info(
            &guild,
            member_count,
            account_count(&members),
        )),
    ))])
}

/// Distinct Realm Accounts among `members`. A member whose Account is unknown counts as one.
fn account_count(members: &[codec::GuildMemberView]) -> u32 {
    let mut known: Vec<u64> = members
        .iter()
        .map(|member| member.realm_account_id)
        .filter(|account| *account != 0)
        .collect();
    known.sort_unstable();
    known.dedup();
    let unknown = members
        .iter()
        .filter(|member| member.realm_account_id == 0)
        .count();
    u32::try_from(known.len() + unknown).unwrap_or(u32::MAX)
}

/// CMSG_GUILD_CREATE founds a Guild for a GM only; the Guild Charter is the player path. mangos
/// replies to neither outcome (`cm:GuildHandler.cpp:46-64`).
fn create_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    name: String,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let gm_level = store.guild_gm_level(actor_guid)?;
    if gm_level == 0 {
        return Ok(Vec::new());
    }
    let Some(actor) = store.guild_character_facts(actor_guid)? else {
        return Ok(Vec::new());
    };
    let outcome = store.guild_op(actor_guid, gm_create(&actor, gm_level, name))?;
    if let GuildOutcome::Refused(refusal) = outcome {
        log::debug!("world: CMSG_GUILD_CREATE from {actor_guid} refused: {refusal:?}");
    }
    Ok(Vec::new())
}

fn gm_create(leader: &CharacterFacts, gm_level: u8, name: String) -> GuildRequest {
    GuildRequest::GmCreate {
        leader_guid: leader.guid,
        leader_name: leader.name.clone(),
        leader_team: lyracore_shared::faction::team_for_race(leader.race),
        leader_realm_account: leader.realm_account_id,
        gm_level,
        name,
    }
}

const GUILD_CREATE_USAGE: &str = "usage: .guild create [<leader>] \"<name>\"";

/// Is this Say line a `.guild` dot-command?
pub(crate) fn is_guild_dot_command(text: &str) -> bool {
    text.strip_prefix(".guild")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// `.guild create [<leader>] "<name>"` (`cm:Level3.cpp:2945-2977`), GM level only. Without a
/// leader name the leader is the actor's selected player, else the actor. `Some(line)` is the
/// system line for the actor; success is silent, like every other dot-command.
pub(crate) fn run_guild_dot_command<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    text: &str,
) -> Result<Option<String>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(None);
    };
    let gm_level = store.guild_gm_level(actor_guid)?;
    if gm_level == 0 {
        return Ok(Some("permission denied".into()));
    }
    let Some((leader_name, guild_name)) = parse_guild_create(text) else {
        return Ok(Some(GUILD_CREATE_USAGE.into()));
    };
    let leader = match leader_name {
        Some(typed) => match store.guild_characters_named(typed)?.as_slice() {
            [] => return Ok(Some(format!("no player named {typed}"))),
            [guid] => store.guild_character_facts(*guid)?,
            _ => return Ok(Some(format!("more than one player named {typed}"))),
        },
        None => match store.guild_selected_target(actor_guid) {
            0 => store.guild_character_facts(actor_guid)?,
            target => match store.guild_character_facts(target)? {
                Some(selected) => Some(selected),
                None => store.guild_character_facts(actor_guid)?,
            },
        },
    };
    let Some(leader) = leader else {
        return Ok(Some(format!(
            "no player named {}",
            leader_name.unwrap_or_default()
        )));
    };
    let outcome = store.guild_op(
        actor_guid,
        gm_create(&leader, gm_level, guild_name.to_string()),
    );
    Ok(match outcome {
        Ok(GuildOutcome::Ran) => None,
        Ok(GuildOutcome::Refused(GuildRefusal::NotGameMaster)) => Some("permission denied".into()),
        Ok(GuildOutcome::Refused(GuildRefusal::AlreadyInGuild)) => {
            Some(format!("{} is already in a guild", leader.name))
        }
        Ok(GuildOutcome::Refused(_)) => Some("guild not created".into()),
        Err(error) if is_fatal(&error) => return Err(error),
        Err(error) => {
            log::warn!("world: .guild create by {actor_guid} failed: {error:#}");
            Some("guild not created".into())
        }
    })
}

/// Split `.guild create [<leader>] "<name>"` into the optional leader name and the quoted guild
/// name. `None` for any other shape.
fn parse_guild_create(text: &str) -> Option<(Option<&str>, &str)> {
    let rest = text.strip_prefix(".guild")?.trim_start();
    let (subcommand, rest) = rest.split_once(char::is_whitespace)?;
    if !subcommand.eq_ignore_ascii_case("create") {
        return None;
    }
    let rest = rest.trim();
    let (leader, quoted) = match rest.strip_prefix('"') {
        Some(_) => (None, rest),
        None => {
            let (leader, quoted) = rest.split_once(char::is_whitespace)?;
            (Some(leader), quoted.trim_start())
        }
    };
    let name = quoted.strip_prefix('"')?.strip_suffix('"')?;
    (!name.is_empty() && !name.contains('"')).then_some((leader, name))
}

/// MSG_TABARDVENDOR_ACTIVATE opens the tabard designer (`cm:NPCHandler.cpp:47-67`). This is a read
/// path, so only the faction refusal is answered here, silently. The NPC flag, reach and life Gates
/// run in the Fee Hold when the emblem is saved.
fn tabard_window_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    npc_guid: u64,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    if store.guild_npc_refuses(npc_guid, actor_guid)? {
        return Ok(Vec::new());
    }
    Ok(vec![tabard_designer_window(npc_guid)])
}

/// The tabard designer window for `npc_guid`. Gossip action 11 opens it too
/// (`cm:Player.cpp:11877-11880`).
pub(crate) fn tabard_designer_window(npc_guid: u64) -> Outbound {
    Outbound::One(ServerOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(
        MSG_TABARDVENDOR_ACTIVATE {
            guid: Guid::new(npc_guid),
        },
    ))
}

/// MSG_SAVE_GUILD_EMBLEM (`cm:GuildHandler.cpp:716-771`). Advisory reads of the Realm-core cache
/// answer NO_GUILD and NOT_GUILD_MASTER before any copper moves, and Realm-core decides again inside
/// the fee. mangos checks the NPC first. Here the NPC Gate runs in the Fee Hold, so a non-member at
/// the wrong NPC hears NO_GUILD. The new emblem reaches every online member, the payer included,
/// through the TABARD_CHANGED Guild Event.
fn save_emblem_outbound<St: GuildActionStore + ?Sized>(
    store: &St,
    player: GuildActionPlayer,
    save: &MSG_SAVE_GUILD_EMBLEM_Client,
) -> Result<Vec<Outbound>> {
    let Some(actor_guid) = player.self_guid else {
        return Ok(Vec::new());
    };
    let result = match store.guild_member(actor_guid)? {
        None => GuildEmblemResult::NoGuild,
        Some(member) if member.rank_id != LEADER_RANK => GuildEmblemResult::NotGuildMaster,
        Some(_) => {
            let request = guild_fee::FeeRequest::Emblem {
                npc_guid: save.vendor.guid(),
                emblem: guild_fee::Emblem {
                    emblem_style: save.emblem_style,
                    emblem_color: save.emblem_color,
                    border_style: save.border_style,
                    border_color: save.border_color,
                    background_color: save.background_color,
                },
            };
            emblem_result(guild_fee::pay(store, actor_guid, request)?)
        }
    };
    Ok(vec![Outbound::One(
        ServerOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(MSG_SAVE_GUILD_EMBLEM_Server { result }),
    )])
}

fn emblem_result(outcome: guild_fee::FeeOutcome) -> GuildEmblemResult {
    use guild_fee::FeeOutcome::{Accepted, Refused};
    match outcome {
        Accepted => GuildEmblemResult::Success,
        Refused(GuildRefusal::NotInGuild) => GuildEmblemResult::NoGuild,
        Refused(GuildRefusal::NotLeader) => GuildEmblemResult::NotGuildMaster,
        Refused(GuildRefusal::NotEnoughMoney) => GuildEmblemResult::NotEnoughMoney,
        // The NPC Gate. mangos names the value INVALIDVENDOR; the client shows nothing for it
        // (`vm:src/game/Handlers/GuildHandler.cpp:686-693`).
        Refused(_) => GuildEmblemResult::NoMessage,
    }
}

/// The Guild Projection for `character_guid`: `(guild_id, rank_id)`, or `(0, 0)` outside a Guild or
/// when Realm-core cannot answer.
pub(crate) fn guild_projection<St: GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
) -> (u32, u32) {
    match store.guild_member(character_guid) {
        Ok(Some(member)) => (member.guild_id, member.rank_id),
        Ok(None) => (0, 0),
        Err(error) => {
            log::warn!("world: Guild Projection for {character_guid} unreadable: {error:#}");
            (0, 0)
        }
    }
}

/// The guild packets of world entry, sent after the viewer is registered. The self CREATE
/// carried `created`, the Guild Projection read before registration; a membership change that
/// landed in between reached no viewer, so a different current value goes out as a VALUES update,
/// `(0, 0)` after a removal. A fresh login of a member also gets the MOTD
/// (`cm:CharacterHandler.cpp:775-785`). A failed read logs and never fails the entry.
pub(crate) fn guild_world_entry<St: GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
    entry: codec::WorldEntry,
    created: (u32, u32),
) -> Vec<Outbound> {
    let member = match store.guild_member(character_guid) {
        Ok(member) => member,
        Err(error) => {
            log::warn!("world: guild entry for {character_guid} unreadable: {error:#}");
            return Vec::new();
        }
    };
    let (guild_id, rank_id) = member
        .as_ref()
        .map_or((0, 0), |member| (member.guild_id, member.rank_id));
    let mut outbound = Vec::new();
    if (guild_id, rank_id) != created {
        let (opcode, body) = codec::build_guild_values(character_guid, guild_id, rank_id);
        outbound.push(Outbound::Raw { opcode, body });
    }
    if member.is_none() || entry != codec::WorldEntry::FreshLogin {
        return outbound;
    }
    match store.guild(guild_id) {
        Ok(Some(guild)) => {
            let (opcode, body) = codec::build_guild_event_raw(event_kind::MOTD, &[guild.motd], 0);
            outbound.push(Outbound::Raw { opcode, body });
        }
        Ok(None) => {}
        Err(error) => log::warn!("world: guild MOTD for {character_guid} unreadable: {error:#}"),
    }
    outbound
}

/// A member entering the world fresh signs on, which tells the other online members. Answers
/// whether SIGNED_ON went out, so the World Session owes a sign-off whatever happens next.
pub(crate) fn guild_sign_on<St: GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
) -> bool {
    if !matches!(store.guild_member(character_guid), Ok(Some(_))) {
        return false;
    }
    let actor_name = store
        .guild_character_facts(character_guid)
        .ok()
        .flatten()
        .map(|facts| facts.name)
        .unwrap_or_default();
    run_best_effort(
        store,
        character_guid,
        GuildRequest::SignOn { actor_name },
        "sign-on",
    )
}

/// The guild part of leaving the world: a member signs off. Runs before the Account Claim is
/// released, while Realm-core still accepts the actor (`cm:WorldSession.cpp:714-724`).
pub(crate) fn guild_world_exit<St: GuildActionStore + ?Sized>(store: &St, character_guid: u64) {
    if matches!(store.guild_member(character_guid), Ok(Some(_))) {
        let _ = run_best_effort(store, character_guid, GuildRequest::SignOff, "sign-off");
    }
}

/// Run one guild op whose failure must not fail the caller. Answers whether it ran.
fn run_best_effort<St: GuildActionStore + ?Sized>(
    store: &St,
    character_guid: u64,
    request: GuildRequest,
    step: &str,
) -> bool {
    match store.guild_op(character_guid, request) {
        Ok(GuildOutcome::Ran) => true,
        Ok(GuildOutcome::Refused(refusal)) => {
            log::debug!("world: guild {step} for {character_guid} refused: {refusal:?}");
            false
        }
        Err(error) => {
            log::warn!("world: guild {step} for {character_guid} failed: {error:#}");
            false
        }
    }
}

/// Resolve a typed member name against a Guild's name snapshots, case-insensitively, as mangos
/// searches its member list (`cm:Guild.h:285-292`). No match and two matches both answer
/// [`MemberMatch::NotInGuild`], so a by-name op never picks one of two homonyms.
// No caller yet: the by-name member ops (promote, demote, remove, notes) build on it.
#[allow(dead_code)]
pub(crate) fn member_by_name(members: &[codec::GuildMemberView], name: &str) -> MemberMatch {
    let mut found = members
        .iter()
        .filter(|member| member.name.to_lowercase() == name.to_lowercase());
    match (found.next(), found.next()) {
        (Some(member), None) => MemberMatch::Found(member.character_guid),
        _ => MemberMatch::NotInGuild,
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemberMatch {
    Found(u64),
    NotInGuild,
}

fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::guild::{founding_gate, name_key, DEFAULT_MOTD, DEFAULT_RANKS};
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        GuildMember, GuildMember_GuildMemberStatus, CMSG_GUILD_CREATE, CMSG_GUILD_QUERY, CMSG_PING,
    };

    const GM: u64 = 5_090_001;
    const BOB: u64 = 5_090_002;
    const CAROL: u64 = 5_090_003;

    /// An in-memory Realm-core plus World Shards. `guild_op` applies the same Gates the Module
    /// applies, so outcomes are observable as rows.
    #[derive(Default)]
    struct InMemoryGuildActions {
        guilds: Mutex<Vec<codec::GuildView>>,
        members: Mutex<Vec<codec::GuildMemberView>>,
        characters: Vec<CharacterFacts>,
        /// GM levels as each actor's Home Shard holds them.
        gm_levels: Vec<(u64, u8)>,
        selected: u64,
        ops: Mutex<Vec<(u64, GuildRequest)>>,
        op_error: Option<String>,
        /// NPCs that refuse every actor by faction.
        refusing_npcs: Vec<u64>,
        /// Every fee request that reached the Home Shard.
        fee_requests: Mutex<Vec<guild_fee::FeeRequest>>,
        /// What the Home Shard answers a hold; `None` holds.
        hold_refusal: Option<GuildRefusal>,
        /// What Realm-core decides; `None` accepts.
        fee_refusal: Option<GuildRefusal>,
    }

    impl InMemoryGuildActions {
        fn guild_named(&self, name: &str) -> Option<codec::GuildView> {
            self.guilds
                .lock()
                .unwrap()
                .iter()
                .find(|guild| guild.name == name)
                .cloned()
        }

        fn add_member(&self, guild_id: u32, character_guid: u64, rank_id: u32, note: &str) {
            self.members.lock().unwrap().push(codec::GuildMemberView {
                character_guid,
                guild_id,
                rank_id,
                name: format!("Snapshot{character_guid}"),
                public_note: note.into(),
                officer_note: format!("officer {note}"),
                realm_account_id: character_guid,
            });
        }
    }

    impl GuildActionStore for InMemoryGuildActions {
        fn guild_member(&self, character_guid: u64) -> Result<Option<codec::GuildMemberView>> {
            Ok(self
                .members
                .lock()
                .unwrap()
                .iter()
                .find(|member| member.character_guid == character_guid)
                .cloned())
        }

        fn guild(&self, guild_id: u32) -> Result<Option<codec::GuildView>> {
            Ok(self
                .guilds
                .lock()
                .unwrap()
                .iter()
                .find(|guild| guild.guild_id == guild_id)
                .cloned())
        }

        fn guild_members(&self, guild_id: u32) -> Result<Vec<codec::GuildMemberView>> {
            Ok(self
                .members
                .lock()
                .unwrap()
                .iter()
                .filter(|member| member.guild_id == guild_id)
                .cloned()
                .collect())
        }

        fn guild_character_facts(&self, character_guid: u64) -> Result<Option<CharacterFacts>> {
            Ok(self
                .characters
                .iter()
                .find(|facts| facts.guid == character_guid)
                .cloned())
        }

        fn guild_characters_named(&self, name: &str) -> Result<Vec<u64>> {
            Ok(self
                .characters
                .iter()
                .filter(|facts| facts.name.eq_ignore_ascii_case(name))
                .map(|facts| facts.guid)
                .collect())
        }

        fn guild_gm_level(&self, actor_guid: u64) -> Result<u8> {
            Ok(self
                .gm_levels
                .iter()
                .find(|(guid, _)| *guid == actor_guid)
                .map_or(0, |(_, level)| *level))
        }

        fn guild_selected_target(&self, _actor_guid: u64) -> u64 {
            self.selected
        }

        fn guild_op(&self, actor_guid: u64, request: GuildRequest) -> Result<GuildOutcome> {
            self.ops.lock().unwrap().push((actor_guid, request.clone()));
            if let Some(error) = &self.op_error {
                return Err(anyhow!("{error}"));
            }
            let GuildRequest::GmCreate {
                leader_guid,
                gm_level,
                name,
                ..
            } = request
            else {
                return Ok(GuildOutcome::Ran);
            };
            let refused = |refusal: GuildRefusal| -> Result<GuildOutcome> {
                Ok(GuildOutcome::Refused(refusal))
            };
            if gm_level == 0 {
                return refused(GuildRefusal::NotGameMaster);
            }
            let leader_in_guild = self.guild_member(leader_guid)?.is_some();
            let mut guilds = self.guilds.lock().unwrap();
            let taken = |key: &str| guilds.iter().any(|guild| name_key(&guild.name) == key);
            if let Err(refusal) = founding_gate(&name, taken, leader_in_guild) {
                return refused(refusal);
            }
            let guild_id = guilds.len() as u32 + 1;
            guilds.push(codec::GuildView {
                guild_id,
                name,
                leader_guid,
                motd: DEFAULT_MOTD.into(),
                ranks: DEFAULT_RANKS
                    .iter()
                    .zip(0u32..)
                    .map(|((name, rights), rank_id)| codec::GuildRankView {
                        rank_id,
                        name: (*name).into(),
                        rights: *rights,
                    })
                    .collect(),
                ..codec::GuildView::default()
            });
            drop(guilds);
            self.add_member(guild_id, leader_guid, 0, "");
            Ok(GuildOutcome::Ran)
        }

        fn guild_npc_refuses(&self, npc_guid: u64, _actor_guid: u64) -> Result<bool> {
            Ok(self.refusing_npcs.contains(&npc_guid))
        }
    }

    /// The fee answers the seam maps to result codes. `guild_fee`'s own tests drive the protocol.
    impl GuildFeeStore for InMemoryGuildActions {
        fn guild_fee_held(&self, _actor_guid: u64) -> Result<Option<guild_fee::FeeHold>> {
            Ok(None)
        }

        fn guild_fee_hold(
            &self,
            _actor_guid: u64,
            request: guild_fee::FeeRequest,
        ) -> Result<Result<guild_fee::FeeHold, GuildRefusal>> {
            self.fee_requests.lock().unwrap().push(request);
            if let Some(refusal) = self.hold_refusal {
                return Ok(Err(refusal));
            }
            let guild_fee::FeeRequest::Emblem { emblem, .. } = request;
            Ok(Ok(guild_fee::FeeHold {
                operation_id: 1,
                terms: guild_fee::FeeTerms::Emblem(emblem),
            }))
        }

        fn guild_fee_decide(
            &self,
            _actor_guid: u64,
            _hold: guild_fee::FeeHold,
        ) -> Result<guild_fee::FeeOutcome> {
            Ok(self.fee_refusal.map_or(
                guild_fee::FeeOutcome::Accepted,
                guild_fee::FeeOutcome::Refused,
            ))
        }

        fn guild_fee_finish(
            &self,
            _actor_guid: u64,
            _operation_id: u64,
            _accepted: bool,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn facts(guid: u64, name: &str) -> CharacterFacts {
        CharacterFacts {
            guid,
            name: name.into(),
            race: 1,
            class: 1,
            level: 60,
            zone_id: 12,
            last_logout_micros: 0,
            online: true,
            realm_account_id: guid,
        }
    }

    fn realm() -> InMemoryGuildActions {
        InMemoryGuildActions {
            characters: vec![
                facts(GM, "Gamemaster"),
                facts(BOB, "Bob"),
                facts(CAROL, "Carol"),
            ],
            gm_levels: vec![(GM, 1)],
            ..InMemoryGuildActions::default()
        }
    }

    fn in_world(guid: u64) -> GuildActionPlayer {
        GuildActionPlayer {
            account_id: 7,
            self_guid: Some(guid),
        }
    }

    fn dispatch(
        store: &InMemoryGuildActions,
        player: GuildActionPlayer,
        msg: ClientOpcodeMessage,
    ) -> Vec<Outbound> {
        match dispatch_guild_action(store, player, msg).unwrap() {
            GuildActionOutcome::Handled { outbound } => outbound,
            GuildActionOutcome::PassThrough(msg) => panic!("{msg} was not handled"),
        }
    }

    fn only_message(outbound: Vec<Outbound>) -> ServerOpcodeMessage {
        match <[Outbound; 1]>::try_from(outbound) {
            Ok([Outbound::One(message)]) => message,
            _ => panic!("expected exactly one typed message"),
        }
    }

    fn command_result_of(outbound: Vec<Outbound>) -> (GuildCommand, GuildCommandResult) {
        match only_message(outbound) {
            ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(result) => {
                (result.command, result.result)
            }
            other => panic!("expected a command result, got {other}"),
        }
    }

    #[test]
    fn dot_create_founds_a_guild_led_by_the_gm_when_nothing_is_selected() {
        let store = realm();
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create \"Tracer Guild\"").unwrap();
        assert_eq!(line, None);
        let guild = store.guild_named("Tracer Guild").expect("guild founded");
        assert_eq!(guild.leader_guid, GM);
        assert_eq!(guild.motd, "No message set.");
        assert_eq!(guild.ranks.len(), 5);
        assert_eq!(
            store
                .guild_member(GM)
                .unwrap()
                .map(|m| (m.guild_id, m.rank_id)),
            Some((guild.guild_id, 0))
        );
    }

    #[test]
    fn a_repeated_dot_create_of_the_same_name_answers_guild_not_created() {
        let store = realm();
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Tracer Guild\"").unwrap();
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create \"tracer guild\"").unwrap();
        assert_eq!(line.as_deref(), Some("guild not created"));
        assert_eq!(store.guilds.lock().unwrap().len(), 1);
        assert_eq!(store.members.lock().unwrap().len(), 1);
    }

    #[test]
    fn dot_create_with_a_taken_name_in_another_case_answers_guild_not_created() {
        let store = realm();
        run_guild_dot_command(&store, in_world(GM), ".guild create Bob \"Tracer Guild\"").unwrap();
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create Carol \"tracer guild\"")
                .unwrap();
        assert_eq!(line.as_deref(), Some("guild not created"));
        assert_eq!(store.guilds.lock().unwrap().len(), 1);
        assert_eq!(store.guild_member(CAROL).unwrap(), None);
    }

    #[test]
    fn dot_create_names_an_offline_leader_found_on_any_shard() {
        let mut store = realm();
        store.characters[1].online = false;
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create bob \"Knights\"").unwrap();
        assert_eq!(line, None);
        assert_eq!(store.guild_named("Knights").unwrap().leader_guid, BOB);
        let (_, request) = store.ops.lock().unwrap()[0].clone();
        assert_eq!(
            request,
            GuildRequest::GmCreate {
                leader_guid: BOB,
                leader_name: "Bob".into(),
                leader_team: lyracore_shared::faction::TEAM_ALLIANCE,
                leader_realm_account: BOB,
                gm_level: 1,
                name: "Knights".into(),
            }
        );
    }

    #[test]
    fn dot_create_for_a_leader_already_in_a_guild_names_the_leader() {
        let store = realm();
        store.add_member(9, BOB, 3, "");
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create Bob \"Knights\"").unwrap();
        assert_eq!(line.as_deref(), Some("Bob is already in a guild"));
        assert!(store.guilds.lock().unwrap().is_empty());
    }

    #[test]
    fn dot_create_for_an_unknown_leader_answers_no_player_named() {
        let store = realm();
        let line =
            run_guild_dot_command(&store, in_world(GM), ".guild create Dave \"Knights\"").unwrap();
        assert_eq!(line.as_deref(), Some("no player named Dave"));
        assert!(store.ops.lock().unwrap().is_empty());
    }

    #[test]
    fn dot_create_without_a_name_uses_the_selected_player() {
        let mut store = realm();
        store.selected = CAROL;
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"").unwrap();
        assert_eq!(store.guild_named("Knights").unwrap().leader_guid, CAROL);
    }

    #[test]
    fn dot_create_with_a_selected_creature_falls_back_to_the_actor() {
        let mut store = realm();
        store.selected = 0xF130_0000_0000_0042;
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"").unwrap();
        assert_eq!(store.guild_named("Knights").unwrap().leader_guid, GM);
    }

    #[test]
    fn dot_create_from_gm_level_zero_is_denied_before_any_request() {
        let store = realm();
        let line =
            run_guild_dot_command(&store, in_world(BOB), ".guild create \"Knights\"").unwrap();
        assert_eq!(line.as_deref(), Some("permission denied"));
        assert!(store.ops.lock().unwrap().is_empty());
        assert!(store.guilds.lock().unwrap().is_empty());
    }

    #[test]
    fn malformed_dot_commands_answer_the_usage_line() {
        let store = realm();
        for text in [
            ".guild",
            ".guild create",
            ".guild create Knights",
            ".guild create Bob Knights",
            ".guild create \"\"",
            ".guild invite Bob",
        ] {
            assert_eq!(
                run_guild_dot_command(&store, in_world(GM), text)
                    .unwrap()
                    .as_deref(),
                Some(GUILD_CREATE_USAGE),
                "{text}"
            );
        }
        assert!(store.ops.lock().unwrap().is_empty());
    }

    #[test]
    fn only_the_guild_word_selects_the_guild_command() {
        assert!(is_guild_dot_command(".guild create \"A B\""));
        assert!(is_guild_dot_command(".guild"));
        assert!(!is_guild_dot_command(".guilds"));
        assert!(!is_guild_dot_command(".gm on"));
    }

    #[test]
    fn a_lost_reducer_transport_ends_the_session() {
        let mut store = realm();
        store.op_error = Some("realm_guild_op reducer transport disconnected: gone".into());
        let error = run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"")
            .expect_err("fatal");
        assert!(error.to_string().contains("transport disconnected"));
    }

    #[test]
    fn guild_create_opcode_from_a_gm_founds_a_guild_and_replies_nothing() {
        let store = realm();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_CREATE(Box::new(CMSG_GUILD_CREATE {
                guild_name: "Knights".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert_eq!(store.guild_named("Knights").unwrap().leader_guid, GM);
    }

    #[test]
    fn guild_create_opcode_from_a_player_changes_nothing_and_sends_nothing() {
        let store = realm();
        let outbound = dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_CREATE(Box::new(CMSG_GUILD_CREATE {
                guild_name: "Knights".into(),
            })),
        );
        assert!(outbound.is_empty());
        assert!(store.ops.lock().unwrap().is_empty());
        assert!(store.guilds.lock().unwrap().is_empty());
    }

    #[test]
    fn guild_query_answers_any_guild_at_character_select() {
        let store = realm();
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"").unwrap();
        let character_select = GuildActionPlayer {
            account_id: 7,
            self_guid: None,
        };
        let message = only_message(dispatch(
            &store,
            character_select,
            ClientOpcodeMessage::CMSG_GUILD_QUERY(CMSG_GUILD_QUERY { guild_id: 1 }),
        ));
        let ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(response) = message else {
            panic!("expected a query response, got {message}");
        };
        assert_eq!(response.name, "Knights");
        assert_eq!(response.rank_names[0], "Guild Master");
        assert_eq!(response.rank_names[4], "Initiate");
        assert_eq!(response.rank_names[5], "");
    }

    #[test]
    fn guild_query_for_an_unknown_guild_answers_not_in_guild() {
        let store = realm();
        let outbound = dispatch(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_GUILD_QUERY(CMSG_GUILD_QUERY { guild_id: 404 }),
        );
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    fn founded_with_members() -> InMemoryGuildActions {
        let mut store = realm();
        store.characters[2].online = false;
        // A day and a half ago: 1.5 * 86_400_000_000 micros.
        store.characters[2].last_logout_micros = now_micros() - 129_600_000_000;
        run_guild_dot_command(&store, in_world(GM), ".guild create \"Knights\"").unwrap();
        store.add_member(1, BOB, 3, "bob note");
        store.add_member(1, CAROL, 4, "carol note");
        store
    }

    fn roster_for(store: &InMemoryGuildActions, viewer: u64) -> Vec<GuildMember> {
        match only_message(dispatch(
            store,
            in_world(viewer),
            ClientOpcodeMessage::CMSG_GUILD_ROSTER,
        )) {
            ServerOpcodeMessage::SMSG_GUILD_ROSTER(roster) => roster.members,
            other => panic!("expected a roster, got {other}"),
        }
    }

    #[test]
    fn roster_lists_live_and_offline_members() {
        let store = founded_with_members();
        let members = roster_for(&store, GM);
        assert_eq!(members.len(), 3);
        let bob = members.iter().find(|m| m.guid.guid() == BOB).unwrap();
        assert_eq!(bob.name, "Bob");
        assert_eq!(bob.status, GuildMember_GuildMemberStatus::Online);
        assert_eq!(bob.rank, 3);
        let carol = members.iter().find(|m| m.guid.guid() == CAROL).unwrap();
        let GuildMember_GuildMemberStatus::Offline { time_offline } = carol.status else {
            panic!("Carol is offline");
        };
        assert!(
            (time_offline - 1.5).abs() < 0.001,
            "days offline: {time_offline}"
        );
    }

    #[test]
    fn officer_notes_reach_only_a_viewer_whose_rank_can_view_them() {
        let store = founded_with_members();
        let by_leader = roster_for(&store, GM);
        assert!(by_leader
            .iter()
            .any(|m| m.officer_note == "officer bob note"));
        let by_member = roster_for(&store, BOB);
        assert!(by_member.iter().all(|m| m.officer_note.is_empty()));
        assert!(by_member.iter().any(|m| m.public_note == "bob note"));
    }

    #[test]
    fn a_member_whose_shard_cannot_answer_lists_offline_under_its_snapshot() {
        let store = founded_with_members();
        store.add_member(1, 5_090_099, 4, "");
        let members = roster_for(&store, GM);
        let lost = members.iter().find(|m| m.guid.guid() == 5_090_099).unwrap();
        assert_eq!(lost.name, "Snapshot5090099");
        assert!(matches!(
            lost.status,
            GuildMember_GuildMemberStatus::Offline { .. }
        ));
    }

    #[test]
    fn roster_outside_a_guild_sends_nothing() {
        let store = realm();
        assert!(dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_ROSTER
        )
        .is_empty());
    }

    #[test]
    fn roster_of_six_hundred_long_note_members_stays_under_the_client_limit() {
        let store = founded_with_members();
        for guid in 0..600 {
            store.add_member(1, 6_000_000 + guid, 4, &"n".repeat(31));
        }
        let message = ServerOpcodeMessage::SMSG_GUILD_ROSTER(Box::new(
            match only_message(dispatch(
                &store,
                in_world(GM),
                ClientOpcodeMessage::CMSG_GUILD_ROSTER,
            )) {
                ServerOpcodeMessage::SMSG_GUILD_ROSTER(roster) => *roster,
                other => panic!("expected a roster, got {other}"),
            },
        ));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        assert!(wire.len() - 4 <= 0x8000 - 4);
    }

    #[test]
    fn guild_info_counts_members_and_distinct_accounts() {
        let store = founded_with_members();
        store.members.lock().unwrap()[2].realm_account_id = BOB;
        let message = only_message(dispatch(
            &store,
            in_world(BOB),
            ClientOpcodeMessage::CMSG_GUILD_INFO,
        ));
        let ServerOpcodeMessage::SMSG_GUILD_INFO(info) = message else {
            panic!("expected guild info, got {message}");
        };
        assert_eq!(info.guild_name, "Knights");
        assert_eq!(info.amount_of_characters_in_guild, 3);
        assert_eq!(info.amount_of_accounts_in_guild, 2);
    }

    #[test]
    fn guild_info_outside_a_guild_answers_not_in_guild() {
        let store = realm();
        let outbound = dispatch(&store, in_world(BOB), ClientOpcodeMessage::CMSG_GUILD_INFO);
        assert_eq!(
            command_result_of(outbound),
            (
                GuildCommand::Create,
                GuildCommandResult::GuildPlayerNotInGuild
            )
        );
    }

    #[test]
    fn unrelated_opcode_passes_through() {
        let store = realm();
        let outcome = dispatch_guild_action(
            &store,
            in_world(GM),
            ClientOpcodeMessage::CMSG_PING(CMSG_PING::default()),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            GuildActionOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }

    fn raw(outbound: Vec<Outbound>) -> Vec<(u16, Vec<u8>)> {
        outbound
            .into_iter()
            .map(|out| match out {
                Outbound::Raw { opcode, body } => (opcode, body),
                _ => panic!("guild entry packets are raw"),
            })
            .collect()
    }

    #[test]
    fn a_fresh_login_whose_create_matched_gets_only_the_motd() {
        let store = founded_with_members();
        let outbound = guild_world_entry(&store, BOB, codec::WorldEntry::FreshLogin, (1, 3));
        assert_eq!(
            raw(outbound),
            vec![codec::build_guild_event_raw(
                event_kind::MOTD,
                &["No message set.".into()],
                0
            )]
        );
    }

    #[test]
    fn a_membership_change_after_the_create_read_is_re_sent() {
        let store = founded_with_members();
        let joined = guild_world_entry(&store, BOB, codec::WorldEntry::WorldPort, (0, 0));
        assert_eq!(raw(joined), vec![codec::build_guild_values(BOB, 1, 3)]);
        let removed = guild_world_entry(&store, 5_090_404, codec::WorldEntry::WorldPort, (1, 3));
        assert_eq!(
            raw(removed),
            vec![codec::build_guild_values(5_090_404, 0, 0)]
        );
    }

    #[test]
    fn a_non_member_whose_create_matched_gets_nothing() {
        let store = founded_with_members();
        let outbound = guild_world_entry(&store, 5_090_404, codec::WorldEntry::FreshLogin, (0, 0));
        assert!(outbound.is_empty());
    }

    #[test]
    fn a_member_signs_on_and_reports_it() {
        let store = founded_with_members();
        assert!(guild_sign_on(&store, BOB));
        assert_eq!(
            store.ops.lock().unwrap().last().cloned(),
            Some((
                BOB,
                GuildRequest::SignOn {
                    actor_name: "Bob".into()
                }
            ))
        );
        let before = store.ops.lock().unwrap().len();
        assert!(!guild_sign_on(&store, 5_090_404));
        assert_eq!(store.ops.lock().unwrap().len(), before);
    }

    #[test]
    fn a_failed_sign_on_owes_no_sign_off() {
        let mut store = founded_with_members();
        store.op_error = Some("realm_guild_op reducer transport disconnected: gone".into());
        assert!(!guild_sign_on(&store, BOB));
    }

    #[test]
    fn world_exit_signs_off_members_only() {
        let store = founded_with_members();
        guild_world_exit(&store, BOB);
        guild_world_exit(&store, 5_090_404);
        let ops = store.ops.lock().unwrap();
        assert_eq!(ops.last().cloned(), Some((BOB, GuildRequest::SignOff)));
        assert!(ops.iter().all(|(actor, _)| *actor != 5_090_404));
    }

    #[test]
    fn member_names_resolve_case_insensitively_and_homonyms_do_not_resolve() {
        let member = |guid, name: &str| codec::GuildMemberView {
            character_guid: guid,
            name: name.into(),
            ..codec::GuildMemberView::default()
        };
        let members = [member(1, "Bob"), member(2, "Alice"), member(3, "alice")];
        assert_eq!(member_by_name(&members, "BOB"), MemberMatch::Found(1));
        assert_eq!(member_by_name(&members, "Alice"), MemberMatch::NotInGuild);
        assert_eq!(member_by_name(&members, "Carol"), MemberMatch::NotInGuild);
    }

    const DESIGNER: u64 = 5_090_010;

    fn activate_tabard_vendor(
        store: &InMemoryGuildActions,
        player: GuildActionPlayer,
    ) -> Vec<Outbound> {
        dispatch(
            store,
            player,
            ClientOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(MSG_TABARDVENDOR_ACTIVATE {
                guid: Guid::new(DESIGNER),
            }),
        )
    }

    #[test]
    fn the_tabard_designer_window_opens_for_its_npc() {
        let store = realm();
        let message = only_message(activate_tabard_vendor(&store, in_world(BOB)));
        assert_eq!(
            message,
            ServerOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(MSG_TABARDVENDOR_ACTIVATE {
                guid: Guid::new(DESIGNER),
            })
        );
    }

    #[test]
    fn a_tabard_designer_that_refuses_by_faction_stays_silent() {
        let store = InMemoryGuildActions {
            refusing_npcs: vec![DESIGNER],
            ..realm()
        };
        assert!(activate_tabard_vendor(&store, in_world(BOB)).is_empty());
        let character_select = GuildActionPlayer {
            account_id: 7,
            self_guid: None,
        };
        assert!(activate_tabard_vendor(&realm(), character_select).is_empty());
    }

    const EMBLEM: guild_fee::Emblem = guild_fee::Emblem {
        emblem_style: 11,
        emblem_color: 12,
        border_style: 3,
        border_color: 14,
        background_color: 15,
    };

    fn save_emblem(store: &InMemoryGuildActions, actor: u64) -> GuildEmblemResult {
        let message = only_message(dispatch(
            store,
            in_world(actor),
            ClientOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(Box::new(MSG_SAVE_GUILD_EMBLEM_Client {
                vendor: Guid::new(DESIGNER),
                emblem_style: EMBLEM.emblem_style,
                emblem_color: EMBLEM.emblem_color,
                border_style: EMBLEM.border_style,
                border_color: EMBLEM.border_color,
                background_color: EMBLEM.background_color,
            })),
        ));
        match message {
            ServerOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(saved) => saved.result,
            other => panic!("expected an emblem result, got {other}"),
        }
    }

    /// Bob leads Knights; Carol is a member at rank 3.
    fn led_by_bob(
        hold_refusal: Option<GuildRefusal>,
        fee_refusal: Option<GuildRefusal>,
    ) -> InMemoryGuildActions {
        let store = InMemoryGuildActions {
            hold_refusal,
            fee_refusal,
            ..realm()
        };
        run_guild_dot_command(&store, in_world(GM), ".guild create Bob \"Knights\"").unwrap();
        store.add_member(1, CAROL, 3, "");
        store
    }

    #[test]
    fn the_guild_leader_pays_for_the_emblem_at_the_vendor_it_named() {
        let store = led_by_bob(None, None);
        assert_eq!(save_emblem(&store, BOB), GuildEmblemResult::Success);
        assert_eq!(
            *store.fee_requests.lock().unwrap(),
            vec![guild_fee::FeeRequest::Emblem {
                npc_guid: DESIGNER,
                emblem: EMBLEM,
            }]
        );
    }

    #[test]
    fn the_advisory_reads_answer_before_any_copper_moves() {
        let store = led_by_bob(None, None);
        assert_eq!(save_emblem(&store, GM), GuildEmblemResult::NoGuild);
        assert_eq!(
            save_emblem(&store, CAROL),
            GuildEmblemResult::NotGuildMaster
        );
        assert!(store.fee_requests.lock().unwrap().is_empty());
    }

    #[test]
    fn hold_refusals_map_to_their_emblem_results() {
        for (refusal, result) in [
            (
                GuildRefusal::NotEnoughMoney,
                GuildEmblemResult::NotEnoughMoney,
            ),
            (GuildRefusal::NpcRefused, GuildEmblemResult::NoMessage),
        ] {
            let store = led_by_bob(Some(refusal), None);
            assert_eq!(save_emblem(&store, BOB), result, "{refusal:?}");
        }
    }

    #[test]
    fn realm_core_refusals_map_to_their_emblem_results() {
        for (refusal, result) in [
            (GuildRefusal::NotLeader, GuildEmblemResult::NotGuildMaster),
            (GuildRefusal::NotInGuild, GuildEmblemResult::NoGuild),
        ] {
            let store = led_by_bob(None, Some(refusal));
            assert_eq!(save_emblem(&store, BOB), result, "{refusal:?}");
        }
    }
}
