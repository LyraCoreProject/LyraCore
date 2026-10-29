#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import resource
import socket
import struct
import sys
import threading
import time
import traceback

import admission
from eventai_speech import check_eventai_speech
from headless import (AUTH_CHALLENGE, Logon, cast, creates, destroys, item_pointer, packed_guid, read_exact,
                      spell_go_misses, spell_id, teleport_ack, values)
from host import MODULE, Host
from operator_items import check_operator_items
from operator_creatures import check_operator_creatures
import reload
import retention
from spell_source import check_spell_source_behavior

# The guid of the seeded Character Tester, on the TEST Account.
TESTER = 1


def native_and_peer(host, at=(0, 20, 0, 0), grouped=False):
    """Enters the Character Native at `at` (map, x, y, z) and the mage Peer at x = 10 on map 0.
    `grouped` makes Native invite Peer into a group, and Peer accept."""
    client = host.enter_world('Native', at=at)
    peer = host.enter_world('Peer', cls=8, at=(0, 10, 0, 0))
    if grouped:
        client.send(0x6e, b'Peer\0')
        client.collect()
        peer.send(0x72)
    client.collect()
    peer.collect()
    return client, peer


def spawn(host, near, entry, distance):
    """Spawns one creature `distance` yards from the Character `near` and returns its guid."""
    query = f'SELECT guid FROM game_world_entity WHERE entry = {entry}'
    before = {row[0] for row in host.rows(query)}
    host.call('debug_spawn_at_feet', [near, entry, distance])
    spawned = {row[0] for row in host.rows(query)} - before
    assert len(spawned) == 1, spawned
    return spawned.pop()


def terrain_chunk(host, cell_x, cell_y, area, liquid_level=0):
    """Replaces the flat terrain chunk at (cell_x, cell_y) on map 0; liquid_level 0 means no liquid."""
    cell = f'map_id = 0 AND cell_x = {cell_x} AND cell_y = {cell_y}'
    host.restore('game_terrain_chunk', cell)
    host.sql(f'DELETE FROM game_terrain_chunk WHERE {cell}')
    liquid = f'{liquid_level},1' if liquid_level else '0,0'
    host.call('import_terrain_chunks_append', [f'0,{cell_x},{cell_y},{liquid},0,{area},' + ':'.join(['0'] * 145)])


def ignored_requests(client):
    for opcode in [0x0008, 0x0009, 0x00c6, 0x00d9, 0x00e1, 0x02ad, 0x02ca, 0x02f8,
                   0x000a, 0x0020, 0x01f4, 0x020b, 0x022f, 0x0331, 0x0334]:
        client.send(opcode, bytes([0xff]) * 32)
    client.send(0x0008, bytes([0xff]) * (0xffff - 4))
    assert client.collect() == []


def acknowledge_controls(host, client, peer, packets):
    guid = client.guid
    expected = []
    for opcode, body in packets:
        if opcode not in (0xe8, 0xea, 0xde, 0xdf):
            continue
        counter = struct.unpack_from('<I', body, 1 + body[0].bit_count())[0]
        x, y, z, orientation, timestamp = host.rows(f'SELECT x,y,z,orientation,last_move_ms FROM game_world_entity WHERE guid = {guid}')[0]
        movement = struct.pack('<IIffffI', 0, (timestamp + 1) & 0xffffffff, x, y, z, orientation, 0)
        request, relay = {0xe8: (0xe9, 0xec), 0xea: (0xeb, 0xed), 0xde: (0x2d0, 0x2b1), 0xdf: (0x2d0, 0x2b1)}[opcode]
        trailer = struct.pack('<I', int(opcode == 0xde)) if opcode in (0xde, 0xdf) else b''
        client.send(request, struct.pack('<QI', guid, counter) + movement + trailer)
        client.collect()
        expected.append(relay)
    observed = []
    for _ in range(40):
        observed += peer.collect()
        if all(any(opcode == relay and packed_guid(body) == guid for opcode, body in observed) for relay in expected):
            return observed
        time.sleep(0.01)
    raise AssertionError(('missing movement receipt Relay', expected, [hex(opcode) for opcode, _ in observed]))


def check_creature_catalogue(host):
    client = host.enter_world('Catalogue', at=(0, 8000, 8000, 0))
    entry = 5090042
    for table in ['game_creature_template', 'game_creature_level_stats', 'game_creature_display']:
        host.restore(table, f'entry = {entry}')
    host.sql(f"INSERT INTO game_creature_template (entry,name,subname,display_id,level,health,faction_template,npc_flags,unit_flags,creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,money_min,money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class) VALUES ({entry},'Catalogue fixture','',100,10,152,35,0,0,1,0,0,3,1,2000,0,0,10,152,0,20,30,50,0,0,0,0)")
    host.sql(f'INSERT INTO game_creature_level_stats (id,entry,level,health,damage_min,damage_max,armor) VALUES ({(entry << 8) | 10},{entry},10,152,20,30,50)')
    host.sql(f'INSERT INTO game_creature_display (id,entry,display_id,scale,weight) VALUES ({entry << 3},{entry},100,1,0),({(entry << 3) + 1},{entry},200,2,1)')
    guid = spawn(host, client.guid, entry, 5)
    assert host.rows(f'SELECT health,max_health,armor,display_id,scale_x FROM game_world_entity WHERE guid = {guid}') == [[152,152,50,200,2]]
    for imported in [True, False]:
        if not imported:
            host.sql(f'DELETE FROM game_creature_level_stats WHERE entry = {entry}')
        for edits, health, damage in [
            ('damage_min = 40, damage_max = 40', 152, 40),
            ('level = 11, max_level = 11', 152, 40),
            ('max_level = 0', 152, 40),
            ('health = 250, max_level_health = 250', 250, 40),
        ]:
            host.sql(f'UPDATE game_creature_template SET {edits} WHERE entry = {entry}')
            next_guid = spawn(host, client.guid, entry, 10)
            expected_health = health if imported else health * 4
            expected_damage = damage if imported else damage * 5 // 2
            assert host.rows(f'SELECT health FROM game_world_entity WHERE guid = {next_guid}') == [[expected_health]]
            host.call('debug_compute_swing', [next_guid, client.guid])
            assert host.rows("SELECT base_min,base_max FROM game_debug_readout WHERE key = 'swing'") == [[expected_damage, expected_damage]]
        host.sql(f'UPDATE game_creature_template SET level = 10, max_level = 10, health = 152, max_level_health = 152 WHERE entry = {entry}')


def check_pet_level_scaling(host):
    entry = 51006
    host.restore('game_creature_template', f'entry = {entry}')
    host.restore('game_creature_level_stats', f'entry = {entry}')
    host.sql(f'UPDATE game_creature_template SET creature_family = 5, level = 10, max_level = 10, health = 100, max_level_health = 100, npc_flags = 0, rank = 3, damage_min = 20, damage_max = 30 WHERE entry = {entry}')
    for source, name, expected in [('vmangos', 'Petimport', (22, 33)), ('classic-db', 'Petclassic', (55, 82))]:
        host.sql(f'DELETE FROM game_creature_level_stats WHERE entry = {entry}')
        if source == 'vmangos':
            host.sql(f'INSERT INTO game_creature_level_stats (id,entry,level,health,damage_min,damage_max,armor) VALUES ({(entry << 8) | 10},{entry},10,100,20,30,0)')
        owner = host.enter_world(name, 3, 3, at=(0, 8000, 8000, 0))
        host.call('debug_set_level', [owner.guid, 11])
        wild = spawn(host, owner.guid, entry, 5)
        host.call('debug_force_cast_at', [owner.guid, 50300, wild])
        pet, threshold = host.rows(f'SELECT live_pet_guid,next_level_xp FROM game_hunter_pet_protocol WHERE owner_guid = {owner.guid}')[0]
        host.sql(f'UPDATE game_hunter_pet SET pet_xp = {threshold - 1} WHERE owner_guid = {owner.guid}')
        prey = spawn(host, owner.guid, 51000, 5)
        host.sql(f'UPDATE game_world_entity SET level = 11 WHERE guid = {prey}')
        host.call('debug_apply_damage', [prey, 1, owner.guid])
        host.call('debug_kill_creature', [owner.guid, prey])
        assert host.rows(f'SELECT level FROM game_world_entity WHERE guid = {pet}') == [[11]], (source, host.rows(f'SELECT level,pet_xp FROM game_hunter_pet WHERE owner_guid = {owner.guid}'))
        host.call('debug_compute_swing', [pet, owner.guid])
        assert host.rows("SELECT base_min,base_max FROM game_debug_readout WHERE key = 'swing'") == [list(expected)], source


def check_sessionless_movement(host):
    guid = TESTER
    host.call('debug_spawn_player_entity', [guid])
    assert not host.rows(f'SELECT character_guid FROM game_native_player WHERE character_guid = {guid}')
    host.call('debug_teleport', [guid, 0, 6000, 1000, 0, 0])
    host.call('debug_creature_path', [guid, 6005, 1000, 0, False])
    started = host.rows(f'SELECT last_move_ms FROM game_world_entity WHERE guid = {guid}')[0][0]
    advanced = False
    for _ in range(100):
        time.sleep(0.05)
        x, moved = host.rows(f'SELECT x,last_move_ms FROM game_world_entity WHERE guid = {guid}')[0]
        if 6000 < x < 6005 and moved != started:
            advanced = True
        if not host.rows(f'SELECT guid FROM game_creature_spline WHERE guid = {guid}'):
            break
    assert advanced, ('sessionless movement did not record the Realm Clock', x, moved, started)
    x, moved = host.rows(f'SELECT x,last_move_ms FROM game_world_entity WHERE guid = {guid}')[0]
    assert x == 6005 and moved != started, ('sessionless arrival did not record the Realm Clock', x, moved, started)


def check_charge_movement(host):
    client, peer = native_and_peer(host)
    guid, peer_guid = client.guid, peer.guid
    spell = 5099100
    host.sql(f"INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES ({spell},'Native Charge',0,0,0,0,0,25,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0)")
    host.sql(f'INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES ({spell * 4},{spell},0,9,0,0,0.0,0,1,0.0,0,0,0,0,0,0,0,false,0.0)')
    for target, x in [(guid, 10), (peer_guid, 11)]:
        host.call('debug_teleport', [target, 0, x, 0, 0, 0])
    client.collect()
    peer.collect()
    host.call('debug_learn_spell', [guid, spell])
    client.send(0xee, struct.pack('<IIffffI', 0, 3_600_000, 16, 0, 0, 0, 0))
    client.collect()
    peer.collect()
    target = spawn(host, guid, 51000, 15)
    client.send(0x12e, cast(spell, target))
    cast_packets = client.collect()
    assert any(opcode == 0x132 and spell_id(body) == spell for opcode, body in cast_packets), [(hex(opcode), body.hex()) for opcode, body in cast_packets]
    time.sleep(1.5)
    client.collect()
    peer.collect()
    x, y, z, orientation = host.rows(f'SELECT x,y,z,orientation FROM game_world_entity WHERE guid = {guid}')[0]
    assert x > 20, ('Charge did not move the Character', x)
    assert host.rows(f'SELECT last_move_ms FROM game_world_entity WHERE guid = {guid}') == [[3_600_000]]
    client.send(0xee, struct.pack('<IIffffI', 0, 3_605_000, x+6, y, z, orientation, 0))
    client.collect()
    assert host.rows(f'SELECT x,last_move_ms FROM game_world_entity WHERE guid = {guid}') == [[x+6, 3_605_000]]
    observed = []
    for _ in range(50):
        observed += peer.collect()
        if any(opcode == 0xee and packed_guid(body) == guid for opcode, body in observed):
            break
        time.sleep(0.02)
    else:
        raise AssertionError(('missing movement after Charge', [(hex(opcode), body.hex()) for opcode, body in observed]))


def check_death(host):
    client, peer = native_and_peer(host)
    guid, peer_guid = client.guid, peer.guid
    corpse = 0xf101_0000_0000_0000 | guid
    host.restore('game_graveyard', 'id = 5090401')
    host.sql("INSERT INTO game_graveyard (id,map_id,x,y,z,o,name) VALUES (5090401,0,20,0,0,0,'Native graveyard')")
    host.restore('game_creature_template', 'entry = 51004', 'npc_flags')
    host.sql('UPDATE game_creature_template SET npc_flags = 32 WHERE entry = 51004')
    healer = spawn(host, guid, 51004, 2)
    assert healer not in creates(client.collect())
    assert healer not in creates(peer.collect())
    client.send(0x216)
    assert (0x216, b'\0') in client.collect()
    host.call('debug_set_health', [guid, 0])
    dying = client.collect()
    assert values(dying, guid)[22] == 0
    assert any(opcode == 0xe8 for opcode, _ in dying)
    assert not any(opcode == 0x269 for opcode, _ in dying)
    acknowledge_controls(host, client, peer, dying)
    client.send(0x15a)
    released = client.collect()
    assert corpse in creates(released)
    assert any(opcode == 0xea for opcode, _ in released)
    assert any(opcode == 0xde for opcode, _ in released)
    assert healer in creates(released)
    delay = next(struct.unpack('<I', body)[0] for opcode, body in released if opcode == 0x269)
    assert 29000 <= delay <= 30000, (delay, host.sql(f'SELECT reclaim_delay_micros, created_at FROM game_corpse WHERE guid = {corpse}'))
    observed = acknowledge_controls(host, client, peer, released)
    assert corpse in creates(observed)
    assert healer not in creates(observed)
    assert not any(opcode == 0x269 for opcode, _ in observed)
    client.send(0x15a)
    assert corpse not in creates(client.collect())
    client.send(0x216)
    assert (0x216, struct.pack('<BIfffI', 1, 0, 20, 0, 0, 0)) in client.collect()
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    host.sql(f'UPDATE game_corpse SET reclaim_delay_micros = 0 WHERE guid = {corpse}')
    host.sql(f'UPDATE game_corpse SET x = 250, cell = 0 WHERE guid = {corpse}')
    host.call('debug_backfill_cell_ids', [])
    assert corpse in destroys(client.collect())
    assert corpse in destroys(peer.collect())
    assert host.rows(f'SELECT cell FROM game_corpse WHERE guid = {corpse}')[0][0] != 0
    host.sql(f'UPDATE game_corpse SET x = 20, cell = 1 WHERE guid = {corpse}')
    host.call('debug_backfill_cell_ids', [])
    assert corpse in creates(client.collect())
    assert corpse in creates(peer.collect())
    host.call('debug_create_fixture_instance', [guid])
    assert corpse in destroys(client.collect())
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    host.call('debug_enter_instance', [guid, 0])
    assert corpse in creates(client.collect())
    host.call('debug_teleport', [guid, 0, 250, 0, 0, 0])
    assert corpse in destroys(client.collect())
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    host.call('debug_teleport', [guid, 1, 20, 0, 0, 0])
    assert corpse not in creates(client.collect())
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    host.call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    assert corpse in creates(client.collect())
    client.send(0x4b)
    client.collect()
    assert host.rows(f'SELECT guid FROM game_corpse WHERE guid = {corpse}')
    client.send(0x3d, struct.pack('<Q', guid))
    relogged = client.collect()
    assert corpse in creates(relogged)
    assert any(opcode == 0xde for opcode, _ in relogged)
    assert healer in creates(relogged)
    assert (0x269, bytes(4)) in relogged
    peer.collect()
    client.send(0x1d2, struct.pack('<Q', corpse))
    reclaimed = client.collect()
    assert corpse in destroys(reclaimed)
    assert any(opcode == 0xdf for opcode, _ in reclaimed)
    assert healer in destroys(reclaimed)
    assert corpse in destroys(acknowledge_controls(host, client, peer, reclaimed))
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[False]]
    assert not host.rows(f'SELECT guid FROM game_corpse WHERE guid = {corpse}')
    for spell, kind, points, target, parameter in [(51991, 14, 40, 3, 0), (51992, 38, 40, 0, 0), (51993, 181, 0, 0, 51992)]:
        host.sql(f"INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES ({spell},'Native resurrection',0,0,0,0,0,100,60000,1,0,0,1,0,0,0,0,false,0,0,0,0,0,0,0,0)")
        host.sql(f'INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES ({spell * 4},{spell},0,{kind},{points},0,0.0,0,{target},0.0,0,0,0,{parameter},15,0,0,false,0.0)')
    host.call('debug_learn_spell', [peer_guid, 51991])
    host.call('debug_set_health', [guid, 0])
    client.collect()
    peer.collect()
    peer.send(0x12e, cast(51991, guid))
    assert not any(opcode == 0x15b for opcode, _ in peer.collect())
    offered = client.collect()
    offer = next(body for opcode, body in offered if opcode == 0x15b)
    assert struct.unpack_from('<Q', offer)[0] == peer_guid
    name_length = struct.unpack_from('<I', offer, 8)[0]
    assert offer[12 + name_length:] == bytes(2)
    client.send(0x15c, struct.pack('<QB', guid, 1))
    client.collect()
    assert host.rows(f'SELECT target_guid FROM game_resurrect_request WHERE target_guid = {guid}')
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    client.send(0x15c, struct.pack('<QB', peer_guid, 0))
    client.collect()
    assert not host.rows(f'SELECT target_guid FROM game_resurrect_request WHERE target_guid = {guid}')
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    peer.send(0x12e, cast(51991, guid))
    peer.collect()
    assert any(opcode == 0x15b for opcode, _ in client.collect())
    client.send(0x15c, struct.pack('<QB', peer_guid, 1))
    client.collect()
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[False]]
    assert not host.rows(f'SELECT target_guid FROM game_resurrect_request WHERE target_guid = {guid}')
    host.call('debug_force_cast', [guid, 51993])
    client.collect()
    peer.collect()
    host.call('debug_set_health', [guid, 0])
    assert values(client.collect(), guid)[1224] == 51992
    assert 1224 not in values(peer.collect(), guid)
    client.send(0x2b3)
    self_resurrected = client.collect()
    assert values(self_resurrected, guid)[1224] == 0
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[False]]
    assert not host.rows(f'SELECT character_guid FROM game_self_resurrect_option WHERE character_guid = {guid}')
    host.call('debug_set_health', [guid, 0])
    client.send(0x15a)
    client.collect()
    peer.collect()
    grant = host.rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {guid} AND layer = 2 AND entity_guid = {corpse}')
    assert len(grant) == 1
    host.call('debug_age_corpse', [corpse, 310])
    bones = client.collect()
    assert values(bones, corpse)[35] == 1
    assert (0x216, b'\0') in bones
    assert values(peer.collect(), corpse)[35] == 1
    assert host.rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {guid} AND layer = 2 AND entity_guid = {corpse}') == grant
    client.send(0x216)
    assert (0x216, b'\0') in client.collect()
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    host.call('debug_age_corpse', [corpse, 610])
    assert corpse in destroys(client.collect())
    assert corpse in destroys(peer.collect())
    assert not host.rows(f'SELECT id FROM game_aoi_grant WHERE layer = 2 AND entity_guid = {corpse}')
    host.call('debug_teleport', [guid, 0, 250, 0, 0, 0])
    client.collect()
    client.send(0x21c, struct.pack('<Q', healer))
    client.collect()
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    host.call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    client.collect()
    client.send(0x21c, struct.pack('<Q', peer_guid))
    client.collect()
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    host.restore('game_gossip_option', 'row_id = 5090403')
    host.sql("INSERT INTO game_gossip_option (row_id,entry,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES (5090403,51004,0,0,'Return me to life.',6,0,0,0,0)")
    client.send(0x17b, struct.pack('<Q', healer))
    greeted = client.collect()
    menu = next(body for opcode, body in greeted if opcode == 0x17d)
    assert struct.unpack_from('<I', menu, 12)[0] == 1 and b'Return me to life.\0' in menu
    assert not any(opcode == 0x222 for opcode, _ in greeted)
    client.send(0x17c, struct.pack('<QI', healer, 0))
    selected = [packet for packet in client.collect() if packet[0] in (0x17e, 0x222)]
    assert selected == [(0x17e, b''), (0x222, struct.pack('<Q', healer))], selected
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[True]]
    client.send(0x21c, struct.pack('<Q', healer))
    revived = client.collect()
    assert not any(opcode == 0x222 for opcode, _ in revived)
    assert healer in destroys(revived)
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[False]]
    client.send(0x21c, struct.pack('<Q', healer))
    assert not any(opcode == 0x222 for opcode, _ in client.collect())
    assert host.rows(f'SELECT dead FROM game_world_entity WHERE guid = {guid}') == [[False]]
    host.sql('DELETE FROM game_gossip_option WHERE row_id = 5090403')
    peer.collect()


