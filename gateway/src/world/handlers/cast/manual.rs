//! Manual-completion routes: enchant, disenchant, fishing and lock opening. Their durable
//! operations emit no cast event, so this module sends the client's completion sequence itself.
//! The four routes share one shape — decode a target, request a durable operation, then either the
//! shared success sequence or the shared simple failure — and differ only in which target they
//! decode and which operation they call.

use super::*;
use wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags_Gameobject as GoTarget;
use wow_world_messages::vanilla::SpellCastTargets_SpellCastTargetFlags_Item as ItemTarget;

/// Which manual-completion operation a cast requests, decided once in `route_for` from the spell's
/// effect metadata.
pub(crate) enum ManualRoute {
    /// Enchant or disenchant an item; the target is an item-instance GUID.
    Enchant(EnchantRoute),
    /// Fishing; needs no target.
    Fish,
    /// Pick Lock; the target is a GameObject GUID.
    OpenLock,
}

/// The item-instance GUID off an ITEM-target cast, or 0 if the cast carries none.
fn item_target(c: &CMSG_CAST_SPELL) -> u64 {
    match c.targets.target_flags.get_item() {
        Some(ItemTarget::Item { item }) => item.guid(),
        _ => 0,
    }
}

/// The GameObject GUID off a cast, accepting both target-flag shapes the vanilla client sends for
/// Pick Lock, or 0 if the cast carries neither.
fn gameobject_target(c: &CMSG_CAST_SPELL) -> u64 {
    match c.targets.target_flags.get_gameobject() {
        Some(GoTarget::Gameobject { gameobject }) => gameobject.guid(),
        Some(GoTarget::ObjectUnk { object_unk }) => object_unk.guid(),
        None => 0,
    }
}

/// Enchant, disenchant, fishing and lock opening: decode the route's target, request the durable
/// operation, then send the shared completion sequence on success or the shared simple failure on
/// refusal. None of these reducers emit a cast event, so the gateway is the only sender either way.
pub(super) fn manual_completion_cast<St: CastStore + ?Sized>(
    store: &St,
    session: &mut ProtocolSession,
    c: &CMSG_CAST_SPELL,
    route: ManualRoute,
) -> Result<ProtocolReply> {
    let spell = c.spell;
    // `Err` carries the reason the request ended in a Refusal: the Module's, or the Gateway's own
    // when the cast names no usable target.
    let completed: std::result::Result<Actor, String> = 'request: {
        let Some(actor) = session.actor() else {
            break 'request Err(NO_ACTOR.to_string());
        };
        let result = match route {
            ManualRoute::Enchant(enchant_route) => {
                let item_guid = item_target(c);
                if item_guid == 0 {
                    break 'request Err("enchant: no item target in cast".to_string());
                }
                let Some(slot) = store.item_slot_by_guid(item_guid) else {
                    break 'request Err(format!(
                        "enchant: item {item_guid} not in the Character inventory"
                    ));
                };
                match enchant_route {
                    EnchantRoute::Disenchant => store.disenchant_item(actor, slot),
                    EnchantRoute::Enchant(enchant_id) => {
                        store.enchant_item_on_slot(actor, slot, enchant_id)
                    }
                }
            }
            ManualRoute::Fish => store.fish(actor),
            ManualRoute::OpenLock => {
                let go_guid = gameobject_target(c);
                if go_guid == 0 {
                    break 'request Err("pick_lock: no gameobject target in cast".to_string());
                }
                store.pick_lock(actor, go_guid)
            }
        };
        match result {
            Ok(()) => Ok(actor),
            Err(e) => Err(refusal_reason(e)?),
        }
    };

    let outbound = match completed {
        Err(reason) => {
            log::debug!(
                "world: manual-completion cast {spell} rejected (account {}): {reason}",
                session.account_id
            );
            vec![Outbound::One(ServerOpcodeMessage::SMSG_CAST_RESULT(
                Box::new(SMSG_CAST_RESULT {
                    spell,
                    result: SMSG_CAST_RESULT_SimpleSpellCastResult::Failure,
                }),
            ))]
        }
        Ok(actor) => vec![
            Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(Box::new(
                codec::build_spell_start(actor.guid(), spell, 0, 0, None),
            ))),
            Outbound::Raw {
                opcode: OP_CAST_RESULT,
                body: codec::build_cast_result_ok(spell),
            },
            Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
                codec::build_spell_go(actor.guid(), spell, 0, None),
            ))),
        ],
    };
    Ok(outbound.into())
}
