//! Item wire mapping: the `CMSG_ITEM_QUERY_SINGLE` reply and the item CREATE_OBJECT (items
//! slice-1), plus the flattened template/instance row views. Pure code-motion out of `mod.rs`.

use super::*;

/// An item-template row as the gateway reads it from `game_item_template`, flattened for the
/// `CMSG_ITEM_QUERY_SINGLE` reply + the item CREATE (items slice-1). Decoupled from the SDK row
/// type. `class`/`subclass` together pick the typed `ItemClassAndSubClass`; the rest map 1:1.
#[derive(Clone, Debug, Default)]
pub struct ItemTemplateView {
    pub entry: u32,
    pub class: u8,
    pub subclass: u8,
    pub name: String,
    pub display_id: u32,
    pub quality: u8,
    pub inventory_type: u8,
    pub item_level: u8,
    pub required_level: u8,
    pub max_durability: u32,
    pub buy_price: u32,
    pub sell_price: u32,
    pub max_stack: u32,
    pub damage_min: f32,
    pub damage_max: f32,
    pub delay_ms: u32,
    // equip stat bonuses (mirrored from module ItemTemplate)
    pub stat_strength: i32,
    pub stat_agility: i32,
    pub stat_stamina: i32,
    pub stat_intellect: i32,
    pub stat_spirit: i32,
    pub stat_armor: i32,  // bonus armor for the tooltip
    pub block_value: i32, // shield block value (0 for non-shields)
    // On-use/on-equip spell slots 1-2 (id + ItemSpellTriggerType) — drive the client green "Use:" text.
    pub spellid_1: u32,
    pub spelltrigger_1: u8,
    pub spellid_2: u32,
    pub spelltrigger_2: u8,
    // NOTE: no `container_slots` here. The bag CREATE reads it off `ItemInstanceView` (below), which
    // gets it from its own `game_item_template` lookup at read/subscription time — a template mirror
    // on this struct would be a second, never-read copy of the same column.
    /// Item binding: the cmangos `bonding` value (0=NoBind,1=BoP,2=BoE,3=BoU,
    /// 4/5=QuestItem), sent as-is into `SMSG_ITEM_QUERY_SINGLE_RESPONSE.bonding` so the client renders
    /// "Binds when picked up/equipped". Trade/mail enforcement lives in the trade-window/mailbox
    /// systems — this is query-only.
    pub bonding: u8,
    /// Sheathe posture: the raw cmangos `item_template.sheath` byte — where the weapon stows
    /// visually (Worn Shortsword=3 → hip; shields/2H on the back). Passed THROUGH to the wire (cmangos
    /// sends `pProto->Sheath` verbatim; gtker's SheatheType variant NAMES don't track the client's
    /// actual postures — values rule).
    pub sheath: u8,
    // The dropped item_template columns, mirrored straight from the module's
    // `ItemTemplate` (no consumer system reads these server-side yet — this view carries them ONLY
    // so `build_item_query_response` can fill in the wire fields it already writes hardcoded 0/default
    // for — the free gateway win).
    pub holy_res: i32,
    pub fire_res: i32,
    pub nature_res: i32,
    pub frost_res: i32,
    pub shadow_res: i32,
    pub arcane_res: i32,
    pub spellid_3: u32,
    pub spelltrigger_3: u8,
    pub spellid_4: u32,
    pub spelltrigger_4: u8,
    pub spellid_5: u32,
    pub spelltrigger_5: u8,
    pub required_skill: u32,
    pub required_skill_rank: u32,
    pub required_reputation_faction: u32,
    pub required_reputation_rank: u32,
    pub max_count: u32,
    pub item_flags: u32,
    pub page_text: u32,
    pub start_quest: u32,
    pub bag_family: u32,
}

/// An owned-item instance as the gateway reads it from `game_item_instance` (items slice-1),
/// joined with its template for the CREATE descriptors. Decoupled from the SDK row type.
#[derive(Clone, Debug, Default)]
pub struct ItemInstanceView {
    pub guid: u64,
    pub entry: u32,
    pub owner_guid: u64, // the owning character — ITEM_FIELD_OWNER / ITEM_FIELD_CONTAINED
    pub slot: u8,        // the inventory slot it occupies (ItemSlot ordinal)
    pub stack_count: u32,
    pub durability: u32,
    pub max_durability: u32,
    /// Number of container slots (0 = regular item, > 0 = bag). Drives the CREATE_OBJECT choice:
    /// bags are sent as `ObjectType::Container` with `CONTAINER_FIELD_NUM_SLOTS` set so the client
    /// opens the bag window. Non-bags use `ObjectType::Item` as before (baseline-safe).
    pub container_slots: u8,
}

