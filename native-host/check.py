#!/usr/bin/env python3
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import secrets
import socket
import struct
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
KEY = bytes([0x57] * 40)
PRIME = bytes.fromhex('b79b3e2a87823cab8f5ebfbf8eb10108535006298b5badbd5b53e1895e644b89')


def sha(*parts):
    return hashlib.sha1(b''.join(parts)).digest()


def verifier(username, password, salt):
    secret = int.from_bytes(sha(salt, sha(f'{username.upper()}:{password.upper()}'.encode())), 'little')
    return pow(7, secret, int.from_bytes(PRIME, 'little')).to_bytes(32, 'little')


class Logon:
    def __init__(self, port):
        self.socket = socket.create_connection(('127.0.0.1', port), timeout=10)

    def challenge(self, username, build=5875):
        self.username = username.upper()
        body = struct.pack('<I3BHIIIi', 0x576f57, 1, 12, 1, build, 0x783836, 0x57696e, 0x656e5553, 0)
        body += bytes([127, 0, 0, 1, len(username)]) + username.encode()
        self.socket.sendall(bytes([0, 3]) + struct.pack('<H', len(body)) + body)
        header = read_exact(self.socket, 3)
        assert header[:2] == bytes(2), header.hex()
        if header[2]:
            return header[2]
        self.public = read_exact(self.socket, 32)
        assert read_exact(self.socket, 2) == bytes([1, 7])
        assert read_exact(self.socket, 33) == bytes([32]) + PRIME
        self.salt = read_exact(self.socket, 32)
        read_exact(self.socket, 16)
        assert read_exact(self.socket, 1) == bytes(1)
        return 0

    def prove(self, password):
        prime = int.from_bytes(PRIME, 'little')
        private = int.from_bytes(secrets.token_bytes(32), 'little')
        public = pow(7, private, prime).to_bytes(32, 'little')
        secret = int.from_bytes(sha(self.salt, sha(f'{self.username}:{password.upper()}'.encode())), 'little')
        scrambling = int.from_bytes(sha(public, self.public), 'little')
        shared = pow((int.from_bytes(self.public, 'little') - 3 * pow(7, secret, prime)) % prime, private + scrambling * secret, prime).to_bytes(32, 'little')
        zeros = len(shared) - len(shared.lstrip(b'\0'))
        shared = shared[zeros + zeros % 2:]
        key = bytes(value for pair in zip(sha(shared[::2]), sha(shared[1::2])) for value in pair)
        xor = bytes(left ^ right for left, right in zip(sha(PRIME), sha(bytes([7]))))
        proof = sha(xor, sha(self.username.encode()), self.salt, public, self.public, key)
        self.socket.sendall(bytes([1]) + public + proof + bytes(22))
        opcode, status = read_exact(self.socket, 2)
        assert opcode == 1
        if status:
            return status, None
        assert read_exact(self.socket, 20) == sha(public, proof, key)
        assert read_exact(self.socket, 4) == bytes(4)
        return 0, key

    def realms(self):
        self.socket.sendall(bytes([0x10]) + bytes(4))
        opcode, size = struct.unpack('<BH', read_exact(self.socket, 3))
        assert opcode == 0x10
        return read_exact(self.socket, size)

    def close(self):
        self.socket.close()


class Cipher:
    def __init__(self, key):
        self.key = key
        self.index = 0
        self.previous = 0

    def encrypt(self, data):
        out = bytearray()
        for value in data:
            value = ((value ^ self.key[self.index]) + self.previous) & 255
            out.append(value)
            self.previous = value
            self.index = (self.index + 1) % len(self.key)
        return bytes(out)

    def decrypt(self, data):
        out = bytearray()
        for value in data:
            out.append(((value - self.previous) & 255) ^ self.key[self.index])
            self.previous = value
            self.index = (self.index + 1) % len(self.key)
        return bytes(out)


def read_exact(sock, count):
    out = bytearray()
    while len(out) < count:
        data = sock.recv(count - len(out))
        if not data:
            raise EOFError('native socket closed')
        out.extend(data)
    return bytes(out)


class World:
    def __init__(self, port, username='NATIVE', build=5875, valid_proof=True, key=KEY):
        self.socket = socket.create_connection(('127.0.0.1', port), timeout=10)
        self.encrypt = Cipher(key)
        self.decrypt = Cipher(key)
        header = read_exact(self.socket, 4)
        assert header == bytes([0, 6, 0xec, 1]), header.hex()
        server_seed = read_exact(self.socket, 4)
        client_seed = secrets.token_bytes(4)
        proof = sha(username.encode(), bytes(4), client_seed, server_seed, key)
        if not valid_proof:
            proof = bytes(20)
        body = struct.pack('<II', build, 0) + username.encode() + b'\0' + client_seed + proof + bytes(4)
        self.socket.sendall(struct.pack('>H', len(body) + 4) + struct.pack('<I', 0x1ed) + body)

    def read(self, encrypted=True):
        header = read_exact(self.socket, 4)
        if encrypted:
            header = self.decrypt.decrypt(header)
        size = struct.unpack('>H', header[:2])[0]
        opcode = struct.unpack('<H', header[2:])[0]
        assert size >= 2
        return opcode, read_exact(self.socket, size - 2)

    def send(self, opcode, body=b''):
        header = struct.pack('>H', len(body) + 4) + struct.pack('<I', opcode)
        self.socket.sendall(self.encrypt.encrypt(header) + body)

    def collect(self, auto_ack=True):
        sequence = secrets.randbits(32)
        self.send(0x1dc, struct.pack('<II', sequence, 0))
        packets = []
        while True:
            packet = self.read()
            if packet == (0x1dd, struct.pack('<I', sequence)):
                return packets
            if packet[0] != 0x1dd:
                packets.append(packet)
            if auto_ack and packet[0] in (0xc7, 0x3e):
                if packet[0] == 0xc7:
                    self.send(0xc7, teleport_ack(packet[1]))
                else:
                    self.send(0xdc)
                sequence = secrets.randbits(32)
                self.send(0x1dc, struct.pack('<II', sequence, 0))

    def close(self):
        self.socket.close()


def ignored_requests(client):
    for opcode in [0x0008, 0x0009, 0x00c6, 0x00d9, 0x00e1, 0x02ad, 0x02ca, 0x02f8,
                   0x000a, 0x0020, 0x01f4, 0x020b, 0x022f, 0x0331, 0x0334]:
        client.send(opcode, bytes([0xff]) * 32)
    assert client.collect() == []


def teleport_ack(body):
    counter = struct.unpack_from('<I', body, 1 + body[0].bit_count())[0]
    return struct.pack('<QII', packed_guid(body), counter, 0)


def packed_guid(body):
    mask, body = body[0], iter(body[1:])
    return int.from_bytes(bytes(next(body) if mask & (1 << index) else 0 for index in range(8)), 'little')


def cast(spell, target=0):
    if not target:
        return struct.pack('<IH', spell, 0)
    raw = target.to_bytes(8, 'little')
    mask = sum(1 << index for index, value in enumerate(raw) if value)
    return struct.pack('<IH', spell, 2) + bytes([mask]) + bytes(value for value in raw if value)


def spell_id(body):
    for _ in range(2):
        body = body[1 + body[0].bit_count():]
    return struct.unpack_from('<I', body)[0]


def spell_go_misses(body):
    for _ in range(2):
        body = body[1 + body[0].bit_count():]
    offset = 7 + body[6] * 8
    return [struct.unpack_from('<QB', body, offset + 1 + index * 9)
            for index in range(body[offset])]


def creates(packets):
    return [packed_guid(body[6:]) for opcode, body in packets if opcode == 0xa9 and body[5] in (2, 3)]


def destroys(packets):
    return [struct.unpack('<Q', body)[0] for opcode, body in packets if opcode == 0xaa]


def values(packets, guid):
    fields = {}
    for opcode, body in packets:
        if opcode != 0xa9 or body[:6] != struct.pack('<IBB', 1, 0, 0) or packed_guid(body[6:]) != guid:
            continue
        offset = 7 + body[6].bit_count()
        words = body[offset]
        mask = int.from_bytes(body[offset + 1:offset + 1 + words * 4], 'little')
        offset += 1 + words * 4
        for index in range(words * 32):
            if mask & (1 << index):
                fields[index] = struct.unpack_from('<I', body, offset)[0]
                offset += 4
        assert offset == len(body)
    return fields


def item_pointer(fields, slot):
    # Build 5875 PLAYER_FIELD_INV_SLOT_HEAD is descriptor 486.
    index = 486 + 2 * slot
    return fields[index] | fields[index + 1] << 32


