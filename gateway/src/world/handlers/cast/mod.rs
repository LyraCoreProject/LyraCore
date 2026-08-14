//! Cast-lifecycle seam: one entry point owns `CMSG_CAST_SPELL` route selection, client target
//! decoding, durable dispatch and the ordered client-visible messages.
//!
//! The module root holds the seam types, the route decision tree and the ordinary cast route
//! (instant, timed, next-swing, ground-area and ground-targeted casts). The ranged auto-repeat and
//! manual-completion routes are recognised here and passed through to the combat handler until
//! their own sibling modules exist.

use super::super::*;
use wow_world_messages::vanilla::CMSG_CAST_SPELL;

/// `SMSG_CAST_RESULT`. Both bodies are hand-rolled (gtker's typed message inverts the status
/// semantics), so they ride `Outbound::Raw` under this opcode.
const OP_CAST_RESULT: u16 = 0x0130;

/// Imported spell taxonomy and the durable cast operations the module needs. Narrow on purpose:
/// nothing here exposes a coordinator read, a reducer handle or a spell table to the caller.
pub(crate) trait CastStore: Send + Sync {
    /// True iff `spell_id` is an auto-repeat ranged attack (Auto Shot / wand Shoot) — the
    /// `RANGED_AUTO_REPEAT` cast_flags bit, so a new ranged ability onboards as data.
    fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool;

    /// Enchant/disenchant routing from `spell_id`'s effect rows; `None` for every other cast.
    fn enchant_route(&self, spell_id: u32) -> Option<EnchantRoute>;

    /// True iff `spell_id` is a Fishing cast (E_FISH).
    fn spell_is_fishing(&self, spell_id: u32) -> bool;

    /// True iff `spell_id` is an Open-Lock cast (E_OPEN_LOCK).
    fn spell_is_open_lock(&self, spell_id: u32) -> bool;

    /// True iff `spell_id` spawns a ground area (E_PERSISTENT_AREA) — its GO carries no hit list.
    fn spell_is_ground_area(&self, spell_id: u32) -> bool;

    /// The spell's cast time in ms — 0 is instant, `None` is unknown metadata.
    fn spell_cast_time(&self, spell_id: u32) -> Option<u32>;

    /// True iff `spell_id` queues on the caster's next melee swing (Heroic Strike/Cleave).
    fn spell_queues_next_swing(&self, spell_id: u32) -> bool;

    /// Cast a spell at a unit. `target_guid` 0 means no unit target — the module then applies its
    /// own self-cast rule.
    fn cast_spell(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        target_guid: u64,
    ) -> Result<()>;

    /// Cast a ground-targeted spell at the exact point the client clicked.
    #[allow(clippy::too_many_arguments)]
    fn cast_spell_at(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()>;
}

impl CastStore for crate::stdb::Coordinator {
    fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_is_ranged_auto_repeat(self, spell_id)
    }

    fn enchant_route(&self, spell_id: u32) -> Option<EnchantRoute> {
        crate::stdb::Coordinator::enchant_route(self, spell_id)
    }

    fn spell_is_fishing(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_is_fishing(self, spell_id)
    }

    fn spell_is_open_lock(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_is_open_lock(self, spell_id)
    }

    fn spell_is_ground_area(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_is_ground_area(self, spell_id)
    }

    fn spell_cast_time(&self, spell_id: u32) -> Option<u32> {
        crate::stdb::Coordinator::spell_cast_time(self, spell_id)
    }

    fn spell_queues_next_swing(&self, spell_id: u32) -> bool {
        crate::stdb::Coordinator::spell_queues_next_swing(self, spell_id)
    }

    fn cast_spell(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        target_guid: u64,
    ) -> Result<()> {
        crate::stdb::Coordinator::cast_spell(self, account_id, self_guid, spell_id, target_guid)
    }

