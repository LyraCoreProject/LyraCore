import hashlib
import secrets
import socket
import struct

KEY = bytes([0x57] * 40)
PRIME = bytes.fromhex('b79b3e2a87823cab8f5ebfbf8eb10108535006298b5badbd5b53e1895e644b89')
# SMSG_AUTH_CHALLENGE header: size 6, opcode 0x1ec, sent before the cipher starts.
AUTH_CHALLENGE = bytes([0, 6, 0xec, 1])


def sha(*parts):
    return hashlib.sha1(b''.join(parts)).digest()


def verifier(username, password, salt):
    secret = int.from_bytes(sha(salt, sha(f'{username.upper()}:{password.upper()}'.encode())), 'little')
    return pow(7, secret, int.from_bytes(PRIME, 'little')).to_bytes(32, 'little')


def read_exact(sock, count):
    out = bytearray()
    while len(out) < count:
        data = sock.recv(count - len(out))
        if not data:
            raise EOFError('native socket closed')
        out.extend(data)
    return bytes(out)


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


def request_header(opcode, body_size):
    return struct.pack('>H', body_size + 4) + struct.pack('<I', opcode)


def reply_header(header):
    """Returns (body size, opcode) from a decrypted 4-byte server header."""
    size = struct.unpack('>H', header[:2])[0]
    if size < 2:
        raise ValueError('invalid world frame size')
    return size - 2, struct.unpack('<H', header[2:])[0]


def auth_session(username, key, server_seed, build=5875, valid_proof=True):
    client_seed = secrets.token_bytes(4)
    proof = sha(username.encode(), bytes(4), client_seed, server_seed, key) if valid_proof else bytes(20)
    body = struct.pack('<II', build, 0) + username.encode() + b'\0' + client_seed + proof + bytes(4)
    return request_header(0x1ed, len(body)) + body


class World:
    """One World Session. `guid` is the Character it entered, or None."""

    def __init__(self, port, username='NATIVE', build=5875, valid_proof=True, key=KEY):
        self.socket = socket.create_connection(('127.0.0.1', port), timeout=10)
        self.username = username
        self.guid = None
        self.entered = []
        self.encrypt = Cipher(key)
        self.decrypt = Cipher(key)
        header = read_exact(self.socket, 4)
        assert header == AUTH_CHALLENGE, header.hex()
        self.socket.sendall(auth_session(username, key, read_exact(self.socket, 4), build, valid_proof))

    def read(self, encrypted=True):
        header = read_exact(self.socket, 4)
        if encrypted:
            header = self.decrypt.decrypt(header)
        size, opcode = reply_header(header)
        return opcode, read_exact(self.socket, size)

    def send(self, opcode, body=b''):
        self.socket.sendall(self.encrypt.encrypt(request_header(opcode, len(body))) + body)

    def collect(self, auto_ack=True):
        """Returns every packet sent before a ping fence, optionally acknowledging teleports."""
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

    def read_until_closed(self, timeout=3):
        self.socket.settimeout(timeout)
        try:
            while True:
                self.read()
        except EOFError:
            pass

    def close(self):
        self.socket.close()


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


def values(packets, guid, create=False):
    fields = {}
    for opcode, body in packets:
        if opcode != 0xa9 or body[:5] != struct.pack('<IB', 1, 0) or body[5] not in ((3,) if create else (0,)) or packed_guid(body[6:]) != guid:
            continue
        offset = 7 + body[6].bit_count()
        if create and body[offset] in (1, 2):  # TYPEID_ITEM or TYPEID_CONTAINER.
            assert body[offset + 1] == 0x10  # ALL, followed by its u32.
            offset += 2 + 4
        elif create:
            assert body[offset] == 4  # TYPEID_PLAYER.
            assert body[offset + 1] in (0x30, 0x31)  # LIVING, ALL and optional SELF.
            flags = struct.unpack_from('<I', body, offset + 2)[0]
            assert flags & ~0x13f == 0, 'Character fixture must have a flat movement block'
            offset += 2 + 52 + 4
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
