//! Moderation in the admin chat: the submission card and its buttons.
use crate::{
    bot::{ADMINS_ONLY, App, answer, keyboard, notify, send_html, user_id},
    db::Decision,
    model::{Submission, User},
    text::{caption_html, fill, plain, to_html},
    time,
};
use anyhow::{Context, Result};
use teloxide::{
    prelude::*,
    types::{
        InlineKeyboardButtonKind, InlineKeyboardMarkup, MessageId, ParseMode, ReplyParameters,
    },
};

const CAPTION_LIMIT: usize = 1024;
const TEXT_LIMIT: usize = 4096;

fn fits(html: &str, limit: usize) -> bool {
    plain(html).encode_utf16().count() <= limit
}

fn card_header(app: &App, user: &User, s: &Submission) -> String {
    let t = &app.texts.admin;
    let user_info = match &user.username {
        Some(name) => format!("@{name}"),
        None => format!("ID: {}", user.user_id),
    };
    let total = user.total_submissions_count.to_string();
    fill(
        &t.submission_header,
        &[
            ("submission_number", &total),
            ("user_info", &user_info),
            ("total_submissions", &total),
            ("timestamp", &time::format_utc(s.submission_ts)),
            (
                "blocked_status",
                if user.is_blocked {
                    &t.blocked_indicator
                } else {
                    ""
                },
            ),
            ("note_section", ""),
            (
                "authorship_info",
                if s.show_authorship {
                    &t.authorship_yes
                } else {
                    &t.authorship_no
                },
            ),
        ],
    )
}

fn block_button(app: &App, user: i64, blocked: bool) -> (&str, String) {
    let b = &app.texts.admin.buttons;
    if blocked {
        (b.unblock_user.as_str(), format!("adm_unblk:{user}"))
    } else {
        (b.block_user.as_str(), format!("adm_blk:{user}"))
    }
}

fn moderation_keyboard(app: &App, s: &Submission, blocked: bool) -> InlineKeyboardMarkup {
    let b = &app.texts.admin.buttons;
    let id = &s.submission_id;
    keyboard(vec![
        vec![(b.approve_publish.as_str(), format!("adm_app_pub:{id}"))],
        vec![
            (b.approve_only.as_str(), format!("adm_app:{id}")),
            (b.reject.as_str(), format!("adm_rej:{id}")),
        ],
        vec![block_button(app, s.user_id, blocked)],
    ])
}

/// Posts the moderation card. Content that cannot carry the card as its caption
/// (albums, captions over Telegram's limits) is copied first and the card follows
/// as a reply to it.
pub async fn present(bot: &Bot, app: &App, s: &Submission) -> Result<()> {
    let user = app.db.user(s.user_id).await?.context("author not found")?;
    let header = card_header(app, &user, s);
    let tag = format!("\n\n[ID: {}]", &s.submission_id[..8]);
    let markup = moderation_keyboard(app, s, user.is_blocked);
    let admin_chat = ChatId(app.config.admin_chat_id);
    let source = s
        .user_chat_id
        .zip(s.user_message_id)
        .map(|(chat, id)| (ChatId(chat), MessageId(id as i32)));
    let body = match &s.text_content {
        Some(text) => format!("{header}\n\n{text}{tag}"),
        None => format!("{header}{tag}"),
    };
    let parts = s.album_parts();

    let reply_header = format!("{header}{tag}");
    let card = if let (Some((chat, _)), false) = (source, parts.is_empty()) {
        let ids = parts.iter().map(|p| MessageId(p.message_id));
        let copied = bot.copy_messages(admin_chat, chat, ids).await?;
        reply_card(bot, app, reply_header, markup, copied.first().copied()).await?
    } else if s.has_media {
        let (chat, id) = source.context("media submission without the original message")?;
        if fits(&body, CAPTION_LIMIT) {
            let copy = bot.copy_message(admin_chat, chat, id).caption(body);
            copy.parse_mode(ParseMode::Html)
                .reply_markup(markup)
                .await?
        } else {
            let copied = bot.copy_message(admin_chat, chat, id).await?;
            reply_card(bot, app, reply_header, markup, Some(copied)).await?
        }
    } else if let (Some((chat, id)), false) = (source, fits(&body, TEXT_LIMIT)) {
        let copied = bot.copy_message(admin_chat, chat, id).await?;
        reply_card(bot, app, reply_header, markup, Some(copied)).await?
    } else {
        send_html(bot, app.config.admin_chat_id, body)
            .reply_markup(markup)
            .await?
            .id
    };
    app.db
        .set_admin_message(&s.submission_id, card.0.into())
        .await?;
    tracing::info!(submission = %s.submission_id, "submission presented to admins");
    Ok(())
}

