import struct


def check_instance_resets(host):
    leader = host.enter_world('Resetleader', at=(0, 1000, 1000, 0))
    member = host.enter_world('Resetmember', at=(0, 1000, 1000, 0))
    solo = host.enter_world('Resetsolo', at=(0, 1000, 1000, 0))
    leader.send(0x6e, b'Resetmember\0')
    leader.collect()
    member.collect()
    member.send(0x72)
    member.collect()
    leader.collect()

    def lease(client, map_id):
        host.call('debug_create_fixture_instance', [client.guid])
        client.collect()
        instance = host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {client.guid}')[0][0]
        host.call('debug_enter_instance', [client.guid, 0])
        client.collect()
        host.sql(f'UPDATE game_instance SET map_id = {map_id} WHERE instance_id = {instance}')
        host.sql(f'UPDATE game_instance_binding SET map_id = {map_id} WHERE character_guid = {client.guid} AND instance_id = {instance}')
        return instance

    party_run = lease(leader, 36)
    solo_run = lease(solo, 36)
    raid = lease(solo, 409)
    member.send(0x31d)
    assert not any(op in (0x31e, 0x31f) for op, _ in member.collect())
    assert host.rows(f'SELECT reset_requested FROM game_instance WHERE instance_id = {party_run}') == [[False]]
    member.send(0x4b)
    assert (0x4d, b'') in member.collect()
    leader.send(0x31d)
    assert (0x31f, struct.pack('<II', 1, 36)) in leader.collect()
    member.send(0x3d, struct.pack('<Q', member.guid))
    member.collect()
    host.sql(f'DELETE FROM game_instance_binding WHERE character_guid = {leader.guid}')
    leader.send(0x31d)
    assert (0x31e, struct.pack('<I', 36)) in leader.collect()
    assert host.rows(f'SELECT reset_requested FROM game_instance WHERE instance_id = {party_run}') in ([], [[True]])
    assert host.rows(f'SELECT reset_requested FROM game_instance WHERE instance_id = {solo_run}') == [[False]]
    solo.send(0x31d)
    packets = solo.collect()
    assert (0x31e, struct.pack('<I', 36)) in packets
    assert not any(op in (0x31e, 0x31f) and body[-4:] == struct.pack('<I', 409) for op, body in packets)
    assert host.rows(f'SELECT reset_requested FROM game_instance WHERE instance_id = {solo_run}') in ([], [[True]])
    assert host.rows(f'SELECT reset_requested FROM game_instance WHERE instance_id = {raid}') == [[False]]
    for instance in (party_run, solo_run, raid):
        if host.rows(f'SELECT instance_id FROM game_instance WHERE instance_id = {instance}'):
            host.call('debug_reap_instance', [instance])
