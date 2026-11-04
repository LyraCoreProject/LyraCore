import struct

from headless import creates


def check(host):
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

    def package_character(world, name):
        host.call('debug_request_package_character', ['companions', name, 1, 1], database=world)
        request = host.rows(f"SELECT id FROM game_package_character_request WHERE name = '{name}'", world)[0][0]
        def ready():
            host.call('debug_take_package_character_result', ['companions', request], database=world)
            return not host.rows(f'SELECT id FROM game_package_character_request WHERE id = {request}', world)
        host.wait_until(ready, 'Package creation did not finish', timeout=20)
        guid = host.rows(f"SELECT guid FROM game_character WHERE name = '{name}'", world)[0][0]
        host.sql(f'UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {guid}', world)
        host.call('debug_spawn_player_entity', [guid], database=world)
        host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {guid}') == [[True]],
                        'Package Group presence did not arrive')
        return guid

    def members(expected):
        wanted = sorted(expected)
        return all(sorted(row[0] for row in host.rows('SELECT character_guid FROM game_group_member', world)) == wanted
                   for world in (None, *worlds))

    def invite(world, actor, target):
        host.call('debug_emit_sessionless_group_intent', [actor, target, False], database=world)

    def leave(world, actor):
        host.call('debug_emit_sessionless_group_intent', [actor, 0, True], database=world)
        host.wait_until(lambda: members([]), 'Package leave did not disband the two-member Group')

    alpha = package_character(east, 'Alpha')
    beta = package_character(west, 'Beta')
    invite(east, alpha, beta)
    host.restart()
    for world in worlds:
        host.rows('SELECT guid FROM game_character', world)
    host.wait_until(lambda: members([alpha, beta]), 'Package Group did not survive restart', timeout=20)
    assert len(host.rows('SELECT group_id FROM game_group')) == 1
    assert all(not host.rows('SELECT connection_id FROM game_native_player', world) for world in worlds)
    leave(west, beta)

    host.call('debug_set_sessionless_action_consent', [beta, False], database=west)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {beta}') == [[False]],
                    'revoked consent did not reach Realm')
    invite(east, alpha, beta)
    host.wait_until(lambda: not host.rows('SELECT id FROM game_realm_party_command', east), 'refused invite did not settle')
    assert members([])
    assert not host.rows('SELECT id FROM game_group_invite')
    host.call('debug_set_sessionless_action_consent', [beta, True], database=west)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {beta}') == [[True]],
                    'restored consent did not reach Realm')

    account = host.account('GROUPPLAYER')
    client = host.connect(account.username)
    assert client.read()[0] == 0x1ee
    host.call('operator_set_character_start_map', [1, 1, 0])
    client.send(0x36, b'Playername\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
    assert client.read() == (0x3a, b'\x2e')
    player = host.rows("SELECT guid FROM game_character WHERE name = 'Playername'", east)[0][0]
    host.sql(f'UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {player}', east)
    client.send(0x3d, struct.pack('<Q', player))
    packets = []
    while player not in creates(packets):
        packets.append(client.read())
    client.collect()
    host.wait_until(lambda: host.rows(f'SELECT online FROM game_party_character WHERE guid = {player}') == [[True]],
                    'player presence did not reach Realm')
    client.send(0x6e, b'Beta\0')
    host.wait_until(lambda: members([player, beta]), 'Package did not accept player invitation')
    leave(west, beta)
    client.collect()
    invite(west, beta, player)
    packets = []
    host.wait_until(lambda: packets.extend(client.collect()) or any(code == 0x6f for code, _ in packets),
                    'player did not receive Package invitation')
    client.send(0x72, b'')
    host.wait_until(lambda: members([player, beta]), 'player could not accept Package invitation')
    leave(west, beta)
    client.close()

    host.call('debug_logout_character', [beta], database=west)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {beta}') == [[False]],
                    'Dormant Package Character stayed available for Group invitations')
    host.call('debug_spawn_player_entity', [beta], database=west)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {beta}') == [[True]],
                    'restored Package body did not publish Group presence')
    host.call('debug_set_sessionless_action_consent', [alpha, False], database=east)
    host.call('operator_report_package_groups', database=east)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {alpha}') == [[False]],
                    'presence report reopened revoked consent')
    assert members([])
    for world in worlds:
        assert all(not online for [online] in host.rows("SELECT online FROM game_character WHERE name = 'Alpha'", world))
        assert all(not online for [online] in host.rows("SELECT online FROM game_character WHERE name = 'Beta'", world))