    fn cast_spell_at(
        &self,
        account_id: u64,
        self_guid: u64,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()> {
        crate::stdb::Coordinator::cast_spell_at(
            self,
            account_id,
            self_guid,
            spell_id,
            target_guid,
            x,
            y,
            z,
        )
    }
}

/// Everything the cast module knows about the caller. `self_guid` is `None` when the session has no
/// character in the world: the durable call then carries guid 0 and no synchronous message is sent,
/// which is what the pre-seam handler did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CastPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
    /// A ranged auto-repeat loop is armed. Only the ranged route reads it.
    pub(crate) ranged_repeat: bool,
}

/// Session state the dispatcher applies before it sends the outbound batch. The ordinary cast route
/// never sets one; ranged auto-repeat activation and cancellation do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CastTransition {
    /// `Some(armed)` writes the session's ranged auto-repeat flag; `None` leaves it alone.
    pub(crate) ranged_repeat: Option<bool>,
}

/// The result of one cast request. `Handled` carries the complete, ordered client-visible batch —
/// exact message order is external behaviour for the vanilla client, so the outcome states it
/// rather than leaving it to the caller.
pub(crate) enum CastOutcome {
    Handled {
        transition: CastTransition,
        outbound: Vec<Outbound>,
    },
    PassThrough(ClientOpcodeMessage),
}

/// Which route a `CMSG_CAST_SPELL` takes, selected from imported taxonomy alone.
enum CastRoute {
    /// Auto Shot and wand Shoot: an auto-repeat attack loop, not a one-shot cast.
    RangedAutoRepeat,
    /// Enchant, disenchant, fishing and lock opening. Their reducers emit no cast event, so the
    /// route sends the completion sequence itself.
    ManualCompletion,
    /// Every other cast: instant, timed, next-swing, ground-area and ground-targeted.
    Ordinary,
}

fn route_for<St: CastStore + ?Sized>(store: &St, spell_id: u32) -> CastRoute {
    if store.spell_is_ranged_auto_repeat(spell_id) {
        CastRoute::RangedAutoRepeat
    } else if store.enchant_route(spell_id).is_some()
        || store.spell_is_fishing(spell_id)
        || store.spell_is_open_lock(spell_id)
    {
        CastRoute::ManualCompletion
    } else {
        CastRoute::Ordinary
    }
}

/// A dead reducer transport cannot serve any further request, so it ends the session. Every other
/// durable error is a gameplay refusal the player sees as a cast failure.
fn is_transport_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().contains("reducer transport disconnected"))
}

/// The client's unit target, or 0 for "none" — the module substitutes the caster.
fn unit_target(c: &CMSG_CAST_SPELL) -> u64 {
    c.targets
        .target_flags
        .get_unit()
        .map_or(0, |u| u.unit_target.guid())
}

/// The ground point the player clicked (`DEST_LOCATION`), if the cast carries one.
fn dest_target(c: &CMSG_CAST_SPELL) -> Option<(f32, f32, f32)> {
    c.targets
        .target_flags
        .get_dest_location()
        .map(|d| (d.destination.x, d.destination.y, d.destination.z))
}

/// Route one client message through the cast seam. Anything that is not a cast request, and every
/// route a sibling module does not own yet, passes through untouched.
pub(crate) fn dispatch_cast<St: CastStore + ?Sized>(
    store: &St,
    player: CastPlayer,
    msg: ClientOpcodeMessage,
) -> Result<CastOutcome> {
    match msg {
        ClientOpcodeMessage::CMSG_CAST_SPELL(c) => match route_for(store, c.spell) {
            CastRoute::Ordinary => ordinary_cast(store, player, &c),
            CastRoute::RangedAutoRepeat | CastRoute::ManualCompletion => Ok(
                CastOutcome::PassThrough(ClientOpcodeMessage::CMSG_CAST_SPELL(c)),
            ),
        },
        other => Ok(CastOutcome::PassThrough(other)),
    }
}

