use crate::{admin, config::Config, db::Database, stats, text::Texts, user};
use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use teloxide::{
    payloads::SendMessageSetters,
    prelude::*,
    requests::JsonRequest,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode},
};
use tokio::sync::Notify;

pub const ADMINS_ONLY: &str = "⛔️ Только для администраторов";
const HANDLER_FAILED: &str = "❌ Произошла ошибка. Попробуйте позже.";
/// Album parts arrive as separate updates, normally in one batch.
const ALBUM_DELAY: Duration = Duration::from_millis(1500);

pub struct App {
    pub config: Config,
    pub db: Database,
    pub texts: Texts,
    /// Wakes the publication worker after a new scheduling decision.
    pub wake: Notify,
    pub album_delay: Duration,
    /// From getMe; commands addressed to other bots (`/stats@other`) are ignored.
    pub username: Option<String>,
    pub(crate) albums: Mutex<HashMap<String, Vec<Message>>>,
    /// Held from claiming a publication until its outcome is stored. Shutdown
    /// takes it so that a send in progress completes instead of being aborted.
    pub sending: tokio::sync::Mutex<()>,
}

impl App {
    pub fn new(config: Config, db: Database, texts: Texts) -> Self {
        Self {
            config,
            db,
            texts,
            wake: Notify::new(),
            album_delay: ALBUM_DELAY,
            username: None,
            albums: Mutex::default(),
            sending: tokio::sync::Mutex::new(()),
        }
    }

    /// reqwest errors include the request URL, and with it the bot token.
    pub fn redact(&self, text: &str) -> String {
        text.replace(&self.config.token, "[REDACTED]")
    }

    pub async fn finish_album(self: &Arc<Self>, bot: &Bot, group: &str) -> Result<()> {
        user::finish_album(bot, self, group).await
    }

    /// Processes albums still waiting for their collection delay (used on shutdown).
    pub async fn flush_albums(self: &Arc<Self>, bot: &Bot) {
        let groups: Vec<String> = self
            .albums
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        for group in groups {
            if let Err(error) = self.finish_album(bot, &group).await {
                tracing::error!(error = %self.redact(&format!("{error:#}")), "album not processed");
            }
        }
    }
}

pub async fn handle_message(bot: Bot, message: Message, app: Arc<App>) -> Result<()> {
    if message.from.is_none() {
        return Ok(());
    }
    // Known commands come before the dialogue state, so `/stats` is never taken
    // as content. Other text starting with `/` is ordinary text, as in aiogram.
    match command(&message, app.username.as_deref()) {
        Some("start") if message.chat.is_private() => {
            return user::start(&bot, &app, &message).await;
        }
        Some("start") => return Ok(()),
        Some("stats") => return stats::command(&bot, &app, &message).await,
        _ => {}
    }
    if !message.chat.is_private() {
        return Ok(());
    }
    user::receive(&bot, &app, &message).await
}

pub async fn handle_callback(bot: Bot, query: CallbackQuery, app: Arc<App>) -> Result<()> {
    let result = route_callback(&bot, &app, &query).await;
    if result.is_err() {
        // Stop the spinner; the error itself is logged by the dispatcher.
        let _ = answer(&bot, &query, Some(HANDLER_FAILED), true).await;
    }
    result
}

async fn route_callback(bot: &Bot, app: &Arc<App>, query: &CallbackQuery) -> Result<()> {
    let data = query.data.as_deref().unwrap_or_default();
    match data {
        "suggest_content" | "cancel_submission" | "confirm_content" | "authorship_yes"
        | "authorship_no" => user::callback(bot, app, query, data).await,
        _ if data.starts_with("adm_") => admin::callback(bot, app, query, data).await,
        _ if data.starts_with("stats:") => stats::callback(bot, app, query, data).await,
        // Buttons of old messages: stop the spinner and do nothing.
        _ => answer(bot, query, None, false).await,
    }
}

/// The command name if `message` is a bot command meant for this bot.
fn command<'a>(message: &'a Message, username: Option<&str>) -> Option<&'a str> {
    let word = message
        .text()?
        .split_whitespace()
        .next()?
        .strip_prefix('/')?;
    let (name, mention) = word
        .split_once('@')
        .map_or((word, None), |(n, m)| (n, Some(m)));
    match (mention, username) {
        (Some(m), Some(me)) if !m.eq_ignore_ascii_case(me) => None,
        _ => Some(name),
    }
}

pub fn user_id(user: &teloxide::types::User) -> i64 {
    user.id.0 as i64
}

pub async fn answer(
    bot: &Bot,
    query: &CallbackQuery,
    text: Option<&str>,
    alert: bool,
) -> Result<()> {
    let mut request = bot.answer_callback_query(query.id.clone());
    if let Some(text) = text {
        request = request.text(text).show_alert(alert);
    }
    request.await?;
    Ok(())
}

pub fn send_html(
    bot: &Bot,
    chat: i64,
    text: impl Into<String>,
) -> JsonRequest<teloxide::payloads::SendMessage> {
    bot.send_message(ChatId(chat), text)
        .parse_mode(ParseMode::Html)
}

pub fn keyboard<S: Into<String>>(rows: Vec<Vec<(S, String)>>) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(rows.into_iter().map(|row| {
        row.into_iter()
            .map(|(text, data)| InlineKeyboardButton::callback(text, data))
            .collect::<Vec<_>>()
    }))
}

/// Sends a best-effort message; failures (user blocked the bot, chat gone) are logged.
pub async fn notify(bot: &Bot, app: &App, chat: i64, text: &str, button: Option<&str>) {
    let mut request = send_html(bot, chat, text);
    if let Some(button) = button {
        request =
            request.reply_markup(keyboard(vec![vec![(button, "suggest_content".to_owned())]]));
    }
    if let Err(error) = request.await {
        tracing::warn!(chat, error = %app.redact(&error.to_string()), "message not delivered");
    }
}
