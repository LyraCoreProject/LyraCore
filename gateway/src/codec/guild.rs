//! Guild wire mapping: the query response, roster, guild info, command result, the raw
//! SMSG_GUILD_EVENT and the Guild Projection values update.

use super::*;
use wow_world_messages::vanilla::{
    GuildCommand, GuildCommandResult, GuildMember, GuildMember_GuildMemberStatus,
    SMSG_GUILD_COMMAND_RESULT, SMSG_GUILD_INFO, SMSG_GUILD_QUERY_RESPONSE, SMSG_GUILD_ROSTER,
};

/// SMSG_GUILD_EVENT. gtker's vanilla type has no trailing guid, so the event is encoded raw.
pub const SMSG_GUILD_EVENT_OPCODE: u16 = 0x0092;

/// The largest SMSG_GUILD_ROSTER body the 1.12 client accepts: the packet limit minus the header
/// (`vm:src/game/Guild/Guild.h:41`).
pub const GUILD_ROSTER_MAX_BODY: usize = 0x8000 - 4;

/// One Guild Rank as the Gateway reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildRankView {
    pub rank_id: u32,
    pub name: String,
    pub rights: u32,
}

/// One Guild as the Gateway reads it from Realm-core. `ranks` is ordered by `rank_id`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildView {
    pub guild_id: u32,
    pub name: String,
    pub leader_guid: u64,
    pub motd: String,
    pub info: String,
    pub emblem_style: u32,
    pub emblem_color: u32,
    pub border_style: u32,
    pub border_color: u32,
    pub background_color: u32,
    pub created_micros: i64,
    pub ranks: Vec<GuildRankView>,
}

impl GuildView {
    /// The Rank Rights of `rank_id`. An unknown rank grants nothing (`cm:Guild.cpp:660-666`).
    pub fn rank_rights(&self, rank_id: u32) -> u32 {
        self.ranks
            .iter()
            .find(|rank| rank.rank_id == rank_id)
            .map_or(0, |rank| rank.rights)
    }
}

/// One membership row as the Gateway reads it from Realm-core.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuildMemberView {
    pub character_guid: u64,
    pub guild_id: u32,
    pub rank_id: u32,
    /// The name snapshot Realm-core keeps for the member.
    pub name: String,
    pub public_note: String,
    pub officer_note: String,
    pub realm_account_id: u64,
}

/// One SMSG_GUILD_ROSTER line, already resolved for one viewer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuildRosterLine {
    pub guid: u64,
    pub name: String,
    pub rank_id: u32,
    pub level: u8,
    pub class: u8,
    pub zone_id: u32,
    /// `None` when online; otherwise days since the member's last logout.
    pub days_offline: Option<f32>,
    pub public_note: String,
    /// Empty unless the viewer's Guild Rank holds VIEWOFFNOTE.
    pub officer_note: String,
}

/// SMSG_GUILD_QUERY_RESPONSE: the name, ten rank names (empty past the last rank) and the emblem
/// (`cm:Guild.cpp:760-783`).
pub fn build_guild_query_response(guild: &GuildView) -> SMSG_GUILD_QUERY_RESPONSE {
    let mut rank_names: [String; 10] = Default::default();
    for (slot, rank) in rank_names.iter_mut().zip(&guild.ranks) {
        slot.clone_from(&rank.name);
    }
    SMSG_GUILD_QUERY_RESPONSE {
        id: guild.guild_id,
        name: guild.name.clone(),
        rank_names,
        emblem_style: guild.emblem_style,
        emblem_color: guild.emblem_color,
        border_style: guild.border_style,
        border_color: guild.border_color,
        background_color: guild.background_color,
    }
}

/// SMSG_GUILD_COMMAND_RESULT for `command`, naming `name` where the client line has a `%s`.
pub fn build_guild_command_result(
    command: GuildCommand,
    name: String,
    result: GuildCommandResult,
) -> SMSG_GUILD_COMMAND_RESULT {
    SMSG_GUILD_COMMAND_RESULT {
        command,
        string: name,
        result,
    }
}

