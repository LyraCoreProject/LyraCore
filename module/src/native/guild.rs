use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        GuildCommand, GuildCommandResult,
    },
};
use lyracore_shared::{
    faction::team_for_race,
    guild::{event_kind, has_right, rights, GuildRefusal},
};
use spacetimedb::{ConnectionId, ReducerContext};

use super::{game_native_player, NativeWorldConnection, Rollback};
use crate::{
    chat::game_character_contact,
    game_world_entity,
    guild::{
        self, game_guild, game_guild_member, GuildAcceptRequest, GuildEvent, GuildGmCreate,
        GuildInviteRequest, GuildNoteEdit, GuildOp, GuildRankEdit,
    },
    Character,
};

#[derive(Clone, Copy)]
enum Reply {
    Silent,
    Membership,
    Motd,
    Info,
    Note,
    Rank,
}

#[allow(
    clippy::too_many_lines,
    reason = "Guild opcode dispatch stays in one match"
)]
pub(super) fn receive(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let actor = super::available_character(ctx, guid)?;
    let id = connection.connection_id;
    use ClientOpcodeMessage as C;
    let mut typed_name = String::new();
    let mut target_name = String::new();
    let mut success = None;
    let (op, reply) = match message {
        C::CMSG_GUILD_QUERY(request) => {
            let packet = view(ctx, request.guild_id).map_or_else(not_in_guild, |guild| {
                ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
                    codec::build_guild_query_response(&guild),
                ))
            });
            return Ok(super::send(ctx, id, packet, false)?);
        }
        C::CMSG_GUILD_ROSTER => {
            if let Some(member) = guild::member(ctx, guid) {
                rosters(ctx, member.guild_id, &[(id, member.rank_id)]);
            }
            return Ok(());
        }
        C::CMSG_GUILD_INFO => {
            let packet = own_guild(ctx, guid).map_or_else(not_in_guild, |guild| {
                let members: Vec<_> = ctx
                    .db
                    .game_guild_member()
                    .by_guild()
                    .filter(guild.guild_id)
                    .collect();
                let accounts: std::collections::BTreeSet<_> = members
                    .iter()
                    .map(|member| member.realm_account_id)
                    .filter(|id| *id != 0)
                    .collect();
                let unknown = members
                    .iter()
                    .filter(|member| member.realm_account_id == 0)
                    .count();
                ServerOpcodeMessage::SMSG_GUILD_INFO(Box::new(codec::build_guild_info(
                    &guild,
                    members.len() as u32,
                    (accounts.len() + unknown) as u32,
                )))
            });
            return Ok(super::send(ctx, id, packet, false)?);
        }
        C::CMSG_GUILD_CREATE(request) => (
            create(
                &actor,
                crate::auth::gm_level(ctx, actor.account_id),
                request.guild_name,
            ),
            Reply::Silent,
        ),
        C::CMSG_GUILD_INVITE(request) => {
            typed_name = request.invited_player;
            let Some(target) =
                crate::helpers::character_by_name(ctx, &typed_name).filter(|target| {
                    ctx.db
                        .game_world_entity()
                        .guid()
                        .find(target.guid)
                        .is_some()
                })
            else {
                return Ok(super::send(
                    ctx,
                    id,
                    command(
                        GuildCommand::Invite,
                        typed_name,
                        GuildCommandResult::GuildPlayerNotFoundS,
                    ),
                    false,
                )?);
            };
            target_name = target.name.clone();
            let ignores = ctx
                .db
                .game_character_contact()
                .by_owner()
                .filter(target.guid)
                .any(|contact| contact.is_ignore && contact.target_guid == guid);
            (
                GuildOp::Invite(GuildInviteRequest {
                    target_guid: target.guid,
                    actor_team: team_for_race(actor.race),
                    target_team: team_for_race(target.race),
                    target_ignores_actor: ignores,
                }),
                Reply::Membership,
            )
        }
        C::CMSG_GUILD_ACCEPT => (
            GuildOp::Accept(GuildAcceptRequest {
                actor_name: actor.name.clone(),
                actor_team: team_for_race(actor.race),
            }),
            Reply::Silent,
        ),
        C::CMSG_GUILD_DECLINE => (
            GuildOp::Decline {
                actor_name: actor.name.clone(),
            },
            Reply::Silent,
        ),
        C::CMSG_GUILD_LEAVE => {
            let Some(guild) = own_guild(ctx, guid) else {
                return Ok(super::send(ctx, id, not_in_guild(), false)?);
            };
            if guild.leader_guid != guid {
                success = Some(command(
                    GuildCommand::Quit,
                    guild.name,
                    GuildCommandResult::PlayerNoMoreInGuild,
                ));
            }
            (GuildOp::Leave, Reply::Membership)
        }
        C::CMSG_GUILD_REMOVE(request) => {
            target_name = request.player_name;
            (
                GuildOp::Remove {
                    target_guid: named_member(ctx, guid, &target_name),
                },
                Reply::Membership,
            )
        }
        C::CMSG_GUILD_PROMOTE(request) => {
            target_name = request.player_name;
            (
                GuildOp::Promote {
                    target_guid: named_member(ctx, guid, &target_name),
                },
                Reply::Membership,
            )
        }
        C::CMSG_GUILD_DEMOTE(request) => {
            target_name = request.player_name;
            (
                GuildOp::Demote {
                    target_guid: named_member(ctx, guid, &target_name),
                },
                Reply::Membership,
            )
        }
        C::CMSG_GUILD_LEADER(request) => {
            target_name = request.new_guild_leader_name;
            (
                GuildOp::SetLeader {
                    target_guid: named_member(ctx, guid, &target_name),
                },
                Reply::Membership,
            )
        }
        C::CMSG_GUILD_DISBAND => (GuildOp::Disband, Reply::Membership),
        C::CMSG_GUILD_MOTD(request) => (
            GuildOp::SetMotd {
                text: request.message_of_the_day,
            },
            Reply::Motd,
        ),
        C::CMSG_GUILD_INFO_TEXT(request) => (
            GuildOp::SetInfo {
                text: request.guild_info,
            },
            Reply::Info,
        ),
        C::CMSG_GUILD_SET_PUBLIC_NOTE(request) => {
            target_name = request.player_name;
            (
                GuildOp::SetPublicNote(GuildNoteEdit {
                    target_guid: named_member(ctx, guid, &target_name),
                    text: request.note,
                }),
                Reply::Note,
            )
        }
        C::CMSG_GUILD_SET_OFFICER_NOTE(request) => {
            target_name = request.player_name;
            (
                GuildOp::SetOfficerNote(GuildNoteEdit {
                    target_guid: named_member(ctx, guid, &target_name),
                    text: request.note,
                }),
                Reply::Note,
            )
        }
        C::CMSG_GUILD_RANK(request) => (
            GuildOp::EditRank(GuildRankEdit {
                rank_id: request.rank_id,
                rights: request.rights,
                name: request.rank_name,
            }),
            Reply::Rank,
        ),
        C::CMSG_GUILD_ADD_RANK(request) => (
            GuildOp::AddRank {
                name: request.rank_name,
            },
            Reply::Rank,
        ),
        C::CMSG_GUILD_DEL_RANK => (GuildOp::DeleteRank, Reply::Rank),
        _ => return Err("opcode is not a guild request".into()),
    };
    if let Err(refusal) = guild::apply_op(ctx, guid, actor.account_id, op.clone()) {
        let packet = refused(reply, &op, refusal, &typed_name, &target_name);
        let cause = refusal.as_tag().to_string();
        return Err(match packet {
            Some(packet) => Rollback::reply(packet, cause),
            None => Rollback::silent(cause),
        });
    }
    if let Some(packet) = success {
        super::send(ctx, id, packet, false)?;
    }
    Ok(())
}

