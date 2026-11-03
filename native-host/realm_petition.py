import struct

from headless import creates


def template(host, shard):
    if host.rows('SELECT entry FROM game_item_template WHERE entry = 5863', shard):
        return
    result = host.sql('SELECT * FROM game_item_template WHERE entry = 51', shard)[0]
    row = dict(zip([column['name']['some'] for column in result['schema']['elements']], result['rows'][0]))
    row.update(entry=5863, name='Guild Charter', max_stack=1, max_count=1, bonding=1, inventory_type=0)
    insert(host, shard, 'game_item_template', row)


def insert(host, shard, table, row):
    values = ["'" + value.replace("'", "''") + "'" if isinstance(value, str)
        else str(value).lower() for value in row.values()]
    host.sql(f"INSERT INTO {table} ({','.join(row)}) VALUES ({','.join(values)})", shard)


def settled(host, shard, guid):
    host.wait_until(lambda: not host.rows(f'SELECT guid FROM game_guild_transaction WHERE guid = {guid}', shard)
        and not host.rows('SELECT id FROM game_realm_guild_command', shard), 'guild transaction did not settle')


def enter(client, guid):
    client.send(0x3d, struct.pack('<Q', guid))
    packets = []
    while guid not in creates(packets):
        packets.append(client.read())
    client.collect()


def remove(client, guid, received):
    client.send(0x4b)
    received(client, 0x4d)
    client.send(0x38, struct.pack('<Q', guid))
    assert received(client, 0x3c) == [(0x3c, b'\x39')]
    client.close()


def check_import(host, worlds, character, received):
    eastern, western = worlds
    for shard in worlds:
        template(host, shard)
    owner_account, owner, guid = character('Petoldlead', 0)
    signer_account, signer, signer_guid = character('Petoldsign', 1)
    settled(host, eastern, guid)
    settled(host, western, signer_guid)
    host.call('debug_grant_item', [guid, 5863, 1], database=eastern)
    charter = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5863', eastern)[0][0]
    for client in (owner, signer):
        client.send(0x4b)
        received(client, 0x4d)
    petition = dict(petition_id=7000, charter_item_guid=charter, owner_guid=guid,
        owner_name='Petoldlead', team=469, name='Legacy Petition', created_micros=1234)
    signature = dict(signature_key=7000 << 8, petition_id=7000, signer_guid=signer_guid,
        signer_name='Petoldsign', signer_realm_account=signer_account.id, signed_micros=5678)
    insert(host, eastern, 'game_guild_petition', petition)
    insert(host, eastern, 'game_guild_petition_signature', signature)
    for shard in worlds:
        host.call('operator_report_character_profiles', database=shard)
    host.wait_until(lambda: host.rows(f'SELECT guid FROM game_character_profile WHERE guid = {signer_guid}'),
        'legacy signer profile did not reach Realm')
    host.sql(f'UPDATE game_guild_petition_signature SET signer_realm_account = 0 WHERE petition_id = 7000', eastern)
    host.call('operator_report_guild_petitions', database=eastern)
    settled(host, eastern, guid)
    assert not host.rows('SELECT petition_id FROM game_guild_petition WHERE petition_id = 7000')
    assert host.rows('SELECT petition_id FROM game_guild_petition WHERE petition_id = 7000', eastern) == [[7000]]
    host.sql(f'UPDATE game_guild_petition_signature SET signer_realm_account = {signer_account.id} WHERE petition_id = 7000', eastern)
    host.call('operator_report_guild_petitions', database=eastern)
    settled(host, eastern, guid)
    assert host.rows('SELECT petition_id, charter_item_guid, created_micros FROM game_guild_petition WHERE petition_id = 7000') == [[7000, charter, 1234]]
    assert host.rows('SELECT signer_guid, signer_realm_account, signed_micros FROM game_guild_petition_signature WHERE petition_id = 7000') == [[signer_guid, signer_account.id, 5678]]
    assert not host.rows('SELECT petition_id FROM game_guild_petition', eastern)
    assert not host.rows('SELECT signature_key FROM game_guild_petition_signature', eastern)
    assert host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {charter}', eastern) == [[charter]]
    host.call('operator_report_guild_petitions', database=eastern)
    assert host.rows('SELECT last_id FROM game_guild_petition_sequence') == [[7000]]
    enter(owner, guid)
    settled(host, eastern, guid)
    owner.send(0x1be, struct.pack('<Q', charter))
    assert received(owner, 0x1bf) == [(0x1bf, struct.pack('<QQIBQI', charter, guid, 7000, 1, signer_guid, 0))]
    host.call('gw_destroy_guild_charter', [{'guid': guid, 'ownership': None}, charter], database=eastern)
    settled(host, eastern, guid)
    assert not host.rows('SELECT petition_id FROM game_guild_petition WHERE petition_id = 7000')
    assert not host.rows('SELECT signature_key FROM game_guild_petition_signature WHERE petition_id = 7000')
    remove(owner, guid, received)
    signer.send(0x38, struct.pack('<Q', signer_guid))
    assert received(signer, 0x3c) == [(0x3c, b'\x39')]
    signer.close()


