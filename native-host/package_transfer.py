from package_groups import package_character, shards


def check(host):
    east, west = shards(host)
    dungeon = host.add_database(guid_base=3_000_000_000)
    host.call('debug_seed_scenario_fixtures', database=dungeon)
    host.sql('DELETE FROM game_character WHERE guid = 1', dungeon)
    worlds = (east, west, dungeon)
    host.call('operator_add_peer_shard', [host.identity(dungeon), dungeon])
    host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=dungeon)
    host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=dungeon)
    for world in worlds:
        for peer in worlds:
            if peer != world:
                host.call('operator_add_peer_shard', [host.identity(peer), peer], database=world)
    for shard in (None, *worlds):
        for map_id, world in ((0, east), (1, west), (36, dungeon)):
            host.call('operator_set_map_owner', [map_id, host.identity(world)], database=shard)
        host.call('operator_set_instance_host', [36, host.identity(dungeon), True], database=shard)
    forward, backward, entrance, exit_trigger, local = range(5090660, 5090665)
    for world in worlds:
        for trigger, origin, target in ((forward, 0, 1), (backward, 1, 0), (entrance, 0, 36), (exit_trigger, 36, 0), (local, 36, 36)):
            host.sql(f'INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) VALUES ({trigger},{origin},0,0,0,5,0,0,0,0)', world)
            host.sql(f"INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) VALUES ({trigger},{target},0,0,0,0,'Package Transfer')", world)

    alpha = package_character(host, east, 'Mover')
    beta = package_character(host, east, 'Partner')
    host.call('debug_emit_sessionless_group_intent', [alpha, beta, False], database=east)
    host.wait_until(lambda: len(host.rows('SELECT character_guid FROM game_group_member')) == 2, 'Package Group did not form')
    group = host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {alpha}')[0][0]
    inventory = host.rows(f'SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {alpha}', east)
    owner = host.rows(f'SELECT account_id FROM game_character WHERE guid = {alpha}', east)[0][0]

    def request(world, guid, trigger):
        host.call('debug_request_package_relocation', ['companions', guid, trigger], database=world)
        return host.rows(f'SELECT id FROM game_package_relocation WHERE guid = {guid}', world)[-1][0]

    def settled(world, id, outcome):
        def done():
            rows = host.rows(f'SELECT result FROM game_package_relocation WHERE id = {id}', world)
            return rows and rows[0][0][0] == outcome
        host.wait_until(done, f'Package relocation did not reach result {outcome}', timeout=30)

    def take(world, id):
        host.call('debug_take_package_relocation', ['companions', id], database=world)
        assert not host.rows(f'SELECT id FROM game_package_relocation WHERE id = {id}', world)

    def arrived(source, target, guid, id, expected_group=group):
        settled(source, id, 1)
        assert host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {guid}', target) == [[guid]]
        assert host.rows(f'SELECT online FROM game_character WHERE guid = {guid}', target) == [[False]]
        if source != target:
            assert not host.rows(f'SELECT guid FROM game_character WHERE guid = {guid}', source)
        world = host.rows(f'SELECT world FROM game_character_directory WHERE guid = {guid}')[0][0][0]
        target_identity = int(host.identity(target)['__identity__'], 16)
        assert int(world, 16) == target_identity
        def present():
            rows = host.rows(f'SELECT world, active FROM game_package_party_authority WHERE guid = {guid}')
            return rows and int(rows[0][0][0], 16) == target_identity and rows[0][1]
        host.wait_until(present, 'Package authority did not reach its destination', timeout=20)
        assert host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {guid}', target) == ([[expected_group]] if expected_group else [])
        assert not host.rows(f'SELECT transfer_id FROM game_transfer_in WHERE character_guid = {guid}', target)
        assert not host.rows(f'SELECT transfer_id FROM game_transfer_out WHERE character_guid = {guid}', source)
        assert all(not host.rows('SELECT connection_id FROM game_native_player', shard) for shard in worlds)

    host.call('debug_set_sessionless_action_consent', [alpha, False], database=east)
    try:
        request(east, alpha, forward)
        raise AssertionError('revoked consent allowed relocation')
    except RuntimeError as error:
        assert 'disabled' in str(error)
    assert not host.rows('SELECT id FROM game_package_relocation', east)
    host.call('debug_set_sessionless_action_consent', [alpha, True], database=east)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {alpha}') == [[True]], 'consent did not arrive')
    conflict = host.account('CONFLICT', database=west)
    host.sql(f'UPDATE game_account SET id = {owner} WHERE id = {conflict.id}', west)
    refused = request(east, alpha, forward)
    settled(east, refused, 2)
    assert host.rows(f'SELECT map_id FROM game_world_entity WHERE guid = {alpha}', east) == [[0]]
    assert not host.rows(f'SELECT guid FROM game_character WHERE guid = {alpha}', west)
    host.sql(f'DELETE FROM game_account WHERE id = {owner}', west)
    take(east, refused)
    for shard in (None, *worlds):
        try:
            host.call('idc_package_relocation', [[]], database=shard)
            raise AssertionError('non-Peer relocation was accepted')
        except RuntimeError as error:
            assert 'Peer Shard' in str(error)
    host.call('operator_remove_peer_shard', [host.identity(west)])
    crossing = request(east, alpha, forward)
    host.wait_until(lambda: not host.rows(f'SELECT guid FROM game_character WHERE guid = {alpha}', east),
                    'import did not commit before directory settlement', timeout=20)
    assert host.rows(f'SELECT character_guid FROM game_transfer_in WHERE character_guid = {alpha}', west) == [[alpha]]
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {alpha}', west)
    host.call('debug_take_package_relocation', ['companions', crossing], database=east)
    assert host.rows(f'SELECT result FROM game_package_relocation WHERE id = {crossing}', east)[0][0][0] == 0
    try:
        host.call('teardown_package', ['companions'], database=east)
        raise AssertionError('teardown discarded a pending relocation')
    except RuntimeError as error:
        assert 'relocation is pending' in str(error)
    host.restart()
    for world in worlds:
        host.rows('SELECT guid FROM game_character', world)
    assert host.rows(f'SELECT character_guid FROM game_transfer_in WHERE character_guid = {alpha}', west) == [[alpha]]
    assert not host.rows(f'SELECT guid FROM game_world_entity WHERE guid = {alpha}', west)
    host.call('operator_add_peer_shard', [host.identity(west), west])
    arrived(east, west, alpha, crossing)
    try:
        host.call('debug_take_package_relocation', ['other', crossing], database=east)
        raise AssertionError('another Package consumed the relocation result')
    except RuntimeError as error:
        assert 'another Package' in str(error)
    assert sorted(host.rows(f'SELECT guid, entry, slot, stack_count FROM game_item_instance WHERE owner_guid = {alpha}', west)) == sorted(inventory)
    assert host.rows(f'SELECT package_name FROM game_package_account WHERE account_id = {owner}', west) == [['companions']]
    host.restart()
    for world in worlds:
        host.rows('SELECT guid FROM game_character', world)
    settled(east, crossing, 1)
    take(east, crossing)
    returning = request(west, alpha, backward)
    arrived(west, east, alpha, returning)
    take(west, returning)
    entry = request(east, alpha, entrance)
    host.restart()
    for world in worlds:
        host.rows('SELECT guid FROM game_character', world)
    arrived(east, dungeon, alpha, entry)
    run = host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {alpha}', dungeon)[0][0]
    assert run != 0
    assert host.rows(f'SELECT party_id FROM game_instance WHERE instance_id = {run}', dungeon) == [[group]]
    take(east, entry)
    joined = request(east, beta, entrance)
    arrived(east, dungeon, beta, joined)
    assert host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {beta}', dungeon) == [[run]]
    take(east, joined)
    repeated = request(dungeon, alpha, local)
    arrived(dungeon, dungeon, alpha, repeated)
    assert host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {alpha}', dungeon) == [[run]]
    take(dungeon, repeated)
    leaving = request(dungeon, alpha, exit_trigger)
    arrived(dungeon, east, alpha, leaving)
    take(dungeon, leaving)
    assert host.rows(f'SELECT instance_id FROM game_instance_binding WHERE character_guid = {alpha} AND map_id = 36', east) == [[run]]
    reentry = request(east, alpha, entrance)
    arrived(east, dungeon, alpha, reentry)
    assert host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {alpha}', dungeon) == [[run]]
    take(east, reentry)
    assert all(not host.rows('SELECT id FROM game_package_arrival', shard) for shard in worlds)

    for shard in (None, *worlds):
        host.call('operator_set_instance_host', [36, host.identity(dungeon), False], database=shard)
        host.call('operator_set_instance_host', [36, host.identity(east), True], database=shard)
    solo = package_character(host, east, 'Solorun')
    admission = request(east, solo, entrance)
    arrived(east, east, solo, admission, expected_group=0)
    assert host.rows(f'SELECT map_id FROM game_world_entity WHERE guid = {solo}', east) == [[36]]
    assert host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {solo}', east)[0][0] != 0
    take(east, admission)
