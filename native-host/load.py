import asyncio
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import secrets
import struct
import time

from check import Cipher, KEY, ROOT, sha


def usage(pid):
    fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    cpu = (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK')
    status = Path(f'/proc/{pid}/status').read_text().splitlines()
    rss = next(int(line.split()[1]) * 1024 for line in status if line.startswith('VmRSS:'))
    return cpu, rss


def percentile(values, fraction):
    if not values:
        return None
    return sorted(values)[min(len(values) - 1, int(len(values) * fraction))]


class Client:
    def __init__(self, reader, writer, guid, x, metrics):
        self.reader, self.writer = reader, writer
        self.guid, self.x = guid, x
        self.encrypt, self.decrypt = Cipher(KEY), Cipher(KEY)
        self.pending = {}
        self.sequence = 0
        self.closed = None
        self.metrics = metrics
        self.received = 0
        self.final_positions = {}
        self.final_seen = set()
        self.task = None

    async def read(self):
        header = self.decrypt.decrypt(await self.reader.readexactly(4))
        size, opcode = struct.unpack('>H', header[:2])[0], struct.unpack('<H', header[2:])[0]
        if size < 2:
            raise ValueError('invalid world frame size')
        return opcode, await self.reader.readexactly(size - 2)

    def send(self, opcode, body=b''):
        header = struct.pack('>H', len(body) + 4) + struct.pack('<I', opcode)
        self.writer.write(self.encrypt.encrypt(header) + body)

    async def drain(self):
        buffered = bytearray()
        frame = None
        try:
            while True:
                chunk = await self.reader.read(65536)
                if not chunk:
                    raise ConnectionError('native socket closed')
                buffered.extend(chunk)
                offset = 0
                while True:
                    if frame is None:
                        if len(buffered) - offset < 4:
                            break
                        header = self.decrypt.decrypt(buffered[offset:offset + 4])
                        size = int.from_bytes(header[:2], 'big')
                        if size < 2:
                            raise ValueError('invalid world frame size')
                        frame = size - 2, int.from_bytes(header[2:], 'little')
                        offset += 4
                    size, opcode = frame
                    if len(buffered) - offset < size:
                        break
                    self.received_packet(opcode, buffered[offset:offset + size])
                    offset += size
                    frame = None
                del buffered[:offset]
                await asyncio.sleep(0)
        except asyncio.CancelledError:
            raise
        except (OSError, asyncio.IncompleteReadError, ValueError, struct.error) as error:
            self.closed = type(error).__name__
            for pending in self.pending.values():
                if not pending.done():
                    pending.set_exception(ConnectionError(self.closed))
            self.pending.clear()

    def received_packet(self, opcode, body):
        if opcode == 0x1dd:
            sequence = struct.unpack('<I', body)[0]
            pending = self.pending.pop(sequence, None)
            if pending and not pending.done():
                pending.set_result(time.monotonic())
        elif opcode == 0xee and self.metrics['started'] is not None:
            self.received += 1
            self.metrics['received_movements'] += 1
            if self.received % 100 == 0 and len(self.metrics['motion_ms']) < 100_000:
                offset = 1 + body[0].bit_count()
                timestamp = struct.unpack_from('<I', body, offset + 4)[0]
                elapsed = (time.monotonic() - self.metrics['started']) * 1000
                self.metrics['motion_ms'].append(elapsed - timestamp)
        elif opcode == 0xb7 and self.final_positions:
            values = iter(body[1:])
            guid = int.from_bytes(bytes(next(values) if body[0] & (1 << i) else 0 for i in range(8)), 'little')
            offset = 1 + body[0].bit_count()
            position = struct.unpack_from('<fff', body, offset + 8)
            if guid not in self.final_positions or position != self.final_positions[guid]:
                raise ValueError('incorrect final movement recipient or position')
            self.final_seen.add(guid)

    async def fence(self):
        if self.closed:
            raise ConnectionError(self.closed)
        self.sequence += 1
        sequence = self.sequence
        pending = asyncio.get_running_loop().create_future()
        self.pending[sequence] = pending
        started = time.monotonic()
        self.send(0x1dc, struct.pack('<II', sequence, 0))
        await self.writer.drain()
        try:
            finished = await asyncio.wait_for(pending, timeout=10)
            return (finished - started) * 1000
        finally:
            self.pending.pop(sequence, None)

    async def close(self):
        self.writer.close()
        try:
            await self.writer.wait_closed()
        except OSError:
            pass
        if self.task:
            self.task.cancel()
            await asyncio.gather(self.task, return_exceptions=True)


async def run(pid, port, call, sql, count, seconds, frequency, group_size):
    wasm = ROOT / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'
    module_hash = hashlib.sha256(wasm.read_bytes()).hexdigest()
    metrics = {'started': None, 'received_movements': 0, 'motion_ms': []}
    clients = []
    call('debug_seed_scenario_fixtures', [])
    try:
        for index in range(count):
            username = f'LOAD{index}'
            digits, value = [], index
            for _ in range(4):
                digits.append(chr(ord('a') + value % 26))
                value //= 26
            name = 'Load' + ''.join(digits)
            call('provision_account', [username, [0] * 32, [1] * 32])
            account = sql(f"SELECT id FROM game_account WHERE username = '{username}'")[0]['rows'][0][0]
            call('establish_session', [account, list(KEY), [f'0x{index + 1000:064x}']])
            call('create_character', [account, name, 1, 1, 0, 0, 0, 0, 0, 0])
            guid = sql(f"SELECT guid FROM game_character WHERE name = '{name}'")[0]['rows'][0][0]
            x = 1000 + (index // group_size) * 500
            sql(f'UPDATE game_character SET x = {x}, y = 1000, z = 0 WHERE guid = {guid}')
            reader, writer = await asyncio.open_connection('127.0.0.1', port)
            client = Client(reader, writer, guid, x, metrics)
            clients.append(client)
            challenge = await asyncio.wait_for(reader.readexactly(8), timeout=10)
            assert challenge[:4] == bytes([0, 6, 0xec, 1])
            seed = secrets.token_bytes(4)
            proof = sha(username.encode(), bytes(4), seed, challenge[4:], KEY)
            body = struct.pack('<II', 5875, 0) + username.encode() + b'\0' + seed + proof + bytes(4)
            writer.write(struct.pack('>H', len(body) + 4) + struct.pack('<I', 0x1ed) + body)
            await writer.drain()
            opcode, body = await asyncio.wait_for(client.read(), timeout=10)
            assert opcode == 0x1ee and body[0] == 12, (opcode, body.hex())
            client.task = asyncio.create_task(client.drain())
            client.send(0x3d, struct.pack('<Q', guid))
            await client.fence()
        await asyncio.gather(*(client.fence() for client in clients))
        assert len(sql('SELECT character_guid FROM game_native_player')[0]['rows']) == count
        cpu_start, rss = usage(pid)
        own_start = resource.getrusage(resource.RUSAGE_SELF)
        metrics['started'] = time.monotonic()
        started = metrics['started']
        deadline, next_tick, next_sample = started + seconds, started, started
        sent, expected_received, ping_ms, loop_ms = 0, 0, [], []
        sampled = clients[::max(1, count // 10)]
        while time.monotonic() < deadline:
            now = time.monotonic()
            loop_ms.append(max(0, now - next_tick) * 1000)
            timestamp = int((now - started) * 1000)
            displacement = math.sin((now - started) * 0.5) * 5
            for index, client in enumerate(clients):
                if not client.closed:
                    body = struct.pack('<IIffffI', 0, timestamp, client.x + displacement, 1000, 0, 0, 0)
                    client.send(0xee, body)
                    sent += 1
                    group_start = (index // group_size) * group_size
                    expected_received += min(group_size, count - group_start) - 1
            await asyncio.gather(*(client.writer.drain() for client in clients if not client.closed))
            if now >= next_sample:
                ping_ms.extend(await asyncio.gather(*(client.fence() for client in sampled if not client.closed)))
                rss = max(rss, usage(pid)[1])
                next_sample = now + 1
            next_tick = max(next_tick + 1 / frequency, time.monotonic())
            await asyncio.sleep(max(0, next_tick - time.monotonic()))
        elapsed = time.monotonic() - started
        cpu_end, final_rss = usage(pid)
        own_end = resource.getrusage(resource.RUSAGE_SELF)
        final_started = time.monotonic()
        for index, client in enumerate(clients):
            group_start = (index // group_size) * group_size
            peers = clients[group_start:group_start + group_size]
            client.final_positions = {peer.guid: (peer.x, 1000, 0) for peer in peers if peer != client}
            if not client.closed:
                client.send(0xb7, struct.pack('<IIffffI', 0, int(elapsed * 1000), client.x, 1000, 0, 0, 0))
        await asyncio.gather(*(client.fence() for client in clients if not client.closed))
        while any(len(client.final_seen) != len(client.final_positions) for client in clients):
            if time.monotonic() - final_started >= 10 or any(client.closed for client in clients):
                break
            await asyncio.sleep(0.01)
        final_seconds = time.monotonic() - final_started
        result = {
            'module': 'lyracore-module', 'module_sha256': module_hash,
            'clients': count, 'characters_per_aoi_group': group_size,
            'requested_moves_per_second_per_client': frequency, 'duration_seconds': elapsed,
            'sent_movements': sent, 'sent_movements_per_second': sent / elapsed,
            'received_movements': metrics['received_movements'],
            'uncoalesced_movement_deliveries': expected_received,
            'final_positions_received': sum(len(client.final_seen) for client in clients),
            'final_positions_expected': sum(len(client.final_positions) for client in clients),
            'final_movement_seconds': final_seconds,
            'closed_clients': sum(client.closed is not None for client in clients),
            'host_cpu_cores': (cpu_end - cpu_start) / elapsed,
            'host_peak_sampled_rss_mib': max(rss, final_rss) / 1024 / 1024,
            'driver_cpu_cores': (own_end.ru_utime + own_end.ru_stime - own_start.ru_utime - own_start.ru_stime) / elapsed,
            'request_fence_p95_ms': percentile(ping_ms, 0.95),
            'movement_delivery_p95_ms': percentile(metrics['motion_ms'], 0.95),
            'driver_loop_p95_ms': percentile(loop_ms, 0.95),
            'live_native_characters': len(sql('SELECT character_guid FROM game_native_player')[0]['rows']),
            'aoi_viewers': len(sql('SELECT guid FROM game_aoi_viewer')[0]['rows']),
            'aoi_grants': len(sql('SELECT id FROM game_aoi_grant')[0]['rows']),
        }
        print(json.dumps(result, indent=2))
        assert result['closed_clients'] == 0, 'native clients closed during movement load'
        assert result['final_positions_received'] == result['final_positions_expected'], 'missing final observer positions'
    finally:
        await asyncio.gather(*(client.close() for client in clients), return_exceptions=True)