def check_environment(host):
    client, peer = native_and_peer(host)
    guid, peer_guid = client.guid, peer.guid
    health = host.rows(f'SELECT max_health FROM game_world_entity WHERE guid = {guid}')[0][0]
    host.call('debug_set_health', [guid, health])
    client.collect()
    peer.collect()
    x, y, z, orientation, timestamp = host.rows(f'SELECT x,y,z,orientation,last_move_ms FROM game_world_entity WHERE guid = {guid}')[0]
    client.send(0xc9, struct.pack('<IIffffI', 0, timestamp + 1, x, y, z, orientation, 2500))
    landed = client.collect()
    remaining = host.rows(f'SELECT health FROM game_world_entity WHERE guid = {guid}')[0][0]
    assert remaining < health
    assert (0x1fc, struct.pack('<QBIII', guid, 2, health - remaining, 0, 0)) in landed
    assert not any(opcode == 0x1fc for opcode, _ in peer.collect())
    host.call('debug_set_health', [guid, health])
    client.collect()
    for zone in (12, 40):
        if not host.rows(f'SELECT id FROM game_area WHERE id = {zone}'):
            host.sql(f"INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES ({zone},0,0,-1,0,0,0,'Native weather')")
    terrain_chunk(host, 513, 512, 12, liquid_level=10)
    terrain_chunk(host, 512, 512, 40, liquid_level=10)
    host.call('debug_teleport', [guid, 0, -40, -1, 0, 0])
    host.call('debug_teleport', [peer_guid, 0, -40, -1, 0, 0])
    client.collect()
    peer.collect()
    for zone in (12, 40):
        host.restore('game_zone_weather', f'zone_id = {zone}')
    host.call('gw_force_zone_weather', [12, 1, 0.5])
    assert (0x2f4, struct.pack('<IfIB', 1, 0.5, 8534, 0)) in client.collect()
    assert (0x2f4, struct.pack('<IfIB', 1, 0.5, 8534, 0)) in peer.collect()
    host.call('gw_force_zone_weather', [40, 2, 0.25])
    assert not any(opcode == 0x2f4 for opcode, _ in client.collect() + peer.collect())
    client.send(0xee, struct.pack('<IIffffI', 0, 100000, -40, -1, 0, 0, 0))
    client.collect()
    client.send(0xee, struct.pack('<IIffffI', 0, 102000, -31, -1, 0, 0, 0))
    crossing = client.collect()
    world_states = [body for opcode, body in crossing if opcode == 0x2c2]
    assert len(world_states) == 1
    assert world_states[0][:8] == struct.pack('<II', 0, 40)
    state_count = struct.unpack_from('<H', world_states[0], 8)[0]
    assert state_count == 109 and len(world_states[0]) == 10 + state_count * 8
    states = [struct.unpack_from('<Ii', world_states[0], 10 + offset * 8) for offset in range(state_count)]
    assert states[:3] == [(0x07ae, 1), (0x0532, 1), (0x0531, 0)]
    assert (0x07a3, 9999) in states and (0x06f4, 2000) in states
    assert states[-1] == (0, 0)
    assert (0x2f4, struct.pack('<IfIB', 2, 0.25, 8536, 0)) in crossing
    assert not any(opcode in (0x2c2, 0x2f4) for opcode, _ in peer.collect())
    client.send(0x1f4, struct.pack('<I', 12))
    client.collect()
    assert host.rows(f'SELECT zone_id FROM game_world_entity WHERE guid = {guid}') == [[40]]
    host.call('gw_force_zone_weather', [12, 3, 0.75])
    assert not any(opcode == 0x2f4 for opcode, _ in client.collect())
    assert (0x2f4, struct.pack('<IfIB', 3, 0.75, 8558, 0)) in peer.collect()
    host.call('gw_force_zone_weather', [40, 0, 0])
    assert (0x2f4, bytes(13)) in client.collect()
    assert not any(opcode == 0x2f4 for opcode, _ in peer.collect())
    client.send(0x4b)
    client.collect()
    host.call('gw_force_zone_weather', [40, 1, 0.5])
    assert not any(opcode == 0x2f4 for opcode, _ in client.collect() + peer.collect())
    client.send(0x3d, struct.pack('<Q', guid))
    entered = client.collect()
    assert (0x2f4, struct.pack('<IfIB', 1, 0.5, 8534, 0)) in entered
    assert [opcode for opcode, _ in entered if opcode in (0x2c2, 0x2f4)] == [0x2c2, 0x2f4]
    health = host.rows(f'SELECT max_health FROM game_world_entity WHERE guid = {guid}')[0][0]
    host.call('debug_set_health', [guid, health])
    client.collect()
    peer.collect()
    client.send(0xee, struct.pack('<IIfffffI', 0x200000, 200000, -31, -1, 0, 0, 0, 0))
    assert (0x1d9, struct.pack('<IIIIBI', 1, 60000, 60000, 0xffffffff, 0, 0)) in client.collect()
    assert not any(opcode in (0x1d9, 0x1db, 0x1fc) for opcode, _ in peer.collect())
    host.sql(f'UPDATE game_breath_state SET remaining_air_micros = 100000 WHERE entity_guid = {guid}')
    drowned = []
    for _ in range(60):
        drowned += client.collect()
        if any(opcode == 0x1fc for opcode, _ in drowned):
            break
        time.sleep(0.05)
    assert (0x1fc, struct.pack('<QBIII', guid, 1, (health * 20 + 99) // 100, 0, 0)) in drowned
    assert values(drowned, guid)[22] < health
    assert not any(opcode in (0x1d9, 0x1db, 0x1fc) for opcode, _ in peer.collect())
    client.send(0xee, struct.pack('<IIffffI', 0, 201000, -31, -1, 0, 0, 0))
    assert (0x1db, struct.pack('<I', 1)) in client.collect()
    client.send(0xee, struct.pack('<IIfffffI', 0x200000, 202000, -31, -1, 0, 0, 0, 0))
    assert any(opcode == 0x1d9 for opcode, _ in client.collect())
    host.call('debug_set_health', [guid, 0])
    assert (0x1db, struct.pack('<I', 1)) in client.collect()
    assert not host.rows(f'SELECT entity_guid FROM game_breath_state WHERE entity_guid = {guid}')
    host.call('debug_set_health', [guid, health])
    client.collect()
    client.send(0xee, struct.pack('<IIfffffI', 0x200000, 203000, -31, -1, 0, 0, 0, 0))
    assert any(opcode == 0x1d9 for opcode, _ in client.collect())
    client.send(0x4b)
    client.collect()
    assert not host.rows(f'SELECT entity_guid FROM game_breath_state WHERE entity_guid = {guid}')
    client.send(0x3d, struct.pack('<Q', guid))
    assert not any(opcode == 0x1d9 for opcode, _ in client.collect())
    host.call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    host.call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    client.collect()
    peer.collect()


def check_fatigue(host):
    client, peer = native_and_peer(host)
    guid = client.guid
    health, level = host.rows(f'SELECT max_health, level FROM game_world_entity WHERE guid = {guid}')[0]
    host.call('debug_set_health', [guid, health])
    terrain_chunk(host, 512, 512, 40, liquid_level=10)
    host.restore('game_terrain_deep_water', f'key = {(512 << 16) | 512}')
    host.sql(f'INSERT INTO game_terrain_deep_water (key,tiles) VALUES ({(512 << 16) | 512},{(1 << 64) - 1})')
    host.call('debug_teleport', [guid, 0, -31, -1, 0, 0])
    client.collect()
    peer.collect()
    timestamp = host.rows(f'SELECT last_move_ms FROM game_world_entity WHERE guid = {guid}')[0][0]
    client.send(0xee, struct.pack('<IIffffI', 0, timestamp + 1000, -31, -1, 0, 0, 0))
    assert (0x1d9, struct.pack('<IIIIBI', 0, 60000, 60000, 0xffffffff, 0, 0)) in client.collect()
    assert not any(opcode in (0x1d9, 0x1db, 0x1fc) for opcode, _ in peer.collect())
    host.sql(f'UPDATE game_fatigue_state SET remaining_micros = 100000 WHERE entity_guid = {guid}')
    exhausted = []
    for _ in range(60):
        exhausted += client.collect()
        if any(opcode == 0x1fc for opcode, _ in exhausted):
            break
        time.sleep(0.05)
    logs = [struct.unpack('<QBIII', body) for opcode, body in exhausted if opcode == 0x1fc]
    assert all(log[:2] == (guid, 0) and health // 5 <= log[2] < health // 5 + max(level, 1) for log in logs)
    assert logs and values(exhausted, guid)[22] < health
    assert not any(opcode in (0x1d9, 0x1db, 0x1fc) for opcode, _ in peer.collect())
    host.sql(f'DELETE FROM game_terrain_deep_water WHERE key = {(512 << 16) | 512}')
    client.send(0xee, struct.pack('<IIffffI', 0, timestamp + 2000, -31, -1, 0, 0, 0))
    recovering = [struct.unpack('<IIIIBI', body) for opcode, body in client.collect() if opcode == 0x1d9]
    assert [(timer, duration, scale) for timer, _, duration, scale, _, _ in recovering] == [(0, 60000, 10)]
    host.sql(f'UPDATE game_fatigue_state SET remaining_micros = 59990000 WHERE entity_guid = {guid}')
    recovered = []
    for _ in range(60):
        recovered += client.collect()
        if any(opcode == 0x1db for opcode, _ in recovered):
            break
        time.sleep(0.05)
    assert (0x1db, struct.pack('<I', 0)) in recovered
    assert not host.rows(f'SELECT entity_guid FROM game_fatigue_state WHERE entity_guid = {guid}')
    assert not any(opcode in (0x1d9, 0x1db, 0x1fc) for opcode, _ in peer.collect())
    host.call('debug_set_health', [guid, health])
    host.call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    client.collect()
    peer.collect()


def check_social(host):
    client, peer = native_and_peer(host)
    guid, peer_guid = client.guid, peer.guid
    def status(code, target, online=None):
        body = struct.pack('<BQ', code, target)
        if online is not None:
            body += struct.pack('<BIII', *online)
        return 0x68, body

    def list_packets(packets):
        return [packet for packet in packets if packet[0] in (0x67, 0x6b)]

    client.send(0x66)
    assert list_packets(client.collect()) == [(0x67, b'\0'), (0x6b, b'\0')]
    client.send(0x69, b'Missingfriend\0')
    assert status(4, 0) in client.collect()
    client.send(0x69, b'nAtIvE\0')
    assert status(9, guid) in client.collect()
    client.send(0x6c, b'Native\0')
    assert status(12, guid) in client.collect()
    level, zone = host.rows(f'SELECT level, zone_id FROM game_world_entity WHERE guid = {peer_guid}')[0]
    client.send(0x69, b'pEeR\0')
    assert status(6, peer_guid, (1, zone, level, 8)) in client.collect()
    assert not any(opcode == 0x68 for opcode, _ in peer.collect())
    client.send(0x69, b'Peer\0')
    assert status(8, peer_guid) in client.collect()
    peer.send(0x95, struct.pack('<II', 20, 7) + b'AFK friend\0')
    peer.collect()
    client.send(0x66)
    friend = b'\1' + struct.pack('<QBIII', peer_guid, 2, zone, level, 8)
    assert list_packets(client.collect()) == [(0x67, friend), (0x6b, b'\0')]
    client.send(0x6c, b'Peer\0')
    assert status(15, peer_guid) in client.collect()
    client.send(0x6c, b'Peer\0')
    assert status(14, peer_guid) in client.collect()
    client.send(0x66)
    assert list_packets(client.collect()) == [(0x67, friend), (0x6b, b'\1' + struct.pack('<Q', peer_guid))]
    client.send(0x6d, struct.pack('<Q', peer_guid))
    assert status(16, peer_guid) in client.collect()
    client.send(0x6d, struct.pack('<Q', peer_guid))
    assert status(13, peer_guid) in client.collect()
    host.call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    peer.collect()
    assert not any(opcode == 0x68 for opcode, _ in client.collect())
    peer.send(0x4b)
    peer.collect()
    assert [packet for packet in client.collect() if packet[0] == 0x68] == [status(3, peer_guid)]
    client.send(0x66)
    assert list_packets(client.collect()) == [(0x67, b'\1' + struct.pack('<QB', peer_guid, 0)), (0x6b, b'\0')]
    client.send(0x6a, struct.pack('<Q', peer_guid))
    assert status(5, peer_guid) in client.collect()
    client.send(0x69, b'Peer\0')
    assert status(7, peer_guid) in client.collect()
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    level, zone = host.rows(f'SELECT level, zone_id FROM game_world_entity WHERE guid = {peer_guid}')[0]
    assert [packet for packet in client.collect() if packet[0] == 0x68] == [status(2, peer_guid, (1, zone, level, 8))]
    client.send(0x6a, struct.pack('<Q', peer_guid))
    assert status(5, peer_guid) in client.collect()
    client.send(0x6a, struct.pack('<Q', peer_guid))
    assert status(4, peer_guid) in client.collect()
    for ignored, count, opcode, code in [(False, 50, 0x69, 1), (True, 25, 0x6c, 11)]:
        for index in range(count):
            host.sql(f"INSERT INTO game_character_contact (id,owner_guid,owner_identity,target_guid,is_ignore) VALUES ({5091000 + index},{guid},0x{'00' * 32},{900000 + index},{str(ignored).lower()})")
        client.send(opcode, b'Peer\0')
        assert status(code, peer_guid) in client.collect()
        assert len(host.rows(f'SELECT id FROM game_character_contact WHERE owner_guid = {guid}')) == count
        host.sql(f'DELETE FROM game_character_contact WHERE owner_guid = {guid}')

    def who(name='', guild='', minimum=0, maximum=100, races=0xffffffff, classes=0xffffffff, zones=(), strings=()):
        body = struct.pack('<II', minimum, maximum) + name.encode() + b'\0' + guild.encode() + b'\0'
        body += struct.pack('<III', races, classes, len(zones)) + b''.join(struct.pack('<I', zone) for zone in zones)
        body += struct.pack('<I', len(strings)) + b''.join(value.encode() + b'\0' for value in strings)
        client.send(0x62, body)

    def who_names(packets):
        packets = [body for opcode, body in packets if opcode == 0x63]
        assert len(packets) == 1, packets
        body = packets[0]
        listed, total = struct.unpack_from('<II', body)
        assert listed == total
        rest = body[8:]
        names = []
        for _ in range(listed):
            name, rest = rest.split(b'\0', 1)
            _, rest = rest.split(b'\0', 1)
            names.append(name.decode())
            rest = rest[16:]
        assert not rest
        return sorted(names)

    who()
    who()
    assert who_names(client.collect()) == ['Native', 'Peer']
    time.sleep(1.05)
    who(name='pEE')
    assert who_names(client.collect()) == ['Peer']
    for arguments, expected in [({'classes': 1 << 8}, ['Peer']), ({'races': 1 << 2}, []), ({'guild': 'missing'}, []), ({'minimum': 99}, []), ({'strings': ('missing', 'pEE')}, ['Peer']), ({'zones': (0xffffff,)}, [])]:
        host.sql('UPDATE game_native_world_connection SET who_until_micros = 0')
        who(**arguments)
        actual = who_names(client.collect())
        assert actual == expected, (arguments, actual)
    for arguments in [{'zones': tuple(range(11))}, {'strings': ('a', 'b', 'c', 'd', 'e')}]:
        host.sql('UPDATE game_native_world_connection SET who_until_micros = 0')
        who(**arguments)
        assert not any(opcode == 0x63 for opcode, _ in client.collect())
    peer.send(0x4b)
    peer.collect()
    host.sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.send(0x69, b'Peer\0')
    assert status(10, peer_guid) in client.collect()
    client.send(0x6c, b'Peer\0')
    assert status(15, peer_guid) in client.collect()
    client.send(0x6d, struct.pack('<Q', peer_guid))
    client.collect()
    host.sql('UPDATE game_native_world_connection SET who_until_micros = 0')
    who()
    assert who_names(client.collect()) == ['Native']
    peer.send(0x4b)
    peer.collect()
    host.sql(f'UPDATE game_character SET race = 1 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    host.call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    client.collect()


def check_guild(host):
    client, peer = native_and_peer(host)
    guid, peer_guid = client.guid, peer.guid
    def command(operation, name, result):
        return 0x93, struct.pack('<I', operation) + name.encode() + b'\0' + struct.pack('<I', result)

    def event(kind, strings=(), subject=0):
        body = bytes([kind, len(strings)]) + b''.join(s.encode() + b'\0' for s in strings)
        return 0x92, body + (struct.pack('<Q', subject) if subject else b'')

    def roster(packets):
        body = next(body for opcode, body in packets if opcode == 0x8a)
        count = struct.unpack_from('<I', body)[0]
        offset = 4

        def string():
            nonlocal offset
            end = body.index(0, offset)
            result = body[offset:end].decode()
            offset = end + 1
            return result

        motd, info = string(), string()
        ranks = struct.unpack_from('<I', body, offset)[0]
        offset += 4
        rights = struct.unpack_from('<' + 'I' * ranks, body, offset)
        offset += 4 * ranks
        members = {}
        for _ in range(count):
            member, online = struct.unpack_from('<QB', body, offset)
            offset += 9
            name = string()
            rank, level, cls, zone = struct.unpack_from('<IBBI', body, offset)
            offset += 10
            days = None if online else struct.unpack_from('<f', body, offset)[0]
            offset += 0 if online else 4
            members[member] = (name, online, rank, level, cls, zone, days, string(), string())
        assert offset == len(body)
        return motd, info, rights, members

    def invite():
        client.send(0x82, b'pEeR\0')
        client.collect()
        assert (0x83, b'Native\0Native Guild\0') in peer.collect()

    def accept():
        peer.send(0x84)
        packets = peer.collect()
        assert event(3, ['Peer'], peer_guid) in packets
        assert values(packets, peer_guid)[191] == guild_id
        assert values(packets, peer_guid)[192] == 4
        return client.collect()

    host.call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    host.call('debug_teleport', [peer_guid, 0, 21, 0, 0, 0])
    client.collect()
    peer.collect()
    client.send(0x54, struct.pack('<I', 999999))
    assert command(0, '', 9) in client.collect()
    peer.send(0x81, b'Forbidden Guild\0')
    assert not any(op in (0x55, 0x93) for op, _ in peer.collect())
    assert not host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {peer_guid}')
    host.restore('game_account', "username = 'NATIVE'", 'gm_level')
    gm = host.rows("SELECT gm_level FROM game_account WHERE username = 'NATIVE'")[0][0]
    host.call('set_gm_level', ['Native', 1])
    client.send(0x81, b'Temporary Guild\0')
    assert values(client.collect(), guid)[191] != 0
    peer.collect()
    client.send(0x8f)
    assert event(8) in client.collect()
    peer.collect()
    client.send(0x95, struct.pack('<II', 0, 7) + b'.guild create Native "Native Guild"\0')
    created = client.collect()
    guild_id = host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {guid}')[0][0]
    assert values(created, guid)[191] == guild_id
    assert values(peer.collect(), guid)[191] == guild_id
    client.send(0x54, struct.pack('<I', guild_id))
    query = next(body for opcode, body in client.collect() if opcode == 0x55)
    assert query.startswith(struct.pack('<I', guild_id) + b'Native Guild\0Guild Master\0Officer\0')
    client.send(0x82, b'Missingguildmember\0')
    assert command(1, 'Missingguildmember', 11) in client.collect()
    peer.send(0x6c, b'Native\0')
    peer.collect()
    client.send(0x82, b'Peer\0')
    client.collect()
    assert not any(opcode == 0x83 for opcode, _ in peer.collect())
    assert not host.rows(f'SELECT guild_id FROM game_guild_invite WHERE target_guid = {peer_guid}')
    peer.send(0x6d, struct.pack('<Q', guid))
    peer.collect()
    invite()
    client.send(0x82, b'pEeR\0')
    assert command(1, 'Peer', 5) in client.collect()
    peer.send(0x85)
    peer.collect()
    assert (0x86, b'Peer\0') in client.collect()
    peer.send(0x4b)
    peer.collect()
    client.collect()
    client.send(0x82, b'Peer\0')
    assert command(1, 'Peer', 11) in client.collect()
    host.sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.collect()
    client.send(0x82, b'pEeR\0')
    assert command(1, 'pEeR', 12) in client.collect()
    peer.send(0x4b)
    peer.collect()
    client.collect()
    host.sql(f'UPDATE game_character SET race = 1 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.collect()
    invite()
    joined = accept()
    assert event(3, ['Peer'], peer_guid) in joined
    assert values(joined, peer_guid)[191] == guild_id
    client.send(0x82, b'pEeR\0')
    assert command(1, 'Peer', 3) in client.collect()
    client.send(0x8d)
    assert command(3, '', 8) in client.collect()
    peer.send(0x91, b'Forbidden MOTD\0')
    assert command(1, '', 8) in peer.collect()
    peer.send(0x2fc, b'Forbidden info\0')
    assert command(0, '', 8) in peer.collect()
    client.send(0x91, b'Native guild MOTD\0')
    assert event(2, ['Native guild MOTD']) in client.collect()
    assert event(2, ['Native guild MOTD']) in peer.collect()
    client.send(0x2fc, b'Native guild info\0')
    client.collect()
    client.send(0x234, b'pEeR\0Public note\0')
    assert roster(client.collect())[3][peer_guid][-2:] == ('Public note', '')
    client.send(0x235, b'Peer\0Private officer note\0')
    assert roster(client.collect())[3][peer_guid][-2:] == ('Public note', 'Private officer note')
    peer.send(0x89)
    hidden = roster(peer.collect())
    assert hidden[:2] == ('Native guild MOTD', 'Native guild info')
    assert hidden[3][peer_guid][-2:] == ('Public note', '')
    assert hidden[3][guid][1] == 1
    peer.send(0x235, b'Native\0Forbidden note\0')
    assert command(1, '', 8) in peer.collect()
    client.send(0x87)
    info = next(body for opcode, body in client.collect() if opcode == 0x88)
    assert struct.unpack_from('<II', info, len(info) - 8) == (2, 2)
    peer.send(0x232, b'Forbidden rank\0')
    assert command(1, '', 8) in peer.collect()
    client.send(0x232, b'New rank\0')
    assert len(roster(client.collect())[2]) == 6
    assert roster(peer.collect())[3][peer_guid][-1] == ''
    client.send(0x231, struct.pack('<II', 4, 0x4040) + b'Readers\0')
    assert len(roster(client.collect())[2]) == 6
    refreshed = peer.collect()
    assert [op for op, _ in refreshed if op in (0x55, 0x8a)] == [0x55, 0x8a]
    assert roster(refreshed)[3][peer_guid][-1] == 'Private officer note'
    client.send(0x231, struct.pack('<II', 4, 0x43) + b'Initiate\0')
    client.collect()
    assert roster(peer.collect())[3][peer_guid][-1] == ''
    client.send(0x233)
    assert len(roster(client.collect())[2]) == 5
    peer.collect()
    client.send(0x8b, b'pEeR\0')
    promoted = client.collect()
    assert event(0, ['Native', 'Peer', 'Member']) in promoted
    assert values(promoted, peer_guid)[192] == 3
    assert values(peer.collect(), peer_guid)[192] == 3
    client.send(0x8c, b'Peer\0')
    assert event(1, ['Native', 'Peer', 'Initiate']) in client.collect()
    assert values(peer.collect(), peer_guid)[192] == 4
    client.send(0x8c, b'Peer\0')
    assert command(1, 'Peer', 14) in client.collect()
    client.send(0x8b, b'Native\0')
    assert command(1, '', 6) in client.collect()
    peer.send(0x90, b'Peer\0')
    assert command(1, '', 8) in peer.collect()
    client.send(0x90, b'pEeR\0')
    assert event(7, ['Native', 'Peer']) in client.collect()
    leadership = peer.collect()
    assert values(leadership, peer_guid)[192] == 0
    assert values(leadership, guid)[192] == 1
    peer.send(0x90, b'Native\0')
    peer.collect()
    client.collect()
    peer.send(0x4b)
    peer.collect()
    assert event(13, ['Peer'], peer_guid) in client.collect()
    client.send(0x89)
    offline = roster(client.collect())[3][peer_guid]
    assert offline[1] == 0 and 0 <= offline[6] < 0.01
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    entered = peer.collect()
    assert event(2, ['Native guild MOTD']) in entered
    assert event(12, ['Peer'], peer_guid) not in entered
    assert event(12, ['Peer'], peer_guid) in client.collect()
    host.call('debug_teleport', [peer_guid, 1, 21, 0, 0, 0])
    peer.collect(auto_ack=False)
    assert not any(opcode == 0x92 for opcode, _ in client.collect())
    client.send(0x89)
    assert roster(client.collect())[3][peer_guid][1] == 1
    peer.send(0xdc)
    assert not any(opcode == 0x92 for opcode, _ in peer.collect())
    client.collect()
    client.send(0x91, b'Across maps\0')
    assert event(2, ['Across maps']) in client.collect()
    assert event(2, ['Across maps']) in peer.collect()
    peer.send(0x8d)
    departed = peer.collect()
    assert command(3, 'Native Guild', 0) in departed
    assert values(departed, peer_guid)[191] == 0
    assert event(4, ['Peer'], peer_guid) in client.collect()
    invite()
    accept()
    client.send(0x8e, b'pEeR\0')
    assert event(5, ['Peer', 'Native']) in client.collect()
    assert values(peer.collect(), peer_guid)[191] == 0
    invite()
    accept()
    peer_account = host.rows(f'SELECT account_id FROM game_character WHERE guid = {peer_guid}')[0][0]
    host.call('create_character', [peer_account, 'Departed', 1, 1, 0, 0, 0, 0, 0, 0])
    departed_guid = host.rows("SELECT guid FROM game_character WHERE name = 'Departed'")[0][0]
    host.call('realm_guild_op', [{'guid': guid, 'ownership': None}, {'invite': {
        'target_guid': departed_guid, 'actor_team': 469, 'target_team': 469,
        'target_ignores_actor': False}}])
    host.call('realm_guild_op', [{'guid': departed_guid, 'ownership': None}, {'accept': {
        'actor_name': 'Departed', 'actor_team': 469}}])
    client.collect()
    peer.collect()
    client.send(0x90, b'Departed\0')
    assert event(7, ['Native', 'Departed']) in client.collect()
    peer.collect()
    host.call('debug_delete_character', [departed_guid])
    succession = client.collect()
    assert event(7, ['Departed', 'Native']) in succession
    assert event(4, ['Departed'], departed_guid) in succession
    assert values(succession, guid)[192] == 0
    assert not host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {departed_guid}')
    assert host.rows(f'SELECT leader_guid FROM game_guild WHERE guild_id = {guild_id}') == [[guid]]
    peer.collect()
    client.send(0x8f)
    assert event(8) in client.collect()
    disbanded = peer.collect()
    assert event(8) in disbanded
    assert values(disbanded, peer_guid)[191] == 0
    assert not host.rows(f'SELECT guild_id FROM game_guild WHERE guild_id = {guild_id}')
    assert not host.rows(f'SELECT character_guid FROM game_guild_member WHERE guild_id = {guild_id}')
    assert not host.rows(f'SELECT id FROM game_guild_rank WHERE guild_id = {guild_id}')
    host.call('set_gm_level', ['Native', gm])
    host.call('debug_teleport', [peer_guid, 0, 21, 0, 0, 0])
    peer.collect()
    client.collect()


def check_petitions(host):
    client, peer = native_and_peer(host)
    guid, peer_guid = client.guid, peer.guid
    def command(operation, name, result):
        return 0x93, struct.pack('<I', operation) + name.encode() + b'\0' + struct.pack('<I', result)

    def buy(name='Native Charter', npc=None):
        client.send(0x1bd, struct.pack('<QIQ', vendor if npc is None else npc, 0, 0) + name.encode() + b'\0' + bytes(51))
        return client.collect()

    def sign(world, signer, charter, result=0):
        world.send(0x1c0, struct.pack('<QB', charter, 0))
        expected = (0x1c1, struct.pack('<QQI', charter, signer, result))
        assert expected in world.collect()
        return expected

    def money():
        return host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]

    def current():
        return host.rows(f'SELECT petition_id, charter_item_guid FROM game_guild_petition WHERE owner_guid = {guid}')[0]

    def emblem(world, npc, result):
        world.send(0x1f1, struct.pack('<QIIIII', npc, 11, 12, 3, 14, 15))
        packets = world.collect()
        assert (0x1f1, struct.pack('<I', result)) in packets
        return packets

    template = host.sql('SELECT * FROM game_item_template WHERE entry = 51')[0]
    row = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    row.update(entry=5863, name='Guild Charter', max_stack=1, max_count=1, bonding=1, inventory_type=0)
    if not host.rows('SELECT entry FROM game_item_template WHERE entry = 5863'):
        literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in row.values()]
        host.sql(f"INSERT INTO game_item_template ({','.join(row)}) VALUES ({','.join(literals)})")
    host.call('debug_teleport', [peer_guid, 0, 21, 0, 0, 0])
    peer.collect()
    old_money = money()
    vendor = spawn(host, guid, 51004, 2)
    host.sql(f'UPDATE game_world_entity SET npc_flags = 1536 WHERE guid = {vendor}')
    client.collect()
    peer.collect()
    client.send(0x1f2, struct.pack('<Q', vendor))
    assert (0x1f2, struct.pack('<Q', vendor)) in client.collect()
    client.send(0x1bb, struct.pack('<Q', vendor))
    assert any(opcode == 0x1bc for opcode, _ in client.collect())
    emblem(client, vendor, 2)
    host.call('debug_set_money', [guid, 150000])
    client.collect()
    assert command(0, '1', 6) in buy('1')
    assert money() == 150000
    assert not host.rows(f'SELECT petition_id FROM game_guild_petition WHERE owner_guid = {guid}')
    buy(npc=peer_guid)
    assert money() == 150000
    host.call('debug_set_money', [guid, 999])
    client.collect()
    assert (0x1a5, struct.pack('<QIB', vendor, 5863, 2)) in buy()
    assert money() == 999
    host.call('debug_set_money', [guid, 150000])
    client.collect()
    bought = buy()
    petition_id, charter = current()
    assert money() == 149000
    assert charter in creates(bought)
    assert values(bought, charter)[22] == petition_id
    assert not host.rows('SELECT payer_guid FROM game_guild_fee_hold')
    assert not host.rows('SELECT operation_id FROM game_guild_fee_decision')
    buy()
    assert money() == 149000 and current() == [petition_id, charter]
    client.send(0x1be, struct.pack('<Q', charter))
    expected = (0x1bf, struct.pack('<QQIB', charter, guid, petition_id, 0))
    assert expected in client.collect()
    client.send(0x1c6, struct.pack('<IQ', petition_id, charter))
    response = next(body for opcode, body in client.collect() if opcode == 0x1c7)
    assert response.startswith(struct.pack('<IQ', petition_id, guid) + b'Native Charter\0')
    sign(client, guid, charter, 3)
    client.send(0x1c4, struct.pack('<Q', charter))
    assert (0x1c5, struct.pack('<I', 4)) in client.collect()
    assert current() == [petition_id, charter]
    client.send(0x1c3, struct.pack('<QQ', charter, peer_guid))
    client.collect()
    assert expected in peer.collect()
    peer.send(0x1c2, struct.pack('<Q', charter))
    peer.collect()
    assert (0x1c2, struct.pack('<Q', peer_guid)) in client.collect()
    expected = sign(peer, peer_guid, charter)
    assert expected in client.collect()
    expected = sign(peer, peer_guid, charter, 1)
    assert expected in client.collect()
    peer.send(0x2c1, struct.pack('<Q', charter) + b'Forbidden name\0')
    assert not any(opcode == 0x2c1 for opcode, _ in peer.collect())
    client.send(0x2c1, struct.pack('<Q', charter) + b'1\0')
    assert command(0, '1', 6) in client.collect()
    client.send(0x2c1, struct.pack('<Q', charter) + b'Native Founders\0')
    assert (0x2c1, struct.pack('<Q', charter) + b'Native Founders\0') in client.collect()
    client.send(0x4b)
    client.collect()
    peer.collect()
    client.send(0x3d, struct.pack('<Q', guid))
    assert values(client.collect(), charter)[22] == petition_id
    peer.collect()
    host.call('gw_destroy_guild_charter', [{'guid': guid, 'ownership': None}, charter])
    assert charter in destroys(client.collect())
    host.call('debug_set_money', [guid, 0])
    client.collect()
    buy()
    assert current() == [petition_id, charter]
    assert host.rows(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}') == [[peer_guid]]
    host.call('debug_set_money', [guid, 150000])
    client.collect()
    buy('Native Founders')
    old_petition = petition_id
    petition_id, charter = current()
    assert petition_id != old_petition and money() == 149000
    assert not host.rows(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {old_petition}')
    assert sign(peer, peer_guid, charter) in client.collect()

    peer_account = host.rows(f'SELECT account_id FROM game_character WHERE guid = {peer_guid}')[0][0]
    host.call('create_character', [peer_account, 'Peeralt', 1, 8, 0, 0, 0, 0, 0, 0])
    alternate = host.rows("SELECT guid FROM game_character WHERE name = 'Peeralt'")[0][0]
    peer.send(0x4b)
    peer.collect()
    client.collect()
    peer.send(0x3d, struct.pack('<Q', alternate))
    peer.collect()
    client.collect()
    assert sign(peer, alternate, charter, 1) in client.collect()
    assert len(host.rows(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}')) == 1
    peer.send(0x4b)
    peer.collect()
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.collect()
    extras = []
    for suffix in ['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight']:
        world = host.enter_world('Signer' + suffix, cls=8)
        signer = world.guid
        extras.append((world, signer))
        assert sign(world, signer, charter) in client.collect()
    assert len(host.rows(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}')) == 9
    peer.send(0x1c4, struct.pack('<Q', charter))
    assert not any(opcode == 0x1c5 for opcode, _ in peer.collect())
    assert current() == [petition_id, charter]
    client.send(0x1c4, struct.pack('<Q', charter))
    founded = client.collect()
    assert [packet for packet in founded if packet[0] in (0x93, 0x1c5)] == [command(0, 'Native Founders', 0), (0x1c5, bytes(4))]
    assert charter in destroys(founded)
    guild_id = host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {guid}')[0][0]
    assert values(founded, guid)[191] == guild_id
    assert command(14, 'Native Founders', 0) in peer.collect()
    for world, signer in extras:
        founded = world.collect()
        assert command(14, 'Native Founders', 0) in founded
        assert values(founded, signer)[191] == guild_id
    assert len(host.rows(f'SELECT character_guid FROM game_guild_member WHERE guild_id = {guild_id}')) == 10
    assert not host.rows(f'SELECT petition_id FROM game_guild_petition WHERE owner_guid = {guid}')
    assert not host.rows(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}')
    assert not host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {charter}')
    emblem(peer, vendor, 3)
    host.call('debug_set_money', [guid, 99999])
    client.collect()
    emblem(client, vendor, 4)
    assert money() == 99999
    host.call('debug_set_money', [guid, 150000])
    client.collect()
    emblem(client, peer_guid, 5)
    assert money() == 150000
    packets = emblem(client, vendor, 0)
    assert (0x92, bytes([9, 0])) in packets
    assert [opcode for opcode, _ in packets if opcode in (0x92, 0x55, 0x1f1)] == [0x92, 0x55, 0x1f1]
    assert money() == 50000
    assert host.rows(f'SELECT emblem_style, emblem_color, border_style, border_color, background_color FROM game_guild WHERE guild_id = {guild_id}') == [[11, 12, 3, 14, 15]]
    assert (0x92, bytes([9, 0])) in peer.collect()
    assert not host.rows('SELECT payer_guid FROM game_guild_fee_hold')
    assert not host.rows('SELECT operation_id FROM game_guild_fee_decision')
    client.send(0x8f)
    client.collect()
    peer.collect()
    host.call('debug_grant_item', [guid, 5863, 1])
    inert = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5863')[0][0]
    client.collect()
    client.send(0x4b)
    client.collect()
    peer.collect()
    client.send(0x3d, struct.pack('<Q', guid))
    assert inert in destroys(client.collect())
    assert not host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {inert}')
    peer.collect()
    host.call('debug_set_money', [guid, old_money])
    client.collect()
    peer.collect()


def check_trade(host):

    def status(code):
        return 0x120, struct.pack('<I', code)

    def trading(packets):
        return [packet for packet in packets if packet[0] in (0x120, 0x121)]

    worlds = []
    for index, name in enumerate(['Traderone', 'Tradertwo', 'Tradeviewer']):
        guid = host.character(name, cls=8, at=(0, 1000 + index, 1000, 0))
        host.sql(f'DELETE FROM game_item_instance WHERE owner_guid = {guid} AND slot >= 23 AND slot <= 38')
        worlds.append((host.log_in(guid), guid))
        host.call('debug_set_money', [guid, 1000])
    (first, first_guid), (second, second_guid), (watcher, _) = worlds
    first.collect()
    second.collect()
    watcher.collect()

    def propose():
        first.send(0x116, struct.pack('<Q', second_guid))
        assert not trading(first.collect())
        assert (0x120, struct.pack('<IQ', 1, first_guid)) in second.collect()

    def begin():
        propose()
        second.send(0x117)
        assert status(2) in second.collect()
        assert status(2) in first.collect()

    def cancel():
        first.send(0x11c)
        assert status(3) in first.collect()
        assert status(3) in second.collect()
        assert not host.rows('SELECT id FROM game_trade_session')
        assert not host.rows('SELECT id FROM game_trade_slot')

    def offers(owner, partner, gold, slot=None, entry=0, stack=0, enchantment=0, random_property=0):
        for world, partner_flag in [(owner, 0), (partner, 1)]:
            packets = world.collect()
            bodies = [body for opcode, body in packets if opcode == 0x121]
            assert len(bodies) == 1, trading(packets)
            body = bodies[0]
            assert len(body) == 444
            assert struct.unpack_from('<BIIII', body) == (partner_flag, 7, 7, gold, 0)
            if slot is not None:
                item = body[17 + slot * 61:17 + (slot + 1) * 61]
                assert struct.unpack_from('<I', item, 1)[0] == entry
                assert struct.unpack_from('<I', item, 9)[0] == stack
                assert struct.unpack_from('<I', item, 25)[0] == enchantment
                assert struct.unpack_from('<I', item, 45)[0] == random_property
            if partner_flag == 0:
                owner_packets = packets
        return owner_packets

    def accept_first():
        first.send(0x11a, bytes(4))
        assert not trading(first.collect())
        assert status(4) in second.collect()

    for target in [0, first_guid]:
        first.send(0x116, struct.pack('<Q', target))
        packets = first.collect()
        assert status(6) in packets, trading(packets)
    second.send(0x6c, b'Traderone\0')
    second.collect()
    first.send(0x116, struct.pack('<Q', second_guid))
    assert status(14) in first.collect()
    second.send(0x6d, struct.pack('<Q', first_guid))
    second.collect()
    for opcode, result in [(0x118, 0), (0x119, 14)]:
        propose()
        first.send(0x117)
        assert not trading(first.collect())
        assert host.rows('SELECT open FROM game_trade_session') == [[False]]
        second.send(opcode)
        assert not trading(second.collect())
        assert status(result) in first.collect()
        assert not host.rows('SELECT id FROM game_trade_session')
    host.call('debug_teleport', [second_guid, 0, 1011, 1000, 0, 0])
    second.collect()
    first.collect()
    first.send(0x116, struct.pack('<Q', second_guid))
    assert status(10) in first.collect()
    host.call('debug_teleport', [second_guid, 0, 1001, 1000, 0, 0])
    second.collect()
    first.collect()

    host.call('debug_grant_item', [first_guid, 5090050, 1])
    host.call('debug_grant_item', [first_guid, 5090054, 1])
    host.call('debug_grant_item', [second_guid, 5090052, 5])
    weapon, weapon_slot = host.rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090050')[0]
    reins, reins_slot = host.rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090054')[0]
    food, food_slot = host.rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090052')[0]
    host.sql(f'UPDATE game_item_instance SET enchant_id = 7745, random_property_id = 1900, durability = 11 WHERE guid = {weapon}')
    first.collect()
    second.collect()
    begin()
    first.send(0x11d, bytes([0, 255, weapon_slot]))
    offers(first, second, 0, 0, 5090050, 1, 823, 1900)
    first.send(0x11d, bytes([6, 255, reins_slot]))
    offers(first, second, 0, 6)
    host.sql(f'UPDATE game_item_instance SET soulbound = false WHERE guid = {reins}')
    first.send(0x11d, bytes([6, 255, reins_slot]))
    offers(first, second, 0, 6, 5090054, 1)
    second.send(0x11d, bytes([0, 255, food_slot]))
    offers(second, first, 0, 0, 5090052, 5)
    for request in [bytes([7, 255, weapon_slot]), bytes([1, 1, weapon_slot]), bytes([1, 255, 4])]:
        first.send(0x11d, request)
        assert not trading(first.collect())
        assert not trading(second.collect())
    first.send(0x11d, bytes([1, 255, weapon_slot]))
    offers(first, second, 0, 1)
    assert len(host.rows('SELECT id FROM game_trade_slot')) == 3
    accept_first()
    second.send(0x11f, struct.pack('<I', 25))
    assert status(7) in offers(second, first, 25, 0, 5090052, 5)
    assert host.rows('SELECT initiator_accepted, target_accepted FROM game_trade_session') == [[False, False]]
    first.send(0x11b)
    assert not trading(first.collect())
    assert status(7) in second.collect()
    accept_first()
    second.send(0x11e, bytes([0]))
    assert status(7) in offers(second, first, 25, 0)
    second.send(0x11d, bytes([0, 255, food_slot]))
    offers(second, first, 25, 0, 5090052, 5)
    first.send(0x11f, struct.pack('<I', 50))
    offers(first, second, 50, 0, 5090050, 1, 823, 1900)
    accept_first()
    second.send(0x10e, bytes([255, food_slot, 255, 38, 1]))
    assert any(opcode == 0x112 for opcode, _ in second.collect())
    assert host.rows(f'SELECT stack_count FROM game_item_instance WHERE guid = {food}') == [[5]]
    assert host.rows('SELECT initiator_accepted, target_accepted FROM game_trade_session') == [[True, False]]
    assert not trading(first.collect())
    host.sql('UPDATE game_item_template SET entry = 4000000050 WHERE entry = 5090050')
    try:
        second.send(0x11a, bytes(4))
        failed = second.collect() + first.collect()
        assert not trading(failed) and not creates(failed)
        assert food not in destroys(failed) and weapon not in destroys(failed)
        assert host.rows(f'SELECT owner_guid FROM game_item_instance WHERE guid = {weapon}') == [[first_guid]]
        assert host.rows(f'SELECT owner_guid FROM game_item_instance WHERE guid = {food}') == [[second_guid]]
        assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {first_guid}') == [[1000]]
        assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {second_guid}') == [[1000]]
        assert host.rows('SELECT initiator_accepted, target_accepted FROM game_trade_session') == [[True, False]]
    finally:
        host.sql('UPDATE game_item_template SET entry = 5090050 WHERE entry = 4000000050')
    second.send(0x11a, bytes(4))
    second_done, first_done = second.collect(), first.collect()
    assert status(8) in second_done and status(8) in first_done
    assert weapon in destroys(first_done) and food in destroys(second_done)
    new_weapon, enchantment, random_property, durability = host.rows(f'SELECT guid, enchant_id, random_property_id, durability FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090050')[0]
    new_food, food_slot = host.rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090052')[0]
    assert new_weapon != weapon and new_weapon in creates(second_done)
    assert new_food != food and new_food in creates(first_done)
    assert (enchantment, random_property, durability) == (7745, 1900, 11)
    assert host.rows(f'SELECT owner_guid FROM game_item_instance WHERE guid = {reins}') == [[first_guid]]
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {first_guid}') == [[975]]
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {second_guid}') == [[1025]]
    assert not host.rows('SELECT id FROM game_trade_session') and not host.rows('SELECT id FROM game_trade_slot')

    begin()
    first.send(0x11f, struct.pack('<I', 976))
    offers(first, second, 976)
    accept_first()
    second.send(0x11a, bytes(4))
    assert (0x120, struct.pack('<IIBI', 12, 0x1d, 1, 0)) in second.collect()
    assert (0x120, struct.pack('<IIBI', 12, 0x1d, 0, 0)) in first.collect()
    assert not host.rows('SELECT id FROM game_trade_session')
    occupied = len(host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {second_guid} AND slot >= 23 AND slot <= 38'))
    for _ in range(16 - occupied):
        host.call('debug_grant_item', [second_guid, 5090050, 1])
    second.collect()
    first.collect()
    begin()
    first.send(0x11d, bytes([0, 255, food_slot]))
    offers(first, second, 0, 0, 5090052, 5)
    accept_first()
    second.send(0x11a, bytes(4))
    assert (0x120, struct.pack('<IIBI', 12, 0x32, 0, 0)) in second.collect()
    assert (0x120, struct.pack('<IIBI', 12, 0x32, 1, 0)) in first.collect()
    assert host.rows(f'SELECT owner_guid, stack_count FROM game_item_instance WHERE guid = {new_food}') == [[first_guid, 5]]
    assert not host.rows('SELECT id FROM game_trade_session')

    begin()
    host.call('debug_teleport', [second_guid, 0, 1011, 1000, 0, 0])
    assert not trading(second.collect())
    first.collect()
    accept_first()
    second.send(0x11a, bytes(4))
    assert status(10) in second.collect()
    assert status(7) in first.collect()
    assert host.rows('SELECT initiator_accepted, target_accepted FROM game_trade_session') == [[False, False]]
    cancel()
    host.call('debug_teleport', [second_guid, 0, 1001, 1000, 0, 0])
    second.collect()
    first.collect()
    begin()
    host.call('debug_set_health', [first_guid, 0])
    assert status(3) in first.collect() and status(3) in second.collect()
    first.send(0x116, struct.pack('<Q', second_guid))
    assert status(17) in first.collect()
    second.send(0x116, struct.pack('<Q', first_guid))
    assert status(18) in second.collect()
    host.call('debug_set_health', [first_guid, 1000])
    first.collect()
    second.collect()
    begin()
    second.send(0x4b)
    second.collect()
    assert status(3) in first.collect()
    assert not host.rows('SELECT id FROM game_trade_session')
    second.send(0x3d, struct.pack('<Q', second_guid))
    second.collect()
    first.collect()
    begin()
    host.call('debug_teleport', [second_guid, 1, 1001, 1000, 0, 0])
    departing = second.collect(auto_ack=False)
    assert status(3) in first.collect()
    assert departing.index(status(3)) < next(i for i, (opcode, _) in enumerate(departing) if opcode == 0x3e)
    assert not host.rows('SELECT id FROM game_trade_session')
    second.send(0xdc)
    second.collect()
    host.call('debug_teleport', [second_guid, 0, 1001, 1000, 0, 0])
    second.collect()
    first.collect()
    begin()
    host.call('debug_create_fixture_instance', [second_guid])
    assert status(3) in second.collect() and status(3) in first.collect()
    assert not host.rows('SELECT id FROM game_trade_session')
    assert host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {second_guid}')[0][0] != 0
    assert not trading(watcher.collect())


def check_duels(host):

    def duel_packets(packets):
        return [packet for packet in packets if packet[0] in (0x167, 0x168, 0x169, 0x16a, 0x16b, 0x2b7)]

    def until(world, predicate):
        deadline, packets = time.monotonic() + 5, []
        while time.monotonic() < deadline:
            packets.extend(world.collect())
            if predicate(packets):
                return packets
            time.sleep(0.05)
        raise AssertionError(f'duel transition did not arrive: {duel_packets(packets)}')

    host.sql("INSERT INTO game_gameobject_template (entry,type_id,display_id,name,data0,data1,gather_skill_line,respawn_secs,gather_gray,lock_id,size) VALUES (5090490,16,787,'Native duel flag',0,0,0,0,0,0,1.0)")
    host.sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES (5090173,'Native duel challenge',0,0,0,0,0,30,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0)")
    host.sql('INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES (20360692,5090173,0,34,0,0,0.0,0,1,0.0,0,0,0,5090490,13,0,0,false,0.0)')
    worlds = []
    for index, name in enumerate(['Duelone', 'Dueltwo', 'Duelviewer']):
        world = host.enter_world(name, cls=8, at=(0, 1000 + index, 1000, 0))
        worlds.append((world, world.guid))
    (first, first_guid), (second, second_guid), (watcher, watcher_guid) = worlds
    assert first_guid in creates(watcher.entered) and second_guid in creates(watcher.entered)
    host.call('debug_learn_spell', [first_guid, 5090173])
    first.collect()
    second.collect()
    watcher.collect()

    def request():
        first.send(0x12e, cast(5090173, second_guid))
        packets = first.collect()
        assert host.rows('SELECT state FROM game_duel') == [[0]], duel_packets(packets)
        flag = host.rows('SELECT flag_guid FROM game_duel')[0][0]
        for received in [packets, second.collect()]:
            assert (0x167, struct.pack('<QQ', flag, first_guid)) in received
            assert creates(received).count(flag) == 1
            for participant in [first_guid, second_guid]:
                fields = values(received, participant)
                assert fields[188] | fields[189] << 32 == flag
            assert received.index((0x167, struct.pack('<QQ', flag, first_guid))) > next(i for i, (opcode, body) in enumerate(received) if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == flag)
        observed = watcher.collect()
        assert not duel_packets(observed) and flag not in creates(observed)
        return flag

    def activate(flag):
        second.send(0x16c, struct.pack('<Q', flag))
        assert (0x2b7, struct.pack('<I', 3000)) in second.collect()
        assert (0x2b7, struct.pack('<I', 3000)) in first.collect()
        assert host.rows('SELECT state FROM game_duel') == [[1]]
        host.sql('UPDATE game_duel SET countdown_deadline_micros = 1')
        active = until(first, lambda packets: values(packets, first_guid).get(196) == 1)
        partner = second.collect()
        for received in [active, partner]:
            assert values(received, first_guid)[196] == 1
            assert values(received, second_guid)[196] == 2
        assert not duel_packets(watcher.collect())

    def completion(packets, flag, reason=None, winner='Dueltwo', loser='Duelone'):
        assert (0x16a, bytes([reason is not None])) in packets, duel_packets(packets)
        if reason is None:
            assert not any(opcode == 0x16b for opcode, _ in packets)
        else:
            expected = (0x16b, bytes([reason]) + winner.encode() + b'\0' + loser.encode() + b'\0')
            assert packets.count(expected) == 1, duel_packets(packets)
        assert flag in destroys(packets)
        for participant in [first_guid, second_guid]:
            fields = values(packets, participant)
            assert [fields.get(index) for index in (188, 189, 196)] == [0, 0, 0]
        assert not host.rows('SELECT id FROM game_duel')

    flag = request()
    for world, offered_flag in [(first, flag), (second, flag + 1), (watcher, flag)]:
        world.send(0x16c, struct.pack('<Q', offered_flag))
        assert not duel_packets(world.collect())
        assert host.rows('SELECT state FROM game_duel') == [[0]]
    second.send(0x16d, struct.pack('<Q', flag + 1))
    assert not duel_packets(second.collect())
    assert host.rows('SELECT state FROM game_duel') == [[0]]
    second.send(0x16d, struct.pack('<Q', flag))
    completion(second.collect(), flag)
    completion(first.collect(), flag)
    assert not duel_packets(watcher.collect())

    flag = request()
    activate(flag)
    first.send(0x16d, struct.pack('<Q', flag))
    completion(first.collect(), flag, 1)
    completion(second.collect(), flag, 1)
    assert duel_packets(watcher.collect()) == [(0x16b, b'\x01Dueltwo\0Duelone\0')]

    flag = request()
    activate(flag)
    host.call('debug_create_fixture_instance', [watcher_guid])
    watcher.collect()
    host.call('debug_teleport', [first_guid, 0, 1060, 1000, 0, 0])
    assert (0x168, b'') in until(first, lambda packets: (0x168, b'') in packets)
    assert not duel_packets(second.collect()) and not duel_packets(watcher.collect())
    host.call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
    assert (0x169, b'') in until(first, lambda packets: (0x169, b'') in packets)
    assert not duel_packets(second.collect())
    host.call('debug_teleport', [first_guid, 0, 1060, 1000, 0, 0])
    until(first, lambda packets: (0x168, b'') in packets)
    host.sql('UPDATE game_duel SET initiator_out_of_bounds_deadline_micros = 1')
    completion(until(first, lambda packets: (0x16a, b'\x01') in packets), flag, 1)
    completion(second.collect(), flag, 1)
    assert not duel_packets(watcher.collect())
    host.call('debug_enter_instance', [watcher_guid, 0])
    watcher.collect()
    host.call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
    first.collect()
    second.collect()
    watcher.collect()

    flag = request()
    activate(flag)
    host.call('debug_teleport', [watcher_guid, 0, 2000, 1000, 0, 0])
    watcher.collect()
    # Regeneration must not change the one-health knockout before the observation.
    max_health = host.rows(f'SELECT max_health FROM game_world_entity WHERE guid = {second_guid}')[0][0]
    host.sql(f'UPDATE game_world_entity SET max_health = 1 WHERE guid = {second_guid}')
    host.call('debug_set_health', [second_guid, 1])
    first.collect()
    second.collect()
    first.send(0x141, struct.pack('<Q', second_guid))
    knocked_out, deadline = [], time.monotonic() + 10
    while time.monotonic() < deadline:
        host.sql(f'UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {first_guid}')
        knocked_out.extend(first.collect())
        if (0x16a, b'\x01') in knocked_out:
            break
        time.sleep(0.05)
    completion(knocked_out, flag, 0, 'Duelone', 'Dueltwo')
    completion(second.collect(), flag, 0, 'Duelone', 'Dueltwo')
    remaining = host.rows(f'SELECT health, dead FROM game_world_entity WHERE guid = {second_guid}')
    assert remaining == [[1, False]], remaining
    assert not host.rows(f'SELECT owner_guid FROM game_corpse WHERE owner_guid = {second_guid}')
    assert not host.rows(f'SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {first_guid}')
    assert not duel_packets(watcher.collect())
    host.sql(f'UPDATE game_world_entity SET max_health = {max_health} WHERE guid = {second_guid}')
    host.call('debug_set_health', [second_guid, 1000])
    host.call('debug_teleport', [watcher_guid, 0, 1002, 1000, 0, 0])
    watcher.collect()
    first.collect()
    second.collect()

    flag = request()
    host.sql('UPDATE game_duel SET created_at_micros = 0')
    completion(until(first, lambda packets: (0x16a, b'\0') in packets), flag)
    completion(second.collect(), flag)
    assert not duel_packets(watcher.collect())
    flag = request()
    host.call('debug_teleport', [second_guid, 1, 1001, 1000, 0, 0])
    departing = second.collect(auto_ack=False)
    completion(departing, flag)
    completion(first.collect(), flag)
    assert departing.index((0x16a, b'\0')) < next(i for i, (opcode, _) in enumerate(departing) if opcode == 0x3e)
    second.send(0xdc)
    second.collect()
    host.call('debug_teleport', [second_guid, 0, 1001, 1000, 0, 0])
    second.collect()
    first.collect()
    watcher.collect()
    flag = request()
    second.send(0x4b)
    second.collect()
    completion(first.collect(), flag)
    assert not duel_packets(watcher.collect())
    second.send(0x3d, struct.pack('<Q', second_guid))
    second.collect()
    first.collect()
    watcher.collect()
    flag = request()
    host.call('debug_set_health', [second_guid, 0])
    completion(first.collect(), flag)
    completion(second.collect(), flag)
    assert not duel_packets(watcher.collect())
    host.call('debug_set_health', [second_guid, 1000])
    second.collect()
    first.collect()
    watcher.collect()
    flag = request()
    host.call('debug_create_fixture_instance', [second_guid])
    completion(second.collect(), flag)
    completion(first.collect(), flag)
    assert not duel_packets(watcher.collect())


def check_world_objects(host):

    def splines(packets, guid):
        result = []
        for opcode, body in packets:
            if opcode != 0xdd or packed_guid(body) != guid:
                continue
            offset = 1 + body[0].bit_count()
            x, y, z, spline_id, kind = struct.unpack_from('<fffIB', body, offset)
            offset += 17
            angle = None
            if kind == 4:
                angle = struct.unpack_from('<f', body, offset)[0]
                offset += 4
            else:
                assert kind == 0
            flags, duration, count = struct.unpack_from('<III', body, offset)
            destination = struct.unpack_from('<fff', body, offset + 12)
            result.append(dict(start=(x, y, z), id=spline_id, kind=kind, angle=angle,
                               flags=flags, duration=duration, count=count, destination=destination))
        return result

    worlds = []
    for name in ['Splineone', 'Splinetwo', 'Splinethree']:
        world = host.enter_world(name, at=(0, 3000, 1000, 0))
        worlds.append((world, world.guid))
    (first, first_guid), (second, second_guid), (outside, outside_guid) = worlds
    host.call('debug_create_fixture_instance', [first_guid])
    first.collect()
    instance = host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {first_guid}')[0][0]
    # A dedicated slow tick leaves spline timing under the fixture's control.
    host.call('debug_arm_instance_tick', [instance, 600000])
    host.call('debug_enter_instance', [second_guid, instance])
    second.collect()
    host.restore('game_creature_template', 'entry = 51004', 'npc_flags')
    host.sql('UPDATE game_creature_template SET npc_flags = 0 WHERE entry = 51004')
    creature = spawn(host, first_guid, 51004, 2)
    sx, sy, sz = host.rows(f'SELECT x, y, z FROM game_world_entity WHERE guid = {creature}')[0]
    first_created, second_created = creates(first.collect()), creates(second.collect())
    assert creature in first_created and creature in second_created, (
        creature, first_created, second_created,
        host.rows(f'SELECT guid, map_id, instance_id, x, y, z, dead FROM game_world_entity WHERE guid = {creature} OR guid = {first_guid} OR guid = {second_guid}'),
        host.rows(f'SELECT viewer_guid, entity_guid FROM game_aoi_grant WHERE entity_guid = {creature}'))
    outside.collect()
    weapon = host.rows('SELECT display_id FROM game_item_template WHERE entry = 5090050')[0][0]
    host.call('debug_encounter_equip', [creature, 5090050, 0, 5090050])
    equipped = values(first.collect(), creature)
    assert [equipped[index] for index in (37, 38, 39)] == [weapon, 0, weapon]
    assert values(second.collect(), creature) == equipped
    assert not values(outside.collect(), creature)

    door_entry = 5090640
    door = (0xf110 << 48) | door_entry
    host.call('debug_spawn_gameobject', [door_entry, 0, 259, 0, 0, 3002, 1000, 0, 0, 0, 0, 0])
    spawned = outside.collect()
    assert door in creates(spawned)
    assert values(spawned, door, create=True)[14] == 1
    assert [values(spawned, door)[i] for i in range(10, 14)] == [0, 0, 0, 0x3f800000]
    assert door not in creates(first.collect() + second.collect())
    grant = host.rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {outside_guid} AND layer = 1 AND entity_guid = {door}')[0][0]
    outside.send(0xb1, struct.pack('<Q', door))
    toggled = outside.collect()
    assert values(toggled, door)[14] == 0 and door not in creates(toggled)
    assert host.rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {outside_guid} AND layer = 1 AND entity_guid = {door}') == [[grant]]
    assert not values(first.collect() + second.collect(), door)
    host.sql(f'UPDATE game_gameobject SET rotation_0 = 0.5, rotation_1 = 0.5, rotation_2 = 0.5, rotation_3 = 0.5 WHERE guid = {door}')
    outside.send(0xb1, struct.pack('<Q', door))
    closed = outside.collect()
    assert values(closed, door)[14] == 1
    assert [values(closed, door)[i] for i in range(10, 14)] == [0x3f000000] * 4
    host.call('debug_encounter_open_door', [door_entry, 0])
    opened = outside.collect()
    assert values(opened, door)[14] == 0 and door not in creates(opened)
    assert [values(opened, door)[i] for i in range(10, 14)] == [0x3f000000] * 4
    host.call('debug_encounter_open_door', [door_entry, 0])
    assert not values(outside.collect(), door)
    host.call('debug_enter_instance', [first_guid, 0])
    joined = first.collect()
    assert door in creates(joined)
    assert values(joined, door, create=True)[14] == 0
    assert [values(joined, door)[i] for i in range(10, 14)] == [0x3f000000] * 4
    create_index = next(i for i, (opcode, body) in enumerate(joined) if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == door)
    rotation_index = next(i for i, (opcode, body) in enumerate(joined) if opcode == 0xa9 and body[5] == 0 and packed_guid(body[6:]) == door)
    assert create_index < rotation_index
    host.call('debug_enter_instance', [first_guid, instance])
    restored = first.collect()
    assert [values(restored, creature)[i] for i in (37, 38, 39)] == [weapon, 0, weapon]
    second.collect()
    outside.collect()
    host.call('debug_encounter_equip', [creature, 0, 5090050, 0])
    swapped = values(first.collect(), creature)
    assert [swapped[i] for i in (37, 38, 39)] == [0, weapon, 0]
    second.collect()
    host.call('debug_encounter_equip', [creature, 0, 0, 0])
    cleared = values(first.collect(), creature)
    assert [cleared[i] for i in (37, 38, 39)] == [0, 0, 0]
    second.collect()
    assert not values(outside.collect(), creature)
    first.send(0x101, struct.pack('<I', 1))
    assert (0x29d, b'\x01') in first.collect()
    assert not any(opcode == 0x29d for opcode, _ in second.collect() + outside.collect())
    host.call('debug_apply_damage', [first_guid, 1, 0])
    assert (0x29d, b'\0') in first.collect()
    assert not any(opcode == 0x29d for opcode, _ in second.collect() + outside.collect())
    host.call('debug_set_level', [first_guid, 20])
    host.call('debug_set_skill', [first_guid, 762, 75])
    first.collect()
    second.collect()
    host.call('debug_force_cast', [first_guid, 50310])
    mounted = first.collect()
    observed = second.collect()
    own_speed = [struct.unpack('<f', body[-4:])[0] for opcode, body in mounted if opcode == 0xe2 and packed_guid(body) == first_guid]
    peer_speed = [struct.unpack('<f', body[-4:])[0] for opcode, body in observed if opcode == 0x2fe and packed_guid(body) == first_guid]
    assert own_speed and all(abs(speed - 11.2) < 0.0001 for speed in own_speed), own_speed
    assert peer_speed == own_speed and not any(opcode == 0xe2 for opcode, _ in observed)
    assert not any(opcode in (0xe2, 0x2fe) for opcode, _ in outside.collect())
    first.send(0x136, struct.pack('<I', 50310))
    dismounted = first.collect()
    observed = second.collect()
    assert [struct.unpack('<f', body[-4:])[0] for opcode, body in dismounted if opcode == 0xe2 and packed_guid(body) == first_guid] == [7.0]
    assert [struct.unpack('<f', body[-4:])[0] for opcode, body in observed if opcode == 0x2fe and packed_guid(body) == first_guid] == [7.0]
    assert not any(opcode == 0xe2 for opcode, _ in observed)
    host.call('debug_teleport', [outside_guid, 0, 3300, 1000, 0, 0])
    assert door in destroys(outside.collect())
    host.call('debug_teleport', [outside_guid, 0, 3000, 1000, 0, 0])
    assert door in creates(outside.collect())

    ground_spell = 5090640
    for table, field, changes in [
        ('game_spell', 'spell_id', dict(spell_id=ground_spell, name='Native Ground', cost=0, cast_time_ms=0, gcd_ms=0, cooldown_ms=0, duration_ms=10000, attributes=0)),
        ('game_spell_effect', 'spell_id', dict(id=ground_spell << 2, spell_id=ground_spell, effect_index=0, kind=27, target=1, base_points=0, die_sides=0, per_level=0, period_ms=1000, radius_yd=10, enters_combat=False)),
    ]:
        source = host.sql(f'SELECT * FROM {table} WHERE {field} = 133')[0]
        row = dict(zip([column['name']['some'] for column in source['schema']['elements']], source['rows'][0]))
        row.update(changes)
        literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in row.values()]
        host.sql(f"INSERT INTO {table} ({','.join(row)}) VALUES ({','.join(literals)})")
    host.call('debug_learn_spell', [first_guid, ground_spell])
    first.collect()
    first.send(0x12e, cast(ground_spell))
    appeared = first.collect()
    objects = host.rows(f'SELECT guid FROM game_dynamic_object WHERE spell_id = {ground_spell}')
    assert len(objects) == 1, (objects, appeared)
    ground = objects[0][0]
    assert ground in creates(appeared) and ground in creates(second.collect())
    assert ground not in creates(outside.collect())
    assert host.rows(f'SELECT entity_guid FROM game_aoi_grant WHERE viewer_guid = {first_guid} AND layer = 3') == [[ground]]
    original_grant = host.rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {first_guid} AND layer = 3 AND entity_guid = {ground}')
    host.sql(f'UPDATE game_dynamic_object SET cell = 0 WHERE guid = {ground}')
    host.call('debug_enter_instance', [second_guid, 0])
    second.collect()
    host.call('debug_enter_instance', [second_guid, instance])
    assert ground not in creates(second.collect())
    host.call('debug_backfill_cell_ids', [])
    assert ground in creates(second.collect())
    assert ground not in creates(first.collect())
    assert host.rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {first_guid} AND layer = 3 AND entity_guid = {ground}') == original_grant
    host.call('debug_enter_instance', [second_guid, 0])
    assert ground not in creates(second.collect())
    host.call('debug_enter_instance', [second_guid, instance])
    assert ground in creates(second.collect())
    first.collect()
    deadline = time.monotonic() + 15
    expired_first, expired_second = [], []
    while host.rows(f'SELECT guid FROM game_dynamic_object WHERE guid = {ground}'):
        assert time.monotonic() < deadline, 'ground effect did not expire'
        expired_first += first.collect()
        expired_second += second.collect()
        time.sleep(0.05)
    expired_first += first.collect()
    expired_second += second.collect()
    assert ground in destroys(expired_first) and ground in destroys(expired_second)
    assert not host.rows(f'SELECT id FROM game_aoi_grant WHERE layer = 3 AND entity_guid = {ground}')
    assert ground not in creates(outside.collect())

    host.call('debug_encounter_move', [creature, sx + 20, sy, sz, False])
    first_moves = splines(first.collect(), creature)
    assert len(first_moves) == 1 and first_moves == splines(second.collect(), creature)
    assert first_moves[0]['start'] == (sx, sy, sz)
    assert first_moves[0]['destination'] == (sx + 20, sy, sz)
    assert first_moves[0]['duration'] == 8000 and first_moves[0]['flags'] == 0
    assert not splines(outside.collect(), creature)

    host.call('debug_enter_instance', [second_guid, 0])
    second.collect()
    first.collect()
    host.call('debug_creature_waypoints', [creature, [[sx + 20, sy, sz], [sx + 20, sy + 20, sz]], False])
    curved = splines(first.collect(), creature)
    assert len(curved) == 1 and curved[0]['count'] == 2
    assert curved[0]['id'] > first_moves[0]['id']
    assert curved[0]['destination'] == (sx + 20, sy + 20, sz)
    assert not splines(second.collect() + outside.collect(), creature)
    host.call('debug_enter_instance', [second_guid, instance])
    joined = second.collect()
    assert creature in creates(joined)
    replay = splines(joined, creature)
    assert len(replay) == 1 and replay[0]['id'] == curved[0]['id']
    assert 0 < replay[0]['duration'] < curved[0]['duration']
    assert replay[0]['start'][0] > curved[0]['start'][0]
    create_index = next(i for i, (opcode, body) in enumerate(joined) if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == creature)
    move_index = next(i for i, (opcode, body) in enumerate(joined) if opcode == 0xdd and packed_guid(body) == creature)
    assert create_index < move_index
    first.collect()

    host.call('debug_creature_face', [creature, sx + 20, sy + 40])
    facing = splines(first.collect(), creature)
    assert len(facing) == 2 and facing[0]['kind'] == 0 and facing[0]['duration'] == 0
    assert facing[1]['kind'] == 4 and facing[1]['duration'] == 0
    assert facing[1]['id'] > facing[0]['id'] > curved[0]['id']
    assert facing == splines(second.collect(), creature)
    assert not splines(outside.collect(), creature)
    host.call('debug_enter_instance', [second_guid, 0])
    second.collect()
    host.call('debug_enter_instance', [second_guid, instance])
    assert splines(second.collect(), creature) == [facing[1]]
    first.collect()

    host.call('debug_teleport', [first_guid, 1, 3000, 1000, 0, 0])
    first.collect()
    second.collect()
    host.call('debug_creature_path', [creature, sx + 20, sy + 20, sz, True])
    running = splines(second.collect(), creature)
    assert len(running) == 1 and running[0]['flags'] == 0x100
    assert not splines(first.collect() + outside.collect(), creature)
    host.call('debug_enter_instance', [second_guid, 0])
    second.collect()
    host.call('debug_reap_instance', [instance])
    assert not host.rows(f'SELECT guid FROM game_creature_spline WHERE guid = {creature}')
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {creature}')


def check_transports(host):

    entry, path = 5090400, 5090401
    transport = (0x1FC0 << 48) | entry

    def transport_create(packets):
        for index, (opcode, body) in enumerate(packets):
            if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == transport:
                offset = 7 + body[6].bit_count()
                assert body[4] == 1 and body[offset:offset + 2] == bytes([5, 0x52])
                return index, struct.unpack_from('<I', body, offset + 2 + 16 + 4)[0]
        return None

    def peer_movement(packets, guid):
        for index, (opcode, body) in enumerate(packets):
            if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == guid:
                offset = 7 + body[6].bit_count()
                assert body[offset] == 4
                return index, body[offset + 2:offset + 2 + 48]
        return None

    def heartbeat(time, x, y, offset=None):
        flags = 0x02000000 if offset else 0
        tail = struct.pack('<Qffff', transport, *offset) if offset else b''
        return struct.pack('<IIffff', flags, time, x, y, 0, 0) + tail + struct.pack('<I', 0)

    def until(predicate):
        for _ in range(60):
            if predicate():
                return
            time.sleep(0.05)
        raise AssertionError('Transport state did not settle')

    for index, (x, flags, delay) in enumerate([(6995, 0, 0), (7000, 2, 10), (7005, 0, 0), (7010, 0, 0),
                                               (7015, 2, 10), (7020, 0, 0), (7025, 0, 0)]):
        host.sql(f'INSERT INTO game_taxi_path_node (id,path_id,node_index,map_id,x,y,z,flags,delay_ms) '
            f'VALUES ({path + 1 + index},{path},{index},1,{x},7000,0,{flags},{delay * 1000})')
    host.sql(f"INSERT INTO game_transport (entry,name,display_id,size,flags,taxi_path_id,move_speed,accel_rate,"
        f"period_ms,map_id) VALUES ({entry},'Native ferry',3015,1,40,{path},10,5,40000,1)")
    worlds = {}
    try:
        def enter(name):
            world = host.enter_world(name, at=(1, 7030, 7010, 0))
            worlds[name] = (world, world.guid)
            return world, world.guid, world.entered

        passenger, passenger_guid, entered = enter('Passenger')
        _, progress = transport_create(entered)
        assert progress < 40000
        passenger.send(0x5e, struct.pack('<IQ', entry, transport))
        response = next(body for opcode, body in passenger.collect() if opcode == 0x5f)
        assert struct.unpack_from('<III', response) == (entry, 15, 3015)
        assert struct.unpack_from('<III', response, len(response) - 96) == (path, 10, 5)
        watcher, _, entered = enter('Watcher')
        assert transport_create(entered)
        passenger.collect()

        passenger.send(0xee, heartbeat(10000, 7060, 7000.5, (2, 1, 0, 0)))
        passenger.collect()
        assert not host.rows(f'SELECT character_guid FROM game_transport_passenger WHERE character_guid = {passenger_guid}')
        passenger.send(0xee, heartbeat(11000, 7008, 7000.5, (2, 1, 0, 0)))
        passenger.collect()
        assert host.rows(f'SELECT transport_entry, x, y FROM game_transport_passenger WHERE character_guid = {passenger_guid}') == [[entry, 2.0, 1.0]]
        relayed = []
        until(lambda: relayed.extend(watcher.collect()) or any(
            opcode == 0xee and packed_guid(body) == passenger_guid for opcode, body in relayed))
        body = next(body for opcode, body in relayed if opcode == 0xee and packed_guid(body) == passenger_guid)
        tail = body[1 + body[0].bit_count():]
        assert struct.unpack_from('<I', tail)[0] & 0x02000000
        assert struct.unpack_from('<Q', tail, 24)[0] == transport
        until(lambda: abs(host.rows(f'SELECT y FROM game_world_entity WHERE guid = {passenger_guid}')[0][0] - 6999) < 0.01)
        x = host.rows(f'SELECT x FROM game_world_entity WHERE guid = {passenger_guid}')[0][0]
        assert 6997 <= x <= 7019, x
        passenger.send(0xee, heartbeat(11500, 100, 100, (500, 0, 0, 0)))
        passenger.collect()
        assert host.rows(f'SELECT x, y FROM game_transport_passenger WHERE character_guid = {passenger_guid}') == [[2.0, 1.0]]
        assert host.rows(f'SELECT x FROM game_world_entity WHERE guid = {passenger_guid}')[0][0] != 100

        _, _, entered = enter('Latecomer')
        transport_index, _ = transport_create(entered)
        passenger_index, movement = peer_movement(entered, passenger_guid)
        assert transport_index < passenger_index
        flags, _, _, _, _, _, carried_by, *offset = struct.unpack('<IIffffQffff', movement)
        assert flags & 0x02000000 and carried_by == transport and offset == [2, 1, 0, 0]

        passenger.send(0xee, heartbeat(12000, 7010, 7020))
        passenger.collect()
        assert not host.rows(f'SELECT character_guid FROM game_transport_passenger WHERE character_guid = {passenger_guid}')
        time.sleep(0.6)
        assert host.rows(f'SELECT x, y FROM game_world_entity WHERE guid = {passenger_guid}') == [[7010.0, 7020.0]]
        assert not host.rows(f'SELECT guid FROM game_movement_violation WHERE guid = {passenger_guid}')

        passenger.send(0xee, heartbeat(13000, 7008, 7000.5, (2, 1, 0, 0)))
        passenger.collect()
        assert host.rows(f'SELECT character_guid FROM game_transport_passenger WHERE character_guid = {passenger_guid}')
        passenger.send(0x4b)
        assert [opcode for opcode, _ in passenger.collect()][-2:] == [0x4c, 0x4d]
        assert not host.rows(f'SELECT character_guid FROM game_transport_passenger WHERE character_guid = {passenger_guid}')
        until(lambda: not host.rows(f'SELECT scheduled_id FROM game_transport_carry_schedule WHERE scheduled_id = {entry}'))
    finally:
        host.sql(f'DELETE FROM game_transport WHERE entry = {entry}')
        host.sql(f'DELETE FROM game_taxi_path_node WHERE path_id = {path}')



def check_transport_crossings(host):

    entry, path = 5090410, 5090411
    transport = (0x1FC0 << 48) | entry

    def creates(packets, guid):
        return [index for index, (opcode, body) in enumerate(packets)
                if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == guid]

    def crossing_map():
        rows = host.rows(f'SELECT map_id FROM game_transport_crossing WHERE transport_entry = {entry}')
        return rows[0][0] if rows else None

    def until(predicate, seconds):
        deadline = time.time() + seconds
        while time.time() < deadline:
            if predicate():
                return
            time.sleep(0.05)
        raise AssertionError('Transport crossing did not settle')

    nodes = [(0, 6980, 0, 0), (0, 7000, 2, 3), (0, 7020, 0, 0), (0, 7040, 0, 0), (0, 7060, 0, 0),
             (1, 7080, 0, 0), (1, 7100, 0, 0), (1, 7120, 2, 3), (1, 7140, 0, 0), (1, 7160, 0, 0)]
    for index, (map_id, x, flags, delay) in enumerate(nodes):
        host.sql(f'INSERT INTO game_taxi_path_node (id,path_id,node_index,map_id,x,y,z,flags,delay_ms) '
            f'VALUES ({path + 1 + index},{path},{index},{map_id},{x},7000,0,{flags},{delay * 1000})')
    host.sql(f"INSERT INTO game_transport (entry,name,display_id,size,flags,taxi_path_id,move_speed,accel_rate,"
        f"period_ms,map_id) VALUES ({entry},'Native crossing',3015,1,40,{path},10,5,24000,0)")
    try:
        sailor = host.enter_world('Sailor', at=(0, 7020, 7000, 0))
        docker = host.enter_world('Docker', at=(0, 6900, 7000, 0))
        lander = host.enter_world('Lander', at=(1, 7200, 7000, 0))
        until(lambda: crossing_map() == 0, 30)
        for world in (sailor, docker, lander):
            world.collect()

        heartbeat = struct.pack('<IIffff', 0x02000000, 10000, 7020, 7000.5, 0, 0)
        sailor.send(0xee, heartbeat + struct.pack('<QffffI', transport, 2, 1, 0, 0, 0))
        sailor.collect()
        assert host.rows(f'SELECT transport_entry FROM game_transport_passenger WHERE character_guid = {sailor.guid}') == [[entry]]

        departing = []
        until(lambda: departing.extend(sailor.collect(auto_ack=False)) or any(opcode == 0x3e for opcode, _ in departing), 30)
        pending = next(body for opcode, body in departing if opcode == 0x3f)
        assert struct.unpack('<III', pending) == (1, entry, 0)
        new_world = next(body for opcode, body in departing if opcode == 0x3e)
        assert struct.unpack('<Iffff', new_world) == (1, 2, 1, 0, 0)
        assert crossing_map() == 1
        assert (0xaa, struct.pack('<Q', transport)) in docker.collect()
        assert creates(lander.collect(), transport)
        assert host.rows(f'SELECT transport_entry FROM game_transport_passenger WHERE character_guid = {sailor.guid}') == [[entry]]

        until(lambda: crossing_map() == 0, 30)
        assert host.rows(f'SELECT transport_entry FROM game_transport_passenger WHERE character_guid = {sailor.guid}') == [[entry]]
        sailor.send(0xdc)
        stranded = sailor.collect(auto_ack=False)
        assert (0x3f, struct.pack('<III', 0, entry, 1)) in stranded
        assert (0x3e, struct.pack('<Iffff', 0, 2, 1, 0, 0)) in stranded
        seen = lander.collect()
        stranded_create = seen[creates(seen, sailor.guid)[0]][1]
        offset = 7 + stranded_create[6].bit_count()
        assert not struct.unpack_from('<I', stranded_create, offset + 2)[0] & 0x02000000

        sailor.send(0xdc)
        arrived = sailor.collect()
        assert creates(arrived, transport)[0] < creates(arrived, sailor.guid)[0]
        own = arrived[creates(arrived, sailor.guid)[0]][1]
        offset = 7 + own[6].bit_count()
        assert own[offset] == 4
        flags, _, _, _, _, _, carried_by, *carried_at = struct.unpack_from('<IIffffQffff', own, offset + 2)
        assert flags & 0x02000000 and carried_by == transport and carried_at == [2, 1, 0, 0]
        assert len(creates(arrived, transport)) == 1
        until(lambda: host.rows(f'SELECT map_id FROM game_world_entity WHERE guid = {sailor.guid}') == [[0]], 3)
        until(lambda: 6950 <= host.rows(f'SELECT x FROM game_world_entity WHERE guid = {sailor.guid}')[0][0] <= 7070, 3)
        assert host.rows(f'SELECT transport_entry FROM game_transport_passenger WHERE character_guid = {sailor.guid}') == [[entry]]
        assert not host.rows(f'SELECT guid FROM game_movement_violation WHERE guid = {sailor.guid}')

        x, y = host.rows(f'SELECT x, y FROM game_world_entity WHERE guid = {sailor.guid}')[0]
        sailor.send(0xee, struct.pack('<IIffffI', 0, 20000, x, y, 0, 0, 0))
        sailor.collect()
        assert not host.rows(f'SELECT character_guid FROM game_transport_passenger WHERE character_guid = {sailor.guid}')

        docker.collect()
        host.sql(f'DELETE FROM game_transport WHERE entry = {entry}')
        until(lambda: crossing_map() is None, 30)
        assert (0xaa, struct.pack('<Q', transport)) in docker.collect()
    finally:
        host.sql(f'DELETE FROM game_transport WHERE entry = {entry}')
        host.sql(f'DELETE FROM game_taxi_path_node WHERE path_id = {path}')


def check_areatriggers(host):

    sphere, box, dungeon, exit_trigger, exploration = range(5090630, 5090635)
    for trigger, map_id, x, radius, length, width, height, yaw in [
        (sphere, 0, 1000, 5, 0, 0, 0, 0),
        (box, 0, 1100, 0, 8, 2, 4, 1.5707964),
        (dungeon, 0, 1000, 5, 0, 0, 0, 0),
        (exit_trigger, 36, 1000, 10, 0, 0, 0, 0),
        (exploration, 0, 1000, 5, 0, 0, 0, 0),
    ]:
        host.sql(f'INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) VALUES ({trigger},{map_id},{x},1000,0,{radius},{length},{width},{height},{yaw})')
    for trigger, target_map, x in [(sphere, 0, 1100), (box, 1, 1000), (dungeon, 36, 1000), (exit_trigger, 0, 1000)]:
        host.sql(f"INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) VALUES ({trigger},{target_map},{x},1000,0,0,'Native trigger')")
    template = host.sql('SELECT * FROM game_quest_template WHERE entry = 50900')[0]
    quest = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    quest.update(entry=sphere, title='Native exploration', min_level=1, quest_level=1,
                 prev_quest_id=0, required_races=0, required_classes=0, limit_time=0, src_item=0, src_item_count=0)
    literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in quest.values()]
    host.sql(f"INSERT INTO game_quest_template ({','.join(quest)}) VALUES ({','.join(literals)})")
    for index, trigger in enumerate([sphere, exploration]):
        host.sql(f'INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) VALUES ({sphere + index},{sphere},{index},3,{trigger},1)')
    worlds = []
    for name in ['Areaone', 'Areatwo', 'Areathree']:
        world = host.enter_world(name, at=(0, 1000, 1000, 0))
        worlds.append((world, world.guid))
        host.call('debug_set_level', [world.guid, 60])
    (first, first_guid), (second, second_guid), (solo, solo_guid) = worlds
    host.call('debug_grant_quest', [first_guid, sphere])
    for world, _ in worlds:
        world.collect()

    def enter(world, trigger):
        world.send(0xb4, struct.pack('<I', trigger))
        return world.collect()

    def location(guid):
        return host.rows(f'SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {guid}')[0]

    def counts():
        return host.rows(f'SELECT counts FROM game_character_quest WHERE character_guid = {first_guid} AND quest_entry = {sphere}')[0][0]

    def no_port(packets):
        assert not any(opcode in (0xc7, 0x3e, 0x3f) for opcode, _ in packets)

    no_port(enter(first, 0))
    host.sql(f'UPDATE game_area_trigger SET map_id = 1 WHERE id = {sphere}')
    no_port(enter(first, sphere))
    host.sql(f'UPDATE game_area_trigger SET map_id = 0 WHERE id = {sphere}')
    assert counts() == [0, 0]
    for x, y, z in [(1005.1, 1000, 0), (1000, 1000, 5.1)]:
        host.call('debug_teleport', [first_guid, 0, x, y, z, 0])
        first.collect()
        no_port(enter(first, sphere))
        assert counts() == [0, 0]
    host.call('debug_teleport', [first_guid, 0, 1003, 1004, 0, 0])
    first.collect()
    packets = enter(first, sphere)
    assert any(opcode == 0xc7 for opcode, _ in packets)
    assert location(first_guid) == [0, 0, 1100, 1000, 0] and counts() == [1, 0]
    host.call('debug_teleport', [first_guid, 0, 1101.1, 1000, 0, 0])
    first.collect()
    no_port(enter(first, box))
    host.call('debug_teleport', [first_guid, 0, 1100, 1003.5, 1.5, 0])
    first.collect()
    packets = enter(first, box)
    assert any(opcode == 0x3e for opcode, _ in packets) and first_guid in creates(packets)
    assert location(first_guid) == [1, 0, 1000, 1000, 0]
    host.call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
    first.collect()
    packets = enter(first, exploration)
    no_port(packets)
    assert counts() == [1, 1]
    assert (0x198, struct.pack('<I', sphere)) in packets
    assert (0x198, struct.pack('<I', sphere)) not in enter(first, exploration)

    first.send(0x6e, b'Areatwo\0')
    first.collect()
    second.collect()
    second.send(0x72)
    second.collect()
    first.collect()
    group = host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {first_guid}')[0][0]
    assert host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {second_guid}') == [[group]]
    packets = enter(first, dungeon)
    assert any(opcode == 0x3e for opcode, _ in packets)
    party_instance = location(first_guid)[1]
    assert party_instance != 0 and location(first_guid) == [36, party_instance, 1000, 1000, 0]
    assert host.rows(f'SELECT map_id, party_id FROM game_instance WHERE instance_id = {party_instance}') == [[36, group]]
    assert first_guid in destroys(second.collect())
    solo.collect()
    packets = enter(second, dungeon)
    assert first_guid in creates(packets) and location(second_guid)[1] == party_instance
    assert second_guid in creates(first.collect())
    solo.collect()
    packets = enter(solo, dungeon)
    solo_instance = location(solo_guid)[1]
    assert solo_instance != 0 and solo_instance != party_instance
    assert not ({first_guid, second_guid} & set(creates(packets)))
    assert solo_guid not in creates(first.collect() + second.collect())
    assert not host.rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {solo_guid} AND entity_guid = {first_guid}')
    packets = enter(first, exit_trigger)
    assert any(opcode == 0x3e for opcode, _ in packets) and location(first_guid)[:2] == [0, 0]
    enter(first, dungeon)
    assert location(first_guid)[1] == party_instance
    assert host.rows(f'SELECT instance_id FROM game_instance_binding WHERE character_guid = {first_guid} AND map_id = 36') == [[party_instance]]

    def countdowns(packets):
        result = []
        for opcode, body in packets:
            if opcode == 0x286:
                timer, reason = struct.unpack('<II', body)
                assert reason == (timer > 0)
                result.append(timer)
        return result

    for world, _ in worlds:
        world.collect()
    second.send(0x7b)
    assert countdowns(second.collect()) == [60000]
    assert countdowns(first.collect()) == [60000]
    assert not countdowns(solo.collect())
    assert host.rows(f'SELECT character_guid FROM game_instance_removal WHERE character_guid = {first_guid}') == [[first_guid]]
    first.close()
    first = host.connect('AREAONE')
    worlds[0] = (first, first_guid)
    assert first.read()[0] == 0x1ee
    first.send(0x3d, struct.pack('<Q', first_guid))
    resumed = countdowns(first.collect())
    assert len(resumed) == 1 and 0 < resumed[0] < 60000, resumed
    assert location(first_guid)[1] == party_instance
    first.send(0x6e, b'Areatwo\0')
    invited_first = first.collect()
    invited_second = second.collect()
    second.send(0x72)
    assert countdowns(invited_second + second.collect()) == [0]
    assert countdowns(invited_first + first.collect()) == [0]
    assert not host.rows(f'SELECT character_guid FROM game_instance_removal WHERE character_guid = {first_guid} OR character_guid = {second_guid}')
    second.send(0x7b)
    assert countdowns(second.collect()) == [60000]
    assert countdowns(first.collect()) == [60000]
    host.call('debug_expire_instance_removal', [second_guid])
    expired = second.collect()
    assert countdowns(expired)[0] == 0 and any(opcode == 0x3e for opcode, _ in expired)
    assert location(second_guid)[:2] == [0, 0]
    assert not host.rows(f'SELECT character_guid FROM game_instance_removal WHERE character_guid = {second_guid}')
    assert not countdowns(first.collect() + solo.collect())
    for world, guid in worlds:
        enter(world, exit_trigger)
        assert location(guid)[:2] == [0, 0]


def check_pets(host):

    worlds = []
    for name, race, character_class in [('Petone', 3, 3), ('Pettwo', 1, 1)]:
        world = host.enter_world(name, race, character_class, at=(0, 1000, 1000, 0))
        worlds.append((world, world.guid))
    (owner, owner_guid), (observer, observer_guid) = worlds
    host.call('debug_set_level', [owner_guid, 60])
    host.restore('game_creature_template', 'entry = 51006', 'creature_family', 'level', 'health', 'npc_flags')
    host.sql('UPDATE game_creature_template SET creature_family = 5, level = 5, health = 100, npc_flags = 0 WHERE entry = 51006')
    wild = spawn(host, owner_guid, 51006, 5)
    owner.collect()
    observer.collect()
    host.call('debug_force_cast_at', [owner_guid, 50300, wild])
    pet = host.rows(f'SELECT live_pet_guid FROM game_hunter_pet_protocol WHERE owner_guid = {owner_guid}')[0][0]
    owned, observed = owner.collect(), observer.collect()
    assert pet in creates(owned) and pet in creates(observed)
    assert wild in destroys(owned) and wild in destroys(observed)
    bar = next(index for index, (opcode, body) in enumerate(owned) if opcode == 0x179 and body[:8] == struct.pack('<Q', pet))
    assert next(index for index, packet in enumerate(owned) if pet in creates([packet])) < bar
    assert not any(opcode == 0x179 for opcode, _ in observed)
    summon = values(owned, owner_guid)
    assert summon[8] | summon[9] << 32 == pet
    owner_values, observer_values = values(owned, pet), values(observed, pet)
    assert owner_values[139] == observer_values[139] == owner_guid
    assert owner_values[140] == observer_values[140]
    assert owner_values[27] > 0 and owner_values[33] == 1000000 and owner_values[141] == 0
    assert all(field not in observer_values for field in (27, 33, 141, 142))
    old_stamp = owner_values[140]

    def rename(world, name, guid=pet):
        world.send(0x177, struct.pack('<Q', guid) + name.encode() + b'\0')
        return world.collect()

    assert (0x178, b'') not in rename(observer, 'F1')
    assert (0x178, b'') in rename(owner, 'F1')
    assert (0x178, b'') in rename(owner, 'F')
    assert host.rows(f'SELECT rename_allowed FROM game_hunter_pet WHERE owner_guid = {owner_guid}') == [[True]]
    renamed = rename(owner, 'Fang')
    observed = observer.collect()
    current = host.rows(f'SELECT name, rename_allowed, name_timestamp FROM game_hunter_pet WHERE owner_guid = {owner_guid}')[0]
    assert current[0:2] == ['Fang', False] and current[2] > old_stamp
    assert values(renamed, pet)[140] == values(observed, pet)[140] == current[2]
    assert not values(renamed, pet)[46] & 0x10
    assert all(field not in values(observed, pet) for field in (27, 33, 141, 142))
    observer.send(0x52, struct.pack('<IQ', owner_guid, pet))
    assert (0x53, struct.pack('<I', owner_guid) + b'Fang\0' + struct.pack('<I', current[2])) in observer.collect()
    assert (0x178, b'') not in rename(owner, 'F1')
    assert host.rows(f'SELECT name FROM game_hunter_pet WHERE owner_guid = {owner_guid}') == [['Fang']]

    baseline = host.rows(f'SELECT happiness FROM game_hunter_pet WHERE owner_guid = {owner_guid}')[0][0]
    owner.collect()
    observer.collect()
    deadline = time.monotonic() + 16
    changed = None
    while changed is None:
        updates = values(owner.collect(), pet)
        if 27 in updates and updates[27] < baseline:
            changed = updates[27]
        assert time.monotonic() < deadline, 'pet care did not update the owner'
        if changed is None:
            time.sleep(0.05)
    assert changed < baseline
    assert all(field not in values(observer.collect(), pet) for field in (27, 33, 141, 142))
    host.call('debug_teleport', [observer_guid, 0, 2000, 1000, 0, 0])
    assert pet in destroys(observer.collect())
    host.call('debug_teleport', [observer_guid, 0, 1000, 1000, 0, 0])
    replayed = observer.collect()
    assert pet in creates(replayed)
    assert values(replayed, pet)[140] == current[2]
    assert not any(opcode == 0x179 for opcode, _ in replayed)
    assert all(field not in values(replayed, pet) for field in (27, 33, 141, 142))
    owner.send(0x175, struct.pack('<QIQ', pet, 0x07000003, 0))
    dismissed, observed = owner.collect(), observer.collect()
    assert pet in destroys(dismissed) and pet in destroys(observed)
    assert (0x179, bytes(8)) in dismissed
    assert next(index for index, packet in enumerate(dismissed) if pet in destroys([packet])) < dismissed.index((0x179, bytes(8)))
    assert not any(opcode == 0x179 for opcode, _ in observed)
    assert values(dismissed, owner_guid)[8] == values(dismissed, owner_guid)[9] == 0
    assert host.rows(f'SELECT name FROM game_hunter_pet WHERE owner_guid = {owner_guid}') == [['Fang']]
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {pet}')


def check_auctions(host):

    def auction_packets(packets):
        return [packet for packet in packets if packet[0] in (0x255, 0x25b, 0x25c, 0x25d, 0x25e, 0x25f, 0x265, 0x28d)]

    def result(packets, auction_id, action, code, extra=b''):
        expected = (0x25b, struct.pack('<III', auction_id, action, code) + extra)
        assert expected in packets, auction_packets(packets)

    template = host.sql('SELECT * FROM game_item_template WHERE entry = 51')[0]
    item = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    item.update(entry=5090620, name='Auction Test Sword')
    item.update({'class': 2, 'subclass': 7, 'inventory_type': 13, 'quality': 2,
                 'max_stack': 1, 'max_count': 0, 'bonding': 0, 'required_level': 1,
                 'sell_price': 1000, 'max_durability': 30})
    literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in item.values()]
    host.sql(f"INSERT INTO game_item_template ({','.join(item)}) VALUES ({','.join(literals)})")
    for house in [1, 2, 7]:
        if not host.rows(f'SELECT id FROM game_auction_house WHERE id = {house}'):
            host.sql(f"INSERT INTO game_auction_house (id,faction,deposit_rate,consignment_rate,name) VALUES ({house},0,5,5,'Native Auction')")
    for faction in [12, 55, 120]:
        host.restore('game_faction_template', f'id = {faction}')
        if not host.rows(f'SELECT id FROM game_faction_template WHERE id = {faction}'):
            host.sql(f'INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES ({faction},0,0,0,0,0,0,0,0,0,0,0,0)')
    worlds = []
    for name, character_class in [('Auctionone', 1), ('Auctiontwo', 8), ('Auctionthree', 1)]:
        guid = host.character(name, cls=character_class, at=(0, 1000, 1000, 0))
        host.sql(f'DELETE FROM game_item_instance WHERE owner_guid = {guid} AND slot >= 23 AND slot <= 38')
        worlds.append((host.log_in(guid), guid))
        host.call('debug_set_money', [guid, 10000])
    (seller, seller_guid), (bidder, bidder_guid), (winner, winner_guid) = worlds
    npc = spawn(host, seller_guid, 51004, 2)
    host.sql(f'UPDATE game_world_entity SET npc_flags = 4096, faction_template = 12 WHERE guid = {npc}')
    for world, _ in worlds:
        world.collect()

    def purse(guid):
        return host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]

    def grant():
        host.call('debug_grant_item', [seller_guid, 5090620, 1])
        guid = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {seller_guid} AND entry = 5090620')[0][0]
        host.sql(f'UPDATE game_item_instance SET durability = 17, enchant_id = 7745, random_property_id = 1900 WHERE guid = {guid}')
        seller.collect()
        return guid

    def sell(item_guid, start=100, buyout=500, duration=120, auctioneer=npc):
        seller.send(0x256, struct.pack('<QQIII', auctioneer, item_guid, start, buyout, duration))
        return seller.collect()

    def listing(item_guid):
        packets = sell(item_guid)
        auctions = host.rows(f'SELECT id FROM game_auction WHERE item_guid = {item_guid}')
        assert auctions, (
            auction_packets(packets), purse(seller_guid),
            host.rows(f'SELECT slot, soulbound FROM game_item_instance WHERE guid = {item_guid}'),
            host.rows(f'SELECT x, y, map_id, instance_id, npc_flags, faction_template, dead FROM game_world_entity WHERE guid = {npc}'),
        )
        auction_id = auctions[0][0]
        result(packets, auction_id, 0, 0)
        assert item_guid in destroys(packets)
        return auction_id

    def bid(world, auction_id, offer):
        world.send(0x25a, struct.pack('<QII', npc, auction_id, offer))
        return world.collect()

    def cancel(world, auction_id):
        world.send(0x257, struct.pack('<QI', npc, auction_id))
        return world.collect()

    def page(world, opcode=0x258, offset=0, name='', minimum=0, maximum=0,
             inventory=0xffffffff, item_class=0xffffffff, subclass=0xffffffff, quality=0xffffffff, usable=False, auctioneer=npc):
        body = struct.pack('<QI', auctioneer, offset)
        if opcode == 0x258:
            body += name.encode() + b'\0' + struct.pack('<BBIIIIB', minimum, maximum, inventory, item_class, subclass, quality, usable)
        if opcode == 0x264:
            body += bytes(4)
        world.send(opcode, body)
        expected = {0x258: 0x25c, 0x259: 0x25d, 0x264: 0x265}[opcode]
        body = next(body for received, body in world.collect() if received == expected)
        count = struct.unpack_from('<I', body)[0]
        assert len(body) == 8 + count * 64, body.hex()
        return [struct.unpack_from('<7IQ4IQI', body, 4 + index * 64) for index in range(count)], struct.unpack_from('<I', body, 4 + count * 64)[0]

    seller.send(0x255, struct.pack('<Q', npc))
    assert (0x255, struct.pack('<QI', npc, 1)) in seller.collect()
    assert page(seller) == ([], 0)
    for column, invalid, original in [('npc_flags', 0, 4096), ('map_id', 1, 0), ('instance_id', 99, 0), ('dead', 'true', 'false')]:
        host.sql(f'UPDATE game_world_entity SET {column} = {invalid} WHERE guid = {npc}')
        seller.send(0x255, struct.pack('<Q', npc))
        assert not auction_packets(seller.collect())
        assert page(seller) == ([], 0)
        host.sql(f'UPDATE game_world_entity SET {column} = {original} WHERE guid = {npc}')
    host.call('debug_teleport', [seller_guid, 0, 1020, 1000, 0, 0])
    seller.collect()
    seller.send(0x255, struct.pack('<Q', npc))
    assert not auction_packets(seller.collect())
    assert page(seller) == ([], 0)
    host.call('debug_teleport', [seller_guid, 0, 1000, 1000, 0, 0])
    seller.collect()
    item_guid = grant()
    result(sell(item_guid, auctioneer=0), 0, 0, 2)
    for args in [dict(start=0), dict(buyout=50), dict(duration=121)]:
        result(sell(item_guid, **args), 0, 0, 2)
    host.sql(f'UPDATE game_item_instance SET soulbound = true WHERE guid = {item_guid}')
    result(sell(item_guid), 0, 0, 4)
    host.sql(f'UPDATE game_item_instance SET soulbound = false WHERE guid = {item_guid}')
    host.call('debug_set_money', [seller_guid, 49])
    result(sell(item_guid), 0, 0, 3)
    assert host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {item_guid}') and purse(seller_guid) == 49
    host.call('debug_set_money', [seller_guid, 10000])
    auction_id = listing(item_guid)
    assert purse(seller_guid) == 9950
    assert host.rows(f'SELECT deposit, item_durability, item_enchant_id, random_property_id FROM game_auction WHERE id = {auction_id}') == [[50, 17, 7745, 1900]]
    result(sell(item_guid), auction_id, 0, 0)
    assert purse(seller_guid) == 9950
    result(sell(item_guid, buyout=600), 0, 0, 4)
    assert len(host.rows(f'SELECT auction_id FROM game_auction_operation_receipt WHERE actor_guid = {seller_guid}')) == 1
    entries, total = page(bidder, name='tEsT sWoRd', minimum=1, maximum=1, inventory=13, item_class=2, subclass=7, quality=2)
    assert total == 1 and entries[0][:8] == (auction_id, 5090620, 823, 1900, 0, 1, 0, seller_guid)
    assert entries[0][8:11] == (100, 100, 500) and entries[0][12:] == (0, 0)
    for args in [dict(name='missing'), dict(minimum=2), dict(quality=3), dict(inventory=4), dict(item_class=4), dict(subclass=0), dict(usable=True)]:
        assert page(bidder, **args) == ([], 0)
    assert page(bidder, offset=1) == ([], 1)
    assert page(bidder, 0x259) == ([], 0)
    assert page(seller, 0x259)[1] == 1
    host.sql(f'UPDATE game_world_entity SET faction_template = 55 WHERE guid = {npc}')
    assert page(bidder)[1] == 1
    host.sql(f'UPDATE game_world_entity SET faction_template = 120 WHERE guid = {npc}')
    assert page(bidder) == ([], 0)
    result(bid(bidder, auction_id, 100), auction_id, 2, 4)
    result(cancel(seller, auction_id), 0, 1, 2)
    host.sql(f'UPDATE game_world_entity SET faction_template = 12 WHERE guid = {npc}')
    result(bid(seller, auction_id, 100), auction_id, 2, 10)
    result(bid(bidder, auction_id, 99), auction_id, 2, 7)
    result(bid(bidder, auction_id, 10001), auction_id, 2, 3)
    result(bid(bidder, auction_id, 100), auction_id, 2, 0, struct.pack('<I', 5))
    assert purse(bidder_guid) == 9900
    assert (0x25f, struct.pack('<IIIQII', auction_id, 100, 5, bidder_guid, 5090620, 1900)) in seller.collect()
    assert not auction_packets(winner.collect())
    assert page(bidder, 0x264)[1] == 1 and page(winner, 0x264) == ([], 0)
    result(bid(winner, auction_id, 100), auction_id, 2, 5, struct.pack('<QII', bidder_guid, 100, 5))
    result(bid(winner, auction_id, 104), auction_id, 2, 7)
    result(bid(winner, auction_id, 105), auction_id, 2, 0, struct.pack('<I', 6))
    assert purse(winner_guid) == 9895
    assert (0x25e, struct.pack('<IIQIIII', 1, auction_id, bidder_guid, 100, 5, 5090620, 1900)) in bidder.collect()
    assert host.rows(f'SELECT money FROM game_mail WHERE recipient_guid = {bidder_guid}') == [[100]]
    seller.collect()
    bought = bid(bidder, auction_id, 700)
    result(bought, auction_id, 2, 0, struct.pack('<I', 25))
    assert (0x25e, struct.pack('<IIQIIII', 1, auction_id, bidder_guid, 0, 25, 5090620, 1900)) in bought
    assert purse(bidder_guid) == 9400
    assert not host.rows(f'SELECT id FROM game_auction WHERE id = {auction_id}')
    assert not host.rows(f'SELECT auction_id FROM game_auction_expiry WHERE auction_id = {auction_id}')
    assert host.rows(f'SELECT money FROM game_mail WHERE recipient_guid = {seller_guid}') == [[525]]
    assert host.rows(f'SELECT item_entry, item_durability, item_enchant_id, random_property_id FROM game_mail WHERE recipient_guid = {bidder_guid} AND item_entry != 0') == [[5090620, 17, 7745, 1900]]
    assert host.rows(f'SELECT money FROM game_mail WHERE recipient_guid = {winner_guid}') == [[105]]
    assert (0x25f, struct.pack('<IIIQII', auction_id, 500, 25, 0, 5090620, 1900)) in seller.collect()
    winner.collect()
    result(bid(bidder, auction_id, 700), auction_id, 2, 4)
    assert purse(bidder_guid) == 9400

    cancelled = listing(grant())
    result(cancel(bidder, cancelled), 0, 1, 2)
    result(bid(winner, cancelled, 100), cancelled, 2, 0, struct.pack('<I', 5))
    seller.collect()
    host.call('debug_set_money', [seller_guid, 4])
    assert not auction_packets(cancel(seller, cancelled))
    assert host.rows(f'SELECT id FROM game_auction WHERE id = {cancelled}') and purse(seller_guid) == 4
    host.call('debug_set_money', [seller_guid, 1000])
    result(cancel(seller, cancelled), cancelled, 1, 0)
    assert purse(seller_guid) == 995
    assert (0x28d, struct.pack('<III', cancelled, 5090620, 1900)) in winner.collect()
    assert sorted(host.rows(f'SELECT money FROM game_mail WHERE recipient_guid = {winner_guid}')) == [[100], [105]]
    assert host.rows(f'SELECT item_entry FROM game_mail WHERE recipient_guid = {seller_guid} AND item_entry != 0') == [[5090620]]
    result(cancel(seller, cancelled), 0, 1, 2)
    assert purse(seller_guid) == 995

    for price in [0, 100]:
        expiring = listing(grant())
        if price:
            result(bid(winner, expiring, price), expiring, 2, 0, struct.pack('<I', 5))
            seller.collect()
        host.call('debug_auction_due', [expiring])
        deadline = time.monotonic() + 5
        while host.rows(f'SELECT id FROM game_auction WHERE id = {expiring}'):
            assert time.monotonic() < deadline, 'auction timer did not settle'
            time.sleep(0.05)
        assert (0x25f, struct.pack('<IIIQII', expiring, price, 5 if price else 1, 0, 5090620, 1900)) in seller.collect()
        packets = winner.collect()
        if price:
            assert (0x25e, struct.pack('<IIQIIII', 1, expiring, winner_guid, 0, 5, 5090620, 1900)) in packets
            assert (0x285, bytes(4)) in packets
        else:
            assert not auction_packets(packets)
    assert sorted(host.rows(f'SELECT money FROM game_mail WHERE recipient_guid = {seller_guid} AND money != 0')) == [[145], [525]]
    assert len(host.rows(f'SELECT id FROM game_mail WHERE recipient_guid = {seller_guid} AND item_entry != 0')) == 2
    assert host.rows(f'SELECT item_entry FROM game_mail WHERE recipient_guid = {winner_guid} AND item_entry != 0') == [[5090620]]
    assert not host.rows('SELECT * FROM game_auction_hold')
    assert not host.rows('SELECT * FROM game_auction_bid_hold')
    assert not host.rows('SELECT * FROM game_auction_bid_decision')


def check_mail(host):

    def mail_packets(packets):
        return [packet for packet in packets if packet[0] in (0x239, 0x23b, 0x244, 0x284, 0x285)]

    def result(packets, mail_id, action, code, extra=b''):
        expected = (0x239, struct.pack('<III', mail_id, action, code) + extra)
        assert expected in packets, mail_packets(packets)

    mailbox = 0xf110000000000a01
    host.call('debug_stage_meeting_stone', [5090610, mailbox, 1, 60, 1581, 0, 1001, 1000, 0])
    host.sql('UPDATE game_gameobject_template SET type_id = 19 WHERE entry = 5090610')
    host.sql("INSERT INTO game_item_template (entry,class,subclass,name,display_id,quality,inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race,random_property) VALUES (8383,0,0,'Plain Letter',0,0,0,0,0,0,0,0,1,0.0,0.0,0,0,0,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,1503,255,0)")
    worlds = []
    for name, race in [('Mailone', 1), ('Mailtwo', 1), ('Mailhorde', 2)]:
        account = host.account(name.upper())
        guid = host.character(name, race, account=account, at=(0, 1000, 1000, 0))
        host.sql(f'DELETE FROM game_item_instance WHERE owner_guid = {guid} AND slot >= 23 AND slot <= 38')
        worlds.append((host.log_in(guid), guid))
        host.call('debug_set_money', [guid, 1000])
        if name == 'Mailone':
            alt_guid = host.character('Mailalt', account=account)
    (first, first_guid), (second, second_guid), (watcher, _) = worlds
    for world, _ in worlds:
        world.collect()

    def send(name='Mailtwo', subject='letter', body='private body', money=0, cod=0, item=0, box=mailbox):
        first.send(0x238, struct.pack('<Q', box) + name.encode() + b'\0' + subject.encode() + b'\0' + body.encode() + b'\0' + struct.pack('<IIQIIII', 0, 0, item, money, cod, 0, 0))
        return first.collect()

    def request(world, opcode, mail_id, box=mailbox):
        world.send(opcode, struct.pack('<QI', box, mail_id) + (bytes(4) if opcode == 0x24a else b''))
        return world.collect()

    def text(world, mail_id, hint=0):
        world.send(0x243, struct.pack('<IQ', mail_id, hint))
        return [body for opcode, body in world.collect() if opcode == 0x244]

    def inbox(world):
        world.send(0x23a, struct.pack('<Q', mailbox))
        return [body for opcode, body in world.collect() if opcode == 0x23b][0]

    def purse(guid):
        return host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]

    second.send(0x284)
    empty = [body for opcode, body in second.collect() if opcode == 0x284]
    assert len(empty) == 1 and struct.unpack('<f', empty[0])[0] < 0
    assert inbox(second) == b'\0'
    assert not mail_packets(send(box=0))
    for name, code in [('Missingmail', 4), ('mAiLoNe', 2), ('Mailhorde', 5)]:
        result(send(name=name), 0, 0, code)
    host.call('debug_teleport', [first_guid, 0, 1020, 1000, 0, 0])
    first.collect()
    assert not mail_packets(send())
    host.call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
    first.collect()
    for column, value in [('map_id', 1), ('instance_id', 99)]:
        host.sql(f'UPDATE game_gameobject SET {column} = {value} WHERE guid = {mailbox}')
        assert not mail_packets(send())
        host.sql(f'UPDATE game_gameobject SET {column} = 0 WHERE guid = {mailbox}')
    assert purse(first_guid) == 1000

    result(send(name='mAiLtWo', money=130, cod=250), 0, 0, 0)
    assert purse(first_guid) == 840
    letter, cod, due = host.rows(f'SELECT id, cod, deliver_micros FROM game_mail WHERE recipient_guid = {second_guid}')[0]
    assert cod == 0 and due == 0
    assert (0x285, bytes(4)) in second.collect()
    assert not mail_packets(watcher.collect())
    listing = inbox(second)
    assert listing[0] == 1 and struct.unpack_from('<I', listing, 1)[0] == letter and b'letter\0' in listing
    assert text(first, letter) == [struct.pack('<I', letter) + b'\0']
    assert text(second, letter) == [struct.pack('<I', letter) + b'private body\0']
    for opcode, action in [(0x245, 1), (0x248, 3), (0x249, 4), (0x24a, 5)]:
        result(request(first, opcode, letter), letter, action, 6)
    request(first, 0x247, letter)
    assert host.rows(f'SELECT was_read FROM game_mail WHERE id = {letter}') == [[False]]
    request(second, 0x247, letter)
    assert host.rows(f'SELECT was_read FROM game_mail WHERE id = {letter}') == [[True]]
    second.send(0x284)
    assert struct.unpack('<f', [body for opcode, body in second.collect() if opcode == 0x284][0])[0] < 0
    result(request(second, 0x245, letter, box=0), letter, 1, 6)
    assert purse(second_guid) == 1000
    result(request(second, 0x245, letter), letter, 1, 0)
    assert purse(second_guid) == 1130
    result(request(second, 0x245, letter), letter, 1, 6)

    for _ in range(16):
        host.call('debug_grant_item', [second_guid, 5090050, 1])
    second.collect()
    result(request(second, 0x24a, letter), letter, 5, 1, struct.pack('<I', 50))
    assert not host.rows(f'SELECT id FROM game_item_text WHERE id = {letter}')
    assert host.rows(f'SELECT check_flags FROM game_mail WHERE id = {letter}') == [[16]]
    host.sql(f'DELETE FROM game_item_instance WHERE owner_guid = {second_guid} AND slot = 38')
    copied = request(second, 0x24a, letter)
    result(copied, letter, 5, 0)
    copy = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {second_guid} AND item_text_id = {letter}')[0][0]
    assert copy in creates(copied)
    assert host.rows(f'SELECT letters FROM game_item_text WHERE id = {letter}') == [[1]]
    result(request(second, 0x24a, letter), letter, 5, 6)
    result(request(second, 0x249, letter), letter, 4, 0)
    assert copy > 0xffffffff
    assert text(second, letter, copy) == [struct.pack('<I', letter) + b'private body\0']
    assert text(second, letter, copy & 0xffffffff) == [struct.pack('<I', letter) + b'private body\0']
    assert text(first, letter, copy) == [struct.pack('<I', letter) + b'\0']
    assert text(second, letter) == [struct.pack('<I', letter) + b'\0']
    host.sql(f'DELETE FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090050 AND slot >= 23')

    host.call('debug_grant_item', [first_guid, 5090050, 1])
    item = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090050')[0][0]
    host.sql(f'UPDATE game_item_instance SET enchant_id = 7745, durability = 11, random_property_id = 1900, soulbound = true WHERE guid = {item}')
    first.collect()
    result(send(item=copy), 0, 0, 19)
    result(send(item=item), 0, 0, 16)
    host.sql(f'UPDATE game_item_instance SET soulbound = false WHERE guid = {item}')
    host.call('debug_set_money', [first_guid, 29])
    first.collect()
    failed = send(item=item)
    result(failed, 0, 0, 3)
    assert item not in destroys(failed)
    assert host.rows(f'SELECT owner_guid FROM game_item_instance WHERE guid = {item}') == [[first_guid]]
    assert purse(first_guid) == 29
    host.call('debug_set_money', [first_guid, 1000])
    first.collect()
    sent = send(item=item, cod=250, subject='delivery: native cod')
    result(sent, 0, 0, 0)
    assert item in destroys(sent) and purse(first_guid) == 970
    assert not host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {item}')
    delayed, due = host.rows(f'SELECT id, deliver_micros FROM game_mail WHERE recipient_guid = {second_guid}')[0]
    assert due > int(time.time() * 1_000_000) + 3_590_000_000
    assert not mail_packets(second.collect()) and inbox(second) == b'\0'
    result(request(second, 0x246, delayed), delayed, 2, 6, bytes(8))
    assert text(second, delayed) == [struct.pack('<I', delayed) + b'\0']
    host.call('debug_deliver_mail_fixture', [second_guid, 'delivery: native cod'])
    arrived = []
    for _ in range(100):
        arrived += second.collect()
        if (0x285, bytes(4)) in arrived:
            break
        time.sleep(0.02)
    assert arrived.count((0x285, bytes(4))) == 1
    delivered_list = inbox(second)
    assert delivered_list[0] == 1
    attachment = delivered_list.index(b'delivery: native cod\0') + len(b'delivery: native cod\0')
    assert struct.unpack_from('<III', delivered_list, attachment + 12) == (5090050, 823, 1900)
    assert struct.unpack_from('<I', delivered_list, attachment + 37)[0] == 11
    second.send(0x284)
    assert (0x284, bytes(4)) in second.collect()
    result(request(second, 0x249, delayed), delayed, 4, 6)
    host.call('debug_set_money', [second_guid, 249])
    second.collect()
    result(request(second, 0x246, delayed), delayed, 2, 3, bytes(8))
    host.call('debug_set_money', [second_guid, 1000])
    second.collect()
    for _ in range(15):
        host.call('debug_grant_item', [second_guid, 5090050, 1])
    second.collect()
    result(request(second, 0x246, delayed), delayed, 2, 1, struct.pack('<I', 50))
    assert purse(second_guid) == 1000
    assert host.rows(f'SELECT item_entry, cod FROM game_mail WHERE id = {delayed}') == [[5090050, 250]]
    assert not mail_packets(first.collect())
    host.sql(f'DELETE FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090050 AND slot >= 23')
    taken = request(second, 0x246, delayed)
    result(taken, delayed, 2, 0, struct.pack('<II', 5090050, 1))
    new_item, enchantment, durability, property_id = host.rows(f'SELECT guid, enchant_id, durability, random_property_id FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090050')[0]
    assert new_item != item and new_item in creates(taken)
    assert (enchantment, durability, property_id) == (7745, 11, 1900)
    assert purse(second_guid) == 750
    assert host.rows(f'SELECT item_entry, cod FROM game_mail WHERE id = {delayed}') == [[0, 0]]
    assert (0x285, bytes(4)) in first.collect()
    payment, money, flags = host.rows(f'SELECT id, money, check_flags FROM game_mail WHERE recipient_guid = {first_guid}')[0]
    assert (money, flags) == (250, 8)
    result(request(first, 0x245, payment), payment, 1, 0)
    assert purse(first_guid) == 1220
    result(request(second, 0x246, delayed), delayed, 2, 6, bytes(8))

    result(send(subject='return me', money=10), 0, 0, 0)
    second.collect()
    returned = host.rows("SELECT id FROM game_mail WHERE subject = 'return me'")[0][0]
    result(request(second, 0x248, returned), returned, 3, 0)
    assert host.rows(f'SELECT recipient_guid, sender_guid, money, check_flags FROM game_mail WHERE id = {returned}') == [[first_guid, second_guid, 10, 2]]
    assert (0x285, bytes(4)) in first.collect()
    result(request(first, 0x248, returned), returned, 3, 6)
    result(request(first, 0x249, returned), returned, 4, 0)
    host.call('debug_grant_item', [first_guid, 5090050, 1])
    alt_item = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090050')[0][0]
    first.collect()
    result(send(name='Mailalt', item=alt_item), 0, 0, 0)
    assert host.rows(f'SELECT deliver_micros, item_entry FROM game_mail WHERE recipient_guid = {alt_guid}') == [[0, 5090050]]
    second.send(0x238, struct.pack('<Q', mailbox) + b'Mailone\0delivery: native copy\0\0' + struct.pack('<IIQIIII', 0, 0, copy, 0, 0, 0, 0))
    result(second.collect(), 0, 0, 0)
    copy_mail = host.rows("SELECT id FROM game_mail WHERE subject = 'delivery: native copy'")[0][0]
    assert text(second, letter, copy) == [struct.pack('<I', letter) + b'\0']
    host.call('debug_deliver_mail_fixture', [first_guid, 'delivery: native copy'])
    for _ in range(100):
        if (0x285, bytes(4)) in first.collect():
            break
        time.sleep(0.02)
    else:
        raise AssertionError('the delivered copy must notify its recipient')
    copied_again = request(first, 0x246, copy_mail)
    result(copied_again, copy_mail, 2, 0, struct.pack('<II', 8383, 1))
    held_copy = host.rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {first_guid} AND item_text_id = {letter}')[0]
    assert text(first, letter, held_copy[0]) == [struct.pack('<I', letter) + b'private body\0']
    first.send(0x111, bytes([255, held_copy[1], 1, 0, 0, 0]))
    assert held_copy[0] in destroys(first.collect())
    assert not host.rows(f'SELECT id FROM game_item_text WHERE id = {letter}')
    assert not host.rows('SELECT escrow_id FROM game_mail_escrow')
    assert not mail_packets(watcher.collect())


def check_meeting_stones(host):

    def queue(area, status):
        return 0x295, struct.pack('<IB', area, status)

    def stone_packets(packets):
        return [packet for packet in packets if packet[0] in (0x295, 0x297, 0x298, 0x299, 0x2bb)]

    stone, other_stone = 0xf110000000000999, 0xf11000000000099a
    host.call('debug_stage_meeting_stone', [5090600, stone, 1, 60, 1581, 0, 1001, 1000, 0])
    host.call('debug_stage_meeting_stone', [5090601, other_stone, 1, 60, 717, 0, 1002, 1000, 0])
    worlds = []
    for name, race, character_class in [
        ('Stoneone', 1, 1), ('Stonetwo', 1, 5), ('Stonethree', 1, 8),
        ('Stonefour', 1, 4), ('Stonefive', 1, 9), ('Stonehorde', 2, 1),
    ]:
        world = host.enter_world(name, race, character_class, at=(0, 1000, 1000, 0))
        worlds.append((world, world.guid))
        assert stone in creates(world.entered) and other_stone in creates(world.entered)
    (first, first_guid), (second, second_guid), (third, third_guid), (fourth, _), (fifth, fifth_guid), (horde, horde_guid) = worlds
    for world, guid in worlds:
        world.collect()
        assert not host.rows(f'SELECT account_id FROM game_account_claim WHERE character_guid = {guid}')

    def join(world, target=stone):
        world.send(0x292, struct.pack('<Q', target))
        return world.collect()

    first.send(0x1ff)
    assert (0x1ff, bytes(4)) in first.collect()
    first.send(0x296)
    assert queue(0, 5) in first.collect()
    assert not stone_packets(join(first, 0))
    host.restore('game_meeting_stone', 'entry = 5090600', 'min_level')
    host.sql('UPDATE game_meeting_stone SET min_level = 2 WHERE entry = 5090600')
    assert not stone_packets(join(first))
    assert not host.rows(f'SELECT character_guid FROM game_meeting_stone_seeker WHERE character_guid = {first_guid}')
    host.sql('UPDATE game_meeting_stone SET min_level = 1 WHERE entry = 5090600')
    host.call('debug_teleport', [first_guid, 0, 1100, 1000, 0, 0])
    first.collect()
    assert not stone_packets(join(first))
    host.call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
    first.collect()
    assert queue(1581, 1) in join(first)
    assert queue(1581, 1) in join(first)
    assert host.rows(f'SELECT area_id, team, class, group_id FROM game_meeting_stone_seeker WHERE character_guid = {first_guid}') == [[1581, 469, 1, 0]]
    assert queue(717, 1) in join(first, other_stone)
    first.send(0x296)
    assert queue(717, 1) in first.collect()
    first.send(0x293)
    assert queue(0, 0) in first.collect()
    assert queue(1581, 1) in join(first)
    first.send(0x4b)
    first.collect()
    assert not host.rows(f'SELECT character_guid FROM game_meeting_stone_seeker WHERE character_guid = {first_guid}')
    first.send(0x3d, struct.pack('<Q', first_guid))
    first.collect()
    first.send(0x296)
    assert queue(0, 5) in first.collect()

    assert queue(1581, 1) in join(horde)
    for world, _ in worlds[:4]:
        assert queue(1581, 1) in join(world)
    assert not host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {first_guid}')
    assert host.rows(f'SELECT area_id, team, class, group_id FROM game_meeting_stone_seeker WHERE character_guid = {horde_guid}') == [[1581, 67, 1, 0]]
    formed = join(fifth)
    group = host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {first_guid}')[0][0]
    assert host.rows(f'SELECT leader_guid FROM game_group WHERE group_id = {group}') == [[first_guid]]
    assert {row[0] for row in host.rows(f'SELECT character_guid FROM game_group_member WHERE group_id = {group}')} == {guid for _, guid in worlds[:5]}
    for index, (world, guid) in enumerate(worlds[:5]):
        packets = formed if index == 4 else world.collect()
        assert (0x297, b'') in packets and queue(0, 5) in packets
        rosters = [body for opcode, body in packets if opcode == 0x7d]
        assert rosters and struct.unpack_from('<I', rosters[-1], 2)[0] == 4
        assert not host.rows(f'SELECT character_guid FROM game_meeting_stone_seeker WHERE character_guid = {guid}')
    assert not stone_packets(horde.collect())
    assert not host.rows(f'SELECT group_id FROM game_meeting_stone_party WHERE group_id = {group}')
    assert (0x2bb, b'\x02') in join(first)
    assert (0x2bb, b'\x01') in join(second)
    first.send(0x75, b'Stonefive\0')
    for world, _ in worlds[:5]:
        world.collect()
    assert queue(1581, 1) in join(first)
    for world, _ in worlds[1:4]:
        assert queue(1581, 1) in world.collect()
    assert host.rows(f'SELECT group_id FROM game_meeting_stone_party WHERE group_id = {group}') == [[group]]
    second.send(0x293)
    assert queue(0, 5) in second.collect()
    assert host.rows(f'SELECT group_id FROM game_meeting_stone_seeker WHERE character_guid = {second_guid}') == [[group]]
    host.call('debug_backdate_meeting_stone_party', [group, 301])
    deadline, reminded = time.monotonic() + 7, []
    while time.monotonic() < deadline:
        reminded.extend(first.collect())
        if (0x298, b'') in reminded:
            break
        time.sleep(0.05)
    assert (0x298, b'') in reminded
    for world, _ in worlds[1:4]:
        assert (0x298, b'') in world.collect()
    assert not stone_packets(fifth.collect()) and not stone_packets(horde.collect())
    first.send(0x75, b'Stonethree\0')
    assert stone_packets(first.collect()) == [queue(0, 3), queue(0, 0)]
    for world in [second, fourth]:
        assert stone_packets(world.collect()) == [queue(0, 3), queue(0, 0)]
    assert stone_packets(third.collect()) == [queue(1581, 4), queue(1581, 1)]
    assert host.rows(f'SELECT group_id FROM game_meeting_stone_seeker WHERE character_guid = {third_guid}') == [[0]]
    third.send(0x293)
    assert queue(0, 0) in third.collect()
    first.send(0x28e)
    first.collect()
    second.collect()
    fourth.collect()
    assert (0x2bb, b'\x03') in join(first)
    assert not host.rows(f'SELECT group_id FROM game_meeting_stone_party WHERE group_id = {group}')
    horde.send(0x293)
    assert queue(0, 0) in horde.collect()
    for world, _ in worlds[:5]:
        world.send(0x7b)
        world.collect()


def check_member_stats(host):
    client, peer = native_and_peer(host, grouped=True)
    guid, peer_guid = client.guid, peer.guid
    def decode(body):
        target = packed_guid(body)
        offset = 1 + body[0].bit_count()
        mask = struct.unpack_from('<I', body, offset)[0]
        offset += 4
        fields = {}
        formats = ['B', 'H', 'H', 'B', 'H', 'H', 'H', 'H', 'HH', None, None,
                   'Q', None, 'H', 'H', 'H', 'B', 'H', 'H', None, None]
        for bit, fmt in enumerate(formats):
            if not mask & (1 << bit):
                continue
            if bit in (9, 10, 19, 20):
                width = 4 if bit in (9, 19) else 2
                dirty = int.from_bytes(body[offset:offset + width], 'little')
                offset += width
                slots = {}
                for slot in range(width * 8):
                    if dirty & (1 << slot):
                        slots[slot] = struct.unpack_from('<H', body, offset)[0]
                        offset += 2
                fields[bit] = slots
            elif bit == 12:
                end = body.index(0, offset)
                fields[bit] = body[offset:end].decode()
                offset = end + 1
            else:
                parts = struct.unpack_from('<' + fmt, body, offset)
                fields[bit] = parts[0] if len(parts) == 1 else parts
                offset += struct.calcsize('<' + fmt)
        assert offset == len(body), (mask, offset, len(body))
        return target, fields

    def full(target):
        client.send(0x27f, struct.pack('<Q', target))
        decoded = [decode(body) for opcode, body in client.collect() if opcode == 0x2f2]
        assert len(decoded) == 1 and decoded[0][0] == target
        return decoded[0][1]

    def changes():
        return [fields for opcode, body in client.collect() if opcode == 0x7e
                for target, fields in [decode(body)] if target == peer_guid]

    def changed_until(predicate):
        deadline, seen = time.monotonic() + 8, []
        while time.monotonic() < deadline:
            seen.extend(changes())
            if any(predicate(fields) for fields in seen):
                return seen
            time.sleep(0.05)
        raise AssertionError(f'member stats did not arrive: {seen}')

    assert full(guid) == {0: 0}
    assert full(1) == {0: 0}
    initial = full(peer_guid)
    assert initial[0] == 1 and initial[6] > 0
    assert 11 not in initial
    host.call('debug_teleport', [peer_guid, 1, 11, 12, 0, 0])
    peer.collect()
    baseline = changed_until(lambda fields: fields.get(8) == (11, 12))
    assert any(fields.get(0) == 1 and fields.get(11) == 0 for fields in baseline)
    health = host.rows(f'SELECT max_health FROM game_world_entity WHERE guid = {peer_guid}')[0][0]
    host.sql(f'UPDATE game_world_entity SET health = {health - 1} WHERE guid = {peer_guid}')
    delta = changed_until(lambda fields: fields.get(1) == min(65535, health - 1))
    assert any(set(fields) == {1} for fields in delta), delta
    host.call('debug_fill_aura_slots', [peer_guid, peer_guid, False, 1])
    host.call('debug_fill_aura_slots', [peer_guid, peer_guid, True, 1])
    host.sql(f'UPDATE game_aura SET spell_id = 123 WHERE target_guid = {peer_guid} AND slot = 0')
    host.sql(f'UPDATE game_aura SET spell_id = 456 WHERE target_guid = {peer_guid} AND slot = 32')
    changed_until(lambda fields: fields.get(9) == {0: 123} and fields.get(10) == {0: 456})
    host.sql(f'DELETE FROM game_aura WHERE target_guid = {peer_guid}')
    changed_until(lambda fields: fields.get(9) == {0: 0} and fields.get(10) == {0: 0})

    unit_bytes = host.rows(f'SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {peer_guid}')[0][0]
    host.sql(f'UPDATE game_world_entity SET unit_bytes_0 = {(unit_bytes & ~0xff00) | 0x0300} WHERE guid = {peer_guid}')
    host.restore('game_creature_template', 'entry = 51006', 'creature_family', 'npc_flags')
    host.sql('UPDATE game_creature_template SET creature_family = 5, npc_flags = 0 WHERE entry = 51006')
    wild = spawn(host, peer_guid, 51006, 3)
    host.sql(f'UPDATE game_world_entity SET level = 1 WHERE guid = {wild}')
    host.call('debug_force_cast_at', [peer_guid, 50300, wild])
    pet_guid = 0xf130000000000000 | peer_guid
    assert host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {pet_guid}')
    host.sql(f"UPDATE game_hunter_pet SET name = 'Nativepet' WHERE owner_guid = {peer_guid}")
    host.sql(f"UPDATE game_hunter_pet_protocol SET name = 'Nativepet' WHERE live_pet_guid = {pet_guid}")
    host.call('debug_fill_aura_slots', [pet_guid, peer_guid, False, 1])
    host.sql(f'UPDATE game_aura SET spell_id = 789 WHERE target_guid = {pet_guid} AND slot = 0')
    changed_until(lambda fields: fields.get(11) == pet_guid and fields.get(12) == 'Nativepet' and fields.get(19) == {0: 789})
    complete = full(peer_guid)
    assert complete[11] == pet_guid and complete[12] == 'Nativepet' and complete[19][0] == 789
    changed_until(lambda fields: fields.get(11) == pet_guid)
    host.call('debug_pet_command', [peer_guid, 0x07000003, 0])
    removed = changed_until(lambda fields: fields.get(11) == 0)
    assert any(fields.get(12) == '' and fields.get(19) == {0: 0} for fields in removed), removed
    host.sql(f'UPDATE game_world_entity SET unit_bytes_0 = {unit_bytes} WHERE guid = {peer_guid}')
    assert not any(fields.get(0) == 0 for fields in changes())
    host.call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect(auto_ack=False)
    assert full(peer_guid) == {0: 0x21}
    # A loading member's tick writes nothing, so two 5 s intervals stand in for proof of one.
    time.sleep(10.5)
    assert not changes(), 'loading members must not be reported offline'
    peer.send(0xdc)
    peer.collect()
    client.collect()
    host.wait_until(
        lambda: host.rows(f'SELECT snapshots FROM game_native_member_stats WHERE owner_guid = {guid}')[0][0] == [],
        'a member stats tick must drop the member the client now sees', timeout=12)
    assert not changes(), 'AOI members must use entity updates'
    peer.send(0x4b)
    peer.collect()
    assert not host.rows(f'SELECT id FROM game_native_member_stats WHERE owner_guid = {peer_guid}')
    assert full(peer_guid) == {0: 0}
    changed_until(lambda fields: fields == {0: 0})
    # The offline packet marks a tick; the next one is due 5 s later.
    time.sleep(6)
    assert not changes(), 'offline status is sent once'
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.collect()
    assert host.rows(f'SELECT id FROM game_native_member_stats WHERE owner_guid = {peer_guid}')


def check_party(host):
    client, peer = native_and_peer(host, grouped=True)
    guid, peer_guid = client.guid, peer.guid
    def command(operation, name, result):
        return 0x7f, struct.pack('<I', operation) + name.encode() + b'\0' + struct.pack('<I', result)

    def roster(packets):
        body = next(body for opcode, body in packets if opcode == 0x7d)
        kind, own, count = struct.unpack_from('<BBI', body)
        offset, members = 6, {}
        for _ in range(count):
            end = body.index(0, offset)
            name = body[offset:end].decode()
            member, online, flags = struct.unpack_from('<QBB', body, end + 1)
            members[member] = (name, online, flags)
            offset = end + 11
        leader = struct.unpack_from('<Q', body, offset)[0]
        offset += 8
        loot = struct.unpack_from('<BQBB', body, offset) if count else None
        assert len(body) == offset + (11 if count else 0)
        return kind, own, members, leader, loot

    def invite(name='Peer', result=0):
        client.send(0x6e, name.encode() + b'\0')
        assert command(0, name, result) in client.collect()

    client.send(0x7b)
    assert (0x7c, b'') in client.collect()
    assert (0x7c, b'') in peer.collect()
    invite('Missinggroupmember', 1)
    invite('Native', 1)
    host.call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    assert any(opcode == 0x3e for opcode, _ in peer.collect(auto_ack=False))
    invite('pEeR')
    assert (0x6f, b'Native\0') in peer.collect(auto_ack=False)
    peer.send(0xdc)
    peer.collect()
    host.call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    peer.send(0x73)
    peer.collect()
    assert (0x74, b'Peer\0') in client.collect()
    assert not host.rows(f'SELECT id FROM game_group_invite WHERE target_guid = {peer_guid}')
    peer.send(0x4b)
    peer.collect()
    invite('Peer', 1)
    host.sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    invite('Peer', 7)
    assert not any(opcode == 0x6f for opcode, _ in peer.collect())
    peer.send(0x4b)
    peer.collect()
    host.sql(f'UPDATE game_character SET race = 1 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    invite()
    peer.collect()
    peer.send(0x72)
    assert roster(peer.collect()) == (0, 0, {guid: ('Native', 1, 0)}, guid, (3, 0, 2, 0))
    assert roster(client.collect()) == (0, 0, {peer_guid: ('Peer', 1, 0)}, guid, (3, 0, 2, 0))
    invite('Peer', 4)
    peer.send(0x75, b'Native\0')
    assert command(2, 'Native', 6) in peer.collect()
    assert not any(opcode in (0x77, 0x7c) for opcode, _ in client.collect())
    peer.send(0x76, struct.pack('<Q', peer_guid))
    assert not any(opcode == 0x7f for opcode, _ in peer.collect())

    host.call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    assert roster(peer.collect())[2] == {guid: ('Native', 1, 0)}
    client.collect()
    peer.send(0x322)
    assert not any(opcode == 0x322 for opcode, _ in peer.collect() + client.collect())
    client.send(0x322)
    assert (0x322, b'') in client.collect()
    assert (0x322, b'') in peer.collect()
    client.send(0x322)
    assert not any(opcode == 0x322 for opcode, _ in client.collect() + peer.collect())
    peer.send(0x322, b'\1')
    assert not any(opcode == 0x322 for opcode, _ in peer.collect())
    assert (0x322, struct.pack('<QB', peer_guid, 1)) in client.collect()
    client.send(0x1d5, struct.pack('<ff', 0.25, 0.75))
    assert not any(opcode == 0x1d5 for opcode, _ in client.collect())
    assert (0x1d5, struct.pack('<Qff', guid, 0.25, 0.75)) in peer.collect()
    client.send(0x1d5, struct.pack('<ff', 0.5, 0.5))
    client.collect()
    assert not any(opcode == 0x1d5 for opcode, _ in peer.collect())
    client.send(0x1fb, struct.pack('<II', 10003, 10002))
    expected = (0x1fb, struct.pack('<IIIQ', 10000, 10000, 10000, guid))
    assert expected in client.collect()
    assert expected in peer.collect()
    client.send(0x1fb, struct.pack('<II', 1, 1))
    assert not any(opcode == 0x1fb for opcode, _ in client.collect() + peer.collect())

    peer.send(0x321, struct.pack('<BQ', 1, guid))
    assert not any(opcode == 0x321 for opcode, _ in peer.collect() + client.collect())
    client.send(0x321, struct.pack('<BQ', 1, guid))
    expected = (0x321, struct.pack('<BBQ', 0, 1, guid))
    assert expected in client.collect()
    assert expected in peer.collect()
    client.send(0x321, struct.pack('<BQ', 2, guid))
    expected = [(0x321, struct.pack('<BBQ', 0, 1, 0)), (0x321, struct.pack('<BBQ', 0, 2, guid))]
    assert [p for p in client.collect() if p[0] == 0x321] == expected
    assert [p for p in peer.collect() if p[0] == 0x321] == expected
    peer.send(0x321, b'\xff')
    icon_list = (0x321, struct.pack('<BBQ', 1, 2, guid))
    assert icon_list in peer.collect()
    client.send(0x7a, struct.pack('<IQI', 2, guid, 3))
    changed = client.collect()
    assert roster(changed)[4] == (2, guid, 3, 0)
    assert [op for op, _ in changed if op in (0x7d, 0x321)] == [0x7d, 0x321]
    assert icon_list in peer.collect()
    peer.send(0x7a, struct.pack('<IQI', 0, 0, 2))
    assert not any(op == 0x7d for op, _ in peer.collect() + client.collect())
    client.send(0x28e)
    changed = client.collect()
    assert command(0, '', 0) in changed
    assert roster(changed)[0] == 1
    assert roster(peer.collect())[0] == 1
    client.send(0x28f, struct.pack('<QB', peer_guid, 1))
    assert roster(client.collect())[2][peer_guid][2] == 0x80
    assert roster(peer.collect())[1] == 0x80
    peer.send(0x27e, b'nAtIvE\0\3')
    assert roster(peer.collect())[2][guid][2] == 3
    assert roster(client.collect())[1] == 3
    peer.send(0x280, b'Peer\0Native\0')
    assert roster(peer.collect())[1] == 0x83
    assert roster(client.collect())[1] == 0
    peer.send(0x27e, b'Native\0\x08')
    assert not any(op == 0x7d for op, _ in peer.collect() + client.collect())
    client.send(0x78, struct.pack('<Q', peer_guid))
    for packets in (client.collect(), peer.collect()):
        assert [op for op, _ in packets if op in (0x79, 0x7d)] == [0x79, 0x7d]
        assert (0x79, b'Peer\0') in packets
        assert roster(packets)[3] == peer_guid
    client.send(0x78, struct.pack('<Q', guid))
    assert not any(op == 0x79 for op, _ in client.collect() + peer.collect())
    peer.send(0x78, struct.pack('<Q', guid))
    peer.collect()
    client.collect()
    peer.send(0x4b)
    peer.collect()
    assert roster(client.collect())[2][peer_guid][1] == 0
    client.send(0x78, struct.pack('<Q', peer_guid))
    assert not any(op == 0x79 for op, _ in client.collect())
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    returned = peer.collect()
    assert roster(returned)[0] == 1 and icon_list in returned
    assert roster(client.collect())[2][peer_guid][1] == 1

    host.call('debug_spawn_player_entity', [1])
    host.call('debug_set_sessionless_action_consent', [1, False])
    invite('Tester')
    assert not host.rows('SELECT id FROM game_group_member WHERE character_guid = 1')
    assert host.rows('SELECT id FROM game_group_invite WHERE target_guid = 1')
    host.call('debug_set_sessionless_action_consent', [1, True])
    invite('Tester')
    assert host.rows('SELECT id FROM game_group_member WHERE character_guid = 1')
    assert not host.rows('SELECT id FROM game_group_invite WHERE target_guid = 1')
    peer.collect()
    client.send(0x76, struct.pack('<Q', peer_guid))
    roster(client.collect())
    assert [op for op, _ in peer.collect() if op in (0x77, 0x7c)] == [0x77, 0x7c]
    invite()
    peer.collect()
    peer.send(0x72)
    peer.collect()
    client.collect()
    client.send(0x75, b'Tester\0')
    roster(client.collect())
    peer.collect()
    client.send(0x7b)
    assert (0x7c, b'') in client.collect()
    assert (0x7c, b'') in peer.collect()
    invite()
    peer.collect()
    peer.send(0x72)
    peer.collect()
    client.collect()
    host.call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    client.collect()


def check_channels(host):
    client, peer = native_and_peer(host)
    guid, peer_guid = client.guid, peer.guid
    name = b'Native channel\0'

    def command(connection, opcode, extra=b'', channel=name):
        connection.send(opcode, channel + extra)

    def notices(packets):
        return [body for opcode, body in packets if opcode == 0x99]

    def notice(code, extra=b''):
        return bytes([code]) + name + extra

    def say(connection, text):
        connection.send(0x95, struct.pack('<II', 14, 7) + name + text.encode() + b'\0')

    command(client, 0x9a)
    assert notices(client.collect()) == [notice(5)]
    command(client, 0x97, b'\0')
    joined = notices(client.collect())
    assert notice(2, struct.pack('<II', 1, 0)) in joined
    channel_id = host.rows("SELECT channel_id FROM game_chat_channel WHERE name_key = 'native channel'")[0][0]
    assert host.rows(f'SELECT account_id, claim_generation FROM game_chat_channel_member WHERE channel_id = {channel_id}')[0][1] == 0
    command(client, 0x9c, b'secret\0')
    assert notices(client.collect()) == [notice(7, struct.pack('<Q', guid))]
    command(peer, 0x97, b'wrong\0')
    assert notices(peer.collect()) == [notice(4)]
    assert not host.rows(f'SELECT id FROM game_chat_channel_member WHERE character_guid = {peer_guid}')
    command(client, 0x9c, b'x' * 129 + b'\0')
    assert notices(client.collect()) == [notice(4)]
    assert host.rows(f'SELECT password FROM game_chat_channel WHERE channel_id = {channel_id}') == [['secret']]
    command(peer, 0x97, b'secret\0')
    assert notice(2, struct.pack('<II', 1, 0)) in notices(peer.collect())
    assert notices(client.collect()) == [notice(0, struct.pack('<Q', peer_guid))]
    command(peer, 0x9c, b'no\0')
    assert notices(peer.collect()) == [notice(6)]
    assert not notices(client.collect())
    command(peer, 0x9e)
    assert notices(peer.collect()) == [notice(11, b'Native\0')]
    command(peer, 0x9a)
    roster = next(body for opcode, body in peer.collect() if opcode == 0x9b)
    assert struct.pack('<Q', guid) in roster and struct.pack('<Q', peer_guid) in roster
    host.restore('game_account', "username = 'NATIVE'", 'gm_level')
    host.call('set_gm_level', ['Native', 1])
    say(client, 'channel line')
    assert any(opcode == 0x96 and b'channel line\0' in body for opcode, body in client.collect())
    assert any(opcode == 0x96 and b'channel line\0' in body for opcode, body in peer.collect())
    command(client, 0xa1, b'Peer\0')
    client.collect()
    peer.collect()
    say(peer, 'muted channel line')
    assert notices(peer.collect()) == [notice(17)]
    assert not any(opcode == 0x96 for opcode, _ in client.collect())
    command(client, 0xa2, b'Peer\0')
    client.collect()
    peer.collect()
    command(client, 0xa8)
    client.collect()
    peer.collect()
    say(peer, 'moderated channel line')
    assert notices(peer.collect()) == [notice(6)]
    client.send(0x6c, b'Peer\0')
    client.collect()
    command(client, 0x9f, b'Peer\0')
    client.collect()
    peer.collect()
    say(peer, 'moderator line')
    assert any(opcode == 0x96 and b'moderator line\0' in body for opcode, body in peer.collect())
    assert any(opcode == 0x96 and b'moderator line\0' in body for opcode, body in client.collect())
    command(client, 0xa0, b'Peer\0')
    client.collect()
    peer.collect()
    command(client, 0xa8)
    client.collect()
    peer.collect()
    say(peer, 'ignored ordinary member')
    assert any(opcode == 0x96 for opcode, _ in peer.collect())
    assert not any(opcode == 0x96 for opcode, _ in client.collect())
    client.send(0x6d, struct.pack('<Q', peer_guid))
    client.collect()
    command(client, 0xa5, b'Peer\0')
    assert notice(20, struct.pack('<QQ', peer_guid, guid)) in notices(client.collect())
    assert notice(20, struct.pack('<QQ', peer_guid, guid)) in notices(peer.collect())
    command(peer, 0x97, b'secret\0')
    assert notices(peer.collect()) == [notice(19)]
    command(client, 0xa6, b'Peer\0')
    assert notice(21, struct.pack('<QQ', peer_guid, guid)) in notices(client.collect())
    command(client, 0xa3, b'Peer\0')
    assert notices(client.collect()) == [notice(29, b'Peer\0')]
    assert notices(peer.collect()) == [notice(24, struct.pack('<Q', guid))]
    peer.send(0x6c, b'Native\0')
    peer.collect()
    command(client, 0xa3, b'Peer\0')
    assert notices(client.collect()) == [notice(29, b'Peer\0')]
    assert not notices(peer.collect())
    peer.send(0x6d, struct.pack('<Q', guid))
    peer.collect()
    command(peer, 0x97, b'secret\0')
    peer.collect()
    client.collect()
    command(client, 0xa4, b'Peer\0')
    assert notice(18, struct.pack('<QQ', peer_guid, guid)) in notices(client.collect())
    peer.collect()
    command(peer, 0x97, b'secret\0')
    peer.collect()
    client.collect()
    command(client, 0x9d, b'Peer\0')
    client.collect()
    peer.collect()
    command(client, 0x9e)
    assert notices(client.collect()) == [notice(11, b'Peer\0')]
    command(peer, 0xa7)
    peer.collect()
    client.collect()
    host.call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    peer.collect()
    assert not notices(client.collect())
    assert host.rows(f'SELECT character_guid FROM game_chat_channel_member WHERE character_guid = {peer_guid}')
    peer.send(0x4b)
    peer.collect()
    assert notice(12, struct.pack('<QBB', guid, 2, 3)) in notices(client.collect())
    assert host.rows(f'SELECT owner_guid FROM game_chat_channel WHERE channel_id = {channel_id}') == [[guid]]
    assert not host.rows(f'SELECT id FROM game_chat_channel_member WHERE character_guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    command(peer, 0x9a)
    assert notices(peer.collect()) == [notice(5)]
    command(client, 0xa3, b'Missingfriend\0')
    assert notices(client.collect()) == [notice(9, b'Missingfriend\0')]
    command(client, 0x98)
    assert notice(3) in notices(client.collect())
    assert not host.rows(f'SELECT channel_id FROM game_chat_channel WHERE channel_id = {channel_id}')
    assert not host.rows(f'SELECT id FROM game_chat_channel_ban WHERE channel_id = {channel_id}')
    command(client, 0x97, b'\0', channel=b'WorldDefense\0')
    client.collect()
    command(client, 0x9e, channel=b'WorldDefense\0')
    assert notices(client.collect()) == [bytes([11]) + b'WorldDefense\0Nobody\0']
    client.send(0x95, struct.pack('<II', 14, 7) + b'WorldDefense\0read only\0')
    assert notices(client.collect()) == [bytes([17]) + b'WorldDefense\0']
    command(client, 0x98, channel=b'WorldDefense\0')
    client.collect()
    host.call('set_gm_level', ['Native', 0])
    host.call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    client.collect()


def check_addon(host):

    def send(world, text, kind=1, target=''):
        body = struct.pack('<II', kind, 0xffffffff)
        if kind in (6, 14):
            body += target.encode() + b'\0'
        world.send(0x95, body + text.encode() + b'\0')

    def replies(world, guid):
        texts = []
        for opcode, body in world.collect():
            if opcode != 0x96 or struct.unpack_from('<I', body, 1)[0] != 0xffffffff:
                continue
            assert body[0] == 6 and struct.unpack_from('<Q', body, 5)[0] == guid
            length = struct.unpack_from('<I', body, 13)[0]
            assert len(body) == 18 + length and body[-2:] == bytes(2)
            texts.append(body[17:-2].decode())
        return texts

    worlds = []
    for name in ['Addonone', 'Addontwo']:
        guid = host.character(name, at=(0, 2000, 1000, 0))
        world = host.connect(name.upper())
        worlds.append((world, guid))
        assert world.read()[0] == 0x1ee
        send(world, 'STC\tv1|ping|0|1/1|before login', 6, 'Addonone')
        assert not replies(world, guid)
        world.send(0x3d, struct.pack('<Q', guid))
        world.collect()
    first, guid = worlds[0]
    second, other_guid = worlds[1]
    for index in range(21):
        send(first, f'STC\tv1|ping|{index}|1/1|burst {index}', 6, 'Addontwo')
    assert replies(first, guid) == [f'STC\tv1|pong|0|1/1|burst {index}' for index in range(20)]
    assert not replies(second, other_guid)
    first.send(0x4b)
    first.collect()
    first.send(0x3d, struct.pack('<Q', guid))
    first.collect()
    send(first, 'STC\tv1|ping|21|1/1|after logout')
    assert not replies(first, guid)
    time.sleep(0.55)
    send(first, 'STC\tv1|ping|22|1/1|refilled')
    assert replies(first, guid) == ['STC\tv1|pong|0|1/1|refilled']
    first.close()
    first = host.connect('ADDONONE')
    worlds[0] = (first, guid)
    assert first.read()[0] == 0x1ee
    first.send(0x3d, struct.pack('<Q', guid))
    first.collect()
    send(first, 'STC\tv1||ping||23||1/1||a||||b', 6, 'Addontwo')
    assert replies(first, guid) == ['STC\tv1|pong|0|1/1|a||b']
    for text in ['STC\tv2|ping|0|1/1|bad', 'STC\tv1|ping|0|1/2|bad', 'STC\tbroken']:
        send(first, text)
    assert not replies(first, guid)
    assert not replies(second, other_guid)

    def command(world, issuer, payload, outcome):
        send(world, f'STC\tv1|fixture.command|0|1/1|{payload}')
        response = replies(world, issuer)
        prefix = 'STC\tv1|fixture.command.result|0|1/1|'
        assert len(response) == 1 and response[0].startswith(prefix), response
        result = response[0][len(prefix):]
        if outcome == 'Malformed':
            assert result == outcome
            return
        intent_id, actual = result.split('|')
        assert actual == outcome, result
        assert host.rows(f'SELECT pending, claim_token FROM game_party_command_intent WHERE id = {intent_id}') == [[False, 0]]
        assert not host.rows(f'SELECT id FROM game_party_command_receipt WHERE intent_id = {intent_id}')
        assert not host.rows('SELECT lane FROM game_party_command_dispatch_lane')
        return int(intent_id)

    command(first, guid, 'bad', 'Malformed')
    command(first, guid, '1', 'NotMember')
    first.send(0x6e, b'Addontwo\0')
    first.collect()
    second.collect()
    second.send(0x72)
    second.collect()
    first.collect()
    host.call('debug_spawn_player_entity', [1])
    host.call('debug_set_sessionless_action_consent', [1, True])
    host.call('debug_teleport', [1, 0, 2001, 1000, 0, 0])
    first.send(0x6e, b'Tester\0')
    first.collect()
    second.collect()
    command(second, other_guid, '1', 'NotLeader')
    command(first, guid, '999999999', 'MissingBot')
    command(first, guid, str(other_guid), 'WrongAccount')
    command(first, guid, f'1|{other_guid}', 'Applied')
    command(first, guid, '1|999999999', 'NotMember')
    host.call('debug_teleport', [other_guid, 1, 2000, 1000, 0, 0])
    second.collect()
    first.collect()
    command(first, guid, f'1|{other_guid}', 'WrongPartition')
    command(first, guid, '1', 'Applied')
    host.call('debug_create_fixture_instance', [1])
    first.collect()
    command(first, guid, '1', 'WrongPartition')
    host.call('debug_enter_instance', [1, 0])
    first.collect()
    host.call('debug_set_sessionless_action_consent', [1, False])
    command(first, guid, '1', 'Suppressed')
    host.call('debug_set_sessionless_action_consent', [1, True])
    command(first, guid, '1|0|999999999', 'TargetUnavailable')
    assert not replies(second, other_guid)
    first.send(0x75, b'Tester\0')
    first.collect()
    first.send(0x7b)
    first.collect()
    second.collect()
    check_companion_actions(host, first, second, guid, other_guid)


def check_companion_actions(host, first, second, guid, other_guid):
    assert not host.rows('SELECT guid FROM game_aoi_viewer WHERE guid = 1')
    assert not host.rows('SELECT id FROM game_aoi_grant WHERE viewer_guid = 1')

    def group_action(target=0, leave=False):
        host.call('debug_emit_sessionless_group_intent', [1, target, leave])
        assert not host.rows('SELECT id FROM game_bot_invite_intent')

    host.call('debug_set_sessionless_action_consent', [1, False])
    group_action(guid)
    assert not any(op == 0x6f for op, _ in first.collect())
    assert not host.rows(f'SELECT id FROM game_group_invite WHERE target_guid = {guid}')
    host.call('debug_set_sessionless_action_consent', [1, True])
    group_action(guid)
    assert (0x6f, b'Tester\0') in first.collect()
    first.send(0x72)
    first.collect()
    assert host.rows('SELECT leader_guid FROM game_group WHERE leader_guid = 1') == [[1]]
    host.sql(f'UPDATE game_character SET race = 2 WHERE guid = {other_guid}')
    group_action(other_guid)
    assert not any(op == 0x6f for op, _ in second.collect())
    assert not host.rows(f'SELECT id FROM game_group_invite WHERE target_guid = {other_guid}')
    host.sql(f'UPDATE game_character SET race = 1 WHERE guid = {other_guid}')
    host.call('debug_set_sessionless_action_consent', [1, False])
    group_action(leave=True)
    assert host.rows('SELECT id FROM game_group_member WHERE character_guid = 1')
    host.call('debug_set_sessionless_action_consent', [1, True])
    group_action(leave=True)
    assert (0x7c, b'') in first.collect()
    assert not host.rows('SELECT id FROM game_group_member WHERE character_guid = 1')

    second.send(0x4b)
    second.collect()
    host.call('debug_spawn_player_entity', [other_guid])
    host.call('debug_set_sessionless_action_consent', [other_guid, False])
    group_action(other_guid)
    assert host.rows(f'SELECT id FROM game_group_invite WHERE target_guid = {other_guid}')
    assert not host.rows(f'SELECT id FROM game_group_member WHERE character_guid = {other_guid}')
    host.call('debug_set_sessionless_action_consent', [other_guid, True])
    group_action(other_guid)
    assert not host.rows(f'SELECT id FROM game_group_invite WHERE target_guid = {other_guid}')
    assert host.rows(f'SELECT id FROM game_group_member WHERE character_guid = {other_guid}')
    group_action(leave=True)
    assert not host.rows(f'SELECT id FROM game_group_member WHERE character_guid = {other_guid}')
    second.send(0x3d, struct.pack('<Q', other_guid))
    second.collect()

    entrance, exit_trigger = 5090650, 5090651
    for trigger, source_map, destination_map in [(entrance, 0, 36), (exit_trigger, 36, 0)]:
        host.sql(f'INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) VALUES ({trigger},{source_map},4000,1000,0,5,0,0,0,0)')
        host.sql(f"INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) VALUES ({trigger},{destination_map},4000,1000,0,0,'Companion crossing')")
    for character in [1, guid, other_guid]:
        host.call('debug_set_level', [character, 60])
        host.call('debug_teleport', [character, 0, 4000, 1000, 0, 0])
    first.collect()
    second.collect()
    first.send(0x6e, b'Tester\0')
    first.collect()
    second.collect()
    first.send(0xb4, struct.pack('<I', entrance))
    first.collect()
    second.collect()
    instance = host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {guid}')[0][0]
    assert instance != 0

    def relocate(trigger, target_map, target_instance, generation, expected=0):
        host.call('debug_replay_sessionless_areatrigger', [1, trigger, target_map, target_instance, generation, expected])

    def refused(trigger, target_map, target_instance, text):
        before = host.rows('SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = 1')
        bindings = host.rows('SELECT instance_id FROM game_instance_binding WHERE character_guid = 1')
        try:
            relocate(trigger, target_map, target_instance, 101)
            raise AssertionError('Companion crossing unexpectedly committed')
        except RuntimeError as error:
            assert text in str(error), str(error)
        assert host.rows('SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = 1') == before
        assert host.rows('SELECT instance_id FROM game_instance_binding WHERE character_guid = 1') == bindings
        assert not host.rows('SELECT id FROM game_character_relocation WHERE character_guid = 1')

    host.call('debug_set_sessionless_action_consent', [1, False])
    refused(entrance, 36, instance, 'CannotAct')
    host.call('debug_set_sessionless_action_consent', [1, True])
    refused(0, 36, instance, 'MissingResource')
    refused(entrance, 37, instance, 'OtherPartition')
    refused(entrance, 36, instance + 1000, 'OtherPartition')
    host.call('debug_teleport', [1, 0, 4010, 1000, 0, 0])
    refused(entrance, 36, instance, 'OutOfRange')
    host.call('debug_teleport', [1, 0, 4000, 1000, 0, 0])
    first.collect()
    second.collect()
    vitals = host.rows('SELECT health, power, money FROM game_world_entity WHERE guid = 1')
    relocate(entrance, 36, instance, 101)
    assert host.rows('SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = 1') == [[36, instance, 4000, 1000, 0]]
    assert host.rows('SELECT health, power, money FROM game_world_entity WHERE guid = 1') == vitals
    assert host.rows('SELECT map_id, pending_instance_id, online FROM game_character WHERE guid = 1') == [[36, instance, False]]
    assert 1 in creates(first.collect())
    assert 1 in destroys(second.collect())
    receipt = host.rows('SELECT id FROM game_character_relocation WHERE character_guid = 1')[0][0]
    assert not host.rows('SELECT id FROM game_bot_transfer_intent WHERE bot_guid = 1')
    assert not host.rows('SELECT transfer_id FROM game_transfer_out WHERE character_guid = 1')
    assert not host.rows('SELECT transfer_id FROM game_transfer_in WHERE character_guid = 1')
    host.call('debug_admit_sessionless_action', [1])
    relocate(entrance, 36, instance, 101, receipt)
    assert not creates(first.collect())
    assert not destroys(second.collect())
    relocate(exit_trigger, 0, 0, 102)
    assert host.rows('SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = 1') == [[0, 0, 4000, 1000, 0]]
    assert 1 in destroys(first.collect())
    assert 1 in creates(second.collect())
    returned = host.rows('SELECT id FROM game_character_relocation WHERE character_guid = 1')
    assert len(returned) == 1 and returned[0][0] != receipt
    first.send(0xb4, struct.pack('<I', exit_trigger))
    first.collect()
    second.collect()
    first.send(0x7b)
    first.collect()


def check_chat(host):
    client, peer = native_and_peer(host, grouped=True)
    guid, peer_guid = client.guid, peer.guid
    def send(connection, kind, message, language=7, target=None):
        body = struct.pack('<II', kind, language)
        if target is not None:
            body += target.encode() + b'\0'
        connection.send(0x95, body + message.encode() + b'\0')

    def line(kind, speaker, message, language=0, tag=0):
        body = struct.pack('<BIQ', kind, language, speaker)
        if kind in (0, 1, 5):
            body += struct.pack('<Q', speaker)
        encoded = message.encode() + b'\0'
        return 0x96, body + struct.pack('<I', len(encoded)) + encoded + bytes([tag])

    def chat(packets):
        return [packet for packet in packets if packet[0] == 0x96]

    host.restore('game_account', "username = 'NATIVE'", 'gm_level')
    host.call('set_gm_level', ['Native', 1])
    send(client, 0, '  native say  ')
    expected = line(0, guid, 'native say', 7)
    assert chat(client.collect()) == [expected]
    assert chat(peer.collect()) == [expected]
    send(client, 0, 'unknown language', 1)
    assert (0x1cb, b"You don't know that language\0") in client.collect()
    assert not chat(peer.collect())
    send(client, 0, 'addon say', 0xffffffff)
    assert not chat(client.collect())
    assert not chat(peer.collect())
    client.send(0x104, struct.pack('<IIQ', 101, 3, peer_guid))
    emote = client.collect()
    assert any(opcode == 0x105 and b'Peer\0' in body for opcode, body in emote)
    assert any(opcode == 0x103 for opcode, _ in emote)
    assert [packet for packet in peer.collect() if packet[0] in (0x103, 0x105)] == [packet for packet in emote if packet[0] in (0x103, 0x105)]
    host.call('debug_teleport', [peer_guid, 0, 320, 0, 100, 0])
    peer.collect()
    client.collect()
    assert not host.rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {peer_guid} AND entity_guid = {guid} AND layer = 0')
    send(client, 0, 'outside say')
    assert chat(client.collect()) == [line(0, guid, 'outside say', 7)]
    assert not chat(peer.collect())
    send(client, 5, 'yell edge')
    expected = line(5, guid, 'yell edge', 7)
    assert chat(client.collect()) == [expected]
    assert chat(peer.collect()) == [expected]
    host.call('debug_teleport', [peer_guid, 0, 321, 0, 0, 0])
    peer.collect()
    client.collect()
    send(client, 5, 'beyond yell')
    client.collect()
    assert not chat(peer.collect())
    host.call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    assert any(opcode == 0x3e for opcode, _ in peer.collect(auto_ack=False))
    client.collect()
    send(client, 6, 'loading whisper', target='Peer')
    assert chat(client.collect()) == [line(7, peer_guid, 'loading whisper')]
    assert chat(peer.collect(auto_ack=False)) == [line(6, guid, 'loading whisper')]
    peer.send(0xdc)
    peer.collect()
    send(client, 5, 'other map')
    client.collect()
    assert not chat(peer.collect())
    send(client, 6, 'cross map whisper', target='pEeR')
    assert chat(client.collect()) == [line(7, peer_guid, 'cross map whisper')]
    assert chat(peer.collect()) == [line(6, guid, 'cross map whisper')]
    send(peer, 20, 'Back soon')
    peer.collect()
    send(client, 6, 'away whisper', target='Peer')
    assert chat(client.collect()) == [line(7, peer_guid, 'away whisper'), line(20, peer_guid, 'Back soon')]
    assert chat(peer.collect()) == [line(6, guid, 'away whisper')]
    host.restore('game_character_contact', 'id = 5090901')
    host.sql(f"INSERT INTO game_character_contact (id,owner_guid,owner_identity,target_guid,is_ignore) VALUES (5090901,{peer_guid},0x{'00' * 32},{guid},true)")
    send(client, 6, 'ignored whisper', target='Peer')
    assert chat(client.collect()) == [line(7, peer_guid, 'ignored whisper'), line(20, peer_guid, 'Back soon'), line(22, peer_guid, 'Peer')]
    assert not chat(peer.collect())
    send(client, 1, 'party ignores the ignore list')
    expected = line(1, guid, 'party ignores the ignore list', 7)
    assert chat(client.collect()) == [expected]
    assert chat(peer.collect()) == [expected]
    send(client, 1, '  PREFIX\taddon bytes  ', 0xffffffff)
    expected = line(1, guid, '  PREFIX\taddon bytes  ', 0xffffffff)
    assert chat(client.collect()) == [expected]
    assert chat(peer.collect()) == [expected]
    host.sql('DELETE FROM game_character_contact WHERE id = 5090901')
    send(peer, 21, 'Busy')
    peer.collect()
    send(client, 6, 'dnd whisper', target='Peer')
    assert chat(client.collect()) == [line(7, peer_guid, 'dnd whisper'), line(21, peer_guid, 'Busy')]
    assert chat(peer.collect()) == [line(6, guid, 'dnd whisper')]
    send(peer, 6, 'dnd speaker', target='Native')
    peer.collect()
    assert chat(client.collect()) == [line(6, peer_guid, 'dnd speaker', tag=2)]
    peer.send(0x225, struct.pack('<Q', guid))
    peer.collect()
    assert chat(client.collect()) == [line(22, peer_guid, 'Peer')]
    peer.send(0x4b)
    peer.collect()
    send(client, 6, 'offline whisper', target='Peer')
    assert (0x2a9, b'Peer\0') in client.collect()
    host.sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    send(client, 6, 'enemy whisper', target='Peer')
    assert (0x219, b'') in client.collect()
    assert not chat(peer.collect())
    peer.send(0x4b)
    peer.collect()
    host.sql(f'UPDATE game_character SET race = 1 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    host.call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    client.collect()
    host.call('set_gm_level', ['Native', 0])
    client.send(0x4b)
    client.collect()
    client.send(0x3d, struct.pack('<Q', guid))
    client.collect()
    peer.collect()
    time.sleep(1.05)
    for index in range(12):
        send(client, 0, f'burst {index}')
    burst = client.collect()
    assert chat(burst) == [line(0, guid, f'burst {index}', 7) for index in range(11)]
    assert any(opcode == 0x1cb and body.startswith(b'You must wait ') for opcode, body in burst)
    assert chat(peer.collect()) == chat(burst)
    client.send(0x4b)
    client.collect()
    client.send(0x3d, struct.pack('<Q', guid))
    client.collect()
    peer.collect()
    send(client, 0, 'still muted')
    assert any(opcode == 0x1cb for opcode, _ in client.collect())
    assert not chat(peer.collect())
    client.send(0x104, struct.pack('<IIQ', 101, 3, peer_guid))
    assert any(opcode == 0x1cb for opcode, _ in client.collect())
    assert not any(opcode in (0x103, 0x105) for opcode, _ in peer.collect())
    send(client, 20, 'Muted but away')
    client.collect()
    send(peer, 6, 'away during mute', target='Native')
    assert chat(peer.collect()) == [line(7, guid, 'away during mute'), line(20, guid, 'Muted but away')]
    assert chat(client.collect()) == [line(6, peer_guid, 'away during mute')]
    send(client, 0, 'unknown while muted', 1)
    assert (0x1cb, b"You don't know that language\0") in client.collect()
    send(client, 20, '')
    client.collect()
    peer.collect()


def check_accept_recovery(host):
    resource.prlimit(host.pid, resource.RLIMIT_NOFILE, (256, 256))
    for kind in ['World', 'Logon']:
        clients = []
        before = host.log_text().count(f'native {kind} accept:')
        try:
            for _ in range(300):
                try:
                    clients.append(socket.create_connection(('127.0.0.1', host.world_port), timeout=1))
                except OSError:
                    break
            if kind == 'Logon':
                clients.append(socket.create_connection(('127.0.0.1', host.logon_port), timeout=1))
            host.wait_until(lambda: host.log_text().count(f'native {kind} accept:') > before,
                f'{kind} listener did not reach descriptor exhaustion', timeout=3)
        finally:
            for client in clients:
                client.close()
        deadline = time.monotonic() + 15
        while True:
            try:
                with socket.create_connection(('127.0.0.1', host.world_port), timeout=2) as recovered:
                    assert read_exact(recovered, 4) == AUTH_CHALLENGE
                recovered = Logon(host.logon_port)
                try:
                    assert recovered.challenge('UNKNOWN') == 4
                finally:
                    recovered.close()
                assert host.sql('SELECT id FROM game_account')
                break
            except (OSError, EOFError):
                if time.monotonic() >= deadline:
                    raise AssertionError(f'{kind} listener failed to recover') from None
                time.sleep(0.1)


def check_session_expiry(host):
    active = host.account('ACTIVEEXPIRY')
    idle = host.account('IDLEEXPIRY')
    client = host.log_in(host.character('Expiry', account=active))
    host.call('debug_expire_session', [active.id])
    host.call('debug_expire_session', [idle.id])
    host.call('debug_accelerate_session_reaper')
    host.wait_until(lambda: not host.rows(f'SELECT account_id FROM game_session WHERE account_id = {idle.id}'),
        'Session reaper did not remove the expired idle Session')
    client.collect()
    assert host.rows(f'SELECT account_id FROM game_session WHERE account_id = {active.id}')
    expired = host.connect('ACTIVEEXPIRY')
    assert expired.read(False) == (0x1ee, bytes([13]))
    expired.close()
    client.collect()
    client.close()
    host.await_empty('game_native_player')
    host.wait_until(lambda: not host.rows(f'SELECT account_id FROM game_session WHERE account_id = {active.id}'),
        'Expired Session remained after its World Session closed')


def check_gm_commands(host):
    account = host.account('GMACCOUNT')
    host.call('revoke_alpha_test_tools', ['GMACCOUNT'])
    alt = host.character('Gmalt', account=account)
    client = host.enter_world('Gmmain', account=account)

    def command(text):
        client.send(0x95, struct.pack('<II', 0, 7) + text.encode() + b'\0')
        return b''.join(body for opcode, body in client.collect() if opcode == 0x96)

    def speed():
        return host.rows(f'SELECT run_speed_mult_bp FROM game_world_entity WHERE guid = {client.guid}')[0][0]

    assert b'permission denied' in command('.speed 2')
    assert speed() == 10000
    host.call('grant_alpha_test_tools', ['GMACCOUNT'])
    command('.speed 2')
    assert speed() == 20000
    assert b'permission denied' in command('.gps')
    host.call('set_gm_level', ['Gmmain', 1])
    assert b'Position: map' in command('.gps')
    client.send(0x4b)
    client.collect()
    client.send(0x3d, struct.pack('<Q', alt))
    client.collect()
    assert b'Position: map' in command('.gps'), 'the GM level covers every Character on the Account'


def check_who_bounds(host):
    clients = [host.enter_world(name) for name in ['Healthywho', 'Zonewho', 'Stringwho']]
    for client, offset in zip(clients[1:], [18, 22]):
        malformed = bytearray(26)
        struct.pack_into('<I', malformed, offset, 0xffffffff)
        client.send(0x62, malformed)
        client.read_until_closed()
        clients[0].collect()
        assert host.rows('SELECT COUNT(*) AS n FROM game_native_player')[0][0] >= 1


def check_decoder_bounds(host):
    guid = host.character('Decoder')
    cases = [(opcode, bytes(offset) + b'\x02') for opcode, offset in [(0x253, 0), (0x28f, 8), (0x2f3, 12), (0x2ee, 16)]]
    cases += [(0x1ca, struct.pack('<III', 0, 0, 1) + b'\0'),
              (0x1ca, struct.pack('<II', 0, 2) + b'a\x02' + struct.pack('<I', 1) + b'\0')]
    before = len(host.log_text())
    for in_world in [False, True]:
        for opcode, body in cases:
            client = host.connect('DECODER')
            assert client.read()[0] == 0x1ee
            if in_world:
                client.send(0x3d, struct.pack('<Q', guid))
                client.collect()
            client.send(opcode, body)
            if in_world:
                client.read_until_closed()
            else:
                client.send(0x37)
                assert any(opcode == 0x3b for opcode, _ in client.collect())
            client.close()
            host.await_empty('game_native_player')
        client = host.connect('DECODER')
        assert client.read()[0] == 0x1ee
        if in_world:
            client.send(0x3d, struct.pack('<Q', guid))
            client.collect()
        ticket = b'\x02' + bytes(18) + struct.pack('<I', 0xffffffff) + bytes([0xff]) * 16
        client.send(0x205, ticket)
        if in_world:
            assert (0x206, struct.pack('<I', 3)) in client.collect()
        else:
            client.send(0x37)
            assert any(opcode == 0x3b for opcode, _ in client.collect())
        client.close()
        host.await_empty('game_native_player')
    host.await_empty('game_native_world_connection')
    errors = host.log_text()[before:]
    assert 'fatal error' not in errors and 'panicked' not in errors, errors


def check_session_revocation(host):
    for index, cause in enumerate(['ban', 'missing', 'replaced', 'banignored']):
        account = host.account('REVOKE' + cause.upper())
        client = host.log_in(host.character('Revoked' + chr(ord('a') + index), account=account))
        guid = client.guid
        position = host.rows(f'SELECT x, y, z FROM game_world_entity WHERE guid = {guid}')
        if cause.startswith('ban'):
            host.sql(f'UPDATE game_account SET banned = true WHERE id = {account.id}')
        elif cause == 'missing':
            host.sql(f'DELETE FROM game_session WHERE account_id = {account.id}')
        else:
            host.call('establish_session', [account.id, [0x59] * 40, [account.identity]])
        client.send(0x0331 if cause == 'banignored' else 0xee,
            struct.pack('<IIffffI', 0, 1000, 1000, 1000, 0, 0, 0))
        deadline = time.monotonic() + 5
        client.socket.settimeout(2)
        while time.monotonic() < deadline:
            try:
                client.read()
            except EOFError:
                break
            except socket.timeout:
                raise AssertionError(f'{cause} did not close the World Session') from None
        else:
            raise AssertionError(f'{cause} did not close the World Session')
        host.await_empty('game_native_player')
        assert host.rows(f'SELECT x, y, z FROM game_character WHERE guid = {guid}') == position
        assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')
        client.close()


def check_slow_readers(host):
    slow, healthy = host.enter_world('Slowreader'), host.enter_world('Fastreader')
    sender = None
    stopped = threading.Event()
    try:
        slow.collect()
        healthy.collect()
        text_id = 5090999
        text = 'x' * 16384
        host.sql(f"INSERT INTO game_npc_text (text_id, text) VALUES ({text_id}, '{text}')")
        query = struct.pack('<IQ', text_id, 0)
        slow.send(0x17f, query)
        reply = next(body for opcode, body in slow.collect() if opcode == 0x180)
        assert len(reply) > len(text) * 2
        slow.socket.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 2048)
        requests_sent = 0

        def flood():
            nonlocal requests_sent
            try:
                for _ in range(512):
                    if stopped.is_set():
                        break
                    slow.send(0x17f, query)
                    requests_sent += 1
            except OSError:
                pass

        sender = threading.Thread(target=flood)
        sender.start()
        started, latencies = time.monotonic(), []
        while time.monotonic() - started < 15:
            ping_started = time.monotonic()
            healthy.collect()
            latencies.append(time.monotonic() - ping_started)
            live = host.rows('SELECT character_guid FROM game_native_player')
            assert [healthy.guid] in live
            if [slow.guid] not in live:
                break
            time.sleep(0.05)
        else:
            raise AssertionError(f'slow reader remained connected after {requests_sent} replies of {len(reply) + 4} bytes')
        assert max(latencies) < 1, latencies
        print(json.dumps({'check': 'slow_reader_isolation',
            'slow_reader_closed_seconds': time.monotonic() - started,
            'healthy_request_max_ms': max(latencies) * 1000}, indent=2))
    finally:
        stopped.set()
        slow.close()
        healthy.close()
        if sender:
            sender.join(timeout=2)
            assert not sender.is_alive(), 'slow-reader sender did not stop'
    host.await_empty('game_native_player')
    host.await_empty('game_native_world_connection')
    host.await_empty('game_aoi_viewer')
    host.await_empty('game_aoi_grant')


def check_recovery(host):
    account = host.account('RECOVERY', password='PASSWORD')
    logon = Logon(host.logon_port)
    assert logon.challenge('RECOVERY') == 0
    status, key = logon.prove('PASSWORD')
    assert status == 0
    client = host.log_in(host.character('Recovered', account=account), key=key)
    guid = client.guid
    client.send(0xb7, struct.pack('<IIffffI', 0, 1000, 1000, 1000, 0, 0, 0))
    client.send(0x97, b'Recovery channel\0\0')
    client.collect()
    assert host.rows(f'SELECT x, y, z FROM game_world_entity WHERE guid = {guid}') == [[1000, 1000, 0]]
    assert host.rows(f'SELECT id FROM game_chat_channel_member WHERE character_guid = {guid}')
    items = host.rows(f'SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}')
    assert items
    host.restart()
    client.close()
    logon.close()
    for table in ['game_native_player', 'game_native_world_connection', 'game_native_cipher',
                  'game_native_logon_connection', 'game_native_admission', 'game_aoi_viewer', 'game_aoi_grant',
                  'game_chat_channel_member']:
        host.await_empty(table)
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')
    assert host.rows(f'SELECT online, x, y, z FROM game_character WHERE guid = {guid}') == [[False, 1000, 1000, 0]]
    assert sorted(host.rows(f'SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}')) == sorted(items)
    assert all(row[0] == 0 for row in host.rows('SELECT active FROM game_native_logon_address'))
    client = host.log_in(guid, key=key)
    assert creates(client.entered).count(guid) == 1
    assert host.rows(f'SELECT x, y, z FROM game_world_entity WHERE guid = {guid}') == [[1000, 1000, 0]]
    client.send(0x97, b'Recovery channel\0\0')
    assert any(opcode == 0x99 for opcode, _ in client.collect())


def check_operator_reducers(host):
    for reducer, arguments in [('debug_age_corpse', [0, 600]), ('debug_backfill_cell_ids', []), ('native_set_admission_limits', [1, 1, 1])]:
        try:
            host.call(reducer, arguments, operator=False)
        except RuntimeError as refusal:
            assert 'operator only' in str(refusal), refusal
        else:
            raise AssertionError(f'{reducer} accepted an anonymous caller')


def check_logon(host):
    account = host.account('LOGON', password='PASSWORD')
    logon = Logon(host.logon_port)
    assert logon.challenge('LOGON') == 0
    started = time.monotonic()
    assert logon.prove('WRONG') == (5, None)
    assert time.monotonic() - started >= 0.18
    assert not host.rows(f'SELECT account_id FROM game_session WHERE account_id = {account.id}')
    assert logon.challenge('logon') == 0
    status, native_key = logon.prove('PASSWORD')
    assert status == 0
    assert host.rows(f'SELECT session_key FROM game_session WHERE account_id = {account.id}')[0][0] == native_key.hex()
    realms = logon.realms()
    assert realms[4] >= 1
    logon.close()
    stale = host.connect('LOGON', key=native_key)
    assert stale.read()[0] == 0x1ee
    logon = Logon(host.logon_port)
    assert logon.challenge('LOGON') == 0
    status, native_key = logon.prove('PASSWORD')
    assert status == 0
    logon.close()
    stale.send(0x37)
    assert stale.socket.recv(1) == b''
    stale.close()
    for options, expected in [({'valid_proof': False}, 13), ({'build': 1234}, 20), ({'username': 'UNKNOWN'}, 21)]:
        client = host.connect(**({'username': 'LOGON', 'key': native_key} | options))
        assert client.read(False) == (0x1ee, bytes([expected])), options
        assert client.socket.recv(1) == b''
        client.close()
    client = host.connect('LOGON', key=native_key)
    assert client.read() == (0x1ee, bytes([12]) + bytes(9))
    ignored_requests(client)
    client.send(0x1dc, struct.pack('<II', 123, 20))
    assert client.read() == (0x1dd, struct.pack('<I', 123))
    client.send(0x37)
    assert client.read() == (0x3b, b'\0')
    client.send(0x1ce)
    opcode, body = client.read()
    assert opcode == 0x1cf and abs(struct.unpack('<I', body)[0] - time.time()) < 5


def check_character_create(host):
    host.account('NATIVE')
    client = host.connect()
    assert client.read() == (0x1ee, bytes([12]) + bytes(9))
    character_body = b'Native\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0])
    client.send(0x36, character_body)
    assert client.read() == (0x3a, bytes([0x2e]))
    client.send(0x36, character_body)
    assert client.read() == (0x3a, bytes([0x31]))
    allocator = host.rows('SELECT high_water FROM game_guid_allocator WHERE id = 0')[0][0]
    old_size = host.rows('SELECT size FROM game_guid_range WHERE id = 0')[0][0]
    host.restore('game_guid_range', 'id = 0', 'size')
    host.sql(f'UPDATE game_guid_range SET size = {allocator + 2} WHERE id = 0')
    client.send(0x36, b'Rollback\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
    assert client.read() == (0x3a, bytes([0x2f]))
    assert not host.rows("SELECT guid FROM game_character WHERE name = 'Rollback'")
    assert host.rows('SELECT high_water FROM game_guid_allocator WHERE id = 0')[0][0] == allocator
    host.sql(f'UPDATE game_guid_range SET size = {old_size} WHERE id = 0')
    guid = host.rows("SELECT guid FROM game_character WHERE name = 'Native'")[0][0]
    host.sql(f'UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {guid}')
    client.send(0x3d, struct.pack('<Q', 1))
    assert client.read() == (0x41, bytes([1]))
    assert not host.rows('SELECT * FROM game_native_player')
    client.send(0x3d, struct.pack('<Q', guid))
    initial = client.collect()
    assert (0x2f4, bytes(12) + bytes([0])) in initial
    assert initial[0][0] == 0x236, [hex(opcode) for opcode, _ in initial]
    assert creates(initial).count(guid) == 1


def check_queries(host):
    client = host.enter_world('Native', at=(0, 0, 0, 0))
    guid = client.guid
    original_position = host.rows(f'SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {guid}')
    ignored_requests(client)
    assert host.rows(f'SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {guid}') == original_position
    client.send(0x26a, struct.pack('<Q', guid))
    assert not any(opcode == 0x96 for opcode, _ in client.collect())
    client.send(0x26a, struct.pack('<Q', 0xffff_ffff))
    assert any(opcode == 0x96 for opcode, _ in client.collect())
    client.send(0xfe, struct.pack('<I', 15))
    assert sum(opcode == 0x96 for opcode, _ in client.collect()) == 1
    client.send(0xfe, struct.pack('<I', 15))
    assert not any(opcode == 0x96 for opcode, _ in client.collect())
    client.send(0x21a)
    assert (0x21b, bytes(4)) in client.collect()
    client.send(0x60, struct.pack('<IQ', 51000, 0))
    queries = client.collect()
    assert any(opcode == 0x61 and body.startswith(struct.pack('<I', 51000) + b'Test Wolf\0') for opcode, body in queries)
    client.send(0x56, struct.pack('<IQ', 0x7fff_ffff, 0))
    assert (0x58, struct.pack('<I', 0xffff_ffff)) in client.collect()
    client.send(0x5e, struct.pack('<IQ', 50100, 0))
    expected_gameobject = struct.pack('<III', 50100, 3, 259) + b'Battered Chest' + bytes(5)
    expected_gameobject += struct.pack('<24I', 52, *([0] * 23))
    assert (0x5f, expected_gameobject) in client.collect()
    client.send(0x5e, struct.pack('<IQ', 0x7fff_ffff, 0))
    assert (0x5f, struct.pack('<I', 0xffff_ffff)) in client.collect()
    client.send(0x17f, struct.pack('<IQ', 0x7fff_ffff, 0))
    assert any(opcode == 0x180 and body[:4] == struct.pack('<I', 0x7fff_ffff) for opcode, body in client.collect())
    client.send(0x50, struct.pack('<Q', 0xffff_ffff))
    assert not any(opcode == 0x51 for opcode, _ in client.collect())


def check_visibility(host):
    client = host.enter_world('Native', at=(0, 0, 0, 0))
    guid = client.guid
    peer = host.log_in(host.character('Peer', cls=8, at=(0, 10, 0, 0)))
    peer_guid = peer.guid
    initial = peer.entered
    assert creates(initial).count(peer_guid) == 1
    assert creates(initial).count(guid) == 1
    private_fields = {716, 717, 1176, *range(150, 163), 165, 166, 169, 170, 171, *range(486, 612), *range(718, 1102)}
    assert not private_fields.intersection(values(initial, guid, create=True))
    assert {716, 717, 1176}.issubset(values(initial, peer_guid, create=True))
    assert creates(client.collect()).count(peer_guid) == 1
    client.send(0x1ca, struct.pack('<II', 0, 9) + b'stuck in\0' + struct.pack('<I', 4) + b'Map\0')
    assert client.collect() == []
    host.call('debug_set_money', [guid, 123456])
    assert values(client.collect(), guid)[1176] == 123456
    assert not private_fields.intersection(values(peer.collect(), guid))
    host.call('debug_set_level', [guid, 2])
    leveled = client.collect()
    assert 717 in values(leveled, guid)
    observed = values(peer.collect(), guid)
    assert observed[34] == 2
    assert not private_fields.intersection(observed)
    host.call('debug_set_level', [guid, 1])
    client.collect()
    peer.collect()


def check_inventory(host):
    client, peer = native_and_peer(host, at=(0, 0, 0, 0))
    guid = client.guid
    old_size = host.rows('SELECT size FROM game_guid_range WHERE id = 0')[0][0]
    host.call('debug_grant_item', [guid, 5090052, 6])
    food_guid, food_slot = host.rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5090052')[0]
    grant = client.collect()
    assert food_guid in creates(grant)
    assert item_pointer(values(grant, guid), food_slot) == food_guid
    assert any(opcode == 0x166 for opcode, _ in grant)
    used = {row[0] for row in host.rows(f'SELECT slot FROM game_item_instance WHERE owner_guid = {guid}')}
    split_slot = next(slot for slot in range(23, 39) if slot not in used)
    client.send(0x10e, bytes([255, food_slot, 255, split_slot, 2]))
    split = client.collect()
    split_guid, count = host.rows(f'SELECT guid, stack_count FROM game_item_instance WHERE owner_guid = {guid} AND slot = {split_slot}')[0]
    assert count == 2
    assert split_guid in creates(split)
    assert values(split, food_guid)[14] == 4
    assert item_pointer(values(split, guid), split_slot) == split_guid
    host.call('debug_grant_item', [guid, 51, 1])
    tunic_guid, tunic_slot = host.rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = 51 AND slot >= 23')[0]
    client.collect()
    peer.collect()
    client.send(0x10d, bytes([food_slot, tunic_slot]))
    swapped = client.collect()
    pointers = values(swapped, guid)
    assert item_pointer(pointers, food_slot) == tunic_guid
    assert item_pointer(pointers, tunic_slot) == food_guid
    assert not any(field >= 486 and field < 612 for field in values(peer.collect(), guid))
    host.restore('game_spell', 'spell_id = 50115', 'cost')
    host.sql('UPDATE game_spell SET cost = 999999 WHERE spell_id = 50115')
    client.send(0xab, bytes([255, split_slot, 0, 0, 0]))
    refused = client.collect()
    assert any(opcode == 0x112 for opcode, _ in refused)
    assert not values(refused, split_guid)
    assert split_guid not in destroys(refused)
    assert host.rows(f'SELECT stack_count FROM game_item_instance WHERE guid = {split_guid}') == [[2]]
    host.sql('UPDATE game_spell SET cost = 0 WHERE spell_id = 50115')
    client.send(0x111, bytes([255, split_slot, 0, 0, 0, 0]))
    removed = client.collect()
    assert split_guid in destroys(removed)
    assert item_pointer(values(removed, guid), split_slot) == 0
    assert not host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {split_guid}')
    weapon_guid = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND slot = 15')[0][0]
    client.send(0x10b, bytes([255, 15, 255]))
    unequipped = client.collect()
    weapon_slot = host.rows(f'SELECT slot FROM game_item_instance WHERE guid = {weapon_guid}')[0][0]
    assert weapon_slot >= 23
    assert item_pointer(values(unequipped, guid), 15) == 0
    assert item_pointer(values(unequipped, guid), weapon_slot) == weapon_guid
    assert values(peer.collect(), guid).get(440) == 0, 'the main-hand visible item entry clears for observers'
    client.send(0x10a, bytes([255, weapon_slot]))
    equipped = client.collect()
    assert item_pointer(values(equipped, guid), 15) == weapon_guid
    assert item_pointer(values(equipped, guid), weapon_slot) == 0
    assert host.rows(f'SELECT slot FROM game_item_instance WHERE guid = {weapon_guid}') == [[15]]
    peer.collect()
    vendor = spawn(host, guid, 51004, 2)
    host.sql(f'UPDATE game_world_entity SET money = 50000 WHERE guid = {guid}')
    client.collect()
    peer.collect()
    client.send(0x19e, struct.pack('<Q', vendor))
    stock = next(body for opcode, body in client.collect() if opcode == 0x19f)
    assert stock[:9] == struct.pack('<QB', vendor, 2)
    assert struct.unpack_from('<I', stock, 9 + 28 + 16)[0] == 10
    client.send(0x1a2, struct.pack('<QIBB', vendor, 5090052, 2, 1))
    bought = client.collect()
    assert values(bought, food_guid)[14] == 6
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}') == [[49980]]
    client.send(0x1a2, struct.pack('<QIBB', vendor, 5090050, 17, 1))
    refused = client.collect()
    assert (0x1a5, struct.pack('<QIB', vendor, 5090050, 8)) in refused, ('BUY_ERR_CANT_CARRY_MORE', refused)
    assert not values(refused, guid) and not creates(refused)
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}') == [[49980]]
    assert not host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5090050')
    client.send(0x1a0, struct.pack('<QQB', vendor, food_guid, 2))
    sold = client.collect()
    assert values(sold, food_guid)[14] == 4
    assert item_pointer(values(sold, guid), 69) == 0x4090_0000_0000_0000
    assert values(sold, guid)[1226] == 4
    assert host.rows(f'SELECT stack_count, price FROM game_character_buyback WHERE player_guid = {guid}') == [[2, 4]]
    client.send(0x290, struct.pack('<QI', vendor, 69))
    bought_back = client.collect()
    assert values(bought_back, food_guid)[14] == 6
    assert item_pointer(values(bought_back, guid), 69) == 0
    assert 0x4090_0000_0000_0000 in destroys(bought_back)
    assert not host.rows(f'SELECT id FROM game_character_buyback WHERE player_guid = {guid}')
    host.sql(f'UPDATE game_world_entity SET npc_flags = 261 WHERE guid = {vendor}')
    client.send(0x1b7, struct.pack('<Q', vendor))
    assert (0x1b8, struct.pack('<Q', vendor)) in client.collect()
    client.send(0x283, bytes([255, tunic_slot]))
    banked = client.collect()
    assert item_pointer(values(banked, guid), tunic_slot) == 0
    assert item_pointer(values(banked, guid), 39) == food_guid
    client.send(0x282, bytes([255, 39]))
    withdrawn = client.collect()
    assert item_pointer(values(withdrawn, guid), 39) == 0
    assert host.rows(f'SELECT slot FROM game_item_instance WHERE guid = {food_guid}')[0][0] < 39
    client.send(0x1b9, struct.pack('<Q', vendor))
    bank_slot = client.collect()
    assert (0x1ba, struct.pack('<I', 3)) in bank_slot
    assert values(bank_slot, guid)[194] >> 16 & 255 == 1
    assert host.rows(f'SELECT bank_bag_slots, money FROM game_world_entity WHERE guid = {guid}') == [[1, 48980]]
    client.send(0x17c, struct.pack('<QI', vendor, 0))
    assert (0x17e, b'') in client.collect()
    for row_id, index, text, action in [(5090901, 0, 'Shop', 3), (5090902, 1, 'Bank', 9)]:
        host.restore('game_gossip_option', f'row_id = {row_id}')
        host.sql(f"INSERT INTO game_gossip_option (row_id,entry,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES ({row_id},51004,{index},0,'{text}',{action},0,0,0,0)")
    client.send(0x17b, struct.pack('<Q', vendor))
    menu = next(body for opcode, body in client.collect() if opcode == 0x17d)
    assert struct.unpack_from('<I', menu, 12)[0] == 2 and b'Shop\0' in menu and b'Bank\0' in menu
    host.sql('UPDATE game_gossip_option SET cond_type = 3 WHERE row_id = 5090901')
    client.send(0x17c, struct.pack('<QI', vendor, 1))
    assert (0x1b8, struct.pack('<Q', vendor)) in client.collect()
    client.send(0x17c, struct.pack('<QI', vendor, 0))
    refused = client.collect()
    assert (0x17e, b'') in refused and not any(opcode == 0x19f for opcode, _ in refused)
    client.send(0x17b, struct.pack('<Q', vendor))
    menu = next(body for opcode, body in client.collect() if opcode == 0x17d)
    assert struct.unpack_from('<I', menu, 12)[0] == 1 and b'Shop\0' not in menu
    host.restore('game_gossip_menu_profile', 'menu_id = 5090901')
    host.restore('game_creature_gossip_menu_override', f'creature_guid = {vendor}')
    host.sql('INSERT INTO game_gossip_menu_profile (menu_id,text_id) VALUES (5090901,1)')
    host.sql(f'INSERT INTO game_creature_gossip_menu_override (creature_guid,menu_id,map_id,instance_id) VALUES ({vendor},5090901,0,0)')
    client.send(0x17c, struct.pack('<QI', vendor, 0))
    assert (0x17e, b'') in client.collect()
    host.sql(f'DELETE FROM game_creature_gossip_menu_override WHERE creature_guid = {vendor}')
    host.sql('DELETE FROM game_gossip_option WHERE row_id = 5090901')
    host.sql('DELETE FROM game_gossip_option WHERE row_id = 5090902')
    host.sql(f'UPDATE game_world_entity SET npc_flags = 129 WHERE guid = {vendor}')
    client.send(0x17b, struct.pack('<Q', vendor))
    menu = next(body for opcode, body in client.collect() if opcode == 0x17d)
    assert b'Make this inn your home.\0' in menu
    client.send(0x17c, struct.pack('<QI', vendor, 0))
    assert (0x2eb, struct.pack('<Q', vendor)) in client.collect()
    client.send(0x1b5, struct.pack('<Q', vendor))
    bound = client.collect()
    assert any(opcode == 0x158 and body[:8] == struct.pack('<Q', vendor) for opcode, body in bound)
    assert host.rows(f'SELECT home_map, home_x, home_y, home_z FROM game_character WHERE guid = {guid}') == host.rows(f'SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {guid}')
    host.sql(f'UPDATE game_world_entity SET npc_flags = 261 WHERE guid = {vendor}')
    host.sql(f'UPDATE game_world_entity SET dead = true, health = 0 WHERE guid = {vendor}')
    client.send(0x1b9, struct.pack('<Q', vendor))
    assert (0x1ba, struct.pack('<I', 2)) in client.collect()
    assert host.rows(f'SELECT bank_bag_slots, money FROM game_world_entity WHERE guid = {guid}') == [[1, 48980]]
    trainer = spawn(host, guid, 51001, 2)
    client.collect()
    peer.collect()
    client.send(0x1b0, struct.pack('<Q', trainer))
    assert any(opcode == 0x1b1 and body[:8] == struct.pack('<Q', trainer) for opcode, body in client.collect())
    client.send(0x1b2, struct.pack('<QI', trainer, 2050))
    trained = client.collect()
    assert (0x1b3, struct.pack('<QI', trainer, 2050)) in trained
    assert (0x12b, struct.pack('<I', 2050)) in trained
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}') == [[48880]]
    assert host.rows(f'SELECT spell_id FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 2050') == [[2050]]
    client.send(0x1b2, struct.pack('<QI', trainer, 2050))
    refused = client.collect()
    assert (0x1b4, struct.pack('<QII', trainer, 2050, 0)) in refused, 'TRAINER_BUY_FAILED Unavailable'
    assert not any(opcode in (0x12b, 0xa9) for opcode, _ in refused)
    host.call('debug_set_skill', [guid, 356, 3])
    skills = values(client.collect(), guid)
    assert any(value == 356 and skills.get(index + 1) == (5 << 16 | 3) for index, value in skills.items())
    host.call('debug_set_skill', [guid, 356, 4])
    skills = values(client.collect(), guid)
    assert any(value == 356 and skills.get(index + 1) == (5 << 16 | 4) for index, value in skills.items())
    assert not any(opcode in (0x19f, 0x1b1, 0x1b3, 0x12b, 0x1ba, 0x166) for opcode, _ in peer.collect())
    host.sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES (5090171,'Fixture craft',0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0)")
    for index in (0, 1):
        host.sql(f'INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES ({20360684 + index},5090171,{index},7,1,0,0.0,0,0,0.0,0,0,0,5090050,8,0,0,false,0.0)')
    host.sql('INSERT INTO game_spell_reagent (id,spell_id,item_entry,count) VALUES (40721368,5090171,5090052,1)')
    host.call('debug_learn_spell', [guid, 5090171])
    client.collect()
    items_before = host.rows(f'SELECT * FROM game_item_instance WHERE owner_guid = {guid}')
    allocator = host.rows('SELECT high_water FROM game_guid_allocator WHERE id = 0')[0][0]
    host.restore('game_guid_range', 'id = 0', 'size')
    host.sql(f'UPDATE game_guid_range SET size = {allocator + 2} WHERE id = 0')
    client.send(0x12e, cast(5090171))
    craft_failed = client.collect()
    assert not creates(craft_failed) and not destroys(craft_failed)
    assert not any(opcode == 0x166 for opcode, _ in craft_failed)
    assert not values(craft_failed, food_guid)
    assert host.rows(f'SELECT * FROM game_item_instance WHERE owner_guid = {guid}') == items_before
    assert host.rows('SELECT high_water FROM game_guid_allocator WHERE id = 0') == [[allocator]]
    host.sql(f'UPDATE game_guid_range SET size = {old_size} WHERE id = 0')
    client.send(0x12e, cast(5090171))
    crafted = client.collect()
    assert len(creates(crafted)) == 2
    assert values(crafted, food_guid)[14] == 5
    assert sum(opcode == 0x166 for opcode, _ in crafted) == 2
    assert len(host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5090050')) == 2
    host.sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES (5090172,'Triggered product',0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0)")
    host.sql('INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES (20360688,5090172,0,7,1,0,0.0,0,0,0.0,0,0,0,5090050,8,0,0,false,0.0)')
    host.sql('UPDATE game_spell_effect SET kind = 5, trigger_spell = 5090172 WHERE id = 20360684')
    client.send(0x12e, cast(5090171))
    nested_craft = client.collect()
    assert len(creates(nested_craft)) == 1
    assert not values(nested_craft, food_guid)
    items = host.rows(f'SELECT slot FROM game_item_instance WHERE owner_guid = {guid}')
    assert len({row[0] for row in items}) == len(items)
    assert host.rows(f'SELECT stack_count FROM game_item_instance WHERE guid = {food_guid}') == [[5]]
    giver = spawn(host, guid, 51003, 2)
    client.collect()
    peer.collect()
    client.send(0x182, struct.pack('<Q', giver))
    assert any(opcode == 0x183 and body[:8] == struct.pack('<Q', giver) for opcode, body in client.collect())
    client.send(0x184, struct.pack('<Q', giver))
    assert any(opcode == 0x188 and b'Wolf Cull\0' in body for opcode, body in client.collect())
    client.send(0x189, struct.pack('<QI', giver, 50900))
    accepted = client.collect()
    assert values(accepted, guid)[198] == 50900
    assert host.rows(f'SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50900') == [[False]]
    client.send(0x189, struct.pack('<QI', giver, 50900))
    refused = client.collect()
    assert [opcode for opcode, _ in refused][-2:] == [0x18f, 0x17e]
    assert len(host.rows(f'SELECT id FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50900')) == 1
    host.restore('game_creature_loot', 'id = 5090901')
    host.restore('game_creature_template', 'entry = 51000', 'money_min', 'money_max', 'level', 'max_level')
    host.sql('DELETE FROM game_creature_loot WHERE creature_entry = 51000')
    host.sql('INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090901,51000,5090052,10000,2,0,false)')
    host.sql('UPDATE game_creature_template SET money_min = 17, money_max = 17 WHERE entry = 51000')
    host.sql(f'UPDATE game_world_entity SET level = 1, xp = 390, next_level_xp = 400 WHERE guid = {guid}')
    host.sql(f'UPDATE game_character SET rested_xp = 10 WHERE guid = {guid}')
    host.sql('UPDATE game_creature_template SET level = 1, max_level = 1 WHERE entry = 51000')
    for kills in (1, 2):
        target = spawn(host, guid, 51000, 3)
        client.collect()
        host.call('debug_apply_damage', [target, 1, guid])
        host.call('debug_kill_creature', [guid, target])
        progress = client.collect()
        assert (0x1d0, struct.pack('<QIBIf', target, 60 if kills == 1 else 50, 0, 50, 1.0)) in progress
        if kills == 1:
            assert any(opcode == 0x1d4 and body[:4] == struct.pack('<I', 2) for opcode, body in progress)
            assert host.rows(f'SELECT level, xp FROM game_world_entity WHERE guid = {guid}') == [[2, 50]]
        assert any(opcode == 0x199 and body[:12] == struct.pack('<III', 50900, 51000, kills) for opcode, body in progress)
        assert values(progress, guid)[199] & 0x3f == kills
    assert values(progress, guid)[199] >> 24 == 1
    assert values(progress, target)[143] & 1
    observed = peer.collect()
    assert not any(opcode in (0x1d0, 0x1d4) for opcode, _ in observed)
    assert values(observed, target)[143] & 1 == 0
    peer.send(0x15d, struct.pack('<Q', target))
    refused = peer.collect()
    assert any(opcode == 0x160 and body[:10] == struct.pack('<QBB', target, 0, 0) for opcode, body in refused)
    client.send(0x15d, struct.pack('<Q', target))
    loot = next(body for opcode, body in client.collect() if opcode == 0x160)
    assert loot[:14] == struct.pack('<QBIB', target, 1, 17, 1)
    assert struct.unpack_from('<BII', loot, 14) == (0, 5090052, 2)
    purse = host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]
    client.send(0x15e)
    assert (0x165, b'') in client.collect()
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}') == [[purse + 17]]
    client.send(0x108, bytes([0]))
    looted = client.collect()
    assert (0x162, bytes([0])) in looted
    assert values(looted, food_guid)[14] == 7
    assert values(looted, target)[143] & 1 == 0
    assert not host.rows(f'SELECT id FROM game_corpse_loot WHERE corpse_guid = {target}')
    client.send(0x15f, struct.pack('<Q', target))
    assert (0x161, struct.pack('<QB', target, 1)) in client.collect()
    client.send(0x15e)
    assert not any(opcode == 0x165 for opcode, _ in client.collect())
    assert not any(opcode in (0x165, 0x162, 0x166) for opcode, _ in peer.collect())
    client.send(0x18a, struct.pack('<QI', giver, 50900))
    assert any(opcode == 0x18d for opcode, _ in client.collect())
    money = host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]
    client.send(0x18e, struct.pack('<QII', giver, 50900, 0))
    reward = client.collect()
    assert any(opcode == 0x191 and body[:4] == struct.pack('<I', 50900) for opcode, body in reward)
    assert (0x1d0, struct.pack('<QIB', 0, 90, 1)) in reward
    assert values(reward, guid)[198] == 0
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}') == [[money + 150]]
    assert host.rows(f'SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50900') == [[True]]
    assert not any(opcode in (0x199, 0x191, 0x197) for opcode, _ in peer.collect())
    host.restore('game_quest_template', 'entry = 50900', 'limit_time')
    host.sql('UPDATE game_quest_template SET limit_time = 1 WHERE entry = 50900')
    client.send(0x189, struct.pack('<QI', giver, 50900))
    assert values(client.collect(), guid)[198] == 50900
    expired = []
    for _ in range(80):
        expired += client.collect()
        if any(opcode == 0x197 for opcode, _ in expired):
            break
        time.sleep(0.05)
    assert (0x197, struct.pack('<I', 50900)) in expired
    client.send(0x194, bytes([0]))
    assert values(client.collect(), guid)[198] == 0
    assert not host.rows(f'SELECT id FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50900')