async fn reply_card(
    bot: &Bot,
    app: &App,
    text: String,
    markup: InlineKeyboardMarkup,
    to: Option<MessageId>,
) -> Result<MessageId> {
    let mut request = send_html(bot, app.config.admin_chat_id, text).reply_markup(markup);
    if let Some(to) = to {
        request = request.reply_parameters(ReplyParameters::new(to));
    }
    Ok(request.await?.id)
}

pub async fn callback(bot: &Bot, app: &App, query: &CallbackQuery, data: &str) -> Result<()> {
    if !app.config.is_admin(user_id(&query.from)) {
        return answer(bot, query, Some(ADMINS_ONLY), true).await;
    }
    let (action, arg) = data.split_once(':').unwrap_or((data, ""));
    let now = time::now();
    match action {
        "adm_blk" => block(bot, app, query, arg, true).await,
        "adm_unblk" => block(bot, app, query, arg, false).await,
        "adm_app_pub" | "adm_cancel_pub" | "adm_conf_pub" | "adm_app" | "adm_rej" => {
            let Some(s) = resolve(bot, app, query, arg).await? else {
                return Ok(());
            };
            let result = match action {
                "adm_app_pub" => ask_publish_confirmation(bot, app, query, &s).await,
                "adm_cancel_pub" => restore_keyboard(bot, app, query, &s).await,
                "adm_conf_pub" => {
                    let at = now + app.config.publication_delay_seconds;
                    decide(bot, app, query, &s, Decision::ApprovePublish { at }).await
                }
                "adm_app" => decide(bot, app, query, &s, Decision::ApproveOnly).await,
                _ => decide(bot, app, query, &s, Decision::Reject).await,
            };
            if let Err(error) = result {
                tracing::error!(action, error = %app.redact(&format!("{error:#}")), "moderation failed");
                let text = if action == "adm_conf_pub" {
                    "❌ Ошибка при обработке"
                } else {
                    "❌ Ошибка"
                };
                answer(bot, query, Some(text), true).await?;
            }
            Ok(())
        }
        _ => answer(bot, query, None, false).await,
    }
}

async fn resolve(
    bot: &Bot,
    app: &App,
    query: &CallbackQuery,
    arg: &str,
) -> Result<Option<Submission>> {
    let Ok(id) = uuid::Uuid::parse_str(arg) else {
        answer(bot, query, Some("❌ Некорректный идентификатор"), true).await?;
        return Ok(None);
    };
    let found = app.db.submission(&id.to_string()).await?;
    if found.is_none() {
        answer(bot, query, Some("❌ Предложка не найдена"), true).await?;
    }
    Ok(found)
}

fn card_message(query: &CallbackQuery) -> Result<&Message> {
    query
        .regular_message()
        .context("moderation card is not accessible")
}

async fn ask_publish_confirmation(
    bot: &Bot,
    app: &App,
    query: &CallbackQuery,
    s: &Submission,
) -> Result<()> {
    let b = &app.texts.admin.buttons;
    let card = card_message(query)?;
    let markup = keyboard(vec![vec![
        (
            b.confirm.as_str(),
            format!("adm_conf_pub:{}", s.submission_id),
        ),
        (
            b.cancel.as_str(),
            format!("adm_cancel_pub:{}", s.submission_id),
        ),
    ]]);
    bot.edit_message_reply_markup(card.chat.id, card.id)
        .reply_markup(markup)
        .await?;
    answer(
        bot,
        query,
        Some(&app.texts.admin.confirm_approve_publish),
        false,
    )
    .await
}

async fn restore_keyboard(
    bot: &Bot,
    app: &App,
    query: &CallbackQuery,
    s: &Submission,
) -> Result<()> {
    let user = app.db.user(s.user_id).await?.context("author not found")?;
    let card = card_message(query)?;
    bot.edit_message_reply_markup(card.chat.id, card.id)
        .reply_markup(moderation_keyboard(app, s, user.is_blocked))
        .await?;
    answer(bot, query, Some("❌ Отменено"), false).await
}

async fn decide(
    bot: &Bot,
    app: &App,
    query: &CallbackQuery,
    s: &Submission,
    decision: Decision,
) -> Result<()> {
    let moderator = user_id(&query.from);
    let Some(decided) = app
        .db
        .decide(&s.submission_id, decision, moderator, time::now())
        .await?
    else {
        return answer(bot, query, Some(&app.texts.admin.already_processed), true).await;
    };
    if matches!(decision, Decision::ApprovePublish { .. }) {
        app.wake.notify_one();
    }
    let (label, toast, notice, button) = decision_texts(app, decision);
    tracing::info!(submission = %decided.submission_id, moderator, status = %decided.status, "decision made");
    notify(bot, app, decided.user_id, notice, Some(button)).await;
    let name = query
        .from
        .username
        .clone()
        .unwrap_or_else(|| moderator.to_string());
    if let Err(error) = add_decision_footer(bot, app, query, label, &name).await {
        tracing::warn!(error = %app.redact(&format!("{error:#}")), "admin card not updated");
    }
    answer(bot, query, Some(toast), false).await
}

