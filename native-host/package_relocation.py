from package_groups import package_character, shards


def check(host):
    east, west = shards(host)
    guid = package_character(host, east, 'Traveller')
    trigger = 5090652
    host.sql(f'INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) VALUES ({trigger},0,0,0,0,5,0,0,0,0)', east)
    host.sql(f"INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) VALUES ({trigger},1,10,10,0,0,'Package relocation')", east)

    def state():
        return [host.rows(f'SELECT * FROM {table} WHERE {key} = {guid}', east) for table, key in (
            ('game_character', 'guid'), ('game_world_entity', 'guid'),
            ('game_character_relocation', 'character_guid'), ('game_bot_transfer_intent', 'bot_guid'),
            ('game_instance_binding', 'character_guid'))]

    def refused(reducer, args):
        before = state()
        try:
            host.call(reducer, args, database=east)
        except RuntimeError as error:
            assert 'OtherPartition' in str(error), str(error)
        else:
            raise AssertionError(f'{reducer} reported success for a destination on another World')
        assert state() == before, 'refused relocation changed Character state'
        assert not host.rows(f'SELECT guid FROM game_character WHERE guid = {guid}', west)

    refused('debug_replay_sessionless_areatrigger', [guid, trigger, 1, 0, 101, 0])
    refused('debug_bot_transfer', [guid, 1, 0, 10, 10, 0, 0, 'Package relocation'])
    host.sql(f'UPDATE game_areatrigger_teleport SET target_map = 2 WHERE trigger_id = {trigger}', east)
    refused('debug_replay_sessionless_areatrigger', [guid, trigger, 2, 0, 101, 0])
    host.sql(f'UPDATE game_areatrigger_teleport SET target_map = 0 WHERE trigger_id = {trigger}', east)
    host.call('debug_replay_sessionless_areatrigger', [guid, trigger, 0, 0, 101, 0], database=east)
    assert host.rows(f'SELECT map_id, instance_id, x, y FROM game_world_entity WHERE guid = {guid}', east) == [[0, 0, 10, 10]]
    receipt = host.rows(f'SELECT id FROM game_character_relocation WHERE character_guid = {guid}', east)[0][0]
    host.call('debug_replay_sessionless_areatrigger', [guid, trigger, 0, 0, 101, receipt], database=east)
    assert not host.rows(f'SELECT connection_id FROM game_native_player WHERE character_guid = {guid}', east)
