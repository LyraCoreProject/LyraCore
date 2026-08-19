//! Vanilla-data importer. Reads the operator's OWN inputs — a cmangos `classic-db` dump (never
//! executed) and/or their own legally-owned 1.12.1 client `Data/` archives — maps them to our
//! `game_*` schema, and loads via SQL clear+reload (see docs/data-ingestion.md for the strategy
//! and the licensing firewall this implements: we ship the tools, the operator supplies the data,
//! neither ships in the repo). The live `tick_creatures` respawn pass turns imported spawns into
//! world entities, so we never hand-encode the entity row.
//!
//! One binary, six modes (pick by flag; the ones that write honor --db/--server/--apply,
//! defaulting to a dry run that prints the plan and writes nothing):
//!   --dump <classic-db .sql[.gz]>   cmangos creature/etc ETL
//!                                   [--world-profile alliance-eastern|alliance-kalimdor|
//!                                    alliance-single|instances]
//!                                   [advanced: --map 0 --center X,Y,Z --radius 180]
//!                                   [--print-extents derives --box from the dump's real spawn
//!                                    extents; always a dry run]
//!   --dbc  <client Data/ dir>       client DBC extraction/checks (dbc.rs); add --spells for the
//!                                   Spell.dbc importer (spell.rs) or --talents for the
//!                                   TalentTab.dbc/Talent.dbc importer (talent.rs)
//!   --terrain <client Data/ dir>    ADT heightmap stream (terrain.rs)
//!   --nav <client Data/ dir>        WMO/M2 nav-grid rasterizer (nav.rs)
//!   --vmap <client Data/ dir>       exact per-cell collision-triangle extract + pack + import
//!                                   (--apply loads `game_vmap_chunk` via import_vmap_chunks; a
//!                                   dry run stops at report — vmap.rs; #520/#521,
//!                                   docs/decisions.md §10)
//!   --vmap-prepare-coverage <id>    derive path-grid coverage for an already-staged vmap
//!                                   generation (--apply calls prepare_/finalize_vmap_nav_coverage,
//!                                   resumable; a dry run reports the plan) — vmap.rs::run_coverage
//!   --go-models <client Data/ dir>  DOOR/BUTTON display → M2 bounding-mesh extract + import,
//!                                   needs --dump too (--apply loads `game_go_model` via
//!                                   import_go_models; a dry run stops at report — go_model.rs)
//!   --dump-collision <client Data/ dir>  WMO/M2 collision-geometry inspector — dry-run only,
//!                                   no --apply (collision.rs)
//!   --pack-client <client Data/ dir>  builds the client patch MPQ + installs addons into the
//!                                   OPERATOR's OWN client (pack_client.rs); standalone, writes
//!                                   to the client dir, not a `game_*` table
//! e.g.  importer --dump .import/classic-db-full.sql --apply
//!       importer --dbc ../wowclient/Data
//!       importer --dbc ../wowclient/Data --talents --apply
//!       importer --dump .import/classic-db-full.sql --map 0 --print-extents

mod collision;
mod dbc;
mod eventai;
mod go_model;
mod nav;
mod pack_client;
mod spell;
mod talent;
mod terrain;
mod vmap;
mod world_import_scope;

use std::collections::HashMap;
use std::io::Read;
use std::process::Command;

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use world_import_scope::{WorldImportProfile, WorldImportScope};

// Kept in step with `module::items::tables`: the importer cannot depend on the wasm Module, but
// it owns the ClassicDB `-1` sentinel conversion that makes these values durable.
const ALL_PLAYABLE_CLASS_MASK: u32 = 0x1ff;
const ALL_PLAYABLE_RACE_MASK: u32 = 0xff;

// --- cmangos column indices (verified against ClassicDB_1_12_1_z2815 schema) -------------------
mod ct {
    // creature_template
    pub const ENTRY: usize = 0;
    pub const NAME: usize = 1;
    pub const SUBNAME: usize = 2;
    pub const MIN_LEVEL: usize = 3;
    pub const MAX_LEVEL: usize = 4;
    pub const MODEL_ID1: usize = 5;
    pub const FACTION: usize = 9;
    pub const SCALE: usize = 10;
    pub const FAMILY: usize = 11;
    pub const CREATURE_TYPE: usize = 12;
    pub const NPC_FLAGS: usize = 16;
    pub const UNIT_FLAGS: usize = 17;
    pub const CREATURE_TYPE_FLAGS: usize = 20;
    pub const RANK: usize = 33;
    pub const MIN_LEVEL_HEALTH: usize = 40;
    pub const MAX_LEVEL_HEALTH: usize = 41;
    pub const MIN_MELEE_DMG: usize = 44; // float; per-creature swing damage (parity #7)
    pub const MAX_MELEE_DMG: usize = 45;
    pub const ARMOR: usize = 48; // → game_creature_template.armor (0 = unmitigated, module fallback)
    pub const MELEE_BASE_ATTACK_TIME: usize = 51;
    pub const MIN_LOOT_GOLD: usize = 54;
    pub const MAX_LOOT_GOLD: usize = 55;
    pub const LOOT_ID: usize = 56; // → creature_loot_template.entry (0 = no loot table)
                                   // Loot-family completeness (work-item 210): PickpocketLootId/SkinLootId sit IMMEDIATELY after LootId
                                   // (LootId, PickpocketLootId, SkinningLootId are consecutive columns). These two carried a `[V]`
                                   // "confirm against your own dump" caveat until the pinned dump's DDL was enumerated directly —
                                   // both are now confirmed, along with every other const here.
    pub const PICKPOCKET_LOOT_ID: usize = 57; // → pickpocketing_loot_template.entry (0 = no table)
    pub const SKIN_LOOT_ID: usize = 58; // → skinning_loot_template.entry (0 = no table)
                                        // Trainer service block: TrainerType, TrainerSpell, TrainerClass, TrainerRace, TrainerTemplateId
                                        // run 71..75, bracketed by the verified `SKIN_LOOT_ID` and `GOSSIP_MENU_ID` anchors. Enumerated
                                        // from the pinned dump's own CREATE TABLE — mangos-classic master has since gained
                                        // DisplayIdProbability1-4 and five stat multipliers, so its order is 4 columns out here and 9 at
                                        // `LootId`. TrainerSpell/TrainerRace/TrainerTemplateId are not ingested: no consumer.
    pub const TRAINER_TYPE: usize = 71; // cmangos enum: CLASS 0 · MOUNTS 1 · TRADESKILLS 2 · PETS 3
    pub const TRAINER_CLASS: usize = 73; // class ID (1..11), NOT a mask; 0 = serves every class
    pub const GOSSIP_MENU_ID: usize = 77; // creature_template.GossipMenuId
}
mod gm {
    // gossip_menu: entry(=creature_template.GossipMenuId), text_id, script_id, condition_id
    pub const ENTRY: usize = 0;
    pub const TEXT_ID: usize = 1;
    pub const CONDITION_ID: usize = 3;
}
mod npct {
    // npc_text (full 81-col format): ID, text0_0, text0_1, lang0, prob0, em0_0..5, text1_0, ...
    pub const ID: usize = 0;
    pub const NCOLS_FULL: usize = 81; // 1 + 8*(2+1+1+6) = 81 (old schema with embedded strings)
                                      // New cmangos schema packs each slot as a single BroadcastTextId. 17 cols: ID + 8 probs + 8 ids.
    pub const NCOLS_SHORT: usize = 17; // ID, Prob0..7, BroadcastTextId0..7
    /// Full-schema per-slot column formula (work-item 217, all 8 slots not just the first): each slot
    /// occupies 10 columns starting at `1 + slot*10` — male, female, lang, prob, em0..em5.
    pub const fn full_slot_base(slot: usize) -> usize {
        1 + slot * 10
    }
    pub const FULL_SLOT_MALE_OFFSET: usize = 0;
    pub const FULL_SLOT_FEMALE_OFFSET: usize = 1;
    pub const FULL_SLOT_PROB_OFFSET: usize = 3;
    /// Short-schema per-slot column formula: prob at `1+slot`, BroadcastTextId at `9+slot`.
    pub const fn short_slot_prob(slot: usize) -> usize {
        1 + slot
    }
    pub const fn short_slot_bcast_id(slot: usize) -> usize {
        9 + slot
    }
}
mod bt {
    // broadcast_text: Id, Text (male), Text1 (female), ...
    pub const ID: usize = 0;
    pub const TEXT: usize = 1; // male greeting text
    pub const TEXT1: usize = 2; // female greeting text
}
mod gmo {
    // gossip_menu_option — DUMP-VERIFIED 2026-07-10 (the [V] guess had OPTION_TYPE at 4, which is
    // option_broadcast_text in this dump — every imported option's `action` was a broadcast_text id
    // like 5299, so the gateway's TRAINER/VENDOR action match never fired and selecting "train me"
    // just closed the window; live find). Real 15-col layout, read off the dump's CREATE TABLE:
    //   menu_id, id, option_icon, option_text, option_broadcast_text, option_id,
    //   npc_option_npcflag, action_menu_id, action_poi_id, action_script_id,
    //   box_coded, box_money, box_text, box_broadcast_text, condition_id
    pub const MENU_ID: usize = 0;
    pub const ID: usize = 1; // dense ordinal within the menu (cmangos row order — importer re-indexes)
    pub const OPTION_ICON: usize = 2;
    pub const OPTION_TEXT: usize = 3; // literal text, OR (new-schema dumps) a broadcast_text id — both tried
    pub const OPTION_TYPE: usize = 5; // option_id/action — see `lyracore_shared::constants::gossip_option`
    pub const ACTION_MENU_ID: usize = 7;
    /// THIS dump's vintage: 15 cols, trailing generic-`conditions` FK id.
    pub const NCOLS_15_CONDITION_FK: usize = 15;
    pub const CONDITION_ID_15: usize = 14;
    /// Legacy vintage fallbacks (other dumps): trailing FK at 10 (11 cols) / inline cond at 11..12 (14 cols).
    pub const NCOLS_WITH_CONDITION_FK: usize = 11;
    pub const CONDITION_ID: usize = 10;
    pub const NCOLS_WITH_INLINE_COND: usize = 14;
    pub const INLINE_COND_TYPE: usize = 11;
    pub const INLINE_COND_VALUE1: usize = 12;
}
mod cond {
    // conditions (mangos/cmangos generic condition table): condition_entry, type, value1, value2.
    // [V] confirm — mangos `ConditionType` (subset this importer maps to the minimal 217 condition
    // set): 8 = CONDITION_QUESTREWARDED, 9 = CONDITION_QUESTTAKEN. Anything else fails open
    // (`lyracore_shared::constants::gossip_condition::NONE`) rather than silently hiding an option.
    pub const ENTRY: usize = 0;
    pub const TYPE: usize = 1;
    pub const VALUE1: usize = 2;
    pub const MANGOS_QUESTREWARDED: u64 = 8;
    pub const MANGOS_QUESTTAKEN: u64 = 9;
    /// `CONDITION_ACTIVE_GAME_EVENT` [V]. No game-event calendar exists here, so this gate must fail
    /// CLOSED — fail-open pitches Children's Week in July.
    pub const MANGOS_ACTIVE_GAME_EVENT: u64 = 12;
}
mod cts {
    // creature_template_spells — ClassicDB_1_12_1_z2815: entry, setId, spell1..spell10
    // (PK (entry,setId)). The modern cmangos dump moved the caster-mob rotation OFF creature_template's
    // old spell1-4 columns into this table; spell1 is the primary nuke for every Elwynn caster
    // (476 Kobold Geomancer → 133 Fireball; 474 Defias Rogue Wizard → 13322 Frostbolt).
    pub const ENTRY: usize = 0;
    pub const SET_ID: usize = 1;
    pub const SPELL1: usize = 2;
}
mod cr {
    // creature (spawn) — ClassicDB_1_12_1_z2815: guid,id,map,spawnMask,position_x,position_y,
    // position_z,orientation,spawntimesecsmin,spawntimesecsmax,spawndist,MovementType,...
    pub const GUID: usize = 0;
    pub const ID: usize = 1; // == creature_template.Entry
    pub const MAP: usize = 2;
    pub const X: usize = 4;
    pub const Y: usize = 5;
    pub const Z: usize = 6;
    pub const O: usize = 7;
    pub const SPAWN_TIME_SECS_MIN: usize = 8; // work-item 047: cmangos spawntimesecsmin (seconds, from death)
    pub const MOVEMENT_TYPE: usize = 11; // 0 idle / 1 random-wander / 2 waypoint
}
mod cm {
    // creature_movement
    pub const ID: usize = 0; // == creature.guid
    pub const X: usize = 2;
    pub const Y: usize = 3;
    pub const Z: usize = 4;
}
mod cmt {
    // cmangos `creature_movement_template` — the ENTRY-keyed twin of `creature_movement`'s
    // guid-keyed paths (work-item 226: Deadmines patrols whose MovementType=2 spawns carry no
    // per-guid rows). [V] BOTH vintages below are cmangos-1.12-from-knowledge, NOT verified against
    // an in-repo dump (none in this sandbox), and no neighbor-anchor is possible (first import from
    // this table). Vintage is sniffed by COLUMN COUNT, the `npct`/`gmo` precedent:
    //   >= 9 cols: Entry, PathId, Point, PositionX, PositionY, PositionZ, Orientation, WaitTime, ScriptId
    //   <  9 cols: Entry,         Point, PositionX, PositionY, PositionZ, Orientation, WaitTime, ScriptId
    // Only PathId 0 (the default path) imports — the module's waypoint cursor models one route per
    // creature. Confirm via `SHOW CREATE TABLE creature_movement_template` on your real dump.
    pub const ENTRY: usize = 0;
    pub const NCOLS_WITH_PATH_ID: usize = 9;
    pub const PATH_ID: usize = 1; // only in the >= 9-col vintage
    /// (point, x, y, z) column indices for a row of `ncols` — the vintage sniff.
    pub const fn pos_indices(ncols: usize) -> (usize, usize, usize, usize) {
        if ncols >= NCOLS_WITH_PATH_ID {
            (2, 3, 4, 5)
        } else {
            (1, 2, 3, 4)
        }
    }
}
mod cls {
    // player_classlevelstats: (class, level, basehp, basemana), PK (class, level)
    pub const CLASS: usize = 0;
    pub const LEVEL: usize = 1;
    pub const BASEHP: usize = 2;
    pub const BASEMANA: usize = 3;
}
mod lvl {
    // player_levelstats: (race, class, level, str, agi, sta, inte, spi), PK (race, class, level)
    pub const RACE: usize = 0;
    pub const CLASS: usize = 1;
    pub const LEVEL: usize = 2;
    pub const STR: usize = 3;
    pub const AGI: usize = 4;
    pub const STA: usize = 5;
    pub const INTE: usize = 6;
    pub const SPI: usize = 7;
}
mod pc {
    // playercreateinfo: (race, class, map, zone, x, y, z, orientation), PK (race, class)
    pub const RACE: usize = 0;
    pub const CLASS: usize = 1;
    pub const MAP: usize = 2;
    pub const ZONE: usize = 3;
    pub const X: usize = 4;
    pub const Y: usize = 5;
    pub const Z: usize = 6;
    pub const O: usize = 7;
}
mod pcs {
    // playercreateinfo_spell — cmangos/mangos-classic 1.12 schema: race, class, Spell(, Note). [V] —
    // anchored against the well-known cmangos community schema (race TINYINT, class TINYINT, Spell
    // MEDIUMINT[, Note TEXT]), NOT verified against an in-repo dump (this sandbox has none). CONFIRM
    // these column indices against your own `classic-db` dump's `playercreateinfo_spell` before
    // trusting the import — a wrong column here silently mis-teaches spells to the wrong race/class.
    pub const RACE: usize = 0; // [V]
    pub const CLASS: usize = 1; // [V]
    pub const SPELL: usize = 2; // [V]
}
mod pca {
    // playercreateinfo_action — cmangos/mangos-classic 1.12 schema: race, class, button, action, type.
    // [V] — same community-schema provenance/caveat as `pcs` above; CONFIRM against your own dump.
    pub const RACE: usize = 0; // [V]
    pub const CLASS: usize = 1; // [V]
    pub const BUTTON: usize = 2; // [V]
    pub const ACTION: usize = 3; // [V]
    pub const TYPE: usize = 4; // [V]
}
mod pci {
    // playercreateinfo_item — cmangos/mangos-classic 1.12 schema: race, class, itemid, amount. [V] —
    // same community-schema provenance/caveat as `pcs` above; CONFIRM against your own dump. `amount`
    // is parsed but currently unused (game_start_item carries no stack-count column — every starter
    // item grants as a single stack, matching the existing CharStartOutfit-sourced rows).
    pub const RACE: usize = 0; // [V]
    pub const CLASS: usize = 1; // [V]
    pub const ITEM: usize = 2; // [V]
    #[allow(dead_code)]
    pub const AMOUNT: usize = 3; // [V] — unused (see above)
}
mod gz {
    // game_graveyard_zone: id(=world_safe_locs id / game_graveyard.id), ghost_zone, faction
    // (0 = both factions; else the cmangos team-faction id — see module/src/world.rs
    // graveyard::team_for_race). PK (id, ghost_zone) in the cmangos schema (a graveyard can serve
    // more than one zone) — we assign our own sequential row_id (build_graveyard_zone_sql), like
    // start_item_sql's explicit-but-sequential id convention.
    pub const ID: usize = 0;
    pub const GHOST_ZONE: usize = 1;
    pub const FACTION: usize = 2;
}
mod at {
    // cmangos `areatrigger_teleport` — VERIFIED against the real dump 2026-07-10 (work-item 245; the
    // original [V] guess was off by FOUR columns and every imported portal row was garbage — the
    // Deadmines "no entrance" gap). Real row shape, read off the dump's own INSERT:
    //   (id, name, required_level, req_item, req_quest?, req_?, target_map, x, y, z, o, message, ?)
    // e.g. (78,'Deadmines - Entering',10,0,0,0,36,-14.5732,-385.475,62.4561,1.5708,'You must be at
    // least level 10 to enter.',0). required_level(2) is not yet consumed (portal level gates are a
    // future item); the three condition columns are skipped.
    pub const ID: usize = 0;
    pub const NAME: usize = 1;
    pub const TARGET_MAP: usize = 6;
    pub const TARGET_X: usize = 7;
    pub const TARGET_Y: usize = 8;
    pub const TARGET_Z: usize = 9;
    pub const TARGET_O: usize = 10;
}
mod it {
    // item_template — ClassicDB_1_12_1_z2815. Only the columns game_item_template uses.
    pub const ENTRY: usize = 0;
    pub const CLASS: usize = 1;
    pub const SUBCLASS: usize = 2;
    pub const NAME: usize = 3;
    pub const DISPLAY_ID: usize = 4;
    pub const QUALITY: usize = 5;
    // Flags (work-item 213): the raw item_template.Flags bitmask (unique/conjured/etc — ItemFlag on
    // the wire). Sits right after Quality(5), before BuyCount(7)/BuyPrice(8) — anchored by the
    // already-verified BUY_PRICE=8 two columns later (Quality(5), Flags(6), BuyCount(7), BuyPrice(8)
    // is the standard cmangos 1.12 item_template column run).
    pub const FLAGS: usize = 6;
    // BuyCount (080): the per-purchase stack a vendor hands over (water/food ×5, ammo ×200) —
    // anchored between the verified Flags(6) and BUY_PRICE(8).
    pub const BUY_COUNT: usize = 7;
    pub const BUY_PRICE: usize = 8;
    pub const SELL_PRICE: usize = 9;
    pub const INVENTORY_TYPE: usize = 10;
    // ClassicDB_1_12_1_z2815 follows the canonical cmangos run: InventoryType(10),
    // AllowableClass(11), AllowableRace(12), ItemLevel(13).  `-1` is the dump's unrestricted
    // sentinel; restrictive masks are unsigned bits and must stay byte-for-byte intact.
    pub const ALLOWED_CLASS: usize = 11;
    pub const ALLOWED_RACE: usize = 12;
    // RequiredSkill/RequiredSkillRank (work-item 213): weapon-skill / mail-plate proficiency gate.
    // Anchored tightly between the already-verified REQUIRED_LEVEL(14) and STACKABLE(23): 14
    // RequiredLevel, 15 RequiredSkill, 16 RequiredSkillRank, 17 RequiredSpell, 18 RequiredHonorRank,
    // 19 RequiredCityRank, 20 RequiredReputationFaction, 21 RequiredReputationRank, 22 maxcount,
    // 23 Stackable(confirmed) — the run lands exactly on the existing STACKABLE=23 anchor.
    pub const REQUIRED_SKILL: usize = 15;
    pub const REQUIRED_SKILL_RANK: usize = 16;
    // RequiredReputationFaction/Rank (work-item 213): the 195 item half. Same anchored run as above.
    pub const REQUIRED_REPUTATION_FACTION: usize = 20;
    pub const REQUIRED_REPUTATION_RANK: usize = 21;
    // maxcount (work-item 213): unique-item stack cap. Same anchored run — one column before the
    // already-verified STACKABLE=23.
    pub const MAX_COUNT: usize = 22;
    pub const ITEM_LEVEL: usize = 13;
    pub const REQUIRED_LEVEL: usize = 14;
    pub const STACKABLE: usize = 23;
    pub const CONTAINER_SLOTS: usize = 24; // how many slots this bag provides (0 for non-bags)
    pub const STAT_TYPE1: usize = 25; // 10 pairs: type{i}=25+2(i-1), value{i}=26+2(i-1)
    pub const DMG_MIN1: usize = 45;
    pub const DMG_MAX1: usize = 46;
    pub const ARMOR: usize = 60;
    // The 6 resistance schools (work-item 213) — cmangos lays them out immediately after Armor(60)
    // and immediately before the already-verified DELAY=67: 60 Armor, 61 HolyRes, 62 FireRes,
    // 63 NatureRes, 64 FrostRes, 65 ShadowRes, 66 ArcaneRes, 67 Delay(confirmed) — the run lands
    // exactly on the existing DELAY anchor.
    pub const HOLY_RES: usize = 61;
    pub const FIRE_RES: usize = 62;
    pub const NATURE_RES: usize = 63;
    pub const FROST_RES: usize = 64;
    pub const SHADOW_RES: usize = 65;
    pub const ARCANE_RES: usize = 66;
    pub const DELAY: usize = 67;
    // On-use/on-equip spell slots (7 cols each: spellid, spelltrigger, charges, ppmRate, cooldown,
    // category, categorycooldown). We import slots 1-2's id+trigger for the client green "Use:" text.
    pub const SPELLID_1: usize = 70;
    pub const SPELLTRIGGER_1: usize = 71;
    pub const SPELLID_2: usize = 77;
    pub const SPELLTRIGGER_2: usize = 78;
    // Spell slots 3-5 (work-item 213): each slot is 7 columns wide, same shape as slots 1-2 above.
    // Slot 3 starts right after slot 2's 7 columns (77..=83), slot 4 after slot 3's (84..=90), slot 5
    // after slot 4's (91..=97) — and slot 5's own 7 columns (98..=104) land exactly on the
    // already-verified BONDING=105 anchor one column later, confirming all three slots.
    pub const SPELLID_3: usize = 84;
    pub const SPELLTRIGGER_3: usize = 85;
    pub const SPELLID_4: usize = 91;
    pub const SPELLTRIGGER_4: usize = 92;
    pub const SPELLID_5: usize = 98;
    pub const SPELLTRIGGER_5: usize = 99;
    pub const BONDING: usize = 105; // item binding (work-item 127): 0=NoBind,1=BoP,2=BoE,3=BoU,4/5=QuestItem
                                    // description(106), PageText(107), LanguageID(108), PageMaterial(109), startquest(110),
                                    // lockid(111), Material(112) sit between BONDING(105, confirmed) and the already-verified
                                    // SHEATH=113 below — PAGE_TEXT and START_QUEST land on that same anchored run.
    pub const PAGE_TEXT: usize = 107; // readable-item page id (work-item 213: needs its own reader packet, deferred)
    pub const START_QUEST: usize = 110; // quest-starter link (work-item 213: 194 consumes)
    pub const SHEATH: usize = 113; // sheath posture, sent verbatim to the client (0=no stow, 1H sword=3,
                                   // shield=4 — opaque client indices, see items/tables.rs::sheath)
    pub const BLOCK: usize = 115; // shield block value (CREATE TABLE: …RandomProperty(114), block(115), itemset(116))
    pub const MAX_DURABILITY: usize = 117;
    // BagFamily (work-item 213): bag-type restriction bitmask. Anchored right after the
    // already-verified MAX_DURABILITY=117: 117 MaxDurability(confirmed), 118 area, 119 Map,
    // 120 BagFamily.
    pub const BAG_FAMILY: usize = 120;
    // FoodType follows ScriptName(121) and DisenchantID(122) in the current ClassicDB schema.
    // Values 1..=8 map to CreatureFamily.pet_food_mask bits; 0 is not pet food.
    pub const FOOD_TYPE: usize = 123;
}
mod clt {
    // creature_loot_template: entry(=creature_template.LootId), item, ChanceOrQuestChance, groupid,
    // mincountOrRef, maxcount, condition_id, comments.
    pub const ENTRY: usize = 0;
    pub const ITEM: usize = 1;
    pub const CHANCE: usize = 2; // >0 = drop %, <0 = quest item (skip — no quest system yet)
    pub const GROUPID: usize = 3; // 0 = independent; >0 = pick-one group within this loot table
    pub const MINCOUNT_OR_REF: usize = 4; // <0 = reference to a shared pool (expanded as a group)
    pub const MAXCOUNT: usize = 5;
}
mod nv {
    // npc_vendor: entry(=creature_template.entry), item, maxcount, incrtime, slot, condition_id, comments.
    pub const ENTRY: usize = 0;
    pub const ITEM: usize = 1;
    pub const MAXCOUNT: usize = 2;
    pub const SLOT: usize = 4;
}
mod qt {
    // quest_template — only the columns game_quest_template + objectives/rewards need (verified against
    // the ClassicDB CREATE TABLE). ReqCreatureOrGOId/Count + RewItemId/Count are 4 contiguous slots each.
    pub const ENTRY: usize = 0;
    pub const ZONE_OR_SORT: usize = 2; // quest-log category: >0 AreaTable zone, <0 QuestSort id
    pub const MIN_LEVEL: usize = 3;
    pub const QUEST_LEVEL: usize = 5;
    pub const REQUIRED_CLASSES: usize = 7; // bitmask, 0 = all classes
    pub const REQUIRED_RACES: usize = 8; // bitmask, 0 = all races
                                         // [V] LimitTime (seconds from accept, 0 = untimed) — unverified column index (no cmangos dump in
                                         // this sandbox to confirm against). Anchored by bracketing against the two already-trusted
                                         // constants either side: cmangos's real `quest_template` layout runs
                                         // ...RequiredMaxRepValue, SuggestedPlayers, LimitTime, QuestFlags, SpecialFlags... so col 19 sits
                                         // exactly 2 slots before the already-verified SPECIAL_FLAGS=21 (QuestFlags at 20 between them).
                                         // Coverage-printed at import time (timed-quest count) rather than hard floor-gated — plausible
                                         // this Elwynn/Westfall slice has ZERO genuinely timed quests (escort quests, the usual timed
                                         // pairing, are deliberately out of scope per work-item 194), so a floor here would risk a false
                                         // failure on a legitimate import; the coverage print is the fail-LOUD net instead.
    pub const LIMIT_TIME: usize = 19;
    pub const SPECIAL_FLAGS: usize = 21; // bit0 = QUEST_SPECIAL_FLAGS_REPEATABLE
    pub const PREV_QUEST_ID: usize = 22; // prerequisite quest (must be turned in first); 0 = none
                                         // [V] NextQuestId/NextQuestInChain (the successor auto-offered on turn-in) — unverified column
                                         // index (no cmangos dump in this sandbox to confirm against). Bracketed between the two
                                         // already-trusted constants either side: 3 slots after PREV_QUEST_ID=22 (PrevQuestId, NextQuestId,
                                         // ExclusiveGroup sit between), 2 slots before SRC_ITEM_ID=27. Coverage-printed at import time
                                         // (chained-quest count) + a presence floor (`FLOOR_QUESTS_CHAINED`, importer/scripts/import-manifest.sh) —
                                         // McBride's 783->7->15->21->54 chain (already verified via the working `prev_quest_id` import)
                                         // should populate this symmetrically, so >=1 is a safe floor, unlike LIMIT_TIME above.
    pub const NEXT_QUEST_IN_CHAIN: usize = 25;
    pub const SRC_ITEM_ID: usize = 27; // SrcItemId — item HANDED to the player on accept (provided item)
    pub const SRC_ITEM_COUNT: usize = 28; // SrcItemCount (0 in the dump means 1)
    pub const TITLE: usize = 30;
    pub const DETAILS: usize = 31; // quest description / story text
    pub const OBJECTIVES: usize = 32; // objective summary line(s)
    pub const OFFER_REWARD_TEXT: usize = 33; // text on the turn-in reward screen
    pub const REQUEST_ITEMS_TEXT: usize = 34; // text on the "not finished yet" screen
    pub const REQ_ITEM_ID1: usize = 40; // ..=43; collect-objective item entries (parity #4)
    pub const REQ_ITEM_COUNT1: usize = 44; // ..=47
    pub const REQ_CREATURE_OR_GO_ID1: usize = 56; // ..=59; >0 = creature entry, <0 = gameobject (skip)
    pub const REQ_CREATURE_OR_GO_COUNT1: usize = 60; // ..=63
    pub const REW_CHOICE_ITEM_ID1: usize = 68; // ..=73 RewChoiceItemId1-6 (pick-1-of-N choice rewards)
    pub const REW_CHOICE_ITEM_COUNT1: usize = 74; // ..=79 RewChoiceItemCount1-6
    pub const REW_ITEM_ID1: usize = 80; // ..=83 (guaranteed reward items)
    pub const REW_ITEM_COUNT1: usize = 84; // ..=87
    pub const REW_REP_FACTION1: usize = 88; // ..=92 RewRepFaction1..5 (we model 2 slots; +1 = faction2)
    pub const REW_REP_VALUE1: usize = 93; // ..=97 RewRepValue1..5 (parallel to the faction cols; +1 = value2)
    pub const REW_OR_REQ_MONEY: usize = 98; // >0 = reward copper, <0 = required copper (clamp ≥ 0)
    pub const REW_MONEY_MAX_LEVEL: usize = 99; // cmangos RewMoneyMaxLevel (col 99, right after 98); /0.6 = authentic quest XP
}
mod qr {
    // creature_questrelation (START) AND creature_involvedrelation (END) share this 2-col layout —
    // as do their GAMEOBJECT twins gameobject_questrelation (START) / gameobject_involvedrelation
    // (END): id = gameobject_template.entry, quest = quest_template.entry (work-item 041).
    pub const CREATURE: usize = 0; // id = creature_template.entry (or gameobject_template.entry for the GO tables)
    pub const QUEST: usize = 1;
}
mod sg {
    // spawn_group: Id, Name, Type, MaxCount, WorldState, WorldStateExpression, Flags, StringId
    pub const ID: usize = 0;
    pub const TYPE: usize = 2; // 0 = creature, 1 = gameobject
    pub const MAX_COUNT: usize = 3; // 0 = unlimited; >0 = cap across all member guids
}
mod sgs {
    // spawn_group_spawn: GroupId, Guid, SlotId, Chance
    pub const GROUP_ID: usize = 0;
    pub const GUID: usize = 1;
}
mod cse {
    // creature_spawn_entry: guid, entry — maps id=0 pool slots to concrete creature_template entries.
    // Each guid may have multiple entries (the live server picks randomly); we pick the first (deterministic).
    pub const GUID: usize = 0;
    pub const ENTRY: usize = 1;
}

const HIGHGUID_UNIT: u64 = 0xF130;

/// Our deterministic creature guid from the cmangos entry + spawn guid (HIGHGUID_UNIT-encoded, like
/// the seed). Stable across re-imports → clear+reload is idempotent. (entry<<24)|(low 24 of db guid).
fn world_guid(entry: u64, db_guid: u64) -> u64 {
    (HIGHGUID_UNIT << 48) | ((entry & 0xFF_FFFF) << 24) | (db_guid & 0xFF_FFFF)
}

const HIGHGUID_GAMEOBJECT: u64 = 0xF110;

/// GO guid from the cmangos gameobject.guid (HIGHGUID_GAMEOBJECT-encoded, matching the gameobject seed's
/// 0xF110<<48 scheme). Stable across re-imports → the import_gameobjects clear+reload is idempotent.
fn go_guid(db_guid: u64) -> u64 {
    (HIGHGUID_GAMEOBJECT << 48) | (db_guid & 0xFFFF_FFFF_FFFF)
}

/// Uniform GATHER-node respawn window (seconds). The dump carries spawntimesecsmin=max=300 for these
/// entries (one Copper outlier at 45/90) — use the real vanilla 300s for all, NOT the module's 180s
/// fallback. (Per-node respawn variance from the spawn row is DEFERRED — single shared value for 8 nodes.)
const GATHER_RESPAWN_SECS: u32 = 300;

/// The known Elwynn gather-node gameobjects, keyed by GO entry →
/// (gather_skill_line, granted item entry, required skill level, gray ceiling).
/// HARDCODED allowlist — cmangos models mining veins + herbs as locked type-3
/// CHESTs (a blind "all type-3" pulls in 30+ treasure chests). The granted item is the chance-100,
/// group-0 row of gameobject_loot_template[lootId]; the required level is Lock.dbc-derived (well-known
/// vanilla values — the importer reads no loot table / Lock.dbc, not worth building for 8 known nodes).
/// MINING = 186, HERBALISM = 182 (matches module skill::skill_line).
/// The `gray` ceiling is the vanilla skillup gray threshold for the node: at/above this skill a gather
/// never yields +1 (skillup_chance_bp returns 0). Values sourced from the vanilla 1.12 skill color bands
/// (orange/yellow/green/gray). The linear collapse (orange→gray) is our simplified single-band model.
const GATHER_NODES: &[(u64, u32, u32, u32, u32)] = &[
    // entry, skill_line, item, orange(=required), gray
    (1731, 186, 2770, 1, 100),
    (1732, 186, 2771, 65, 140), // Copper Vein / Tin Vein
    (1617, 182, 765, 1, 75),
    (1618, 182, 2447, 1, 75), // Silverleaf / Peacebloom
    (1619, 182, 2449, 15, 90),
    (1620, 182, 785, 50, 125), // Earthroot / Mageroyal
    (1621, 182, 2450, 70, 145),
    (1622, 182, 2453, 100, 175), // Briarthorn / Bruiseweed
];

/// Lookup a gather node by GO entry → (skill_line, granted item entry, required level, gray ceiling),
/// or None if the entry is not a known gather node.
fn gather_node(entry: u64) -> Option<(u32, u32, u32, u32)> {
    GATHER_NODES
        .iter()
        .find(|n| n.0 == entry)
        .map(|n| (n.1, n.2, n.3, n.4))
}

/// This repo's stored `game_gameobject_template.type_id` values (mirrors module `go_type::*`,
/// `module/src/gameobject.rs` — importer is a separate crate, so the literal is re-declared here
/// rather than shared). LIVE = dispatched by `apply_use_gameobject`; QUESTGIVER imports unchanged (no
/// dispatch, its gate lives in `game_gameobject_quest`); every OTHER value this slice produces is
/// INERT (template+spawn only, `use` is a no-op) — see `classify_go_type` (work-item 211's widened
/// import: CHEST/DOOR/BUTTON join GOOBER/GATHER as LIVE; everything else that used to be filtered out
/// entirely now imports inert instead).
pub(crate) const GO_DOOR: u8 = 0;
pub(crate) const GO_BUTTON: u8 = 1;
const GO_QUESTGIVER: u8 = lyracore_shared::constants::go_type::QUESTGIVER; // shared const (041) — no drift
const GO_CHEST: u8 = 3;
const GO_GOOBER: u8 = 10;
/// Synthetic — must equal module `go_type::GATHER`. Vanilla's REAL type 25 is
/// GAMEOBJECT_TYPE_FISHINGHOLE; see the TYPE-25 COLLISION GUARD in `classify_go_type`.
const GO_GATHER: u8 = 25;
/// The real cmangos type that collides with [`GO_GATHER`] — see `classify_go_type`.
const CMANGOS_FISHINGHOLE_TYPE: u32 = 25;

/// Classify one `gameobject_template` row's cmangos TYPE into this repo's stored `type_id` — `None`
/// means DROP the row from import entirely (template AND spawn). Pure + fixture-tested.
///
/// TYPE-25 COLLISION GUARD (highest-severity risk this work item touches): cmangos assigns REAL type
/// 25 to GAMEOBJECT_TYPE_FISHINGHOLE, which happens to collide with this repo's SYNTHETIC `GO_GATHER`
/// marker (chosen back when vanilla had no real type 25 in scope — see the module's `go_type::GATHER`
/// doc). `GATHER_NODES` is the ONLY producer of a stored type 25 (checked FIRST, before the raw-25
/// arm below, so a type-3 gather node never falls through to the guard); any REAL cmangos type-25 row
/// is dropped here rather than risk a fishing hole being silently treated as (or colliding with) a
/// gather node by any current or future GATHER-keyed logic.
///
/// Every other in-scope raw type passes through IDENTITY (this repo's type ids match cmangos' for
/// every value except the 3→25 gather remap and the 25 drop) — `main.rs`'s per-type dispatch decides
/// LIVE vs INERT from the resulting value, not from this function.
pub(crate) fn classify_go_type(entry: u64, raw_type: u32) -> Option<u8> {
    match raw_type {
        3 if gather_node(entry).is_some() => Some(GO_GATHER),
        CMANGOS_FISHINGHOLE_TYPE => None,
        t if t <= u8::MAX as u32 => Some(t as u8),
        _ => None, // an out-of-range raw type (corrupt dump row) — drop rather than truncate
    }
}

/// Build ONE `game_gameobject_template` SQL row-tuple for an ALREADY-CLASSIFIED GO (work-item 211).
/// `data0`/`data1` are the RAW dump columns (positionally identical across every cmangos GO type —
/// `got::DATA0`/`got::DATA1`). Carries the cmangos lockId into the END-appended `lock_id` column:
/// CHEST/GOOBER source it from `data0` (CHEST repo-verified — see `got` module doc; GOOBER [V]);
/// DOOR/BUTTON from `data1` ([V] — cmangos DOOR/BUTTON data0=startOpen, data1=lockId); everything else
/// carries 0 (no lock concept this slice, or GATHER's skill-gate stays hand-coded per the plan).
/// `data0`/`data1` template COLUMNS (as opposed to the dump's raw data0/data1) keep their PRE-211
/// meanings: CHEST.data0 is the legacy single-item fallback (stays 0 for a real imported chest —
/// unchanged), CHEST.data1 is the real `gameobject_loot_template` lootId (unchanged, now for every
/// chest instead of a curated allowlist). Returns the row tuple AND, for a CHEST with a nonzero
/// lootId, that lootId (to fold into `chest_loot_ids_used` — widened to EVERY imported chest).
/// `size` rides verbatim into the END-appended `size` column for EVERY type; a 0 (absent/corrupt
/// dump column) stays 0, which the gateway codec renders as 1.0.
fn go_template_row(
    entry: u64,
    stored_type: u8,
    disp: u32,
    name: &str,
    data0: u32,
    data1: u32,
    size: f32,
) -> (String, Option<u32>) {
    let n = sql_text(name);
    match stored_type {
        GO_GATHER => {
            let (skill, item, req, gray) = gather_node(entry)
                .expect("classify_go_type only returns GO_GATHER for a GATHER_NODES entry");
            (format!("({entry},{GO_GATHER},{disp},{n},{item},{req},{skill},{GATHER_RESPAWN_SECS},{gray},0,{size})"), None)
        }
        GO_CHEST => {
            let loot_id = data1;
            (
                format!("({entry},{GO_CHEST},{disp},{n},0,{loot_id},0,0,0,{data0},{size})"),
                (loot_id != 0).then_some(loot_id),
            )
        }
        GO_GOOBER => (
            format!("({entry},{GO_GOOBER},{disp},{n},0,0,0,0,0,{data0},{size})"),
            None,
        ),
        GO_DOOR => (
            format!("({entry},{GO_DOOR},{disp},{n},0,0,0,0,0,{data1},{size})"),
            None,
        ),
        GO_BUTTON => (
            format!("({entry},{GO_BUTTON},{disp},{n},0,0,0,0,0,{data1},{size})"),
            None,
        ),
        other => (
            format!("({entry},{other},{disp},{n},0,0,0,0,0,0,{size})"),
            None,
        ), // QUESTGIVER + every INERT type
    }
}

/// DOOR/BUTTON's initial SPAWN state from the dump's own `data0` (cmangos `startOpen` [V]): nonzero →
/// spawns OPEN (state 1), else CLOSED (state 0, the pre-211 always-closed default). Every other type
/// always spawns state 0 (CHEST closed/unlooted, GOOBER/GATHER/inert unused). Pure + fixture-tested.
fn go_initial_state(stored_type: u8, data0: u32) -> u8 {
    if (stored_type == GO_DOOR || stored_type == GO_BUTTON) && data0 != 0 {
        1
    } else {
        0
    }
}

mod nt {
    // npc_trainer: entry(=creature_template.entry), spell, spellcost, reqskill, reqskillvalue, reqlevel, ...
    pub const ENTRY: usize = 0;
    pub const SPELL: usize = 1;
    pub const COST: usize = 2;
    pub const REQ_SKILL: usize = 3; // SkillLine the offering teaches/requires (0 = a class spell)
    pub const REQ_LEVEL: usize = 5;
}

// A small reviewable CONST data table (the doc above says so): each tuple is one profession's learn ladder.
#[allow(clippy::type_complexity)]
/// Profession learn-spell tiers SYNTHESIZED into game_trainer_spell. cmangos delivers Apprentice→Artisan
/// learning via the gossip "Train me" menu (core C++), NOT via npc_trainer — those rows (reqskill != 0)
/// are RECIPES that REQUIRE the skill, not the skill-learn itself. So for every in-box trainer whose
/// npc_trainer rows carry a reqskill matching one of our 12 skill_lines, we emit one synthesized learn-row
/// per tier carrying the REAL Spell.dbc learn id as spell_id (so SMSG_LEARNED_SPELL resolves client-side,
/// unlike the old 50080-88 markers) + learn_skill_line/learn_skill_cap set (the module routes the buy to
/// crate::skill::learn_profession instead of a spell). Tuple: (skill_line, &[(spell_id, tier_index,
/// required_level)]) — `tier_index` (0=Apprentice,1=Journeyman,2=Expert,3=Artisan) replaced a literal cap
/// (work-item 208): the real cap now comes from `SkillTiers.dbc` (see `dbc::profession_tier_values`),
/// checked against `LEGACY_TIER_CAPS` by `assert_profession_tier_parity` before it's ever emitted — this
/// table itself no longer carries the number, just WHICH tier. The gather/secondary lines (Mining/
/// Herbalism/Skinning/Cooking/First Aid) stop at Expert(2) HERE — their Artisan(3) tier is deferred (not
/// load-bearing for 1-10); the crafting trios (Alchemy/BS/Tailoring/LW) go to Artisan(3). Tier ids verified
/// vs the dump's spell_template (effect-118 base points = tier). The final three professions —
/// Engineering(202)/Enchanting(333)/Fishing(356) — complete the 12: Eng/Ench go to Artisan(3), Fishing
/// stops at Expert(2) like the other secondary lines. KEEP THIS A SMALL REVIEWABLE CONST.
const PROFESSION_LEARN: &[(u32, &[(u32, u8, u8)])] = &[
    (186, &[(2575, 0, 1), (2576, 1, 10), (3564, 2, 20)]), // Mining (2576=Jour, 3564=Expert)
    (182, &[(2366, 0, 1), (2368, 1, 10), (3570, 2, 20)]), // Herbalism
    (393, &[(8613, 0, 1), (8617, 1, 10), (8618, 2, 20)]), // Skinning (8618=Expert, not 10768=Artisan)
    (185, &[(2550, 0, 1), (3102, 1, 10), (3413, 2, 20)]), // Cooking
    (129, &[(3273, 0, 1), (3274, 1, 10), (7924, 2, 20)]), // First Aid
    (
        171,
        &[(2259, 0, 1), (3101, 1, 10), (3464, 2, 20), (11611, 3, 35)],
    ), // Alchemy
    (
        164,
        &[(2018, 0, 1), (3100, 1, 10), (3538, 2, 20), (9785, 3, 35)],
    ), // Blacksmithing (3538=Expert; 9785=Artisan, NOT 9787 Weaponsmith)
    (
        197,
        &[(3908, 0, 1), (3909, 1, 10), (3910, 2, 20), (12180, 3, 35)],
    ), // Tailoring
    (
        165,
        &[(2108, 0, 1), (3104, 1, 10), (3811, 2, 20), (10662, 3, 35)],
    ), // Leatherworking
    // --- The final three (completing the 12). Tier ids VERIFIED vs the dump's spell_template (Effect2=118
    // SKILL, MiscVal2=skill_line, BasePoints2=tier → cap=(BP2+1)*75). Fishing's ids were corrected: the
    // original spec was shifted two tiers (7733 is a LearnSpell wrapper, NOT a skill tier); the real
    // Effect2=118/MiscVal=356 tiers are 7620/7731/7732/18248 → use 7620/7731/7732 (stop at Expert 2).
    (
        202,
        &[(4036, 0, 1), (4037, 1, 10), (4038, 2, 20), (12656, 3, 35)],
    ), // Engineering → Artisan
    (
        333,
        &[(7411, 0, 1), (7412, 1, 10), (7413, 2, 20), (13920, 3, 35)],
    ), // Enchanting → Artisan
    (356, &[(7620, 0, 1), (7731, 1, 10), (7732, 2, 20)]), // Fishing (stops at Expert, like other secondaries)
];

/// The legacy Apprentice/Journeyman/Expert/Artisan tier caps `PROFESSION_LEARN` hardcoded before
/// work-item 208 wired in the real client's `SkillTiers.dbc` data — every profession in vanilla rides
/// this exact 75/150/225/300 progression, indexed by `tier_index`. `assert_profession_tier_parity`
/// checks the DBC-derived caps against this constant; the synthesis loop (below) falls back to it
/// directly when `--dbc` wasn't given (no client data available to check against, so no parity guard
/// is possible — documented fallback).
const LEGACY_TIER_CAPS: [u32; 4] = [75, 150, 225, 300];

/// The tier list a trainer should TEACH, given a SkillLine seen on its npc_trainer rows — `Some(tiers)`
/// iff `line` is one of our 12 mapped professions, else `None` (a class spell or a deferred line). The ONE
/// check on the spell->(skill_line,tier_index) map.
fn profession_tiers(line: u32) -> Option<&'static [(u32, u8, u8)]> {
    PROFESSION_LEARN
        .iter()
        .find(|(l, _)| *l == line)
        .map(|(_, t)| *t)
}

/// PARITY GUARD (work-item 208): for every `PROFESSION_LEARN` entry, assert the REAL client's
/// `SkillRaceClassInfo.dbc`/`SkillTiers.dbc` data (`tier_values`, from `dbc::profession_tier_values`)
/// resolves that `(skill_line, tier_index)` to the exact `LEGACY_TIER_CAPS` cap this importer hardcoded
/// before the DBC import existed. A mismatch means the operator's client disagrees with our assumption
/// that every profession rides the identical 75/150/225/300 progression — bail loudly (naming the
/// offending line + tier) rather than silently emitting a wrong cap into `game_trainer_spell`.
pub(crate) fn assert_profession_tier_parity(
    tier_values: &std::collections::HashMap<u32, [u16; 4]>,
) -> Result<()> {
    for &(line, tiers) in PROFESSION_LEARN {
        let Some(caps) = tier_values.get(&line) else {
            bail!("profession tier parity: skill line {line} has no SkillRaceClassInfo/SkillTiers row in the client data");
        };
        for &(_, tier_index, _) in tiers {
            let dbc_cap = caps[tier_index as usize] as u32;
            let legacy = LEGACY_TIER_CAPS[tier_index as usize];
            if dbc_cap != legacy {
                bail!(
                    "profession tier parity: skill line {line} tier {tier_index} — dbc cap {dbc_cap} != legacy literal {legacy}"
                );
            }
        }
    }
    Ok(())
}
mod sc {
    // spell_chain (cmangos/mangos-classic 1.12 schema): spell_id, prev_spell, first_spell, rank,
    // req_spell. [V] — anchored against the well-known cmangos community schema column ORDER, NOT
    // verified against an in-repo dump (this sandbox has no cmangos dump — same caveat as `pcs` above).
    // CONFIRM these column indices against your own `classic-db` dump's `spell_chain` table before
    // trusting the import — a wrong column here silently mis-gates rank purchases.
    pub const SPELL_ID: usize = 0; // [V]
    pub const PREV_SPELL: usize = 1; // [V]
    pub const FIRST_SPELL: usize = 2; // [V]
    pub const RANK: usize = 3; // [V]
    pub const REQ_SPELL: usize = 4; // [V]
}
mod sls {
    // spell_learn_spell (cmangos/mangos-classic 1.12 schema): entry(parent spell), SpellID(dependent to
    // auto-learn), Active. [V] — same anchoring caveat as `sc` above. `ACTIVE` is parsed (so a future
    // gate can read it) but not consulted by `build_spell_learn_sql` this slice — see its doc.
    pub const ENTRY: usize = 0; // [V] parent_spell
    pub const SPELL_ID: usize = 1; // [V] learn_spell
    pub const ACTIVE: usize = 2; // [V] parsed-but-unused
}
pub(crate) mod got {
    // gameobject_template: entry, type, displayId, name, faction, flags, ExtraFlags, size, data0..23, ...
    pub const ENTRY: usize = 0;
    pub const TYPE: usize = 1;
    pub const DISPLAY_ID: usize = 2;
    pub const NAME: usize = 3;
    // The prop's render scale (OBJECT_FIELD_SCALE_X); gathering nodes are authentically sub-1.0.
    // Carried verbatim — the 0/absent → 1.0 fallback lives in the gateway codec, not here.
    pub const SIZE: usize = 7;
    // Positionally identical across EVERY cmangos GO type (only the per-type MEANING differs — see
    // `go_template_row`'s doc): CHEST data0=lockId (repo-verified) data1=lootId (a
    // gameobject_loot_template entry, work-item 210); DOOR/BUTTON data0=startOpen data1=lockId [V];
    // GOOBER data0=lockId [V]. GATHER nodes read NEITHER (the granted item + required level come from
    // the hardcoded GATHER_NODES map, not Lock.dbc — see that const's doc). `go_template_row` (work-item
    // 211) is what routes DATA0/DATA1 into the right module column per type; this module just names the
    // raw dump columns.
    pub const DATA0: usize = 8;
    pub const DATA1: usize = 9;
}
mod go {
    // gameobject (spawn): guid, id(=gameobject_template.entry), map, spawnMask, x, y, z, orientation, rot0..3, ...
    pub const GUID: usize = 0;
    pub const ID: usize = 1;
    pub const MAP: usize = 2;
    pub const X: usize = 4;
    pub const Y: usize = 5;
    pub const Z: usize = 6;
    pub const O: usize = 7;
    // The cmangos spawn quaternion (issue #515) — the client's real prop orientation, `orientation`
    // (O, above) alone only drives movement-facing math. Positionally right after O per the dump
    // schema comment; carried through byte-verbatim (no derive/snap here — that fallback lives in
    // the gateway codec for the all-zero case, not the importer).
    pub const ROT0: usize = 8;
    pub const ROT1: usize = 9;
    pub const ROT2: usize = 10;
    pub const ROT3: usize = 11;
}
mod ai {
    // areatrigger_involvedrelation: id(=areatrigger id), quest.
    pub const TRIGGER: usize = 0;
    pub const QUEST: usize = 1;
}

// --- mysqldump INSERT parser ------------------------------------------------------------------
// Extracts every value tuple from `INSERT INTO `<table>` VALUES (..),(..);` statements. Handles
// single-quoted strings with `\` escapes (names contain commas/apostrophes). Returns each field as
// an unescaped String (quotes stripped). We PARSE, never execute (the firewall: no GPL code runs).
pub(crate) fn parse_table(dump: &str, table: &str) -> Vec<Vec<String>> {
    let needle = format!("INSERT INTO `{table}` VALUES ");
    let mut rows = Vec::new();
    let mut search = 0;
    while let Some(rel) = dump[search..].find(&needle) {
        let mut i = search + rel + needle.len();
        let bytes = dump.as_bytes();
        // Parse tuples until the terminating `;` at top level.
        let mut field = String::new();
        let mut tuple: Vec<String> = Vec::new();
        let mut in_tuple = false;
        let mut in_quote = false;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if in_quote {
                if c == '\\' && i + 1 < bytes.len() {
                    // escaped char — take the next literally (\' \\ \n ...)
                    let n = bytes[i + 1] as char;
                    field.push(match n {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    });
                    i += 2;
                    continue;
                } else if c == '\'' {
                    in_quote = false;
                } else {
                    field.push(c);
                }
            } else {
                match c {
                    '(' if !in_tuple => {
                        in_tuple = true;
                        tuple.clear();
                        field.clear();
                    }
                    '\'' if in_tuple => in_quote = true,
                    ',' if in_tuple => {
                        tuple.push(std::mem::take(&mut field));
                    }
                    ')' if in_tuple => {
                        tuple.push(std::mem::take(&mut field));
                        rows.push(std::mem::take(&mut tuple));
                        in_tuple = false;
                    }
                    ';' if !in_tuple => break,
                    _ if in_tuple => field.push(c),
                    _ => {}
                }
            }
            i += 1;
        }
        search = i;
    }
    rows
}

pub(crate) fn field(row: &[String], idx: usize) -> &str {
    row.get(idx).map(|s| s.trim()).unwrap_or("")
}

/// ClassicDB uses signed `-1` for an unrestricted allowable-class/race column.  It is normalized
/// only at this import boundary; every other mask is parsed as raw unsigned bits so unknown bits
/// survive into the Module and its item-query response.
fn normalize_item_mask(source: &str, unrestricted: u32) -> u32 {
    if source == "-1" {
        unrestricted
    } else {
        source.parse().unwrap_or(0)
    }
}
/// Drink discriminator → `game_item_template.restores_power` (food vs drink). In classicdb z2815 BOTH
/// food and drink are class 0 (Consumable) with subclass 0 (generic) OR 5 (Food&Drink) — they do NOT
/// split on subclass — so we gate class 0 + subclass {0,5} (excluding potions=1/elixirs=2/flasks=3/
/// scrolls=4/bandages=7 so "Elixir of Water Walking" can't false-positive) and discriminate by NAME.
/// Match WHOLE WORDS, not substrings, so "S`tea`k" / "Snapvine `Water`melon" (food) don't match; drop
/// the collision-prone "beer"/"brew" ("`Beer` Basted Boar Ribs"). wine/ale stay (Dalaran Red Wine is a
/// real mana drink). Deliberate simplification: name-match; the canonical signal is the on-use
/// spell (E_ENERGIZE) — wire that only if a beverage is ever misclassified.
fn is_drink_consumable(class: u8, subclass: u8, name: &str) -> bool {
    if class != 0 || (subclass != 0 && subclass != 5) {
        return false;
    }
    const DRINK_WORDS: &[&str] = &[
        "water", "dew", "drink", "milk", "juice", "tea", "nectar", "wine", "ale", "soda", "coffee",
    ];
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| DRINK_WORDS.contains(&w))
}
/// The Northshire tutorial-wolf faction remap (work-item 214 pulled this out of the creature_template
/// loop as a pure, byte-parity-locked function — the DELIBERATE DEVIATION documented at its call site
/// is unchanged, just relocated here so it's independently unit-testable). Remaps the passive beast
/// faction (32) to the passive-but-attackable faction template 58, but ONLY for `CreatureFamily` ==
/// Wolf (1) — every other family, and every non-32 faction, passes through verbatim.
fn wolf_faction_fixup(faction_raw: u32, family_id: i32) -> u32 {
    if faction_raw == 32 && family_id == 1 {
        58
    } else {
        faction_raw
    }
}
/// SQL string literal with single-quotes doubled.
fn sql_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
/// SQL text from a cmangos field, mapping the literal `NULL` token to an empty string. Our parser
/// strips quotes, so a quoted `'NULL'` and an unquoted `NULL` are indistinguishable — but no creature
/// is named "NULL", so collapsing it to empty is safe and keeps the column NOT-NULL (no `'NULL'` text).
pub(crate) fn sql_text(s: &str) -> String {
    if s == "NULL" {
        "''".into()
    } else {
        sql_str(s)
    }
}

pub(crate) struct Args {
    dump: Option<String>,               // cmangos creature ETL input (.sql[.gz])
    dbc: Option<String>,                // client Data/ dir for the DBC stream
    pub(crate) terrain: Option<String>, // client Data/ dir for the ADT heightmap stream (see terrain.rs)
    pub(crate) dump_collision: Option<String>, // client Data/ dir: 240 spike, WMO/M2 collision dry-run (see collision.rs)
    pub(crate) nav: Option<String>, // client Data/ dir: 241 nav-grid rasterizer (see nav.rs)
    pub(crate) vmap: Option<String>, // client Data/ dir: #520/#521 exact vmap triangle extract+pack+import (see vmap.rs)
    pub(crate) vmap_status: bool, // print active-generation provenance/status without opening client data
    pub(crate) vmap_prepare_coverage: Option<u64>, // generation id: derive path-grid coverage from an already-staged generation (see vmap.rs::run_coverage)
    pub(crate) go_models: Option<String>, // client Data/ dir: DOOR/BUTTON display -> M2 bounding-mesh extract+import (see go_model.rs); needs --dump too
    pack_client: Option<String>, // client Data/ dir for the --pack-client packager (see pack_client.rs)
    print_extents: bool, // with --dump: print the operator's own spawn bbox for --map and exit (work-item 206)
    spells: bool, // with --dbc: import Spell.dbc → game_spell/game_spell_effect (see spell.rs)
    talents: bool, // with --dbc: import TalentTab.dbc + Talent.dbc → game_talent_tab/game_talent (see talent.rs)
    pub(crate) only: Vec<u32>, // with --spells: import ONLY these spell ids ADDITIVELY (surgical
    // delete+reload, no wholesale wipe) — preserves the seed + synthetic test fixtures.
    pub(crate) trainers: Vec<(u32, Vec<u32>)>, // with --spells: repeatable `--trainer <entry>=<id,id,…>`
    // → emit a game_trainer_spell row per id keyed to that trainer CREATURE-TEMPLATE
    // entry. `required_level`/`cost` are DBC-DERIVED (the spell's own spell_level +
    // our formula) — NO cmangos npc_trainer value is read. The ids ARE the per-class
    // offering list (a subset of --only); the importer auto-adds them to the allowlist.
    pub(crate) world_profile: Option<WorldImportProfile>,
    map: i64,
    include_maps: Vec<i64>, // --include-map <id> (repeatable, work-item 226): ADDITIONAL maps whose
    // rows import WHOLE-MAP (no --box/--radius/--exclude geometry — an instance map like Deadmines 36
    // is small and self-contained; its scope IS the map id). The primary --map keeps its box scoping.
    // Same single-run clear+reload as the box slice — this is the "no additive two-pass path" answer
    // for instance maps (see importer/scripts/import-world.sh's ONE-BOX note).
    center: (f64, f64, f64),
    radius: f64,
    // --include-creatures <e1,e2,..>: FORCE-import these creature entries (template + spawns + quest
    // START/END relations + vendor/loot/gossip) regardless of the --box/--exclude geometry — the
    // mechanism to recover out-of-box quest givers (e.g. a Westfall/Stormwind courier NPC whose
    // sibling giver is in-band). Implemented in the spawn loop: a row whose entry is force-included
    // bypasses the geometry gate, so its spawn survives → it flows into `entries` (derived from
    // `spawns`) → every creature-scoped consumer picks it up. See importer/scripts/import-world.sh.
    include_creatures: Vec<u64>,
    bbox: Option<(f64, f64, f64, f64)>, // --box X0,X1,Y0,Y1: a coordinate rectangle that REPLACES the
    // radius sphere for the spawn slice (a radius from a zone corner bleeds into neighbours; a box is exact).
    exclude: Option<(f64, f64, f64, f64)>, // --exclude X0,X1,Y0,Y1: a SUBTRACTED rectangle (in-box but
    // in-exclude → dropped). Carves an inner neighbour out of the box (e.g. the Stormwind front inside Elwynn).
    db: String,
    server: String,
    pub(crate) apply: bool,
    pub(crate) family: Option<String>, // --family <name> (work-item 216): reload ONE --dump family's
    // clear+reload instead of the full ETL. `None` = full run (byte-identical to pre-216 behavior).
    pub(crate) source_sha: String, // --source-sha <sha> (work-item 216): the resolved cmangos
                                   // classic-db commit this dump came from — threaded into every `stamp_import_meta` call this run
                                   // makes. Empty string ("") when not given (no external provenance to record).
}

fn parse_args() -> Result<Args> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I, S>(args: I) -> Result<Args>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut a = Args {
        dump: None,
        dbc: None,
        pack_client: None,
        dump_collision: None,
        nav: None,
        vmap: None,
        vmap_status: false,
        vmap_prepare_coverage: None,
        go_models: None,
        terrain: None,
        print_extents: false,
        spells: false,
        talents: false,
        only: Vec::new(),
        trainers: Vec::new(),
        world_profile: None,
        map: 0,
        include_maps: Vec::new(),
        include_creatures: Vec::new(),
        center: (-8949.95, -132.493, 83.5312), // human/Northshire start
        radius: 180.0,
        bbox: None,
        exclude: None,
        db: "lyracore".into(),
        server: "local".into(),
        apply: false,
        family: None,
        source_sha: String::new(),
    };
    let mut legacy_spatial_args: Vec<&'static str> = Vec::new();
    let mut it = args.into_iter().map(Into::into);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--dump" => a.dump = Some(it.next().context("--dump needs a path")?),
            "--terrain" => {
                a.terrain = Some(it.next().context("--terrain needs the client Data/ dir")?)
            }
            "--dump-collision" => {
                a.dump_collision = Some(
                    it.next()
                        .context("--dump-collision needs the client Data/ dir")?,
                )
            }
            "--nav" => a.nav = Some(it.next().context("--nav needs the client Data/ dir")?),
            "--vmap" => a.vmap = Some(it.next().context("--vmap needs the client Data/ dir")?),
            "--vmap-status" => a.vmap_status = true,
            "--vmap-prepare-coverage" => {
                let v = it
                    .next()
                    .context("--vmap-prepare-coverage needs a generation id")?;
                a.vmap_prepare_coverage = Some(
                    v.parse()
                        .context("--vmap-prepare-coverage generation id must be a u64")?,
                );
            }
            "--go-models" => {
                a.go_models = Some(
                    it.next()
                        .context("--go-models needs the client Data/ dir")?,
                )
            }
            "--dbc" => a.dbc = Some(it.next().context("--dbc needs the client Data/ dir")?),
            "--pack-client" => {
                a.pack_client = Some(
                    it.next()
                        .context("--pack-client needs the client Data/ dir")?,
                )
            }
            // Derive the operator's OWN spawn bounding box for --map from their dump instead of trusting a
            // remembered rectangle (work-item 206: the Westfall widening needed a way to compute --box from
            // real data, not from memory). Prints min/max X/Y/Z + spawn count and returns — writes nothing.
            "--print-extents" => a.print_extents = true,
            "--spells" => a.spells = true,
            "--talents" => a.talents = true,
            "--only" => {
                // Comma-separated spell ids → the additive allowlist. Fail loudly on a malformed id so
                // a typo can't silently import the wrong (or empty) set.
                let v = it
                    .next()
                    .context("--only needs comma-separated spell ids, e.g. 7386,1160")?;
                a.only = v
                    .split(',')
                    .map(|s| s.trim().parse::<u32>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--only wants comma-separated spell ids")?;
                if a.only.is_empty() {
                    bail!("--only given but no ids parsed");
                }
            }
            "--include-creatures" => {
                // Comma-separated creature entries → force-import regardless of --box/--exclude. Fail
                // loudly on a malformed entry (a typo would silently drop a quest giver).
                let v = it.next().context(
                    "--include-creatures needs comma-separated creature entries, e.g. 344,266,415",
                )?;
                a.include_creatures = v
                    .split(',')
                    .map(|s| s.trim().parse::<u64>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--include-creatures wants comma-separated creature entries")?;
                if a.include_creatures.is_empty() {
                    bail!("--include-creatures given but no entries parsed");
                }
            }
            "--trainer" => {
                // Repeatable `--trainer <entry>=<id,id,…>`: bind a per-class offering list to a trainer
                // CREATURE-TEMPLATE entry. The ids are emitted as game_trainer_spell rows with a
                // DBC-derived required_level + a level-keyed cost (NO cmangos value). Fail loudly on a
                // malformed spec so a typo can't silently skip a class's whole trainer tree.
                let v = it.next().context("--trainer needs <entry>=<id,id,…>")?;
                let (entry_s, ids_s) = v
                    .split_once('=')
                    .context("--trainer wants <entry>=<comma-ids>, e.g. 913=78,6674")?;
                let entry: u32 = entry_s
                    .trim()
                    .parse()
                    .context("--trainer <entry> must be a creature-template entry (u32)")?;
                let ids: Vec<u32> = ids_s
                    .split(',')
                    .map(|s| s.trim().parse::<u32>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--trainer wants comma-separated spell ids after '='")?;
                if ids.is_empty() {
                    bail!("--trainer {entry}= given but no ids parsed");
                }
                a.trainers.push((entry, ids));
            }
            "--world-profile" => {
                let name = it.next().context("--world-profile needs a profile name")?;
                if a.world_profile.is_some() {
                    bail!("--world-profile may be given only once");
                }
                a.world_profile = Some(WorldImportProfile::parse(&name)?);
            }
            "--map" => {
                legacy_spatial_args.push("--map");
                a.map = it.next().context("--map")?.parse()?;
            }
            "--include-map" => {
                legacy_spatial_args.push("--include-map");
                // Repeatable: a WHOLE additional map in the slice (work-item 226 — Deadmines, map 36).
                // Fail loudly on a malformed id so a typo can't silently skip the dungeon's content.
                let m: i64 = it
                    .next()
                    .context("--include-map <map id>")?
                    .parse()
                    .context("--include-map wants a map id (integer), e.g. 36")?;
                if !a.include_maps.contains(&m) {
                    a.include_maps.push(m);
                }
            }
            "--radius" => {
                legacy_spatial_args.push("--radius");
                a.radius = it.next().context("--radius")?.parse()?;
            }
            "--box" => {
                legacy_spatial_args.push("--box");
                // A coordinate rectangle X0,X1,Y0,Y1 (REPLACES --radius for the spawn slice). Fail
                // loudly on a malformed box so a typo can't silently import the wrong region.
                let v = it.next().context("--box X0,X1,Y0,Y1")?;
                let p: Vec<f64> = v
                    .split(',')
                    .map(|s| s.trim().parse::<f64>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--box wants four numbers: X0,X1,Y0,Y1")?;
                if p.len() != 4 {
                    bail!(
                        "--box wants exactly 4 comma-separated numbers, got {}",
                        p.len()
                    );
                }
                a.bbox = Some((
                    p[0].min(p[1]),
                    p[0].max(p[1]),
                    p[2].min(p[3]),
                    p[2].max(p[3]),
                ));
            }
            "--exclude" => {
                legacy_spatial_args.push("--exclude");
                // A rectangle SUBTRACTED from --box (in-box AND in-exclude → dropped). Same parse as --box.
                let v = it.next().context("--exclude X0,X1,Y0,Y1")?;
                let p: Vec<f64> = v
                    .split(',')
                    .map(|s| s.trim().parse::<f64>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--exclude wants four numbers: X0,X1,Y0,Y1")?;
                if p.len() != 4 {
                    bail!(
                        "--exclude wants exactly 4 comma-separated numbers, got {}",
                        p.len()
                    );
                }
                a.exclude = Some((
                    p[0].min(p[1]),
                    p[0].max(p[1]),
                    p[2].min(p[3]),
                    p[2].max(p[3]),
                ));
            }
            "--db" => a.db = it.next().context("--db")?,
            "--server" => a.server = it.next().context("--server")?,
            "--apply" => a.apply = true,
            "--family" => {
                // Reload ONE --dump family's clear+reload instead of the full ETL (work-item 216).
                // Validated against the fixed FAMILIES set below (right here, not deferred) so a typo
                // bails loudly before any dump parsing work happens.
                let v = it.next().context("--family <name>")?;
                if !FAMILIES.contains(&v.as_str()) {
                    bail!(
                        "--family {v}: unknown family (valid: {})",
                        FAMILIES.join(", ")
                    );
                }
                a.family = Some(v);
            }
            "--source-sha" => a.source_sha = it.next().context("--source-sha <sha>")?,
            "--center" => {
                legacy_spatial_args.push("--center");
                let v = it.next().context("--center X,Y,Z")?;
                // Fail loudly on a malformed center — silently defaulting a bad value to (0,0,0) or
                // keeping the default would import the WRONG slice without the operator noticing.
                let p: Vec<f64> = v
                    .split(',')
                    .map(|s| s.trim().parse::<f64>())
                    .collect::<std::result::Result<_, _>>()
                    .context("--center wants three numbers: X,Y,Z")?;
                if p.len() != 3 {
                    bail!(
                        "--center wants exactly 3 comma-separated numbers, got {}",
                        p.len()
                    );
                }
                a.center = (p[0], p[1], p[2]);
            }
            other => bail!("unknown arg: {other}"),
        }
    }
    if let Some(profile) = a.world_profile {
        if !legacy_spatial_args.is_empty() {
            legacy_spatial_args.sort_unstable();
            legacy_spatial_args.dedup();
            bail!(
                "--world-profile {} cannot be mixed with legacy spatial arguments: {}",
                profile.name(),
                legacy_spatial_args.join(", ")
            );
        }
    }
    // Need at least one input. `--dump` alone = cmangos creatures; `--dbc` alone = standalone DBC
    // mode (proof/checks); BOTH = cmangos creatures with DBC-resolved scale (display-derived).
    if a.dump.is_none()
        && a.dbc.is_none()
        && a.pack_client.is_none()
        && a.terrain.is_none()
        && a.dump_collision.is_none()
        && a.nav.is_none()
        && a.vmap.is_none()
        && !a.vmap_status
        && a.vmap_prepare_coverage.is_none()
        && a.go_models.is_none()
    {
        bail!("need an input: --dump <classic-db .sql[.gz]>, --dbc <client Data/ dir>, --terrain <client Data/ dir>, or --pack-client <client Data/ dir>");
    }
    // `--go-models` resolves gameobject_template rows (the cmangos dump) against client-owned M2
    // geometry — meaningless without a template source.
    if a.go_models.is_some() && a.dump.is_none() {
        bail!("--go-models needs --dump too (gameobject_template is the template source)");
    }
    // `--print-extents` reads `creature` rows out of the cmangos dump — meaningless without `--dump`.
    if a.print_extents && a.dump.is_none() {
        bail!("--print-extents is only valid with --dump (it reads the cmangos creature table)");
    }
    // `--only` is meaningless without the spell importer (it's the additive-allowlist for Spell.dbc).
    if !a.only.is_empty() && !a.spells {
        bail!("--only is only valid with --spells (the additive Spell.dbc allowlist)");
    }
    if !a.include_creatures.is_empty() && a.dump.is_none() {
        bail!("--include-creatures is only valid with --dump (it force-imports cmangos creature rows)");
    }
    // `--family` covers `--dump` families ONLY (work-item 216) — terrain/dbc/spells are separate
    // invocations with no family concept of their own.
    if a.family.is_some() && a.dump.is_none() {
        bail!("--family is only valid with --dump (it names one of the cmangos-dump ETL families)");
    }
    // `--include-map` extends the `--dump` content slice only (work-item 226): terrain NEVER rides it
    // (an instance map is WMO geometry — deliberately no ADT import, see terrain::map_dir's guard),
    // and the DBC/spell streams are map-agnostic.
    if !a.include_maps.is_empty() {
        if a.dump.is_none() {
            bail!("--include-map is only valid with --dump (it widens the cmangos content slice)");
        }
        if a.include_maps.contains(&a.map) {
            bail!(
                "--include-map {} duplicates --map {} (the primary map is already in the slice)",
                a.map,
                a.map
            );
        }
    }
    // Resolve and validate the complete scope before the caller opens a dump or client archive.
    // `build_dump_plan` resolves the same pure value once and then uses its predicate end to end.
    a.world_import_scope()?;
    // `--trainer` rides the spell importer too (it reads each offering's DBC spell_level). Auto-extend
    // the allowlist with the trainer ids so every offered wrapper's game_spell header is imported in the
    // SAME pass (so the required_level is DBC-derived and the buy path resolves a real rank). When ONLY
    // --trainer was given (no --only), this seeds the allowlist — the surgical/additive path stays on.
    if !a.trainers.is_empty() {
        if !a.spells {
            bail!(
                "--trainer is only valid with --spells (it derives required_level from Spell.dbc)"
            );
        }
        for (_entry, ids) in &a.trainers {
            for &id in ids {
                if !a.only.contains(&id) {
                    a.only.push(id);
                }
            }
        }
    }
    Ok(a)
}

impl Args {
    pub(crate) fn world_import_scope(&self) -> Result<WorldImportScope> {
        let mut scope = match self.world_profile {
            Some(profile) => WorldImportScope::canonical(profile),
            None => WorldImportScope::legacy(
                self.map,
                self.bbox,
                self.center,
                self.radius,
                self.exclude,
                self.include_maps.clone(),
            ),
        }?;
        // The legacy flag remains an explicit extension. Canonical dependencies live in the
        // profile catalogue so callers cannot silently omit or copy them.
        scope.extend_forced_creatures(&self.include_creatures);
        Ok(scope)
    }
}

/// One `spacetime call` as the CLI identity (the module owner after a local publish) — the shared
/// path for every reducer-based loader (spawns, gameobjects, terrain).
pub(crate) fn call_reducer(args: &Args, reducer: &str, payload: &str) -> Result<()> {
    call_reducer_args(args, reducer, &[payload])
}

/// Reducer call with separate positional arguments (generation lifecycle reducers carry both
/// immutable manifest metadata and a packed batch).
pub(crate) fn call_reducer_args(args: &Args, reducer: &str, values: &[&str]) -> Result<()> {
    let mut command = Command::new("spacetime");
    command.args(["call", "-s", &args.server, &args.db, reducer]);
    command.args(values);
    let out = command
        .output()
        .with_context(|| format!("run spacetime call {reducer}"))?;
    if !out.status.success() {
        bail!("{reducer} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// Stamp `family`'s import provenance via the `stamp_import_meta` reducer (work-item 216) — called
/// once per family that ACTUALLY ran, after the apply succeeds. Unlike `call_reducer`'s single
/// packed-string payload (the spawn/gameobject batch shape), `stamp_import_meta` takes 4 separate
/// positional args, so each rides its own CLI argument — the same multi-arg `spacetime call`
/// convention `debug_setup_gather_pool`'s doc comment demonstrates (module/src/debug.rs).
pub(crate) fn stamp_family(
    args: &Args,
    family: &str,
    source_sha: &str,
    file_hash: &str,
    row_count: u64,
) -> Result<()> {
    let row_count_s = row_count.to_string();
    let out = Command::new("spacetime")
        .args([
            "call",
            "-s",
            &args.server,
            &args.db,
            "stamp_import_meta",
            family,
            source_sha,
            file_hash,
            &row_count_s,
        ])
        .output()
        .with_context(|| format!("stamp_import_meta({family})"))?;
    if !out.status.success() {
        bail!(
            "stamp_import_meta({family}) failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// sha256 hex digest of `bytes` (work-item 216: `--source-sha`/provenance). This crate's `Cargo.lock`
/// carries no `sha2` dependency, so — mirroring this importer's existing convention of shelling out to
/// `spacetime` for reducer calls rather than adding a client SDK dependency — this shells out to the
/// system `sha256sum` instead of adding one. Feeds the dump bytes on stdin (`sha256sum -` reads
/// stdin), so no temp file is needed. Returns the lowercase hex digest (`sha256sum`'s first
/// whitespace-delimited field).
pub(crate) fn sha256_hex(bytes: &[u8]) -> Result<String> {
    use std::io::Write;
    let mut child = Command::new("sha256sum")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("spawn sha256sum (is it on PATH?)")?;
    child
        .stdin
        .take()
        .context("sha256sum: no stdin handle")?
        .write_all(bytes)
        .context("write dump bytes to sha256sum stdin")?;
    let out = child.wait_with_output().context("wait for sha256sum")?;
    if !out.status.success() {
        bail!("sha256sum failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let digest = stdout
        .split_whitespace()
        .next()
        .with_context(|| format!("sha256sum produced no output: {stdout:?}"))?;
    Ok(digest.to_string())
}

pub(crate) fn read_dump(path: &str) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("read {path}"))?;
    // mysqldumps are latin1/utf8-ish and may carry stray non-UTF8 bytes; decode lossily on BOTH paths
    // (read_to_string would hard-fail the .gz path on the first bad byte — the plain path never did).
    let raw = if path.ends_with(".gz") {
        let mut v = Vec::new();
        GzDecoder::new(&bytes[..])
            .read_to_end(&mut v)
            .context("gunzip dump")?;
        v
    } else {
        bytes
    };
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// Clear+reload SQL for the two cmangos stat-curve tables (importer P3): `game_class_level_stats`
/// (base HP/mana per class+level) + `game_level_stats` (attributes per race+class+level). Both are
/// no-Timestamp → plain SQL. Keys are packed to match module/src/stats.rs: `(class<<8)|level` and
/// `(race<<16)|(class<<8)|level`. Global (not zone-filtered). Chunked under 500 rows/INSERT.
fn build_stats_sql(dump: &str) -> Vec<String> {
    let mut stmts: Vec<String> = vec![
        "DELETE FROM game_class_level_stats WHERE class_level >= 0".into(),
        "DELETE FROM game_level_stats WHERE race_class_level >= 0".into(),
    ];
    let cls_rows: Vec<String> = parse_table(dump, "player_classlevelstats")
        .iter()
        .filter_map(|r| {
            let class: u32 = field(r, cls::CLASS).parse().ok()?;
            let level: u32 = field(r, cls::LEVEL).parse().ok()?;
            Some(format!(
                "({},{class},{level},{},{})",
                (class << 8) | (level & 0xFF),
                field(r, cls::BASEHP),
                field(r, cls::BASEMANA),
            ))
        })
        .collect();
    push_insert(
        &mut stmts,
        "game_class_level_stats",
        "class_level,class,level,base_health,base_mana",
        &cls_rows,
    );
    let lvl_rows: Vec<String> = parse_table(dump, "player_levelstats")
        .iter()
        .filter_map(|r| {
            let race: u32 = field(r, lvl::RACE).parse().ok()?;
            let class: u32 = field(r, lvl::CLASS).parse().ok()?;
            let level: u32 = field(r, lvl::LEVEL).parse().ok()?;
            Some(format!(
                "({},{race},{class},{level},{},{},{},{},{})",
                (race << 16) | (class << 8) | (level & 0xFF),
                field(r, lvl::STR),
                field(r, lvl::AGI),
                field(r, lvl::STA),
                field(r, lvl::INTE),
                field(r, lvl::SPI),
            ))
        })
        .collect();
    push_insert(
        &mut stmts,
        "game_level_stats",
        "race_class_level,race,class,level,strength,agility,stamina,intellect,spirit",
        &lvl_rows,
    );
    eprintln!(
        "stats: {} class/level rows, {} race/class/level rows",
        cls_rows.len(),
        lvl_rows.len()
    );
    stmts
}

/// Clear+reload SQL for `game_start_position` from cmangos `playercreateinfo` (importer P1) — the
/// per-(race,class) start coords/map/zone for character creation. No Timestamp → plain SQL. PK packs
/// `(race<<8)|class` (matching the module). `display_id` is now vestigial (player display comes from
/// `game_race_info` at login) — emit 0.
fn build_start_position_sql(dump: &str) -> Vec<String> {
    let rows: Vec<String> = parse_table(dump, "playercreateinfo")
        .iter()
        .filter_map(|r| {
            let race: u32 = field(r, pc::RACE).parse().ok()?;
            let class: u32 = field(r, pc::CLASS).parse().ok()?;
            Some(format!(
                "({},{race},{class},{},{},{},{},{},{},0)",
                (race << 8) | class,
                field(r, pc::MAP),
                field(r, pc::ZONE),
                field(r, pc::X),
                field(r, pc::Y),
                field(r, pc::Z),
                field(r, pc::O),
            ))
        })
        .collect();
    let mut stmts = vec!["DELETE FROM game_start_position WHERE race_class >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_start_position",
        "race_class,race,class,map_id,zone_id,x,y,z,orientation,display_id",
        &rows,
    );
    eprintln!("start positions: {} (race,class) rows", rows.len());
    stmts
}

/// Clear+reload SQL for `game_graveyard_zone` from cmangos `game_graveyard_zone` (work-item 209): the
/// zone (+ optional faction restriction) each imported graveyard (`game_graveyard`, loaded separately
/// from `WorldSafeLocs.dbc` by `dbc.rs::graveyard_sql`) serves. `module/src/world.rs`'s
/// `graveyard::resolve_graveyard` reads this via the `by_zone` index to prefer a zone-linked
/// graveyard over a merely-closer unlinked one. GLOBAL, not box-scoped (a small table, like
/// `build_start_position_sql` above) — every graveyard/zone link imports regardless of the creature
/// ETL's map/box filter. No Timestamp → plain SQL.
fn build_graveyard_zone_sql(dump: &str) -> Vec<String> {
    let rows: Vec<String> = parse_table(dump, "game_graveyard_zone")
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let safe_loc_id: u32 = field(r, gz::ID).parse().ok()?;
            let zone_id: u32 = field(r, gz::GHOST_ZONE).parse().ok()?;
            let faction: u32 = field(r, gz::FACTION).parse().ok()?;
            Some(format!(
                "({},{safe_loc_id},{zone_id},{faction})",
                i as u64 + 1
            ))
        })
        .collect();
    let mut stmts = vec!["DELETE FROM game_graveyard_zone WHERE row_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_graveyard_zone",
        "row_id,safe_loc_id,zone_id,faction",
        &rows,
    );
    eprintln!("graveyard zones: {} rows", rows.len());
    stmts
}

/// Clear+reload SQL for `game_areatrigger_teleport` from cmangos `areatrigger_teleport` (work-item 225):
/// dungeon entrance/exit portals — trigger id → target map + destination position. `id` here is the
/// SAME `AreaTrigger.dbc` trigger id `game_area_trigger` (209, `dbc.rs::area_trigger_sql`) carries as
/// its primary key: 209 imports the trigger's GEOMETRY (position/shape, from the client DBC), this
/// imports its TARGET (from the cmangos world dump) — two independent sources joined only by sharing
/// the same id space, exactly like `game_graveyard`/`game_graveyard_zone`'s split. GLOBAL, not
/// box-scoped, on purpose: an areatrigger's own geometry can sit anywhere (Moonbrook's entrance is in
/// Westfall, its exit is inside map 36), and a portal a party can walk into from outside today's
/// creature-ETL box must still resolve — box-scoping this table would silently drop a portal whose
/// TRIGGER lands outside the box even though the trigger volume itself (209's `game_area_trigger`) is
/// never box-filtered either. Same "small global table, own clear+reload" shape as
/// `build_graveyard_zone_sql` above. `[V]` column layout — see the `at` module's doc comment; no dump
/// in this sandbox to confirm against, and no neighbor-anchor is possible for this brand-new table.
fn build_areatrigger_teleport_sql(dump: &str) -> (Vec<String>, usize) {
    let rows: Vec<String> = parse_table(dump, "areatrigger_teleport")
        .iter()
        .filter_map(|r| {
            let trigger_id: u32 = field(r, at::ID).parse().ok()?;
            let target_map: u32 = field(r, at::TARGET_MAP).parse().ok()?;
            let x: f32 = field(r, at::TARGET_X).parse().ok()?;
            let y: f32 = field(r, at::TARGET_Y).parse().ok()?;
            let z: f32 = field(r, at::TARGET_Z).parse().ok()?;
            let o: f32 = field(r, at::TARGET_O).parse().ok()?;
            let name = sql_text(field(r, at::NAME));
            Some(format!(
                "({trigger_id},{target_map},{x},{y},{z},{o},{name})"
            ))
        })
        .collect();
    let n = rows.len();
    let mut stmts = vec!["DELETE FROM game_areatrigger_teleport WHERE trigger_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_areatrigger_teleport",
        "trigger_id,target_map,x,y,z,o,name",
        &rows,
    );
    eprintln!("areatrigger_teleport: {n} portal rows");
    (stmts, n)
}

/// Count the distinct `(race, class)` combos among rows already formatted as `"(id,race,class,...)"`
/// SQL tuples — shared coverage-print helper for the three `playercreateinfo_*` builders below (each
/// row string always starts `(id,race,class,`, so a light textual split is enough; no need to re-parse
/// the source dump rows a second time).
fn distinct_race_class_combos(rows: &[(u32, u32)]) -> usize {
    let mut combos: Vec<(u32, u32)> = rows.to_vec();
    combos.sort_unstable();
    combos.dedup();
    combos.len()
}

/// Clear+reload SQL for `game_createinfo_spell` from cmangos `playercreateinfo_spell` (work-item 212).
/// **FLIPS THE SOURCE FROM SEED TO IMPORT**: `module/src/seed.rs::seed_createinfo_spells` only inserts
/// the hand-authored `CREATEINFO_KIT` rows if the table is EMPTY (the `game_start_position` / Talent.dbc
/// (207) precedent — an importer clear+reload REPLACES whatever is there, seed or a prior import; it
/// does not merge or coexist with it). The `id >= 0` DELETE is a tautology on the unsigned PK (mirrors
/// 207's own phrasing) — it's a wholesale wipe, not a conditional one. A character created BEFORE this
/// import keeps the `game_player_spell` rows ITS creation already copied (durable, per-character); only
/// characters created AFTER the import see the real dump-sourced kit. `race`/`class` are NOT
/// re-validated as wildcards here — a dump row's own 0 (if any) passes through verbatim and
/// `createinfo_row_matches` (module/src/spell/spellbook.rs) already treats 0 as a wildcard on read,
/// so this importer needs no wildcard-specific handling. ALL races import (not just Human) — cheap, and
/// race unlock becomes a pure gate flip later. No Timestamp → plain SQL.
fn build_createinfo_spell_sql(dump: &str) -> Vec<String> {
    let mut race_class_pairs: Vec<(u32, u32)> = Vec::new();
    let rows: Vec<String> = parse_table(dump, "playercreateinfo_spell")
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let race: u32 = field(r, pcs::RACE).parse().ok()?;
            let class: u32 = field(r, pcs::CLASS).parse().ok()?;
            let spell: u32 = field(r, pcs::SPELL).parse().ok()?;
            race_class_pairs.push((race, class));
            Some(format!("({},{race},{class},{spell})", i as u64 + 1))
        })
        .collect();
    let mut stmts = vec!["DELETE FROM game_createinfo_spell WHERE id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_createinfo_spell",
        "id,race,class,spell_id",
        &rows,
    );
    eprintln!(
        "createinfo spells: {} rows across {} (race,class) combos",
        rows.len(),
        distinct_race_class_combos(&race_class_pairs)
    );
    stmts
}

/// Clear+reload SQL for `game_createinfo_action` from cmangos `playercreateinfo_action` (work-item 212)
/// — the vanilla default action-bar layout per (race, class), read by `module/src/action_bar.rs`'s
/// `grant_createinfo_actions` at character creation (copied into `game_player_action`) and, when a
/// character has none (pre-import), by the gateway's login synth fallback
/// (`gateway/src/codec/entity.rs::login_sequence_messages`). This table has NO hand-authored seed
/// counterpart (unlike `game_createinfo_spell`) — an empty table is the fallback signal itself, not a
/// transitional pre-import state to worry about orphaning. No Timestamp → plain SQL.
fn build_createinfo_action_sql(dump: &str) -> Vec<String> {
    let mut race_class_pairs: Vec<(u32, u32)> = Vec::new();
    let rows: Vec<String> = parse_table(dump, "playercreateinfo_action")
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let race: u32 = field(r, pca::RACE).parse().ok()?;
            let class: u32 = field(r, pca::CLASS).parse().ok()?;
            let button: u32 = field(r, pca::BUTTON).parse().ok()?;
            let action: u32 = field(r, pca::ACTION).parse().ok()?;
            let action_type: u32 = field(r, pca::TYPE).parse().ok()?;
            race_class_pairs.push((race, class));
            Some(format!(
                "({},{race},{class},{button},{action},{action_type})",
                i as u64 + 1
            ))
        })
        .collect();
    let mut stmts = vec!["DELETE FROM game_createinfo_action WHERE row_id >= 0".to_string()];
    push_insert(
        &mut stmts,
        "game_createinfo_action",
        "row_id,race,class,button,action,action_type",
        &rows,
    );
    eprintln!(
        "createinfo actions: {} rows across {} (race,class) combos",
        rows.len(),
        distinct_race_class_combos(&race_class_pairs)
    );
    stmts
}

/// `game_start_item.id` floor for `playercreateinfo_item` EXTRAS (work-item 212) — a disjoint range
/// from `dbc.rs::start_item_sql`'s own densely-assigned-from-1 CharStartOutfit ids, so the two writers'
/// clear+reload DELETEs never touch each other's rows regardless of which pass runs (the canonical
/// `importer/scripts/import-world.sh` runs the `--dump` pass THEN the standalone `--dbc` pass — without a
/// disjoint range the later `--dbc` pass's `DELETE FROM game_start_item WHERE id >= 0` would wipe
/// these extras every single run). Comfortably above any realistic CharStartOutfit row count (a few
/// hundred rows across every race/class/gender combo).
const CREATEINFO_ITEM_ID_FLOOR: u64 = 1_000_000;

/// Clear+reload SQL for the `playercreateinfo_item` EXTRAS into `game_start_item` (work-item 212) — the
/// dump's per-(race,class) starter items BEYOND `CharStartOutfit.dbc`'s outfit (today's sole source —
/// see `dbc.rs::start_item_sql`). Deduped against the outfit's own (race_class, item) pairs when
/// `--dbc` is also given (via `dbc::outfit_item_set`) so the SAME item for the SAME (race, class) is
/// never listed twice — `grant_starter_item` (module/src/items/ops.rs) has no dedup of its own, it
/// trusts `game_start_item` to already be clean. Without `--dbc` (no client data available), dedup is
/// skipped — there is nothing to dedup against in that case anyway (a `--dump`-only run has never
/// loaded any outfit rows). Ids are assigned densely from `CREATEINFO_ITEM_ID_FLOOR` (see its doc) —
/// this function's own DELETE only ever touches that range. No Timestamp → plain SQL.
fn build_createinfo_item_sql(dump: &str, dbc_dir: Option<&str>) -> Result<Vec<String>> {
    let outfit_seen: std::collections::HashSet<(u32, i32)> = match dbc_dir {
        Some(dir) => {
            let mut chain = dbc::open_chain(std::path::Path::new(dir))?;
            dbc::outfit_item_set(&mut chain)?
        }
        None => std::collections::HashSet::new(),
    };
    let mut race_class_pairs: Vec<(u32, u32)> = Vec::new();
    let mut seen_extra: std::collections::HashSet<(u32, i32)> = std::collections::HashSet::new();
    let mut deduped = 0usize;
    let mut id = CREATEINFO_ITEM_ID_FLOOR;
    let mut rows: Vec<String> = Vec::new();
    for r in parse_table(dump, "playercreateinfo_item").iter() {
        let (Some(race), Some(class), Some(item)) = (
            field(r, pci::RACE).parse::<u32>().ok(),
            field(r, pci::CLASS).parse::<u32>().ok(),
            field(r, pci::ITEM).parse::<i32>().ok(),
        ) else {
            continue;
        };
        if item <= 0 {
            continue;
        }
        let race_class = (race << 8) | class;
        if outfit_seen.contains(&(race_class, item)) {
            deduped += 1;
            continue; // already granted by CharStartOutfit for this (race, class) — skip the dupe
        }
        if !seen_extra.insert((race_class, item)) {
            continue; // the dump itself lists this (race,class,item) more than once
        }
        race_class_pairs.push((race, class));
        rows.push(format!("({id},{race_class},{item})"));
        id += 1;
    }
    let n = rows.len();
    let mut stmts = vec![format!(
        "DELETE FROM game_start_item WHERE id >= {CREATEINFO_ITEM_ID_FLOOR}"
    )];
    push_insert(
        &mut stmts,
        "game_start_item",
        "id,race_class,item_entry",
        &rows,
    );
    eprintln!(
        "createinfo item extras: {n} rows across {} (race,class) combos ({deduped} deduped against the CharStartOutfit outfit)",
        distinct_race_class_combos(&race_class_pairs)
    );
    Ok(stmts)
}

/// Clear+reload SQL for `game_spell_chain` from cmangos `spell_chain` (work-item 102, REDUCED SCOPE —
/// only the two module-only tables that had no overlap with an already-covered source: talent's
/// `rank_spell_N` (207) covers TALENT ranks, not class-spell ranks, so this is genuinely new data).
/// `module/src/trainer.rs`'s rank-prereq gate (`apply_trainer_buy`) reads this via `spell_id` (the PK)
/// to require the previous rank already known before a higher rank can be trained. No Timestamp → plain
/// SQL. `spell_id > 0` in the DELETE (not `>= 0`, unlike the unsigned-PK tables elsewhere) matches the
/// module's `u32` PK exactly and is still a wholesale wipe (every real spell_id is > 0).
fn build_spell_chain_sql(dump: &str) -> Vec<String> {
    let rows: Vec<String> = parse_table(dump, "spell_chain")
        .iter()
        .filter_map(|r| {
            let spell_id: u32 = field(r, sc::SPELL_ID).parse().ok()?;
            // Defense-in-depth (102 review finding, mirrors build_createinfo_item_sql's guard): a
            // hypothetical spell_id-0 row would slip past the `WHERE spell_id > 0` DELETE and PK-collide
            // loudly on the NEXT reload — skip it here so DELETE and INSERT stay consistent.
            if spell_id == 0 {
                return None;
            }
            let prev_spell: u32 = field(r, sc::PREV_SPELL).parse().ok()?;
            let first_spell: u32 = field(r, sc::FIRST_SPELL).parse().ok()?;
            let rank: u8 = field(r, sc::RANK).parse().ok()?;
            let req_spell: u32 = field(r, sc::REQ_SPELL).parse().ok()?;
            Some(format!(
                "({spell_id},{prev_spell},{first_spell},{rank},{req_spell})"
            ))
        })
        .collect();
    let mut stmts = vec!["DELETE FROM game_spell_chain WHERE spell_id > 0".to_string()];
    push_insert(
        &mut stmts,
        "game_spell_chain",
        "spell_id,prev_spell,first_spell,rank,req_spell",
        &rows,
    );
    eprintln!("spell chain: {} rows", rows.len());
    stmts
}

/// Clear+reload SQL for `game_spell_learn` from cmangos `spell_learn_spell` (work-item 102, reduced
/// scope) — auto-taught dependents (e.g. a weapon-skill spell that also grants a companion passive).
/// `module/src/spell/spellbook.rs::learn_spell_with_dependents` reads this via `parent_spell` (the
/// `by_parent` index) from the trainer-buy grant path only (see that fn's doc). `Active` is PARSED
/// (named in `sls::ACTIVE`, kept as an explicit column read rather than silently skipped) but NOT used
/// to filter rows this slice — cmangos classic-db ships every row Active=1 in practice, and gating on
/// it would need a documented "what does Active=0 mean here" decision this reduced scope doesn't need
/// to make; a later item can add the filter once verified against a real dump. `id` is assigned
/// densely from 1 (the `#[auto_inc]` PK, like `game_trainer_spell`/`game_createinfo_spell` above — the
/// importer supplies explicit ids for a direct SQL INSERT rather than relying on live auto-assignment).
/// No Timestamp → plain SQL.
///
/// Also synthesizes the two armor-proficiency wrapper→passive rows (verified against ClassicDB 1.12):
/// Plate Mail wrapper `PLATE_TRAINER_SPELL_ID` (7109) → passive `PLATE_PASSIVE_SPELL_ID` (750), and
/// Mail wrapper `MAIL_TRAINER_SPELL_ID` (8738) → passive `MAIL_PASSIVE_SPELL_ID` (8737). cmangos
/// `spell_learn_spell` carries no rows for these parents — the module's
/// `learn_spell_with_dependents` (`module/src/spell/spellbook.rs`) needs them to grant the passive the
/// equip Gate reads when a class trainer sells the wrapper. Skipped if the dump ever grows a real row
/// for either parent, so a future upstream fix can't collide with the synthesized one.
fn build_spell_learn_sql(dump: &str) -> Vec<String> {
    use lyracore_shared::constants::armor_proficiency;

    let mut parents_seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut rows: Vec<String> = parse_table(dump, "spell_learn_spell")
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let parent_spell: u32 = field(r, sls::ENTRY).parse().ok()?;
            let learn_spell: u32 = field(r, sls::SPELL_ID).parse().ok()?;
            let _active: u8 = field(r, sls::ACTIVE).parse().unwrap_or(1); // parsed, unused (see doc)
            parents_seen.insert(parent_spell);
            Some(format!("({},{parent_spell},{learn_spell})", i as u64 + 1))
        })
        .collect();
    let mut next_id = rows.len() as u64 + 1;
    for (parent, learn) in [
        (
            armor_proficiency::PLATE_TRAINER_SPELL_ID,
            armor_proficiency::PLATE_PASSIVE_SPELL_ID,
        ),
        (
            armor_proficiency::MAIL_TRAINER_SPELL_ID,
            armor_proficiency::MAIL_PASSIVE_SPELL_ID,
        ),
    ] {
        if parents_seen.contains(&parent) {
            continue; // dump already carries a real spell_learn_spell row for this wrapper
        }
        rows.push(format!("({next_id},{parent},{learn})"));
        next_id += 1;
    }
    let mut stmts = vec!["DELETE FROM game_spell_learn WHERE id > 0".to_string()];
    push_insert(
        &mut stmts,
        "game_spell_learn",
        "id,parent_spell,learn_spell",
        &rows,
    );
    eprintln!("spell learn dependents: {} rows", rows.len());
    stmts
}

/// Map the literal `NULL` token (and pure whitespace) to an empty string for an INTERNAL string
/// comparison — `sql_text` does the SQL-literal-quoted version of the same collapse for values that
/// go straight into an INSERT.
fn null_to_empty(s: &str) -> &str {
    let t = s.trim();
    if t == "NULL" {
        ""
    } else {
        t
    }
}

/// One npc_text slot: `(male, female, probability)`. Empty strings + probability 0.0 = unused slot.
type NpcTextSlot = (String, String, f32);

/// Parse ALL 8 weighted npc_text slots from a raw dump row (work-item 217 — the pre-217 importer only
/// kept the first non-empty slot; the client does its own weighted pick over the full 8, see
/// `gateway::codec::build_npc_text_update`). Handles both schema vintages `npc_text_first_nonempty`
/// already distinguished:
///   • Full 81-col rows: male/female text + probability embedded directly per slot.
///   • Short 17-col rows: only a probability + a `broadcast_text` id per slot (the text lives in the
///     `broadcast` map, resolved by the caller).
/// An unrecognized row shape yields 8 empty/zero slots (same "nothing found" outcome as before).
fn parse_npc_text_slots(
    row: &[String],
    broadcast: &HashMap<u64, (String, String)>,
) -> [NpcTextSlot; 8] {
    core::array::from_fn(|slot| {
        if row.len() >= npct::NCOLS_FULL {
            let base = npct::full_slot_base(slot);
            let male = null_to_empty(field(row, base + npct::FULL_SLOT_MALE_OFFSET)).to_string();
            let female =
                null_to_empty(field(row, base + npct::FULL_SLOT_FEMALE_OFFSET)).to_string();
            let prob: f32 = field(row, base + npct::FULL_SLOT_PROB_OFFSET)
                .parse()
                .unwrap_or(0.0);
            (male, female, prob)
        } else if row.len() == npct::NCOLS_SHORT {
            let prob: f32 = field(row, npct::short_slot_prob(slot))
                .parse()
                .unwrap_or(0.0);
            let bcast_id: u64 = field(row, npct::short_slot_bcast_id(slot))
                .parse()
                .unwrap_or(0);
            let (male, female) = broadcast.get(&bcast_id).cloned().unwrap_or_default();
            (male, female, prob)
        } else {
            (String::new(), String::new(), 0.0)
        }
    })
}

/// The first non-empty (male-then-female) text across all 8 slots — the back-compat single string
/// (`game_npc_text.text`, matching the pre-217 single-slot importer behavior), derived from the SAME
/// parsed slots the multi-slot table uses, so the two can never disagree.
fn slots_first_nonempty(slots: &[NpcTextSlot; 8]) -> String {
    for (male, female, _) in slots {
        if !male.is_empty() {
            return male.clone();
        }
        if !female.is_empty() {
            return female.clone();
        }
    }
    String::new()
}

/// Resolve a `gossip_menu_option` row's mangos condition (inline columns, a `conditions`-table FK, or
/// no condition support at all in this dump vintage) into our minimal `(cond_type, value1, value2)`
/// set (`lyracore_shared::constants::gossip_condition`). An unmapped/unsupported mangos condition type
/// FAILS OPEN (`NONE` = always shown) rather than silently hiding the option, and is logged so the
/// operator can extend the mapping for their dump.
fn resolve_gossip_option_condition(
    row: &[String],
    conditions: &HashMap<u64, (u64, u64)>,
) -> (u32, u32, u32) {
    use lyracore_shared::constants::gossip_condition;
    let map = |mangos_type: u64, value1: u64| -> (u32, u32, u32) {
        match mangos_type {
            cond::MANGOS_QUESTTAKEN => (gossip_condition::QUEST_TAKEN, value1 as u32, 0),
            cond::MANGOS_QUESTREWARDED => (gossip_condition::QUEST_REWARDED, value1 as u32, 0),
            // Fail-CLOSED, against this map's fail-open default: a seasonal option shown year-round
            // is a wrong pitch, where a hidden one is only absent.
            cond::MANGOS_ACTIVE_GAME_EVENT => (gossip_condition::NEVER, 0, 0),
            _ => (gossip_condition::NONE, 0, 0),
        }
    };
    // 15-col vintage (THIS dump): trailing generic-conditions FK at 14 — checked FIRST (its col 11
    // is box_money, which the legacy inline branch would misread as a condition type).
    if row.len() >= gmo::NCOLS_15_CONDITION_FK {
        let cond_id: u64 = field(row, gmo::CONDITION_ID_15).parse().unwrap_or(0);
        if cond_id == 0 {
            return (lyracore_shared::constants::gossip_condition::NONE, 0, 0);
        }
        let Some(&(t, v1)) = conditions.get(&cond_id) else {
            eprintln!(
                "gossip option: dangling condition_id {cond_id} on menu {} option {} — fail-open",
                field(row, gmo::MENU_ID),
                field(row, gmo::ID)
            );
            return (lyracore_shared::constants::gossip_condition::NONE, 0, 0);
        };
        let resolved = map(t, v1);
        if resolved.0 == lyracore_shared::constants::gossip_condition::NONE {
            eprintln!(
                "gossip option: unsupported condition type {t} (id {cond_id}) on menu {} option {} — fail-open",
                field(row, gmo::MENU_ID), field(row, gmo::ID)
            );
        }
        return resolved;
    }
    if row.len() >= gmo::NCOLS_WITH_INLINE_COND {
        let t: u64 = field(row, gmo::INLINE_COND_TYPE).parse().unwrap_or(0);
        let v1: u64 = field(row, gmo::INLINE_COND_VALUE1).parse().unwrap_or(0);
        let resolved = map(t, v1);
        if t != 0 && resolved.0 == gossip_condition::NONE {
            eprintln!(
                "gossip option: unsupported inline condition type {t} on menu {} option {} — fail-open (always shown)",
                field(row, gmo::MENU_ID), field(row, gmo::ID)
            );
        }
        return resolved;
    }
    if row.len() >= gmo::NCOLS_WITH_CONDITION_FK {
        let cond_id: u64 = field(row, gmo::CONDITION_ID).parse().unwrap_or(0);
        if cond_id != 0 {
            if let Some(&(t, v1)) = conditions.get(&cond_id) {
                let resolved = map(t, v1);
                if resolved.0 == gossip_condition::NONE {
                    eprintln!(
                        "gossip option: unsupported condition type {t} (condition_entry {cond_id}) — fail-open (always shown)"
                    );
                }
                return resolved;
            }
        }
    }
    (gossip_condition::NONE, 0, 0)
}

/// Resolve a `gossip_menu_option` row's clickable label: mirrors the dual-schema handling
/// `parse_npc_text_slots` already does for `npc_text` — a new-schema dump's `option_text` column
/// carries a `broadcast_text` id (a bare integer) instead of literal text. Tries the broadcast-id
/// interpretation FIRST, but only trusts it when the id actually resolves (a real hand-written option
/// label is never a bare number that happens to collide with a live `broadcast_text` id, so this
/// ordering is safe for both vintages); falls through to the literal column otherwise.
fn resolve_gossip_option_text(
    row: &[String],
    broadcast: &HashMap<u64, (String, String)>,
) -> String {
    let raw = field(row, gmo::OPTION_TEXT);
    if let Ok(bcast_id) = raw.trim().parse::<u64>() {
        if let Some((male, female)) = broadcast.get(&bcast_id) {
            if !male.is_empty() {
                return male.clone();
            }
            if !female.is_empty() {
                return female.clone();
            }
        }
    }
    null_to_empty(raw).to_string()
}

/// Reclassify a gossip option's `action` by its resolved TEXT, for the handful of cmangos rows whose
/// real behavior is gated in C++ code at `GossipHello`, not by the dump's `OptionType` column — so
/// the column the importer copies through unchanged (see `gossip_option`'s doc) doesn't carry it.
/// Confirmed empirically (#516): every "I wish to unlearn my talents." row across the live dump
/// (38/38) imports with `OptionType=1` (`gossip_option::GOSSIP`), indistinguishable from a plain
/// gossip line, so `filtered_gossip_options`' level-10 gate has nothing to key on without this pass.
/// Falls through to the raw `action` for every other row (the importer-reclassify-by-name pattern —
/// same shape as the wolf-faction and spell-kind reclassification passes).
fn reclassify_gossip_option_action(text: &str, raw_action: u32) -> u32 {
    if text == "I wish to unlearn my talents." {
        lyracore_shared::constants::gossip_option::UNLEARNTALENTS
    } else {
        raw_action
    }
}

/// Build `game_gossip_menu` + `game_npc_text` + `game_npc_text_slot` + `game_gossip_option` SQL rows
/// from the cmangos dump for the given in-box creature template entries. Two `npc_text` data paths:
///   • Full 81-col npc_text rows → text extracted directly.
///   • Short 17-col rows (broadcast_text schema) → BroadcastTextId0..7 → broadcast_text.Text.
/// Returns `GossipEtl` (INSERT value-tuples per table).
struct GossipEtl {
    menu_rows: Vec<String>,
    npc_text_rows: Vec<String>,
    npc_text_slot_rows: Vec<String>,
    option_rows: Vec<String>,
}

fn build_gossip_sql(dump: &str, creature_entries: &std::collections::HashSet<u64>) -> GossipEtl {
    use std::collections::{HashMap, HashSet};

    // 1. Build broadcast_text id → (male, female) text map (new-schema npc_text AND gossip options).
    let mut broadcast: HashMap<u64, (String, String)> = HashMap::new();
    for row in parse_table(dump, "broadcast_text") {
        let id: u64 = field(&row, bt::ID).parse().unwrap_or(0);
        if id == 0 {
            continue;
        }
        let male = null_to_empty(field(&row, bt::TEXT)).to_string();
        let female = null_to_empty(field(&row, bt::TEXT1)).to_string();
        if !male.is_empty() || !female.is_empty() {
            broadcast.insert(id, (male, female));
        }
    }

    // 2. Build npc_text id → (first-nonempty string, full 8-slot array) maps (both row schemas).
    let mut npc_texts: HashMap<u64, String> = HashMap::new();
    let mut npc_text_slots: HashMap<u64, [NpcTextSlot; 8]> = HashMap::new();
    for row in parse_table(dump, "npc_text") {
        let id: u64 = field(&row, npct::ID).parse().unwrap_or(0);
        if id == 0 {
            continue;
        }
        let slots = parse_npc_text_slots(&row, &broadcast);
        let text = slots_first_nonempty(&slots);
        if !text.is_empty() {
            npc_texts.insert(id, text);
            npc_text_slots.insert(id, slots);
        }
    }

    // 3. Parse gossip_menu (cmangos): collect (entry, text_id) pairs for in-box creatures.
    //    gossip_menu.entry = creature_template.GossipMenuId. We emit ONE row per creature_entry. An
    //    entry commonly has several rows — one unconditional greeting plus condition-gated variants —
    //    and row order does not rank them, so the unconditional row is the default title.
    let mut menu_to_text: HashMap<u64, u64> = HashMap::new();
    let mut menu_text_is_conditional: HashMap<u64, bool> = HashMap::new();
    for row in parse_table(dump, "gossip_menu") {
        let entry: u64 = field(&row, gm::ENTRY).parse().unwrap_or(0);
        let text_id: u64 = field(&row, gm::TEXT_ID).parse().unwrap_or(0);
        if entry == 0 || text_id == 0 {
            continue;
        }
        let conditional = field(&row, gm::CONDITION_ID).parse::<u64>().unwrap_or(0) != 0;
        // Among rows of the same kind the first seen wins, so the choice is deterministic.
        let held_conditional = menu_text_is_conditional.get(&entry).copied();
        if held_conditional.is_none() || (held_conditional == Some(true) && !conditional) {
            menu_to_text.insert(entry, text_id);
            menu_text_is_conditional.insert(entry, conditional);
        }
    }

    // 4. Map creature_entry → gossip_menu_id → text_id, collecting all needed text_ids. Also collect
    //    creature_entry → gossip_menu_id UNCONDITIONALLY (needed_text_ids aside) for the option pass
    //    below — a creature can have options without a resolvable title text, and vice versa.
    // Fallback: many cmangos dumps have npc_text id = creature_entry (old convention). When the
    // gossip_menu-resolved text_id is absent from npc_text (common with BroadcastText-era dumps
    // that carry the new text_id but not the referenced npc_text row), try npc_text[entry] directly.
    let mut gossip_rows: Vec<String> = Vec::new();
    let mut needed_text_ids: HashSet<u64> = HashSet::new();
    let mut entry_to_menu: HashMap<u64, u64> = HashMap::new();

    for row in parse_table(dump, "creature_template") {
        let entry: u64 = field(&row, ct::ENTRY).parse().unwrap_or(0);
        if entry == 0 || !creature_entries.contains(&entry) {
            continue;
        }
        let gossip_menu_id: u64 = field(&row, ct::GOSSIP_MENU_ID).parse().unwrap_or(0);
        if gossip_menu_id == 0 {
            continue;
        }
        entry_to_menu.insert(entry, gossip_menu_id);
        // Primary path: gossip_menu_id → text_id → npc_text lookup.
        let text_id = menu_to_text.get(&gossip_menu_id).copied().unwrap_or(0);
        if text_id != 0 && npc_texts.contains_key(&text_id) {
            gossip_rows.push(format!("({entry},{text_id})"));
            needed_text_ids.insert(text_id);
        } else if npc_texts.contains_key(&entry) {
            // Fallback: npc_text id == creature_entry (old cmangos convention). Use it directly so
            // the client sees the NPC's real text even when gossip_menu points to a missing text_id.
            gossip_rows.push(format!("({entry},{entry})"));
            needed_text_ids.insert(entry);
        }
        // else: no text found for this creature — gateway falls back to the generic greeting.
    }

    // 5. Emit game_npc_text + game_npc_text_slot rows for all referenced text_ids.
    let mut npc_text_rows: Vec<String> = Vec::new();
    let mut npc_text_slot_rows: Vec<String> = Vec::new();
    let mut slot_row_id: u64 = 1;
    for &text_id in &needed_text_ids {
        let Some(text) = npc_texts.get(&text_id) else {
            continue;
        };
        npc_text_rows.push(format!("({text_id},{})", sql_text(text)));
        if let Some(slots) = npc_text_slots.get(&text_id) {
            for (slot_index, (male, female, prob)) in slots.iter().enumerate() {
                if male.is_empty() && female.is_empty() && *prob == 0.0 {
                    continue; // unused slot — no row (the gateway treats "no rows" as legacy fallback)
                }
                npc_text_slot_rows.push(format!(
                    "({slot_row_id},{text_id},{slot_index},{},{},{prob})",
                    sql_text(male),
                    sql_text(female),
                ));
                slot_row_id += 1;
            }
        }
    }

    // 6. gossip_menu_option → game_gossip_option, keyed by creature entry (collapsed like the title
    //    text mapping above — a spawned creature only ever shows ITS OWN entry's menu).
    let mut conditions: HashMap<u64, (u64, u64)> = HashMap::new();
    for row in parse_table(dump, "conditions") {
        let id: u64 = field(&row, cond::ENTRY).parse().unwrap_or(0);
        if id == 0 {
            continue;
        }
        let t: u64 = field(&row, cond::TYPE).parse().unwrap_or(0);
        let v1: u64 = field(&row, cond::VALUE1).parse().unwrap_or(0);
        conditions.insert(id, (t, v1));
    }
    let mut menu_to_options: HashMap<u64, Vec<Vec<String>>> = HashMap::new();
    for row in parse_table(dump, "gossip_menu_option") {
        let menu_id: u64 = field(&row, gmo::MENU_ID).parse().unwrap_or(0);
        if menu_id == 0 {
            continue;
        }
        menu_to_options.entry(menu_id).or_default().push(row);
    }
    let mut option_rows: Vec<String> = Vec::new();
    let mut next_row_id: u32 = 1;
    let mut entries_sorted: Vec<&u64> = entry_to_menu.keys().collect();
    entries_sorted.sort();
    for &entry in entries_sorted {
        let menu_id = entry_to_menu[&entry];
        let Some(opts) = menu_to_options.get(&menu_id) else {
            continue;
        };
        let mut sorted_opts: Vec<&Vec<String>> = opts.iter().collect();
        // Dense, deterministic option_index — the cmangos `id` order (matches in-window order).
        sorted_opts.sort_by_key(|r| field(r, gmo::ID).parse::<u64>().unwrap_or(0));
        // Counted rather than enumerated so a dropped row leaves no gap in the index.
        let mut option_index = 0u32;
        for row in sorted_opts.iter() {
            let icon: u32 = field(row, gmo::OPTION_ICON).parse().unwrap_or(0);
            let text = resolve_gossip_option_text(row, &broadcast);
            let action: u32 = reclassify_gossip_option_action(
                &text,
                field(row, gmo::OPTION_TYPE).parse().unwrap_or(0),
            );
            // Quests ride the menu's own quest section, and these rows carry no usable label.
            if action == lyracore_shared::constants::gossip_option::QUESTGIVER {
                continue;
            }
            let action_menu_id: u32 = field(row, gmo::ACTION_MENU_ID).parse().unwrap_or(0);
            let (cond_type, cond_value1, cond_value2) =
                resolve_gossip_option_condition(row, &conditions);
            option_rows.push(format!(
                "({next_row_id},{entry},{option_index},{icon},{},{action},{action_menu_id},{cond_type},{cond_value1},{cond_value2})",
                sql_text(&text),
            ));
            next_row_id += 1;
            option_index += 1;
        }
    }

    eprintln!(
        "gossip: {} creature→text mappings, {} npc_text strings resolved ({} weighted slot rows), {} menu options",
        gossip_rows.len(), npc_text_rows.len(), npc_text_slot_rows.len(), option_rows.len(),
    );
    GossipEtl {
        menu_rows: gossip_rows,
        npc_text_rows,
        npc_text_slot_rows,
        option_rows,
    }
}

/// The normalized quest rows the ETL produces (SQL value-tuples ready to INSERT) plus the reward-item
/// entry set the item-template load must include. A struct (not a tuple) so the six outputs stay named.
struct QuestEtl {
    templates: Vec<String>,
    texts: Vec<String>,
    objectives: Vec<String>,
    reward_items: Vec<String>,
    reward_choices: Vec<String>,
    relations: Vec<String>,
    // GAMEOBJECT giver relations (work-item 041: GO 68 "Wanted Poster" starts q176 Wanted: Hogger,
    // GO 55/56 "Lost Guards" corpses drive q37/q45/q71) — the `game_gameobject_quest` twin of `relations`.
    go_relations: Vec<String>,
    reward_item_entries: std::collections::HashSet<u64>,
    // Collect-objective ReqItemId entries — unioned into the item-template load so every collect-quest
    // item is guaranteed a template even if item_template ever returns to a referenced subset.
    req_item_entries: std::collections::HashSet<u64>,
    // Work-item 194 coverage counters: how many imported quests carry a non-zero next_quest_id
    // (forward-chained) / limit_time (timed). Reported by the caller's eprintln! coverage line and
    // backing `FLOOR_QUESTS_CHAINED` in importer/scripts/import-manifest.sh.
    chained_count: usize,
    timed_count: usize,
}

/// Build the RAW (unflattened) `reference_loot_template` map: pool entry → its own rows
/// `(item, chance, mincountOrRef, maxcount)`. Kept separate from `parse_loot_family` so it is parsed
/// ONCE and shared by every family — creature/pickpocket/gameobject/skinning/fishing all reference the
/// SAME cmangos pool table. `mincountOrRef` is kept RAW (not pre-filtered to ≥0) — a pool row can itself
/// be a reference to ANOTHER pool (`mincountOrRef < 0`), which `resolve_ref_pool` recurses into.
fn parse_ref_pools(dump: &str) -> std::collections::HashMap<u64, Vec<(u64, f64, i64, u32)>> {
    let mut refs: std::collections::HashMap<u64, Vec<(u64, f64, i64, u32)>> =
        std::collections::HashMap::new();
    for row in parse_table(dump, "reference_loot_template") {
        let g: u64 = field(&row, clt::ENTRY).parse().unwrap_or(0);
        let item: u64 = field(&row, clt::ITEM).parse().unwrap_or(0);
        let chance: f64 = field(&row, clt::CHANCE).parse().unwrap_or(0.0);
        let mincount_or_ref: i64 = field(&row, clt::MINCOUNT_OR_REF).parse().unwrap_or(0);
        let cnt: u32 = field(&row, clt::MAXCOUNT).parse().unwrap_or(1);
        refs.entry(g)
            .or_default()
            .push((item, chance, mincount_or_ref, cnt));
    }
    refs
}

/// Recursively flatten a `reference_loot_template` pool (`pool_id`) into its direct item rows
/// `(item, chance, count)`, expanding any NESTED `-ref` row (a pool that itself references another
/// pool) to FULL DEPTH — work-item 210, module doc decision #2 (the module's roll stays one-pass;
/// nesting is entirely an import-time concern). `visited` cycle-guards a single top-level resolution: a
/// pool reachable from itself (directly or transitively) stops re-entering rather than looping forever
/// — cmangos data isn't expected to cycle, but the importer never trusts the operator's dump not to.
/// Quest-only rows (`chance < 0`) inside a pool are skipped (no cmangos pool is quest-gated in this
/// family; the pre-210 one-level behavior already dropped them the same way).
fn resolve_ref_pool(
    pool_id: u64,
    refs: &std::collections::HashMap<u64, Vec<(u64, f64, i64, u32)>>,
    visited: &mut std::collections::HashSet<u64>,
) -> Vec<(u64, f64, u32)> {
    if !visited.insert(pool_id) {
        return Vec::new(); // TRUE cycle: this pool is on the CURRENT recursion path — stop
    }
    let Some(rows) = refs.get(&pool_id) else {
        visited.remove(&pool_id);
        return Vec::new();
    };
    let mut out = Vec::new();
    for &(item, chance, mincount_or_ref, maxcount) in rows {
        if mincount_or_ref < 0 {
            // Nested reference. cmangos semantics: the ref row's own chance GATES entry into the
            // pool, which then rolls one item internally — so the flattened children's chances are
            // NORMALIZED so their group total equals the gate (a 2-deep chain multiplies through).
            // gate 0 = "always" in cmangos reference rows → splice children at their own chances.
            let children = resolve_ref_pool((-mincount_or_ref) as u64, refs, visited);
            let gate = chance.abs();
            if gate > 0.0 {
                let child_total: f64 = children.iter().map(|(_, c, _)| c.max(0.0)).sum();
                if child_total > 0.0 {
                    let scale = gate / child_total;
                    out.extend(children.into_iter().map(|(i, c, n)| (i, c * scale, n)));
                } // all-zero-chance pool under a nonzero gate: nothing to weight — drop (loud in coverage)
            } else {
                out.extend(children);
            }
        } else if item != 0 && chance >= 0.0 {
            out.push((item, chance, maxcount.max(1)));
        }
    }
    // Unwind: only the ACTIVE path blocks re-entry — a shared sub-pool referenced from two sibling
    // rows expands in both (the visited set is a recursion stack, not a global seen-set — review catch).
    visited.remove(&pool_id);
    out
}

/// Parse ANY cmangos `*_loot_template` table into entry-keyed drops `(item, chance_bp, count, group_id,
/// quest_only)` — the five families (`creature_loot_template`, `pickpocketing_loot_template`,
/// `fishing_loot_template` [zone-keyed — its `entry` column is an AreaTable zone id, not a
/// creature/GO/skin id], `gameobject_loot_template`, `skinning_loot_template`) all share this IDENTICAL
/// column layout (see `clt`). Lifted out of the old creature-only `parse_creature_drops` so the quest
/// objective-emission gate (`creature_drop_item_set`) and every family's loot-row builder consume the
/// SAME parse — they can never diverge on what an in-slice entry actually drops. Pure function of
/// `dump`/`table`/`refs`. `reference_loot_template` pools are expanded RECURSIVELY at import
/// (`resolve_ref_pool`, cycle-guarded, decision #2) — a FRESH `visited` set per referencing row, so the
/// SAME pool resolves independently for every entry that references it (only a resolution's OWN
/// recursion path is cycle-guarded, not the whole import). `quest_only` is cmangos's
/// `ChanceOrQuestChance < 0` marker, KEPT (not folded away like the pre-210 `.abs()`-only behavior) so
/// the module can gate the roll on quest possession at roll time (module doc decision #1). A
/// raw-chance-0 row is an empty placeholder → skipped.
// ETL row shapes: each tuple is one parsed cmangos `*_loot_template` row, kept positional so the
// parse and the emit stay side by side.
#[allow(clippy::type_complexity)]
fn parse_loot_family(
    dump: &str,
    table: &str,
    refs: &std::collections::HashMap<u64, Vec<(u64, f64, i64, u32)>>,
) -> std::collections::HashMap<u64, Vec<(u64, u32, u32, u32, bool)>> {
    use std::collections::HashMap;
    // Chances are basis points (×100 from the dump's percent) so sub-1% odds survive. Direct rows keep
    // cmangos's groupid (0 = independent; >0 = a pick-one group). A reference row (mincountOrRef<0)
    // expands its (now recursively-flattened) shared pool into a pick-one GROUP gated by the row's
    // chance: the group total weight = gate_bp, so it drops one pool item gate% of the time. Within the
    // pool, explicit chances take gate_bp × chance/100; equal-chance (0) items split the leftover
    // uniformly — the vanilla group algorithm, representable via chance_bp + group_id.
    let bp = |pct: f64| (pct * 100.0).round() as i64;
    let mut drops_by_entry: HashMap<u64, Vec<(u64, u32, u32, u32, bool)>> = HashMap::new();
    for row in parse_table(dump, table) {
        let entry: u64 = field(&row, clt::ENTRY).parse().unwrap_or(0);
        let item: u64 = field(&row, clt::ITEM).parse().unwrap_or(0);
        let raw_chance: f64 = field(&row, clt::CHANCE).parse().unwrap_or(0.0);
        let groupid: u32 = field(&row, clt::GROUPID).parse().unwrap_or(0);
        let ref_or_min: i64 = field(&row, clt::MINCOUNT_OR_REF).parse().unwrap_or(0);
        let maxcount: u32 = field(&row, clt::MAXCOUNT).parse().unwrap_or(1);
        // A NEGATIVE `ChanceOrQuestChance` is cmangos's QUEST-drop marker: the item drops at |chance|%,
        // but ONLY for a looter who currently needs it (module doc decision #1) — kept as `quest_only`,
        // not folded away. A 0 row is an empty placeholder → skip.
        let quest_only = raw_chance < 0.0;
        let chance = raw_chance.abs();
        if chance == 0.0 {
            continue;
        }
        let drops = drops_by_entry.entry(entry).or_default();
        if ref_or_min < 0 {
            // Reference row → expand the (recursively-flattened) shared pool as a pick-one group gated
            // by `chance`. A FRESH visited-set per row: the same pool_id resolves independently for
            // every referencing row (only guards THIS resolution's own recursion path against a cycle).
            let pool_id = (-ref_or_min) as u64;
            let mut visited = std::collections::HashSet::new();
            let items = resolve_ref_pool(pool_id, refs, &mut visited);
            if items.is_empty() {
                continue;
            }
            let gate_bp = bp(chance).clamp(1, 10000);
            let sum_explicit: i64 = items
                .iter()
                .filter(|(_, c, _)| *c > 0.0)
                .map(|(_, c, _)| (gate_bp as f64 * c / 100.0).round() as i64)
                .sum();
            let n_zero = items.iter().filter(|(_, c, _)| *c <= 0.0).count().max(1) as i64;
            let leftover = (gate_bp as i64 - sum_explicit).max(0);
            // Namespace the pool group above any direct groupid AND uniquely per REFERENCE ROW:
            // one entry can reference the SAME pool twice at different gate chances, and keying on
            // pool_id alone merged those into one pick-one group (review catch). Group ids only need
            // uniqueness WITHIN this entry's row set — the current drops length is monotonic across
            // reference rows (each pushes ≥1 row), so it's a collision-free discriminator.
            let group_id = 1_000_000 + drops.len() as u32;
            for &(ritem, rchance, rcount) in &items {
                let item_bp = if rchance > 0.0 {
                    (gate_bp as f64 * rchance / 100.0).round() as i64
                } else {
                    leftover / n_zero
                };
                drops.push((
                    ritem,
                    item_bp.clamp(0, 10000) as u32,
                    rcount,
                    group_id,
                    quest_only,
                ));
            }
        } else if item != 0 {
            let cbp = bp(chance).clamp(1, 10000) as u32;
            drops.push((item, cbp, maxcount.max(1), groupid, quest_only));
        }
    }
    drops_by_entry
}

/// `creature_loot_template` drops, keyed by LootId. Thin wrapper over `parse_loot_family` kept for its
/// call sites' clarity (`build_items_and_loot`, `creature_drop_item_set`).
// ETL row shape: one resolved creature drop per tuple.
#[allow(clippy::type_complexity)]
fn parse_creature_drops(
    dump: &str,
) -> std::collections::HashMap<u64, Vec<(u64, u32, u32, u32, bool)>> {
    let refs = parse_ref_pools(dump);
    parse_loot_family(dump, "creature_loot_template", &refs)
}

/// The set of item ids that drop from creatures WE ACTUALLY SPAWN (`creature_loot_ids` = in-slice
/// entry→LootId). Drives the COLLECT-objective obtainability gate in `build_quests`: a quest may only
/// emit a COLLECT_ITEM objective for a ReqItem that is in this set (drops in-slice) OR is the quest's
/// SrcItem (granted on accept). Anything else would be an unobtainable objective → soft-lock, so it's
/// skipped (the quest auto-completes instead). Uses the same `parse_creature_drops` the loot builder
/// uses, so the two never disagree on what drops.
fn creature_drop_item_set(
    dump: &str,
    creature_loot_ids: &std::collections::HashMap<u64, u64>,
) -> std::collections::HashSet<u64> {
    let drops_by_lootid = parse_creature_drops(dump);
    let mut set = std::collections::HashSet::new();
    for lootid in creature_loot_ids.values() {
        if let Some(drops) = drops_by_lootid.get(lootid) {
            for &(item, ..) in drops {
                set.insert(item);
            }
        }
    }
    set
}

/// Items lootable from IN-SLICE gameobjects (chests): `gameobject_template` type 3 rows among
/// `used_go` carry their `gameobject_loot_template` lootId in DATA1; union those pools' items.
/// Extends the 042 obtainability gate — the grape/crate/wood collect quests want items that only
/// GO loot drops, and skipping their objectives made the quests auto-complete for a free reward.
/// Same `parse_loot_family` the GO-loot builder uses, so the two never disagree.
fn gameobject_loot_item_set(
    dump: &str,
    used_go: &std::collections::HashSet<u64>,
) -> std::collections::HashSet<u64> {
    const GO_TYPE_CHEST: u64 = 3;
    let refs = parse_ref_pools(dump);
    let loot = parse_loot_family(dump, "gameobject_loot_template", &refs);
    let mut set = std::collections::HashSet::new();
    for row in parse_table(dump, "gameobject_template") {
        let entry: u64 = field(&row, got::ENTRY).parse().unwrap_or(0);
        let ty: u64 = field(&row, got::TYPE).parse().unwrap_or(0);
        if ty != GO_TYPE_CHEST || !used_go.contains(&entry) {
            continue;
        }
        let lootid: u64 = field(&row, got::DATA1).parse().unwrap_or(0);
        if let Some(drops) = loot.get(&lootid) {
            for &(item, ..) in drops {
                set.insert(item);
            }
        }
    }
    set
}

/// Items sold by IN-SLICE vendors (direct `npc_vendor` rows + the `npc_vendor_template`
/// indirection via creature_template col 76) — the third leg of the 042 obtainability gate
/// (q3861's kodo feed is vendor-only). A trimmed twin of the canonical vendor ETL in
/// `build_items_and_loot` (which also carries slots/maxcount); only the ITEM SET matters here.
fn vendor_stock_item_set(
    dump: &str,
    local_creatures: &std::collections::HashSet<u64>,
) -> std::collections::HashSet<u64> {
    const CT_VENDOR_TEMPLATE_ID: usize = 76;
    let mut set = std::collections::HashSet::new();
    for row in parse_table(dump, "npc_vendor") {
        let entry: u64 = field(&row, nv::ENTRY).parse().unwrap_or(0);
        if local_creatures.contains(&entry) {
            let item: u64 = field(&row, nv::ITEM).parse().unwrap_or(0);
            if item > 0 {
                set.insert(item);
            }
        }
    }
    let mut tmpl_items: std::collections::HashMap<u64, Vec<u64>> = std::collections::HashMap::new();
    for row in parse_table(dump, "npc_vendor_template") {
        let t: u64 = field(&row, 0).parse().unwrap_or(0);
        let item: u64 = field(&row, 1).parse().unwrap_or(0);
        if t > 0 && item > 0 {
            tmpl_items.entry(t).or_default().push(item);
        }
    }
    for row in parse_table(dump, "creature_template") {
        let entry: u64 = field(&row, 0).parse().unwrap_or(0);
        if !local_creatures.contains(&entry) {
            continue;
        }
        let vt: u64 = field(&row, CT_VENDOR_TEMPLATE_ID).parse().unwrap_or(0);
        if let Some(items) = tmpl_items.get(&vt) {
            set.extend(items.iter().copied());
        }
    }
    set
}

/// Quest ETL: map cmangos `quest_template` + the two giver-relation tables onto our normalized quest
/// rows, SCOPED to quests whose START or END giver is a creature we actually spawn (`local_creatures`)
/// — so we never emit a `!`/`?` over an NPC that isn't in the slice, and the set stays small. The
/// reward-item entries it returns are unioned into the item-template load so a turn-in's reward items
/// have templates. Auto-inc ids are counter-assigned over deterministic (sorted) orders so clear+reload
/// is idempotent. Only KILL objectives (positive `ReqCreatureOrGOId`) + guaranteed reward items
/// (`RewItemId`) are mapped; gameobject objectives, choice rewards, item/source objectives are later slices.
fn build_quests(
    dump: &str,
    local_creatures: &std::collections::HashSet<u64>,
    local_gameobjects: &std::collections::HashSet<u64>,
    goobers: &std::collections::HashSet<u64>,
    obtainable_items: &std::collections::HashSet<u64>,
) -> QuestEtl {
    use std::collections::HashSet;

    // 0) Local giver relations: (creature, quest) pairs where the creature is in the slice. START =
    //    creature_questrelation, END = creature_involvedrelation. Collect the referenced quest ids.
    // Same 2-col (entry, quest) layout is shared by the GAMEOBJECT giver relation tables below
    // (`qr::CREATURE`/`qr::QUEST` — the field indices are positionally identical), so `local_relations`
    // is reused for both, gated on whichever "local" entry set (creature or GO) is passed in.
    let local_relations = |table: &str, local_entries: &HashSet<u64>| -> Vec<(u64, u64)> {
        parse_table(dump, table)
            .into_iter()
            .filter_map(|row| {
                let entry: u64 = field(&row, qr::CREATURE).parse().ok()?;
                let quest: u64 = field(&row, qr::QUEST).parse().ok()?;
                (local_entries.contains(&entry) && quest > 0).then_some((entry, quest))
            })
            .collect()
    };
    let starts = local_relations("creature_questrelation", local_creatures);
    let ends = local_relations("creature_involvedrelation", local_creatures);
    // GAMEOBJECT giver relations (work-item 041): GO 68 "Wanted Poster" starts q176 Wanted: Hogger
    // (which has NO creature start giver at all — unobtainable without this), and GO 55/56 "Lost
    // Guards" corpses drive the q37/q45/q71 chain. Scoped to GOs we actually spawn (`local_gameobjects`,
    // populated from the geographic gameobject-spawn loop), exactly like the creature relations above.
    let go_starts = local_relations("gameobject_questrelation", local_gameobjects);
    let go_ends = local_relations("gameobject_involvedrelation", local_gameobjects);
    let candidate_quests: HashSet<u64> = starts
        .iter()
        .chain(ends.iter())
        .chain(go_starts.iter())
        .chain(go_ends.iter())
        .map(|&(_, q)| q)
        .collect();

    // 1) quest_template rows for the candidate quests (a relation can dangle past the template set;
    //    only quests with a real template are kept — they drive which relations/objectives we emit).
    let mut quest_rows: Vec<String> = Vec::new();
    let mut quest_text_rows: Vec<String> = Vec::new();
    let mut objective_rows: Vec<String> = Vec::new();
    let mut reward_item_rows: Vec<String> = Vec::new();
    let mut reward_choice_rows: Vec<String> = Vec::new();
    let mut reward_item_entries: HashSet<u64> = HashSet::new();
    let mut req_item_entries: HashSet<u64> = HashSet::new();
    let mut valid_quests: HashSet<u64> = HashSet::new();
    let mut chained_count: usize = 0;
    let mut timed_count: usize = 0;
    let mut obj_id: u64 = 1;
    let mut rew_id: u64 = 1;
    let mut rew_choice_id: u64 = 1;
    // Per-quest count of emitted objectives → the next obj_index for area-trigger objectives (added after
    // the kill/collect loops) so they line up with CharacterQuest.counts.
    let mut quest_obj_count: std::collections::HashMap<u64, u8> = std::collections::HashMap::new();
    for row in parse_table(dump, "quest_template") {
        let entry: u64 = field(&row, qt::ENTRY).parse().unwrap_or(0);
        if entry == 0 || !candidate_quests.contains(&entry) {
            continue;
        }
        valid_quests.insert(entry);
        let min_level: u32 = field(&row, qt::MIN_LEVEL).parse().unwrap_or(0);
        let quest_level: i64 = field(&row, qt::QUEST_LEVEL).parse().unwrap_or(0);
        let money: i64 = field(&row, qt::REW_OR_REQ_MONEY).parse().unwrap_or(0);
        // cmangos RewMoneyMaxLevel — vanilla's Quest::XPValue divides this by 0.6 for the authentic
        // quest XP (lyracore_shared::quest::quest_xp). ~18% of quests have 0 here (no-XP quests). Clamp ≥ 0.
        let reward_money_max_level: i64 = field(&row, qt::REW_MONEY_MAX_LEVEL).parse().unwrap_or(0);
        // Availability gates: prerequisite quest (cmangos PrevQuestId can be negative for exclusive-group
        // semantics we don't model yet — clamp <0 to 0 = "no prereq" so it doesn't wrongly block) +
        // race/class bitmasks. reward_xp stays 0 (computed).
        let prev_quest: i64 = field(&row, qt::PREV_QUEST_ID).parse().unwrap_or(0);
        // Repeatable (SpecialFlags bit0, QUEST_SPECIAL_FLAGS_REPEATABLE): lets the quest be re-accepted
        // right after turn-in (module::quest::apply_turn_in_quest deletes rather than rewards-and-keeps
        // the log row when set).
        let special_flags: u32 = field(&row, qt::SPECIAL_FLAGS).parse().unwrap_or(0);
        let repeatable = (special_flags & 1) != 0;
        let req_races: u32 = field(&row, qt::REQUIRED_RACES).parse().unwrap_or(0);
        let req_classes: u32 = field(&row, qt::REQUIRED_CLASSES).parse().unwrap_or(0);
        let zone_or_sort: i64 = field(&row, qt::ZONE_OR_SORT).parse().unwrap_or(0);
        // Reputation rewards (#13): 2 of the 5 cmangos RewRepFaction/Value slots (most Elwynn quests use
        // ≤1). Faction id stays as-is (0 = none); the value keeps its sign (vanilla values are positive,
        // but a rep LOSS is representable). Deliberate simplification: 2 slots — bump to 5 if a
        // quest needs >2 factions.
        let rf1: u32 = field(&row, qt::REW_REP_FACTION1).parse().unwrap_or(0);
        let rv1: i64 = field(&row, qt::REW_REP_VALUE1).parse().unwrap_or(0);
        let rf2: u32 = field(&row, qt::REW_REP_FACTION1 + 1).parse().unwrap_or(0);
        let rv2: i64 = field(&row, qt::REW_REP_VALUE1 + 1).parse().unwrap_or(0);
        // Provided "source" item (cmangos SrcItemId/SrcItemCount): the item handed to the player on
        // accept. For a delivery quest SrcItemId == ReqItemId, so granting it on accept satisfies the
        // COLLECT objective. A dump count of 0 means 1 (cmangos treats 0 as 1; the module floors at 1).
        let src_item: u64 = field(&row, qt::SRC_ITEM_ID).parse().unwrap_or(0);
        let src_item_count: u32 = field(&row, qt::SRC_ITEM_COUNT).parse().unwrap_or(0);
        // Forward chains + timed quests (work-item 194): NextQuestInChain (0 = no successor; clamp
        // negative to 0 like prev_quest above) and LimitTime seconds (0 = untimed; cmangos never emits
        // a negative LimitTime, but clamp defensively the same way every other numeric column here does).
        let next_quest_raw: i64 = field(&row, qt::NEXT_QUEST_IN_CHAIN).parse().unwrap_or(0);
        let limit_time_raw: i64 = field(&row, qt::LIMIT_TIME).parse().unwrap_or(0);
        if next_quest_raw > 0 {
            chained_count += 1;
        }
        if limit_time_raw > 0 {
            timed_count += 1;
        }
        quest_rows.push(format!(
            "({entry},{min_level},{ql},{title},{money},0,{prev},{req_races},{req_classes},{zone_or_sort},{rf1},{rv1},{rf2},{rv2},{src_item},{src_item_count},{repeatable},{next_quest},{limit_time},{rmm})",
            ql = quest_level.max(0),
            title = sql_text(field(&row, qt::TITLE)),
            money = money.max(0),
            prev = prev_quest.max(0),
            next_quest = next_quest_raw.max(0),
            limit_time = limit_time_raw.max(0),
            rmm = reward_money_max_level.max(0),
        ));
        // Body text (side table) — Details/Objectives/OfferReward/RequestItems, verbatim ($B/$N tokens
        // and all), so the dialogs render real text instead of empty sections.
        quest_text_rows.push(format!(
            "({entry},{details},{objectives},{offer},{request})",
            details = sql_text(field(&row, qt::DETAILS)),
            objectives = sql_text(field(&row, qt::OBJECTIVES)),
            offer = sql_text(field(&row, qt::OFFER_REWARD_TEXT)),
            request = sql_text(field(&row, qt::REQUEST_ITEMS_TEXT)),
        ));
        // Objectives: up to 4 ReqCreatureOrGO slots; a POSITIVE id is a creature kill (negative = a
        // gameobject, skipped). obj_index is the dense 0-based position among emitted objectives, so it
        // lines up with CharacterQuest.counts in the module.
        let mut obj_index: u8 = 0;
        for i in 0..4 {
            let id: i64 = field(&row, qt::REQ_CREATURE_OR_GO_ID1 + i)
                .parse()
                .unwrap_or(0);
            let count: u32 = field(&row, qt::REQ_CREATURE_OR_GO_COUNT1 + i)
                .parse()
                .unwrap_or(0);
            if count == 0 {
                continue;
            }
            if id > 0 {
                objective_rows.push(format!(
                    "({obj_id},{entry},{obj_index},0,{id},{count})", // kind 0 = KILL_CREATURE
                ));
                obj_id += 1;
                obj_index += 1;
            } else if id < 0 {
                // A NEGATIVE id is a gameobject use-objective. Emit it ONLY if that GO is an imported
                // GOOBER (type 10) — else the quest references an un-spawned object and would be stuck.
                let go_entry = (-id) as u64;
                if goobers.contains(&go_entry) {
                    objective_rows.push(format!(
                        "({obj_id},{entry},{obj_index},2,{go_entry},{count})", // kind 2 = USE_GAMEOBJECT
                    ));
                    obj_id += 1;
                    obj_index += 1;
                }
            }
        }
        // Collect objectives (parity #4): up to 4 ReqItemId slots → "hold N of item X". kind 1 =
        // COLLECT_ITEM, target_entry = the item entry; the module checks a LIVE inventory count and
        // consumes them on turn-in. Continues the same dense obj_index space as the kill loop above, so
        // a quest with BOTH (kill + collect) sizes CharacterQuest.counts correctly.
        //
        // ENABLED (EMIT_COLLECT_OBJECTIVES = true): the loot ETL now drops the quest items. The loot path
        // takes `chance.abs()` (build_items_and_loot:794), so cmangos negative-chance QUEST drops (e.g. 752
        // Red Burlap Bandana, 182 Garrick's Head) drop for everyone — the prerequisite this flag waited on.
        // The ReqItemId entries are ALSO force-included into the item-template load (req_item_entries below
        // → extra_item_entries), so every collect item has a template even if item_template ever returns to
        // a subset. With items obtainable, emitting these COLLECT objectives makes the collect quests
        // genuinely completable instead of auto-completing (the old bug). The MODULE mechanic checks a LIVE
        // inventory count + consumes on turn-in. 042: GO-chest loot + vendor stock now count as
        // obtainable too (the gate below unions all three sources), so the grapes/wood/feed quests
        // emit real objectives instead of auto-completing.
        const EMIT_COLLECT_OBJECTIVES: bool = true;
        // The provided item itself needs a template (grant_item Errs "no such item" otherwise) — force
        // it into the item-template load even if it isn't also a ReqItem.
        if src_item > 0 {
            req_item_entries.insert(src_item);
        }
        // Always collect the ReqItemId entries for the item-template force-include, independent of whether
        // the objective is emitted — guarantees a template for every collect item.
        for i in 0..4 {
            let item: i64 = field(&row, qt::REQ_ITEM_ID1 + i).parse().unwrap_or(0);
            let count: u32 = field(&row, qt::REQ_ITEM_COUNT1 + i).parse().unwrap_or(0);
            if item > 0 {
                req_item_entries.insert(item as u64);
            }
            // ZERO-STUCK-QUESTS: emit a COLLECT objective ONLY for an OBTAINABLE item — one that drops
            // from an in-slice creature, drops from an in-slice CHEST, is sold by an in-slice VENDOR
            // (042 — the drops-only gate skipped GO/vendor ReqItems → empty objective set → free
            // turn-in on accept), or is the quest's SrcItem (handed over on accept). A genuinely
            // unobtainable ReqItem (mining/out-of-slice) still gets NO objective → the quest stays
            // completable on its obtainable subset instead of soft-locking.
            let obtainable =
                item > 0 && (obtainable_items.contains(&(item as u64)) || item as u64 == src_item);
            if EMIT_COLLECT_OBJECTIVES && obtainable && count > 0 {
                objective_rows.push(format!(
                    "({obj_id},{entry},{obj_index},1,{item},{count})", // kind 1 = COLLECT_ITEM
                ));
                obj_id += 1;
                obj_index += 1;
            }
        }
        quest_obj_count.insert(entry, obj_index);
        // Guaranteed reward items: up to 4 RewItemId slots.
        for i in 0..4 {
            let item: u64 = field(&row, qt::REW_ITEM_ID1 + i).parse().unwrap_or(0);
            let count: u32 = field(&row, qt::REW_ITEM_COUNT1 + i).parse().unwrap_or(0);
            if item > 0 {
                reward_item_entries.insert(item);
                reward_item_rows.push(format!(
                    "({rew_id},{entry},{item},{cnt})",
                    cnt = count.max(1)
                ));
                rew_id += 1;
            }
        }
        // Choice reward items (pick-1-of-N): up to 6 RewChoiceItemId slots. choice_index = a DENSE rank
        // (0-based over the PRESENT slots — `choice_rank`, NOT the raw dump slot `i`), so an empty leading
        // slot is skipped: this matches the ARRAY POSITION the client shows and replies with as `reward`, and
        // the packet order the gateway sends (it sorts rows by choice_index). All 553 real choice-quests are
        // gap-free (rank == i), but a custom quest with a leading gap would otherwise mismatch the wire pick.
        // The chosen item entries union into the SAME reward_item_entries set so they flow through
        // extra_item_entries into the item-template load — no new union site.
        let mut choice_rank = 0u8;
        for i in 0..6 {
            let item: u64 = field(&row, qt::REW_CHOICE_ITEM_ID1 + i)
                .parse()
                .unwrap_or(0);
            let count: u32 = field(&row, qt::REW_CHOICE_ITEM_COUNT1 + i)
                .parse()
                .unwrap_or(0);
            if item > 0 {
                reward_item_entries.insert(item);
                reward_choice_rows.push(format!(
                    "({rew_choice_id},{entry},{choice_rank},{item},{cnt})",
                    cnt = count.max(1)
                ));
                rew_choice_id += 1;
                choice_rank += 1;
            }
        }
    }

    // 1.5) Explore-area objectives (kind 3) from areatrigger_involvedrelation: a quest completed by
    //      ENTERING a zone (Jasperlode/Fargodeep mines). For each imported quest, append an
    //      EXPLORE_AREATRIGGER objective with target_entry = the trigger id; the client's CMSG_AREATRIGGER
    //      drives enter_areatrigger → on_areatrigger_entered increments it.
    for row in parse_table(dump, "areatrigger_involvedrelation") {
        let trigger: u64 = field(&row, ai::TRIGGER).parse().unwrap_or(0);
        let quest: u64 = field(&row, ai::QUEST).parse().unwrap_or(0);
        if trigger == 0 || !valid_quests.contains(&quest) {
            continue;
        }
        let idx = quest_obj_count.get(&quest).copied().unwrap_or(0);
        objective_rows.push(format!("({obj_id},{quest},{idx},3,{trigger},1)")); // kind 3 = EXPLORE_AREATRIGGER
        obj_id += 1;
        quest_obj_count.insert(quest, idx + 1);
    }

    // 2) creature_quest / gameobject_quest relation rows for the local relations whose quest survived
    //    the template join. Sorted + counter-assigned ids so clear+reload is idempotent (each table's
    //    ids are independent, so the two emitters below use SEPARATE counters). role: 0 = START, 1 = END.
    let mut creature_quest_rows: Vec<String> = Vec::new();
    let mut cq_id: u64 = 1;
    let mut emit_relations = |pairs: &[(u64, u64)], role: u8| {
        let mut local: Vec<(u64, u64)> = pairs
            .iter()
            .copied()
            .filter(|(_, q)| valid_quests.contains(q))
            .collect();
        local.sort_unstable();
        local.dedup();
        for (creature, quest) in local {
            creature_quest_rows.push(format!("({cq_id},{creature},{quest},{role})"));
            cq_id += 1;
        }
    };
    emit_relations(&starts, 0);
    emit_relations(&ends, 1);

    // Same shape, `game_gameobject_quest` table (work-item 041). A SEPARATE counter (`gq_id`) — this
    // table's ids are independent of `game_creature_quest`'s.
    let mut go_quest_rows: Vec<String> = Vec::new();
    let mut gq_id: u64 = 1;
    let mut emit_go_relations = |pairs: &[(u64, u64)], role: u8| {
        let mut local: Vec<(u64, u64)> = pairs
            .iter()
            .copied()
            .filter(|(_, q)| valid_quests.contains(q))
            .collect();
        local.sort_unstable();
        local.dedup();
        for (go_entry, quest) in local {
            go_quest_rows.push(format!("({gq_id},{go_entry},{quest},{role})"));
            gq_id += 1;
        }
    };
    emit_go_relations(&go_starts, 0);
    emit_go_relations(&go_ends, 1);

    QuestEtl {
        templates: quest_rows,
        texts: quest_text_rows,
        objectives: objective_rows,
        reward_items: reward_item_rows,
        reward_choices: reward_choice_rows,
        relations: creature_quest_rows,
        go_relations: go_quest_rows,
        reward_item_entries,
        req_item_entries,
        chained_count,
        timed_count,
    }
}

/// First pass: DIRECT drops only — positive `ChanceOrQuestChance`, real item, `mincountOrRef >= 0`
/// (a negative ref points at another loot group, which we don't expand yet). Quest drops (negative
/// chance) are skipped until a quest system exists. Loot ids are counter-assigned over a sorted
/// creature order so clear+reload stays idempotent. `extra_item_entries` (e.g. quest reward items) are
/// force-included in the item-template load so every referenced item gets a template.
fn build_items_and_loot(
    dump: &str,
    creature_loot_ids: &std::collections::HashMap<u64, u64>,
    vendor_entries: &std::collections::HashSet<u64>,
    extra_item_entries: &std::collections::HashSet<u64>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    use std::collections::HashSet;

    // 0+1) Parse the LootId-keyed creature drops (reference-pool expansion + direct rows). Shared with
    //       creature_drop_item_set so the quest objective-emission gate and the loot rows never diverge.
    let drops_by_lootid = parse_creature_drops(dump);

    // 2) game_creature_loot rows (one per creature×drop) + the referenced item set. Force-include the
    //    starter loadout entries so the login grant (resilient: skips a missing template) still arms a
    //    real weapon after a wipe+reload.
    let mut loot_rows: Vec<String> = Vec::new();
    let mut item_ids: HashSet<u64> = [25, 50, 51, 52].into_iter().collect();
    // Quest reward items (and any other forced entries) need templates too — the turn-in grant joins
    // game_item_template, so a missing reward template would fail the turn-in.
    item_ids.extend(extra_item_entries.iter().copied());
    let mut loot_id: u64 = 1;
    let mut creatures: Vec<(&u64, &u64)> = creature_loot_ids.iter().collect();
    creatures.sort(); // deterministic loot-id assignment across re-imports
    for (creature_entry, lootid) in creatures {
        let Some(drops) = drops_by_lootid.get(lootid) else {
            continue;
        };
        for &(item, chance_bp, count, group_id, quest_only) in drops {
            item_ids.insert(item);
            loot_rows.push(format!(
                "({loot_id},{creature_entry},{item},{chance_bp},{count},{group_id},{quest_only})"
            ));
            loot_id += 1;
        }
    }

    // 2.5) game_npc_vendor rows for the imported VENDOR creatures, adding their stock to the item set so
    //      every sold item gets a template (the gateway joins it for SMSG_LIST_INVENTORY + the buy path).
    let mut vendor_rows: Vec<String> = Vec::new();
    let mut vendor_id: u64 = 1;
    let mut nv_seen: HashSet<(u64, u64)> = HashSet::new(); // dedupe (vendor, item) — cmangos lists variants
    for row in parse_table(dump, "npc_vendor") {
        let entry: u64 = field(&row, nv::ENTRY).parse().unwrap_or(0);
        if !vendor_entries.contains(&entry) {
            continue;
        }
        let item: u64 = field(&row, nv::ITEM).parse().unwrap_or(0);
        if item == 0 || !nv_seen.insert((entry, item)) {
            continue;
        }
        let slot: u32 = field(&row, nv::SLOT).parse().unwrap_or(0);
        let max_count: u32 = field(&row, nv::MAXCOUNT).parse().unwrap_or(0);
        item_ids.insert(item);
        vendor_rows.push(format!("({vendor_id},{entry},{item},{slot},{max_count})"));
        vendor_id += 1;
    }

    // 2.6) npc_vendor_template indirection (work-item 247): 11 spawned vendors (Corina Steele,
    // Brog Hamfist, …) have ZERO direct npc_vendor rows — cmangos serves their goods via
    // creature_template.VendorTemplateId → npc_vendor_template. Flatten that into the same
    // per-creature game_npc_vendor rows (our schema deliberately has no template indirection).
    // npc_vendor_template dump shape (CREATE TABLE, verified): entry, item, maxcount, incrtime,
    // slot, condition_id, comments.
    let mut tmpl_stock: HashMap<u64, Vec<(u64, u32, u32)>> = HashMap::new();
    for row in parse_table(dump, "npc_vendor_template") {
        let t: u64 = field(&row, 0).parse().unwrap_or(0);
        let item: u64 = field(&row, 1).parse().unwrap_or(0);
        let max_count: u32 = field(&row, 2).parse().unwrap_or(0);
        let slot: u32 = field(&row, 4).parse().unwrap_or(0);
        if t != 0 && item != 0 {
            tmpl_stock
                .entry(t)
                .or_default()
                .push((item, slot, max_count));
        }
    }
    // creature_template.VendorTemplateId — column 76 of 87, read off the dump's own CREATE TABLE
    // (247; same dump-verified confidence as the `at::` fix).
    const CT_VENDOR_TEMPLATE_ID: usize = 76;
    for row in parse_table(dump, "creature_template") {
        let entry: u64 = field(&row, ct::ENTRY).parse().unwrap_or(0);
        if !vendor_entries.contains(&entry) {
            continue;
        }
        let vt: u64 = field(&row, CT_VENDOR_TEMPLATE_ID).parse().unwrap_or(0);
        let Some(stock) = tmpl_stock.get(&vt).filter(|_| vt != 0) else {
            continue;
        };
        for &(item, slot, max_count) in stock {
            if !nv_seen.insert((entry, item)) {
                continue; // direct rows win; template stock tops up
            }
            item_ids.insert(item);
            vendor_rows.push(format!("({vendor_id},{entry},{item},{slot},{max_count})"));
            vendor_id += 1;
        }
    }

    // 3) item_template rows — the FULL vanilla item set (importer P4). Every item gets a template so
    //    loot, vendors, quest rewards, AND spell-created items (Mage conjure / quest CreateItem) all
    //    resolve — an unreferenced item is harmless reference data, like mangoS loading the whole table.
    //    (Was a referenced-only subset.) entry 0 is skipped; `item_ids` (the referenced set built above)
    //    is now used only to flag a DANGLING reference — a loot/vendor/quest item with NO item_template
    //    row, which would break that loot/turn-in — so it surfaces rather than silently dropping.
    // ON-USE SPELL OVERRIDE (#387, finishing the spellid_1 migration `module/src/items/rules.rs` named as
    // the end state): the curated alpha consumables + the Hearthstone whose on-use cast is OUR synthetic
    // spell (50110-50119, seeded in `module/src/seed.rs`), not whatever the real vanilla dump's
    // SpellId_1/SpellTrigger_1 says — those real ids either name spells outside our curated `game_spell`
    // set, or (Spiced Wolf Meat, a crafted Cooking product) don't exist in vanilla at all. Applied AFTER
    // the raw dump read below so the curated id always wins; spelltrigger_1 is forced to 0 (on-use)
    // alongside it. Moved here from the now-retired `items::ops::USE_EFFECTS` shadow-map, byte-for-byte,
    // plus the two entries that map ONLY here: the Hearthstone (6948, whose recall used to be a hardcoded
    // item-entry special case in `apply_item_use`) and Roasted Boar Meat (2681, a level-1 food —
    // deliberately routed to the SAME plain HoT as Tough Jerky/Bread, no Well-Fed buff at level 1).
    const USE_SPELL_OVERRIDE: &[(u64, u32)] = &[
        (118, 50110),  // Minor Healing Potion -> Minor Healing (instant E_HEAL)
        (1251, 50111), // Linen Bandage -> Linen Bandage (channeled A_PERIODIC_HEAL HoT)
        (4360, 50096), // Rough Copper Bomb -> Rough Copper Bomb (E_DAMAGE AoE)
        (2455, 50113), // Minor Mana Potion -> Minor Mana Potion (E_ENERGIZE, mana-class gated)
        (159, 50114), // Refreshing Spring Water -> Refreshing Water (A_PERIODIC_ENERGIZE, mana-class gated)
        (5350, 50114), // Conjured Water -> same drink spell
        (4540, 50115), // Tough Hunk of Bread -> Eating (A_PERIODIC_HEAL food HoT)
        (117, 50115), // Tough Jerky -> same food spell
        (2681, 50115), // Roasted Boar Meat (level-1 food) -> same food spell, no Well-Fed buff
        (2680, 50116), // Spiced Wolf Meat -> Well Fed (+Sta/+Spi buff)
        (858, 50117), // Lesser Healing Potion -> Lesser Healing (E_HEAL, rank-2)
        (2581, 50118), // Heavy Linen Bandage -> Heavy Linen Bandage (channeled HoT, rank-2)
        (
            lyracore_shared::constants::starter_item::HEARTHSTONE_ENTRY as u64,
            50119,
        ), // Hearthstone -> Call Stone (E_RECALL_HOME)
    ];
    let use_spell_override = |entry: u64| -> Option<u32> {
        USE_SPELL_OVERRIDE
            .iter()
            .find_map(|&(item, spell)| (item == entry).then_some(spell))
    };

    let mut item_rows: Vec<String> = Vec::new();
    let mut emitted: HashSet<u64> = HashSet::new();
    // Work-item 213 coverage counters: how many items carry a nonzero resistance (any of the 6
    // schools) and how many carry a nonzero startquest link — printed after the loop so a re-import
    // makes the new columns' reach visible without a manual SQL scan.
    let mut resist_nonzero_count: u64 = 0;
    let mut startquest_nonzero_count: u64 = 0;
    for row in parse_table(dump, "item_template") {
        let entry: u64 = field(&row, it::ENTRY).parse().unwrap_or(0);
        if entry == 0 {
            continue;
        }
        emitted.insert(entry);
        // Scan the 10 (stat_type, stat_value) pairs → our 5 primaries. cmangos 1.12 item mod types:
        // 3=AGI 4=STR 5=INT 6=SPI 7=STA (0=mana / 1=health have no game column → dropped). Crit/hit
        // come from item spell effects in 1.12, not base stats, so stat_crit/stat_hit stay 0 on import.
        let (mut s_str, mut s_agi, mut s_sta, mut s_int, mut s_spi) = (0i32, 0, 0, 0, 0);
        for i in 0..10 {
            let st: u8 = field(&row, it::STAT_TYPE1 + 2 * i).parse().unwrap_or(0);
            let sv: i32 = field(&row, it::STAT_TYPE1 + 2 * i + 1).parse().unwrap_or(0);
            match st {
                3 => s_agi += sv,
                4 => s_str += sv,
                5 => s_int += sv,
                6 => s_spi += sv,
                7 => s_sta += sv,
                _ => {}
            }
        }
        let stack: u32 = field(&row, it::STACKABLE)
            .parse::<u32>()
            .unwrap_or(1)
            .max(1);
        let allowed_class =
            normalize_item_mask(field(&row, it::ALLOWED_CLASS), ALL_PLAYABLE_CLASS_MASK);
        let allowed_race =
            normalize_item_mask(field(&row, it::ALLOWED_RACE), ALL_PLAYABLE_RACE_MASK);
        // Drink = Food&Drink consumable whose name reads as a beverage (see is_drink_consumable). The
        // restores_power flag makes apply_item_use refill mana instead of HP. Generic predicate, no item-id list.
        let class_v: u8 = field(&row, it::CLASS).parse().unwrap_or(0);
        let subclass_v: u8 = field(&row, it::SUBCLASS).parse().unwrap_or(0);
        let is_drink = is_drink_consumable(class_v, subclass_v, field(&row, it::NAME));
        // The 6 resistance schools (work-item 213) — data plumbing only, no consumer reads these yet.
        let holy: i32 = field(&row, it::HOLY_RES).parse().unwrap_or(0);
        let fire: i32 = field(&row, it::FIRE_RES).parse().unwrap_or(0);
        let nature: i32 = field(&row, it::NATURE_RES).parse().unwrap_or(0);
        let frost: i32 = field(&row, it::FROST_RES).parse().unwrap_or(0);
        let shadow: i32 = field(&row, it::SHADOW_RES).parse().unwrap_or(0);
        let arcane: i32 = field(&row, it::ARCANE_RES).parse().unwrap_or(0);
        if holy != 0 || fire != 0 || nature != 0 || frost != 0 || shadow != 0 || arcane != 0 {
            resist_nonzero_count += 1;
        }
        // Spell slots 3-5 (work-item 213) — completes the 191 proc engine's item half; data plumbing
        // only, same id+trigger shape as slots 1-2 above.
        let sp3: u32 = field(&row, it::SPELLID_3).parse().unwrap_or(0);
        let spt3: u8 = field(&row, it::SPELLTRIGGER_3).parse().unwrap_or(0);
        let sp4: u32 = field(&row, it::SPELLID_4).parse().unwrap_or(0);
        let spt4: u8 = field(&row, it::SPELLTRIGGER_4).parse().unwrap_or(0);
        let sp5: u32 = field(&row, it::SPELLID_5).parse().unwrap_or(0);
        let spt5: u8 = field(&row, it::SPELLTRIGGER_5).parse().unwrap_or(0);
        // Skill/reputation gates (work-item 213) — mail/plate proficiency + the 195 item half.
        let req_skill: u32 = field(&row, it::REQUIRED_SKILL).parse().unwrap_or(0);
        let req_skill_rank: u32 = field(&row, it::REQUIRED_SKILL_RANK).parse().unwrap_or(0);
        let req_rep_faction: u32 = field(&row, it::REQUIRED_REPUTATION_FACTION)
            .parse()
            .unwrap_or(0);
        let req_rep_rank: u32 = field(&row, it::REQUIRED_REPUTATION_RANK)
            .parse()
            .unwrap_or(0);
        // maxcount/flags/page_text/startquest/bag_family (work-item 213) — data plumbing only.
        let max_count: u32 = field(&row, it::MAX_COUNT).parse().unwrap_or(0);
        let item_flags: u32 = field(&row, it::FLAGS).parse().unwrap_or(0);
        let page_text: u32 = field(&row, it::PAGE_TEXT).parse().unwrap_or(0);
        let start_quest: u32 = field(&row, it::START_QUEST).parse().unwrap_or(0);
        if start_quest != 0 {
            startquest_nonzero_count += 1;
        }
        let bag_family: u32 = field(&row, it::BAG_FAMILY).parse().unwrap_or(0);
        // BuyCount (080): floor at 1 — a 0 in the dump (or a parse miss) must not sell zero items.
        let buy_count: u32 = field(&row, it::BUY_COUNT).parse().unwrap_or(1).max(1);
        let food_type: u8 = field(&row, it::FOOD_TYPE).parse().unwrap_or(0);
        // ON-USE SPELL OVERRIDE (#387): a curated entry's spellid_1/spelltrigger_1 is OUR synthetic
        // on-use cast, forced trigger 0 — see `USE_SPELL_OVERRIDE`'s doc above. Every other item keeps
        // its raw dump value untouched (baseline-safe: the vast majority of the vanilla item catalog is
        // unaffected).
        let (sp1, spt1): (u32, u8) = match use_spell_override(entry) {
            Some(spell_id) => (spell_id, 0),
            None => (
                field(&row, it::SPELLID_1).parse().unwrap_or(0),
                field(&row, it::SPELLTRIGGER_1).parse().unwrap_or(0),
            ),
        };
        // game_item_template column order — see module/src/items/tables.rs. stat_crit/stat_hit
        // are not in 1.12 item base stats, so 0,0 here. block_value is the shield's flat block (0 for
        // non-shields) — makes imported shields actually block (combat::effective_block_value reads it).
        // restores_power (drink) is a Bool → bare true/false SQL literal (the is_negative Bool-SQL note).
        item_rows.push(format!(
            "({entry},{class},{subclass},{name},{disp},{qual},{inv},{ilvl},{rlvl},{dur},{buy},{sell},{stack},{dmin},{dmax},{delay},{s_str},{s_agi},{s_sta},{s_int},{s_spi},0,0,{armor},{block},{drink},{sp1},{spt1},{sp2},{spt2},{cslots},{sheath},{bonding},{holy},{fire},{nature},{frost},{shadow},{arcane},{sp3},{spt3},{sp4},{spt4},{sp5},{spt5},{req_skill},{req_skill_rank},{req_rep_faction},{req_rep_rank},{max_count},{item_flags},{page_text},{start_quest},{bag_family},{buy_count},{food_type},{allowed_class},{allowed_race})",
            sheath = field(&row, it::SHEATH).parse::<u8>().unwrap_or(0),
            sp2 = field(&row, it::SPELLID_2).parse::<u32>().unwrap_or(0),
            spt2 = field(&row, it::SPELLTRIGGER_2).parse::<u8>().unwrap_or(0),
            // ContainerSlots — how many bag slots this item provides (0 for non-bags); col 24 in
            // the cmangos 1.12 item_template schema, immediately after Stackable(23).
            cslots = field(&row, it::CONTAINER_SLOTS).parse::<u8>().unwrap_or(0),
            // Bonding (work-item 127): 0=NoBind,1=BoP,2=BoE,3=BoU,4/5=QuestItem — mapped straight
            // through to the client's "Binds when picked up/equipped" tooltip line.
            bonding = field(&row, it::BONDING).parse::<u8>().unwrap_or(0),
            class = field(&row, it::CLASS),
            subclass = field(&row, it::SUBCLASS),
            name = sql_text(field(&row, it::NAME)),
            drink = is_drink,
            disp = field(&row, it::DISPLAY_ID),
            qual = field(&row, it::QUALITY),
            inv = field(&row, it::INVENTORY_TYPE),
            ilvl = field(&row, it::ITEM_LEVEL),
            rlvl = field(&row, it::REQUIRED_LEVEL),
            dur = field(&row, it::MAX_DURABILITY),
            buy = field(&row, it::BUY_PRICE),
            sell = field(&row, it::SELL_PRICE),
            dmin = field(&row, it::DMG_MIN1),
            dmax = field(&row, it::DMG_MAX1),
            delay = field(&row, it::DELAY),
            armor = field(&row, it::ARMOR),
            block = field(&row, it::BLOCK),
            allowed_class = allowed_class,
            allowed_race = allowed_race,
        ));
    }
    eprintln!(
        "items: {} of {} imported templates carry a nonzero resistance; {} carry a nonzero startquest",
        resist_nonzero_count,
        emitted.len(),
        startquest_nonzero_count
    );

    // Dangling-reference guard: any loot/vendor/quest-reward item with no item_template row would break
    // that drop/turn-in. With the full set imported this should be empty; warn loudly if not.
    let mut dangling: Vec<u64> = item_ids
        .iter()
        .copied()
        .filter(|e| !emitted.contains(e))
        .collect();
    dangling.sort_unstable();
    if !dangling.is_empty() {
        eprintln!(
            "items: WARNING — {} referenced item(s) have NO item_template row (e.g. {:?})",
            dangling.len(),
            dangling.iter().take(8).collect::<Vec<_>>(),
        );
    }

    (item_rows, loot_rows, vendor_rows)
}

/// Append chunked `INSERT INTO {table} ({cols}) VALUES (...)` statements for `rows` — bounded BOTH by
/// row count (≤500) AND by byte size (≤~32KB/statement). The byte bound matters for text-heavy tables
/// like `game_quest_text`: at a full-Elwynn slice its 170 rows of quest prose are one 110KB statement
/// that `spacetime sql` rejects (400) — row-count chunking alone never splits it. No-op for empty `rows`.
/// Data-row count across a family's generated statement list: DELETEs contribute 0; each
/// `push_insert`-shaped INSERT contributes its VALUES tuple count (`),(` separators + 1).
/// Feeds the provenance stamps (work-item 216) for families whose builders return statements
/// rather than row vectors — statement `.len()` would under-report by the batch factor.
/// Only sound for numeric-payload tables (a text value containing `),(` would over-count);
/// the "globals" builders (stats/positions/graveyards/createinfo) are all numeric.
fn insert_row_count(stmts: &[String]) -> u64 {
    stmts
        .iter()
        .filter(|s| s.starts_with("INSERT"))
        .map(|s| s.matches("),(").count() as u64 + 1)
        .sum()
}

pub(crate) fn push_insert(stmts: &mut Vec<String>, table: &str, cols: &str, rows: &[String]) {
    const MAX_BYTES: usize = 32 * 1024; // well under the observed spacetime-sql statement-size limit
    let mut batch: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    let flush = |batch: &mut Vec<&str>, stmts: &mut Vec<String>| {
        if !batch.is_empty() {
            stmts.push(format!(
                "INSERT INTO {table} ({cols}) VALUES {}",
                batch.join(",")
            ));
            batch.clear();
        }
    };
    for row in rows {
        if !batch.is_empty() && (batch.len() >= 500 || bytes + row.len() + 1 > MAX_BYTES) {
            flush(&mut batch, stmts);
            bytes = 0;
        }
        batch.push(row);
        bytes += row.len() + 1;
    }
    flush(&mut batch, stmts);
}

/// Keep a creature inside the scope, or a forced entry anywhere on a planned map. Forced entries
/// bypass geometry only; the map fence always applies. Pool slots cannot match a forced entry.
fn creature_row_kept_in_scope(
    scope: &WorldImportScope,
    map: i64,
    x: f64,
    y: f64,
    z: f64,
    raw_entry: u64,
) -> bool {
    scope.contains_map(map)
        && ((raw_entry != 0 && scope.forced_creature_entries.contains(&raw_entry))
            || scope.contains(map, x, y, z))
}

#[cfg(test)]
pub(crate) fn creature_row_kept(
    args: &Args,
    map: i64,
    x: f64,
    y: f64,
    z: f64,
    raw_entry: u64,
) -> bool {
    let scope = args
        .world_import_scope()
        .expect("Args contains a scope validated by parse_args");
    creature_row_kept_in_scope(&scope, map, x, y, z, raw_entry)
}

#[cfg(test)]
pub(crate) fn map_in_run(args: &Args, map: i64) -> bool {
    args.world_import_scope()
        .expect("Args contains a scope validated by parse_args")
        .contains_map(map)
}

#[cfg(test)]
pub(crate) fn row_in_slice(args: &Args, map: i64, x: f64, y: f64, z: f64) -> bool {
    args.world_import_scope()
        .expect("Args contains a scope validated by parse_args")
        .contains(map, x, y, z)
}

/// What `--print-extents` derived from the dump. `anchor` is the in-slice spawn CLOSEST to the
/// rectangle's centre — a REAL cmangos ground point, which is exactly what `--center` needs (the
/// terrain importer's interpolate self-check compares the ADT height at `--center` against the Z the
/// operator supplied, and the nav importer requires `--center` to rasterize walkable). Issue #24:
/// deriving a second continent's box also means deriving a second continent's centre, and the map-0
/// default (Northshire) is off-slice there.
struct Extents {
    count: u64,
    min: (f64, f64, f64),
    max: (f64, f64, f64),
    anchor: (f64, f64, f64),
}

/// Pure spawn-bbox computation for `--print-extents`: scan every `creature` row on `map` — optionally
/// pre-filtered to a CANDIDATE `--box` rectangle (issue #24: a whole continent's extents are the
/// continent, so deriving a starting-zone box is an iterate-and-narrow loop — pass a rough rectangle,
/// read back the real spawn extents and count inside it) — returning `None` if no row matched
/// (nothing to report). Split out from `print_extents` so the corner math is unit-tested against a
/// synthetic dump without stdout.
fn compute_extents(dump: &str, map: i64, bbox: Option<(f64, f64, f64, f64)>) -> Option<Extents> {
    let mut count = 0u64;
    let mut min = (f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut kept: Vec<(f64, f64, f64)> = Vec::new();
    for row in parse_table(dump, "creature") {
        let m: i64 = field(&row, cr::MAP).parse().unwrap_or(-1);
        if m != map {
            continue;
        }
        let x: f64 = field(&row, cr::X).parse().unwrap_or(0.0);
        let y: f64 = field(&row, cr::Y).parse().unwrap_or(0.0);
        let z: f64 = field(&row, cr::Z).parse().unwrap_or(0.0);
        if matches!(bbox, Some((x0, x1, y0, y1)) if x < x0 || x > x1 || y < y0 || y > y1) {
            continue;
        }
        min = (min.0.min(x), min.1.min(y), min.2.min(z));
        max = (max.0.max(x), max.1.max(y), max.2.max(z));
        kept.push((x, y, z));
        count += 1;
    }
    if count == 0 {
        return None;
    }
    // The spawn nearest the derived rectangle's centre — a real ground point INSIDE the slice, so it
    // is always a valid `--center` (2D distance: the centre of a Z range is meaningless terrain-wise).
    let (mid_x, mid_y) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    let anchor = *kept
        .iter()
        .min_by(|a, b| {
            let d = |p: &(f64, f64, f64)| (p.0 - mid_x).powi(2) + (p.1 - mid_y).powi(2);
            d(a).total_cmp(&d(b))
        })
        .expect("count > 0 ⇒ kept is non-empty");
    Some(Extents {
        count,
        min,
        max,
        anchor,
    })
}

/// The cmangos `playercreateinfo` start positions ON `map`, as `(race, class, zone, x, y, z)` — the
/// canonical starting-zone ANCHORS of a continent (issue #24: "where does a Kalimdor box go?" is
/// answered by the dump's own Valley of Trials / Camp Narache / Shadowglen rows, not from memory).
/// Pure so it is unit-testable without stdout; printed by `print_extents` below.
fn start_positions_on_map(dump: &str, map: i64) -> Vec<(u32, u32, u32, f64, f64, f64)> {
    parse_table(dump, "playercreateinfo")
        .iter()
        .filter_map(|r| {
            let m: i64 = field(r, pc::MAP).parse().ok()?;
            if m != map {
                return None;
            }
            Some((
                field(r, pc::RACE).parse().ok()?,
                field(r, pc::CLASS).parse().ok()?,
                field(r, pc::ZONE).parse().ok()?,
                field(r, pc::X).parse().ok()?,
                field(r, pc::Y).parse().ok()?,
                field(r, pc::Z).parse().ok()?,
            ))
        })
        .collect()
}

/// `--print-extents`: print the operator's OWN `--map` spawn bounding box derived from THEIR dump
/// (min/max X/Y/Z + spawn count), plus a suggested `--box`/`--center` and the map's start positions,
/// and return — writes NOTHING (no SQL, no reducer call). This is how an operator derives a new
/// zone's exact rectangle (e.g. Westfall) instead of trusting a remembered/guessed number (work-item
/// 206), and how a SECOND CONTINENT's starting-zone box gets derived rather than guessed (issue #24):
/// run it bare for the whole-continent extents, pick a starting zone from the printed start
/// positions, then re-run with a rough `--box` around it and read back the real in-box extents/count.
fn print_extents(dump: &str, map: i64, bbox: Option<(f64, f64, f64, f64)>) -> Result<()> {
    let scope = match bbox {
        Some((x0, x1, y0, y1)) => format!(" inside --box {x0:.0},{x1:.0},{y0:.0},{y1:.0}"),
        None => String::new(),
    };
    let Some(e) = compute_extents(dump, map, bbox) else {
        bail!("--print-extents: no creature spawns found on map {map}{scope} — check --map/--box / the dump path");
    };
    println!(
        "print-extents: map {map}{scope} — {} creature spawns",
        e.count
    );
    println!("  X: {:.2} .. {:.2}", e.min.0, e.max.0);
    println!("  Y: {:.2} .. {:.2}", e.min.1, e.max.1);
    println!("  Z: {:.2} .. {:.2}", e.min.2, e.max.2);
    println!(
        "  suggested --box \"{:.0},{:.0},{:.0},{:.0}\"   (X0,X1,Y0,Y1 — widen a little; a spawn extent is exact but a zone's terrain/GOs run past its outermost creature)",
        e.min.0, e.max.0, e.min.1, e.max.1
    );
    println!(
        "  suggested --center \"{:.2},{:.2},{:.2}\"   (the in-slice spawn nearest the rectangle's centre — the terrain/nav self-checks need a REAL ground point inside the box)",
        e.anchor.0, e.anchor.1, e.anchor.2
    );
    let starts = start_positions_on_map(dump, map);
    if starts.is_empty() {
        println!("  start positions on map {map}: none in playercreateinfo (not a starting continent, or the table is absent from this dump)");
    } else {
        println!("  start positions on map {map} (cmangos playercreateinfo — the starting-zone anchors to draw a box around):");
        for (race, class, zone, x, y, z) in starts {
            println!("    race {race} class {class}  zone {zone}  ({x:.2},{y:.2},{z:.2})");
        }
    }
    Ok(())
}

/// Run a sequence of `spacetime sql` statements as the owner. Each is a separate invocation (Simple
/// Query Protocol — no multi-statement transaction), so on failure the sequence stops mid-way; the
/// clear+reload is idempotent, so re-running recovers. Shared by the cmangos and DBC streams.
pub(crate) fn run_sql_statements(args: &Args, stmts: &[String], label: &str) -> Result<()> {
    for (n, s) in stmts.iter().enumerate() {
        eprintln!(
            "{label} sql [{}/{}]: {}…",
            n + 1,
            stmts.len(),
            &s[..s.len().min(60)]
        );
        let out = Command::new("spacetime")
            .args(["sql", "-s", &args.server, &args.db, s])
            .output()
            .context("run spacetime sql")?;
        if !out.status.success() {
            bail!(
                "statement {} failed: {}",
                n + 1,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
    Ok(())
}

/// The fixed set of `--family` values this importer recognizes (work-item 216) — one per independent
/// clear+reload block the `--dump` ETL below already builds (the inventory `import-world.sh`'s header
/// comment describes: "the ETL is clear+reload PER FAMILY"). `--family` covers THESE families only —
/// terrain (`--terrain`), client DBCs (`--dbc`), and Spell.dbc (`--dbc --spells`) are separate
/// invocations with no family concept of their own. "globals" is the P1/P3/212 slice (start
/// positions, the class/level stat curve, graveyard-zone links, createinfo spells/actions/items,
/// work-item 225's areatrigger-teleport dungeon portals) — not box-scoped, but still its own
/// clear+reload, so `--family quests` reloads QUESTS ALONE (not
/// quests-plus-globals). "spellmeta" (work-item 102, reduced scope) is the same "global, not box-scoped,
/// own clear+reload" shape as "globals" — kept as its OWN family rather than folded into "globals"
/// because nothing else reads its output (compute-gateable too, unlike "globals"' entangled siblings —
/// see `build_dump_plan`'s gate).
pub(crate) const FAMILIES: &[&str] = &[
    "creatures",
    "items",
    "loot",
    "gossip",
    "quests",
    "gameobjects",
    "trainers",
    "casts",
    "creature-ai",
    "globals",
    "spellmeta",
];

/// The batch size for the two Timestamp-bearing spawn payloads (`import_creature_spawns`/
/// `import_gameobjects` reducer args) — shared by `build_dump_plan` (which chunks the rows) and
/// `main`'s dry-run print (which reports it).
const SPAWN_BATCH: usize = 250;

/// Whether `name`'s DELETE+INSERT block should be pushed/applied this run: true when `--family` was
/// not given (full run — every family active, BYTE-IDENTICAL to pre-216 behavior) or when it names
/// exactly this family.
pub(crate) fn family_active(args: &Args, name: &str) -> bool {
    match &args.family {
        None => true,
        Some(f) => f == name,
    }
}

/// Everything the `--dump` ETL computes for ONE run: the plain-SQL clear+reload statements (already
/// family-gated — see `family_active`), the two Timestamp-bearing spawn payloads (packed, batched),
/// and the provenance `stamps` (family, row_count) for every family that actually had its block
/// pushed. Built by `build_dump_plan`, applied by `main` (kept separate so tests can inspect a plan
/// without shelling out to `spacetime call`).
pub(crate) struct DumpPlan {
    pub(crate) stmts: Vec<String>,
    pub(crate) spawn_batches: Vec<String>,
    pub(crate) spawn_row_count: usize,
    pub(crate) go_batches: Vec<String>,
    pub(crate) go_row_count: usize,
    pub(crate) stamps: Vec<(&'static str, u64)>,
}

// ETL row shape: one `game_creature_spawn` row per tuple, in the packed-payload column order.
#[allow(clippy::type_complexity)]
/// Build the full `--dump` ETL plan (work-item 216's `--family` split lives entirely in here, via
/// `family_active` gates around each family's DELETE+INSERT block): parses the cmangos dump, filters
/// the content slice, maps every family's rows, and returns the ready-to-apply plan. Pure w.r.t. the
/// outside world (no `spacetime call`/SQL execution) — `main` does that against the returned
/// `DumpPlan`. `--family` gates ONLY which blocks are pushed into `stmts`/counted into `stamps`; every
/// family's prerequisite parsing/scoping still runs (the quests/items builders are entangled — see
/// the module doc — so their compute can't be conditional even when only one is requested).
fn build_dump_plan(
    dump: &str,
    args: &Args,
    display_scales: &Option<std::collections::HashMap<u32, f32>>,
    profession_tier_values: &Option<std::collections::HashMap<u32, [u16; 4]>>,
) -> Result<DumpPlan> {
    let scope = args.world_import_scope()?;
    eprintln!("world import scope: {}", scope.name());
    for slice in &scope.bounded_slices {
        let (x0, x1, y0, y1) = slice.bounds;
        let (sx, sy, sz) = slice.sample;
        eprintln!(
            "  bounded slice {}: map {} x[{x0:.0},{x1:.0}] y[{y0:.0},{y1:.0}], sample ({sx:.2},{sy:.2},{sz:.2})",
            slice.name, slice.map_id
        );
    }
    for map in &scope.whole_maps {
        eprintln!("  whole map: {map}");
    }

    // 0b) creature_spawn_entry: resolves id=0 pool slots to concrete creature_template entries.
    // cmangos places id=0 in `creature` for spawn-pool points; the real entry is in creature_spawn_entry.
    // Each guid may map to multiple entries (random pick at runtime). We pick the first for determinism.
    let mut spawn_entry_map: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    for row in parse_table(dump, "creature_spawn_entry") {
        let guid: u64 = field(&row, cse::GUID).parse().unwrap_or(0);
        let entry: u64 = field(&row, cse::ENTRY).parse().unwrap_or(0);
        if guid > 0 && entry > 0 {
            // keep first entry seen (stable across re-imports; dump order is deterministic)
            spawn_entry_map.entry(guid).or_insert(entry);
        }
    }
    eprintln!(
        "creature_spawn_entry: {} guid→entry mappings loaded",
        spawn_entry_map.len()
    );

    // 1) spawns in range → the content slice.
    // db_guid, entry, x,y,z,o, movement_type, respawn_secs, map (map END-appended, work-item 226 —
    // an --include-map row carries its OWN map into the packed payload, not args.map).
    let mut spawns: Vec<(u64, u64, f64, f64, f64, f64, u8, u32, i64)> = Vec::new();
    let mut pool_resolved = 0u32;
    let mut pool_skipped = 0u32;
    for row in parse_table(dump, "creature") {
        let map: i64 = field(&row, cr::MAP).parse().unwrap_or(-1);
        let x: f64 = field(&row, cr::X).parse().unwrap_or(0.0);
        let y: f64 = field(&row, cr::Y).parse().unwrap_or(0.0);
        let z: f64 = field(&row, cr::Z).parse().unwrap_or(0.0);
        let db_guid: u64 = field(&row, cr::GUID).parse().unwrap_or(0);
        let raw_entry: u64 = field(&row, cr::ID).parse().unwrap_or(0);
        // A forced entry bypasses bounded geometry but not the scope's map fence. Keeping the spawn
        // makes its entry flow into every creature-scoped family below.
        if !creature_row_kept_in_scope(&scope, map, x, y, z, raw_entry) {
            continue;
        }
        // cmangos uses id=0 for random spawn-POOL slots; the concrete entry is in creature_spawn_entry.
        // Resolve via the map (first entry wins for determinism). Skip only if unresolvable.
        let entry = if raw_entry == 0 {
            match spawn_entry_map.get(&db_guid) {
                Some(&e) => {
                    pool_resolved += 1;
                    e
                }
                None => {
                    pool_skipped += 1;
                    continue;
                }
            }
        } else {
            raw_entry
        };
        let o: f64 = field(&row, cr::O).parse().unwrap_or(0.0);
        // cmangos MovementType (0 idle / 1 random-wander / 2 waypoint); default IDLE if absent/garbage
        // so a missing cell never wrongly animates a creature.
        let mt: u8 = field(&row, cr::MOVEMENT_TYPE).parse().unwrap_or(0);
        // work-item 047: per-spawn cmangos respawn timer (the MIN leg of spawntimesecsmin/max — the
        // shorter, more common bound). 0/garbage → 0, which the module's decay pass reads as "not
        // imported" and falls back to the flat legacy timer (baseline-safe).
        let respawn_secs: u32 = field(&row, cr::SPAWN_TIME_SECS_MIN).parse().unwrap_or(0);
        spawns.push((db_guid, entry, x, y, z, o, mt, respawn_secs, map));
    }
    if pool_resolved > 0 || pool_skipped > 0 {
        eprintln!("creature id=0 pool slots: {} resolved via creature_spawn_entry, {} unresolvable (no entry row) → skipped", pool_resolved, pool_skipped);
    }

    // 1b) spawn_group pool dedup: cmangos pools named/rare creatures via spawn_group (MaxCount=1) over
    // several placed guids. Without this, every placed point spawns simultaneously — Hogger x5, etc.
    // Parse spawn_group (creature groups only, MaxCount>0) and spawn_group_spawn (guid→groupId map),
    // then for each pool present in the collected spawns, keep only MaxCount members (first by
    // appearance order in the dump, which is deterministic).
    {
        // group_id → max_count (only creature groups with MaxCount > 0)
        let mut group_max_count: std::collections::HashMap<u64, u32> =
            std::collections::HashMap::new();
        for row in parse_table(dump, "spawn_group") {
            let sg_type: i32 = field(&row, sg::TYPE).parse().unwrap_or(-1);
            if sg_type != 0 {
                continue; // skip gameobject groups (type=1)
            }
            let max_count: u32 = field(&row, sg::MAX_COUNT).parse().unwrap_or(0);
            if max_count == 0 {
                continue; // 0 = unlimited — no cap
            }
            let id: u64 = field(&row, sg::ID).parse().unwrap_or(0);
            group_max_count.insert(id, max_count);
        }

        if !group_max_count.is_empty() {
            // guid → group_id (only for groups that have a MaxCount cap)
            let mut guid_to_group: std::collections::HashMap<u64, u64> =
                std::collections::HashMap::new();
            for row in parse_table(dump, "spawn_group_spawn") {
                let group_id: u64 = field(&row, sgs::GROUP_ID).parse().unwrap_or(0);
                if !group_max_count.contains_key(&group_id) {
                    continue;
                }
                let guid: u64 = field(&row, sgs::GUID).parse().unwrap_or(0);
                guid_to_group.insert(guid, group_id);
            }

            if !guid_to_group.is_empty() {
                // Count how many spawns from each pool are retained so far, drop excess.
                let mut group_seen: std::collections::HashMap<u64, u32> =
                    std::collections::HashMap::new();
                let before = spawns.len();
                spawns.retain(
                    |(db_guid, _entry, _x, _y, _z, _o, _mt, _respawn_secs, _map)| {
                        if let Some(&group_id) = guid_to_group.get(db_guid) {
                            let max = *group_max_count.get(&group_id).unwrap_or(&1);
                            let seen = group_seen.entry(group_id).or_insert(0);
                            if *seen < max {
                                *seen += 1;
                                true // keep this spawn
                            } else {
                                false // pool already at capacity
                            }
                        } else {
                            true // not in any capped pool — keep unconditionally
                        }
                    },
                );
                let dropped = before - spawns.len();
                if dropped > 0 {
                    eprintln!(
                        "spawn_group: dropped {dropped} pooled duplicate spawns (MaxCount cap)"
                    );
                }
            }
        }
    }

    let mut entries: std::collections::HashSet<u64> = spawns.iter().map(|s| s.1).collect();
    // PET/SUMMON templates (Tier 3b): a summoned pet (Warlock's Imp = entry 416) has NO world spawn, so it
    // is never in the geographic slice above — yet its game_creature_template row MUST exist for the
    // E_SUMMON_PET handler to build the pet entity. Force-include the pet entries so their templates import
    // from cmangos creature_template even without a placed spawn. Additive (no spawn is added — pets are
    // spawned at runtime by the summon, not seeded). Extend this list as more pets ship (Succubus 1863,
    // Felhunter 417). [Tier 3b]
    const PET_TEMPLATE_ENTRIES: &[u64] = &[416, 1860]; // Imp (688) + Voidwalker (697).
    entries.extend(PET_TEMPLATE_ENTRIES.iter().copied());
    // Forced-creature template safety net: the spawn-loop gate above already keeps a force-listed
    // entry's placed spawns, whether the profile owns it or the legacy flag extends the scope.
    // But a giver with NO `creature` row (a pure-summon/instanced NPC) produces zero spawns — extend
    // `entries` so its template + quest relations still import, and WARN LOUDLY that it will not
    // physically exist in-world (so the operator doesn't ship a phantom giver).
    let scope_name = scope.name();
    for &e in &scope.forced_creature_entries {
        if !entries.contains(&e) {
            eprintln!(
                "WARNING: forced creature {e} in scope {scope_name}: template will import but NO spawn exists in the \
                 dump — the NPC will NOT physically exist in-world (pure-summon/instanced giver?)"
            );
        }
        entries.insert(e);
    }
    let imported_guid_entries: std::collections::HashMap<u64, u64> = spawns
        .iter()
        .map(|(guid, entry, ..)| (*guid, *entry))
        .collect();
    let importable_templates: std::collections::HashSet<u64> =
        parse_table(dump, "creature_template")
            .into_iter()
            .filter_map(|row| {
                let entry = field(&row, ct::ENTRY).parse().ok()?;
                (field(&row, ct::UNIT_FLAGS).parse::<u32>().unwrap_or(0) & 0x0200_0000 == 0)
                    .then_some(entry)
            })
            .collect();
    let eventai = if family_active(args, "creature-ai") {
        loop {
            let plan = eventai::build(
                dump,
                &entries,
                &imported_guid_entries,
                &importable_templates,
            );
            let added = plan
                .forced_template_entries
                .iter()
                .copied()
                .filter(|entry| entries.insert(*entry))
                .count();
            if added == 0 {
                break plan;
            }
        }
    } else {
        eventai::EventAiPlan::default()
    };
    if family_active(args, "creature-ai") {
        eventai.report();
    }
    for slice in &scope.bounded_slices {
        let count = spawns
            .iter()
            .filter(|spawn| slice.contains(spawn.8, spawn.2, spawn.3, spawn.4))
            .count();
        eprintln!(
            "filter: bounded slice {} (map {}) → {count} creature spawns",
            slice.name, slice.map_id
        );
    }
    for &map in &scope.whole_maps {
        let count = spawns.iter().filter(|spawn| spawn.8 == map).count();
        eprintln!("filter: whole map {map} → {count} creature spawns");
    }
    eprintln!(
        "filter: scope {} total → {} creature spawns, {} entries",
        scope.name(),
        spawns.len(),
        entries.len()
    );

    // 2) templates for those entries.
    let mut templates: Vec<String> = Vec::new();
    // entry → LootId for the imported creatures; drives the item + loot ETL below (0 = no loot table).
    let mut creature_loot_ids: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
    // entry → PickpocketLootId / SkinLootId (work-item 210) — same "0 = no table" convention as LootId.
    let mut creature_pickpocket_ids: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
    let mut creature_skin_ids: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
    // Entries whose template carries UNIT_FLAG_NOT_SELECTABLE (0x02000000): invisible server-side
    // trigger / event creatures (Scourge Invasion Necropolis*, Lunar Festival markers, World Trigger,
    // etc.). Their spawns are in the box but should never appear in regular gameplay — skip template
    // AND suppress their spawns from the packed payload below.
    let mut excluded_entries: std::collections::HashSet<u64> = std::collections::HashSet::new();
    // How many imported templates carry a nonzero `trainer_class`. Reported below so an operator can
    // confirm the columns populated: a wrong index reads a real neighbouring cell, which is the one
    // failure mode here that produces plausible data instead of an error.
    let mut class_trainer_count: usize = 0;
    for row in parse_table(dump, "creature_template") {
        let entry: u64 = field(&row, ct::ENTRY).parse().unwrap_or(0);
        if !entries.contains(&entry) {
            continue;
        }
        let unit_flags_raw: u32 = field(&row, ct::UNIT_FLAGS).parse().unwrap_or(0);
        if unit_flags_raw & 0x0200_0000 != 0 {
            excluded_entries.insert(entry);
            continue;
        }
        let loot_id: u64 = field(&row, ct::LOOT_ID).parse().unwrap_or(0);
        if loot_id > 0 {
            creature_loot_ids.insert(entry, loot_id);
        }
        // Loot-family completeness (work-item 210): PickpocketLootId collapses onto creature_entry
        // directly (like LootId, above); SkinLootId is recorded per-entry too but stays keyed on the
        // loot-id ITSELF in game_skinning_loot (several creatures share one level-banded table).
        let pickpocket_loot_id: u64 = field(&row, ct::PICKPOCKET_LOOT_ID).parse().unwrap_or(0);
        if pickpocket_loot_id > 0 {
            creature_pickpocket_ids.insert(entry, pickpocket_loot_id);
        }
        let skin_loot_id: u64 = field(&row, ct::SKIN_LOOT_ID).parse().unwrap_or(0);
        if skin_loot_id > 0 {
            creature_skin_ids.insert(entry, skin_loot_id);
        }
        // Resolve scale: cmangos Scale==0 means "use the client CreatureDisplayInfo scale". Resolve it
        // from the DBC map when --dbc was given, else floor to 1.0 — never emit a literal 0 (the 5875
        // client renders OBJECT_FIELD_SCALE_X=0 invisibly; see codec set_object_scale_x).
        let display_id: u32 = field(&row, ct::MODEL_ID1).parse().unwrap_or(0);
        let cm_scale: f32 = field(&row, ct::SCALE).parse().unwrap_or(0.0);
        // Accept only a POSITIVE scale (a 0 — or a corrupt negative — falls through to the DBC, then
        // to the 1.0 floor); a negative OBJECT_FIELD_SCALE_X would render as broken as a 0.
        let scale_val: f32 = if cm_scale > 0.0 {
            cm_scale
        } else {
            display_scales
                .as_ref()
                .and_then(|m| m.get(&display_id).copied())
                .filter(|s| *s > 0.0)
                .unwrap_or(1.0)
        };
        // MaxLevel / MaxLevelHealth feed the spawn-time level roll; a dump missing the cell → "0"
        // (the module reads 0 as "no range" and falls back to the min). End-appended, like the column.
        let num_or_zero = |idx: usize| {
            let v = field(&row, idx);
            if v.is_empty() {
                "0"
            } else {
                v
            }
        };
        // DELIBERATE DEVIATION (operator request, 2026-06-24): Northshire's tutorial wolves (Young Wolf
        // 299, Timber Wolf 69) ship on the PASSIVE beast faction 32 — neutral to the player, which the
        // 5875 client treats as non-attackable via the "Attack" action AND Tab-targeting (only a real
        // right-click while in melee range engages a yellow unit). Players read that as "the wolf can't
        // be attacked." We make them PASSIVE-BUT-ATTACKABLE like the kobolds by remapping the neutral-wolf
        // faction (32) to faction template 58 (parent "Creature" faction 7): faction_group = Monster (8)
        // so the player reads them as hostile/red → /attack + Tab work, but enemy_group = 0 so they do
        // NOT proximity-aggro (compute_hostile(wolf→player) = 0 & player_group = 0). Net: red, huntable,
        // but they never jump you — they only retaliate, exactly like the kobolds (ft 25, also Monster +
        // enemy_group 0). Gated on CreatureFamily == Wolf (1) so only wolves flip; everything else keeps
        // its cmangos faction. NOT strictly vanilla (real Northshire wolves are yellow/neutral) — a
        // deliberate playability choice; revert by deleting this block. (Was briefly 38, the aggressive
        // wolf faction, but that has enemy_group=1 → wolves aggroed low-level players; 58 is the passive
        // equivalent.) The remap itself is `wolf_faction_fixup` (byte-parity locked by its own unit
        // tests) — already data-driven on `family_id` before work-item 214; 214 only pulled it out to a
        // pure, independently-testable function.
        let faction_raw: u32 = field(&row, ct::FACTION).parse().unwrap_or(0);
        let family_id: i32 = field(&row, ct::FAMILY).parse().unwrap_or(0);
        let faction_out: u32 = wolf_faction_fixup(faction_raw, family_id);
        // Per-creature melee damage (parity #7): cmangos MinMeleeDmg/MaxMeleeDmg are floats; round to int
        // (vanilla creature swings are whole-number ranges in practice). 0 stays 0 → the module's
        // swing_range_ctx falls back to the flat CREATURE_MELEE range for that creature.
        let dmg_min: u32 = field(&row, ct::MIN_MELEE_DMG)
            .parse::<f32>()
            .unwrap_or(0.0)
            .round() as u32;
        let dmg_max: u32 = field(&row, ct::MAX_MELEE_DMG)
            .parse::<f32>()
            .unwrap_or(0.0)
            .round() as u32;
        // game_creature_template column order (see module/src/creatures/spawn.rs). The trailing
        // `aggro_range` (proximity aggro) has no cmangos source column, so emit 0 (passive —
        // retaliate-only, the column default); aggression tuning is a separate content pass.
        // `pickpocket_loot_id`/`skin_loot_id` (work-item 210) are END-appended after `armor`.
        // Trainer service columns, END-appended after `skin_loot_id`. Both parse to `u8` and fall to
        // 0 on an absent or unparseable cell; for `trainer_class` that 0 is the fail-open "serves
        // everyone" value. Not `num_or_zero`: it only rewrites an EMPTY cell and would splice a
        // non-numeric one into the INSERT, aborting the statement.
        let trainer_type_val: u8 = field(&row, ct::TRAINER_TYPE).parse().unwrap_or(0);
        let trainer_class_val: u8 = field(&row, ct::TRAINER_CLASS).parse().unwrap_or(0);
        if trainer_class_val != 0 {
            class_trainer_count += 1;
        }
        templates.push(format!(
            "({entry},{name},{subname},{display},{level},{health},{faction},{npc},{unit},{ctype},{family},{tflags},{rank},{scale},{bat},{mmin},{mmax},{maxlvl},{maxhp},0,{dmin},{dmax},{armor},{pploot},{skinloot},{ttype},{tclass})",
            ttype = trainer_type_val,
            tclass = trainer_class_val,
            armor = num_or_zero(ct::ARMOR),
            dmin = dmg_min,
            dmax = dmg_max,
            name = sql_text(field(&row, ct::NAME)),
            subname = sql_text(field(&row, ct::SUBNAME)),
            display = display_id,
            level = field(&row, ct::MIN_LEVEL),
            health = field(&row, ct::MIN_LEVEL_HEALTH),
            faction = faction_out,
            npc = field(&row, ct::NPC_FLAGS),
            unit = field(&row, ct::UNIT_FLAGS),
            ctype = field(&row, ct::CREATURE_TYPE),
            family = field(&row, ct::FAMILY),
            tflags = field(&row, ct::CREATURE_TYPE_FLAGS),
            rank = field(&row, ct::RANK),
            scale = scale_val,
            bat = field(&row, ct::MELEE_BASE_ATTACK_TIME),
            mmin = field(&row, ct::MIN_LOOT_GOLD),
            mmax = field(&row, ct::MAX_LOOT_GOLD),
            maxlvl = num_or_zero(ct::MAX_LEVEL),
            maxhp = num_or_zero(ct::MAX_LEVEL_HEALTH),
            pploot = pickpocket_loot_id,
            skinloot = skin_loot_id,
        ));
    }

    // 2b) caster-mob cast rows (game_creature_cast, LEGACY) + rotation rows (game_creature_spell, rank 20).
    //
    //   LEGACY (game_creature_cast): one spell per entry (spell1), used by pass_cast as a fallback ONLY when
    //   a creature has NO game_creature_spell rows. Kept for single-spell casters (no spell2+) so they are
    //   byte-identical to the pre-rotation behaviour.
    //
    //   ROTATION (game_creature_spell): emitted for entries that carry spell2+ in creature_template_spells.
    //   These creatures use the rotation path exclusively (pass_cast skips game_creature_cast when any
    //   game_creature_spell rows exist for the entry). Rotation layout per entry:
    //     • spell1 → condition ALWAYS (0), priority 10 — the primary nuke, always fires at the melee target.
    //     • spell2..10 → condition SELF_MISSING_AURA (3), priority 20 — assumed to be self-buffs (e.g. Frost
    //       Armor 12544 on entries 474/476); they fire FIRST (higher priority) when the buff is absent, then
    //       fall silent once applied. If a spell2+ is actually an alternative nuke (same targets as spell1),
    //       the self-cast will fail at resolve_cast_at target-type validation and the engine gracefully falls
    //       through to spell1 — net behaviour is unchanged. Heals and debuffs need a reclassify pass (deferred
    //       content tuning, not a code change).
    //   All referenced spells (spell2+) must be in game_spell; an un-imported id is a silent no-op (Err in
    //   resolve_cast_at). The --only allowlist in import-world.sh must cover them. [static]
    //
    // game_creature_spell condition discriminants (mirror module/src/creatures/spawn.rs cast_condition):
    const COND_ALWAYS: u8 = 0; // unconditional — cast at melee target (nuke)
    const COND_SELF_MISSING_AURA: u8 = 3; // cast at self when self lacks the spell's aura (buff)

    let mut creature_casts: Vec<String> = Vec::new();
    // Explicit id counter for game_creature_spell (auto_inc field; we assign sequentially from 1 rather
    // than relying on DB auto-assign, matching how game_trainer_spell works in this importer).
    let mut rotation_id: u64 = 1;
    let mut creature_rotation_rows: Vec<String> = Vec::new();
    for row in parse_table(dump, "creature_template_spells") {
        let entry: u64 = field(&row, cts::ENTRY).parse().unwrap_or(0);
        // setId 0 = the creature's own (non-set) spell list; skip the (rare) shared-set rows.
        if field(&row, cts::SET_ID).parse::<u64>().unwrap_or(0) != 0 {
            continue;
        }
        if !entries.contains(&entry) {
            continue;
        }
        let spell1: u64 = field(&row, cts::SPELL1).parse().unwrap_or(0);
        if spell1 == 0 {
            continue; // a row with no primary spell never casts — skip.
        }
        // Collect non-zero spell2..spell10 (slots SPELL1+1 through SPELL1+9).
        let secondary: Vec<u64> = (1..=9)
            .filter_map(|i| {
                let s: u64 = field(&row, cts::SPELL1 + i).parse().unwrap_or(0);
                if s != 0 {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();

        if secondary.is_empty() {
            // Single-spell caster: emit the legacy game_creature_cast row only.
            creature_casts.push(format!("({entry},{spell1})"));
        } else {
            // Multi-spell caster: emit a FULL rotation into game_creature_spell (spell1 ALWAYS nuke +
            // spell2+ SELF_MISSING_AURA buffs). The legacy game_creature_cast row is ALSO emitted so
            // that a future re-import without rotation rows still has a working fallback.
            creature_casts.push(format!("({entry},{spell1})"));
            // spell1: priority 10, ALWAYS (nuke at melee target).
            creature_rotation_rows.push(format!(
                "({rotation_id},{entry},{spell1},10,{COND_ALWAYS},0)"
            ));
            rotation_id += 1;
            // spell2+: priority 20 (fires before the nuke when the buff is absent), SELF_MISSING_AURA.
            for s in &secondary {
                creature_rotation_rows.push(format!(
                    "({rotation_id},{entry},{s},20,{COND_SELF_MISSING_AURA},0)"
                ));
                rotation_id += 1;
            }
        }
    }
    eprintln!(
        "mapped: {} caster-mob cast rows (game_creature_cast); \
         {} rotation rows for {} multi-spell entries (game_creature_spell)",
        creature_casts.len(),
        creature_rotation_rows.len(),
        creature_rotation_rows
            .iter()
            .filter(|r| r.contains(&format!(",10,{COND_ALWAYS},0)")))
            .count(),
    );

    // 3) spawn payload for the `import_creature_spawns` reducer — NOT SQL. `game_creature_spawn` has
    //    Timestamp columns (respawn_at/despawn_at) and SpacetimeDB 2.5 SQL has no Timestamp literal,
    //    so spawns load through a reducer (which stamps `ctx.timestamp`). Rows `;`-separated, fields
    //    `,`: guid,entry,map,x,y,z,orientation,movement_type,respawn_secs. (u64 guids exceed JSON's safe-integer
    //    range, so this is one delimited string — never a per-field JSON number that would lose precision.)
    let guid_of: std::collections::HashMap<u64, u64> = spawns
        .iter()
        .filter(|s| !excluded_entries.contains(&s.1))
        .map(|s| (s.0, world_guid(s.1, s.0)))
        .collect();
    let excluded_count = spawns
        .iter()
        .filter(|s| excluded_entries.contains(&s.1))
        .count();
    if excluded_count > 0 {
        eprintln!(
            "filter: excluded {} event/trigger spawns ({} template entries with UNIT_FLAG_NOT_SELECTABLE)",
            excluded_count, excluded_entries.len()
        );
    }
    let packed_spawns: String = spawns
        .iter()
        .filter(|s| !excluded_entries.contains(&s.1))
        .map(|(db_guid, entry, x, y, z, o, mt, respawn_secs, map)| {
            // `map` is the ROW's own map (work-item 226) — an --include-map spawn (Deadmines 36)
            // must land on ITS map, not the primary --map's. The dump Z rides through UNALTERED on
            // every path: the importer NEVER snaps spawn Z to terrain (there is no ground_z here at
            // all — runtime creature legs snap with a keep-current-Z fallback, module terrain.rs),
            // which is exactly the §4 no-ADT-for-WMO-maps design: a map-36 spawn keeps its
            // floor-correct cmangos Z forever. Pinned by
            // `include_map_spawns_import_whole_map_with_their_own_map_and_verbatim_z`.
            format!(
                "{g},{entry},{map},{x},{y},{z},{o},{mt},{respawn_secs}",
                g = world_guid(*entry, *db_guid),
            )
        })
        .collect::<Vec<_>>()
        .join(";");

    // 4) waypoints for the imported spawns (id assigned by a counter — clear+reload is idempotent).
    let mut waypoints: Vec<String> = Vec::new();
    let mut wp_id: u64 = 1;
    // Spawns that carry DIRECT (guid-keyed) rows — the template expansion below never overrides these.
    let mut direct_wp_guids: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for row in parse_table(dump, "creature_movement") {
        let db_guid: u64 = field(&row, cm::ID).parse().unwrap_or(0);
        let Some(&g) = guid_of.get(&db_guid) else {
            continue;
        };
        waypoints.push(format!(
            "({id},{g},{x},{y},{z})",
            id = wp_id,
            x = field(&row, cm::X),
            y = field(&row, cm::Y),
            z = field(&row, cm::Z),
        ));
        direct_wp_guids.insert(db_guid);
        wp_id += 1;
    }
    // 4b) creature_movement_template (work-item 226): ENTRY-keyed paths, expanded onto every
    // in-slice MovementType=2 spawn of that entry that has NO direct rows (direct always wins).
    // Waypoint Zs — like spawn Zs — ride through VERBATIM (no terrain snap exists in the importer;
    // cmangos waypoints are floor-correct, exactly what a WMO map without ground_z needs, doc §4).
    // Fail-open per row (a garbled row is skipped, the path just shortens) — waypoints are optional
    // gating data (a pathless mob idles at its spawn), per the fail-open/fail-loud split (§5).
    {
        // entry → [(point, x, y, z)] for PathId 0 (see `cmt`'s vintage sniff).
        let mut tmpl_paths: std::collections::HashMap<u64, Vec<(u32, String, String, String)>> =
            std::collections::HashMap::new();
        for row in parse_table(dump, "creature_movement_template") {
            let entry: u64 = field(&row, cmt::ENTRY).parse().unwrap_or(0);
            if entry == 0 {
                continue;
            }
            if row.len() >= cmt::NCOLS_WITH_PATH_ID
                && field(&row, cmt::PATH_ID).parse::<u64>().unwrap_or(0) != 0
            {
                continue; // non-default path — the module models one route per creature
            }
            let (pt_i, x_i, y_i, z_i) = cmt::pos_indices(row.len());
            let point: u32 = field(&row, pt_i).parse().unwrap_or(0);
            tmpl_paths.entry(entry).or_default().push((
                point,
                field(&row, x_i).to_string(),
                field(&row, y_i).to_string(),
                field(&row, z_i).to_string(),
            ));
        }
        // Route order = the dump's Point column, not dump row order (defensive sort — cheap).
        for path in tmpl_paths.values_mut() {
            path.sort_by_key(|p| p.0);
        }
        let mut tmpl_spawns = 0usize;
        if !tmpl_paths.is_empty() {
            for (db_guid, entry, _x, _y, _z, _o, mt, _rs, _map) in &spawns {
                if *mt != 2 || direct_wp_guids.contains(db_guid) {
                    continue; // not waypoint-typed, or already routed by direct rows
                }
                let Some(path) = tmpl_paths.get(entry) else {
                    continue;
                };
                let Some(&g) = guid_of.get(db_guid) else {
                    continue;
                }; // excluded entries have no guid
                for (_pt, x, y, z) in path {
                    waypoints.push(format!("({wp_id},{g},{x},{y},{z})"));
                    wp_id += 1;
                }
                tmpl_spawns += 1;
            }
            eprintln!(
                "creature_movement_template: {} entry paths parsed, expanded onto {} waypoint spawns",
                tmpl_paths.len(), tmpl_spawns
            );
        }
    }

    eprintln!(
        "mapped: {} templates ({class_trainer_count} class-gated trainers), {} spawns ({} excluded), {} waypoints",
        templates.len(),
        spawns.len() - excluded_count,
        excluded_count,
        waypoints.len()
    );

    // 5) static content (no Timestamp columns) → plain SQL clear+reload. The spawns + live-entity
    //    reset are NOT here — they go through the `import_creature_spawns` reducer (step 6), which
    //    clears every CREATURE entity + spawn and loads the new spawns with a valid `ctx.timestamp`.
    //    So we only clear/reload templates + waypoints; the reducer owns the rest.
    // family gate (work-item 216): `--family creatures` reloads this block (+ the spawn/waypoint
    // reducer batches below) alone; every other family's block skips it. `--family` absent → every
    // `family_active` call below is unconditionally true, so the full run is byte-identical to
    // pre-216 behavior — see the full-run parity fixture test.
    let mut stmts: Vec<String> = Vec::new();
    if family_active(args, "creatures") {
        stmts.push("DELETE FROM game_creature_waypoint WHERE id > 0".into());
        stmts.push("DELETE FROM game_creature_template WHERE entry > 0".into());
        push_insert(&mut stmts, "game_creature_template", "entry,name,subname,display_id,level,health,faction_template,npc_flags,unit_flags,creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,money_min,money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class", &templates);
        push_insert(
            &mut stmts,
            "game_creature_waypoint",
            "id,creature_guid,x,y,z",
            &waypoints,
        );
    }

    // Gameobjects (work-item 211 widened import): EVERY in-scope cmangos `gameobject_template` type
    // now imports (template + spawn) — previously only an allowlisted subset (GOOBER/curated GATHER/
    // curated CHEST/QUESTGIVER) did. `classify_go_type` decides the stored `type_id` (or drops the row
    // — the TYPE-25 COLLISION GUARD); `go_template_row` then builds the per-type SQL row. The spawns
    // load via the import_gameobjects reducer (game_gameobject has a Timestamp); the packed spawn row
    // carries an `initial_state` field (work-item 211) so a DOOR/BUTTON can spawn already-open.
    let go_tmpls = parse_table(dump, "gameobject_template");
    struct GoMeta {
        stored_type: u8,
        disp: u32,
        name: String,
        data0: u32,
        data1: u32,
        size: f32,
    }
    let mut dropped_type25: Vec<u64> = Vec::new();
    let go_meta: std::collections::HashMap<u64, GoMeta> = go_tmpls
        .iter()
        .filter_map(|row| {
            let entry: u64 = field(row, got::ENTRY).parse().ok()?;
            let raw_type: u32 = field(row, got::TYPE).parse().ok()?;
            let stored_type = match classify_go_type(entry, raw_type) {
                Some(t) => t,
                None => {
                    dropped_type25.push(entry);
                    return None;
                }
            };
            let disp: u32 = field(row, got::DISPLAY_ID).parse().unwrap_or(0);
            let name = field(row, got::NAME).to_string();
            let data0: u32 = field(row, got::DATA0).parse().unwrap_or(0);
            let data1: u32 = field(row, got::DATA1).parse().unwrap_or(0);
            // 0 on an unparseable/absent column — the gateway reads that as "no size stored" and
            // sends 1.0, never an invisible prop.
            let size: f32 = field(row, got::SIZE).parse().unwrap_or(0.0);
            Some((
                entry,
                GoMeta {
                    stored_type,
                    disp,
                    name,
                    data0,
                    data1,
                    size,
                },
            ))
        })
        .collect();
    // GOOBER entries (ALL, not just spawned) — kept for `build_quests`' USE_GAMEOBJECT objective gate
    // below (unchanged behavior from pre-211: a quest referencing a non-GOOBER or unimported GO entry
    // is dropped rather than left stuck).
    let goober_entries: std::collections::HashSet<u64> = go_meta
        .iter()
        .filter(|(_, m)| m.stored_type == GO_GOOBER)
        .map(|(&e, _)| e)
        .collect();
    // Spawn-membership test: work-item 211 widens this to EVERY classified template (previously a
    // union of 4 allowlists) — the whole point of the widened import. `go_meta.get(&id)` below IS the
    // membership test (a dropped/unclassified entry has no `go_meta` row).
    let mut go_packed_rows: Vec<String> = Vec::new();
    let mut used_go: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut go_slice_counts = vec![0usize; scope.bounded_slices.len()];
    let mut go_whole_map_counts: std::collections::BTreeMap<i64, usize> =
        std::collections::BTreeMap::new();
    for row in parse_table(dump, "gameobject") {
        let map = field(&row, go::MAP).parse::<i64>().unwrap_or(-1);
        let id: u64 = field(&row, go::ID).parse().unwrap_or(0);
        let Some(meta) = go_meta.get(&id) else {
            continue;
        };
        let x: f64 = field(&row, go::X).parse().unwrap_or(0.0);
        let y: f64 = field(&row, go::Y).parse().unwrap_or(0.0);
        let z: f64 = field(&row, go::Z).parse().unwrap_or(0.0);
        // Creature and GameObject rows consume the same scope predicate.
        if !scope.contains(map, x, y, z) {
            continue;
        }
        let db_guid: u64 = field(&row, go::GUID).parse().unwrap_or(0);
        let o: f64 = field(&row, go::O).parse().unwrap_or(0.0);
        // The spawn quaternion (#515) — carried verbatim; 0,0,0,0 (a row with no rotation columns,
        // or a genuinely-identity spawn) is a valid packed value, not a parse failure, so the
        // fallback is `0.0` exactly like every other numeric field this loop parses.
        let rot0: f64 = field(&row, go::ROT0).parse().unwrap_or(0.0);
        let rot1: f64 = field(&row, go::ROT1).parse().unwrap_or(0.0);
        let rot2: f64 = field(&row, go::ROT2).parse().unwrap_or(0.0);
        let rot3: f64 = field(&row, go::ROT3).parse().unwrap_or(0.0);
        let initial_state = go_initial_state(meta.stored_type, meta.data0);
        // `m = map` (the row's own map, never args.map) — the packed-spawn precedent above.
        go_packed_rows.push(format!(
            "{g},{id},{m},{x},{y},{z},{o},{initial_state},{rot0},{rot1},{rot2},{rot3}",
            g = go_guid(db_guid),
            m = map
        ));
        for (index, slice) in scope.bounded_slices.iter().enumerate() {
            if slice.contains(map, x, y, z) {
                go_slice_counts[index] += 1;
            }
        }
        if scope.whole_maps.contains(&map) {
            *go_whole_map_counts.entry(map).or_insert(0) += 1;
        }
        used_go.insert(id);
    }
    for (slice, count) in scope.bounded_slices.iter().zip(go_slice_counts) {
        eprintln!(
            "filter: bounded slice {} (map {}) → {count} gameobject spawns",
            slice.name, slice.map_id
        );
    }
    for &map in &scope.whole_maps {
        let count = go_whole_map_counts.get(&map).copied().unwrap_or(0);
        eprintln!("filter: whole map {map} → {count} gameobject spawns");
    }
    // Template rows for the GOs actually spawned, split by type (`go_template_row`) — a SQL INSERT must
    // name EVERY column (#[default] is NOT applied on INSERT, only on migration — slice-5 data-loss
    // lesson). Also builds the per-type coverage histogram + the CHEST lootId set (widened to EVERY
    // imported chest, not a curated allowlist — work-item 210's scoping now spans the whole live set).
    let mut go_template_rows: Vec<String> = Vec::new();
    let mut chest_loot_ids_used: Vec<u32> = Vec::new();
    let mut go_type_histogram: std::collections::BTreeMap<u8, u32> =
        std::collections::BTreeMap::new();
    let mut used_sorted: Vec<u64> = used_go.iter().copied().collect();
    used_sorted.sort_unstable();
    for entry in used_sorted {
        let meta = &go_meta[&entry];
        *go_type_histogram.entry(meta.stored_type).or_insert(0) += 1;
        let (row, loot_id_used) = go_template_row(
            entry,
            meta.stored_type,
            meta.disp,
            &meta.name,
            meta.data0,
            meta.data1,
            meta.size,
        );
        go_template_rows.push(row);
        if let Some(loot_id) = loot_id_used {
            chest_loot_ids_used.push(loot_id);
        }
    }
    // Per-type coverage (work-item 211): loud so an operator sees exactly what a box pulled in — which
    // types are LIVE (dispatched by `apply_use_gameobject`) vs INERT (template+spawn only, `use` no-ops).
    const GO_LIVE_TYPE_NAMES: &[(u8, &str)] = &[
        (GO_DOOR, "DOOR"),
        (GO_BUTTON, "BUTTON"),
        (GO_CHEST, "CHEST"),
        (GO_GOOBER, "GOOBER"),
        (GO_GATHER, "GATHER"),
    ];
    eprintln!(
        "gameobject type coverage ({} templates, {} spawns):",
        go_template_rows.len(),
        go_packed_rows.len()
    );
    for (&type_id, &count) in &go_type_histogram {
        if type_id == GO_QUESTGIVER {
            // Served by the quest-giver flow (game_gameobject_quest), not by `apply_use_gameobject` —
            // neither LIVE (use-dispatched) nor INERT.
            eprintln!("  type {type_id:>2} QUESTGIVER {count} (quest flow, not use-dispatched)");
            continue;
        }
        match GO_LIVE_TYPE_NAMES.iter().find(|&&(t, _)| t == type_id) {
            Some((_, name)) => eprintln!("  type {type_id:>2} {name:<10} {count} (LIVE)"),
            None => eprintln!("  type {type_id:>2} (unhandled) {count} INERT — use() no-ops"),
        }
    }
    if !dropped_type25.is_empty() {
        eprintln!(
            "  TYPE-25 COLLISION GUARD: dropped {} real cmangos FISHINGHOLE (type 25) template(s) from \
             import entirely (entries {:?}) — module type 25 is reserved for the synthetic GATHER marker",
            dropped_type25.len(), dropped_type25
        );
    }

    // Trainer spell lists: npc_trainer rows for the SPAWNED trainer creatures → game_trainer_spell
    // (cmangos npc_trainer is all DIRECT entries — no template indirection). Lights up the trainer system.
    // Two kinds of row are emitted:
    //   1) CLASS-SPELL offerings — every npc_trainer row, verbatim, with learn_skill_line=0 (the unchanged
    //      spell path; cmangos profession RECIPE rows (reqskill != 0) also ride here — their spells aren't
    //      in game_spell so they're inert, exactly as before, but harmless).
    //   2) SYNTHESIZED profession LEARN rows (PROFESSION_LEARN) — for each in-box trainer seen teaching one
    //      of our 12 lines, the Apprentice→Artisan learn tiers with the REAL Spell.dbc id + learn_skill_line.
    let mut trainer_rows: Vec<String> = Vec::new();
    let mut trainer_id: u64 = 1;
    // Per in-box trainer: the SkillLines it teaches (from reqskill != 0 rows) ∪ the spell ids it already
    // offers (so a synthesized learn-row never collides with a class/recipe row for the SAME trainer).
    let mut trainer_lines: std::collections::BTreeMap<u64, std::collections::BTreeSet<u32>> =
        std::collections::BTreeMap::new();
    let mut trainer_spells: std::collections::HashMap<u64, std::collections::HashSet<u64>> =
        std::collections::HashMap::new();
    for row in parse_table(dump, "npc_trainer") {
        let entry: u64 = field(&row, nt::ENTRY).parse().unwrap_or(0);
        if !entries.contains(&entry) {
            continue;
        }
        let spell: u64 = field(&row, nt::SPELL).parse().unwrap_or(0);
        if spell == 0 {
            continue;
        }
        let cost: u64 = field(&row, nt::COST).parse().unwrap_or(0);
        let reqlevel: u32 = field(&row, nt::REQ_LEVEL).parse().unwrap_or(0);
        // Record the profession lines this trainer teaches (reqskill matching one of our 12) + every spell
        // id it offers, for the synthesis pass below (de-dup keyed on entry).
        let reqskill: u32 = field(&row, nt::REQ_SKILL).parse().unwrap_or(0);
        if profession_tiers(reqskill).is_some() {
            trainer_lines.entry(entry).or_default().insert(reqskill);
        }
        trainer_spells.entry(entry).or_default().insert(spell);
        // CLASS-SPELL / recipe row: learn_skill_line=0, learn_skill_cap=75 — the unchanged spell path (the
        // cap is ignored on a line-0 row but must be NAMED on the INSERT; the 75 matches the module's
        // #[default(75u32)] so an explicit-INSERT row equals a migrated one).
        trainer_rows.push(format!(
            "({trainer_id},{entry},{spell},{cost},{reqlevel},0,75)"
        ));
        trainer_id += 1;
    }
    // Synthesize the profession LEARN rows: for each in-box trainer teaching one of our 12 lines, emit
    // the Apprentice→Artisan tiers (real Spell.dbc id → SMSG_LEARNED_SPELL resolves). cost 0 (the
    // gossip-learn is free in vanilla). Skip a tier whose spell id already collides with a class/recipe
    // row for THIS trainer (so a trainer can't double-insert the same spell_id). Every column is NAMED
    // (slice-5 lesson). The cap (work-item 208) comes from the REAL client's SkillTiers.dbc data when
    // --dbc was given (already parity-checked equal to LEGACY_TIER_CAPS above); without --dbc there's no
    // client data to resolve, so it falls back to LEGACY_TIER_CAPS directly (documented fallback).
    let mut synthesized = 0usize;
    for (entry, lines) in &trainer_lines {
        let offered = trainer_spells.get(entry); // the trainer's existing spell ids (de-dup guard)
        for line in lines {
            for &(spell, tier_index, req) in profession_tiers(*line).unwrap() {
                if offered.is_some_and(|s| s.contains(&(spell as u64))) {
                    continue; // a class/recipe row already carries this spell_id for this trainer
                }
                let cap = profession_tier_values
                    .as_ref()
                    .and_then(|tv| tv.get(line))
                    .map(|caps| caps[tier_index as usize] as u32)
                    .unwrap_or(LEGACY_TIER_CAPS[tier_index as usize]);
                trainer_rows.push(format!(
                    "({trainer_id},{entry},{spell},0,{req},{line},{cap})"
                ));
                trainer_id += 1;
                synthesized += 1;
            }
        }
    }
    eprintln!(
        "mapped: {} trainer spells ({synthesized} synthesized profession learn-rows across {} trainers)",
        trainer_rows.len(), trainer_lines.len(),
    );

    // Quests: map quest_template + the giver-relation tables (creature AND gameobject) for quests given
    // by an in-slice giver. Built BEFORE the item ETL so the reward item entries can be folded into the
    // item-template load (a turn-in's reward grant joins game_item_template).
    // Obtainability gate (ZERO-STUCK-QUESTS): the set of items that drop from in-slice creatures, so
    // build_quests only emits a COLLECT objective for a ReqItem that is obtainable (drops OR is the
    // quest's SrcItem) — else the quest auto-completes instead of soft-locking.
    // 042: obtainable = creature drops ∪ in-slice GO (chest) loot ∪ in-slice vendor stock. The old
    // creature-drops-only gate skipped GO/vendor-sourced ReqItems entirely → an EMPTY objective set →
    // quest_is_complete() == true on accept → free turn-in reward (q3904 grapes / q5545 wood via GO
    // loot, q3861 kodo feed via vendor — all three obtainable in-slice now, verified live).
    let mut obtainable_items = creature_drop_item_set(dump, &creature_loot_ids);
    obtainable_items.extend(gameobject_loot_item_set(dump, &used_go));
    obtainable_items.extend(vendor_stock_item_set(dump, &entries));
    let quests = build_quests(dump, &entries, &used_go, &goober_entries, &obtainable_items);
    eprintln!(
        "mapped: {} quests, {} text, {} objectives, {} reward items, {} choice rewards, \
         {} creature giver relations, {} gameobject giver relations, {} chained (next_quest_id>0) [V], \
         {} timed (limit_time>0) [V]",
        quests.templates.len(), quests.texts.len(), quests.objectives.len(),
        quests.reward_items.len(), quests.reward_choices.len(), quests.relations.len(),
        quests.go_relations.len(), quests.chained_count, quests.timed_count,
    );

    // P4: real item templates + creature loot for the imported creatures (clear+reload, like above).
    // game_item_template is wiped+reloaded with the loot-referenced set ∪ the starter loadout ∪ quest
    // reward items ∪ quest COLLECT (ReqItem) items (so the login grant keeps a real weapon, turn-ins can
    // deliver, AND every collect-quest item has a template); game_creature_loot is fully importer-owned.
    let extra_item_entries: std::collections::HashSet<u64> = quests
        .reward_item_entries
        .iter()
        .chain(quests.req_item_entries.iter())
        .copied()
        .collect();
    let (item_rows, loot_rows, vendor_rows) =
        build_items_and_loot(dump, &creature_loot_ids, &entries, &extra_item_entries);
    eprintln!(
        "mapped: {} item_templates, {} creature_loot rows, {} npc_vendor rows ({} creatures with loot)",
        item_rows.len(), loot_rows.len(), vendor_rows.len(), creature_loot_ids.len()
    );
    if family_active(args, "items") {
        stmts.push("DELETE FROM game_item_template WHERE entry > 0".into());
        stmts.push("DELETE FROM game_creature_loot WHERE id > 0".into());
        stmts.push("DELETE FROM game_npc_vendor WHERE id > 0".into());
        push_insert(&mut stmts, "game_item_template", "entry,class,subclass,name,display_id,quality,inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race", &item_rows);
        push_insert(
            &mut stmts,
            "game_creature_loot",
            "id,creature_entry,item_entry,chance_bp,count,group_id,quest_only",
            &loot_rows,
        );
        push_insert(
            &mut stmts,
            "game_npc_vendor",
            "id,creature_entry,item_entry,slot,max_count",
            &vendor_rows,
        );
    }

    // Loot-family completeness (work-item 210): pickpocket / skinning / gameobject(chest) / fishing.
    // `refs` (the reference_loot_template pool map) is parsed ONCE here and shared by all four —
    // `parse_creature_drops` above already parsed its own copy for the creature family; a second parse
    // for these four is the same low-cost tradeoff this importer already accepts (content-slice sized).
    let refs = parse_ref_pools(dump);

    // PICKPOCKET: keyed directly on CREATURE ENTRY (PickpocketLootId collapsed like LootId), scoped to
    // in-slice creatures only (`creature_pickpocket_ids` is already restricted to `entries`).
    let drops_by_pickpocketid = parse_loot_family(dump, "pickpocketing_loot_template", &refs);
    let mut pickpocket_rows: Vec<String> = Vec::new();
    let mut pickpocket_row_id: u64 = 1;
    let mut pp_creatures: Vec<(&u64, &u64)> = creature_pickpocket_ids.iter().collect();
    pp_creatures.sort(); // deterministic id assignment across re-imports
    for (creature_entry, pp_loot_id) in pp_creatures {
        let Some(drops) = drops_by_pickpocketid.get(pp_loot_id) else {
            continue;
        };
        for &(item, chance_bp, count, group_id, quest_only) in drops {
            pickpocket_rows.push(format!("({pickpocket_row_id},{creature_entry},{item},{chance_bp},{count},{group_id},{quest_only})"));
            pickpocket_row_id += 1;
        }
    }

    // SKINNING: keyed on skin_loot_id DIRECTLY (NOT collapsed onto creature_entry — several creatures of
    // the same level band share ONE table). Scope: the distinct skin_loot_ids actually used by in-slice
    // creatures.
    let drops_by_skinid = parse_loot_family(dump, "skinning_loot_template", &refs);
    let mut skin_ids_used: Vec<u64> = creature_skin_ids.values().copied().collect();
    skin_ids_used.sort_unstable();
    skin_ids_used.dedup();
    let mut skinning_rows: Vec<String> = Vec::new();
    let mut skinning_row_id: u64 = 1;
    for skin_id in skin_ids_used {
        let Some(drops) = drops_by_skinid.get(&skin_id) else {
            continue;
        };
        for &(item, chance_bp, count, group_id, _quest_only) in drops {
            // game_skinning_loot carries no quest_only column (module doc: no cmangos skinning row is
            // ever quest-gated) — discarded here, not emitted.
            skinning_rows.push(format!(
                "({skinning_row_id},{skin_id},{item},{chance_bp},{count},{group_id})"
            ));
            skinning_row_id += 1;
        }
    }

    // GAMEOBJECT (CHEST) loot: keyed on loot_id = the REAL `gameobject_template.Data1` (see `chest_meta`
    // above). Scoped to the lootIds actually used by CHESTs spawned in this box (`chest_loot_ids_used`)
    // — GATHER nodes never touch this table (they stay on the hardcoded GATHER_NODES grant).
    let drops_by_goloot = parse_loot_family(dump, "gameobject_loot_template", &refs);
    let mut go_loot_ids_used: Vec<u32> = chest_loot_ids_used.clone();
    go_loot_ids_used.sort_unstable();
    go_loot_ids_used.dedup();
    let mut gameobject_loot_rows: Vec<String> = Vec::new();
    let mut go_loot_row_id: u64 = 1;
    for loot_id in go_loot_ids_used {
        let Some(drops) = drops_by_goloot.get(&(loot_id as u64)) else {
            continue;
        };
        for &(item, chance_bp, count, group_id, quest_only) in drops {
            gameobject_loot_rows.push(format!(
                "({go_loot_row_id},{loot_id},{item},{chance_bp},{count},{group_id},{quest_only})"
            ));
            go_loot_row_id += 1;
        }
    }

    // FISHING: ZONE-keyed (`entry` IS an AreaTable zone id — no creature/GO indirection to scope by).
    // Imported WHOLE — the same "harmless superset" precedent `item_template` already rides (P4, above):
    // an out-of-slice zone's rows are simply unreachable at roll time (`fishing_zone_id` never resolves a
    // zone we didn't import terrain+AreaTable coverage for), not a dangling-reference risk the way an
    // unscoped creature/GO loot import would be.
    let drops_by_zone = parse_loot_family(dump, "fishing_loot_template", &refs);
    let mut fishing_rows: Vec<String> = Vec::new();
    let mut fishing_row_id: u64 = 1;
    let mut zones: Vec<&u64> = drops_by_zone.keys().collect();
    zones.sort();
    for zone in zones {
        for &(item, chance_bp, count, group_id, _quest_only) in &drops_by_zone[zone] {
            // game_fishing_loot carries no quest_only column — discarded, not emitted.
            fishing_rows.push(format!(
                "({fishing_row_id},{zone},{item},{chance_bp},{count},{group_id})"
            ));
            fishing_row_id += 1;
        }
    }

    eprintln!(
        "mapped: {} pickpocket_loot rows ({} creatures), {} skinning_loot rows ({} skin tables), \
         {} gameobject_loot rows ({} chest loot ids), {} fishing_loot rows ({} zones)",
        pickpocket_rows.len(),
        creature_pickpocket_ids.len(),
        skinning_rows.len(),
        creature_skin_ids
            .values()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        gameobject_loot_rows.len(),
        chest_loot_ids_used.len(),
        fishing_rows.len(),
        drops_by_zone.len(),
    );
    if family_active(args, "loot") {
        stmts.push("DELETE FROM game_pickpocket_loot WHERE id > 0".into());
        stmts.push("DELETE FROM game_skinning_loot WHERE id > 0".into());
        stmts.push("DELETE FROM game_gameobject_loot WHERE id > 0".into());
        stmts.push("DELETE FROM game_fishing_loot WHERE id > 0".into());
        push_insert(
            &mut stmts,
            "game_pickpocket_loot",
            "id,creature_entry,item_entry,chance_bp,count,group_id,quest_only",
            &pickpocket_rows,
        );
        push_insert(
            &mut stmts,
            "game_skinning_loot",
            "id,skin_loot_id,item_entry,chance_bp,count,group_id",
            &skinning_rows,
        );
        push_insert(
            &mut stmts,
            "game_gameobject_loot",
            "id,loot_id,item_entry,chance_bp,count,group_id,quest_only",
            &gameobject_loot_rows,
        );
        push_insert(
            &mut stmts,
            "game_fishing_loot",
            "id,zone_id,item_entry,chance_bp,count,group_id",
            &fishing_rows,
        );
    }

    // Gossip menus + NPC text + menu options (work-item 217): map each in-box gossip NPC's
    // creature_entry to the npc_text id shown in SMSG_GOSSIP_MESSAGE, import the resolved greeting
    // strings (all 8 weighted slots, not just the first), and import its clickable menu options
    // (vendor/innkeeper/trainer/plain-gossip, quest-status conditions folded). Four-table clear+reload.
    let gossip = build_gossip_sql(dump, &entries);
    if family_active(args, "gossip") {
        // RESERVED RANGES (283): the ETL owns only the low id bands (it emits dense low ids); a
        // PACKAGE that mints its own gossip (e.g. the dynamic-events guard) uses the high bands —
        // menu.entry ≥ 1_000_000 (above every real creature entry), and option.row_id / text_id /
        // slot.id ≥ 50_000 — so a routine import never wipes package rows. Documented in danger-zones.
        stmts.push("DELETE FROM game_gossip_menu WHERE entry > 0 AND entry < 1000000".into());
        stmts.push("DELETE FROM game_npc_text WHERE text_id > 0 AND text_id < 50000".into());
        stmts.push("DELETE FROM game_npc_text_slot WHERE id >= 0 AND id < 50000".into());
        stmts.push("DELETE FROM game_gossip_option WHERE row_id >= 0 AND row_id < 50000".into());
        push_insert(
            &mut stmts,
            "game_gossip_menu",
            "entry,text_id",
            &gossip.menu_rows,
        );
        push_insert(
            &mut stmts,
            "game_npc_text",
            "text_id,text",
            &gossip.npc_text_rows,
        );
        push_insert(
            &mut stmts,
            "game_npc_text_slot",
            "id,text_id,slot_index,text_male,text_female,probability",
            &gossip.npc_text_slot_rows,
        );
        push_insert(&mut stmts, "game_gossip_option", "row_id,entry,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2", &gossip.option_rows);
    }

    // Quests: clear+reload the 6 static quest tables (header / body text / objectives / reward items /
    // creature giver relations / gameobject giver relations — work-item 041). game_character_quest
    // (per-player progress) is born in the accept reducer, never here.
    if family_active(args, "quests") {
        stmts.push("DELETE FROM game_quest_template WHERE entry > 0".into());
        stmts.push("DELETE FROM game_quest_text WHERE quest_entry > 0".into());
        stmts.push("DELETE FROM game_quest_objective WHERE id > 0".into());
        stmts.push("DELETE FROM game_quest_reward_item WHERE id > 0".into());
        stmts.push("DELETE FROM game_quest_reward_choice WHERE id > 0".into());
        stmts.push("DELETE FROM game_creature_quest WHERE id > 0".into());
        stmts.push("DELETE FROM game_gameobject_quest WHERE id > 0".into());
        push_insert(&mut stmts, "game_quest_template", "entry,min_level,quest_level,title,reward_money,reward_xp,prev_quest_id,required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,limit_time,reward_money_max_level", &quests.templates);
        push_insert(
            &mut stmts,
            "game_quest_text",
            "quest_entry,details,objectives,offer_reward_text,request_items_text",
            &quests.texts,
        );
        push_insert(
            &mut stmts,
            "game_quest_objective",
            "id,quest_entry,obj_index,kind,target_entry,required_count",
            &quests.objectives,
        );
        push_insert(
            &mut stmts,
            "game_quest_reward_item",
            "id,quest_entry,item_entry,count",
            &quests.reward_items,
        );
        push_insert(
            &mut stmts,
            "game_quest_reward_choice",
            "id,quest_entry,choice_index,item_entry,count",
            &quests.reward_choices,
        );
        push_insert(
            &mut stmts,
            "game_creature_quest",
            "id,creature_entry,quest_entry,role",
            &quests.relations,
        );
        push_insert(
            &mut stmts,
            "game_gameobject_quest",
            "id,go_entry,quest_entry,role",
            &quests.go_relations,
        );
    }

    // GOOBER gameobject templates (family "gameobjects" — no-Timestamp → plain SQL clear+reload).
    // The gameobject SPAWNS load separately via the import_gameobjects reducer below (also family-gated).
    if family_active(args, "gameobjects") {
        stmts.push("DELETE FROM game_gameobject_template WHERE entry > 0".into());
        push_insert(&mut stmts, "game_gameobject_template", "entry,type_id,display_id,name,data0,data1,gather_skill_line,respawn_secs,gather_gray,lock_id,size", &go_template_rows);
    }
    // Trainer spell lists (family "trainers").
    if family_active(args, "trainers") {
        stmts.push("DELETE FROM game_trainer_spell WHERE id > 0".into());
        push_insert(
            &mut stmts,
            "game_trainer_spell",
            "id,trainer_entry,spell_id,cost,required_level,learn_skill_line,learn_skill_cap",
            &trainer_rows,
        );
    }

    // Caster-mob cast + rotation rows (family "casts"). The primary-key is creature_entry, so clear ALL
    // rows then reload the in-box set (importer-owned).
    if family_active(args, "casts") {
        stmts.push("DELETE FROM game_creature_cast WHERE creature_entry > 0".into());
        push_insert(
            &mut stmts,
            "game_creature_cast",
            "creature_entry,spell_id",
            &creature_casts,
        );
        // Rotation rows (game_creature_spell, rank 20): multi-spell casters use these instead of (or in
        // addition to) game_creature_cast. Clear ALL rows first (same pattern as game_creature_cast: the
        // importer owns this table, and an in-box re-import is a full reload). Then re-seed from
        // creature_rotation_rows assembled above.
        stmts.push("DELETE FROM game_creature_spell WHERE id > 0".into());
        push_insert(
            &mut stmts,
            "game_creature_spell",
            "id,creature_entry,spell_id,priority,condition,condition_value",
            &creature_rotation_rows,
        );
    }

    if family_active(args, "creature-ai") {
        stmts.push("DELETE FROM game_creature_ai_event WHERE id > 0".into());
        stmts.push("DELETE FROM game_creature_ai_broadcast_text WHERE id > 0".into());
        stmts.push("DELETE FROM game_creature_ai_summon WHERE id > 0".into());
        push_insert(
            &mut stmts,
            "game_creature_ai_event",
            "id,creature_entry,event_type,action_type,text,spell_id,initial_min_ms,initial_max_ms,repeat_min_ms,repeat_max_ms,source_rule_id,action_order,creature_guid,chance_pct,allowed_phase_mask,source_flags,repeat_policy,event_param_1,event_param_2,event_param_3,event_param_4,event_param_5,event_param_6,action_param_1,action_param_2,action_param_3,target_policy,cast_options",
            &eventai.event_rows,
        );
        push_insert(
            &mut stmts,
            "game_creature_ai_broadcast_text",
            "id,male_text,female_text,chat_type,language_id,emote_delay_1_ms,emote_id_1,emote_delay_2_ms,emote_id_2,emote_delay_3_ms,emote_id_3",
            &eventai.broadcast_rows,
        );
        push_insert(
            &mut stmts,
            "game_creature_ai_summon",
            "id,x,y,z,orientation,lifetime_ms",
            &eventai.summon_rows,
        );
    }

    // P3: the class/level stat curve + P1: all-(race,class) start positions + 209: graveyard-zone
    // links (all global, plain SQL — not box-scoped) + work-item 212's createinfo spells/actions/items
    // + work-item 225's areatrigger-teleport dungeon portals.
    // Family "globals": not box-scoped, but its own clear+reload — gated too, so `--family quests`
    // reloads QUESTS ALONE (not quests-plus-globals). No other family's builder reads these outputs, so
    // gating their compute too (not just the push) is safe — unlike the entangled quests/items pair.
    // "globals" rode by 225 rather than a new dedicated family: `game_areatrigger_teleport` is a small
    // world-wide reference table with the exact same shape as `game_graveyard_zone` right next to it
    // (global, own clear+reload, nothing else in this function reads its output) — no new family value
    // earns its keep for one more small table.
    let mut globals_row_count = 0u64;
    if family_active(args, "globals") {
        let stats_sql = build_stats_sql(dump);
        let start_pos_sql = build_start_position_sql(dump);
        let graveyard_sql = build_graveyard_zone_sql(dump);
        let createinfo_spell_sql = build_createinfo_spell_sql(dump);
        let createinfo_action_sql = build_createinfo_action_sql(dump);
        let createinfo_item_sql = build_createinfo_item_sql(dump, args.dbc.as_deref())?;
        let (areatrigger_teleport_sql, areatrigger_teleport_count) =
            build_areatrigger_teleport_sql(dump);
        // Count DATA ROWS, not statements: these builders return DELETE + batched-INSERT
        // statement lists (push_insert packs up to 500 rows per INSERT), so `.len()` would
        // under-report by the batch factor and defeat the provenance stamp's purpose.
        // `areatrigger_teleport_count` is added as its OWN pre-counted `usize`, not via
        // `insert_row_count` — that helper is "only sound for numeric-payload tables" (its own doc
        // comment), and `game_areatrigger_teleport.name` is free text that could (in principle)
        // contain the `),(` tuple separator `insert_row_count` scans for; `build_areatrigger_teleport_sql`
        // already counts its own rows correctly before formatting them into SQL.
        globals_row_count = insert_row_count(&stats_sql)
            + insert_row_count(&start_pos_sql)
            + insert_row_count(&graveyard_sql)
            + insert_row_count(&createinfo_spell_sql)
            + insert_row_count(&createinfo_action_sql)
            + insert_row_count(&createinfo_item_sql)
            + areatrigger_teleport_count as u64;
        stmts.extend(stats_sql);
        stmts.extend(start_pos_sql);
        stmts.extend(graveyard_sql);
        stmts.extend(createinfo_spell_sql);
        stmts.extend(createinfo_action_sql);
        stmts.extend(createinfo_item_sql);
        stmts.extend(areatrigger_teleport_sql);
    }

    // Spell metadata (family "spellmeta", work-item 102 reduced scope): rank chain + auto-learn
    // dependents. Not box-scoped (global, like "globals"), plain SQL (no Timestamp). Nothing else in
    // this function reads either builder's output, so — unlike the entangled quests/items pair — it's
    // safe to gate the COMPUTE too, not just the push (the "globals" precedent).
    let mut spellmeta_row_count = 0u64;
    if family_active(args, "spellmeta") {
        let spell_chain_sql = build_spell_chain_sql(dump);
        let spell_learn_sql = build_spell_learn_sql(dump);
        spellmeta_row_count =
            insert_row_count(&spell_chain_sql) + insert_row_count(&spell_learn_sql);
        stmts.extend(spell_chain_sql);
        stmts.extend(spell_learn_sql);
    }

    // The spawn payload can hold ~2k rows for a full zone — more than one `spacetime call` string arg
    // takes — so split it into batches. Batch 0 goes to `import_creature_spawns` (clears the live
    // roster + threat ONCE, then loads); every later batch goes to `import_creature_spawns_append`
    // (load only), so the clear happens exactly once and every batch survives.
    let spawn_rows: Vec<&str> = packed_spawns.split(';').filter(|r| !r.is_empty()).collect();
    let batches: Vec<String> = spawn_rows
        .chunks(SPAWN_BATCH)
        .map(|c| c.join(";"))
        .collect();
    let go_batches: Vec<String> = go_packed_rows
        .chunks(SPAWN_BATCH)
        .map(|c| c.join(";"))
        .collect();

    // Provenance stamps (work-item 216): one entry per family that ACTUALLY had its block pushed
    // above — mirrors exactly the `family_active` gates this function used, so a stamp only ever
    // corresponds to data this run's plan actually carries.
    let mut stamps: Vec<(&'static str, u64)> = Vec::new();
    if family_active(args, "creatures") {
        stamps.push(("creatures", templates.len() as u64));
    }
    if family_active(args, "items") {
        stamps.push(("items", item_rows.len() as u64));
    }
    if family_active(args, "loot") {
        stamps.push((
            "loot",
            (pickpocket_rows.len()
                + skinning_rows.len()
                + gameobject_loot_rows.len()
                + fishing_rows.len()) as u64,
        ));
    }
    if family_active(args, "gossip") {
        stamps.push((
            "gossip",
            (gossip.menu_rows.len()
                + gossip.npc_text_rows.len()
                + gossip.npc_text_slot_rows.len()
                + gossip.option_rows.len()) as u64,
        ));
    }
    if family_active(args, "quests") {
        stamps.push(("quests", quests.templates.len() as u64));
    }
    if family_active(args, "gameobjects") {
        stamps.push(("gameobjects", go_template_rows.len() as u64));
    }
    if family_active(args, "trainers") {
        stamps.push(("trainers", trainer_rows.len() as u64));
    }
    if family_active(args, "casts") {
        stamps.push((
            "casts",
            (creature_casts.len() + creature_rotation_rows.len()) as u64,
        ));
    }
    if family_active(args, "creature-ai") {
        stamps.push(("creature-ai", eventai.row_count()));
    }
    if family_active(args, "globals") {
        stamps.push(("globals", globals_row_count));
    }
    if family_active(args, "spellmeta") {
        stamps.push(("spellmeta", spellmeta_row_count));
    }

    Ok(DumpPlan {
        stmts,
        spawn_row_count: spawn_rows.len(),
        go_row_count: go_packed_rows.len(),
        spawn_batches: batches,
        go_batches,
        stamps,
    })
}

fn main() -> Result<()> {
    let args = parse_args()?;

    // `--pack-client` → build the client patch MPQ + install addons into the operator's own client.
    // Standalone mode (independent of the cmangos/DBC import streams).
    if let Some(dir) = args.pack_client.clone() {
        return pack_client::run(&dir, &args);
    }

    // `--terrain` → the ADT heightmap stream (standalone, like --pack-client; see terrain.rs).
    if args.terrain.is_some() {
        return terrain::run(&args);
    }

    // `--dump-collision` → the 240 spike: WMO/M2 collision-geometry dry-run (see collision.rs).
    if args.dump_collision.is_some() {
        return collision::run(&args);
    }

    // `--nav` → the 241 nav-grid rasterizer (see nav.rs).
    if args.nav.is_some() {
        return nav::run(&args);
    }

    // `--vmap` → the #520/#521 exact vmap triangle extract + pack + import (see vmap.rs).
    if args.vmap.is_some() || args.vmap_status {
        return vmap::run(&args);
    }

    // `--vmap-prepare-coverage <generation_id>` → derive path-grid coverage from an already-staged
    // generation's own geometry (see vmap.rs::run_coverage).
    if let Some(generation_id) = args.vmap_prepare_coverage {
        return vmap::run_coverage(&args, generation_id);
    }

    // `--go-models` → DOOR/BUTTON display → M2 bounding-mesh extract + import (see go_model.rs).
    // Reads `--dump` itself (`parse_args` requires both), so it dispatches before the cmangos ETL
    // pass below consumes `args.dump`.
    if args.go_models.is_some() {
        return go_model::run(&args);
    }

    // `--dbc` alone → standalone DBC mode (proof/checks), OR `--dbc --spells` → the Spell.dbc importer,
    // OR `--dbc --talents` → the TalentTab.dbc/Talent.dbc importer. With `--dump`, the DBC dir instead
    // enriches the cmangos templates (display-derived scale) below, so fall through to the cmangos path.
    if args.dump.is_none() {
        let dir = args
            .dbc
            .clone()
            .expect("parse_args guarantees --dump or --dbc");
        if args.talents {
            return talent::run_talents(&dir, &args);
        }
        if args.spells {
            return spell::run_spells(&dir, &args);
        }
        return dbc::run(&dir, &args);
    }

    let dump_path = args
        .dump
        .clone()
        .expect("parse_args guarantees --dump here");
    let dump = read_dump(&dump_path)?;

    // `--print-extents` → derive the operator's OWN spawn bbox for --map and STOP (write nothing).
    // This is how the operator computes the exact --box rectangle for a new zone instead of eyeballing
    // it from memory (work-item 206 — the Westfall widening needed this the ETL didn't have before).
    if args.print_extents {
        return print_extents(&dump, args.map, args.bbox);
    }

    // Optional DBC scale resolution: cmangos creature_template.Scale==0 means "use the client's
    // CreatureDisplayInfo scale". Without it those creatures load at scale_x=0 (invisible). With
    // --dbc we resolve them; without, they fall back to 1.0 (see the template loop).
    let display_scales: Option<std::collections::HashMap<u32, f32>> = match &args.dbc {
        Some(dir) => {
            let m = dbc::load_display_scales(dir)?;
            eprintln!(
                "dbc: loaded {} display scales for Scale==0 resolution",
                m.len()
            );
            Some(m)
        }
        None => None,
    };

    // Optional profession tier-cap resolution (work-item 208): SkillRaceClassInfo.dbc + SkillTiers.dbc
    // give the REAL Apprentice→Artisan caps per profession skill line — checked immediately against
    // `LEGACY_TIER_CAPS` (`assert_profession_tier_parity`, bailing loudly on any disagreement) before the
    // trainer-synthesis pass below ever runs. Without --dbc there's no client data to check against, so
    // the synthesis loop falls back to the legacy literal caps directly (documented fallback, no parity
    // guard possible — see LEGACY_TIER_CAPS).
    let profession_tier_values: Option<std::collections::HashMap<u32, [u16; 4]>> = match &args.dbc {
        Some(dir) => {
            let m = dbc::profession_tier_values(dir)?;
            eprintln!(
                "dbc: loaded profession tier caps for {} skill lines",
                m.len()
            );
            assert_profession_tier_parity(&m)?;
            Some(m)
        }
        None => None,
    };

    let plan = build_dump_plan(&dump, &args, &display_scales, &profession_tier_values)?;

    if !args.apply {
        println!(
            "-- DRY RUN: {} SQL statements, then {} creature-spawn batch(es) for {} spawns + {} gameobject batch(es) (of {SPAWN_BATCH}).\n",
            plan.stmts.len(), plan.spawn_batches.len(), plan.spawn_row_count, plan.go_batches.len()
        );
        for s in &plan.stmts {
            println!("{s};\n");
        }
        println!(
            "-- load spawns via reducer: batch 0 = import_creature_spawns (clears+loads), the rest = import_creature_spawns_append (load only).\n  e.g. spacetime call -s {} {} import_creature_spawns '<batch>'\n  (sample row: {})",
            args.server, args.db, plan.spawn_batches.first().and_then(|b| b.split(';').next()).unwrap_or("")
        );
        return Ok(());
    }

    // The spawn-batch emptiness check only matters when creatures are actually being (re)loaded this
    // run — `--family quests` (etc.) on a box with zero matched spawns is a legitimate (if unusual) run.
    if family_active(&args, "creatures") && plan.spawn_batches.is_empty() {
        bail!("no spawns matched the filter — check --box / --radius / --center");
    }

    // Apply: SQL first (templates + waypoints ready), then the spawn batches (the first resets live
    // state + loads; the next tick_creatures builds the entities from the just-loaded templates).
    run_sql_statements(&args, &plan.stmts, "creatures")?;
    if family_active(&args, "creatures") {
        eprintln!(
            "reducer: loading {} spawns in {} batch(es)…",
            plan.spawn_row_count,
            plan.spawn_batches.len()
        );
        for (i, batch) in plan.spawn_batches.iter().enumerate() {
            let reducer = if i == 0 {
                "import_creature_spawns"
            } else {
                "import_creature_spawns_append"
            };
            call_reducer(&args, reducer, batch).with_context(|| format!("batch {i}"))?;
        }
    }
    // Gameobject (GOOBER) spawns via the import_gameobjects reducer (game_gameobject has a Timestamp).
    // Same batch-0-clears / append-rest pattern as creatures. // note: an empty slice skips the GO clear
    // (the reducer errors on an empty payload) — fine in practice (Elwynn always has GOOBERs).
    if !plan.go_batches.is_empty() && family_active(&args, "gameobjects") {
        eprintln!(
            "reducer: loading {} gameobjects in {} batch(es)…",
            plan.go_row_count,
            plan.go_batches.len()
        );
        for (i, batch) in plan.go_batches.iter().enumerate() {
            let reducer = if i == 0 {
                "import_gameobjects"
            } else {
                "import_gameobjects_append"
            };
            call_reducer(&args, reducer, batch).with_context(|| format!("batch {i}"))?;
        }
    }
    eprintln!("import applied.");

    // Provenance stamps (work-item 216): `plan.stamps` already carries exactly the families whose
    // block was pushed (see `build_dump_plan`) — stamp each one now that the apply above succeeded.
    let file_hash = sha256_hex(dump.as_bytes())?;
    for (family, row_count) in &plan.stamps {
        stamp_family(&args, family, &args.source_sha, &file_hash, *row_count)
            .with_context(|| format!("stamp_import_meta({family})"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_names_with_commas_and_escapes() {
        let dump = "x INSERT INTO `creature_template` VALUES (1,'A, the Bold','Sub\\'s',5),(2,'B',NULL,7); y";
        let rows = parse_table(dump, "creature_template");
        assert_eq!(rows.len(), 2);
        assert_eq!(field(&rows[0], 0), "1");
        assert_eq!(field(&rows[0], 1), "A, the Bold"); // comma inside the quoted name kept
        assert_eq!(field(&rows[0], 2), "Sub's"); // \' unescaped
        assert_eq!(field(&rows[0], 3), "5");
        assert_eq!(field(&rows[1], 2), "NULL");
    }

    #[test]
    fn world_guid_is_highguid_unit_encoded_and_unique_per_spawn() {
        let g = world_guid(299, 42);
        assert_eq!(g >> 48, HIGHGUID_UNIT); // high bits mark a creature for the client
        assert_eq!((g >> 24) & 0xFF_FFFF, 299); // entry in the middle
        assert_eq!(g & 0xFF_FFFF, 42); // spawn db-guid in the low bits
        assert_ne!(world_guid(299, 42), world_guid(299, 43));
    }

    #[test]
    fn sql_str_escapes_apostrophes() {
        assert_eq!(sql_str("Hogger's"), "'Hogger''s'");
    }

    /// Byte-parity lock (work-item 214): the Northshire tutorial-wolf faction remap must keep behaving
    /// exactly as it did inline before being pulled out into `wolf_faction_fixup`.
    #[test]
    fn wolf_faction_fixup_only_remaps_passive_faction_32_wolves() {
        assert_eq!(wolf_faction_fixup(32, 1), 58); // passive beast faction + Wolf family → remapped
        assert_eq!(wolf_faction_fixup(32, 0), 32); // faction 32 but NOT a wolf → unchanged
        assert_eq!(wolf_faction_fixup(14, 1), 14); // a wolf, but not on faction 32 → unchanged
    }

    #[test]
    fn gather_nodes_map_is_well_formed() {
        // Lookup resolves the Copper Vein (mining) + Peacebloom (herb) anchors and rejects a non-node.
        assert_eq!(gather_node(1731), Some((186, 2770, 1, 100))); // Copper Vein → Mining, Copper Ore, req 1, gray 100
        assert_eq!(gather_node(1618), Some((182, 2447, 1, 75))); // Peacebloom → Herbalism, Peacebloom, req 1, gray 75
        assert_eq!(gather_node(1732), Some((186, 2771, 65, 140))); // Tin Vein → Mining, Tin Ore, req 65, gray 140
        assert_eq!(gather_node(106318), None); // Battered Chest (a treasure chest) is NOT a gather node
        assert_eq!(gather_node(153453), None); // Solid Chest is NOT a gather node
        assert_eq!(gather_node(0), None);
        // No duplicate GO entries (a dup would emit two templates for one entry → INSERT pk clash).
        let mut entries: Vec<u64> = GATHER_NODES.iter().map(|n| n.0).collect();
        entries.sort_unstable();
        let dups = entries.windows(2).filter(|w| w[0] == w[1]).count();
        assert_eq!(dups, 0, "GATHER_NODES has a duplicate GO entry");
        // Every node uses a real gather skill (MINING 186 / HERBALISM 182) and grants a real item entry.
        for &(entry, skill, item, _req, _gray) in GATHER_NODES {
            assert!(
                skill == 186 || skill == 182,
                "entry {entry} has skill {skill} (not Mining/Herb)"
            );
            assert!(item > 0, "entry {entry} grants item 0");
        }
    }

    // ---- work-item 211: widened GO import — classifier, per-type row builder, TYPE-25 guard ----

    #[test]
    fn classify_go_type_remaps_gather_and_passes_everything_else_through_identity() {
        // A GATHER_NODES entry (real cmangos type 3, a "locked CHEST" in cmangos' own model) remaps to
        // the synthetic 25 marker — this is the ONLY entry point that ever produces stored type 25.
        assert_eq!(classify_go_type(1731, 3), Some(GO_GATHER)); // Copper Vein
                                                                // A type-3 entry NOT in GATHER_NODES is a real CHEST now (work-item 211 widened import — used
                                                                // to require the TREASURE_CHESTS allowlist; ANY chest imports today).
        assert_eq!(classify_go_type(999_999, 3), Some(GO_CHEST));
        assert_eq!(classify_go_type(106318, 3), Some(GO_CHEST)); // Battered Chest (formerly allowlisted)
                                                                 // DOOR/BUTTON/QUESTGIVER/GOOBER pass through identity (this repo's ids match cmangos' here).
        assert_eq!(classify_go_type(1, 0), Some(GO_DOOR));
        assert_eq!(classify_go_type(2, 1), Some(GO_BUTTON));
        assert_eq!(classify_go_type(3, 2), Some(GO_QUESTGIVER));
        assert_eq!(classify_go_type(4, 10), Some(GO_GOOBER));
        // An unhandled-but-in-scope type (e.g. 8 SPELL_FOCUS, 6 TRAP, 19 MAILBOX) imports INERT —
        // identity passthrough, not a drop.
        assert_eq!(classify_go_type(5, 8), Some(8));
        assert_eq!(classify_go_type(6, 19), Some(19));
    }

    #[test]
    fn classify_go_type_25_collision_guard_drops_a_real_fishinghole_but_not_a_gather_node() {
        // THE load-bearing case: cmangos' REAL type 25 (GAMEOBJECT_TYPE_FISHINGHOLE) must NEVER be
        // stored as module type 25 (the synthetic GATHER marker) — it is dropped from import entirely.
        assert_eq!(
            classify_go_type(70000, 25),
            None,
            "a real FISHINGHOLE (type 25) must be dropped"
        );
        // A GATHER_NODES entry stays classified as GATHER even though its STORED type (25) numerically
        // equals the guarded raw type — because the raw type it actually carries in the dump is 3, not
        // 25 (the guard only fires on the RAW type; the two paths never collide in practice).
        assert_eq!(classify_go_type(1731, 3), Some(GO_GATHER));
        // A non-gather, non-fishinghole entry is unaffected by the guard.
        assert_eq!(classify_go_type(70001, 3), Some(GO_CHEST));
    }

    #[test]
    fn go_template_row_carries_lock_id_per_type_and_widens_chest_loot() {
        // CHEST: dump data0 (lockId) -> template lock_id column; dump data1 (lootId) -> template data1,
        // AND is returned as the "fold into chest_loot_ids_used" value (widened to EVERY chest now).
        let (row, loot_used) =
            go_template_row(106318, GO_CHEST, 259, "Battered Chest", 4242, 2277, 1.0);
        assert_eq!(row, "(106318,3,259,'Battered Chest',0,2277,0,0,0,4242,1)");
        assert_eq!(loot_used, Some(2277));
        // A CHEST with lootId 0 (unimported drop table, or the seed/demo chest) does NOT get folded in.
        let (_, none_used) = go_template_row(1, GO_CHEST, 1, "No Loot Chest", 0, 0, 1.0);
        assert_eq!(none_used, None);
        // DOOR/BUTTON: dump data1 (lockId) -> template lock_id column (data0 [startOpen] does NOT ride
        // the template — it feeds the SPAWN row's initial state instead, see `go_initial_state`).
        let (door_row, door_loot) =
            go_template_row(2000, GO_DOOR, 100, "Stormwind Gate", 1, 555, 1.0);
        assert_eq!(door_row, "(2000,0,100,'Stormwind Gate',0,0,0,0,0,555,1)");
        assert_eq!(door_loot, None);
        let (button_row, _) = go_template_row(2001, GO_BUTTON, 101, "Lever", 0, 777, 1.0);
        assert_eq!(button_row, "(2001,1,101,'Lever',0,0,0,0,0,777,1)");
        // GOOBER: dump data0 (lockId) -> template lock_id column.
        let (goober_row, _) =
            go_template_row(3000, GO_GOOBER, 200, "Suspicious Lever", 888, 999, 1.0);
        assert_eq!(
            goober_row,
            "(3000,10,200,'Suspicious Lever',0,0,0,0,0,888,1)"
        );
        // GATHER: unaffected by lock_id (always 0) — the existing gather-row shape, now with a 10th
        // (lock_id) + 11th (size) column appended. A node's sub-1.0 size rides through verbatim.
        let (gather_row, gather_loot) =
            go_template_row(1731, GO_GATHER, 259, "Copper Vein", 0, 0, 0.5);
        assert_eq!(
            gather_row,
            "(1731,25,259,'Copper Vein',2770,1,186,300,100,0,0.5)"
        );
        assert_eq!(gather_loot, None);
        // QUESTGIVER + an INERT type both fall through to the all-zero catch-all arm (11 cols). An
        // absent dump size arrives here as 0 and stays 0.
        let (qg_row, _) = go_template_row(4000, GO_QUESTGIVER, 50, "Wanted Poster", 1, 2, 0.0);
        assert_eq!(qg_row, "(4000,2,50,'Wanted Poster',0,0,0,0,0,0,0)");
        let (inert_row, _) = go_template_row(5000, 8, 60, "Spell Focus", 3, 4, 1.75);
        assert_eq!(inert_row, "(5000,8,60,'Spell Focus',0,0,0,0,0,0,1.75)");
    }

    #[test]
    fn go_initial_state_opens_only_a_started_open_door_or_button() {
        // DOOR/BUTTON with a nonzero startOpen (dump data0) spawn OPEN (state 1).
        assert_eq!(go_initial_state(GO_DOOR, 1), 1);
        assert_eq!(go_initial_state(GO_BUTTON, 1), 1);
        // DOOR/BUTTON with startOpen == 0 spawn CLOSED (state 0) — the pre-211 default.
        assert_eq!(go_initial_state(GO_DOOR, 0), 0);
        assert_eq!(go_initial_state(GO_BUTTON, 0), 0);
        // Every OTHER type always spawns state 0 regardless of data0 (data0 means something else there
        // — e.g. a CHEST's data0 is its lockId, not a startOpen flag).
        assert_eq!(go_initial_state(GO_CHEST, 1), 0);
        assert_eq!(go_initial_state(GO_GOOBER, 1), 0);
        assert_eq!(go_initial_state(GO_GATHER, 1), 0);
        assert_eq!(go_initial_state(GO_QUESTGIVER, 1), 0);
    }

    #[test]
    fn cr_columns_map_spawn_time_and_movement_type() {
        // work-item 047: creature (spawn) row layout — guid,id,map,spawnMask,x,y,z,o,
        // spawntimesecsmin,spawntimesecsmax,spawndist,MovementType — verify the column CONSTANTS
        // actually land on the cmangos min-respawn-seconds and MovementType fields, not neighbors
        // (a silent off-by-one here would misread every imported creature's respawn timer).
        let dump = "x INSERT INTO `creature` VALUES (1,299,0,1,10.0,20.0,30.0,1.5,180,300,0,2); y";
        let rows = parse_table(dump, "creature");
        assert_eq!(rows.len(), 1);
        assert_eq!(field(&rows[0], cr::GUID), "1");
        assert_eq!(field(&rows[0], cr::ID), "299");
        assert_eq!(field(&rows[0], cr::MAP), "0");
        assert_eq!(field(&rows[0], cr::X), "10.0");
        assert_eq!(field(&rows[0], cr::O), "1.5");
        assert_eq!(field(&rows[0], cr::SPAWN_TIME_SECS_MIN), "180");
        assert_eq!(field(&rows[0], cr::MOVEMENT_TYPE), "2");
        // Absent/garbage spawntimesecsmin parses as the "not imported" sentinel (0), which the module
        // reads as "fall back to the flat legacy respawn timer" (see CreatureSpawn::respawn_secs).
        let dump_missing = "x INSERT INTO `creature` VALUES (1,299,0,1,10.0,20.0,30.0,1.5); y";
        let rows_missing = parse_table(dump_missing, "creature");
        let respawn_secs: u32 = field(&rows_missing[0], cr::SPAWN_TIME_SECS_MIN)
            .parse()
            .unwrap_or(0);
        assert_eq!(respawn_secs, 0);
    }

    #[test]
    fn compute_extents_derives_min_max_xyz_and_count_for_the_target_map_only() {
        // work-item 206: --print-extents derives the operator's real --box from THEIR dump. Three
        // spawns on map 0 (min/max should span them) + one on map 1 (must be excluded).
        let dump = "x INSERT INTO `creature` VALUES \
            (1,100,0,1,-11000.0,1000.0,10.0,0,300,300,0,0),\
            (2,101,0,1,-9000.0,-500.0,50.0,0,300,300,0,0),\
            (3,102,0,1,-10000.0,2000.0,-5.0,0,300,300,0,0),\
            (4,103,1,1,0.0,0.0,0.0,0,300,300,0,0); y";
        let e = compute_extents(dump, 0, None).expect("map 0 has spawns");
        assert_eq!(e.count, 3); // the map-1 row is excluded
        assert_eq!(e.min, (-11000.0, -500.0, -5.0));
        assert_eq!(e.max, (-9000.0, 2000.0, 50.0));
        // A map with no spawns at all reports None (nothing to derive a box from).
        assert!(compute_extents(dump, 7, None).is_none());
        // Issue #24: map 1's extents are derived from the MAP-1 rows alone — a second continent's box
        // comes out of the same one code path, never out of the map-0 numbers.
        let k = compute_extents(dump, 1, None).expect("map 1 has one spawn");
        assert_eq!(
            (k.count, k.min, k.max),
            (1, (0.0, 0.0, 0.0), (0.0, 0.0, 0.0))
        );
    }

    #[test]
    fn compute_extents_narrows_to_a_candidate_box_and_anchors_center_on_a_real_spawn() {
        // Issue #24: a whole continent's extents ARE the continent, so deriving a starting-zone box is
        // iterate-and-narrow — pass a rough rectangle and read back the true in-box extents + count.
        // The anchor is the in-box spawn nearest the rectangle centre: a REAL cmangos ground point, so
        // it is always a valid `--center` for the terrain interpolate / nav walkability self-checks.
        let dump = "x INSERT INTO `creature` VALUES \
            (1,100,1,1,-600.0,-4200.0,38.0,0,300,300,0,0),\
            (2,101,1,1,-500.0,-4100.0,40.0,0,300,300,0,0),\
            (3,102,1,1,-550.0,-4150.0,39.0,0,300,300,0,0),\
            (4,103,1,1,6000.0,500.0,10.0,0,300,300,0,0); y";
        let e = compute_extents(dump, 1, Some((-700.0, -400.0, -4300.0, -4000.0)))
            .expect("in-box spawns");
        assert_eq!(
            e.count, 3,
            "the far-away map-1 spawn is outside the candidate box"
        );
        assert_eq!(e.min, (-600.0, -4200.0, 38.0));
        assert_eq!(e.max, (-500.0, -4100.0, 40.0));
        // Rectangle centre is (-550,-4150) — spawn 3 sits exactly on it, Z carried verbatim.
        assert_eq!(e.anchor, (-550.0, -4150.0, 39.0));
        // A candidate box with nothing in it reports None rather than an empty/infinite rectangle.
        assert!(compute_extents(dump, 1, Some((0.0, 100.0, 0.0, 100.0))).is_none());
    }

    #[test]
    fn start_positions_are_listed_for_the_requested_map_only() {
        // Issue #24: the printed starting-zone anchors come from the dump's OWN playercreateinfo, which
        // is how a Kalimdor box gets drawn around Valley of Trials instead of around a remembered
        // coordinate. Human(1) is map 0; Orc(2)/Tauren(6) are map 1.
        let dump = "x INSERT INTO `playercreateinfo` VALUES \
            (1,1,0,12,-8949.95,-132.493,83.5312,0),\
            (2,1,1,14,-618.518,-4251.67,38.718,0),\
            (6,1,1,215,-2917.58,-257.98,52.9968,0); y";
        assert_eq!(
            start_positions_on_map(dump, 0),
            vec![(1, 1, 12, -8949.95, -132.493, 83.5312)]
        );
        let kalimdor = start_positions_on_map(dump, 1);
        assert_eq!(
            kalimdor.len(),
            2,
            "both Kalimdor races are anchors: {kalimdor:?}"
        );
        assert_eq!(kalimdor[0], (2, 1, 14, -618.518, -4251.67, 38.718));
        assert_eq!(kalimdor[1].2, 215, "Mulgore's zone id rides along");
        assert!(
            start_positions_on_map(dump, 36).is_empty(),
            "an instance map has no start positions"
        );
    }

    #[test]
    fn sql_text_collapses_null_token_to_empty() {
        assert_eq!(sql_text("NULL"), "''"); // cmangos `NULL` subname → empty, not the text "NULL"
        assert_eq!(sql_text("Weaponsmith"), "'Weaponsmith'");
    }

    #[test]
    fn build_graveyard_zone_sql_parses_id_ghost_zone_and_faction() {
        // work-item 209: cmangos game_graveyard_zone columns are (id, ghost_zone, faction) — id is
        // the world_safe_locs/game_graveyard id (safe_loc_id here), ghost_zone the zone it serves,
        // faction 0 = both / else a team-faction id. Northshire+Goldshire+Eastvale serve zone 12
        // (Alliance-only, 469); Sentinel Hill serves zone 40 (also 469).
        let dump =
            "x INSERT INTO `game_graveyard_zone` VALUES (105,12,469),(106,12,469),(80,40,469); y";
        let stmts = build_graveyard_zone_sql(dump);
        assert_eq!(
            stmts[0],
            "DELETE FROM game_graveyard_zone WHERE row_id >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_graveyard_zone "))
            .unwrap();
        // row_id,safe_loc_id,zone_id,faction — row_id assigned sequentially (1-based), not the
        // cmangos id (which collides across factions/zones for the SAME graveyard in a real dump).
        assert!(insert.contains("(1,105,12,469)"), "{insert}");
        assert!(insert.contains("(2,106,12,469)"), "{insert}");
        assert!(insert.contains("(3,80,40,469)"), "{insert}");
    }

    #[test]
    fn build_areatrigger_teleport_sql_maps_columns_by_tuple_position() {
        // work-item 225: cmangos areatrigger_teleport columns are (id, name, target_map,
        // target_position_x, target_position_y, target_position_z, target_orientation) — see the `at`
        // module's `[V]` doc comment. Two rows with DISTINCT values in every column (no accidental
        // digit reuse across fields) so a column-index typo/transposition would fail this test, not
        // pass it vacuously; one row's name carries an apostrophe to prove `sql_text` escaping rides
        // through this builder too.
        // The name's apostrophe is backslash-escaped (`\'`), the MySQL/mysqldump convention
        // `parse_table` actually implements (see its own doc comment) — NOT the doubled-quote (`''`)
        // ANSI-SQL convention `sql_text`'s OUTPUT side re-escapes it into for `spacetime sql`.
        // The REAL dump-verified 13-col shape (at:: 2026-07-10): id, name, required_level,
        // req_item, req_quest, req_?, target_map, x, y, z, o, message, ? — the old 7-col fixture
        // pre-dated the offset fix and parsed 0 rows (fixture drift, found 2026-07-12).
        let dump = "x INSERT INTO `areatrigger_teleport` VALUES \
            (1447,'Deadmines',10,0,0,0,36,-114.8,-9.5,-9.6,3.14,'',0),\
            (1448,'Moonbrook\\'s Exit',0,0,0,0,0,-11235.7,1668.3,23.9,1.57,'',0); y";
        let (stmts, n) = build_areatrigger_teleport_sql(dump);
        assert_eq!(n, 2);
        assert_eq!(
            stmts[0],
            "DELETE FROM game_areatrigger_teleport WHERE trigger_id >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_areatrigger_teleport "))
            .unwrap();
        // trigger_id,target_map,x,y,z,o,name — NOT the dump's own (id,name,target_map,x,y,z,o) order;
        // `name` moves to the END of our column list (module table field order), everything else keeps
        // the dump's tuple position. A transposed x/y/z or a name landing in the wrong slot fails here.
        assert!(
            insert.contains("(1447,36,-114.8,-9.5,-9.6,3.14,'Deadmines')"),
            "{insert}"
        );
        assert!(
            insert.contains("(1448,0,-11235.7,1668.3,23.9,1.57,'Moonbrook''s Exit')"),
            "{insert}"
        );
    }

    #[test]
    fn drink_discriminator_keys_on_beverage_names_within_food_and_drink() {
        // Drinks → restore mana. In classicdb z2815 these are class 0 / subclass 0 (NOT 5), incl. the key
        // mage drink Conjured Water — the subclass-0 path is the common case.
        assert!(is_drink_consumable(0, 0, "Conjured Water"));
        assert!(is_drink_consumable(0, 0, "Morning Glory Dew")); // "dew"
        assert!(is_drink_consumable(0, 0, "Ice Cold Milk"));
        assert!(is_drink_consumable(0, 5, "Refreshing Spring Water")); // subclass 5 also valid
        assert!(is_drink_consumable(0, 5, "Bottle of Dalaran Red Wine"));
        // Food (same class/subclass as drink) → NOT a drink (stays HP) — discriminated purely by name.
        assert!(!is_drink_consumable(0, 0, "Tender Wolf Steak")); // "tea" is a substring, not a word
        assert!(!is_drink_consumable(0, 0, "Snapvine Watermelon")); // "water" substring, not a word
        assert!(!is_drink_consumable(0, 5, "Beer Basted Boar Ribs")); // "beer" dropped (food collision)
        assert!(!is_drink_consumable(0, 0, "Tough Jerky"));
        // Right name, excluded class/subclass → never a drink.
        assert!(!is_drink_consumable(2, 7, "Watery Blade")); // a weapon named "water…"
        assert!(!is_drink_consumable(0, 1, "Healing Potion")); // potion (subclass 1) excluded
        assert!(!is_drink_consumable(0, 2, "Elixir of Water Walking")); // elixir (subclass 2) excluded
    }

    // ---- LOOT-FAMILY COMPLETENESS (work-item 210) ----

    /// `resolve_ref_pool` flattens a 2-DEEP reference chain (pool 1 → item A direct + a `-ref` to pool 2
    /// → item B) into ONE flat list, and a CYCLE (pool 3 → pool 4 → pool 3) terminates instead of
    /// looping forever — the cycle guard returns an empty tail for the pool it re-enters, so pool 3's
    /// own direct item still comes through.
    #[test]
    fn resolve_ref_pool_flattens_two_deep_and_terminates_a_cycle() {
        use std::collections::{HashMap, HashSet};
        let mut refs: HashMap<u64, Vec<(u64, f64, i64, u32)>> = HashMap::new();
        // Pool 1: item 100 direct (30%) + a reference to pool 2 (mincountOrRef = -2).
        refs.insert(1, vec![(100, 30.0, 0, 1), (0, 0.0, -2, 1)]);
        // Pool 2 (nested, 2 deep from pool 1): item 200 direct.
        refs.insert(2, vec![(200, 100.0, 0, 3)]);
        let mut visited = HashSet::new();
        let flat = resolve_ref_pool(1, &refs, &mut visited);
        assert_eq!(
            flat.len(),
            2,
            "pool 1's own item + pool 2's nested item, flattened to one list"
        );
        assert!(flat
            .iter()
            .any(|&(item, chance, _)| item == 100 && chance == 30.0));
        assert!(flat
            .iter()
            .any(|&(item, chance, count)| item == 200 && chance == 100.0 && count == 3));

        // Cycle: pool 3 → pool 4 → pool 3. Pool 3's OWN direct item (300) still resolves; the cyclic
        // re-entry into pool 3 (via pool 4) returns empty instead of recursing forever.
        let mut cyc: HashMap<u64, Vec<(u64, f64, i64, u32)>> = HashMap::new();
        cyc.insert(3, vec![(300, 50.0, 0, 1), (0, 0.0, -4, 1)]);
        cyc.insert(4, vec![(0, 0.0, -3, 1)]); // pool 4 references pool 3 right back
        let mut visited2 = HashSet::new();
        let flat2 = resolve_ref_pool(3, &cyc, &mut visited2);
        assert_eq!(
            flat2,
            vec![(300, 50.0, 1)],
            "the cycle terminates; only pool 3's own direct item survives"
        );

        // An unresolvable pool id (no rows at all) is an empty flatten, never a panic.
        let mut visited3 = HashSet::new();
        assert!(resolve_ref_pool(999, &refs, &mut visited3).is_empty());
    }

    /// `parse_loot_family` sign handling: a POSITIVE `ChanceOrQuestChance` is an ordinary drop
    /// (`quest_only == false`); a NEGATIVE one is cmangos's quest-drop marker — `quest_only == true`,
    /// with `chance_bp` taking the ABSOLUTE value (so a -25.5% quest row still emits a real 2550 bp,
    /// not a negative/garbage chance).
    #[test]
    fn parse_loot_family_reads_the_quest_only_sign_and_abs_value() {
        use std::collections::HashMap;
        // entry, item, ChanceOrQuestChance, groupid, mincountOrRef, maxcount
        let dump = "x INSERT INTO `pickpocketing_loot_template` VALUES \
            (500,1,50.0,0,0,1),\
            (500,2,-25.5,0,0,1); y";
        let refs: HashMap<u64, Vec<(u64, f64, i64, u32)>> = HashMap::new();
        let drops = parse_loot_family(dump, "pickpocketing_loot_template", &refs);
        let rows = drops.get(&500).expect("entry 500 has rows");
        assert_eq!(rows.len(), 2);
        let ordinary = rows.iter().find(|&&(item, ..)| item == 1).unwrap();
        assert_eq!(
            *ordinary,
            (1, 5000, 1, 0, false),
            "positive chance → an ordinary (non-quest) row"
        );
        let quest_row = rows.iter().find(|&&(item, ..)| item == 2).unwrap();
        assert_eq!(
            *quest_row,
            (2, 2550, 1, 0, true),
            "negative chance → quest_only, chance_bp is the ABS value"
        );
    }

    /// Work-item 213: the dropped item_template columns (resistances, extra spell slots, skill/rep
    /// gates, maxcount/flags/page_text/startquest/bag_family) parse from their VERIFIED cmangos
    /// column indices into the right positions of the emitted `game_item_template` row, appended
    /// after `bonding` in struct order. Builds a synthetic 121-column row (indices 0..=120, reaching
    /// BagFamily) via the `it` module's own constants, so this test breaks if an index constant ever
    /// drifts from the row layout it asserts against.
    #[test]
    fn item_template_parses_the_work_item_213_columns_into_the_right_positions() {
        use std::collections::{HashMap, HashSet};

        let mut cols = vec!["0".to_string(); 124];
        cols[it::ENTRY] = "90001".into();
        cols[it::CLASS] = "4".into(); // Armor — never a drink, regardless of subclass
        cols[it::SUBCLASS] = "1".into();
        cols[it::NAME] = "__NAME__".into(); // swapped for a quoted literal below
        cols[it::DISPLAY_ID] = "1234".into();
        cols[it::QUALITY] = "3".into();
        cols[it::FLAGS] = "512".into();
        cols[it::BUY_PRICE] = "1000".into();
        cols[it::SELL_PRICE] = "200".into();
        cols[it::INVENTORY_TYPE] = "5".into();
        cols[it::ALLOWED_CLASS] = "-1".into();
        cols[it::ALLOWED_RACE] = "-1".into();
        cols[it::ITEM_LEVEL] = "40".into();
        cols[it::REQUIRED_LEVEL] = "30".into();
        cols[it::REQUIRED_SKILL] = "165".into();
        cols[it::REQUIRED_SKILL_RANK] = "150".into();
        cols[it::REQUIRED_REPUTATION_FACTION] = "69".into();
        cols[it::REQUIRED_REPUTATION_RANK] = "3".into();
        cols[it::MAX_COUNT] = "7".into();
        cols[it::STACKABLE] = "1".into();
        cols[it::ARMOR] = "55".into();
        cols[it::HOLY_RES] = "3".into();
        cols[it::FIRE_RES] = "12".into();
        cols[it::SPELLID_3] = "12345".into();
        cols[it::SPELLTRIGGER_3] = "1".into();
        cols[it::SPELLID_4] = "12346".into();
        cols[it::SPELLTRIGGER_4] = "2".into();
        cols[it::SPELLID_5] = "12347".into();
        cols[it::SPELLTRIGGER_5] = "0".into();
        cols[it::BONDING] = "2".into();
        cols[it::PAGE_TEXT] = "999".into();
        cols[it::START_QUEST] = "777".into();
        cols[it::MAX_DURABILITY] = "80".into();
        cols[it::BAG_FAMILY] = "6".into();
        cols[it::FOOD_TYPE] = "1".into();

        let tuple = cols
            .join(",")
            .replacen("__NAME__", "'Ring of Fire Resistance'", 1);
        let dump = format!("x INSERT INTO `item_template` VALUES ({tuple}); y");

        let (item_rows, _loot, _vendor) =
            build_items_and_loot(&dump, &HashMap::new(), &HashSet::new(), &HashSet::new());
        assert_eq!(item_rows.len(), 1);
        assert_eq!(
            item_rows[0],
            "(90001,4,1,'Ring of Fire Resistance',1234,3,5,40,30,80,1000,200,1,0,0,0,0,0,0,0,0,0,0,55,0,false,0,0,0,0,0,0,2,3,12,0,0,0,0,12345,1,12346,2,12347,0,165,150,69,3,7,512,999,777,6,1,1,511,255)",
        );
    }

    /// A template with no resistance/spell-3-5/skill/rep/maxcount/flags/page_text/startquest/
    /// bag_family data at all keeps every work-item 213 column at 0 — the additive-migration
    /// baseline (byte-identical to the pre-213 shape for an ordinary existing item like the starter
    /// Worn Shortsword). Built the same index-constant-driven way as the row above, so a real
    /// no-frills weapon row round-trips with every new column zeroed.
    #[test]
    fn item_template_with_no_new_data_keeps_the_work_item_213_columns_zero() {
        use std::collections::{HashMap, HashSet};

        let mut cols = vec!["0".to_string(); 124];
        cols[it::ENTRY] = "25".into();
        cols[it::CLASS] = "2".into(); // Weapon
        cols[it::SUBCLASS] = "7".into(); // Sword (one-hand)
        cols[it::NAME] = "__NAME__".into();
        cols[it::DISPLAY_ID] = "1521".into();
        cols[it::QUALITY] = "0".into();
        cols[it::INVENTORY_TYPE] = "21".into();
        cols[it::ITEM_LEVEL] = "1".into();
        cols[it::REQUIRED_LEVEL] = "1".into();
        cols[it::MAX_DURABILITY] = "35".into();
        cols[it::BUY_PRICE] = "100".into();
        cols[it::SELL_PRICE] = "20".into();
        cols[it::STACKABLE] = "1".into();
        cols[it::DMG_MIN1] = "1".into();
        cols[it::DMG_MAX1] = "3".into();
        cols[it::DELAY] = "1800".into();

        let tuple = cols.join(",").replacen("__NAME__", "'Worn Shortsword'", 1);
        let dump = format!("x INSERT INTO `item_template` VALUES ({tuple}); y");

        let (item_rows, _loot, _vendor) =
            build_items_and_loot(&dump, &HashMap::new(), &HashSet::new(), &HashSet::new());
        assert_eq!(item_rows.len(), 1);
        assert_eq!(
            item_rows[0],
            "(25,2,7,'Worn Shortsword',1521,0,21,1,1,35,100,20,1,1,3,1800,0,0,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0)",
        );
    }

    #[test]
    fn item_template_preserves_restrictive_eligibility_mask_bits() {
        use std::collections::{HashMap, HashSet};

        let mut cols = vec!["0".to_string(); 124];
        cols[it::ENTRY] = "90003".into();
        cols[it::NAME] = "__NAME__".into();
        cols[it::ALLOWED_CLASS] = "2147483650".into();
        cols[it::ALLOWED_RACE] = "2147483652".into();

        let tuple = cols.join(",").replacen("__NAME__", "'Opaque Mask Item'", 1);
        let dump = format!("x INSERT INTO `item_template` VALUES ({tuple}); y");
        let (item_rows, _loot, _vendor) =
            build_items_and_loot(&dump, &HashMap::new(), &HashSet::new(), &HashSet::new());

        assert_eq!(item_rows.len(), 1);
        assert!(
            item_rows[0].ends_with(",2147483650,2147483652)"),
            "restrictive masks, including unknown high bits, must remain unsigned and verbatim"
        );
    }

    // --- work-item 212: playercreateinfo_* fixture tests ------------------------------------------

    #[test]
    fn build_createinfo_spell_sql_parses_race_class_spell_and_wipes_wholesale() {
        // Human(1) Warrior(1) wildcard-style rows (race==0 class kit, class==0 racial) pass through
        // verbatim — the importer does no wildcard interpretation itself (module reads it at grant
        // time via createinfo_row_matches). A concrete Dwarf(3) Warrior(1) row also imports.
        let dump = "x INSERT INTO `playercreateinfo_spell` VALUES (0,1,78),(1,0,20598),(3,1,78); y";
        let stmts = build_createinfo_spell_sql(dump);
        assert_eq!(stmts[0], "DELETE FROM game_createinfo_spell WHERE id >= 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_createinfo_spell "))
            .unwrap();
        assert!(insert.contains("(1,0,1,78)"), "{insert}");
        assert!(insert.contains("(2,1,0,20598)"), "{insert}");
        assert!(insert.contains("(3,3,1,78)"), "{insert}");
    }

    #[test]
    fn build_createinfo_action_sql_parses_race_class_button_action_type() {
        let dump = "x INSERT INTO `playercreateinfo_action` VALUES (1,1,0,6603,0),(1,1,1,78,0); y";
        let stmts = build_createinfo_action_sql(dump);
        assert_eq!(
            stmts[0],
            "DELETE FROM game_createinfo_action WHERE row_id >= 0"
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_createinfo_action "))
            .unwrap();
        // row_id,race,class,button,action,action_type
        assert!(insert.contains("(1,1,1,0,6603,0)"), "{insert}");
        assert!(insert.contains("(2,1,1,1,78,0)"), "{insert}");
    }

    #[test]
    fn distinct_race_class_combos_dedupes_pairs() {
        assert_eq!(
            distinct_race_class_combos(&[(1, 1), (1, 1), (3, 1), (1, 4)]),
            3
        );
        assert_eq!(distinct_race_class_combos(&[]), 0);
    }

    #[test]
    fn build_createinfo_item_sql_without_dbc_imports_every_row_undeduped() {
        // No --dbc → no outfit set to dedup against, so every distinct (race,class,item) imports.
        let dump = "x INSERT INTO `playercreateinfo_item` VALUES (1,1,6948,1),(1,1,2092,1); y";
        let stmts = build_createinfo_item_sql(dump, None).expect("no dbc dir needed");
        assert_eq!(
            stmts[0],
            format!("DELETE FROM game_start_item WHERE id >= {CREATEINFO_ITEM_ID_FLOOR}")
        );
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_start_item "))
            .unwrap();
        let race_class = (1u32 << 8) | 1u32;
        assert!(
            insert.contains(&format!("({CREATEINFO_ITEM_ID_FLOOR},{race_class},6948)")),
            "{insert}"
        );
        assert!(
            insert.contains(&format!(
                "({},{race_class},2092)",
                CREATEINFO_ITEM_ID_FLOOR + 1
            )),
            "{insert}"
        );
    }

    #[test]
    fn build_createinfo_item_sql_dedupes_within_itself_and_skips_nonpositive_items() {
        // The dump lists the SAME (race,class,item) twice (a malformed/duplicated dump row) — only
        // one row should import. item 0 (an empty slot sentinel, if the dump ever carries one) is
        // skipped, not imported as a bogus item_entry.
        let dump =
            "x INSERT INTO `playercreateinfo_item` VALUES (1,1,6948,1),(1,1,6948,1),(1,1,0,1); y";
        let stmts = build_createinfo_item_sql(dump, None).expect("no dbc dir needed");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_start_item "))
            .unwrap();
        assert_eq!(insert.matches("6948").count(), 1, "{insert}");
        assert!(!insert.contains(",0)"), "item 0 must not import: {insert}");
    }

    // --- work-item 102 (reduced scope): spell_chain / spell_learn_spell fixture parse/shape ---------

    #[test]
    fn build_spell_chain_sql_parses_rank_chain_and_wipes_wholesale() {
        // Fireball rank 2 (spell_id 143): prev=133 (rank 1), first=133, rank=2, no extra req_spell.
        // Rank 1 itself (133): prev=0 (no predecessor), first=133, rank=1.
        let dump = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133,2,0); y";
        let stmts = build_spell_chain_sql(dump);
        assert_eq!(stmts[0], "DELETE FROM game_spell_chain WHERE spell_id > 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_chain "))
            .unwrap();
        // spell_id,prev_spell,first_spell,rank,req_spell
        assert!(insert.contains("(133,0,133,1,0)"), "{insert}");
        assert!(insert.contains("(143,133,133,2,0)"), "{insert}");
    }

    #[test]
    fn build_spell_chain_sql_empty_table_yields_delete_only_no_insert() {
        // No cmangos `spell_chain` rows at all (e.g. an un-imported dump) — the DELETE still runs
        // (wholesale wipe), but push_insert emits no INSERT for zero rows.
        let stmts = build_spell_chain_sql("x y");
        assert_eq!(
            stmts,
            vec!["DELETE FROM game_spell_chain WHERE spell_id > 0".to_string()]
        );
    }

    #[test]
    fn build_spell_learn_sql_parses_parent_and_dependent_ids_densely_from_one() {
        // entry(parent), SpellID(dependent), Active — Active is parsed but NOT used to filter (see the
        // builder's doc): an Active=0 row still imports, identically to an Active=1 row.
        let dump = "x INSERT INTO `spell_learn_spell` VALUES (196,197,1),(196,198,0); y";
        let stmts = build_spell_learn_sql(dump);
        assert_eq!(stmts[0], "DELETE FROM game_spell_learn WHERE id > 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_learn "))
            .unwrap();
        // id,parent_spell,learn_spell — ids assigned densely from 1, dump order preserved.
        assert!(insert.contains("(1,196,197)"), "{insert}");
        assert!(
            insert.contains("(2,196,198)"),
            "Active=0 row must still import: {insert}"
        );
    }

    #[test]
    fn build_spell_learn_sql_empty_table_still_synthesizes_armor_proficiency_wrappers() {
        // No cmangos spell_learn_spell rows at all — the two verified armor-proficiency wrapper→passive
        // pairs still synthesize (cmangos ships neither; see the builder's doc).
        let stmts = build_spell_learn_sql("x y");
        assert_eq!(stmts[0], "DELETE FROM game_spell_learn WHERE id > 0");
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_learn "))
            .unwrap();
        assert!(
            insert.contains("(1,7109,750)"),
            "Plate Mail wrapper→passive: {insert}"
        );
        assert!(
            insert.contains("(2,8738,8737)"),
            "Mail wrapper→passive: {insert}"
        );
    }

    #[test]
    fn build_spell_learn_sql_synthesizes_armor_proficiency_wrappers_exactly_once_alongside_real_rows(
    ) {
        // Real spell_learn_spell rows for unrelated spells coexist with the two synthesized pairs, each
        // appearing exactly once, with ids continuing densely past the real rows.
        let dump = "x INSERT INTO `spell_learn_spell` VALUES (196,197,1); y";
        let stmts = build_spell_learn_sql(dump);
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_learn "))
            .unwrap();
        assert!(insert.contains("(1,196,197)"), "{insert}");
        assert!(insert.contains("(2,7109,750)"), "{insert}");
        assert!(insert.contains("(3,8738,8737)"), "{insert}");
        assert_eq!(insert.matches("7109,750").count(), 1, "{insert}");
        assert_eq!(insert.matches("8738,8737").count(), 1, "{insert}");
    }

    #[test]
    fn build_spell_learn_sql_does_not_double_synthesize_if_dump_already_has_the_wrapper_row() {
        // Defense-in-depth: if a future cmangos dump grows a real spell_learn_spell row for one of the
        // wrapper parents, the synthesis must not add a second, colliding row for that parent.
        let dump = "x INSERT INTO `spell_learn_spell` VALUES (7109,750,1); y";
        let stmts = build_spell_learn_sql(dump);
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_learn "))
            .unwrap();
        assert_eq!(
            insert.matches("7109,750").count(),
            1,
            "must not double-insert the plate wrapper: {insert}"
        );
        // The mail pair still synthesizes — only the colliding parent is skipped.
        assert!(insert.contains("8738,8737"), "{insert}");
    }

    #[test]
    fn family_spellmeta_filter_produces_only_spellmeta_family_stmts() {
        // `--family spellmeta` (work-item 102, reduced scope, mirroring the 216 `--family` gate
        // parity pattern): the plan must carry ONLY the two spellmeta blocks — no other family's
        // statements — even on a dump that also carries an in-box quest fixture.
        let dump = format!(
            "{} INSERT INTO `spell_chain` VALUES (133,0,133,1,0); \
             INSERT INTO `spell_learn_spell` VALUES (196,197,1); y",
            tiny_quest_dump(102, 502)
        );
        let mut args = test_args();
        args.family = Some("spellmeta".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");

        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("DELETE FROM game_spell_chain")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO game_spell_chain") && s.contains("133")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("DELETE FROM game_spell_learn")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO game_spell_learn") && s.contains("196")),
            "{:?}",
            plan.stmts
        );
        let non_spellmeta_markers = [
            "game_creature_template",
            "game_creature_waypoint",
            "game_item_template",
            "game_creature_loot",
            "game_npc_vendor",
            "game_gossip_menu",
            "game_npc_text",
            "game_gossip_option",
            "game_gameobject_template",
            "game_trainer_spell",
            "game_creature_cast",
            "game_creature_spell",
            "game_level_stats",
            "game_start_position",
            "game_graveyard_zone",
            "game_createinfo_spell",
            "game_createinfo_action",
            "game_quest_template",
        ];
        for marker in non_spellmeta_markers {
            assert!(
                !plan.stmts.iter().any(|s| s.contains(marker)),
                "family=spellmeta must not touch {marker}: {:?}",
                plan.stmts
            );
        }
        // Only the spellmeta family gets a provenance stamp (1 chain row + 1 real learn row + the 2
        // synthesized armor-proficiency wrapper→passive rows).
        assert_eq!(plan.stamps, vec![("spellmeta", 4u64)], "{:?}", plan.stamps);
    }

    // --- work-item 217: gossip completeness (menu options, multi-slot npc_text, conditions) --------

    /// Build a `creature_template` INSERT tuple wide enough to reach `ct::GOSSIP_MENU_ID` (col 77):
    /// entry at col 0, gossip_menu_id at col 77, everything else `0`.
    fn creature_template_row(entry: u64, gossip_menu_id: u64) -> String {
        creature_template_row_with_trainer(entry, gossip_menu_id, 0, 0)
    }

    /// `creature_template_row` plus `TrainerType` (71) and `TrainerClass` (73). Both sit below
    /// `ct::GOSSIP_MENU_ID`, so the row is already wide enough. The gap between them is
    /// `TrainerSpell`, which is not ingested.
    fn creature_template_row_with_trainer(
        entry: u64,
        gossip_menu_id: u64,
        trainer_type: u64,
        trainer_class: u64,
    ) -> String {
        let mut cols = vec!["0".to_string(); ct::GOSSIP_MENU_ID + 1];
        cols[ct::ENTRY] = entry.to_string();
        cols[ct::GOSSIP_MENU_ID] = gossip_menu_id.to_string();
        cols[ct::TRAINER_TYPE] = trainer_type.to_string();
        cols[ct::TRAINER_CLASS] = trainer_class.to_string();
        format!("({})", cols.join(","))
    }

    /// A minimal `gossip_menu_option` row in the SHORT (no-condition) 7-column vintage:
    /// `menu_id, id, option_icon, option_text, option_type, npc_option_npcflag, action_menu_id`.
    fn gossip_option_row_short(
        menu_id: u64,
        id: u64,
        icon: u64,
        text: &str,
        option_type: u64,
    ) -> String {
        // The REAL dump-verified 15-col gossip_menu_option shape (gmo:: 2026-07-10): menu_id, id,
        // option_icon, option_text, option_broadcast_text, option_id(TYPE), npc_option_npcflag,
        // action_menu_id, action_poi_id, action_script_id, box_coded, box_money, box_text,
        // box_broadcast_text, condition_id. The old 7-col fixture pre-dated the offset fix and
        // silently parsed 0 options (fixture drift, found 2026-07-12).
        format!("({menu_id},{id},{icon},'{text}',0,{option_type},0,0,0,0,0,0,'',0,0)")
    }

    #[test]
    fn build_gossip_sql_orders_menu_options_by_dense_index_and_maps_actions() {
        // An Elwynn-innkeeper-shaped NPC: 3 options (plain gossip / vendor / innkeeper) inserted in
        // `id` order — assert the importer keeps that order (dense 0..2 option_index) and copies the
        // cmangos action code through verbatim (see `lyracore_shared::constants::gossip_option`).
        let entry = 300u64;
        let menu_id = 900u64;
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {}, {}, {}; y",
            creature_template_row(entry, menu_id),
            gossip_option_row_short(menu_id, 0, 0, "Lets talk.", 1),
            gossip_option_row_short(menu_id, 1, 1, "Id like to browse your goods.", 3),
            gossip_option_row_short(menu_id, 2, 0, "Make this inn your home.", 8),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert_eq!(etl.option_rows.len(), 3, "{:?}", etl.option_rows);
        // row_id,entry,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2
        assert!(
            etl.option_rows[0].contains(&format!("(1,{entry},0,0,'Lets talk.',1,0,0,0,0)")),
            "{}",
            etl.option_rows[0]
        );
        assert!(
            etl.option_rows[1].contains(&format!(
                "(2,{entry},1,1,'Id like to browse your goods.',3,0,0,0,0)"
            )),
            "{}",
            etl.option_rows[1]
        );
        assert!(
            etl.option_rows[2].contains(&format!(
                "(3,{entry},2,0,'Make this inn your home.',8,0,0,0,0)"
            )),
            "{}",
            etl.option_rows[2]
        );
    }

    #[test]
    fn build_gossip_sql_reindexes_dense_option_order_even_when_cmangos_ids_have_gaps() {
        // cmangos option `id`s aren't guaranteed contiguous (a deleted row leaves a gap) — the importer
        // must still emit a DENSE 0-based `option_index` (the gateway's render/select alignment depends
        // on it), sorted by the cmangos id.
        let entry = 301u64;
        let menu_id = 901u64;
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {}, {}; y",
            creature_template_row(entry, menu_id),
            gossip_option_row_short(menu_id, 5, 0, "Second.", 1), // higher cmangos id, inserted first
            gossip_option_row_short(menu_id, 2, 0, "First.", 1),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert_eq!(etl.option_rows.len(), 2);
        // (row_id,entry,option_index,icon,text,...) — option_index 0 must land on the LOWER cmangos id.
        assert!(
            etl.option_rows[0].contains(&format!("{entry},0,0,'First.'")),
            "{}",
            etl.option_rows[0]
        );
        assert!(
            etl.option_rows[1].contains(&format!("{entry},1,0,'Second.'")),
            "{}",
            etl.option_rows[1]
        );
    }

    #[test]
    fn build_gossip_sql_drops_questgiver_placeholder_rows_and_keeps_the_index_dense() {
        // An `OptionType=2` row's label is the literal string "GOSSIP_OPTION_QUESTGIVER" (13 NPCs in
        // the dump, Innkeeper Farley among them), and its quests arrive via the quest section anyway.
        use lyracore_shared::constants::gossip_option;
        let entry = 320u64;
        let menu_id = 920u64;
        let options = [
            gossip_option_row_short(menu_id, 0, 0, "Chat.", gossip_option::GOSSIP as u64),
            gossip_option_row_short(
                menu_id,
                1,
                0,
                "GOSSIP_OPTION_QUESTGIVER",
                gossip_option::QUESTGIVER as u64,
            ),
            gossip_option_row_short(menu_id, 2, 0, "Train me.", gossip_option::TRAINER as u64),
        ]
        .join(",");
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {options}; y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert_eq!(etl.option_rows.len(), 2, "{:?}", etl.option_rows);
        assert!(
            !etl.option_rows
                .iter()
                .any(|r| r.contains("GOSSIP_OPTION_QUESTGIVER")),
            "{:?}",
            etl.option_rows
        );
        // The surviving rows are indexed 0 and 1 — the drop must leave no gap for the menu to render.
        assert!(etl.option_rows[0].contains(&format!("{entry},0,0,'Chat.'")));
        assert!(etl.option_rows[1].contains(&format!("{entry},1,0,'Train me.'")));
    }

    #[test]
    fn build_gossip_sql_takes_the_unconditional_greeting_as_the_menu_title() {
        // Several rows per entry, in no meaningful order: one unconditional greeting plus
        // condition-gated variants. First-row-wins made a conditional greeting the permanent title.
        let entry = 321u64;
        let menu_id = 921u64;
        // npc_text short (17-col) rows: id, prob0..7, BroadcastTextId0..7 — only slot 0 populated.
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu` VALUES ({menu_id},5100,0,44),({menu_id},5101,0,0); \
             INSERT INTO `npc_text` VALUES (5100,1.0,0,0,0,0,0,0,0,8100,0,0,0,0,0,0,0), \
             (5101,1.0,0,0,0,0,0,0,0,8101,0,0,0,0,0,0,0); \
             INSERT INTO `broadcast_text` VALUES (8100,'Come back when the war is over.','NULL'), \
             (8101,'Well met, traveler.','NULL'); y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert_eq!(
            etl.menu_rows,
            vec![format!("({entry},5101)")],
            "the unconditional row (condition_id 0) is the default title"
        );
    }

    #[test]
    fn build_gossip_sql_hides_a_seasonal_event_option_instead_of_failing_open() {
        // mangos type 12 = CONDITION_ACTIVE_GAME_EVENT, the one condition that fails CLOSED.
        let entry = 322u64;
        let menu_id = 922u64;
        let option =
            format!("({menu_id},0,0,'What is Children'' Week?',1,0,0,0,0,0,'NULL',12,0,0)");
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {option}; y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert_eq!(etl.option_rows.len(), 1);
        assert!(
            etl.option_rows[0].ends_with(&format!(
                "{},0,0)",
                lyracore_shared::constants::gossip_condition::NEVER
            )),
            "{}",
            etl.option_rows[0]
        );
    }

    #[test]
    fn build_gossip_sql_folds_an_inline_quest_taken_condition() {
        // The INLINE-condition dump vintage (14 cols: the 11-col FK shape + cond_type/value1/value2
        // appended). mangos type 9 = CONDITION_QUESTTAKEN → our QUEST_TAKEN (1), value1 = the quest id.
        let entry = 302u64;
        let menu_id = 902u64;
        // 14 cols: menu_id,id,icon,text,type,npcflag,action_menu_id,poi,box_coded,box_money,box_text,
        // cond_type(11),cond_value1(12),cond_value2(13, unused by the current minimal condition set).
        let option =
            format!("({menu_id},0,0,'Have you started my quest?',1,0,0,0,0,0,'NULL',9,777,0)");
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES {option}; y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert_eq!(etl.option_rows.len(), 1);
        // cond_type=1 (QUEST_TAKEN), cond_value1=777, cond_value2=0 — the trailing 3 fields.
        assert!(
            etl.option_rows[0].ends_with("1,777,0)"),
            "{}",
            etl.option_rows[0]
        );
    }

    #[test]
    fn build_gossip_sql_resolves_condition_via_the_generic_conditions_table_and_fails_open_when_unmapped(
    ) {
        // The FK-vintage dump (11 cols, `condition_id` at col 10) joins a generic `conditions` row.
        // mangos type 8 = CONDITION_QUESTREWARDED → our QUEST_REWARDED (2). A SECOND option references a
        // condition type this importer doesn't map (5) — it must fail OPEN (cond_type 0, always shown),
        // never silently hide the option.
        let entry = 303u64;
        let menu_id = 903u64;
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu_option` VALUES \
                ({menu_id},0,0,'Turn in reward.',1,0,0,0,0,'NULL',501), \
                ({menu_id},1,0,'Unsupported gate.',1,0,0,0,0,'NULL',502); \
             INSERT INTO `conditions` VALUES (501,8,555,0), (502,5,1,0); y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert_eq!(etl.option_rows.len(), 2);
        assert!(
            etl.option_rows[0].ends_with("2,555,0)"),
            "{}",
            etl.option_rows[0]
        ); // QUEST_REWARDED
        assert!(
            etl.option_rows[1].ends_with("0,0,0)"),
            "{}",
            etl.option_rows[1]
        ); // fail-open
    }

    #[test]
    fn build_gossip_sql_resolves_broadcast_text_for_both_npc_text_and_option_labels() {
        // The BroadcastText-era dump: npc_text is the SHORT 17-col schema (id + 8 probs + 8
        // BroadcastTextIds) and the option's `option_text` column carries a bare broadcast_text id
        // instead of literal text — both must resolve through `broadcast_text`.
        let entry = 304u64;
        let menu_id = 904u64;
        let text_id = 5000u64;
        // npc_text short row: id, prob0..7, bcast0..7 — only slot 0 populated (bcast id 8001).
        let npc_text_row = format!("({text_id},1.0,0,0,0,0,0,0,0,8001,0,0,0,0,0,0,0)");
        let option = gossip_option_row_short(menu_id, 0, 0, "9002", 1); // option_text = bcast id, quoted as a string
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu` VALUES ({menu_id},{text_id}); \
             INSERT INTO `npc_text` VALUES {npc_text_row}; \
             INSERT INTO `gossip_menu_option` VALUES {option}; \
             INSERT INTO `broadcast_text` VALUES (8001,'Greetings, traveler.','NULL'), (9002,'Care to trade?','NULL'); y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert_eq!(etl.menu_rows, vec![format!("({entry},{text_id})")]);
        assert!(
            etl.npc_text_rows
                .iter()
                .any(|r| r.contains("Greetings, traveler.")),
            "{:?}",
            etl.npc_text_rows
        );
        assert_eq!(etl.option_rows.len(), 1);
        assert!(
            etl.option_rows[0].contains("'Care to trade?'"),
            "{}",
            etl.option_rows[0]
        );
    }

    #[test]
    fn build_gossip_sql_emits_every_populated_npc_text_slot_and_skips_empty_ones() {
        // Full 81-col npc_text row with slot 0 AND slot 3 populated (a weighted-variant greeting) —
        // all 8 slots must be considered, not just the first, and an all-empty/zero-prob slot must not
        // emit a row at all (the gateway reads "no rows" as the legacy single-slot fallback).
        let entry = 305u64;
        let menu_id = 905u64;
        let text_id = 5001u64;
        let mut cols = vec!["NULL".to_string(); 81];
        cols[0] = text_id.to_string();
        // slot 0: base 1 — male/female/lang/prob
        cols[1] = "'Well met.'".into();
        cols[2] = "'Well met, traveler.'".into();
        cols[3] = "0".into();
        cols[4] = "0.6".into();
        // slot 3: base 31
        cols[31] = "'Watch yourself.'".into();
        cols[32] = "NULL".into();
        cols[33] = "0".into();
        cols[34] = "0.4".into();
        let npc_text_row = format!("({})", cols.join(","));
        let dump = format!(
            "x INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `gossip_menu` VALUES ({menu_id},{text_id}); \
             INSERT INTO `npc_text` VALUES {npc_text_row}; y",
            creature_template_row(entry, menu_id),
        );
        let entries: std::collections::HashSet<u64> = [entry].into_iter().collect();
        let etl = build_gossip_sql(&dump, &entries);
        assert!(
            etl.npc_text_rows[0].contains("Well met."),
            "{:?}",
            etl.npc_text_rows
        ); // first-nonempty
        assert_eq!(
            etl.npc_text_slot_rows.len(),
            2,
            "{:?}",
            etl.npc_text_slot_rows
        );
        // Row shape: (id,text_id,slot_index,text_male,text_female,probability).
        let slot0 = etl
            .npc_text_slot_rows
            .iter()
            .find(|r| r.contains(&format!(",{text_id},0,")))
            .expect("slot 0 row");
        assert!(
            slot0.contains("'Well met.'")
                && slot0.contains("'Well met, traveler.'")
                && slot0.ends_with("0.6)"),
            "{slot0}"
        );
        let slot3 = etl
            .npc_text_slot_rows
            .iter()
            .find(|r| r.contains(&format!(",{text_id},3,")))
            .expect("slot 3 row");
        assert!(
            slot3.contains("'Watch yourself.'") && slot3.ends_with("0.4)"),
            "{slot3}"
        );
    }

    // --- work-item 216: import pipeline tooling (--family, --source-sha, provenance stamps) --------

    #[test]
    fn sha256_hex_matches_known_test_vectors() {
        // Well-known NIST test vectors — pins `sha256_hex`'s `sha256sum` shell-out against a value
        // nobody can silently get wrong.
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"").unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn families_fixed_set_accepts_every_named_family_and_rejects_unknown() {
        // Mirrors `parse_args`'s `--family` validation exactly (`FAMILIES.contains(&v.as_str())`) —
        // pins the fixed set work-item 216 promised: covers --dump families only (terrain/dbc/spells
        // are separate invocations, not in this list).
        for name in [
            "creatures",
            "items",
            "loot",
            "gossip",
            "quests",
            "gameobjects",
            "trainers",
            "casts",
            "creature-ai",
            "globals",
            "spellmeta",
        ] {
            assert!(
                FAMILIES.contains(&name),
                "{name} should be a valid --family value"
            );
        }
        assert!(
            !FAMILIES.contains(&"terrain"),
            "terrain is a separate invocation, not a --family"
        );
        assert!(
            !FAMILIES.contains(&"dbc"),
            "dbc is a separate invocation, not a --family"
        );
        assert!(
            !FAMILIES.contains(&"spells"),
            "spells is a separate invocation, not a --family"
        );
        assert!(
            !FAMILIES.contains(&"bogus"),
            "an unrecognized name must not silently validate"
        );
    }

    #[test]
    fn family_active_is_true_for_every_family_when_family_is_none_and_exclusive_otherwise() {
        let mut args = test_args();
        args.family = None;
        for name in FAMILIES {
            assert!(
                family_active(&args, name),
                "family=None must activate every family ({name})"
            );
        }
        args.family = Some("quests".to_string());
        assert!(family_active(&args, "quests"));
        for name in FAMILIES.iter().filter(|&&n| n != "quests") {
            assert!(
                !family_active(&args, name),
                "family=Some(quests) must exclude {name}"
            );
        }
    }

    #[test]
    fn command_selects_each_canonical_world_profile() {
        for name in WorldImportProfile::NAMES {
            let args = parse_args_from([
                "--dump",
                "/path/need-not-exist.sql",
                "--world-profile",
                name,
            ])
            .unwrap_or_else(|error| panic!("profile {name} should parse: {error:#}"));
            let scope = args
                .world_import_scope()
                .expect("profile resolves to a scope");
            assert_eq!(scope.name(), *name);
        }
    }

    #[test]
    fn command_refuses_profile_and_legacy_spatial_arguments_before_input_reads() {
        let error = parse_args_from([
            "--dump",
            "/definitely/not/read.sql",
            "--world-profile",
            "alliance-eastern",
            "--map",
            "0",
            "--box",
            "-1,1,-1,1",
        ])
        .err()
        .expect("ambiguous command must fail");
        let message = format!("{error:#}");
        assert!(message.contains("alliance-eastern"), "{message}");
        assert!(message.contains("--map"), "{message}");
        assert!(message.contains("--box"), "{message}");
    }

    #[test]
    fn command_refuses_malformed_or_empty_legacy_scopes() {
        let flat = parse_args_from(["--dump", "/path/need-not-exist.sql", "--box", "1,1,-1,1"])
            .err()
            .expect("flat box must fail");
        assert!(format!("{flat:#}").contains("positive width and height"));

        let removed = parse_args_from([
            "--dump",
            "/path/need-not-exist.sql",
            "--box",
            "-1,1,-1,1",
            "--exclude",
            "-2,2,-2,2",
        ])
        .err()
        .expect("fully excluded box must fail");
        assert!(format!("{removed:#}").contains("scope is empty"));
    }

    // ---- Work-item 226: --include-map (the Deadmines map-36 slice) ------------------------------

    #[test]
    fn row_in_slice_keeps_box_geometry_for_the_primary_map_and_whole_map_for_included() {
        let mut args = test_args();
        args.bbox = Some((-9000.0, -8900.0, -200.0, -100.0));
        args.include_maps = vec![36];
        // Primary map: the box still gates, exactly as before --include-map existed.
        assert!(
            row_in_slice(&args, 0, -8949.95, -132.493, 83.5),
            "in-box map-0 row imports"
        );
        assert!(
            !row_in_slice(&args, 0, -10650.0, 1180.0, 34.0),
            "out-of-box map-0 row does not"
        );
        // Included map: WHOLE map, no geometry — Deadmines interior coords are nowhere near the box.
        assert!(
            row_in_slice(&args, 36, -16.4, -383.07, -33.35),
            "map-36 row imports anywhere"
        );
        // A map that is neither primary nor included never imports (Kalimdor stays out).
        assert!(!row_in_slice(&args, 1, -16.4, -383.07, -33.35));
        // The --exclude carve still applies to the PRIMARY map only.
        args.exclude = Some((-8960.0, -8940.0, -140.0, -120.0));
        assert!(
            !row_in_slice(&args, 0, -8949.95, -132.493, 83.5),
            "excluded map-0 row drops"
        );
        assert!(
            row_in_slice(&args, 36, -8949.95, -132.493, 83.5),
            "an included map ignores --exclude"
        );
    }

    #[test]
    fn canonical_eastern_command_keeps_two_disjoint_map_zero_corridors_and_drops_the_gap() {
        let dump = format!(
            "INSERT INTO `creature` VALUES \
             (1,100,0,1,-9000,0,80,0,300,300,0,0),\
             (2,200,0,1,-6200,300,380,0,300,300,0,0),\
             (3,300,0,1,-7500,300,200,0,300,300,0,0),\
             (4,344,0,1,-12000,300,200,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{},{},{}; \
             INSERT INTO `gameobject_template` VALUES \
             (400,0,1,'Human Door',0,0,0,0,0,0,0,0),\
             (500,0,1,'Dwarf Door',0,0,0,0,0,0,0,0),\
             (600,0,1,'Gap Door',0,0,0,0,0,0,0,0); \
             INSERT INTO `gameobject` VALUES \
             (10,400,0,0,-9000,0,80,0,0,0,0,1),\
             (11,500,0,0,-6200,300,380,0,0,0,0,1),\
             (12,600,0,0,-7500,300,200,0,0,0,0,1);",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            creature_template_row(300, 0),
            creature_template_row(344, 0),
        );
        let args = parse_args_from([
            "--dump",
            "synthetic.sql",
            "--world-profile",
            "alliance-eastern",
        ])
        .expect("profile command");
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("profile dump plan");

        let creatures = plan.spawn_batches.join(";");
        assert!(creatures
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("100")));
        assert!(creatures
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("200")));
        assert!(!creatures
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("300")));
        assert!(
            creatures
                .split(';')
                .any(|row| row.split(',').nth(1) == Some("344")),
            "the profile catalogue retains its out-of-slice quest dependency"
        );

        let gameobjects = plan.go_batches.join(";");
        assert!(gameobjects
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("400")));
        assert!(gameobjects
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("500")));
        assert!(!gameobjects
            .split(';')
            .any(|row| row.split(',').nth(1) == Some("600")));
    }

    #[test]
    fn canonical_single_command_keeps_bounded_maps_and_whole_instance_but_fences_forced_rows() {
        let dump = format!(
            "INSERT INTO `creature` VALUES \
             (1,100,0,1,-9000,0,80,0,300,300,0,0),\
             (2,200,1,1,10311,831,1326,0,300,300,0,0),\
             (3,300,36,1,-16,-383,-33,0,300,300,0,0),\
             (4,400,2,1,0,0,0,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{},{},{}; \
             INSERT INTO `gameobject_template` VALUES \
             (500,0,1,'Eastern Door',0,0,0,0,0,0,0,0),\
             (600,0,1,'Kalimdor Door',0,0,0,0,0,0,0,0),\
             (700,0,1,'Instance Door',0,0,0,0,0,0,0,0); \
             INSERT INTO `gameobject` VALUES \
             (10,500,0,0,-9000,0,80,0,0,0,0,1),\
             (11,600,1,0,10311,831,1326,0,0,0,0,1),\
             (12,700,36,0,90000,-90000,-33,0,0,0,0,1);",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            creature_template_row(300, 0),
            creature_template_row(400, 0),
        );
        let args = parse_args_from([
            "--dump",
            "synthetic.sql",
            "--world-profile",
            "alliance-single",
            "--include-creatures",
            "400",
        ])
        .expect("profile command");
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("profile dump plan");

        let creatures = plan.spawn_batches.join(";");
        for entry in ["100", "200", "300"] {
            assert!(
                creatures
                    .split(';')
                    .any(|row| row.split(',').nth(1) == Some(entry)),
                "entry {entry} retained: {creatures}"
            );
        }
        assert!(
            !creatures
                .split(';')
                .any(|row| row.split(',').nth(1) == Some("400")),
            "forced creature on an unplanned map is fenced: {creatures}"
        );

        let gameobjects = plan.go_batches.join(";");
        for entry in ["500", "600", "700"] {
            assert!(
                gameobjects
                    .split(';')
                    .any(|row| row.split(',').nth(1) == Some(entry)),
                "gameobject {entry} retained: {gameobjects}"
            );
        }
    }

    #[test]
    fn force_include_creatures_keeps_an_out_of_box_row_by_entry() {
        let mut args = test_args();
        args.bbox = Some((-9000.0, -8900.0, -200.0, -100.0));
        // An out-of-box Westfall giver (Magistrate Solomon 344 at ~-9219,-2222) is dropped normally…
        assert!(!creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 344));
        // …but kept once its entry is force-included, WITHOUT changing in-box geometry.
        args.include_creatures = vec![344, 266];
        assert!(
            creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 344),
            "force-listed entry bypasses box"
        );
        assert!(
            creature_row_kept(&args, 0, -8950.0, -150.0, 83.5, 999),
            "in-box row still kept"
        );
        assert!(
            !creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 999),
            "a non-listed out-of-box entry still drops"
        );
        // A pool slot (raw_entry 0) never matches a force-listed giver — falls through to geometry.
        assert!(!creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 0));
    }

    #[test]
    fn the_map_fence_drops_a_force_included_entry_that_spawns_on_a_foreign_map() {
        // Issue #24 (Phase B): --include-creatures matches on ENTRY, so before the map fence a
        // force-listed giver's spawn on ANOTHER map rode into the slice — i.e. a Kalimdor shard's ETL
        // could import an Elwynn spawn (or vice versa) purely because an entry was force-listed. No
        // flag combination may cross the map boundary; the fence is the gate, not script hygiene.
        let mut args = test_args();
        args.map = 1; // a Kalimdor world shard
        args.bbox = Some((-700.0, -400.0, -4300.0, -4000.0)); // Valley of Trials-ish
        args.include_creatures = vec![344];
        assert!(
            !creature_row_kept(&args, 0, -9219.0, -2222.0, 68.0, 344),
            "a map-0 spawn must NEVER enter a map-1 run, force-listed or not"
        );
        assert!(
            creature_row_kept(&args, 1, -550.0, -4150.0, 39.0, 344),
            "the in-box map-1 row is kept"
        );
        // Symmetric: on the canonical map-0 run a force-listed entry's map-1 spawn is fenced out too,
        // while its in-corridor map-0 spawn still bypasses the box exactly as work-item 206 needs.
        let mut m0 = test_args();
        m0.bbox = Some((-11400.0, -8000.0, -3100.0, 2000.0));
        m0.include_creatures = vec![344];
        assert!(m0.map == 0 && creature_row_kept(&m0, 0, -9219.0, -2222.0, 68.0, 344));
        assert!(!creature_row_kept(&m0, 1, -550.0, -4150.0, 39.0, 344));
        // --include-map still opens a whole extra map to the same force-include path (Deadmines).
        m0.include_maps = vec![36];
        assert!(creature_row_kept(&m0, 36, -16.4, -383.07, -33.35, 644));
        assert!(map_in_run(&m0, 0) && map_in_run(&m0, 36) && !map_in_run(&m0, 1));
    }

    /// The trainer columns land from the indices they are actually written at, with `TrainerSpell`
    /// skipped between them.
    ///
    /// This is the only automated guard on those indices. A wrong one reads a real neighbouring cell
    /// and produces plausible data rather than an error, so the fixture gives type and class
    /// different values and poisons every neighbour.
    #[test]
    fn trainer_type_and_class_import_from_their_own_columns() {
        // Paladin trainer: type 0 (CLASS) / class 2. Type stays at the enum's 0, so the class cell
        // alone carries the signal — the same shape the live gate keys on.
        let mut paladin = vec!["0".to_string(); ct::GOSSIP_MENU_ID + 1];
        paladin[ct::ENTRY] = "925".to_string();
        paladin[ct::TRAINER_TYPE] = "0".to_string();
        paladin[ct::TRAINER_CLASS] = "2".to_string();
        // Poison every neighbour so a slipped index reads a rejected value, not a plausible 0. Note
        // 71+1 and 73-1 are the same cell: the two ingested columns are not adjacent.
        paladin[70] = "66".to_string(); // MovementType — one before TrainerType
        paladin[72] = "7".to_string(); // TrainerSpell — between the two we ingest
        paladin[74] = "9".to_string(); // TrainerRace  — one after TrainerClass
        let dump = format!(
            "x INSERT INTO `creature` VALUES \
             (1,925,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\
             (2,11867,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\
             (3,620,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES ({}),{},{}; y",
            paladin.join(","),
            // Weapon master: TRADESKILLS (2) with NO class — serves everyone.
            creature_template_row_with_trainer(11867, 0, 2, 0),
            // A plain critter: both cells 0, the overwhelming majority of the dump.
            creature_template_row_with_trainer(620, 0, 0, 0),
        );
        let args = test_args();
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let insert = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_creature_template"))
            .unwrap_or_else(|| panic!("template insert present: {:?}", plan.stmts));
        // The two columns are named, and END-appended after skin_loot_id (the migration rule).
        assert!(
            insert.contains("skin_loot_id,trainer_type,trainer_class"),
            "trainer columns are named and end-appended: {insert}"
        );
        // Every emitted tuple, keyed by entry -> its last two cells.
        let values = insert
            .split_once(" VALUES ")
            .expect("the insert names its columns then VALUES")
            .1;
        let trailing: std::collections::HashMap<&str, (&str, &str)> = values
            .split("),(")
            .map(|t| {
                let t = t
                    .trim_start_matches('(')
                    .trim_end_matches(';')
                    .trim_end_matches(')');
                let f: Vec<&str> = t.split(',').collect();
                (f[0], (f[f.len() - 2], f[f.len() - 1]))
            })
            .collect();
        assert_eq!(
            trailing.get("925"),
            Some(&("0", "2")),
            "the Paladin trainer imports type 0 / class 2 — a slipped index would read 66, 7 or 9: {insert}"
        );
        assert_eq!(
            trailing.get("11867"),
            Some(&("2", "0")),
            "the weapon master imports TRADESKILLS with class 0 (serves everyone): {insert}"
        );
        assert_eq!(
            trailing.get("620"),
            Some(&("0", "0")),
            "an ordinary creature imports 0/0 — the fail-open default: {insert}"
        );
    }

    /// The verbatim `npc_trainer` → `game_trainer_spell` path (unchanged by the armor-proficiency
    /// wrapper work) already carries `required_level` and `cost` straight through for an armor-wrapper
    /// offering — this pins that behavior rather than re-implementing it. The gap the wrapper work
    /// closes is `game_spell_learn` (see `build_spell_learn_sql_*` tests), not this path.
    #[test]
    fn npc_trainer_armor_wrapper_row_imports_verbatim_required_level_and_cost() {
        // A Paladin trainer (entry 925, in-box at the test-args default center) offering the Plate
        // Mail wrapper (7109) at req level 40 for 22000 copper — real-world ClassicDB shape.
        let dump = format!(
            "x INSERT INTO `creature` VALUES \
             (1,925,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `npc_trainer` VALUES (925,7109,22000,0,0,40); y",
            creature_template_row(925, 0),
        );
        let args = test_args();
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let insert = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_trainer_spell"))
            .unwrap_or_else(|| panic!("trainer_spell insert present: {:?}", plan.stmts));
        // id,trainer_entry,spell_id,cost,required_level,learn_skill_line,learn_skill_cap
        assert!(
            insert.contains(",925,7109,22000,40,0,75)"),
            "wrapper row keeps req level 40 and verbatim cost: {insert}"
        );
    }

    #[test]
    fn include_map_spawns_import_whole_map_with_their_own_map_and_verbatim_z() {
        // One in-box map-0 creature, one map-36 creature far outside the box with a WMO-interior Z,
        // one map-1 creature (NOT included — must be dropped), plus a map-36 DOOR gameobject.
        // Pins the three work-item 226 invariants: (a) --include-map imports the whole extra map,
        // (b) each packed row carries its OWN map id (not args.map), and (c) the dump Z rides
        // through UNALTERED — there is no ground-z snap anywhere in the import path (design doc §4:
        // map 36 has no terrain; a spawn's cmangos Z IS its floor-correct height forever).
        let dump = format!(
            "x INSERT INTO `creature` VALUES \
             (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\
             (2,644,36,1,-16.4,-383.07,-33.35,1.5,7200,7200,0,0),\
             (3,999,1,1,-16.4,-383.07,-33.35,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{},{}; \
             INSERT INTO `gameobject_template` VALUES (101,0,446,'Deadmines Door',0,0,0,0,0,0,0,0); \
             INSERT INTO `gameobject` VALUES (50,101,36,0,-95.0,-345.0,-30.5,2.0,0,0,0,0); y",
            creature_template_row(100, 0),
            creature_template_row(644, 0),
            creature_template_row(999, 0),
        );
        let mut args = test_args();
        args.include_maps = vec![36];
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let spawn_payload = plan.spawn_batches.join(";");
        let rows: Vec<&str> = spawn_payload.split(';').collect();
        // (a)+(b)+(c) for creatures: the map-36 row is present, on map 36, Z byte-verbatim.
        let dm_row = rows
            .iter()
            .find(|r| r.split(',').nth(1) == Some("644"))
            .unwrap_or_else(|| panic!("map-36 spawn imported: {spawn_payload}"));
        let f: Vec<&str> = dm_row.split(',').collect();
        assert_eq!(
            f[2], "36",
            "the packed row carries the spawn's OWN map: {dm_row}"
        );
        assert_eq!(
            f[5], "-33.35",
            "the dump Z is verbatim — no snap, no rounding: {dm_row}"
        );
        // The primary map-0 row still imports (map field 0), and map 1 is NOT in the slice.
        assert!(
            rows.iter().any(|r| {
                let f: Vec<&str> = r.split(',').collect();
                f.get(1) == Some(&"100") && f.get(2) == Some(&"0")
            }),
            "in-box map-0 spawn still imports: {spawn_payload}"
        );
        assert!(
            !rows.iter().any(|r| r.split(',').nth(1) == Some("999")),
            "a map neither primary nor included must be dropped: {spawn_payload}"
        );
        // The map-36 gameobject rides the same rule, carrying map 36 in its packed row.
        let go_payload = plan.go_batches.join(";");
        let go_row = go_payload
            .split(';')
            .find(|r| r.split(',').nth(1) == Some("101"))
            .unwrap_or_else(|| panic!("map-36 GO imported: {go_payload}"));
        assert_eq!(
            go_row.split(',').nth(2),
            Some("36"),
            "GO packed row carries map 36: {go_row}"
        );
    }

    #[test]
    fn gameobject_spawn_quaternion_rides_through_the_packed_row_verbatim() {
        // Issue #515: the importer used to drop rotation_0..3 entirely (only `orientation` carried).
        // A bench with a real, non-trivial cmangos spawn quaternion must reach the packed
        // guid,id,map,x,y,z,o,initial_state,rot0,rot1,rot2,rot3 row byte-verbatim, in that order.
        let dump =
            "INSERT INTO `gameobject_template` VALUES (446,0,259,'Wooden Bench',0,0,0,0,0,0,0,0); \
             INSERT INTO `gameobject` VALUES \
             (60,446,0,0,-8949.95,-132.493,83.5312,1.57,0.1,0.2,0.70710678,0.70710678);"
                .to_string();
        let args = test_args();
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let go_row = plan
            .go_batches
            .join(";")
            .split(';')
            .find(|r| r.split(',').nth(1) == Some("446"))
            .map(|r| r.to_string())
            .unwrap_or_else(|| panic!("the bench GO must import"));
        let f: Vec<&str> = go_row.split(',').collect();
        assert_eq!(f.len(), 12, "packed row carries all 12 fields: {go_row}");
        assert_eq!(f[6], "1.57", "orientation rides through: {go_row}");
        assert_eq!(f[8], "0.1", "rot0 rides through: {go_row}");
        assert_eq!(f[9], "0.2", "rot1 rides through: {go_row}");
        assert_eq!(f[10], "0.70710678", "rot2 rides through: {go_row}");
        assert_eq!(f[11], "0.70710678", "rot3 rides through: {go_row}");
    }

    #[test]
    fn creature_movement_template_expands_onto_waypoint_spawns_without_direct_rows() {
        // Two waypoint-typed (MovementType=2) spawns of the same entry 644 on map 36, one direct-
        // routed spawn of entry 645: the entry-keyed creature_movement_template path expands onto
        // BOTH 644 spawns (each gets its own copy, keyed by its world guid), a PathId!=0 row is
        // skipped (9-col vintage), and the direct creature_movement rows for 645 win (no template
        // expansion for it, proven by 645 also having template rows that must NOT be emitted).
        let dump = format!(
            "x INSERT INTO `creature` VALUES \
             (10,644,36,1,-16.4,-383.07,-33.35,0,300,300,0,2),\
             (11,644,36,1,-20.0,-390.0,-33.35,0,300,300,0,2),\
             (12,645,36,1,-50.0,-400.0,-33.0,0,300,300,0,2); \
             INSERT INTO `creature_template` VALUES {},{}; \
             INSERT INTO `creature_movement` VALUES (12,1,-51,-401,-33,0,0,0); \
             INSERT INTO `creature_movement_template` VALUES \
             (644,0,2,-18,-385,-33.35,0,0,0),\
             (644,0,1,-17,-384,-33.35,0,0,0),\
             (644,1,1,-99,-99,-99,0,0,0),\
             (645,0,1,-77,-77,-77,0,0,0); y",
            creature_template_row(644, 0),
            creature_template_row(645, 0),
        );
        let mut args = test_args();
        args.include_maps = vec![36];
        args.family = Some("creatures".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let wp_insert = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_creature_waypoint"))
            .unwrap_or_else(|| panic!("waypoint insert present: {:?}", plan.stmts));
        // Both 644 spawns got the 2-point path — 4 template points total + 1 direct 645 point.
        assert_eq!(
            wp_insert.matches("-17,-384,-33.35").count(),
            2,
            "point 1 lands on both spawns: {wp_insert}"
        );
        assert_eq!(
            wp_insert.matches("-18,-385,-33.35").count(),
            2,
            "point 2 lands on both spawns: {wp_insert}"
        );
        // Point order is the dump's Point column (1 before 2), not dump row order (2 listed first).
        let g10 = world_guid(644, 10).to_string();
        let p1 = wp_insert
            .find(&format!("{g10},-17,-384"))
            .expect("spawn 10 point 1");
        let p2 = wp_insert
            .find(&format!("{g10},-18,-385"))
            .expect("spawn 10 point 2");
        assert!(p1 < p2, "route order follows the Point column: {wp_insert}");
        // PathId 1 rows are skipped; direct rows win for 645 (its template row never lands).
        assert!(
            !wp_insert.contains("-99,-99"),
            "PathId!=0 must be skipped: {wp_insert}"
        );
        assert!(
            !wp_insert.contains("-77,-77"),
            "direct creature_movement wins over the template: {wp_insert}"
        );
        assert!(
            wp_insert.contains("-51,-401,-33"),
            "the direct 645 row itself imports: {wp_insert}"
        );
    }

    /// A default `Args` for tests that need the struct but not real CLI input (mirrors `parse_args`'s
    /// defaults where it matters for `build_dump_plan`: center/radius match the Northshire default so
    /// a fixture creature placed exactly at the center lands in scope).
    fn test_args() -> Args {
        Args {
            dump: None,
            dbc: None,
            terrain: None,
            pack_client: None,
            dump_collision: None,
            nav: None,
            vmap: None,
            vmap_status: false,
            vmap_prepare_coverage: None,
            go_models: None,
            print_extents: false,
            spells: false,
            talents: false,
            only: Vec::new(),
            trainers: Vec::new(),
            world_profile: None,
            map: 0,
            include_maps: Vec::new(),
            include_creatures: Vec::new(),
            center: (-8949.95, -132.493, 83.5312),
            radius: 180.0,
            bbox: None,
            exclude: None,
            db: "lyracore".into(),
            server: "local".into(),
            apply: false,
            family: None,
            source_sha: String::new(),
        }
    }

    /// A minimal `quest_template` INSERT tuple wide enough to reach `qt::REW_MONEY_MAX_LEVEL` (col 99):
    /// entry at col 0, title at col 30, everything else `0`. Mirrors `creature_template_row`'s pattern.
    fn quest_template_row(entry: u64, title: &str) -> String {
        let mut cols = vec!["0".to_string(); qt::REW_MONEY_MAX_LEVEL + 1];
        cols[qt::ENTRY] = entry.to_string();
        cols[qt::TITLE] = format!("'{title}'");
        format!("({})", cols.join(","))
    }

    /// Like `quest_template_row` but also stamps `qt::NEXT_QUEST_IN_CHAIN` / `qt::LIMIT_TIME`
    /// (work-item 194) — the chained/timed quest fixture. Also stamps `qt::REW_MONEY_MAX_LEVEL` with a
    /// distinctive non-zero value (777) — distinctive so the emitted `reward_money_max_level` column
    /// can be pinned by VALUE (not just "a comma follows"), and non-zero so a bug that always emits the
    /// `unwrap_or(0)` default would still be caught (issue #58 review: the prior end-anchored assertion
    /// only asserted the trailing comma, never the value it carried).
    fn quest_template_row_chained_timed(
        entry: u64,
        title: &str,
        next_quest: u64,
        limit_time: u32,
    ) -> String {
        let mut cols = vec!["0".to_string(); qt::REW_MONEY_MAX_LEVEL + 1];
        cols[qt::ENTRY] = entry.to_string();
        cols[qt::TITLE] = format!("'{title}'");
        cols[qt::NEXT_QUEST_IN_CHAIN] = next_quest.to_string();
        cols[qt::LIMIT_TIME] = limit_time.to_string();
        cols[qt::REW_MONEY_MAX_LEVEL] = "777".to_string();
        format!("({})", cols.join(","))
    }

    /// A tiny but REAL synthetic dump: one in-box creature (a quest giver) + its template + one
    /// quest + the giver relation linking them — just enough for `build_dump_plan` to run
    /// end-to-end (every family's prerequisite parsing touches this data) without a real cmangos
    /// dump.
    fn tiny_quest_dump(creature_entry: u64, quest_entry: u64) -> String {
        format!(
            "x INSERT INTO `creature` VALUES (1,{creature_entry},0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES ({creature_entry},{quest_entry}); y",
            creature_template_row(creature_entry, 0),
            quest_template_row(quest_entry, "Test Quest"),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn eventai_rule_row(
        id: u64,
        subject: i64,
        event: u32,
        inverse_phase_mask: u32,
        chance: u32,
        flags: u32,
        params: [u32; 6],
        actions: [[i64; 4]; 3],
    ) -> String {
        let mut columns = vec![
            id.to_string(),
            subject.to_string(),
            event.to_string(),
            inverse_phase_mask.to_string(),
            chance.to_string(),
            flags.to_string(),
        ];
        columns.extend(params.into_iter().map(|value| value.to_string()));
        columns.extend(actions.into_iter().flatten().map(|value| value.to_string()));
        format!("({})", columns.join(","))
    }

    fn eventai_broadcast_row(id: u32, male: &str, female: &str) -> String {
        format!("({id},'{male}','{female}',1,0,0,0,0,0,0,5,6,7,8,9,10,0)")
    }

    fn eventai_fixture_dump() -> String {
        let mut rules = vec![
            eventai_rule_row(
                10,
                100,
                0,
                2,
                100,
                0x421,
                [1_000, 2_000, 3_000, 4_000, 0, 0],
                [[1, 900, 901, 902], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                11,
                100,
                2,
                0,
                75,
                0,
                [80, 20, 100, 200, 0, 0],
                [[11, 42, 1, 0xA21], [5, 7, 0, 0], [25, 0, 0, 0]],
            ),
            eventai_rule_row(
                12,
                100,
                14,
                0,
                100,
                0,
                [500, 30, 100, 200, 0, 0],
                [[32, 200, 17, 700], [11, 42, 12, 0], [39, 40, 0, 0]],
            ),
            eventai_rule_row(
                13,
                100,
                11,
                0,
                100,
                0,
                [0; 6],
                [[22, 3, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                14,
                100,
                9,
                0,
                100,
                0,
                [5, 20, 100, 200, 0, 0],
                [[29, 15, -45, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                15,
                100,
                4,
                0,
                100,
                0,
                [0; 6],
                [[54, 900, 1, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                16,
                -2,
                6,
                0,
                100,
                0,
                [0; 6],
                [[1, -1, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                17,
                200,
                11,
                0,
                100,
                0,
                [0; 6],
                [[1, 901, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
        ];
        for (offset, target) in [0, 2, 4, 6, 8, 12, 18].into_iter().enumerate() {
            rules.push(eventai_rule_row(
                20 + offset as u64,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 42, target, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ));
        }
        format!(
            "x INSERT INTO `creature` VALUES \
             (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0),\
             (2,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{}; \
             INSERT INTO `broadcast_text` VALUES {},{},{},{}; \
             INSERT INTO `script_texts` VALUES (-1,'Old\\'s','','','','','','','','',0,1,0,2,0); \
             INSERT INTO `creature_ai_summons` VALUES (700,1,2,3,4,5); \
             INSERT INTO `creature_ai_scripts` VALUES {}; y",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            eventai_broadcast_row(900, "Hogger\\'s", "Hoggerette\\'s"),
            eventai_broadcast_row(901, "One", "One"),
            eventai_broadcast_row(902, "Two", "Two"),
            eventai_broadcast_row(903, "Unused", "Unused"),
            rules.join(","),
        )
    }

    #[test]
    fn eventai_plan_keeps_source_rule_groups_and_native_mappings() {
        let dump = eventai_fixture_dump();
        let mut args = test_args();
        args.family = Some("creature-ai".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("EventAI plan");
        assert_eq!(plan.stamps, vec![("creature-ai", 24)]);
        assert_eq!(plan.stmts.len(), 6, "{:?}", plan.stmts);
        assert_eq!(
            &plan.stmts[..3],
            [
                "DELETE FROM game_creature_ai_event WHERE id > 0",
                "DELETE FROM game_creature_ai_broadcast_text WHERE id > 0",
                "DELETE FROM game_creature_ai_summon WHERE id > 0",
            ],
            "a full EventAI reload replaces both imported rows and reserved fixtures"
        );
        assert!(plan.stmts.iter().all(|statement| {
            statement.contains("game_creature_ai_event")
                || statement.contains("game_creature_ai_broadcast_text")
                || statement.contains("game_creature_ai_summon")
        }));
        assert!(!plan.stmts.iter().any(|statement| {
            statement.contains("game_creature_ai_state")
                || statement.contains("game_creature_ai_rule_state")
                || statement.contains("game_creature_ai_timer")
        }));

        let events = plan
            .stmts
            .iter()
            .find(|statement| statement.starts_with("INSERT INTO game_creature_ai_event"))
            .expect("EventAI rows");
        assert!(
            events.contains("(4611686018427387944,100,1,0,''"),
            "{events}"
        );
        assert!(
            events.contains(",10,0,0,100,4294967293,3,1,1000,2000,3000,4000"),
            "{events}"
        );
        assert!(
            events.contains(",11,0,0,75,4294967295,0,0,20,80,100,200"),
            "{events}"
        );
        assert!(
            events.contains(",15,0,0,100,4294967295,0,0,0,0,0,0,0,0,900,0,0,0,0)"),
            "{events}"
        );
        assert!(
            events.contains(&format!(",16,0,{},100,4294967295", world_guid(100, 2))),
            "{events}"
        );
        assert!(events.contains(",17,0,0,100,4294967295,0,0"), "{events}");
        assert!(events.contains(",15,4294967251,0,0,0)"), "{events}");
        for target in [0, 1, 3, 4, 5, 6, 8, 9, 10] {
            assert!(
                events.contains(&format!(",{target},")),
                "missing native target {target}: {events}"
            );
        }
        let texts = plan
            .stmts
            .iter()
            .find(|statement| statement.starts_with("INSERT INTO game_creature_ai_broadcast_text"))
            .expect("EventAI texts");
        assert!(texts.contains("Hogger''s"), "{texts}");
        assert!(texts.contains("Old''s"), "{texts}");
        assert!(!texts.contains("Unused"), "{texts}");
        assert!(texts.contains(",8,5,9,6,10,7)"), "{texts}");

        let again = build_dump_plan(&dump, &args, &None, &None).expect("repeat EventAI plan");
        assert_eq!(plan.stmts, again.stmts);
    }

    #[test]
    fn eventai_inactive_family_does_not_extend_quest_scope() {
        let dump = format!(
            "x INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {},{}; \
             INSERT INTO `quest_template` VALUES {},{}; \
             INSERT INTO `creature_questrelation` VALUES (100,500),(200,501); \
             INSERT INTO `creature_ai_summons` VALUES (700,1,2,3,4,5); \
             INSERT INTO `creature_ai_scripts` VALUES {}; y",
            creature_template_row(100, 0),
            creature_template_row(200, 0),
            quest_template_row(500, "In scope"),
            quest_template_row(501, "Summon only"),
            eventai_rule_row(
                10,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[32, 200, 0, 700], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
        );
        let mut args = test_args();
        args.family = Some("quests".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("quest-only plan");
        let quests = plan
            .stmts
            .iter()
            .find(|statement| statement.starts_with("INSERT INTO game_quest_template"))
            .expect("quest rows");
        assert!(quests.contains("(500,"), "{quests}");
        assert!(!quests.contains("(501,"), "{quests}");
        assert_eq!(plan.stamps, vec![("quests", 1)]);
    }

    #[test]
    fn eventai_drops_invalid_rules_into_explicit_buckets() {
        let mut dump = eventai_fixture_dump().replacen(
            "(903,'Unused','Unused',1,0,0,0,0,0,0,5,6,7,8,9,10,0)",
            "(903,'Unused','Unused',2,0,0,0,0,0,0,5,6,7,8,9,10,0)",
            1,
        );
        let invalid = [
            eventai_rule_row(40, 100, 99, 0, 100, 0, [0; 6], [[1, 900, 0, 0]; 3]),
            eventai_rule_row(41, 100, 0, 0, 0, 0, [0, 1, 0, 1, 0, 0], [[1, 900, 0, 0]; 3]),
            eventai_rule_row(
                42,
                100,
                0,
                u32::MAX,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[1, 900, 0, 0]; 3],
            ),
            eventai_rule_row(
                43,
                100,
                0,
                0,
                100,
                2,
                [0, 1, 0, 1, 0, 0],
                [[1, 900, 0, 0]; 3],
            ),
            eventai_rule_row(
                44,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[99, 0, 0, 0]; 3],
            ),
            eventai_rule_row(
                45,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[1, 999, 0, 0]; 3],
            ),
            eventai_rule_row(
                46,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 0, 0, 0]; 3],
            ),
            eventai_rule_row(
                47,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 42, 99, 0]; 3],
            ),
            eventai_rule_row(
                48,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[32, 999, 0, 700]; 3],
            ),
            eventai_rule_row(
                49,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[32, 200, 0, 999]; 3],
            ),
            eventai_rule_row(
                50,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[1, 4_294_967_296, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                51,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[54, 900, 0, 77], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                52,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[1, 903, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                53,
                100,
                14,
                0,
                100,
                0,
                [1, 30, 0, 1, 0, 0],
                [[11, 42, 10, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                54,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 42, 1, 0x2], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
            eventai_rule_row(
                55,
                100,
                0,
                0,
                100,
                0,
                [0, 1, 0, 1, 0, 0],
                [[11, 42, 1, 0x4], [0, 0, 0, 0], [0, 0, 0, 0]],
            ),
        ];
        dump = dump.replacen("; y", &format!(",{},(999); y", invalid.join(",")), 1);
        let entries = [100].into_iter().collect();
        let guids = [(1, 100), (2, 100)].into_iter().collect();
        let templates = [100, 200].into_iter().collect();
        let plan = eventai::build(&dump, &entries, &guids, &templates);
        for (reason, value) in [
            ("unsupported_event", 99),
            ("invalid_chance", 0),
            ("empty_phase_mask", u32::MAX as u64),
            ("unsupported_flag", 2),
            ("unsupported_action", 99),
            ("missing_broadcast_text", 999),
            ("invalid_spell", 0),
            ("unsupported_target", 99),
            ("missing_summon_creature", 999),
            ("missing_summon_location", 999),
            ("malformed_rule", 1),
            ("invalid_numeric", 13),
            ("unsupported_text_template", 77),
            ("unsupported_chat_type", 2),
            ("unsupported_target", 10),
            ("unsupported_triggered_cast", 2),
            ("unsupported_force_cast", 4),
        ] {
            assert_eq!(plan.dropped(reason, value), 1, "{reason}/{value}");
        }
        assert_eq!(plan.event_counts(99), (1, 0, 1, 0));
        assert_eq!(plan.action_counts(99), (3, 0, 3, 0));
    }

    #[test]
    fn family_quests_filter_produces_only_quest_family_stmts() {
        // `--family quests` (work-item 216): the plan must carry ONLY the quest tables' DELETE+INSERT
        // — no creature/item/loot/gossip/gameobject/trainer/cast/globals statements — even though every
        // family's prerequisite parsing still ran (the quest giver relation needed the creature entry
        // to resolve, per `build_quests`' doc).
        let dump = tiny_quest_dump(100, 500);
        let mut args = test_args();
        args.family = Some("quests".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");

        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("DELETE FROM game_quest_template")),
            "{:?}",
            plan.stmts
        );
        assert!(
            plan.stmts
                .iter()
                .any(|s| s.starts_with("INSERT INTO game_quest_template") && s.contains("500")),
            "{:?}",
            plan.stmts
        );
        let non_quest_markers = [
            "game_creature_template",
            "game_creature_waypoint",
            "game_item_template",
            "game_creature_loot",
            "game_npc_vendor",
            "game_pickpocket_loot",
            "game_skinning_loot",
            "game_gameobject_loot",
            "game_fishing_loot",
            "game_gossip_menu",
            "game_npc_text",
            "game_gossip_option",
            "game_gameobject_template",
            "game_trainer_spell",
            "game_creature_cast",
            "game_creature_spell",
            "game_level_stats",
            "game_start_position",
            "game_graveyard_zone",
            "game_createinfo_spell",
            "game_createinfo_action",
            "game_spell_chain",
            "game_spell_learn",
        ];
        for marker in non_quest_markers {
            assert!(
                !plan.stmts.iter().any(|s| s.contains(marker)),
                "family=quests must not touch {marker}: {:?}",
                plan.stmts
            );
        }
        // Only the quests family gets a provenance stamp.
        assert_eq!(plan.stamps, vec![("quests", 1u64)], "{:?}", plan.stamps);
    }

    /// Work-item 194: `next_quest_id`/`limit_time` thread through the FULL import (dump row → parsed
    /// column → the emitted `game_quest_template` INSERT tuple), not just `build_quests`' own struct —
    /// extends the quests-family fixture with a chained+timed quest.
    #[test]
    fn chained_and_timed_quest_columns_thread_through_the_full_import() {
        let dump = format!(
            "x INSERT INTO `creature` VALUES (1,100,0,1,-8949.95,-132.493,83.5312,0,300,300,0,0); \
             INSERT INTO `creature_template` VALUES {}; \
             INSERT INTO `quest_template` VALUES {}; \
             INSERT INTO `creature_questrelation` VALUES (100,500); y",
            creature_template_row(100, 0),
            quest_template_row_chained_timed(500, "Chained Timed Quest", 501, 3600),
        );
        let mut args = test_args();
        args.family = Some("quests".to_string());
        let plan = build_dump_plan(&dump, &args, &None, &None).expect("build_dump_plan");
        let insert = plan
            .stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_quest_template") && s.contains("500"))
            .unwrap_or_else(|| panic!("quest 500 row present: {:?}", plan.stmts));
        // Match the triple adjacent-and-in-order rather than anchored at the row's end: `reward_money_max_level`
        // (daf083c, authentic quest XP) END-appended a THIRD column after next_quest_id/limit_time, and an
        // end-anchored check broke on that legitimate append even though both still threaded through
        // correctly. Pin the VALUE of all three (777 is the fixture's distinctive REW_MONEY_MAX_LEVEL, see
        // `quest_template_row_chained_timed`) rather than just checking a comma follows — a comma-only check
        // would pass even if reward_money_max_level silently came out wrong (e.g. always the unwrap_or(0)
        // default). Any FURTHER end-append is still expected to land after this triple, never between them.
        assert!(insert.contains(",501,3600,777"), "next_quest_id=501, limit_time=3600, reward_money_max_level=777 must thread through, in order, as END-appended columns: {insert}");
    }

    #[test]
    fn insert_row_count_counts_values_tuples_not_statements() {
        // The provenance-stamp fix (216 review finding): a 3-row batched INSERT is 3 rows, a DELETE
        // is 0 — statement count (2) would under-report.
        let mut stmts = vec!["DELETE FROM game_x WHERE id >= 0".to_string()];
        push_insert(
            &mut stmts,
            "game_x",
            "a,b",
            &["(1,2)".into(), "(3,4)".into(), "(5,6)".into()],
        );
        assert_eq!(stmts.len(), 2, "one DELETE + one batched INSERT: {stmts:?}");
        assert_eq!(insert_row_count(&stmts), 3);
        assert_eq!(insert_row_count(&[]), 0);
    }

    #[test]
    fn family_absent_full_run_is_the_union_of_every_single_family_run() {
        // Full-run SELF-CONSISTENCY (work-item 216): with `--family` ABSENT every family is active,
        // so the full run's `stmts` must be a SUPERSET of every individual family's own filtered run
        // over the SAME dump, and re-running must be deterministic. HONESTY NOTE: this is NOT a
        // parity test against the pre-refactor `main()` body (no old code remains to call) — it
        // cross-checks `build_dump_plan` against itself, so a bug introduced identically into the
        // full-run and per-family paths would pass. The byte-parity of the 216 refactor itself was
        // established by a manual statement-by-statement diff during review, not by this test.
        let dump = tiny_quest_dump(101, 501);
        let mut full_args = test_args();
        full_args.family = None;
        let full_plan = build_dump_plan(&dump, &full_args, &None, &None).expect("full run");
        let full_set: std::collections::HashSet<&String> = full_plan.stmts.iter().collect();

        for &family in FAMILIES {
            let mut args = test_args();
            args.family = Some(family.to_string());
            let plan = build_dump_plan(&dump, &args, &None, &None)
                .unwrap_or_else(|e| panic!("family {family} failed: {e}"));
            for s in &plan.stmts {
                assert!(
                    full_set.contains(s),
                    "full run is missing a {family} statement: {s}"
                );
            }
            // Every family that ran got exactly one stamp, matching its own solo run's stamp count.
            assert_eq!(
                plan.stamps.len(),
                1,
                "family {family} plan.stamps: {:?}",
                plan.stamps
            );
            assert_eq!(plan.stamps[0].0, family);
        }
        // The full run stamps every family exactly once, in FAMILIES order.
        let stamped_families: Vec<&str> = full_plan.stamps.iter().map(|(f, _)| *f).collect();
        assert_eq!(
            stamped_families,
            FAMILIES.to_vec(),
            "{:?}",
            full_plan.stamps
        );

        // Determinism: re-running the full plan over the identical dump/args is byte-for-byte identical.
        let full_plan_again =
            build_dump_plan(&dump, &full_args, &None, &None).expect("full run again");
        assert_eq!(
            full_plan.stmts, full_plan_again.stmts,
            "build_dump_plan must be deterministic"
        );
    }

    // --- #223: bad input / missing files / partial datasets ---------------------------------------
    // The importer's whole job is to eat an operator-supplied file we have never seen. Every test
    // below pins what happens when that file is WRONG — missing, truncated, corrupt, or half a
    // schema — because the failure mode we cannot afford is a silent one: an import that "succeeds"
    // with garbage rows poisons the live world tables, and a panic mid-run leaves the clear+reload
    // half-applied (the DELETE ran, the INSERT never did).

    /// A unique scratch file per test (std-only; no `tempfile` dep — mirrors `pack_client.rs`'s
    /// `Scratch`). Removed on drop so a FAILING assert still cleans up after itself.
    struct ScratchFile(std::path::PathBuf);
    impl ScratchFile {
        fn new(tag: &str, bytes: &[u8]) -> Self {
            let p = std::env::temp_dir().join(format!(
                "lyracore-importer-test-{}-{tag}",
                std::process::id()
            ));
            std::fs::write(&p, bytes).expect("write scratch file");
            ScratchFile(p)
        }
        fn path(&self) -> &str {
            self.0.to_str().expect("scratch path is utf8")
        }
    }
    impl Drop for ScratchFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Value tuples in ONE `INSERT INTO t (cols) VALUES (..),(..)` statement (same counting rule as
    /// `insert_row_count`, per-statement — `matches('(')` would over-count by one for the column
    /// list). Used to assert that the bad rows of a partial dataset really were dropped.
    fn tuple_count(insert: &str) -> usize {
        insert.matches("),(").count() + 1
    }

    #[test]
    fn read_dump_on_a_missing_path_errors_and_names_the_path() {
        // A mistyped `--dump` is the single most common operator error, and the run that follows is
        // destructive (clear+reload). The error MUST name the path the operator actually passed —
        // a bare "No such file or directory" leaves them guessing which of --dump/--dbc/--terrain
        // was wrong. Both the plain and the .gz branch route through the same `read` context, so
        // both are pinned: read_dump reads the bytes BEFORE it looks at the extension.
        for path in [
            "/nonexistent/lyracore/importer/path.sql",
            "/nonexistent/lyracore/importer/path.sql.gz",
        ] {
            let err = read_dump(path).expect_err("a missing dump must not read as an empty dump");
            let msg = format!("{err:#}");
            assert!(
                msg.contains(path),
                "the error must name the path the operator passed, got: {msg}"
            );
        }
    }

    #[test]
    fn read_dump_on_a_gz_path_holding_non_gzip_bytes_errors_at_the_gunzip_step() {
        // A half-downloaded / plain-text-renamed-to-.gz dump must fail LOUDLY at decompression, not
        // panic and not decode to an empty string (an empty dump would make every builder emit its
        // DELETEs with no INSERTs — i.e. it would WIPE the live tables and reload nothing).
        let f = ScratchFile::new(
            "not-really-gzip.sql.gz",
            b"INSERT INTO `spell_chain` VALUES (1);",
        );
        let err = read_dump(f.path()).expect_err("non-gzip bytes behind a .gz name must error");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("gunzip dump"),
            "the error must point at the decompression step, got: {msg}"
        );
    }

    #[test]
    fn read_dump_decodes_invalid_utf8_lossily_instead_of_failing() {
        // DELIBERATE design decision, pinned so nobody "fixes" it into `read_to_string`: real
        // cmangos dumps are latin1/utf8-ish and carry stray non-UTF8 bytes inside creature/item
        // names. A strict decode would abort the WHOLE import over one bad byte in one name (and
        // `read_to_string` cannot even be used on the .gz path). We decode lossily on both paths:
        // the offending byte becomes U+FFFD and every valid row around it still imports.
        let f = ScratchFile::new("latin1.sql", b"INSERT INTO `t` VALUES (1,'Caf\xE9 Ale');");
        let text = read_dump(f.path()).expect("a stray non-UTF8 byte must not fail the import");
        assert!(
            text.starts_with("INSERT INTO `t` VALUES (1,'Caf") && text.ends_with(" Ale');"),
            "the ASCII around the bad byte must survive verbatim, got: {text:?}"
        );
        assert!(
            text.contains('\u{FFFD}'),
            "the bad byte must become the replacement char, not be dropped: {text:?}"
        );
        // And the lossily-decoded text is still parseable — the row is not lost.
        let rows = parse_table(&text, "t");
        assert_eq!(
            rows.len(),
            1,
            "the row around the bad byte must still parse"
        );
        assert_eq!(field(&rows[0], 0), "1");
    }

    #[test]
    fn parse_table_returns_no_rows_for_a_table_the_dump_never_mentions() {
        // A --family run asks for tables a partial dump may simply not contain (an operator can
        // dump `creature` without `spell_chain`). That must be an EMPTY result, never a panic:
        // every builder is expected to degrade to "DELETE, no INSERT" for a table it cannot find.
        let dump = "x INSERT INTO `creature_template` VALUES (1,'A',NULL,5); y";
        assert!(
            parse_table(dump, "spell_chain").is_empty(),
            "an absent table must yield zero rows, not a panic"
        );
        assert!(parse_table("", "creature_template").is_empty());
        assert!(parse_table("-- comment only, no INSERTs\n", "creature").is_empty());
    }

    #[test]
    fn parse_table_name_match_is_exact_not_a_prefix_of_a_longer_table() {
        // `creature` is a PREFIX of `creature_template`, and cmangos ships both. The needle carries
        // the closing backtick (``INSERT INTO `creature` VALUES ``), so the match is exact in BOTH
        // directions — asking for the short name never harvests the long table's rows (which would
        // import creature_template columns as spawn rows: total garbage), and asking for the long
        // name never matches the short table. Pinned because the guard is a single backtick in a
        // format! string, trivially lost in a refactor.
        let templates = "x INSERT INTO `creature_template` VALUES (1,'A',NULL,5); y";
        assert!(
            parse_table(templates, "creature").is_empty(),
            "`creature` must NOT match `creature_template` rows"
        );
        let spawns = "x INSERT INTO `creature` VALUES (7,299,0,1.0); y";
        assert!(
            parse_table(spawns, "creature_template").is_empty(),
            "`creature_template` must NOT match `creature` rows"
        );
        // ...and each still finds its own table in the same dump.
        let both = format!("{templates}\n{spawns}");
        assert_eq!(parse_table(&both, "creature_template").len(), 1);
        assert_eq!(parse_table(&both, "creature").len(), 1);
    }

    #[test]
    fn parse_table_keeps_complete_tuples_and_drops_an_unterminated_one() {
        // A truncated dump (interrupted download / disk full) ends mid-statement. The good tuples
        // ALREADY closed by a `)` are kept; the half-written tail is dropped, because a row is only
        // committed on its closing paren. No panic, no partial row with missing columns.
        let truncated = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133";
        let rows = parse_table(truncated, "spell_chain");
        assert_eq!(
            rows.len(),
            1,
            "the closed tuple survives, the truncated tail is dropped: {rows:?}"
        );
        assert_eq!(field(&rows[0], 0), "133");
        // A missing terminating `;` alone (both tuples closed) loses nothing.
        let no_semicolon = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133,133,2,0)";
        assert_eq!(parse_table(no_semicolon, "spell_chain").len(), 2);
    }

    #[test]
    fn parse_table_unterminated_quote_swallows_the_rest_of_the_dump() {
        // HONEST PIN of the REAL behavior, which is NOT per-row skip: an unclosed `'` puts the
        // parser in quote mode until the next `'` ANYWHERE downstream, so it eats the remainder of
        // the file — including later, perfectly well-formed statements for the same table. Rows
        // BEFORE the corruption still import. The consequence to know: a single unbalanced quote
        // silently TRUNCATES an import rather than dropping one row, so a suspiciously low row
        // count in the importer's own `eprintln!` totals is the symptom to look for.
        let dump = "x INSERT INTO `creature_template` VALUES (1,'Good'),(2,'oops); \
                    INSERT INTO `creature_template` VALUES (3,'Later'); y";
        let rows = parse_table(dump, "creature_template");
        assert_eq!(
            rows.len(),
            1,
            "only the rows before the unbalanced quote import: {rows:?}"
        );
        assert_eq!(field(&rows[0], 1), "Good");
    }

    #[test]
    fn build_stats_sql_drops_rows_whose_key_columns_are_missing_or_non_numeric() {
        // Partial-dataset policy (`filter_map(...parse().ok()?)`): a row we cannot key is DROPPED,
        // the import does NOT abort. Losing one stat row costs one class/level's base HP; aborting
        // costs the operator the whole run. The KEY columns only are validated, so this test also
        // documents the sharp edge below.
        let dump = "x INSERT INTO `player_classlevelstats` VALUES (1,1,20,0),\
                    ('warrior',2,29,0),(1,'two',29,0); \
                    INSERT INTO `player_levelstats` VALUES (1,1,1,23,20,22,20,21),(1,1,'x',1,1,1,1,1); y";
        let stmts = build_stats_sql(dump);
        let cls = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_class_level_stats "))
            .expect("the one good class row must still import");
        assert_eq!(
            tuple_count(cls),
            1,
            "exactly one class/level row survives; the two unparseable ones are dropped: {cls}"
        );
        assert!(
            cls.contains(&format!("({},1,1,20,0)", (1u32 << 8) | 1)),
            "{cls}"
        );
        let lvl = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_level_stats "))
            .expect("the one good race/class/level row must still import");
        assert_eq!(
            tuple_count(lvl),
            1,
            "the non-numeric level row is dropped, not imported with a garbage key: {lvl}"
        );
    }

    #[test]
    fn build_stats_sql_emits_empty_tokens_for_a_row_short_of_its_value_columns() {
        // SHARP EDGE, pinned honestly rather than wished away: only the KEY columns (class/level)
        // are `parse()`d — `base_health`/`base_mana` are interpolated verbatim. A row truncated to
        // its keys therefore emits `(...,,)`, which `spacetime sql` rejects at APPLY time (loud) —
        // it does not silently insert zeroes. If this ever needs hardening, the fix is to parse the
        // value columns too; until then this test documents that the guard is apply-time, not
        // parse-time, so nobody assumes short rows are already filtered.
        let dump = "x INSERT INTO `player_classlevelstats` VALUES (1,1); y";
        let stmts = build_stats_sql(dump);
        let cls = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_class_level_stats "))
            .expect("the short row is NOT dropped — its keys parsed");
        assert!(
            cls.contains(",,)"),
            "a keys-only row emits empty value tokens (apply-time failure, not silent zeroes): {cls}"
        );
    }

    #[test]
    fn build_stats_sql_empty_dump_yields_deletes_only_no_insert() {
        // An empty/absent stats section must still WIPE (clear+reload semantics) and emit no
        // INSERT — `push_insert` must not produce a syntactically-broken `VALUES` with no tuples.
        let stmts = build_stats_sql("");
        assert_eq!(
            stmts,
            vec![
                "DELETE FROM game_class_level_stats WHERE class_level >= 0".to_string(),
                "DELETE FROM game_level_stats WHERE race_class_level >= 0".to_string(),
            ],
            "an empty dump must yield the two DELETEs and nothing else"
        );
    }

    #[test]
    fn build_graveyard_zone_sql_drops_unparseable_rows_and_leaves_a_gap_in_row_id() {
        // Bad rows drop (a graveyard link we cannot key would resolve corpses to the wrong zone).
        // HONEST PIN: `enumerate()` runs BEFORE `filter_map`, so row_id is the PRE-filter index —
        // a dropped row leaves a HOLE in the sequence (1,3 here, not 1,2). That is harmless (row_id
        // is an opaque surrogate key and the DELETE is wholesale), but it means row_id is NOT a
        // count of imported rows and must never be used as one.
        let dump = "x INSERT INTO `game_graveyard_zone` VALUES (105,12,469),(106,'zone',469),(80,40,469); y";
        let stmts = build_graveyard_zone_sql(dump);
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_graveyard_zone "))
            .expect("the two good graveyard links must still import");
        assert!(insert.contains("(1,105,12,469)"), "{insert}");
        assert!(
            insert.contains("(3,80,40,469)"),
            "row_id skips the dropped row's index — pre-filter enumerate: {insert}"
        );
        assert_eq!(
            tuple_count(insert),
            2,
            "the non-numeric zone row must be dropped, not imported: {insert}"
        );
    }

    #[test]
    fn build_graveyard_zone_sql_empty_dump_yields_delete_only_no_insert() {
        let stmts = build_graveyard_zone_sql("");
        assert_eq!(
            stmts,
            vec!["DELETE FROM game_graveyard_zone WHERE row_id >= 0".to_string()],
            "an empty dump must wipe and reload nothing, not emit an empty INSERT"
        );
    }

    #[test]
    fn build_spell_chain_sql_drops_rows_with_a_short_or_out_of_range_column() {
        // Every one of the five columns is parsed here, so ANY malformed column drops the row —
        // stricter than build_stats_sql above, and deliberately so: a half-parsed rank-prereq row
        // would let a player train rank 3 without rank 2. `rank` is a u8, so a >255 rank is also
        // rejected rather than wrapping.
        let dump = "x INSERT INTO `spell_chain` VALUES (133,0,133,1,0),(143,133),\
                    (145,133,133,300,0),(147,133,133,2,'x'),(0,0,0,1,0); y";
        let stmts = build_spell_chain_sql(dump);
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_spell_chain "))
            .expect("the one well-formed chain row must still import");
        assert_eq!(
            tuple_count(insert),
            1,
            "short / u8-overflowing / non-numeric / spell_id-0 rows all drop: {insert}"
        );
        assert!(insert.contains("(133,0,133,1,0)"), "{insert}");
    }

    #[test]
    fn build_createinfo_spell_sql_drops_bad_rows_and_leaves_a_gap_in_the_id() {
        // A starter-kit row we cannot key would teach a spell to the wrong race/class at character
        // creation — drop it. Same pre-filter `enumerate()` shape as the graveyard builder: the id
        // of the surviving third row is 3, not 2. Pinned so a future "densify the ids" change is a
        // deliberate one and not a surprise.
        let dump = "x INSERT INTO `playercreateinfo_spell` VALUES (0,1,78),(1,0),(3,1,'fireball'),(3,1,78); y";
        let stmts = build_createinfo_spell_sql(dump);
        let insert = stmts
            .iter()
            .find(|s| s.starts_with("INSERT INTO game_createinfo_spell "))
            .expect("the two well-formed starter-spell rows must still import");
        assert_eq!(
            tuple_count(insert),
            2,
            "the short row and the non-numeric spell row must drop: {insert}"
        );
        assert!(insert.contains("(1,0,1,78)"), "{insert}");
        assert!(
            insert.contains("(4,3,1,78)"),
            "the id is the PRE-filter index, so it skips the two dropped rows: {insert}"
        );
    }

    #[test]
    fn build_createinfo_spell_sql_empty_dump_yields_delete_only_no_insert() {
        // Note the consequence this pins: with no `playercreateinfo_spell` in the dump, the import
        // WIPES the table. That is the intended wholesale-reload semantics (see the builder's doc),
        // but it means a --family run over a partial dump must not include this family by accident.
        let stmts = build_createinfo_spell_sql("");
        assert_eq!(
            stmts,
            vec!["DELETE FROM game_createinfo_spell WHERE id >= 0".to_string()]
        );
    }
}
