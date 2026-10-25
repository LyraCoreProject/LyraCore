use super::*;

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
    pub max_count: u32,
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
        // The wire uses 0xFFFF_FFFF for unlimited stock.
        let max_count = if it.max_count == 0 {
            0xFFFF_FFFF
        } else {
            it.max_count
        };
        body.extend_from_slice(&max_count.to_le_bytes());
        body.extend_from_slice(&it.buy_price.to_le_bytes());
        body.extend_from_slice(&it.max_durability.to_le_bytes());
        body.extend_from_slice(&it.buy_count.max(1).to_le_bytes());
    }
    (SMSG_LIST_INVENTORY_OPCODE, body)
}