def check_gossip_submenus(host):
    client = host.enter_world('Gossip', at=(0, 0, 0, 0))
    guid = client.guid
    host.call('debug_set_level', [guid, 60])
    trainer = spawn(host, guid, 51001, 2)
    host.restore('game_creature_template', 'entry = 51001', 'trainer_type', 'trainer_class')
    host.sql('UPDATE game_creature_template SET trainer_type = 0, trainer_class = 1 WHERE entry = 51001')
    host.sql(f'UPDATE game_world_entity SET npc_flags = 17 WHERE guid = {trainer}')
    host.restore('game_gossip_option', 'row_id = 5090920')
    host.sql("INSERT INTO game_gossip_option (row_id,entry,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES (5090920,51001,0,0,'I wish to unlearn my talents.',1,5090920,0,0,0)")
    for menu, text in [(5090920, 16777215), (5090921, 70)]:
        host.restore('game_gossip_menu_profile', f'menu_id = {menu}')
        host.sql(f'INSERT INTO game_gossip_menu_profile (menu_id,text_id) VALUES ({menu},{text})')
    for row, menu, index, text, action, target, condition in [
        (5090920, 5090920, 0, 'Yes. I do.', 16, 0, 0),
        (5090921, 5090920, 1, 'Hidden answer', 1, 0, 3),
        (5090922, 5090920, 2, 'More', 1, 5090921, 0),
        (5090923, 5090921, 0, 'Back', 1, 5090920, 0),
    ]:
        host.restore('game_gossip_menu_profile_option', f'row_id = {row}')
        host.sql(f"INSERT INTO game_gossip_menu_profile_option (row_id,menu_id,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES ({row},{menu},{index},0,'{text}',{action},{target},{condition},0,0)")

    def click(index):
        client.send(0x17c, struct.pack('<QI', trainer, index))
        return client.collect()

    def submenu():
        client.send(0x17b, struct.pack('<Q', trainer))
        root = next(body for opcode, body in client.collect() if opcode == 0x17d)
        assert b'I wish to unlearn my talents.\0' in root
        packets = click(0)
        assert not any(opcode == 0x17e for opcode, _ in packets)
        return next(body for opcode, body in packets if opcode == 0x17d)

    menu = submenu()
    assert struct.unpack_from('<II', menu, 8) == (16777215, 2)
    assert b'Yes. I do.\0' in menu and b'Hidden answer\0' not in menu
    nested = next(body for opcode, body in click(1) if opcode == 0x17d)
    assert struct.unpack_from('<II', nested, 8) == (70, 1) and b'Back\0' in nested
    assert any(opcode == 0x17d for opcode, _ in click(0))
    host.sql('UPDATE game_gossip_menu_profile_option SET cond_type = 3 WHERE row_id = 5090920')
    refused = click(0)
    assert (0x17e, b'') in refused and not any(opcode == 0x2aa for opcode, _ in refused)
    host.sql('UPDATE game_gossip_menu_profile_option SET cond_type = 0 WHERE row_id = 5090920')
    submenu()
    assert (0x2aa, struct.pack('<QI', trainer, 10000)) in click(0)

    for trainer_type, trainer_class in [(1, 1), (2, 1), (3, 1), (0, 8)]:
        host.sql(f'UPDATE game_creature_template SET trainer_type = {trainer_type}, trainer_class = {trainer_class} WHERE entry = 51001')
        menu = submenu()
        assert b'Yes. I do.\0' not in menu and struct.unpack_from('<I', menu, 12)[0] == 1
    host.sql('UPDATE game_creature_template SET trainer_type = 0, trainer_class = 1 WHERE entry = 51001')
    submenu()
    host.restore('game_creature_gossip_menu_override', f'creature_guid = {trainer}')
    host.sql(f'INSERT INTO game_creature_gossip_menu_override (creature_guid,menu_id,map_id,instance_id) VALUES ({trainer},5090921,0,0)')
    refused = click(0)
    assert (0x17e, b'') in refused and not any(opcode == 0x2aa for opcode, _ in refused)
    host.sql(f'DELETE FROM game_creature_gossip_menu_override WHERE creature_guid = {trainer}')
    host.sql('UPDATE game_gossip_option SET action_menu_id = 5090929 WHERE row_id = 5090920')
    client.send(0x17b, struct.pack('<Q', trainer))
    assert any(opcode == 0x17d for opcode, _ in client.collect())
    assert (0x17e, b'') in click(0)
    host.sql('UPDATE game_gossip_option SET action = 16, action_menu_id = 0 WHERE row_id = 5090920')
    client.send(0x17b, struct.pack('<Q', trainer))
    assert any(opcode == 0x17d for opcode, _ in client.collect())
    assert (0x2aa, struct.pack('<QI', trainer, 10000)) in click(0)


