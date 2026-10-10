//! Cast-lifecycle seam: one entry point owns `CMSG_CAST_SPELL` route selection, client target
//! decoding, durable dispatch and the ordered client-visible messages.
//!
//! The module root holds the seam types, the route decision tree and the ordinary cast route
//! (instant, timed, next-swing, ground-area and ground-targeted casts). Ranged auto-repeat lives in
//! [`ranged`], the routes whose reducers emit no cast event — enchant, disenchant, fishing and lock
//! opening — in [`manual`], and both cancellations in [`cancel`].
//!
//! Deleting this module does not delete the decisions in it. Spell taxonomy routing, client target
//! decoding, durable-operation selection, ranged auto-repeat state, failure mapping and the
//! message-order rules would spread back across the broad combat handler and the store adapters,
//! which is where they lived before and why one cast change needed edits in several files.

mod cancel;
mod manual;
mod ranged;

use super::super::*;
use super::MeleeActionStore;
use crate::stdb::{classify, DurableFailure};
use wow_world_messages::vanilla::CMSG_CAST_SPELL;

/// `SMSG_CAST_RESULT`. Both bodies are hand-rolled (gtker's typed message inverts the status
/// semantics), so they ride `Outbound::Raw` under this opcode.
const OP_CAST_RESULT: u16 = 0x0130;

/// Imported spell taxonomy and the durable cast operations the module needs. Narrow on purpose:
/// nothing here exposes a coordinator read, a reducer handle or a spell table to the caller.
pub(crate) trait CastStore: MeleeActionStore + Send + Sync {
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
    fn cast_spell(&self, actor: Actor, spell_id: u32, target_guid: u64) -> Result<()>;

    /// Cast a ground-targeted spell at the exact point the client clicked.
    fn cast_spell_at(
        &self,
        actor: Actor,
        spell_id: u32,
        target_guid: u64,
        x: f32,
        y: f32,
        z: f32,
    ) -> Result<()>;

    /// Cast a spell whose explicit target is an owned inventory item. The module validates the
    /// effect kind and consumes the item only after the gameplay gates pass.
    fn cast_item_target(&self, actor: Actor, spell_id: u32, slot: u8) -> Result<()>;

    /// Arm the ranged auto-repeat loop on `target_guid` with `spell_id`. The module requires an
    /// equipped ranged weapon; `Err` is the refusal the player sees as a cast failure.
    fn start_ranged_attack(&self, actor: Actor, target_guid: u64, spell_id: u32) -> Result<()>;

    /// The bag slot holding the item instance a client spell-target names, so the enchant and
    /// disenchant operations receive a slot rather than a guid.
    fn item_slot_by_guid(&self, item_guid: u64) -> Option<u8>;

    /// Disenchant the item in `slot`. The module validates skill and disenchantability, and yields
    /// the resulting reagents into the bag.
    fn disenchant_item(&self, actor: Actor, slot: u8) -> Result<()>;

    /// Apply `enchant_id` to the item in `slot`. The module validates skill, consumes the reagent
    /// and stamps the enchant on the item instance.
    fn enchant_item_on_slot(&self, actor: Actor, slot: u8, enchant_id: u32) -> Result<()>;

    /// The instant-resolve Fishing catch.
    fn fish(&self, actor: Actor) -> Result<()>;

    /// Pick the lock on GameObject `go_guid`. The module gates range, the lock requirement and the
    /// caller's skill; `Err` is the refusal the player sees as a cast failure.
    fn pick_lock(&self, actor: Actor, go_guid: u64) -> Result<()>;

    /// Drop the caller's pending cast, so a scheduled completion cannot fire later. Under the
    /// one-pending-cast rule the caller identifies the cast, so the client's spell id is unused.
    fn cancel_cast(&self, actor: Actor) -> Result<()>;

    /// Remove the caller's own aura named by the wire spell id. The aura relay re-syncs the buff bar.
    fn cancel_aura(&self, actor: Actor, spell_id: u32) -> Result<()>;

