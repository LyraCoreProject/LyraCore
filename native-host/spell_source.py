import struct

from headless import cast


def check_spell_source_behavior(host):
    caster = host.enter_world('Sourcecaster', cls=8, at=(0, 8000, 9000, 0))
    peer = host.enter_world('Sourcetarget', cls=8, at=(0, 8005, 9000, 0))
    a, b = caster.guid, peer.guid
    next_id = 5099500

    def spell(flags=0, kind=0xbe, amount=1, coefficient=-1, **fields):
        nonlocal next_id
        next_id += 1
        sid = next_id
        header = dict(spell_id=sid, name=f"'Source {sid}'", power_type=0, cost=0,
                      cast_time_ms=0, gcd_ms=0, cooldown_ms=0, range_yd=0, duration_ms=60000,
                      school_mask=1, dispel_type=0, mechanic=0, max_stacks=5, aura_interrupt=0,
                      attributes=0, spell_level=0, max_level=0, is_negative='false', cast_flags=0,
                      stances=0, family_name=0, family_flags=0, proc_flags=0, proc_chance=0,
                      proc_charges=0, custom_flags=flags, min_target_level=0, single_target_key=1)
        header.update(fields)
        effect = dict(id=sid*4, spell_id=sid, effect_index=0, kind=kind, base_points=amount,
                      die_sides=0, per_level=0, period_ms=0, target=3, radius_yd=0,
                      chain_targets=0, trigger_spell=0, effect_mechanic=0, p0=0, p0_kind=0,
                      p1=0, script_id=0, enters_combat='false', bonus_coefficient=coefficient)
        for table, row in [('game_spell', header), ('game_spell_effect', effect)]:
            host.restore(table, f'spell_id = {sid}')
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

    heal = spell(kind=2, amount=10, coefficient=0.5)
    fixed_heal = spell(flags=0x10, kind=2, amount=10, coefficient=0.5)
    for sid, restored in [(heal, 60), (fixed_heal, 10)]:
        host.sql(f'UPDATE game_world_entity SET health = 100, max_health = 10000 WHERE guid = {b}')
        apply(sid)
        assert host.rows(f'SELECT health FROM game_world_entity WHERE guid = {b}') == [[100 + restored]]
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
    host.call('debug_engage', [wolf, a])
    host.call('debug_teleport', [a, 0, 9000, 9000, 0, 0])
    host.wait_until(lambda: not auras(ordinary), 'evade did not clear the ordinary aura')
    assert auras(persistent), 'evade cleared the preserved aura'
