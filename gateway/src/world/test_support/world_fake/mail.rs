use super::super::*;

/// The Module's Delivery Delay rule (`module/src/mail.rs::delivery_delay_secs`): an item sent to
/// another Realm Account waits one hour, and everything else arrives at once.
pub(crate) fn delivery_delay_secs(has_item: bool, same_account: bool) -> u32 {
    if has_item && !same_account {
        3_600
    } else {
        0
    }
}

/// A mail's `deliver_secs` when it arrives `delay_secs` from now. 0 means from creation.
pub(crate) fn delivered_after(delay_secs: u32) -> i64 {
    if delay_secs == 0 {
        0
    } else {
        crate::world::mail::now_secs() + i64::from(delay_secs)
    }
}

#[derive(Default)]
pub(crate) struct MailState {
    /// The mail rows on THIS database, as `(recipient_guid, row)`. The realm handle owns them on a
    /// sharded gateway and a world shard's staying empty is how a test tells "the mailbox read went
    /// to the authority" from "it quietly went back to being shard-local"; on a single-database
    /// gateway the one handle owns them instead. Same fixture either way — that is the point.
    pub(crate) mails: std::sync::Mutex<Vec<(u64, codec::MailView)>>,
    /// Gameobject guids that ARE a mailbox within reach on this shard. Empty (derive-Default)
    /// refuses every mailbox, which is the wrong-map / out-of-range / not-a-mailbox arm.
    pub(crate) mailboxes: Vec<u64>,
    /// `game_world_entity.money` per guid, on THIS database — the purse the postage comes out of.
    /// A guid with no row here cannot pay, which is also the module's answer for a character with no
    /// live entity on the shard being asked.
    pub(crate) purses: std::sync::Mutex<Vec<(u64, u32)>>,
    /// Letters written on THIS database: `(sender, recipient, subject, body, attached money)`.
    /// Recorded separately from `mails` because WHICH call wrote the row is what tells the two
    /// planes apart — `mail_send` on one database, `mail_commit` on the mail plane of a sharded one.
    #[allow(clippy::type_complexity)]
    pub(crate) sent_mail: std::sync::Mutex<Vec<(u64, u64, String, String, u32)>>,
    /// The escrow ledger on THIS database, as `(the guid the fence is filed under, the row)`. A
    /// letter's fence is filed under its SENDER and lives on their shard; a take's is filed under
    /// the PAYEE and lives on the plane holding the mail row.
    #[allow(clippy::type_complexity)]
    pub(crate) mail_escrows: std::sync::Mutex<Vec<(u64, crate::world::mail::HeldEscrow)>>,
    /// How many `mail_escrows_of` reads answer empty before a just-filed fence shows up. The same
    /// cross-connection lag `escrow_reads_before_visible` dials in for `escrowed_transfer`, made
    /// dialable for the mail escrow read `held_fence` polls.
    pub(crate) mail_escrow_reads_before_visible: std::sync::atomic::AtomicUsize,
    /// The Realm Account name THIS Shard holds per Character guid. A Character missing here is
    /// one this Shard cannot name: absent, or on a shadow Account.
    pub(crate) realm_accounts: std::sync::Mutex<Vec<(u64, String)>>,
    /// The `same_account` each mail send, fence and return carried, in call order.
    pub(crate) same_account_seen: std::sync::Mutex<Vec<(&'static str, bool)>>,
    /// `game_mail_escrow.delivered` per escrow id: the attestation that licenses the settle. Kept
    /// beside the fence rather than in it so the fake cannot settle one it never attested.
    pub(crate) attested: std::sync::Mutex<Vec<(u64, bool)>>,
    /// Delivery/payout receipts on THIS database, `(escrow_id, recipient or payee)` — the
    /// idempotency key that makes a replayed commit or payout a no-op.
    pub(crate) mail_receipts: std::sync::Mutex<Vec<(u64, u64)>>,
    /// `game_item_text` on THIS database: `(item_text_id, text)`. Filed by `mail_copy_text`, read
    /// back by `item_text` — the mail plane's half of a Letter Copy.
    pub(crate) item_texts: std::sync::Mutex<Vec<(u32, String)>>,
    /// Plain Letters `mail_grant_letter` granted on THIS database: `(payee_guid, item_text_id)`.
    /// Not `mail_items` — a Letter Copy mints a fresh item, it never moves one out of a mail row.
    pub(crate) granted_letters: std::sync::Mutex<Vec<(u64, u32)>>,
    /// `game_mail.item_text_id` per mail id: the attached Plain Letter's text id. Kept beside
    /// `mails` because `MailView` does not carry it.
    pub(crate) mail_item_text_ids: std::sync::Mutex<Vec<(u64, u32)>>,
    /// The mail-escrow step to fail on THIS database — a gateway killed before that step's
    /// transaction committed. `transfer`'s `kill_at` for the mail drive, and a `Mutex` because a
    /// re-drive test has to bring the database back up before driving again.
    pub(crate) mail_kill_at: std::sync::Mutex<Option<String>>,
    /// `game_item_instance` on THIS database: `item_guid -> (owner_guid, snapshot)`. The mail
    /// attachment path deletes from here at send and inserts at take, which is the whole "a fenced
    /// item is in nobody's bags" property.
    pub(crate) mail_items: std::sync::Mutex<Vec<(u64, u64, crate::world::mail::AttachedItem)>>,
    /// This shard's bags have no room — the fixture behind the full-bag refusal on a take.
    /// Atomic so a test can flip it AFTER the fixture is wrapped in an `Arc`, like `purses`.
    pub(crate) bags_full: std::sync::atomic::AtomicBool,
}

impl MailStore for WorldFake {
    fn mail_list(&self, recipient_guid: u64) -> Result<Vec<codec::MailView>> {
        self.rec("mail_list");
        Ok(self
            .mail
            .mails
            .lock()
            .unwrap()
            .iter()
            .filter(|(to, _)| *to == recipient_guid)
            .map(|(_, m)| m.clone())
            .collect())
    }

