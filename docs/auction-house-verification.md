# Stormwind auction house client check

Execution status: outstanding. This checklist needs a human with an unmodified 1.12.1 build-5875
client. The automated suite does not claim this client eyeball.

Use only an isolated non-production LyraCore stack. Never point these steps at a production database,
and never use `spacetime publish -c`. Prepare two characters, Seller and Bidder, with recorded copper
balances and two distinct, mailable item stacks. Record the named Stormwind auctioneer used.

- [ ] On Seller, stand within 10 yards of the auctioneer and open the auction house. Confirm the
      window stays usable.
- [ ] List both exact stacks for 12 hours. Give one a buyout and leave the other below its buyout.
      Confirm the owner tab shows both and Seller lost only the two deposits.
- [ ] On Bidder, browse and search by item name. Confirm both rows, stack details, prices, owner, and
      remaining time.
- [ ] Bid on the expiry listing. Confirm the bidder tab and the exact copper debit.
- [ ] Buy out the other listing with an offer at or above its buyout. Confirm the displayed result
      charges exactly the buyout, not the submitted overbid.
- [ ] At a mailbox, confirm Bidder receives the exact buyout item and Seller receives buyout proceeds:
      price minus the 5 percent Stormwind cut plus its deposit. Collect both and recheck bags and
      copper.
- [ ] After the 12-hour deadline, confirm the bid listing disappears from browse, owner, and bidder
      views. Confirm Bidder receives that exact item and Seller receives winning bid minus the
      5 percent cut plus its deposit.
- [ ] Collect the expiry item and copper. Reopen the mailbox and auction house, then reconnect both
      clients. Confirm no duplicate item, proceeds, refund, or active listing appears.

Record the server commit, client build shown on the login screen, character names, auctioneer name,
auction identifiers, before/after copper, item stack details, and any visible failure or disconnect.
