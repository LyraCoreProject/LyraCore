import struct


def check_operator_items(host):
    wearer = host.enter_world('Itemwearer', cls=8, at=(0, 20, 0, 0))
    observer = host.enter_world('Itemobserver', at=(0, 30, 0, 0))
    host.account('ITEMSELECT')
    selection = host.connect('ITEMSELECT')
    assert selection.read()[0] == 0x1ee
    template = host.sql('SELECT * FROM game_item_template WHERE entry = 25')[0]
    fields = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    del fields['entry']
    fields.update(name='Operator tunic', **{'class': 4, 'subclass': 1, 'inventory_type': 5})
    host.restore('game_item_template', 'entry >= 5200500 AND entry <= 5999999')
    wearer.collect()
    observer.collect()
    selection.collect()
    host.call('operator_apply_item_edits', [[{'createItem': {'fields': fields}}]])
    entry = host.rows("SELECT entry FROM game_item_template WHERE name = 'Operator tunic'")[0][0]

    def response(client, name):
        packets = [body for opcode, body in client.collect() if opcode == 0x58]
        assert len(packets) == 1, packets
        assert struct.unpack_from('<I', packets[0])[0] == entry, packets
        assert packets[0][12:].split(b'\0', 1)[0] == name.encode(), packets

    response(wearer, 'Operator tunic')
    response(observer, 'Operator tunic')
    response(selection, 'Operator tunic')
    for client in [wearer, observer]:
        host.call('debug_grant_item', [client.guid, entry, 1])
    slot = host.rows(f'SELECT slot FROM game_item_instance WHERE owner_guid = {wearer.guid} AND entry = {entry}')[0][0]
    host.call('debug_equip_item', [wearer.guid, slot])
    columns = 'max_health,max_power,spirit,sheet_str_bonus,sheet_sta_bonus,sheet_spi_bonus'
    before = host.rows(f'SELECT {columns} FROM game_world_entity WHERE guid = {wearer.guid}')[0]
    observer_before = host.rows(f'SELECT {columns} FROM game_world_entity WHERE guid = {observer.guid}')[0]
    durability = host.rows(f'SELECT durability FROM game_item_instance WHERE owner_guid = {wearer.guid} AND entry = {entry}')[0][0]
    wearer.collect()
    observer.collect()
    fields.update(name='Revised tunic', stat_strength=15, stat_stamina=20, stat_intellect=11, stat_spirit=7, max_durability=999)
    update = {'updateItem': {'entry': entry, 'fields': fields}}
    intermediate = {'updateItem': {'entry': entry, 'fields': dict(fields, name='Intermediate tunic')}}
    host.call('operator_apply_item_edits', [[intermediate, update]])
    after = host.rows(f'SELECT {columns} FROM game_world_entity WHERE guid = {wearer.guid}')[0]
    assert after[0] > before[0], (before, after)
    assert after[1] > before[1], (before, after)
    assert after[2] == before[2] + 7, (before, after)
    assert after[3:] == [15, 20, 7], after
    assert host.rows(f'SELECT {columns} FROM game_world_entity WHERE guid = {observer.guid}')[0] == observer_before
    assert host.rows(f'SELECT durability FROM game_item_instance WHERE owner_guid = {wearer.guid} AND entry = {entry}')[0][0] == durability
    response(wearer, 'Revised tunic')
    response(observer, 'Revised tunic')
    response(selection, 'Revised tunic')

    fields['stat_stamina'] = 40
    try:
        host.call('operator_apply_item_edits', [[update, {'updateItem': {'entry': 7000000, 'fields': fields}}]])
    except RuntimeError as refusal:
        assert 'item edit 1' in str(refusal), refusal
    else:
        raise AssertionError('a protected item update succeeded')
    assert host.rows(f'SELECT {columns} FROM game_world_entity WHERE guid = {wearer.guid}')[0] == after
    for client in [wearer, observer, selection]:
        assert not [body for opcode, body in client.collect() if opcode == 0x58]
