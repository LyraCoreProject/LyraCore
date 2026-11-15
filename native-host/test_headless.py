from concurrent.futures import ThreadPoolExecutor
import socket
import struct
import unittest

from headless import World, read_exact


class Plain:
    @staticmethod
    def encrypt(data):
        return data

    decrypt = encrypt


def pong_frame(sequence):
    return struct.pack('>H', 6) + struct.pack('<HI', 0x1dd, sequence)


class CollectTest(unittest.TestCase):
    def world(self, timeout):
        client, self.server = socket.socketpair()
        self.addCleanup(client.close)
        self.addCleanup(self.server.close)
        client.settimeout(timeout)
        self.server.settimeout(timeout)
        world = World.__new__(World)
        world.socket, world.encrypt, world.decrypt = client, Plain, Plain
        return world

    def ping_sequence(self):
        return struct.unpack('<xxxxxxII', read_exact(self.server, 14))[0]

    def test_logout_completion_survives_a_lost_fence(self):
        world = self.world(timeout=5)
        logout = [(0x4c, bytes([0, 0, 0, 0, 1])), (0x4d, b'')]

        def lose_first_pong():
            self.ping_sequence()
            for opcode, body in logout:
                self.server.sendall(struct.pack('>H', len(body) + 2) + struct.pack('<H', opcode) + body)
            self.server.sendall(pong_frame(self.ping_sequence()))

        with ThreadPoolExecutor(max_workers=1) as peer:
            reply = peer.submit(lose_first_pong)
            self.assertEqual(world.collect(), logout)
            reply.result()

    def test_a_session_that_never_answers_times_out(self):
        with self.assertRaises(TimeoutError):
            self.world(timeout=2).collect()


if __name__ == '__main__':
    unittest.main()
