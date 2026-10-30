use super::tables::ItemTemplate;

// Class/race IDs are one-based bit indices; a zero durable mask grants no eligibility.
pub(crate) fn eligibility_mask_allows(mask: u32, id: u8) -> bool {
    id != 0 && (id as u32) <= u32::BITS && mask & (1u32 << (id - 1)) != 0
}

pub(crate) fn meets_required_skill(
    required_skill: u32,
    required_skill_rank: u32,
    current_skill: Option<u16>,
) -> bool {
    required_skill == 0
        || current_skill.is_some_and(|current| u32::from(current) >= required_skill_rank)
}

pub(crate) fn meets_required_reputation(
    required_faction: u32,
    required_rank: u32,
    standing: Option<i32>,
) -> bool {
    required_faction == 0
        || u32::from(crate::reputation::reputation_rank(standing.unwrap_or(0))) >= required_rank
}

pub(crate) fn sell_value(unit_price: u32, count: u32) -> u32 {
    unit_price.saturating_mul(count)
}

pub(crate) fn buy_cost(unit_price: u32, count: u32) -> u32 {
    unit_price.saturating_mul(count)
}

// Repair currently costs one copper per point; DBC DurabilityCost is not imported.
pub(crate) fn repair_cost(max_durability: u32, durability: u32) -> u32 {
    max_durability.saturating_sub(durability)
}

pub(crate) fn merge_amount(src: u32, dst: u32, max_stack: u32) -> u32 {
    src.min(max_stack.saturating_sub(dst))
}

pub(crate) fn can_equip_mainhand(class: u8, inventory_type: u8) -> bool {
    class == 2 && matches!(inventory_type, 13 | 17 | 21)
}

pub mod bonding {
    pub const NONE: u8 = 0;
    pub const BIND_ON_PICKUP: u8 = 1;
    pub const BIND_ON_EQUIP: u8 = 2;
    #[cfg(test)]
    pub const BIND_ON_USE: u8 = 3;
    #[cfg(test)]
    pub const QUEST_ITEM: u8 = 4;
    #[cfg(test)]
    pub const QUEST_ITEM2: u8 = 5;
}

pub(crate) fn binds_on_grant(item_bonding: u8) -> bool {
    item_bonding == bonding::BIND_ON_PICKUP
}

pub(crate) fn binds_on_equip(item_bonding: u8) -> bool {
    item_bonding == bonding::BIND_ON_EQUIP
}

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
    pub const FINGER1: u8 = 10; // FINGER2 is slot 11.
    pub const TRINKET1: u8 = 12; // TRINKET2 is slot 13.
    pub const BACK: u8 = 14;
    pub const MAINHAND: u8 = 15;
    pub const OFFHAND: u8 = 16;
    pub const RANGED: u8 = 17;
    pub const TABARD: u8 = 18;
    pub const END: u8 = TABARD;
}

pub mod invtype {
    #[cfg(test)]
    pub const NON_EQUIP: u8 = 0;
    pub const HEAD: u8 = 1;
    pub const NECK: u8 = 2;
    pub const SHOULDERS: u8 = 3;
    pub const BODY: u8 = 4;
    pub const CHEST: u8 = 5;
    pub const WAIST: u8 = 6;
    pub const LEGS: u8 = 7;
    pub const FEET: u8 = 8;
    pub const WRISTS: u8 = 9;
    pub const HANDS: u8 = 10;
    pub const FINGER: u8 = 11;
    pub const TRINKET: u8 = 12;
    pub const WEAPON: u8 = 13; // One-handed weapon, either hand.
    pub const SHIELD: u8 = 14;
    pub const RANGED: u8 = 15;
    pub const CLOAK: u8 = 16;
    pub const TWO_HAND_WEAPON: u8 = 17;
    pub const BAG: u8 = 18;
    pub const TABARD: u8 = 19;
    pub const ROBE: u8 = 20;
    pub const WEAPON_MAINHAND: u8 = 21;
    pub const WEAPON_OFFHAND: u8 = 22;
    pub const HOLDABLE: u8 = 23;
    pub const THROWN: u8 = 25;
    pub const RANGED_RIGHT: u8 = 26; // Gun or wand.
}

