# Meeting stone client check

Execution status: outstanding. This checklist needs a human with an unmodified 1.12.1 build-5875
client. The automated suite proves the queue, the Stone Adds and the event order on durable
databases. It cannot prove what the client sends or draws.

Use only an isolated non-production LyraCore stack. Never point these steps at a production database,
and never use `spacetime publish -c`. Run the stack sharded (`./lyracore dev up`, not `--single`),
so the Deadmines instance is on the Instance Pool and Westfall is on the default World Shard. Import
the `gameobjects` family with this change, or `game_meeting_stone` is empty and every stone is
silent. Run the Gateway at debug log level.

Prepare three Alliance Characters on two clients: Leader and Seeker inside the Deadmines stone's
level range, and Low below it. Leader needs three playerbots it can invite.

- [ ] Hover the Deadmines meeting stone in Westfall. Confirm the tooltip shows its level range.
- [ ] On Low, right-click the stone. Confirm the client refuses with its own level message and the
      Gateway log shows no meeting stone line for Low.
- [ ] On Seeker, right-click the stone. Confirm the minimap button appears, its tooltip names The
      Deadmines, and chat shows the joined line. A `CMSG_GAMEOBJ_USE` on a stone does nothing on
      this server, so the button proves the client sent `CMSG_MEETINGSTONE_JOIN`.
- [ ] Click the minimap button and confirm the dialog. Confirm the button goes and chat shows the
      left-queue line.
- [ ] Queue Seeker again and enter the Deadmines instance portal. After the loading screen, confirm
      the button is still there: the client asked `CMSG_MEETINGSTONE_INFO` from the Instance Pool.
- [ ] Log Seeker out and back in. Confirm no button and no meeting stone line. Logout drops a solo
      Seeker silently.
- [ ] Put Leader in a party it does not lead and right-click the stone. Confirm the not-leader
      failure line. Repeat as the leader of a Raid and of a full Party, and confirm the raid and the
      full-group failure lines.
- [ ] With Seeker inside the Deadmines instance, have Leader invite Seeker and queue the Party at the
      stone. Confirm both clients show the button and the joined line, although Seeker is on
      another shard.
- [ ] Have Seeker leave that queued Party. Confirm Seeker's button goes, and Leader sees the
      member-left line and keeps its button.
- [ ] Invite Seeker again, queue, and kick Seeker. Confirm Leader sees the member-removed line and
      loses its button. Confirm Seeker sees the looking-for-a-new-party line and keeps a button.
- [ ] Queue a Party with Leader and Seeker, then pass the lead to Seeker. Confirm both keep the
      button and no meeting stone line appears.
- [ ] Queue the Party again and convert it to a Raid. Confirm every member loses the button and sees
      the left-queue line. Both cores keep a Raid queued; this server does not.
- [ ] Leave Leader's Party queued for 5 minutes. Confirm every member sees the in-progress line.
- [ ] Queue Seeker alone and send it into the Deadmines instance. Invite three playerbots to Leader
      by ordinary invite, with classes that leave a role open that Seeker's class fills, and queue
      Leader's Party in Westfall. Confirm Leader sees the member-added line for Seeker, then every
      member sees the complete line and loses the button, in that order.
- [ ] In that stone-formed Party, confirm Seeker's party frame lists all five members, `/p` reaches
      both clients across the two shards, and a kill near Leader and Seeker in Westfall splits its XP
      between them.

Record the server commit, client build shown on the login screen, Character names and levels, the
stone's level range, each chat line seen, and any visible failure or disconnect.
