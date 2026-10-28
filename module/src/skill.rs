use spacetimedb::{table, Identity, ReducerContext, Table};

use lyracore_shared::constants::starter_item;

use crate::skilldata::{game_skill_ability, game_skill_availability};
use crate::{game_item_template, game_world_entity, WorldEntity};

pub const SKILL_PER_LEVEL: u32 = 5;

pub mod skill_line {
    pub const AXE_1H: u32 = 44;
    pub const AXE_2H: u32 = 172;
    pub const MACE_1H: u32 = 54;
    pub const MACE_2H: u32 = 160;
    pub const POLEARM: u32 = 229;
    pub const SWORD_1H: u32 = 43;
    pub const SWORD_2H: u32 = 55;
    pub const STAFF: u32 = 136;
    pub const FIST: u32 = 473;
    pub const DAGGER: u32 = 173;
    pub const UNARMED: u32 = 162;
    pub const DEFENSE: u32 = 95;
    pub const COOKING: u32 = 185;
    pub const SKINNING: u32 = 393;
    pub const LEATHERWORKING: u32 = 165;
    pub const HERBALISM: u32 = 182;
    pub const MINING: u32 = 186;
    pub const ALCHEMY: u32 = 171;
    pub const FIRST_AID: u32 = 129;
    pub const TAILORING: u32 = 197;
    pub const BLACKSMITHING: u32 = 164;
    pub const ENGINEERING: u32 = 202;
    pub const ENCHANTING: u32 = 333;
    pub const FISHING: u32 = 356;

    pub const LOCKPICKING: u32 = 633;
    pub const RIDING: u32 = lyracore_shared::trainer::RIDING_SKILL_LINE;
}

pub(crate) fn is_profession_line(line: u32) -> bool {
    use skill_line::*;
    matches!(
        line,
        COOKING
            | FIRST_AID
            | TAILORING
            | LEATHERWORKING
            | BLACKSMITHING
            | ENGINEERING
            | ALCHEMY
            | ENCHANTING
            | MINING
            | HERBALISM
            | SKINNING
            | FISHING
    )
}

pub const APPRENTICE_CAP: u16 = 75;

pub(crate) fn weapon_subclass_to_skill_line(subclass: u8) -> u32 {
    use skill_line::*;
    match subclass {
        0 => AXE_1H,
        1 => AXE_2H,
        4 => MACE_1H,
        5 => MACE_2H,
        6 => POLEARM,
        7 => SWORD_1H,
        8 => SWORD_2H,
        10 => STAFF,
        13 => FIST,
        15 => DAGGER,
        _ => UNARMED,
    }
}

pub(crate) fn skill_cap_for_level(level: u32) -> u32 {
    level * SKILL_PER_LEVEL
}

pub(crate) fn skill_diff(defense_skill: u32, weapon_skill: u32) -> u32 {
    defense_skill.saturating_sub(weapon_skill)
}

pub(crate) fn next_skill(current: u32, cap: u32) -> u32 {
    (current + 1).min(cap)
}

const BP_SCALE: f64 = 10_000.0;

const SEAM_HEADROOM: u32 = 7;

const CATCH_UP_LOG_OFFSET: f64 = 5.0;

const CATCH_UP_BASE_SLOPE: f64 = 0.22;
const CATCH_UP_LOW_LEVEL_LIFT: f64 = 12.0;

const INTELLECT_ASSIST_MAX: f64 = 0.10;

const INTELLECT_ASSIST_BANDS: [(u32, f64, f64); 4] = [
    (60, 60.0, 750.0),
    (70, 70.0, 1500.0),
    (80, 80.0, 3000.0),
    (u32::MAX, 255.0, 6000.0),
];

fn combat_headroom(current: u32, cap: u32, level: u32) -> Option<u32> {
    if cap == 0 || level == 0 {
        return None;
    }
    cap.checked_sub(current).filter(|&headroom| headroom > 0)
}

fn combat_practice_fraction(headroom: u32, cap: u32, level: u32) -> f64 {
    let seam = SEAM_HEADROOM as f64;
    let headroom_f = headroom as f64;
    let cap_f = cap as f64;
    let seam_floor = seam / cap_f;
    if headroom >= SEAM_HEADROOM {
        let slope = CATCH_UP_BASE_SLOPE * (1.0 + CATCH_UP_LOW_LEVEL_LIFT / level as f64);
        seam_floor
            + slope * ((headroom_f + CATCH_UP_LOG_OFFSET) / (seam + CATCH_UP_LOG_OFFSET)).ln()
    } else {
        (headroom_f * headroom_f) / (seam * cap_f)
    }
}

fn intellect_assist(intellect: u32, level: u32) -> f64 {
    let (_, reference_level, full_assist_intellect) = INTELLECT_ASSIST_BANDS
        .iter()
        .copied()
        .find(|&(ceiling, _, _)| level <= ceiling)
        .unwrap_or(INTELLECT_ASSIST_BANDS[INTELLECT_ASSIST_BANDS.len() - 1]);
    (INTELLECT_ASSIST_MAX
        * (intellect as f64 / full_assist_intellect)
        * (reference_level / level as f64))
        .clamp(0.0, INTELLECT_ASSIST_MAX)
}