/// Build `SMSG_ITEM_QUERY_SINGLE_RESPONSE` so the client caches the item's name/tooltip/icon (the
/// item analogue of the creature query). The 1.12 client only queries an item it has *encountered*
/// (i.e. has an object for), so this is what makes `GetItemInfo`/the bag tooltip resolve a real name
/// instead of staying blank. The response struct derives `Default`, so we set only the load-bearing
/// fields (class/subclass, name, icon display, quality, prices, equip type, level, the weapon's
/// damage + speed, sheathe, durability) and leave the ~40 cosmetic fields at their zero defaults.
pub fn build_item_query_response(
    item: u32,
    t: Option<&ItemTemplateView>,
) -> SMSG_ITEM_QUERY_SINGLE_RESPONSE {
    // Unknown entry → reply `found: None` (NotFound) so the client stops re-asking, matching a real
    // server's miss (the creature query just logs + drops, but an item miss would re-query forever).
    let Some(t) = t else {
        return SMSG_ITEM_QUERY_SINGLE_RESPONSE { item, found: None };
    };
    // ItemClassAndSubClass packs subclass in the high u32, class in the low u32 (e.g. a one-hand
    // sword = (7 << 32) | 2 = 0x7_0000_0002). An unknown pairing degrades to the enum's default
    // rather than failing the query (the worst case is a wrong category line on the tooltip).
    let class_and_sub_class =
        ItemClassAndSubClass::try_from((u64::from(t.subclass) << 32) | u64::from(t.class))
            .unwrap_or_default();
    // Build a compact stats array: pack non-zero stats into the first N slots (max 10), pad the
    // rest with (Mana, 0) which the 1.12 client skips when rendering the tooltip. The 7 stat columns
    // the module stores map directly to 5 attribute stats + crit/hit (which don't show as "+N Stat"
    // lines in vanilla — skip them to avoid confusing the tooltip). stat_armor feeds the `armor`
    // field instead.
    let stats: [ItemStat; 10] = {
        let pairs: [(ItemStatType, i32); 5] = [
            (ItemStatType::Strength, t.stat_strength),
            (ItemStatType::Agility, t.stat_agility),
            (ItemStatType::Stamina, t.stat_stamina),
            (ItemStatType::Intellect, t.stat_intellect),
            (ItemStatType::Spirit, t.stat_spirit),
        ];
        let mut out = [ItemStat {
            stat_type: ItemStatType::Mana,
            value: 0,
        }; 10];
        let mut slot = 0usize;
        for (ty, val) in pairs {
            if val != 0 && slot < 10 {
                out[slot] = ItemStat {
                    stat_type: ty,
                    value: val,
                };
                slot += 1;
            }
        }
        out
    };
    // On-use/on-equip spell slots → the client renders the green "Use:/Equip:/Chance on hit:" text from
    // its OWN Spell.dbc. All 5 slots are populated from the template (slots 3-5 were wired
    // in later — previously hardcoded empty). spell_trigger maps the raw ItemSpellTriggerType byte
    // (0=on-use, 1=on-equip, 2=chance-on-hit).
    let spell_slot = |id: u32, trig: u8| ItemSpells {
        spell: id,
        spell_trigger: SpellTriggerType::try_from(u32::from(trig)).unwrap_or_default(),
        ..Default::default()
    };
    let mut spells = [ItemSpells::default(); 5];
    spells[0] = spell_slot(t.spellid_1, t.spelltrigger_1);
    spells[1] = spell_slot(t.spellid_2, t.spelltrigger_2);
    spells[2] = spell_slot(t.spellid_3, t.spelltrigger_3);
    spells[3] = spell_slot(t.spellid_4, t.spelltrigger_4);
    spells[4] = spell_slot(t.spellid_5, t.spelltrigger_5);
    SMSG_ITEM_QUERY_SINGLE_RESPONSE {
        item: t.entry,
        found: Some(SMSG_ITEM_QUERY_SINGLE_RESPONSE_found {
            class_and_sub_class,
            name1: t.name.clone(),
            display_id: t.display_id,
            quality: ItemQuality::try_from(u32::from(t.quality)).unwrap_or_default(),
            // The raw item_template.Flags bitmask (unique/conjured/etc) — was
            // hardcoded to the wire default (empty) before the column existed server-side.
            flags: ItemFlag::new(t.item_flags),
            buy_price: Gold::new(t.buy_price),
            sell_price: Gold::new(t.sell_price),
            inventory_type: InventoryType::try_from(u32::from(t.inventory_type))
                .unwrap_or_default(),
            item_level: Level::new(t.item_level),
            required_level: Level::new(t.required_level),
            // Weapon-skill / mail-plate proficiency gate. An out-of-range skill id
            // degrades to Skill::None rather than failing the query (matches the other try_from
            // degrades in this function).
            required_skill: Skill::try_from(t.required_skill).unwrap_or_default(),
            required_skill_rank: t.required_skill_rank,
            // The reputation-gated-item half of faction reaction gating.
            required_faction: Faction::try_from(t.required_reputation_faction).unwrap_or_default(),
            required_faction_rank: t.required_reputation_rank,
            stackable: t.max_stack.max(1),
            // Unique-item stack cap (was hardcoded 0 before the column existed
            // server-side).
            max_count: t.max_count,
            stats,
            armor: t.stat_armor,
            // block carries the shield's base block value (0 for every non-shield); u32 cast is
            // safe because every seeded block_value is ≥ 0 (i32 future-proofs enchant negatives).
            block: t.block_value.max(0) as u32,
            // The weapon's white-damage range + swing speed (drives the tooltip "N - M Damage" and
            // "Speed X.XX"); slot 0 physical, the rest left empty.
            damages: {
                let mut d = [ItemDamageType::default(); 5];
                d[0] = ItemDamageType {
                    damage_minimum: t.damage_min,
                    damage_maximum: t.damage_max,
                    school: SpellSchool::Normal,
                };
                d
            },
            // The 6 resistance schools — resist gear tooltips were dead (always 0)
            // until these columns existed server-side.
            holy_resistance: t.holy_res,
            fire_resistance: t.fire_res,
            nature_resistance: t.nature_res,
            frost_resistance: t.frost_res,
            shadow_resistance: t.shadow_res,
            arcane_resistance: t.arcane_res,
            delay: t.delay_ms,
            // The per-item sheathe posture (was a blanket MainHand → every weapon/shield stowed
            // in the same spot). An out-of-range byte degrades to None rather than failing the query.
            sheathe_type: SheatheType::try_from(t.sheath).unwrap_or_default(),
            spells,
            // Item binding: the raw cmangos byte maps directly onto the wire enum
            // (NoBind=0, PickUp=1 "Binds when picked up", Equip=2 "Binds when equipped", Use=3,
            // QuestItem=4/5) — an out-of-range byte degrades to NoBind rather than failing the query.
            bonding: Bonding::try_from(u32::from(t.bonding)).unwrap_or_default(),
            // Readable-item page id (needs its own page_text dump table + reader
            // packet before a player can actually read one — data plumbing only for now) and the
            // quest-starter link (consumed by the quest system).
            page_text: t.page_text,
            start_quest: t.start_quest,
            max_durability: t.max_durability,
            // Bag-type restriction bitmask (Soul Bag/Quiver/etc). An out-of-range
            // value degrades to BagFamily::None rather than failing the query.
            bag_family: BagFamily::try_from(t.bag_family).unwrap_or_default(),
            // Ranged weapon RANGE multiplier (mangos `RangedModRange`, % of base). The client scales a
            // ranged attack's spell range (Auto Shot = 35yd) by THIS — a `Default::default()` of 0.0
            // collapsed the effective range to 0, so Auto Shot read "out of range" at EVERY distance. The
            // mangos column defaults to 100.0 for ALL items (only ranged weapons consult it); we don't
            // import per-item overrides, so send the universal vanilla default. [hunter Auto Shot fix]
            ranged_range_modification: 100.0,
            ..Default::default()
        }),
    }
}

