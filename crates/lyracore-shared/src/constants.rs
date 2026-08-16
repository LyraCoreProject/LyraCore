//! Game/protocol constants the slice relies on.

/// Object `TypeId` values (1.12). Used as the `object_type_id` byte in update blocks.
pub mod type_id {
    pub const OBJECT: u8 = 0;
    pub const ITEM: u8 = 1;
    pub const CONTAINER: u8 = 2;
    pub const UNIT: u8 = 3;
    pub const PLAYER: u8 = 4;
    pub const GAMEOBJECT: u8 = 5;
    pub const DYNAMICOBJECT: u8 = 6;
    pub const CORPSE: u8 = 7;
}

/// `OBJECT_FIELD_TYPE` is a *mask* of (1 << TypeId) for every type the object is.
/// A player is OBJECT | UNIT | PLAYER.
pub mod type_mask {
    use super::type_id;
    pub const OBJECT: u32 = 1 << type_id::OBJECT;
    pub const UNIT: u32 = 1 << type_id::UNIT;
    pub const PLAYER_BIT: u32 = 1 << type_id::PLAYER;
    /// Value to store in `game_world_entity.type_mask` for a player: 0x19.
    pub const PLAYER: u32 = OBJECT | UNIT | PLAYER_BIT;
    /// Value to store in `game_world_entity.type_mask` for a creature (no PLAYER bit): 0x9.
    /// The codec branches on the PLAYER bit to build a Unit vs Player CREATE_OBJECT.
    pub const CREATURE: u32 = OBJECT | UNIT;
}

/// `UNIT_DYNAMIC_FLAGS` bits (vanilla 1.12.1). These drive client-side render/interaction state that
/// is independent of `UNIT_FIELD_FLAGS`. Stored in `game_world_entity.dynamic_flags` and emitted by
/// the codec at CREATE and via a VALUES relay on change.
pub mod unit_dynamic_flags {
    /// `UNIT_DYNFLAG_LOOTABLE` (0x1). Set on a corpse that has loot for the viewer → the client
    /// shows the loot cursor. A real corpse with no loot carries no dynamic flag at all.
    pub const LOOTABLE: u32 = 0x0001;
    /// `UNIT_DYNFLAG_DEAD` (0x20). **NOT a corpse marker.** In vanilla this is *feign death*: the
    /// client renders the unit lying down but treats it as ALIVE and still attackable (used for
    /// fake-dead ambush mobs and a feigning hunter). A genuinely dead creature is signalled to the
    /// client by `UNIT_FIELD_HEALTH = 0` alone — do NOT set this on a kill (it re-enables the attack
    /// cursor on the corpse). Kept here only to document the bit and prevent its misuse.
    pub const DEAD: u32 = 0x0020;
}

/// `PLAYER_FLAGS` bits (vanilla 1.12, descriptor idx 190). Stored in `game_world_entity.player_flags`.
pub mod player_flags {
    /// `PLAYER_FLAGS_GHOST` (0x10). The gameplay GHOST state set on Release Spirit: the
    /// player is dead-but-walking (can move/run to the corpse, can't act). Distinct from the ghost
    /// *render* — that's `unit_vis_flags::GHOST` in `UNIT_FIELD_BYTES_1`.
    pub const GHOST: u32 = 0x0010;
}