pub(crate) fn combat_skillup_chance_bp(
    current: u32,
    cap: u32,
    level: u32,
    intellect: u32,
    weapon: bool,
) -> u32 {
    let Some(headroom) = combat_headroom(current, cap, level) else {
        return 0;
    };
    let practice = combat_practice_fraction(headroom, cap, level).clamp(0.0, 1.0);
    let assist = if weapon {
        intellect_assist(intellect, level)
    } else {
        0.0
    };
    ((practice + assist).clamp(0.0, 1.0) * BP_SCALE) as u32
}

pub(crate) fn skillup_chance_bp(current: u32, orange: u32, gray: u32) -> u32 {
    if gray <= orange {
        return 10_000;
    }
    if current < orange {
        return 10_000;
    }
    if current >= gray {
        return 0;
    }
    let span = gray - orange;
    (gray - current) * 10_000 / span
}

pub(crate) fn raised_cap(current_cap: u16, offered_cap: u16) -> u16 {
    current_cap.max(offered_cap)
}

#[table(accessor = game_player_skill, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct PlayerSkill {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub owner_identity: Identity,
    pub skill_line: u32,
    pub current: u16,
    pub max_rank: u16,
}

impl PlayerSkill {
    pub(crate) fn insert(ctx: &ReducerContext, row: Self) -> Self {
        let row = ctx.db.game_player_skill().insert(row);
        crate::native::skill_changed(ctx, None, Some(&row));
        row
    }

    pub(crate) fn update(self, ctx: &ReducerContext) -> Self {
        let previous = ctx.db.game_player_skill().id().find(self.id);
        let row = ctx.db.game_player_skill().id().update(self);
        crate::native::skill_changed(ctx, previous.as_ref(), Some(&row));
        row
    }

    pub(crate) fn delete(ctx: &ReducerContext, id: u64) {
        if let Some(row) = ctx.db.game_player_skill().id().find(id) {
            ctx.db.game_player_skill().id().delete(id);
            crate::native::skill_changed(ctx, Some(&row), None);
        }
    }
}

crate::character_owned!(delete, fn sweep_delete_game_player_skill(ctx, character_guid) {
    let skills = ctx.db.game_player_skill();
    for r in skills.by_character().filter(&character_guid).collect::<Vec<_>>() {
        PlayerSkill::delete(ctx, r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_player_skill(ctx, character_guid, io) {
    table = game_player_skill,
    by = by_character,
    remint = id,
});
crate::character_owned!(restamp, fn sweep_restamp_game_player_skill(ctx, character_guid, identity) {
    let skills = ctx.db.game_player_skill();
    for mut r in skills.by_character().filter(&character_guid).collect::<Vec<_>>() {
        if r.owner_identity != identity {
            r.owner_identity = identity;
            r.update(ctx);
        }
    }
});

fn equipped_weapon_skill_line(ctx: &ReducerContext, guid: u64) -> u32 {
    let Some(inst) = crate::items::item_in_slot(ctx, guid, starter_item::MAINHAND_SLOT) else {
        return skill_line::UNARMED;
    };
    let Some(tmpl) = ctx.db.game_item_template().entry().find(inst.entry) else {
        return skill_line::UNARMED;
    };
    if tmpl.class != starter_item::CLASS_WEAPON || crate::items::item_is_broken(&tmpl, &inst) {
        return skill_line::UNARMED;
    }
    weapon_subclass_to_skill_line(tmpl.subclass)
}

fn skill_value_or_cap(ctx: &ReducerContext, guid: u64, line: u32, level: u32) -> u32 {
    ctx.db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == line)
        .map(|s| s.current as u32)
        .unwrap_or_else(|| skill_cap_for_level(level))
}

pub(crate) fn effective_weapon_skill(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let line = equipped_weapon_skill_line(ctx, attacker.guid);
    skill_value_or_cap(ctx, attacker.guid, line, attacker.level)
}

pub(crate) fn effective_defense_skill(ctx: &ReducerContext, target: &WorldEntity) -> u32 {
    let base = skill_value_or_cap(ctx, target.guid, skill_line::DEFENSE, target.level);
    let aura = crate::spell::combat_field_bonus(ctx, target.guid, crate::spell::COMBAT_DEFENSE);
    let gear =
        crate::items::equipped_stat_bonus(ctx, target.guid, crate::items::EquipStat::Defense);
    (i64::from(base) + i64::from(aura) + i64::from(gear)).clamp(0, i64::from(u32::MAX)) as u32
}

pub(crate) fn skill_diff_ctx(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
) -> u32 {
    skill_diff(
        effective_defense_skill(ctx, target),
        effective_weapon_skill(ctx, attacker),
    )
}

fn raise_skill(ctx: &ReducerContext, guid: u64, line: u32) -> Option<u32> {
    let skills = ctx.db.game_player_skill();
    let mut row = skills
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == line)?;
    let next = next_skill(row.current as u32, row.max_rank as u32) as u16;
    if next != row.current {
        row.current = next;
        row.update(ctx);
    }
    Some(next as u32)
}

fn roll_and_raise_skill(
    ctx: &ReducerContext,
    guid: u64,
    line: u32,
    level: u32,
    intellect: u32,
    weapon: bool,
) {
    let Some(row) = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == line)
    else {
        return;
    };
    let chance = combat_skillup_chance_bp(
        row.current as u32,
        row.max_rank as u32,
        level,
        intellect,
        weapon,
    );
    if chance > 0 && ctx.random::<u32>() % 10_000 < chance {
        raise_skill(ctx, guid, line);
    }
}