/// Build a generic `SMSG_INVENTORY_CHANGE_FAILURE` — the error popup for a rejected inventory
/// action (equip, move, store, or use). The gateway doesn't have the specific reason at this
/// level (the module surfaced only an opaque `Err`), so we use `ItemCantBeEquipped` with zero
/// item guids. Any non-OK result triggers the client's error sound and prevents the snap-back
/// from looking like success. Specific codes (with the actual item guids and cause) require
/// the module to surface them via a structured error; this is the gateway-only baseline.
pub fn build_inventory_change_failure() -> SMSG_INVENTORY_CHANGE_FAILURE {
    SMSG_INVENTORY_CHANGE_FAILURE::ItemCantBeEquipped {
        item1: Guid::new(0),
        item2: Guid::new(0),
        bag_type_subclass: 0,
    }
}

/// Build `SMSG_BUY_FAILED` — the red on-screen error for a rejected vendor purchase.
/// Maps the module's Err string to the closest `BuyResult` code (displayed as a toast by the
/// 1.12 client). `vendor_guid` is the NPC, `item_entry` is the `c.item` from the client packet.
pub fn build_buy_failed(vendor_guid: u64, item_entry: u32, err: &str) -> SMSG_BUY_FAILED {
    let result = if err.contains("not enough money") {
        BuyResult::NotEnoughMoney
    } else if err.contains("out of range") || err.contains("another map") {
        BuyResult::DistanceTooFar
    } else if err.contains("inventory full") {
        BuyResult::CantCarryMore
    } else if err.contains("sold out") || err.contains("no stock") {
        BuyResult::ItemSoldOut
    } else {
        // "vendor does not sell that item", "item cannot be bought", "dead", etc. — generic.
        BuyResult::CantFindItem
    };
    SMSG_BUY_FAILED {
        guid: Guid::new(vendor_guid),
        item: item_entry,
        result,
    }
}

