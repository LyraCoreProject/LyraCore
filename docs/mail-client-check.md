# Mail client check

Execution status: outstanding. This checklist needs a human with an unmodified 1.12.1 build-5875
client. The automated suite does not claim this client eyeball.

Use only an isolated non-production LyraCore stack. Never point these steps at a production database,
and never use `spacetime publish -c`. Point the WoW client's realmlist at 127.0.0.1, not production.
Prepare a Character on Account A, a Character on Account B, and an alt Character on Account A, each
with a recorded copper balance and a mailable item stack. Record the server commit and the client
build shown on the login screen before starting.

- [ ] Send mail with an item between the two Account-A characters. Confirm it arrives at once, with
      the new-mail envelope and a toast.
- [ ] Send money-only mail from Account A to Account B. Confirm it arrives at once.
- [ ] Send mail with an item from Account A to Account B. Confirm nothing arrives yet, then confirm
      it arrives about one hour later, with the new-mail envelope and a toast.
- [ ] Open the mailbox, read one mail, close the window, walk away and back, and reopen the mailbox.
      Confirm the mail still shows read.
- [ ] Send three mails in sequence and open the mailbox. Confirm the list shows them newest first.
- [ ] Send an item with a cash on delivery price from one Account-A Character to the Account-A alt.
      On the alt, take the item and pay. Confirm the sender's inbox shows the payment as
      "COD Payment: `<original subject>`", with the prefix once, not twice.
- [ ] Send an item from one Account-A Character to the Account-A alt. On the alt, return it. Confirm
      the sender gets it back at once, it shows as returned, and the mailbox offers Delete on it, not
      Return.
- [ ] Call `debug_stage_mail_expiry_fixture` on the non-production database. Its "timer: returns"
      letter is 30 days old, so its Mail Timer returns it to its sender, Character 5090070, at once.
      Then give it to one of your Characters with
      `spacetime sql <database> "UPDATE game_mail SET recipient_guid = <your guid> WHERE subject = 'timer: returns'"`
      (a u64 column, so no Timestamp literal is needed). Log in and confirm the mail shows as
      returned, with its item, and offers Delete, not Return.
- [ ] On a Character with a mail that has a body, use "Make Permanent Copy" to create a Letter Copy,
      then delete that mail. Confirm the Letter Copy still reads correctly from the bags.
- [ ] Mail that Letter Copy to another Character and take it there. Confirm it still reads correctly.
- [ ] Trade that Letter Copy to another Character. Confirm it still reads correctly after the trade.
- [ ] On a sharded stack, carry that Letter Copy across a Shard Boundary: take the boat or zeppelin
      from Eastern Kingdoms to Kalimdor. Confirm it still reads correctly on the new Shard.
- [ ] List that Letter Copy on the auction house and buy it out on the other Character. Confirm it
      still reads correctly after the sale.
- [ ] Turn in a quest from the imported data that sends a Reward Letter, for example one of the
      Membership Card Renewal quests 3644 to 3647, and wait out its delay. Confirm the inbox shows
      the quest giver's name as the sender, the subject the client renders for an empty subject with
      a Mail Template id, the template text once, not twice, with `$B` and `$n` filled in by the
      client, the attached item, and no Return button.
- [ ] Optional. As the seeded Character 1, call `debug_stage_reward_letter_fixture` (operator-only)
      and turn in its quest shaped like quest 8728, which sends 1,000,000 copper after 129,600
      seconds. Make the letter visible at once with
      `spacetime sql <database> "UPDATE game_mail SET deliver_micros = 0 WHERE recipient_guid = 1"`.
      Reopen the mailbox and confirm it shows the sending creature's name, the copper, and no Return
      button. This path shows no new-mail toast, because the Mail Arrival fires at the old instant.

Record the server commit, client build shown on the login screen, character names and Accounts,
before/after copper, item and Letter Copy details, and any visible failure or disconnect.