pub(crate) fn gain_weapon_skill(ctx: &ReducerContext, attacker: &WorldEntity) {
    let line = equipped_weapon_skill_line(ctx, attacker.guid);
    roll_and_raise_skill(
        ctx,
        attacker.guid,
        line,
        attacker.level,
        attacker.intellect,
        true,
    );
}

pub(crate) fn gain_defense_skill(ctx: &ReducerContext, target: &WorldEntity) {
    roll_and_raise_skill(
        ctx,
        target.guid,
        skill_line::DEFENSE,
        target.level,
        0,
        false,
    );
}

pub(crate) fn is_combat_skill_line(line: u32) -> bool {
    lyracore_shared::trainer::is_combat_skill_line(line)
}

pub(crate) fn is_riding_skill_line(line: u32) -> bool {
    line == skill_line::RIDING
}

pub(crate) fn learn_riding(ctx: &ReducerContext, guid: u64, owner: Identity, rank: u16) {
    let skills = ctx.db.game_player_skill();
    if let Some(mut row) = skills
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == skill_line::RIDING)
    {
        let tier = raised_cap(row.max_rank, rank).max(row.current);
        if tier != row.max_rank || tier != row.current {
            row.max_rank = tier;
            row.current = tier;
            row.update(ctx);
        }
    } else {
        PlayerSkill::insert(
            ctx,
            PlayerSkill {
                id: 0,
                character_guid: guid,
                owner_identity: owner,
                skill_line: skill_line::RIDING,
                current: rank,
                max_rank: rank,
            },
        );
    }
}

