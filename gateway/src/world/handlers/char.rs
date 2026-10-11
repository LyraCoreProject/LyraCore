//! Character selection, world entry, and connection-level ping and Realm Clock replies.

use super::super::*;

/// Character select, and the per-Character reads world entry builds the self CREATE from.
pub(crate) trait CharacterStore: Send + Sync {
    /// The account's characters for the character-select screen. In production this
    /// reads the per-player `game_character` subscription (RLS-restricted to the owner).
    fn characters(&self, account_id: u64) -> Result<Vec<codec::CharacterView>>;

    /// Create a character for the account (`CMSG_CHAR_CREATE`). Returns the game outcome
    /// (success / name-in-use / failed); `Err` only for a Transport Loss.
    fn create_character(
        &self,
        account_id: u64,
        name: &str,
        race: u8,
        class: u8,
        gender: u8,
        appearance: codec::Appearance,
    ) -> Result<codec::CharCreateOutcome>;

    /// Delete `character` for the account (`CMSG_CHAR_DELETE`). Returns the game outcome
    /// (success/failed); `Err` only for a Transport Loss. Ownership is enforced module-side (the
    /// character must belong to `account_id`).
    fn delete_character(
        &self,
        account_id: u64,
        character: Actor,
    ) -> Result<codec::CharDeleteOutcome>;

    /// Look up a character by guid (any owner) to answer `CMSG_NAME_QUERY` — the queried guid is
    /// usually a peer, so this is not account-scoped.
    fn character_by_guid(&self, guid: u64) -> Result<Option<codec::CharacterView>>;

    /// Does any World Shard hold this Character? `false` means every configured Shard was readable
    /// and had no row. An incomplete, unhealthy, or changing Shard set must return `Err`.
    fn character_exists_on_any_world_shard(&self, guid: u64) -> Result<bool>;

    /// The character's learned skill lines as `(skill_line, current, max_rank)` — feeds the self
    /// CREATE's SkillInfo block. Empty when no `game_player_skill` rows exist.
    fn player_skills(&self, character_guid: u64) -> Result<Vec<(u32, u16, u16)>>;

    /// The EFFECTIVE armor for `guid` (base + worn gear armor) for the self-login CREATE's
    /// `UNIT_FIELD_RESISTANCES[0]` — so the character sheet shows real worn armor on relog. Auras aren't
    /// folded here (they self-correct via the on_aura relay). Mirrors the module's combat `effective_armor`.
    fn effective_armor(&self, guid: u64) -> u32;

    fn effective_magic_resistances(&self, guid: u64) -> [u32; 6];

    /// The character's active spell-modifier auras as raw (family_mask, op, amount, is_pct) rows —
    /// the SMSG_SET_FLAT/PCT_SPELL_MODIFIER mirror source.
    fn spell_modifiers(&self, character_guid: u64) -> Vec<(u32, u8, i32, bool)>;

    /// The player's LEARNED spells (`game_player_spell`, beyond the class kit) — chained into the
    /// login SMSG_INITIAL_SPELLS so a taught ability (e.g. Auto Shot) reaches the client spellbook.
    fn player_learned_spells(&self, player_guid: u64) -> Result<Vec<u32>>;

    /// The player's persisted reputation standings (`game_player_reputation`) as `(reputation_index,
    /// standing)` pairs — folded into the login `SMSG_INITIALIZE_FACTIONS` so a relog shows
    /// the real standing instead of the all-neutral stub.
    fn player_reputations(&self, player_guid: u64) -> Result<Vec<(i32, i32, bool)>>;

    /// The player's IMPORTED action-bar rows (`game_player_action`) as `(button,
    /// action, action_type)` triples — empty pre-import (the common case today), in which case the
    /// login codec falls back to synthesizing the bar from the spellbook (byte-identical to before
    /// this method existed).
    fn player_actions(&self, player_guid: u64) -> Result<Vec<(u8, u32, u8)>>;
}

pub(crate) struct Character;