/// `UNIT_NPC_FLAGS` (vanilla 1.12) — what interactions a creature offers; stored in
/// `game_world_entity.npc_flags` and surfaced to the client (gossip-eye / vendor cursor / `!`).
pub mod npc_flags {
    /// `UNIT_NPC_FLAG_GOSSIP` (0x1) — right-click opens a gossip menu.
    pub const GOSSIP: u32 = 0x0000_0001;
    /// `UNIT_NPC_FLAG_VENDOR` (0x4) — sells items (`CMSG_LIST_INVENTORY` → the vendor window). NOTE:
    /// vanilla 1.12 / cmangos-classic numbering — VENDOR is 0x4, not the 0x80 of later cores (where 0x80
    /// is INNKEEPER). Confirmed against the cmangos creature_template (every npc_vendor creature has 0x4).
    pub const VENDOR: u32 = 0x0000_0004;
    /// `UNIT_NPC_FLAG_FLIGHTMASTER` (0x8) — exposes the vanilla taxi service.
    pub const TAXI: u32 = 0x0000_0008;
    /// `UNIT_NPC_FLAG_TRAINER` (0x10) — teaches spells (`CMSG_TRAINER_LIST` → the trainer window). vanilla
    /// 1.12 / cmangos-classic numbering; class trainers (Llane Beshere, Priestess Anetta, …) carry this.
    pub const TRAINER: u32 = 0x0000_0010;
    /// `UNIT_NPC_FLAG_SPIRITHEALER` (0x20, vanilla 1.12 / cmangos-classic) — the graveyard Spirit
    /// Healer that resurrects a GHOST in place at reduced vitals (`CMSG_SPIRIT_HEALER_ACTIVATE`).
    /// Spirit Healer (entry 6491) ships `npc_flags=0x21 = GOSSIP|SPIRITHEALER`; the 5875 client renders
    /// the spirit-healer dialog on a ghost's right-click directly from this replicated flag. NOTE:
    /// DISTINCT from REPAIR (0x4000) — armorers carry that flag, not SPIRITHEALER.
    pub const SPIRITHEALER: u32 = 0x0000_0020;
    /// `UNIT_NPC_FLAG_INNKEEPER` (0x80, vanilla 1.12 / cmangos-classic) — binds the player's hearthstone
    /// home. cmangos 1.12 numbers INNKEEPER 0x80 (NOT the 0x10000 of later TBC+ cores); proven by the
    /// dump's bit histogram (54 innkeepers carry 0x80) and by the 5875 client rendering the inn icon from
    /// the raw flag. Innkeeper Farley(295) ships `npc_flags=135 = GOSSIP|QUESTGIVER|VENDOR|INNKEEPER`. The
    /// Northshire/Goldshire innkeepers carry this; the gossip "Make this inn your home" → `bind_home`.
    pub const INNKEEPER: u32 = 0x0000_0080;
    /// `UNIT_NPC_FLAG_BANKER` (0x100, vanilla 1.12 / cmangos-classic) — opens the bank window and
    /// gates every move into or out of a bank slot.
    pub const BANKER: u32 = 0x0000_0100;
    /// `UNIT_NPC_FLAG_AUCTIONEER` (0x1000, vanilla 1.12) — opens an auction-house window.
    pub const AUCTIONEER: u32 = 0x0000_1000;
    /// `UNIT_NPC_FLAG_REPAIR` (0x4000, vanilla 1.12 / cmangos-classic) — repairs item durability
    /// (`CMSG_REPAIR_ITEM`). Armorers (Corina Steele 54, Quartermaster Hudson 1249, Hicks 1645, …)
    /// carry 0x4000|0x4 (REPAIR+VENDOR). NOTE: 0x1000 (4096) is AUCTIONEER in this numbering, NOT repair.
    pub const REPAIR: u32 = 0x0000_4000;
}

/// GameObject `type_id` values (cmangos `GAMEOBJECT_TYPE_*`). `module/src/gameobject.rs`'s `go_type`
/// module owns the full LIVE dispatch set (CHEST/GOOBER/GATHER/DOOR/BUTTON + docs) and re-exports its
/// `QUESTGIVER` constant from here so it can never drift from the gateway's copy — the gateway (which
/// does not depend on the module crate) needs to recognize a QUESTGIVER GO independently, to route
/// `CMSG_GAMEOBJ_USE` on one to the quest window instead of the loot/toggle reducer path (work-item 041).
pub mod go_type {
    /// `GAMEOBJECT_TYPE_QUESTGIVER` (2) — a GO whose `use` (right-click) opens the quest window
    /// (Wanted Poster GO 68 starts q176; the Lost Guards corpses GO 55/56 drive the q37/q45/q71 END
    /// chain) rather than rolling loot or toggling state.
    pub const QUESTGIVER: u8 = 2;
}

/// `UNIT_FIELD_BYTES_1` byte-3 visibility flags (vanilla 1.12). Drives client render state, stored in
/// `game_world_entity.unit_bytes_1`. The byte-3 position is 1.12-specific (it moved in WotLK).
pub mod unit_vis_flags {
    /// `UNIT_VIS_FLAG_GHOST` — bit value 0x01 in BYTE 3 of `UNIT_FIELD_BYTES_1`, i.e. `0x0100_0000` in
    /// the packed u32. The semi-transparent ghost render. OR this into `unit_bytes_1`; the
    /// codec's `unpack4` → `set_unit_bytes_1(a,b,c,d)` then places it in byte 3 (`d`).
    pub const GHOST: u32 = 0x0100_0000;
}