pub(crate) fn raise_combat_caps(ctx: &ReducerContext, guid: u64, new_level: u32) {
    let cap = skill_cap_for_level(new_level) as u16;
    let skills = ctx.db.game_player_skill();
    for mut row in skills.by_character().filter(&guid).collect::<Vec<_>>() {
        let tracks_level =
            is_combat_skill_line(row.skill_line) || row.skill_line == skill_line::LOCKPICKING;
        if tracks_level && row.max_rank < cap {
            row.max_rank = cap;
            row.update(ctx);
        }
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
fn profile_mask_admits(mask: u32, id: u8) -> bool {
    mask == 0 || (id != 0 && (id as u32) <= u32::BITS && mask & (1u32 << (id - 1)) != 0)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
const PROFILE_SKILL_AVAILABILITY_LIMIT: usize = 16;

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn reconcile_profile_skill(
    ctx: &ReducerContext,
    guid: u64,
    line: u32,
) -> Result<bool, crate::actor::ActionRefusal> {
    if !is_combat_skill_line(line) {
        return Err(crate::actor::ActionRefusal::new(
            crate::actor::ActionRefusalKind::Class,
            format!("skill line {line} is not a combat skill"),
        ));
    }
    let actor = crate::helpers::live_entity(ctx, guid).map_err(|_| {
        crate::actor::ActionRefusal::new(
            crate::actor::ActionRefusalKind::MissingActor,
            format!("learner {guid} not in world"),
        )
    })?;
    let imported = ctx.db.game_skill_availability().count() > 0;
    let (admitted, identity_admitted, overflow) = if imported {
        let mut admitted = false;
        let mut identity_admitted = false;
        let mut overflow = false;
        for (examined, row) in ctx
            .db
            .game_skill_availability()
            .by_skill_line()
            .filter(&line)
            .take(PROFILE_SKILL_AVAILABILITY_LIMIT + 1)
            .enumerate()
        {
            if examined == PROFILE_SKILL_AVAILABILITY_LIMIT {
                overflow = true;
                break;
            }
            let identity_matches = profile_mask_admits(row.race_mask, actor.race())
                && profile_mask_admits(row.class_mask, actor.class());
            identity_admitted |= identity_matches;
            if identity_matches && i64::from(row.min_level) <= i64::from(actor.level) {
                admitted = true;
                break;
            }
        }
        (admitted, identity_admitted, overflow)
    } else {
        (
            line == skill_line::DEFENSE
                || line == skill_line::UNARMED
                || class_weapon_skill_lines(actor.class()).contains(&line),
            false,
            false,
        )
    };
    if !admitted {
        if overflow {
            return Err(crate::actor::ActionRefusal::new(
                crate::actor::ActionRefusalKind::ProfileLimit,
                format!(
                    "skill line {line} admission exceeded {PROFILE_SKILL_AVAILABILITY_LIMIT} availability rows"
                ),
            ));
        }
        let kind = if imported && identity_admitted {
            crate::actor::ActionRefusalKind::Level
        } else {
            crate::actor::ActionRefusalKind::Class
        };
        return Err(crate::actor::ActionRefusal::new(
            kind,
            format!(
                "skill line {line} is not available to race {} class {} at level {}",
                actor.race(),
                actor.class(),
                actor.level
            ),
        ));
    }

    let cap = skill_cap_for_level(actor.level) as u16;
    let skills = ctx.db.game_player_skill();
    if let Some(mut row) = skills
        .by_character()
        .filter(&guid)
        .find(|row| row.skill_line == line)
    {
        if row.max_rank >= cap {
            return Ok(false);
        }
        row.max_rank = cap;
        row.update(ctx);
    } else {
        PlayerSkill::insert(
            ctx,
            PlayerSkill {
                id: 0,
                character_guid: guid,
                owner_identity: actor.owner_identity,
                skill_line: line,
                current: cap,
                max_rank: cap,
            },
        );
    }
    Ok(true)
}

pub(crate) fn gain_profession_skill(
    ctx: &ReducerContext,
    guid: u64,
    line: u32,
    orange: u32,
    gray: u32,
) {
    let current = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == line)
        .map(|s| s.current as u32)
        .unwrap_or(0);
    if ctx.random::<u32>() % 10_000 < skillup_chance_bp(current, orange, gray) {
        let Some(new_current) = raise_skill(ctx, guid, line) else {
            return;
        };
        let Some(owner) = ctx
            .db
            .game_world_entity()
            .guid()
            .find(guid)
            .map(|e| e.owner_identity)
        else {
            return;
        };
        grant_autolearn_abilities(ctx, guid, owner, line, new_current);
    }
}

pub(crate) fn learn_profession(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    skill_line: u32,
    cap: u16,
) {
    let skills = ctx.db.game_player_skill();
    if let Some(mut row) = skills
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == skill_line)
    {
        let new_cap = raised_cap(row.max_rank, cap);
        if new_cap != row.max_rank {
            row.max_rank = new_cap;
            row.update(ctx);
        }
    } else {
        PlayerSkill::insert(
            ctx,
            PlayerSkill {
                id: 0,
                character_guid: guid,
                owner_identity: owner,
                skill_line,
                current: 1,
                max_rank: cap,
            },
        );
    }
    let current = skills
        .by_character()
        .filter(&guid)
        .find(|s| s.skill_line == skill_line)
        .map(|s| s.current as u32)
        .unwrap_or(1);
    grant_autolearn_abilities(ctx, guid, owner, skill_line, current);
}

pub(crate) fn grant_autolearn_abilities(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    skill_line: u32,
    current: u32,
) {
    let spells: Vec<u32> = ctx
        .db
        .game_skill_ability()
        .by_skill_line()
        .filter(&skill_line)
        .filter(|a| matches!(a.acquire_method, 1 | 2) && a.min_skill.max(0) as u32 <= current)
        .map(|a| a.spell_id)
        .collect();
    for spell_id in spells {
        crate::spell::learn_spell(ctx, guid, owner, spell_id);
    }
}

pub(crate) const PICK_LOCK_SPELL_ID: u32 = 1804;

pub(crate) fn grant_lockpicking_on_learn(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    spell_id: u32,
) {
    if spell_id != PICK_LOCK_SPELL_ID {
        return;
    }
    let skills = ctx.db.game_player_skill();
    if skills
        .by_character()
        .filter(&guid)
        .any(|s| s.skill_line == skill_line::LOCKPICKING)
    {
        return;
    }
    let level = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .map(|e| e.level)
        .unwrap_or(1);
    let cap = skill_cap_for_level(level.max(1)) as u16;
    PlayerSkill::insert(
        ctx,
        PlayerSkill {
            id: 0,
            character_guid: guid,
            owner_identity: owner,
            skill_line: skill_line::LOCKPICKING,
            current: 1,
            max_rank: cap,
        },
    );
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_learn_profession(
    ctx: &ReducerContext,
    character_guid: u64,
    skill_line: u32,
    cap: u32,
) -> Result<(), String> {
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    learn_profession(
        ctx,
        character_guid,
        entity.owner_identity,
        skill_line,
        cap as u16,
    );
    spacetimedb::log::info!(
        "debug_learn_profession: guid={character_guid} learned profession skill_line={skill_line} (cap {cap})"
    );
    Ok(())
}

// Synthetic profession markers name trainer offerings, never game_spell rows.
#[cfg(feature = "debug_reducers")]
const LEARN_COOKING_SPELL_ID: u32 = 50080;
#[cfg(feature = "debug_reducers")]
const LEARN_SKINNING_SPELL_ID: u32 = 50081;
#[cfg(feature = "debug_reducers")]
const LEARN_LEATHERWORKING_SPELL_ID: u32 = 50082;
#[cfg(feature = "debug_reducers")]
const LEARN_MINING_SPELL_ID: u32 = 50083;
#[cfg(feature = "debug_reducers")]
const LEARN_HERBALISM_SPELL_ID: u32 = 50084;
#[cfg(feature = "debug_reducers")]
const LEARN_ALCHEMY_SPELL_ID: u32 = 50085;
#[cfg(feature = "debug_reducers")]
const LEARN_FIRST_AID_SPELL_ID: u32 = 50086;
#[cfg(feature = "debug_reducers")]
const LEARN_TAILORING_SPELL_ID: u32 = 50087;
#[cfg(feature = "debug_reducers")]
const LEARN_BLACKSMITHING_SPELL_ID: u32 = 50088;

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_learn_profession_from_trainer(
    ctx: &ReducerContext,
    character_guid: u64,
    profession: u32,
) -> Result<(), String> {
    let spell_id = match profession {
        x if x == skill_line::COOKING => LEARN_COOKING_SPELL_ID,
        x if x == skill_line::SKINNING => LEARN_SKINNING_SPELL_ID,
        x if x == skill_line::LEATHERWORKING => LEARN_LEATHERWORKING_SPELL_ID,
        x if x == skill_line::MINING => LEARN_MINING_SPELL_ID,
        x if x == skill_line::HERBALISM => LEARN_HERBALISM_SPELL_ID,
        x if x == skill_line::ALCHEMY => LEARN_ALCHEMY_SPELL_ID,
        x if x == skill_line::FIRST_AID => LEARN_FIRST_AID_SPELL_ID,
        x if x == skill_line::TAILORING => LEARN_TAILORING_SPELL_ID,
        x if x == skill_line::BLACKSMITHING => LEARN_BLACKSMITHING_SPELL_ID,
        _ => {
            return Err(format!(
                "no profession-learn marker for skill_line {profession}"
            ))
        }
    };
    let player = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|_| format!("learner {character_guid} not in world"))?;
    let trainer = crate::helpers::nearest_entity(ctx, &player, |e| {
        !e.is_player()
            && !e.dead
            && e.npc_flags & lyracore_shared::constants::npc_flags::TRAINER != 0
    })
    .ok_or_else(|| "no live trainer near learner".to_string())?;
    crate::trainer::apply_trainer_buy(ctx, character_guid, trainer.guid, spell_id)
        .map_err(|refusal| refusal.as_tag().into())
}

#[cfg(feature = "debug_reducers")]
const LEARN_AXE_1H_SPELL_ID: u32 = 50130;
#[cfg(feature = "debug_reducers")]
const LEARN_POLEARM_SPELL_ID: u32 = 50131;

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_learn_weapon_from_trainer(
    ctx: &ReducerContext,
    character_guid: u64,
    skill_line: u32,
) -> Result<(), String> {
    let spell_id = match skill_line {
        x if x == self::skill_line::AXE_1H => LEARN_AXE_1H_SPELL_ID,
        x if x == self::skill_line::POLEARM => LEARN_POLEARM_SPELL_ID,
        _ => {
            return Err(format!(
                "no weapon-learn marker for skill_line {skill_line}"
            ))
        }
    };
    let player = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|_| format!("learner {character_guid} not in world"))?;
    let trainer = crate::helpers::nearest_entity(ctx, &player, |e| {
        !e.is_player()
            && !e.dead
            && e.npc_flags & lyracore_shared::constants::npc_flags::TRAINER != 0
    })
    .ok_or_else(|| "no live trainer near learner".to_string())?;
    crate::trainer::apply_trainer_buy(ctx, character_guid, trainer.guid, spell_id)
        .map_err(|refusal| refusal.as_tag().into())
}

