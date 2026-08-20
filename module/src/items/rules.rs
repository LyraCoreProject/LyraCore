//! Pure equip/economy taxonomy + arithmetic — the ctx-free item rules. Equip-slot/invtype vocabulary,
//! the type→slot resolver and equip predicates, the level gate, the vendor/merge arithmetic, and the
//! `EquipStat` projection. All unit-testable on plain values without a live module; the pure
//! taxonomy/arithmetic tests live at the bottom of this file.
//!
//! `consumable_heal` — the flat-plus-item-level HP formula that stood in for a real on-use effect id —
//! is RETIRED (#387): every on-use item now carries a real `spellid_1` cast through `begin_cast`
//! (`items::ops::apply_item_use`), so the magnitude lives in `game_spell_effect.base_points` like any
//! other spell, not in a second, item-only formula.

use super::tables::ItemTemplate;

/// Whether a Classic class/race eligibility mask names `id`. IDs are one-based in the source
/// format, so class/race 1 is bit 0. A zero durable mask is malformed/restrictive and fails
/// closed; only the importer may normalize the source unrestricted sentinel.
pub(crate) fn eligibility_mask_allows(mask: u32, id: u8) -> bool {
    id != 0 && (id as u32) <= u32::BITS && mask & (1u32 << (id - 1)) != 0
}

/// Whether a character's optional skill row meets an item's requirement. A zero skill id has no
/// gate; every non-zero id needs a durable row at or above the required rank.
pub(crate) fn meets_required_skill(
    required_skill: u32,
    required_skill_rank: u32,
    current_skill: Option<u16>,
) -> bool {
    required_skill == 0
        || current_skill.is_some_and(|current| u32::from(current) >= required_skill_rank)
}

/// Whether a character's standing meets an item's reputation requirement. Missing durable
/// reputation rows use the Module's established Neutral (zero-standing) semantics.
pub(crate) fn meets_required_reputation(
    required_faction: u32,
    required_rank: u32,
    standing: Option<i32>,
) -> bool {
    required_faction == 0
        || u32::from(crate::reputation::reputation_rank(standing.unwrap_or(0))) >= required_rank
}

/// Gold a vendor pays for a stack: the per-unit `sell_price` times the stack count (cmangos sells a
/// whole stack at once). Pure — unit-tested. Saturating so a pathological count never wraps the copper
/// total (the credit itself is also saturating on the player's money). A 0 `sell_price` is the
/// "no vendor value" sentinel handled by the caller, not here.
pub fn sell_value(unit_price: u32, count: u32) -> u32 {
    unit_price.saturating_mul(count)
}

/// Copper a vendor charges for a buy: the per-unit `buy_price` times the count (cmangos charges
/// `buy_price * count`). Pure — unit-tested. Saturating so a pathological count never wraps the copper
/// total; the affordability check then compares against the player's money. A 0 `buy_price` is the
/// "no vendor value" sentinel rejected by the caller (an item the vendor never sells), not here.
pub fn buy_cost(unit_price: u32, count: u32) -> u32 {
    unit_price.saturating_mul(count)
}

/// Copper to repair ONE item from `durability` back to `max_durability`: 1 copper per restored point.
/// Pure — unit-tested. A simple, correct proxy for vanilla's DBC DurabilityCost table (which we don't
/// import): monotonic in damage, zero when undamaged, saturating. `repair_all` sums this over the body.
pub fn repair_cost(max_durability: u32, durability: u32) -> u32 {
    max_durability.saturating_sub(durability)
}

/// How many units move from a source stack onto a destination stack of the SAME item: as many as the
/// source has, but never more than the destination's remaining headroom (`max_stack - dst`). Pure —
/// unit-tested. `saturating_sub` keeps an already-full (or over-full) destination at 0 headroom rather
/// than wrapping. The merge caller uses this for the move-onto-same-stackable case (FEATURE B). [entity]
pub fn merge_amount(src: u32, dst: u32, max_stack: u32) -> u32 {
    src.min(max_stack.saturating_sub(dst))
}

/// Whether an item may be equipped in the MAIN-HAND slot (15). Pure — unit-tested. Gates only the
/// main-hand: the item must be a Weapon (`class == 2`) whose `inventory_type` is a main-hand-capable
/// equip type — 13 = INVTYPE_WEAPON (one-hand), 17 = INVTYPE_2HWEAPON, 21 = INVTYPE_WEAPONMAINHAND.
/// Anything else (food, armor, an off-hand-only weapon type) is rejected, so you can't equip e.g.
/// Tough Jerky in the weapon slot. The other 18 equipment slots are validated by `can_equip_into` /
/// `inventory_type_to_slot`, which this function backs for main-hand specifically. [reference]
pub fn can_equip_mainhand(class: u8, inventory_type: u8) -> bool {
    class == 2 && matches!(inventory_type, 13 | 17 | 21)
}

/// The cmangos `item_template.bonding` values — item binding taxonomy. Only NONE, BIND_ON_PICKUP
/// (BoP) and BIND_ON_EQUIP (BoE) drive a binding trigger today; BIND_ON_USE and the two quest-item
/// codes are recognized but not yet wired to any binding trigger in the pipeline. [reference]
pub mod bonding {
    pub const NONE: u8 = 0;
    pub const BIND_ON_PICKUP: u8 = 1; // BoP — binds the instant it's granted (quest reward, loot, starter kit, vendor buy)
    pub const BIND_ON_EQUIP: u8 = 2; // BoE — binds the first time it's equipped
    pub const BIND_ON_USE: u8 = 3;
    pub const QUEST_ITEM: u8 = 4;
    pub const QUEST_ITEM2: u8 = 5;
}

/// Does a template with this `bonding` value bind THE INSTANT it's granted (any `store_item`/
/// `grant_starter_item` insert — quest reward, loot, starter kit, vendor buy, buyback)? Pure —
/// unit-tested. Only BIND_ON_PICKUP (BoP) does; everything else (unbound, BoE, BoU, quest items)
/// does not bind at grant time. [reference]
pub fn binds_on_grant(item_bonding: u8) -> bool {
    item_bonding == bonding::BIND_ON_PICKUP
}

/// Does a template with this `bonding` value bind the FIRST time it's equipped? Pure — unit-tested.
/// Only BIND_ON_EQUIP (BoE) does; a BoP item is already bound by `binds_on_grant` before it can ever
/// be equipped, and every other value never binds on equip in this slice. [reference]
pub fn binds_on_equip(item_bonding: u8) -> bool {
    item_bonding == bonding::BIND_ON_EQUIP
}

