//! One-time import of the Python bot's PostgreSQL data, exported per table with
//! `psql -Atc "select coalesce(json_agg(t), '[]') from <table> t" > <table>.json`.
use crate::db::Database;
use anyhow::{Context, Result, ensure};
use jiff::{civil::DateTime, tz::TimeZone};
use serde::{Deserialize, de::DeserializeOwned};
use std::{fmt, path::Path};
use teloxide::utils::html::escape;

// Columns of Alembic 001_initial_schema. Unknown columns abort the import rather
// than being dropped silently.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PgUser {
    user_id: i64,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    is_blocked: bool,
    admin_note: Option<String>,
    total_submissions_count: i64,
    registration_timestamp: String,
    last_interaction_timestamp: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PgSubmission {
    submission_id: String,
    user_id: i64,
    submission_timestamp: String,
    status: String,
    moderator_id: Option<i64>,
    decision_timestamp: Option<String>,
    show_authorship: bool,
    message_id_in_admin_chat: Option<i64>,
    message_id_in_channel: Option<i64>,
    user_message_id: Option<i64>,
    user_chat_id: Option<i64>,
    has_media: bool,
    media_type: Option<String>,
    media_file_id: Option<String>,
    text_content: Option<String>,
    scheduled_publication_time: Option<String>,
    publication_error_message: Option<String>,
    publication_retry_count: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PgActionLog {
    log_id: i64,
    action_type: String,
    admin_user_id: i64,
    target_user_id: Option<i64>,
    submission_id: Option<String>,
    action_timestamp: String,
    additional_context: Option<String>,
}

/// Aggregates of the imported database, compared with the same numbers taken from
/// PostgreSQL during the switch-over.
pub struct Summary {
    pub users: i64,
    pub blocked: i64,
    pub notes: i64,
    pub total_submissions_count: i64,
    pub submissions: i64,
    pub admin_action_logs: i64,
    /// `status:count`, sorted by status.
    pub statuses: Vec<(String, i64)>,
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let statuses: Vec<String> = self
            .statuses
            .iter()
            .map(|(s, n)| format!("{s}:{n}"))
            .collect();
        write!(
            f,
            "users={} blocked={} notes={} total_submissions_count={} submissions={} admin_action_logs={} statuses={}",
            self.users,
            self.blocked,
            self.notes,
            self.total_submissions_count,
            self.submissions,
            self.admin_action_logs,
            statuses.join(",")
        )
    }
}

fn read<T: DeserializeOwned>(dir: &Path, table: &str) -> Result<Vec<T>> {
    let path = dir.join(format!("{table}.json"));
    let json =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(json.trim()).with_context(|| format!("invalid {table}.json"))
}

/// PostgreSQL `timestamp without time zone` holding naive UTC, as Python stored it.
fn seconds(value: &str) -> Result<i64> {
    let civil: DateTime = value
        .parse()
        .with_context(|| format!("invalid timestamp {value:?}"))?;
    Ok(civil.to_zoned(TimeZone::UTC)?.timestamp().as_second())
}

fn seconds_opt(value: Option<&str>) -> Result<Option<i64>> {
    value.map(seconds).transpose()
}

pub async fn run(db: &Database, dir: &Path) -> Result<Summary> {
    let users: Vec<PgUser> = read(dir, "users")?;
    let submissions: Vec<PgSubmission> = read(dir, "submissions")?;
    let logs: Vec<PgActionLog> = read(dir, "admin_action_logs")?;

    let mut tx = db.pool().begin_with("BEGIN IMMEDIATE").await?;
    let existing: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM users) + (SELECT count(*) FROM submissions)
              + (SELECT count(*) FROM admin_action_logs)",
    )
    .fetch_one(&mut *tx)
    .await?;
    ensure!(
        existing == 0,
        "the database is not empty; import only into a fresh DATA_DIR"
    );

    for u in &users {
        sqlx::query("INSERT INTO users VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(u.user_id)
            .bind(&u.username)
            .bind(&u.first_name)
            .bind(&u.last_name)
            .bind(u.is_blocked)
            .bind(&u.admin_note)
            .bind(u.total_submissions_count)
            .bind(seconds(&u.registration_timestamp)?)
            .bind(seconds(&u.last_interaction_timestamp)?)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("user {}", u.user_id))?;
    }
    for s in &submissions {
        sqlx::query(
            "INSERT INTO submissions (submission_id, user_id, submission_ts, status, moderator_id, decision_ts,
                show_authorship, message_id_in_admin_chat, message_id_in_channel, user_message_id, user_chat_id,
                has_media, media_type, media_file_id, text_content, scheduled_ts, publication_error_message,
                publication_retry_count)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::parse_str(&s.submission_id).context("invalid submission_id")?.to_string())
        .bind(s.user_id)
        .bind(seconds(&s.submission_timestamp)?)
        .bind(&s.status)
        .bind(s.moderator_id)
        .bind(seconds_opt(s.decision_timestamp.as_deref())?)
        .bind(s.show_authorship)
        .bind(s.message_id_in_admin_chat)
        .bind(s.message_id_in_channel)
        .bind(s.user_message_id)
        .bind(s.user_chat_id)
        .bind(s.has_media)
        .bind(&s.media_type)
        .bind(&s.media_file_id)
        // Python kept the plain text; this bot stores HTML.
        .bind(s.text_content.as_deref().map(escape))
        .bind(seconds_opt(s.scheduled_publication_time.as_deref())?)
        .bind(&s.publication_error_message)
        .bind(s.publication_retry_count)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("submission {}", s.submission_id))?;
    }
    for l in &logs {
        sqlx::query("INSERT INTO admin_action_logs VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(l.log_id)
            .bind(&l.action_type)
            .bind(l.admin_user_id)
            .bind(l.target_user_id)
            .bind(l.submission_id.as_deref().map(str::to_lowercase))
            .bind(seconds(&l.action_timestamp)?)
            .bind(&l.additional_context)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("admin action log {}", l.log_id))?;
    }
    tx.commit().await?;
    summary(db).await
}

pub async fn summary(db: &Database) -> Result<Summary> {
    let pool = db.pool();
    let (users, blocked, notes, total_submissions_count): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE is_blocked), count(admin_note),
                coalesce(sum(total_submissions_count), 0)
         FROM users",
    )
    .fetch_one(pool)
    .await?;
    let submissions = sqlx::query_scalar("SELECT count(*) FROM submissions")
        .fetch_one(pool)
        .await?;
    let admin_action_logs = sqlx::query_scalar("SELECT count(*) FROM admin_action_logs")
        .fetch_one(pool)
        .await?;
    let statuses =
        sqlx::query_as("SELECT status, count(*) FROM submissions GROUP BY status ORDER BY status")
            .fetch_all(pool)
            .await?;
    Ok(Summary {
        users,
        blocked,
        notes,
        total_submissions_count,
        submissions,
        admin_action_logs,
        statuses,
    })
}
