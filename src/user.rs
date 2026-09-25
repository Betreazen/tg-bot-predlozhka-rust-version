//! The submission flow: `/start`, content, confirmation, authorship.
use crate::{
    admin,
    bot::{App, answer, keyboard, send_html, user_id},
    db::{NewSubmission, Profile},
    model::{AlbumPart, Draft, MediaKind, Step, Submission},
    text::{caption_html, fill, message_html, plain},
    time,
};
use anyhow::Result;
use std::sync::Arc;
use teloxide::prelude::*;

/// teloxide substitutes `u32::MAX` when Telegram omits `file_size`; Python
/// treated a missing size as 0, so an unknown size never rejects a file.
const UNKNOWN_SIZE: u32 = u32::MAX;

enum Rejected {
    Unsupported,
    TooLarge,
}

pub async fn start(bot: &Bot, app: &App, message: &Message) -> Result<()> {
    let Some(from) = &message.from else {
        return Ok(());
    };
    let uid = user_id(from);
    app.db.clear_dialogue(uid).await?;
    app.db.touch_user(&profile(from), time::now()).await?;
    let welcome = &app.texts.user.welcome;
    send_html(bot, uid, format!("{}\n\n{}", welcome.title, welcome.text))
        .reply_markup(keyboard(vec![vec![(
            welcome.button.as_str(),
            "suggest_content".to_owned(),
        )]]))
        .await?;
    tracing::info!(user = uid, "user started bot");
    Ok(())
}

pub async fn callback(bot: &Bot, app: &App, query: &CallbackQuery, data: &str) -> Result<()> {
    let uid = user_id(&query.from);
    let private = query.regular_message().is_some_and(|m| m.chat.is_private());
    if !private {
        return answer(bot, query, None, false).await;
    }
    let dialogue = app.db.dialogue(uid).await?;
    match (data, dialogue) {
        ("suggest_content", _) => return suggest(bot, app, query, uid).await,
        ("cancel_submission", Some(_)) => {
            app.db.clear_dialogue(uid).await?;
            send_html(bot, uid, &app.texts.user.submission_cancelled).await?;
        }
        ("confirm_content", Some((Step::WaitingForConfirmation, Some(draft)))) => {
            ask_authorship(bot, app, uid, &draft).await?;
        }
        ("authorship_yes" | "authorship_no", Some((Step::WaitingForAuthorship, Some(draft)))) => {
            return submit(bot, app, query, draft, data == "authorship_yes").await;
        }
        // A button from a finished or abandoned flow.
        _ => {}
    }
    answer(bot, query, None, false).await
}

async fn is_blocked(app: &App, uid: i64) -> Result<bool> {
    Ok(app.config.enable_blocking && app.db.user(uid).await?.is_some_and(|u| u.is_blocked))
}

fn limit_text(app: &App) -> String {
    plain(&fill(
        &app.texts.user.limit_exceeded,
        &[("limit", &app.config.submissions_per_day.to_string())],
    ))
}

async fn suggest(bot: &Bot, app: &App, query: &CallbackQuery, uid: i64) -> Result<()> {
    if is_blocked(app, uid).await? {
        return answer(
            bot,
            query,
            Some(&plain(&app.texts.notifications.user_blocked)),
            true,
        )
        .await;
    }
    // Advisory: the authoritative check runs when the submission is stored.
    let since = time::day_start(&app.config.timezone, time::now())?;
    if app.db.count_since(uid, since).await? >= app.config.submissions_per_day {
        return answer(bot, query, Some(&limit_text(app)), true).await;
    }
    app.db
        .set_dialogue(uid, Step::WaitingForContent, None)
        .await?;
    let t = &app.texts.user;
    send_html(bot, uid, &t.submission_prompt)
        .reply_markup(keyboard(vec![vec![(
            t.submission_cancel_button.as_str(),
            "cancel_submission".to_owned(),
        )]]))
        .await?;
    answer(bot, query, None, false).await
}