def check_enchantments(host):
    client, peer = native_and_peer(host)
    guid = client.guid
    weapon = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND slot = 15')[0][0]
    # Build 5875 PLAYER_VISIBLE_ITEM_16_0 is descriptor 440; the permanent enchant follows the entry.
    visible_enchant = 441
    host.restore('game_item_template', 'entry = 10940')
    template = host.sql('SELECT * FROM game_item_template WHERE entry = 5090052')[0]
    dust = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    dust.update(entry=10940, name='Strange Dust')
    literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in dust.values()]
    host.sql(f"INSERT INTO game_item_template ({','.join(dust)}) VALUES ({','.join(literals)})")
    host.call('debug_grant_item', [guid, 10940, 1])
    client.collect()
    host.call('debug_enchant_item', [guid, 15, 7745])
    enchanted = client.collect()
    assert values(enchanted, weapon)[22] == 823
    assert values(enchanted, guid)[visible_enchant] == 823
    assert values(peer.collect(), guid)[visible_enchant] == 823
    observer = host.enter_world('Observer', at=(0, 5, 0, 0))
    assert values(observer.entered, guid, create=True)[visible_enchant] == 823
    client.close()
    host.wait_until(lambda: host.rows(f'SELECT online FROM game_character WHERE guid = {guid}') == [[False]],
                    'Native did not log out')
    entered = host.log_in(guid).entered
    assert values(entered, weapon, create=True)[22] == 823
    assert values(entered, guid, create=True)[visible_enchant] == 823