/// `UNIT_FIELD_BYTES_2` BYTE 0 — the sheath state (vanilla 1.12), stored in
/// `game_world_entity.unit_bytes_2`. Says whether a weapon is DRAWN or STOWED; the per-item
/// `item_template.sheath` byte (a different field, sent in the item query) says WHERE a stowed
/// weapon hangs. Both are needed to render a sheathed weapon correctly. [#101]
pub mod sheath_state {
    /// Nothing drawn — weapons hang in their stow positions. The state every row starts in.
    pub const UNARMED: u8 = 0;
    /// Melee weapon(s) drawn and held.
    pub const MELEE: u8 = 1;
    /// Ranged weapon drawn and held.
    pub const RANGED: u8 = 2;

    /// The client's `CMSG_SETSHEATHED` payload is attacker-controlled: anything outside 0..=2 is a
    /// malformed or hostile packet, and writing it into byte 0 would desync every observer's render.
    pub fn is_valid(state: u8) -> bool {
        matches!(state, UNARMED | MELEE | RANGED)
    }

    /// Replace byte 0 of a packed `unit_bytes_2`, preserving bytes 1-3 (PvP flags, pet flags,
    /// shapeshift form) so a sheath change never clobbers a neighbour that later starts using them.
    pub fn packed_with(unit_bytes_2: u32, state: u8) -> u32 {
        (unit_bytes_2 & 0xFFFF_FF00) | state as u32
    }
}

/// `UNIT_FIELD_FLAGS` bits (vanilla 1.12), stored in `game_world_entity.unit_flags`, sent in the CREATE
/// and relayed as a VALUES change.
pub mod unit_flags {
    /// `UNIT_FLAG_NOT_SELECTABLE` (0x02000000) in the vanilla 1.12 UnitFlags enum. The client does
    /// not offer an interaction cursor for such a unit, and server-side service gates must agree.
    pub const NOT_SELECTABLE: u32 = 0x0200_0000;
    /// `UNIT_FLAG_IN_COMBAT` (0x00080000) — the unit is fighting. Observers render the in-combat state;
    /// set at any hostile action (`combat::enter_combat`), cleared ~`COMBAT_DROP_MS` after the last one
    /// (the tick's combat-drop pass). Covers a pure caster, whom the auto-attack stance can't show.
    pub const IN_COMBAT: u32 = 0x0008_0000;
    /// `UNIT_FLAG_TAXI_FLIGHT` (0x00100000) — the player is under server-controlled flight-path
    /// movement. This is presentation/state, not the route cursor; the authoritative route lives in
    /// the module's active-taxi-flight row.
    pub const TAXI_FLIGHT: u32 = 0x0010_0000;
}

/// Aura+spell tracer — a minimal ADDITIVE spell pipeline to live-test the spell-tier wire format
/// (one castable self-buff). Battle Shout: a real spell every L1 Human Warrior owns, so the client
/// resolves its icon/SFX from its own `Spell.dbc`. The aura renders in slot 0 — the only gtker-public
/// aura slot (slots 1..47 need the deferred hand-rolled update-mask encoder).
pub mod tracer_spell {
    pub const SPELL_ID: u32 = 6673; // Battle Shout (Rank 1)
    pub const AURA_SLOT: u8 = 0; // default first slot; a new distinct aura takes the lowest free slot
    pub const AURA_SLOTS: u8 = 48; // total UNIT_FIELD_AURA slots (the raw-send path reaches them all)
    /// The 1.12 client sections `UNIT_FIELD_AURA` by SLOT INDEX, not by an aura flag: slots `0..32` render
    /// as the buff frame, `32..48` as the debuff frame. `pick_aura_slot` must keep buffs below this and
    /// debuffs at/above it, or a debuff shows in the buff row (and can starve buff slots).
    pub const BUFF_SLOT_COUNT: u8 = 32;
    pub const AURA_FLAGS: u8 = 0x09; // positive + effect-index-0 (cmangos AFLAG); try 0x1F if invisible
    pub const AURA_DURATION_MICROS: i64 = 30_000_000; // server-side expiry (client shows the DBC timer)
}