pub async fn receive(bot: &Bot, app: &Arc<App>, message: &Message) -> Result<()> {
    let Some(from) = &message.from else {
        return Ok(());
    };
    let uid = user_id(from);
    if !matches!(
        app.db.dialogue(uid).await?,
        Some((Step::WaitingForContent, _))
    ) {
        return Ok(());
    }
    if let Some(group) = message.media_group_id() {
        collect_album_part(bot, app, &group.0, message);
        return Ok(());
    }
    match classify(message, app.config.max_file_size_mb) {
        Ok(media) => {
            let text = message_html(message);
            let draft = Draft {
                chat_id: message.chat.id.0,
                message_id: message.id.0,
                text,
                media,
                album: vec![],
            };
            accept(bot, app, uid, &draft).await
        }
        Err(rejected) => reject(bot, app, uid, rejected).await,
    }
}

fn collect_album_part(bot: &Bot, app: &Arc<App>, group: &str, message: &Message) {
    let first = {
        let mut albums = app.albums.lock().unwrap_or_else(|e| e.into_inner());
        let parts = albums.entry(group.to_owned()).or_default();
        parts.push(message.clone());
        parts.len() == 1
    };
    if first {
        let (bot, app, group) = (bot.clone(), app.clone(), group.to_owned());
        tokio::spawn(async move {
            tokio::time::sleep(app.album_delay).await;
            if let Err(error) = finish_album(&bot, &app, &group).await {
                tracing::error!(error = %app.redact(&format!("{error:#}")), "album not processed");
            }
        });
    }
}

pub async fn finish_album(bot: &Bot, app: &App, group: &str) -> Result<()> {
    let Some(mut parts) = app
        .albums
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(group)
    else {
        return Ok(());
    };
    parts.sort_by_key(|m| m.id.0);
    let Some(first) = parts.first() else {
        return Ok(());
    };
    let Some(uid) = first.from.as_ref().map(user_id) else {
        return Ok(());
    };
    if !matches!(
        app.db.dialogue(uid).await?,
        Some((Step::WaitingForContent, _))
    ) {
        return Ok(());
    }
    let mut album = Vec::with_capacity(parts.len());
    for part in &parts {
        match classify(part, app.config.max_file_size_mb) {
            Ok(Some((kind, file_id))) => album.push(AlbumPart {
                message_id: part.id.0,
                kind,
                file_id,
                caption: caption_html(part),
            }),
            Ok(None) => return reject(bot, app, uid, Rejected::Unsupported).await,
            Err(rejected) => return reject(bot, app, uid, rejected).await,
        }
    }
    let text = album.iter().find_map(|p| p.caption.clone());
    let draft = Draft {
        chat_id: first.chat.id.0,
        message_id: first.id.0,
        text,
        media: None,
        album,
    };
    accept(bot, app, uid, &draft).await
}

/// Text (`None`) or the single media file of a message, as the Python bot
/// accepted them: text, photo, video, document (GIFs included) and audio.
fn classify(message: &Message, max_mb: u64) -> Result<Option<(MediaKind, String)>, Rejected> {
    let (kind, file) = if let Some(sizes) = message.photo() {
        let largest = sizes.last().ok_or(Rejected::Unsupported)?;
        (MediaKind::Photo, &largest.file)
    } else if let Some(video) = message.video() {
        (MediaKind::Video, &video.file)
    } else if let Some(document) = message.document() {
        (MediaKind::Document, &document.file)
    } else if let Some(animation) = message.animation() {
        (MediaKind::Animation, &animation.file)
    } else if let Some(audio) = message.audio() {
        (MediaKind::Audio, &audio.file)
    } else if message.text().is_some() {
        return Ok(None);
    } else {
        return Err(Rejected::Unsupported);
    };
    if file.size != UNKNOWN_SIZE && u64::from(file.size) > max_mb * 1024 * 1024 {
        return Err(Rejected::TooLarge);
    }
    Ok(Some((kind, file.id.0.clone())))
}

