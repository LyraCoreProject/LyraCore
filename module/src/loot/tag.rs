use std::collections::BTreeSet;

use spacetimedb::{table, ReducerContext, Table};

use lyracore_shared::group::RAID_MAX_MEMBERS;
use lyracore_shared::loot::LootRefusal;

use crate::game_world_entity;

use super::game_corpse_loot_eligible;

#[cfg(feature = "debug_reducers")]
use crate::{
    game_corpse_loot, game_creature_template, game_group, game_group_member, game_item_template,
};

#[table(accessor = game_creature_quest_tap)]
pub struct CreatureQuestTap {
    #[primary_key]
    pub creature_guid: u64,
    pub character_guid: u64,
}

#[table(
    accessor = game_creature_quest_tap_member,
    index(accessor = by_creature, btree(columns = [creature_guid]))
)]
pub struct CreatureQuestTapMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub creature_guid: u64,
    pub character_guid: u64,
}

// Preserve the original party identity even if both members leave and current rows disappear.
#[table(accessor = game_creature_loot_tag_group)]
pub struct CreatureLootTagGroup {
    #[primary_key]
    pub creature_guid: u64,
    pub group_id: u64,
}

pub(crate) struct DeathEntitlement {
    pub group_id: Option<u64>,
    pub recipients: Vec<u64>,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LiveLootTagEligibility {
    Available,
    Foreign,
    ReadLimit,
}

pub(crate) fn controlling_character(ctx: &ReducerContext, source_guid: u64) -> Option<u64> {
    let entities = ctx.db.game_world_entity();
    let source = entities.guid().find(source_guid)?;
    if source.is_player() {
        return (!source.dead).then_some(source.guid);
    }
    entities
        .guid()
        .find(source.owner_guid)
        .filter(|owner| owner.is_player() && !owner.dead)
        .map(|owner| owner.guid)
}

// TAPPED is stored; TAPPED_BY_PLAYER remains viewer-relative.
pub(crate) fn record_first_threat(
    ctx: &ReducerContext,
    creature_guid: u64,
    source_guid: u64,
) -> bool {
    let tags = ctx.db.game_creature_quest_tap();
    if tags.creature_guid().find(creature_guid).is_some() {
        return false;
    }

    let entities = ctx.db.game_world_entity();
    let Some(mut creature) = entities.guid().find(creature_guid) else {
        return false;
    };
    if creature.is_player() || creature.owner_guid != 0 || creature.dead {
        return false;
    }
    let Some(character_guid) = controlling_character(ctx, source_guid) else {
        return false;
    };

    let group = crate::group::group_of(ctx, character_guid);
    let mut members: BTreeSet<u64> = group
        .as_ref()
        .map(|membership| {
            crate::group::members_of(ctx, membership.group_id)
                .into_iter()
                .map(|member| member.character_guid)
                .collect()
        })
        .unwrap_or_default();
    members.insert(character_guid);

    tags.insert(CreatureQuestTap {
        creature_guid,
        character_guid,
    });
    if let Some(membership) = group {
        ctx.db
            .game_creature_loot_tag_group()
            .insert(CreatureLootTagGroup {
                creature_guid,
                group_id: membership.group_id,
            });
    }
    let tag_members = ctx.db.game_creature_quest_tap_member();
    for character_guid in members {
        tag_members.insert(CreatureQuestTapMember {
            id: 0,
            creature_guid,
            character_guid,
        });
    }

    let stored = (creature.dynamic_flags | lyracore_shared::constants::unit_dynamic_flags::TAPPED)
        & !lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER;
    if creature.dynamic_flags != stored {
        creature.dynamic_flags = stored;
        creature.update(ctx);
    }
    true
}

pub(crate) fn death_entitlement(
    ctx: &ReducerContext,
    creature_guid: u64,
    x: f32,
    y: f32,
    map_id: u32,
    instance_id: u64,
) -> Option<DeathEntitlement> {
    let tag = ctx
        .db
        .game_creature_quest_tap()
        .creature_guid()
        .find(creature_guid)?;
    let group_id = ctx
        .db
        .game_creature_loot_tag_group()
        .creature_guid()
        .find(creature_guid)
        .map(|group| group.group_id);
    let entities = ctx.db.game_world_entity();
    let recipients = ctx
        .db
        .game_creature_quest_tap_member()
        .by_creature()
        .filter(&creature_guid)
        .filter_map(|member| {
            if !membership_is_current(ctx, group_id, tag.character_guid, member.character_guid) {
                return None;
            }
            let character = entities.guid().find(member.character_guid)?;
            let dx = character.x - x;
            let dy = character.y - y;
            (character.is_player()
                && crate::group::eligible_for_kill_reward(
                    character.dead,
                    character.map_id == map_id,
                    character.instance_id == instance_id,
                    dx * dx + dy * dy,
                ))
            .then_some(member.character_guid)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Some(DeathEntitlement {
        group_id,
        recipients,
    })
}

fn membership_is_current(
    ctx: &ReducerContext,
    tag_group_id: Option<u64>,
    tagger_guid: u64,
    member_guid: u64,
) -> bool {
    match tag_group_id {
        Some(group_id) => crate::group::group_of(ctx, member_guid)
            .is_some_and(|membership| membership.group_id == group_id),
        None => member_guid == tagger_guid,
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn live_loot_tag_eligibility(
    ctx: &ReducerContext,
    creature_guid: u64,
    character_guid: u64,
) -> LiveLootTagEligibility {
    let Some(tag) = ctx
        .db
        .game_creature_quest_tap()
        .creature_guid()
        .find(creature_guid)
    else {
        return LiveLootTagEligibility::Available;
    };
    let members: Vec<_> = ctx
        .db
        .game_creature_quest_tap_member()
        .by_creature()
        .filter(&creature_guid)
        .take(RAID_MAX_MEMBERS + 1)
        .collect();
    if members.len() > RAID_MAX_MEMBERS {
        return LiveLootTagEligibility::ReadLimit;
    }
    let group_id = ctx
        .db
        .game_creature_loot_tag_group()
        .creature_guid()
        .find(creature_guid)
        .map(|group| group.group_id);
    if members
        .iter()
        .any(|member| member.character_guid == character_guid)
        && membership_is_current(ctx, group_id, tag.character_guid, character_guid)
    {
        LiveLootTagEligibility::Available
    } else {
        LiveLootTagEligibility::Foreign
    }
}

// Rejoining must not restore rights from an earlier Loot Tag snapshot.
pub(crate) fn revoke_group_member(ctx: &ReducerContext, group_id: u64, character_guid: u64) {
    let tagged_creatures: Vec<u64> = ctx
        .db
        .game_creature_loot_tag_group()
        .iter()
        .filter(|tag| tag.group_id == group_id)
        .map(|tag| tag.creature_guid)
        .collect();
    let members = ctx.db.game_creature_quest_tap_member();
    for creature_guid in tagged_creatures {
        for member in members
            .by_creature()
            .filter(&creature_guid)
            .filter(|member| member.character_guid == character_guid)
            .collect::<Vec<_>>()
        {
            members.id().delete(member.id);
        }
        if let Some(creature) = ctx.db.game_world_entity().guid().find(creature_guid) {
            crate::native::loot_flags(ctx, &creature);
        }
    }
}

pub(crate) fn record_corpse_eligibility(
    ctx: &ReducerContext,
    corpse_guid: u64,
    recipients: &[u64],
) {
    let eligible = ctx.db.game_corpse_loot_eligible();
    for id in eligible
        .by_corpse()
        .filter(&corpse_guid)
        .map(|row| row.id)
        .collect::<Vec<_>>()
    {
        eligible.id().delete(id);
    }
    for eligible_guid in recipients.iter().copied().collect::<BTreeSet<_>>() {
        eligible.insert(super::CorpseLootEligible {
            id: 0,
            corpse_guid,
            eligible_guid,
        });
    }
    if let Some(corpse) = ctx.db.game_world_entity().guid().find(corpse_guid) {
        crate::native::loot_flags(ctx, &corpse);
    }
}

pub(crate) fn corpse_eligible_recipients(ctx: &ReducerContext, corpse_guid: u64) -> Vec<u64> {
    ctx.db
        .game_corpse_loot_eligible()
        .by_corpse()
        .filter(&corpse_guid)
        .map(|row| row.eligible_guid)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn corpse_eligible_for_access(recipients: &[u64], actor_guid: u64) -> bool {
    recipients.binary_search(&actor_guid).is_ok()
}

// Empty corpse eligibility refuses everyone; GameObjects do not use Loot Tags.
pub(crate) fn corpse_access_gate(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
) -> Result<(), String> {
    corpse_access(ctx, actor_guid, corpse_guid).map_err(|refusal| {
        super::refused(
            refusal,
            &format!("actor_guid={actor_guid} corpse_guid={corpse_guid}"),
        )
    })
}

pub(crate) fn corpse_access(
    ctx: &ReducerContext,
    actor_guid: u64,
    corpse_guid: u64,
) -> Result<(), LootRefusal> {
    let eligible = corpse_eligible_recipients(ctx, corpse_guid);
    if corpse_eligible_for_access(&eligible, actor_guid) {
        return Ok(());
    }
    Err(LootRefusal::LootTagIneligible)
}

pub(crate) fn clear(ctx: &ReducerContext, creature_guid: u64) {
    ctx.db
        .game_creature_quest_tap()
        .creature_guid()
        .delete(creature_guid);
    ctx.db
        .game_creature_loot_tag_group()
        .creature_guid()
        .delete(creature_guid);
    let members = ctx.db.game_creature_quest_tap_member();
    for member in members
        .by_creature()
        .filter(&creature_guid)
        .collect::<Vec<_>>()
    {
        members.id().delete(member.id);
    }

    let entities = ctx.db.game_world_entity();
    if let Some(mut creature) = entities.guid().find(creature_guid) {
        let tag_flags = lyracore_shared::constants::unit_dynamic_flags::TAPPED
            | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER;
        if creature.dynamic_flags & tag_flags != 0 {
            creature.dynamic_flags &= !tag_flags;
            creature.update(ctx);
        }
    }
}

#[cfg(feature = "debug_reducers")]
const LOOT_TAG_FIXTURE_ENTRY: u32 = 51_000;
#[cfg(feature = "debug_reducers")]
const LOOT_TAG_FIXTURE_GROUP: u64 = 5_093_850;
#[cfg(feature = "debug_reducers")]
const LOOT_TAG_FIXTURE_CHARACTER_A: u64 = 5_093_851;
#[cfg(feature = "debug_reducers")]
const LOOT_TAG_FIXTURE_CHARACTER_B: u64 = 5_093_852;
#[cfg(feature = "debug_reducers")]
const LOOT_TAG_FIXTURE_CHARACTER_C: u64 = 5_093_853;
#[cfg(feature = "debug_reducers")]
const LOOT_TAG_FIXTURE_CHARACTER_D: u64 = 5_093_854;
#[cfg(feature = "debug_reducers")]
const LOOT_TAG_FIXTURE_CHARACTER_E: u64 = 5_093_855;
#[cfg(feature = "debug_reducers")]
const LOOT_TAG_FIXTURE_CHARACTER_F: u64 = 5_093_856;

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
#[allow(clippy::too_many_lines)]
pub fn debug_verify_loot_tag_fixture(ctx: &ReducerContext) -> Result<(), String> {
    let origin = ctx
        .db
        .game_creature_template()
        .entry()
        .find(LOOT_TAG_FIXTURE_ENTRY)
        .ok_or_else(|| "Loot Tag fixture creature template is missing".to_string())?;
    let base_x = -8_900.0;
    for (guid, x) in [
        (LOOT_TAG_FIXTURE_CHARACTER_A, base_x),
        (LOOT_TAG_FIXTURE_CHARACTER_B, base_x + 1.0),
        (LOOT_TAG_FIXTURE_CHARACTER_C, base_x + 2.0),
        (LOOT_TAG_FIXTURE_CHARACTER_D, base_x + 3.0),
        (LOOT_TAG_FIXTURE_CHARACTER_E, base_x + 4.0),
        (LOOT_TAG_FIXTURE_CHARACTER_F, base_x + 5.0),
    ] {
        insert_fixture_entity(ctx, &origin, guid, x, true, 0);
    }
    let first_tag = fixture_creature_guid(1);
    insert_fixture_entity(ctx, &origin, first_tag, base_x, false, 0);
    crate::threat::add_threat(ctx, first_tag, LOOT_TAG_FIXTURE_CHARACTER_A, 10);
    crate::threat::add_threat(ctx, first_tag, LOOT_TAG_FIXTURE_CHARACTER_B, 100);
    expect_tagger(ctx, first_tag, LOOT_TAG_FIXTURE_CHARACTER_A)?;

    let aggression = fixture_creature_guid(2);
    insert_fixture_entity(ctx, &origin, aggression, base_x, false, 0);
    crate::combat::arm_creature_engagement(ctx, aggression, LOOT_TAG_FIXTURE_CHARACTER_A, false);
    if tagger(ctx, aggression).is_some() {
        return Err("creature aggression created a Loot Tag".to_string());
    }
    crate::combat::disengage(ctx, aggression);

    let pet = fixture_creature_guid(3);
    insert_fixture_entity(
        ctx,
        &origin,
        pet,
        base_x,
        false,
        LOOT_TAG_FIXTURE_CHARACTER_B,
    );
    let pet_target = fixture_creature_guid(4);
    insert_fixture_entity(ctx, &origin, pet_target, base_x, false, 0);
    crate::threat::add_threat(ctx, pet_target, pet, 10);
    expect_tagger(ctx, pet_target, LOOT_TAG_FIXTURE_CHARACTER_B)?;

    let tagless = fixture_creature_guid(5);
    insert_fixture_entity(ctx, &origin, tagless, base_x, false, 0);
    let mut tagless_live = ctx
        .db
        .game_world_entity()
        .guid()
        .find(tagless)
        .ok_or_else(|| "tag-less fixture creature is missing".to_string())?;
    tagless_live.money = 17;
    tagless_live.dynamic_flags |= lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
    tagless_live.update(ctx);
    ctx.db.game_corpse_loot().insert(super::CorpseLoot {
        id: 0,
        corpse_guid: tagless,
        slot: 0,
        item_entry: 25,
        count: 1,
        quest_only: false,
        reserved_for: 0,
        designated_looter_guid: 0,
        master_only: false,
        withheld: false,
        random_property_id: 0,
    });
    let tagless_xp = entity_xp(ctx, LOOT_TAG_FIXTURE_CHARACTER_B)?;
    if !crate::combat::kill_creature(ctx, tagless, Some(LOOT_TAG_FIXTURE_CHARACTER_B)) {
        return Err("tag-less fixture creature did not die".to_string());
    }
    let tagless_corpse = ctx
        .db
        .game_world_entity()
        .guid()
        .find(tagless)
        .ok_or_else(|| "tag-less corpse disappeared".to_string())?;
    if tagless_corpse.money != 0
        || tagless_corpse.dynamic_flags & lyracore_shared::constants::unit_dynamic_flags::LOOTABLE
            != 0
        || entity_xp(ctx, LOOT_TAG_FIXTURE_CHARACTER_B)? != tagless_xp
        || !corpse_eligible_recipients(ctx, tagless).is_empty()
        || ctx
            .db
            .game_corpse_loot()
            .by_corpse()
            .filter(&tagless)
            .next()
            .is_some()
    {
        return Err("tag-less death produced player rewards or loot".to_string());
    }

    let group = ctx.db.game_group().insert(crate::Group {
        group_id: LOOT_TAG_FIXTURE_GROUP,
        leader_guid: LOOT_TAG_FIXTURE_CHARACTER_A,
        loot_method: crate::group::loot_method::GROUP,
        loot_threshold: 2,
        rr_cursor: 0,
        master_looter_guid: 0,
        group_type: 0,
    });
    let members = ctx.db.game_group_member();
    for character_guid in [
        LOOT_TAG_FIXTURE_CHARACTER_A,
        LOOT_TAG_FIXTURE_CHARACTER_C,
        LOOT_TAG_FIXTURE_CHARACTER_D,
    ] {
        members.insert(crate::GroupMember {
            id: 0,
            group_id: group.group_id,
            character_guid,
            owner_identity: spacetimedb::Identity::ZERO,
            raid_slot: 0,
        });
    }
    let grouped = fixture_creature_guid(6);
    insert_fixture_entity(ctx, &origin, grouped, base_x, false, 0);
    crate::threat::add_threat(ctx, grouped, LOOT_TAG_FIXTURE_CHARACTER_A, 10);
    expect_live_loot_tag_eligibility(
        ctx,
        grouped,
        LOOT_TAG_FIXTURE_CHARACTER_C,
        LiveLootTagEligibility::Available,
    )?;
    members.insert(crate::GroupMember {
        id: 0,
        group_id: group.group_id,
        character_guid: LOOT_TAG_FIXTURE_CHARACTER_E,
        owner_identity: spacetimedb::Identity::ZERO,
        raid_slot: 0,
    });
    expect_live_loot_tag_eligibility(
        ctx,
        grouped,
        LOOT_TAG_FIXTURE_CHARACTER_E,
        LiveLootTagEligibility::Foreign,
    )?;
    crate::group::remove_member(
        ctx,
        LOOT_TAG_FIXTURE_CHARACTER_C,
        crate::group::Departure::Left,
    );
    members.insert(crate::GroupMember {
        id: 0,
        group_id: group.group_id,
        character_guid: LOOT_TAG_FIXTURE_CHARACTER_C,
        owner_identity: spacetimedb::Identity::ZERO,
        raid_slot: 0,
    });
    expect_live_loot_tag_eligibility(
        ctx,
        grouped,
        LOOT_TAG_FIXTURE_CHARACTER_C,
        LiveLootTagEligibility::Foreign,
    )?;
    move_fixture_entity(ctx, LOOT_TAG_FIXTURE_CHARACTER_D, base_x + 100.0)?;
    expect_live_loot_tag_eligibility(
        ctx,
        grouped,
        LOOT_TAG_FIXTURE_CHARACTER_D,
        LiveLootTagEligibility::Available,
    )?;
    let tagger_xp = entity_xp(ctx, LOOT_TAG_FIXTURE_CHARACTER_A)?;
    let foreign_xp = entity_xp(ctx, LOOT_TAG_FIXTURE_CHARACTER_B)?;
    if !crate::combat::kill_creature(ctx, grouped, Some(LOOT_TAG_FIXTURE_CHARACTER_B)) {
        return Err("grouped fixture creature did not die".to_string());
    }
    if corpse_eligible_recipients(ctx, grouped) != vec![LOOT_TAG_FIXTURE_CHARACTER_A]
        || entity_xp(ctx, LOOT_TAG_FIXTURE_CHARACTER_A)? <= tagger_xp
        || entity_xp(ctx, LOOT_TAG_FIXTURE_CHARACTER_B)? != foreign_xp
    {
        return Err(
            "tag-owned death rewarded a leaver who rejoined, later joiner, distant member, or foreign killer"
                .to_string(),
        );
    }

    let departed_tagger = fixture_creature_guid(14);
    insert_fixture_entity(ctx, &origin, departed_tagger, base_x, false, 0);
    crate::threat::add_threat(ctx, departed_tagger, LOOT_TAG_FIXTURE_CHARACTER_A, 10);
    crate::group::remove_member(
        ctx,
        LOOT_TAG_FIXTURE_CHARACTER_A,
        crate::group::Departure::Left,
    );
    expect_live_loot_tag_eligibility(
        ctx,
        departed_tagger,
        LOOT_TAG_FIXTURE_CHARACTER_E,
        LiveLootTagEligibility::Available,
    )?;
    crate::group::remove_member(
        ctx,
        LOOT_TAG_FIXTURE_CHARACTER_C,
        crate::group::Departure::Left,
    );
    crate::group::remove_member(
        ctx,
        LOOT_TAG_FIXTURE_CHARACTER_D,
        crate::group::Departure::Left,
    );
    revoke_group_member(ctx, LOOT_TAG_FIXTURE_GROUP, LOOT_TAG_FIXTURE_CHARACTER_E);
    expect_live_loot_tag_eligibility(
        ctx,
        departed_tagger,
        LOOT_TAG_FIXTURE_CHARACTER_E,
        LiveLootTagEligibility::Foreign,
    )?;

    let retained = fixture_creature_guid(7);
    insert_fixture_entity(ctx, &origin, retained, base_x, false, 0);
    crate::threat::add_threat(ctx, retained, LOOT_TAG_FIXTURE_CHARACTER_F, 10);
    crate::combat::arm_creature_engagement(ctx, retained, LOOT_TAG_FIXTURE_CHARACTER_F, false);
    crate::combat::MeleeAttack::insert(
        ctx,
        crate::MeleeAttack {
            attacker_guid: LOOT_TAG_FIXTURE_CHARACTER_B,
            target_guid: retained,
            last_swing_ms: 0,
            ranged_spell_id: 0,
            last_offhand_swing_ms: 0,
            rout_ends_ms: 0,
            pursuit_ends_ms: 0,
            leash_x: 0.0,
            leash_y: 0.0,
            swing_error: 0,
        },
    );
    crate::combat::kill_player(
        ctx,
        LOOT_TAG_FIXTURE_CHARACTER_F,
        LOOT_TAG_FIXTURE_CHARACTER_B,
    );
    expect_tagger(ctx, retained, LOOT_TAG_FIXTURE_CHARACTER_F)?;
    crate::combat::disengage(ctx, retained);
    if tagger(ctx, retained).is_some()
        || ctx
            .db
            .game_world_entity()
            .guid()
            .find(retained)
            .is_some_and(|entity| {
                entity.dynamic_flags
                    & (lyracore_shared::constants::unit_dynamic_flags::TAPPED
                        | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER)
                    != 0
            })
    {
        return Err("disengage retained the Loot Tag or stored tag flags".to_string());
    }

    let healed_target = fixture_creature_guid(8);
    insert_fixture_entity(ctx, &origin, healed_target, base_x, false, 0);
    crate::combat::arm_creature_engagement(ctx, healed_target, LOOT_TAG_FIXTURE_CHARACTER_A, false);
    crate::threat::add_heal_threat(
        ctx,
        LOOT_TAG_FIXTURE_CHARACTER_B,
        LOOT_TAG_FIXTURE_CHARACTER_A,
        20,
    );
    expect_tagger(ctx, healed_target, LOOT_TAG_FIXTURE_CHARACTER_B)?;

    let taunted = fixture_creature_guid(9);
    insert_fixture_entity(ctx, &origin, taunted, base_x, false, 0);
    crate::threat::taunt(ctx, taunted, LOOT_TAG_FIXTURE_CHARACTER_A);
    expect_tagger(ctx, taunted, LOOT_TAG_FIXTURE_CHARACTER_A)?;

    let lethal = fixture_creature_guid(10);
    insert_fixture_entity(ctx, &origin, lethal, base_x, false, 0);
    let damage = crate::combat::final_damage(ctx, lethal, u32::MAX);
    let outcome = crate::combat::apply_hit(
        ctx,
        LOOT_TAG_FIXTURE_CHARACTER_B,
        lethal,
        damage,
        crate::combat::Hit::weapon(crate::combat::HitSource::MainHand, false),
    );
    let lethal_corpse_has_tag_flags =
        ctx.db
            .game_world_entity()
            .guid()
            .find(lethal)
            .is_some_and(|entity| {
                entity.dynamic_flags
                    & (lyracore_shared::constants::unit_dynamic_flags::TAPPED
                        | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER)
                    != 0
            });
    if !outcome.killed
        || corpse_eligible_recipients(ctx, lethal) != vec![LOOT_TAG_FIXTURE_CHARACTER_B]
        || tagger(ctx, lethal).is_some()
        || lethal_corpse_has_tag_flags
    {
        return Err(
            "a lethal first hit missed eligibility or retained its live Loot Tag".to_string(),
        );
    }

    verify_corpse_loot_gates(ctx, &origin, base_x)?;

    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn verify_corpse_loot_gates(
    ctx: &ReducerContext,
    origin: &crate::CreatureTemplate,
    x: f32,
) -> Result<(), String> {
    const ITEM_ENTRY: u32 = crate::professions::LEATHER_ENTRY;
    if ctx
        .db
        .game_item_template()
        .entry()
        .find(ITEM_ENTRY)
        .is_none()
    {
        let mut template = ctx
            .db
            .game_item_template()
            .iter()
            .next()
            .ok_or_else(|| "Loot Tag fixture item template is missing".to_string())?;
        template.entry = ITEM_ENTRY;
        ctx.db.game_item_template().insert(template);
    }

    let empty = fixture_creature_guid(11);
    insert_fixture_corpse(ctx, origin, empty, x, 0);
    expect_loot_tag_refusal(corpse_access_gate(ctx, LOOT_TAG_FIXTURE_CHARACTER_A, empty))?;
    expect_loot_tag_refusal(crate::loot::apply_loot_money(
        ctx,
        LOOT_TAG_FIXTURE_CHARACTER_A,
        empty,
    ))?;

    let party_corpse = fixture_creature_guid(12);
    insert_fixture_corpse(ctx, origin, party_corpse, x, 9);
    ctx.db.game_corpse_loot().insert(super::CorpseLoot {
        id: 0,
        corpse_guid: party_corpse,
        slot: 0,
        item_entry: ITEM_ENTRY,
        count: 1,
        quest_only: false,
        reserved_for: 0,
        designated_looter_guid: 0,
        master_only: false,
        withheld: false,
        random_property_id: 0,
    });
    record_corpse_eligibility(
        ctx,
        party_corpse,
        &[LOOT_TAG_FIXTURE_CHARACTER_A, LOOT_TAG_FIXTURE_CHARACTER_B],
    );
    crate::loot::open_creature_corpse(ctx, LOOT_TAG_FIXTURE_CHARACTER_A, party_corpse)
        .map_err(|refusal| refusal.as_tag().to_string())?;
    crate::loot::open_creature_corpse(ctx, LOOT_TAG_FIXTURE_CHARACTER_B, party_corpse)
        .map_err(|refusal| refusal.as_tag().to_string())?;
    for refusal in [
        crate::loot::open_creature_corpse(ctx, LOOT_TAG_FIXTURE_CHARACTER_E, party_corpse)
            .map_err(|refusal| refusal.as_tag().to_string()),
        crate::items::apply_take_loot(ctx, LOOT_TAG_FIXTURE_CHARACTER_E, party_corpse, 0),
        crate::loot::apply_loot_money(ctx, LOOT_TAG_FIXTURE_CHARACTER_E, party_corpse),
        crate::professions::skin_corpse(ctx, LOOT_TAG_FIXTURE_CHARACTER_E, party_corpse)
            .map_err(String::from),
    ] {
        expect_loot_tag_refusal(refusal)?;
    }
    if ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&party_corpse)
        .next()
        .is_none()
        || ctx
            .db
            .game_world_entity()
            .guid()
            .find(party_corpse)
            .is_none_or(|corpse| corpse.money != 9 || corpse.skinned)
    {
        return Err("a Loot Tag Refusal changed the party corpse".to_string());
    }

    crate::items::apply_take_loot(ctx, LOOT_TAG_FIXTURE_CHARACTER_B, party_corpse, 0)?;
    crate::loot::apply_loot_money(ctx, LOOT_TAG_FIXTURE_CHARACTER_B, party_corpse)?;
    insert_fixture_skinning(ctx, LOOT_TAG_FIXTURE_CHARACTER_B);
    crate::professions::skin_corpse(ctx, LOOT_TAG_FIXTURE_CHARACTER_B, party_corpse)?;

    let solo_corpse = fixture_creature_guid(13);
    insert_fixture_corpse(ctx, origin, solo_corpse, x, 17);
    ctx.db.game_corpse_loot().insert(super::CorpseLoot {
        id: 0,
        corpse_guid: solo_corpse,
        slot: 0,
        item_entry: ITEM_ENTRY,
        count: 1,
        quest_only: false,
        reserved_for: 0,
        designated_looter_guid: 0,
        master_only: false,
        withheld: false,
        random_property_id: 0,
    });
    record_corpse_eligibility(ctx, solo_corpse, &[LOOT_TAG_FIXTURE_CHARACTER_A]);
    crate::loot::open_creature_corpse(ctx, LOOT_TAG_FIXTURE_CHARACTER_A, solo_corpse)
        .map_err(|refusal| refusal.as_tag().to_string())?;
    crate::items::apply_take_loot(ctx, LOOT_TAG_FIXTURE_CHARACTER_A, solo_corpse, 0)?;
    crate::loot::apply_loot_money(ctx, LOOT_TAG_FIXTURE_CHARACTER_A, solo_corpse)?;
    insert_fixture_skinning(ctx, LOOT_TAG_FIXTURE_CHARACTER_A);
    crate::professions::skin_corpse(ctx, LOOT_TAG_FIXTURE_CHARACTER_A, solo_corpse)?;
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn insert_fixture_corpse(
    ctx: &ReducerContext,
    template: &crate::CreatureTemplate,
    guid: u64,
    x: f32,
    money: u32,
) {
    insert_fixture_entity(ctx, template, guid, x, false, 0);
    let entities = ctx.db.game_world_entity();
    let mut corpse = entities
        .guid()
        .find(guid)
        .expect("fixture corpse was inserted");
    corpse.dead = true;
    corpse.health = 0;
    corpse.money = money;
    corpse.update(ctx);
}

#[cfg(feature = "debug_reducers")]
fn insert_fixture_skinning(ctx: &ReducerContext, character_guid: u64) {
    crate::PlayerSkill::insert(
        ctx,
        crate::PlayerSkill {
            id: 0,
            character_guid,
            owner_identity: spacetimedb::Identity::ZERO,
            skill_line: crate::skill::skill_line::SKINNING,
            current: 1,
            max_rank: 75,
        },
    );
}

#[cfg(feature = "debug_reducers")]
fn expect_loot_tag_refusal(result: Result<(), String>) -> Result<(), String> {
    match result {
        Err(reason) if reason == LootRefusal::LootTagIneligible.as_tag() => Ok(()),
        Ok(()) => Err("foreign Actor passed the Loot Tag Gate".to_string()),
        Err(reason) => Err(format!("unexpected Loot Tag Refusal: {reason}")),
    }
}

#[cfg(feature = "debug_reducers")]
fn expect_live_loot_tag_eligibility(
    ctx: &ReducerContext,
    creature_guid: u64,
    character_guid: u64,
    expected: LiveLootTagEligibility,
) -> Result<(), String> {
    let actual = live_loot_tag_eligibility(ctx, creature_guid, character_guid);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "live Loot Tag eligibility for {character_guid} on {creature_guid} was {actual:?}, expected {expected:?}"
        ))
    }
}

#[cfg(feature = "debug_reducers")]
fn fixture_creature_guid(low: u64) -> u64 {
    (0xF130_u64 << 48) | (u64::from(LOOT_TAG_FIXTURE_ENTRY) << 24) | 0x00FE_0000 | low
}

#[cfg(feature = "debug_reducers")]
fn insert_fixture_entity(
    ctx: &ReducerContext,
    template: &crate::CreatureTemplate,
    guid: u64,
    x: f32,
    is_character: bool,
    owner_guid: u64,
) {
    let spawn = crate::CreatureSpawn {
        guid,
        entry: template.entry,
        map_id: 0,
        x,
        y: 0.0,
        z: 0.0,
        orientation: 0.0,
        respawn_at: crate::creatures::timer_never(ctx),
        despawn_at: crate::creatures::timer_never(ctx),
        movement_type: crate::creatures::MOVEMENT_IDLE,
        respawn_secs: 0,
        life_seq: 0,
        source_guid: 0,
        instance_id: 0,
    };
    let mut entity = crate::creatures::build_creature_entity(ctx, &spawn, template, 0, 0);
    entity.owner_guid = owner_guid;
    if is_character {
        entity.type_mask = lyracore_shared::constants::type_mask::PLAYER;
        entity.entry = 0;
        entity.account_id = guid;
        entity.faction_template = 1;
        entity.next_level_xp = crate::xp::xp_to_next_level(1);
    }
    entity.insert(ctx);
}

#[cfg(feature = "debug_reducers")]
fn move_fixture_entity(ctx: &ReducerContext, guid: u64, x: f32) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut entity = entities
        .guid()
        .find(guid)
        .ok_or_else(|| format!("Loot Tag fixture entity {guid} is missing"))?;
    let (y, z) = (entity.y, entity.z);
    entity
        .place(x, y, z, crate::world::MoveClock::Unchanged)
        .unexplored();
    entity.update(ctx);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn tagger(ctx: &ReducerContext, creature_guid: u64) -> Option<u64> {
    ctx.db
        .game_creature_quest_tap()
        .creature_guid()
        .find(creature_guid)
        .map(|tag| tag.character_guid)
}

#[cfg(feature = "debug_reducers")]
fn expect_tagger(ctx: &ReducerContext, creature_guid: u64, expected: u64) -> Result<(), String> {
    if tagger(ctx, creature_guid) != Some(expected) {
        return Err(format!(
            "creature {creature_guid} did not retain first tagger {expected}"
        ));
    }
    let entity = ctx
        .db
        .game_world_entity()
        .guid()
        .find(creature_guid)
        .ok_or_else(|| format!("tagged creature {creature_guid} is missing"))?;
    if entity.dynamic_flags & lyracore_shared::constants::unit_dynamic_flags::TAPPED == 0
        || entity.dynamic_flags & lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER
            != 0
    {
        return Err(format!(
            "creature {creature_guid} stored the wrong Loot Tag dynamic flags"
        ));
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
fn entity_xp(ctx: &ReducerContext, guid: u64) -> Result<u32, String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .map(|entity| entity.xp)
        .ok_or_else(|| format!("Loot Tag fixture Character {guid} is missing"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpse_access_requires_a_resolved_eligibility_row() {
        assert!(!corpse_eligible_for_access(&[], 7));
        assert!(corpse_eligible_for_access(&[7], 7));
        assert!(!corpse_eligible_for_access(&[7], 8));
        assert!(corpse_eligible_for_access(&[2, 7, 11], 7));
    }
}
