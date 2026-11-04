import struct

from headless import creates
from package_groups import package_character, shards
from realm_guild import roster


def check(host):
    east, west = worlds = shards(host)
    alpha = package_character(host, east, 'Alpha')
    beta = package_character(host, west, 'Beta')
    host.wait_until(lambda: len(host.rows('SELECT guid FROM game_guild_character')) == 2,
                    'Package guild presence did not arrive')
    for shard in (None, *worlds):
        try:
            host.call('idc_package_guild', [[]], database=shard)
            raise AssertionError('untrusted Package guild message was accepted')
        except RuntimeError as error:
            assert 'caller is not a Peer Shard' in str(error)

    account = host.account('GUILDPLAYER')
    host.sql(f'UPDATE game_account SET gm_level = 1 WHERE id = {account.id}')
    client = host.connect(account.username)
    assert client.read()[0] == 0x1ee
    host.call('operator_set_character_start_map', [1, 1, 0])
    client.send(0x36, b'Guildleader\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
    assert client.read() == (0x3a, b'\x2e')
    leader = host.rows("SELECT guid FROM game_character WHERE name = 'Guildleader'", east)[0][0]
    host.sql(f'UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {leader}', east)
    client.send(0x3d, struct.pack('<Q', leader))
    packets = []
    while leader not in creates(packets):
        packets.append(client.read())
    client.collect()
    client.send(0x95, struct.pack('<II', 0, 7) + b'.guild create "Package Guild"\0')
    host.wait_until(lambda: host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {leader}'),
                    'native leader did not create the guild')
    guild = host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {leader}')[0][0]

    def members(expected):
        host.wait_until(lambda: all(sorted(host.rows(
            f'SELECT character_guid FROM game_guild_member WHERE guild_id = {guild}', shard)) == sorted([[g] for g in expected])
            for shard in (None, *worlds)), 'guild membership did not converge')

    def invitation(world, guid, present):
        def arrived():
            rows = host.rows(f'SELECT invitation FROM game_package_guild_invitation WHERE guid = {guid}', world)
            return bool(rows) and rows[0][0][0] == (0 if present else 1)
        host.wait_until(arrived, 'Package guild invitation did not converge')

    def action(world, guid, verb, target=0):
        host.call('debug_package_guild_action', [guid, verb, target], database=world)
        host.wait_until(lambda: not host.rows('SELECT id FROM game_package_guild_delivery', world),
                        'Package guild request did not settle')

    members([leader])
    client.send(0x82, b'Beta\0')
    invitation(west, beta, True)
    members([leader])
    host.call('debug_set_sessionless_action_consent', [beta, False], database=west)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {beta}') == [[False]],
                    'revoked consent did not reach Realm')
    try:
        action(west, beta, 'accept')
        raise AssertionError('guild acceptance ignored revoked consent')
    except RuntimeError as error:
        assert 'group:action_suppressed' in str(error), str(error)
    members([leader])
    host.call('debug_set_sessionless_action_consent', [beta, True], database=west)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {beta}') == [[True]],
                    'restored consent did not reach Realm')
    host.call('debug_package_guild_action', [beta, 'accept', 0], database=west)
    client.close()
    host.restart()
    for world in worlds:
        host.rows('SELECT guid FROM game_character', world)
    members([leader, beta])
    invitation(west, beta, False)
    action(west, beta, 'invite', alpha)
    assert not host.rows(f'SELECT target_guid FROM game_guild_invite WHERE target_guid = {alpha}'), 'Initiate invited a member'

    client = host.connect(account.username)
    assert client.read()[0] == 0x1ee
    client.send(0x3d, struct.pack('<Q', leader))
    packets = []
    while leader not in creates(packets):
        packets.append(client.read())
    client.collect()
    for rank in (3, 2, 1):
        client.send(0x8b, b'Beta\0')
        host.wait_until(lambda: host.rows(f'SELECT rank_id FROM game_guild_member WHERE character_guid = {beta}') == [[rank]],
                        'Package member promotion did not arrive')
    action(west, beta, 'invite', alpha)
    invitation(east, alpha, True)
    action(east, alpha, 'decline')
    invitation(east, alpha, False)
    assert not host.rows(f'SELECT target_guid FROM game_guild_invite WHERE target_guid = {alpha}')
    action(west, beta, 'invite', alpha)
    invitation(east, alpha, True)
    action(east, alpha, 'accept')
    members([leader, alpha, beta])

    authority = host.rows(f'SELECT revision FROM game_package_party_authority WHERE guid = {beta}', west)
    host.call('debug_set_level', [beta, 12], database=west)
    host.wait_until(lambda: host.rows(f'SELECT level FROM game_guild_character WHERE guid = {beta}') == [[12]],
                    'Package guild level did not refresh without native players on its World')
    assert host.rows(f'SELECT revision FROM game_package_party_authority WHERE guid = {beta}', west) == authority
    client.collect()
    client.send(0x89)
    packets = []
    host.wait_until(lambda: packets.extend(client.collect()) or any(code == 0x8a for code, _ in packets),
                    'native guild roster did not arrive')
    entries = roster([body for code, body in packets if code == 0x8a][-1])[2]
    assert entries[beta][:5] == ('Beta', 1, 1, 12, 1)
    assert entries[alpha][:2] == ('Alpha', 1)
    action(west, beta, 'leave')
    members([leader, alpha])
    client.send(0x82, b'Beta\0')
    invitation(west, beta, True)
    client.send(0x8f)
    members([])
    invitation(west, beta, False)
    assert not host.rows('SELECT target_guid FROM game_guild_invite')
    for world, guid in ((east, alpha), (west, beta)):
        assert host.rows(f'SELECT online FROM game_character WHERE guid = {guid}', world) == [[False]]
        assert not host.rows(f'SELECT connection_id FROM game_native_player WHERE character_guid = {guid}', world)
    client.close()
    check_invitation_ordering(host)
    check_local(host)


