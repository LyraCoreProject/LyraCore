//! Class-trainer family: open the trainer window and buy spells. Pure code-motion out of
//! `world/mod.rs`.

use super::super::*;
use crate::stdb::{classify, DurableFailure};
use lyracore_shared::trainer::TrainerRefusal;
use wow_world_messages::vanilla::TrainingFailureReason;

/// Trainer windows, talents, and the action-bar and reputation settings.
pub(crate) trait TrainerStore: Send + Sync {
    /// Does this trainer serve `player_guid`'s class? Gates the window and the "train" gossip
    /// option through the same predicate the module buys with. Fail-open on missing data.
    fn trainer_serves(&self, player_guid: u64, trainer_guid: u64) -> Result<bool>;

    /// The spells a class trainer (`trainer_guid`) teaches, each pre-folded with the player's level +
    /// known-state for the `SMSG_TRAINER_LIST` Green/Red/Gray rendering (`CMSG_TRAINER_LIST`).
    fn trainer_list(
        &self,
        player_guid: u64,
        trainer_guid: u64,
    ) -> Result<Vec<codec::TrainerSpellView>>;

    /// Buy/learn `spell_id` from trainer `trainer_guid` (`CMSG_TRAINER_BUY_SPELL`). The module gates it
    /// (range / level / cost / not-already-known) and a Refusal comes back as an outcome; `Err` means
    /// the durable result is unknown.
    fn buy_trainer_spell(
        &self,
        actor: Actor,
        trainer_guid: u64,
        spell_id: u32,
    ) -> Result<TrainerBuyOutcome>;

    /// The skill line a trainer offering teaches (`game_trainer_spell.learn_skill_line`), or 0 for an
    /// ordinary spell offering. The buy path reads it to tell a SKILL purchase from a SPELL purchase:
    /// a riding offering's id is a marker with no Spell.dbc row, so echoing it as a learned spell would
    /// push the client something it cannot resolve. 0 on any missing read (fail toward the spell echo,
    /// which is the pre-existing behaviour).
    fn trainer_offer_skill_line(&self, trainer_guid: u64, spell_id: u32) -> u32;

    /// Return the `grant_spell_id` for `talent_id` (0 = passive, no ability granted), so the gateway
    /// can push `SMSG_LEARNED_SPELL` for ability talents after a successful `learn_talent`.
    fn talent_grant_spell(&self, talent_id: u32) -> u32;

    /// Persist one action-bar button (`CMSG_SET_ACTION_BUTTON`); action 0 clears the slot.
    fn set_action_button(
        &self,
        actor: Actor,
        button: u8,
        action: u32,
        action_type: u8,
    ) -> Result<()>;

    /// Persist the rep pane's At-War checkbox (`CMSG_SET_FACTION_ATWAR`).
    /// `reputation_index` is the client's 0..63 rep-array slot, NOT a faction id.
    fn set_faction_at_war(
        &self,
        actor: Actor,
        reputation_index: u32,
        at_war: bool,
    ) -> Result<InteractionOutcome>;

    /// Talent-pane sync after a successful `learn_talent`: `(teach_spell, superseded_prev,
    /// points_remaining)` — the rank-spell to relay as LEARNED/SUPERCEDED (the 1.12 TalentFrame
    /// derives shown ranks from known rank-spells) and the live PLAYER_CHARACTER_POINTS1 value
    /// (earned − spent). `talent_id = 0` → just the points.
    fn talent_pane_sync(&self, character_guid: u64, talent_id: u32) -> (u32, u32, u32);

    /// Sum of the character's spent talent ranks — non-zero gates the login points correction.
    fn talent_points_spent(&self, character_guid: u64) -> u32;

    /// Spend a talent point on `talent_id` (`CMSG_LEARN_TALENT`). The module gates it (points available
    /// / max rank / prerequisites); a gameplay `Err` is per-action, not session-fatal.
    fn learn_talent(&self, actor: Actor, talent_id: u32) -> Result<()>;

    /// Quote the Module's current talent reset cost for the confirmation dialog.
    fn talent_reset_cost(&self, character_guid: u64) -> Option<u32>;

    /// Confirm a talent reset. The Module checks the trainer, range and price.
    fn reset_talents(&self, actor: Actor, trainer_guid: u64) -> Result<InteractionOutcome>;