def check_quest_chain(host):
    client = host.enter_world('Chainer')
    guid = client.guid
    first, second, elsewhere = range(5090670, 5090673)
    template = host.sql('SELECT * FROM game_quest_template WHERE entry = 50900')[0]
    quest = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    for entry, next_quest in [(first, second), (second, elsewhere), (elsewhere, 0)]:
        quest.update(entry=entry, title=f'Chain {entry}', repeatable=False, next_quest_id=next_quest)
        literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value).lower() if isinstance(value, bool) else str(value) for value in quest.values()]
        host.restore('game_quest_template', f'entry = {entry}')
        host.sql(f"INSERT INTO game_quest_template ({','.join(quest)}) VALUES ({','.join(literals)})")
    for relation, entry, role in [(first, first, 1), (second, second, 0), (second + 10, second, 1), (elsewhere, elsewhere, 1)]:
        host.restore('game_creature_quest', f'id = {relation}')
        host.sql(f'INSERT INTO game_creature_quest (id,creature_entry,quest_entry,role) VALUES ({relation},51003,{entry},{role})')
    giver = spawn(host, guid, 51003, 2)
    host.call('debug_grant_quest', [guid, first])
    host.restore('game_quest_event_requirement', f'id = {first}')
    host.restore('game_character_quest_event_credit', f'id = {first}')
    host.sql(f'INSERT INTO game_quest_event_requirement (id,quest_entry) VALUES ({first},{first})')
    client.collect()
    client.send(0x18e, struct.pack('<QII', giver, first, 0))
    pending = client.collect()
    assert any(opcode == 0x18b for opcode, _ in pending), pending
    assert not any(opcode in (0x18d, 0x191) for opcode, _ in pending), pending
    assert host.rows(f'SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = {first}') == [[False]]
    host.sql(f'INSERT INTO game_character_quest_event_credit (id,character_guid,quest_entry) VALUES ({first},{guid},{first})')
    client.collect()
    client.send(0x18e, struct.pack('<QII', giver, first, 0))
    turned_in = [(opcode, body) for opcode, body in client.collect() if opcode in (0x191, 0x188)]
    assert [opcode for opcode, _ in turned_in] == [0x191, 0x188], turned_in
    assert turned_in[0][1][:4] == struct.pack('<I', first)
    assert turned_in[1][1][:12] == struct.pack('<QI', giver, second)
    host.call('debug_grant_quest', [guid, second])
    client.collect()
    client.send(0x18e, struct.pack('<QII', giver, second, 0))
    last = client.collect()
    assert any(opcode == 0x191 and body[:4] == struct.pack('<I', second) for opcode, body in last)
    assert not any(opcode == 0x188 for opcode, _ in last)


