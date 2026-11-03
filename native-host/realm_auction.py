import struct

from realm_petition import enter, insert, remove, settled, transfer, vendor


ITEM = 5090620


def fixtures(host, worlds):
    for shard in (None, *worlds):
        result = host.sql('SELECT * FROM game_item_template WHERE entry = 51', shard)[0]
        row = dict(zip([column['name']['some'] for column in result['schema']['elements']], result['rows'][0]))
        row.update(entry=ITEM, name='Realm Auction Sword', max_stack=1, max_count=0, bonding=0,
            inventory_type=13, quality=2, required_level=1, sell_price=1000, max_durability=30)
        row.update({'class': 2, 'subclass': 7})
        host.sql(f'DELETE FROM game_item_template WHERE entry = {ITEM}', shard)
        insert(host, shard, 'game_item_template', row)
        for house in (1, 7):
            if not host.rows(f'SELECT id FROM game_auction_house WHERE id = {house}', shard):
                host.sql(f"INSERT INTO game_auction_house (id,faction,deposit_rate,consignment_rate,name) VALUES ({house},0,5,5,'Realm Auction')", shard)
        for faction in (12, 120):
            if not host.rows(f'SELECT id FROM game_faction_template WHERE id = {faction}', shard):
                host.sql(f'INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES ({faction},0,0,0,0,0,0,0,0,0,0,0,0)', shard)


def auctioneer(host, shard, guid):
    npc = vendor(host, shard, guid)
    host.sql(f'UPDATE game_world_entity SET npc_flags = 4096, faction_template = 12 WHERE guid = {npc}', shard)
    return npc


def grant(host, shard, guid):
    host.call('debug_grant_item', [guid, ITEM, 1], database=shard)
    item = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = {ITEM}', shard)[0][0]
    host.sql(f'UPDATE game_item_instance SET durability = 17, enchant_id = 7745, random_property_id = 1900 WHERE guid = {item}', shard)
    return item


def sell(client, npc, item):
    client.send(0x256, struct.pack('<QQIII', npc, item, 100, 500, 120))


def check_import(host, worlds, character, received):
    fixtures(host, worlds)
    owners = []
    expected = []
    for index, shard in enumerate(worlds):
        _, client, guid = character(('Auctionoldone', 'Auctionoldtwo')[index], index)
        settled(host, shard, guid)
        npc = auctioneer(host, shard, guid)
        host.call('debug_set_money', [guid, 1000], database=shard)
        item = grant(host, shard, guid)
        host.call('operator_set_shard_role', [{'none': []}], database=shard)
        try:
            sell(client, npc, item)
            [(_, response)] = received(client, 0x25b)
            assert struct.unpack('<III', response)[1:] == (0, 0)
        finally:
            host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=shard)
        expected.extend(host.rows('SELECT owner_guid,item_guid,deposit,created_at,expires_at,item_enchant_id FROM game_auction', shard))
        client.send(0x4b)
        received(client, 0x4d)
        owners.append((client, guid, shard))
    host.wait_until(lambda: not host.rows('SELECT account_id FROM game_native_away'), 'migration still has World Sessions')
    for shard in worlds:
        host.call('operator_route_auctions', database=shard)
    host.wait_until(lambda: all(not host.rows('SELECT operation FROM game_auction_transaction', shard) for shard in worlds), 'Listing migration did not settle')
    assert sorted(host.rows('SELECT owner_guid,item_guid,deposit,created_at,expires_at,item_enchant_id FROM game_auction')) == sorted(expected)
    assert all(not host.rows('SELECT id FROM game_auction', shard) for shard in worlds)
    assert len({row[0] for row in host.rows('SELECT id FROM game_auction')}) == 2
    for client, guid, shard in owners:
        client.send(0x38, struct.pack('<Q', guid))
        assert received(client, 0x3c) == [(0x3c, b'\x3a')]
        [listing] = host.rows(f'SELECT id FROM game_auction WHERE owner_guid = {guid}')
        host.call('debug_auction_due', listing)
        host.wait_until(lambda: not host.rows(f'SELECT id FROM game_auction WHERE owner_guid = {guid}'), 'imported Listing did not expire')
        host.wait_until(lambda: host.rows(f'SELECT id FROM game_mail WHERE recipient_guid = {guid}', shard), 'expired Listing item did not return')
        client.send(0x38, struct.pack('<Q', guid))
        assert received(client, 0x3c) == [(0x3c, b'\x39')]
        client.close()