pub(crate) const LEARN_APPRENTICE_RIDING_SPELL_ID: u32 = 50132;
pub(crate) const LEARN_JOURNEYMAN_RIDING_SPELL_ID: u32 = 50133;

pub(crate) fn riding_marker_rank(spell_id: u32) -> Option<u16> {
    match spell_id {
        LEARN_APPRENTICE_RIDING_SPELL_ID => Some(75),
        LEARN_JOURNEYMAN_RIDING_SPELL_ID => Some(150),
        _ => None,
    }
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_learn_riding(
    ctx: &ReducerContext,
    character_guid: u64,
    rank: u32,
) -> Result<(), String> {
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    let rank = u16::try_from(rank).unwrap_or(u16::MAX);
    learn_riding(ctx, character_guid, entity.owner_identity, rank);
    spacetimedb::log::info!("debug_learn_riding: guid={character_guid} riding -> {rank}");
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_learn_riding_from_trainer(
    ctx: &ReducerContext,
    character_guid: u64,
    rank: u32,
) -> Result<(), String> {
    let spell_id = match rank {
        75 => LEARN_APPRENTICE_RIDING_SPELL_ID,
        150 => LEARN_JOURNEYMAN_RIDING_SPELL_ID,
        _ => return Err(format!("no riding offering at rank {rank} (75 or 150)")),
    };
    let player = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|_| format!("learner {character_guid} not in world"))?;
    let trainer = crate::helpers::nearest_entity(ctx, &player, |e| {
        !e.is_player()
            && !e.dead
            && e.npc_flags & lyracore_shared::constants::npc_flags::TRAINER != 0
    })
    .ok_or_else(|| "no live trainer near learner".to_string())?;
    crate::trainer::apply_trainer_buy(ctx, character_guid, trainer.guid, spell_id)
        .map_err(|refusal| refusal.as_tag().into())
}

pub(crate) fn class_starting_weapon_skill_lines(class: u8) -> &'static [u32] {
    use skill_line::*;
    match class {
        1 => &[AXE_1H, MACE_1H, SWORD_1H],
        2 => &[MACE_1H, MACE_2H],
        3 => &[AXE_1H, DAGGER],
        4 => &[DAGGER],
        5 => &[MACE_1H],
        7 => &[MACE_1H, STAFF],
        8 => &[STAFF],
        9 => &[DAGGER],
        11 => &[DAGGER, MACE_1H, STAFF],
        _ => &[],
    }
}

pub(crate) fn class_weapon_skill_lines(class: u8) -> &'static [u32] {
    use skill_line::*;
    match class {
        1 => &[
            AXE_1H, AXE_2H, MACE_1H, MACE_2H, POLEARM, SWORD_1H, SWORD_2H, FIST, DAGGER,
        ],
        2 => &[
            AXE_1H, AXE_2H, MACE_1H, MACE_2H, POLEARM, SWORD_1H, SWORD_2H,
        ],
        3 => &[
            AXE_1H, AXE_2H, DAGGER, FIST, POLEARM, STAFF, SWORD_1H, SWORD_2H,
        ],
        4 => &[DAGGER, FIST, MACE_1H, SWORD_1H],
        5 => &[DAGGER, MACE_1H, STAFF],
        7 => &[AXE_1H, AXE_2H, DAGGER, FIST, MACE_1H, MACE_2H, STAFF],
        8 => &[DAGGER, STAFF, SWORD_1H],
        9 => &[DAGGER, STAFF, SWORD_1H],
        11 => &[DAGGER, FIST, MACE_1H, MACE_2H, STAFF],
        _ => &[],
    }
}