    /// The rank a trainer offering actually teaches (LearnSpell wrapper → its trigger; a
    /// self-contained rank resolves to itself). Mirrors the module's buy-time resolution so
    /// SMSG_LEARNED_SPELL books the granted spell, never the wrapper.
    fn resolve_learn_target(&self, spell_id: u32) -> u32;

    /// The KNOWN rank `new_spell` supersedes — Some(prev) drives SMSG_SUPERCEDED_SPELL on a buy.
    fn superseded_old_rank(&self, new_spell: u32, player_guid: u64) -> Option<u32>;

    /// A character's live presence `(online, level, class, zone_id)` on THIS Shard only. `None` if
    /// the guid doesn't resolve to any character here.
    fn character_presence(&self, guid: u64) -> Result<Option<(bool, u8, u8, u32)>>;
}

/// Settle a Durable Request whose failure the client never hears about: a Refusal is logged and the
/// World Session goes on, and a Transport Loss ends it.
pub(super) fn settle_per_action(what: &str, account_id: u64, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error) => match classify(&error) {
            DurableFailure::Refusal { reason } => {
                log::debug!("world: {what} ignored (account {account_id}): {reason}");
                Ok(())
            }
            DurableFailure::TransportLoss => Err(error),
        },
    }
}

/// How the Module answered a trainer purchase. A Refusal is a gameplay answer the client can render;
/// a failed Durable Request is not, so it never reaches here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrainerBuyOutcome {
    Learned,
    Refused(TrainingFailureReason),
}

// gtker vanilla carries three failure reasons, so several gameplay Refusals share one. The client
// pre-gates the Learn button on the list's Green state, which is what keeps the collapse cosmetic.
impl From<TrainerRefusal> for TrainerBuyOutcome {
    fn from(refusal: TrainerRefusal) -> Self {
        Self::Refused(match refusal {
            TrainerRefusal::NotEnoughMoney => TrainingFailureReason::NotEnoughMoney,
            TrainerRefusal::LevelTooLow | TrainerRefusal::PreviousRankMissing => {
                TrainingFailureReason::NotEnoughSkill
            }
            TrainerRefusal::Unavailable
            | TrainerRefusal::NotOffered
            | TrainerRefusal::AlreadyKnown => TrainingFailureReason::Unavailable,
        })
    }
}