/// SMSG_GUILD_INFO: the name, the founding day, month (1..=12) and year on the Realm Clock, the
/// member count and the Account count (`cm:GuildHandler.cpp:240-259`).
pub fn build_guild_info(
    guild: &GuildView,
    member_count: u32,
    account_count: u32,
) -> SMSG_GUILD_INFO {
    let days = guild.created_micros.div_euclid(86_400_000_000);
    let (year, month, day) = lyracore_shared::calendar::civil_from_days(days);
    SMSG_GUILD_INFO {
        guild_name: guild.name.clone(),
        created_day: day,
        created_month: month,
        created_year: u32::try_from(year).unwrap_or_default(),
        amount_of_characters_in_guild: member_count,
        amount_of_accounts_in_guild: account_count,
    }
}

/// SMSG_GUILD_ROSTER. Lines are added in order until the next one would push the body past
/// [`GUILD_ROSTER_MAX_BODY`] (`vm:src/game/Guild/Guild.cpp:803-860`); the rest are left out.
pub fn build_guild_roster(
    guild: &GuildView,
    lines: impl IntoIterator<Item = GuildRosterLine>,
) -> SMSG_GUILD_ROSTER {
    let rights: Vec<u32> = guild.ranks.iter().map(|rank| rank.rights).collect();
    let mut room = GUILD_ROSTER_MAX_BODY
        .saturating_sub(4 + guild.motd.len() + 1 + guild.info.len() + 1 + 4 + 4 * rights.len());
    let mut members = Vec::new();
    for line in lines {
        let size = roster_line_size(&line);
        if size > room {
            break;
        }
        room -= size;
        members.push(GuildMember {
            guid: Guid::new(line.guid),
            status: match line.days_offline {
                Some(time_offline) => GuildMember_GuildMemberStatus::Offline { time_offline },
                None => GuildMember_GuildMemberStatus::Online,
            },
            name: line.name,
            rank: line.rank_id,
            level: Level::new(line.level),
            class: Class::try_from(line.class).unwrap_or_default(),
            area: Area::try_from(line.zone_id).unwrap_or(Area::None),
            public_note: line.public_note,
            officer_note: line.officer_note,
        });
    }
    SMSG_GUILD_ROSTER {
        motd: guild.motd.clone(),
        guild_info: guild.info.clone(),
        rights,
        members,
    }
}

/// Bytes one roster line takes on the wire: guid, status, name, rank, level, class, zone, the
/// offline time when offline, and both notes.
fn roster_line_size(line: &GuildRosterLine) -> usize {
    const FIXED: usize = 8 + 1 + 4 + 1 + 1 + 4;
    let offline = if line.days_offline.is_some() { 4 } else { 0 };
    let strings = [&line.name, &line.public_note, &line.officer_note];
    FIXED + offline + strings.iter().map(|s| s.len() + 1).sum::<usize>()
}

