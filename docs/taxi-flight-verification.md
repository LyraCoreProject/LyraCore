# Taxi flight verification

The current direct-route baseline rejects any catalogue path containing a nonzero point `delay_ms`
or `flags`. Those fields require client-visible semantics that a continuous spline cannot safely
approximate. If catalogue geometry mutates into that unsupported shape during a paid flight, the
module cancels at the last authoritative position without refunding or granting the destination.

The build-5875 wire harness is maintainer-owned and is not stored in this repository. The checkout
under `.lyracore/wire-harness/<sha>/` is fetched from `.wire-harness-rev`; do not patch that ignored
cache from a LyraCore pull request.

For the next harness release, add a LyraCore adapter scenario that:

1. Logs `TEST` into the seeded fixture and moves it to the reserved flight master.
2. Sends `CMSG_TAXINODE_STATUS_QUERY` and verifies the fixture source is initially unknown.
3. Sends `CMSG_TAXIQUERYAVAILABLENODES`, verifies discovery, and checks taxi-mask bits 255 and 256.
4. Sends `CMSG_ACTIVATETAXI` for the direct 255-to-256 route and expects an OK activation reply.
5. Requires one `SMSG_MONSTER_MOVE` for the player with `RUN_MODE | FLYING`, at least two absolute
   spline points, the seeded mount display, and monotonically advancing observer-visible position.
6. Sends conflicting movement and action input during the flight and verifies it cannot move or act.
7. Waits for the exact seeded destination, then verifies the mount and taxi flag clear and a relog
   rebuilds at that same map, position, and orientation.

Publish that harness change as a tagged release, update `.wire-harness-rev` to the tag and commit,
and run the full adapter suite before removing the pull request's `needs-live-eyeball` marker.

The attended gate remains separate: an unmodified 1.12.1 build-5875 client must confirm the taxi
map, multi-point spline, mount, flight animation, landing cleanup, and session stability by eye.