/// Class-trainer family: open the trainer window (`CMSG_TRAINER_LIST` → `SMSG_TRAINER_LIST`, each spell
/// Green/Red/Gray) and learn a spell (`CMSG_TRAINER_BUY_SPELL` → the module buy →
/// `SMSG_TRAINER_BUY_*` + a live `SMSG_LEARNED_SPELL` so it hits the action bar without a relog).
/// Needs the in-world player guid (a trainer is only clicked in-world); in CharSelect the opcodes
/// pass through. A buy Refusal is per-action — surfaced as `SMSG_TRAINER_BUY_FAILED`.
pub(crate) fn handle_trainer<St: CharacterStore + NpcStore + TrainerStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    msg: ClientOpcodeMessage,
) -> Result<Option<ClientOpcodeMessage>> {
    let Some(actor) = (match &conn.state {
        WorldState::InWorld(iw) => Actor::new(iw.self_guid),
        WorldState::CharSelect => None,
    }) else {
        return Ok(Some(msg));
    };
    let self_guid = actor.guid();
    match msg {
        ClientOpcodeMessage::CMSG_TRAINER_LIST(c) => {
            let trainer_guid = c.guid.guid();
            // A trainer that dislikes you refuses the window (silent drop).
            if store
                .npc_refuses_interaction(trainer_guid, self_guid)
                .unwrap_or(false)
            {
                return Ok(None);
            }
            // Wrong class: silent drop, not an empty window — an empty list is indistinguishable
            // from a trainer whose offerings were never imported.
            if !store
                .trainer_serves(self_guid, trainer_guid)
                .unwrap_or(true)
            {
                return Ok(None);
            }
            let spells = store.trainer_list(self_guid, trainer_guid)?;
            // Deliberate simplification: a generic greeting — the per-NPC trainer greeting text is
            // a later npc_text slice (same as the vendor's generic gossip line).
            let list =
                codec::build_trainer_list(trainer_guid, &spells, "I can teach you a thing or two.");
            send(
                tx,
                Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_LIST(Box::new(list))),
            )?;
        }
        ClientOpcodeMessage::CMSG_TRAINER_BUY_SPELL(c) => {
            let trainer_guid = c.guid.guid();
            let spell_id = c.id;
            // A Refusal arrives as an outcome. An error leaves the durable result unknown, so it
            // ends the session instead of posing as a gameplay answer.
            match store.buy_trainer_spell(actor, trainer_guid, spell_id)? {
                TrainerBuyOutcome::Learned => {
                    // Confirm + push the spell live so it appears on the action bar without a relog.
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(Box::new(
                            codec::build_trainer_buy_succeeded(trainer_guid, spell_id),
                        ))),
                    )?;
                    // RIDING buy: the offering teaches a SKILL, and its trainer-list id is a marker with
                    // no Spell.dbc row behind it — echoing that as a learned spell would push the client
                    // an id it cannot resolve. The skill pane already moves on its own, from the live
                    // `game_player_skill` relay, so a riding purchase needs no spell echo at all.
                    // Profession offerings keep theirs: the importer synthesizes them with real
                    // learn-spell ids the client does resolve.
                    if store.trainer_offer_skill_line(trainer_guid, spell_id)
                        == lyracore_shared::trainer::RIDING_SKILL_LINE
                    {
                        return Ok(None);
                    }
                    // Book the RESOLVED rank (465), not the wrapper (1875) — the module granted
                    // the trigger spell; echoing the wrapper put "the spell that teaches Devotion
                    // Aura" in the player's General tab until relog.
                    // A RANK UPGRADE (the chain prev is already known) sends SUPERCEDED
                    // instead — the client REPLACES the old rank's book entry (vanilla) rather
                    // than stacking "Rank 1" next to "Rank 2". WIRE ORDER: cmangos writes
                    // old u16 THEN new u16; gtker's field names claim new-first — per the
                    // field-names-lie precedent we follow cmangos, so `new_spell_id` (the FIRST
                    // wire slot) carries the OLD rank. If live verify shows the NEW rank
                    // vanishing instead, swap these two.
                    let resolved = store.resolve_learn_target(spell_id);
                    match store.superseded_old_rank(resolved, self_guid) {
                        Some(old_rank) => {
                            use wow_world_messages::vanilla::SMSG_SUPERCEDED_SPELL;
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(
                                    SMSG_SUPERCEDED_SPELL {
                                        new_spell_id: old_rank as u16,
                                        old_spell_id: resolved as u16,
                                    },
                                )),
                            )?;
                        }
                        None => {
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                    codec::build_learned_spell(resolved),
                                )),
                            )?;
                        }
                    }
                    // An armor-proficiency purchase widens what this Character may wear, and the
                    // client only learns that from SMSG_SET_PROFICIENCY. Re-read the spellbook the
                    // buy just changed and resend the ARMOR mask; the weapon table never moves.
                    if teaches_armor_proficiency(spell_id) || teaches_armor_proficiency(resolved) {
                        send_armor_proficiency(tx, store, self_guid)?;
                    }
                }
                TrainerBuyOutcome::Refused(reason) => {
                    log::debug!(
                        "world: trainer buy refused (account {}): {reason:?}",
                        conn.account_id
                    );
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_TRAINER_BUY_FAILED(Box::new(
                            codec::build_trainer_buy_failed(trainer_guid, spell_id, reason),
                        ))),
                    )?;
                }
            }
        }
        // Spend a talent point (`CMSG_LEARN_TALENT`). The module gates points/prereqs; on success the
        // passive aura relay covers stat/buff updates. If this talent also grants a learnable ability
        // (`grant_spell_id != 0`), push `SMSG_LEARNED_SPELL` so the action bar is usable without a relog.
        // Action-bar persistence: the client sends ONE of these per drag/clear and expects the
        // full bar back at login (SMSG_ACTION_BUTTONS). Unhandled until now — every bar change
        // was lost on relog (only the creation-seeded buttons survived; user find via a
        // talent-learned Consecration vanishing from the bar). `action`+`misc` are the client's
        // packed u24 payload (spell id, or item id spilling into misc); best-effort (a failure
        // must never drop the session — the button just won't stick).
        ClientOpcodeMessage::CMSG_SET_ACTION_BUTTON(c) => {
            let action = c.action as u32 | ((c.misc as u32) << 16);
            settle_per_action(
                "set_action_button",
                conn.account_id,
                store.set_action_button(actor, c.button, action, c.action_type),
            )?;
        }
        ClientOpcodeMessage::CMSG_LEARN_TALENT(c) => {
            let talent_id = c.talent.as_int();
            let grant_spell_id = store.talent_grant_spell(talent_id);
            match store.learn_talent(actor, talent_id) {
                Ok(()) => {
                    if grant_spell_id != 0 {
                        send(
                            tx,
                            Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                codec::build_learned_spell(grant_spell_id),
                            )),
                        )?;
                    }

                    let (teach, superseded, remaining) =
                        store.talent_pane_sync(self_guid, talent_id);
                    if teach != 0 && teach != grant_spell_id {
                        if superseded != 0 {
                            use wow_world_messages::vanilla::SMSG_SUPERCEDED_SPELL;
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(
                                    SMSG_SUPERCEDED_SPELL {
                                        new_spell_id: superseded as u16, // cmangos wire order: OLD rides the first slot
                                        old_spell_id: teach as u16,
                                    },
                                )),
                            )?;
                        } else {
                            send(
                                tx,
                                Outbound::One(ServerOpcodeMessage::SMSG_LEARNED_SPELL(
                                    codec::build_learned_spell(teach),
                                )),
                            )?;
                        }
                    }
                    send(
                        tx,
                        Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                            codec::build_talent_points_values(self_guid, remaining),
                        ))),
                    )?;
                    // Spell-modifier mirror: the pick may have applied an A_SPELLMOD
                    // passive — re-send the aggregated totals so the client's cast bars match
                    // the server's folded timings immediately (idempotent absolute values).
                    for m in codec::build_spell_modifier_msgs(&store.spell_modifiers(self_guid)) {
                        send(tx, Outbound::One(m))?;
                    }
                }
                Err(error) => settle_per_action("learn_talent", conn.account_id, Err(error))?,
            }
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
}

