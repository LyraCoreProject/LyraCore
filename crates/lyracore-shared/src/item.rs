//! Item rules shared by the module and gateway.

pub mod armor_subclass {
    pub const MISC: u8 = 0;
    pub const CLOTH: u8 = 1;
    pub const LEATHER: u8 = 2;
    pub const MAIL: u8 = 3;
    pub const PLATE: u8 = 4;
    pub const SHIELD: u8 = 6;
}

pub mod weapon_subclass {
    pub const AXE_1H: u8 = 0;
    pub const AXE_2H: u8 = 1;
    pub const BOW: u8 = 2;
    pub const GUN: u8 = 3;
    pub const MACE_1H: u8 = 4;
    pub const MACE_2H: u8 = 5;
    pub const POLEARM: u8 = 6;
    pub const SWORD_1H: u8 = 7;
    pub const SWORD_2H: u8 = 8;
    pub const STAFF: u8 = 10;
    pub const FIST: u8 = 13;
    pub const DAGGER: u8 = 15;
    pub const THROWN: u8 = 16;
    pub const CROSSBOW: u8 = 18;
    pub const WAND: u8 = 19;
    pub const FISHING_POLE: u8 = 20;
}

/// Whether a class may equip an item class/subclass under the existing vanilla proficiency model.
pub fn can_equip_proficiency(player_class: u8, item_class: u8, item_subclass: u8) -> bool {
    match item_class {
        4 => armor_proficiency(player_class, item_subclass),
        2 => weapon_proficiency(player_class, item_subclass),
        _ => true,
    }
}

fn armor_proficiency(player_class: u8, subclass: u8) -> bool {
    use armor_subclass::*;

    if matches!(subclass, MISC | CLOTH) {
        return true;
    }
    match player_class {
        1 | 2 => matches!(subclass, LEATHER | MAIL | PLATE | SHIELD),
        3 => matches!(subclass, LEATHER | MAIL),
        4 | 11 => subclass == LEATHER,
        7 => matches!(subclass, LEATHER | MAIL | SHIELD),
        5 | 8 | 9 => false,
        _ => false,
    }
}

fn weapon_proficiency(player_class: u8, subclass: u8) -> bool {
    use weapon_subclass::*;

    if subclass == FISHING_POLE {
        return true;
    }
    match player_class {
        1 => subclass != WAND,
        2 => matches!(
            subclass,
            AXE_1H | AXE_2H | MACE_1H | MACE_2H | POLEARM | SWORD_1H | SWORD_2H
        ),
        3 => matches!(
            subclass,
            AXE_1H
                | AXE_2H
                | BOW
                | CROSSBOW
                | DAGGER
                | FIST
                | GUN
                | POLEARM
                | STAFF
                | SWORD_1H
                | SWORD_2H
                | THROWN
        ),
        4 => matches!(
            subclass,
            BOW | CROSSBOW | DAGGER | FIST | GUN | MACE_1H | SWORD_1H | THROWN
        ),
        5 => matches!(subclass, DAGGER | MACE_1H | STAFF | WAND),
        7 => matches!(
            subclass,
            AXE_1H | AXE_2H | DAGGER | FIST | MACE_1H | MACE_2H | STAFF
        ),
        8 | 9 => matches!(subclass, DAGGER | STAFF | SWORD_1H | WAND),
        11 => matches!(subclass, DAGGER | FIST | MACE_1H | MACE_2H | STAFF),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_class_proficiencies_are_available_to_gateway_reads() {
        assert!(!can_equip_proficiency(8, 4, 4));
        assert!(can_equip_proficiency(1, 4, 4));
        assert!(can_equip_proficiency(8, 2, 19));
        assert!(!can_equip_proficiency(1, 2, 19));
        assert!(can_equip_proficiency(8, 7, 0));
    }
}