def check_death(client, peer, call, sql, guid, peer_guid):
    corpse = 0xf101_0000_0000_0000 | guid
    sql("INSERT INTO game_graveyard (id,map_id,x,y,z,o,name) VALUES (5090401,0,20,0,0,0,'Native graveyard')")
    sql('UPDATE game_creature_template SET npc_flags = 32 WHERE entry = 51004')
    call('debug_spawn_at_feet', [guid, 51004, 2])
    healer = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51004')[0]['rows'])
    assert healer not in creates(client.collect())
    assert healer not in creates(peer.collect())
    client.send(0x216)
    assert (0x216, b'\0') in client.collect()
    call('debug_set_health', [guid, 0])
    dying = client.collect()
    assert values(dying, guid)[22] == 0
    assert not any(opcode == 0x269 for opcode, _ in dying)
    client.send(0x15a)
    released = client.collect()
    assert corpse in creates(released)
    assert healer in creates(released)
    delay = next(struct.unpack('<I', body)[0] for opcode, body in released if opcode == 0x269)
    assert 29000 <= delay <= 30000, (delay, sql(f'SELECT reclaim_delay_micros, created_at FROM game_corpse WHERE guid = {corpse}'))
    observed = peer.collect()
    assert corpse in creates(observed)
    assert healer not in creates(observed)
    assert not any(opcode == 0x269 for opcode, _ in observed)
    client.send(0x15a)
    assert corpse not in creates(client.collect())
    client.send(0x216)
    assert (0x216, struct.pack('<BIfffI', 1, 0, 20, 0, 0, 0)) in client.collect()
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    sql(f'UPDATE game_corpse SET reclaim_delay_micros = 0 WHERE guid = {corpse}')
    sql(f'UPDATE game_corpse SET x = 250, cell = 0 WHERE guid = {corpse}')
    call('debug_backfill_cell_ids', [])
    assert corpse in destroys(client.collect())
    assert corpse in destroys(peer.collect())
    assert sql(f'SELECT cell FROM game_corpse WHERE guid = {corpse}')[0]['rows'][0][0] != 0
    sql(f'UPDATE game_corpse SET x = 20, cell = 1 WHERE guid = {corpse}')
    call('debug_backfill_cell_ids', [])
    assert corpse in creates(client.collect())
    assert corpse in creates(peer.collect())
    call('debug_create_fixture_instance', [guid])
    assert corpse in destroys(client.collect())
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    call('debug_enter_instance', [guid, 0])
    assert corpse in creates(client.collect())
    call('debug_teleport', [guid, 0, 250, 0, 0, 0])
    assert corpse in destroys(client.collect())
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    call('debug_teleport', [guid, 1, 20, 0, 0, 0])
    assert corpse not in creates(client.collect())
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    assert corpse in creates(client.collect())
    client.send(0x4b)
    client.collect()
    assert sql(f'SELECT guid FROM game_corpse WHERE guid = {corpse}')[0]['rows']
    client.send(0x3d, struct.pack('<Q', guid))
    relogged = client.collect()
    assert corpse in creates(relogged)
    assert healer in creates(relogged)
    assert (0x269, bytes(4)) in relogged
    peer.collect()
    client.send(0x1d2, struct.pack('<Q', corpse))
    reclaimed = client.collect()
    assert corpse in destroys(reclaimed)
    assert healer in destroys(reclaimed)
    assert corpse in destroys(peer.collect())
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[False]]
    assert not sql(f'SELECT guid FROM game_corpse WHERE guid = {corpse}')[0]['rows']
    for spell, kind, points, target, parameter in [(51991, 14, 40, 3, 0), (51992, 38, 40, 0, 0), (51993, 181, 0, 0, 51992)]:
        sql(f"INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES ({spell},'Native resurrection',0,0,0,0,0,100,60000,1,0,0,1,0,0,0,0,false,0,0,0,0,0,0,0)")
        sql(f'INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES ({spell * 4},{spell},0,{kind},{points},0,0.0,0,{target},0.0,0,0,0,{parameter},15,0,0,false)')
    call('debug_learn_spell', [peer_guid, 51991])
    call('debug_set_health', [guid, 0])
    client.collect()
    peer.collect()
    peer.send(0x12e, cast(51991, guid))
    assert not any(opcode == 0x15b for opcode, _ in peer.collect())
    offered = client.collect()
    offer = next(body for opcode, body in offered if opcode == 0x15b)
    assert struct.unpack_from('<Q', offer)[0] == peer_guid
    name_length = struct.unpack_from('<I', offer, 8)[0]
    assert offer[12 + name_length:] == bytes(2)
    client.send(0x15c, struct.pack('<QB', guid, 1))
    client.collect()
    assert sql(f'SELECT target_guid FROM game_resurrect_request WHERE target_guid = {guid}')[0]['rows']
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    client.send(0x15c, struct.pack('<QB', peer_guid, 0))
    client.collect()
    assert not sql(f'SELECT target_guid FROM game_resurrect_request WHERE target_guid = {guid}')[0]['rows']
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    peer.send(0x12e, cast(51991, guid))
    peer.collect()
    assert any(opcode == 0x15b for opcode, _ in client.collect())
    client.send(0x15c, struct.pack('<QB', peer_guid, 1))
    client.collect()
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[False]]
    assert not sql(f'SELECT target_guid FROM game_resurrect_request WHERE target_guid = {guid}')[0]['rows']
    call('debug_force_cast', [guid, 51993])
    client.collect()
    peer.collect()
    call('debug_set_health', [guid, 0])
    assert values(client.collect(), guid)[1224] == 51992
    assert 1224 not in values(peer.collect(), guid)
    client.send(0x2b3)
    self_resurrected = client.collect()
    assert values(self_resurrected, guid)[1224] == 0
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[False]]
    assert not sql(f'SELECT character_guid FROM game_self_resurrect_option WHERE character_guid = {guid}')[0]['rows']
    call('debug_set_health', [guid, 0])
    client.send(0x15a)
    client.collect()
    peer.collect()
    grant = sql(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {guid} AND layer = 2 AND entity_guid = {corpse}')[0]['rows']
    assert len(grant) == 1
    call('debug_age_corpse', [corpse, 310])
    bones = client.collect()
    assert values(bones, corpse)[35] == 1
    assert (0x216, b'\0') in bones
    assert values(peer.collect(), corpse)[35] == 1
    assert sql(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {guid} AND layer = 2 AND entity_guid = {corpse}')[0]['rows'] == grant
    client.send(0x216)
    assert (0x216, b'\0') in client.collect()
    client.send(0x1d2, struct.pack('<Q', corpse))
    client.collect()
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    call('debug_age_corpse', [corpse, 610])
    assert corpse in destroys(client.collect())
    assert corpse in destroys(peer.collect())
    assert not sql(f'SELECT id FROM game_aoi_grant WHERE layer = 2 AND entity_guid = {corpse}')[0]['rows']
    call('debug_teleport', [guid, 0, 250, 0, 0, 0])
    client.collect()
    client.send(0x21c, struct.pack('<Q', healer))
    client.collect()
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    client.collect()
    client.send(0x21c, struct.pack('<Q', peer_guid))
    client.collect()
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[True]]
    client.send(0x17b, struct.pack('<Q', healer))
    assert (0x222, struct.pack('<Q', healer)) in client.collect()
    client.send(0x21c, struct.pack('<Q', healer))
    revived = client.collect()
    assert not any(opcode == 0x222 for opcode, _ in revived)
    assert healer in destroys(revived)
    assert sql(f'SELECT dead FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[False]]
    peer.collect()


def check_environment(client, peer, call, sql, guid, peer_guid):
    for zone in (12, 40):
        if not sql(f'SELECT id FROM game_area WHERE id = {zone}')[0]['rows']:
            sql(f"INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES ({zone},0,0,-1,0,0,0,'Native weather')")
    call('import_terrain_chunks_append', ['0,513,512,10,1,0,12,' + ':'.join(['0'] * 145)])
    sql('UPDATE game_terrain_chunk SET area_id = 40, liquid_level = 10.0, has_liquid = true WHERE cell_x = 512 AND cell_y = 512')
    call('debug_teleport', [guid, 0, -40, -1, 0, 0])
    call('debug_teleport', [peer_guid, 0, -40, -1, 0, 0])
    client.collect()
    peer.collect()
    call('gw_force_zone_weather', [12, 1, 0.5])
    assert (0x2f4, struct.pack('<IfIB', 1, 0.5, 8534, 0)) in client.collect()
    assert (0x2f4, struct.pack('<IfIB', 1, 0.5, 8534, 0)) in peer.collect()
    call('gw_force_zone_weather', [40, 2, 0.25])
    assert not any(opcode == 0x2f4 for opcode, _ in client.collect() + peer.collect())
    client.send(0xee, struct.pack('<IIffffI', 0, 100000, -40, -1, 0, 0, 0))
    client.collect()
    client.send(0xee, struct.pack('<IIffffI', 0, 102000, -31, -1, 0, 0, 0))
    crossing = client.collect()
    assert (0x2c2, struct.pack('<IIH', 0, 40, 0)) in crossing
    assert (0x2f4, struct.pack('<IfIB', 2, 0.25, 8536, 1)) in crossing
    assert not any(opcode in (0x2c2, 0x2f4) for opcode, _ in peer.collect())
    client.send(0x1f4, struct.pack('<I', 12))
    client.collect()
    assert sql(f'SELECT zone_id FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[40]]
    call('gw_force_zone_weather', [12, 3, 0.75])
    assert not any(opcode == 0x2f4 for opcode, _ in client.collect())
    assert (0x2f4, struct.pack('<IfIB', 3, 0.75, 8558, 0)) in peer.collect()
    call('gw_force_zone_weather', [40, 0, 0])
    assert (0x2f4, bytes(13)) in client.collect()
    assert not any(opcode == 0x2f4 for opcode, _ in peer.collect())
    client.send(0x4b)
    client.collect()
    call('gw_force_zone_weather', [40, 1, 0.5])
    assert not any(opcode == 0x2f4 for opcode, _ in client.collect() + peer.collect())
    client.send(0x3d, struct.pack('<Q', guid))
    entered = client.collect()
    assert (0x2f4, struct.pack('<IfIB', 1, 0.5, 8534, 1)) in entered
    assert [opcode for opcode, _ in entered if opcode in (0x2c2, 0x2f4)] == [0x2c2, 0x2f4]
    health = sql(f'SELECT max_health FROM game_world_entity WHERE guid = {guid}')[0]['rows'][0][0]
    call('debug_set_health', [guid, health])
    client.collect()
    peer.collect()
    client.send(0xee, struct.pack('<IIfffffI', 0x200000, 200000, -31, -1, 0, 0, 0, 0))
    assert (0x1d9, struct.pack('<IIIIBI', 1, 60000, 60000, 0xffffffff, 0, 0)) in client.collect()
    assert not any(opcode in (0x1d9, 0x1db, 0x1fc) for opcode, _ in peer.collect())
    sql(f'UPDATE game_breath_state SET remaining_air_micros = 100000 WHERE entity_guid = {guid}')
    drowned = []
    for _ in range(60):
        drowned += client.collect()
        if any(opcode == 0x1fc for opcode, _ in drowned):
            break
        time.sleep(0.05)
    assert (0x1fc, struct.pack('<QBIII', guid, 1, (health * 20 + 99) // 100, 0, 0)) in drowned
    assert values(drowned, guid)[22] < health
    assert not any(opcode in (0x1d9, 0x1db, 0x1fc) for opcode, _ in peer.collect())
    client.send(0xee, struct.pack('<IIffffI', 0, 201000, -31, -1, 0, 0, 0))
    assert (0x1db, struct.pack('<I', 1)) in client.collect()
    client.send(0xee, struct.pack('<IIfffffI', 0x200000, 202000, -31, -1, 0, 0, 0, 0))
    assert any(opcode == 0x1d9 for opcode, _ in client.collect())
    call('debug_set_health', [guid, 0])
    assert (0x1db, struct.pack('<I', 1)) in client.collect()
    assert not sql(f'SELECT entity_guid FROM game_breath_state WHERE entity_guid = {guid}')[0]['rows']
    call('debug_set_health', [guid, health])
    client.collect()
    client.send(0xee, struct.pack('<IIfffffI', 0x200000, 203000, -31, -1, 0, 0, 0, 0))
    assert any(opcode == 0x1d9 for opcode, _ in client.collect())
    client.send(0x4b)
    client.collect()
    assert not sql(f'SELECT entity_guid FROM game_breath_state WHERE entity_guid = {guid}')[0]['rows']
    client.send(0x3d, struct.pack('<Q', guid))
    assert not any(opcode == 0x1d9 for opcode, _ in client.collect())
    call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    client.collect()
    peer.collect()


def check_social(client, peer, call, sql, guid, peer_guid):
    def status(code, target, online=None):
        body = struct.pack('<BQ', code, target)
        if online is not None:
            body += struct.pack('<BIII', *online)
        return 0x68, body

    def list_packets(packets):
        return [packet for packet in packets if packet[0] in (0x67, 0x6b)]

    client.send(0x66)
    assert list_packets(client.collect()) == [(0x67, b'\0'), (0x6b, b'\0')]
    client.send(0x69, b'Missingfriend\0')
    assert status(4, 0) in client.collect()
    client.send(0x69, b'nAtIvE\0')
    assert status(9, guid) in client.collect()
    client.send(0x6c, b'Native\0')
    assert status(12, guid) in client.collect()
    level, zone = sql(f'SELECT level, zone_id FROM game_world_entity WHERE guid = {peer_guid}')[0]['rows'][0]
    client.send(0x69, b'pEeR\0')
    assert status(6, peer_guid, (1, zone, level, 8)) in client.collect()
    assert not any(opcode == 0x68 for opcode, _ in peer.collect())
    client.send(0x69, b'Peer\0')
    assert status(8, peer_guid) in client.collect()
    peer.send(0x95, struct.pack('<II', 20, 7) + b'AFK friend\0')
    peer.collect()
    client.send(0x66)
    friend = b'\1' + struct.pack('<QBIII', peer_guid, 2, zone, level, 8)
    assert list_packets(client.collect()) == [(0x67, friend), (0x6b, b'\0')]
    client.send(0x6c, b'Peer\0')
    assert status(15, peer_guid) in client.collect()
    client.send(0x6c, b'Peer\0')
    assert status(14, peer_guid) in client.collect()
    client.send(0x66)
    assert list_packets(client.collect()) == [(0x67, friend), (0x6b, b'\1' + struct.pack('<Q', peer_guid))]
    client.send(0x6d, struct.pack('<Q', peer_guid))
    assert status(16, peer_guid) in client.collect()
    client.send(0x6d, struct.pack('<Q', peer_guid))
    assert status(13, peer_guid) in client.collect()
    call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    peer.collect()
    assert not any(opcode == 0x68 for opcode, _ in client.collect())
    peer.send(0x4b)
    peer.collect()
    assert [packet for packet in client.collect() if packet[0] == 0x68] == [status(3, peer_guid)]
    client.send(0x66)
    assert list_packets(client.collect()) == [(0x67, b'\1' + struct.pack('<QB', peer_guid, 0)), (0x6b, b'\0')]
    client.send(0x6a, struct.pack('<Q', peer_guid))
    assert status(5, peer_guid) in client.collect()
    client.send(0x69, b'Peer\0')
    assert status(7, peer_guid) in client.collect()
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    level, zone = sql(f'SELECT level, zone_id FROM game_world_entity WHERE guid = {peer_guid}')[0]['rows'][0]
    assert [packet for packet in client.collect() if packet[0] == 0x68] == [status(2, peer_guid, (1, zone, level, 8))]
    client.send(0x6a, struct.pack('<Q', peer_guid))
    assert status(5, peer_guid) in client.collect()
    client.send(0x6a, struct.pack('<Q', peer_guid))
    assert status(4, peer_guid) in client.collect()
    for ignored, count, opcode, code in [(False, 50, 0x69, 1), (True, 25, 0x6c, 11)]:
        for index in range(count):
            sql(f"INSERT INTO game_character_contact (id,owner_guid,owner_identity,target_guid,is_ignore) VALUES ({5091000 + index},{guid},0x{'00' * 32},{900000 + index},{str(ignored).lower()})")
        client.send(opcode, b'Peer\0')
        assert status(code, peer_guid) in client.collect()
        assert len(sql(f'SELECT id FROM game_character_contact WHERE owner_guid = {guid}')[0]['rows']) == count
        sql(f'DELETE FROM game_character_contact WHERE owner_guid = {guid}')

    def who(name='', guild='', minimum=0, maximum=100, races=0xffffffff, classes=0xffffffff, zones=(), strings=()):
        body = struct.pack('<II', minimum, maximum) + name.encode() + b'\0' + guild.encode() + b'\0'
        body += struct.pack('<III', races, classes, len(zones)) + b''.join(struct.pack('<I', zone) for zone in zones)
        body += struct.pack('<I', len(strings)) + b''.join(value.encode() + b'\0' for value in strings)
        client.send(0x62, body)

    def who_names(packets):
        packets = [body for opcode, body in packets if opcode == 0x63]
        assert len(packets) == 1, packets
        body = packets[0]
        listed, total = struct.unpack_from('<II', body)
        assert listed == total
        rest = body[8:]
        names = []
        for _ in range(listed):
            name, rest = rest.split(b'\0', 1)
            _, rest = rest.split(b'\0', 1)
            names.append(name.decode())
            rest = rest[16:]
        assert not rest
        return sorted(names)

    who()
    who()
    assert who_names(client.collect()) == ['Native', 'Peer']
    time.sleep(1.05)
    who(name='pEE')
    assert who_names(client.collect()) == ['Peer']
    for arguments, expected in [({'classes': 1 << 8}, ['Peer']), ({'races': 1 << 2}, []), ({'guild': 'missing'}, []), ({'minimum': 99}, []), ({'strings': ('missing', 'pEE')}, ['Peer']), ({'zones': (0xffffff,)}, [])]:
        sql('UPDATE game_native_world_connection SET who_until_micros = 0')
        who(**arguments)
        actual = who_names(client.collect())
        assert actual == expected, (arguments, actual)
    for arguments in [{'zones': tuple(range(11))}, {'strings': ('a', 'b', 'c', 'd', 'e')}]:
        sql('UPDATE game_native_world_connection SET who_until_micros = 0')
        who(**arguments)
        assert not any(opcode == 0x63 for opcode, _ in client.collect())
    peer.send(0x4b)
    peer.collect()
    sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.send(0x69, b'Peer\0')
    assert status(10, peer_guid) in client.collect()
    client.send(0x6c, b'Peer\0')
    assert status(15, peer_guid) in client.collect()
    client.send(0x6d, struct.pack('<Q', peer_guid))
    client.collect()
    sql('UPDATE game_native_world_connection SET who_until_micros = 0')
    who()
    assert who_names(client.collect()) == ['Native']
    peer.send(0x4b)
    peer.collect()
    sql(f'UPDATE game_character SET race = 1 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    client.collect()


def check_guild(client, peer, call, sql, guid, peer_guid):
    def command(operation, name, result):
        return 0x93, struct.pack('<I', operation) + name.encode() + b'\0' + struct.pack('<I', result)

    def event(kind, strings=(), subject=0):
        body = bytes([kind, len(strings)]) + b''.join(s.encode() + b'\0' for s in strings)
        return 0x92, body + (struct.pack('<Q', subject) if subject else b'')

    def roster(packets):
        body = next(body for opcode, body in packets if opcode == 0x8a)
        count = struct.unpack_from('<I', body)[0]
        offset = 4

        def string():
            nonlocal offset
            end = body.index(0, offset)
            result = body[offset:end].decode()
            offset = end + 1
            return result

        motd, info = string(), string()
        ranks = struct.unpack_from('<I', body, offset)[0]
        offset += 4
        rights = struct.unpack_from('<' + 'I' * ranks, body, offset)
        offset += 4 * ranks
        members = {}
        for _ in range(count):
            member, online = struct.unpack_from('<QB', body, offset)
            offset += 9
            name = string()
            rank, level, cls, zone = struct.unpack_from('<IBBI', body, offset)
            offset += 10
            days = None if online else struct.unpack_from('<f', body, offset)[0]
            offset += 0 if online else 4
            members[member] = (name, online, rank, level, cls, zone, days, string(), string())
        assert offset == len(body)
        return motd, info, rights, members

    def invite():
        client.send(0x82, b'pEeR\0')
        client.collect()
        assert (0x83, b'Native\0Native Guild\0') in peer.collect()

    def accept():
        peer.send(0x84)
        packets = peer.collect()
        assert event(3, ['Peer'], peer_guid) in packets
        assert values(packets, peer_guid)[191] == guild_id
        assert values(packets, peer_guid)[192] == 4
        return client.collect()

    call('debug_teleport', [guid, 0, 20, 0, 0, 0])
    call('debug_teleport', [peer_guid, 0, 21, 0, 0, 0])
    client.collect()
    peer.collect()
    client.send(0x54, struct.pack('<I', 999999))
    assert command(0, '', 9) in client.collect()
    peer.send(0x81, b'Forbidden Guild\0')
    assert not any(op in (0x55, 0x93) for op, _ in peer.collect())
    assert not sql(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {peer_guid}')[0]['rows']
    gm = sql(f'SELECT gm_level FROM game_character WHERE guid = {guid}')[0]['rows'][0][0]
    call('set_gm_level', ['Native', 1])
    client.send(0x81, b'Temporary Guild\0')
    assert values(client.collect(), guid)[191] != 0
    peer.collect()
    client.send(0x8f)
    assert event(8) in client.collect()
    peer.collect()
    client.send(0x95, struct.pack('<II', 0, 7) + b'.guild create Native "Native Guild"\0')
    created = client.collect()
    guild_id = sql(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {guid}')[0]['rows'][0][0]
    assert values(created, guid)[191] == guild_id
    assert values(peer.collect(), guid)[191] == guild_id
    client.send(0x54, struct.pack('<I', guild_id))
    query = next(body for opcode, body in client.collect() if opcode == 0x55)
    assert query.startswith(struct.pack('<I', guild_id) + b'Native Guild\0Guild Master\0Officer\0')
    client.send(0x82, b'Missingguildmember\0')
    assert command(1, 'Missingguildmember', 11) in client.collect()
    peer.send(0x6c, b'Native\0')
    peer.collect()
    client.send(0x82, b'Peer\0')
    client.collect()
    assert not any(opcode == 0x83 for opcode, _ in peer.collect())
    assert not sql(f'SELECT guild_id FROM game_guild_invite WHERE target_guid = {peer_guid}')[0]['rows']
    peer.send(0x6d, struct.pack('<Q', guid))
    peer.collect()
    invite()
    client.send(0x82, b'pEeR\0')
    assert command(1, 'Peer', 5) in client.collect()
    peer.send(0x85)
    peer.collect()
    assert (0x86, b'Peer\0') in client.collect()
    peer.send(0x4b)
    peer.collect()
    client.collect()
    client.send(0x82, b'Peer\0')
    assert command(1, 'Peer', 11) in client.collect()
    sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.collect()
    client.send(0x82, b'pEeR\0')
    assert command(1, 'pEeR', 12) in client.collect()
    peer.send(0x4b)
    peer.collect()
    client.collect()
    sql(f'UPDATE game_character SET race = 1 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.collect()
    invite()
    joined = accept()
    assert event(3, ['Peer'], peer_guid) in joined
    assert values(joined, peer_guid)[191] == guild_id
    client.send(0x82, b'pEeR\0')
    assert command(1, 'Peer', 3) in client.collect()
    client.send(0x8d)
    assert command(3, '', 8) in client.collect()
    peer.send(0x91, b'Forbidden MOTD\0')
    assert command(1, '', 8) in peer.collect()
    peer.send(0x2fc, b'Forbidden info\0')
    assert command(0, '', 8) in peer.collect()
    client.send(0x91, b'Native guild MOTD\0')
    assert event(2, ['Native guild MOTD']) in client.collect()
    assert event(2, ['Native guild MOTD']) in peer.collect()
    client.send(0x2fc, b'Native guild info\0')
    client.collect()
    client.send(0x234, b'pEeR\0Public note\0')
    assert roster(client.collect())[3][peer_guid][-2:] == ('Public note', '')
    client.send(0x235, b'Peer\0Private officer note\0')
    assert roster(client.collect())[3][peer_guid][-2:] == ('Public note', 'Private officer note')
    peer.send(0x89)
    hidden = roster(peer.collect())
    assert hidden[:2] == ('Native guild MOTD', 'Native guild info')
    assert hidden[3][peer_guid][-2:] == ('Public note', '')
    assert hidden[3][guid][1] == 1
    peer.send(0x235, b'Native\0Forbidden note\0')
    assert command(1, '', 8) in peer.collect()
    client.send(0x87)
    info = next(body for opcode, body in client.collect() if opcode == 0x88)
    assert struct.unpack_from('<II', info, len(info) - 8) == (2, 2)
    peer.send(0x232, b'Forbidden rank\0')
    assert command(1, '', 8) in peer.collect()
    client.send(0x232, b'New rank\0')
    assert len(roster(client.collect())[2]) == 6
    assert roster(peer.collect())[3][peer_guid][-1] == ''
    client.send(0x231, struct.pack('<II', 4, 0x4040) + b'Readers\0')
    assert len(roster(client.collect())[2]) == 6
    refreshed = peer.collect()
    assert [op for op, _ in refreshed if op in (0x55, 0x8a)] == [0x55, 0x8a]
    assert roster(refreshed)[3][peer_guid][-1] == 'Private officer note'
    client.send(0x231, struct.pack('<II', 4, 0x43) + b'Initiate\0')
    client.collect()
    assert roster(peer.collect())[3][peer_guid][-1] == ''
    client.send(0x233)
    assert len(roster(client.collect())[2]) == 5
    peer.collect()
    client.send(0x8b, b'pEeR\0')
    promoted = client.collect()
    assert event(0, ['Native', 'Peer', 'Member']) in promoted
    assert values(promoted, peer_guid)[192] == 3
    assert values(peer.collect(), peer_guid)[192] == 3
    client.send(0x8c, b'Peer\0')
    assert event(1, ['Native', 'Peer', 'Initiate']) in client.collect()
    assert values(peer.collect(), peer_guid)[192] == 4
    client.send(0x8c, b'Peer\0')
    assert command(1, 'Peer', 14) in client.collect()
    client.send(0x8b, b'Native\0')
    assert command(1, '', 6) in client.collect()
    peer.send(0x90, b'Peer\0')
    assert command(1, '', 8) in peer.collect()
    client.send(0x90, b'pEeR\0')
    assert event(7, ['Native', 'Peer']) in client.collect()
    leadership = peer.collect()
    assert values(leadership, peer_guid)[192] == 0
    assert values(leadership, guid)[192] == 1
    peer.send(0x90, b'Native\0')
    peer.collect()
    client.collect()
    peer.send(0x4b)
    peer.collect()
    assert event(13, ['Peer'], peer_guid) in client.collect()
    client.send(0x89)
    offline = roster(client.collect())[3][peer_guid]
    assert offline[1] == 0 and 0 <= offline[6] < 0.01
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    entered = peer.collect()
    assert event(2, ['Native guild MOTD']) in entered
    assert event(12, ['Peer'], peer_guid) not in entered
    assert event(12, ['Peer'], peer_guid) in client.collect()
    call('debug_teleport', [peer_guid, 1, 21, 0, 0, 0])
    peer.collect(auto_ack=False)
    assert not any(opcode == 0x92 for opcode, _ in client.collect())
    client.send(0x89)
    assert roster(client.collect())[3][peer_guid][1] == 1
    peer.send(0xdc)
    assert not any(opcode == 0x92 for opcode, _ in peer.collect())
    client.collect()
    client.send(0x91, b'Across maps\0')
    assert event(2, ['Across maps']) in client.collect()
    assert event(2, ['Across maps']) in peer.collect()
    peer.send(0x8d)
    departed = peer.collect()
    assert command(3, 'Native Guild', 0) in departed
    assert values(departed, peer_guid)[191] == 0
    assert event(4, ['Peer'], peer_guid) in client.collect()
    invite()
    accept()
    client.send(0x8e, b'pEeR\0')
    assert event(5, ['Peer', 'Native']) in client.collect()
    assert values(peer.collect(), peer_guid)[191] == 0
    invite()
    accept()
    peer_account = sql(f'SELECT account_id FROM game_character WHERE guid = {peer_guid}')[0]['rows'][0][0]
    call('create_character', [peer_account, 'Departed', 1, 1, 0, 0, 0, 0, 0, 0])
    departed_guid = sql("SELECT guid FROM game_character WHERE name = 'Departed'")[0]['rows'][0][0]
    call('realm_guild_op', [{'guid': guid, 'ownership': None}, {'invite': {
        'target_guid': departed_guid, 'actor_team': 469, 'target_team': 469,
        'target_ignores_actor': False}}])
    call('realm_guild_op', [{'guid': departed_guid, 'ownership': None}, {'accept': {
        'actor_name': 'Departed', 'actor_team': 469}}])
    client.collect()
    peer.collect()
    client.send(0x90, b'Departed\0')
    assert event(7, ['Native', 'Departed']) in client.collect()
    peer.collect()
    call('debug_delete_character', [departed_guid])
    succession = client.collect()
    assert event(7, ['Departed', 'Native']) in succession
    assert event(4, ['Departed'], departed_guid) in succession
    assert values(succession, guid)[192] == 0
    assert not sql(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {departed_guid}')[0]['rows']
    assert sql(f'SELECT leader_guid FROM game_guild WHERE guild_id = {guild_id}')[0]['rows'] == [[guid]]
    peer.collect()
    client.send(0x8f)
    assert event(8) in client.collect()
    disbanded = peer.collect()
    assert event(8) in disbanded
    assert values(disbanded, peer_guid)[191] == 0
    assert not sql(f'SELECT guild_id FROM game_guild WHERE guild_id = {guild_id}')[0]['rows']
    assert not sql(f'SELECT character_guid FROM game_guild_member WHERE guild_id = {guild_id}')[0]['rows']
    assert not sql(f'SELECT id FROM game_guild_rank WHERE guild_id = {guild_id}')[0]['rows']
    call('set_gm_level', ['Native', gm])
    call('debug_teleport', [peer_guid, 0, 21, 0, 0, 0])
    peer.collect()
    client.collect()


def check_petitions(client, peer, call, sql, guid, peer_guid):
    def command(operation, name, result):
        return 0x93, struct.pack('<I', operation) + name.encode() + b'\0' + struct.pack('<I', result)

    def buy(name='Native Charter', npc=None):
        client.send(0x1bd, struct.pack('<QIQ', vendor if npc is None else npc, 0, 0) + name.encode() + b'\0' + bytes(51))
        return client.collect()

    def sign(world, signer, charter, result=0):
        world.send(0x1c0, struct.pack('<QB', charter, 0))
        expected = (0x1c1, struct.pack('<QQI', charter, signer, result))
        assert expected in world.collect()
        return expected

    def money():
        return sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'][0][0]

    def current():
        return sql(f'SELECT petition_id, charter_item_guid FROM game_guild_petition WHERE owner_guid = {guid}')[0]['rows'][0]

    def emblem(world, npc, result):
        world.send(0x1f1, struct.pack('<QIIIII', npc, 11, 12, 3, 14, 15))
        packets = world.collect()
        assert (0x1f1, struct.pack('<I', result)) in packets
        return packets

    template = sql('SELECT * FROM game_item_template WHERE entry = 51')[0]
    row = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    row.update(entry=5863, name='Guild Charter', max_stack=1, max_count=1, bonding=1, inventory_type=0)
    if not sql('SELECT entry FROM game_item_template WHERE entry = 5863')[0]['rows']:
        literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in row.values()]
        sql(f"INSERT INTO game_item_template ({','.join(row)}) VALUES ({','.join(literals)})")
    old_money = money()
    old_vendors = {row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51004')[0]['rows']}
    call('debug_spawn_at_feet', [guid, 51004, 2])
    vendor = next(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51004')[0]['rows'] if row[0] not in old_vendors)
    sql(f'UPDATE game_world_entity SET npc_flags = 1536 WHERE guid = {vendor}')
    client.collect()
    peer.collect()
    client.send(0x1f2, struct.pack('<Q', vendor))
    assert (0x1f2, struct.pack('<Q', vendor)) in client.collect()
    client.send(0x1bb, struct.pack('<Q', vendor))
    assert any(opcode == 0x1bc for opcode, _ in client.collect())
    emblem(client, vendor, 2)
    call('debug_set_money', [guid, 150000])
    client.collect()
    assert command(0, '1', 6) in buy('1')
    assert money() == 150000
    assert not sql(f'SELECT petition_id FROM game_guild_petition WHERE owner_guid = {guid}')[0]['rows']
    buy(npc=peer_guid)
    assert money() == 150000
    call('debug_set_money', [guid, 999])
    client.collect()
    assert (0x1a5, struct.pack('<QIB', vendor, 5863, 2)) in buy()
    assert money() == 999
    call('debug_set_money', [guid, 150000])
    client.collect()
    bought = buy()
    petition_id, charter = current()
    assert money() == 149000
    assert charter in creates(bought)
    assert values(bought, charter)[22] == petition_id
    assert not sql('SELECT payer_guid FROM game_guild_fee_hold')[0]['rows']
    assert not sql('SELECT operation_id FROM game_guild_fee_decision')[0]['rows']
    buy()
    assert money() == 149000 and current() == [petition_id, charter]
    client.send(0x1be, struct.pack('<Q', charter))
    expected = (0x1bf, struct.pack('<QQIB', charter, guid, petition_id, 0))
    assert expected in client.collect()
    client.send(0x1c6, struct.pack('<IQ', petition_id, charter))
    response = next(body for opcode, body in client.collect() if opcode == 0x1c7)
    assert response.startswith(struct.pack('<IQ', petition_id, guid) + b'Native Charter\0')
    sign(client, guid, charter, 3)
    client.send(0x1c4, struct.pack('<Q', charter))
    assert (0x1c5, struct.pack('<I', 4)) in client.collect()
    assert current() == [petition_id, charter]
    client.send(0x1c3, struct.pack('<QQ', charter, peer_guid))
    client.collect()
    assert expected in peer.collect()
    peer.send(0x1c2, struct.pack('<Q', charter))
    peer.collect()
    assert (0x1c2, struct.pack('<Q', peer_guid)) in client.collect()
    expected = sign(peer, peer_guid, charter)
    assert expected in client.collect()
    expected = sign(peer, peer_guid, charter, 1)
    assert expected in client.collect()
    peer.send(0x2c1, struct.pack('<Q', charter) + b'Forbidden name\0')
    assert not any(opcode == 0x2c1 for opcode, _ in peer.collect())
    client.send(0x2c1, struct.pack('<Q', charter) + b'1\0')
    assert command(0, '1', 6) in client.collect()
    client.send(0x2c1, struct.pack('<Q', charter) + b'Native Founders\0')
    assert (0x2c1, struct.pack('<Q', charter) + b'Native Founders\0') in client.collect()
    client.send(0x4b)
    client.collect()
    peer.collect()
    client.send(0x3d, struct.pack('<Q', guid))
    assert values(client.collect(), charter)[22] == petition_id
    peer.collect()
    call('gw_destroy_guild_charter', [{'guid': guid, 'ownership': None}, charter])
    assert charter in destroys(client.collect())
    call('debug_set_money', [guid, 0])
    client.collect()
    buy()
    assert current() == [petition_id, charter]
    assert sql(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}')[0]['rows'] == [[peer_guid]]
    call('debug_set_money', [guid, 150000])
    client.collect()
    buy('Native Founders')
    old_petition = petition_id
    petition_id, charter = current()
    assert petition_id != old_petition and money() == 149000
    assert not sql(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {old_petition}')[0]['rows']
    assert sign(peer, peer_guid, charter) in client.collect()

    peer_account = sql(f'SELECT account_id FROM game_character WHERE guid = {peer_guid}')[0]['rows'][0][0]
    call('create_character', [peer_account, 'Peeralt', 1, 8, 0, 0, 0, 0, 0, 0])
    alternate = sql("SELECT guid FROM game_character WHERE name = 'Peeralt'")[0]['rows'][0][0]
    peer.send(0x4b)
    peer.collect()
    client.collect()
    peer.send(0x3d, struct.pack('<Q', alternate))
    peer.collect()
    client.collect()
    assert sign(peer, alternate, charter, 1) in client.collect()
    assert len(sql(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}')[0]['rows']) == 1
    peer.send(0x4b)
    peer.collect()
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.collect()
    extras = []
    try:
        for index, suffix in enumerate(['one', 'two', 'three', 'four', 'five', 'six', 'seven', 'eight']):
            username, name = 'SIGNER' + suffix.upper(), 'Signer' + suffix
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = sql(f"SELECT id FROM game_account WHERE username = '{username}'")[0]['rows'][0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([80 + index]).hex() * 32]])
            call('create_character', [account, name, 1, 8, 0, 0, 0, 0, 0, 0])
            signer = sql(f"SELECT guid FROM game_character WHERE name = '{name}'")[0]['rows'][0][0]
            world = World(client.socket.getpeername()[1], username=username)
            extras.append((world, signer))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', signer))
            world.collect()
            assert sign(world, signer, charter) in client.collect()
        assert len(sql(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}')[0]['rows']) == 9
        peer.send(0x1c4, struct.pack('<Q', charter))
        assert not any(opcode == 0x1c5 for opcode, _ in peer.collect())
        assert current() == [petition_id, charter]
        client.send(0x1c4, struct.pack('<Q', charter))
        founded = client.collect()
        assert [packet for packet in founded if packet[0] in (0x93, 0x1c5)] == [command(0, 'Native Founders', 0), (0x1c5, bytes(4))]
        assert charter in destroys(founded)
        guild_id = sql(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {guid}')[0]['rows'][0][0]
        assert values(founded, guid)[191] == guild_id
        assert command(14, 'Native Founders', 0) in peer.collect()
        for world, signer in extras:
            founded = world.collect()
            assert command(14, 'Native Founders', 0) in founded
            assert values(founded, signer)[191] == guild_id
        assert len(sql(f'SELECT character_guid FROM game_guild_member WHERE guild_id = {guild_id}')[0]['rows']) == 10
        assert not sql(f'SELECT petition_id FROM game_guild_petition WHERE owner_guid = {guid}')[0]['rows']
        assert not sql(f'SELECT signer_guid FROM game_guild_petition_signature WHERE petition_id = {petition_id}')[0]['rows']
        assert not sql(f'SELECT guid FROM game_item_instance WHERE guid = {charter}')[0]['rows']
        emblem(peer, vendor, 3)
        call('debug_set_money', [guid, 99999])
        client.collect()
        emblem(client, vendor, 4)
        assert money() == 99999
        call('debug_set_money', [guid, 150000])
        client.collect()
        emblem(client, peer_guid, 5)
        assert money() == 150000
        packets = emblem(client, vendor, 0)
        assert (0x92, bytes([9, 0])) in packets
        assert [opcode for opcode, _ in packets if opcode in (0x92, 0x55, 0x1f1)] == [0x92, 0x55, 0x1f1]
        assert money() == 50000
        assert sql(f'SELECT emblem_style, emblem_color, border_style, border_color, background_color FROM game_guild WHERE guild_id = {guild_id}')[0]['rows'] == [[11, 12, 3, 14, 15]]
        assert (0x92, bytes([9, 0])) in peer.collect()
        assert not sql('SELECT payer_guid FROM game_guild_fee_hold')[0]['rows']
        assert not sql('SELECT operation_id FROM game_guild_fee_decision')[0]['rows']
        client.send(0x8f)
        client.collect()
        peer.collect()
    finally:
        for world, _ in extras:
            world.close()
    call('debug_grant_item', [guid, 5863, 1])
    inert = sql(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5863')[0]['rows'][0][0]
    client.collect()
    client.send(0x4b)
    client.collect()
    peer.collect()
    client.send(0x3d, struct.pack('<Q', guid))
    assert inert in destroys(client.collect())
    assert not sql(f'SELECT guid FROM game_item_instance WHERE guid = {inert}')[0]['rows']
    peer.collect()
    peer_money = sql(f'SELECT money FROM game_world_entity WHERE guid = {peer_guid}')[0]['rows'][0][0]
    call('debug_set_money', [peer_guid, 1000])
    peer.collect()
    call('gw_guild_fee_hold', [5090499, {'guid': peer_guid, 'ownership': None},
                              {'charter': {'npc_guid': vendor, 'name': 'Pending Legacy Charter'}}])
    held = sql(f'SELECT charter_item_guid FROM game_guild_fee_hold WHERE payer_guid = {peer_guid}')[0]['rows'][0][0]
    peer.collect()
    peer.send(0x4b)
    peer.collect()
    client.collect()
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    pending = peer.collect()
    assert held in creates(pending) and held not in destroys(pending)
    assert sql(f'SELECT guid FROM game_item_instance WHERE guid = {held}')[0]['rows'] == [[held]]
    call('gw_guild_fee_finish', [5090499, {'guid': peer_guid, 'ownership': None}, False])
    assert held in destroys(peer.collect())
    assert sql(f'SELECT money FROM game_world_entity WHERE guid = {peer_guid}')[0]['rows'] == [[1000]]
    assert not sql('SELECT payer_guid FROM game_guild_fee_hold')[0]['rows']
    call('debug_set_money', [peer_guid, peer_money])
    call('debug_set_money', [guid, old_money])
    client.collect()
    peer.collect()


def check_trade(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    def status(code):
        return 0x120, struct.pack('<I', code)

    def trading(packets):
        return [packet for packet in packets if packet[0] in (0x120, 0x121)]

    worlds = []
    try:
        for index, name in enumerate(['Traderone', 'Tradertwo', 'Tradeviewer']):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([61 + index]).hex() * 32]])
            call('create_character', [account, name, 1, 8, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'DELETE FROM game_item_instance WHERE owner_guid = {guid} AND slot >= 23 AND slot <= 38')
            sql(f'UPDATE game_character SET map_id = 0, x = {1000 + index}, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', guid))
            world.collect()
            call('debug_set_money', [guid, 1000])
        (first, first_guid), (second, second_guid), (watcher, _) = worlds
        first.collect()
        second.collect()
        watcher.collect()

        def propose():
            first.send(0x116, struct.pack('<Q', second_guid))
            assert not trading(first.collect())
            assert (0x120, struct.pack('<IQ', 1, first_guid)) in second.collect()

        def begin():
            propose()
            second.send(0x117)
            assert status(2) in second.collect()
            assert status(2) in first.collect()

        def cancel():
            first.send(0x11c)
            assert status(3) in first.collect()
            assert status(3) in second.collect()
            assert not rows('SELECT id FROM game_trade_session')
            assert not rows('SELECT id FROM game_trade_slot')

        def offers(owner, partner, gold, slot=None, entry=0, stack=0, enchantment=0, random_property=0):
            for world, partner_flag in [(owner, 0), (partner, 1)]:
                packets = world.collect()
                bodies = [body for opcode, body in packets if opcode == 0x121]
                assert len(bodies) == 1, trading(packets)
                body = bodies[0]
                assert len(body) == 444
                assert struct.unpack_from('<BIIII', body) == (partner_flag, 7, 7, gold, 0)
                if slot is not None:
                    item = body[17 + slot * 61:17 + (slot + 1) * 61]
                    assert struct.unpack_from('<I', item, 1)[0] == entry
                    assert struct.unpack_from('<I', item, 9)[0] == stack
                    assert struct.unpack_from('<I', item, 25)[0] == enchantment
                    assert struct.unpack_from('<I', item, 45)[0] == random_property
                if partner_flag == 0:
                    owner_packets = packets
            return owner_packets

        def accept_first():
            first.send(0x11a, bytes(4))
            assert not trading(first.collect())
            assert status(4) in second.collect()

        for target in [0, first_guid]:
            first.send(0x116, struct.pack('<Q', target))
            packets = first.collect()
            assert status(6) in packets, trading(packets)
        second.send(0x6c, b'Traderone\0')
        second.collect()
        first.send(0x116, struct.pack('<Q', second_guid))
        assert status(14) in first.collect()
        second.send(0x6d, struct.pack('<Q', first_guid))
        second.collect()
        for opcode, result in [(0x118, 0), (0x119, 14)]:
            propose()
            first.send(0x117)
            assert not trading(first.collect())
            assert rows('SELECT open FROM game_trade_session') == [[False]]
            second.send(opcode)
            assert not trading(second.collect())
            assert status(result) in first.collect()
            assert not rows('SELECT id FROM game_trade_session')
        call('debug_teleport', [second_guid, 0, 1011, 1000, 0, 0])
        second.collect()
        first.collect()
        first.send(0x116, struct.pack('<Q', second_guid))
        assert status(10) in first.collect()
        call('debug_teleport', [second_guid, 0, 1001, 1000, 0, 0])
        second.collect()
        first.collect()

        call('debug_grant_item', [first_guid, 5090050, 1])
        call('debug_grant_item', [first_guid, 5090054, 1])
        call('debug_grant_item', [second_guid, 5090052, 5])
        weapon, weapon_slot = rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090050')[0]
        reins, reins_slot = rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090054')[0]
        food, food_slot = rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090052')[0]
        sql(f'UPDATE game_item_instance SET enchant_id = 7745, random_property_id = 1900, durability = 11 WHERE guid = {weapon}')
        first.collect()
        second.collect()
        begin()
        first.send(0x11d, bytes([0, 255, weapon_slot]))
        offers(first, second, 0, 0, 5090050, 1, 823, 1900)
        first.send(0x11d, bytes([6, 255, reins_slot]))
        offers(first, second, 0, 6)
        sql(f'UPDATE game_item_instance SET soulbound = false WHERE guid = {reins}')
        first.send(0x11d, bytes([6, 255, reins_slot]))
        offers(first, second, 0, 6, 5090054, 1)
        second.send(0x11d, bytes([0, 255, food_slot]))
        offers(second, first, 0, 0, 5090052, 5)
        for request in [bytes([7, 255, weapon_slot]), bytes([1, 1, weapon_slot]), bytes([1, 255, 4])]:
            first.send(0x11d, request)
            assert not trading(first.collect())
            assert not trading(second.collect())
        first.send(0x11d, bytes([1, 255, weapon_slot]))
        offers(first, second, 0, 1)
        assert len(rows('SELECT id FROM game_trade_slot')) == 3
        accept_first()
        second.send(0x11f, struct.pack('<I', 25))
        assert status(7) in offers(second, first, 25, 0, 5090052, 5)
        assert rows('SELECT initiator_accepted, target_accepted FROM game_trade_session') == [[False, False]]
        first.send(0x11b)
        assert not trading(first.collect())
        assert status(7) in second.collect()
        accept_first()
        second.send(0x11e, bytes([0]))
        assert status(7) in offers(second, first, 25, 0)
        second.send(0x11d, bytes([0, 255, food_slot]))
        offers(second, first, 25, 0, 5090052, 5)
        first.send(0x11f, struct.pack('<I', 50))
        offers(first, second, 50, 0, 5090050, 1, 823, 1900)
        accept_first()
        second.send(0x10e, bytes([255, food_slot, 255, 38, 1]))
        assert any(opcode == 0x112 for opcode, _ in second.collect())
        assert rows(f'SELECT stack_count FROM game_item_instance WHERE guid = {food}') == [[5]]
        assert rows('SELECT initiator_accepted, target_accepted FROM game_trade_session') == [[True, False]]
        assert not trading(first.collect())
        sql('UPDATE game_item_template SET entry = 4000000050 WHERE entry = 5090050')
        try:
            second.send(0x11a, bytes(4))
            failed = second.collect() + first.collect()
            assert not trading(failed) and not creates(failed)
            assert food not in destroys(failed) and weapon not in destroys(failed)
            assert rows(f'SELECT owner_guid FROM game_item_instance WHERE guid = {weapon}') == [[first_guid]]
            assert rows(f'SELECT owner_guid FROM game_item_instance WHERE guid = {food}') == [[second_guid]]
            assert rows(f'SELECT money FROM game_world_entity WHERE guid = {first_guid}') == [[1000]]
            assert rows(f'SELECT money FROM game_world_entity WHERE guid = {second_guid}') == [[1000]]
            assert rows('SELECT initiator_accepted, target_accepted FROM game_trade_session') == [[True, False]]
        finally:
            sql('UPDATE game_item_template SET entry = 5090050 WHERE entry = 4000000050')
        second.send(0x11a, bytes(4))
        second_done, first_done = second.collect(), first.collect()
        assert status(8) in second_done and status(8) in first_done
        assert weapon in destroys(first_done) and food in destroys(second_done)
        new_weapon, enchantment, random_property, durability = rows(f'SELECT guid, enchant_id, random_property_id, durability FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090050')[0]
        new_food, food_slot = rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090052')[0]
        assert new_weapon != weapon and new_weapon in creates(second_done)
        assert new_food != food and new_food in creates(first_done)
        assert (enchantment, random_property, durability) == (7745, 1900, 11)
        assert rows(f'SELECT owner_guid FROM game_item_instance WHERE guid = {reins}') == [[first_guid]]
        assert rows(f'SELECT money FROM game_world_entity WHERE guid = {first_guid}') == [[975]]
        assert rows(f'SELECT money FROM game_world_entity WHERE guid = {second_guid}') == [[1025]]
        assert not rows('SELECT id FROM game_trade_session') and not rows('SELECT id FROM game_trade_slot')

        begin()
        first.send(0x11f, struct.pack('<I', 976))
        offers(first, second, 976)
        accept_first()
        second.send(0x11a, bytes(4))
        assert (0x120, struct.pack('<IIBI', 12, 0x1d, 1, 0)) in second.collect()
        assert (0x120, struct.pack('<IIBI', 12, 0x1d, 0, 0)) in first.collect()
        assert not rows('SELECT id FROM game_trade_session')
        occupied = len(rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {second_guid} AND slot >= 23 AND slot <= 38'))
        for _ in range(16 - occupied):
            call('debug_grant_item', [second_guid, 5090050, 1])
        second.collect()
        first.collect()
        begin()
        first.send(0x11d, bytes([0, 255, food_slot]))
        offers(first, second, 0, 0, 5090052, 5)
        accept_first()
        second.send(0x11a, bytes(4))
        assert (0x120, struct.pack('<IIBI', 12, 0x32, 0, 0)) in second.collect()
        assert (0x120, struct.pack('<IIBI', 12, 0x32, 1, 0)) in first.collect()
        assert rows(f'SELECT owner_guid, stack_count FROM game_item_instance WHERE guid = {new_food}') == [[first_guid, 5]]
        assert not rows('SELECT id FROM game_trade_session')

        begin()
        call('debug_teleport', [second_guid, 0, 1011, 1000, 0, 0])
        assert not trading(second.collect())
        first.collect()
        accept_first()
        second.send(0x11a, bytes(4))
        assert status(10) in second.collect()
        assert status(7) in first.collect()
        assert rows('SELECT initiator_accepted, target_accepted FROM game_trade_session') == [[False, False]]
        cancel()
        call('debug_teleport', [second_guid, 0, 1001, 1000, 0, 0])
        second.collect()
        first.collect()
        begin()
        call('debug_set_health', [first_guid, 0])
        assert status(3) in first.collect() and status(3) in second.collect()
        first.send(0x116, struct.pack('<Q', second_guid))
        assert status(17) in first.collect()
        second.send(0x116, struct.pack('<Q', first_guid))
        assert status(18) in second.collect()
        call('debug_set_health', [first_guid, 1000])
        first.collect()
        second.collect()
        begin()
        second.send(0x4b)
        second.collect()
        assert status(3) in first.collect()
        assert not rows('SELECT id FROM game_trade_session')
        second.send(0x3d, struct.pack('<Q', second_guid))
        second.collect()
        first.collect()
        begin()
        call('debug_teleport', [second_guid, 1, 1001, 1000, 0, 0])
        departing = second.collect(auto_ack=False)
        assert status(3) in first.collect()
        assert departing.index(status(3)) < next(i for i, (opcode, _) in enumerate(departing) if opcode == 0x3e)
        assert not rows('SELECT id FROM game_trade_session')
        second.send(0xdc)
        second.collect()
        call('debug_teleport', [second_guid, 0, 1001, 1000, 0, 0])
        second.collect()
        first.collect()
        begin()
        call('debug_create_fixture_instance', [second_guid])
        assert status(3) in second.collect() and status(3) in first.collect()
        assert not rows('SELECT id FROM game_trade_session')
        assert rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {second_guid}')[0][0] != 0
        assert not trading(watcher.collect())
        assert not trading(client.collect()) and not trading(peer.collect())
    finally:
        for world, _ in worlds:
            world.close()


def check_duels(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    def duel_packets(packets):
        return [packet for packet in packets if packet[0] in (0x167, 0x168, 0x169, 0x16a, 0x16b, 0x2b7)]

    def until(world, predicate):
        deadline, packets = time.monotonic() + 5, []
        while time.monotonic() < deadline:
            packets.extend(world.collect())
            if predicate(packets):
                return packets
            time.sleep(0.05)
        raise AssertionError(f'duel transition did not arrive: {duel_packets(packets)}')

    sql("INSERT INTO game_gameobject_template (entry,type_id,display_id,name,data0,data1,gather_skill_line,respawn_secs,gather_gray,lock_id,size) VALUES (5090490,16,787,'Native duel flag',0,0,0,0,0,0,1.0)")
    sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES (5090173,'Native duel challenge',0,0,0,0,0,30,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0)")
    sql('INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES (20360692,5090173,0,34,0,0,0.0,0,1,0.0,0,0,0,5090490,13,0,0,false)')
    worlds = []
    try:
        for index, name in enumerate(['Duelone', 'Dueltwo', 'Duelviewer']):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([65 + index]).hex() * 32]])
            call('create_character', [account, name, 1, 8, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'UPDATE game_character SET map_id = 0, x = {1000 + index}, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', guid))
            entered = world.collect()
        (first, first_guid), (second, second_guid), (watcher, watcher_guid) = worlds
        assert first_guid in creates(entered) and second_guid in creates(entered)
        call('debug_learn_spell', [first_guid, 5090173])
        first.collect()
        second.collect()
        watcher.collect()

        def request():
            first.send(0x12e, cast(5090173, second_guid))
            packets = first.collect()
            assert rows('SELECT state FROM game_duel') == [[0]], duel_packets(packets)
            flag = rows('SELECT flag_guid FROM game_duel')[0][0]
            for received in [packets, second.collect()]:
                assert (0x167, struct.pack('<QQ', flag, first_guid)) in received
                assert creates(received).count(flag) == 1
                for participant in [first_guid, second_guid]:
                    fields = values(received, participant)
                    assert fields[188] | fields[189] << 32 == flag
                assert received.index((0x167, struct.pack('<QQ', flag, first_guid))) > next(i for i, (opcode, body) in enumerate(received) if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == flag)
            observed = watcher.collect()
            assert not duel_packets(observed) and flag not in creates(observed)
            return flag

        def activate(flag):
            second.send(0x16c, struct.pack('<Q', flag))
            assert (0x2b7, struct.pack('<I', 3000)) in second.collect()
            assert (0x2b7, struct.pack('<I', 3000)) in first.collect()
            assert rows('SELECT state FROM game_duel') == [[1]]
            sql('UPDATE game_duel SET countdown_deadline_micros = 1')
            active = until(first, lambda packets: values(packets, first_guid).get(196) == 1)
            partner = second.collect()
            for received in [active, partner]:
                assert values(received, first_guid)[196] == 1
                assert values(received, second_guid)[196] == 2
            assert not duel_packets(watcher.collect())

        def completion(packets, flag, reason=None, winner='Dueltwo', loser='Duelone'):
            assert (0x16a, bytes([reason is not None])) in packets, duel_packets(packets)
            if reason is None:
                assert not any(opcode == 0x16b for opcode, _ in packets)
            else:
                expected = (0x16b, bytes([reason]) + winner.encode() + b'\0' + loser.encode() + b'\0')
                assert packets.count(expected) == 1, duel_packets(packets)
            assert flag in destroys(packets)
            for participant in [first_guid, second_guid]:
                fields = values(packets, participant)
                assert [fields.get(index) for index in (188, 189, 196)] == [0, 0, 0]
            assert not rows('SELECT id FROM game_duel')

        flag = request()
        for world, offered_flag in [(first, flag), (second, flag + 1), (watcher, flag)]:
            world.send(0x16c, struct.pack('<Q', offered_flag))
            assert not duel_packets(world.collect())
            assert rows('SELECT state FROM game_duel') == [[0]]
        second.send(0x16d, struct.pack('<Q', flag + 1))
        assert not duel_packets(second.collect())
        assert rows('SELECT state FROM game_duel') == [[0]]
        second.send(0x16d, struct.pack('<Q', flag))
        completion(second.collect(), flag)
        completion(first.collect(), flag)
        assert not duel_packets(watcher.collect())

        flag = request()
        activate(flag)
        first.send(0x16d, struct.pack('<Q', flag))
        completion(first.collect(), flag, 1)
        completion(second.collect(), flag, 1)
        assert duel_packets(watcher.collect()) == [(0x16b, b'\x01Dueltwo\0Duelone\0')]

        flag = request()
        activate(flag)
        call('debug_create_fixture_instance', [watcher_guid])
        watcher.collect()
        call('debug_teleport', [first_guid, 0, 1060, 1000, 0, 0])
        assert (0x168, b'') in until(first, lambda packets: (0x168, b'') in packets)
        assert not duel_packets(second.collect()) and not duel_packets(watcher.collect())
        call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
        assert (0x169, b'') in until(first, lambda packets: (0x169, b'') in packets)
        assert not duel_packets(second.collect())
        call('debug_teleport', [first_guid, 0, 1060, 1000, 0, 0])
        until(first, lambda packets: (0x168, b'') in packets)
        sql('UPDATE game_duel SET initiator_out_of_bounds_deadline_micros = 1')
        completion(until(first, lambda packets: (0x16a, b'\x01') in packets), flag, 1)
        completion(second.collect(), flag, 1)
        assert not duel_packets(watcher.collect())
        call('debug_enter_instance', [watcher_guid, 0])
        watcher.collect()
        call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
        first.collect()
        second.collect()
        watcher.collect()

        flag = request()
        activate(flag)
        call('debug_teleport', [watcher_guid, 0, 2000, 1000, 0, 0])
        watcher.collect()
        call('debug_set_health', [second_guid, 1])
        first.collect()
        second.collect()
        first.send(0x141, struct.pack('<Q', second_guid))
        knocked_out, deadline = [], time.monotonic() + 10
        while time.monotonic() < deadline:
            sql(f'UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {first_guid}')
            knocked_out.extend(first.collect())
            if (0x16a, b'\x01') in knocked_out:
                break
            time.sleep(0.05)
        completion(knocked_out, flag, 0, 'Duelone', 'Dueltwo')
        completion(second.collect(), flag, 0, 'Duelone', 'Dueltwo')
        assert rows(f'SELECT health, dead FROM game_world_entity WHERE guid = {second_guid}') == [[1, False]]
        assert not rows(f'SELECT owner_guid FROM game_corpse WHERE owner_guid = {second_guid}')
        assert not rows(f'SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {first_guid}')
        assert not duel_packets(watcher.collect())
        call('debug_set_health', [second_guid, 1000])
        call('debug_teleport', [watcher_guid, 0, 1002, 1000, 0, 0])
        watcher.collect()
        first.collect()
        second.collect()

        flag = request()
        sql('UPDATE game_duel SET created_at_micros = 0')
        completion(until(first, lambda packets: (0x16a, b'\0') in packets), flag)
        completion(second.collect(), flag)
        assert not duel_packets(watcher.collect())
        flag = request()
        call('debug_teleport', [second_guid, 1, 1001, 1000, 0, 0])
        departing = second.collect(auto_ack=False)
        completion(departing, flag)
        completion(first.collect(), flag)
        assert departing.index((0x16a, b'\0')) < next(i for i, (opcode, _) in enumerate(departing) if opcode == 0x3e)
        second.send(0xdc)
        second.collect()
        call('debug_teleport', [second_guid, 0, 1001, 1000, 0, 0])
        second.collect()
        first.collect()
        watcher.collect()
        flag = request()
        second.send(0x4b)
        second.collect()
        completion(first.collect(), flag)
        assert not duel_packets(watcher.collect())
        second.send(0x3d, struct.pack('<Q', second_guid))
        second.collect()
        first.collect()
        watcher.collect()
        flag = request()
        call('debug_set_health', [second_guid, 0])
        completion(first.collect(), flag)
        completion(second.collect(), flag)
        assert not duel_packets(watcher.collect())
        call('debug_set_health', [second_guid, 1000])
        second.collect()
        first.collect()
        watcher.collect()
        flag = request()
        call('debug_create_fixture_instance', [second_guid])
        completion(second.collect(), flag)
        completion(first.collect(), flag)
        assert not duel_packets(watcher.collect())
        assert not duel_packets(client.collect()) and not duel_packets(peer.collect())
    finally:
        for world, _ in worlds:
            world.close()


def check_world_objects(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    def splines(packets, guid):
        result = []
        for opcode, body in packets:
            if opcode != 0xdd or packed_guid(body) != guid:
                continue
            offset = 1 + body[0].bit_count()
            x, y, z, spline_id, kind = struct.unpack_from('<fffIB', body, offset)
            offset += 17
            angle = None
            if kind == 4:
                angle = struct.unpack_from('<f', body, offset)[0]
                offset += 4
            else:
                assert kind == 0
            flags, duration, count = struct.unpack_from('<III', body, offset)
            destination = struct.unpack_from('<fff', body, offset + 12)
            result.append(dict(start=(x, y, z), id=spline_id, kind=kind, angle=angle,
                               flags=flags, duration=duration, count=count, destination=destination))
        return result

    worlds = []
    try:
        for index, name in enumerate(['Splineone', 'Splinetwo', 'Splinethree']):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([140 + index]).hex() * 32]])
            call('create_character', [account, name, 1, 1, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'UPDATE game_character SET map_id = 0, x = 3000, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', guid))
            world.collect()
        (first, first_guid), (second, second_guid), (outside, outside_guid) = worlds
        call('debug_create_fixture_instance', [first_guid])
        first.collect()
        instance = rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {first_guid}')[0][0]
        # A dedicated slow tick leaves spline timing under the fixture's control.
        call('debug_arm_instance_tick', [instance, 600000])
        call('debug_enter_instance', [second_guid, instance])
        second.collect()
        sql('UPDATE game_creature_template SET npc_flags = 0 WHERE entry = 51004')
        old_creatures = {row[0] for row in rows('SELECT guid FROM game_world_entity WHERE entry = 51004')}
        call('debug_spawn_at_feet', [first_guid, 51004, 2])
        creature = next(row[0] for row in rows('SELECT guid FROM game_world_entity WHERE entry = 51004') if row[0] not in old_creatures)
        sx, sy, sz = rows(f'SELECT x, y, z FROM game_world_entity WHERE guid = {creature}')[0]
        first_created, second_created = creates(first.collect()), creates(second.collect())
        assert creature in first_created and creature in second_created, (
            creature, first_created, second_created,
            rows(f'SELECT guid, map_id, instance_id, x, y, z, dead FROM game_world_entity WHERE guid = {creature} OR guid = {first_guid} OR guid = {second_guid}'),
            rows(f'SELECT viewer_guid, entity_guid FROM game_aoi_grant WHERE entity_guid = {creature}'))
        outside.collect()
        weapon = rows('SELECT display_id FROM game_item_template WHERE entry = 5090050')[0][0]
        call('debug_encounter_equip', [creature, 5090050, 0, 5090050])
        equipped = values(first.collect(), creature)
        assert [equipped[index] for index in (37, 38, 39)] == [weapon, 0, weapon]
        assert values(second.collect(), creature) == equipped
        assert not values(outside.collect(), creature)

        door_entry = 5090640
        door = (0xf110 << 48) | door_entry
        call('debug_spawn_gameobject', [door_entry, 0, 259, 0, 0, 3002, 1000, 0, 0, 0, 0, 0])
        spawned = outside.collect()
        assert door in creates(spawned)
        assert [values(spawned, door)[i] for i in range(10, 14)] == [0, 0, 0, 0x3f800000]
        assert door not in creates(first.collect() + second.collect())
        grant = rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {outside_guid} AND layer = 1 AND entity_guid = {door}')[0][0]
        outside.send(0xb1, struct.pack('<Q', door))
        toggled = outside.collect()
        assert values(toggled, door)[14] == 1 and door not in creates(toggled)
        assert rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {outside_guid} AND layer = 1 AND entity_guid = {door}') == [[grant]]
        assert not values(first.collect() + second.collect(), door)
        sql(f'UPDATE game_gameobject SET rotation_0 = 0.5, rotation_1 = 0.5, rotation_2 = 0.5, rotation_3 = 0.5 WHERE guid = {door}')
        outside.send(0xb1, struct.pack('<Q', door))
        closed = outside.collect()
        assert values(closed, door)[14] == 0
        assert [values(closed, door)[i] for i in range(10, 14)] == [0x3f000000] * 4
        call('debug_encounter_open_door', [door_entry, 0])
        opened = outside.collect()
        assert values(opened, door)[14] == 1 and door not in creates(opened)
        assert [values(opened, door)[i] for i in range(10, 14)] == [0x3f000000] * 4
        call('debug_encounter_open_door', [door_entry, 0])
        assert not values(outside.collect(), door)
        call('debug_enter_instance', [first_guid, 0])
        joined = first.collect()
        assert door in creates(joined)
        assert [values(joined, door)[i] for i in range(10, 14)] == [0x3f000000] * 4
        create_index = next(i for i, (opcode, body) in enumerate(joined) if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == door)
        rotation_index = next(i for i, (opcode, body) in enumerate(joined) if opcode == 0xa9 and body[5] == 0 and packed_guid(body[6:]) == door)
        assert create_index < rotation_index
        call('debug_enter_instance', [first_guid, instance])
        restored = first.collect()
        assert [values(restored, creature)[i] for i in (37, 38, 39)] == [weapon, 0, weapon]
        second.collect()
        outside.collect()
        call('debug_encounter_equip', [creature, 0, 5090050, 0])
        swapped = values(first.collect(), creature)
        assert [swapped[i] for i in (37, 38, 39)] == [0, weapon, 0]
        second.collect()
        call('debug_encounter_equip', [creature, 0, 0, 0])
        cleared = values(first.collect(), creature)
        assert [cleared[i] for i in (37, 38, 39)] == [0, 0, 0]
        second.collect()
        assert not values(outside.collect(), creature)
        first.send(0x101, struct.pack('<I', 1))
        assert (0x29d, b'\x01') in first.collect()
        assert not any(opcode == 0x29d for opcode, _ in second.collect() + outside.collect())
        call('debug_apply_damage', [first_guid, 1, 0])
        assert (0x29d, b'\0') in first.collect()
        assert not any(opcode == 0x29d for opcode, _ in second.collect() + outside.collect())
        call('debug_set_level', [first_guid, 20])
        call('debug_set_skill', [first_guid, 762, 75])
        first.collect()
        second.collect()
        call('debug_force_cast', [first_guid, 50310])
        mounted = first.collect()
        observed = second.collect()
        own_speed = [struct.unpack('<f', body[-4:])[0] for opcode, body in mounted if opcode == 0xe2 and packed_guid(body) == first_guid]
        peer_speed = [struct.unpack('<f', body[-4:])[0] for opcode, body in observed if opcode == 0x2fe and packed_guid(body) == first_guid]
        assert own_speed and all(abs(speed - 11.2) < 0.0001 for speed in own_speed), own_speed
        assert peer_speed == own_speed and not any(opcode == 0xe2 for opcode, _ in observed)
        assert not any(opcode in (0xe2, 0x2fe) for opcode, _ in outside.collect())
        first.send(0x136, struct.pack('<I', 50310))
        dismounted = first.collect()
        observed = second.collect()
        assert [struct.unpack('<f', body[-4:])[0] for opcode, body in dismounted if opcode == 0xe2 and packed_guid(body) == first_guid] == [7.0]
        assert [struct.unpack('<f', body[-4:])[0] for opcode, body in observed if opcode == 0x2fe and packed_guid(body) == first_guid] == [7.0]
        assert not any(opcode == 0xe2 for opcode, _ in observed)
        call('debug_teleport', [outside_guid, 0, 3300, 1000, 0, 0])
        assert door in destroys(outside.collect())
        call('debug_teleport', [outside_guid, 0, 3000, 1000, 0, 0])
        assert door in creates(outside.collect())

        ground_spell = 5090640
        for table, field, changes in [
            ('game_spell', 'spell_id', dict(spell_id=ground_spell, name='Native Ground', cost=0, cast_time_ms=0, gcd_ms=0, cooldown_ms=0, duration_ms=10000, attributes=0)),
            ('game_spell_effect', 'spell_id', dict(id=ground_spell << 2, spell_id=ground_spell, effect_index=0, kind=27, target=1, base_points=0, die_sides=0, per_level=0, period_ms=1000, radius_yd=10, enters_combat=False)),
        ]:
            source = sql(f'SELECT * FROM {table} WHERE {field} = 133')[0]
            row = dict(zip([column['name']['some'] for column in source['schema']['elements']], source['rows'][0]))
            row.update(changes)
            literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in row.values()]
            sql(f"INSERT INTO {table} ({','.join(row)}) VALUES ({','.join(literals)})")
        call('debug_learn_spell', [first_guid, ground_spell])
        first.collect()
        first.send(0x12e, cast(ground_spell))
        appeared = first.collect()
        objects = rows(f'SELECT guid FROM game_dynamic_object WHERE spell_id = {ground_spell}')
        assert len(objects) == 1, (objects, appeared)
        ground = objects[0][0]
        assert ground in creates(appeared) and ground in creates(second.collect())
        assert ground not in creates(outside.collect())
        assert rows(f'SELECT entity_guid FROM game_aoi_grant WHERE viewer_guid = {first_guid} AND layer = 3') == [[ground]]
        original_grant = rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {first_guid} AND layer = 3 AND entity_guid = {ground}')
        sql(f'UPDATE game_dynamic_object SET cell = 0 WHERE guid = {ground}')
        call('debug_enter_instance', [second_guid, 0])
        second.collect()
        call('debug_enter_instance', [second_guid, instance])
        assert ground not in creates(second.collect())
        call('debug_backfill_cell_ids', [])
        assert ground in creates(second.collect())
        assert ground not in creates(first.collect())
        assert rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {first_guid} AND layer = 3 AND entity_guid = {ground}') == original_grant
        call('debug_enter_instance', [second_guid, 0])
        assert ground not in creates(second.collect())
        call('debug_enter_instance', [second_guid, instance])
        assert ground in creates(second.collect())
        first.collect()
        deadline = time.monotonic() + 15
        expired_first, expired_second = [], []
        while rows(f'SELECT guid FROM game_dynamic_object WHERE guid = {ground}'):
            assert time.monotonic() < deadline, 'ground effect did not expire'
            expired_first += first.collect()
            expired_second += second.collect()
            time.sleep(0.05)
        expired_first += first.collect()
        expired_second += second.collect()
        assert ground in destroys(expired_first) and ground in destroys(expired_second)
        assert not rows(f'SELECT id FROM game_aoi_grant WHERE layer = 3 AND entity_guid = {ground}')
        assert ground not in creates(outside.collect())

        call('debug_encounter_move', [creature, sx + 20, sy, sz, False])
        first_moves = splines(first.collect(), creature)
        assert len(first_moves) == 1 and first_moves == splines(second.collect(), creature)
        assert first_moves[0]['start'] == (sx, sy, sz)
        assert first_moves[0]['destination'] == (sx + 20, sy, sz)
        assert first_moves[0]['duration'] == 8000 and first_moves[0]['flags'] == 0
        assert not splines(outside.collect(), creature)

        call('debug_enter_instance', [second_guid, 0])
        second.collect()
        first.collect()
        call('debug_creature_waypoints', [creature, [[sx + 20, sy, sz], [sx + 20, sy + 20, sz]], False])
        curved = splines(first.collect(), creature)
        assert len(curved) == 1 and curved[0]['count'] == 2
        assert curved[0]['id'] > first_moves[0]['id']
        assert curved[0]['destination'] == (sx + 20, sy + 20, sz)
        assert not splines(second.collect() + outside.collect(), creature)
        call('debug_enter_instance', [second_guid, instance])
        joined = second.collect()
        assert creature in creates(joined)
        replay = splines(joined, creature)
        assert len(replay) == 1 and replay[0]['id'] == curved[0]['id']
        assert 0 < replay[0]['duration'] < curved[0]['duration']
        assert replay[0]['start'][0] > curved[0]['start'][0]
        create_index = next(i for i, (opcode, body) in enumerate(joined) if opcode == 0xa9 and body[5] in (2, 3) and packed_guid(body[6:]) == creature)
        move_index = next(i for i, (opcode, body) in enumerate(joined) if opcode == 0xdd and packed_guid(body) == creature)
        assert create_index < move_index
        first.collect()

        call('debug_creature_face', [creature, sx + 20, sy + 40])
        facing = splines(first.collect(), creature)
        assert len(facing) == 2 and facing[0]['kind'] == 0 and facing[0]['duration'] == 0
        assert facing[1]['kind'] == 4 and facing[1]['duration'] == 0
        assert facing[1]['id'] > facing[0]['id'] > curved[0]['id']
        assert facing == splines(second.collect(), creature)
        assert not splines(outside.collect(), creature)
        call('debug_enter_instance', [second_guid, 0])
        second.collect()
        call('debug_enter_instance', [second_guid, instance])
        assert splines(second.collect(), creature) == [facing[1]]
        first.collect()

        call('debug_teleport', [first_guid, 1, 3000, 1000, 0, 0])
        first.collect()
        second.collect()
        call('debug_creature_path', [creature, sx + 20, sy + 20, sz, True])
        running = splines(second.collect(), creature)
        assert len(running) == 1 and running[0]['flags'] == 0x100
        assert not splines(first.collect() + outside.collect() + client.collect() + peer.collect(), creature)
        call('debug_enter_instance', [second_guid, 0])
        second.collect()
        call('debug_reap_instance', [instance])
        assert not rows(f'SELECT guid FROM game_creature_spline WHERE guid = {creature}')
        assert not rows(f'SELECT guid FROM game_world_entity WHERE guid = {creature}')
    finally:
        for world, _ in worlds:
            world.close()


def check_areatriggers(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    sphere, box, dungeon, exit_trigger, exploration = range(5090630, 5090635)
    for trigger, map_id, x, radius, length, width, height, yaw in [
        (sphere, 0, 1000, 5, 0, 0, 0, 0),
        (box, 0, 1100, 0, 8, 2, 4, 1.5707964),
        (dungeon, 0, 1000, 5, 0, 0, 0, 0),
        (exit_trigger, 36, 1000, 10, 0, 0, 0, 0),
        (exploration, 0, 1000, 5, 0, 0, 0, 0),
    ]:
        sql(f'INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) VALUES ({trigger},{map_id},{x},1000,0,{radius},{length},{width},{height},{yaw})')
    for trigger, target_map, x in [(sphere, 0, 1100), (box, 1, 1000), (dungeon, 36, 1000), (exit_trigger, 0, 1000)]:
        sql(f"INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) VALUES ({trigger},{target_map},{x},1000,0,0,'Native trigger')")
    template = sql('SELECT * FROM game_quest_template WHERE entry = 50900')[0]
    quest = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    quest.update(entry=sphere, title='Native exploration', min_level=1, quest_level=1,
                 prev_quest_id=0, required_races=0, required_classes=0, limit_time=0, src_item=0, src_item_count=0)
    literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in quest.values()]
    sql(f"INSERT INTO game_quest_template ({','.join(quest)}) VALUES ({','.join(literals)})")
    for index, trigger in enumerate([sphere, exploration]):
        sql(f'INSERT INTO game_quest_objective (id,quest_entry,obj_index,kind,target_entry,required_count) VALUES ({sphere + index},{sphere},{index},3,{trigger},1)')
    worlds = []
    try:
        for index, name in enumerate(['Areaone', 'Areatwo', 'Areathree']):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([130 + index]).hex() * 32]])
            call('create_character', [account, name, 1, 1, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'UPDATE game_character SET map_id = 0, x = 1000, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', guid))
            world.collect()
            call('debug_set_level', [guid, 60])
        (first, first_guid), (second, second_guid), (solo, solo_guid) = worlds
        call('debug_grant_quest', [first_guid, sphere])
        for world, _ in worlds:
            world.collect()

        def enter(world, trigger):
            world.send(0xb4, struct.pack('<I', trigger))
            return world.collect()

        def location(guid):
            return rows(f'SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {guid}')[0]

        def counts():
            return rows(f'SELECT counts FROM game_character_quest WHERE character_guid = {first_guid} AND quest_entry = {sphere}')[0][0]

        def no_port(packets):
            assert not any(opcode in (0xc7, 0x3e, 0x3f) for opcode, _ in packets)

        no_port(enter(first, 0))
        sql(f'UPDATE game_area_trigger SET map_id = 1 WHERE id = {sphere}')
        no_port(enter(first, sphere))
        sql(f'UPDATE game_area_trigger SET map_id = 0 WHERE id = {sphere}')
        assert counts() == [0, 0]
        for x, y, z in [(1005.1, 1000, 0), (1000, 1000, 5.1)]:
            call('debug_teleport', [first_guid, 0, x, y, z, 0])
            first.collect()
            no_port(enter(first, sphere))
            assert counts() == [0, 0]
        call('debug_teleport', [first_guid, 0, 1003, 1004, 0, 0])
        first.collect()
        packets = enter(first, sphere)
        assert any(opcode == 0xc7 for opcode, _ in packets)
        assert location(first_guid) == [0, 0, 1100, 1000, 0] and counts() == [1, 0]
        call('debug_teleport', [first_guid, 0, 1101.1, 1000, 0, 0])
        first.collect()
        no_port(enter(first, box))
        call('debug_teleport', [first_guid, 0, 1100, 1003.5, 1.5, 0])
        first.collect()
        packets = enter(first, box)
        assert any(opcode == 0x3e for opcode, _ in packets) and first_guid in creates(packets)
        assert location(first_guid) == [1, 0, 1000, 1000, 0]
        call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
        first.collect()
        packets = enter(first, exploration)
        no_port(packets)
        assert counts() == [1, 1]
        assert (0x198, struct.pack('<I', sphere)) in packets
        assert (0x198, struct.pack('<I', sphere)) not in enter(first, exploration)

        first.send(0x6e, b'Areatwo\0')
        first.collect()
        second.collect()
        second.send(0x72)
        second.collect()
        first.collect()
        group = rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {first_guid}')[0][0]
        assert rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {second_guid}') == [[group]]
        packets = enter(first, dungeon)
        assert any(opcode == 0x3e for opcode, _ in packets)
        party_instance = location(first_guid)[1]
        assert party_instance != 0 and location(first_guid) == [36, party_instance, 1000, 1000, 0]
        assert rows(f'SELECT map_id, party_id FROM game_instance WHERE instance_id = {party_instance}') == [[36, group]]
        assert first_guid in destroys(second.collect())
        solo.collect()
        packets = enter(second, dungeon)
        assert first_guid in creates(packets) and location(second_guid)[1] == party_instance
        assert second_guid in creates(first.collect())
        solo.collect()
        packets = enter(solo, dungeon)
        solo_instance = location(solo_guid)[1]
        assert solo_instance != 0 and solo_instance != party_instance
        assert not ({first_guid, second_guid} & set(creates(packets)))
        assert solo_guid not in creates(first.collect() + second.collect())
        assert not rows(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {solo_guid} AND entity_guid = {first_guid}')
        packets = enter(first, exit_trigger)
        assert any(opcode == 0x3e for opcode, _ in packets) and location(first_guid)[:2] == [0, 0]
        enter(first, dungeon)
        assert location(first_guid)[1] == party_instance
        assert rows(f'SELECT instance_id FROM game_instance_binding WHERE character_guid = {first_guid} AND map_id = 36') == [[party_instance]]

        def countdowns(packets):
            result = []
            for opcode, body in packets:
                if opcode == 0x286:
                    timer, reason = struct.unpack('<II', body)
                    assert reason == 1
                    result.append(timer)
            return result

        for world, _ in worlds:
            world.collect()
        second.send(0x7b)
        assert countdowns(second.collect()) == [60000]
        assert countdowns(first.collect()) == [60000]
        assert not countdowns(solo.collect())
        assert rows(f'SELECT character_guid FROM game_instance_removal WHERE character_guid = {first_guid}') == [[first_guid]]
        first.close()
        first = World(client.socket.getpeername()[1], username='AREAONE')
        worlds[0] = (first, first_guid)
        assert first.read()[0] == 0x1ee
        first.send(0x3d, struct.pack('<Q', first_guid))
        resumed = countdowns(first.collect())
        assert len(resumed) == 1 and 0 < resumed[0] < 60000, resumed
        assert location(first_guid)[1] == party_instance
        first.send(0x6e, b'Areatwo\0')
        invited_first = first.collect()
        invited_second = second.collect()
        second.send(0x72)
        assert countdowns(invited_second + second.collect()) == [0]
        assert countdowns(invited_first + first.collect()) == [0]
        assert not rows(f'SELECT character_guid FROM game_instance_removal WHERE character_guid = {first_guid} OR character_guid = {second_guid}')
        second.send(0x7b)
        assert countdowns(second.collect()) == [60000]
        assert countdowns(first.collect()) == [60000]
        call('debug_expire_instance_removal', [second_guid])
        expired = second.collect()
        assert countdowns(expired)[0] == 0 and any(opcode == 0x3e for opcode, _ in expired)
        assert location(second_guid)[:2] == [0, 0]
        assert not rows(f'SELECT character_guid FROM game_instance_removal WHERE character_guid = {second_guid}')
        assert not countdowns(first.collect() + solo.collect())
        for world, guid in worlds:
            enter(world, exit_trigger)
            assert location(guid)[:2] == [0, 0]
        assert not any(opcode == 0x3e for opcode, _ in client.collect() + peer.collect())
    finally:
        for world, _ in worlds:
            world.close()


def check_pets(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    worlds = []
    try:
        for index, (name, race, character_class) in enumerate([('Petone', 3, 3), ('Pettwo', 1, 1)]):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([120 + index]).hex() * 32]])
            call('create_character', [account, name, race, character_class, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'UPDATE game_character SET map_id = 0, x = 1000, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', guid))
            world.collect()
        (owner, owner_guid), (observer, observer_guid) = worlds
        call('debug_set_level', [owner_guid, 60])
        sql('UPDATE game_creature_template SET creature_family = 5, level = 5, health = 100, npc_flags = 0 WHERE entry = 51006')
        before = {row[0] for row in rows('SELECT guid FROM game_world_entity WHERE entry = 51006 AND owner_guid = 0')}
        call('debug_spawn_at_feet', [owner_guid, 51006, 5])
        wild = next(row[0] for row in rows('SELECT guid FROM game_world_entity WHERE entry = 51006 AND owner_guid = 0') if row[0] not in before)
        owner.collect()
        observer.collect()
        call('debug_force_cast_at', [owner_guid, 50300, wild])
        pet = rows(f'SELECT live_pet_guid FROM game_hunter_pet_protocol WHERE owner_guid = {owner_guid}')[0][0]
        owned, observed = owner.collect(), observer.collect()
        assert pet in creates(owned) and pet in creates(observed)
        assert wild in destroys(owned) and wild in destroys(observed)
        bar = next(index for index, (opcode, body) in enumerate(owned) if opcode == 0x179 and body[:8] == struct.pack('<Q', pet))
        assert next(index for index, packet in enumerate(owned) if pet in creates([packet])) < bar
        assert not any(opcode == 0x179 for opcode, _ in observed)
        summon = values(owned, owner_guid)
        assert summon[8] | summon[9] << 32 == pet
        owner_values, observer_values = values(owned, pet), values(observed, pet)
        assert owner_values[139] == observer_values[139] == owner_guid
        assert owner_values[140] == observer_values[140]
        assert owner_values[27] > 0 and owner_values[33] == 1000000 and owner_values[141] == 0
        assert all(field not in observer_values for field in (27, 33, 141, 142))
        old_stamp = owner_values[140]

        def rename(world, name, guid=pet):
            world.send(0x177, struct.pack('<Q', guid) + name.encode() + b'\0')
            return world.collect()

        assert (0x178, b'') not in rename(observer, 'F1')
        assert (0x178, b'') in rename(owner, 'F1')
        assert (0x178, b'') in rename(owner, 'F')
        assert rows(f'SELECT rename_allowed FROM game_hunter_pet WHERE owner_guid = {owner_guid}') == [[True]]
        renamed = rename(owner, 'Fang')
        observed = observer.collect()
        current = rows(f'SELECT name, rename_allowed, name_timestamp FROM game_hunter_pet WHERE owner_guid = {owner_guid}')[0]
        assert current[0:2] == ['Fang', False] and current[2] > old_stamp
        assert values(renamed, pet)[140] == values(observed, pet)[140] == current[2]
        assert not values(renamed, pet)[46] & 0x10
        assert all(field not in values(observed, pet) for field in (27, 33, 141, 142))
        observer.send(0x52, struct.pack('<IQ', owner_guid, pet))
        assert (0x53, struct.pack('<I', owner_guid) + b'Fang\0' + struct.pack('<I', current[2])) in observer.collect()
        assert (0x178, b'') not in rename(owner, 'F1')
        assert rows(f'SELECT name FROM game_hunter_pet WHERE owner_guid = {owner_guid}') == [['Fang']]

        baseline = rows(f'SELECT happiness FROM game_hunter_pet WHERE owner_guid = {owner_guid}')[0][0]
        owner.collect()
        observer.collect()
        deadline = time.monotonic() + 16
        changed = None
        while changed is None:
            updates = values(owner.collect(), pet)
            if 27 in updates and updates[27] < baseline:
                changed = updates[27]
            assert time.monotonic() < deadline, 'pet care did not update the owner'
            if changed is None:
                time.sleep(0.05)
        assert changed < baseline
        assert all(field not in values(observer.collect(), pet) for field in (27, 33, 141, 142))
        call('debug_teleport', [observer_guid, 0, 2000, 1000, 0, 0])
        assert pet in destroys(observer.collect())
        call('debug_teleport', [observer_guid, 0, 1000, 1000, 0, 0])
        replayed = observer.collect()
        assert pet in creates(replayed)
        assert values(replayed, pet)[140] == current[2]
        assert not any(opcode == 0x179 for opcode, _ in replayed)
        assert all(field not in values(replayed, pet) for field in (27, 33, 141, 142))
        owner.send(0x175, struct.pack('<QIQ', pet, 0x07000003, 0))
        dismissed, observed = owner.collect(), observer.collect()
        assert pet in destroys(dismissed) and pet in destroys(observed)
        assert (0x179, bytes(8)) in dismissed
        assert next(index for index, packet in enumerate(dismissed) if pet in destroys([packet])) < dismissed.index((0x179, bytes(8)))
        assert not any(opcode == 0x179 for opcode, _ in observed)
        assert values(dismissed, owner_guid)[8] == values(dismissed, owner_guid)[9] == 0
        assert rows(f'SELECT name FROM game_hunter_pet WHERE owner_guid = {owner_guid}') == [['Fang']]
        assert not rows(f'SELECT guid FROM game_world_entity WHERE guid = {pet}')
        assert not any(opcode == 0x179 for opcode, _ in client.collect() + peer.collect())
    finally:
        for world, _ in worlds:
            world.close()


def check_auctions(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    def auction_packets(packets):
        return [packet for packet in packets if packet[0] in (0x255, 0x25b, 0x25c, 0x25d, 0x25e, 0x25f, 0x265, 0x28d)]

    def result(packets, auction_id, action, code, extra=b''):
        expected = (0x25b, struct.pack('<III', auction_id, action, code) + extra)
        assert expected in packets, auction_packets(packets)

    template = sql('SELECT * FROM game_item_template WHERE entry = 51')[0]
    item = dict(zip([column['name']['some'] for column in template['schema']['elements']], template['rows'][0]))
    item.update(entry=5090620, name='Auction Test Sword')
    item.update({'class': 2, 'subclass': 7, 'inventory_type': 13, 'quality': 2,
                 'max_stack': 1, 'max_count': 0, 'bonding': 0, 'required_level': 1,
                 'sell_price': 1000, 'max_durability': 30})
    literals = ["'" + value.replace("'", "''") + "'" if isinstance(value, str) else str(value) for value in item.values()]
    sql(f"INSERT INTO game_item_template ({','.join(item)}) VALUES ({','.join(literals)})")
    for house in [1, 2, 7]:
        if not rows(f'SELECT id FROM game_auction_house WHERE id = {house}'):
            sql(f"INSERT INTO game_auction_house (id,faction,deposit_rate,consignment_rate,name) VALUES ({house},0,5,5,'Native Auction')")
    for faction in [12, 55, 120]:
        if not rows(f'SELECT id FROM game_faction_template WHERE id = {faction}'):
            sql(f'INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES ({faction},0,0,0,0,0,0,0,0,0,0,0,0)')
    worlds = []
    try:
        for index, (name, character_class) in enumerate([('Auctionone', 1), ('Auctiontwo', 8), ('Auctionthree', 1)]):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([110 + index]).hex() * 32]])
            call('create_character', [account, name, 1, character_class, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'DELETE FROM game_item_instance WHERE owner_guid = {guid} AND slot >= 23 AND slot <= 38')
            sql(f'UPDATE game_character SET map_id = 0, x = 1000, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', guid))
            world.collect()
            call('debug_set_money', [guid, 10000])
        (seller, seller_guid), (bidder, bidder_guid), (winner, winner_guid) = worlds
        old_vendors = {row[0] for row in rows('SELECT guid FROM game_world_entity WHERE entry = 51004')}
        call('debug_spawn_at_feet', [seller_guid, 51004, 2])
        npc = next(row[0] for row in rows('SELECT guid FROM game_world_entity WHERE entry = 51004') if row[0] not in old_vendors)
        sql(f'UPDATE game_world_entity SET npc_flags = 4096, faction_template = 12 WHERE guid = {npc}')
        for world, _ in worlds:
            world.collect()

        def purse(guid):
            return rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]

        def grant():
            call('debug_grant_item', [seller_guid, 5090620, 1])
            guid = rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {seller_guid} AND entry = 5090620')[0][0]
            sql(f'UPDATE game_item_instance SET durability = 17, enchant_id = 7745, random_property_id = 1900 WHERE guid = {guid}')
            seller.collect()
            return guid

        def sell(item_guid, start=100, buyout=500, duration=120, auctioneer=npc):
            seller.send(0x256, struct.pack('<QQIII', auctioneer, item_guid, start, buyout, duration))
            return seller.collect()

        def listing(item_guid):
            packets = sell(item_guid)
            auctions = rows(f'SELECT id FROM game_auction WHERE item_guid = {item_guid}')
            assert auctions, (
                auction_packets(packets), purse(seller_guid),
                rows(f'SELECT slot, soulbound FROM game_item_instance WHERE guid = {item_guid}'),
                rows(f'SELECT x, y, map_id, instance_id, npc_flags, faction_template, dead FROM game_world_entity WHERE guid = {npc}'),
            )
            auction_id = auctions[0][0]
            result(packets, auction_id, 0, 0)
            assert item_guid in destroys(packets)
            return auction_id

        def bid(world, auction_id, offer):
            world.send(0x25a, struct.pack('<QII', npc, auction_id, offer))
            return world.collect()

        def cancel(world, auction_id):
            world.send(0x257, struct.pack('<QI', npc, auction_id))
            return world.collect()

        def page(world, opcode=0x258, offset=0, name='', minimum=0, maximum=0,
                 inventory=0xffffffff, item_class=0xffffffff, subclass=0xffffffff, quality=0xffffffff, usable=False, auctioneer=npc):
            body = struct.pack('<QI', auctioneer, offset)
            if opcode == 0x258:
                body += name.encode() + b'\0' + struct.pack('<BBIIIIB', minimum, maximum, inventory, item_class, subclass, quality, usable)
            if opcode == 0x264:
                body += bytes(4)
            world.send(opcode, body)
            expected = {0x258: 0x25c, 0x259: 0x25d, 0x264: 0x265}[opcode]
            body = next(body for received, body in world.collect() if received == expected)
            count = struct.unpack_from('<I', body)[0]
            assert len(body) == 8 + count * 64, body.hex()
            return [struct.unpack_from('<7IQ4IQI', body, 4 + index * 64) for index in range(count)], struct.unpack_from('<I', body, 4 + count * 64)[0]

        seller.send(0x255, struct.pack('<Q', npc))
        assert (0x255, struct.pack('<QI', npc, 1)) in seller.collect()
        assert page(seller) == ([], 0)
        for column, invalid, original in [('npc_flags', 0, 4096), ('map_id', 1, 0), ('instance_id', 99, 0), ('dead', 'true', 'false')]:
            sql(f'UPDATE game_world_entity SET {column} = {invalid} WHERE guid = {npc}')
            seller.send(0x255, struct.pack('<Q', npc))
            assert not auction_packets(seller.collect())
            assert page(seller) == ([], 0)
            sql(f'UPDATE game_world_entity SET {column} = {original} WHERE guid = {npc}')
        call('debug_teleport', [seller_guid, 0, 1020, 1000, 0, 0])
        seller.collect()
        seller.send(0x255, struct.pack('<Q', npc))
        assert not auction_packets(seller.collect())
        assert page(seller) == ([], 0)
        call('debug_teleport', [seller_guid, 0, 1000, 1000, 0, 0])
        seller.collect()
        item_guid = grant()
        result(sell(item_guid, auctioneer=0), 0, 0, 2)
        for args in [dict(start=0), dict(buyout=50), dict(duration=121)]:
            result(sell(item_guid, **args), 0, 0, 2)
        sql(f'UPDATE game_item_instance SET soulbound = true WHERE guid = {item_guid}')
        result(sell(item_guid), 0, 0, 4)
        sql(f'UPDATE game_item_instance SET soulbound = false WHERE guid = {item_guid}')
        call('debug_set_money', [seller_guid, 49])
        result(sell(item_guid), 0, 0, 3)
        assert rows(f'SELECT guid FROM game_item_instance WHERE guid = {item_guid}') and purse(seller_guid) == 49
        call('debug_set_money', [seller_guid, 10000])
        auction_id = listing(item_guid)
        assert purse(seller_guid) == 9950
        assert rows(f'SELECT deposit, item_durability, item_enchant_id, random_property_id FROM game_auction WHERE id = {auction_id}') == [[50, 17, 7745, 1900]]
        result(sell(item_guid), auction_id, 0, 0)
        assert purse(seller_guid) == 9950
        result(sell(item_guid, buyout=600), 0, 0, 4)
        assert len(rows(f'SELECT auction_id FROM game_auction_operation_receipt WHERE actor_guid = {seller_guid}')) == 1
        entries, total = page(bidder, name='tEsT sWoRd', minimum=1, maximum=1, inventory=13, item_class=2, subclass=7, quality=2)
        assert total == 1 and entries[0][:8] == (auction_id, 5090620, 823, 1900, 0, 1, 0, seller_guid)
        assert entries[0][8:11] == (100, 100, 500) and entries[0][12:] == (0, 0)
        for args in [dict(name='missing'), dict(minimum=2), dict(quality=3), dict(inventory=4), dict(item_class=4), dict(subclass=0), dict(usable=True)]:
            assert page(bidder, **args) == ([], 0)
        assert page(bidder, offset=1) == ([], 1)
        assert page(bidder, 0x259) == ([], 0)
        assert page(seller, 0x259)[1] == 1
        sql(f'UPDATE game_world_entity SET faction_template = 55 WHERE guid = {npc}')
        assert page(bidder)[1] == 1
        sql(f'UPDATE game_world_entity SET faction_template = 120 WHERE guid = {npc}')
        assert page(bidder) == ([], 0)
        result(bid(bidder, auction_id, 100), auction_id, 2, 4)
        result(cancel(seller, auction_id), 0, 1, 2)
        sql(f'UPDATE game_world_entity SET faction_template = 12 WHERE guid = {npc}')
        result(bid(seller, auction_id, 100), auction_id, 2, 10)
        result(bid(bidder, auction_id, 99), auction_id, 2, 7)
        result(bid(bidder, auction_id, 10001), auction_id, 2, 3)
        result(bid(bidder, auction_id, 100), auction_id, 2, 0, struct.pack('<I', 5))
        assert purse(bidder_guid) == 9900
        assert (0x25f, struct.pack('<IIIQII', auction_id, 100, 5, bidder_guid, 5090620, 1900)) in seller.collect()
        assert not auction_packets(winner.collect())
        assert page(bidder, 0x264)[1] == 1 and page(winner, 0x264) == ([], 0)
        result(bid(winner, auction_id, 100), auction_id, 2, 5, struct.pack('<QII', bidder_guid, 100, 5))
        result(bid(winner, auction_id, 104), auction_id, 2, 7)
        result(bid(winner, auction_id, 105), auction_id, 2, 0, struct.pack('<I', 6))
        assert purse(winner_guid) == 9895
        assert (0x25e, struct.pack('<IIQIIII', 1, auction_id, bidder_guid, 100, 5, 5090620, 1900)) in bidder.collect()
        assert rows(f'SELECT money FROM game_mail WHERE recipient_guid = {bidder_guid}') == [[100]]
        seller.collect()
        bought = bid(bidder, auction_id, 700)
        result(bought, auction_id, 2, 0, struct.pack('<I', 25))
        assert (0x25e, struct.pack('<IIQIIII', 1, auction_id, bidder_guid, 0, 25, 5090620, 1900)) in bought
        assert purse(bidder_guid) == 9400
        assert not rows(f'SELECT id FROM game_auction WHERE id = {auction_id}')
        assert not rows(f'SELECT auction_id FROM game_auction_expiry WHERE auction_id = {auction_id}')
        assert rows(f'SELECT money FROM game_mail WHERE recipient_guid = {seller_guid}') == [[525]]
        assert rows(f'SELECT item_entry, item_durability, item_enchant_id, random_property_id FROM game_mail WHERE recipient_guid = {bidder_guid} AND item_entry != 0') == [[5090620, 17, 7745, 1900]]
        assert rows(f'SELECT money FROM game_mail WHERE recipient_guid = {winner_guid}') == [[105]]
        assert (0x25f, struct.pack('<IIIQII', auction_id, 500, 25, 0, 5090620, 1900)) in seller.collect()
        winner.collect()
        result(bid(bidder, auction_id, 700), auction_id, 2, 4)
        assert purse(bidder_guid) == 9400

        cancelled = listing(grant())
        result(cancel(bidder, cancelled), 0, 1, 2)
        result(bid(winner, cancelled, 100), cancelled, 2, 0, struct.pack('<I', 5))
        seller.collect()
        call('debug_set_money', [seller_guid, 4])
        assert not auction_packets(cancel(seller, cancelled))
        assert rows(f'SELECT id FROM game_auction WHERE id = {cancelled}') and purse(seller_guid) == 4
        call('debug_set_money', [seller_guid, 1000])
        result(cancel(seller, cancelled), cancelled, 1, 0)
        assert purse(seller_guid) == 995
        assert (0x28d, struct.pack('<III', cancelled, 5090620, 1900)) in winner.collect()
        assert sorted(rows(f'SELECT money FROM game_mail WHERE recipient_guid = {winner_guid}')) == [[100], [105]]
        assert rows(f'SELECT item_entry FROM game_mail WHERE recipient_guid = {seller_guid} AND item_entry != 0') == [[5090620]]
        result(cancel(seller, cancelled), 0, 1, 2)
        assert purse(seller_guid) == 995

        for price in [0, 100]:
            expiring = listing(grant())
            if price:
                result(bid(winner, expiring, price), expiring, 2, 0, struct.pack('<I', 5))
                seller.collect()
            call('debug_auction_due', [expiring])
            deadline = time.monotonic() + 5
            while rows(f'SELECT id FROM game_auction WHERE id = {expiring}'):
                assert time.monotonic() < deadline, 'auction timer did not settle'
                time.sleep(0.05)
            assert (0x25f, struct.pack('<IIIQII', expiring, price, 5 if price else 1, 0, 5090620, 1900)) in seller.collect()
            packets = winner.collect()
            if price:
                assert (0x25e, struct.pack('<IIQIIII', 1, expiring, winner_guid, 0, 5, 5090620, 1900)) in packets
                assert (0x285, bytes(4)) in packets
            else:
                assert not auction_packets(packets)
        assert sorted(rows(f'SELECT money FROM game_mail WHERE recipient_guid = {seller_guid} AND money != 0')) == [[145], [525]]
        assert len(rows(f'SELECT id FROM game_mail WHERE recipient_guid = {seller_guid} AND item_entry != 0')) == 2
        assert rows(f'SELECT item_entry FROM game_mail WHERE recipient_guid = {winner_guid} AND item_entry != 0') == [[5090620]]
        assert not rows('SELECT * FROM game_auction_hold')
        assert not rows('SELECT * FROM game_auction_bid_hold')
        assert not rows('SELECT * FROM game_auction_bid_decision')
        assert not auction_packets(client.collect() + peer.collect())
    finally:
        for world, _ in worlds:
            world.close()


def check_mail(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    def mail_packets(packets):
        return [packet for packet in packets if packet[0] in (0x239, 0x23b, 0x244, 0x284, 0x285)]

    def result(packets, mail_id, action, code, extra=b''):
        expected = (0x239, struct.pack('<III', mail_id, action, code) + extra)
        assert expected in packets, mail_packets(packets)

    mailbox = 0xf110000000000a01
    call('debug_stage_meeting_stone', [5090610, mailbox, 1, 60, 1581, 0, 1001, 1000, 0])
    sql('UPDATE game_gameobject_template SET type_id = 19 WHERE entry = 5090610')
    sql("INSERT INTO game_item_template (entry,class,subclass,name,display_id,quality,inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race,random_property) VALUES (8383,0,0,'Plain Letter',0,0,0,0,0,0,0,0,1,0.0,0.0,0,0,0,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,1503,255,0)")
    worlds = []
    try:
        for index, (name, race) in enumerate([('Mailone', 1), ('Mailtwo', 1), ('Mailhorde', 2)]):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([100 + index]).hex() * 32]])
            call('create_character', [account, name, race, 1, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'DELETE FROM game_item_instance WHERE owner_guid = {guid} AND slot >= 23 AND slot <= 38')
            sql(f'UPDATE game_character SET map_id = 0, x = 1000, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', guid))
            world.collect()
            call('debug_set_money', [guid, 1000])
            if index == 0:
                call('create_character', [account, 'Mailalt', 1, 1, 0, 0, 0, 0, 0, 0])
        (first, first_guid), (second, second_guid), (watcher, _) = worlds
        alt_guid = rows("SELECT guid FROM game_character WHERE name = 'Mailalt'")[0][0]
        for world, _ in worlds:
            world.collect()

        def send(name='Mailtwo', subject='letter', body='private body', money=0, cod=0, item=0, box=mailbox):
            first.send(0x238, struct.pack('<Q', box) + name.encode() + b'\0' + subject.encode() + b'\0' + body.encode() + b'\0' + struct.pack('<IIQIIII', 0, 0, item, money, cod, 0, 0))
            return first.collect()

        def request(world, opcode, mail_id, box=mailbox):
            world.send(opcode, struct.pack('<QI', box, mail_id) + (bytes(4) if opcode == 0x24a else b''))
            return world.collect()

        def text(world, mail_id, hint=0):
            world.send(0x243, struct.pack('<IQ', mail_id, hint))
            return [body for opcode, body in world.collect() if opcode == 0x244]

        def inbox(world):
            world.send(0x23a, struct.pack('<Q', mailbox))
            return [body for opcode, body in world.collect() if opcode == 0x23b][0]

        def purse(guid):
            return rows(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0][0]

        second.send(0x284)
        empty = [body for opcode, body in second.collect() if opcode == 0x284]
        assert len(empty) == 1 and struct.unpack('<f', empty[0])[0] < 0
        assert inbox(second) == b'\0'
        assert not mail_packets(send(box=0))
        for name, code in [('Missingmail', 4), ('mAiLoNe', 2), ('Mailhorde', 5)]:
            result(send(name=name), 0, 0, code)
        call('debug_teleport', [first_guid, 0, 1020, 1000, 0, 0])
        first.collect()
        assert not mail_packets(send())
        call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
        first.collect()
        for column, value in [('map_id', 1), ('instance_id', 99)]:
            sql(f'UPDATE game_gameobject SET {column} = {value} WHERE guid = {mailbox}')
            assert not mail_packets(send())
            sql(f'UPDATE game_gameobject SET {column} = 0 WHERE guid = {mailbox}')
        assert purse(first_guid) == 1000

        result(send(name='mAiLtWo', money=130, cod=250), 0, 0, 0)
        assert purse(first_guid) == 840
        letter, cod, due = rows(f'SELECT id, cod, deliver_micros FROM game_mail WHERE recipient_guid = {second_guid}')[0]
        assert cod == 0 and due == 0
        assert (0x285, bytes(4)) in second.collect()
        assert not mail_packets(watcher.collect())
        listing = inbox(second)
        assert listing[0] == 1 and struct.unpack_from('<I', listing, 1)[0] == letter and b'letter\0' in listing
        assert text(first, letter) == [struct.pack('<I', letter) + b'\0']
        assert text(second, letter) == [struct.pack('<I', letter) + b'private body\0']
        for opcode, action in [(0x245, 1), (0x248, 3), (0x249, 4), (0x24a, 5)]:
            result(request(first, opcode, letter), letter, action, 6)
        request(first, 0x247, letter)
        assert rows(f'SELECT was_read FROM game_mail WHERE id = {letter}') == [[False]]
        request(second, 0x247, letter)
        assert rows(f'SELECT was_read FROM game_mail WHERE id = {letter}') == [[True]]
        second.send(0x284)
        assert struct.unpack('<f', [body for opcode, body in second.collect() if opcode == 0x284][0])[0] < 0
        result(request(second, 0x245, letter, box=0), letter, 1, 6)
        assert purse(second_guid) == 1000
        result(request(second, 0x245, letter), letter, 1, 0)
        assert purse(second_guid) == 1130
        result(request(second, 0x245, letter), letter, 1, 6)

        for _ in range(16):
            call('debug_grant_item', [second_guid, 5090050, 1])
        second.collect()
        result(request(second, 0x24a, letter), letter, 5, 1, struct.pack('<I', 50))
        assert not rows(f'SELECT id FROM game_item_text WHERE id = {letter}')
        assert rows(f'SELECT check_flags FROM game_mail WHERE id = {letter}') == [[16]]
        sql(f'DELETE FROM game_item_instance WHERE owner_guid = {second_guid} AND slot = 38')
        copied = request(second, 0x24a, letter)
        result(copied, letter, 5, 0)
        copy = rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {second_guid} AND item_text_id = {letter}')[0][0]
        assert copy in creates(copied)
        assert rows(f'SELECT letters FROM game_item_text WHERE id = {letter}') == [[1]]
        result(request(second, 0x24a, letter), letter, 5, 6)
        result(request(second, 0x249, letter), letter, 4, 0)
        assert copy > 0xffffffff
        assert text(second, letter, copy) == [struct.pack('<I', letter) + b'private body\0']
        assert text(second, letter, copy & 0xffffffff) == [struct.pack('<I', letter) + b'private body\0']
        assert text(first, letter, copy) == [struct.pack('<I', letter) + b'\0']
        assert text(second, letter) == [struct.pack('<I', letter) + b'\0']
        sql(f'DELETE FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090050 AND slot >= 23')

        call('debug_grant_item', [first_guid, 5090050, 1])
        item = rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090050')[0][0]
        sql(f'UPDATE game_item_instance SET enchant_id = 7745, durability = 11, random_property_id = 1900, soulbound = true WHERE guid = {item}')
        first.collect()
        result(send(item=copy), 0, 0, 19)
        result(send(item=item), 0, 0, 16)
        sql(f'UPDATE game_item_instance SET soulbound = false WHERE guid = {item}')
        call('debug_set_money', [first_guid, 29])
        first.collect()
        failed = send(item=item)
        result(failed, 0, 0, 3)
        assert item not in destroys(failed)
        assert rows(f'SELECT owner_guid FROM game_item_instance WHERE guid = {item}') == [[first_guid]]
        assert purse(first_guid) == 29
        call('debug_set_money', [first_guid, 1000])
        first.collect()
        sent = send(item=item, cod=250, subject='delivery: native cod')
        result(sent, 0, 0, 0)
        assert item in destroys(sent) and purse(first_guid) == 970
        assert not rows(f'SELECT guid FROM game_item_instance WHERE guid = {item}')
        delayed, due = rows(f'SELECT id, deliver_micros FROM game_mail WHERE recipient_guid = {second_guid}')[0]
        assert due > int(time.time() * 1_000_000) + 3_590_000_000
        assert not mail_packets(second.collect()) and inbox(second) == b'\0'
        result(request(second, 0x246, delayed), delayed, 2, 6, bytes(8))
        assert text(second, delayed) == [struct.pack('<I', delayed) + b'\0']
        call('debug_deliver_mail_fixture', [second_guid, 'delivery: native cod'])
        arrived = []
        for _ in range(100):
            arrived += second.collect()
            if (0x285, bytes(4)) in arrived:
                break
            time.sleep(0.02)
        assert arrived.count((0x285, bytes(4))) == 1
        delivered_list = inbox(second)
        assert delivered_list[0] == 1
        attachment = delivered_list.index(b'delivery: native cod\0') + len(b'delivery: native cod\0')
        assert struct.unpack_from('<III', delivered_list, attachment + 12) == (5090050, 823, 1900)
        assert struct.unpack_from('<I', delivered_list, attachment + 37)[0] == 11
        second.send(0x284)
        assert (0x284, bytes(4)) in second.collect()
        result(request(second, 0x249, delayed), delayed, 4, 6)
        call('debug_set_money', [second_guid, 249])
        second.collect()
        result(request(second, 0x246, delayed), delayed, 2, 3, bytes(8))
        call('debug_set_money', [second_guid, 1000])
        second.collect()
        for _ in range(15):
            call('debug_grant_item', [second_guid, 5090050, 1])
        second.collect()
        result(request(second, 0x246, delayed), delayed, 2, 1, struct.pack('<I', 50))
        assert purse(second_guid) == 1000
        assert rows(f'SELECT item_entry, cod FROM game_mail WHERE id = {delayed}') == [[5090050, 250]]
        assert not mail_packets(first.collect())
        sql(f'DELETE FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090050 AND slot >= 23')
        taken = request(second, 0x246, delayed)
        result(taken, delayed, 2, 0, struct.pack('<II', 5090050, 1))
        new_item, enchantment, durability, property_id = rows(f'SELECT guid, enchant_id, durability, random_property_id FROM game_item_instance WHERE owner_guid = {second_guid} AND entry = 5090050')[0]
        assert new_item != item and new_item in creates(taken)
        assert (enchantment, durability, property_id) == (7745, 11, 1900)
        assert purse(second_guid) == 750
        assert rows(f'SELECT item_entry, cod FROM game_mail WHERE id = {delayed}') == [[0, 0]]
        assert (0x285, bytes(4)) in first.collect()
        payment, money, flags = rows(f'SELECT id, money, check_flags FROM game_mail WHERE recipient_guid = {first_guid}')[0]
        assert (money, flags) == (250, 8)
        result(request(first, 0x245, payment), payment, 1, 0)
        assert purse(first_guid) == 1220
        result(request(second, 0x246, delayed), delayed, 2, 6, bytes(8))

        result(send(subject='return me', money=10), 0, 0, 0)
        second.collect()
        returned = rows("SELECT id FROM game_mail WHERE subject = 'return me'")[0][0]
        result(request(second, 0x248, returned), returned, 3, 0)
        assert rows(f'SELECT recipient_guid, sender_guid, money, check_flags FROM game_mail WHERE id = {returned}') == [[first_guid, second_guid, 10, 2]]
        assert (0x285, bytes(4)) in first.collect()
        result(request(first, 0x248, returned), returned, 3, 6)
        result(request(first, 0x249, returned), returned, 4, 0)
        call('debug_grant_item', [first_guid, 5090050, 1])
        alt_item = rows(f'SELECT guid FROM game_item_instance WHERE owner_guid = {first_guid} AND entry = 5090050')[0][0]
        first.collect()
        result(send(name='Mailalt', item=alt_item), 0, 0, 0)
        assert rows(f'SELECT deliver_micros, item_entry FROM game_mail WHERE recipient_guid = {alt_guid}') == [[0, 5090050]]
        second.send(0x238, struct.pack('<Q', mailbox) + b'Mailone\0delivery: native copy\0\0' + struct.pack('<IIQIIII', 0, 0, copy, 0, 0, 0, 0))
        result(second.collect(), 0, 0, 0)
        copy_mail = rows("SELECT id FROM game_mail WHERE subject = 'delivery: native copy'")[0][0]
        assert text(second, letter, copy) == [struct.pack('<I', letter) + b'\0']
        call('debug_deliver_mail_fixture', [first_guid, 'delivery: native copy'])
        for _ in range(100):
            if (0x285, bytes(4)) in first.collect():
                break
            time.sleep(0.02)
        copied_again = request(first, 0x246, copy_mail)
        result(copied_again, copy_mail, 2, 0, struct.pack('<II', 8383, 1))
        held_copy = rows(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {first_guid} AND item_text_id = {letter}')[0]
        assert text(first, letter, held_copy[0]) == [struct.pack('<I', letter) + b'private body\0']
        first.send(0x111, bytes([255, held_copy[1], 1, 0, 0, 0]))
        assert held_copy[0] in destroys(first.collect())
        assert not rows(f'SELECT id FROM game_item_text WHERE id = {letter}')
        assert not rows('SELECT escrow_id FROM game_mail_escrow')
        assert not mail_packets(watcher.collect() + client.collect() + peer.collect())
    finally:
        for world, _ in worlds:
            world.close()


def check_meeting_stones(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    def queue(area, status):
        return 0x295, struct.pack('<IB', area, status)

    def stone_packets(packets):
        return [packet for packet in packets if packet[0] in (0x295, 0x297, 0x298, 0x299, 0x2bb)]

    stone, other_stone = 0xf110000000000999, 0xf11000000000099a
    call('debug_stage_meeting_stone', [5090600, stone, 1, 60, 1581, 0, 1001, 1000, 0])
    call('debug_stage_meeting_stone', [5090601, other_stone, 1, 60, 717, 0, 1002, 1000, 0])
    worlds = []
    try:
        for index, (name, race, character_class) in enumerate([
            ('Stoneone', 1, 1), ('Stonetwo', 1, 5), ('Stonethree', 1, 8),
            ('Stonefour', 1, 4), ('Stonefive', 1, 9), ('Stonehorde', 2, 1),
        ]):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([90 + index]).hex() * 32]])
            call('create_character', [account, name, race, character_class, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'UPDATE game_character SET map_id = 0, x = 1000, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            world.send(0x3d, struct.pack('<Q', guid))
            entered = world.collect()
            assert stone in creates(entered) and other_stone in creates(entered)
        (first, first_guid), (second, second_guid), (third, third_guid), (fourth, _), (fifth, fifth_guid), (horde, horde_guid) = worlds
        for world, guid in worlds:
            world.collect()
            assert not rows(f'SELECT account_id FROM game_account_claim WHERE character_guid = {guid}')

        def join(world, target=stone):
            world.send(0x292, struct.pack('<Q', target))
            return world.collect()

        first.send(0x1ff)
        assert (0x1ff, bytes(4)) in first.collect()
        first.send(0x296)
        assert queue(0, 5) in first.collect()
        assert not stone_packets(join(first, 0))
        sql('UPDATE game_meeting_stone SET min_level = 2 WHERE entry = 5090600')
        assert not stone_packets(join(first))
        assert not rows(f'SELECT character_guid FROM game_meeting_stone_seeker WHERE character_guid = {first_guid}')
        sql('UPDATE game_meeting_stone SET min_level = 1 WHERE entry = 5090600')
        call('debug_teleport', [first_guid, 0, 1100, 1000, 0, 0])
        first.collect()
        assert not stone_packets(join(first))
        call('debug_teleport', [first_guid, 0, 1000, 1000, 0, 0])
        first.collect()
        assert queue(1581, 1) in join(first)
        assert queue(1581, 1) in join(first)
        assert rows(f'SELECT area_id, team, class, group_id FROM game_meeting_stone_seeker WHERE character_guid = {first_guid}') == [[1581, 469, 1, 0]]
        assert queue(717, 1) in join(first, other_stone)
        first.send(0x296)
        assert queue(717, 1) in first.collect()
        first.send(0x293)
        assert queue(0, 0) in first.collect()
        assert queue(1581, 1) in join(first)
        first.send(0x4b)
        first.collect()
        assert not rows(f'SELECT character_guid FROM game_meeting_stone_seeker WHERE character_guid = {first_guid}')
        first.send(0x3d, struct.pack('<Q', first_guid))
        first.collect()
        first.send(0x296)
        assert queue(0, 5) in first.collect()

        assert queue(1581, 1) in join(horde)
        for world, _ in worlds[:4]:
            assert queue(1581, 1) in join(world)
        assert not rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {first_guid}')
        assert rows(f'SELECT area_id, team, class, group_id FROM game_meeting_stone_seeker WHERE character_guid = {horde_guid}') == [[1581, 67, 1, 0]]
        formed = join(fifth)
        group = rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {first_guid}')[0][0]
        assert rows(f'SELECT leader_guid FROM game_group WHERE group_id = {group}') == [[first_guid]]
        assert {row[0] for row in rows(f'SELECT character_guid FROM game_group_member WHERE group_id = {group}')} == {guid for _, guid in worlds[:5]}
        for index, (world, guid) in enumerate(worlds[:5]):
            packets = formed if index == 4 else world.collect()
            assert (0x297, b'') in packets and queue(0, 5) in packets
            rosters = [body for opcode, body in packets if opcode == 0x7d]
            assert rosters and struct.unpack_from('<I', rosters[-1], 2)[0] == 4
            assert not rows(f'SELECT character_guid FROM game_meeting_stone_seeker WHERE character_guid = {guid}')
        assert not stone_packets(horde.collect())
        assert not rows(f'SELECT group_id FROM game_meeting_stone_party WHERE group_id = {group}')
        assert (0x2bb, b'\x02') in join(first)
        assert (0x2bb, b'\x01') in join(second)
        first.send(0x75, b'Stonefive\0')
        for world, _ in worlds[:5]:
            world.collect()
        assert queue(1581, 1) in join(first)
        for world, _ in worlds[1:4]:
            assert queue(1581, 1) in world.collect()
        assert rows(f'SELECT group_id FROM game_meeting_stone_party WHERE group_id = {group}') == [[group]]
        second.send(0x293)
        assert queue(0, 5) in second.collect()
        assert rows(f'SELECT group_id FROM game_meeting_stone_seeker WHERE character_guid = {second_guid}') == [[group]]
        call('debug_backdate_meeting_stone_party', [group, 301])
        deadline, reminded = time.monotonic() + 7, []
        while time.monotonic() < deadline:
            reminded.extend(first.collect())
            if (0x298, b'') in reminded:
                break
            time.sleep(0.05)
        assert (0x298, b'') in reminded
        for world, _ in worlds[1:4]:
            assert (0x298, b'') in world.collect()
        assert not stone_packets(fifth.collect()) and not stone_packets(horde.collect())
        first.send(0x75, b'Stonethree\0')
        assert stone_packets(first.collect()) == [queue(0, 3), queue(0, 0)]
        for world in [second, fourth]:
            assert stone_packets(world.collect()) == [queue(0, 3), queue(0, 0)]
        assert stone_packets(third.collect()) == [queue(1581, 4), queue(1581, 1)]
        assert rows(f'SELECT group_id FROM game_meeting_stone_seeker WHERE character_guid = {third_guid}') == [[0]]
        third.send(0x293)
        assert queue(0, 0) in third.collect()
        first.send(0x28e)
        first.collect()
        second.collect()
        fourth.collect()
        assert (0x2bb, b'\x03') in join(first)
        assert not rows(f'SELECT group_id FROM game_meeting_stone_party WHERE group_id = {group}')
        horde.send(0x293)
        assert queue(0, 0) in horde.collect()
        assert not stone_packets(client.collect()) and not stone_packets(peer.collect())
        for world, _ in worlds[:5]:
            world.send(0x7b)
            world.collect()
    finally:
        for world, _ in worlds:
            world.close()


def check_member_stats(client, peer, call, sql, guid, peer_guid):
    def decode(body):
        target = packed_guid(body)
        offset = 1 + body[0].bit_count()
        mask = struct.unpack_from('<I', body, offset)[0]
        offset += 4
        fields = {}
        formats = ['B', 'H', 'H', 'B', 'H', 'H', 'H', 'H', 'HH', None, None,
                   'Q', None, 'H', 'H', 'H', 'B', 'H', 'H', None, None]
        for bit, fmt in enumerate(formats):
            if not mask & (1 << bit):
                continue
            if bit in (9, 10, 19, 20):
                width = 4 if bit in (9, 19) else 2
                dirty = int.from_bytes(body[offset:offset + width], 'little')
                offset += width
                slots = {}
                for slot in range(width * 8):
                    if dirty & (1 << slot):
                        slots[slot] = struct.unpack_from('<H', body, offset)[0]
                        offset += 2
                fields[bit] = slots
            elif bit == 12:
                end = body.index(0, offset)
                fields[bit] = body[offset:end].decode()
                offset = end + 1
            else:
                parts = struct.unpack_from('<' + fmt, body, offset)
                fields[bit] = parts[0] if len(parts) == 1 else parts
                offset += struct.calcsize('<' + fmt)
        assert offset == len(body), (mask, offset, len(body))
        return target, fields

    def full(target):
        client.send(0x27f, struct.pack('<Q', target))
        decoded = [decode(body) for opcode, body in client.collect() if opcode == 0x2f2]
        assert len(decoded) == 1 and decoded[0][0] == target
        return decoded[0][1]

    def changes():
        return [fields for opcode, body in client.collect() if opcode == 0x7e
                for target, fields in [decode(body)] if target == peer_guid]

    def changed_until(predicate):
        deadline, seen = time.monotonic() + 8, []
        while time.monotonic() < deadline:
            seen.extend(changes())
            if any(predicate(fields) for fields in seen):
                return seen
            time.sleep(0.05)
        raise AssertionError(f'member stats did not arrive: {seen}')

    assert full(guid) == {0: 0}
    assert full(1) == {0: 0}
    initial = full(peer_guid)
    assert initial[0] == 1 and initial[6] > 0
    assert 11 not in initial
    call('debug_teleport', [peer_guid, 1, 11, 12, 0, 0])
    peer.collect()
    baseline = changed_until(lambda fields: fields.get(8) == (11, 12))
    assert any(fields.get(0) == 1 and fields.get(11) == 0 for fields in baseline)
    health = sql(f'SELECT max_health FROM game_world_entity WHERE guid = {peer_guid}')[0]['rows'][0][0]
    sql(f'UPDATE game_world_entity SET health = {health - 1} WHERE guid = {peer_guid}')
    delta = changed_until(lambda fields: fields.get(1) == min(65535, health - 1))
    assert any(set(fields) == {1} for fields in delta), delta
    call('debug_fill_aura_slots', [peer_guid, peer_guid, False, 1])
    call('debug_fill_aura_slots', [peer_guid, peer_guid, True, 1])
    sql(f'UPDATE game_aura SET spell_id = 123 WHERE target_guid = {peer_guid} AND slot = 0')
    sql(f'UPDATE game_aura SET spell_id = 456 WHERE target_guid = {peer_guid} AND slot = 32')
    changed_until(lambda fields: fields.get(9) == {0: 123} and fields.get(10) == {0: 456})
    sql(f'DELETE FROM game_aura WHERE target_guid = {peer_guid}')
    changed_until(lambda fields: fields.get(9) == {0: 0} and fields.get(10) == {0: 0})

    unit_bytes = sql(f'SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {peer_guid}')[0]['rows'][0][0]
    sql(f'UPDATE game_world_entity SET unit_bytes_0 = {(unit_bytes & ~0xff00) | 0x0300} WHERE guid = {peer_guid}')
    sql('UPDATE game_creature_template SET creature_family = 5, npc_flags = 0 WHERE entry = 51006')
    before = {row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51006')[0]['rows']}
    call('debug_spawn_at_feet', [peer_guid, 51006, 3])
    added = {row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51006')[0]['rows']} - before
    assert len(added) == 1
    wild = added.pop()
    sql(f'UPDATE game_world_entity SET level = 1 WHERE guid = {wild}')
    call('debug_force_cast_at', [peer_guid, 50300, wild])
    pet_guid = 0xf130000000000000 | peer_guid
    assert sql(f'SELECT guid FROM game_world_entity WHERE guid = {pet_guid}')[0]['rows']
    sql(f"UPDATE game_hunter_pet SET name = 'Nativepet' WHERE owner_guid = {peer_guid}")
    sql(f"UPDATE game_hunter_pet_protocol SET name = 'Nativepet' WHERE live_pet_guid = {pet_guid}")
    call('debug_fill_aura_slots', [pet_guid, peer_guid, False, 1])
    sql(f'UPDATE game_aura SET spell_id = 789 WHERE target_guid = {pet_guid} AND slot = 0')
    changed_until(lambda fields: fields.get(11) == pet_guid and fields.get(12) == 'Nativepet' and fields.get(19) == {0: 789})
    complete = full(peer_guid)
    assert complete[11] == pet_guid and complete[12] == 'Nativepet' and complete[19][0] == 789
    changed_until(lambda fields: fields.get(11) == pet_guid)
    call('debug_pet_command', [peer_guid, 0x07000003, 0])
    removed = changed_until(lambda fields: fields.get(11) == 0)
    assert any(fields.get(12) == '' and fields.get(19) == {0: 0} for fields in removed), removed
    sql(f'UPDATE game_world_entity SET unit_bytes_0 = {unit_bytes} WHERE guid = {peer_guid}')
    assert not any(fields.get(0) == 0 for fields in changes())
    call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect(auto_ack=False)
    assert full(peer_guid) == {0: 0x21}
    time.sleep(5.2)
    assert not changes(), 'loading members must not be reported offline'
    peer.send(0xdc)
    peer.collect()
    client.collect()
    time.sleep(5.2)
    assert not changes(), 'AOI members must use entity updates'
    peer.send(0x4b)
    peer.collect()
    assert not sql(f'SELECT id FROM game_native_member_stats WHERE owner_guid = {peer_guid}')[0]['rows']
    assert full(peer_guid) == {0: 0}
    changed_until(lambda fields: fields == {0: 0})
    time.sleep(5.2)
    assert not changes(), 'offline status is sent once'
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    client.collect()
    assert sql(f'SELECT id FROM game_native_member_stats WHERE owner_guid = {peer_guid}')[0]['rows']


def check_party(client, peer, call, sql, guid, peer_guid):
    def command(operation, name, result):
        return 0x7f, struct.pack('<I', operation) + name.encode() + b'\0' + struct.pack('<I', result)

    def roster(packets):
        body = next(body for opcode, body in packets if opcode == 0x7d)
        kind, own, count = struct.unpack_from('<BBI', body)
        offset, members = 6, {}
        for _ in range(count):
            end = body.index(0, offset)
            name = body[offset:end].decode()
            member, online, flags = struct.unpack_from('<QBB', body, end + 1)
            members[member] = (name, online, flags)
            offset = end + 11
        leader = struct.unpack_from('<Q', body, offset)[0]
        offset += 8
        loot = struct.unpack_from('<BQBB', body, offset) if count else None
        assert len(body) == offset + (11 if count else 0)
        return kind, own, members, leader, loot

    def invite(name='Peer', result=0):
        client.send(0x6e, name.encode() + b'\0')
        assert command(0, name, result) in client.collect()

    client.send(0x7b)
    assert (0x7c, b'') in client.collect()
    assert (0x7c, b'') in peer.collect()
    invite('Missinggroupmember', 1)
    invite('Native', 1)
    invite('pEeR')
    assert (0x6f, b'Native\0') in peer.collect()
    peer.send(0x73)
    peer.collect()
    assert (0x74, b'Peer\0') in client.collect()
    assert not sql(f'SELECT id FROM game_group_invite WHERE target_guid = {peer_guid}')[0]['rows']
    peer.send(0x4b)
    peer.collect()
    invite('Peer', 1)
    sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    invite('Peer', 7)
    assert not any(opcode == 0x6f for opcode, _ in peer.collect())
    peer.send(0x4b)
    peer.collect()
    sql(f'UPDATE game_character SET race = 1 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    invite()
    peer.collect()
    peer.send(0x72)
    assert roster(peer.collect()) == (0, 0, {guid: ('Native', 1, 0)}, guid, (3, 0, 2, 0))
    assert roster(client.collect()) == (0, 0, {peer_guid: ('Peer', 1, 0)}, guid, (3, 0, 2, 0))
    invite('Peer', 4)
    peer.send(0x75, b'Native\0')
    assert command(2, 'Native', 6) in peer.collect()
    assert not any(opcode in (0x77, 0x7c) for opcode, _ in client.collect())
    peer.send(0x76, struct.pack('<Q', peer_guid))
    assert not any(opcode == 0x7f for opcode, _ in peer.collect())

    call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    assert roster(peer.collect())[2] == {guid: ('Native', 1, 0)}
    client.collect()
    peer.send(0x322)
    assert not any(opcode == 0x322 for opcode, _ in peer.collect() + client.collect())
    client.send(0x322)
    assert (0x322, b'') in client.collect()
    assert (0x322, b'') in peer.collect()
    client.send(0x322)
    assert not any(opcode == 0x322 for opcode, _ in client.collect() + peer.collect())
    peer.send(0x322, b'\1')
    assert not any(opcode == 0x322 for opcode, _ in peer.collect())
    assert (0x322, struct.pack('<QB', peer_guid, 1)) in client.collect()
    client.send(0x1d5, struct.pack('<ff', 0.25, 0.75))
    assert not any(opcode == 0x1d5 for opcode, _ in client.collect())
    assert (0x1d5, struct.pack('<Qff', guid, 0.25, 0.75)) in peer.collect()
    client.send(0x1d5, struct.pack('<ff', 0.5, 0.5))
    client.collect()
    assert not any(opcode == 0x1d5 for opcode, _ in peer.collect())
    client.send(0x1fb, struct.pack('<II', 10003, 10002))
    expected = (0x1fb, struct.pack('<IIIQ', 10000, 10000, 10000, guid))
    assert expected in client.collect()
    assert expected in peer.collect()
    client.send(0x1fb, struct.pack('<II', 1, 1))
    assert not any(opcode == 0x1fb for opcode, _ in client.collect() + peer.collect())

    peer.send(0x321, struct.pack('<BQ', 1, guid))
    assert not any(opcode == 0x321 for opcode, _ in peer.collect() + client.collect())
    client.send(0x321, struct.pack('<BQ', 1, guid))
    expected = (0x321, struct.pack('<BBQ', 0, 1, guid))
    assert expected in client.collect()
    assert expected in peer.collect()
    client.send(0x321, struct.pack('<BQ', 2, guid))
    expected = [(0x321, struct.pack('<BBQ', 0, 1, 0)), (0x321, struct.pack('<BBQ', 0, 2, guid))]
    assert [p for p in client.collect() if p[0] == 0x321] == expected
    assert [p for p in peer.collect() if p[0] == 0x321] == expected
    peer.send(0x321, b'\xff')
    icon_list = (0x321, struct.pack('<BBQ', 1, 2, guid))
    assert icon_list in peer.collect()
    client.send(0x7a, struct.pack('<IQI', 2, guid, 3))
    changed = client.collect()
    assert roster(changed)[4] == (2, guid, 3, 0)
    assert [op for op, _ in changed if op in (0x7d, 0x321)] == [0x7d, 0x321]
    assert icon_list in peer.collect()
    peer.send(0x7a, struct.pack('<IQI', 0, 0, 2))
    assert not any(op == 0x7d for op, _ in peer.collect() + client.collect())
    client.send(0x28e)
    changed = client.collect()
    assert command(0, '', 0) in changed
    assert roster(changed)[0] == 1
    assert roster(peer.collect())[0] == 1
    client.send(0x28f, struct.pack('<QB', peer_guid, 1))
    assert roster(client.collect())[2][peer_guid][2] == 0x80
    assert roster(peer.collect())[1] == 0x80
    peer.send(0x27e, b'nAtIvE\0\3')
    assert roster(peer.collect())[2][guid][2] == 3
    assert roster(client.collect())[1] == 3
    peer.send(0x280, b'Peer\0Native\0')
    assert roster(peer.collect())[1] == 0x83
    assert roster(client.collect())[1] == 0
    peer.send(0x27e, b'Native\0\x08')
    assert not any(op == 0x7d for op, _ in peer.collect() + client.collect())
    client.send(0x78, struct.pack('<Q', peer_guid))
    for packets in (client.collect(), peer.collect()):
        assert [op for op, _ in packets if op in (0x79, 0x7d)] == [0x79, 0x7d]
        assert (0x79, b'Peer\0') in packets
        assert roster(packets)[3] == peer_guid
    client.send(0x78, struct.pack('<Q', guid))
    assert not any(op == 0x79 for op, _ in client.collect() + peer.collect())
    peer.send(0x78, struct.pack('<Q', guid))
    peer.collect()
    client.collect()
    peer.send(0x4b)
    peer.collect()
    assert roster(client.collect())[2][peer_guid][1] == 0
    client.send(0x78, struct.pack('<Q', peer_guid))
    assert not any(op == 0x79 for op, _ in client.collect())
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    returned = peer.collect()
    assert roster(returned)[0] == 1 and icon_list in returned
    assert roster(client.collect())[2][peer_guid][1] == 1

    call('debug_spawn_player_entity', [1])
    call('debug_set_sessionless_action_consent', [1, False])
    invite('Tester')
    assert not sql('SELECT id FROM game_group_member WHERE character_guid = 1')[0]['rows']
    assert sql('SELECT id FROM game_group_invite WHERE target_guid = 1')[0]['rows']
    call('debug_set_sessionless_action_consent', [1, True])
    invite('Tester')
    assert sql('SELECT id FROM game_group_member WHERE character_guid = 1')[0]['rows']
    assert not sql('SELECT id FROM game_group_invite WHERE target_guid = 1')[0]['rows']
    peer.collect()
    client.send(0x76, struct.pack('<Q', peer_guid))
    roster(client.collect())
    assert [op for op, _ in peer.collect() if op in (0x77, 0x7c)] == [0x77, 0x7c]
    invite()
    peer.collect()
    peer.send(0x72)
    peer.collect()
    client.collect()
    client.send(0x75, b'Tester\0')
    roster(client.collect())
    peer.collect()
    client.send(0x7b)
    assert (0x7c, b'') in client.collect()
    assert (0x7c, b'') in peer.collect()
    invite()
    peer.collect()
    peer.send(0x72)
    peer.collect()
    client.collect()
    call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    client.collect()


def check_channels(client, peer, call, sql, guid, peer_guid):
    name = b'Native channel\0'

    def command(connection, opcode, extra=b'', channel=name):
        connection.send(opcode, channel + extra)

    def notices(packets):
        return [body for opcode, body in packets if opcode == 0x99]

    def notice(code, extra=b''):
        return bytes([code]) + name + extra

    def say(connection, text):
        connection.send(0x95, struct.pack('<II', 14, 7) + name + text.encode() + b'\0')

    command(client, 0x9a)
    assert notices(client.collect()) == [notice(5)]
    command(client, 0x97, b'\0')
    joined = notices(client.collect())
    assert notice(2, struct.pack('<II', 1, 0)) in joined
    channel_id = sql("SELECT channel_id FROM game_chat_channel WHERE name_key = 'native channel'")[0]['rows'][0][0]
    assert sql(f'SELECT account_id, claim_generation FROM game_chat_channel_member WHERE channel_id = {channel_id}')[0]['rows'][0][1] == 0
    command(client, 0x9c, b'secret\0')
    assert notices(client.collect()) == [notice(7, struct.pack('<Q', guid))]
    command(peer, 0x97, b'wrong\0')
    assert notices(peer.collect()) == [notice(4)]
    assert not sql(f'SELECT id FROM game_chat_channel_member WHERE character_guid = {peer_guid}')[0]['rows']
    command(client, 0x9c, b'x' * 129 + b'\0')
    assert notices(client.collect()) == [notice(4)]
    assert sql(f'SELECT password FROM game_chat_channel WHERE channel_id = {channel_id}')[0]['rows'] == [['secret']]
    command(peer, 0x97, b'secret\0')
    assert notice(2, struct.pack('<II', 1, 0)) in notices(peer.collect())
    assert notices(client.collect()) == [notice(0, struct.pack('<Q', peer_guid))]
    command(peer, 0x9c, b'no\0')
    assert notices(peer.collect()) == [notice(6)]
    assert not notices(client.collect())
    command(peer, 0x9e)
    assert notices(peer.collect()) == [notice(11, b'Native\0')]
    command(peer, 0x9a)
    roster = next(body for opcode, body in peer.collect() if opcode == 0x9b)
    assert struct.pack('<Q', guid) in roster and struct.pack('<Q', peer_guid) in roster
    sql(f'UPDATE game_character SET gm_level = 1 WHERE guid = {guid}')
    say(client, 'channel line')
    assert any(opcode == 0x96 and b'channel line\0' in body for opcode, body in client.collect())
    assert any(opcode == 0x96 and b'channel line\0' in body for opcode, body in peer.collect())
    command(client, 0xa1, b'Peer\0')
    client.collect()
    peer.collect()
    say(peer, 'muted channel line')
    assert notices(peer.collect()) == [notice(17)]
    assert not any(opcode == 0x96 for opcode, _ in client.collect())
    command(client, 0xa2, b'Peer\0')
    client.collect()
    peer.collect()
    command(client, 0xa8)
    client.collect()
    peer.collect()
    say(peer, 'moderated channel line')
    assert notices(peer.collect()) == [notice(6)]
    client.send(0x6c, b'Peer\0')
    client.collect()
    command(client, 0x9f, b'Peer\0')
    client.collect()
    peer.collect()
    say(peer, 'moderator line')
    assert any(opcode == 0x96 and b'moderator line\0' in body for opcode, body in peer.collect())
    assert any(opcode == 0x96 and b'moderator line\0' in body for opcode, body in client.collect())
    command(client, 0xa0, b'Peer\0')
    client.collect()
    peer.collect()
    command(client, 0xa8)
    client.collect()
    peer.collect()
    say(peer, 'ignored ordinary member')
    assert any(opcode == 0x96 for opcode, _ in peer.collect())
    assert not any(opcode == 0x96 for opcode, _ in client.collect())
    client.send(0x6d, struct.pack('<Q', peer_guid))
    client.collect()
    command(client, 0xa5, b'Peer\0')
    assert notice(20, struct.pack('<QQ', peer_guid, guid)) in notices(client.collect())
    assert notice(20, struct.pack('<QQ', peer_guid, guid)) in notices(peer.collect())
    command(peer, 0x97, b'secret\0')
    assert notices(peer.collect()) == [notice(19)]
    command(client, 0xa6, b'Peer\0')
    assert notice(21, struct.pack('<QQ', peer_guid, guid)) in notices(client.collect())
    command(client, 0xa3, b'Peer\0')
    assert notices(client.collect()) == [notice(29, b'Peer\0')]
    assert notices(peer.collect()) == [notice(24, struct.pack('<Q', guid))]
    peer.send(0x6c, b'Native\0')
    peer.collect()
    command(client, 0xa3, b'Peer\0')
    assert notices(client.collect()) == [notice(29, b'Peer\0')]
    assert not notices(peer.collect())
    peer.send(0x6d, struct.pack('<Q', guid))
    peer.collect()
    command(peer, 0x97, b'secret\0')
    peer.collect()
    client.collect()
    command(client, 0xa4, b'Peer\0')
    assert notice(18, struct.pack('<QQ', peer_guid, guid)) in notices(client.collect())
    peer.collect()
    command(peer, 0x97, b'secret\0')
    peer.collect()
    client.collect()
    command(client, 0x9d, b'Peer\0')
    client.collect()
    peer.collect()
    command(client, 0x9e)
    assert notices(client.collect()) == [notice(11, b'Peer\0')]
    command(peer, 0xa7)
    peer.collect()
    client.collect()
    call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    peer.collect()
    assert not notices(client.collect())
    assert sql(f'SELECT character_guid FROM game_chat_channel_member WHERE character_guid = {peer_guid}')[0]['rows']
    peer.send(0x4b)
    peer.collect()
    assert notice(12, struct.pack('<QBB', guid, 2, 3)) in notices(client.collect())
    assert sql(f'SELECT owner_guid FROM game_chat_channel WHERE channel_id = {channel_id}')[0]['rows'] == [[guid]]
    assert not sql(f'SELECT id FROM game_chat_channel_member WHERE character_guid = {peer_guid}')[0]['rows']
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    command(peer, 0x9a)
    assert notices(peer.collect()) == [notice(5)]
    command(client, 0xa3, b'Missingfriend\0')
    assert notices(client.collect()) == [notice(9, b'Missingfriend\0')]
    command(client, 0x98)
    assert notice(3) in notices(client.collect())
    assert not sql(f'SELECT channel_id FROM game_chat_channel WHERE channel_id = {channel_id}')[0]['rows']
    assert not sql(f'SELECT id FROM game_chat_channel_ban WHERE channel_id = {channel_id}')[0]['rows']
    command(client, 0x97, b'\0', channel=b'WorldDefense\0')
    client.collect()
    command(client, 0x9e, channel=b'WorldDefense\0')
    assert notices(client.collect()) == [bytes([11]) + b'WorldDefense\0Nobody\0']
    client.send(0x95, struct.pack('<II', 14, 7) + b'WorldDefense\0read only\0')
    assert notices(client.collect()) == [bytes([17]) + b'WorldDefense\0']
    command(client, 0x98, channel=b'WorldDefense\0')
    client.collect()
    sql(f'UPDATE game_character SET gm_level = 0 WHERE guid = {guid}')
    call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    client.collect()


def check_addon(client, peer, call, sql):
    def rows(query):
        return sql(query)[0]['rows']

    def send(world, text, kind=1, target=''):
        body = struct.pack('<II', kind, 0xffffffff)
        if kind in (6, 14):
            body += target.encode() + b'\0'
        world.send(0x95, body + text.encode() + b'\0')

    def replies(world, guid):
        texts = []
        for opcode, body in world.collect():
            if opcode != 0x96 or struct.unpack_from('<I', body, 1)[0] != 0xffffffff:
                continue
            assert body[0] == 6 and struct.unpack_from('<Q', body, 5)[0] == guid
            length = struct.unpack_from('<I', body, 13)[0]
            assert len(body) == 18 + length and body[-2:] == bytes(2)
            texts.append(body[17:-2].decode())
        return texts

    worlds = []
    try:
        for index, name in enumerate(['Addonone', 'Addontwo']):
            username = name.upper()
            call('provision_account', [username, list(bytes(32)), list(bytes(32))])
            account = rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0]
            call('establish_session', [account, list(KEY), ['0x' + bytes([150 + index]).hex() * 32]])
            call('create_character', [account, name, 1, 1, 0, 0, 0, 0, 0, 0])
            guid = rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
            sql(f'UPDATE game_character SET map_id = 0, x = 2000, y = 1000, z = 0 WHERE guid = {guid}')
            world = World(client.socket.getpeername()[1], username=username)
            worlds.append((world, guid))
            assert world.read()[0] == 0x1ee
            send(world, 'STC\tv1|ping|0|1/1|before login', 6, 'Addonone')
            assert not replies(world, guid)
            world.send(0x3d, struct.pack('<Q', guid))
            world.collect()
        first, guid = worlds[0]
        second, other_guid = worlds[1]
        for index in range(21):
            send(first, f'STC\tv1|ping|{index}|1/1|burst {index}', 6, 'Addontwo')
        assert replies(first, guid) == [f'STC\tv1|pong|0|1/1|burst {index}' for index in range(20)]
        assert not replies(second, other_guid)
        first.send(0x4b)
        first.collect()
        first.send(0x3d, struct.pack('<Q', guid))
        first.collect()
        send(first, 'STC\tv1|ping|21|1/1|after logout')
        assert not replies(first, guid)
        time.sleep(0.55)
        send(first, 'STC\tv1|ping|22|1/1|refilled')
        assert replies(first, guid) == ['STC\tv1|pong|0|1/1|refilled']
        first.close()
        first = World(client.socket.getpeername()[1], username='ADDONONE')
        worlds[0] = (first, guid)
        assert first.read()[0] == 0x1ee
        first.send(0x3d, struct.pack('<Q', guid))
        first.collect()
        send(first, 'STC\tv1||ping||23||1/1||a||||b', 6, 'Addontwo')
        assert replies(first, guid) == ['STC\tv1|pong|0|1/1|a||b']
        for text in ['STC\tv2|ping|0|1/1|bad', 'STC\tv1|ping|0|1/2|bad', 'STC\tbroken']:
            send(first, text)
        assert not replies(first, guid)
        assert not replies(second, other_guid)

        def command(world, issuer, payload, outcome):
            send(world, f'STC\tv1|fixture.command|0|1/1|{payload}')
            response = replies(world, issuer)
            prefix = 'STC\tv1|fixture.command.result|0|1/1|'
            assert len(response) == 1 and response[0].startswith(prefix), response
            result = response[0][len(prefix):]
            if outcome == 'Malformed':
                assert result == outcome
                return
            intent_id, actual = result.split('|')
            assert actual == outcome, result
            assert rows(f'SELECT pending, claim_token FROM game_party_command_intent WHERE id = {intent_id}') == [[False, 0]]
            assert not rows(f'SELECT id FROM game_party_command_receipt WHERE intent_id = {intent_id}')
            assert not rows('SELECT lane FROM game_party_command_dispatch_lane')
            return int(intent_id)

        command(first, guid, 'bad', 'Malformed')
        command(first, guid, '1', 'NotMember')
        first.send(0x6e, b'Addontwo\0')
        first.collect()
        second.collect()
        second.send(0x72)
        second.collect()
        first.collect()
        call('debug_spawn_player_entity', [1])
        call('debug_set_sessionless_action_consent', [1, True])
        call('debug_teleport', [1, 0, 2001, 1000, 0, 0])
        first.send(0x6e, b'Tester\0')
        first.collect()
        second.collect()
        command(second, other_guid, '1', 'NotLeader')
        command(first, guid, '999999999', 'MissingBot')
        command(first, guid, str(other_guid), 'WrongAccount')
        command(first, guid, f'1|{other_guid}', 'Applied')
        command(first, guid, '1|999999999', 'NotMember')
        call('debug_teleport', [other_guid, 1, 2000, 1000, 0, 0])
        second.collect()
        first.collect()
        command(first, guid, f'1|{other_guid}', 'WrongPartition')
        command(first, guid, '1', 'Applied')
        call('debug_create_fixture_instance', [1])
        first.collect()
        command(first, guid, '1', 'WrongPartition')
        call('debug_enter_instance', [1, 0])
        first.collect()
        call('debug_set_sessionless_action_consent', [1, False])
        command(first, guid, '1', 'Suppressed')
        call('debug_set_sessionless_action_consent', [1, True])
        command(first, guid, '1|0|999999999', 'TargetUnavailable')
        assert not replies(second, other_guid)
        first.send(0x75, b'Tester\0')
        first.collect()
        first.send(0x7b)
        first.collect()
        second.collect()
        check_companion_actions(first, second, call, sql, guid, other_guid)
    finally:
        for world, _ in worlds:
            world.close()
        client.collect()
        peer.collect()


def check_companion_actions(first, second, call, sql, guid, other_guid):
    assert not sql('SELECT guid FROM game_aoi_viewer WHERE guid = 1')[0]['rows']
    assert not sql('SELECT id FROM game_aoi_grant WHERE viewer_guid = 1')[0]['rows']
    def rows(query):
        return sql(query)[0]['rows']

    def group_action(target=0, leave=False):
        call('debug_emit_sessionless_group_intent', [1, target, leave])
        assert not rows('SELECT id FROM game_bot_invite_intent')

    call('debug_set_sessionless_action_consent', [1, False])
    group_action(guid)
    assert not any(op == 0x6f for op, _ in first.collect())
    assert not rows(f'SELECT id FROM game_group_invite WHERE target_guid = {guid}')
    call('debug_set_sessionless_action_consent', [1, True])
    group_action(guid)
    assert (0x6f, b'Tester\0') in first.collect()
    first.send(0x72)
    first.collect()
    assert rows('SELECT leader_guid FROM game_group WHERE leader_guid = 1') == [[1]]
    sql(f'UPDATE game_character SET race = 2 WHERE guid = {other_guid}')
    group_action(other_guid)
    assert not any(op == 0x6f for op, _ in second.collect())
    assert not rows(f'SELECT id FROM game_group_invite WHERE target_guid = {other_guid}')
    sql(f'UPDATE game_character SET race = 1 WHERE guid = {other_guid}')
    call('debug_set_sessionless_action_consent', [1, False])
    group_action(leave=True)
    assert rows('SELECT id FROM game_group_member WHERE character_guid = 1')
    call('debug_set_sessionless_action_consent', [1, True])
    group_action(leave=True)
    assert (0x7c, b'') in first.collect()
    assert not rows('SELECT id FROM game_group_member WHERE character_guid = 1')

    second.send(0x4b)
    second.collect()
    call('debug_spawn_player_entity', [other_guid])
    call('debug_set_sessionless_action_consent', [other_guid, False])
    group_action(other_guid)
    assert rows(f'SELECT id FROM game_group_invite WHERE target_guid = {other_guid}')
    assert not rows(f'SELECT id FROM game_group_member WHERE character_guid = {other_guid}')
    call('debug_set_sessionless_action_consent', [other_guid, True])
    group_action(other_guid)
    assert not rows(f'SELECT id FROM game_group_invite WHERE target_guid = {other_guid}')
    assert rows(f'SELECT id FROM game_group_member WHERE character_guid = {other_guid}')
    group_action(leave=True)
    assert not rows(f'SELECT id FROM game_group_member WHERE character_guid = {other_guid}')
    second.send(0x3d, struct.pack('<Q', other_guid))
    second.collect()

    entrance, exit_trigger = 5090650, 5090651
    for trigger, source_map, destination_map in [(entrance, 0, 36), (exit_trigger, 36, 0)]:
        sql(f'INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) VALUES ({trigger},{source_map},4000,1000,0,5,0,0,0,0)')
        sql(f"INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) VALUES ({trigger},{destination_map},4000,1000,0,0,'Companion crossing')")
    for character in [1, guid, other_guid]:
        call('debug_set_level', [character, 60])
        call('debug_teleport', [character, 0, 4000, 1000, 0, 0])
    first.collect()
    second.collect()
    first.send(0x6e, b'Tester\0')
    first.collect()
    second.collect()
    first.send(0xb4, struct.pack('<I', entrance))
    first.collect()
    second.collect()
    instance = rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {guid}')[0][0]
    assert instance != 0

    def relocate(trigger, target_map, target_instance, generation, expected=0):
        call('debug_replay_sessionless_areatrigger', [1, trigger, target_map, target_instance, generation, expected])

    def refused(trigger, target_map, target_instance, text):
        before = rows('SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = 1')
        bindings = rows('SELECT instance_id FROM game_instance_binding WHERE character_guid = 1')
        try:
            relocate(trigger, target_map, target_instance, 101)
            raise AssertionError('Companion crossing unexpectedly committed')
        except RuntimeError as error:
            assert text in str(error), str(error)
        assert rows('SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = 1') == before
        assert rows('SELECT instance_id FROM game_instance_binding WHERE character_guid = 1') == bindings
        assert not rows('SELECT id FROM game_character_relocation WHERE character_guid = 1')

    call('debug_set_sessionless_action_consent', [1, False])
    refused(entrance, 36, instance, 'CannotAct')
    call('debug_set_sessionless_action_consent', [1, True])
    refused(0, 36, instance, 'MissingResource')
    refused(entrance, 37, instance, 'OtherPartition')
    refused(entrance, 36, instance + 1000, 'OtherPartition')
    call('debug_teleport', [1, 0, 4010, 1000, 0, 0])
    refused(entrance, 36, instance, 'OutOfRange')
    call('debug_teleport', [1, 0, 4000, 1000, 0, 0])
    first.collect()
    second.collect()
    vitals = rows('SELECT health, power, money FROM game_world_entity WHERE guid = 1')
    relocate(entrance, 36, instance, 101)
    assert rows('SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = 1') == [[36, instance, 4000, 1000, 0]]
    assert rows('SELECT health, power, money FROM game_world_entity WHERE guid = 1') == vitals
    assert rows('SELECT map_id, pending_instance_id, online FROM game_character WHERE guid = 1') == [[36, instance, False]]
    assert 1 in creates(first.collect())
    assert 1 in destroys(second.collect())
    receipt = rows('SELECT id FROM game_character_relocation WHERE character_guid = 1')[0][0]
    assert not rows('SELECT id FROM game_bot_transfer_intent WHERE bot_guid = 1')
    assert not rows('SELECT transfer_id FROM game_transfer_out WHERE character_guid = 1')
    assert not rows('SELECT transfer_id FROM game_transfer_in WHERE character_guid = 1')
    call('debug_admit_sessionless_action', [1])
    relocate(entrance, 36, instance, 101, receipt)
    assert not creates(first.collect())
    assert not destroys(second.collect())
    relocate(exit_trigger, 0, 0, 102)
    assert rows('SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = 1') == [[0, 0, 4000, 1000, 0]]
    assert 1 in destroys(first.collect())
    assert 1 in creates(second.collect())
    returned = rows('SELECT id FROM game_character_relocation WHERE character_guid = 1')
    assert len(returned) == 1 and returned[0][0] != receipt
    first.send(0xb4, struct.pack('<I', exit_trigger))
    first.collect()
    second.collect()
    first.send(0x7b)
    first.collect()


def check_chat(client, peer, call, sql, guid, peer_guid):
    def send(connection, kind, message, language=7, target=None):
        body = struct.pack('<II', kind, language)
        if target is not None:
            body += target.encode() + b'\0'
        connection.send(0x95, body + message.encode() + b'\0')

    def line(kind, speaker, message, language=0, tag=0):
        body = struct.pack('<BIQ', kind, language, speaker)
        if kind in (0, 1, 5):
            body += struct.pack('<Q', speaker)
        encoded = message.encode() + b'\0'
        return 0x96, body + struct.pack('<I', len(encoded)) + encoded + bytes([tag])

    def chat(packets):
        return [packet for packet in packets if packet[0] == 0x96]

    sql(f'UPDATE game_character SET gm_level = 1 WHERE guid = {guid}')
    send(client, 0, '  native say  ')
    expected = line(0, guid, 'native say', 7)
    assert chat(client.collect()) == [expected]
    assert chat(peer.collect()) == [expected]
    send(client, 0, 'unknown language', 1)
    assert (0x1cb, b"You don't know that language\0") in client.collect()
    assert not chat(peer.collect())
    send(client, 0, 'addon say', 0xffffffff)
    assert not chat(client.collect())
    assert not chat(peer.collect())
    client.send(0x104, struct.pack('<IIQ', 101, 3, peer_guid))
    emote = client.collect()
    assert any(opcode == 0x105 and b'Peer\0' in body for opcode, body in emote)
    assert any(opcode == 0x103 for opcode, _ in emote)
    assert [packet for packet in peer.collect() if packet[0] in (0x103, 0x105)] == [packet for packet in emote if packet[0] in (0x103, 0x105)]
    call('debug_teleport', [peer_guid, 0, 320, 0, 100, 0])
    peer.collect()
    client.collect()
    assert not sql(f'SELECT id FROM game_aoi_grant WHERE viewer_guid = {peer_guid} AND entity_guid = {guid} AND layer = 0')[0]['rows']
    send(client, 0, 'outside say')
    assert chat(client.collect()) == [line(0, guid, 'outside say', 7)]
    assert not chat(peer.collect())
    send(client, 5, 'yell edge')
    expected = line(5, guid, 'yell edge', 7)
    assert chat(client.collect()) == [expected]
    assert chat(peer.collect()) == [expected]
    call('debug_teleport', [peer_guid, 0, 321, 0, 0, 0])
    peer.collect()
    client.collect()
    send(client, 5, 'beyond yell')
    client.collect()
    assert not chat(peer.collect())
    call('debug_teleport', [peer_guid, 1, 11, 0, 0, 0])
    peer.collect()
    client.collect()
    send(client, 5, 'other map')
    client.collect()
    assert not chat(peer.collect())
    send(client, 6, 'cross map whisper', target='pEeR')
    assert chat(client.collect()) == [line(7, peer_guid, 'cross map whisper')]
    assert chat(peer.collect()) == [line(6, guid, 'cross map whisper')]
    send(peer, 20, 'Back soon')
    peer.collect()
    send(client, 6, 'away whisper', target='Peer')
    assert chat(client.collect()) == [line(7, peer_guid, 'away whisper'), line(20, peer_guid, 'Back soon')]
    assert chat(peer.collect()) == [line(6, guid, 'away whisper')]
    sql(f"INSERT INTO game_character_contact (id,owner_guid,owner_identity,target_guid,is_ignore) VALUES (5090901,{peer_guid},0x{'22' * 32},{guid},true)")
    send(client, 6, 'ignored whisper', target='Peer')
    assert chat(client.collect()) == [line(7, peer_guid, 'ignored whisper'), line(20, peer_guid, 'Back soon'), line(22, peer_guid, 'Peer')]
    assert not chat(peer.collect())
    send(client, 1, 'party ignores the ignore list')
    expected = line(1, guid, 'party ignores the ignore list', 7)
    assert chat(client.collect()) == [expected]
    assert chat(peer.collect()) == [expected]
    send(client, 1, '  PREFIX\taddon bytes  ', 0xffffffff)
    expected = line(1, guid, '  PREFIX\taddon bytes  ', 0xffffffff)
    assert chat(client.collect()) == [expected]
    assert chat(peer.collect()) == [expected]
    sql('DELETE FROM game_character_contact WHERE id = 5090901')
    send(peer, 21, 'Busy')
    peer.collect()
    send(client, 6, 'dnd whisper', target='Peer')
    assert chat(client.collect()) == [line(7, peer_guid, 'dnd whisper'), line(21, peer_guid, 'Busy')]
    assert chat(peer.collect()) == [line(6, guid, 'dnd whisper')]
    send(peer, 6, 'dnd speaker', target='Native')
    peer.collect()
    assert chat(client.collect()) == [line(6, peer_guid, 'dnd speaker', tag=2)]
    peer.send(0x225, struct.pack('<Q', guid))
    peer.collect()
    assert chat(client.collect()) == [line(22, peer_guid, 'Peer')]
    peer.send(0x4b)
    peer.collect()
    send(client, 6, 'offline whisper', target='Peer')
    assert (0x2a9, b'Peer\0') in client.collect()
    sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    send(client, 6, 'enemy whisper', target='Peer')
    assert (0x219, b'') in client.collect()
    assert not chat(peer.collect())
    peer.send(0x4b)
    peer.collect()
    sql(f'UPDATE game_character SET race = 1 WHERE guid = {peer_guid}')
    peer.send(0x3d, struct.pack('<Q', peer_guid))
    peer.collect()
    call('debug_teleport', [peer_guid, 0, 11, 0, 0, 0])
    peer.collect()
    client.collect()
    sql(f'UPDATE game_character SET gm_level = 0 WHERE guid = {guid}')
    client.send(0x4b)
    client.collect()
    client.send(0x3d, struct.pack('<Q', guid))
    client.collect()
    peer.collect()
    time.sleep(1.05)
    for index in range(12):
        send(client, 0, f'burst {index}')
    burst = client.collect()
    assert chat(burst) == [line(0, guid, f'burst {index}', 7) for index in range(11)]
    assert any(opcode == 0x1cb and body.startswith(b'You must wait ') for opcode, body in burst)
    assert chat(peer.collect()) == chat(burst)
    client.send(0x4b)
    client.collect()
    client.send(0x3d, struct.pack('<Q', guid))
    client.collect()
    peer.collect()
    send(client, 0, 'still muted')
    assert any(opcode == 0x1cb for opcode, _ in client.collect())
    assert not chat(peer.collect())
    client.send(0x104, struct.pack('<IIQ', 101, 3, peer_guid))
    assert any(opcode == 0x1cb for opcode, _ in client.collect())
    assert not any(opcode in (0x103, 0x105) for opcode, _ in peer.collect())
    send(client, 20, 'Muted but away')
    client.collect()
    send(peer, 6, 'away during mute', target='Native')
    assert chat(peer.collect()) == [line(7, guid, 'away during mute'), line(20, guid, 'Muted but away')]
    assert chat(client.collect()) == [line(6, peer_guid, 'away during mute')]
    send(client, 0, 'unknown while muted', 1)
    assert (0x1cb, b"You don't know that language\0") in client.collect()
    send(client, 20, '')
    client.collect()
    peer.collect()


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def check_session_revocation(world_port, call, sql, await_empty):
    for index, cause in enumerate(['ban', 'missing', 'replaced', 'banignored']):
        username, name = 'REVOKE' + cause.upper(), 'Revoked' + chr(ord('a') + index)
        call('provision_account', [username, [0] * 32, [1] * 32])
        account = sql(f"SELECT id FROM game_account WHERE username = '{username}'")[0]['rows'][0][0]
        identity = [f'0x{index + 6000:064x}']
        call('establish_session', [account, list(KEY), identity])
        call('create_character', [account, name, 1, 1, 0, 0, 0, 0, 0, 0])
        guid = sql(f"SELECT guid FROM game_character WHERE name = '{name}'")[0]['rows'][0][0]
        client = World(world_port, username=username)
        try:
            assert client.read()[0] == 0x1ee
            client.send(0x3d, struct.pack('<Q', guid))
            assert guid in creates(client.collect())
            position = sql(f'SELECT x, y, z FROM game_world_entity WHERE guid = {guid}')[0]['rows']
            if cause.startswith('ban'):
                sql(f'UPDATE game_account SET banned = true WHERE id = {account}')
            elif cause == 'missing':
                sql(f'DELETE FROM game_session WHERE account_id = {account}')
            else:
                call('establish_session', [account, [0x59] * 40, identity])
            client.send(0x0331 if cause == 'banignored' else 0xee,
                struct.pack('<IIffffI', 0, 1000, 1000, 1000, 0, 0, 0))
            deadline = time.monotonic() + 5
            client.socket.settimeout(2)
            while time.monotonic() < deadline:
                try:
                    client.read()
                except EOFError:
                    break
                except socket.timeout:
                    raise AssertionError(f'{cause} did not close the World Session') from None
            else:
                raise AssertionError(f'{cause} did not close the World Session')
            await_empty('game_native_player')
            assert sql(f'SELECT x, y, z FROM game_character WHERE guid = {guid}')[0]['rows'] == position
            assert not sql(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')[0]['rows']
        finally:
            client.close()


def check_slow_readers(world_port, call, sql, await_empty):
    call('debug_seed_scenario_fixtures', [])
    clients = []
    sender = None
    stopped = threading.Event()
    try:
        for index, name in enumerate(['Slowreader', 'Fastreader']):
            username = name.upper()
            call('provision_account', [username, [0] * 32, [1] * 32])
            account = sql(f"SELECT id FROM game_account WHERE username = '{username}'")[0]['rows'][0][0]
            call('establish_session', [account, list(KEY), [f'0x{index + 5000:064x}']])
            call('create_character', [account, name, 1, 1, 0, 0, 0, 0, 0, 0])
            guid = sql(f"SELECT guid FROM game_character WHERE name = '{name}'")[0]['rows'][0][0]
            client = World(world_port, username=username)
            clients.append((client, guid))
            assert client.read()[0] == 0x1ee
            client.send(0x3d, struct.pack('<Q', guid))
            assert guid in creates(client.collect())
        slow, slow_guid = clients[0]
        healthy, healthy_guid = clients[1]
        slow.collect()
        healthy.collect()
        item = sql(f'SELECT entry FROM game_item_instance WHERE owner_guid = {slow_guid}')[0]['rows'][0][0]
        slow.socket.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 2048)
        slow.socket.settimeout(0.5)

        def flood():
            try:
                while not stopped.is_set():
                    slow.send(0x56, struct.pack('<IQ', item, 0))
            except OSError:
                pass

        sender = threading.Thread(target=flood)
        sender.start()
        started, latencies = time.monotonic(), []
        while time.monotonic() - started < 15:
            ping_started = time.monotonic()
            healthy.collect()
            latencies.append(time.monotonic() - ping_started)
            live = sql('SELECT character_guid FROM game_native_player')[0]['rows']
            assert [healthy_guid] in live
            if [slow_guid] not in live:
                break
            time.sleep(0.05)
        else:
            raise AssertionError('slow reader remained connected after its output blocked')
        assert max(latencies) < 1, latencies
        print(json.dumps({'check': 'slow_reader_isolation',
            'slow_reader_closed_seconds': time.monotonic() - started,
            'healthy_request_max_ms': max(latencies) * 1000}, indent=2))
    finally:
        stopped.set()
        for client, _ in clients:
            client.close()
        if sender:
            sender.join(timeout=2)
            assert not sender.is_alive(), 'slow-reader sender did not stop'
    await_empty('game_native_player')
    await_empty('game_native_world_connection')
    await_empty('game_aoi_viewer')
    await_empty('game_aoi_grant')
    print('PASS native slow-reader isolation and cleanup')


def check_recovery(world_port, logon_port, call, sql, restart, await_empty):
    call('debug_seed_scenario_fixtures', [])
    salt = secrets.token_bytes(32)
    call('provision_account', ['RECOVERY', list(salt), list(verifier('RECOVERY', 'PASSWORD', salt))])
    account = sql("SELECT id FROM game_account WHERE username = 'RECOVERY'")[0]['rows'][0][0]
    logon = Logon(logon_port)
    assert logon.challenge('RECOVERY') == 0
    status, key = logon.prove('PASSWORD')
    assert status == 0
    call('create_character', [account, 'Recovered', 1, 1, 0, 0, 0, 0, 0, 0])
    guid = sql("SELECT guid FROM game_character WHERE name = 'Recovered'")[0]['rows'][0][0]
    client = World(world_port, username='RECOVERY', key=key)
    assert client.read()[0] == 0x1ee
    client.send(0x3d, struct.pack('<Q', guid))
    assert guid in creates(client.collect())
    client.send(0xb7, struct.pack('<IIffffI', 0, 1000, 1000, 1000, 0, 0, 0))
    client.send(0x97, b'Recovery channel\0\0')
    client.collect()
    assert sql(f'SELECT x, y, z FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[1000, 1000, 0]]
    assert sql(f'SELECT id FROM game_chat_channel_member WHERE character_guid = {guid}')[0]['rows']
    items = sql(f'SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}')[0]['rows']
    assert items
    restart()
    client.close()
    logon.close()
    for table in ['game_native_player', 'game_native_world_connection', 'game_native_cipher',
                  'game_native_logon_connection', 'game_native_admission', 'game_aoi_viewer', 'game_aoi_grant',
                  'game_chat_channel_member']:
        await_empty(table)
    assert not sql(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')[0]['rows']
    assert sql(f'SELECT online, x, y, z FROM game_character WHERE guid = {guid}')[0]['rows'] == [[False, 1000, 1000, 0]]
    assert sorted(sql(f'SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}')[0]['rows']) == sorted(items)
    assert all(row[0] == 0 for row in sql('SELECT active FROM game_native_logon_address')[0]['rows'])
    client = World(world_port, username='RECOVERY', key=key)
    try:
        assert client.read()[0] == 0x1ee
        client.send(0x3d, struct.pack('<Q', guid))
        assert creates(client.collect()).count(guid) == 1
        assert sql(f'SELECT x, y, z FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[1000, 1000, 0]]
        client.send(0x97, b'Recovery channel\0\0')
        assert any(opcode == 0x99 for opcode, _ in client.collect())
    finally:
        client.close()
    await_empty('game_native_player')
    print('PASS native crash recovery with durable Character, inventory and fresh AOI')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--host', type=Path, required=True)
    parser.add_argument('--load-clients', type=int, default=0)
    parser.add_argument('--load-seconds', type=float, default=30)
    parser.add_argument('--load-hz', type=float, default=10)
    parser.add_argument('--load-group-size', type=int, default=16)
    parser.add_argument('--check-recovery', action='store_true')
    parser.add_argument('--check-slow-readers', action='store_true')
    parser.add_argument('--check-sessions', action='store_true')
    parser.add_argument('--check-admission', action='store_true')
    args = parser.parse_args()
    if (not 0 <= args.load_clients <= 4096 or args.load_group_size <= 0
            or any(not math.isfinite(value) or value <= 0 for value in [args.load_seconds, args.load_hz])):
        parser.error('load bounds require 0-4096 clients and positive duration, frequency and group size')
    if sum([bool(args.load_clients), args.check_recovery, args.check_slow_readers, args.check_sessions, args.check_admission]) > 1:
        parser.error('select one of load measurement, crash recovery, slow readers, Sessions or admission')
    with tempfile.TemporaryDirectory(prefix='lyracore-native-check-') as directory:
        run = Path(directory)
        private, public = run / 'private.pem', run / 'public.pem'
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'EC', '-pkeyopt', 'ec_paramgen_curve:P-256', '-out', str(private)], check=True, capture_output=True)
        private.chmod(0o600)
        subprocess.run(['openssl', 'ec', '-in', str(private), '-pubout', '-out', str(public)], check=True, capture_output=True)
        ports = set()
        while len(ports) < 3:
            ports.add(free_port())
        http_port, world_port, logon_port = ports
        database = f'disposable-native-{time.time_ns()}'
        address = f'http://127.0.0.1:{http_port}'
        env = dict(os.environ, LYRACORE_NATIVE_WORLD_ADDR=f'127.0.0.1:{world_port}', LYRACORE_NATIVE_LOGON_ADDR=f'127.0.0.1:{logon_port}', LYRACORE_NATIVE_DATABASE=database, RUST_LOG='warn')
        token = None

        def request(path, body=None, method='GET', content_type='application/json', authenticated=True):
            headers = {'Content-Type': content_type}
            if token and authenticated:
                headers['Authorization'] = 'Bearer ' + token
            req = urllib.request.Request(address + path, body, headers, method=method)
            try:
                with urllib.request.urlopen(req, timeout=120) as response:
                    return response.read()
            except urllib.error.HTTPError as error:
                raise RuntimeError(f'{path}: HTTP {error.code}: {error.read().decode()}') from error

        def call(name, args):
            return request(f'/v1/database/{database}/call/{name}', json.dumps(args).encode(), 'POST')

        def sql(query):
            return json.loads(request(f'/v1/database/{database}/sql', query.encode(), 'POST', 'text/plain'))

        def await_empty(table):
            for _ in range(100):
                if not sql(f'SELECT * FROM {table}')[0]['rows']:
                    return
                time.sleep(0.05)
            raise AssertionError(f'{table} retained disconnected clients')

        with (run / 'host.log').open('w') as log:
            def start_host():
                return subprocess.Popen([str(args.host), 'start', '--listen-addr', f'127.0.0.1:{http_port}', '--data-dir', str(run / 'data'), '--non-interactive', '--jwt-priv-key-path', str(private), '--jwt-pub-key-path', str(public), '--page_pool_max_size', '1073741824'], env=env, stdout=log, stderr=subprocess.STDOUT)

            def restart():
                nonlocal host
                host.kill()
                host.wait(timeout=10)
                host = start_host()
                for _ in range(200):
                    try:
                        sql('SELECT id FROM game_account')
                        return
                    except (OSError, RuntimeError):
                        time.sleep(0.1)
                raise AssertionError('native host did not recover')

            host = start_host()
            try:
                for _ in range(200):
                    try:
                        identity = json.loads(request('/v1/identity', b'', 'POST'))
                        token = identity['token']
                        break
                    except (OSError, RuntimeError):
                        time.sleep(0.1)
                assert token, 'native host did not start'
                wasm = ROOT / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'
                request(f'/v1/database/{database}?host_type=Wasm', wasm.read_bytes(), 'PUT', 'application/wasm')
                call('claim_operator', [])
                for name, arguments in [('debug_age_corpse', [0, 600]), ('debug_backfill_cell_ids', []), ('native_set_admission_limits', [1, 1, 1])]:
                    try:
                        request(f'/v1/database/{database}/call/{name}', json.dumps(arguments).encode(), 'POST', authenticated=False)
                    except RuntimeError as refusal:
                        assert 'operator only' in str(refusal), refusal
                    else:
                        raise AssertionError(f'{name} accepted an anonymous caller')
                call('install_guid_range', [0])
                if args.check_admission:
                    import admission
                    call('debug_seed_scenario_fixtures', [])
                    admission.run(world_port, call, sql, await_empty, idle_seconds=92)
                    return
                if args.check_sessions:
                    call('debug_seed_scenario_fixtures', [])
                    check_session_revocation(world_port, call, sql, await_empty)
                    print('PASS native Session revocation before in-world actions')
                    return
                if args.check_slow_readers:
                    check_slow_readers(world_port, call, sql, await_empty)
                    return
                if args.check_recovery:
                    check_recovery(world_port, logon_port, call, sql, restart, await_empty)
                    return
                if args.load_clients:
                    import asyncio
                    import load
                    asyncio.run(load.run(host.pid, world_port, call, sql, args.load_clients,
                        args.load_seconds, args.load_hz, args.load_group_size))
                    return
                salt = secrets.token_bytes(32)
                call('provision_account', ['NATIVE', list(salt), list(verifier('NATIVE', 'PASSWORD', salt))])
                account_id = sql("SELECT id FROM game_account WHERE username = 'NATIVE'")[0]['rows'][0][0]
                logon = Logon(logon_port)
                assert logon.challenge('NATIVE') == 0
                started = time.monotonic()
                assert logon.prove('WRONG') == (5, None)
                assert time.monotonic() - started >= 0.18
                assert not sql(f'SELECT account_id FROM game_session WHERE account_id = {account_id}')[0]['rows']
                assert logon.challenge('native') == 0
                status, native_key = logon.prove('PASSWORD')
                assert status == 0
                assert sql(f'SELECT session_key FROM game_session WHERE account_id = {account_id}')[0]['rows'][0][0] == native_key.hex()
                realms = logon.realms()
                assert realms[4] >= 1
                logon.close()
                stale = World(world_port, key=native_key)
                assert stale.read()[0] == 0x1ee
                logon = Logon(logon_port)
                assert logon.challenge('NATIVE') == 0
                status, native_key = logon.prove('PASSWORD')
                assert status == 0
                logon.close()
                stale.send(0x37)
                assert stale.socket.recv(1) == b''
                stale.close()
                for options, expected in [({'valid_proof': False}, 13), ({'build': 1234}, 20), ({'username': 'UNKNOWN'}, 21)]:
                    client = World(world_port, key=native_key, **options)
                    assert client.read(False) == (0x1ee, bytes([expected])), options
                    assert client.socket.recv(1) == b''
                    client.close()
                client = World(world_port, key=native_key)
                assert client.read() == (0x1ee, bytes([12]) + bytes(9))
                ignored_requests(client)
                client.send(0x1dc, struct.pack('<II', 123, 20))
                assert client.read() == (0x1dd, struct.pack('<I', 123))
                client.send(0x37)
                assert client.read() == (0x3b, b'\0')
                client.send(0x1ce)
                opcode, body = client.read()
                assert opcode == 0x1cf and abs(struct.unpack('<I', body)[0] - time.time()) < 5
                call('debug_seed_scenario_fixtures', [])
                character_body = b'Native\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0])
                client.send(0x36, character_body)
                assert client.read() == (0x3a, bytes([0x2e]))
                client.send(0x36, character_body)
                assert client.read() == (0x3a, bytes([0x31]))
                allocator = sql('SELECT high_water FROM game_guid_allocator WHERE id = 0')[0]['rows'][0][0]
                old_size = sql('SELECT size FROM game_guid_range WHERE id = 0')[0]['rows'][0][0]
                sql(f'UPDATE game_guid_range SET size = {allocator + 2} WHERE id = 0')
                client.send(0x36, b'Rollback\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
                assert client.read() == (0x3a, bytes([0x2f]))
                assert not sql("SELECT guid FROM game_character WHERE name = 'Rollback'")[0]['rows']
                assert sql('SELECT high_water FROM game_guid_allocator WHERE id = 0')[0]['rows'][0][0] == allocator
                sql(f'UPDATE game_guid_range SET size = {old_size} WHERE id = 0')
                guid = sql("SELECT guid FROM game_character WHERE name = 'Native'")[0]['rows'][0][0]
                sql(f'UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {guid}')
                client.send(0x3d, struct.pack('<Q', 1))
                assert client.read() == (0x41, bytes([1]))
                assert not sql('SELECT * FROM game_native_player')[0]['rows']
                client.send(0x3d, struct.pack('<Q', guid))
                initial = client.collect()
                assert (0x2f4, bytes(12) + bytes([1])) in initial
                assert initial[0][0] == 0x236, [hex(opcode) for opcode, _ in initial]
                assert creates(initial).count(guid) == 1
                original_position = sql(f'SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {guid}')[0]['rows']
                ignored_requests(client)
                assert sql(f'SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == original_position
                client.send(0x26a, struct.pack('<Q', guid))
                assert not any(opcode == 0x96 for opcode, _ in client.collect())
                client.send(0x26a, struct.pack('<Q', 0xffff_ffff))
                assert any(opcode == 0x96 for opcode, _ in client.collect())
                client.send(0xfe, struct.pack('<I', 15))
                assert sum(opcode == 0x96 for opcode, _ in client.collect()) == 1
                client.send(0xfe, struct.pack('<I', 15))
                assert not any(opcode == 0x96 for opcode, _ in client.collect())
                client.send(0x21a)
                assert (0x21b, bytes(4)) in client.collect()
                client.send(0x60, struct.pack('<IQ', 51000, 0))
                queries = client.collect()
                assert any(opcode == 0x61 and body.startswith(struct.pack('<I', 51000) + b'Test Wolf\0') for opcode, body in queries)
                client.send(0x56, struct.pack('<IQ', 0x7fff_ffff, 0))
                assert (0x58, struct.pack('<I', 0xffff_ffff)) in client.collect()
                client.send(0x5e, struct.pack('<IQ', 0x7fff_ffff, 0))
                assert (0x5f, struct.pack('<I', 0xffff_ffff)) in client.collect()
                client.send(0x17f, struct.pack('<IQ', 0x7fff_ffff, 0))
                assert any(opcode == 0x180 and body[:4] == struct.pack('<I', 0x7fff_ffff) for opcode, body in client.collect())
                client.send(0x50, struct.pack('<Q', 0xffff_ffff))
                assert not any(opcode == 0x51 for opcode, _ in client.collect())
                call('provision_account', ['PEER', list(bytes(32)), list(bytes(32))])
                peer_account = sql("SELECT id FROM game_account WHERE username = 'PEER'")[0]['rows'][0][0]
                call('establish_session', [peer_account, list(KEY), ['0x' + '22' * 32]])
                call('create_character', [peer_account, 'Peer', 1, 8, 0, 0, 0, 0, 0, 0])
                peer_guid = sql("SELECT guid FROM game_character WHERE name = 'Peer'")[0]['rows'][0][0]
                sql(f'UPDATE game_character SET x = 10, y = 0, z = 0 WHERE guid = {peer_guid}')
                peer = World(world_port, username='PEER')
                assert peer.read()[0] == 0x1ee
                peer.send(0x3d, struct.pack('<Q', peer_guid))
                initial = peer.collect()
                assert creates(initial).count(peer_guid) == 1
                assert creates(initial).count(guid) == 1
                assert creates(client.collect()).count(peer_guid) == 1
                call('debug_grant_item', [guid, 5090052, 6])
                food_guid, food_slot = sql(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5090052')[0]['rows'][0]
                grant = client.collect()
                assert food_guid in creates(grant)
                assert item_pointer(values(grant, guid), food_slot) == food_guid
                assert any(opcode == 0x166 for opcode, _ in grant)
                used = {row[0] for row in sql(f'SELECT slot FROM game_item_instance WHERE owner_guid = {guid}')[0]['rows']}
                split_slot = next(slot for slot in range(23, 39) if slot not in used)
                client.send(0x10e, bytes([255, food_slot, 255, split_slot, 2]))
                split = client.collect()
                split_guid, count = sql(f'SELECT guid, stack_count FROM game_item_instance WHERE owner_guid = {guid} AND slot = {split_slot}')[0]['rows'][0]
                assert count == 2
                assert split_guid in creates(split)
                assert values(split, food_guid)[14] == 4
                assert item_pointer(values(split, guid), split_slot) == split_guid
                call('debug_grant_item', [guid, 51, 1])
                tunic_guid, tunic_slot = sql(f'SELECT guid, slot FROM game_item_instance WHERE owner_guid = {guid} AND entry = 51 AND slot >= 23')[0]['rows'][0]
                client.collect()
                peer.collect()
                client.send(0x10d, bytes([food_slot, tunic_slot]))
                swapped = client.collect()
                pointers = values(swapped, guid)
                assert item_pointer(pointers, food_slot) == tunic_guid
                assert item_pointer(pointers, tunic_slot) == food_guid
                assert not any(field >= 486 and field < 612 for field in values(peer.collect(), guid))
                sql('UPDATE game_spell SET cost = 999999 WHERE spell_id = 50115')
                client.send(0xab, bytes([255, split_slot, 0, 0, 0]))
                refused = client.collect()
                assert any(opcode == 0x112 for opcode, _ in refused)
                assert not values(refused, split_guid)
                assert split_guid not in destroys(refused)
                assert sql(f'SELECT stack_count FROM game_item_instance WHERE guid = {split_guid}')[0]['rows'] == [[2]]
                sql('UPDATE game_spell SET cost = 0 WHERE spell_id = 50115')
                client.send(0x111, bytes([255, split_slot, 0, 0, 0, 0]))
                removed = client.collect()
                assert split_guid in destroys(removed)
                assert item_pointer(values(removed, guid), split_slot) == 0
                assert not sql(f'SELECT guid FROM game_item_instance WHERE guid = {split_guid}')[0]['rows']
                weapon_guid = sql(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND slot = 15')[0]['rows'][0][0]
                client.send(0x10b, bytes([255, 15, 255]))
                unequipped = client.collect()
                weapon_slot = sql(f'SELECT slot FROM game_item_instance WHERE guid = {weapon_guid}')[0]['rows'][0][0]
                assert weapon_slot >= 23
                assert item_pointer(values(unequipped, guid), 15) == 0
                assert item_pointer(values(unequipped, guid), weapon_slot) == weapon_guid
                assert any(opcode == 0xa9 for opcode, _ in peer.collect())
                client.send(0x10a, bytes([255, weapon_slot]))
                equipped = client.collect()
                assert item_pointer(values(equipped, guid), 15) == weapon_guid
                assert item_pointer(values(equipped, guid), weapon_slot) == 0
                assert sql(f'SELECT slot FROM game_item_instance WHERE guid = {weapon_guid}')[0]['rows'] == [[15]]
                peer.collect()
                call('debug_spawn_at_feet', [guid, 51004, 2])
                vendor = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51004')[0]['rows'])
                sql(f'UPDATE game_world_entity SET money = 50000 WHERE guid = {guid}')
                client.collect()
                peer.collect()
                client.send(0x19e, struct.pack('<Q', vendor))
                stock = next(body for opcode, body in client.collect() if opcode == 0x19f)
                assert stock[:9] == struct.pack('<QB', vendor, 2)
                assert struct.unpack_from('<I', stock, 9 + 28 + 16)[0] == 10
                client.send(0x1a2, struct.pack('<QIBB', vendor, 5090052, 2, 1))
                bought = client.collect()
                assert values(bought, food_guid)[14] == 6
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[49980]]
                client.send(0x1a2, struct.pack('<QIBB', vendor, 5090050, 17, 1))
                refused = client.collect()
                assert any(opcode == 0x1a5 for opcode, _ in refused)
                assert not values(refused, guid) and not creates(refused)
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[49980]]
                assert not sql(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5090050')[0]['rows']
                client.send(0x1a0, struct.pack('<QQB', vendor, food_guid, 2))
                sold = client.collect()
                assert values(sold, food_guid)[14] == 4
                assert item_pointer(values(sold, guid), 69) == 0x4090_0000_0000_0000
                assert values(sold, guid)[1226] == 4
                assert sql(f'SELECT stack_count, price FROM game_character_buyback WHERE player_guid = {guid}')[0]['rows'] == [[2, 4]]
                client.send(0x290, struct.pack('<QI', vendor, 69))
                bought_back = client.collect()
                assert values(bought_back, food_guid)[14] == 6
                assert item_pointer(values(bought_back, guid), 69) == 0
                assert 0x4090_0000_0000_0000 in destroys(bought_back)
                assert not sql(f'SELECT id FROM game_character_buyback WHERE player_guid = {guid}')[0]['rows']
                sql(f'UPDATE game_world_entity SET npc_flags = 261 WHERE guid = {vendor}')
                client.send(0x1b7, struct.pack('<Q', vendor))
                assert (0x1b8, struct.pack('<Q', vendor)) in client.collect()
                client.send(0x283, bytes([255, tunic_slot]))
                banked = client.collect()
                assert item_pointer(values(banked, guid), tunic_slot) == 0
                assert item_pointer(values(banked, guid), 39) == food_guid
                client.send(0x282, bytes([255, 39]))
                withdrawn = client.collect()
                assert item_pointer(values(withdrawn, guid), 39) == 0
                assert sql(f'SELECT slot FROM game_item_instance WHERE guid = {food_guid}')[0]['rows'][0][0] < 39
                client.send(0x1b9, struct.pack('<Q', vendor))
                bank_slot = client.collect()
                assert (0x1ba, struct.pack('<I', 3)) in bank_slot
                assert sql(f'SELECT bank_bag_slots, money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[1, 48980]]
                client.send(0x17c, struct.pack('<QI', vendor, 0))
                assert (0x17e, b'') in client.collect()
                for row_id, index, text, action in [(5090901, 0, 'Shop', 3), (5090902, 1, 'Bank', 9)]:
                    sql(f"INSERT INTO game_gossip_option (row_id,entry,option_index,icon,text,action,action_menu_id,cond_type,cond_value1,cond_value2) VALUES ({row_id},51004,{index},0,'{text}',{action},0,0,0,0)")
                client.send(0x17b, struct.pack('<Q', vendor))
                menu = next(body for opcode, body in client.collect() if opcode == 0x17d)
                assert struct.unpack_from('<I', menu, 12)[0] == 2 and b'Shop\0' in menu and b'Bank\0' in menu
                sql('UPDATE game_gossip_option SET cond_type = 3 WHERE row_id = 5090901')
                client.send(0x17c, struct.pack('<QI', vendor, 1))
                assert (0x1b8, struct.pack('<Q', vendor)) in client.collect()
                client.send(0x17c, struct.pack('<QI', vendor, 0))
                refused = client.collect()
                assert (0x17e, b'') in refused and not any(opcode == 0x19f for opcode, _ in refused)
                client.send(0x17b, struct.pack('<Q', vendor))
                menu = next(body for opcode, body in client.collect() if opcode == 0x17d)
                assert struct.unpack_from('<I', menu, 12)[0] == 1 and b'Shop\0' not in menu
                sql('INSERT INTO game_gossip_menu_profile (menu_id,text_id) VALUES (5090901,1)')
                sql(f'INSERT INTO game_creature_gossip_menu_override (creature_guid,menu_id,map_id,instance_id) VALUES ({vendor},5090901,0,0)')
                client.send(0x17c, struct.pack('<QI', vendor, 0))
                assert (0x17e, b'') in client.collect()
                sql(f'DELETE FROM game_creature_gossip_menu_override WHERE creature_guid = {vendor}')
                sql('DELETE FROM game_gossip_option WHERE row_id = 5090901')
                sql('DELETE FROM game_gossip_option WHERE row_id = 5090902')
                sql(f'UPDATE game_world_entity SET npc_flags = 129 WHERE guid = {vendor}')
                client.send(0x17b, struct.pack('<Q', vendor))
                menu = next(body for opcode, body in client.collect() if opcode == 0x17d)
                assert b'Make this inn your home.\0' in menu
                client.send(0x17c, struct.pack('<QI', vendor, 0))
                assert (0x2eb, struct.pack('<Q', vendor)) in client.collect()
                client.send(0x1b5, struct.pack('<Q', vendor))
                bound = client.collect()
                assert any(opcode == 0x158 and body[:8] == struct.pack('<Q', vendor) for opcode, body in bound)
                assert sql(f'SELECT home_map, home_x, home_y, home_z FROM game_character WHERE guid = {guid}')[0]['rows'] == sql(f'SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {guid}')[0]['rows']
                sql(f'UPDATE game_world_entity SET npc_flags = 261 WHERE guid = {vendor}')
                sql(f'UPDATE game_world_entity SET dead = true, health = 0 WHERE guid = {vendor}')
                client.send(0x1b9, struct.pack('<Q', vendor))
                assert (0x1ba, struct.pack('<I', 2)) in client.collect()
                assert sql(f'SELECT bank_bag_slots, money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[1, 48980]]
                call('debug_spawn_at_feet', [guid, 51001, 2])
                trainer = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51001')[0]['rows'])
                client.collect()
                peer.collect()
                client.send(0x1b0, struct.pack('<Q', trainer))
                assert any(opcode == 0x1b1 and body[:8] == struct.pack('<Q', trainer) for opcode, body in client.collect())
                client.send(0x1b2, struct.pack('<QI', trainer, 2050))
                trained = client.collect()
                assert (0x1b3, struct.pack('<QI', trainer, 2050)) in trained
                assert (0x12b, struct.pack('<I', 2050)) in trained
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[48880]]
                assert sql(f'SELECT spell_id FROM game_player_spell WHERE character_guid = {guid} AND spell_id = 2050')[0]['rows'] == [[2050]]
                client.send(0x1b2, struct.pack('<QI', trainer, 2050))
                refused = client.collect()
                assert any(opcode == 0x1b4 for opcode, _ in refused)
                assert not any(opcode in (0x12b, 0xa9) for opcode, _ in refused)
                call('debug_set_skill', [guid, 356, 3])
                skills = values(client.collect(), guid)
                assert any(value == 356 and skills.get(index + 1) == (5 << 16 | 3) for index, value in skills.items())
                call('debug_set_skill', [guid, 356, 4])
                skills = values(client.collect(), guid)
                assert any(value == 356 and skills.get(index + 1) == (5 << 16 | 4) for index, value in skills.items())
                assert not any(opcode in (0x19f, 0x1b1, 0x1b3, 0x12b, 0x1ba, 0x166) for opcode, _ in peer.collect())
                sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES (5090171,'Fixture craft',0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0)")
                for index in (0, 1):
                    sql(f'INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES ({20360684 + index},5090171,{index},7,1,0,0.0,0,0,0.0,0,0,0,5090050,8,0,0,false)')
                sql('INSERT INTO game_spell_reagent (id,spell_id,item_entry,count) VALUES (40721368,5090171,5090052,1)')
                call('debug_learn_spell', [guid, 5090171])
                client.collect()
                items_before = sql(f'SELECT * FROM game_item_instance WHERE owner_guid = {guid}')[0]['rows']
                allocator = sql('SELECT high_water FROM game_guid_allocator WHERE id = 0')[0]['rows'][0][0]
                sql(f'UPDATE game_guid_range SET size = {allocator + 2} WHERE id = 0')
                client.send(0x12e, cast(5090171))
                craft_failed = client.collect()
                assert not creates(craft_failed) and not destroys(craft_failed)
                assert not any(opcode == 0x166 for opcode, _ in craft_failed)
                assert not values(craft_failed, food_guid)
                assert sql(f'SELECT * FROM game_item_instance WHERE owner_guid = {guid}')[0]['rows'] == items_before
                assert sql('SELECT high_water FROM game_guid_allocator WHERE id = 0')[0]['rows'] == [[allocator]]
                sql(f'UPDATE game_guid_range SET size = {old_size} WHERE id = 0')
                client.send(0x12e, cast(5090171))
                crafted = client.collect()
                assert len(creates(crafted)) == 2
                assert values(crafted, food_guid)[14] == 5
                assert sum(opcode == 0x166 for opcode, _ in crafted) == 2
                assert len(sql(f'SELECT guid FROM game_item_instance WHERE owner_guid = {guid} AND entry = 5090050')[0]['rows']) == 2
                sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES (5090172,'Triggered product',0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,false,0,0,0,0,0,0,0)")
                sql('INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES (20360688,5090172,0,7,1,0,0.0,0,0,0.0,0,0,0,5090050,8,0,0,false)')
                sql('UPDATE game_spell_effect SET kind = 5, trigger_spell = 5090172 WHERE id = 20360684')
                client.send(0x12e, cast(5090171))
                nested_craft = client.collect()
                assert len(creates(nested_craft)) == 1
                assert not values(nested_craft, food_guid)
                items = sql(f'SELECT slot FROM game_item_instance WHERE owner_guid = {guid}')[0]['rows']
                assert len({row[0] for row in items}) == len(items)
                assert sql(f'SELECT stack_count FROM game_item_instance WHERE guid = {food_guid}')[0]['rows'] == [[5]]
                call('debug_spawn_at_feet', [guid, 51003, 2])
                giver = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51003')[0]['rows'])
                client.collect()
                peer.collect()
                client.send(0x182, struct.pack('<Q', giver))
                assert any(opcode == 0x183 and body[:8] == struct.pack('<Q', giver) for opcode, body in client.collect())
                client.send(0x184, struct.pack('<Q', giver))
                assert any(opcode == 0x188 and b'Wolf Cull\0' in body for opcode, body in client.collect())
                client.send(0x189, struct.pack('<QI', giver, 50900))
                accepted = client.collect()
                assert values(accepted, guid)[198] == 50900
                assert sql(f'SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50900')[0]['rows'] == [[False]]
                client.send(0x189, struct.pack('<QI', giver, 50900))
                refused = client.collect()
                assert [opcode for opcode, _ in refused][-2:] == [0x18f, 0x17e]
                assert len(sql(f'SELECT id FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50900')[0]['rows']) == 1
                sql('DELETE FROM game_creature_loot WHERE creature_entry = 51000')
                sql('INSERT INTO game_creature_loot (id,creature_entry,item_entry,chance_bp,count,group_id,quest_only) VALUES (5090901,51000,5090052,10000,2,0,false)')
                sql('UPDATE game_creature_template SET money_min = 17, money_max = 17 WHERE entry = 51000')
                sql(f'UPDATE game_world_entity SET level = 1, xp = 390, next_level_xp = 400 WHERE guid = {guid}')
                sql(f'UPDATE game_character SET rested_xp = 10 WHERE guid = {guid}')
                sql('UPDATE game_creature_template SET level = 1, max_level = 1 WHERE entry = 51000')
                for kills in (1, 2):
                    call('debug_spawn_at_feet', [guid, 51000, 3])
                    target = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51000')[0]['rows'])
                    client.collect()
                    call('debug_apply_damage', [target, 1, guid])
                    call('debug_kill_creature', [guid, target])
                    progress = client.collect()
                    assert (0x1d0, struct.pack('<QIBIf', target, 60 if kills == 1 else 50, 0, 50, 1.0)) in progress
                    if kills == 1:
                        assert any(opcode == 0x1d4 and body[:4] == struct.pack('<I', 2) for opcode, body in progress)
                        assert sql(f'SELECT level, xp FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[2, 50]]
                    assert any(opcode == 0x199 and body[:12] == struct.pack('<III', 50900, 51000, kills) for opcode, body in progress)
                    assert values(progress, guid)[199] & 0x3f == kills
                assert values(progress, guid)[199] >> 24 == 1
                assert values(progress, target)[143] & 1
                observed = peer.collect()
                assert not any(opcode in (0x1d0, 0x1d4) for opcode, _ in observed)
                assert values(observed, target)[143] & 1 == 0
                peer.send(0x15d, struct.pack('<Q', target))
                refused = peer.collect()
                assert any(opcode == 0x160 and body[:10] == struct.pack('<QBB', target, 0, 0) for opcode, body in refused)
                client.send(0x15d, struct.pack('<Q', target))
                loot = next(body for opcode, body in client.collect() if opcode == 0x160)
                assert loot[:14] == struct.pack('<QBIB', target, 1, 17, 1)
                assert struct.unpack_from('<BII', loot, 14) == (0, 5090052, 2)
                purse = sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'][0][0]
                client.send(0x15e)
                assert (0x165, b'') in client.collect()
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[purse + 17]]
                client.send(0x108, bytes([0]))
                looted = client.collect()
                assert (0x162, bytes([0])) in looted
                assert values(looted, food_guid)[14] == 7
                assert values(looted, target)[143] & 1 == 0
                assert not sql(f'SELECT id FROM game_corpse_loot WHERE corpse_guid = {target}')[0]['rows']
                client.send(0x15f, struct.pack('<Q', target))
                assert (0x161, struct.pack('<QB', target, 1)) in client.collect()
                client.send(0x15e)
                assert not any(opcode == 0x165 for opcode, _ in client.collect())
                assert not any(opcode in (0x165, 0x162, 0x166) for opcode, _ in peer.collect())
                client.send(0x18a, struct.pack('<QI', giver, 50900))
                assert any(opcode == 0x18d for opcode, _ in client.collect())
                money = sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'][0][0]
                client.send(0x18e, struct.pack('<QII', giver, 50900, 0))
                reward = client.collect()
                assert any(opcode == 0x191 and body[:4] == struct.pack('<I', 50900) for opcode, body in reward)
                assert (0x1d0, struct.pack('<QIB', 0, 90, 1)) in reward
                assert values(reward, guid)[198] == 0
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[money + 150]]
                assert sql(f'SELECT rewarded FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50900')[0]['rows'] == [[True]]
                assert not any(opcode in (0x199, 0x191, 0x197) for opcode, _ in peer.collect())
                sql('UPDATE game_quest_template SET limit_time = 1 WHERE entry = 50900')
                client.send(0x189, struct.pack('<QI', giver, 50900))
                assert values(client.collect(), guid)[198] == 50900
                expired = []
                for _ in range(80):
                    expired += client.collect()
                    if any(opcode == 0x197 for opcode, _ in expired):
                        break
                    time.sleep(0.05)
                assert (0x197, struct.pack('<I', 50900)) in expired
                client.send(0x194, bytes([0]))
                assert values(client.collect(), guid)[198] == 0
                assert not sql(f'SELECT id FROM game_character_quest WHERE character_guid = {guid} AND quest_entry = 50900')[0]['rows']
                peer.collect()
                call('debug_stage_meeting_stone_group', [guid, [peer_guid], False])
                sql('UPDATE game_creature_loot SET item_entry = 5090050, count = 1 WHERE id = 5090901')
                call('debug_spawn_at_feet', [guid, 51000, 3])
                corpse = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51000')[0]['rows'])
                client.collect()
                peer.collect()
                call('debug_apply_damage', [corpse, 1, guid])
                call('debug_kill_creature', [guid, corpse])
                rolls = client.collect()
                assert any(opcode == 0x2a1 and body[:16] == struct.pack('<QII', corpse, 0, 5090050) for opcode, body in rolls)
                assert any(opcode == 0x2a1 for opcode, _ in peer.collect())
                assert sql(f'SELECT withheld FROM game_corpse_loot WHERE corpse_guid = {corpse}')[0]['rows'] == [[True]]
                client.send(0x2a0, struct.pack('<QIB', corpse, 0, 1))
                assert any(opcode == 0x2a2 for opcode, _ in client.collect())
                peer.send(0x2a0, struct.pack('<QIB', corpse, 0, 0))
                awarded = client.collect()
                for _ in range(50):
                    if any(opcode == 0x29f for opcode, _ in awarded):
                        break
                    time.sleep(0.02)
                    awarded += client.collect()
                assert any(opcode == 0x29f for opcode, _ in awarded)
                assert any(opcode == 0x166 for opcode, _ in awarded)
                assert any(opcode == 0x29f for opcode, _ in peer.collect())
                assert not sql(f'SELECT id FROM game_loot_roll WHERE corpse_guid = {corpse}')[0]['rows']
                assert not sql(f'SELECT id FROM game_corpse_loot WHERE corpse_guid = {corpse}')[0]['rows']
                client.send(0x15d, struct.pack('<Q', corpse))
                client.collect()
                peer.send(0x15d, struct.pack('<Q', corpse))
                peer.collect()
                purse = sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'][0][0]
                peer_purse = sql(f'SELECT money FROM game_world_entity WHERE guid = {peer_guid}')[0]['rows'][0][0]
                client.send(0x15e)
                assert (0x163, struct.pack('<I', 9)) in client.collect()
                shared = peer.collect()
                assert (0x163, struct.pack('<I', 8)) in shared
                assert (0x165, b'') in shared
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[purse + 9]]
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {peer_guid}')[0]['rows'] == [[peer_purse + 8]]
                client.send(0xee, struct.pack('<IIffffI', 0, 1000, 2, 0, 0, 0, 0))
                for _ in range(100):
                    if sql(f'SELECT guid FROM game_entity_motion WHERE guid = {guid}')[0]['rows']:
                        break
                    time.sleep(0.02)
                motion = peer.collect()
                assert any(opcode == 0xee and packed_guid(body) == guid for opcode, body in motion)
                assert not any(opcode == 0xee for opcode, _ in client.collect())
                call('debug_teleport', [guid, 0, 150, 0, 0, 0])
                assert destroys(peer.collect()).count(guid) == 1
                assert destroys(client.collect()).count(peer_guid) == 1
                call('debug_teleport', [guid, 0, 0, 0, 0, 0])
                assert creates(peer.collect()).count(guid) == 1
                assert creates(client.collect()).count(peer_guid) == 1
                call('debug_force_cast', [guid, 1784])
                assert destroys(peer.collect()).count(guid) == 1
                call('debug_apply_damage', [guid, 1, 0])
                assert creates(peer.collect()).count(guid) == 1
                call('debug_learn_spell', [peer_guid, 168])
                sql('UPDATE game_spell SET duration_ms = 6000, gcd_ms = 0 WHERE spell_id = 168')
                peer.send(0x12e, cast(168))
                buff = peer.collect()
                observed = client.collect()
                assert [opcode for opcode, _ in buff][:3] == [0x131, 0x130, 0x132]
                assert sum(opcode == 0x132 and spell_id(body) == 168 for opcode, body in buff) == 1
                assert sum(opcode == 0x132 and spell_id(body) == 168 for opcode, body in observed) == 1
                assert any(opcode == 0x137 and 5900 <= struct.unpack_from('<I', body, 1)[0] <= 6000 for opcode, body in buff)
                assert not any(opcode == 0x137 for opcode, _ in observed)
                assert sql(f'SELECT id FROM game_aura WHERE target_guid = {peer_guid} AND spell_id = 168')[0]['rows']
                peer.send(0x136, struct.pack('<I', 168))
                cancelled = peer.collect()
                assert any(opcode == 0xa9 for opcode, _ in cancelled)
                assert not sql(f'SELECT id FROM game_aura WHERE target_guid = {peer_guid} AND spell_id = 168')[0]['rows']
                client.collect()
                call('debug_spawn_at_feet', [guid, 51000, 40])
                wolf = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51000')[0]['rows'])
                assert wolf in creates(client.collect())
                assert wolf in creates(peer.collect())
                client.send(0x141, struct.pack('<Q', guid))
                assert not any(opcode == 0x143 for opcode, _ in client.collect())
                assert not sql(f'SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}')[0]['rows']
                client.send(0x141, struct.pack('<Q', wolf))
                attack = client.collect()
                observed = peer.collect()
                assert (0x143, struct.pack('<QQ', guid, wolf)) in attack
                assert (0x143, struct.pack('<QQ', guid, wolf)) in observed
                for _ in range(60):
                    if any(opcode == 0x145 for opcode, _ in attack):
                        break
                    time.sleep(0.05)
                    attack += client.collect()
                assert any(opcode == 0x145 for opcode, _ in attack)
                assert not any(opcode == 0x145 for opcode, _ in observed + peer.collect())
                client.send(0x141, struct.pack('<Q', wolf))
                assert (0x143, struct.pack('<QQ', guid, wolf)) in client.collect()
                assert not any(opcode == 0x143 for opcode, _ in peer.collect())
                client.send(0x142)
                assert any(opcode == 0x144 for opcode, _ in client.collect())
                assert any(opcode == 0x144 for opcode, _ in peer.collect())
                assert not sql(f'SELECT * FROM game_melee_attack WHERE attacker_guid = {guid}')[0]['rows']
                call('debug_teleport', [guid, 0, 38, 0, 0, 0])
                client.collect()
                peer.collect()
                client.send(0x141, struct.pack('<Q', wolf))
                attack = []
                for _ in range(60):
                    attack += client.collect()
                    if any(opcode == 0x14a and packed_guid(body[4:]) == guid for opcode, body in attack):
                        break
                    time.sleep(0.05)
                assert any(opcode == 0x14a and packed_guid(body[4:]) == guid for opcode, body in attack)
                assert any(opcode == 0x14a and packed_guid(body[4:]) == guid for opcode, body in peer.collect())
                call('debug_set_health', [wolf, 0])
                client.collect()
                peer.collect()
                client.send(0x141, struct.pack('<Q', wolf))
                assert (0x148, b'') in client.collect()
                call('debug_teleport', [guid, 0, 0, 0, 0, 0])
                client.collect()
                peer.collect()
                call('debug_learn_spell', [peer_guid, 133])
                sql('UPDATE game_spell SET cast_time_ms = 1500, gcd_ms = 0, cooldown_ms = 0, cost = 0 WHERE spell_id = 133')
                call('debug_spawn_at_feet', [guid, 51000, 35])
                spell_target = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51000')[0]['rows'])
                peer.collect()
                client.collect()
                peer.send(0x12e, cast(133, spell_target))
                started = peer.collect()
                assert any(opcode == 0x131 and spell_id(body) == 133 for opcode, body in started), started
                assert not any(opcode == 0x132 for opcode, _ in started)
                assert any(opcode == 0x131 and spell_id(body) == 133 for opcode, body in client.collect())
                peer.send(0x12f, struct.pack('<I', 133))
                assert any(opcode == 0x133 for opcode, _ in peer.collect())
                observed = client.collect()
                assert any(opcode == 0x2a6 for opcode, _ in observed)
                assert not any(opcode == 0x133 for opcode, _ in observed)
                time.sleep(1.6)
                assert not any(opcode == 0x132 for opcode, _ in peer.collect())
                assert not sql(f'SELECT * FROM game_pending_cast WHERE caster_guid = {peer_guid}')[0]['rows']
                client.collect()
                for _ in range(10):
                    peer.send(0x12e, cast(133, spell_target))
                    completed = []
                    for _ in range(100):
                        completed += peer.collect()
                        missed = [miss for opcode, body in completed
                                  if opcode == 0x132 and spell_id(body) == 133
                                  for miss in spell_go_misses(body)]
                        if missed or any(opcode == 0x250 for opcode, _ in completed):
                            break
                        time.sleep(0.05)
                    if not missed:
                        break
                    assert missed == [(spell_target, 2)], missed
                    client.collect()
                order = [opcode for opcode, body in completed if opcode in (0x130, 0x131, 0x132, 0x250)]
                assert order == [0x131, 0x130, 0x132, 0x250], order
                observed = client.collect()
                assert sum(opcode == 0x132 and spell_id(body) == 133 for opcode, body in observed) == 1
                assert any(opcode == 0x250 for opcode, _ in observed)
                assert not any(opcode == 0x130 for opcode, _ in observed)
                call('debug_set_health', [spell_target, 0])
                peer.collect()
                client.collect()
                sql("INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES (5090901,0,0,1000,0,1,0,'Native exploration')")
                call('import_terrain_chunks_append', ['0,512,512,0,false,0,5090901,' + ':'.join(['0'] * 145)])
                call('debug_explore_at', [guid, 0, -1, -1])
                explored = client.collect()
                assert (0x1f8, struct.pack('<II', 5090901, 50)) in explored
                assert (0x1d0, struct.pack('<QIB', 0, 50, 1)) in explored
                assert values(explored, guid)[1142] == 256
                call('debug_explore_at', [guid, 0, -1, -1])
                assert not any(opcode in (0x1f8, 0x1d0) for opcode, _ in client.collect())
                assert not any(opcode in (0x1f8, 0x1d0) for opcode, _ in peer.collect())
                call('debug_check_rest_at', [guid, 0, -9464, 42])
                rested = values(client.collect(), guid)[194]
                assert rested >> 24 == 1 and (rested >> 16) & 255 == 1
                call('debug_check_rest_at', [guid, 0, 0, 0])
                unrested = values(client.collect(), guid)[194]
                assert unrested >> 24 == 2 and unrested & 0xffffff == rested & 0xffffff
                peer.collect()
                sql('INSERT INTO game_faction (faction_id,reputation_index,base_standing) VALUES (5090901,62,0)')
                call('debug_grant_reputation', [guid, 5090901, 3175])
                assert (0x124, struct.pack('<IIi', 1, 62, 3175)) in client.collect()
                call('debug_grant_reputation', [guid, 5090901, 25])
                assert (0x124, struct.pack('<IIi', 1, 62, 3200)) in client.collect()
                client.send(0x125, struct.pack('<IB', 62, 1))
                client.collect()
                assert sql(f'SELECT at_war FROM game_player_reputation WHERE character_guid = {guid} AND faction_id = 5090901')[0]['rows'] == [[True]]
                client.send(0x318, struct.pack('<i', 62))
                assert values(client.collect(), guid)[1261] == 62
                client.send(0x318, struct.pack('<i', 64))
                refused = client.collect()
                assert any(opcode == 0x96 for opcode, _ in refused)
                assert not values(refused, guid)
                assert sql(f'SELECT watched_faction_index FROM game_character WHERE guid = {guid}')[0]['rows'] == [[62]]
                client.send(0x125, struct.pack('<IB', 0xffffffff, 1))
                assert any(opcode == 0x96 for opcode, _ in client.collect())
                assert not any(opcode in (0x124, 0x96) for opcode, _ in peer.collect())
                call('debug_set_level', [guid, 10])
                sql("INSERT INTO game_talent_tab (tab_id,name,class_mask,race_mask,order_index) VALUES (5090901,'Native talent',1,0,0)")
                sql("INSERT INTO game_talent (talent_id,name,tree_id,tier,column,max_rank,spell_id,required_talent_id,required_points_in_tree,grant_spell_id,tab_id,rank_spell_2,rank_spell_3,rank_spell_4,rank_spell_5,required_talent_rank,required_spell_id) VALUES (124,'Native talent',0,0,0,1,51990,0,0,0,5090901,0,0,0,0,0,0)")
                sql("INSERT INTO game_spell (spell_id,name,power_type,cost,cast_time_ms,gcd_ms,cooldown_ms,range_yd,duration_ms,school_mask,dispel_type,mechanic,max_stacks,aura_interrupt,attributes,spell_level,max_level,is_negative,cast_flags,stances,family_name,family_flags,proc_flags,proc_chance,proc_charges) VALUES (51990,'Native passive',0,0,0,0,0,0,4294967295,1,0,0,1,0,64,0,0,false,0,0,0,0,0,0,0)")
                for index, kind, points, param, mask in [(0, 160, 5, 0, 0), (1, 172, -5, 10, 1)]:
                    sql(f'INSERT INTO game_spell_effect (id,spell_id,effect_index,kind,base_points,die_sides,per_level,period_ms,target,radius_yd,chain_targets,trigger_spell,effect_mechanic,p0,p0_kind,p1,script_id,enters_combat) VALUES ({207960 + index},51990,{index},{kind},{points},0,0.0,0,0,0.0,0,0,0,{param},0,{mask},0,false)')
                client.collect()
                peer.collect()
                client.send(0x251, struct.pack('<II', 124, 0))
                learned = client.collect()
                assert (0x12b, struct.pack('<I', 51990)) in learned
                assert (0x266, struct.pack('<BBi', 0, 10, -5)) in learned
                assert values(learned, guid)[1102] == 0
                assert sql(f'SELECT sheet_str_bonus FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[5]]
                client.send(0x251, struct.pack('<II', 124, 1))
                assert not any(opcode in (0x12b, 0x266) for opcode, _ in client.collect())
                assert sql(f'SELECT rank FROM game_character_talent WHERE character_guid = {guid}')[0]['rows'] == [[1]]
                before_reset = sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'][0][0]
                client.send(0x2aa, struct.pack('<Q', trainer))
                reset = client.collect()
                assert (0x17e, b'') in reset
                assert (0x203, struct.pack('<H', 51990)) in reset
                assert (0x266, struct.pack('<BBi', 0, 10, 0)) in reset
                assert values(reset, guid)[1102] == 1
                assert sql(f'SELECT sheet_str_bonus, money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[0, before_reset - 10000]]
                assert not sql(f'SELECT rank FROM game_character_talent WHERE character_guid = {guid}')[0]['rows']
                client.send(0x251, struct.pack('<II', 124, 0))
                assert values(client.collect(), guid)[1102] == 0
                assert not any(opcode in (0x12b, 0x203, 0x266) for opcode, _ in peer.collect())
                call('debug_spawn_at_feet', [guid, 51004, 2])
                flight_master = max(row[0] for row in sql('SELECT guid FROM game_world_entity WHERE entry = 51004')[0]['rows'])
                sql(f'UPDATE game_world_entity SET npc_flags = 9 WHERE guid = {flight_master}')
                sql(f'UPDATE game_world_entity SET movement_flags = 0, unit_flags = 0 WHERE guid = {guid}')
                for node, client_node, x in [(5090301, 253, 0), (5090302, 254, 64)]:
                    sql(f"INSERT INTO game_taxi_node (id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance) VALUES ({node},{client_node},0,{x},0,0,'Native flight',295,1147)")
                sql('INSERT INTO game_taxi_path (id,source_node_id,destination_node_id,fare) VALUES (5090303,5090301,5090302,25)')
                for index, x in [(0, 0), (1, 64)]:
                    sql(f'INSERT INTO game_taxi_path_node (id,path_id,node_index,map_id,x,y,z,flags,delay_ms) VALUES ({5090304 + index},5090303,{index},0,{x},0,0,0,0)')
                client.collect()
                peer.collect()
                client.send(0x1aa, struct.pack('<Q', flight_master))
                assert (0x1ab, struct.pack('<QB', flight_master, 0)) in client.collect()
                client.send(0x1ac, struct.pack('<Q', flight_master))
                taxi_menu = next(body for opcode, body in client.collect() if opcode == 0x1a9)
                assert taxi_menu[:16] == struct.pack('<IQI', 1, flight_master, 253)
                client.send(0x1aa, struct.pack('<Q', flight_master))
                assert (0x1ab, struct.pack('<QB', flight_master, 1)) in client.collect()
                money_before = sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'][0][0]
                client.send(0x1ad, struct.pack('<QII', flight_master, 253, 254))
                assert (0x1ae, struct.pack('<I', 6)) in client.collect()
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[money_before]]
                sql(f'INSERT INTO game_character_taxi_node (id,character_guid,node_id) VALUES (0,{guid},5090302)')
                client.send(0x1ad, struct.pack('<QII', flight_master, 253, 254))
                takeoff = client.collect()
                assert (0x1ae, bytes(4)) in takeoff
                assert next(index for index, (opcode, _) in enumerate(takeoff) if opcode == 0x1ae) < next(index for index, (opcode, _) in enumerate(takeoff) if opcode == 0xdd)
                assert values(takeoff, guid)[133] == 1147
                assert any(opcode == 0xdd and packed_guid(body) == guid for opcode, body in peer.collect())
                assert sql(f'SELECT money FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[money_before - 25]]
                flying_message = b'Whisper while flying\0'
                client.send(0x95, struct.pack('<II', 6, 7) + b'Peer\0' + flying_message)
                flight_chat = client.collect()
                assert (0x96, struct.pack('<BIQI', 7, 0, peer_guid, len(flying_message)) + flying_message + b'\0') in flight_chat
                assert (0x96, struct.pack('<BIQI', 6, 0, guid, len(flying_message)) + flying_message + b'\0') in peer.collect()
                client.send(0x95, struct.pack('<II', 0, 7) + b'Speech while flying\0')
                flight_speech = client.collect()
                assert not any(opcode == 0x96 for opcode, _ in flight_speech)
                assert not any(opcode == 0x96 for opcode, _ in peer.collect())
                client.send(0x1ad, struct.pack('<QII', flight_master, 253, 254))
                busy = client.collect()
                assert (0x1ae, struct.pack('<I', 7)) in busy
                client.send(0xee, struct.pack('<IIffffI', 0, 9999, 10000, 0, 0, 0, 0))
                suppressed = client.collect()
                assert sql(f'SELECT x FROM game_world_entity WHERE guid = {guid}')[0]['rows'][0][0] < 100
                landed = takeoff + flight_chat + flight_speech + busy + suppressed
                for _ in range(80):
                    landed += client.collect()
                    if not sql(f'SELECT character_guid FROM game_active_taxi_flight WHERE character_guid = {guid}')[0]['rows']:
                        break
                    time.sleep(0.05)
                landed += client.collect()
                assert sql(f'SELECT x, mount_display_id FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[64.0, 0]]
                assert values(landed, guid)[133] == 0
                assert not sql(f'SELECT character_guid FROM game_taxi_passenger_spline WHERE character_guid = {guid}')[0]['rows']
                assert not sql(f'SELECT request_id FROM game_taxi_service_reply WHERE character_guid = {guid}')[0]['rows']
                peer.collect()
                client.collect()
                call('debug_teleport', [guid, 0, 12, 0, 0, 0])
                first_teleport = next(body for opcode, body in client.collect(auto_ack=False) if opcode == 0xc7)
                call('debug_teleport', [guid, 0, 16, 0, 0, 0])
                second_teleport = next(body for opcode, body in client.collect(auto_ack=False) if opcode == 0xc7)
                assert teleport_ack(first_teleport) != teleport_ack(second_teleport)
                client.send(0xc7, teleport_ack(first_teleport))
                client.send(0xdc)
                client.send(0xee, struct.pack('<IIffffI', 0, 10001, 999, 0, 0, 0, 0))
                client.send(0x141, struct.pack('<Q', spell_target))
                client.collect(auto_ack=False)
                assert sql(f'SELECT x FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[16.0]]
                assert not sql(f'SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {guid}')[0]['rows']
                wrong_mover = bytearray(teleport_ack(second_teleport))
                wrong_mover[1] ^= 1
                client.send(0xc7, bytes(wrong_mover))
                client.send(0xee, struct.pack('<IIffffI', 0, 10002, 999, 0, 0, 0, 0))
                client.collect(auto_ack=False)
                assert sql(f'SELECT x FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[16.0]]
                peer.collect()
                client.send(0xc7, teleport_ack(second_teleport))
                client.collect()
                arrived = peer.collect()
                assert any(opcode == 0xc5 and packed_guid(body) == guid
                           and struct.unpack_from('<f', body, 9 + body[0].bit_count())[0] == 16
                           for opcode, body in arrived)
                client.send(0xee, struct.pack('<IIffffI', 0, 10003, 17, 0, 0, 0, 0))
                client.collect()
                assert sql(f'SELECT x FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[17.0]]
                peer.collect()
                call('debug_teleport', [guid, 1, 4, 5, 6, 0.75])
                departing = client.collect(auto_ack=False)
                assert [opcode for opcode, _ in departing] == [0x3f, 0x3e]
                assert departing[1][1] == struct.pack('<Iffff', 1, 4, 5, 6, 0.75)
                assert destroys(peer.collect()).count(guid) == 1
                assert not sql(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')[0]['rows']
                client.send(0xee, struct.pack('<IIffffI', 0, 10004, 999, 0, 0, 0, 0))
                client.send(0xc7, teleport_ack(second_teleport))
                client.collect(auto_ack=False)
                assert not sql(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')[0]['rows']
                client.send(0xdc)
                ported = client.collect()
                assert creates(ported).count(guid) == 1
                assert peer_guid not in creates(ported)
                assert not any(opcode == 0x236 for opcode, _ in ported)
                assert values(ported, guid)[1142] == 256
                assert sql(f'SELECT map_id, x, y, z, orientation FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[1, 4.0, 5.0, 6.0, 0.75]]
                client.send(0xdc)
                assert guid not in creates(client.collect())
                call('debug_teleport', [guid, 0, 18, 0, 0, 0])
                returned = client.collect()
                assert creates(returned).index(guid) < creates(returned).index(peer_guid)
                assert creates(peer.collect()).count(guid) == 1
                call('debug_teleport', [guid, 1, 20, 0, 0, 0])
                client.collect(auto_ack=False)
                client.close()
                for _ in range(100):
                    if sql(f'SELECT online FROM game_character WHERE guid = {guid}')[0]['rows'] == [[False]]:
                        break
                    time.sleep(0.02)
                assert sql(f'SELECT online, map_id, x FROM game_character WHERE guid = {guid}')[0]['rows'] == [[False, 1, 20.0]]
                client = World(world_port, key=native_key)
                assert client.read()[0] == 0x1ee
                client.send(0x3d, struct.pack('<Q', guid))
                assert creates(client.collect()).count(guid) == 1
                assert sql(f'SELECT map_id, x FROM game_world_entity WHERE guid = {guid}')[0]['rows'] == [[1, 20.0]]
                call('debug_teleport', [guid, 0, 20, 0, 0, 0])
                client.collect()
                peer.collect()
                call('debug_teleport', [guid, 1, 24, 0, 0, 0])
                client.collect(auto_ack=False)
                client.send(0x4b)
                loading_logout = client.collect(auto_ack=False)
                assert [opcode for opcode, _ in loading_logout] == [0x4c, 0x4d]
                assert sql(f'SELECT online, map_id, x FROM game_character WHERE guid = {guid}')[0]['rows'] == [[False, 1, 24.0]]
                client.send(0x3d, struct.pack('<Q', guid))
                client.collect()
                call('debug_teleport', [guid, 0, 20, 0, 0, 0])
                client.collect()
                peer.collect()
                check_death(client, peer, call, sql, guid, peer_guid)
                check_environment(client, peer, call, sql, guid, peer_guid)
                check_social(client, peer, call, sql, guid, peer_guid)
                check_channels(client, peer, call, sql, guid, peer_guid)
                check_party(client, peer, call, sql, guid, peer_guid)
                check_member_stats(client, peer, call, sql, guid, peer_guid)
                check_guild(client, peer, call, sql, guid, peer_guid)
                check_petitions(client, peer, call, sql, guid, peer_guid)
                check_trade(client, peer, call, sql)
                check_duels(client, peer, call, sql)
                check_meeting_stones(client, peer, call, sql)
                check_mail(client, peer, call, sql)
                check_auctions(client, peer, call, sql)
                check_pets(client, peer, call, sql)
                check_areatriggers(client, peer, call, sql)
                check_world_objects(client, peer, call, sql)
                check_addon(client, peer, call, sql)
                check_chat(client, peer, call, sql, guid, peer_guid)
                replacement = World(world_port, key=native_key)
                assert replacement.read()[0] == 0x1ee
                replacement.send(0x3d, struct.pack('<Q', guid))
                resumed = replacement.collect()
                assert creates(resumed).count(guid) == 1
                assert values(resumed, guid)[1142] == 256
                assert values(resumed, guid)[1102] == 0
                assert (0x266, struct.pack('<BBi', 0, 10, -5)) in resumed
                assert not any(opcode == 0x1f8 for opcode, _ in resumed)
                while True:
                    try:
                        client.read()
                    except EOFError:
                        break
                client.close()
                time.sleep(0.05)
                assert sql(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')[0]['rows']
                replacement.send(0x4b)
                logout = replacement.collect()
                assert [opcode for opcode, _ in logout][-2:] == [0x4c, 0x4d]
                assert not sql(f'SELECT guid FROM game_world_entity WHERE guid = {guid}')[0]['rows']
                assert guid in destroys(peer.collect())
                peer.send(0xee, struct.pack('<IIffffI', 0, 2000, 11, 0, 0, 0, 0))
                peer.collect()
                assert not any(opcode == 0xee for opcode, _ in replacement.collect())
                replacement.send(0x38, struct.pack('<Q', peer_guid))
                assert replacement.read() == (0x3c, bytes([0x3a]))
                replacement.send(0x38, struct.pack('<Q', guid))
                assert replacement.read() == (0x3c, bytes([0x39]))
                assert not sql(f'SELECT guid FROM game_character WHERE guid = {guid}')[0]['rows']
                replacement.close()
                peer.close()
                await_empty('game_native_member_stats')
                await_empty('game_native_player')
                await_empty('game_native_world_connection')
                await_empty('game_native_cipher')
                await_empty('game_native_logon_connection')
                limited = Logon(logon_port)
                for attempt in range(3):
                    assert limited.challenge('NATIVE') == 0
                    assert limited.prove('WRONG') == (5, None)
                assert limited.socket.recv(1) == b''
                limited.close()
                await_empty('game_native_logon_connection')
                connections = [Logon(logon_port) for _ in range(8)]
                for _ in range(100):
                    if len(sql('SELECT * FROM game_native_logon_connection')[0]['rows']) == 8:
                        break
                    time.sleep(0.02)
                assert len(sql('SELECT * FROM game_native_logon_connection')[0]['rows']) == 8
                excess = Logon(logon_port)
                assert excess.socket.recv(1) == b''
                excess.close()
                for connection in connections:
                    connection.close()
                await_empty('game_native_logon_connection')
                assert all(row[0] == 0 for row in sql('SELECT active FROM game_native_logon_address')[0]['rows'])
                check_session_revocation(world_port, call, sql, await_empty)
                import admission
                admission.run(world_port, call, sql, await_empty)
                print('PASS native logon and world protocol checks')
            except BaseException:
                print('\n'.join((run / 'host.log').read_text().splitlines()[-15:]))
                raise
            finally:
                host.terminate()
                try:
                    host.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    host.kill()
                    host.wait()


if __name__ == '__main__':
    main()
