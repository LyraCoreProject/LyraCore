# Auction house client check

Execution status: outstanding. This checklist needs a human with an unmodified 1.12.1 build-5875
client. The automated suite does not claim this client eyeball.

Use only an isolated non-production LyraCore stack. Never point these steps at a production database,
and never use `spacetime publish -c`. Prepare two characters, Seller and Bidder, with recorded copper
balances and two distinct, mailable item stacks. Record the named auctioneer and imported house
used. If possible, repeat the checklist at a house with different imported rates.

- [ ] On Seller, stand within 10 yards of the auctioneer and open the auction house. Confirm the
      window stays usable.
- [ ] List both exact stacks for 12 hours. Give one a buyout and leave the other below its buyout.
      Confirm the owner tab shows both and Seller lost only the two deposits.
- [ ] On Bidder, browse and search by item name. Confirm both rows, stack details, prices, owner, and
      remaining time.
- [ ] Bid on the expiry listing. Confirm the bidder tab and the exact copper debit.
- [ ] Buy out the other listing with an offer at or above its buyout. Confirm the displayed result
      charges exactly the buyout, not the submitted overbid.
- [ ] At a mailbox, confirm Bidder receives the exact buyout item and Seller receives buyout
      proceeds: price minus the house's imported consignment percentage plus its imported-rate
      deposit. Collect both and recheck bags and copper.
- [ ] After the 12-hour deadline, confirm the bid listing disappears from browse, owner, and bidder
      views. Confirm Bidder receives that exact item and Seller receives winning bid minus the
      imported consignment percentage plus its deposit.
- [ ] Collect the expiry item and copper. Reopen the mailbox and auction house, then reconnect both
      clients. Confirm no duplicate item, proceeds, refund, or active listing appears.
- [ ] Get outbid on a listing. Confirm the bidder's mail shows the subject "Outbid on `<item name>`"
      from the auction house, with no Return.
- [ ] Let a listing settle by buyout or by winning bid. Confirm the bidder's mail shows the subject
      "Auction won: `<item name>`" from the house, the invoice names the winning price, and there is
      no Return.
- [ ] After that same settlement, confirm the seller's mail shows the subject "Auction successful:
      `<item name>`" from the house, the invoice names the sale price, and there is no Return.
- [ ] Let a listing run past its deadline with no winning bid. Confirm the seller's mail shows the
      subject "Auction expired: `<item name>`" from the house, the item attached, and no Return.
- [ ] Cancel an active listing that has no bid. Confirm the item returns to the seller by mail with
      the subject "Auction cancelled: `<item name>`" from the house, and the seller's copper is
      unchanged.
- [ ] Cancel an active listing that has a bid. Confirm the seller's copper drops by exactly the
      Auction Cut, the bidder's mail shows an "Auction cancelled" refund of the full bid, and the
      item returns to the seller by mail.
- [ ] Attempt Cancellation with copper below the Auction Cut. Confirm the client shows no error and
      the listing stays active and unchanged.
- [ ] With a second client on a Character on another shard, in turn trigger an outbid, a win, a sale,
      an expiry and a cancellation. Confirm that Character's chat shows, in order,
      `ERR_AUCTION_OUTBID_S`, `ERR_AUCTION_WON_S`, `ERR_AUCTION_SOLD_S`, `ERR_AUCTION_EXPIRED_S` and
      `ERR_AUCTION_REMOVED_S`.
- [ ] Stand at the Stormwind, Ironforge and Booty Bay auctioneers in turn. Confirm each opens its
      auction house.
- [ ] List an item at the Stormwind auctioneer, then browse for it from Ironforge and from Darnassus.
      Confirm the same listing shows at both, since Stormwind, Ironforge and Darnassus share one
      Alliance market. List a second item at Booty Bay and confirm it does not show from Stormwind,
      Ironforge or Darnassus, since Booty Bay's market is the neutral one, shared with neither team.

Record the server commit, client build shown on the login screen, character names, auctioneer name,
auction identifiers, before/after copper, item stack details, and any visible failure or disconnect.