pub(crate) fn ensure_player_skills(
    ctx: &ReducerContext,
    guid: u64,
    owner: Identity,
    level: u32,
    class: u8,
) {
    let cap = skill_cap_for_level(level) as u16;
    let equipped_weapon = equipped_weapon_skill_line(ctx, guid);
    let skills = ctx.db.game_player_skill();

    let mut lines: std::collections::HashSet<u32> = std::collections::HashSet::new();
    lines.insert(skill_line::DEFENSE);
    lines.insert(skill_line::UNARMED);
    lines.insert(equipped_weapon);
    for &l in class_starting_weapon_skill_lines(class) {
        lines.insert(l);
    }

    for line in lines {
        let present = skills
            .by_character()
            .filter(&guid)
            .any(|s| s.skill_line == line);
        if !present {
            PlayerSkill::insert(
                ctx,
                PlayerSkill {
                    id: 0,
                    character_guid: guid,
                    owner_identity: owner,
                    skill_line: line,
                    current: cap,
                    max_rank: cap,
                },
            );
        }
    }

    raise_combat_caps(ctx, guid, level);
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_set_skill(
    ctx: &ReducerContext,
    character_guid: u64,
    skill_line: u32,
    value: u32,
) -> Result<(), String> {
    let entity = crate::helpers::live_entity(ctx, character_guid)?;
    let cap = skill_cap_for_level(entity.level);
    let current = value.min(cap) as u16;
    let max_rank = cap as u16;
    let skills = ctx.db.game_player_skill();
    if let Some(mut row) = skills
        .by_character()
        .filter(&character_guid)
        .find(|s| s.skill_line == skill_line)
    {
        row.current = current;
        row.max_rank = max_rank;
        row.update(ctx);
    } else {
        PlayerSkill::insert(
            ctx,
            PlayerSkill {
                id: 0,
                character_guid,
                owner_identity: entity.owner_identity,
                skill_line,
                current,
                max_rank,
            },
        );
    }
    spacetimedb::log::info!(
        "debug_set_skill: guid={character_guid} line={skill_line} -> {current}/{max_rank}"
    );
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_reseed_skills(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    // Refuse after Transfer begins because a reseed would miss the exported skill snapshot.
    let ch = crate::helpers::require_character(ctx, character_guid)
        .map_err(|_| format!("no game_character row for guid {character_guid}"))?;
    ensure_player_skills(ctx, ch.guid, ctx.sender(), ch.level as u32, ch.class);
    spacetimedb::log::info!(
        "debug_reseed_skills: guid={character_guid} class={} level={}",
        ch.class,
        ch.level
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn riding_is_neither_a_combat_line_nor_a_profession() {
        assert_eq!(skill_line::RIDING, 762, "verbatim SkillLine.dbc id");
        assert!(is_riding_skill_line(skill_line::RIDING));

        assert!(!is_combat_skill_line(skill_line::RIDING));
        assert!(!is_profession_line(skill_line::RIDING));
        for line in [
            skill_line::SWORD_1H,
            skill_line::DEFENSE,
            skill_line::COOKING,
        ] {
            assert!(!is_riding_skill_line(line), "line {line} is not riding");
        }
    }

    #[test]
    fn riding_markers_map_to_the_two_vanilla_tiers() {
        assert_eq!(
            riding_marker_rank(LEARN_APPRENTICE_RIDING_SPELL_ID),
            Some(75)
        );
        assert_eq!(
            riding_marker_rank(LEARN_JOURNEYMAN_RIDING_SPELL_ID),
            Some(150)
        );
        assert_eq!(
            riding_marker_rank(2050),
            None,
            "a real spell id is no marker"
        );
        assert_ne!(
            LEARN_APPRENTICE_RIDING_SPELL_ID, LEARN_JOURNEYMAN_RIDING_SPELL_ID,
            "two tiers need two marker ids or one offering shadows the other"
        );
    }

    #[test]
    fn skill_line_ids_are_the_verbatim_vanilla_dbc_values() {
        assert_eq!(skill_line::SWORD_1H, 43);
        assert_eq!(skill_line::AXE_1H, 44);
        assert_eq!(skill_line::MACE_1H, 54);
        assert_eq!(skill_line::SWORD_2H, 55);
        assert_eq!(skill_line::DEFENSE, 95);
        assert_eq!(skill_line::STAFF, 136);
        assert_eq!(skill_line::MACE_2H, 160);
        assert_eq!(skill_line::UNARMED, 162);
        assert_eq!(skill_line::AXE_2H, 172);
        assert_eq!(skill_line::DAGGER, 173);
        assert_eq!(skill_line::POLEARM, 229);
        assert_eq!(skill_line::FIST, 473);
    }

    #[test]
    fn weapon_subclass_maps_to_vanilla_skill_lines() {
        assert_eq!(weapon_subclass_to_skill_line(7), skill_line::SWORD_1H);
        assert_eq!(weapon_subclass_to_skill_line(8), skill_line::SWORD_2H);
        assert_eq!(weapon_subclass_to_skill_line(0), skill_line::AXE_1H);
        assert_eq!(weapon_subclass_to_skill_line(15), skill_line::DAGGER);
        assert_eq!(weapon_subclass_to_skill_line(10), skill_line::STAFF);
        assert_eq!(weapon_subclass_to_skill_line(16), skill_line::UNARMED);
        assert_eq!(weapon_subclass_to_skill_line(19), skill_line::UNARMED);
        assert_eq!(weapon_subclass_to_skill_line(255), skill_line::UNARMED);
    }

    #[test]
    fn skill_cap_is_level_times_five() {
        assert_eq!(skill_cap_for_level(1), 5);
        assert_eq!(skill_cap_for_level(60), 300);
        assert_eq!(skill_cap_for_level(0), 0);
    }

    #[test]
    fn next_skill_steps_one_and_caps() {
        assert_eq!(next_skill(0, 300), 1);
        assert_eq!(next_skill(299, 300), 300);
        assert_eq!(next_skill(300, 300), 300);
        assert_eq!(next_skill(305, 300), 300);
    }

    #[test]
    fn combat_skillup_chance_zero_on_degenerate_inputs() {
        assert_eq!(
            combat_skillup_chance_bp(10, 0, 10, 0, false),
            0,
            "cap=0 never rolls"
        );
        assert_eq!(
            combat_skillup_chance_bp(10, 300, 0, 0, false),
            0,
            "level=0 never rolls"
        );
        assert_eq!(
            combat_skillup_chance_bp(300, 300, 60, 0, false),
            0,
            "at cap never rolls"
        );
        assert_eq!(
            combat_skillup_chance_bp(305, 300, 60, 0, false),
            0,
            "over cap (shouldn't happen) never rolls"
        );
    }

    #[test]
    fn combat_skillup_chance_meets_at_the_seam_headroom_of_seven() {
        // Headroom 6 tapers as 36/(7*cap); headroom 7 starts catch-up at 7/cap.
        assert_eq!(combat_skillup_chance_bp(294, 300, 60, 0, false), 171);
        assert_eq!(combat_skillup_chance_bp(293, 300, 60, 0, false), 233);
        assert_eq!(combat_skillup_chance_bp(54, 60, 12, 0, false), 857);
        assert_eq!(combat_skillup_chance_bp(53, 60, 12, 0, false), 1_166);
    }

    #[test]
    fn combat_skillup_catch_up_is_faster_at_low_level() {
        let cap = 300u32;
        let headroom = 60u32;
        let current = cap - headroom;
        let low = combat_skillup_chance_bp(current, cap, 10, 0, false);
        let mid = combat_skillup_chance_bp(current, cap, 30, 0, false);
        let high = combat_skillup_chance_bp(current, cap, 60, 0, false);
        assert!(
            low > mid,
            "level 10 must catch up faster than level 30 ({low} vs {mid})"
        );
        assert!(
            mid > high,
            "level 30 must catch up faster than level 60 ({mid} vs {high})"
        );
    }

    #[test]
    fn intellect_assist_is_clamped_and_zero_without_intellect() {
        for level in [1u32, 30, 60] {
            assert_eq!(intellect_assist(0, level), 0.0, "no intellect => no assist");
            assert!(
                intellect_assist(u32::MAX, level) <= INTELLECT_ASSIST_MAX,
                "assist must clamp at {INTELLECT_ASSIST_MAX}"
            );
        }
        assert_eq!(intellect_assist(100_000, 60), INTELLECT_ASSIST_MAX);
    }

    #[test]
    fn combat_skillup_chance_is_non_increasing_as_current_climbs() {
        let cap = 300u32;
        let level = 60;
        let mut prev = 10_001u32;
        for current in 0..=cap {
            let c = combat_skillup_chance_bp(current, cap, level, 0, false);
            assert!(
                c <= prev,
                "chance must not rise: at {current}/{cap} got {c} after {prev}"
            );
            prev = c;
        }
    }

    #[test]
    fn combat_skillup_chance_weapon_intellect_bonus_only_applies_to_weapon_lines() {
        let (current, cap, level, intellect) = (100u32, 300u32, 60u32, 500u32);
        let defense = combat_skillup_chance_bp(current, cap, level, intellect, false);
        let weapon = combat_skillup_chance_bp(current, cap, level, intellect, true);
        assert!(
            weapon > defense,
            "weapon line should get a strictly higher chance from intellect > 0"
        );
        let weapon_no_int = combat_skillup_chance_bp(current, cap, level, 0, true);
        assert_eq!(
            weapon_no_int, defense,
            "zero intellect => no bonus => weapon matches defense"
        );
    }

    #[test]
    fn combat_skillup_chance_never_exceeds_10000bp() {
        for cap in [5u32, 60, 300] {
            for current in (0..cap).step_by(3) {
                for level in [1u32, 30, 60] {
                    let c = combat_skillup_chance_bp(current, cap, level, 750, true);
                    assert!(
                        c <= 10_000,
                        "chance {c} exceeds 10000bp at current={current} cap={cap} level={level}"
                    );
                }
            }
        }
    }

    #[test]
    fn skill_diff_is_defense_minus_weapon_clamped() {
        assert_eq!(skill_diff(300, 300), 0);
        assert_eq!(skill_diff(315, 300), 15);
        assert_eq!(skill_diff(300, 315), 0);
    }

    #[test]
    fn is_combat_skill_line_true_for_dagger_false_for_cooking() {
        assert!(
            is_combat_skill_line(skill_line::DAGGER),
            "Dagger is a combat (weapon) line"
        );
        assert!(
            !is_combat_skill_line(skill_line::COOKING),
            "Cooking is a profession line, not combat"
        );
    }

    #[test]
    fn profession_skill_lines_are_verbatim_vanilla_dbc_values() {
        assert_eq!(skill_line::COOKING, 185);
        assert_eq!(skill_line::SKINNING, 393);
        assert_eq!(skill_line::LEATHERWORKING, 165);
        assert_eq!(skill_line::HERBALISM, 182);
        assert_eq!(skill_line::MINING, 186);
        assert_eq!(skill_line::ALCHEMY, 171);
        assert_eq!(skill_line::FIRST_AID, 129);
        assert_eq!(skill_line::TAILORING, 197);
        assert_eq!(skill_line::BLACKSMITHING, 164);
        assert_eq!(skill_line::ENGINEERING, 202);
        assert_eq!(skill_line::ENCHANTING, 333);
        assert_eq!(skill_line::FISHING, 356);
    }

    #[test]
    fn profession_skill_adds_one_per_craft_capped_at_apprentice_75() {
        let cap = APPRENTICE_CAP as u32;
        assert_eq!(cap, 75);
        assert_eq!(next_skill(1, cap), 2);
        assert_eq!(next_skill(74, cap), 75);
        assert_eq!(next_skill(75, cap), 75);
        assert!(
            skill_cap_for_level(1) < cap,
            "profession cap 75 must exceed the level-1 combat cap (5)"
        );
        assert_eq!(next_skill(5, cap), 6);
    }

    #[test]
    fn skillup_chance_curve_orange_full_gray_zero_and_monotone() {
        assert_eq!(skillup_chance_bp(80, 60, 80), 0);
        assert_eq!(skillup_chance_bp(90, 60, 80), 0);
        assert_eq!(skillup_chance_bp(60, 60, 120), 10_000);
        assert_eq!(skillup_chance_bp(40, 60, 120), 10_000);
        assert_eq!(skillup_chance_bp(50, 1, 0), 10_000);
        assert_eq!(skillup_chance_bp(50, 60, 60), 10_000);
        assert_eq!(skillup_chance_bp(50, 60, 30), 10_000);
        assert_eq!(skillup_chance_bp(70, 60, 80), 5_000);
        assert_eq!(skillup_chance_bp(65, 60, 80), 7_500);
        let mut prev = 10_001u32;
        for current in 0u32..=130 {
            let c = skillup_chance_bp(current, 60, 120);
            assert!(
                c <= prev,
                "chance must not rise: at {current} got {c} after {prev}"
            );
            prev = c;
        }
    }

    #[test]
    fn raised_cap_lifts_only_a_higher_tier() {
        assert_eq!(raised_cap(75, 150), 150);
        assert_eq!(raised_cap(150, 225), 225);
        assert_eq!(raised_cap(225, 300), 300);
        assert_eq!(raised_cap(75, 75), 75);
        assert_eq!(raised_cap(150, 75), 150);
    }

    #[test]
    fn class_weapon_skill_lines_are_the_vanilla_melee_proficiencies() {
        let lines = |class| {
            let mut lines = class_weapon_skill_lines(class).to_vec();
            lines.sort_unstable();
            lines
        };
        assert_eq!(lines(1), [43, 44, 54, 55, 160, 172, 173, 229, 473]);
        assert_eq!(lines(2), [43, 44, 54, 55, 160, 172, 229]);
        assert_eq!(lines(3), [43, 44, 55, 136, 172, 173, 229, 473]);
        assert_eq!(lines(4), [43, 54, 173, 473]);
        assert_eq!(lines(5), [54, 136, 173]);
        assert_eq!(lines(7), [44, 54, 136, 160, 172, 173, 473]);
        assert_eq!(lines(8), [43, 136, 173]);
        assert_eq!(lines(9), [43, 136, 173]);
        assert_eq!(lines(11), [54, 136, 160, 173, 473]);
        assert!(lines(0).is_empty());
        assert!(lines(6).is_empty());
    }

    #[test]
    fn combat_skillup_chance_matches_worked_examples() {
        // 7/300 + 0.22*(1 + 12/60)*ln(205/12) = 0.7726
        assert_eq!(combat_skillup_chance_bp(100, 300, 60, 0, false), 7_725);
        // Level 60 with 750 intellect adds the full 0.10 assist.
        assert_eq!(combat_skillup_chance_bp(100, 300, 60, 750, true), 8_725);
        // 1/(7*300) = 0.00048
        assert_eq!(combat_skillup_chance_bp(299, 300, 60, 0, false), 4);
        // 7/60 + 0.22*(1 + 12/12)*ln(45/12) = 0.6982
        assert_eq!(combat_skillup_chance_bp(20, 60, 12, 0, false), 6_982);
        // 1/(7*5) = 0.0286
        assert_eq!(combat_skillup_chance_bp(4, 5, 1, 0, false), 285);
    }
}
