import struct


def check_eventai_speech(host):
    from check import spawn, terrain_chunk

    entry = 5099700
    for table, condition in [
        ('game_creature_template', f'entry = {entry}'),
        ('game_creature_ai_definition', f'creature_entry = {entry}'),
        ('game_creature_ai_broadcast_text', f'id >= {entry} AND id <= {entry + 2}'),
        ('game_area', f'id >= {entry} AND id <= {entry + 1}'),
    ]:
        host.restore(table, condition)
    host.sql(f"INSERT INTO game_creature_template (entry,name,subname,display_id,level,health,faction_template,npc_flags,unit_flags,creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,money_min,money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class) VALUES ({entry},'Speech fixture','',100,10,152,35,0,0,1,0,0,0,1,2000,0,0,10,152,0,1,1,0,0,0,0,0)")
    for offset, message in [(1, 'Arrival'), (2, 'Death yell')]:
        host.sql(f"INSERT INTO game_creature_ai_broadcast_text (id,male_text,female_text,chat_type,language_id,emote_delay_1_ms,emote_id_1,emote_delay_2_ms,emote_id_2,emote_delay_3_ms,emote_id_3,sound_id) VALUES ({entry + offset},'{message}','',6,0,0,0,0,0,0,0,{entry + offset})")
    host.call('import_creature_ai_definitions_append', [
        'entry:5099700@18275307164375685540@'
        '5099701,spawn:always,100,4294967295,once,all,ordinary,any-posture,speak:zone-yell:self:5099701~'
        '5099702,death:always,100,4294967295,once,all,ordinary,any-posture,speak:zone-yell:self:5099702'
    ])
    for zone in [entry, entry + 1]:
        host.sql(f"INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES ({zone},0,0,0,0,0,0,'Speech zone')")
    for x, zone in [(8010, entry), (9010, entry), (10010, entry + 1)]:
        terrain_chunk(host, int((17066.666 - x) / (533.3333 / 16)), 271, zone)
    near = host.enter_world('Speechnear', at=(0, 8010, 8010, 0))
    far = host.enter_world('Speechfar', at=(0, 9010, 8010, 0))
    other = host.enter_world('Speechother', at=(0, 10010, 8010, 0))
    for client in [near, far, other]:
        client.collect()
    creature = spawn(host, near.guid, entry, 1)

    def receives(client, sound, message=None):
        packets = client.collect()
        assert (0x2d2, struct.pack('<I', sound)) in packets, packets
        if message:
            assert any(opcode == 0x96 and message in body for opcode, body in packets), packets

    receives(near, entry + 1, b'Arrival')
    receives(far, entry + 1, b'Arrival')
    assert not any(opcode == 0x2d2 for opcode, _ in other.collect())
    host.call('debug_kill_creature', [near.guid, creature])
    receives(near, entry + 2, b'Death yell')
    receives(far, entry + 2, b'Death yell')
    assert not any(opcode == 0x2d2 for opcode, _ in other.collect())

    for client, instance in [(near, 1), (far, 1), (other, 2)]:
        host.sql(f'UPDATE game_world_entity SET map_id = 509, instance_id = {instance} WHERE guid = {client.guid}')
    spawn(host, near.guid, entry, 1)
    receives(near, entry + 1)
    receives(far, entry + 1)
    assert not any(opcode == 0x2d2 for opcode, _ in other.collect())