def check_quest_items(host):
    client = host.enter_world('Collector')
    guid = client.guid
    host.restore('game_quest_objective', 'id = 5090660')
    host.sql('INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) VALUES (5090660,50900,1,1,5090052,8)')
    host.call('debug_grant_quest', [guid, 50900])
    client.collect()
    # The client's toast adds the packet's count to its bags, so the packet leads the item's update.
    # The tracker redraws only when a quest log counter changes, so the counter follows the item.
    for count, carried in ((3, 3), (1, 4)):
        host.call('debug_grant_item', [guid, 5090052, count])
        packets = client.collect()
        opcodes = [opcode for opcode, _ in packets]
        assert (0x19a, struct.pack('<II', 5090052, count)) in packets
        assert opcodes.index(0x19a) < opcodes.index(0xa9)
        quest_log = [index for index, packet in enumerate(packets) if 199 in values([packet], guid)]
        assert quest_log and quest_log[-1] > opcodes.index(0xa9)
        assert values(packets, guid)[199] >> 6 & 0x3f == carried
    jerky_slot = host.rows(f'SELECT slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5090052')[0][0]
    used = {row[0] for row in host.rows(f'SELECT slot FROM game_item_instance WHERE owner_guid = {guid}')}
    split_slot = next(slot for slot in range(23, 39) if slot not in used)
    client.send(0x10e, bytes([255, jerky_slot, 255, split_slot, 2]))
    split = client.collect()
    assert host.rows(f'SELECT stack_count FROM game_item_instance WHERE owner_guid = {guid} AND slot = {split_slot}') == [[2]]
    client.send(0x10d, bytes([split_slot, jerky_slot]))
    merged = client.collect()
    assert host.rows(f'SELECT slot, stack_count FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5090052') == [[jerky_slot, 4]]
    assert not any(opcode in (0x166, 0x19a) for opcode, _ in split + merged)