fn command(command: GuildCommand, name: String, result: GuildCommandResult) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(Box::new(codec::build_guild_command_result(
        command, name, result,
    )))
}

fn not_in_guild() -> ServerOpcodeMessage {
    command(
        GuildCommand::Create,
        String::new(),
        GuildCommandResult::GuildPlayerNotInGuild,
    )
}

fn refused(
    reply: Reply,
    op: &GuildOp,
    refusal: GuildRefusal,
    typed: &str,
    target: &str,
) -> Option<ServerOpcodeMessage> {
    use GuildCommandResult as R;
    use GuildRefusal as F;
    let mut cmd = GuildCommand::Invite;
    let mut name = String::new();
    let result = match (reply, refusal) {
        (Reply::Silent, _) => return None,
        (_, F::NotInGuild) => return Some(not_in_guild()),
        (Reply::Membership, F::NoPermission | F::NotLeader) => R::GuildPermissionsOrLeader,
        (Reply::Membership, cause) => match (op, cause) {
            (GuildOp::Invite(_), F::NotAllied) => {
                name = typed.into();
                R::GuildNotAllied
            }
            (GuildOp::Invite(_), F::AlreadyInGuild) => {
                name = target.into();
                R::AlreadyInGuildS
            }
            (GuildOp::Invite(_), F::AlreadyInvited) => {
                name = target.into();
                R::AlreadyInvitedToGuildS
            }
            (_, F::TargetNotInGuild) => {
                name = target.into();
                R::GuildPlayerNotInGuildS
            }
            (GuildOp::Remove { .. } | GuildOp::Leave, F::LeaderCannotLeave) => {
                cmd = GuildCommand::Quit;
                R::GuildPermissionsOrLeader
            }
            (GuildOp::Remove { .. }, F::RankTooHigh) => {
                cmd = GuildCommand::Quit;
                name = target.into();
                R::GuildRankTooHighS
            }
            (_, F::RankTooHigh) => {
                name = target.into();
                R::GuildRankTooHighS
            }
            (_, F::RankTooLow) => {
                name = target.into();
                R::GuildRankTooLowS
            }
            (_, F::TargetIsSelf) => R::GuildNameInvalid,
            _ => R::GuildPermissionsOrLeader,
        },
        (Reply::Motd | Reply::Note, F::NoPermission) | (Reply::Rank, F::NotLeader) => {
            R::GuildPermissionsOrLeader
        }
        (Reply::Info, F::NoPermission) => {
            cmd = GuildCommand::Create;
            R::GuildPermissionsOrLeader
        }
        (Reply::Note, F::TargetNotInGuild) => {
            name = target.into();
            R::GuildPlayerNotInGuildS
        }
        _ => return None,
    };
    Some(command(cmd, name, result))
}

