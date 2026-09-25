//! Delayed publication to the channel, retries and startup recovery.
use crate::{
    admin,
    bot::{App, send_html},
    db::RetryOutcome,
    model::{AlbumPart, MediaKind, Submission},
    text::fill,
    time,
};
use anyhow::{Result, bail};
use std::{sync::Arc, time::Duration};
use teloxide::{
    prelude::*,
    types::{
        InputFile, InputMedia, InputMediaAudio, InputMediaDocument, InputMediaPhoto,
        InputMediaVideo, MessageId, ParseMode,
    },
    utils::html::escape,
};

const DB_ERROR_PAUSE: Duration = Duration::from_secs(30);

/// Sleeps until the next scheduled publication or until woken by a new decision;
/// with nothing scheduled it waits without timers, so an idle bot uses no CPU.
pub async fn run(bot: Bot, app: Arc<App>) {
    loop {
        let next = match publish_due(&bot, &app).await {
            Ok(()) => app.db.next_scheduled().await,
            Err(error) => Err(error),
        };
        match next {
            Ok(None) => app.wake.notified().await,
            Ok(Some(at)) => {
                let wait = Duration::from_secs((at - time::now()).max(0) as u64);
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = app.wake.notified() => {}
                }
            }
            Err(error) => {
                tracing::error!(error = %app.redact(&format!("{error:#}")), "publication worker failed");
                tokio::time::sleep(DB_ERROR_PAUSE).await;
            }
        }
    }
}

pub async fn publish_due(bot: &Bot, app: &App) -> Result<()> {
    loop {
        let _sending = app.sending.lock().await;
        let Some(s) = app.db.claim_due(time::now()).await? else {
            return Ok(());
        };
        match send(bot, app, &s).await {
            Ok(message) => {
                app.db
                    .mark_published(&s.submission_id, message.0.into(), time::now())
                    .await?;
                tracing::info!(submission = %s.submission_id, "published to channel");
            }
            Err(error) => {
                let error = app.redact(&format!("{error:#}"));
                let c = &app.config;
                let outcome = app
                    .db
                    .publication_failed(
                        &s.submission_id,
                        &error,
                        time::now(),
                        c.max_retry_attempts,
                        c.retry_delay_seconds,
                    )
                    .await?;
                tracing::error!(submission = %s.submission_id, ?outcome, %error, "publication failed");
                if let RetryOutcome::Failed { attempt } = outcome {
                    report_failure(bot, app, &s, &error, attempt).await;
                }
            }
        }
    }
}

/// Author line, footer and hashtags, appended to the submission text as in Python.
async fn suffix(app: &App, s: &Submission) -> Result<String> {
    let mut out = String::new();
    if s.show_authorship
        && let Some(user) = app.db.user(s.user_id).await?
    {
        match (&user.username, user.first_name.as_deref()) {
            (Some(name), _) => out.push_str(&format!("\n\nАвтор: @{name}")),
            (None, Some(first)) if !first.is_empty() => {
                out.push_str(&format!("\n\nАвтор: {}", escape(first)))
            }
            (None, _) => out.push_str("\n\nАвтор: Аноним"),
        }
    }
    for extra in [&app.config.footer_text, &app.config.hashtags] {
        if !extra.is_empty() {
            out.push_str("\n\n");
            out.push_str(extra);
        }
    }
    Ok(out)
}

fn joined(text: Option<&str>, suffix: &str) -> Option<String> {
    let all = format!("{}{suffix}", text.unwrap_or_default());
    (!all.is_empty()).then_some(all)
}