    fn mail_by_id(&self, mail_id: u64) -> Result<Option<codec::MailView>> {
        Ok(self
            .mail
            .mails
            .lock()
            .unwrap()
            .iter()
            .find(|(_, m)| m.id == mail_id)
            .map(|(_, m)| m.clone()))
    }

    fn realm_account_name(&self, character_guid: u64) -> Result<Option<String>> {
        Ok(self
            .mail
            .realm_accounts
            .lock()
            .unwrap()
            .iter()
            .find(|(guid, _)| *guid == character_guid)
            .map(|(_, name)| name.clone()))
    }

    fn mailbox_in_range(&self, mailbox_guid: u64, _player_guid: u64) -> Result<bool> {
        self.rec("mailbox_in_range");
        Ok(self.mail.mailboxes.contains(&mailbox_guid))
    }

    /// Models the module's `apply_mark_read`: the row lookup scoped to `recipient_guid` IS the
    /// authorization, so a mail that exists but belongs to someone else, or has not arrived yet,
    /// fails the same way a nonexistent id does.
    fn mail_mark_read(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_mark_read");
        let mut mails = self.mail.mails.lock().unwrap();
        let now = crate::world::mail::now_secs();
        match mails
            .iter_mut()
            .find(|(to, m)| *to == recipient_guid && m.id == mail_id && m.is_delivered(now))
        {
            Some((_, m)) => {
                m.was_read = true;
                Ok(())
            }
            None => Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL)),
        }
    }

    /// Models the module's `apply_delete`: same merged not-found/not-yours/not-arrived refusal as
    /// mark-read, and a priced mail is refused.
    fn mail_delete(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_delete");
        let mut mails = self.mail.mails.lock().unwrap();
        let now = crate::world::mail::now_secs();
        let Some(at) = mails
            .iter()
            .position(|(to, m)| *to == recipient_guid && m.id == mail_id && m.is_delivered(now))
        else {
            return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
        };
        if mails[at].1.cod > 0 {
            return Err(anyhow!(lyracore_shared::mail::COD_MAIL_UNDELETABLE));
        }
        mails.remove(at);
        Ok(())
    }

    /// Models the module's `apply_return`: the SAME row, re-addressed to whoever sent it, with
    /// whatever it still carries (or nothing) travelling unchanged — except the cash-on-delivery
    /// price, which is dropped, because the row is going back to whoever set it. Only a delivered
    /// Character mail with a sender goes back, and only once. An item going back to another
    /// Account waits its Delivery Delay.
    fn mail_return(&self, recipient_guid: u64, mail_id: u64, same_account: bool) -> Result<()> {
        self.rec("mail_return");
        self.saw_same_account("mail_return", same_account);
        let mut mails = self.mail.mails.lock().unwrap();
        let now = crate::world::mail::now_secs();
        let Some((to, m)) = mails
            .iter_mut()
            .find(|(to, m)| *to == recipient_guid && m.id == mail_id && m.is_delivered(now))
        else {
            return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
        };
        let lyracore_shared::mail::MailSender::Character(sender @ 1..) = m.sender() else {
            return Err(anyhow!(lyracore_shared::mail::NO_SENDER_TO_RETURN_TO));
        };
        if m.check_flags & lyracore_shared::mail::CHECK_MASK_RETURNED != 0 {
            return Err(anyhow!(lyracore_shared::mail::ALREADY_RETURNED));
        }
        m.sender_guid = recipient_guid;
        m.was_read = false;
        m.cod = 0;
        m.check_flags = lyracore_shared::mail::CHECK_MASK_RETURNED;
        m.deliver_secs = now + i64::from(delivery_delay_secs(m.item_entry != 0, same_account));
        *to = sender;
        Ok(())
    }

    /// Models the module's `apply_send`: the postage plus the attached coin leave the purse and the
    /// row is written, in ONE call — the single-database plane's one transaction. The id is
    /// per-database, as the module's `auto_inc` is.
    #[allow(clippy::too_many_arguments)]
    fn mail_send(
        &self,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item_guid: u64,
        same_account: bool,
    ) -> Result<()> {
        self.rec("mail_send");
        self.saw_same_account("mail_send", same_account);
        let item = self.detach(sender_guid, item_guid)?;
        self.debit(sender_guid, lyracore_shared::mail::total_cost(money))?;
        self.mail.sent_mail.lock().unwrap().push((
            sender_guid,
            recipient_guid,
            subject.clone(),
            body.clone(),
            money,
        ));
        self.write_mail(
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            cod,
            &item,
            false,
            delivery_delay_secs(!item.is_empty(), same_account),
        );
        Ok(())
    }

    /// Models the module's `apply_take_item`: the COD debit, the grant, the clear and the seller's
    /// payout row are ONE transaction, so a full bag or a price the taker cannot pay leaves the
    /// letter exactly as it was, and a second take finds an empty one.
    fn mail_take_item(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        let (item, settlement) = {
            let mails = self.mail.mails.lock().unwrap();
            let now = crate::world::mail::now_secs();
            let Some((_, m)) = mails
                .iter()
                .find(|(to, m)| *to == recipient_guid && m.id == mail_id && m.is_delivered(now))
            else {
                return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
            };
            if m.item_entry == 0 {
                return Err(anyhow!(lyracore_shared::mail::NOTHING_TO_TAKE));
            }
            (
                crate::world::mail::AttachedItem {
                    entry: m.item_entry,
                    stack_count: m.item_stack_count,
                    durability: m.item_durability,
                    enchant_id: m.item_enchant_id,
                    soulbound: m.item_soulbound,
                    random_property_id: m.random_property_id,
                    item_text_id: self.attached_text_id(mail_id),
                },
                lyracore_shared::mail::cod_settlement(
                    m.cod,
                    m.sender_guid,
                    &m.subject,
                    recipient_guid,
                ),
            )
        };
        self.rec("mail_take_item");
        if let Some(s) = &settlement {
            self.debit(s.payer_guid, s.copper)
                .map_err(|_| anyhow!(lyracore_shared::mail::COD_NOT_AFFORDABLE))?;
        }
        if let Err(e) = self.store_snapshot(recipient_guid, &item) {
            // The fake cannot roll back, so it undoes the one write it made — the real module gets
            // this from the transaction, and asserting on it is the point of the full-bag test.
            if let Some(s) = &settlement {
                self.credit(s.payer_guid, s.copper);
            }
            return Err(e);
        }
        let mut mails = self.mail.mails.lock().unwrap();
        if let Some((_, m)) = mails
            .iter_mut()
            .find(|(to, m)| *to == recipient_guid && m.id == mail_id)
        {
            m.item_entry = 0;
            m.item_stack_count = 0;
            m.item_durability = 0;
            m.item_enchant_id = 0;
            m.item_soulbound = false;
            m.cod = 0;
        }
        drop(mails);
        self.take_attached_text_id(mail_id);
        if let Some(s) = settlement {
            self.write_mail(
                s.payer_guid,
                s.payee_guid,
                s.subject,
                String::new(),
                s.copper,
                0,
                &crate::world::mail::AttachedItem::default(),
                true,
                0,
            );
        }
        Ok(())
    }

    fn mail_item_room(&self, _payee_guid: u64) -> Result<()> {
        self.rec("mail_item_room");
        if self
            .mail
            .bags_full
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(anyhow!(lyracore_shared::mail::INVENTORY_FULL));
        }
        Ok(())
    }

    /// Models `mail_text::apply_copy_text`: sets COPIED and files the body as item text, on the
    /// database that owns the mail row. Refused for a mail that is not the caller's, is not
    /// delivered, has no body, or is already GRANTED — the same Gates the plan function pins. A
    /// replay before GRANTED is set is `Ok`, and the text insert is skipped when the id already has
    /// a row (a returned mail keeps its id, so a second recipient's copy can reuse it).
    fn mail_copy_text(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_copy_text");
        let mut mails = self.mail.mails.lock().unwrap();
        let now = crate::world::mail::now_secs();
        let Some((_, m)) = mails
            .iter_mut()
            .find(|(to, m)| *to == recipient_guid && m.id == mail_id && m.is_delivered(now))
        else {
            return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
        };
        if m.body.is_empty() {
            return Err(anyhow!("mail: this mail has no text to copy"));
        }
        if m.check_flags & lyracore_shared::mail::CHECK_FLAG_LETTER_GRANTED != 0 {
            return Err(anyhow!("mail: this letter was already made permanent"));
        }
        m.check_flags |= lyracore_shared::mail::CHECK_MASK_COPIED;
        let text_id = lyracore_shared::mail::item_text_id_for(m.id, &m.body);
        let text = m.body.clone();
        drop(mails);
        let mut texts = self.mail.item_texts.lock().unwrap();
        if !texts.iter().any(|(id, _)| *id == text_id) {
            texts.push((text_id, text));
        }
        Ok(())
    }

    /// Models `items::grant_letter_item`: one Plain Letter, refused by the same full-bag fixture
    /// every other grant uses. A no-op when the payee already holds an item carrying `item_text_id`
    /// — the crash-window guard for a retry between this call landing and `mail_mark_letter_granted`
    /// recording that it did, not the durable "already got one" record (that is GRANTED, on the mail
    /// row).
    fn mail_grant_letter(&self, payee_guid: u64, item_text_id: u32) -> Result<()> {
        self.rec("mail_grant_letter");
        if self
            .mail
            .granted_letters
            .lock()
            .unwrap()
            .iter()
            .any(|&(guid, id)| guid == payee_guid && id == item_text_id)
        {
            return Ok(());
        }
        if self
            .mail
            .bags_full
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(anyhow!(lyracore_shared::mail::INVENTORY_FULL));
        }
        self.mail
            .granted_letters
            .lock()
            .unwrap()
            .push((payee_guid, item_text_id));
        Ok(())
    }

    /// Models `mail_text::apply_mark_letter_granted`: sets GRANTED on the mail row. Once this lands,
    /// `mail_copy_text` refuses for good, whether or not the granted item still exists.
    fn mail_mark_letter_granted(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_mark_letter_granted");
        let mut mails = self.mail.mails.lock().unwrap();
        let Some((_, m)) = mails
            .iter_mut()
            .find(|(to, m)| *to == recipient_guid && m.id == mail_id)
        else {
            return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
        };
        m.check_flags |= lyracore_shared::mail::CHECK_FLAG_LETTER_GRANTED;
        Ok(())
    }

    /// Models the Coordinator's `item_text`: a PK read of `game_item_text` on THIS database.
    fn item_text(&self, item_text_id: u32) -> Result<Option<String>> {
        self.rec("item_text");
        Ok(self
            .mail
            .item_texts
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == item_text_id)
            .map(|(_, text)| text.clone()))
    }

    /// Models the Coordinator's `owns_item_with_text`: does `owner_guid` hold a granted letter
    /// carrying `item_text_id`? The fake has no item-guid fixture to answer `hint_item_guid`'s fast
    /// path distinctly, so both routes resolve through the same `granted_letters` lookup.
    fn owns_item_with_text(
        &self,
        owner_guid: u64,
        item_text_id: u32,
        _hint_item_guid: u64,
    ) -> Result<bool> {
        self.rec("owns_item_with_text");
        Ok(item_text_id != 0
            && self
                .mail
                .granted_letters
                .lock()
                .unwrap()
                .iter()
                .any(|&(guid, id)| guid == owner_guid && id == item_text_id))
    }

    /// Models the module's `apply_take_money`: the credit and the clear are one transaction, so a
    /// second take finds an empty row.
    fn mail_take_money(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.rec("mail_take_money");
        let money = {
            let mut mails = self.mail.mails.lock().unwrap();
            let now = crate::world::mail::now_secs();
            let Some((_, m)) = mails
                .iter_mut()
                .find(|(to, m)| *to == recipient_guid && m.id == mail_id && m.is_delivered(now))
            else {
                return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
            };
            if m.money == 0 {
                return Err(anyhow!(lyracore_shared::mail::NOTHING_TO_TAKE));
            }
            std::mem::take(&mut m.money)
        };
        self.credit(recipient_guid, money);
        Ok(())
    }

    /// Models `mail_escrow::apply_fence`: the whole cost leaves the purse into a fence row here.
    fn mail_fence(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        postage: u32,
        item_guid: u64,
        cod: u32,
        cod_source_mail_id: u64,
        same_account: bool,
    ) -> Result<()> {
        self.rec("mail_fence");
        self.saw_same_account("mail_fence", same_account);
        self.mail_kill("mail_fence")?;
        let escrows = self.mail.mail_escrows.lock().unwrap();
        if escrows.iter().any(|(_, e)| e.escrow_id == escrow_id) {
            return Ok(()); // replay — the purse must not be debited twice for one letter
        }
        drop(escrows);
        // The attachment before the debit: it is the refusal that can still fire, and nothing has
        // been written when it does.
        let item = self.detach(sender_guid, item_guid)?;
        self.debit(sender_guid, money.saturating_add(postage))?;
        let delivery_delay_secs = delivery_delay_secs(!item.is_empty(), same_account);
        self.mail.mail_escrows.lock().unwrap().push((
            sender_guid,
            crate::world::mail::HeldEscrow {
                escrow_id,
                recipient_guid,
                subject,
                body,
                money,
                postage,
                payout: false,
                mail_id: cod_source_mail_id,
                item,
                cod,
                delivery_delay_secs,
                reward: None,
            },
        ));
        self.mail.attested.lock().unwrap().push((escrow_id, false));
        Ok(())
    }

    /// Models `mail_escrow::apply_commit`: the row plus a receipt, idempotent on the escrow id.
    fn mail_commit(
        &self,
        escrow_id: u64,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        item: crate::world::mail::AttachedItem,
        cod: u32,
        cod_source_mail_id: u64,
        delivery_delay_secs: u32,
        reward: Option<lyracore_shared::mail::RewardHeader>,
    ) -> Result<()> {
        self.rec("mail_commit");
        self.mail_kill("mail_commit")?;
        let mut receipts = self.mail.mail_receipts.lock().unwrap();
        if receipts.iter().any(|(id, _)| *id == escrow_id) {
            return Ok(());
        }
        // A COD payment pays only a price its payer still owes on a delivered mail.
        if cod_source_mail_id != 0 {
            let now = crate::world::mail::now_secs();
            let owed = self.mail.mails.lock().unwrap().iter().any(|(to, m)| {
                m.id == cod_source_mail_id && *to == sender_guid && m.cod > 0 && m.is_delivered(now)
            });
            if !owed {
                return Err(anyhow!(
                    "mail {cod_source_mail_id} owes {sender_guid} no delivered price"
                ));
            }
        }
        receipts.push((escrow_id, recipient_guid));
        drop(receipts);
        self.mail.sent_mail.lock().unwrap().push((
            sender_guid,
            recipient_guid,
            subject.clone(),
            body.clone(),
            money,
        ));
        self.write_mail(
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            cod,
            &item,
            cod_source_mail_id != 0,
            // A payment arrives at once whatever the commit carries.
            if cod_source_mail_id != 0 {
                0
            } else {
                delivery_delay_secs
            },
        );
        if let Some(header) = reward {
            // Models `crate::world::mail::Letter::reward`: from the quest giver, naming its Mail Template.
            let (sender_kind, sender_guid, sender_entry) = header.giver.sender().columns();
            let mut mails = self.mail.mails.lock().unwrap();
            if let Some((_, m)) = mails.iter_mut().max_by_key(|(_, m)| m.id) {
                m.sender_guid = sender_guid;
                m.sender_kind = sender_kind;
                m.sender_entry = sender_entry;
                m.mail_template_id = header.mail_template_id;
                m.check_flags = lyracore_shared::mail::CHECK_MASK_HAS_BODY;
            }
        }
        // The price stops being owed in the SAME call that delivers the payment for it — the
        // module clears it inside the commit's transaction, which is what makes a COD take charge
        // once however the drive is interrupted.
        if cod_source_mail_id != 0 {
            if let Some((_, m)) = self
                .mail
                .mails
                .lock()
                .unwrap()
                .iter_mut()
                .find(|(_, m)| m.id == cod_source_mail_id)
            {
                m.cod = 0;
            }
        }
        Ok(())
    }

    /// Models `mail_escrow::apply_take_fence`: the copper leaves the ROW into a fence here.
    fn mail_take_money_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_money: u32,
    ) -> Result<()> {
        self.rec("mail_take_money_fence");
        self.mail_kill("mail_take_money_fence")?;
        if self
            .mail
            .mail_escrows
            .lock()
            .unwrap()
            .iter()
            .any(|(_, e)| e.escrow_id == escrow_id)
        {
            return Ok(());
        }
        let money = {
            let mut mails = self.mail.mails.lock().unwrap();
            let now = crate::world::mail::now_secs();
            let Some((_, m)) = mails
                .iter_mut()
                .find(|(to, m)| *to == payee_guid && m.id == mail_id && m.is_delivered(now))
            else {
                return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
            };
            if m.money == 0 {
                return Err(anyhow!(lyracore_shared::mail::NOTHING_TO_TAKE));
            }
            if m.money != expect_money {
                return Err(anyhow!(
                    "refusing to fence an amount the payout would not match"
                ));
            }
            std::mem::take(&mut m.money)
        };
        self.mail.mail_escrows.lock().unwrap().push((
            payee_guid,
            crate::world::mail::HeldEscrow {
                escrow_id,
                recipient_guid: payee_guid,
                subject: String::new(),
                body: String::new(),
                money,
                postage: 0,
                payout: true,
                mail_id,
                item: crate::world::mail::AttachedItem::default(),
                cod: 0,
                delivery_delay_secs: 0,
                reward: None,
            },
        ));
        self.mail.attested.lock().unwrap().push((escrow_id, false));
        Ok(())
    }

    /// Models `mail_escrow::apply_take_item_fence`: the ATTACHMENT leaves the row into a fence.
    fn mail_take_item_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_entry: u32,
    ) -> Result<()> {
        self.rec("mail_take_item_fence");
        self.mail_kill("mail_take_item_fence")?;
        if self
            .mail
            .mail_escrows
            .lock()
            .unwrap()
            .iter()
            .any(|(_, e)| e.escrow_id == escrow_id)
        {
            return Ok(());
        }
        let item = {
            let mut mails = self.mail.mails.lock().unwrap();
            let now = crate::world::mail::now_secs();
            let Some((_, m)) = mails
                .iter_mut()
                .find(|(to, m)| *to == payee_guid && m.id == mail_id && m.is_delivered(now))
            else {
                return Err(anyhow!(lyracore_shared::mail::NOT_YOUR_MAIL));
            };
            if m.item_entry == 0 {
                return Err(anyhow!(lyracore_shared::mail::NOTHING_TO_TAKE));
            }
            if m.item_entry != expect_entry {
                return Err(anyhow!(
                    "refusing to fence an item the grant would not match"
                ));
            }
            let item = crate::world::mail::AttachedItem {
                entry: m.item_entry,
                stack_count: m.item_stack_count,
                durability: m.item_durability,
                enchant_id: m.item_enchant_id,
                soulbound: m.item_soulbound,
                random_property_id: m.random_property_id,
                item_text_id: self.take_attached_text_id(mail_id),
            };
            m.item_entry = 0;
            m.item_stack_count = 0;
            m.item_durability = 0;
            m.item_enchant_id = 0;
            m.item_soulbound = false;
            item
        };
        self.mail.mail_escrows.lock().unwrap().push((
            payee_guid,
            crate::world::mail::HeldEscrow {
                escrow_id,
                recipient_guid: payee_guid,
                subject: String::new(),
                body: String::new(),
                money: 0,
                postage: 0,
                payout: true,
                mail_id,
                item,
                cod: 0,
                delivery_delay_secs: 0,
                reward: None,
            },
        ));
        self.mail.attested.lock().unwrap().push((escrow_id, false));
        Ok(())
    }

    /// Models `mail_escrow::apply_item_payout`: the grant plus a receipt, idempotent on the escrow
    /// id, and refused by a full bag — which leaves the fence holding the item.
    fn mail_item_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        _mail_id: u64,
        item: crate::world::mail::AttachedItem,
    ) -> Result<()> {
        self.rec("mail_item_payout");
        self.mail_kill("mail_item_payout")?;
        let receipts = self.mail.mail_receipts.lock().unwrap();
        if receipts.iter().any(|(id, _)| *id == escrow_id) {
            return Ok(());
        }
        drop(receipts);
        self.store_snapshot(payee_guid, &item)?;
        self.mail
            .mail_receipts
            .lock()
            .unwrap()
            .push((escrow_id, payee_guid));
        Ok(())
    }

    /// Models `mail_escrow::apply_payout`: the credit plus a receipt, idempotent on the escrow id.
    fn mail_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        _mail_id: u64,
        amount: u32,
    ) -> Result<()> {
        self.rec("mail_payout");
        self.mail_kill("mail_payout")?;
        let mut receipts = self.mail.mail_receipts.lock().unwrap();
        if receipts.iter().any(|(id, _)| *id == escrow_id) {
            return Ok(());
        }
        if !self
            .mail
            .purses
            .lock()
            .unwrap()
            .iter()
            .any(|(g, _)| *g == payee_guid)
        {
            return Err(anyhow!(lyracore_shared::mail::NOT_IN_WORLD));
        }
        receipts.push((escrow_id, payee_guid));
        drop(receipts);
        self.credit(payee_guid, amount);
        Ok(())
    }

    fn mail_confirm_delivery(&self, escrow_id: u64) -> Result<()> {
        self.rec("mail_confirm_delivery");
        self.mail_kill("mail_confirm_delivery")?;
        let mut attested = self.mail.attested.lock().unwrap();
        match attested.iter_mut().find(|(id, _)| *id == escrow_id) {
            Some((_, done)) => {
                *done = true;
                Ok(())
            }
            None => Err(anyhow!("mail escrow {escrow_id}: nothing fenced here")),
        }
    }

    /// Models `mail_escrow::apply_settle`, delete-last included: it REFUSES while unattested.
    fn mail_settle(&self, escrow_id: u64) -> Result<()> {
        self.rec("mail_settle");
        self.mail_kill("mail_settle")?;
        let attested = self
            .mail
            .attested
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == escrow_id)
            .map(|(_, done)| *done);
        match attested {
            None => Ok(()), // already settled, or this call reached the wrong database
            Some(false) => Err(anyhow!(
                "mail escrow {escrow_id}: delivery not attested — refusing to destroy the fence"
            )),
            Some(true) => {
                self.mail
                    .mail_escrows
                    .lock()
                    .unwrap()
                    .retain(|(_, e)| e.escrow_id != escrow_id);
                self.mail
                    .attested
                    .lock()
                    .unwrap()
                    .retain(|(id, _)| *id != escrow_id);
                Ok(())
            }
        }
    }

    fn mail_escrows_of(&self, sender_guid: u64) -> Result<Vec<crate::world::mail::HeldEscrow>> {
        self.rec("mail_escrows_of");
        if self
            .mail
            .mail_escrow_reads_before_visible
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |left| left.checked_sub(1),
            )
            .is_ok()
        {
            return Ok(Vec::new()); // the coordinator cache has not caught up yet
        }
        Ok(self
            .mail
            .mail_escrows
            .lock()
            .unwrap()
            .iter()
            .filter(|(owner, _)| *owner == sender_guid)
            .map(|(_, e)| e.clone())
            .collect())
    }
}

