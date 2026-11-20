from collections import namedtuple
from sql_defaults import insert_defaults

import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

from headless import KEY, World, creates, verifier

ROOT = Path(__file__).resolve().parent.parent
MODULE = ROOT / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'

Account = namedtuple('Account', 'id username identity')
SETTLE_SECONDS = 30
PUBLISH_SECONDS = 600
# Transfer waits end before the 30 s stale Escrow reap, which would also clear the state they wait on.
TRANSFER_SECONDS = 15


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


class Host:
    """A Native Host on free local ports with its own data directory, the Module published and
    the Operator claimed. Use it as a context manager; it stops the process and deletes the data
    on exit."""

    def __init__(self, binary, module=MODULE, dev=False, seeded=False, config=''):
        self.binary = Path(binary)
        self.module = Path(module)
        self.dev = dev
        self.seeded = seeded
        self.config = config
        self.clients = []
        self.restorations = []
        self.identities = 0
        self.databases = []
        self.process = None
        self.token = None

    def __enter__(self):
        self.directory = tempfile.TemporaryDirectory(prefix='lyracore-native-check-')
        run = Path(self.directory.name)
        self.private, self.public = run / 'private.pem', run / 'public.pem'
        subprocess.run(['openssl', 'genpkey', '-algorithm', 'EC', '-pkeyopt', 'ec_paramgen_curve:P-256', '-out', str(self.private)], check=True, capture_output=True)
        self.private.chmod(0o600)
        subprocess.run(['openssl', 'ec', '-in', str(self.private), '-pubout', '-out', str(self.public)], check=True, capture_output=True)
        self.database = f'disposable-native-{time.time_ns()}'
        self.data = run / 'data'
        self.data.mkdir()
        config = '[logs]\nlevel = "WARN"\ndirectives = ["spacetimedb_standalone::native_world=debug"]\n'
        (self.data / 'config.toml').write_text(config + self.config)
        self.log_path = run / 'host.log'
        self.log = self.log_path.open('w')
        try:
            self.start_on_free_ports()
            self.publish(self.module.read_bytes())
            self.call('claim_operator')
            self.call('install_guid_range', [0])
            if self.seeded:
                self.call('debug_seed_scenario_fixtures')
            self.characters = self.character_guids()
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def start_on_free_ports(self):
        """Another process can take a port between free_port() and the Host binding it, which stops the Host."""
        for _ in range(3):
            ports = set()
            while len(ports) < 3:
                ports.add(free_port())
            self.http_port, self.world_port, self.logon_port = ports
            self.start()
            self.wait_until(lambda: self.identify() or self.process.poll() is not None, 'native host did not start', timeout=20)
            if self.process.poll() is None:
                return
        raise AssertionError('native host stopped on every set of free ports')

    def __exit__(self, *_):
        self.stop()
        self.log.close()
        self.directory.cleanup()

    def start(self):
        env = dict(os.environ, LYRACORE_NATIVE_WORLD_ADDR=f'127.0.0.1:{self.world_port}',
            LYRACORE_NATIVE_LOGON_ADDR=f'127.0.0.1:{self.logon_port}', LYRACORE_NATIVE_DATABASE=self.database)
        if self.dev:
            env['LYRACORE_NATIVE_DEV'] = '1'
        self.process = subprocess.Popen([str(self.binary), 'start', '--listen-addr', f'127.0.0.1:{self.http_port}',
            '--data-dir', str(self.data), '--non-interactive', '--jwt-priv-key-path', str(self.private),
            '--jwt-pub-key-path', str(self.public), '--page_pool_max_size', '1073741824'],
            env=env, stdout=self.log, stderr=subprocess.STDOUT)

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()

    def restart(self, before_start=None, dev=None):
        """Kills the process, runs `before_start`, then starts it on the same data and ports."""
        self.process.kill()
        self.process.wait(timeout=10)
        if before_start:
            before_start()
        if dev is not None:
            self.dev = dev
        self.start()
        for database in (None, *self.databases):
            self.wait_until(lambda: self.answers(database), f'database {database or self.database} did not recover')

    @property
    def pid(self):
        return self.process.pid

    def log_text(self):
        self.log.flush()
        return self.log_path.read_text()

    def request(self, path, body=None, method='GET', content_type='application/json', authenticated=True, timeout=120):
        headers = {'Content-Type': content_type}
        if self.token and authenticated:
            headers['Authorization'] = 'Bearer ' + self.token
        request = urllib.request.Request(f'http://127.0.0.1:{self.http_port}{path}', body, headers, method=method)
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            raise RuntimeError(f'{path}: HTTP {error.code}: {error.read().decode()}') from error

    def identify(self):
        try:
            self.token = json.loads(self.request('/v1/identity', b'', 'POST'))['token']
        except (OSError, RuntimeError):
            return False
        return True

    def answers(self, database=None):
        try:
            self.sql('SELECT id FROM game_account', database)
        except (OSError, RuntimeError):
            return False
        return True

    def publish(self, wasm, database=None):
        return self.request(f'/v1/database/{database or self.database}?host_type=Wasm', wasm, 'PUT', 'application/wasm', timeout=PUBLISH_SECONDS)

    def add_database(self, guid_base=1):
        """Publishes and claims another database on this Host. Returns its name."""
        database = f'{self.database}-extra-{len(self.databases) + 1}'
        self.publish(self.module.read_bytes(), database)
        self.call('claim_operator', database=database)
        self.call('install_guid_range', [guid_base], database=database)
        self.databases.append(database)
        return database

    def identity(self, database=None):
        identity = self.request(f'/v1/database/{database or self.database}/identity').decode().strip('"')
        return {'__identity__': '0x' + identity.removeprefix('0x')}

    def call(self, reducer, args=(), operator=True, database=None):
        """Calls a reducer as the Operator, or anonymously. Raises RuntimeError when the call fails."""
        return self.request(f'/v1/database/{database or self.database}/call/{reducer}',
            json.dumps(list(args)).encode(), 'POST', authenticated=operator)

    def sql(self, query, database=None):
        query = insert_defaults(query)
        return json.loads(self.request(f'/v1/database/{database or self.database}/sql', query.encode(), 'POST',
            'text/plain'))

    def rows(self, query, database=None):
        return self.sql(query, database)[0]['rows']

    def wait_until(self, condition, message, timeout=SETTLE_SECONDS, interval=0.05):
        """Returns the first truthy result of `condition()`, or raises AssertionError(message)."""
        deadline = time.monotonic() + timeout
        while True:
            result = condition()
            if result:
                return result
            if time.monotonic() >= deadline:
                raise AssertionError(message)
            time.sleep(interval)

    def await_guild_settled(self, guid, *databases):
        """Waits until the Character has no guild transaction on any of the databases. One refuses a Transfer
        and a cross-map teleport, as does any pending payment (`payment_pending`), Auction ones included.
        Without databases it checks the default one."""
        self.wait_until(lambda: not any(self.rows(f'SELECT guid FROM game_guild_transaction WHERE guid = {guid}', database)
                                        for database in databases or (None,)),
                        f'guild transaction of Character {guid} did not settle')

    def await_empty(self, table):
        self.wait_until(lambda: not self.rows(f'SELECT * FROM {table}'), f'{table} retained disconnected clients')

    def account(self, username, password=None, key=KEY, database=None):
        """Provisions an Account. Without a password it also establishes a Session for `key`, so a
        World Session can authenticate without the logon port."""
        if password is not None:
            salt = secrets.token_bytes(32)
            self.call('provision_account', [username, list(salt), list(verifier(username, password, salt))])
            return Account(self.rows(f"SELECT id FROM game_account WHERE username = '{username}'")[0][0], username, None)
        self.call('provision_account', [username, [0] * 32, [1] * 32], database=database)
        account = self.rows(f"SELECT id FROM game_account WHERE username = '{username}'", database)[0][0]
        self.identities += 1
        identity = f'0x{self.identities:064x}'
        self.call('establish_session', [account, list(key), [identity]], database=database)
        return Account(account, username, identity)

    def character(self, name, race=1, cls=1, account=None, at=None):
        """Creates a Character, by default on a new Account named after it. `at` is (map, x, y, z)."""
        account = account or self.account(name.upper())
        self.call('create_character', [account.id, name, race, cls, 0, 0, 0, 0, 0, 0])
        guid = self.rows(f"SELECT guid FROM game_character WHERE name = '{name}'")[0][0]
        if at:
            map_id, x, y, z = at
            self.sql(f'UPDATE game_character SET map_id = {map_id}, x = {x}, y = {y}, z = {z} WHERE guid = {guid}')
        return guid

    def connect(self, username='NATIVE', **options):
        """Opens a World Session that `reset()` closes. Options go to `World`."""
        client = World(self.world_port, username=username, **options)
        self.clients.append(client)
        return client

    def log_in(self, guid, key=KEY):
        """Opens a World Session for the Character's Account with `key` and enters the world. The
        Account needs a Session for `key`. The World Session keeps its entry packets in `entered`."""
        account = self.rows(f'SELECT account_id FROM game_character WHERE guid = {guid}')[0][0]
        username = self.rows(f'SELECT username FROM game_account WHERE id = {account}')[0][0]
        client = self.connect(username, key=key)
        opcode, body = client.read()
        assert (opcode, body[:1]) == (0x1ee, b'\x0c'), (hex(opcode), body.hex())
        client.send(0x3d, guid.to_bytes(8, 'little'))
        client.entered = client.collect()
        assert guid in creates(client.entered), f'Character {guid} did not enter the world'
        client.guid = guid
        return client

    def enter_world(self, name, race=1, cls=1, account=None, at=None):
        return self.log_in(self.character(name, race, cls, account, at))

    def character_guids(self):
        return {row[0] for row in self.rows('SELECT guid FROM game_character')}

    def restore(self, table, condition, *columns):
        """Records the columns of the row that matches `condition`, so that `reset()` writes them
        back. The columns must hold numbers or booleans. When no row matches, `reset()` deletes the
        rows that match then."""
        rows = self.rows(f'SELECT {", ".join(columns) or "*"} FROM {table} WHERE {condition}')
        assert not columns or len(rows) <= 1, f'restore() needs at most one row in {table} WHERE {condition}'
        self.restorations.append((table, condition, columns, rows))

    def reset(self):
        """Closes the World Sessions this Host opened, deletes every Character created since start
        and writes back the rows recorded by `restore()`."""
        for client in self.clients:
            client.close()
        self.clients.clear()
        self.await_empty('game_native_player')
        for guid in sorted(self.character_guids() - self.characters):
            self.call('debug_delete_character', [guid])
        while self.restorations:
            table, condition, columns, rows = self.restorations.pop()
            if not rows:
                self.sql(f'DELETE FROM {table} WHERE {condition}')
            elif columns:
                values = ', '.join(f'{column} = {json.dumps(value)}' for column, value in zip(columns, rows[0]))
                self.sql(f'UPDATE {table} SET {values} WHERE {condition}')
