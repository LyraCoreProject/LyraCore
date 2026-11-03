import struct

from realm_petition import enter, insert, remove, settled, transfer


def check(host, worlds, character, received):
    east, west = worlds
    box = 0xf110000000000b01
    for index, shard in enumerate(worlds):
        host.call('debug_stage_meeting_stone', [5090611, box, 1, 60, 1581, index, 1, 0, 0], database=shard)
        host.sql('UPDATE game_gameobject_template SET type_id = 19 WHERE entry = 5090611', shard)
        result = host.sql('SELECT * FROM game_item_template WHERE entry = 51', shard)[0]
        row = dict(zip([column['name']['some'] for column in result['schema']['elements']], result['rows'][0]))
        row.update(entry=8383, name='Plain Letter', max_stack=1, max_count=0, bonding=0, inventory_type=0)
        insert(host, shard, 'game_item_template', row)
    _, first, sender = character('Realmletter', 0)
    _, second, recipient = character('Realmreader', 1)
    for client, guid, shard in [(first, sender, east), (second, recipient, west)]:
        settled(host, shard, guid)
        host.call('debug_set_money', [guid, 5000], database=shard)
        client.collect()
    host.wait_until(lambda: host.rows(f'SELECT guid FROM game_character_profile WHERE guid = {recipient}', east), 'recipient profile missing')

    def send(client, name, subject, body='', money=0, cod=0, item=0):
        client.send(0x238, struct.pack('<Q', box) + name.encode() + b'\0' + subject.encode() + b'\0' + body.encode()
            + b'\0' + struct.pack('<IIQIIII', 0, 0, item, money, cod, 0, 0))
        assert (0x239, bytes(12)) in received(client, 0x239)

    def letter(shard, subject):
        rows = host.wait_until(lambda: host.rows(f"SELECT id FROM game_mail WHERE subject = '{subject}'", shard), 'Mail did not arrive')
        assert len(rows) == 1
        return rows[0][0]

    def request(client, opcode, mail, action, code=0, extra=b''):
        client.send(opcode, struct.pack('<QI', box, mail) + (bytes(4) if opcode == 0x24a else b''))
        assert (0x239, struct.pack('<III', mail, action, code) + extra) in received(client, 0x239)

    def text(client, text_id, hint=0):
        client.send(0x243, struct.pack('<IQ', text_id, hint))
        return received(client, 0x244)[-1][1]

    def money(shard, guid):
        return host.rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}', shard)[0][0]

    second.send(0x4b)
    received(second, 0x4d)
    send(first, 'rEaLmReAdEr', 'offline money', 'private across Shards', money=100)
    original = letter(west, 'offline money')
    assert money(east, sender) == 4870
    assert not host.rows("SELECT id FROM game_mail WHERE subject = 'offline money'", east)
    enter(second, recipient)
    settled(host, west, recipient)
    assert text(second, original) == struct.pack('<I', original) + b'private across Shards\0'
    assert text(first, original) == struct.pack('<I', original) + b'\0'
    request(second, 0x245, original, 1)
    assert money(west, recipient) == 5100
    request(second, 0x245, original, 1, 6)
    request(second, 0x24a, original, 5)
    copy, old_text = host.rows(f'SELECT guid, item_text_id FROM game_item_instance WHERE owner_guid = {recipient} AND entry = 8383', west)[0]
    assert text(second, old_text, copy) == struct.pack('<I', old_text) + b'private across Shards\0'

    host.call('debug_grant_item', [sender, 5090050, 1], database=east)
    item = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {sender} AND entry = 5090050', east)[0][0]
    host.sql(f'UPDATE game_item_instance SET enchant_id = 7745, durability = 11, random_property_id = 1900, soulbound = false WHERE guid = {item}', east)
    first.collect()
    send(first, 'Realmreader', 'delivery: realm cod', item=item, cod=250)
    cod_mail = letter(west, 'delivery: realm cod')
    assert not host.rows(f'SELECT guid FROM game_item_instance WHERE guid = {item}', east)
    request(second, 0x246, cod_mail, 2, 6, bytes(8))
    host.call('debug_deliver_mail_fixture', [recipient, 'delivery: realm cod'], database=west)
    host.call('debug_set_money', [recipient, 249], database=west)
    second.collect()
    request(second, 0x246, cod_mail, 2, 3, bytes(8))
    host.call('debug_set_money', [recipient, 5100], database=west)
    second.collect()
    request(second, 0x246, cod_mail, 2, 0, struct.pack('<II', 5090050, 1))
    assert money(west, recipient) == 4850
    assert host.rows(f'SELECT enchant_id, durability, random_property_id FROM game_item_instance WHERE owner_guid = {recipient} AND entry = 5090050', west) == [[7745, 11, 1900]]
    payment = letter(east, 'delivery: realm cod')
    assert host.rows(f'SELECT money, check_flags FROM game_mail WHERE id = {payment}', east) == [[250, 8]]
    request(first, 0x245, payment, 1)
    assert money(east, sender) == 5090

    send(first, 'Realmreader', 'return across Shards', money=10)
    back = letter(west, 'return across Shards')
    request(second, 0x248, back, 3)
    returned = letter(east, 'return across Shards')
    assert host.rows(f'SELECT recipient_guid, sender_guid, money, check_flags FROM game_mail WHERE id = {returned}', east) == [[sender, recipient, 10, 2]]
    request(first, 0x248, returned, 3, 6)

    host.call('debug_grant_item', [sender, 5090050, 1], database=east)
    delayed_item = host.rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {sender} AND entry = 5090050', east)[0][0]
    send(first, 'Realmreader', 'delivery: carried delay', item=delayed_item, money=25)
    delayed_mail = letter(west, 'delivery: carried delay')
    deliver_at = host.rows(f'SELECT deliver_micros FROM game_mail WHERE id = {delayed_mail}', west)[0][0]

    host.sql(f"INSERT INTO game_item_text (id,text,letters) VALUES ({old_text},'unrelated destination text',4294967295)", east)
    host.call('operator_remove_peer_shard', [host.identity()], database=west)
    send(first, 'Realmreader', 'during Transfer', money=50)
    host.wait_until(lambda: host.rows('SELECT id FROM game_mail_forward_retry'), 'refused destination was not retained for retry')
    transfer(host, second, recipient, west, 0, received)
    crossing = letter(east, 'during Transfer')
    assert host.rows(f'SELECT money FROM game_mail WHERE id = {crossing}', east) == [[50]]
    assert not host.rows("SELECT id FROM game_mail WHERE subject = 'during Transfer'", west)
    host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=west)
    host.wait_until(lambda: not host.rows('SELECT id FROM game_mail_forward_retry'), 'Mail retry did not drain')
    carried = letter(east, 'offline money')
    new_text = host.rows(f'SELECT item_text_id FROM game_item_instance WHERE guid = {copy}', east)[0][0]
    assert new_text != old_text
    assert text(second, new_text, copy) == struct.pack('<I', new_text) + b'private across Shards\0'
    assert host.rows(f'SELECT text FROM game_item_text WHERE id = {old_text}', east) == [['unrelated destination text']]
    assert host.rows(f'SELECT mail_id FROM game_mail_timer WHERE mail_id = {carried}', east) == [[carried]]
    assert not host.rows(f'SELECT id FROM game_item_text WHERE id = {old_text}', west)
    moved_delay = letter(east, 'delivery: carried delay')
    assert host.rows(f'SELECT deliver_micros FROM game_mail WHERE id = {moved_delay}', east) == [[deliver_at]]
    assert host.rows(f'SELECT mail_id FROM game_mail_timer WHERE mail_id = {moved_delay}', east) == [[moved_delay]]
    request(second, 0x245, moved_delay, 1, 6)
    host.call('debug_deliver_mail_fixture', [recipient, 'delivery: carried delay'], database=east)
    request(second, 0x245, moved_delay, 1)

    transfer(host, second, recipient, east, 1, received)
    send(second, 'Realmletter', 'delivery: copied letter', item=copy)
    attached = letter(east, 'delivery: copied letter')
    host.call('debug_deliver_mail_fixture', [sender, 'delivery: copied letter'], database=east)
    request(first, 0x246, attached, 2, 0, struct.pack('<II', 8383, 1))
    received_copy, received_text = host.rows(f'SELECT guid, item_text_id FROM game_item_instance WHERE owner_guid = {sender} AND entry = 8383', east)[0]
    assert text(first, received_text, received_copy) == struct.pack('<I', received_text) + b'private across Shards\0'
    _, witness, witness_guid = character('Mailwitness', 1)
    assert text(witness, received_text, received_copy) == struct.pack('<I', received_text) + b'\0'
    settled(host, west, witness_guid)
    remove(witness, witness_guid, received)
    for client, guid in [(first, sender), (second, recipient)]:
        remove(client, guid, received)