def check_group_loot(host):
    client, peer = native_and_peer(host, at=(0, 0, 0, 0))
    guid, peer_guid = client.guid, peer.guid
    host.restore('game_creature_loot', 'id = 5090901')
    host.restore('game_creature_template', 'entry = 51000', 'money_min', 'money_max', 'level', 'max_level')
    host.sql('DELETE FROM game_creature_loot WHERE creature_entry = 51000')
    host.sql('INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090901,51000,5090050,10000,1,0,false)')
    host.sql('UPDATE game_creature_template SET money_min = 17, money_max = 17, level = 1, max_level = 1 WHERE entry = 51000')
    host.call('debug_stage_meeting_stone_group', [guid, [peer_guid], False])
    corpse = spawn(host, guid, 51000, 3)
    client.collect()
    peer.collect()
    host.call('debug_apply_damage', [corpse, 1, guid])
    host.call('debug_kill_creature', [guid, corpse])
    rolls = client.collect()
    assert any(opcode == 0x2a1 and body[:16] == struct.pack('<QII', corpse, 0, 5090050) for opcode, body in rolls)
    assert any(opcode == 0x2a1 for opcode, _ in peer.collect())
    assert host.rows(f'SELECT withheld FROM game_corpse_loot WHERE corpse_guid = {corpse}') == [[True]]
    client.send(0x2a0, struct.pack('<QIB', corpse, 0, 1))
    assert any(opcode == 0x2a2 for opcode, _ in client.collect())
    peer.send(0x2a0, struct.pack('<QIB', corpse, 0, 0))
    awarded = client.collect()
    for _ in range(50):
        if any(opcode == 0x29f for opcode, _ in awarded):
            break
        time.sleep(0.02)
        awarded += client.collect()
    assert any(opcode == 0x29f for opcode, _ in awarded)
    assert any(opcode == 0x166 for opcode, _ in awarded)
    peer_awarded = peer.collect()
    for _ in range(50):
        if any(opcode == 0x29f for opcode, _ in peer_awarded):
            break
        time.sleep(0.02)
        peer_awarded += peer.collect()
    assert any(opcode == 0x29f for opcode, _ in peer_awarded), peer_awarded
    assert not host.rows(f'SELECT id FROM game_loot_roll WHERE corpse_guid = {corpse}')
    assert not host.rows(f'SELECT id FROM game_corpse_loot WHERE corpse_guid = {corpse}')
    client.send(0x15d, struct.pack('<Q', corpse))
    client.collect()
    peer.send(0x15d, struct.pack('<Q', corpse))
    peer.collect()
    purse = host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]
    peer_purse = host.rows(f'SELECT money FROM game_world_entity WHERE guid = {peer_guid}')[0][0]
    client.send(0x15e)
    assert (0x163, struct.pack('<I', 9)) in client.collect()
    shared = peer.collect()
    assert (0x163, struct.pack('<I', 8)) in shared
    assert (0x165, b'') in shared
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}') == [[purse + 9]]
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {peer_guid}') == [[peer_purse + 8]]


def check_aoi(host):
    client, peer = native_and_peer(host, at=(0, 0, 0, 0))
    guid, peer_guid = client.guid, peer.guid
    client.send(0xee, struct.pack('<IIffffI', 0, 1000, 2, 0, 0, 0, 0))
    for _ in range(100):
        if host.rows(f'SELECT guid FROM game_entity_motion WHERE guid = {guid}'):
            break
        time.sleep(0.02)
    motion = peer.collect()
    assert any(opcode == 0xee and packed_guid(body) == guid for opcode, body in motion)
    assert not any(opcode == 0xee for opcode, _ in client.collect())
    host.call('debug_teleport', [guid, 0, 150, 0, 0, 0])
    assert destroys(peer.collect()).count(guid) == 1
    assert destroys(client.collect()).count(peer_guid) == 1
    host.call('debug_teleport', [guid, 0, 0, 0, 0, 0])
    assert creates(peer.collect()).count(guid) == 1
    assert creates(client.collect()).count(peer_guid) == 1
    host.call('debug_force_cast', [guid, 1784])
    assert destroys(peer.collect()).count(guid) == 1
    host.call('debug_apply_damage', [guid, 1, 0])
    assert creates(peer.collect()).count(guid) == 1


def check_auras(host):
    client, peer = native_and_peer(host, at=(0, 0, 0, 0))
    peer_guid = peer.guid
    host.call('debug_learn_spell', [peer_guid, 168])
    host.restore('game_spell', 'spell_id = 168', 'duration_ms', 'gcd_ms')
    host.sql('UPDATE game_spell SET duration_ms = 6000, gcd_ms = 0 WHERE spell_id = 168')
    peer.send(0x12e, cast(168))
    buff = peer.collect()
    observed = client.collect()
    assert [opcode for opcode, _ in buff][:3] == [0x131, 0x130, 0x132]
    assert sum(opcode == 0x132 and spell_id(body) == 168 for opcode, body in buff) == 1
    assert sum(opcode == 0x132 and spell_id(body) == 168 for opcode, body in observed) == 1
    assert any(opcode == 0x137 and 5900 <= struct.unpack_from('<I', body, 1)[0] <= 6000 for opcode, body in buff)
    assert not any(opcode == 0x137 for opcode, _ in observed)
    assert host.rows(f'SELECT id FROM game_aura WHERE target_guid = {peer_guid} AND spell_id = 168')
    peer.send(0x136, struct.pack('<I', 168))
    cancelled = peer.collect()
    assert values(cancelled, peer_guid).get(47) == 0, 'cancelling the buff clears UNIT_FIELD_AURA slot 0'
    assert not host.rows(f'SELECT id FROM game_aura WHERE target_guid = {peer_guid} AND spell_id = 168')
    client.collect()


