from package_groups import package_character, shards


def check(host):
    east, west = worlds = shards(host)
    alpha = package_character(host, east, 'Keeper')
    beta = package_character(host, west, 'Retiree')
    host.call('debug_emit_sessionless_group_intent', [alpha, beta, False], database=east)
    host.wait_until(lambda: len(host.rows('SELECT character_guid FROM game_group_member')) == 2, 'Group did not form')
    guild_id = 500
    guild = dict(guild_id=guild_id, name_key='retirement guild', name='Retirement Guild', leader_guid=alpha,
                 team=469, motd='', info='', emblem_style=0, emblem_color=0, border_style=0,
                 border_color=0, background_color=0, created_micros=0)
    ranks = [dict(id=rank + 1, guild_id=guild_id, rank_id=rank, name=name, rights=0xff1ff if rank < 2 else 0x43)
             for rank, name in enumerate(('Guild Master', 'Officer', 'Veteran', 'Member', 'Initiate'))]
    members = []
    for guid, name, shard, rank in ((alpha, 'Keeper', east, 0), (beta, 'Retiree', west, 4)):
        account = host.rows(f'SELECT account_id FROM game_character WHERE guid = {guid}', shard)[0][0]
        members.append(dict(character_guid=guid, guild_id=guild_id, rank_id=rank, name=name,
                            public_note='', officer_note='', realm_account_id=account, joined_micros=0))
    host.call('operator_import_guild', [dict(guild=guild, ranks=ranks, members=members, invites=[])])
    host.wait_until(lambda: all(len(host.rows('SELECT character_guid FROM game_guild_member', shard)) == 2
                               for shard in (None, *worlds)), 'guild did not project')

    def state(guid, shard):
        return [host.rows(f'SELECT * FROM {table} WHERE {key} = {guid}', shard) for table, key in (
            ('game_character', 'guid'), ('game_world_entity', 'guid'), ('game_item_instance', 'owner_guid'))]

    def refused(package, guid, shard, reason):
        before = state(guid, shard)
        try:
            host.call('debug_delete_package_character', [package, guid], database=shard)
            raise AssertionError('Package deletion was accepted')
        except RuntimeError as error:
            assert reason in str(error), str(error)
        assert state(guid, shard) == before, 'refused deletion changed the Character'

    refused('other', beta, west, 'another Package')
    refused('companions', alpha, east, 'Guild Leader')
    host.call('debug_set_sessionless_action_consent', [beta, False], database=west)
    host.wait_until(lambda: host.rows(f'SELECT active FROM game_package_party_authority WHERE guid = {beta}') == [[False]],
                    'revoked consent did not settle')
    host.call('operator_remove_peer_shard', [host.identity(west)])
    host.call('debug_delete_package_character', ['companions', beta], database=west)
    assert state(beta, west) == [[], [], []]
    assert host.rows(f'SELECT guid FROM game_character_directory WHERE guid = {beta}') == [[beta]]
    host.restart()
    for world in worlds:
        host.rows('SELECT guid FROM game_character', world)
    assert state(beta, west) == [[], [], []]
    host.call('operator_add_peer_shard', [host.identity(west), west])
    host.wait_until(lambda: not host.rows(f'SELECT guid FROM game_character_directory WHERE guid = {beta}'),
                    'deleted Character remained in the Realm directory', timeout=30)
    host.wait_until(lambda: all(not host.rows('SELECT character_guid FROM game_group_member', shard)
                               for shard in (None, *worlds)), 'deleted Character remained in its Group', timeout=20)
    host.wait_until(lambda: all(host.rows('SELECT character_guid FROM game_guild_member', shard) == [[alpha]]
                               for shard in (None, *worlds)), 'deleted Character remained in its guild', timeout=20)
    host.wait_until(lambda: all(host.rows(f'SELECT deleted FROM game_character_profile WHERE guid = {beta}', shard) == [[True]]
                               for shard in (None, *worlds)), 'deletion profile did not converge', timeout=20)
    assert not host.rows(f'SELECT guid FROM game_character_name WHERE guid = {beta}')
    replacement = package_character(host, east, 'Retiree')
    assert replacement != beta
    assert host.rows(f'SELECT guid FROM game_character_directory WHERE guid = {replacement}') == [[replacement]]
    host.call('debug_logout_character', [replacement], database=east)
    host.call('debug_delete_package_character', ['companions', replacement], database=east)
    assert state(replacement, east) == [[], [], []]
    host.wait_until(lambda: not host.rows(f'SELECT guid FROM game_character_directory WHERE guid = {replacement}'),
                    'Dormant Character remained in the Realm directory', timeout=30)
    assert all(not host.rows('SELECT connection_id FROM game_native_player', shard) for shard in worlds)