    // The two reads below are shared with the character, vendor and query paths. They are declared
    // on this family only, because a second declaration of the same name on another family would
    // make every call through the `WorldStore` umbrella ambiguous. The
    // ranged teardown comes from `MeleeActionStore`: melee and ranged share one durable row, so it
    // has one declaration, on the seam that owns that row.

    /// Every item a character owns. The ranged route reads the equipped launcher and the projectile
    /// stacks from it.
    fn player_items(&self, owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>>;

    /// One item template by entry — the launcher's class/subclass and the projectile's display id.
    fn item_template(&self, entry: u32) -> Result<Option<codec::ItemTemplateView>>;
}

/// Everything the cast module knows about the caller. `self_guid` is `None` when the session has no
/// character in the world: no durable request is made and no synchronous message is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CastPlayer {
    pub(crate) account_id: u64,
    pub(crate) self_guid: Option<u64>,
    /// A ranged auto-repeat loop is armed. Only the ranged route reads it.
    pub(crate) ranged_repeat: bool,
}

impl CastPlayer {
    fn actor(self) -> Option<Actor> {
        self.self_guid.and_then(Actor::new)
    }
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
    /// route sends the completion sequence itself. The payload is the one classification decision,
    /// made once here instead of re-testing the same effect metadata inside the route.
    ManualCompletion(manual::ManualRoute),
    /// Every other cast: instant, timed, next-swing, ground-area and ground-targeted.
    Ordinary,
}

fn route_for<St: CastStore + ?Sized>(store: &St, spell_id: u32) -> CastRoute {
    if store.spell_is_ranged_auto_repeat(spell_id) {
        CastRoute::RangedAutoRepeat
    } else if let Some(route) = store.enchant_route(spell_id) {
        CastRoute::ManualCompletion(manual::ManualRoute::Enchant(route))
    } else if store.spell_is_fishing(spell_id) {
        CastRoute::ManualCompletion(manual::ManualRoute::Fish)
    } else if store.spell_is_open_lock(spell_id) {
        CastRoute::ManualCompletion(manual::ManualRoute::OpenLock)
    } else {
        CastRoute::Ordinary
    }
}

/// What the cast failure mapping reads when the session has no Actor to cast as.
const NO_ACTOR: &str = "no resolved actor";

