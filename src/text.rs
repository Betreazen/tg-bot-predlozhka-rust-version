//! Texts come from the Python bot's `messages.json`, unchanged. Only the keys the
//! bot uses are declared, so a missing one fails at startup instead of mid-flow.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
pub struct Texts {
    pub user: UserTexts,
    pub notifications: Notifications,
    pub admin: AdminTexts,
    pub statistics: StatisticsTexts,
}

#[derive(Deserialize)]
pub struct Welcome {
    pub title: String,
    pub text: String,
    pub button: String,
}

#[derive(Deserialize)]
pub struct UserTexts {
    pub welcome: Welcome,
    pub submission_prompt: String,
    pub submission_cancel_button: String,
    pub submission_received: String,
    pub confirm_button: String,
    pub cancel_button: String,
    pub authorship_question: String,
    pub authorship_yes: String,
    pub authorship_no: String,
    pub submission_accepted: String,
    pub submission_cancelled: String,
    pub limit_exceeded: String,
    pub file_too_large: String,
    pub unsupported_media: String,
    pub error_occurred: String,
}

#[derive(Deserialize)]
pub struct Notifications {
    pub approved_and_published: String,
    pub approved_only: String,
    pub rejected: String,
    pub user_blocked: String,
    pub user_unblocked: String,
}

#[derive(Deserialize)]
pub struct AdminButtons {
    pub approve_publish: String,
    pub approve_only: String,
    pub reject: String,
    pub block_user: String,
    pub unblock_user: String,
    pub confirm: String,
    pub cancel: String,
}

#[derive(Deserialize)]
pub struct AdminTexts {
    pub submission_header: String,
    pub blocked_indicator: String,
    pub authorship_yes: String,
    pub authorship_no: String,
    pub buttons: AdminButtons,
    pub confirm_approve_publish: String,
    pub decision_made: String,
    pub already_processed: String,
    pub publication_error: String,
    pub bot_started: String,
}

#[derive(Deserialize)]
pub struct Navigation {
    pub prev_month: String,
    pub next_month: String,
    pub current_month: String,
}

#[derive(Deserialize)]
pub struct StatisticsTexts {
    pub title: String,
    pub submissions_total: String,
    pub submissions_approved: String,
    pub submissions_published: String,
    pub submissions_rejected: String,
    pub unique_users: String,
    pub new_users: String,
    pub blocked_users: String,
    pub admin_performance: String,
    pub admin_line: String,
    pub rates: String,
    pub navigation: Navigation,
}

impl Texts {
    pub fn load(path: &Path) -> Result<Self> {
        let json =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        Self::parse(&json)
    }

    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).context("invalid messages.json")
    }
}

/// Python `str.format` with named fields, done in one pass so that a substituted
/// value containing `{name}` is never expanded again. Unknown fields stay as-is.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let found = after.find('}').and_then(|end| {
            let name = &after[..end];
            values
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (end, *v))
        });
        match found {
            Some((end, value)) => {
                out.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Callback alerts are shown as plain text; Python sent the HTML tags through
/// verbatim (`<b>Лимит превышен</b>`). Strip them for alerts only.
pub fn plain(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}