async fn send(bot: &Bot, app: &App, s: &Submission) -> Result<MessageId> {
    let channel = ChatId(app.config.channel_id);
    let suffix = suffix(app, s).await?;
    let parts = s.album_parts();
    if !parts.is_empty() {
        // The extra lines go under the album's caption, or the first item without one.
        let target = parts.iter().position(|p| p.caption.is_some()).unwrap_or(0);
        let media = parts.iter().enumerate().map(|(i, p)| {
            let caption = if i == target {
                joined(p.caption.as_deref(), &suffix)
            } else {
                p.caption.clone()
            };
            input_media(p, caption)
        });
        let sent = bot.send_media_group(channel, media).await?;
        return match sent.first() {
            Some(first) => Ok(first.id),
            None => bail!("sendMediaGroup returned no messages"),
        };
    }
    let text = joined(s.text_content.as_deref(), &suffix);
    match (s.has_media, s.user_chat_id, s.user_message_id) {
        (true, Some(chat), Some(id)) => {
            let mut copy = bot.copy_message(channel, ChatId(chat), MessageId(id as i32));
            if let Some(caption) = text {
                copy = copy.caption(caption).parse_mode(ParseMode::Html);
            }
            Ok(copy.await?)
        }
        // A text message has no caption to replace, so it is sent anew.
        (false, _, _) if text.is_some() => {
            Ok(
                send_html(bot, app.config.channel_id, text.unwrap_or_default())
                    .await?
                    .id,
            )
        }
        _ => bail!("submission has nothing to publish"),
    }
}

fn input_media(part: &AlbumPart, caption: Option<String>) -> InputMedia {
    let file = InputFile::file_id(part.file_id.clone().into());
    macro_rules! with_caption {
        ($media:expr) => {{
            let media = $media.parse_mode(ParseMode::Html);
            match caption {
                Some(c) => media.caption(c),
                None => media,
            }
        }};
    }
    match part.kind {
        MediaKind::Photo => InputMedia::Photo(with_caption!(InputMediaPhoto::new(file))),
        MediaKind::Video => InputMedia::Video(with_caption!(InputMediaVideo::new(file))),
        MediaKind::Audio => InputMedia::Audio(with_caption!(InputMediaAudio::new(file))),
        // Telegram albums hold no animations; treat one as a document like Python did.
        MediaKind::Document | MediaKind::Animation => {
            InputMedia::Document(with_caption!(InputMediaDocument::new(file)))
        }
    }
}

async fn report_failure(bot: &Bot, app: &App, s: &Submission, error: &str, attempt: i64) {
    let username = match app
        .db
        .user(s.user_id)
        .await
        .ok()
        .flatten()
        .and_then(|u| u.username)
    {
        Some(name) => format!("@{name}"),
        None => format!("ID:{}", s.user_id),
    };
    let text = fill(
        &app.texts.admin.publication_error,
        &[
            ("submission_id", &s.submission_id),
            ("username", &username),
            ("error", &escape(error)),
            ("attempt", &attempt.to_string()),
            ("max_attempts", &app.config.max_retry_attempts.to_string()),
        ],
    );
    send_best_effort(bot, app, app.config.error_chat_id, text).await;
}

async fn send_best_effort(bot: &Bot, app: &App, chat: i64, text: String) {
    if let Err(error) = send_html(bot, chat, text).await {
        tracing::error!(chat, error = %app.redact(&error.to_string()), "service message not delivered");
    }
}

/// Runs once before polling starts.
pub async fn startup(bot: &Bot, app: &App) -> Result<()> {
    let recovery = app
        .db
        .recover(time::now(), app.config.publication_delay_seconds)
        .await?;
    for s in &recovery.interrupted {
        tracing::warn!(submission = %s.submission_id, "publication interrupted by restart; not resent");
        let text = format!(
            "⚠️ <b>Публикация прервана</b>\n\n📋 Предложка ID: {}\n\nБот перезапустился во время отправки в канал. \
             Проверьте канал: если поста нет, опубликуйте его вручную.",
            s.submission_id
        );
        send_best_effort(bot, app, app.config.error_chat_id, text).await;
    }
    for s in &recovery.undelivered {
        if let Err(error) = admin::present(bot, app, s).await {
            tracing::error!(submission = %s.submission_id, error = %app.redact(&format!("{error:#}")), "card not delivered");
        }
    }
    if recovery.pending > 10 {
        let text = format!("⚠️ {} предложений ожидают модерации", recovery.pending);
        send_best_effort(bot, app, app.config.admin_chat_id, text).await;
    }
    send_best_effort(
        bot,
        app,
        app.config.admin_chat_id,
        app.texts.admin.bot_started.clone(),
    )
    .await;
    app.wake.notify_one();
    Ok(())
}