/// The Module's reason for a Refusal, which the client sees as a cast failure. A Transport Loss
/// cannot serve any further request, so it ends the World Session.
fn refusal_reason(error: anyhow::Error) -> Result<String> {
    match classify(&error) {
        DurableFailure::Refusal { reason } => Ok(reason.to_string()),
        DurableFailure::TransportLoss => Err(error),
    }
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
            CastRoute::RangedAutoRepeat => ranged::activate(store, player, &c),
            CastRoute::ManualCompletion(route) => {
                manual::manual_completion_cast(store, player, &c, route)
            }
        },
        ClientOpcodeMessage::CMSG_CANCEL_AUTO_REPEAT_SPELL => ranged::cancel(store, player),
        ClientOpcodeMessage::CMSG_CANCEL_CAST(_) => cancel::cancel_cast(store, player),
        ClientOpcodeMessage::CMSG_CANCEL_AURA(c) => cancel::cancel_aura(store, player, c.id),
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
        if let Some(caster) = player.actor().map(Actor::guid) {
            // vmangos order: START(0) then the raw CAST_RESULT(OK) then GO. The 5875 client needs
            // that 5-byte ack before GO to make m_currentSpells clearable.
            // A cast at a clicked ground point echoes the point in both target blocks.
            let dest = dest_target(c);
            let start = match dest {
                Some(dest) => codec::build_spell_start_dest(caster, spell, dest),
                None => codec::build_spell_start(caster, spell, 0, 0, None),
            };
            outbound.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
                Box::new(start),
            )));
            outbound.push(Outbound::Raw {
                opcode: OP_CAST_RESULT,
                body: codec::build_cast_result_ok(spell),
            });
            // Any cast at a clicked ground point and a caster-anchored ground area (Consecration)
            // impact the ground: an EMPTY hit list, or the self-cast fallback puts the caster in
            // `hits[]` and the client plays the impact animation on the caster.
            let go = if let Some(dest) = dest {
                codec::build_spell_go_dest(caster, spell, dest)
            } else if store.spell_is_ground_area(spell) {
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
    let item_guid = match c.targets.target_flags.get_item() {
        Some(wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags_Item::Item {
            item,
        }) => item.guid(),
        _ => 0,
    };
    let refusal = 'request: {
        let Some(actor) = player.actor() else {
            break 'request Some(NO_ACTOR.to_string());
        };
        let result = if item_guid != 0 {
            let Some(slot) = store.item_slot_by_guid(item_guid) else {
                break 'request Some(format!(
                    "item target {item_guid} is not in the player's bag"
                ));
            };
            store.cast_item_target(actor, spell, slot)
        } else {
            match dest_target(c) {
                Some((x, y, z)) => store.cast_spell_at(actor, spell, target, x, y, z),
                None => store.cast_spell(actor, spell, target),
            }
        };
        match result {
            Ok(()) => None,
            Err(e) => Some(refusal_reason(e)?),
        }
    };
    if let Some(reason) = refusal {
        // Carry the mapped REASON so the client prints the red error line ("Not enough rage",
        // "You must be behind your target"). A bare failure only resets the button and leaves
        // server-only gates — behind, stealth, stance, react window — invisible.
        log::debug!(
            "world: cast {spell} rejected (account {}): {reason}",
            player.account_id
        );
        outbound.push(Outbound::Raw {
            opcode: OP_CAST_RESULT,
            body: codec::build_cast_result_failed(spell, codec::cast_failure_reason_for(&reason)),
        });
    }
    Ok(CastOutcome::Handled {
        transition: CastTransition::default(),
        outbound,
    })
}