/// The hand-authored starter weapon a fresh character carries in its first backpack
/// slot. Entry 25 is the real vanilla L1-Warrior "Worn Shortsword", so the client resolves its icon
/// from its own data once we answer `CMSG_ITEM_QUERY_SINGLE`. Values are hand-authored (licensing
/// firewall: derived, not bulk-imported).
pub mod starter_item {
    pub const ENTRY: u32 = 25; // "Worn Shortsword"
    /// `HIGHGUID_ITEM` — tag in bits 48..63 of an item GUID so the client routes it as an item
    /// (cmangos `HIGHGUID_ITEM = 0x4000`).
    pub const HIGHGUID_ITEM: u64 = 0x4000;
    /// `INVENTORY_SLOT_ITEM_START` (23) — the first backpack slot. Vanilla inventory layout:
    /// equipment 0..18, equipped-bag slots 19..22, backpack item slots 23..38. Stored as the
    /// instance's `slot` and passed to gtker's typed `set_player_field_inv(ItemSlot, Guid)`.
    pub const BACKPACK_SLOT_0: u8 = 23;
    /// `EQUIPMENT_SLOT_MAINHAND` (15) — the main-hand weapon slot. A L1 Warrior starts with
    /// the Worn Shortsword EQUIPPED here (vanilla-authentic), so the client renders it on the 3D model
    /// via `PLAYER_VISIBLE_ITEM` (set for equipment slots 0..18 in addition to the inv-slot guid).
    pub const MAINHAND_SLOT: u8 = 15;
    pub const CLASS_WEAPON: u8 = 2;
    pub const SUBCLASS_SWORD_1H: u8 = 7;
    pub const QUALITY_POOR: u8 = 0;
    pub const INVTYPE_WEAPON_MAINHAND: u8 = 21;
    pub const ITEM_LEVEL: u8 = 2;
    pub const REQUIRED_LEVEL: u8 = 1;
    pub const MAX_DURABILITY: u32 = 20;
    pub const BUY_PRICE: u32 = 35; // copper
    pub const SELL_PRICE: u32 = 7; // copper
    pub const DISPLAY_ID: u32 = 1542; // a basic one-hand-sword inventory model in the 5875 client
    pub const DAMAGE_MIN: f32 = 1.0;
    pub const DAMAGE_MAX: f32 = 3.0;
    pub const DELAY_MS: u32 = 1900;

    /// Hearthstone (item entry 6948) — every character starts with one; using it recalls to the bound
    /// home (`Character::home_*`). Granted into the backpack by `grant_starter_item`.
    pub const HEARTHSTONE_ENTRY: u32 = 6948;
    /// Backpack slot the starter Hearthstone lands in — the LAST backpack slot, so it never collides with
    /// the per-class outfit which stows from `BACKPACK_SLOT_0` (23) upward.
    pub const HEARTHSTONE_SLOT: u8 = 38;
}

/// Dual Wield (spell 674, `IDS_ROGUE`) — the talent/ability that lets a character equip a second
/// one-hand weapon into the off-hand slot and swing it as a reduced second attack stream. A L10 Rogue
/// can learn it in vanilla; Warrior Dual Wield (level 20) is out of scope for this constant's consumer
/// (`combat::equipped_offhand_weapon_damage` / `items::rules::resolve_equip_slot`/`can_equip_into`).
pub mod dual_wield {
    pub const SPELL_ID: u32 = 674;
}

/// The 6 vanilla movement speeds emitted in a CREATE block's movement section.
/// (Flight speeds were added in TBC and are absent here.)
pub mod speeds {
    pub const WALK: f32 = 2.5;
    pub const RUN: f32 = 7.0;
    /// Warrior Charge rush speed — the caster spline-rushes far faster than a normal run (mangos
    /// `MoveCharge` uses ~25 yd/s). Tuning knob for the charge feel; not a movement-cap for anything else.
    pub const CHARGE: f32 = 25.0;
    pub const RUN_BACK: f32 = 4.5;
    pub const SWIM: f32 = 4.722_222;
    pub const SWIM_BACK: f32 = 2.5;
    pub const TURN_RATE: f32 = std::f32::consts::PI;
}

