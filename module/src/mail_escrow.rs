use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table};

// Retired Mail Escrow tables keep their declarations; see docs/danger-zones.md.
#[table(
    accessor = game_mail_escrow,
    index(accessor = by_sender, btree(columns = [sender_guid]))
)]
pub struct MailEscrow {
    #[primary_key]
    pub escrow_id: u64,
    pub sender_guid: u64,
    pub recipient_guid: u64,
    pub subject: String,
    pub body: String,
    pub money: u32,
    pub postage: u32,
    pub created_micros: i64,
    pub delivered: bool,
    #[default(false)]
    pub payout: bool,
    #[default(0u64)]
    pub mail_id: u64,
    #[default(0u32)]
    pub item_entry: u32,
    #[default(0u32)]
    pub item_stack_count: u32,
    #[default(0u32)]
    pub item_durability: u32,
    #[default(0u32)]
    pub item_enchant_id: u32,
    #[default(false)]
    pub item_soulbound: bool,
    #[default(0u32)]
    pub cod: u32,
    #[default(0)]
    pub random_property_id: u32,
    #[default(0u32)]
    pub delivery_delay_secs: u32,
    #[default(0u8)]
    pub sender_kind: u8,
    #[default(0u32)]
    pub sender_entry: u32,
    #[default(0u32)]
    pub mail_template_id: u32,
    #[default(0u32)]
    pub item_text_id: u32,
}

#[table(
    accessor = game_mail_delivery,
    index(accessor = by_recipient, btree(columns = [recipient_guid]))
)]
pub struct MailDelivery {
    #[primary_key]
    pub escrow_id: u64,
    pub mail_id: u64,
    pub recipient_guid: u64,
    pub created_micros: i64,
}
#[table(accessor = game_mail_escrow_reaper_schedule, scheduled(reap_mail_escrows))]
pub struct MailEscrowReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}
crate::character_owned!(delete, fn sweep_delete_game_mail_escrow(ctx, character_guid) {
    let escrows = ctx.db.game_mail_escrow();
    for r in escrows.by_sender().filter(&character_guid).collect::<Vec<_>>() {
        escrows.escrow_id().delete(r.escrow_id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_mail_escrow(ctx, character_guid, io) {
    table = game_mail_escrow,
    by = by_sender,
    keep_key,
});
crate::character_owned!(delete, fn sweep_delete_game_mail_delivery(ctx, character_guid) {
    let receipts = ctx.db.game_mail_delivery();
    for r in receipts.by_recipient().filter(&character_guid).collect::<Vec<_>>() {
        receipts.escrow_id().delete(r.escrow_id);
    }
});
crate::character_owned!(not_transported, fn sweep_transfer_game_mail_delivery());

// Migration cannot remove a schedule, so a leftover row fires once and deletes itself.
#[reducer]
pub fn reap_mail_escrows(ctx: &ReducerContext, schedule: MailEscrowReaperSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    ctx.db
        .game_mail_escrow_reaper_schedule()
        .scheduled_id()
        .delete(schedule.scheduled_id);
}
