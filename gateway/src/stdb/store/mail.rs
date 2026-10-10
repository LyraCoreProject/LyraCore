//! `Coordinator`'s [`MailStore`] adapter.

use anyhow::Result;

use crate::codec;
use crate::world::MailStore;

use crate::stdb::Coordinator;

impl MailStore for Coordinator {
    fn mail_list(&self, recipient_guid: u64) -> Result<Vec<codec::MailView>> {
        self.mail_list(recipient_guid)
    }

    fn mail_by_id(&self, mail_id: u64) -> Result<Option<codec::MailView>> {
        Ok(self.mail_by_id(mail_id))
    }

    fn realm_account_name(&self, character_guid: u64) -> Result<Option<String>> {
        Ok(self.realm_account_name(character_guid))
    }

    fn mailbox_in_range(&self, mailbox_guid: u64, player_guid: u64) -> Result<bool> {
        self.mailbox_in_range(mailbox_guid, player_guid)
    }

    fn mail_mark_read(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_mark_read(recipient_guid, mail_id)
    }

    fn mail_delete(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_delete(recipient_guid, mail_id)
    }

    fn mail_return(&self, recipient_guid: u64, mail_id: u64, same_account: bool) -> Result<()> {
        self.mail_return(recipient_guid, mail_id, same_account)
    }

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
        self.mail_send(
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            cod,
            item_guid,
            same_account,
        )
    }

    fn mail_take_money(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_take_money(recipient_guid, mail_id)
    }

    fn mail_take_item(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_take_item(recipient_guid, mail_id)
    }

    fn mail_item_room(&self, payee_guid: u64) -> Result<()> {
        self.mail_item_room(payee_guid)
    }

    fn mail_copy_text(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_copy_text(recipient_guid, mail_id)
    }

    fn mail_grant_letter(&self, payee_guid: u64, item_text_id: u32) -> Result<()> {
        self.mail_grant_letter(payee_guid, item_text_id)
    }

    fn mail_mark_letter_granted(&self, recipient_guid: u64, mail_id: u64) -> Result<()> {
        self.mail_mark_letter_granted(recipient_guid, mail_id)
    }

    fn item_text(&self, item_text_id: u32) -> Result<Option<String>> {
        self.item_text(item_text_id)
    }

    fn owns_item_with_text(
        &self,
        owner_guid: u64,
        item_text_id: u32,
        hint_item_guid: u64,
    ) -> Result<bool> {
        self.owns_item_with_text(owner_guid, item_text_id, hint_item_guid)
    }

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
        self.mail_fence(
            escrow_id,
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            postage,
            item_guid,
            cod,
            cod_source_mail_id,
            same_account,
        )
    }

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
        self.mail_commit(
            escrow_id,
            sender_guid,
            recipient_guid,
            subject,
            body,
            money,
            item,
            cod,
            cod_source_mail_id,
            delivery_delay_secs,
            reward,
        )
    }

    fn mail_take_money_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_money: u32,
    ) -> Result<()> {
        self.mail_take_money_fence(escrow_id, payee_guid, mail_id, expect_money)
    }

    fn mail_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        amount: u32,
    ) -> Result<()> {
        self.mail_payout(escrow_id, payee_guid, mail_id, amount)
    }

    fn mail_take_item_fence(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        expect_entry: u32,
    ) -> Result<()> {
        self.mail_take_item_fence(escrow_id, payee_guid, mail_id, expect_entry)
    }

    fn mail_item_payout(
        &self,
        escrow_id: u64,
        payee_guid: u64,
        mail_id: u64,
        item: crate::world::mail::AttachedItem,
    ) -> Result<()> {
        self.mail_item_payout(escrow_id, payee_guid, mail_id, item)
    }

    fn mail_confirm_delivery(&self, escrow_id: u64) -> Result<()> {
        self.mail_confirm_delivery(escrow_id)
    }

    fn mail_settle(&self, escrow_id: u64) -> Result<()> {
        self.mail_settle(escrow_id)
    }

    fn mail_escrows_of(&self, sender_guid: u64) -> Result<Vec<crate::world::mail::HeldEscrow>> {
        self.mail_escrows_of(sender_guid)
    }
}