def check_melee(host):
    client, peer = native_and_peer(host, at=(0, 0, 0, 0))
    guid = client.guid
    wolf = spawn(host, guid, 51000, 40)
    assert wolf in creates(client.collect())
    assert wolf in creates(peer.collect())
    client.send(0x141, struct.pack('<Q', guid))
    assert not any(opcode == 0x143 for opcode, _ in client.collect())
    assert not host.rows(f'SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}')
    client.send(0x141, struct.pack('<Q', wolf))
    attack = client.collect()
    observed = peer.collect()
    assert (0x143, struct.pack('<QQ', guid, wolf)) in attack
    assert (0x143, struct.pack('<QQ', guid, wolf)) in observed
    for _ in range(60):
        if any(opcode == 0x145 for opcode, _ in attack):
            break
        time.sleep(0.05)
        attack += client.collect()
    assert any(opcode == 0x145 for opcode, _ in attack)
    assert not any(opcode == 0x145 for opcode, _ in observed + peer.collect())
    client.send(0x141, struct.pack('<Q', wolf))
    assert (0x143, struct.pack('<QQ', guid, wolf)) in client.collect()
    assert not any(opcode == 0x143 for opcode, _ in peer.collect())
    client.send(0x142)
    assert any(opcode == 0x144 for opcode, _ in client.collect())
    assert any(opcode == 0x144 for opcode, _ in peer.collect())
    assert not host.rows(f'SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}')
    host.call('debug_teleport', [guid, 0, 38, 0, 0, 0])
    client.collect()
    peer.collect()
    client.send(0x141, struct.pack('<Q', wolf))
    attack = []
    for _ in range(60):
        attack += client.collect()
        if any(opcode == 0x14a and packed_guid(body[4:]) == guid for opcode, body in attack):
            break
        time.sleep(0.05)
    assert any(opcode == 0x14a and packed_guid(body[4:]) == guid for opcode, body in attack)
    assert any(opcode == 0x14a and packed_guid(body[4:]) == guid for opcode, body in peer.collect())
    host.call('debug_set_health', [wolf, 0])
    client.collect()
    peer.collect()
    client.send(0x141, struct.pack('<Q', wolf))
    assert (0x148, b'') in client.collect()
    host.call('debug_teleport', [guid, 0, 0, 0, 0, 0])
    client.collect()
    peer.collect()


def check_casting(host):
    client, peer = native_and_peer(host, at=(0, 0, 0, 0))
    guid, peer_guid = client.guid, peer.guid
    host.call('debug_learn_spell', [peer_guid, 133])
    host.restore('game_spell', 'spell_id = 133', 'cast_time_ms', 'gcd_ms', 'cooldown_ms', 'cost')
    host.sql('UPDATE game_spell SET cast_time_ms = 1500, gcd_ms = 0, cooldown_ms = 0, cost = 0 WHERE spell_id = 133')
    spell_target = spawn(host, guid, 51000, 35)
    peer.collect()
    client.collect()
    peer.send(0x12e, cast(133, spell_target))
    started = peer.collect()
    assert any(opcode == 0x131 and spell_id(body) == 133 for opcode, body in started), started
    assert not any(opcode == 0x132 for opcode, _ in started)
    assert any(opcode == 0x131 and spell_id(body) == 133 for opcode, body in client.collect())
    failed_other = (0x2a6, struct.pack('<QI', peer_guid, 133))
    interrupted = [failed_other, (0x130, struct.pack('<IBB', 133, 2, 0x23))]
    peer.send(0x12f, struct.pack('<I', 133))
    cancelled = peer.collect()
    assert [packet for packet in cancelled if packet[0] in (0x130, 0x133, 0x2a6)] == interrupted, cancelled
    observed = client.collect()
    assert failed_other in observed
    assert not any(opcode in (0x130, 0x133) for opcode, _ in observed)
    time.sleep(1.6)
    assert not any(opcode == 0x132 for opcode, _ in peer.collect())
    assert not host.rows(f'SELECT * FROM game_pending_cast WHERE caster_guid = {peer_guid}')
    client.collect()
    peer.send(0x12e, cast(133, spell_target))
    assert any(opcode == 0x131 and spell_id(body) == 133 for opcode, body in peer.collect())
    client.collect()
    x, y, z, orientation, timestamp = host.rows(f'SELECT x,y,z,orientation,last_move_ms FROM game_world_entity WHERE guid = {peer_guid}')[0]
    peer.send(0xee, struct.pack('<IIffffI', 1, (timestamp + 500) & 0xffffffff, x + 2, y, z, orientation, 0))
    moved = peer.collect()
    assert [packet for packet in moved if packet[0] in (0x130, 0x133, 0x2a6)] == interrupted, moved
    assert failed_other in client.collect()
    assert not host.rows(f'SELECT * FROM game_pending_cast WHERE caster_guid = {peer_guid}')
    peer.send(0xee, struct.pack('<IIffffI', 0, (timestamp + 1000) & 0xffffffff, x, y, z, orientation, 0))
    peer.collect()
    client.collect()
    host.sql('UPDATE game_spell SET cost = 10 WHERE spell_id = 133')
    host.sql(f'UPDATE game_world_entity SET power = 10 WHERE guid = {peer_guid}')
    peer.send(0x12e, cast(133, spell_target))
    started = peer.collect()
    assert any(opcode == 0x131 and spell_id(body) == 133 for opcode, body in started), started
    host.sql(f'UPDATE game_world_entity SET power = 0 WHERE guid = {peer_guid}')
    refused, deadline = [], time.monotonic() + 5
    while time.monotonic() < deadline:
        refused += [packet for packet in peer.collect() if packet[0] in (0x130, 0x132, 0x133, 0x2a6)]
        if len(refused) >= len(interrupted):
            break
        time.sleep(0.05)
    assert refused == interrupted, refused
    assert not host.rows(f'SELECT * FROM game_pending_cast WHERE caster_guid = {peer_guid}')
    host.sql('UPDATE game_spell SET cost = 0 WHERE spell_id = 133')
    client.collect()
    for _ in range(10):
        peer.send(0x12e, cast(133, spell_target))
        completed = []
        for _ in range(100):
            completed += peer.collect()
            missed = [miss for opcode, body in completed
                      if opcode == 0x132 and spell_id(body) == 133
                      for miss in spell_go_misses(body)]
            if missed or any(opcode == 0x250 for opcode, _ in completed):
                break
            time.sleep(0.05)
        if not missed:
            break
        assert missed == [(spell_target, 2)], missed
        client.collect()
    order = [opcode for opcode, body in completed if opcode in (0x130, 0x131, 0x132, 0x250)]
    assert order == [0x131, 0x130, 0x132, 0x250], order
    observed = client.collect()
    assert sum(opcode == 0x132 and spell_id(body) == 133 for opcode, body in observed) == 1
    assert any(opcode == 0x250 for opcode, _ in observed)
    assert not any(opcode == 0x130 for opcode, _ in observed)
    host.call('debug_set_health', [spell_target, 0])
    peer.collect()
    client.collect()


def check_combo_points(host):
    rogue = host.enter_world('Rogue', cls=4, at=(0, 0, 0, 0))
    observer = host.enter_world('Observer', at=(0, 10, 0, 0))
    guid = rogue.guid
    host.call('debug_learn_spell', [guid, 133])
    host.restore('game_spell', 'spell_id = 133', 'gcd_ms', 'cast_flags')
    host.restore('game_spell_effect', 'spell_id = 133', 'kind')
    host.sql('UPDATE game_spell SET gcd_ms = 0 WHERE spell_id = 133')
    first = spawn(host, guid, 51000, 5)
    second = spawn(host, guid, 51000, 6)
    rogue.collect()
    observer.collect()
    held = f'SELECT target_guid, points FROM game_combo_point WHERE owner_guid = {guid}'

    def effect(kind):
        host.sql(f'UPDATE game_spell_effect SET kind = {kind} WHERE spell_id = 133')
        finisher = 0x08 if kind == 0x0d else 0  # SPELL_ATTR_COMBO_FINISHER.
        host.sql(f'UPDATE game_spell SET cast_flags = {finisher} WHERE spell_id = 133')

    def shown(packets=()):
        seen = list(packets)
        def combo():
            seen.extend(rogue.collect())
            fields = values(seen, guid)
            return fields if 1222 in fields else None
        fields = host.wait_until(combo, 'no PLAYER_FIELD_BYTES update reached the rogue')
        return fields[714] | fields[715] << 32, fields[1222] >> 8 & 0xff

    effect(0x0c)  # E_ADD_COMBO.
    for points in (1, 2):
        rogue.send(0x12e, cast(133, first))
        assert shown() == (first, points)
    assert host.rows(held) == [[first, 2]]
    assert 1222 not in values(observer.collect(), guid)

    effect(0x0d)  # E_FINISHER_DAMAGE.
    rogue.send(0x12e, cast(133, second))
    refused = []
    host.wait_until(lambda: refused.extend(rogue.collect()) or any(opcode == 0x130 for opcode, _ in refused),
                    'the finisher on a target without combo points was not refused')
    assert (0x130, struct.pack('<IBB', 133, 2, 0x46)) in refused, refused
    assert host.rows(held) == [[first, 2]]
    for _ in range(10):
        rogue.send(0x12e, cast(133, first))
        packets = []
        def finished():
            packets.extend(rogue.collect())
            return (1222 in values(packets, guid)
                    or any(opcode == 0x24b for opcode, _ in packets))
        host.wait_until(finished, 'the finisher neither spent its points nor reported a miss')
        # Client-initiated casts report avoidance through SPELLLOGMISS after their launch GO.
        missed = [struct.unpack('<IQBIQB', body) for opcode, body in packets if opcode == 0x24b]
        if not missed:
            assert shown(packets) == (first, 0)
            break
        assert len(missed) == 1 and missed[0][:5] == (133, guid, 0, 1, first), missed
        assert missed[0][5] in (1, 3, 4), missed
        assert host.rows(held) == [[first, 2]], 'an avoided finisher consumed combo points'
    else:
        raise AssertionError('all finisher casts were avoided')
    assert not host.rows(held)

    effect(0x0c)
    rogue.send(0x12e, cast(133, second))
    assert shown() == (second, 1)
    host.call('debug_set_health', [second, 0])
    assert shown() == (second, 0)
    assert not host.rows(held)
    host.call('debug_set_health', [first, 0])
    observer.collect()


def check_progression(host):
    client, peer = native_and_peer(host, at=(0, 0, 0, 0))
    guid, peer_guid = client.guid, peer.guid
    host.call('debug_set_level', [guid, 2])
    host.call('debug_set_money', [guid, 50000])
    packed = host.rows(f'SELECT player_bytes_2 FROM game_world_entity WHERE guid = {guid}')[0][0]
    host.sql(f'UPDATE game_world_entity SET bank_bag_slots = 1, player_bytes_2 = {packed & 0xff00ffff | 1 << 16} WHERE guid = {guid}')
    trainer = spawn(host, guid, 51001, 2)
    spell_target = spawn(host, guid, 51000, 35)
    client.collect()
    peer.collect()
    host.sql("INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES (5090901,0,0,1000,0,1,0,'Native exploration')")
    terrain_chunk(host, 512, 512, 5090901)
    host.call('debug_explore_at', [guid, 0, -1, -1])
    explored = client.collect()
    assert (0x1f8, struct.pack('<II', 5090901, 50)) in explored
    assert (0x1d0, struct.pack('<QIB', 0, 50, 1)) in explored
    assert values(explored, guid)[1142] == 256
    host.call('debug_explore_at', [guid, 0, -1, -1])
    assert not any(opcode in (0x1f8, 0x1d0) for opcode, _ in client.collect())
    assert not any(opcode in (0x1f8, 0x1d0) for opcode, _ in peer.collect())
    host.restore('game_area_trigger', 'id = 5090902')
    host.restore('game_tavern_area_trigger', 'trigger_id = 5090902')
    host.sql('INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) VALUES (5090902,0,-9464,42,56,20,0,0,0,0)')
    host.sql('INSERT INTO game_tavern_area_trigger (trigger_id) VALUES (5090902)')
    inn_z = host.rows('SELECT z FROM game_area_trigger WHERE id = 5090902')[0][0]
    host.sql(f'UPDATE game_world_entity SET z = {inn_z} WHERE guid = {guid}')
    rest_bytes = host.rows(f'SELECT player_bytes_2 FROM game_world_entity WHERE guid = {guid}')[0][0]
    host.call('debug_check_rest_at', [guid, 0, -9464, 42])
    rested = values(client.collect(), guid)
    assert rested[190] & 0x20 and 194 not in rested
    assert host.rows(f'SELECT player_bytes_2 FROM game_world_entity WHERE guid = {guid}') == [[rest_bytes]]
    host.call('debug_check_rest_at', [guid, 0, 0, 0])
    unrested = values(client.collect(), guid)
    assert unrested[190] & 0x20 == 0 and 194 not in unrested
    assert host.rows(f'SELECT player_bytes_2 FROM game_world_entity WHERE guid = {guid}') == [[rest_bytes]]
    host.sql(f'UPDATE game_world_entity SET z = 0 WHERE guid = {guid}')
    peer.collect()
    host.sql('INSERT INTO game_faction (faction_id,reputation_index,base_standing) VALUES (5090901,62,0)')
    host.call('debug_grant_reputation', [guid, 5090901, 3175])
    assert (0x124, struct.pack('<IIi', 1, 62, 3175)) in client.collect()
    host.call('debug_grant_reputation', [guid, 5090901, 25])
    assert (0x124, struct.pack('<IIi', 1, 62, 3200)) in client.collect()
    client.send(0x125, struct.pack('<IB', 62, 1))
    client.collect()
    assert host.rows(f'SELECT at_war FROM game_player_reputation WHERE character_guid = {guid} AND faction_id = 5090901') == [[True]]
    client.send(0x318, struct.pack('<i', 62))
    assert values(client.collect(), guid)[1261] == 62
    client.send(0x318, struct.pack('<i', 64))
    refused = client.collect()
    assert any(opcode == 0x96 for opcode, _ in refused)
    assert 1261 not in values(refused, guid)
    assert host.rows(f'SELECT watched_faction_index FROM game_character WHERE guid = {guid}') == [[62]]
    client.send(0x125, struct.pack('<IB', 0xffffffff, 1))
    assert any(opcode == 0x96 for opcode, _ in client.collect())
    assert not any(opcode in (0x124, 0x96) for opcode, _ in peer.collect())
    host.call('debug_set_level', [guid, 10])
    host.sql("INSERT INTO game_talent_tab (tab_id,name,class_mask,race_mask,order_index) VALUES (5090901,'Native talent',1,0,0)")
    host.sql("INSERT INTO game_talent (talent_id,name,tree_id,tier,column,max_rank,spell_id,required_talent_id,required_points_in_tree,grant_spell_id,tab_id,rank_spell_2,rank_spell_3,rank_spell_4,rank_spell_5,required_talent_rank,required_spell_id) VALUES (124,'Native talent',0,0,0,1,51990,0,0,0,5090901,0,0,0,0,0,0)")
    host.sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges,max_duration_ms) VALUES (51990,'Native passive',0,0,0,0,0,0,4294967295,1,0,0,1,0,64,0,0,false,0,0,0,0,0,0,0,0)")
    for index, kind, points, param, mask in [(0, 160, 5, 0, 0), (1, 172, -5, 10, 1)]:
        host.sql(f'INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat,points_per_combo) VALUES ({207960 + index},51990,{index},{kind},{points},0,0.0,0,0,0.0,0,0,0,{param},0,{mask},0,false,0.0)')
    client.collect()
    peer.collect()
    client.send(0x251, struct.pack('<II', 124, 0))
    learned = client.collect()
    assert (0x12b, struct.pack('<I', 51990)) in learned
    assert (0x266, struct.pack('<BBi', 0, 10, -5)) in learned
    assert values(learned, guid)[1102] == 0
    assert host.rows(f'SELECT sheet_str_bonus FROM game_world_entity WHERE guid = {guid}') == [[5]]
    client.send(0x251, struct.pack('<II', 124, 1))
    assert not any(opcode in (0x12b, 0x266) for opcode, _ in client.collect())
    assert host.rows(f'SELECT rank FROM game_character_talent WHERE character_guid = {guid}') == [[1]]
    before_reset = host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]
    client.send(0x2aa, struct.pack('<Q', trainer))
    reset = client.collect()
    assert (0x17e, b'') in reset
    assert (0x203, struct.pack('<H', 51990)) in reset
    assert (0x266, struct.pack('<BBi', 0, 10, 0)) in reset
    assert values(reset, guid)[1102] == 1
    assert host.rows(f'SELECT sheet_str_bonus, money FROM game_world_entity WHERE guid = {guid}') == [[0, before_reset - 10000]]
    assert not host.rows(f'SELECT rank FROM game_character_talent WHERE character_guid = {guid}')
    client.send(0x251, struct.pack('<II', 124, 0))
    assert values(client.collect(), guid)[1102] == 0
    assert not any(opcode in (0x12b, 0x203, 0x266) for opcode, _ in peer.collect())
    host.call('debug_teleport', [guid, 0, 12, 0, 0, 0])
    first_teleport = next(body for opcode, body in client.collect(auto_ack=False) if opcode == 0xc7)
    host.call('debug_teleport', [guid, 0, 16, 0, 0, 0])
    second_teleport = next(body for opcode, body in client.collect(auto_ack=False) if opcode == 0xc7)
    assert teleport_ack(first_teleport) != teleport_ack(second_teleport)
    client.send(0xc7, teleport_ack(first_teleport))
    client.send(0xdc)
    client.send(0xee, struct.pack('<IIffffI', 0, 10001, 999, 0, 0, 0, 0))
    client.send(0x141, struct.pack('<Q', spell_target))
    client.collect(auto_ack=False)
    assert host.rows(f'SELECT x FROM game_world_entity WHERE guid = {guid}') == [[16.0]]
    assert not host.rows(f'SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {guid}')
    wrong_mover = bytearray(teleport_ack(second_teleport))
    wrong_mover[1] ^= 1
    client.send(0xc7, bytes(wrong_mover))
    client.send(0xee, struct.pack('<IIffffI', 0, 10002, 999, 0, 0, 0, 0))
    client.collect(auto_ack=False)
    assert host.rows(f'SELECT x FROM game_world_entity WHERE guid = {guid}') == [[16.0]]
    peer.collect()
    client.send(0xc7, teleport_ack(second_teleport))
    client.collect()
    arrived = peer.collect()
    assert any(opcode == 0xc5 and packed_guid(body) == guid
               and struct.unpack_from('<f', body, 9 + body[0].bit_count())[0] == 16
               for opcode, body in arrived)
    client.send(0xee, struct.pack('<IIffffI', 0, 10003, 17, 0, 0, 0, 0))
    client.collect()
    assert host.rows(f'SELECT x FROM game_world_entity WHERE guid = {guid}') == [[17.0]]
    peer.collect()
    host.call('debug_teleport', [guid, 1, 4, 5, 6, 0.75])
    departing = client.collect(auto_ack=False)
    assert [opcode for opcode, _ in departing] == [0x3f, 0x3e]
    assert departing[1][1] == struct.pack('<Iffff', 1, 4, 5, 6, 0.75)
    assert destroys(peer.collect()).count(guid) == 1
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')
    client.send(0xee, struct.pack('<IIffffI', 0, 10004, 999, 0, 0, 0, 0))
    client.send(0xc7, teleport_ack(second_teleport))
    client.collect(auto_ack=False)
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')
    client.send(0xdc)
    ported = client.collect()
    assert creates(ported).count(guid) == 1
    assert peer_guid not in creates(ported)
    assert not any(opcode == 0x236 for opcode, _ in ported)
    assert values(ported, guid)[1142] == 256
    assert host.rows(f'SELECT map_id, x, y, z, orientation FROM game_world_entity WHERE guid = {guid}') == [[1, 4.0, 5.0, 6.0, 0.75]]
    client.send(0xdc)
    assert guid not in creates(client.collect())
    host.call('debug_teleport', [guid, 0, 18, 0, 0, 0])
    returned = client.collect()
    assert creates(returned).index(guid) < creates(returned).index(peer_guid)
    assert creates(peer.collect()).count(guid) == 1
    host.call('debug_teleport', [guid, 1, 20, 0, 0, 0])
    client.collect(auto_ack=False)
    client.close()
    for _ in range(100):
        if host.rows(f'SELECT online FROM game_character WHERE guid = {guid}') == [[False]]:
            break
        time.sleep(0.02)
    assert host.rows(f'SELECT online, map_id, x FROM game_character WHERE guid = {guid}') == [[False, 1, 20.0]]
    client = host.connect()
    assert client.read()[0] == 0x1ee
    client.send(0x3d, struct.pack('<Q', guid))
    assert creates(client.collect()).count(guid) == 1
    assert host.rows(f'SELECT map_id, x FROM game_world_entity WHERE guid = {guid}') == [[1, 20.0]]
    host.call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    client.collect()
    peer.collect()
    host.call('debug_teleport', [guid, 1, 24, 0, 0, 0])
    client.collect(auto_ack=False)
    client.send(0x4b)
    loading_logout = client.collect(auto_ack=False)
    assert [opcode for opcode, _ in loading_logout] == [0x4c, 0x4d]
    assert host.rows(f'SELECT online, map_id, x FROM game_character WHERE guid = {guid}') == [[False, 1, 24.0]]
    client.send(0x3d, struct.pack('<Q', guid))
    client.collect()
    host.call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    client.collect()
    peer.collect()
    def connections():
        return sorted(json.dumps(row) for row in host.rows('SELECT connection_id FROM game_native_world_connection'))
    before_replacement = connections()
    replacement = host.connect()
    assert replacement.read()[0] == 0x1ee
    replacement.send(0x3d, struct.pack('<Q', guid))
    resumed = replacement.collect()
    assert creates(resumed).count(guid) == 1
    assert values(resumed, guid)[1142] == 256
    assert values(resumed, guid)[1102] == 0
    assert (0x266, struct.pack('<BBi', 0, 10, -5)) in resumed
    assert not any(opcode == 0x1f8 for opcode, _ in resumed)
    while True:
        try:
            client.read()
        except EOFError:
            break
    client.close()
    host.wait_until(
        lambda: (now := connections()) != before_replacement and len(now) == len(before_replacement),
        'the replaced World Session must close its connection')
    assert host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')
    replacement.send(0x4b)
    replacement.send(0xee, struct.pack('<IIffffI', 0, 3000, 12, 0, 0, 0, 0))
    replacement.send(0x13d, struct.pack('<Q', peer_guid))
    replacement.send(0x50, struct.pack('<Q', peer_guid))
    replacement.send(0x125, struct.pack('<IB', 0, 1))
    replacement.send(0x318, struct.pack('<i', -1))
    replacement.send(0x258, struct.pack('<QI', 0, 0) + b'\0' + struct.pack('<BBIIIIB', 0, 0, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0))
    logout = replacement.collect()
    assert [opcode for opcode, _ in logout][-2:] == [0x4c, 0x4d]
    replacement.send(0x37)
    assert any(opcode == 0x3b for opcode, _ in replacement.collect())
    replacement.send(0x2c7, struct.pack('<Q', guid) + b'Renamed\0')
    assert replacement.read() == (0x2c8, b'\x2f')
    assert host.rows(f'SELECT name FROM game_character WHERE guid = {guid}') == [['Native']]
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')
    assert guid in destroys(peer.collect())
    peer.send(0xee, struct.pack('<IIffffI', 0, 2000, 11, 0, 0, 0, 0))
    peer.collect()
    assert not any(opcode == 0xee for opcode, _ in replacement.collect())
    replacement.send(0x38, struct.pack('<Q', peer_guid))
    assert replacement.read() == (0x3c, bytes([0x3a]))
    replacement.send(0x38, struct.pack('<Q', guid))
    assert replacement.read() == (0x3c, bytes([0x39]))
    assert not host.rows(f'SELECT guid FROM game_character WHERE guid = {guid}')
    replacement.close()
    peer.close()
    host.await_empty('game_native_member_stats')
    host.await_empty('game_native_player')
    host.await_empty('game_native_world_connection')
    host.await_empty('game_native_cipher')
    host.await_empty('game_native_logon_connection')


def check_taxi(host):
    client, peer = native_and_peer(host, at=(0, 0, 0, 0))
    guid, peer_guid = client.guid, peer.guid
    host.call('debug_set_money', [guid, 1000])
    flight_master = spawn(host, guid, 51004, 2)
    host.sql(f'UPDATE game_world_entity SET npc_flags = 9 WHERE guid = {flight_master}')
    host.sql(f'UPDATE game_world_entity SET movement_flags = 0, unit_flags = 0 WHERE guid = {guid}')
    for node, client_node, x in [(5090301, 253, 0), (5090302, 254, 64)]:
        host.sql(f"INSERT INTO game_taxi_node (id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance) VALUES ({node},{client_node},0,{x},0,0,'Native flight',295,1147)")
    host.sql('INSERT INTO game_taxi_path (id,source_node_id,destination_node_id,fare) VALUES (5090303,5090301,5090302,25)')
    for index, x in [(0, 0), (1, 64)]:
        host.sql(f'INSERT INTO game_taxi_path_node (id,path_id,node_index,map_id,x,y,z,flags,delay_ms) VALUES ({5090304 + index},5090303,{index},0,{x},0,0,0,0)')
    client.collect()
    peer.collect()
    client.send(0x1aa, struct.pack('<Q', flight_master))
    assert (0x1ab, struct.pack('<QB', flight_master, 0)) in client.collect()
    client.send(0x1ac, struct.pack('<Q', flight_master))
    taxi_menu = next(body for opcode, body in client.collect() if opcode == 0x1a9)
    assert taxi_menu[:16] == struct.pack('<IQI', 1, flight_master, 253)
    client.send(0x1aa, struct.pack('<Q', flight_master))
    assert (0x1ab, struct.pack('<QB', flight_master, 1)) in client.collect()
    money_before = host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]
    client.send(0x1ad, struct.pack('<QII', flight_master, 253, 254))
    assert (0x1ae, struct.pack('<I', 6)) in client.collect()
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}') == [[money_before]]
    host.sql(f'INSERT INTO game_character_taxi_node (id,character_guid,node_id) VALUES (0,{guid},5090302)')
    client.send(0x1ad, struct.pack('<QII', flight_master, 253, 254))
    takeoff = client.collect()
    assert (0x1ae, bytes(4)) in takeoff
    assert next(index for index, (opcode, _) in enumerate(takeoff) if opcode == 0x1ae) < next(index for index, (opcode, _) in enumerate(takeoff) if opcode == 0xdd)
    assert values(takeoff, guid)[133] == 1147
    assert any(opcode == 0xdd and packed_guid(body) == guid for opcode, body in peer.collect())
    assert host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}') == [[money_before - 25]]
    flying_message = b'Whisper while flying\0'
    client.send(0x95, struct.pack('<II', 6, 7) + b'Peer\0' + flying_message)
    flight_chat = client.collect()
    assert (0x96, struct.pack('<BIQI', 7, 0, peer_guid, len(flying_message)) + flying_message + b'\0') in flight_chat
    assert (0x96, struct.pack('<BIQI', 6, 0, guid, len(flying_message)) + flying_message + b'\0') in peer.collect()
    client.send(0x95, struct.pack('<II', 0, 7) + b'Speech while flying\0')
    flight_speech = client.collect()
    assert not any(opcode == 0x96 for opcode, _ in flight_speech)
    assert not any(opcode == 0x96 for opcode, _ in peer.collect())
    client.send(0x1ad, struct.pack('<QII', flight_master, 253, 254))
    busy = client.collect()
    assert (0x1ae, struct.pack('<I', 7)) in busy
    client.send(0xee, struct.pack('<IIffffI', 0, 9999, 10000, 0, 0, 0, 0))
    suppressed = client.collect()
    assert host.rows(f'SELECT x FROM game_world_entity WHERE guid = {guid}')[0][0] < 100
    landed = takeoff + flight_chat + flight_speech + busy + suppressed
    for _ in range(80):
        landed += client.collect()
        if not host.rows(f'SELECT character_guid FROM game_active_taxi_flight WHERE character_guid = {guid}'):
            break
        time.sleep(0.05)
    landed += client.collect()
    assert host.rows(f'SELECT x, mount_display_id FROM game_world_entity WHERE guid = {guid}') == [[64.0, 0]]
    assert values(landed, guid)[133] == 0
    assert not host.rows(f'SELECT character_guid FROM game_taxi_passenger_spline WHERE character_guid = {guid}')
    assert not host.rows(f'SELECT request_id FROM game_taxi_service_reply WHERE character_guid = {guid}')
    peer.collect()
    client.collect()


def check_logon_limits(host):
    host.account('LIMITED', password='PASSWORD')
    limited = Logon(host.logon_port)
    for attempt in range(3):
        assert limited.challenge('LIMITED') == 0
        assert limited.prove('WRONG') == (5, None)
    assert limited.socket.recv(1) == b''
    limited.close()
    host.await_empty('game_native_logon_connection')
    connections = [Logon(host.logon_port) for _ in range(8)]
    for _ in range(100):
        if len(host.rows('SELECT * FROM game_native_logon_connection')) == 8:
            break
        time.sleep(0.02)
    assert len(host.rows('SELECT * FROM game_native_logon_connection')) == 8
    excess = Logon(host.logon_port)
    assert excess.socket.recv(1) == b''
    excess.close()
    for connection in connections:
        connection.close()
    host.await_empty('game_native_logon_connection')
    assert all(row[0] == 0 for row in host.rows('SELECT active FROM game_native_logon_address'))


def named(*checks):
    return {check.__name__.removeprefix('check_').replace('_', '-'): check for check in checks}


# These checks share one seeded Host and run in this order, with reset() after each.
SHARED = named(
    check_operator_reducers, check_operator_items, check_operator_creatures, check_logon, check_character_create, check_queries, check_visibility,
    check_inventory, check_creature_catalogue, check_pet_level_scaling, check_spell_source_behavior, check_gossip_submenus, check_enchantments, check_quest_chain, check_quest_items, check_group_loot, check_aoi, check_auras, check_melee,
    check_casting, check_progression, check_taxi, check_death, check_environment, check_fatigue, check_social,
    check_channels, check_party,
    check_member_stats, check_guild, check_petitions, check_trade, check_duels, check_meeting_stones, check_mail,
    check_auctions, check_pets, check_areatriggers, check_world_objects, check_transports, check_transport_crossings, check_addon, check_chat, check_eventai_speech,
    check_sessionless_movement, check_charge_movement, check_logon_limits, check_decoder_bounds,
    check_session_expiry, check_gm_commands, check_who_bounds, check_session_revocation, check_combo_points,
) | {'admission': admission.run}

# Each of these checks gets its own Host, built from these options.
ALONE = {
    'reload': (reload.run, {'dev': True, 'seeded': True}),
    'retention': (retention.check, {'dev': True, 'seeded': True, 'config': retention.CONFIG}),
    'admission-wait': (lambda host: admission.run(host, idle_seconds=92), {'seeded': True}),
    'recovery': (check_recovery, {'seeded': True}),
    'slow-readers': (check_slow_readers, {'seeded': True}),
    'accept-recovery': (check_accept_recovery, {}),
}


def run(host, checks):
    """Runs the checks on one Host and returns the names of those that failed or did not run."""
    failed = []
    for index, (name, check) in enumerate(checks):
        started = time.monotonic()
        log_start = len(host.log_text())
        try:
            check(host)
        except Exception:
            traceback.print_exc()
            print('\n'.join(host.log_text()[log_start:].splitlines()[-100:]))
            print(f'FAIL {name}', flush=True)
            failed.append(name)
        else:
            print(f'PASS {name} ({time.monotonic() - started:.1f}s)', flush=True)
        try:
            host.reset()
        except Exception:
            traceback.print_exc()
            remaining = [name for name, _ in checks[index + 1:]]
            print(f'FAIL reset after {name}; not run: {", ".join(remaining) or "none"}', flush=True)
            return failed + ([] if name in failed else [name]) + remaining
    return failed


def run_on_new_host(binary, module, options, checks):
    try:
        with Host(binary, module, **options) as host:
            return run(host, checks)
    except Exception:
        traceback.print_exc()
        print(f'FAIL Host start; not run: {", ".join(name for name, _ in checks)}', flush=True)
        return [name for name, _ in checks]


def main():
    parser = argparse.ArgumentParser(description='Runs native checks against a Disposable Host. '
        'Without CHECK names it runs every shared check.')
    parser.add_argument('--host', type=Path, help='spacetimedb-standalone built by prepare-host.py')
    parser.add_argument('--module', type=Path, default=MODULE)
    parser.add_argument('--list', action='store_true', help='print the check names and exit')
    parser.add_argument('checks', nargs='*', metavar='CHECK')
    args = parser.parse_args()
    if args.list:
        print('Shared:', ' '.join(SHARED))
        print('Own Host:', ' '.join(ALONE))
        return
    if not args.host:
        parser.error('--host is required')
    unknown = [name for name in args.checks if name not in SHARED and name not in ALONE]
    if unknown:
        parser.error(f'unknown checks: {", ".join(unknown)}; see --list')
    names = args.checks or list(SHARED)
    failed = []
    shared = [(name, check) for name, check in SHARED.items() if name in names]
    if shared:
        failed += run_on_new_host(args.host, args.module, {'seeded': True}, shared)
    for name in [name for name in ALONE if name in names]:
        check, options = ALONE[name]
        failed += run_on_new_host(args.host, args.module, options, [(name, check)])
    if failed:
        print(f'FAILED: {", ".join(failed)}')
        sys.exit(1)


if __name__ == '__main__':
    main()
