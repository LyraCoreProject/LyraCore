use wow_world_messages::vanilla::{
    CreatureFamily, GossipItem, NpcTextUpdate, QuestItem, SMSG_CREATURE_QUERY_RESPONSE_found,
    SMSG_CREATURE_QUERY_RESPONSE, SMSG_GOSSIP_MESSAGE, SMSG_NPC_TEXT_UPDATE,
};
use wow_world_messages::Guid;

#[derive(Clone, Debug, Default)]
pub struct CreatureView {
    pub entry: u32,
    pub name: String,
    pub subname: String,
    pub display_id: u32,
    pub creature_type: u32,
    pub creature_family: u8,
    pub type_flags: u32,
    pub rank: u32,
}

pub fn build_creature_query_response(c: &CreatureView) -> SMSG_CREATURE_QUERY_RESPONSE {
    SMSG_CREATURE_QUERY_RESPONSE {
        creature_entry: c.entry,
        found: Some(SMSG_CREATURE_QUERY_RESPONSE_found {
            name1: c.name.clone(),
            name2: String::new(),
            name3: String::new(),
            name4: String::new(),
            sub_name: c.subname.clone(),
            type_flags: c.type_flags,
            creature_type: c.creature_type,
            creature_family: CreatureFamily::try_from(c.creature_family).unwrap_or_default(),
            creature_rank: c.rank,
            unknown0: 0,
            spell_data_id: 0,
            display_id: c.display_id,
            civilian: 0,
            racial_leader: 0,
        }),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipMenuView {
    /// Zero selects the entry-owned menu; any other value names a runtime menu profile.
    pub source_menu_id: u32,
    pub options: Vec<GossipOptionView>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GossipOptionView {
    pub row_id: u32,
    pub icon: u32,
    pub text: String,
    pub action: u32,
    pub action_menu_id: u32,
    pub cond_type: u32,
    pub cond_value1: u32,
    pub cond_value2: u32,
}

/// Synthesized options have no durable row; Package hooks must not treat this as a row id.
pub const SYNTHESIZED_ROW_ID: u32 = u32::MAX;

pub fn gossip_menu_options(
    imported: Vec<GossipOptionView>,
    is_vendor: bool,
    is_innkeeper: bool,
) -> Vec<GossipOptionView> {
    use lyracore_shared::constants::gossip_option;
    let mut opts = imported;
    let mut synthesize = |action: u32, icon: u32, text: &str| {
        if opts.iter().any(|o| o.action == action) {
            return;
        }
        opts.push(GossipOptionView {
            row_id: SYNTHESIZED_ROW_ID,
            icon,
            text: text.to_string(),
            action,
            ..GossipOptionView::default()
        });
    };
    if is_vendor {
        synthesize(gossip_option::VENDOR, 1, "I'd like to browse your goods.");
    }
    if is_innkeeper {
        synthesize(gossip_option::INNKEEPER, 0, "Make this inn your home.");
    }
    opts
}

pub const GOSSIP_GREETING_TEXT_ID: u32 = 1;

const GOSSIP_GREETING: &str = "Greetings, traveler. I have nothing for you at this time.";

pub fn build_gossip_message(
    npc_guid: u64,
    title_text_id: u32,
    quests: Vec<QuestItem>,
    options: &[GossipOptionView],
) -> SMSG_GOSSIP_MESSAGE {
    // The client echoes the option's list position; resolve it against this snapshot.
    let gossips: Vec<GossipItem> = options
        .iter()
        .enumerate()
        .map(|(i, o)| GossipItem {
            id: i as u32,
            item_icon: o.icon as u8,
            coded: false,
            message: o.text.clone(),
        })
        .collect();
    SMSG_GOSSIP_MESSAGE {
        guid: Guid::new(npc_guid),
        title_text_id,
        gossips,
        quests,
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NpcTextView {
    pub slots: [(String, String, f32); 8],
}

pub fn build_npc_text_update(text_id: u32, view: Option<&NpcTextView>) -> SMSG_NPC_TEXT_UPDATE {
    let texts: [NpcTextUpdate; 8] = match view {
        Some(v) => core::array::from_fn(|i| {
            let (male, female, prob) = &v.slots[i];
            NpcTextUpdate {
                probability: *prob,
                texts: [male.clone(), female.clone()],
                ..NpcTextUpdate::default()
            }
        }),
        None => {
            let mut texts: [NpcTextUpdate; 8] = core::array::from_fn(|_| NpcTextUpdate::default());
            texts[0] = NpcTextUpdate {
                probability: 1.0,
                texts: [GOSSIP_GREETING.to_string(), GOSSIP_GREETING.to_string()],
                ..NpcTextUpdate::default()
            };
            texts
        }
    };
    SMSG_NPC_TEXT_UPDATE { text_id, texts }
}

pub const SMSG_LIST_INVENTORY_OPCODE: u16 = 0x019F;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VendorItemView {
    pub item_entry: u32,
    pub display_id: u32,
    pub buy_price: u32,
    pub max_durability: u32,
    pub available_count: u32, // u32::MAX means unlimited; zero means sold out.
    pub buy_count: u32,
}

/// gtker's vendor records include ExtendedCost, a u32 absent from build 5875.
pub fn build_list_inventory_raw(vendor_guid: u64, items: &[VendorItemView]) -> (u16, Vec<u8>) {
    let items = &items[..items.len().min(255)];
    let mut body = Vec::with_capacity(9 + items.len() * 28);
    body.extend_from_slice(&vendor_guid.to_le_bytes());
    body.push(items.len() as u8);
    if items.is_empty() {
        // An empty vendor list still needs an error byte; zero means no items.
        body.push(0);
        return (SMSG_LIST_INVENTORY_OPCODE, body);
    }
    for (i, it) in items.iter().enumerate() {
        body.extend_from_slice(&((i as u32) + 1).to_le_bytes());
        body.extend_from_slice(&it.item_entry.to_le_bytes());
        body.extend_from_slice(&it.display_id.to_le_bytes());
        body.extend_from_slice(&it.available_count.to_le_bytes());
        body.extend_from_slice(&it.buy_price.to_le_bytes());
        body.extend_from_slice(&it.max_durability.to_le_bytes());
        body.extend_from_slice(&it.buy_count.max(1).to_le_bytes());
    }
    (SMSG_LIST_INVENTORY_OPCODE, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::{GossipItem, Level, QuestItem, SMSG_GOSSIP_MESSAGE};
    use wow_world_messages::Guid;

    #[test]
    fn gossip_message_echoes_guid_with_a_greeting_option() {
        let menu =
            |is_vendor, is_innkeeper| gossip_menu_options(Vec::new(), is_vendor, is_innkeeper);
        let m = build_gossip_message(
            0xF130_0000_0000_0001,
            GOSSIP_GREETING_TEXT_ID,
            Vec::new(),
            &menu(false, false),
        );
        assert_eq!(m.guid.guid(), 0xF130_0000_0000_0001);
        assert_eq!(m.title_text_id, GOSSIP_GREETING_TEXT_ID);
        const {
            assert!(
                GOSSIP_GREETING_TEXT_ID <= 0xFF_FFFF,
                "title_text_id must be a low, in-range npc_text id"
            )
        };
        assert!(
            m.gossips.is_empty(),
            "an NPC with no options has a greeting-only window"
        );
        let v = build_gossip_message(
            0xF130_0000_0000_0001,
            GOSSIP_GREETING_TEXT_ID,
            Vec::new(),
            &menu(true, false),
        );
        assert_eq!(v.gossips.len(), 1);
        assert_eq!(v.gossips[0].id, 0);
        assert_eq!(v.gossips[0].item_icon, 1);
        assert!(!v.gossips[0].coded);
        assert!(m.quests.is_empty());

        use lyracore_shared::constants::npc_flags;
        assert_eq!(
            npc_flags::INNKEEPER,
            0x80,
            "cmangos 1.12 INNKEEPER is 0x80, not 0x10000"
        );
        const {
            assert!(
                135 & npc_flags::INNKEEPER != 0,
                "Farley(295) npc_flags=135 must gate as an innkeeper"
            )
        };
        assert_eq!(
            135 & npc_flags::VENDOR,
            0x4,
            "the VENDOR (0x4) bit still passes — no regression"
        );

        let inn = build_gossip_message(
            0xF130_0000_0000_0001,
            GOSSIP_GREETING_TEXT_ID,
            Vec::new(),
            &menu(false, true),
        );
        assert_eq!(inn.gossips[0].message, "Make this inn your home.");
        assert_eq!(inn.gossips[0].id, 0);
        let both = build_gossip_message(
            0xF130_0000_0000_0001,
            GOSSIP_GREETING_TEXT_ID,
            Vec::new(),
            &menu(true, true),
        );
        assert_eq!(both.gossips.len(), 2);
        assert_eq!(both.gossips[0].message, "I'd like to browse your goods.");
        assert_eq!(both.gossips[1].message, "Make this inn your home.");
        assert_eq!(both.gossips[1].id, 1);
    }

    #[test]
    fn gossip_message_renders_imported_options_verbatim() {
        use lyracore_shared::constants::gossip_option;
        let opt = |icon, text: &str, action| GossipOptionView {
            icon,
            text: text.into(),
            action,
            ..Default::default()
        };
        let opts = vec![
            opt(0, "Care for a chat?", gossip_option::GOSSIP),
            opt(1, "Let me see your goods.", gossip_option::VENDOR),
            opt(
                0,
                "I'd like to stay here a while.",
                gossip_option::INNKEEPER,
            ),
        ];
        let m = build_gossip_message(
            1,
            GOSSIP_GREETING_TEXT_ID,
            Vec::new(),
            &gossip_menu_options(opts, true, true),
        );
        assert_eq!(m.gossips.len(), 3);
        assert_eq!(m.gossips[0].message, "Care for a chat?");
        assert_eq!(m.gossips[0].id, 0);
        assert_eq!(m.gossips[1].message, "Let me see your goods.");
        assert_eq!(m.gossips[1].item_icon, 1);
        assert_eq!(m.gossips[2].message, "I'd like to stay here a while.");
        assert_eq!(m.gossips[2].id, 2);
    }

    #[test]
    fn gossip_menu_options_appends_only_the_missing_flag_lines() {
        use lyracore_shared::constants::gossip_option;
        let chat = GossipOptionView {
            row_id: 7,
            text: "Care for a chat?".into(),
            action: gossip_option::GOSSIP,
            ..Default::default()
        };
        let merged = gossip_menu_options(vec![chat.clone()], true, false);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].row_id, 7, "the imported row keeps its identity");
        assert_eq!(merged[1].text, "I'd like to browse your goods.");
        assert_eq!(merged[1].action, gossip_option::VENDOR);
        assert_eq!(merged[1].row_id, SYNTHESIZED_ROW_ID);
        let merged = gossip_menu_options(vec![chat.clone()], false, true);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].action, gossip_option::INNKEEPER);
        let has_vendor = GossipOptionView {
            row_id: 9,
            text: "Let me see your goods.".into(),
            action: gossip_option::VENDOR,
            ..Default::default()
        };
        let merged = gossip_menu_options(vec![chat, has_vendor], true, false);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].text, "Let me see your goods.");
    }

    #[test]
    fn gossip_message_body_is_exact_5875_layout() {
        let guid = 0xF130_0000_0000_0001u64;
        let msg = SMSG_GOSSIP_MESSAGE {
            guid: Guid::new(guid),
            title_text_id: GOSSIP_GREETING_TEXT_ID,
            gossips: vec![GossipItem {
                id: 0,
                item_icon: 1,
                coded: false,
                message: "Hi".to_string(),
            }],
            quests: vec![QuestItem {
                quest_id: 0x29A,
                quest_icon: 7,
                level: Level::new(3),
                title: "Q".to_string(),
            }],
        };
        let mut framed = Vec::new();
        ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(Box::new(msg))
            .write_unencrypted_server(&mut framed)
            .unwrap();
        assert_eq!(
            u16::from_le_bytes([framed[2], framed[3]]),
            0x017D,
            "opcode == SMSG_GOSSIP_MESSAGE"
        );
        let body = &framed[4..];

        let mut want: Vec<u8> = Vec::new();
        want.extend_from_slice(&guid.to_le_bytes());
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(&0u32.to_le_bytes());
        want.push(1u8);
        want.push(0u8);
        want.extend_from_slice(b"Hi\0");
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(&0x29Au32.to_le_bytes());
        want.extend_from_slice(&7u32.to_le_bytes());
        want.extend_from_slice(&3u32.to_le_bytes());
        want.extend_from_slice(b"Q\0");

        assert_eq!(
            body,
            want.as_slice(),
            "gossip body must be the exact 5875 layout\n got {:02x?}\nwant {:02x?}",
            body,
            want
        );
    }

    #[test]
    fn npc_text_update_carries_the_greeting_in_slot_0() {
        let t = build_npc_text_update(GOSSIP_GREETING_TEXT_ID, None);
        assert_eq!(t.text_id, GOSSIP_GREETING_TEXT_ID);
        assert_eq!(t.texts.len(), 8);
        assert!(t.texts[0].texts[0].contains("Greetings"));
        assert_eq!(t.texts[0].probability, 1.0);
        assert_eq!(t.texts[1].probability, 0.0);
        let mut view = NpcTextView::default();
        view.slots[0] = (
            "Hey, citizen!".to_string(),
            "Hey, citizen!".to_string(),
            1.0,
        );
        let t2 = build_npc_text_update(4938, Some(&view));
        assert_eq!(t2.text_id, 4938);
        assert_eq!(t2.texts[0].texts[0], "Hey, citizen!");
        assert_eq!(t2.texts[0].probability, 1.0);
    }

    #[test]
    fn npc_text_update_ships_every_populated_slot_with_its_own_probability() {
        let mut view = NpcTextView::default();
        view.slots[0] = (
            "Well met.".to_string(),
            "Well met, traveler.".to_string(),
            0.6,
        );
        view.slots[3] = (
            "Watch yourself.".to_string(),
            "Watch yourself.".to_string(),
            0.4,
        );
        let t = build_npc_text_update(77, Some(&view));
        assert_eq!(
            t.texts[0].texts,
            ["Well met.".to_string(), "Well met, traveler.".to_string()]
        );
        assert_eq!(t.texts[0].probability, 0.6);
        assert_eq!(
            t.texts[3].texts,
            ["Watch yourself.".to_string(), "Watch yourself.".to_string()]
        );
        assert_eq!(t.texts[3].probability, 0.4);
        assert_eq!(t.texts[1].probability, 0.0);
        assert!(t.texts[1].texts[0].is_empty());
    }

    #[test]
    fn list_inventory_raw_body_is_exact_vanilla_layout() {
        let vendor_guid = 0xF130_0000_0000_0001u64;
        let item = VendorItemView {
            item_entry: 25,
            display_id: 1542,
            buy_price: 35,
            max_durability: 20,
            available_count: u32::MAX,
            buy_count: 1,
        };
        let (opcode, body) = build_list_inventory_raw(vendor_guid, &[item]);
        assert_eq!(opcode, 0x019F, "SMSG_LIST_INVENTORY opcode (vanilla 5875)");
        assert_eq!(opcode, SMSG_LIST_INVENTORY_OPCODE);

        let expected: Vec<u8> = vec![
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0xF1, 0x01, 0x01, 0x00, 0x00, 0x00, 0x19,
            0x00, 0x00, 0x00, 0x06, 0x06, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x23, 0x00, 0x00,
            0x00, 0x14, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        ];
        assert_eq!(
            body, expected,
            "raw SMSG_LIST_INVENTORY body must match the vanilla layout byte-for-byte"
        );

        let (_op, empty) = build_list_inventory_raw(vendor_guid, &[]);
        assert_eq!(
            empty,
            vec![0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0xF1, 0x00, 0x00],
            "empty vendor body is guid + count 0 + error byte"
        );
    }

    #[test]
    fn list_inventory_raw_truncates_stock_to_255_items() {
        let items: Vec<VendorItemView> = (0..300u32)
            .map(|i| VendorItemView {
                item_entry: i,
                display_id: 1,
                buy_price: 1,
                max_durability: 1,
                available_count: 1,
                buy_count: 1,
            })
            .collect();
        let (_op, body) = build_list_inventory_raw(1, &items);
        assert_eq!(body[8], 255, "the count byte must cap at 255");
        assert_eq!(
            body.len(),
            9 + 255 * 28,
            "envelope(9) + 255 items * 28 bytes each"
        );
    }

    #[test]
    fn creature_query_response_maps_names_and_flags_and_serializes() {
        let c = CreatureView {
            entry: 6741,
            name: "Innkeeper Farley".to_string(),
            subname: "Innkeeper".to_string(),
            display_id: 2068,
            creature_type: 7,
            creature_family: 0,
            type_flags: 0x8,
            rank: 0,
        };
        let msg = build_creature_query_response(&c);
        assert_eq!(msg.creature_entry, 6741);
        {
            let found = msg.found.as_ref().expect("a real template must be found");
            assert_eq!(found.name1, "Innkeeper Farley");
            assert_eq!(found.sub_name, "Innkeeper");
            assert_eq!(found.display_id, 2068);
            assert_eq!(found.creature_type, 7);
            assert_eq!(found.type_flags, 0x8);
        }
        let mut buf = Vec::new();
        ServerOpcodeMessage::SMSG_CREATURE_QUERY_RESPONSE(Box::new(msg))
            .write_unencrypted_server(&mut buf)
            .unwrap();
        assert!(!buf.is_empty());
    }
}