/// The 19 vanilla `EQUIPMENT_SLOT_*` ordinals (0..=18) — the equipment region of the player inventory
/// that renders gear on the 3D model (`PLAYER_VISIBLE_ITEM[slot]`). The main-hand value matches
/// `starter_item::MAINHAND_SLOT` (15) — kept in lockstep with the constants crate. The "pair" slots
/// (fingers 10/11, trinkets 12/13) are the FIRST of their pair; the equip path lands in the first free
/// one of the pair (the second is `+1`). cmangos `EquipmentSlots`. [reference]
pub mod equip_slot {
    pub const HEAD: u8 = 0;
    pub const NECK: u8 = 1;
    pub const SHOULDERS: u8 = 2;
    pub const SHIRT: u8 = 3;
    pub const CHEST: u8 = 4;
    pub const WAIST: u8 = 5;
    pub const LEGS: u8 = 6;
    pub const FEET: u8 = 7;
    pub const WRISTS: u8 = 8;
    pub const HANDS: u8 = 9;
    pub const FINGER1: u8 = 10; // FINGER2 == 11 (the pair's second slot)
    pub const TRINKET1: u8 = 12; // TRINKET2 == 13 (the pair's second slot)
    pub const BACK: u8 = 14;
    pub const MAINHAND: u8 = 15;
    pub const OFFHAND: u8 = 16;
    pub const RANGED: u8 = 17;
    pub const TABARD: u8 = 18;
    /// Inclusive upper bound of the equipment region (TABARD). Slots 0..=18 are equipment; 19..=22 are
    /// equipped-bag slots; 23.. is the backpack — only 0..=18 are model-visible / validated as equip.
    pub const END: u8 = TABARD;
}

/// The vanilla `INVTYPE_*` codes (`item_template.inventory_type`) this slice maps to equipment slots.
/// A small named subset of the full enum — the ones representing wearable/wieldable gear. [reference]
pub mod invtype {
    pub const NON_EQUIP: u8 = 0;
    pub const HEAD: u8 = 1;
    pub const NECK: u8 = 2;
    pub const SHOULDERS: u8 = 3;
    pub const BODY: u8 = 4; // shirt
    pub const CHEST: u8 = 5;
    pub const WAIST: u8 = 6;
    pub const LEGS: u8 = 7;
    pub const FEET: u8 = 8;
    pub const WRISTS: u8 = 9;
    pub const HANDS: u8 = 10;
    pub const FINGER: u8 = 11;
    pub const TRINKET: u8 = 12;
    pub const WEAPON: u8 = 13; // one-hand (either hand)
    pub const SHIELD: u8 = 14;
    pub const RANGED: u8 = 15; // bow
    pub const CLOAK: u8 = 16; // back
    pub const TWO_HAND_WEAPON: u8 = 17;
    /// `INVTYPE_BAG` (18) — a container/bag item. Equips into bag slots 19..=22 (NOT equipment 0..=18).
    /// `inventory_type_to_slot` returns `None` for this type (bags are NOT equipment); the equip path
    /// routes bag items to `first_free_bag_equip_slot` instead of `resolve_equip_slot`. [reference]
    pub const BAG: u8 = 18;
    pub const TABARD: u8 = 19;
    pub const ROBE: u8 = 20; // chest (cloth robe)
    pub const WEAPON_MAINHAND: u8 = 21;
    pub const WEAPON_OFFHAND: u8 = 22;
    pub const HOLDABLE: u8 = 23; // off-hand frill / tome
    pub const THROWN: u8 = 25; // ranged
    pub const RANGED_RIGHT: u8 = 26; // gun / wand → ranged
}

/// Map an item's `inventory_type` to the single canonical `EQUIPMENT_SLOT_*` ordinal it equips into,
/// or `None` if the type isn't equippable (e.g. `INVTYPE_NON_EQUIP` for food/junk). Pure — unit-tested.
/// Covers all 19 equipment slots. The "pair" types (finger/trinket) return the FIRST slot of their
/// pair (10/12); the equip path then picks the first
/// FREE slot of the pair. 2H + main-hand-only weapons and a plain one-hand all map to MAINHAND (15);
/// off-hand weapon / shield / holdable map to OFFHAND (16); bow/gun/thrown/wand map to RANGED (17).
/// The vanilla first-free-slot rule, as a pure type→slot lookup of our own. [reference]
pub fn inventory_type_to_slot(inventory_type: u8) -> Option<u8> {
    use equip_slot as e;
    use invtype as t;
    Some(match inventory_type {
        t::HEAD => e::HEAD,
        t::NECK => e::NECK,
        t::SHOULDERS => e::SHOULDERS,
        t::BODY => e::SHIRT,
        t::CHEST | t::ROBE => e::CHEST, // a cloth robe wears in the chest slot
        t::WAIST => e::WAIST,
        t::LEGS => e::LEGS,
        t::FEET => e::FEET,
        t::WRISTS => e::WRISTS,
        t::HANDS => e::HANDS,
        t::FINGER => e::FINGER1,   // first of the finger pair (10/11)
        t::TRINKET => e::TRINKET1, // first of the trinket pair (12/13)
        t::CLOAK => e::BACK,
        // Main hand: a plain one-hand, a two-hander, or a main-hand-only weapon.
        t::WEAPON | t::TWO_HAND_WEAPON | t::WEAPON_MAINHAND => e::MAINHAND,
        // Off hand: an off-hand-only weapon, a shield, or a holdable frill/tome.
        t::WEAPON_OFFHAND | t::SHIELD | t::HOLDABLE => e::OFFHAND,
        // Ranged: a bow, a gun/wand (RANGED_RIGHT), or a thrown weapon.
        t::RANGED | t::RANGED_RIGHT | t::THROWN => e::RANGED,
        t::TABARD => e::TABARD,
        _ => return None, // NON_EQUIP and anything unrecognized isn't equippable
    })
}

/// The pair partner of a dual-slot equipment ordinal: FINGER1↔FINGER2 (10/11), TRINKET1↔TRINKET2
/// (12/13). `None` for every single-occupancy slot. Pure — unit-tested. Lets the equip path place a
/// ring/trinket in the first FREE slot of its pair (vanilla equips into ring-2 when ring-1 is taken).
pub(crate) fn paired_equip_slot(slot: u8) -> Option<u8> {
    use equip_slot as e;
    match slot {
        x if x == e::FINGER1 => Some(e::FINGER1 + 1),
        x if x == e::TRINKET1 => Some(e::TRINKET1 + 1),
        _ => None,
    }
}