impl<St: CharacterStore + GuildActionStore + SessionStore + ?Sized> ProtocolFamily<St>
    for Character
{
    #[allow(clippy::too_many_lines)] // One match arm per Character opcode.
    fn handle(
        store: &St,
        session: &mut ProtocolSession,
        request: ProtocolRequest,
    ) -> Result<ProtocolReply> {
        let mut reply = ProtocolReply::default();
        let response = match request.message()? {
            ClientOpcodeMessage::CMSG_PING(ping) => Some(ServerOpcodeMessage::SMSG_PONG(
                wow_world_messages::vanilla::SMSG_PONG {
                    sequence_id: ping.sequence_id,
                },
            )),
            ClientOpcodeMessage::CMSG_QUERY_TIME => {
                let seconds = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_secs();
                Some(ServerOpcodeMessage::SMSG_QUERY_TIME_RESPONSE(
                    wow_world_messages::vanilla::SMSG_QUERY_TIME_RESPONSE {
                        time: u32::try_from(seconds)?,
                    },
                ))
            }
            ClientOpcodeMessage::CMSG_CHAR_ENUM => {
                let characters = store.characters(session.account_id)?;
                Some(ServerOpcodeMessage::SMSG_CHAR_ENUM(Box::new(
                    codec::build_char_enum(&characters)?,
                )))
            }
            ClientOpcodeMessage::CMSG_CHAR_CREATE(character) => {
                let appearance = codec::Appearance {
                    skin: character.skin_color,
                    face: character.face,
                    hair_style: character.hair_style,
                    hair_color: character.hair_color,
                    facial_hair: character.facial_hair,
                };
                let outcome = store.create_character(
                    session.account_id,
                    character.name.as_str(),
                    character.race.as_int(),
                    character.class.as_int(),
                    character.gender.as_int(),
                    appearance,
                )?;
                Some(ServerOpcodeMessage::SMSG_CHAR_CREATE(
                    codec::build_char_create_response(outcome),
                ))
            }
            ClientOpcodeMessage::CMSG_CHAR_DELETE(request) => {
                let character_guid = request.guid.guid();
                // Realm-core holds Guild leadership. An unreadable answer must prevent deletion.
                let outcome = match (
                    Actor::new(character_guid),
                    super::leads_a_guild(store, character_guid),
                ) {
                    (None, _) => codec::CharDeleteOutcome::Failed,
                    (Some(character), Ok(false)) => {
                        store.delete_character(session.account_id, character)?
                    }
                    (Some(_), Ok(true)) => {
                        log::info!("world: Guild Leader {character_guid} is not deleted");
                        codec::CharDeleteOutcome::Failed
                    }
                    (Some(_), Err(error)) => {
                        log::warn!(
                            "world: Guild Leader check for {character_guid} failed, not deleted: \
                             {error:#}"
                        );
                        codec::CharDeleteOutcome::Failed
                    }
                };
                Some(ServerOpcodeMessage::SMSG_CHAR_DELETE(
                    codec::build_char_delete_response(outcome),
                ))
            }
            ClientOpcodeMessage::CMSG_PLAYER_LOGIN(request) => {
                let character = Actor::new(request.guid.guid())
                    .ok_or_else(|| anyhow!("CMSG_PLAYER_LOGIN names no Character"))?;
                reply.after_queue = Some(WorldSessionAction::Login(character));
                None
            }
            ClientOpcodeMessage::MSG_MOVE_WORLDPORT_ACK => {
                reply.after_queue = Some(WorldSessionAction::WorldPortAck);
                None
            }
            ClientOpcodeMessage::CMSG_LOGOUT_CANCEL => {
                Some(ServerOpcodeMessage::SMSG_LOGOUT_CANCEL_ACK)
            }
            ClientOpcodeMessage::CMSG_LOGOUT_REQUEST | ClientOpcodeMessage::CMSG_PLAYER_LOGOUT => {
                if let WorldState::InWorld(world) = &session.state {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    if store.player_combat_until_ms(world.self_guid) > now_ms {
                        return Ok(
                            vec![Outbound::One(ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(
                                codec::logout_denied_in_combat(),
                            ))]
                            .into(),
                        );
                    }
                }
                reply
                    .outbound
                    .push(Outbound::Batch(codec::logout_sequence()));
                reply.after_queue = Some(WorldSessionAction::Logout);
                None
            }
            ClientOpcodeMessage::CMSG_PLAYED_TIME => {
                if let Some(guid) = session.self_guid() {
                    if let Some(character) = store.character_by_guid(guid)? {
                        let now_micros = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_micros() as u64;
                        reply
                            .outbound
                            .push(Outbound::One(ServerOpcodeMessage::SMSG_PLAYED_TIME(
                                codec::build_played_time(
                                    character.played_total_secs,
                                    character.session_start_micros,
                                    now_micros,
                                ),
                            )));
                    }
                }
                None
            }
            message => return Err(anyhow!("Character received an unowned opcode: {message}")),
        };
        if let Some(response) = response {
            reply.outbound.push(Outbound::One(response));
        }
        Ok(reply)
    }
}