impl WorldFake {
    /// One mail-escrow step boundary. `Err` is the gateway dying before this step committed: the
    /// call never lands, and nothing after it in the drive runs either.
    pub(crate) fn mail_kill(&self, step: &str) -> Result<()> {
        if self.mail.mail_kill_at.lock().unwrap().as_deref() == Some(step) {
            anyhow::bail!(
                "injected: the gateway died before {step} on {}",
                self.topology.shard
            );
        }
        Ok(())
    }

    /// Take `copper` out of a purse on THIS database, or refuse and take nothing — the module's
    /// `charge_postage`, including its "no live entity here" arm for a guid with no purse row.
    pub(crate) fn debit(&self, guid: u64, copper: u32) -> Result<()> {
        let mut purses = self.mail.purses.lock().unwrap();
        match purses.iter_mut().find(|(g, _)| *g == guid) {
            Some((_, money)) if *money >= copper => {
                *money -= copper;
                Ok(())
            }
            _ => Err(anyhow!(lyracore_shared::mail::NOT_ENOUGH_MONEY)),
        }
    }

    pub(crate) fn credit(&self, guid: u64, copper: u32) {
        let mut purses = self.mail.purses.lock().unwrap();
        if let Some((_, money)) = purses.iter_mut().find(|(g, _)| *g == guid) {
            *money = money.saturating_add(copper);
        }
    }