/// Canonical Human/Warrior starting fixture (Elwynn Forest). Confirm the exact float Z
/// against a 1.12 `playercreateinfo` when seeding — a wrong Z drops the character.
pub mod start_human_warrior {
    pub const RACE: u8 = 1; // Human
    pub const CLASS: u8 = 1; // Warrior
    pub const MAP_ID: u32 = 0; // Eastern Kingdoms
    pub const ZONE_ID: u32 = 12; // Elwynn Forest
    pub const X: f32 = -8949.95;
    pub const Y: f32 = -132.493;
    pub const Z: f32 = 83.5312;
    pub const ORIENTATION: f32 = 0.0;
}

/// Vanilla 5875 represents known taxi nodes as eight 32-bit words. Node ids are one-based bit
/// positions in that fixed mask, so a value outside this range cannot cross the protocol intact.
pub mod taxi_protocol {
    pub const NODE_MASK_WORDS: u32 = 8;
    pub const CLIENT_NODE_ID_MIN: u32 = 1;
    pub const CLIENT_NODE_ID_MAX: u32 = NODE_MASK_WORDS * u32::BITS;
    pub const REPLY_STATUS: u8 = 1;
    pub const REPLY_OPEN: u8 = 2;
    pub const REPLY_ACTIVATE: u8 = 3;

    // Stable module-to-gateway activation results. They deliberately match the vanilla
    // ActivateTaxiReply discriminants, but remain shared primitive constants so the wasm module
    // does not depend on the wire-codec crate and the gateway never has to parse refusal prose.
    pub const ACTIVATE_OK: u8 = 0;
    pub const ACTIVATE_UNSPECIFIED_SERVER_ERROR: u8 = 1;
    pub const ACTIVATE_NO_SUCH_PATH: u8 = 2;
    pub const ACTIVATE_NOT_ENOUGH_MONEY: u8 = 3;
    pub const ACTIVATE_TOO_FAR_AWAY: u8 = 4;
    pub const ACTIVATE_NO_VENDOR_NEARBY: u8 = 5;
    pub const ACTIVATE_NOT_VISITED: u8 = 6;
    pub const ACTIVATE_PLAYER_BUSY: u8 = 7;
    pub const ACTIVATE_PLAYER_ALREADY_MOUNTED: u8 = 8;
    pub const ACTIVATE_PLAYER_SHAPE_SHIFTED: u8 = 9;
    pub const ACTIVATE_PLAYER_MOVING: u8 = 10;
    pub const ACTIVATE_SAME_NODE: u8 = 11;
    pub const ACTIVATE_NOT_STANDING: u8 = 12;
}

/// Reserved, hand-authored taxi catalogue used by the headless flight flow. Its storage ids sit
/// beside the existing 509xxxx scenario rows so a DBC import can wholesale-replace its derived
/// catalogue and then restore this route without colliding with client-derived primary keys. The
/// separate client ids fit the vanilla protocol mask and are protected by a unique table index.
///
/// These values are shared by the native importer and the wasm module. Keeping the two writers on
/// one definition prevents a fresh database and a post-import database from growing different
/// versions of the fixture.
pub mod taxi_fixture {
    pub const STORAGE_ID_FLOOR: u32 = 5_090_000;

    pub const SOURCE_NODE_STORAGE_ID: u32 = 5_090_100;
    pub const DESTINATION_NODE_STORAGE_ID: u32 = 5_090_101;
    pub const PATH_ID: u32 = 5_090_102;
    pub const POINT_IDS: [u32; 3] = [5_090_103, 5_090_104, 5_090_105];

    // These synthetic wire ids deliberately exercise the final two bits of the fixed 256-bit mask.
    // They are useful to headless protocol tests, but do not create matching map art in a real client.
    pub const SOURCE_CLIENT_NODE_ID: u32 = super::taxi_protocol::CLIENT_NODE_ID_MAX - 1;
    pub const DESTINATION_CLIENT_NODE_ID: u32 = super::taxi_protocol::CLIENT_NODE_ID_MAX;

