use spacetimedb::{
    reducer, sats::bsatn, table, ReducerContext, ScheduleAt, SpacetimeType, Table, TimeDuration,
};

use super::directory::{self, game_character_directory, game_character_profile};
use crate::{
    mail::{self, Mail},
    mail_text::{self, game_item_text},
    peer::{realm_shard, require_sender, world_shard},
    remote_bindings::peer as remote,
};

#[derive(SpacetimeType)]
pub struct MailParcel {
    mail: Mail,
    text: Option<String>,
}

#[table(accessor = game_mail_forward, outbox(remote::idc_mail_receive))]
pub struct MailForward {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub parcel: Vec<u8>,
}

#[table(accessor = game_mail_forward_retry, scheduled(retry_mail_forward))]
pub struct MailForwardRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub parcel: Vec<u8>,
}

crate::outside_table_catalog!(game_mail_forward, game_mail_forward_retry);
crate::virtual_clock_exempt!(game_mail_forward_retry);

fn encode(parcel: &MailParcel) -> Vec<u8> {
    bsatn::to_vec(parcel).expect("Mail parcel encodes")
}

fn forward(ctx: &ReducerContext, database: spacetimedb::Identity, parcel: Vec<u8>) {
    ctx.db.game_mail_forward().insert(MailForward {
        id: 0,
        database: remote::Identity(database),
        parcel,
    });
}

fn later(ctx: &ReducerContext, parcel: Vec<u8>) {
    ctx.db.game_mail_forward_retry().insert(MailForwardRetry {
        id: 0,
        scheduled_at: (ctx.timestamp + TimeDuration::from_micros(1_000_000)).into(),
        parcel,
    });
}

pub(crate) fn route(ctx: &ReducerContext, row: &Mail) -> bool {
    if realm_shard(ctx).is_none() && world_shard(ctx).is_none() {
        return false;
    }
    if realm_shard(ctx).is_some()
        && crate::helpers::character_by_guid(ctx, row.recipient_guid).is_some()
        && !crate::transfer::is_in_transit(ctx, row.recipient_guid)
    {
        return false;
    }
    let text = (row.item_text_id != 0)
        .then(|| ctx.db.game_item_text().id().find(row.item_text_id))
        .flatten()
        .map(|text| text.text);
    let parcel = MailParcel {
        mail: row.clone(),
        text,
    };
    if let Some(realm) = realm_shard(ctx) {
        forward(ctx, realm, encode(&parcel));
    } else {
        dispatch(ctx, parcel);
    }
    mail_text::drop_letter_text(ctx, row.item_text_id);
    true
}

fn dispatch(ctx: &ReducerContext, mut parcel: MailParcel) {
    let recipient = parcel.mail.recipient_guid;
    if let Some(entry) = ctx.db.game_character_directory().guid().find(recipient) {
        forward(ctx, entry.world, encode(&parcel));
    } else if ctx
        .db
        .game_character_profile()
        .guid()
        .find(recipient)
        .is_some_and(|profile| profile.deleted)
    {
        if mail::plan_return(Some(&parcel.mail), recipient) == mail::ReturnPlan::Return {
            parcel.mail = mail::returned(parcel.mail, crate::time::now(ctx));
            dispatch(ctx, parcel);
        }
    } else {
        later(ctx, encode(&parcel));
    }
}

#[reducer]
pub fn idc_mail_receive(ctx: &ReducerContext, parcel: Vec<u8>) -> Result<(), String> {
    if world_shard(ctx).is_some() {
        directory::require_world(ctx)?;
        dispatch(
            ctx,
            bsatn::from_slice(&parcel).map_err(|error| error.to_string())?,
        );
        return Ok(());
    }
    require_sender(ctx, realm_shard(ctx))?;
    let mut parcel: MailParcel = bsatn::from_slice(&parcel).map_err(|error| error.to_string())?;
    let guid = parcel.mail.recipient_guid;
    if crate::helpers::character_by_guid(ctx, guid).is_none()
        || crate::transfer::is_in_transit(ctx, guid)
    {
        return Err("Mail recipient is not settled on this World Shard".into());
    }
    parcel.mail.item_text_id = match parcel.text {
        Some(text) => mail_text::import_text(ctx, text, 1)?,
        None if parcel.mail.item_text_id == 0 => 0,
        None => return Err("Mail attachment is missing its Item Text".into()),
    };
    mail::receive_letter(ctx, parcel.mail);
    Ok(())
}

#[reducer(on_result(game_mail_forward))]
pub fn mail_forward_answered(ctx: &ReducerContext, call: MailForward, result: Result<(), String>) {
    if let Err(error) = result {
        spacetimedb::log::warn!("Mail delivery through {} waits: {error}", call.database.0);
        later(ctx, call.parcel);
    }
}

#[reducer]
pub fn retry_mail_forward(ctx: &ReducerContext, retry: MailForwardRetry) -> Result<(), String> {
    if !crate::time::scheduler_authorized(ctx) {
        return Err("scheduler only".into());
    }
    if let Some(realm) = realm_shard(ctx) {
        forward(ctx, realm, retry.parcel);
    } else if world_shard(ctx).is_some() {
        dispatch(
            ctx,
            bsatn::from_slice(&retry.parcel).map_err(|error| error.to_string())?,
        );
    } else {
        later(ctx, retry.parcel);
    }
    Ok(())
}

#[reducer]
pub fn operator_route_mail(ctx: &ReducerContext) -> Result<(), String> {
    use super::game_native_player;
    use crate::mail::game_mail;
    crate::helpers::require_operator(ctx)?;
    if realm_shard(ctx).is_none() || ctx.db.game_native_player().iter().next().is_some() {
        return Err("Mail migration requires a World Shard with gameplay closed".into());
    }
    let rows = ctx.db.game_mail().iter().collect::<Vec<_>>();
    for row in rows {
        if row.item_text_id != 0
            && ctx
                .db
                .game_item_text()
                .id()
                .find(row.item_text_id)
                .is_none()
        {
            return Err(format!(
                "Mail {} is missing Item Text {}",
                row.id, row.item_text_id
            ));
        }
        mail::deliver_or_route(ctx, &row);
    }
    Ok(())
}