pub(crate) fn inventory_type_to_slot(inventory_type: u8) -> Option<u8> {
    use equip_slot as e;
    use invtype as t;
    Some(match inventory_type {
        t::HEAD => e::HEAD,
        t::NECK => e::NECK,
        t::SHOULDERS => e::SHOULDERS,
        t::BODY => e::SHIRT,
        t::CHEST | t::ROBE => e::CHEST,
        t::WAIST => e::WAIST,
        t::LEGS => e::LEGS,
        t::FEET => e::FEET,
        t::WRISTS => e::WRISTS,
        t::HANDS => e::HANDS,
        t::FINGER => e::FINGER1,
        t::TRINKET => e::TRINKET1,
        t::CLOAK => e::BACK,
        t::WEAPON | t::TWO_HAND_WEAPON | t::WEAPON_MAINHAND => e::MAINHAND,
        t::WEAPON_OFFHAND | t::SHIELD | t::HOLDABLE => e::OFFHAND,
        t::RANGED | t::RANGED_RIGHT | t::THROWN => e::RANGED,
        t::TABARD => e::TABARD,
        _ => return None,
    })
}

pub(crate) fn paired_equip_slot(slot: u8) -> Option<u8> {
    use equip_slot as e;
    match slot {
        x if x == e::FINGER1 => Some(e::FINGER1 + 1),
        x if x == e::TRINKET1 => Some(e::TRINKET1 + 1),
        _ => None,
    }
}