    pub const MAP_ID: u32 = super::start_human_warrior::MAP_ID;
    pub const SOURCE_X: f32 = super::start_human_warrior::X + 5.0;
    pub const SOURCE_Y: f32 = super::start_human_warrior::Y - 5.0;
    pub const SOURCE_Z: f32 = super::start_human_warrior::Z;
    pub const DESTINATION_X: f32 = SOURCE_X + 60.0;
    pub const DESTINATION_Y: f32 = SOURCE_Y;
    pub const DESTINATION_Z: f32 = SOURCE_Z;
    pub const MIDPOINT_X: f32 = SOURCE_X + 30.0;
    pub const MIDPOINT_Y: f32 = SOURCE_Y;
    pub const MIDPOINT_Z: f32 = SOURCE_Z + 12.0;

    pub const SOURCE_NAME: &str = "Northshire Test Flight";
    pub const DESTINATION_NAME: &str = "Northshire Test Landing";
    pub const FARE: u32 = 25;
    // Build-5875 display ids used only by the synthetic fixture. Imported nodes retain the exact
    // Horde-first / Alliance-second pair from the operator's TaxiNodes.dbc.
    pub const MOUNT_DISPLAY_HORDE: u32 = 295;
    pub const MOUNT_DISPLAY_ALLIANCE: u32 = 1147;

    pub const FLIGHT_MASTER_ENTRY: u32 = 51_006;
    pub const FLIGHT_MASTER_GUID: u64 =
        (0xF130_u64 << 48) | ((FLIGHT_MASTER_ENTRY as u64) << 24) | 1;
}

/// Gossip menu OPTION (work-item 217): `game_gossip_option.action` codes as they land verbatim from
/// the cmangos dump's `gossip_menu_option.OptionType`/`option_id` column (the importer copies it
/// through unchanged — this module documents what the values MEAN so the gateway dispatch and the
/// importer agree without duplicating magic numbers). `[V]` — confirm against your own dump; only
/// GOSSIP/BANKER/VENDOR/TAXI/TRAINER/INNKEEPER are read by the dispatcher today, the rest are
/// inert (submenu navigation remains deferred).
pub mod gossip_option {
    pub const GOSSIP: u32 = 1; // plain text / submenu link (submenu navigation deferred, work-item 217)
    /// Quests reach the window through its QUEST section, never an option row, so the importer drops
    /// these — the dump's rows carry the literal placeholder label "GOSSIP_OPTION_QUESTGIVER".
    pub const QUESTGIVER: u32 = 2;
    pub const VENDOR: u32 = 3; // opens the vendor window (routes to build_list_inventory_raw)
    pub const TAXI: u32 = 4; // flight master (system 136, not wired — inert)
    pub const TRAINER: u32 = 5; // opens SMSG_TRAINER_LIST
    pub const INNKEEPER: u32 = 8; // binds the caller's hearthstone home (bind_home)
    pub const BANKER: u32 = 9; // opens the bank window
    /// cmangos `GOSSIP_OPTION_UNLEARNTALENTS`. NOT what the raw dump carries — every "I wish to
    /// unlearn my talents." row imports with `action=GOSSIP` (cmangos gates it in C++ code at
    /// GossipHello, not via this column), so the importer reclassifies that specific row's text to
    /// this action at import time (`resolve_gossip_option_text`'s caller in `importer/src/main.rs`).
    /// Gated to level 10+ (talents don't exist below that) by `filtered_gossip_options` — #516.
    pub const UNLEARNTALENTS: u32 = 16;
}

/// Minimum level a talent point exists at (mirrors vanilla's `PLAYER_LEVEL_MIN_TALENTS` — the client
/// hides the talent pane below this). Gates the "I wish to unlearn my talents." gossip option.
pub const MIN_TALENT_LEVEL: u8 = 10;

/// `game_gossip_option.cond_type` — the MINIMAL condition set work-item 217 enforces (quest-status
/// gates, the common case in the dump). Anything the importer can't map to one of these folds to
/// `NONE` (fail-open + logged), so an unsupported condition never wrongly HIDES an option. The one
/// exception is `NEVER`, for a gate whose subject does not exist here at all.
pub mod gossip_condition {
    /// Always show the option (no gate, or an unsupported/unmapped condition — fail-open).
    pub const NONE: u32 = 0;
    /// Show only while `cond_value1` (a quest id) is in the player's quest log (accepted, whether or
    /// not yet turned in) — `quest_status(guid, quest_id).0`.
    pub const QUEST_TAKEN: u32 = 1;
    /// Show only once `cond_value1` (a quest id) has been turned in — `quest_status(guid, quest_id).1`.
    pub const QUEST_REWARDED: u32 = 2;
    /// Never show. The fail-CLOSED placeholder for a condition whose subject does not exist here yet
    /// — a seasonal event gate folded to `NONE` would pitch Children's Week in July.
    pub const NEVER: u32 = 3;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_type_mask_is_0x19() {
        assert_eq!(type_mask::PLAYER, 0x19);
    }