async fn reject(bot: &Bot, app: &App, uid: i64, rejected: Rejected) -> Result<()> {
    let t = &app.texts.user;
    let text = match rejected {
        Rejected::Unsupported => t.unsupported_media.clone(),
        Rejected::TooLarge => fill(
            &t.file_too_large,
            &[("max_size", &app.config.max_file_size_mb.to_string())],
        ),
    };
    send_html(bot, uid, text).await?;
    Ok(())
}

async fn accept(bot: &Bot, app: &App, uid: i64, draft: &Draft) -> Result<()> {
    if !app.config.require_confirmation {
        return ask_authorship(bot, app, uid, draft).await;
    }
    app.db
        .set_dialogue(uid, Step::WaitingForConfirmation, Some(draft))
        .await?;
    let t = &app.texts.user;
    send_html(bot, uid, &t.submission_received)
        .reply_markup(keyboard(vec![vec![
            (t.confirm_button.as_str(), "confirm_content".to_owned()),
            (t.cancel_button.as_str(), "cancel_submission".to_owned()),
        ]]))
        .await?;
    Ok(())
}

async fn ask_authorship(bot: &Bot, app: &App, uid: i64, draft: &Draft) -> Result<()> {
    app.db
        .set_dialogue(uid, Step::WaitingForAuthorship, Some(draft))
        .await?;
    let t = &app.texts.user;
    send_html(bot, uid, &t.authorship_question)
        .reply_markup(keyboard(vec![
            vec![(t.authorship_yes.as_str(), "authorship_yes".to_owned())],
            vec![(t.authorship_no.as_str(), "authorship_no".to_owned())],
        ]))
        .await?;
    Ok(())
}

async fn submit(
    bot: &Bot,
    app: &App,
    query: &CallbackQuery,
    draft: Draft,
    show_authorship: bool,
) -> Result<()> {
    let uid = user_id(&query.from);
    // The flow may have started before the user was blocked.
    if is_blocked(app, uid).await? {
        app.db.clear_dialogue(uid).await?;
        return answer(
            bot,
            query,
            Some(&plain(&app.texts.notifications.user_blocked)),
            true,
        )
        .await;
    }
    let now = time::now();
    app.db.touch_user(&profile(&query.from), now).await?;
    let new = new_submission(uid, draft, show_authorship)?;
    let since = time::day_start(&app.config.timezone, now)?;
    let Some(submission) = app
        .db
        .create_submission(&new, now, since, app.config.submissions_per_day)
        .await?
    else {
        app.db.clear_dialogue(uid).await?;
        return answer(bot, query, Some(&limit_text(app)), true).await;
    };
    tracing::info!(user = uid, submission = %submission.submission_id, "submission created");
    report_outcome(bot, app, uid, &submission).await?;
    answer(bot, query, None, false).await
}

fn new_submission(uid: i64, draft: Draft, show_authorship: bool) -> Result<NewSubmission> {
    Ok(NewSubmission {
        user_id: uid,
        user_chat_id: draft.chat_id,
        user_message_id: draft.message_id.into(),
        show_authorship,
        text_content: draft.text,
        media: draft.media,
        album: (!draft.album.is_empty())
            .then(|| serde_json::to_string(&draft.album))
            .transpose()?,
    })
}

/// Sends the card to the admins and tells the author whether it got there.
async fn report_outcome(bot: &Bot, app: &App, uid: i64, submission: &Submission) -> Result<()> {
    let t = &app.texts.user;
    let text = match admin::present(bot, app, submission).await {
        Ok(()) => &t.submission_accepted,
        Err(error) => {
            // Stays pending without a card; it is presented again at the next start.
            let error = app.redact(&format!("{error:#}"));
            tracing::error!(submission = %submission.submission_id, %error, "moderation card not delivered");
            &t.error_occurred
        }
    };
    send_html(bot, uid, text).await?;
    Ok(())
}

fn profile(user: &teloxide::types::User) -> Profile<'_> {
    Profile {
        user_id: user_id(user),
        username: user.username.as_deref(),
        first_name: Some(user.first_name.as_str()),
        last_name: user.last_name.as_deref(),
    }
}