/// The ordinary cast route: instant, timed, next-swing, ground-area and ground-targeted casts.
fn ordinary_cast<St: CastStore + ?Sized>(
    store: &St,
    player: CastPlayer,
    c: &CMSG_CAST_SPELL,
) -> Result<CastOutcome> {
    let spell = c.spell;
    // Thread the client's unit target so target-keyed effects — combo finishers, enemy spells —
    // see the real target. No unit target stays 0 so the module applies its own self-cast rule.
    let target = unit_target(c);
    let mut outbound = Vec::new();

    // An INSTANT cast clears the client's cast slot HERE, before the durable request. The
    // cast-event relay would deliver START/GO after the aura callback (the SDK fires table
    // callbacks in a fixed alphabetical order), so the applied buff would reach the client first
    // and wedge its cast slot in "Another action is in progress". Unknown cast time counts as
    // instant: a stray START/GO is harmless, a missing one wedges.
    let instant = store.spell_cast_time(spell).is_none_or(|t| t == 0);
    // A next-swing spell (Heroic Strike/Cleave) sends nothing. The client lights the button on the
    // press and holds the pending cast until the durable swing-fire emits its completion; the
    // sequence below would un-light the button and resolve the cast at queue time.
    let queues_swing = instant && store.spell_queues_next_swing(spell);
    if instant && !queues_swing {
        if let Some(caster) = player.self_guid {
            // vmangos order: START(0) then the raw CAST_RESULT(OK) then GO. The 5875 client needs
            // that 5-byte ack before GO to make m_currentSpells clearable.
            outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
                Box::new(codec::build_spell_start(caster, spell, 0, 0, None)),
            )));
            outbound.push(Outbound::Raw {
                opcode: OP_CAST_RESULT,
                body: codec::build_cast_result_ok(spell),
            });
            // A ground-area spell (Consecration) impacts the ground: an EMPTY hit list, or the
            // self-cast fallback puts the caster in `hits[]` and the client plays the impact
            // animation on the caster.
            let go = if store.spell_is_ground_area(spell) {
                codec::build_spell_go_area(caster, spell)
            } else {
                codec::build_spell_go(caster, spell, target, None)
            };
            outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
                go,
            ))));
        }
    }

    // A DEST_LOCATION block is a ground click (Flamestrike/Blizzard/Rain of Fire) — the durable
    // ground cast anchors the area there. The clearing sequence above is unchanged either way.
    let self_guid = player.self_guid.unwrap_or(0);
    let result = match dest_target(c) {
        Some((x, y, z)) => {
            store.cast_spell_at(player.account_id, self_guid, spell, target, x, y, z)
        }
        None => store.cast_spell(player.account_id, self_guid, spell, target),
    };
    if let Err(e) = result {
        if is_transport_failure(&e) {
            return Err(e);
        }
        // Carry the mapped REASON so the client prints the red error line ("Not enough rage",
        // "You must be behind your target"). A bare failure only resets the button and leaves
        // server-only gates — behind, stealth, stance, react window — invisible.
        log::debug!(
            "world: cast {spell} rejected (account {}): {e}",
            player.account_id
        );
        outbound.push(Outbound::Raw {
            opcode: OP_CAST_RESULT,
            body: codec::build_cast_result_failed(spell, codec::cast_failure_reason_for(&e.to_string())),
        });
    }
    Ok(CastOutcome::Handled {
        transition: CastTransition::default(),
        outbound,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        Guid, SpellCastTargets, SpellCastTargets_SpellCastTargetFlags,
        SpellCastTargets_SpellCastTargetFlags_DestLocation,
        SpellCastTargets_SpellCastTargetFlags_Unit, Vector3d, CMSG_PING,
    };

    /// One recorded durable cast: account, caster, spell and unit target.
    type Cast = (u64, u64, u32, u64);
    /// One recorded durable ground cast: a [`Cast`] plus the click point.
    type GroundCast = (u64, u64, u32, u64, f32, f32, f32);

    /// Spell-route metadata and durable results for the cast routes, plus a record of every durable
    /// call. Nothing else: no vendors, parties, mail, transfer or unrelated world state.
    #[derive(Default)]
    struct InMemoryCasts {
        cast_time_ms: Option<u32>,
        queues_next_swing: bool,
        ground_area: bool,
        ranged_auto_repeat: Vec<u32>,
        enchant: Option<EnchantRoute>,
        fishing: Vec<u32>,
        open_lock: Vec<u32>,
        cast_error: Option<String>,
        casts: Mutex<Vec<Cast>>,
        ground_casts: Mutex<Vec<GroundCast>>,
    }

    impl InMemoryCasts {
        fn refusing(error: &str) -> Self {
            Self {
                cast_error: Some(error.into()),
                ..Default::default()
            }
        }

        fn instant() -> Self {
            Self {
                cast_time_ms: Some(0),
                ..Default::default()
            }
        }

        fn durable_result(&self) -> Result<()> {
            self.cast_error
                .as_ref()
                .map_or_else(|| Ok(()), |e| Err(anyhow!("{e}")))
        }
    }

    impl CastStore for InMemoryCasts {
        fn spell_is_ranged_auto_repeat(&self, spell_id: u32) -> bool {
            self.ranged_auto_repeat.contains(&spell_id)
        }

        fn enchant_route(&self, _spell_id: u32) -> Option<EnchantRoute> {
            self.enchant
        }

        fn spell_is_fishing(&self, spell_id: u32) -> bool {
            self.fishing.contains(&spell_id)
        }

        fn spell_is_open_lock(&self, spell_id: u32) -> bool {
            self.open_lock.contains(&spell_id)
        }

        fn spell_is_ground_area(&self, _spell_id: u32) -> bool {
            self.ground_area
        }

        fn spell_cast_time(&self, _spell_id: u32) -> Option<u32> {
            self.cast_time_ms
        }

        fn spell_queues_next_swing(&self, _spell_id: u32) -> bool {
            self.queues_next_swing
        }

        fn cast_spell(
            &self,
            account_id: u64,
            self_guid: u64,
            spell_id: u32,
            target_guid: u64,
        ) -> Result<()> {
            self.casts
                .lock()
                .unwrap()
                .push((account_id, self_guid, spell_id, target_guid));
            self.durable_result()
        }

        fn cast_spell_at(
            &self,
            account_id: u64,
            self_guid: u64,
            spell_id: u32,
            target_guid: u64,
            x: f32,
            y: f32,
            z: f32,
        ) -> Result<()> {
            self.ground_casts
                .lock()
                .unwrap()
                .push((account_id, self_guid, spell_id, target_guid, x, y, z));
            self.durable_result()
        }
    }

    const ACCOUNT: u64 = 7;
    const CASTER: u64 = 42;

    fn player() -> CastPlayer {
        CastPlayer {
            account_id: ACCOUNT,
            self_guid: Some(CASTER),
            ranged_repeat: false,
        }
    }

    fn cast(spell: u32, targets: SpellCastTargets) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CAST_SPELL(Box::new(CMSG_CAST_SPELL { spell, targets }))
    }

    fn unit_targets(guid: u64) -> SpellCastTargets {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
                SpellCastTargets_SpellCastTargetFlags_Unit {
                    unit_target: Guid::new(guid),
                },
            ),
        }
    }

    fn dest_targets(x: f32, y: f32, z: f32) -> SpellCastTargets {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_dest_location(
                SpellCastTargets_SpellCastTargetFlags_DestLocation {
                    destination: Vector3d { x, y, z },
                },
            ),
        }
    }

    /// The handled outcome, or a panic naming what came back instead.
    fn handled(outcome: CastOutcome) -> (CastTransition, Vec<Outbound>) {
        match outcome {
            CastOutcome::Handled {
                transition,
                outbound,
            } => (transition, outbound),
            CastOutcome::PassThrough(_) => panic!("expected a handled cast"),
        }
    }

    /// One label per outbound unit, in order — the synchronous sequence is the contract, and it
    /// spans both raw and typed sends.
    fn sequence(outbound: &[Outbound]) -> Vec<String> {
        outbound
            .iter()
            .map(|out| match out {
                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(_)) => "START".to_string(),
                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(_)) => "GO".to_string(),
                Outbound::Raw {
                    opcode: OP_CAST_RESULT,
                    body,
                } if body.len() == 5 => "CAST_RESULT(OK)".to_string(),
                Outbound::Raw {
                    opcode: OP_CAST_RESULT,
                    body,
                } => format!("CAST_RESULT(FAILED {:#04x})", body[5]),
                _ => "UNEXPECTED".to_string(),
            })
            .collect()
    }

    /// The single `SMSG_SPELL_GO` in the batch.
    fn spell_go(outbound: &[Outbound]) -> &wow_world_messages::vanilla::SMSG_SPELL_GO {
        outbound
            .iter()
            .find_map(|out| match out {
                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(go)) => Some(&**go),
                _ => None,
            })
            .expect("the batch has no SMSG_SPELL_GO")
    }

    // ── Instant casts ────────────────────────────────────────────────────────

    #[test]
    fn instant_unit_target_cast_clears_the_client_then_requests_the_durable_cast() {
        let store = InMemoryCasts::instant();

        let (transition, outbound) =
            handled(dispatch_cast(&store, player(), cast(100, unit_targets(77))).unwrap());

        assert_eq!(
            sequence(&outbound),
            ["START", "CAST_RESULT(OK)", "GO"],
            "the 5875 client needs the OK ack between START and GO"
        );
        assert_eq!(transition, CastTransition::default());
        assert_eq!(
            store.casts.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER, 100, 77)]
        );
    }

    #[test]
    fn instant_cast_with_no_unit_target_passes_target_zero_to_the_durable_cast() {
        let store = InMemoryCasts::instant();

        handled(dispatch_cast(&store, player(), cast(100, SpellCastTargets::default())).unwrap());

        assert_eq!(
            store.casts.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER, 100, 0)],
            "target 0 preserves the module's self-cast fallback"
        );
    }

    #[test]
    fn missing_cast_time_metadata_is_treated_as_instant() {
        let store = InMemoryCasts::default();

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(42, unit_targets(77))).unwrap());

        assert_eq!(sequence(&outbound), ["START", "CAST_RESULT(OK)", "GO"]);
    }

    #[test]
    fn instant_unit_target_cast_puts_the_client_target_in_the_spell_go() {
        let store = InMemoryCasts::instant();

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(100, unit_targets(77))).unwrap());

        assert_eq!(
            spell_go(&outbound)
                .hits
                .iter()
                .map(|g| g.guid())
                .collect::<Vec<_>>(),
            [77]
        );
    }

    #[test]
    fn instant_ground_area_cast_sends_a_spell_go_with_an_empty_hit_list() {
        let store = InMemoryCasts {
            ground_area: true,
            ..InMemoryCasts::instant()
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(118, unit_targets(77))).unwrap());

        assert_eq!(sequence(&outbound), ["START", "CAST_RESULT(OK)", "GO"]);
        assert!(
            spell_go(&outbound).hits.is_empty(),
            "a ground area impacts the ground, not a unit"
        );
    }

    // ── Casts that resolve later ─────────────────────────────────────────────

    #[test]
    fn timed_cast_sends_no_synchronous_messages_and_still_requests_the_durable_cast() {
        let store = InMemoryCasts {
            cast_time_ms: Some(1500),
            ..Default::default()
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(100, unit_targets(77))).unwrap());

        assert!(
            outbound.is_empty(),
            "the cast-event relay owns a timed cast's lifecycle"
        );
        assert_eq!(
            store.casts.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER, 100, 77)]
        );
    }

    #[test]
    fn next_swing_cast_sends_no_synchronous_messages_and_still_requests_the_durable_cast() {
        let store = InMemoryCasts {
            queues_next_swing: true,
            ..InMemoryCasts::instant()
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(78, unit_targets(77))).unwrap());

        assert!(
            outbound.is_empty(),
            "the client holds the button until the durable swing fires the completion"
        );
        assert_eq!(
            store.casts.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER, 78, 77)]
        );
    }

    // ── Ground targeting ─────────────────────────────────────────────────────

    #[test]
    fn destination_target_cast_requests_the_durable_ground_cast_with_the_client_coordinates() {
        let store = InMemoryCasts {
            cast_time_ms: Some(2000),
            ..Default::default()
        };

        handled(
            dispatch_cast(&store, player(), cast(2120, dest_targets(-8913.5, 554.25, 93.75)))
                .unwrap(),
        );

        assert_eq!(
            store.ground_casts.lock().unwrap().as_slice(),
            &[(ACCOUNT, CASTER, 2120, 0, -8913.5, 554.25, 93.75)]
        );
        assert!(store.casts.lock().unwrap().is_empty());
    }

    // ── Refusal ──────────────────────────────────────────────────────────────

    #[test]
    fn refused_instant_cast_maps_the_reason_after_the_clearing_sequence() {
        let store = InMemoryCasts {
            cast_error: Some("not enough power".into()),
            ..InMemoryCasts::instant()
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(100, unit_targets(77))).unwrap());

        assert_eq!(
            sequence(&outbound),
            [
                "START",
                "CAST_RESULT(OK)",
                "GO",
                "CAST_RESULT(FAILED 0x4d)"
            ],
            "the client clears the pending cast first, then shows the refusal"
        );
    }

    #[test]
    fn refused_timed_cast_sends_only_the_mapped_failure() {
        let store = InMemoryCasts {
            cast_time_ms: Some(1500),
            ..InMemoryCasts::refusing("target is out of range")
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(100, unit_targets(77))).unwrap());

        assert_eq!(sequence(&outbound), ["CAST_RESULT(FAILED 0x59)"]);
    }

    #[test]
    fn a_dead_reducer_transport_is_session_fatal() {
        let store = InMemoryCasts {
            cast_time_ms: Some(1500),
            ..InMemoryCasts::refusing("cast_spell reducer transport disconnected: channel closed")
        };

        let error = match dispatch_cast(&store, player(), cast(100, unit_targets(77))) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };

        assert!(format!("{error:#}").contains("reducer transport disconnected"));
    }

    // ── Player context ───────────────────────────────────────────────────────

    #[test]
    fn a_player_with_no_character_in_world_gets_no_messages_and_a_zero_actor() {
        let store = InMemoryCasts::instant();
        let player = CastPlayer {
            account_id: ACCOUNT,
            self_guid: None,
            ranged_repeat: false,
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player, cast(100, unit_targets(77))).unwrap());

        assert!(outbound.is_empty());
        assert_eq!(
            store.casts.lock().unwrap().as_slice(),
            &[(ACCOUNT, 0, 100, 77)]
        );
    }

    // ── Routes other modules own ─────────────────────────────────────────────

    #[test]
    fn ranged_auto_repeat_enchant_fishing_and_lock_opening_pass_through_untouched() {
        let store = InMemoryCasts {
            ranged_auto_repeat: vec![75],
            fishing: vec![7620],
            open_lock: vec![1804],
            ..InMemoryCasts::instant()
        };

        for spell in [75, 7620, 1804] {
            assert!(
                matches!(
                    dispatch_cast(&store, player(), cast(spell, unit_targets(77))).unwrap(),
                    CastOutcome::PassThrough(ClientOpcodeMessage::CMSG_CAST_SPELL(c)) if c.spell == spell
                ),
                "spell {spell} must reach the combat handler unchanged"
            );
        }

        let enchanting = InMemoryCasts {
            enchant: Some(EnchantRoute::Disenchant),
            ..InMemoryCasts::instant()
        };
        assert!(matches!(
            dispatch_cast(&enchanting, player(), cast(13262, unit_targets(0))).unwrap(),
            CastOutcome::PassThrough(_)
        ));

        assert!(store.casts.lock().unwrap().is_empty());
        assert!(enchanting.casts.lock().unwrap().is_empty());
    }

    #[test]
    fn unrelated_opcodes_pass_through_to_the_next_dispatcher() {
        let store = InMemoryCasts::default();

        assert!(matches!(
            dispatch_cast(
                &store,
                player(),
                ClientOpcodeMessage::CMSG_PING(CMSG_PING::default())
            )
            .unwrap(),
            CastOutcome::PassThrough(ClientOpcodeMessage::CMSG_PING(_))
        ));
    }
}
