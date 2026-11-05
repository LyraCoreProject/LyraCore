def check(host):
    def request(world, name, package='companions', race=1):
        host.call('debug_request_package_character', [package, name, race, 1], database=world)
        return host.rows(f"SELECT id FROM game_package_character_request WHERE name = '{name}'", world)[-1][0]

    def take(world, request_id, package='companions'):
        def consumed():
            host.call('debug_take_package_character_result', [package, request_id], database=world)
            return not host.rows(f'SELECT id FROM game_package_character_request WHERE id = {request_id}', world)
        host.wait_until(consumed, 'Package Character result did not settle', timeout=20)

    def refused_call(reducer, args, world, reason):
        try:
            host.call(reducer, args, database=world)
        except RuntimeError as error:
            assert reason in str(error), str(error)
        else:
            raise AssertionError(f'{reducer} did not refuse {reason}')

    local = request(None, 'Localbot', package='local')
    local_guid = host.rows("SELECT guid FROM game_character WHERE name = 'Localbot'")[0][0]
    take(None, local, package='local')
    assert host.rows(f'SELECT guid FROM game_character WHERE guid = {local_guid}') == [[local_guid]]

    east, west = [host.add_database(guid_base=base) for base in (1_000_000_000, 2_000_000_000)]
    worlds = (east, west)
    for world in worlds:
        host.call('debug_seed_scenario_fixtures', database=world)
        host.sql('DELETE FROM game_character WHERE guid = 1', world)
        host.call('operator_add_peer_shard', [host.identity(world), world])
        host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=world)
    host.sql('UPDATE game_start_position SET map_id = 1 WHERE race_class = 257', west)
    host.call('operator_set_shard_role', [{'some': {'realm': host.identity(east)}}])
    for world in worlds:
        host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=world)
        for peer in worlds:
            if peer != world:
                host.call('operator_add_peer_shard', [host.identity(peer), peer], database=world)
    for map_id, world in enumerate(worlds):
        for owner in (None, *worlds):
            host.call('operator_set_map_owner', [map_id, host.identity(world)], database=owner)
    host.wait_until(lambda: len(host.rows('SELECT world FROM game_character_names_ready')) == 2,
                    'World names did not arrive')

    first, second = request(east, 'Sharedbot'), request(west, 'sHAREDBOT')
    take(east, first)
    take(west, second)
    characters = [host.rows("SELECT guid,account_id,online FROM game_character WHERE name = 'Sharedbot'", east),
                  host.rows("SELECT guid,account_id,online FROM game_character WHERE name = 'sHAREDBOT'", west)]
    assert sum(len(rows) for rows in characters) == 1
    guid, account, online = next(rows[0] for rows in characters if rows)
    assert online is False
    assert host.rows(f'SELECT guid FROM game_character_directory WHERE guid = {guid}') == [[guid]]
    assert host.rows(f'SELECT guid FROM game_character_name WHERE guid = {guid}') == [[guid]]
    assert all(not value for value in host.rows(f'SELECT salt,verifier FROM game_account WHERE id = {account}')[0])
    assert host.rows(f'SELECT package_name FROM game_package_account WHERE account_id = {account}') == [['companions']]
    assert 'NAME_IN_USE' in ''.join(host.request(f'/v1/database/{world}/logs?num_lines=100').decode() for world in worlds)

    requests = [(worlds[i % 2], request(worlds[i % 2], f'Capacity{chr(97+i)}')) for i in range(10)]
    host.restart()
    for world in worlds:
        host.rows('SELECT guid FROM game_character', world)
    for world, request_id in requests:
        take(world, request_id)
    accounts = host.rows("SELECT account_id FROM game_package_account WHERE package_name = 'companions'")
    counts = [len(host.rows(f'SELECT guid FROM game_character_directory WHERE account_id = {account_id}'))
              for [account_id] in accounts]
    assert sorted(counts) == [1, 10], counts
    assert sum(len(host.rows('SELECT guid FROM game_character', world)) for world in worlds) == 11
    assert not host.rows('SELECT account_id FROM game_session WHERE account_id > 1')

    host.sql('UPDATE game_start_position SET map_id = 0 WHERE race_class = 257', west)
    failed = request(west, 'Retrybot')
    take(west, failed)
    assert not host.rows("SELECT name FROM game_character_name WHERE name = 'retrybot'")
    assert not host.rows("SELECT guid FROM game_character WHERE name = 'Retrybot'", west)
    host.sql('UPDATE game_start_position SET map_id = 1 WHERE race_class = 257', west)
    retry = request(west, 'Retrybot')
    refused_call('debug_take_package_character_result', ['other', retry], west, 'another Package')
    take(west, retry)
    assert len(host.rows("SELECT guid FROM game_character WHERE name = 'Retrybot'", west)) == 1

    available = next(account_id for [account_id] in accounts
                     if len(host.rows(f'SELECT guid FROM game_character_directory WHERE account_id = {account_id}')) < 10)
    conflict_world = next(world for world in worlds if host.rows(f'SELECT id FROM game_account WHERE id = {available}', world))
    username = host.rows(f'SELECT username FROM game_account WHERE id = {available}', conflict_world)[0][0]
    host.sql(f"UPDATE game_account SET username = 'legacy#collision' WHERE id = {available}", conflict_world)
    conflict = request(conflict_world, 'Conflictedbot')
    take(conflict_world, conflict)
    assert host.rows(f'SELECT username FROM game_account WHERE id = {available}', conflict_world) == [['legacy#collision']]
    assert not host.rows("SELECT name FROM game_character_name WHERE name = 'conflictedbot'")
    assert not host.rows("SELECT guid FROM game_character WHERE name = 'Conflictedbot'", conflict_world)
    host.sql(f"UPDATE game_account SET username = '{username}' WHERE id = {available}", conflict_world)
    repaired = request(conflict_world, 'Conflictedbot')
    take(conflict_world, repaired)
    assert len(host.rows("SELECT guid FROM game_character WHERE name = 'Conflictedbot'", conflict_world)) == 1

    host.sql("INSERT INTO game_package_teardown (package_name) VALUES ('stopped')", east)
    refused_call('debug_request_package_character', ['stopped', 'Stoppedbot', 1, 1], east, 'Package is stopped')
    host.sql("INSERT INTO game_package_teardown (package_name) VALUES ('realm_stopped')")
    stopped = request(east, 'Realmstopped', package='realm-stopped')
    take(east, stopped, package='realm-stopped')
    assert not host.rows("SELECT guid FROM game_character WHERE name = 'Realmstopped'", east)
    assert not host.rows("SELECT name FROM game_character_name WHERE name = 'realmstopped'")
    for payload in ([], [3] + [0] * 8 + [2] + [0] * 4):
        refused_call('idc_package_creation', [payload], east, 'Peer Shard')

    for world in worlds:
        assert not host.rows('SELECT id FROM game_package_character_request', world)
        assert not host.rows('SELECT connection_id FROM game_native_player', world)
