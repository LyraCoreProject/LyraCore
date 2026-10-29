import struct

from headless import creates, destroys


def check_operator_creatures(host):
    client = host.enter_world('Creatureeditor', at=(0, 20, 0, 0))
    host.account('CREATURESELECT')
    selection = host.connect('CREATURESELECT')
    assert selection.read()[0] == 0x1ee
    template = host.sql('SELECT * FROM game_creature_template WHERE entry = 620')[0]
    fields = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    del fields['entry']
    host.restore('game_faction_template', 'id = 50909')
    if not host.rows('SELECT id FROM game_faction_template WHERE id = 50909'):
        host.sql('INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES (50909,0,0,0,0,0,0,0,0,0,0,0,0)')
    fields.update(name='Operator creature', faction_template=50909, max_level=fields['level'], max_level_health=fields['health'])
    host.restore('game_creature_template', 'entry >= 5200500 AND entry <= 5999999')
    client.collect()
    selection.collect()
    host.call('operator_apply_creature_edits', [[{'createCreature': {'fields': fields}}]])
    entry = host.rows("SELECT entry FROM game_creature_template WHERE name = 'Operator creature'")[0][0]

    def response(packets, name):
        responses = [body for opcode, body in packets if opcode == 0x61]
        assert len(responses) == 1, responses
        assert struct.unpack_from('<I', responses[0])[0] == entry, responses
        assert responses[0][4:].split(b'\0', 1)[0] == name.encode(), responses

    response(client.collect(), 'Operator creature')
    response(selection.collect(), 'Operator creature')
    host.call('operator_create_creature_spawn', [entry, {
        'map_id': 0, 'x': 24.0, 'y': 0.0, 'z': 0.0, 'orientation': 0.0,
        'movement_type': 0, 'respawn_secs': 120,
    }])
    guid = host.rows(f'SELECT guid FROM game_creature_spawn WHERE entry = {entry}')[0][0]
    host.wait_until(lambda: host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}'), 'creature did not spawn')
    packets = client.collect()
    assert guid in creates(packets), packets
    life = host.rows(f'SELECT life_seq FROM game_creature_spawn WHERE guid = {guid}')[0][0]
    fields.update(name='Revised creature', display_id=100, npc_flags=129)
    update = {'updateCreature': {'entry': entry, 'fields': fields}}
    host.call('operator_apply_creature_edits', [[update, update]])
    host.wait_until(lambda: host.rows(f'SELECT display_id,npc_flags FROM game_world_entity WHERE guid = {guid}') == [[100, 129]], 'creature did not respawn')
    packets = client.collect()
    response(packets, 'Revised creature')
    response(selection.collect(), 'Revised creature')
    assert guid in destroys(packets), packets
    assert guid in creates(packets), packets
    assert host.rows(f'SELECT life_seq FROM game_creature_spawn WHERE guid = {guid}') == [[life + 1]]
    fields['name'] = 'Refused creature'
    try:
        host.call('operator_apply_creature_edits', [[update, {'setCreatureCast': {'creature_entry': entry, 'spell_id': {'some': 4000000000}}}]])
    except RuntimeError as refusal:
        assert 'creature edit 1' in str(refusal), refusal
    else:
        raise AssertionError('a missing spell succeeded')
    for observer in [client, selection]:
        assert not [body for opcode, body in observer.collect() if opcode == 0x61]
    assert host.rows(f'SELECT name FROM game_creature_template WHERE entry = {entry}') == [['Revised creature']]
    assert host.rows(f'SELECT life_seq FROM game_creature_spawn WHERE guid = {guid}') == [[life + 1]]
    host.call('operator_delete_creature_spawn', [guid])