/// Build the item or container CREATE_OBJECT (items slice-1 / bag extension). A non-spatial object:
/// NO living/position movement block — just `UPDATEFLAG_ALL`. Branches on `inst.container_slots`:
/// - Regular items (0): `ObjectType::Item` + `UpdateMask::Item` (baseline-safe, byte-identical).
/// - Bags (> 0): `ObjectType::Container` + `UpdateMask::Container` with `CONTAINER_FIELD_NUM_SLOTS`
///   set so the client shows a bag window with the correct slot count. `ITEM_FIELD_OWNER` and
///   `ITEM_FIELD_CONTAINED` both point at the owning player. Sent before the player self-spawn.
pub fn build_item_create_object(inst: &ItemInstanceView) -> SMSG_UPDATE_OBJECT {
    let owner = Guid::new(inst.owner_guid);
    let update_flag =
        MovementBlock_UpdateFlag::empty().set_all(MovementBlock_UpdateFlag_All { unknown1: 1 });
    let guid3 = Guid::new(inst.guid);
    if inst.container_slots > 0 {
        // This item is a bag — send a CONTAINER CREATE so the client shows the bag window.
        let container = UpdateContainer::builder()
            .set_object_guid(guid3)
            .set_object_entry(inst.entry as i32)
            .set_object_scale_x(1.0)
            .set_item_owner(owner)
            .set_item_contained(owner)
            .set_item_stack_count(inst.stack_count.max(1) as i32)
            .set_item_durability(inst.durability as i32)
            .set_item_maxdurability(inst.max_durability as i32)
            .set_container_num_slots(inst.container_slots as i32)
            .finalize();
        SMSG_UPDATE_OBJECT {
            has_transport: 0,
            objects: vec![Object::CreateObject2 {
                guid3,
                object_type: ObjectType::Container,
                movement2: MovementBlock { update_flag },
                mask2: UpdateMask::Container(container),
            }],
        }
    } else {
        // Regular item — `ObjectType::Item`, byte-identical to the pre-bag path.
        let item = UpdateItem::builder()
            .set_object_guid(guid3)
            .set_object_entry(inst.entry as i32)
            .set_object_scale_x(1.0)
            .set_item_owner(owner)
            .set_item_contained(owner)
            .set_item_stack_count(inst.stack_count.max(1) as i32)
            .set_item_durability(inst.durability as i32)
            .set_item_maxdurability(inst.max_durability as i32)
            .finalize();
        SMSG_UPDATE_OBJECT {
            has_transport: 0,
            objects: vec![Object::CreateObject2 {
                guid3,
                object_type: ObjectType::Item,
                movement2: MovementBlock { update_flag },
                mask2: UpdateMask::Item(item),
            }],
        }
    }
}

use wow_world_messages::vanilla::{
    NewItemChatAlert, NewItemCreationType, NewItemSource, SMSG_ITEM_PUSH_RESULT,
};

/// `SMSG_ITEM_PUSH_RESULT` — the "You receive item: [X]." chat/toast for a
/// windowless gain, and the client's trigger to re-check watched-quest item objectives (the
/// tracker + "Wolf Meat: 3/8" floaty — 1.12 recomputes those from its bags when this arrives).
/// `stack_add` sends item_slot 0xFFFFFFFF (the mangoszero added-to-stack convention). Source is
/// always Looted/Received — the relay can't see WHERE the row came from (loot/buy/conjure);
/// "You receive item" reads fine for all three. Deliberate simplification: thread a source hint
/// through when this grates.
pub fn build_item_push_result(
    player_guid: u64,
    bag_slot: u8,
    item_slot: u32,
    entry: u32,
    count: u32,
    stack_add: bool,
) -> SMSG_ITEM_PUSH_RESULT {
    SMSG_ITEM_PUSH_RESULT {
        guid: Guid::new(player_guid),
        source: NewItemSource::Looted,
        creation_type: NewItemCreationType::Received,
        alert_chat: NewItemChatAlert::Show,
        bag_slot,
        item_slot: if stack_add { 0xFFFF_FFFF } else { item_slot },
        item: entry,
        item_suffix_factor: 0,
        item_random_property_id: 0,
        item_count: count,
    }
}