def check(host, worlds, character, received):
    east, west = worlds
    _, seller, owner = character('Realmseller', 0)
    _, bidder, buyer = character('Realmbidder', 1)
    _, rival, competitor = character('Realmrival', 0)
    for shard, guid in ((east, owner), (west, buyer), (east, competitor)):
        settled(host, shard, guid)
        host.call('debug_set_money', [guid, 10000], database=shard)
    npc = auctioneer(host, east, owner)
    remote_npc = auctioneer(host, west, buyer)

    def money(shard, guid):
        return host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}', shard)[0][0]

    def result(client, listing, action, code=0, extra=b''):
        assert (0x25b, struct.pack('<III', listing, action, code) + extra) in received(client, 0x25b)

    def listing(with_text=False):
        item = grant(host, east, owner)
        if with_text:
            host.sql("INSERT INTO game_item_text (id,text,letters) VALUES (5090620,'auction letter',1)", east)
            host.sql("INSERT INTO game_item_text (id,text,letters) VALUES (5090620,'unrelated text',4294967295)", west)
            host.sql(f'UPDATE game_item_instance SET item_text_id = 5090620 WHERE guid = {item}', east)
        sell(seller, npc, item)
        [(_, response)] = received(seller, 0x25b)
        identifier, action, code = struct.unpack('<III', response)
        assert (action, code) == (0, 0), response.hex()
        assert host.rows(f'SELECT deposit,item_durability,item_enchant_id,random_property_id FROM game_auction WHERE id = {identifier}') == [[50, 17, 7745, 1900]]
        assert not host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {item}', east)
        assert not host.rows('SELECT id FROM game_auction', east)
        sell(seller, npc, item)
        result(seller, identifier, 0)
        return identifier

    def bid(client, auctioneer, identifier, offer):
        client.send(0x25a, struct.pack('<QII', auctioneer, identifier, offer))

    def page(client, auctioneer, opcode=0x258, name='', offset=0):
        body = struct.pack('<QI', auctioneer, offset)
        if opcode == 0x258:
            body += name.encode() + b'\0' + struct.pack('<BBIIIIB', 0, 0, 0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, 0)
        if opcode == 0x264:
            body += bytes(4)
        client.send(opcode, body)
        [(_, body)] = received(client, {0x258: 0x25c, 0x259: 0x25d, 0x264: 0x265}[opcode])
        count = struct.unpack_from('<I', body)[0]
        assert len(body) == 8 + count * 64
        return [struct.unpack_from('<I', body, 4 + index * 64)[0] for index in range(count)], struct.unpack_from('<I', body, 4 + count * 64)[0]

    identifier = listing()
    assert money(east, owner) == 9950
    assert page(bidder, remote_npc, name='aUcTiOn SwOrD') == ([identifier], 1)
    assert page(bidder, remote_npc, name='missing') == ([], 0)
    assert page(bidder, remote_npc, offset=1) == ([], 1)
    assert page(seller, npc, 0x259) == ([identifier], 1)
    host.sql(f'UPDATE game_world_entity SET faction_template = 120 WHERE guid = {remote_npc}', west)
    assert page(bidder, remote_npc) == ([], 0)
    bid(bidder, remote_npc, identifier, 100)
    result(bidder, identifier, 2, 4)
    assert money(west, buyer) == 10000
    host.sql(f'UPDATE game_world_entity SET faction_template = 12 WHERE guid = {remote_npc}', west)
    bid(seller, npc, identifier, 100)
    result(seller, identifier, 2, 10)
    assert money(east, owner) == 9950
    bid(bidder, remote_npc, identifier, 99)
    result(bidder, identifier, 2, 7)
    bid(bidder, remote_npc, identifier, 100)
    result(bidder, identifier, 2, extra=struct.pack('<I', 5))
    assert money(west, buyer) == 9900
    assert page(bidder, remote_npc, 0x264) == ([identifier], 1)
    assert any(struct.unpack_from('<I', body)[0] == identifier for _, body in received(seller, 0x25f))
    bid(rival, npc, identifier, 105)
    result(rival, identifier, 2, extra=struct.pack('<I', 6))
    received(bidder, 0x25e)
    host.wait_until(lambda: host.rows(f'SELECT money FROM game_mail WHERE recipient_guid = {buyer}', west) == [[100]], 'displaced bid did not refund remotely')
    bid(bidder, remote_npc, identifier, 700)
    result(bidder, identifier, 2, extra=struct.pack('<I', 25))
    assert money(west, buyer) == 9400
    assert not host.rows(f'SELECT id FROM game_auction WHERE id = {identifier}')
    host.wait_until(lambda: host.rows(f'SELECT money FROM game_mail WHERE recipient_guid = {owner}', east) == [[525]], 'seller proceeds did not arrive')
    host.wait_until(lambda: host.rows(f'SELECT item_entry,item_durability,item_enchant_id,random_property_id FROM game_mail WHERE recipient_guid = {buyer} AND item_entry != 0', west) == [[ITEM, 17, 7745, 1900]], 'won item changed in delivery')
    host.wait_until(lambda: host.rows(f'SELECT money FROM game_mail WHERE recipient_guid = {competitor}', east) == [[105]], 'displaced buyout bid was lost')
    bid(bidder, remote_npc, identifier, 700)
    result(bidder, identifier, 2, 4)
    assert money(west, buyer) == 9400

    cancelled = listing()
    bid(bidder, remote_npc, cancelled, 100)
    result(bidder, cancelled, 2, extra=struct.pack('<I', 5))
    seller.send(0x257, struct.pack('<QI', npc, cancelled))
    result(seller, cancelled, 1)
    assert money(east, owner) == 9895
    assert not host.rows(f'SELECT id FROM game_auction WHERE id = {cancelled}')

    pending = listing(with_text=True)
    host.call('operator_remove_peer_shard', [host.identity()], database=west)
    bid(bidder, remote_npc, pending, 700)
    host.wait_until(lambda: host.rows(f'SELECT operation FROM game_auction_transaction WHERE guid = {buyer}', west), 'bid was not retained pending its answer')
    host.wait_until(lambda: not host.rows(f'SELECT id FROM game_auction WHERE id = {pending}'), 'Realm did not commit the buyout')
    assert money(west, buyer) == 8600
    host.call('debug_teleport', [buyer, 0, 0, 0, 0, 0], database=west)
    assert host.rows(f'SELECT map_id FROM game_world_entity WHERE guid = {buyer}', west) == [[1]]
    host.call('debug_set_money', [buyer, 4294967295], database=west)
    host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=west)
    host.wait_until(lambda: host.rows(f'SELECT copper FROM game_auction_transaction WHERE guid = {buyer}', west) == [[200]], 'buyout excess was not retained at purse cap')
    host.call('debug_set_money', [buyer, 1000], database=west)
    result(bidder, pending, 2, extra=struct.pack('<I', 25))
    assert money(west, buyer) == 1200
    host.wait_until(lambda: not host.rows(f'SELECT operation FROM game_auction_transaction WHERE guid = {buyer}', west), 'bid settlement did not release its fence')

    host.wait_until(lambda: host.rows(f'SELECT item_text_id FROM game_mail WHERE recipient_guid = {buyer} AND item_text_id != 0', west), 'won Letter Copy did not arrive')
    [[text_id]] = host.rows(f'SELECT item_text_id FROM game_mail WHERE recipient_guid = {buyer} AND item_text_id != 0', west)
    assert text_id != 5090620
    assert host.rows(f'SELECT text,letters FROM game_item_text WHERE id = {text_id}', west) == [['auction letter', 1]]
    assert host.rows('SELECT text FROM game_item_text WHERE id = 5090620', west) == [['unrelated text']]
    assert not host.rows('SELECT id FROM game_item_text WHERE id = 5090620', east)

    expiring = listing()
    transfer(host, seller, owner, east, 1, received)
    expired_mail = f"SELECT item_entry FROM game_mail WHERE recipient_guid = {owner} AND subject = '{ITEM}:1900:3'"
    assert not host.rows(expired_mail, west)
    host.call('debug_auction_due', [expiring])
    host.wait_until(lambda: host.rows(expired_mail, west) == [[ITEM]], 'expiry did not follow the seller through Transfer')
    assert not host.rows(f'SELECT id FROM game_auction WHERE id = {expiring}')
    for client, guid in ((seller, owner), (bidder, buyer), (rival, competitor)):
        remove(client, guid, received)