/// Whether moving an item with `inventory_type` (of item `class`) INTO equipment `dest_slot` is a valid
/// equip. Pure — unit-tested. The main-hand slot (15) defers to `can_equip_mainhand` exactly: only a
/// class==2 weapon of invtype 13/17/21 equips there, so a class-0 food in invtype 21 is rejected. For
/// the dual-slot pairs the SECOND slot (finger-2 11, trinket-2 13) accepts the same type as its first.
/// Every other equipment slot accepts an item whose `inventory_type_to_slot` resolves to it. A
/// non-equipment `dest_slot` (>18) is not an equip move and returns `false` here (the caller only
/// consults this for equipment destinations).
///
/// `can_dual_wield`: when `true` (the caster knows Dual Wield, spell 674), a plain one-hander
/// (`INVTYPE_WEAPON`, 13 — the ONLY type `can_equip_mainhand` accepts besides the main-hand-only/2H
/// types, which stay main-hand-only) is ALSO accepted at OFFHAND (16), on top of its existing MAINHAND
/// acceptance below. When `false`, a one-hander is only accepted at MAINHAND (matching
/// `inventory_type_to_slot`'s WEAPON→MAINHAND mapping) — baseline-safe for every caller that hasn't
/// threaded Dual Wield knowledge. [reference]
pub fn can_equip_into(class: u8, inventory_type: u8, dest_slot: u8, can_dual_wield: bool) -> bool {
    use equip_slot as e;
    // Main-hand defers to the stricter `can_equip_mainhand` gate (weapon class + specific invtypes).
    if dest_slot == e::MAINHAND {
        return can_equip_mainhand(class, inventory_type);
    }
    if dest_slot == e::OFFHAND && can_dual_wield && class == 2 && inventory_type == invtype::WEAPON
    {
        return true; // a one-hander may ALSO land in the off-hand when Dual Wield is known
    }
    // The second slot of a pair accepts whatever resolves to the pair's first slot.
    let resolved = match inventory_type_to_slot(inventory_type) {
        Some(s) => s,
        None => return false,
    };
    let canonical = match dest_slot {
        x if x == e::FINGER1 + 1 => e::FINGER1,
        x if x == e::TRINKET1 + 1 => e::TRINKET1,
        other => other,
    };
    resolved == canonical
}

pub use lyracore_shared::item::{armor_subclass, weapon_subclass, Proficiency};

/// Whether a character of `player_level` meets an item's `required_level` to EQUIP or USE it. Pure —
/// unit-tested. A required_level of 1 (every seeded item today) is met by every character, so this gate
/// is a no-op for the existing loadout; only items SQL-raised above the character level are rejected.
/// Vanilla "requires level N" — you can carry a too-high item in the bag, you just can't equip/use it.
pub fn meets_required_level(player_level: u32, required_level: u8) -> bool {
    player_level >= required_level as u32
}

/// Which equip-stat an `equipped_stat_bonus` query sums across a unit's worn gear. A small typed enum
/// (vs. raw stat ids) so the call site reads self-documentingly and the column pick is exhaustive-checked
/// by the compiler. The five base attributes mirror the `UNIT_FIELD_STAT` order / the spell module's `STAT_*`; the
/// combat ratings (`Crit`/`Hit`) mirror the `COMBAT_*` attack-table fields; `Armor` mirrors the
/// `A_MOD_RESISTANCE(armor)` school. The variants compose with the aura sums in combat/ — each gear
/// total is added ALONGSIDE the matching aura total into the same effective-* helper. `Stamina` and
/// `Intellect` feed `recompute_vitals`'s max-health/max-mana derivation; `Spirit` is summable but not
/// folded into any pool (no Spirit-driven derive exists yet), so it's kept for symmetry with the other
/// four base attributes — hence `#[allow(dead_code)]` on the otherwise-unconstructed variant. [reference]
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EquipStat {
    Strength,
    Agility,
    Stamina,
    Intellect,
    Spirit,
    Crit,
    Hit,
    Armor,
}

/// This item template's contribution to one `EquipStat` — the single typed column the variant names.
/// Pure (no ctx): the per-template projection `equipped_stat_bonus` sums across a unit's worn pieces, so
/// the stat-sum arithmetic is unit-testable on a plain `ItemTemplate` without a live module. A column the
/// item doesn't carry is 0 (every starter/loadout template, e.g. entry 25, is all-zero → contributes 0,
/// baseline-safe). [reference]
pub(crate) fn template_stat(tmpl: &ItemTemplate, which: EquipStat) -> i32 {
    match which {
        EquipStat::Strength => tmpl.stat_strength,
        EquipStat::Agility => tmpl.stat_agility,
        EquipStat::Stamina => tmpl.stat_stamina,
        EquipStat::Intellect => tmpl.stat_intellect,
        EquipStat::Spirit => tmpl.stat_spirit,
        EquipStat::Crit => tmpl.stat_crit,
        EquipStat::Hit => tmpl.stat_hit,
        EquipStat::Armor => tmpl.stat_armor,
    }
}

/// ENCHANTING (completing the 13) — the per-instance enchant overlay table. `(enchant_id, EquipStat,
/// amount)`: an item carrying `enchant_id` adds `amount` to the named `EquipStat` ON TOP of its template
/// stat. The same single-meaning model as `template_stat` (typed col → amount), so an enchant folds through
/// the EXACT effective-* pipeline (combat swing/dodge/armor/crit/hit + the spell module's Stamina/Intellect → max
/// HP/mana). A handful of low-rank enchants is enough for the alpha (the real `SpellItemEnchantment.dbc`
/// import is DEFERRED). 0 (the column default) is the "no enchant" sentinel — never a row here, so an
/// unenchanted item contributes 0 (baseline-safe). [reference]
const ENCHANTS: &[(u32, EquipStat, i32)] = &[
    // Two low-rank exemplar enchants spanning a server-VERIFIABLE stat each: a +Strength weapon enchant
    // (moves the melee swing readout) and a +Stamina chest enchant (moves max-HP via recompute_vitals).
    (7745, EquipStat::Strength, 3), // "Minor Strength"-style weapon enchant: a flat +3 STR (verify via debug_compute_swing)
    (7748, EquipStat::Stamina, 3), // "Minor Stamina"-style chest enchant: a flat +3 STA (verify via recompute_vitals max-HP)
];

/// This enchant's contribution to one `EquipStat` (0 if it doesn't touch that stat, or the id is 0/unknown).
/// Pure (no ctx) so the overlay arithmetic is unit-testable on plain values. Sums matching rows (an enchant
/// id appears at most once per stat today, but summing keeps a future multi-stat enchant correct).
/// `equipped_stat_bonus` adds this alongside `template_stat` for each equipped instance. [reference]
pub(crate) fn enchant_stat(enchant_id: u32, which: EquipStat) -> i32 {
    if enchant_id == 0 {
        return 0; // the "no enchant" sentinel — never matches a row, short-circuit
    }
    ENCHANTS
        .iter()
        .filter(|(id, stat, _)| *id == enchant_id && *stat == which)
        .map(|(_, _, amount)| *amount)
        .sum()
}

/// Whether `enchant_id` is a known, applyable enchant (in the ENCHANTS table). Gates `enchant_item` so a
/// player can't stamp an arbitrary id onto an instance. Pure — unit-tested. 0 is "none" → not applyable here.
pub(crate) fn is_known_enchant(enchant_id: u32) -> bool {
    enchant_id != 0 && ENCHANTS.iter().any(|(id, _, _)| *id == enchant_id)
}