/// SMSG_GUILD_EVENT as `(opcode, body)`: `u8 event`, `u8 count`, the strings, then the subject guid
/// when nonzero (`cm:Guild.cpp:886-911`).
pub fn build_guild_event_raw(kind: u8, strings: &[String], subject_guid: u64) -> (u16, Vec<u8>) {
    let strings = &strings[..strings.len().min(usize::from(u8::MAX))];
    let mut body = vec![kind, strings.len() as u8];
    for string in strings {
        body.extend_from_slice(string.as_bytes());
        body.push(0);
    }
    if subject_guid != 0 {
        body.extend_from_slice(&subject_guid.to_le_bytes());
    }
    (SMSG_GUILD_EVENT_OPCODE, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guild() -> GuildView {
        GuildView {
            guild_id: 7,
            name: "Tracer Guild".into(),
            motd: "No message set.".into(),
            ranks: lyracore_shared::guild::DEFAULT_RANKS
                .iter()
                .zip(0u32..)
                .map(|((name, rights), rank_id)| GuildRankView {
                    rank_id,
                    name: (*name).into(),
                    rights: *rights,
                })
                .collect(),
            emblem_style: 1,
            emblem_color: 2,
            border_style: 3,
            border_color: 4,
            background_color: 5,
            ..GuildView::default()
        }
    }

    #[test]
    fn query_response_writes_ten_rank_names_and_the_emblem() {
        let message = ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
            build_guild_query_response(&guild()),
        ));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        let mut expected = 7u32.to_le_bytes().to_vec();
        expected.extend_from_slice(b"Tracer Guild\0");
        expected.extend_from_slice(b"Guild Master\0Officer\0Veteran\0Member\0Initiate\0");
        expected.extend_from_slice(&[0; 5]);
        for value in 1u32..=5 {
            expected.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(&wire[4..], expected.as_slice());
        assert_eq!(u16::from_le_bytes([wire[2], wire[3]]), 0x0055);
    }

    #[test]
    fn guild_event_writes_the_subject_guid_after_the_strings_only_when_named() {
        let (opcode, signed_on) = build_guild_event_raw(
            lyracore_shared::guild::event_kind::SIGNED_ON,
            &["Alice".to_string()],
            0x0102,
        );
        assert_eq!(opcode, 0x0092);
        let mut expected = vec![12, 1];
        expected.extend_from_slice(b"Alice\0");
        expected.extend_from_slice(&[0x02, 0x01, 0, 0, 0, 0, 0, 0]);
        assert_eq!(signed_on, expected);

        let (_, motd) = build_guild_event_raw(
            lyracore_shared::guild::event_kind::MOTD,
            &["No message set.".to_string()],
            0,
        );
        let mut expected = vec![2, 1];
        expected.extend_from_slice(b"No message set.\0");
        assert_eq!(motd, expected);
    }

    fn self_create(guild_id: u32, guild_rank: u32) -> wow_world_messages::vanilla::UpdatePlayer {
        let entity = EntityView {
            guid: 1,
            type_mask: lyracore_shared::constants::type_mask::PLAYER_BIT,
            // Human Warrior: the CREATE encoder refuses race 0.
            unit_bytes_0: 1 | (1 << 8) | (1 << 24),
            guild_id,
            guild_rank,
            ..EntityView::default()
        };
        let create = build_create_object(&entity, CreateKind::SelfPlayer, &[], &[]).unwrap();
        let [Object::CreateObject2 {
            mask2: UpdateMask::Player(player),
            ..
        }] = create.objects.as_slice()
        else {
            panic!("the self CREATE must carry a player mask");
        };
        player.clone()
    }

    #[test]
    fn the_self_create_carries_the_guild_projection() {
        let player = self_create(7, 3);
        assert_eq!(player.player_guildid(), Some(7));
        assert_eq!(player.player_guildrank(), Some(3));
    }

    #[test]
    fn the_self_create_writes_an_explicit_zero_outside_any_guild() {
        let player = self_create(0, 0);
        assert_eq!(player.player_guildid(), Some(0));
        assert_eq!(player.player_guildrank(), Some(0));
    }

    #[test]
    fn the_character_list_carries_each_guild_id() {
        let characters = [CharacterView {
            guid: 1,
            name: "Leader".into(),
            race: 1,
            class: 1,
            guild_id: 7,
            ..CharacterView::default()
        }];
        let list = build_char_enum(&characters).unwrap();
        assert_eq!(list.characters[0].guild_id, 7);
    }

    #[test]
    fn guild_values_carry_only_the_guild_id_and_rank() {
        let (opcode, body) = build_guild_values(0x0102, 7, 3);
        assert_eq!(opcode, 0x00A9);
        let updates = lyracore_shared::values_mask::parse_values_updates(&body);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].guid, 0x0102);
        assert_eq!(updates[0].fields, vec![(191, 7), (192, 3)]);
    }

    #[test]
    fn guild_info_reads_the_founding_date_in_whole_utc_days() {
        let mut guild = guild();
        // 2026-09-23T23:59:59Z
        guild.created_micros = 1_790_207_999_000_000;
        let info = build_guild_info(&guild, 3, 2);
        assert_eq!(
            (info.created_day, info.created_month, info.created_year),
            (23, 9, 2026)
        );
        assert_eq!(info.amount_of_characters_in_guild, 3);
        assert_eq!(info.amount_of_accounts_in_guild, 2);
    }

    #[test]
    fn roster_stops_before_the_line_that_would_pass_the_client_limit() {
        let long_note = "n".repeat(31);
        let line = |guid| GuildRosterLine {
            guid,
            name: "Membername".into(),
            rank_id: 4,
            level: 60,
            class: 1,
            zone_id: 12,
            days_offline: Some(1.5),
            public_note: long_note.clone(),
            officer_note: long_note.clone(),
        };
        let roster = build_guild_roster(&guild(), (1..=600).map(line));
        let message = ServerOpcodeMessage::SMSG_GUILD_ROSTER(Box::new(roster.clone()));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        // Header: count 4 + MOTD 16 + info 1 + rank count 4 + five rank words 20 = 45 bytes.
        // Each line: 8 + 1 + 11 + 4 + 1 + 1 + 4 + 4 + 32 + 32 = 98 bytes.
        // (32764 - 45) / 98 = 333 lines, 45 + 333 * 98 = 32679 bytes.
        assert_eq!(roster.members.len(), 333);
        assert_eq!(wire.len() - 4, 32_679);
    }

    /// Frame a client body the way the 1.12 client sends it: u16 big-endian size, u32 opcode.
    fn client_packet(opcode: u32, body: &[u8]) -> ClientOpcodeMessage {
        let mut framed = ((body.len() + 4) as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(&opcode.to_le_bytes());
        framed.extend_from_slice(body);
        ClientOpcodeMessage::read_unencrypted(&mut framed.as_slice()).unwrap()
    }

    fn server_body(message: ServerOpcodeMessage) -> (u16, Vec<u8>) {
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        (u16::from_le_bytes([wire[2], wire[3]]), wire[4..].to_vec())
    }

    /// `cm:GuildHandler.cpp:722-723`: the vendor guid, then five u32 design values.
    #[test]
    fn save_guild_emblem_reads_the_vendor_then_five_design_values() {
        let mut body = 0xF130_0000_0000_0042u64.to_le_bytes().to_vec();
        for value in [11u32, 12, 3, 14, 15] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        let ClientOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(save) = client_packet(0x01F1, &body) else {
            panic!("0x1F1 is MSG_SAVE_GUILD_EMBLEM");
        };
        assert_eq!(save.vendor.guid(), 0xF130_0000_0000_0042);
        assert_eq!(
            [
                save.emblem_style,
                save.emblem_color,
                save.border_style,
                save.border_color,
                save.background_color
            ],
            [11, 12, 3, 14, 15]
        );
    }

    /// `cm:GuildHandler.cpp:766-771` writes one u32; `cm:Guild.h:146-151` numbers the results.
    #[test]
    fn save_guild_emblem_answers_one_result_word() {
        use wow_world_messages::vanilla::{GuildEmblemResult, MSG_SAVE_GUILD_EMBLEM_Server};
        for (result, word) in [
            (GuildEmblemResult::Success, 0u32),
            (GuildEmblemResult::NoGuild, 2),
            (GuildEmblemResult::NotGuildMaster, 3),
            (GuildEmblemResult::NotEnoughMoney, 4),
            (GuildEmblemResult::NoMessage, 5),
        ] {
            let message =
                ServerOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(MSG_SAVE_GUILD_EMBLEM_Server { result });
            assert_eq!(server_body(message), (0x01F1, word.to_le_bytes().to_vec()));
        }
    }

    /// `cm:NPCHandler.cpp:49-50,62-67`: the client sends the NPC's full guid and the server echoes
    /// it unpacked.
    #[test]
    fn tabard_vendor_activate_carries_the_full_npc_guid_both_ways() {
        use wow_world_messages::vanilla::MSG_TABARDVENDOR_ACTIVATE;
        let guid = 0xF130_0000_0000_0042u64;
        let ClientOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(activate) =
            client_packet(0x01F2, &guid.to_le_bytes())
        else {
            panic!("0x1F2 is MSG_TABARDVENDOR_ACTIVATE");
        };
        assert_eq!(activate.guid.guid(), guid);
        let window = ServerOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(MSG_TABARDVENDOR_ACTIVATE {
            guid: Guid::new(guid),
        });
        assert_eq!(server_body(window), (0x01F2, guid.to_le_bytes().to_vec()));
    }

    /// GE_TABARDCHANGE is event 9 with no strings and no guid (`cm:Guild.h:111`).
    #[test]
    fn tabard_changed_is_event_nine_with_nothing_after_it() {
        let (opcode, body) =
            build_guild_event_raw(lyracore_shared::guild::event_kind::TABARD_CHANGED, &[], 0);
        assert_eq!((opcode, body), (0x0092, vec![9, 0]));
    }
}
