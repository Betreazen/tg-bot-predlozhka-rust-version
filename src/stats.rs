//! `/stats`: monthly statistics for admins, as in `statistics_handlers.py`.
use crate::{
    bot::{ADMINS_ONLY, App, answer, send_html, user_id},
    db::MonthCounts,
    text::{Texts, fill},
    time,
};
use anyhow::{Context, Result};
use teloxide::{prelude::*, types::InlineKeyboardMarkup, types::ParseMode};

const MONTHS: [&str; 12] = [
    "Январь",
    "Февраль",
    "Март",
    "Апрель",
    "Май",
    "Июнь",
    "Июль",
    "Август",
    "Сентябрь",
    "Октябрь",
    "Ноябрь",
    "Декабрь",
];
const FAILED: &str = "❌ Ошибка получения статистики";

/// Python printed `round(x, 1)`, which shows `50.0`; a rate with a zero
/// denominator was the integer `0` and printed without a decimal.
fn percent(part: i64, whole: i64) -> String {
    if whole > 0 {
        format!("{:.1}", part as f64 / whole as f64 * 100.0)
    } else {
        "0".into()
    }
}

pub fn rates(total: i64, approved: i64, published: i64, rejected: i64) -> (String, String, String) {
    (
        percent(approved, total),
        percent(published, approved),
        percent(rejected, total),
    )
}

pub fn render(t: &Texts, c: &MonthCounts, year: i16, month: i8) -> String {
    let s = &t.statistics;
    let n = |template: &str, count: i64| fill(template, &[("count", &count.to_string())]);
    let period = format!("{} {year}", MONTHS[(month.clamp(1, 12) - 1) as usize]);
    let mut lines = vec![
        fill(&s.title, &[("period", &period)]),
        String::new(),
        n(&s.submissions_total, c.total),
        n(&s.submissions_approved, c.approved),
        n(&s.submissions_published, c.published),
        n(&s.submissions_rejected, c.rejected),
        String::new(),
        n(&s.unique_users, c.unique_users),
        n(&s.new_users, c.new_users),
        n(&s.blocked_users, c.blocked_users),
    ];
    if c.total > 0 {
        let (approval, publication, rejection) =
            rates(c.total, c.approved, c.published, c.rejected);
        lines.push(String::new());
        lines.push(fill(
            &s.rates,
            &[
                ("approval_rate", &approval),
                ("publication_rate", &publication),
                ("rejection_rate", &rejection),
            ],
        ));
    }
    if !c.admins.is_empty() {
        lines.push(String::new());
        lines.push(s.admin_performance.clone());
        for admin in &c.admins {
            let name = admin
                .username
                .clone()
                .unwrap_or_else(|| format!("ID:{}", admin.moderator_id));
            lines.push(fill(
                &s.admin_line,
                &[("username", &name), ("count", &admin.decisions.to_string())],
            ));
        }
    }
    lines.join("\n")
}

pub fn keyboard(t: &Texts, year: i16, month: i8, current: (i16, i8)) -> InlineKeyboardMarkup {
    crate::bot::keyboard(rows(t, year, month, current))
}

fn rows(t: &Texts, year: i16, month: i8, current: (i16, i8)) -> Vec<Vec<(String, String)>> {
    let nav = &t.statistics.navigation;
    let (py, pm) = if month > 1 {
        (year, month - 1)
    } else {
        (year - 1, 12)
    };
    let (ny, nm) = if month < 12 {
        (year, month + 1)
    } else {
        (year + 1, 1)
    };
    let mut first = vec![(nav.prev_month.clone(), format!("stats:{py}:{pm}"))];
    if (year, month) == current {
        return vec![first];
    }
    first.push((nav.next_month.clone(), format!("stats:{ny}:{nm}")));
    vec![
        first,
        vec![(
            nav.current_month.clone(),
            format!("stats:{}:{}", current.0, current.1),
        )],
    ]
}

async fn build(app: &App, year: i16, month: i8) -> Result<(String, InlineKeyboardMarkup)> {
    let tz = &app.config.timezone;
    let (start, end) = time::month_range(tz, year, month)?;
    let counts = app.db.month_counts(start, end).await?;
    let current = time::year_month(tz, time::now())?;
    Ok((
        render(&app.texts, &counts, year, month),
        keyboard(&app.texts, year, month, current),
    ))
}

pub async fn command(bot: &Bot, app: &App, message: &Message) -> Result<()> {
    let chat = message.chat.id.0;
    if !message
        .from
        .as_ref()
        .is_some_and(|u| app.config.is_admin(user_id(u)))
    {
        send_html(bot, chat, ADMINS_ONLY).await?;
        return Ok(());
    }
    let result = async {
        let (year, month) = time::year_month(&app.config.timezone, time::now())?;
        build(app, year, month).await
    }
    .await;
    match result {
        Ok((text, markup)) => send_html(bot, chat, text).reply_markup(markup).await?,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "statistics failed");
            send_html(bot, chat, FAILED).await?
        }
    };
    Ok(())
}

pub async fn callback(bot: &Bot, app: &App, query: &CallbackQuery, data: &str) -> Result<()> {
    if !app.config.is_admin(user_id(&query.from)) {
        return answer(bot, query, Some(ADMINS_ONLY), true).await;
    }
    let result = async {
        let mut parts = data.split(':').skip(1).map(str::parse::<i16>);
        let (Some(Ok(year)), Some(Ok(month))) = (parts.next(), parts.next()) else {
            anyhow::bail!("invalid statistics period");
        };
        let (text, markup) =
            build(app, year, i8::try_from(month).context("invalid month")?).await?;
        match query.regular_message() {
            Some(m) if m.text().is_some() => {
                bot.edit_message_text(m.chat.id, m.id, text)
                    .parse_mode(ParseMode::Html)
                    .reply_markup(markup)
                    .await?;
            }
            _ => {
                let chat = query
                    .regular_message()
                    .map_or(user_id(&query.from), |m| m.chat.id.0);
                send_html(bot, chat, text).reply_markup(markup).await?;
            }
        }
        anyhow::Ok(())
    }
    .await;
    match result {
        Ok(()) => answer(bot, query, None, false).await,
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "statistics failed");
            answer(bot, query, Some(FAILED), true).await
        }
    }
}