/// The focused in-memory cast adapter and the shared seam-test helpers. Every route module's tests
/// use this one fake, so `pub(super)` reaches the sibling route modules.
#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::stdb::ReducerCallError;
    use std::sync::Mutex;
    use wow_world_messages::vanilla::{
        Guid, SpellCastTargets, SpellCastTargets_SpellCastTargetFlags,
        SpellCastTargets_SpellCastTargetFlags_DestLocation,
        SpellCastTargets_SpellCastTargetFlags_Gameobject,
        SpellCastTargets_SpellCastTargetFlags_Item, SpellCastTargets_SpellCastTargetFlags_Unit,
        Vector3d, CMSG_PING,
    };

    /// One recorded durable cast: caster, spell and unit target.
    pub(crate) type Cast = (u64, u32, u64);
    /// One recorded durable ground cast: a [`Cast`] plus the click point.
    pub(crate) type GroundCast = (u64, u32, u64, f32, f32, f32);
    /// One recorded ranged activation: caster, unit target and spell.
    pub(crate) type RangedAttack = (u64, u64, u32);
    /// One recorded `enchant_item_on_slot` call: slot and enchant id.
    pub(crate) type EnchantCall = (u8, u32);
    /// One recorded `pick_lock` call: caster and GameObject guid.
    pub(crate) type PickLockCall = (u64, u64);
    /// One recorded `cancel_aura` call: caster and wire spell id.
    pub(crate) type CancelAuraCall = (u64, u32);

    /// Spell-route metadata, item state and durable results for the cast routes, plus a record of
    /// every durable call. Nothing else: no vendors, parties, mail, transfer or unrelated world
    /// state.
    #[derive(Default)]
    pub(crate) struct InMemoryCasts {
        pub(crate) cast_time_ms: Option<u32>,
        pub(crate) queues_next_swing: bool,
        pub(crate) ground_area: bool,
        pub(crate) ranged_auto_repeat: Vec<u32>,
        pub(crate) enchant: Option<EnchantRoute>,
        pub(crate) fishing: Vec<u32>,
        pub(crate) open_lock: Vec<u32>,
        /// The Module's reason when the ordinary cast operations refuse.
        pub(crate) cast_error: Option<String>,
        /// When set, `start_ranged_attack` refuses with this reason.
        pub(crate) ranged_error: Option<String>,
        /// Every durable operation fails with a Transport Loss.
        pub(crate) transport_lost: bool,
        /// The caller's owned items, and the templates their entries resolve to.
        pub(crate) items: Vec<codec::ItemInstanceView>,
        pub(crate) templates: Vec<codec::ItemTemplateView>,
        /// Item-instance guid → bag slot, for the manual-completion item target.
        pub(crate) item_slots: Vec<(u64, u8)>,
        /// Shared refusal for every manual-completion durable operation.
        pub(crate) manual_error: Option<String>,
        /// Shared refusal for both cancellation operations.
        pub(crate) cancel_error: Option<String>,
        pub(crate) casts: Mutex<Vec<Cast>>,
        pub(crate) ground_casts: Mutex<Vec<GroundCast>>,
        pub(crate) item_target_casts: Mutex<Vec<(u64, u32, u8)>>,
        pub(crate) ranged_attacks: Mutex<Vec<RangedAttack>>,
        pub(crate) stop_attacks: Mutex<Vec<u64>>,
        pub(crate) disenchant_calls: Mutex<Vec<u8>>,
        pub(crate) enchant_calls: Mutex<Vec<EnchantCall>>,
        pub(crate) fish_calls: Mutex<Vec<u64>>,
        pub(crate) pick_lock_calls: Mutex<Vec<PickLockCall>>,
        pub(crate) cancel_cast_calls: Mutex<Vec<u64>>,
        pub(crate) cancel_aura_calls: Mutex<Vec<CancelAuraCall>>,
        /// The attackers with a live engagement: an armed ranged loop adds its caster, a stop
        /// removes it.
        pub(crate) engaged: Mutex<Vec<u64>>,
        /// Spell ids of the auras on the caster; a cancellation removes the one it names.
        pub(crate) auras: Mutex<Vec<u32>>,
        /// Casters with a pending cast; a cancellation removes the caller's.
        pub(crate) pending_casts: Mutex<Vec<u64>>,
    }

    impl InMemoryCasts {
        pub(crate) fn refusing(error: &str) -> Self {
            Self {
                cast_error: Some(error.into()),
                ..Default::default()
            }
        }

        pub(crate) fn instant() -> Self {
            Self {
                cast_time_ms: Some(0),
                ..Default::default()
            }
        }

        /// The Module's answer to a durable request: a Refusal with `reason`, or success.
        fn answer(&self, operation: &str, reason: &Option<String>) -> Result<()> {
            if self.transport_lost {
                return Err(ReducerCallError::transport_lost(operation).into());
            }
            reason.as_ref().map_or(Ok(()), |reason| {
                Err(ReducerCallError::refused(operation, reason).into())
            })
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

        fn cast_spell(&self, actor: Actor, spell_id: u32, target_guid: u64) -> Result<()> {
            self.casts
                .lock()
                .unwrap()
                .push((actor.guid(), spell_id, target_guid));
            self.answer("gw_cast_spell", &self.cast_error)
        }

        fn cast_spell_at(
            &self,
            actor: Actor,
            spell_id: u32,
            target_guid: u64,
            x: f32,
            y: f32,
            z: f32,
        ) -> Result<()> {
            self.ground_casts
                .lock()
                .unwrap()
                .push((actor.guid(), spell_id, target_guid, x, y, z));
            self.answer("gw_cast_spell_at", &self.cast_error)
        }

        fn cast_item_target(&self, actor: Actor, spell_id: u32, slot: u8) -> Result<()> {
            self.item_target_casts
                .lock()
                .unwrap()
                .push((actor.guid(), spell_id, slot));
            self.answer("gw_cast_item_target", &self.cast_error)
        }

        fn item_slot_by_guid(&self, item_guid: u64) -> Option<u8> {
            self.item_slots
                .iter()
                .find(|(g, _)| *g == item_guid)
                .map(|&(_, s)| s)
        }

        fn disenchant_item(&self, _actor: Actor, slot: u8) -> Result<()> {
            self.disenchant_calls.lock().unwrap().push(slot);
            self.answer("gw_disenchant", &self.manual_error)
        }

        fn enchant_item_on_slot(&self, _actor: Actor, slot: u8, enchant_id: u32) -> Result<()> {
            self.enchant_calls.lock().unwrap().push((slot, enchant_id));
            self.answer("gw_enchant_item", &self.manual_error)
        }

        fn fish(&self, actor: Actor) -> Result<()> {
            self.fish_calls.lock().unwrap().push(actor.guid());
            self.answer("gw_fish", &self.manual_error)
        }

        fn pick_lock(&self, actor: Actor, go_guid: u64) -> Result<()> {
            self.pick_lock_calls
                .lock()
                .unwrap()
                .push((actor.guid(), go_guid));
            self.answer("gw_pick_lock", &self.manual_error)
        }

        fn cancel_cast(&self, actor: Actor) -> Result<()> {
            self.cancel_cast_calls.lock().unwrap().push(actor.guid());
            self.answer("gw_cancel_cast", &self.cancel_error)?;
            self.pending_casts
                .lock()
                .unwrap()
                .retain(|&caster| caster != actor.guid());
            Ok(())
        }

        fn cancel_aura(&self, actor: Actor, spell_id: u32) -> Result<()> {
            self.cancel_aura_calls
                .lock()
                .unwrap()
                .push((actor.guid(), spell_id));
            self.answer("gw_cancel_aura", &self.cancel_error)?;
            self.auras.lock().unwrap().retain(|&aura| aura != spell_id);
            Ok(())
        }

        fn start_ranged_attack(&self, actor: Actor, target_guid: u64, spell_id: u32) -> Result<()> {
            self.answer("gw_ranged_attack", &self.ranged_error)?;
            self.ranged_attacks
                .lock()
                .unwrap()
                .push((actor.guid(), target_guid, spell_id));
            let mut engaged = self.engaged.lock().unwrap();
            if !engaged.contains(&actor.guid()) {
                engaged.push(actor.guid());
            }
            Ok(())
        }

        fn player_items(&self, _owner_guid: u64) -> Result<Vec<codec::ItemInstanceView>> {
            Ok(self.items.clone())
        }

        fn item_template(&self, entry: u32) -> Result<Option<codec::ItemTemplateView>> {
            Ok(self.templates.iter().find(|t| t.entry == entry).cloned())
        }
    }

    /// The ranged teardown the cast module shares with the melee seam. `start_attack` is never
    /// reached from a cast route; it exists because the two share one durable engagement row.
    impl MeleeActionStore for InMemoryCasts {
        fn start_attack(&self, _actor: Actor, _target_guid: u64) -> Result<()> {
            unreachable!("no cast route arms a melee engagement")
        }

        fn stop_attack(&self, actor: Actor) -> Result<()> {
            self.answer("gw_stop_attack", &None)?;
            self.stop_attacks.lock().unwrap().push(actor.guid());
            self.engaged
                .lock()
                .unwrap()
                .retain(|&guid| guid != actor.guid());
            Ok(())
        }
    }

    pub(crate) const ACCOUNT: u64 = 7;
    pub(crate) const CASTER: u64 = 42;

    pub(crate) fn player() -> CastPlayer {
        CastPlayer {
            account_id: ACCOUNT,
            self_guid: Some(CASTER),
            ranged_repeat: false,
        }
    }

    pub(crate) fn cast(spell: u32, targets: SpellCastTargets) -> ClientOpcodeMessage {
        ClientOpcodeMessage::CMSG_CAST_SPELL(Box::new(CMSG_CAST_SPELL { spell, targets }))
    }

    pub(crate) fn unit_targets(guid: u64) -> SpellCastTargets {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
                SpellCastTargets_SpellCastTargetFlags_Unit {
                    unit_target: Guid::new(guid),
                },
            ),
        }
    }

    pub(crate) fn dest_targets(x: f32, y: f32, z: f32) -> SpellCastTargets {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_dest_location(
                SpellCastTargets_SpellCastTargetFlags_DestLocation {
                    destination: Vector3d { x, y, z },
                },
            ),
        }
    }

    fn item_targets(item_guid: u64) -> SpellCastTargets {
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_item(
                SpellCastTargets_SpellCastTargetFlags_Item::Item {
                    item: Guid::new(item_guid),
                },
            ),
        }
    }

    /// The GAMEOBJECT target shape, or its ObjectUnk sibling — the two the vanilla client sends for
    /// Pick Lock.
    fn gameobject_targets(go_guid: u64, unk_shape: bool) -> SpellCastTargets {
        let target = if unk_shape {
            SpellCastTargets_SpellCastTargetFlags_Gameobject::ObjectUnk {
                object_unk: Guid::new(go_guid),
            }
        } else {
            SpellCastTargets_SpellCastTargetFlags_Gameobject::Gameobject {
                gameobject: Guid::new(go_guid),
            }
        };
        SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_gameobject(target),
        }
    }

    /// The handled outcome, or a panic naming what came back instead.
    pub(crate) fn handled(outcome: CastOutcome) -> (CastTransition, Vec<Outbound>) {
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
    pub(crate) fn sequence(outbound: &[Outbound]) -> Vec<String> {
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
                Outbound::One(ServerOpcodeMessage::SMSG_CAST_RESULT(r))
                    if matches!(r.result, SMSG_CAST_RESULT_SimpleSpellCastResult::Failure) =>
                {
                    "CAST_RESULT(FAILURE)".to_string()
                }
                _ => "UNEXPECTED".to_string(),
            })
            .collect()
    }

    /// The single `SMSG_SPELL_GO` in the batch.
    pub(crate) fn spell_go(outbound: &[Outbound]) -> &wow_world_messages::vanilla::SMSG_SPELL_GO {
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
        assert_eq!(store.casts.lock().unwrap().as_slice(), &[(CASTER, 100, 77)]);
    }

    #[test]
    fn instant_cast_with_no_unit_target_passes_target_zero_to_the_durable_cast() {
        let store = InMemoryCasts::instant();

        handled(dispatch_cast(&store, player(), cast(100, SpellCastTargets::default())).unwrap());

        assert_eq!(
            store.casts.lock().unwrap().as_slice(),
            &[(CASTER, 100, 0)],
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

    #[test]
    fn instant_destination_cast_echoes_the_clicked_point_and_lists_no_hits() {
        let store = InMemoryCasts::instant();

        let (_, outbound) = handled(
            dispatch_cast(
                &store,
                player(),
                cast(1725, dest_targets(-8913.5, 554.25, 93.75)),
            )
            .unwrap(),
        );

        assert_eq!(sequence(&outbound), ["START", "CAST_RESULT(OK)", "GO"]);
        let Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(start)) = &outbound[0] else {
            panic!("the sequence opens with START");
        };
        let start_point = start
            .targets
            .target_flags
            .get_dest_location()
            .expect("the START echoes the destination")
            .destination;
        assert_eq!(
            (start_point.x, start_point.y, start_point.z),
            (-8913.5, 554.25, 93.75)
        );
        let go = spell_go(&outbound);
        assert!(
            go.hits.is_empty(),
            "a ground-targeted cast impacts the ground, not the caster"
        );
        let point = go
            .targets
            .target_flags
            .get_dest_location()
            .expect("the GO echoes the destination")
            .destination;
        assert_eq!((point.x, point.y, point.z), (-8913.5, 554.25, 93.75));
        assert_eq!(
            store.ground_casts.lock().unwrap().as_slice(),
            &[(CASTER, 1725, 0, -8913.5, 554.25, 93.75)]
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
        assert_eq!(store.casts.lock().unwrap().as_slice(), &[(CASTER, 100, 77)]);
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
        assert_eq!(store.casts.lock().unwrap().as_slice(), &[(CASTER, 78, 77)]);
    }

    // ── Ground targeting ─────────────────────────────────────────────────────

    #[test]
    fn destination_target_cast_requests_the_durable_ground_cast_with_the_client_coordinates() {
        let store = InMemoryCasts {
            cast_time_ms: Some(2000),
            ..Default::default()
        };

        handled(
            dispatch_cast(
                &store,
                player(),
                cast(2120, dest_targets(-8913.5, 554.25, 93.75)),
            )
            .unwrap(),
        );

        assert_eq!(
            store.ground_casts.lock().unwrap().as_slice(),
            &[(CASTER, 2120, 0, -8913.5, 554.25, 93.75)]
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
            ["START", "CAST_RESULT(OK)", "GO", "CAST_RESULT(FAILED 0x4d)"],
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
            transport_lost: true,
            ..Default::default()
        };

        let error = match dispatch_cast(&store, player(), cast(100, unit_targets(77))) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };

        assert!(matches!(classify(&error), DurableFailure::TransportLoss));
    }

    // ── Player context ───────────────────────────────────────────────────────

    #[test]
    fn a_player_with_no_character_in_world_makes_no_request_and_gets_a_failed_cast() {
        let store = InMemoryCasts::instant();
        let player = CastPlayer {
            account_id: ACCOUNT,
            self_guid: None,
            ranged_repeat: false,
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player, cast(100, unit_targets(77))).unwrap());

        assert_eq!(sequence(&outbound), ["CAST_RESULT(FAILED 0x17)"]);
        assert!(store.casts.lock().unwrap().is_empty());
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

    // ── Manual completion: enchant, disenchant, fishing, lock opening ────────

    #[test]
    fn enchant_cast_resolves_the_item_guid_to_its_slot_and_completes_manually() {
        let store = InMemoryCasts {
            enchant: Some(EnchantRoute::Enchant(777)),
            item_slots: vec![(500, 4)],
            ..Default::default()
        };

        let (transition, outbound) =
            handled(dispatch_cast(&store, player(), cast(7418, item_targets(500))).unwrap());

        assert_eq!(sequence(&outbound), ["START", "CAST_RESULT(OK)", "GO"]);
        assert_eq!(transition, CastTransition::default());
        assert_eq!(store.enchant_calls.lock().unwrap().as_slice(), &[(4, 777)]);
        assert!(store.disenchant_calls.lock().unwrap().is_empty());
        assert!(
            store.casts.lock().unwrap().is_empty(),
            "an enchant cast never reaches cast_spell"
        );
    }

    #[test]
    fn disenchant_cast_resolves_the_item_guid_to_its_slot_and_completes_manually() {
        let store = InMemoryCasts {
            enchant: Some(EnchantRoute::Disenchant),
            item_slots: vec![(500, 9)],
            ..Default::default()
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(13262, item_targets(500))).unwrap());

        assert_eq!(sequence(&outbound), ["START", "CAST_RESULT(OK)", "GO"]);
        assert_eq!(store.disenchant_calls.lock().unwrap().as_slice(), &[9]);
        assert!(store.enchant_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn enchant_without_an_owned_item_target_fails_with_no_durable_operation() {
        let store = InMemoryCasts {
            enchant: Some(EnchantRoute::Enchant(777)),
            ..Default::default()
        };

        let (_, outbound) = handled(
            dispatch_cast(&store, player(), cast(7418, SpellCastTargets::default())).unwrap(),
        );

        assert_eq!(
            sequence(&outbound),
            ["CAST_RESULT(FAILURE)"],
            "no item target means no success sequence"
        );
        assert!(store.enchant_calls.lock().unwrap().is_empty());
        assert!(store.disenchant_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn disenchant_with_an_item_guid_not_in_the_bag_fails_with_no_durable_operation() {
        let store = InMemoryCasts {
            enchant: Some(EnchantRoute::Disenchant),
            ..Default::default()
        };

        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(13262, item_targets(500))).unwrap());

        assert_eq!(sequence(&outbound), ["CAST_RESULT(FAILURE)"]);
        assert!(store.disenchant_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn fishing_cast_requests_the_fish_reducer_with_no_target_and_completes_manually() {
        let store = InMemoryCasts {
            fishing: vec![7620],
            ..Default::default()
        };

        let (_, outbound) = handled(
            dispatch_cast(&store, player(), cast(7620, SpellCastTargets::default())).unwrap(),
        );

        assert_eq!(sequence(&outbound), ["START", "CAST_RESULT(OK)", "GO"]);
        assert_eq!(store.fish_calls.lock().unwrap().as_slice(), &[CASTER]);
    }

    #[test]
    fn fishing_refusal_sends_only_the_simple_failure_and_stays_non_fatal() {
        let store = InMemoryCasts {
            fishing: vec![7620],
            manual_error: Some("fishing skill too low".into()),
            ..Default::default()
        };

        let (_, outbound) = handled(
            dispatch_cast(&store, player(), cast(7620, SpellCastTargets::default())).unwrap(),
        );

        assert_eq!(sequence(&outbound), ["CAST_RESULT(FAILURE)"]);
    }

    #[test]
    fn a_dead_reducer_transport_on_a_manual_route_is_session_fatal() {
        let store = InMemoryCasts {
            fishing: vec![7620],
            transport_lost: true,
            ..Default::default()
        };

        let error = match dispatch_cast(&store, player(), cast(7620, SpellCastTargets::default())) {
            Err(error) => error,
            Ok(_) => panic!("a dead reducer transport must end the session"),
        };
        assert!(matches!(classify(&error), DurableFailure::TransportLoss));
    }

    #[test]
    fn pick_lock_cast_decodes_either_gameobject_target_shape_and_completes_manually() {
        for unk_shape in [false, true] {
            let store = InMemoryCasts {
                open_lock: vec![1804],
                ..Default::default()
            };

            let (_, outbound) = handled(
                dispatch_cast(
                    &store,
                    player(),
                    cast(1804, gameobject_targets(0xABCD, unk_shape)),
                )
                .unwrap(),
            );

            assert_eq!(sequence(&outbound), ["START", "CAST_RESULT(OK)", "GO"]);
            assert_eq!(
                store.pick_lock_calls.lock().unwrap().as_slice(),
                &[(CASTER, 0xABCD)],
                "unk_shape={unk_shape}"
            );
        }
    }

    #[test]
    fn pick_lock_without_a_gameobject_target_fails_with_no_durable_operation() {
        let store = InMemoryCasts {
            open_lock: vec![1804],
            ..Default::default()
        };

        let (_, outbound) = handled(
            dispatch_cast(&store, player(), cast(1804, SpellCastTargets::default())).unwrap(),
        );

        assert_eq!(sequence(&outbound), ["CAST_RESULT(FAILURE)"]);
        assert!(store.pick_lock_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn item_target_cast_resolves_owned_guid_to_slot() {
        let store = InMemoryCasts {
            cast_time_ms: Some(0),
            item_slots: vec![(500, 7)],
            ..Default::default()
        };
        let (_, outbound) =
            handled(dispatch_cast(&store, player(), cast(6991, item_targets(500))).unwrap());
        assert_eq!(sequence(&outbound), ["START", "CAST_RESULT(OK)", "GO"]);
        assert_eq!(
            *store.item_target_casts.lock().unwrap(),
            [(CASTER, 6991, 7)]
        );
        assert!(store.casts.lock().unwrap().is_empty());
    }
}