pub(crate) fn can_equip_into(
    class: u8,
    inventory_type: u8,
    dest_slot: u8,
    can_dual_wield: bool,
) -> bool {
    use equip_slot as e;
    if dest_slot == e::MAINHAND {
        return can_equip_mainhand(class, inventory_type);
    }
    if dest_slot == e::OFFHAND && can_dual_wield && class == 2 && inventory_type == invtype::WEAPON
    {
        return true;
    }
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

#[cfg(test)]
pub use lyracore_shared::item::armor_subclass;
pub use lyracore_shared::item::{weapon_subclass, Proficiency};

pub(crate) fn meets_required_level(player_level: u32, required_level: u8) -> bool {
    player_level >= required_level as u32
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EquipStat {
    Strength,
    Agility,
    Stamina,
    Intellect,
    Spirit,
    Health,
    Mana,
    HolyResistance,
    FireResistance,
    NatureResistance,
    FrostResistance,
    ShadowResistance,
    ArcaneResistance,
    Crit,
    Hit,
    Armor,
    WeaponDamage,
    SpellPower,
    HealingPower,
    ManaPerFive,
    HealthPerFive,
    Defense,
    Dodge,
    Parry,
    Block,
}

pub(crate) fn template_stat(tmpl: &ItemTemplate, which: EquipStat) -> i32 {
    match which {
        EquipStat::Strength => tmpl.stat_strength,
        EquipStat::Agility => tmpl.stat_agility,
        EquipStat::Stamina => tmpl.stat_stamina,
        EquipStat::Intellect => tmpl.stat_intellect,
        EquipStat::Spirit => tmpl.stat_spirit,
        EquipStat::HolyResistance => tmpl.holy_res,
        EquipStat::FireResistance => tmpl.fire_res,
        EquipStat::NatureResistance => tmpl.nature_res,
        EquipStat::FrostResistance => tmpl.frost_res,
        EquipStat::ShadowResistance => tmpl.shadow_res,
        EquipStat::ArcaneResistance => tmpl.arcane_res,
        EquipStat::Crit => tmpl.stat_crit,
        EquipStat::Hit => tmpl.stat_hit,
        EquipStat::Armor => tmpl.stat_armor,
        EquipStat::Health
        | EquipStat::Mana
        | EquipStat::WeaponDamage
        | EquipStat::SpellPower
        | EquipStat::HealingPower
        | EquipStat::ManaPerFive
        | EquipStat::HealthPerFive
        | EquipStat::Defense
        | EquipStat::Dodge
        | EquipStat::Parry
        | EquipStat::Block => 0,
    }
}

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

    fn can_equip_proficiency(player_class: u8, item_class: u8, item_subclass: u8) -> bool {
        Proficiency::derive(player_class, true, true).can_equip(item_class, item_subclass)
    }

    #[test]
    fn sell_value_multiplies_by_stack_count() {
        assert_eq!(sell_value(0, 5), 0);
        assert_eq!(sell_value(2, 5), 10);
        assert_eq!(sell_value(u32::MAX, 2), u32::MAX);
    }

    #[test]
    fn buy_cost_multiplies_by_count() {
        assert_eq!(buy_cost(0, 5), 0);
        assert_eq!(buy_cost(35, 2), 70);
        assert_eq!(buy_cost(u32::MAX, 2), u32::MAX);
    }

    #[test]
    fn repair_cost_is_one_copper_per_restored_point() {
        assert_eq!(repair_cost(100, 100), 0);
        assert_eq!(repair_cost(100, 0), 100);
        assert_eq!(repair_cost(100, 60), 40);
        assert_eq!(repair_cost(0, 0), 0);
        assert_eq!(repair_cost(50, 80), 0);
    }

    #[test]
    fn merge_amount_caps_at_destination_headroom() {
        assert_eq!(merge_amount(3, 2, 20), 3);
        assert_eq!(merge_amount(15, 10, 20), 10);
        assert_eq!(merge_amount(5, 20, 20), 0);
        assert_eq!(merge_amount(5, 25, 20), 0);
    }

    #[test]
    fn can_equip_mainhand_only_accepts_mainhand_weapon_types() {
        assert!(can_equip_mainhand(2, 21));
        assert!(can_equip_mainhand(2, 13));
        assert!(can_equip_mainhand(2, 17));
        assert!(!can_equip_mainhand(0, 21));
        assert!(!can_equip_mainhand(4, 21));
        assert!(!can_equip_mainhand(2, 5));
    }

    #[test]
    fn meets_required_level_gates_below_requirement_only() {
        assert!(meets_required_level(1, 1));
        assert!(meets_required_level(5, 1));
        assert!(!meets_required_level(1, 5));
        assert!(meets_required_level(5, 5));
    }

    #[test]
    fn explicit_equip_requirements_fail_closed_and_admit_qualified_characters() {
        assert!(eligibility_mask_allows(1 << 7, 8));
        assert!(!eligibility_mask_allows(1 << 7, 9));
        assert!(!eligibility_mask_allows(0, 8));
        assert!(!eligibility_mask_allows(u32::MAX, 0));

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

        assert!(!meets_required_skill(171, 50, None));
        assert!(!meets_required_skill(171, 50, Some(49)));
        assert!(meets_required_skill(171, 50, Some(50)));
        assert!(meets_required_skill(0, 50, None));

        assert!(!meets_required_reputation(72, 4, None));
        assert!(!meets_required_reputation(72, 4, Some(2_999)));
        assert!(meets_required_reputation(72, 4, Some(3_000)));
        assert!(meets_required_reputation(0, 7, None));
    }

    #[test]
    fn inventory_type_to_slot_maps_each_invtype_to_its_equipment_slot() {
        use equip_slot as e;
        use invtype as t;
        assert_eq!(inventory_type_to_slot(t::HEAD), Some(e::HEAD));
        assert_eq!(inventory_type_to_slot(t::NECK), Some(e::NECK));
        assert_eq!(inventory_type_to_slot(t::SHOULDERS), Some(e::SHOULDERS));
        assert_eq!(inventory_type_to_slot(t::BODY), Some(e::SHIRT));
        assert_eq!(inventory_type_to_slot(t::CHEST), Some(e::CHEST));
        assert_eq!(inventory_type_to_slot(t::ROBE), Some(e::CHEST));
        assert_eq!(inventory_type_to_slot(t::WAIST), Some(e::WAIST));
        assert_eq!(inventory_type_to_slot(t::LEGS), Some(e::LEGS));
        assert_eq!(inventory_type_to_slot(t::FEET), Some(e::FEET));
        assert_eq!(inventory_type_to_slot(t::WRISTS), Some(e::WRISTS));
        assert_eq!(inventory_type_to_slot(t::HANDS), Some(e::HANDS));
        assert_eq!(inventory_type_to_slot(t::FINGER), Some(e::FINGER1));
        assert_eq!(inventory_type_to_slot(t::TRINKET), Some(e::TRINKET1));
        assert_eq!(inventory_type_to_slot(t::CLOAK), Some(e::BACK));
        assert_eq!(inventory_type_to_slot(t::TABARD), Some(e::TABARD));
        assert_eq!(inventory_type_to_slot(t::WEAPON), Some(e::MAINHAND));
        assert_eq!(
            inventory_type_to_slot(t::TWO_HAND_WEAPON),
            Some(e::MAINHAND)
        );
        assert_eq!(
            inventory_type_to_slot(t::WEAPON_MAINHAND),
            Some(e::MAINHAND)
        );
        assert_eq!(inventory_type_to_slot(t::WEAPON_OFFHAND), Some(e::OFFHAND));
        assert_eq!(inventory_type_to_slot(t::SHIELD), Some(e::OFFHAND));
        assert_eq!(inventory_type_to_slot(t::HOLDABLE), Some(e::OFFHAND));
        assert_eq!(inventory_type_to_slot(t::RANGED), Some(e::RANGED));
        assert_eq!(inventory_type_to_slot(t::RANGED_RIGHT), Some(e::RANGED));
        assert_eq!(inventory_type_to_slot(t::THROWN), Some(e::RANGED));
        assert_eq!(inventory_type_to_slot(t::NON_EQUIP), None);
        assert_eq!(inventory_type_to_slot(99), None);
    }

    #[test]
    fn can_equip_into_validates_armor_and_offhand_and_rejects_mismatch() {
        use equip_slot as e;
        use invtype as t;
        assert!(can_equip_into(4, t::HEAD, e::HEAD, false));
        assert!(!can_equip_into(4, t::HEAD, e::CHEST, false));
        assert!(can_equip_into(4, t::SHIELD, e::OFFHAND, false));
        assert!(!can_equip_into(4, t::SHIELD, e::MAINHAND, false));
        assert!(can_equip_into(4, t::ROBE, e::CHEST, false));
        assert!(can_equip_into(4, t::FINGER, e::FINGER1, false));
        assert!(can_equip_into(4, t::FINGER, e::FINGER1 + 1, false));
        assert!(can_equip_into(4, t::TRINKET, e::TRINKET1 + 1, false));
        assert!(!can_equip_into(4, t::FINGER, e::TRINKET1, false));
        assert!(!can_equip_into(0, t::NON_EQUIP, e::CHEST, false));
        assert!(!can_equip_into(4, t::HEAD, 23, false));
    }

    #[test]
    fn can_equip_into_offhand_only_accepts_one_hander_with_dual_wield() {
        use equip_slot as e;
        use invtype as t;
        assert!(!can_equip_into(2, t::WEAPON, e::OFFHAND, false));
        assert!(can_equip_into(2, t::WEAPON, e::OFFHAND, true));
        assert!(!can_equip_into(2, t::TWO_HAND_WEAPON, e::OFFHAND, true));
        assert!(!can_equip_into(2, t::WEAPON_MAINHAND, e::OFFHAND, true));
        assert!(!can_equip_into(4, t::WEAPON, e::OFFHAND, true));
        assert!(can_equip_into(2, t::WEAPON, e::MAINHAND, true));
        assert!(can_equip_into(2, t::WEAPON, e::MAINHAND, false));
    }

    #[test]
    fn paired_equip_slot_only_for_finger_and_trinket() {
        use equip_slot as e;
        assert_eq!(paired_equip_slot(e::FINGER1), Some(e::FINGER1 + 1));
        assert_eq!(paired_equip_slot(e::TRINKET1), Some(e::TRINKET1 + 1));
        assert_eq!(paired_equip_slot(e::HEAD), None);
        assert_eq!(paired_equip_slot(e::MAINHAND), None);
        assert_eq!(paired_equip_slot(e::FINGER1 + 1), None);
    }

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
            random_property: 0,
            spellppmrate_1: 0.0,
            spellppmrate_2: 0.0,
            spellppmrate_3: 0.0,
            spellppmrate_4: 0.0,
            spellppmrate_5: 0.0,
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
        t.holy_res = 1;
        t.fire_res = 2;
        t.nature_res = 3;
        t.frost_res = 4;
        t.shadow_res = 5;
        t.arcane_res = 6;
        assert_eq!(template_stat(&t, EquipStat::Strength), 10);
        assert_eq!(template_stat(&t, EquipStat::Agility), 20);
        assert_eq!(template_stat(&t, EquipStat::Armor), 50);
        assert_eq!(template_stat(&t, EquipStat::Crit), 100);
        assert_eq!(template_stat(&t, EquipStat::Hit), 30);
        assert_eq!(template_stat(&t, EquipStat::HolyResistance), 1);
        assert_eq!(template_stat(&t, EquipStat::FireResistance), 2);
        assert_eq!(template_stat(&t, EquipStat::NatureResistance), 3);
        assert_eq!(template_stat(&t, EquipStat::FrostResistance), 4);
        assert_eq!(template_stat(&t, EquipStat::ShadowResistance), 5);
        assert_eq!(template_stat(&t, EquipStat::ArcaneResistance), 6);
        assert_eq!(template_stat(&t, EquipStat::Stamina), 0);
        assert_eq!(template_stat(&blank_template(25), EquipStat::Strength), 0);
    }

    #[test]
    fn equipped_stats_count_worn_items_and_skip_broken_or_bagged_ones() {
        use lyracore_shared::constants::starter_item::BACKPACK_SLOT_0;
        use lyracore_shared::item_property::{equipped_stat, item_stat};
        let mut chest = blank_template(2001);
        chest.stat_strength = 10;
        chest.max_durability = 80;
        let mut ring = blank_template(2002);
        ring.stat_strength = 5;
        ring.stat_agility = 20;
        ring.stat_armor = 50;
        let mut bracers = blank_template(2003);
        bracers.stat_strength = 7;
        bracers.max_durability = 30;
        let mut belt = blank_template(2004);
        belt.stat_strength = 3;
        let carried = [
            (equip_slot::CHEST, &chest, 1),
            (equip_slot::FINGER1, &ring, 0),
            (equip_slot::WRISTS, &bracers, 0),
            (BACKPACK_SLOT_0, &belt, 0),
        ];
        let total = |which| {
            equipped_stat(carried.iter().map(|&(slot, tmpl, durability)| {
                let amount = item_stat(template_stat(tmpl, which), 0, 0);
                (slot, tmpl.max_durability, durability, amount)
            }))
        };
        assert_eq!(total(EquipStat::Strength), 15);
        assert_eq!(total(EquipStat::Agility), 20);
        assert_eq!(total(EquipStat::Armor), 50);
        assert_eq!(total(EquipStat::Crit), 0);
        assert_eq!(equipped_stat([]), 0);
    }

    #[test]
    fn resolve_equip_slot_picks_first_free_of_a_pair() {
        use equip_slot as e;
        use invtype as t;
        assert_eq!(resolve_equip_slot(t::HEAD, false, |_| false), Some(e::HEAD));
        assert_eq!(resolve_equip_slot(t::HEAD, false, |_| true), Some(e::HEAD));
        assert_eq!(
            resolve_equip_slot(t::FINGER, false, |_| false),
            Some(e::FINGER1)
        );
        assert_eq!(
            resolve_equip_slot(t::FINGER, false, |s| s == e::FINGER1),
            Some(e::FINGER1 + 1)
        );
        assert_eq!(
            resolve_equip_slot(t::FINGER, false, |_| true),
            Some(e::FINGER1)
        );
        assert_eq!(
            resolve_equip_slot(t::TRINKET, false, |s| s == e::TRINKET1),
            Some(e::TRINKET1 + 1)
        );
        assert_eq!(resolve_equip_slot(t::NON_EQUIP, false, |_| false), None);
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
        assert_eq!(
            resolve_equip_slot(t::WEAPON, true, |s| s == e::MAINHAND),
            Some(e::OFFHAND)
        );
        assert_eq!(
            resolve_equip_slot(t::WEAPON, false, |s| s == e::MAINHAND),
            Some(e::MAINHAND)
        );
        assert_eq!(
            resolve_equip_slot(t::WEAPON, true, |_| true),
            Some(e::MAINHAND)
        );
        assert_eq!(
            resolve_equip_slot(t::TWO_HAND_WEAPON, true, |s| s == e::MAINHAND),
            Some(e::MAINHAND)
        );
    }

    #[test]
    fn proficiency_mage_cloth_only() {
        assert!(can_equip_proficiency(8, 4, 0));
        assert!(can_equip_proficiency(8, 4, 1));
        assert!(!can_equip_proficiency(8, 4, 2));
        assert!(!can_equip_proficiency(8, 4, 3));
        assert!(!can_equip_proficiency(8, 4, 4));
        assert!(!can_equip_proficiency(8, 4, 6));
    }

    #[test]
    fn proficiency_warlock_cloth_only() {
        assert!(can_equip_proficiency(9, 4, 0));
        assert!(can_equip_proficiency(9, 4, 1));
        assert!(!can_equip_proficiency(9, 4, 2));
        assert!(!can_equip_proficiency(9, 4, 4));
    }

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

    #[test]
    fn hunters_and_shamans_need_the_trained_passive_for_mail() {
        use armor_subclass as a;
        const ARMOR: u8 = 4;
        for class in [3u8, 7] {
            assert!(!Proficiency::derive(class, false, false).can_equip(ARMOR, a::MAIL));
            assert!(Proficiency::derive(class, false, true).can_equip(ARMOR, a::MAIL));
            assert!(!Proficiency::derive(class, true, true).can_equip(ARMOR, a::PLATE));
        }
    }

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
        assert!(can_equip_proficiency(1, 4, 1));
        assert!(can_equip_proficiency(1, 4, 2));
        assert!(can_equip_proficiency(1, 4, 3));
        assert!(can_equip_proficiency(1, 4, 4));
        assert!(can_equip_proficiency(1, 4, 6));
    }

    #[test]
    fn proficiency_paladin_can_wear_plate() {
        assert!(can_equip_proficiency(2, 4, 4));
        assert!(can_equip_proficiency(2, 4, 6));
    }

    #[test]
    fn proficiency_rogue_leather_only() {
        assert!(can_equip_proficiency(4, 4, 1));
        assert!(can_equip_proficiency(4, 4, 2));
        assert!(!can_equip_proficiency(4, 4, 3));
        assert!(!can_equip_proficiency(4, 4, 4));
        assert!(!can_equip_proficiency(4, 4, 6));
    }

    #[test]
    fn proficiency_druid_leather_only() {
        assert!(can_equip_proficiency(11, 4, 2));
        assert!(!can_equip_proficiency(11, 4, 3));
        assert!(!can_equip_proficiency(11, 4, 6));
    }

    #[test]
    fn proficiency_hunter_leather_and_mail() {
        assert!(can_equip_proficiency(3, 4, 2));
        assert!(can_equip_proficiency(3, 4, 3));
        assert!(!can_equip_proficiency(3, 4, 4));
        assert!(!can_equip_proficiency(3, 4, 6));
    }

    #[test]
    fn proficiency_shaman_leather_mail_shield() {
        assert!(can_equip_proficiency(7, 4, 2));
        assert!(can_equip_proficiency(7, 4, 3));
        assert!(!can_equip_proficiency(7, 4, 4));
        assert!(can_equip_proficiency(7, 4, 6));
    }

    #[test]
    fn proficiency_non_armor_always_allowed() {
        assert!(can_equip_proficiency(8, 0, 0));
        assert!(can_equip_proficiency(8, 7, 0));
        assert!(can_equip_proficiency(9, 12, 0));
    }

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
