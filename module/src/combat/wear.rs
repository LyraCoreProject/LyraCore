use spacetimedb::ReducerContext;

use crate::conformance_rolls::{draw, RollStream};
use crate::items::equip_slot;
use crate::WorldEntity;

// vmangos DurabilityLossChance.Damage, 0.5%, in basis points.
const DURABILITY_LOSS_CHANCE_DAMAGE_BP: u32 = 50;

// vmangos Unit::DealDamage: hit-taken wear, then hit-done wear, on any non-killing damage above 0.
pub(crate) fn wear_for_landed_damage(
    ctx: &ReducerContext,
    attacker: Option<&WorldEntity>,
    victim: &WorldEntity,
) {
    if victim.is_player() {
        roll_equipment_wear(ctx, victim.guid);
    }
    if let Some(attacker) = attacker.filter(|attacker| attacker.is_player()) {
        roll_equipment_wear(ctx, attacker.guid);
    }
}

fn roll_equipment_wear(ctx: &ReducerContext, character_guid: u64) {
    let stream = RollStream::Wear(character_guid);
    if draw(ctx, stream) % 10_000 >= DURABILITY_LOSS_CHANCE_DAMAGE_BP {
        return;
    }
    let slot = (draw(ctx, stream) % (u32::from(equip_slot::END) + 1)) as u8;
    if let Some(mut item) =
        crate::items::item_in_slot(ctx, character_guid, slot).filter(|item| item.durability > 0)
    {
        item.durability -= 1;
        item.update(ctx);
    }
}