/// The equipment slot an item in `from_slot` should EQUIP into, given its `inventory_type` and the
/// player's current equipped set — the auto-resolve vanilla performs on a right-click-equip. Pure over
/// the (resolved-slot, occupied-set) inputs so it's unit-testable. Returns `None` if the item isn't
/// equippable. For a dual-slot pair (finger/trinket): if the FIRST slot is free it goes there, else the
/// SECOND; if both are taken it falls back to the first (an equip into an occupied slot SWAPS the
/// resident back into `from_slot`, same as a manual move). Single-occupancy slots always return their
/// one slot. The `occupied` predicate reports whether a given equip slot currently holds an item.
///
/// `can_dual_wield`: when `true` AND `inventory_type` is a plain one-hander (`INVTYPE_WEAPON`, 13,
/// which normally resolves ONLY to MAINHAND) AND main-hand is already occupied AND off-hand is free,
/// this redirects the second one-hander to OFFHAND instead of falling through to the MAINHAND swap —
/// so a Rogue who has learned Dual Wield and right-click-equips a second one-hander lands it in the
/// off-hand, not swapping out the first weapon. `false` (or a 2H/main-hand-only weapon, or main-hand
/// free, or off-hand occupied) resolves to MAINHAND as usual — baseline-safe for every caller that
/// hasn't threaded Dual Wield knowledge.
pub(crate) fn resolve_equip_slot(
    inventory_type: u8,
    can_dual_wield: bool,
    occupied: impl Fn(u8) -> bool,
) -> Option<u8> {
    let primary = inventory_type_to_slot(inventory_type)?;
    if can_dual_wield
        && inventory_type == invtype::WEAPON
        && primary == equip_slot::MAINHAND
        && occupied(equip_slot::MAINHAND)
        && !occupied(equip_slot::OFFHAND)
    {
        return Some(equip_slot::OFFHAND);
    }
    // For a pair, prefer the first free of the two; otherwise the canonical (first) slot.
    if let Some(second) = paired_equip_slot(primary) {
        if !occupied(primary) {
            return Some(primary);
        }
        if !occupied(second) {
            return Some(second);
        }
    }
    Some(primary)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::items::tables::ItemTemplate;

    /// The class CEILING: what a class can equip with both level-40 armor upgrades trained. The
    /// equip Gate asks the stricter `Proficiency::from_spellbook` question; these cases pin the
    /// class table itself, which training never changes.
    fn can_equip_proficiency(player_class: u8, item_class: u8, item_subclass: u8) -> bool {
        Proficiency::class_ceiling(player_class).can_equip(item_class, item_subclass)
    }

    #[test]
    fn sell_value_multiplies_by_stack_count() {
        assert_eq!(sell_value(0, 5), 0); // a 0-priced (valueless) item is worth nothing per the caller's guard
        assert_eq!(sell_value(2, 5), 10); // 5 units at 2 copper each = 10 copper
        assert_eq!(sell_value(u32::MAX, 2), u32::MAX); // saturates instead of wrapping the copper total
    }

    #[test]
    fn buy_cost_multiplies_by_count() {
        assert_eq!(buy_cost(0, 5), 0); // a 0-priced (unbuyable) item is rejected by the caller, costs nothing here
        assert_eq!(buy_cost(35, 2), 70); // 2 units at 35 copper each = 70 copper
        assert_eq!(buy_cost(u32::MAX, 2), u32::MAX); // saturates instead of wrapping the copper total
    }

    #[test]
    fn repair_cost_is_one_copper_per_restored_point() {
        assert_eq!(repair_cost(100, 100), 0); // undamaged → free (button greyed in vanilla)
        assert_eq!(repair_cost(100, 0), 100); // fully broken → full max_durability in copper
        assert_eq!(repair_cost(100, 60), 40); // 40 points of damage → 40 copper
        assert_eq!(repair_cost(0, 0), 0); // a no-durability item never costs anything
        assert_eq!(repair_cost(50, 80), 0); // saturating: durability above max never wraps
    }

    #[test]
    fn merge_amount_caps_at_destination_headroom() {
        // The whole source fits under max_stack → all 3 move (src empties, then deletes).
        assert_eq!(merge_amount(3, 2, 20), 3);
        // Source larger than headroom (20 - 10 = 10) → capped at 10, leaving a remainder in src.
        assert_eq!(merge_amount(15, 10, 20), 10);
        // Destination already full → zero headroom → nothing moves (no-op, no swap).
        assert_eq!(merge_amount(5, 20, 20), 0);
        // An over-full destination still yields 0 (saturating_sub never wraps).
        assert_eq!(merge_amount(5, 25, 20), 0);
    }

    #[test]
    fn can_equip_mainhand_only_accepts_mainhand_weapon_types() {
        // A Weapon (class 2) in a main-hand-capable inventory_type equips.
        assert!(can_equip_mainhand(2, 21)); // INVTYPE_WEAPONMAINHAND
        assert!(can_equip_mainhand(2, 13)); // INVTYPE_WEAPON (one-hand)
        assert!(can_equip_mainhand(2, 17)); // INVTYPE_2HWEAPON
                                            // Non-weapon classes are rejected even with a weapon-shaped inventory_type.
        assert!(!can_equip_mainhand(0, 21)); // class 0 = Consumable (food)
        assert!(!can_equip_mainhand(4, 21)); // class 4 = Armor
                                             // A weapon with a non-main-hand inventory_type (5 = INVTYPE_CHEST) is rejected.
        assert!(!can_equip_mainhand(2, 5));
    }

    #[test]
    fn meets_required_level_gates_below_requirement_only() {
        assert!(meets_required_level(1, 1)); // level 1 meets a required_level 1 item (every seeded item)
        assert!(meets_required_level(5, 1)); // a higher-level character meets a low requirement
        assert!(!meets_required_level(1, 5)); // a level-1 character can't equip/use a level-5 item
        assert!(meets_required_level(5, 5)); // exactly meeting the requirement is allowed
    }

    #[test]
    fn explicit_equip_requirements_fail_closed_and_admit_qualified_characters() {
        // Class/race ids are one-based bits. A malformed durable zero mask never becomes universal.
        assert!(eligibility_mask_allows(1 << 7, 8));
        assert!(!eligibility_mask_allows(1 << 7, 9));
        assert!(!eligibility_mask_allows(0, 8));
        assert!(!eligibility_mask_allows(u32::MAX, 0));

        // Vanilla class ids skip 6 and 10; the unrestricted sentinel must admit all nine playable
        // classes, Druid (11) included, and set no bit for the unused ids.
        for class_id in [1u8, 2, 3, 4, 5, 7, 8, 9, 11] {
            assert!(eligibility_mask_allows(
                crate::items::tables::ALL_PLAYABLE_CLASS_MASK,
                class_id
            ));
        }
        assert!(!eligibility_mask_allows(
            crate::items::tables::ALL_PLAYABLE_CLASS_MASK,
            6
        ));
        assert!(!eligibility_mask_allows(
            crate::items::tables::ALL_PLAYABLE_CLASS_MASK,
            10
        ));

        // A required skill needs both a durable row and enough current rank.
        assert!(!meets_required_skill(171, 50, None));
        assert!(!meets_required_skill(171, 50, Some(49)));
        assert!(meets_required_skill(171, 50, Some(50)));
        assert!(meets_required_skill(0, 50, None));

        // Missing reputation uses Neutral (standing 0, rank 3), not a faction base standing.
        assert!(!meets_required_reputation(72, 4, None));
        assert!(!meets_required_reputation(72, 4, Some(2_999)));
        assert!(meets_required_reputation(72, 4, Some(3_000)));
        assert!(meets_required_reputation(0, 7, None));
    }

    #[test]
    fn inventory_type_to_slot_maps_each_invtype_to_its_equipment_slot() {
        use equip_slot as e;
        use invtype as t;
        // Armor / apparel.
        assert_eq!(inventory_type_to_slot(t::HEAD), Some(e::HEAD)); // 1 -> 0
        assert_eq!(inventory_type_to_slot(t::NECK), Some(e::NECK)); // 2 -> 1
        assert_eq!(inventory_type_to_slot(t::SHOULDERS), Some(e::SHOULDERS)); // 3 -> 2
        assert_eq!(inventory_type_to_slot(t::BODY), Some(e::SHIRT)); // 4 -> 3 (shirt)
        assert_eq!(inventory_type_to_slot(t::CHEST), Some(e::CHEST)); // 5 -> 4
        assert_eq!(inventory_type_to_slot(t::ROBE), Some(e::CHEST)); // 20 (robe) -> 4 (chest)
        assert_eq!(inventory_type_to_slot(t::WAIST), Some(e::WAIST)); // 6 -> 5
        assert_eq!(inventory_type_to_slot(t::LEGS), Some(e::LEGS)); // 7 -> 6
        assert_eq!(inventory_type_to_slot(t::FEET), Some(e::FEET)); // 8 -> 7
        assert_eq!(inventory_type_to_slot(t::WRISTS), Some(e::WRISTS)); // 9 -> 8
        assert_eq!(inventory_type_to_slot(t::HANDS), Some(e::HANDS)); // 10 -> 9
        assert_eq!(inventory_type_to_slot(t::FINGER), Some(e::FINGER1)); // 11 -> 10 (first of pair)
        assert_eq!(inventory_type_to_slot(t::TRINKET), Some(e::TRINKET1)); // 12 -> 12 (first of pair)
        assert_eq!(inventory_type_to_slot(t::CLOAK), Some(e::BACK)); // 16 -> 14
        assert_eq!(inventory_type_to_slot(t::TABARD), Some(e::TABARD)); // 19 -> 18
                                                                        // Weapons → main hand (1H, 2H, main-hand-only all land in 15).
        assert_eq!(inventory_type_to_slot(t::WEAPON), Some(e::MAINHAND)); // 13 -> 15
        assert_eq!(
            inventory_type_to_slot(t::TWO_HAND_WEAPON),
            Some(e::MAINHAND)
        ); // 17 -> 15
        assert_eq!(
            inventory_type_to_slot(t::WEAPON_MAINHAND),
            Some(e::MAINHAND)
        ); // 21 -> 15
           // Off hand → 16 (off-hand weapon, shield, holdable).
        assert_eq!(inventory_type_to_slot(t::WEAPON_OFFHAND), Some(e::OFFHAND)); // 22 -> 16
        assert_eq!(inventory_type_to_slot(t::SHIELD), Some(e::OFFHAND)); // 14 -> 16
        assert_eq!(inventory_type_to_slot(t::HOLDABLE), Some(e::OFFHAND)); // 23 -> 16
                                                                           // Ranged → 17 (bow, gun/wand, thrown).
        assert_eq!(inventory_type_to_slot(t::RANGED), Some(e::RANGED)); // 15 -> 17
        assert_eq!(inventory_type_to_slot(t::RANGED_RIGHT), Some(e::RANGED)); // 26 -> 17
        assert_eq!(inventory_type_to_slot(t::THROWN), Some(e::RANGED)); // 25 -> 17
                                                                        // Non-equippable types map to no slot.
        assert_eq!(inventory_type_to_slot(t::NON_EQUIP), None); // 0 (food/junk)
        assert_eq!(inventory_type_to_slot(99), None); // unrecognized
    }

    #[test]
    fn can_equip_into_mainhand_is_byte_identical_to_can_equip_mainhand() {
        use equip_slot::MAINHAND;
        // Every case `can_equip_mainhand` accepts/rejects must be unchanged at slot 15.
        assert!(can_equip_into(2, 21, MAINHAND, false)); // weapon main-hand
        assert!(can_equip_into(2, 13, MAINHAND, false)); // one-hand
        assert!(can_equip_into(2, 17, MAINHAND, false)); // two-hand
        assert!(!can_equip_into(0, 21, MAINHAND, false)); // food in invtype 21 still rejected (class check)
        assert!(!can_equip_into(4, 21, MAINHAND, false)); // armor still rejected at main-hand
        assert!(!can_equip_into(2, 5, MAINHAND, false)); // a chest-invtype weapon rejected at main-hand
                                                         // Cross-check: identical to the original predicate across the same inputs.
        for &(c, it) in &[(2u8, 21u8), (2, 13), (2, 17), (0, 21), (4, 21), (2, 5)] {
            assert_eq!(
                can_equip_into(c, it, MAINHAND, false),
                can_equip_mainhand(c, it)
            );
        }
    }

    #[test]
    fn can_equip_into_validates_armor_and_offhand_and_rejects_mismatch() {
        use equip_slot as e;
        use invtype as t;
        // A helm (class 4 Armor, INVTYPE_HEAD) equips into HEAD but NOT into CHEST.
        assert!(can_equip_into(4, t::HEAD, e::HEAD, false));
        assert!(!can_equip_into(4, t::HEAD, e::CHEST, false));
        // A shield lands in OFFHAND (16), not main-hand or chest.
        assert!(can_equip_into(4, t::SHIELD, e::OFFHAND, false));
        assert!(!can_equip_into(4, t::SHIELD, e::MAINHAND, false));
        // A robe (cloth chest) equips into CHEST.
        assert!(can_equip_into(4, t::ROBE, e::CHEST, false));
        // The SECOND ring slot (11) accepts a finger item (first-of-pair resolves to 10).
        assert!(can_equip_into(4, t::FINGER, e::FINGER1, false)); // 10
        assert!(can_equip_into(4, t::FINGER, e::FINGER1 + 1, false)); // 11 (pair partner)
        assert!(can_equip_into(4, t::TRINKET, e::TRINKET1 + 1, false)); // 13 (trinket-2)
                                                                        // A ring does NOT fit a trinket slot or vice-versa.
        assert!(!can_equip_into(4, t::FINGER, e::TRINKET1, false));
        // A non-equippable (food) fits nowhere.
        assert!(!can_equip_into(0, t::NON_EQUIP, e::CHEST, false));
        // A non-equipment destination slot (>18) is never an equip.
        assert!(!can_equip_into(4, t::HEAD, 23, false)); // backpack slot 0
    }

    #[test]
    fn can_equip_into_offhand_only_accepts_one_hander_with_dual_wield() {
        use equip_slot as e;
        use invtype as t;
        // A plain one-hander (class 2, INVTYPE_WEAPON) is REJECTED at OFFHAND without Dual Wield — it
        // only resolves to MAINHAND.
        assert!(!can_equip_into(2, t::WEAPON, e::OFFHAND, false));
        // WITH Dual Wield known, the same one-hander IS accepted at OFFHAND.
        assert!(can_equip_into(2, t::WEAPON, e::OFFHAND, true));
        // A 2H weapon or main-hand-only weapon is NEVER accepted at OFFHAND, even with Dual Wield —
        // only the plain one-hander (INVTYPE_WEAPON, 13) gets the off-hand carve-out.
        assert!(!can_equip_into(2, t::TWO_HAND_WEAPON, e::OFFHAND, true));
        assert!(!can_equip_into(2, t::WEAPON_MAINHAND, e::OFFHAND, true));
        // A non-weapon class is never accepted at OFFHAND via the dual-wield carve-out either.
        assert!(!can_equip_into(4, t::WEAPON, e::OFFHAND, true));
        // MAINHAND acceptance is unaffected by the flag either way.
        assert!(can_equip_into(2, t::WEAPON, e::MAINHAND, true));
        assert!(can_equip_into(2, t::WEAPON, e::MAINHAND, false));
    }

    #[test]
    fn paired_equip_slot_only_for_finger_and_trinket() {
        use equip_slot as e;
        assert_eq!(paired_equip_slot(e::FINGER1), Some(e::FINGER1 + 1)); // 10 -> 11
        assert_eq!(paired_equip_slot(e::TRINKET1), Some(e::TRINKET1 + 1)); // 12 -> 13
        assert_eq!(paired_equip_slot(e::HEAD), None); // single-occupancy slots have no pair
        assert_eq!(paired_equip_slot(e::MAINHAND), None);
        assert_eq!(paired_equip_slot(e::FINGER1 + 1), None); // the second slot isn't itself a pair head
    }

    /// A bare `ItemTemplate` for the stat-sum tests: zero everything except the stat columns the test
    /// sets via the returned mutable handle. Keeps the pure stat arithmetic testable without a module.
    // pub(crate): reused by `trade::tests` (#121) — the one 100-field template fixture.
    pub(crate) fn blank_template(entry: u32) -> ItemTemplate {
        ItemTemplate {
            entry,
            class: 4,
            subclass: 0,
            name: String::new(),
            display_id: 0,
            quality: 0,
            inventory_type: 0,
            item_level: 0,
            required_level: 0,
            max_durability: 0,
            buy_price: 0,
            sell_price: 0,
            max_stack: 1,
            damage_min: 0.0,
            damage_max: 0.0,
            delay_ms: 0,
            stat_strength: 0,
            stat_agility: 0,
            stat_stamina: 0,
            stat_intellect: 0,
            stat_spirit: 0,
            stat_crit: 0,
            stat_hit: 0,
            stat_armor: 0,
            block_value: 0,
            restores_power: false,
            spellid_1: 0,
            spelltrigger_1: 0,
            spellid_2: 0,
            spelltrigger_2: 0,
            container_slots: 0,
            sheath: 0,
            bonding: 0,
            holy_res: 0,
            fire_res: 0,
            nature_res: 0,
            frost_res: 0,
            shadow_res: 0,
            arcane_res: 0,
            spellid_3: 0,
            spelltrigger_3: 0,
            spellid_4: 0,
            spelltrigger_4: 0,
            spellid_5: 0,
            spelltrigger_5: 0,
            required_skill: 0,
            required_skill_rank: 0,
            required_reputation_faction: 0,
            required_reputation_rank: 0,
            max_count: 0,
            item_flags: 0,
            page_text: 0,
            start_quest: 0,
            bag_family: 0,
            buy_count: 1,
            food_type: 0,
            allowed_class: crate::items::tables::ALL_PLAYABLE_CLASS_MASK,
            allowed_race: crate::items::tables::ALL_PLAYABLE_RACE_MASK,
        }
    }

    #[test]
    fn template_stat_projects_the_named_column() {
        let mut t = blank_template(1000);
        t.stat_strength = 10;
        t.stat_agility = 20;
        t.stat_armor = 50;
        t.stat_crit = 100;
        t.stat_hit = 30;
        assert_eq!(template_stat(&t, EquipStat::Strength), 10);
        assert_eq!(template_stat(&t, EquipStat::Agility), 20);
        assert_eq!(template_stat(&t, EquipStat::Armor), 50);
        assert_eq!(template_stat(&t, EquipStat::Crit), 100);
        assert_eq!(template_stat(&t, EquipStat::Hit), 30);
        // An untouched column is 0 — the starter/loadout templates carry no stats, so they contribute 0.
        assert_eq!(template_stat(&t, EquipStat::Stamina), 0);
        assert_eq!(template_stat(&blank_template(25), EquipStat::Strength), 0);
    }

    /// Re-scoped (was `equipped_stat_sum_adds_across_pieces_and_is_zero_for_no_gear`): the original name
    /// oversold this as covering `equipped_stat_bonus`, but that fn needs a live `ReducerContext` (it
    /// joins the owner's equipped `ItemInstance` rows through `game_item_template`, filters to the
    /// equipment slot range, and excludes broken items — none of which this test touches). What's
    /// actually pinned here is only the pure per-stat SUM `template_stat` feeds into that fold: summing
    /// `template_stat` over a worn set adds per-stat independently and is 0 for an empty/all-zero set.
    #[test]
    fn template_stat_sums_independently_per_stat_and_is_zero_for_no_gear() {
        let sum = |pieces: &[ItemTemplate], which: EquipStat| -> i32 {
            pieces.iter().map(|t| template_stat(t, which)).sum()
        };
        // No gear (a creature or an unequipped player) → 0 for every stat (byte-identical readout).
        assert_eq!(sum(&[], EquipStat::Strength), 0);
        assert_eq!(sum(&[], EquipStat::Armor), 0);
        // Two pieces: a +10 STR chest and a +5 STR / +20 AGI / +50 armor item sum per-stat independently.
        let mut chest = blank_template(2001);
        chest.stat_strength = 10;
        let mut ring = blank_template(2002);
        ring.stat_strength = 5;
        ring.stat_agility = 20;
        ring.stat_armor = 50;
        let worn = [chest, ring];
        assert_eq!(sum(&worn, EquipStat::Strength), 15); // 10 + 5
        assert_eq!(sum(&worn, EquipStat::Agility), 20); // 0 + 20
        assert_eq!(sum(&worn, EquipStat::Armor), 50); // 0 + 50
        assert_eq!(sum(&worn, EquipStat::Crit), 0); // neither piece carries crit
                                                    // All-zero templates (the existing loadout, e.g. entry 25) contribute nothing → baseline-safe.
        let plain = [blank_template(25), blank_template(51)];
        assert_eq!(sum(&plain, EquipStat::Strength), 0);
        assert_eq!(sum(&plain, EquipStat::Hit), 0);
    }

    /// Re-scoped (was `enchant_overlay_adds_its_stat_on_top_of_the_template`): the name implied this pins
    /// `equipped_stat_bonus`'s actual fold, but that fold runs over live equipped `ItemInstance` rows (a
    /// ctx fn, not exercised here) — this test only hand-adds `template_stat + enchant_stat`, the two pure
    /// pieces, to show the composition is sensible. What IS real and unit-tested here: `enchant_stat`
    /// (7745 = +3 STR, 7748 = +3 STA, touching no other stat; id 0 and an unknown id add 0 to every stat)
    /// and `is_known_enchant`'s validity gate.
    #[test]
    fn enchant_stat_adds_its_amount_on_top_of_a_hand_summed_template_stat() {
        // A base weapon with +5 STR from its template; the +3 STR enchant (7745) lifts effective STR to +8.
        let mut weapon = blank_template(3001);
        weapon.stat_strength = 5;
        let base = template_stat(&weapon, EquipStat::Strength);
        assert_eq!(base, 5);
        assert_eq!(
            enchant_stat(7745, EquipStat::Strength),
            3,
            "the +STR weapon enchant adds 3"
        );
        assert_eq!(
            base + enchant_stat(7745, EquipStat::Strength),
            8,
            "effective STR rose by the enchant"
        );
        // The +STR enchant touches ONLY Strength — it adds nothing to other stats.
        assert_eq!(enchant_stat(7745, EquipStat::Stamina), 0);
        assert_eq!(enchant_stat(7745, EquipStat::Agility), 0);
        // The +STA chest enchant (7748) moves Stamina (→ max-HP via recompute_vitals), not Strength.
        assert_eq!(enchant_stat(7748, EquipStat::Stamina), 3);
        assert_eq!(enchant_stat(7748, EquipStat::Strength), 0);
        // The "no enchant" sentinel (0) and any unknown id add 0 to EVERY stat → an unenchanted/legacy item
        // reads byte-identical (baseline-safe). And only known ids are applyable.
        for which in [
            EquipStat::Strength,
            EquipStat::Stamina,
            EquipStat::Agility,
            EquipStat::Armor,
        ] {
            assert_eq!(enchant_stat(0, which), 0, "enchant_id 0 = no overlay");
            assert_eq!(
                enchant_stat(99999, which),
                0,
                "an unknown enchant adds nothing"
            );
        }
        assert!(
            is_known_enchant(7745) && is_known_enchant(7748),
            "the seeded enchants are applyable"
        );
        assert!(
            !is_known_enchant(0) && !is_known_enchant(99999),
            "0 and unknown ids are not applyable"
        );
    }

    #[test]
    fn resolve_equip_slot_picks_first_free_of_a_pair() {
        use equip_slot as e;
        use invtype as t;
        // Single slot: always its one slot, regardless of occupancy.
        assert_eq!(resolve_equip_slot(t::HEAD, false, |_| false), Some(e::HEAD));
        assert_eq!(resolve_equip_slot(t::HEAD, false, |_| true), Some(e::HEAD));
        // Ring, both free → first of pair (10).
        assert_eq!(
            resolve_equip_slot(t::FINGER, false, |_| false),
            Some(e::FINGER1)
        );
        // Ring, first taken → second (11).
        assert_eq!(
            resolve_equip_slot(t::FINGER, false, |s| s == e::FINGER1),
            Some(e::FINGER1 + 1)
        );
        // Ring, both taken → falls back to the first (a swap into ring-1).
        assert_eq!(
            resolve_equip_slot(t::FINGER, false, |_| true),
            Some(e::FINGER1)
        );
        // Trinket pair behaves the same.
        assert_eq!(
            resolve_equip_slot(t::TRINKET, false, |s| s == e::TRINKET1),
            Some(e::TRINKET1 + 1)
        );
        // A non-equippable resolves to no slot.
        assert_eq!(resolve_equip_slot(t::NON_EQUIP, false, |_| false), None);
        // Weapon (one-hander), main-hand free → resolves to MAINHAND regardless of the dual-wield flag
        // (nothing to redirect from yet).
        assert_eq!(
            resolve_equip_slot(t::WEAPON, true, |_| false),
            Some(e::MAINHAND)
        );
        assert_eq!(
            resolve_equip_slot(t::WEAPON, false, |_| false),
            Some(e::MAINHAND)
        );
    }

    #[test]
    fn resolve_equip_slot_routes_second_one_hander_to_offhand_with_dual_wield() {
        use equip_slot as e;
        use invtype as t;
        // Main-hand occupied, off-hand free, Dual Wield known → the second one-hander goes to OFFHAND.
        assert_eq!(
            resolve_equip_slot(t::WEAPON, true, |s| s == e::MAINHAND),
            Some(e::OFFHAND)
        );
        // Same occupancy WITHOUT Dual Wield → falls through to a MAINHAND swap (baseline-safe).
        assert_eq!(
            resolve_equip_slot(t::WEAPON, false, |s| s == e::MAINHAND),
            Some(e::MAINHAND)
        );
        // Main-hand occupied AND off-hand occupied, even with Dual Wield → no free off-hand to redirect
        // into, falls back to MAINHAND (a swap).
        assert_eq!(
            resolve_equip_slot(t::WEAPON, true, |_| true),
            Some(e::MAINHAND)
        );
        // A 2H weapon is NEVER redirected to OFFHAND even with Dual Wield known (primary != MAINHAND
        // guard doesn't apply here since 2H DOES resolve to MAINHAND, but it should still just swap,
        // never split into the off-hand slot).
        assert_eq!(
            resolve_equip_slot(t::TWO_HAND_WEAPON, true, |s| s == e::MAINHAND),
            Some(e::MAINHAND)
        );
    }

    #[test]
    fn proficiency_mage_cloth_only() {
        // Mage (class 8) may equip cloth (subclass 1) and misc accessories (subclass 0).
        assert!(can_equip_proficiency(8, 4, 0)); // misc — ring/neck/trinket/cloak
        assert!(can_equip_proficiency(8, 4, 1)); // cloth
                                                 // Mage cannot equip leather, mail, or plate.
        assert!(!can_equip_proficiency(8, 4, 2)); // leather
        assert!(!can_equip_proficiency(8, 4, 3)); // mail
        assert!(!can_equip_proficiency(8, 4, 4)); // plate — THE MAIN REGRESSION
        assert!(!can_equip_proficiency(8, 4, 6)); // shield
    }

    #[test]
    fn proficiency_warlock_cloth_only() {
        assert!(can_equip_proficiency(9, 4, 0)); // misc
        assert!(can_equip_proficiency(9, 4, 1)); // cloth
        assert!(!can_equip_proficiency(9, 4, 2)); // leather
        assert!(!can_equip_proficiency(9, 4, 4)); // plate
    }

    /// EQUIP GATE, PLATE: plate is a level-40 trainer purchase, not a class birthright. The Gate
    /// reads no level at all here — a Warrior at 1 and at 60 are refused alike until spell 750 is in
    /// the spellbook, and mail is wearable from the first level either way.
    #[test]
    fn plate_needs_the_trained_passive_whatever_the_level() {
        use armor_subclass as a;
        const ARMOR: u8 = 4;
        for class in [1u8, 2] {
            let untrained = Proficiency::derive(class, false, false);
            assert!(
                !untrained.can_equip(ARMOR, a::PLATE),
                "class {class} at any level without 750"
            );
            assert!(
                untrained.can_equip(ARMOR, a::MAIL),
                "class {class} wears mail from level 1"
            );
            assert!(Proficiency::derive(class, true, false).can_equip(ARMOR, a::PLATE));
        }
    }

    /// EQUIP GATE, MAIL: Hunter and Shaman start in leather and buy the mail upgrade (8737) at 40.
    #[test]
    fn hunters_and_shamans_need_the_trained_passive_for_mail() {
        use armor_subclass as a;
        const ARMOR: u8 = 4;
        for class in [3u8, 7] {
            assert!(!Proficiency::derive(class, false, false).can_equip(ARMOR, a::MAIL));
            assert!(Proficiency::derive(class, false, true).can_equip(ARMOR, a::MAIL));
            // The plate flag is not theirs to use.
            assert!(!Proficiency::derive(class, true, true).can_equip(ARMOR, a::PLATE));
        }
    }

    /// Proficiency gates the BODY only. Every non-equip destination — bags, bank, trade, mail,
    /// vendor purchases — reaches its mutation without this rule, which is why the Gate lives in
    /// `apply_item_move`'s equipment branch alone.
    #[test]
    fn proficiency_leaves_non_equippable_item_classes_alone() {
        let untrained_warrior = Proficiency::derive(1, false, false);
        for item_class in [0u8, 1, 3, 5, 6, 7, 9, 11, 12, 15] {
            assert!(
                untrained_warrior.can_equip(item_class, 4),
                "item class {item_class} carries no proficiency restriction"
            );
        }
    }

    #[test]
    fn proficiency_warrior_can_wear_plate() {
        // The class CEILING (the auction "usable" filter's question): a Warrior's reachable tiers
        // include plate once trained. The equip Gate asks the stricter question above.
        assert!(can_equip_proficiency(1, 4, 1)); // cloth
        assert!(can_equip_proficiency(1, 4, 2)); // leather
        assert!(can_equip_proficiency(1, 4, 3)); // mail
        assert!(can_equip_proficiency(1, 4, 4)); // plate — Warrior can equip this
        assert!(can_equip_proficiency(1, 4, 6)); // shield
    }

    #[test]
    fn proficiency_paladin_can_wear_plate() {
        assert!(can_equip_proficiency(2, 4, 4)); // plate
        assert!(can_equip_proficiency(2, 4, 6)); // shield
    }

    #[test]
    fn proficiency_rogue_leather_only() {
        assert!(can_equip_proficiency(4, 4, 1)); // cloth (universally allowed)
        assert!(can_equip_proficiency(4, 4, 2)); // leather
        assert!(!can_equip_proficiency(4, 4, 3)); // mail
        assert!(!can_equip_proficiency(4, 4, 4)); // plate
        assert!(!can_equip_proficiency(4, 4, 6)); // shield
    }

    #[test]
    fn proficiency_druid_leather_only() {
        assert!(can_equip_proficiency(11, 4, 2)); // leather
        assert!(!can_equip_proficiency(11, 4, 3)); // mail
        assert!(!can_equip_proficiency(11, 4, 6)); // shield
    }

    #[test]
    fn proficiency_hunter_leather_and_mail() {
        assert!(can_equip_proficiency(3, 4, 2)); // leather
        assert!(can_equip_proficiency(3, 4, 3)); // mail — reachable, trained at 40
        assert!(!can_equip_proficiency(3, 4, 4)); // plate
        assert!(!can_equip_proficiency(3, 4, 6)); // shield
    }

    #[test]
    fn proficiency_shaman_leather_mail_shield() {
        assert!(can_equip_proficiency(7, 4, 2)); // leather
        assert!(can_equip_proficiency(7, 4, 3)); // mail
        assert!(!can_equip_proficiency(7, 4, 4)); // plate
        assert!(can_equip_proficiency(7, 4, 6)); // shield
    }

    #[test]
    fn proficiency_non_armor_always_allowed() {
        // Non-armor item classes (consumables, quest items, etc.) have no class restriction.
        assert!(can_equip_proficiency(8, 0, 0)); // consumable
        assert!(can_equip_proficiency(8, 7, 0)); // trade goods
        assert!(can_equip_proficiency(9, 12, 0)); // quest item
    }

    #[test]
    fn proficiency_wand_only_for_caster_classes() {
        use weapon_subclass as w;
        // Only Mage (8), Warlock (9), Priest (5) may use wands.
        assert!(can_equip_proficiency(8, 2, w::WAND));
        assert!(can_equip_proficiency(9, 2, w::WAND));
        assert!(can_equip_proficiency(5, 2, w::WAND));
        // Warrior and Rogue cannot use wands.
        assert!(!can_equip_proficiency(1, 2, w::WAND));
        assert!(!can_equip_proficiency(4, 2, w::WAND));
    }

    /// Item binding: `binds_on_grant` is true ONLY for BoP; `binds_on_equip` is true ONLY for BoE.
    /// Every other bonding value (unbound, BoU, both quest-item codes) binds at neither trigger today —
    /// the pure taxonomy `ops::store_item`/`grant_starter_item` (grant) and `apply_item_move`'s equip
    /// branch (equip) key off.
    #[test]
    fn binding_taxonomy_grant_vs_equip_triggers() {
        use bonding as b;
        assert!(binds_on_grant(b::BIND_ON_PICKUP));
        assert!(!binds_on_grant(b::NONE));
        assert!(!binds_on_grant(b::BIND_ON_EQUIP));
        assert!(!binds_on_grant(b::BIND_ON_USE));
        assert!(!binds_on_grant(b::QUEST_ITEM));
        assert!(!binds_on_grant(b::QUEST_ITEM2));

        assert!(binds_on_equip(b::BIND_ON_EQUIP));
        assert!(!binds_on_equip(b::NONE));
        assert!(!binds_on_equip(b::BIND_ON_PICKUP));
        assert!(!binds_on_equip(b::BIND_ON_USE));
        assert!(!binds_on_equip(b::QUEST_ITEM));
        assert!(!binds_on_equip(b::QUEST_ITEM2));
    }

    #[test]
    fn proficiency_fishing_pole_any_class() {
        use weapon_subclass as w;
        for class in [1u8, 2, 3, 4, 5, 7, 8, 9, 11] {
            assert!(can_equip_proficiency(class, 2, w::FISHING_POLE));
        }
    }
}
