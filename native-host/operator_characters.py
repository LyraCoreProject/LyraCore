import struct

from headless import creates, teleport_ack


def check_operator_characters(host):
    for map_id in [0, 1]:
        host.restore('game_map', f'id = {map_id}')
        if not host.rows(f'SELECT id FROM game_map WHERE id = {map_id}'):
            host.sql(f"INSERT INTO game_map (id,internal_name,name,map_type,linked_zone,parent,player_limit,reset_delay,ghost_entrance_map,ghost_entrance_x,ghost_entrance_y) VALUES ({map_id},'Test','Test',0,0,0,0,0,-1,0,0)")
        cell = int((17066.666 - 100) / (533.3333 / 16))
        key = (map_id << 32) | (cell << 16) | cell
        host.restore('game_terrain_chunk', f'key = {key}')
        heights = ':'.join(['10'] * 145)
        if not host.rows(f'SELECT key FROM game_terrain_chunk WHERE key = {key}'):
            host.call('import_terrain_chunks_append', [f'{map_id},{cell},{cell},0,0,0,0,{heights}'])
    client = host.enter_world('Meldplayer', at=(0, 20, 0, 0))
    guid = client.guid
    args = ['mELDpLAYER', 0, 100, 100, 10.5, 1.25]

    def refuse_pending():
        before = host.rows(f'SELECT * FROM game_character WHERE guid = {guid}')
        try:
            host.call('operator_teleport_character', args)
        except RuntimeError as refusal:
            assert 'character is already teleporting' in str(refusal), refusal
        else:
            raise AssertionError('teleport accepted before the previous acknowledgement')
        assert host.rows(f'SELECT * FROM game_character WHERE guid = {guid}') == before
        assert not [op for op, _ in client.collect(auto_ack=False) if op in [0xc7, 0x3f, 0x3e]]

    client.collect()
    host.call('operator_teleport_character', args)
    packets = client.collect(auto_ack=False)
    near = [body for opcode, body in packets if opcode == 0xc7]
    assert len(near) == 1, packets
    offset = 1 + near[0][0].bit_count() + 12
    assert struct.unpack_from('<ffff', near[0], offset) == (100, 100, 10.5, 1.25)
    refuse_pending()
    client.send(0xc7, teleport_ack(near[0]))
    client.collect()
    assert host.rows(f'SELECT map_id,x,y,z,orientation FROM game_world_entity WHERE guid = {guid}') == [[0, 100, 100, 10.5, 1.25]]

    args[1] = 1
    host.call('operator_teleport_character', args)
    packets = client.collect(auto_ack=False)
    assert [op for op, _ in packets if op in [0x3f, 0x3e]] == [0x3f, 0x3e], packets
    assert next(body for op, body in packets if op == 0x3e) == struct.pack('<Iffff', 1, 100, 100, 10.5, 1.25)
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')
    refuse_pending()
    client.send(0xdc)
    assert guid in creates(client.collect())
    assert host.rows(f'SELECT map_id,x,y,z,orientation FROM game_world_entity WHERE guid = {guid}') == [[1, 100, 100, 10.5, 1.25]]

    client.send(0x4b)
    assert (0x4d, b'') in client.collect()
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')
    args[1] = 0
    host.call('operator_teleport_character', args)
    assert not [op for op, _ in client.collect() if op in [0xc7, 0x3f, 0x3e]]
    client.send(0x3d, struct.pack('<Q', guid))
    assert guid in creates(client.collect())
    assert host.rows(f'SELECT map_id,x,y,z,orientation FROM game_world_entity WHERE guid = {guid}') == [[0, 100, 100, 10.5, 1.25]]