    /// The module's `detach_item`: the mailable verdict, then the DELETE. `item_guid` 0 is a letter
    /// with no attachment.
    pub(crate) fn detach(
        &self,
        sender_guid: u64,
        item_guid: u64,
    ) -> Result<crate::world::mail::AttachedItem> {
        if item_guid == 0 {
            return Ok(crate::world::mail::AttachedItem::default());
        }
        let mut items = self.mail.mail_items.lock().unwrap();
        match items
            .iter()
            .position(|(g, owner, _)| *g == item_guid && *owner == sender_guid)
        {
            None => Err(anyhow!(lyracore_shared::mail::NOT_YOUR_ITEM)),
            Some(i) if items[i].2.soulbound => {
                Err(anyhow!(lyracore_shared::mail::ITEM_IS_SOULBOUND))
            }
            Some(i) => Ok(items.remove(i).2),
        }
    }

    /// The module's `store_instance_state`: one new row carrying the recorded state, or the item
    /// module's own full-bag refusal.
    pub(crate) fn store_snapshot(
        &self,
        owner_guid: u64,
        item: &crate::world::mail::AttachedItem,
    ) -> Result<()> {
        if self
            .mail
            .bags_full
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(anyhow!(lyracore_shared::mail::INVENTORY_FULL));
        }
        let mut items = self.mail.mail_items.lock().unwrap();
        let guid = items.iter().map(|(g, _, _)| *g).max().unwrap_or(0) + 1;
        items.push((guid, owner_guid, item.clone()));
        Ok(())
    }

    /// Every item `owner` holds on this database — the assertion surface for "it left the bags",
    /// "it arrived unchanged" and "it never arrived twice".
    pub(crate) fn bags_of(&self, owner: u64) -> Vec<crate::world::mail::AttachedItem> {
        self.mail
            .mail_items
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, o, _)| *o == owner)
            .map(|(_, _, i)| i.clone())
            .collect()
    }

    /// The module's `insert_letter` for a Character's letter: the row both write paths reach, so a
    /// letter written by the single-database send and one written by the escrow's commit cannot
    /// differ. It has HAS_BODY with a body and COPIED without one, like `Letter::from_character`,
    /// and only COD_PAYMENT when it pays a price. It arrives `delay_secs` from now.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn write_mail(
        &self,
        sender_guid: u64,
        recipient_guid: u64,
        subject: String,
        body: String,
        money: u32,
        cod: u32,
        item: &crate::world::mail::AttachedItem,
        cod_payment: bool,
        delay_secs: u32,
    ) {
        let check_flags = if cod_payment {
            lyracore_shared::mail::CHECK_MASK_COD_PAYMENT
        } else if body.is_empty() {
            lyracore_shared::mail::CHECK_MASK_COPIED
        } else {
            lyracore_shared::mail::CHECK_MASK_HAS_BODY
        };
        let mut mails = self.mail.mails.lock().unwrap();
        let id = mails.iter().map(|(_, m)| m.id).max().unwrap_or(0) + 1;
        mails.push((
            recipient_guid,
            codec::MailView {
                id,
                sender_guid,
                subject,
                body,
                money,
                cod,
                item_entry: item.entry,
                item_stack_count: item.stack_count,
                item_durability: item.durability,
                item_enchant_id: item.enchant_id,
                item_soulbound: item.soulbound,
                random_property_id: item.random_property_id,
                created_at_secs: 1_000,
                check_flags,
                deliver_secs: delivered_after(delay_secs),
                ..Default::default()
            },
        ));
        drop(mails);
        if item.item_text_id != 0 {
            self.mail
                .mail_item_text_ids
                .lock()
                .unwrap()
                .push((id, item.item_text_id));
        }
    }

    /// The text id of the letter attached to mail `mail_id`, 0 for any other attachment.
    pub(crate) fn attached_text_id(&self, mail_id: u64) -> u32 {
        self.mail
            .mail_item_text_ids
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| *id == mail_id)
            .map_or(0, |(_, text_id)| *text_id)
    }

    /// Take the attached letter's text id off mail `mail_id`, as the take clears the attachment.
    pub(crate) fn take_attached_text_id(&self, mail_id: u64) -> u32 {
        let mut ids = self.mail.mail_item_text_ids.lock().unwrap();
        let Some(at) = ids.iter().position(|(id, _)| *id == mail_id) else {
            return 0;
        };
        ids.remove(at).1
    }

    pub(crate) fn saw_same_account(&self, call: &'static str, same_account: bool) {
        self.mail
            .same_account_seen
            .lock()
            .unwrap()
            .push((call, same_account));
    }
}