def vendor(host, shard, guid):
    before = {row[0] for row in host.rows('SELECT guid FROM game_world_entity WHERE entry = 51004', shard)}
    host.call('debug_spawn_at_feet', [guid, 51004, 2], database=shard)
    after = {row[0] for row in host.rows('SELECT guid FROM game_world_entity WHERE entry = 51004', shard)}
    [npc] = after - before
    host.sql(f'UPDATE game_world_entity SET npc_flags = 1536 WHERE guid = {npc}', shard)
    return npc


def transfer(host, client, guid, source, map_id, received):
    settled(host, source, guid)
    host.call('debug_teleport', [guid, map_id, 0, 0, 0, 0], database=source)
    received(client, 0x3e, auto_ack=False)
    client.send(0xdc)
    packets = []
    while guid not in creates(packets):
        packets.append(client.read())
    client.collect()


def check(host, worlds, character, received):
    eastern, western = worlds
    _, owner, guid = character('Charterlead', 0)
    signers = [character('Pet' + suffix, index % 2) for index, suffix in
        enumerate(('one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight', 'nine'))]
    for index, (_, _, signer) in enumerate(signers):
        settled(host, worlds[index % 2], signer)
    settled(host, eastern, guid)
    npc = vendor(host, eastern, guid)
    host.call('debug_set_money', [guid, 150000], database=eastern)
    owner.collect()

    def money(shard, character_guid=guid):
        return host.rows(f'SELECT money FROM game_world_entity WHERE guid = {character_guid}', shard)[0][0]

    def buy(name, npc_guid=npc):
        owner.send(0x1bd, struct.pack('<QIQ', npc_guid, 0, 0) + name.encode() + b'\0' + bytes(51))

    def emblem(client, npc_guid, result):
        client.send(0x1f1, struct.pack('<QIIIII', npc_guid, 11, 12, 3, 14, 15))
        assert (0x1f1, struct.pack('<I', result)) in received(client, 0x1f1)

    owner.send(0x1f1, struct.pack('<QIIIII', npc, 11, 12, 3, 14, 15))
    owner.send(0x4b)
    received(owner, 0x4d)
    settled(host, eastern, guid)
    assert host.rows(f'SELECT money FROM game_character WHERE guid = {guid}', eastern) == [[150000]], 'Realm refusal lost the refund during logout'
    enter(owner, guid)
    settled(host, eastern, guid)
    emblem(owner, npc, 2)
    settled(host, eastern, guid)
    assert money(eastern) == 150000, 'Realm refusal did not refund the debit'
    buy('Across Founders')
    host.wait_until(lambda: host.rows(f'SELECT petition_id FROM game_guild_petition WHERE owner_guid = {guid}'),
        'charter purchase did not reach Realm')
    settled(host, eastern, guid)
    petition_id, charter = host.rows(f'SELECT petition_id, charter_item_guid FROM game_guild_petition WHERE owner_guid = {guid}')[0]
    assert petition_id > 7000 and money(eastern) == 149000
    assert host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {charter}', eastern) == [[charter]]
    assert not host.rows(f'SELECT petition_id FROM game_guild_petition WHERE owner_guid = {guid}', eastern)
    owner.collect()
    buy('Duplicate Charter')
    received(owner, 0x112)
    assert money(eastern) == 149000
    first = signers[0]
    owner.send(0x1c3, struct.pack('<QQ', charter, first[2]))
    assert received(first[1], 0x1bf) == [(0x1bf, struct.pack('<QQIB', charter, guid, petition_id, 0))]
    owner.collect()
    owner.send(0x2c1, struct.pack('<Q', charter) + b'Realm Founders\0')
    assert received(owner, 0x2c1) == [(0x2c1, struct.pack('<Q', charter) + b'Realm Founders\0')]
    transfer(host, owner, guid, eastern, 1, received)
    settled(host, western, guid)
    assert host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {charter}', western) == [[charter]]
    for index, (_, client, signer) in enumerate(signers):
        client.collect()
        client.send(0x1c0, struct.pack('<QB', charter, 0))
        expected = (0x1c1, struct.pack('<QQI', charter, signer, 0))
        assert expected in received(client, 0x1c1)
        assert expected in received(owner, 0x1c1)
        if index == 7:
            owner.send(0x1c4, struct.pack('<Q', charter))
            assert received(owner, 0x1c5) == [(0x1c5, struct.pack('<I', 4))]
            settled(host, western, guid)
            assert host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {charter}', western) == [[charter]]
    assert len(host.rows(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}')) == 9
    owner.send(0x1c4, struct.pack('<Q', charter))
    assert received(owner, 0x1c5) == [(0x1c5, bytes(4))]
    settled(host, western, guid)
    guild_id = host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {guid}')[0][0]
    for shard in (None, *worlds):
        host.wait_until(lambda shard=shard: len(host.rows(f'SELECT character_guid FROM game_guild_member WHERE guild_id = {guild_id}', shard)) == 10,
            'founders did not receive guild membership')
    assert not host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {charter}', western)
    assert not host.rows(f'SELECT petition_id FROM game_guild_petition WHERE petition_id = {petition_id}')
    assert not host.rows(f'SELECT signature_key FROM game_guild_petition_signature WHERE petition_id = {petition_id}')
    npc_west = vendor(host, western, guid)
    _, officer, officer_guid = signers[1]
    host.call('debug_set_money', [officer_guid, 150000], database=western)
    officer.collect()
    emblem(officer, npc_west, 3)
    settled(host, western, officer_guid)
    assert money(western, officer_guid) == 150000
    owner.collect()
    emblem(owner, npc_west, 0)
    settled(host, western, guid)
    assert money(western) == 49000
    for shard in (None, *worlds):
        host.wait_until(lambda shard=shard: host.rows(f'SELECT emblem_style, emblem_color FROM game_guild WHERE guild_id = {guild_id}', shard) == [[11, 12]],
            'emblem was not projected to every World')
    owner.send(0x8f)
    received(owner, 0x92)
    host.wait_until(lambda: not host.rows(f'SELECT guild_id FROM game_guild WHERE guild_id = {guild_id}'), 'guild was not disbanded')
    host.call('debug_grant_item', [guid, 5863, 1], database=western)
    inert = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5863', western)[0][0]
    owner.send(0x4b)
    received(owner, 0x4d)
    enter(owner, guid)
    settled(host, western, guid)
    assert not host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {inert}', western), 'Realm absence did not remove an inert charter on login'
    remove(owner, guid, received)
    for _, client, signer in signers:
        remove(client, signer, received)