fn named_member(ctx: &ReducerContext, actor: u64, name: &str) -> u64 {
    let Some(member) = guild::member(ctx, actor) else {
        return 0;
    };
    let key = name.to_lowercase();
    let mut found = ctx
        .db
        .game_guild_member()
        .by_guild()
        .filter(member.guild_id)
        .filter(|member| member.name.to_lowercase() == key);
    match (found.next(), found.next()) {
        (Some(member), None) => member.character_guid,
        _ => 0,
    }
}

fn own_guild(ctx: &ReducerContext, guid: u64) -> Option<codec::GuildView> {
    view(ctx, guild::member(ctx, guid)?.guild_id)
}

fn view(ctx: &ReducerContext, id: u32) -> Option<codec::GuildView> {
    let guild = ctx.db.game_guild().guild_id().find(id)?;
    Some(codec::GuildView {
        guild_id: id,
        name: guild.name,
        leader_guid: guild.leader_guid,
        team: guild.team,
        motd: guild.motd,
        info: guild.info,
        emblem_style: guild.emblem_style,
        emblem_color: guild.emblem_color,
        border_style: guild.border_style,
        border_color: guild.border_color,
        background_color: guild.background_color,
        created_micros: guild.created_micros,
        ranks: guild::ranks(ctx, id)
            .into_iter()
            .map(|rank| codec::GuildRankView {
                rank_id: rank.rank_id,
                name: rank.name,
                rights: rank.rights,
            })
            .collect(),
    })
}

fn roster_lines(ctx: &ReducerContext, id: u32) -> Vec<codec::GuildRosterLine> {
    let now = ctx.timestamp.to_micros_since_unix_epoch().max(0) as u64;
    ctx.db
        .game_guild_member()
        .by_guild()
        .filter(id)
        .map(|member| {
            let mut line = codec::GuildRosterLine {
                guid: member.character_guid,
                name: member.name,
                rank_id: member.rank_id,
                days_offline: Some(0.0),
                public_note: member.public_note,
                officer_note: member.officer_note,
                ..Default::default()
            };
            if let Some(character) = crate::helpers::character_by_guid(ctx, member.character_guid) {
                let entity = ctx
                    .db
                    .game_world_entity()
                    .guid()
                    .find(member.character_guid);
                line.name = character.name;
                line.level = entity
                    .as_ref()
                    .map_or(character.level, |entity| entity.level.min(255) as u8);
                line.class = character.class;
                line.zone_id = entity
                    .as_ref()
                    .map_or(character.zone_id, |entity| entity.zone_id);
                line.days_offline = if entity.is_some()
                    || super::is_player(ctx, member.character_guid)
                {
                    None
                } else {
                    Some(if character.last_logout_micros == 0 {
                        0.0
                    } else {
                        now.saturating_sub(character.last_logout_micros) as f32 / 86_400_000_000.0
                    })
                };
            }
            line
        })
        .collect()
}

