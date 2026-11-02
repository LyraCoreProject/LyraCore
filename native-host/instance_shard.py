import struct

from headless import creates


def check(host):
    continent = host.add_database(guid_base=1_000_000_000)
    instances = host.add_database(guid_base=2_000_000_000)
    worlds = (continent, instances)
    for shard in worlds:
        host.call('debug_seed_scenario_fixtures', database=shard)
        host.sql('DELETE FROM game_character WHERE guid = 1', shard)
        host.call('operator_add_peer_shard', [host.identity(shard), shard])
        host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=shard)
    host.call('operator_set_shard_role', [{'some': {'realm': host.identity(continent)}}])
    for shard in worlds:
        host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=shard)
        for peer in worlds:
            if peer != shard:
                host.call('operator_add_peer_shard', [host.identity(peer), peer], database=shard)
    for map_id, shard in ((0, continent), (36, instances)):
        for owner in (None, *worlds):
            host.call('operator_set_map_owner', [map_id, host.identity(shard)], database=owner)
    host.call('operator_set_character_start_map', [1, 1, 0])
    entrance, exit_trigger = 5090632, 5090633
    for shard in worlds:
        for trigger, map_id, destination in ((entrance, 0, 36), (exit_trigger, 36, 0)):
            host.sql(f'INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) '
                     f'VALUES ({trigger},{map_id},1000,1000,0,10,0,0,0,0)', shard)
            host.sql(f"INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) "
                     f"VALUES ({trigger},{destination},1000,1000,0,0,'Instance Shard check')", shard)

    def login(client, guid):
        client.send(0x3d, struct.pack('<Q', guid))
        packets = []
        while guid not in creates(packets):
            packets.append(client.read())
        return packets + client.collect()

    def character(name):
        account = host.account(name.upper())
        client = host.connect(name.upper())
        assert client.read()[0] == 0x1ee
        client.send(0x36, name.encode() + b'\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
        assert client.read() == (0x3a, b'\x2e')
        guid = host.rows(f"SELECT guid FROM game_character WHERE name = '{name}'", continent)[0][0]
        host.sql(f'UPDATE game_character SET x = 1000, y = 1000, z = 0 WHERE guid = {guid}', continent)
        login(client, guid)
        host.wait_until(lambda: host.rows(f'SELECT guid FROM game_party_character WHERE guid = {guid}') == [[guid]],
                        'Realm did not receive party presence')
        return account, client, guid

    _, leader, leader_guid = character('Leader')
    _, member, member_guid = character('Member')
    _, solo, solo_guid = character('Solitary')

    def received(client, opcode):
        packets = []
        host.wait_until(lambda: packets.extend(client.collect()) or any(code == opcode for code, _ in packets),
                        f'missing packet {opcode:#x}')
        return [body for code, body in packets if code == opcode]

    leader.send(0x6e, b'Member\0')
    assert received(member, 0x6f) == [b'Leader\0']
    member.send(0x72)
    received(leader, 0x7d)
    received(member, 0x7d)
    group = host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {leader_guid}')[0][0]

    def cross(client, guid, source, target, trigger):
        client.collect()
        inventory = host.rows(f'SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {guid}', source)
        client.send(0xb4, struct.pack('<I', trigger))
        packets = []
        while guid not in creates(packets):
            packet = client.read()
            packets.append(packet)
            if packet[0] == 0x3e:
                client.send(0xdc)
        client.collect()
        host.wait_until(lambda: bool(host.rows(f'SELECT character_guid FROM game_native_player WHERE character_guid = {guid}', target)),
                        'destination did not adopt the Character')
        assert not host.rows(f'SELECT guid FROM game_character WHERE guid = {guid}', source)
        assert sorted(host.rows(f'SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {guid}', target)) == sorted(inventory)
        return host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {guid}', target)[0][0]

    run = cross(leader, leader_guid, continent, instances, entrance)
    assert run != 0
    assert host.rows('SELECT instance_id FROM game_instance', continent) == []
    assert host.rows(f'SELECT map_id, party_id FROM game_instance WHERE instance_id = {run}', instances) == [[36, group]]
    assert host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {leader_guid}', instances) == [[group]]
    assert host.rows(f'SELECT leader_guid FROM game_group WHERE group_id = {group}') == [[leader_guid]]

    host.sql(f'DELETE FROM game_group_member WHERE character_guid = {leader_guid}', continent)
    host.wait_until(lambda: host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {leader_guid}', continent) == [[group]],
                    'periodic presence did not repair the source projection after Transfer', timeout=12)
    leader.send(0x7a, struct.pack('<IQI', 2, leader_guid, 3))
    host.wait_until(lambda: host.rows(f'SELECT loot_method, master_looter_guid, loot_threshold FROM game_group WHERE group_id = {group}', continent) == [[2, leader_guid, 3]],
                    'loot rules did not reach the other World Shard')
    leader.send(0x7a, struct.pack('<IQI', 3, 0, 2))
    host.wait_until(lambda: host.rows(f'SELECT loot_method FROM game_group WHERE group_id = {group}', instances) == [[3]],
                    'Group Loot was not restored')
    member.collect()
    leader.send(0x95, struct.pack('<II', 1, 7) + b'Across the portal\0')
    assert any(b'Across the portal\0' in body for body in received(member, 0x96))
    assert cross(member, member_guid, continent, instances, entrance) == run
    separate = cross(solo, solo_guid, continent, instances, entrance)
    assert separate not in (0, run)
    assert host.rows(f'SELECT guid FROM game_world_entity WHERE map_id = 36 AND instance_id = {run} AND guid = {solo_guid}', instances) == []

    assert cross(member, member_guid, instances, continent, exit_trigger) == 0
    assert host.rows(f'SELECT instance_id FROM game_instance_binding WHERE character_guid = {member_guid} AND map_id = 36', continent) == [[run]]
    assert cross(member, member_guid, continent, instances, entrance) == run

    member.send(0x4b)
    received(member, 0x4d)
    login(member, member_guid)
    assert host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {member_guid}', instances) == [[run]]

    for client in (leader, member):
        client.send(0x4b)
        received(client, 0x4d)
    host.call('debug_reap_instance', [run], database=instances)
    assert host.rows(f'SELECT instance_id FROM game_instance WHERE instance_id = {run}', instances) == []
    login(member, member_guid)
    assert host.rows(f'SELECT map_id, instance_id FROM game_world_entity WHERE guid = {member_guid}', continent) == [[0, 0]]
    assert not host.rows(f'SELECT guid FROM game_character WHERE guid = {member_guid}', instances)
    assert host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {member_guid}') == [[group]]

    inventory_before = host.rows(f'SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {member_guid}', continent)
    host.call('operator_remove_peer_shard', [host.identity()], database=instances)
    member.send(0xb4, struct.pack('<I', entrance))
    received(member, 0x1cb)
    assert host.rows(f'SELECT map_id, instance_id FROM game_world_entity WHERE guid = {member_guid}', continent) == [[0, 0]]
    assert not host.rows(f'SELECT guid FROM game_character WHERE guid = {member_guid}', instances)
    assert host.rows(f'SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {member_guid}', continent) == inventory_before
    host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=instances)
    replacement = cross(member, member_guid, continent, instances, entrance)
    assert replacement not in (0, run, separate)
    login(leader, leader_guid)
    assert cross(leader, leader_guid, continent, instances, entrance) == replacement
    host.sql('DELETE FROM game_creature_loot WHERE creature_entry = 51000', instances)
    host.sql('INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090901,51000,5090050,10000,1,0,false)', instances)
    existing = set(row[0] for row in host.rows('SELECT guid FROM game_world_entity WHERE entry = 51000', instances))
    host.call('debug_spawn_at_feet', [leader_guid, 51000, 3], database=instances)
    spawned = set(row[0] for row in host.rows('SELECT guid FROM game_world_entity WHERE entry = 51000', instances)) - existing
    assert len(spawned) == 1
    corpse = spawned.pop()
    host.call('debug_apply_damage', [corpse, 1, leader_guid], database=instances)
    host.call('debug_kill_creature', [leader_guid, corpse], database=instances)
    assert host.rows(f'SELECT group_id FROM game_loot_roll WHERE corpse_guid = {corpse}', instances) == [[group]]
    before = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {leader_guid} AND entry = 5090050', instances)
    member.send(0x7b)
    host.wait_until(lambda: not host.rows(f'SELECT id FROM game_loot_roll WHERE corpse_guid = {corpse}', instances),
                    'disband did not settle the World Shard loot roll')
    assert len(host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {leader_guid} AND entry = 5090050', instances)) == len(before) + 1
    assert not host.rows(f'SELECT id FROM game_loot_roll_vote WHERE voter_guid = {member_guid}', instances)
    for client in (leader, member, solo):
        client.close()