/// Does this trainer offering teach an armor proficiency? Both the trainer-list wrapper and the
/// passive it resolves to count, so the check lands whichever id the buy path reports.
fn teaches_armor_proficiency(spell_id: u32) -> bool {
    use lyracore_shared::constants::armor_proficiency::*;

    matches!(
        spell_id,
        PLATE_TRAINER_SPELL_ID
            | PLATE_PASSIVE_SPELL_ID
            | MAIL_TRAINER_SPELL_ID
            | MAIL_PASSIVE_SPELL_ID
    )
}

/// Push the Character's ARMOR `SMSG_SET_PROFICIENCY` from its CURRENT spellbook, so the client
/// re-tints its bags without a relog. Read after the buy: the mask states what the Character knows
/// now, not what the purchase was meant to grant, so a buy the Module only half-applied never
/// tints an item the equip Gate would still refuse.
fn send_armor_proficiency<St: CharacterStore + TrainerStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    self_guid: u64,
) -> Result<()> {
    let learned = store.player_learned_spells(self_guid).unwrap_or_default();
    // `character_presence` is the store's existing class read; a proficiency buy happens twice in a
    // character's life, so the lookup is not worth a dedicated one.
    let Some((_, _, player_class, _)) = store.character_presence(self_guid).ok().flatten() else {
        return Ok(());
    };
    send(
        tx,
        Outbound::One(codec::build_armor_proficiency_msg(player_class, &learned)),
    )
}

/// The vanilla request names a reputation-list index, not a Faction.dbc id.
pub(crate) fn handle_at_war<St: TrainerStore + SessionStore + ?Sized>(
    tx: &SessionTx,
    store: &St,
    conn: &mut WorldConn,
    body: &[u8],
) -> Result<()> {
    anyhow::ensure!(
        body.len() == 5 && body[4] <= 1,
        "invalid CMSG_SET_FACTION_ATWAR body"
    );
    let Some(actor) = social::self_guid(conn).and_then(Actor::new) else {
        return Ok(());
    };
    let index = u32::from_le_bytes(body[..4].try_into()?);
    if let Some((opcode, info)) = conn.move_coalesce.flush_now() {
        forward_movement(store, conn, opcode, &info)?;
    }
    if let InteractionOutcome::Refused(reason) =
        store.set_faction_at_war(actor, index, body[4] != 0)?
    {
        send(
            tx,
            Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                codec::build_gm_system_message(reason),
            ))),
        )?;
    }
    Ok(())
}