fn rosters(ctx: &ReducerContext, guild_id: u32, recipients: &[(ConnectionId, u32)]) {
    if recipients.is_empty() {
        return;
    }
    let Some(guild) = view(ctx, guild_id) else {
        return;
    };
    let lines = roster_lines(ctx, guild_id);
    for show_notes in [false, true] {
        let recipients: Vec<_> = recipients
            .iter()
            .filter(|(_, rank)| {
                has_right(guild.rank_rights(*rank), rights::VIEWOFFNOTE) == show_notes
            })
            .collect();
        if recipients.is_empty() {
            continue;
        }
        let lines = lines.iter().cloned().map(|mut line| {
            if !show_notes {
                line.officer_note.clear();
            }
            line
        });
        let packet = encode(ServerOpcodeMessage::SMSG_GUILD_ROSTER(Box::new(
            codec::build_guild_roster(&guild, lines),
        )));
        for (connection, _) in recipients {
            super::send_raw(ctx, *connection, packet.opcode, packet.body.clone(), false);
        }
    }
}

fn encode(message: ServerOpcodeMessage) -> lyracore_protocol::world::Packet {
    lyracore_protocol::world::encode(message).expect("bounded guild message fits a world frame")
}

pub(crate) fn membership_changed(ctx: &ReducerContext, guid: u64) {
    let (guild_id, rank_id) =
        guild::member(ctx, guid).map_or((0, 0), |row| (row.guild_id, row.rank_id));
    let (opcode, body) = codec::build_guild_values(guid, guild_id, rank_id);
    super::visibility::broadcast(ctx, guid, opcode, body, false);
}

pub(crate) fn event(ctx: &ReducerContext, event: &GuildEvent) {
    if event_kind::is_petition(event.kind) {
        super::petition::event(ctx, event);
        return;
    }
    let recipients: Vec<_> = if event.recipient_guid != 0 {
        ctx.db
            .game_native_player()
            .character_guid()
            .find(event.recipient_guid)
            .into_iter()
            .collect()
    } else {
        ctx.db
            .game_guild_member()
            .by_guild()
            .filter(event.guild_id)
            .filter(|member| {
                event.kind != event_kind::SIGNED_ON || member.character_guid != event.subject_guid
            })
            .filter_map(|member| {
                ctx.db
                    .game_native_player()
                    .character_guid()
                    .find(member.character_guid)
            })
            .collect()
    };
    if recipients.is_empty() {
        return;
    }
    if matches!(
        event.kind,
        event_kind::ROSTER_TO_ACTOR | event_kind::ROSTER_REFRESH
    ) {
        let recipients: Vec<_> = recipients
            .iter()
            .filter_map(|player| {
                guild::member(ctx, player.character_guid)
                    .filter(|member| member.guild_id == event.guild_id)
                    .map(|member| (player.connection_id, member.rank_id))
            })
            .collect();
        if event.kind == event_kind::ROSTER_REFRESH {
            if let Some(guild) = view(ctx, event.guild_id) {
                let query = encode(ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
                    codec::build_guild_query_response(&guild),
                )));
                for (connection, _) in &recipients {
                    super::send_raw(ctx, *connection, query.opcode, query.body.clone(), false);
                }
            }
        }
        rosters(ctx, event.guild_id, &recipients);
        return;
    }
    let (opcode, body) = match event.kind {
        event_kind::INVITE => {
            let packet = encode(ServerOpcodeMessage::SMSG_GUILD_INVITE(Box::new(
                codec::build_guild_invite(
                    event.strings.first().cloned().unwrap_or_default(),
                    event.strings.get(1).cloned().unwrap_or_default(),
                ),
            )));
            (packet.opcode, packet.body)
        }
        event_kind::DECLINE => {
            codec::build_guild_decline_raw(event.strings.first().map_or("", String::as_str))
        }
        _ => codec::build_guild_event_raw(event.kind, &event.strings, event.subject_guid),
    };
    let tabard = (event.kind == event_kind::TABARD_CHANGED)
        .then(|| view(ctx, event.guild_id))
        .flatten()
        .map(|guild| {
            encode(ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
                codec::build_guild_query_response(&guild),
            )))
        });
    for player in recipients {
        super::send_raw(ctx, player.connection_id, opcode, body.clone(), false);
        if let Some(query) = &tabard {
            super::send_raw(
                ctx,
                player.connection_id,
                query.opcode,
                query.body.clone(),
                false,
            );
        }
    }
}

