import socket
import struct
import threading
import unittest

from headless import World


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
        world = World.__new__(World)
        world.socket, world.encrypt, world.decrypt = client, Plain, Plain
        return world

    def ping_sequence(self):
        return struct.unpack('<xxxxxxII', self.server.recv(14))[0]

    def test_a_lost_pong_gets_a_new_fence(self):
        world = self.world(timeout=5)

        def lose_first_pong():
            self.ping_sequence()
            self.server.sendall(pong_frame(self.ping_sequence()))

        peer = threading.Thread(target=lose_first_pong)
        peer.start()
        self.assertEqual(world.collect(), [])
        peer.join()

    def test_a_session_that_never_answers_times_out(self):
        with self.assertRaises(TimeoutError):
            self.world(timeout=2).collect()


if __name__ == '__main__':
    unittest.main()