/// Footer label, admin toast, author notification and its button (Python texts).
fn decision_texts(
    app: &App,
    decision: Decision,
) -> (&'static str, &'static str, &str, &'static str) {
    let n = &app.texts.notifications;
    match decision {
        Decision::ApprovePublish { .. } => (
            "Принято и запланировано к публикации",
            "✅ Принято и запланировано к публикации",
            &n.approved_and_published,
            "📝 Предложить ещё контент",
        ),
        Decision::ApproveOnly => (
            "Принято без публикации",
            "✅ Принято без публикации",
            &n.approved_only,
            "📝 Предложить контент",
        ),
        Decision::Reject => (
            "Отклонено",
            "❌ Отклонено",
            &n.rejected,
            "🔄 Попробовать снова",
        ),
    }
}

/// Removes the buttons and appends the decision to the card, keeping its formatting.
async fn add_decision_footer(
    bot: &Bot,
    app: &App,
    query: &CallbackQuery,
    label: &str,
    moderator: &str,
) -> Result<()> {
    let card = card_message(query)?;
    let footer = fill(
        &app.texts.admin.decision_made,
        &[
            ("decision", label),
            ("moderator", moderator),
            ("timestamp", &time::format_utc(time::now())),
        ],
    );
    bot.edit_message_reply_markup(card.chat.id, card.id).await?;
    if card.caption().is_some() {
        let caption = caption_html(card).unwrap_or_default() + &footer;
        bot.edit_message_caption(card.chat.id, card.id)
            .caption(caption)
            .parse_mode(ParseMode::Html)
            .await?;
    } else {
        let text = to_html(card.text(), card.entities()).unwrap_or_default() + &footer;
        bot.edit_message_text(card.chat.id, card.id, text)
            .parse_mode(ParseMode::Html)
            .await?;
    }
    Ok(())
}

async fn block(
    bot: &Bot,
    app: &App,
    query: &CallbackQuery,
    arg: &str,
    blocked: bool,
) -> Result<()> {
    let target = arg.parse::<i64>().ok();
    let done = match target {
        Some(id) => app.db.set_blocked(id, blocked).await?,
        None => false,
    };
    let (Some(target), true) = (target, done) else {
        let text = if blocked {
            "❌ Ошибка блокировки"
        } else {
            "❌ Ошибка разблокировки"
        };
        return answer(bot, query, Some(text), true).await;
    };
    tracing::info!(user = target, blocked, "block status changed");
    let n = &app.texts.notifications;
    notify(
        bot,
        app,
        target,
        if blocked {
            &n.user_blocked
        } else {
            &n.user_unblocked
        },
        None,
    )
    .await;
    if let Err(error) = toggle_block_button(bot, app, query, target, blocked).await {
        tracing::warn!(error = %app.redact(&format!("{error:#}")), "block button not updated");
    }
    let toast = if blocked {
        "🚫 Пользователь заблокирован"
    } else {
        "✅ Пользователь разблокирован"
    };
    answer(bot, query, Some(toast), false).await
}

/// Swaps «Заблокировать» / «Разблокировать» on the card that was pressed.
async fn toggle_block_button(
    bot: &Bot,
    app: &App,
    query: &CallbackQuery,
    user: i64,
    blocked: bool,
) -> Result<()> {
    let card = card_message(query)?;
    let Some(markup) = card.reply_markup() else {
        return Ok(());
    };
    let targets = [format!("adm_blk:{user}"), format!("adm_unblk:{user}")];
    let mut changed = false;
    let rows = markup.inline_keyboard.iter().map(|row| {
        row.iter()
            .map(|button| match &button.kind {
                InlineKeyboardButtonKind::CallbackData(data) if targets.contains(data) => {
                    changed = true;
                    let (text, data) = block_button(app, user, blocked);
                    teloxide::types::InlineKeyboardButton::callback(text, data)
                }
                _ => button.clone(),
            })
            .collect::<Vec<_>>()
    });
    let markup = InlineKeyboardMarkup::new(rows.collect::<Vec<_>>());
    if changed {
        bot.edit_message_reply_markup(card.chat.id, card.id)
            .reply_markup(markup)
            .await?;
    }
    Ok(())
}