pub(super) fn presence(ctx: &ReducerContext, guid: u64, online: bool) {
    if guild::member(ctx, guid).is_none() {
        return;
    }
    let Some(character) = crate::helpers::character_by_guid(ctx, guid) else {
        return;
    };
    let op = if online {
        GuildOp::SignOn {
            actor_name: character.name,
        }
    } else {
        GuildOp::SignOff
    };
    if let Err(refusal) = guild::apply_op(ctx, guid, character.account_id, op) {
        spacetimedb::log::warn!("guild presence of {guid}: {}", refusal.as_tag());
    }
    if online {
        if let (Some(guild), Some(player)) = (
            own_guild(ctx, guid),
            ctx.db.game_native_player().character_guid().find(guid),
        ) {
            let (opcode, body) = codec::build_guild_event_raw(event_kind::MOTD, &[guild.motd], 0);
            super::send_raw(ctx, player.connection_id, opcode, body, false);
        }
    }
}

fn create(leader: &Character, gm_level: u8, name: String) -> GuildOp {
    GuildOp::GmCreate(GuildGmCreate {
        leader_guid: leader.guid,
        leader_name: leader.name.clone(),
        leader_team: team_for_race(leader.race),
        leader_realm_account: leader.account_id,
        gm_level,
        name,
    })
}

pub(super) fn is_dot_command(text: &str) -> bool {
    text.strip_prefix(".guild")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

pub(super) fn dot_command(
    ctx: &ReducerContext,
    actor: &crate::WorldEntity,
    text: &str,
) -> Result<(), Rollback> {
    let character =
        crate::helpers::character_by_guid(ctx, actor.guid).ok_or("Character is not available")?;
    let result = (|| {
        let gm_level = crate::auth::gm_level(ctx, character.account_id);
        if gm_level == 0 {
            return Err("permission denied".to_string());
        }
        let (leader_name, name) =
            parse_create(text).ok_or("usage: .guild create [<leader>] \"<name>\"")?;
        let account_id = character.account_id;
        let leader = match leader_name {
            Some(name) => crate::helpers::character_by_name(ctx, name)
                .ok_or_else(|| format!("no player named {name}"))?,
            None => crate::helpers::character_by_guid(ctx, actor.target_guid).unwrap_or(character),
        };
        guild::apply_op(
            ctx,
            actor.guid,
            account_id,
            create(&leader, gm_level, name.into()),
        )
        .map_err(|refusal| match refusal {
            GuildRefusal::NotGameMaster => "permission denied".into(),
            GuildRefusal::AlreadyInGuild => format!("{} is already in a guild", leader.name),
            _ => "guild not created".into(),
        })
    })();
    result.map_err(|cause| {
        Rollback::reply(
            ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(codec::build_gm_system_message(
                cause.clone(),
            ))),
            cause,
        )
    })
}

fn parse_create(text: &str) -> Option<(Option<&str>, &str)> {
    let rest = text.strip_prefix(".guild")?.trim_start();
    let (subcommand, rest) = rest.split_once(char::is_whitespace)?;
    if !subcommand.eq_ignore_ascii_case("create") {
        return None;
    }
    let rest = rest.trim();
    let (leader, quoted) = if rest.starts_with('"') {
        (None, rest)
    } else {
        let (leader, quoted) = rest.split_once(char::is_whitespace)?;
        (Some(leader), quoted.trim_start())
    };
    let name = quoted.strip_prefix('"')?.strip_suffix('"')?;
    (!name.is_empty() && !name.contains('"')).then_some((leader, name))
}