    /// `packed_with` must touch BYTE 0 only. Byte 1 is PvP flags, byte 2 pet flags, byte 3 the
    /// shapeshift form — a sheath toggle that cleared a druid's form byte would pop them out of Bear
    /// on every `Z` press, which is exactly the class of bug a blind `= state as u32` would ship. [#101]
    #[test]
    fn packing_a_sheath_state_preserves_the_other_three_bytes() {
        let neighbours = 0xAB_CD_EF_00_u32; // bytes 1-3 occupied, byte 0 clear
        assert_eq!(
            sheath_state::packed_with(neighbours, sheath_state::MELEE),
            0xAB_CD_EF_01
        );
        // And going back to stowed clears byte 0 without disturbing them.
        assert_eq!(
            sheath_state::packed_with(0xAB_CD_EF_02, sheath_state::UNARMED),
            0xAB_CD_EF_00
        );
    }

    /// The `CMSG_SETSHEATHED` payload is attacker-controlled; only 0/1/2 are real states.
    #[test]
    fn sheath_state_validation_rejects_out_of_range_bytes() {
        for ok in [
            sheath_state::UNARMED,
            sheath_state::MELEE,
            sheath_state::RANGED,
        ] {
            assert!(sheath_state::is_valid(ok), "{ok} is a real sheath state");
        }
        for bad in [3u8, 4, 17, 255] {
            assert!(!sheath_state::is_valid(bad), "{bad} must be refused");
        }
    }

    #[test]
    fn creature_type_mask_is_0x9() {
        // OBJECT | UNIT, no PLAYER bit — the codec branches on the PLAYER bit to pick Unit vs Player.
        assert_eq!(type_mask::CREATURE, 0x9);
    }

    #[test]
    fn vanilla_auction_vocabulary_uses_the_build_5875_values() {
        assert_eq!(npc_flags::AUCTIONEER, 0x1000);
    }

    /// Work-item 041: pin the shared GameObject QUESTGIVER type id against cmangos — this is the
    /// SINGLE source both `module/src/gameobject.rs::go_type::QUESTGIVER` and the gateway's
    /// `CMSG_GAMEOBJ_USE` dispatch read, so a silent edit here would desync both sides at once.
    #[test]
    fn go_type_questgiver_is_cmangos_type_2() {
        assert_eq!(go_type::QUESTGIVER, 2);
    }

    /// Work-item 217: the gossip option action codes the dispatcher matches on must be pairwise
    /// distinct — a collision here would silently misroute one action to another's handler.
    #[test]
    fn gossip_option_actions_are_distinct() {
        use gossip_option::*;
        let mut codes = [GOSSIP, QUESTGIVER, BANKER, VENDOR, TAXI, TRAINER, INNKEEPER];
        codes.sort_unstable();
        assert_eq!(
            codes.windows(2).filter(|w| w[0] == w[1]).count(),
            0,
            "gossip_option action codes must not collide"
        );
        // The cmangos `GossipOptionType` numbering.
        assert_eq!(
            (GOSSIP, QUESTGIVER, VENDOR, TAXI, TRAINER, INNKEEPER, BANKER),
            (1, 2, 3, 4, 5, 8, 9)
        );
    }

    #[test]
    fn gossip_condition_none_is_the_fail_open_zero_value() {
        // `NONE == 0` is load-bearing: the importer's fail-open path for an unmapped condition writes
        // `cond_type = 0` and relies on it meaning "always show" (never a wrongly-hidden option).
        assert_eq!(gossip_condition::NONE, 0);
        assert_ne!(
            gossip_condition::QUEST_TAKEN,
            gossip_condition::QUEST_REWARDED
        );
    }
}