def check_invitation_ordering(host):
    world = host.add_database(guid_base=3_000_000_000)
    [operator_hex] = host.rows('SELECT identity FROM game_operator')[0][0]
    operator = {'__identity__': f'0x{int(operator_hex, 16):064x}'}
    host.call('operator_add_peer_shard', [operator, 'invitation ordering fixture'], database=world)
    host.call('operator_set_shard_role', [{'some': {'world': operator}}], database=world)
    guid = 9_000_000_020

    def projection(revision, present):
        payload = struct.pack('<BQQB', 1, guid, revision, 0 if present else 1)
        if present:
            payload += struct.pack('<IQq', 22, 7, 100)
        host.call('idc_package_guild', [list(payload)], database=world)

    projection(10, True)
    projection(20, False)
    cleared = host.rows('SELECT revision, invitation FROM game_package_guild_invitation', world)
    projection(10, True)
    assert host.rows('SELECT revision, invitation FROM game_package_guild_invitation', world) == cleared


def check_local(host):
    shard = host.add_database(guid_base=4_000_000_000)
    host.call('debug_seed_scenario_fixtures', database=shard)
    host.call('debug_request_package_character', ['companions', 'Localbot', 1, 1], database=shard)
    guid, account = host.rows("SELECT guid, account_id FROM game_character WHERE name = 'Localbot'", shard)[0]
    host.call('debug_spawn_player_entity', [guid], database=shard)
    actor = {'guid': guid, 'ownership': None}
    host.call('realm_guild_op', [actor, {'gmCreate': {
        'leader_guid': guid, 'leader_name': 'Localbot', 'leader_team': 469,
        'leader_realm_account': account, 'gm_level': 1, 'name': 'Local Guild',
    }}], database=shard)
    host.call('debug_request_package_character', ['companions', 'Localpeer', 1, 1], database=shard)
    peer = host.rows("SELECT guid FROM game_character WHERE name = 'Localpeer'", shard)[0][0]
    host.call('debug_spawn_player_entity', [peer], database=shard)
    host.call('debug_package_guild_action', [guid, 'invite', peer], database=shard)
    host.call('debug_package_guild_action', [peer, 'decline', 0], database=shard)
    assert not host.rows('SELECT target_guid FROM game_guild_invite', shard)
    host.call('debug_package_guild_action', [guid, 'invite', peer], database=shard)
    host.call('debug_package_guild_action', [peer, 'accept', 0], database=shard)
    assert len(host.rows('SELECT character_guid FROM game_guild_member', shard)) == 2
    host.call('debug_package_guild_action', [peer, 'leave', 0], database=shard)
    host.call('debug_package_guild_action', [guid, 'leave', 0], database=shard)
    assert not host.rows('SELECT guild_id FROM game_guild', shard)
    assert not host.rows('SELECT connection_id FROM game_native_player', shard)
