import struct

from headless import cast, spell_id, spell_go_misses


def check_spell_source_behavior(host):
    caster = host.enter_world('Sourcecaster', cls=8, at=(0, 8000, 9000, 0))
    peer = host.enter_world('Sourcetarget', cls=8, at=(0, 8005, 9000, 0))
    a, b = caster.guid, peer.guid
    next_id = 5099500

    def spell(flags=0, kind=0xbe, amount=1, coefficient=-1, **fields):
        nonlocal next_id
        next_id += 1
        sid = fields.get('spell_id', next_id)
        header = dict(spell_id=sid, name=f"'Source {sid}'", power_type=0, cost=0,
                      cast_time_ms=0, gcd_ms=0, cooldown_ms=0, range_yd=0, duration_ms=60000,
                      school_mask=1, dispel_type=0, mechanic=0, max_stacks=5, aura_interrupt=0,
                      attributes=0, spell_level=0, max_level=0, is_negative='false', cast_flags=0,
                      stances=0, family_name=0, family_flags=0, proc_flags=0, proc_chance=0,
                      proc_charges=0, max_duration_ms=0, custom_flags=flags, min_target_level=0, single_target_key=1)
        header.update(fields)
        effect = dict(id=sid*4, spell_id=sid, effect_index=0, kind=kind, base_points=amount,
                      die_sides=0, per_level=0, period_ms=0, target=3, radius_yd=0,
                      chain_targets=0, trigger_spell=0, effect_mechanic=0, p0=0, p0_kind=0,
                      p1=0, script_id=0, enters_combat='false', points_per_combo=0, bonus_coefficient=coefficient)
        for table, row in [('game_spell', header), ('game_spell_effect', effect)]:
            host.restore(table, f'spell_id = {sid}')
            host.sql(f'DELETE FROM {table} WHERE spell_id = {sid}')
            host.sql(f"INSERT INTO {table} ({','.join(row)}) VALUES ({','.join(str(x) for x in row.values())})")
        return sid

    def apply(sid, source=a, target=b):
        host.call('debug_force_cast_at', [source, sid, target])

    def auras(sid):
        return host.rows(f'SELECT caster_guid,target_guid,slot,stacks,amount FROM game_aura WHERE spell_id = {sid}')

    def refused(sid, text):
        try:
            apply(sid)
        except RuntimeError as error:
            assert text in str(error), str(error)
        else:
            raise AssertionError(f'spell {sid} passed its Gate')

    shared = spell(flags=1)
    apply(shared)
    apply(shared, source=b)
    assert len(auras(shared)) == 1 and auras(shared)[0][3] == 2, auras(shared)
    separate = spell(flags=0x1000)
    apply(separate)
    apply(separate, source=b)
    rows = auras(separate)
    assert len(rows) == 2 and len({row[2] for row in rows}) == 2, rows
    for flags, negative in [(2, True), (4, False), (6, False)]:
        sid = spell(flags=flags, is_negative='true' if flags & 4 else 'false')
        apply(sid)
        assert (auras(sid)[0][2] >= 32) == negative, auras(sid)

    single = spell(flags=0x100)
    apply(single)
    apply(single, target=a)
    assert [row[1] for row in auras(single)] == [a], auras(single)
    stealth = spell(kind=0xa8)
    apply(stealth, source=b)
    assert auras(stealth)
    breaks = spell(flags=0x200)
    apply(breaks)
    assert not auras(stealth)

    host.call('debug_teleport', [a, 0, 8000, 9000, 0, 3.14159265])
    facing = spell(flags=0x80)
    refused(facing, 'front')
    host.call('debug_teleport', [a, 0, 8000, 9000, 0, 0])
    apply(facing)
    behind = spell(flags=0x40)
    host.call('debug_teleport', [b, 0, 8005, 9000, 0, 3.14159265])
    refused(behind, 'BEHIND')
    host.call('debug_teleport', [b, 0, 8005, 9000, 0, 0])
    apply(behind)

    minimum = spell(min_target_level=10, cost=3)
    host.sql(f'UPDATE game_world_entity SET level = 9, power = 100 WHERE guid = {b}')
    host.sql(f'UPDATE game_world_entity SET power = 100 WHERE guid = {a}')
    host.call('debug_learn_spell', [a, minimum])
    caster.collect()
    caster.send(0x12e, cast(minimum, b))
    packets = caster.collect()
    assert any(op == 0x130 and body == struct.pack('<IBB', minimum, 2, 0x2b) for op, body in packets), packets
    assert host.rows(f'SELECT power FROM game_world_entity WHERE guid = {a}') == [[100]]
    assert not auras(minimum)
    host.sql(f'UPDATE game_world_entity SET level = 10 WHERE guid = {b}')
    apply(minimum)

    host.sql(f'UPDATE game_world_entity SET intellect = 520 WHERE guid = {a}')
    shared_heal = spell(flags=1, kind=0x91, amount=10, coefficient=0.5)
    apply(shared_heal)
    apply(shared_heal, source=b)
    assert len(auras(shared_heal)) == 1 and auras(shared_heal)[0][0] == a and auras(shared_heal)[0][3:] == [2, 60], auras(shared_heal)

    unit_flags, combat_active, combat_until_ms = host.rows(f'SELECT unit_flags, combat_active, combat_until_ms FROM game_world_entity WHERE guid = {b}')[0]
    host.sql(f'UPDATE game_world_entity SET unit_flags = {unit_flags | 0x80000}, combat_active = true, combat_until_ms = 9999999999999 WHERE guid = {b}')
    heal = spell(kind=2, amount=10, coefficient=0.5)
    fixed_heal = spell(flags=0x10, kind=2, amount=10, coefficient=0.5)
    for sid, restored in [(heal, 60), (fixed_heal, 10)]:
        host.sql(f'UPDATE game_world_entity SET health = 100, max_health = 10000 WHERE guid = {b}')
        apply(sid)
        assert host.rows(f'SELECT health FROM game_world_entity WHERE guid = {b}') == [[100 + restored]]
    host.sql(f'UPDATE game_world_entity SET unit_flags = {unit_flags}, combat_active = {str(combat_active).lower()}, combat_until_ms = {combat_until_ms} WHERE guid = {b}')
    periodic = spell(kind=0x91, amount=10, coefficient=0.25)
    apply(periodic)
    assert auras(periodic)[0][4] == 35, auras(periodic)

    for flags in [0, 8]:
        channel = spell(flags=flags, kind=0x91, cast_flags=0x80, range_yd=10)
        host.sql(f'UPDATE game_spell_effect SET period_ms = 100 WHERE spell_id = {channel}')
        host.call('debug_teleport', [b, 0, 8100, 9000, 0, 0])
        refused(channel, 'range')
        host.call('debug_teleport', [b, 0, 8005, 9000, 0, 0])
        apply(channel)
        host.call('debug_teleport', [b, 0, 8100, 9000, 0, 0])
        if flags:
            before = host.rows(f'SELECT next_tick_micros FROM game_aura WHERE spell_id = {channel}')[0][0]
            host.wait_until(lambda: host.rows(f'SELECT next_tick_micros FROM game_aura WHERE spell_id = {channel}')[0][0] > before, 'unlimited channel stopped')
        else:
            host.wait_until(lambda: not auras(channel), 'ordinary channel survived out of range')
    host.call('debug_teleport', [b, 0, 8005, 9000, 0, 0])

    baseline = spell(kind=0xa0, amount=0)
    host.sql(f'UPDATE game_spell_effect SET p0_kind = 1, p0 = 2 WHERE spell_id = {baseline}')
    apply(baseline)
    for flags in [0, 8]:
        channel = spell(flags=flags, kind=0xa0, amount=10, cast_flags=0x80, range_yd=10)
        host.sql(f'UPDATE game_spell_effect SET p0_kind = 1, p0 = 2 WHERE spell_id = {channel}')
        before_max = host.rows(f'SELECT max_health FROM game_world_entity WHERE guid = {b}')[0][0]
        host.call('debug_begin_cast', [a, channel, b])
        assert auras(channel)
        host.call('debug_teleport', [b, 0, 8100, 9000, 0, 0])
        if flags:
            host.call('debug_begin_cast', [b, spell(), b])
            assert auras(channel)
        else:
            host.wait_until(lambda: not auras(channel), 'nonperiodic channel survived out of range')
            assert host.rows(f'SELECT max_health FROM game_world_entity WHERE guid = {b}') == [[before_max]], (before_max, host.rows(f'SELECT max_health FROM game_world_entity WHERE guid = {b}'))
        host.call('debug_teleport', [b, 0, 8005, 9000, 0, 0])

    for flags, turner in [(0x80, a), (0x40, b)]:
        delayed = spell(flags=flags, cast_time_ms=500, cost=3)
        host.call('debug_teleport', [a, 0, 8000, 9000, 0, 0])
        host.call('debug_teleport', [b, 0, 8005, 9000, 0, 0])
        host.sql(f'UPDATE game_world_entity SET power = 100, max_power = 100 WHERE guid = {a}')
        host.call('debug_learn_spell', [a, delayed])
        caster.collect()
        caster.send(0x12e, cast(delayed, b))
        pending = f'SELECT scheduled_id FROM game_pending_cast WHERE caster_guid = {a}'
        host.wait_until(lambda: host.rows(pending), 'cast did not start')
        host.call('debug_teleport', [turner, 0, 8000 if turner == a else 8005, 9000, 0, 3.14159265])
        assert host.rows(pending), 'turning cancelled before completion'
        host.wait_until(lambda: not host.rows(pending), 'cast did not complete')
        assert not auras(delayed), 'completion ignored the facing requirement'
        assert host.rows(f'SELECT power FROM game_world_entity WHERE guid = {a}') == [[100]]
        packets = caster.collect()
        assert any(op == 0x130 and body[:5] == struct.pack('<IB', delayed, 2) for op, body in packets), packets

    original_factions = {guid: host.rows(f'SELECT faction_template FROM game_world_entity WHERE guid = {guid}')[0][0] for guid in [a, b]}
    for faction, group, enemy in [(50908, 1, 2), (50909, 2, 1)]:
        host.restore('game_faction_template', f'id = {faction}')
        host.sql(f'DELETE FROM game_faction_template WHERE id = {faction}')
        host.sql(f'INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES ({faction},{faction},{group},{group},{enemy},0,0,0,0,0,0,0,0)')
    host.sql(f'UPDATE game_world_entity SET faction_template = 50908 WHERE guid = {a}')
    for target_kind, refused_faction, admitted_faction in [(2, 50909, 50908), (1, 50908, 50909)]:
        triggered_aura = spell(kind=0xa0, is_negative='true' if target_kind == 1 else 'false')
        host.sql(f'UPDATE game_spell_effect SET target = {target_kind}, p0_kind = 1, p0 = 2 WHERE spell_id = {triggered_aura}')
        holder = spell(kind=0x93)
        host.sql(f'UPDATE game_spell_effect SET target = 0, period_ms = 100, trigger_spell = {triggered_aura} WHERE spell_id = {holder}')
        host.sql(f'UPDATE game_world_entity SET faction_template = {refused_faction} WHERE guid = {b}')
        host.call('debug_begin_cast', [a, holder, b])
        first_tick = host.rows(f'SELECT next_tick_micros FROM game_aura WHERE spell_id = {holder}')[0][0]
        host.wait_until(lambda: host.rows(f'SELECT next_tick_micros FROM game_aura WHERE spell_id = {holder}')[0][0] > first_tick + 100000,
                        'periodic trigger did not run')
        assert not auras(triggered_aura), 'triggered cast ignored faction targeting'
        host.sql(f'UPDATE game_world_entity SET faction_template = {admitted_faction} WHERE guid = {b}')
        host.wait_until(lambda: auras(triggered_aura), 'eligible triggered aura did not land')
        host.sql(f'DELETE FROM game_aura WHERE spell_id = {holder}')
        host.sql(f'DELETE FROM game_aura WHERE spell_id = {triggered_aura}')
    for guid, faction in original_factions.items():
        host.sql(f'UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}')

    host.sql(f'DELETE FROM game_aura WHERE caster_guid = {a}')
    for channeled in [0x80, 0]:
        missile = spell(flags=0xc0, kind=2, amount=1000, coefficient=0, range_yd=1, cost=1000, gcd_ms=60000, cooldown_ms=60000)
        channel = spell(flags=0x80, kind=0x93, cast_flags=channeled, range_yd=10)
        host.sql(f'UPDATE game_spell_effect SET target = 0, period_ms = 100, trigger_spell = {missile} WHERE spell_id = {channel}')
        host.call('debug_teleport', [a, 0, 8000, 9000, 0, 0])
        host.call('debug_teleport', [b, 0, 8008, 9000, 0, 0])
        host.call('debug_begin_cast', [a, channel, b])
        host.call('debug_teleport', [a, 0, 8000, 9000, 0, 3.14159265])
        host.call('debug_teleport', [b, 0, 8008, 9000, 0, 3.14159265])
        host.sql(f'UPDATE game_world_entity SET health = 100, max_health = 10000 WHERE guid = {b}')
        host.wait_until(lambda: host.rows(f'SELECT health FROM game_world_entity WHERE guid = {b}')[0][0] >= 2100,
                        'turning stopped aura-triggered missiles')
        assert auras(channel), 'turning ended the channel'
        assert not host.rows(f'SELECT id FROM game_spell_cd WHERE caster_guid = {a} AND spell_id = {missile}')
        assert host.rows(f'SELECT power FROM game_world_entity WHERE guid = {a}') == [[100]]
        host.sql(f'DELETE FROM game_aura WHERE spell_id = {channel}')

    missile = spell(spell_id=7268, name="'Arcane Missile'", flags=0x80, kind=1, amount=24,
                    coefficient=0.132, range_yd=30, school_mask=64)
    missiles = spell(spell_id=5143, name="'Arcane Missiles'", flags=0x80, kind=0x93,
                     cast_flags=0x80, range_yd=30, duration_ms=3000, cost=85, school_mask=64)
    host.sql(f'UPDATE game_spell_effect SET target = 1 WHERE spell_id = {missile}')
    host.sql(f'UPDATE game_spell_effect SET target = 0, period_ms = 1000, trigger_spell = {missile} WHERE spell_id = {missiles}')
    before = {row[0] for row in host.rows('SELECT guid FROM game_world_entity WHERE entry = 51000')}
    host.call('debug_teleport', [a, 0, 8000, 9000, 0, 0])
    host.call('debug_spawn_at_feet', [a, 51000, 28])
    victim = ({row[0] for row in host.rows('SELECT guid FROM game_world_entity WHERE entry = 51000')} - before).pop()
    host.sql(f'UPDATE game_world_entity SET health = 10000, max_health = 10000 WHERE guid = {victim}')
    caster.collect()
    host.call('debug_begin_cast', [a, missiles, victim])
    host.call('debug_teleport', [a, 0, 8000, 9000, 0, 3.14159265])
    packets = []

    def missile_ticks_arrived():
        packets.extend(caster.collect())
        return sum(op == 0x132 and spell_id(body) == 7268 for op, body in packets) >= 2

    host.wait_until(missile_ticks_arrived, 'turning stopped Arcane Missile 7268')
    if any(op == 0x132 and spell_id(body) == 7268 and not spell_go_misses(body)
           for op, body in packets):
        host.wait_until(lambda: host.rows(f'SELECT health FROM game_world_entity WHERE guid = {victim}')[0][0] < 10000,
                        'a launched Arcane Missile did not deal damage')
    host.sql(f'DELETE FROM game_aura WHERE spell_id = {missiles}')

    area_channel = spell(kind=0x91, amount=1000, coefficient=0, cast_flags=0x80, range_yd=1)
    host.sql(f'UPDATE game_spell_effect SET target = 5, radius_yd = 10, period_ms = 100 WHERE spell_id = {area_channel}')
    host.sql(f'UPDATE game_world_entity SET health = 100, max_health = 10000 WHERE guid = {b}')
    host.call('debug_begin_cast', [a, area_channel, a])
    host.wait_until(lambda: host.rows(f'SELECT health FROM game_world_entity WHERE guid = {b}')[0][0] >= 2100,
                    'a secondary area recipient broke the self-range channel')
    assert any(row[1] == b for row in auras(area_channel)), auras(area_channel)
    host.sql(f'DELETE FROM game_aura WHERE spell_id = {area_channel}')

    before = {row[0] for row in host.rows('SELECT guid FROM game_world_entity WHERE entry = 51000')}
    host.call('debug_spawn_at_feet', [a, 51000, 4])
    wolf = ({row[0] for row in host.rows('SELECT guid FROM game_world_entity WHERE entry = 51000')} - before).pop()
    host.sql(f'UPDATE game_world_entity SET health = 10000, max_health = 10000, armor = 100000, level = 5 WHERE guid = {wolf}')
    for flags, expected in [(0x10, {50, 75}), (0x30, {200, 300})]:
        damage = spell(flags=flags, kind=1, amount=200, coefficient=0.5)
        for _ in range(10):
            host.sql(f'UPDATE game_world_entity SET health = 10000 WHERE guid = {wolf}')
            apply(damage, target=wolf)
            dealt = 10000 - host.rows(f'SELECT health FROM game_world_entity WHERE guid = {wolf}')[0][0]
            if dealt:
                assert dealt in expected, (flags, dealt)
                break
        else:
            raise AssertionError('all damage casts missed')
    ordinary = spell()
    persistent = spell(flags=0x400)
    apply(ordinary, source=wolf, target=wolf)
    apply(persistent, source=wolf, target=wolf)
    player_buff = spell()
    permanent_buff = spell(duration_ms=4294967295)
    player_debuff = spell(is_negative='true')
    for sid in [player_buff, permanent_buff, player_debuff]:
        apply(sid, target=wolf)
    host.call('debug_engage', [wolf, a])
    host.call('debug_teleport', [a, 0, 9000, 9000, 0, 0])
    host.wait_until(lambda: not auras(ordinary), 'evade did not clear the ordinary aura')
    assert auras(persistent), 'evade cleared the preserved aura'
    assert auras(player_buff), 'evade cleared a finite player buff'
    assert not auras(permanent_buff), 'evade kept a permanent player buff'
    assert not auras(player_debuff), 'evade kept a player debuff'
