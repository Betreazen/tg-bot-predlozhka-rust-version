use crate::model::{Draft, MediaKind, Step, Submission, User, status};
use anyhow::{Context, Result, ensure};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, time::Duration};

const SCHEMA_VERSION: i64 = 1;

pub struct Profile<'a> {
    pub user_id: i64,
    pub username: Option<&'a str>,
    pub first_name: Option<&'a str>,
    pub last_name: Option<&'a str>,
}

pub struct NewSubmission {
    pub user_id: i64,
    pub user_chat_id: i64,
    pub user_message_id: i64,
    pub show_authorship: bool,
    pub text_content: Option<String>,
    pub media: Option<(MediaKind, String)>,
    /// JSON array of album parts.
    pub album: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    ApprovePublish { at: i64 },
    ApproveOnly,
    Reject,
}

impl Decision {
    fn status(self) -> &'static str {
        match self {
            Self::ApprovePublish { .. } => status::SCHEDULED,
            Self::ApproveOnly => status::ACCEPTED_NOT_PUBLISHED,
            Self::Reject => status::REJECTED,
        }
    }
    fn action(self) -> &'static str {
        match self {
            Self::ApprovePublish { .. } => "approve_publish",
            Self::ApproveOnly => "approve_only",
            Self::Reject => "reject",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryOutcome {
    Retry { attempt: i64 },
    Failed { attempt: i64 },
}

pub struct Recovery {
    /// Were being sent when the process stopped; now `publication_failed`.
    pub interrupted: Vec<Submission>,
    /// `pending` whose moderation card never reached the admin chat.
    pub undelivered: Vec<Submission>,
    pub pending: i64,
}

#[derive(Debug, sqlx::FromRow)]
pub struct AdminDecisions {
    pub moderator_id: i64,
    pub username: Option<String>,
    pub decisions: i64,
}

#[derive(Debug, Default)]
pub struct MonthCounts {
    pub total: i64,
    pub approved: i64,
    pub published: i64,
    pub rejected: i64,
    pub unique_users: i64,
    pub new_users: i64,
    pub blocked_users: i64,
    pub admins: Vec<AdminDecisions>,
}

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        let options = SqliteConnectOptions::new()
            .filename(dir.join("bot.db"))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        // One connection serialises every write, which is what the rate limit and
        // the one-shot decision/publication claims rely on.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&pool)
            .await?;
        ensure!(
            version <= SCHEMA_VERSION,
            "database schema is newer than this application"
        );
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(include_str!("../migrations/0001_schema.sql"))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn check(&self) -> Result<()> {
        let check: String = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_one(&self.pool)
            .await?;
        ensure!(check == "ok", "SQLite integrity check failed");
        Ok(())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub async fn touch_user(&self, p: &Profile<'_>, now: i64) -> Result<User> {
        Ok(sqlx::query_as(
            "INSERT INTO users (user_id, username, first_name, last_name, registration_ts, last_interaction_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(user_id) DO UPDATE SET
                username = coalesce(excluded.username, username),
                first_name = coalesce(excluded.first_name, first_name),
                last_name = coalesce(excluded.last_name, last_name),
                last_interaction_ts = excluded.last_interaction_ts
             RETURNING *",
        )
        .bind(p.user_id)
        .bind(p.username)
        .bind(p.first_name)
        .bind(p.last_name)
        .bind(now)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn user(&self, id: i64) -> Result<Option<User>> {
        Ok(sqlx::query_as("SELECT * FROM users WHERE user_id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn set_blocked(&self, id: i64, blocked: bool) -> Result<bool> {
        let done = sqlx::query("UPDATE users SET is_blocked = ? WHERE user_id = ?")
            .bind(blocked)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn dialogue(&self, user: i64) -> Result<Option<(Step, Option<Draft>)>> {
        let row: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT state, draft FROM dialogues WHERE user_id = ?")
                .bind(user)
                .fetch_optional(&self.pool)
                .await?;
        row.map(|(state, draft)| {
            let step = serde_json::from_str(&state).context("invalid stored dialogue state")?;
            let draft = draft
                .map(|d| serde_json::from_str(&d))
                .transpose()
                .context("invalid stored draft")?;
            Ok((step, draft))
        })
        .transpose()
    }

    pub async fn set_dialogue(&self, user: i64, step: Step, draft: Option<&Draft>) -> Result<()> {
        sqlx::query(
            "INSERT INTO dialogues (user_id, state, draft) VALUES (?, ?, ?)
             ON CONFLICT(user_id) DO UPDATE SET state = excluded.state, draft = excluded.draft",
        )
        .bind(user)
        .bind(serde_json::to_string(&step)?)
        .bind(draft.map(serde_json::to_string).transpose()?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn clear_dialogue(&self, user: i64) -> Result<()> {
        sqlx::query("DELETE FROM dialogues WHERE user_id = ?")
            .bind(user)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn count_since(&self, user: i64, since: i64) -> Result<i64> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM submissions WHERE user_id = ? AND submission_ts >= ?",
        )
        .bind(user)
        .bind(since)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Checks the daily limit and stores the submission in one write transaction,
    /// so two quick confirmations cannot both pass. `None` means over the limit.
    pub async fn create_submission(
        &self,
        new: &NewSubmission,
        now: i64,
        day_start: i64,
        limit: i64,
    ) -> Result<Option<Submission>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let today: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM submissions WHERE user_id = ? AND submission_ts >= ?",
        )
        .bind(new.user_id)
        .bind(day_start)
        .fetch_one(&mut *tx)
        .await?;
        if today >= limit {
            return Ok(None);
        }
        let submission: Submission = sqlx::query_as(
            "INSERT INTO submissions (submission_id, user_id, submission_ts, status, show_authorship,
                user_message_id, user_chat_id, has_media, media_type, media_file_id, text_content, album)
             VALUES (?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?) RETURNING *",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(new.user_id)
        .bind(now)
        .bind(new.show_authorship)
        .bind(new.user_message_id)
        .bind(new.user_chat_id)
        .bind(new.media.is_some() || new.album.is_some())
        .bind(new.media.as_ref().map(|(kind, _)| kind.as_str()))
        .bind(new.media.as_ref().map(|(_, file)| file.as_str()))
        .bind(&new.text_content)
        .bind(&new.album)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("UPDATE users SET total_submissions_count = total_submissions_count + 1 WHERE user_id = ?")
            .bind(new.user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM dialogues WHERE user_id = ?")
            .bind(new.user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(submission))
    }

    pub async fn submission(&self, id: &str) -> Result<Option<Submission>> {
        Ok(
            sqlx::query_as("SELECT * FROM submissions WHERE submission_id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn set_admin_message(&self, id: &str, message_id: i64) -> Result<()> {
        sqlx::query("UPDATE submissions SET message_id_in_admin_chat = ? WHERE submission_id = ?")
            .bind(message_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Applies a moderation decision only if the submission is still pending.
    /// `None` means someone already decided (the Python bot's `AlreadyDecidedError`).
    pub async fn decide(
        &self,
        id: &str,
        decision: Decision,
        moderator: i64,
        now: i64,
    ) -> Result<Option<Submission>> {
        let scheduled = match decision {
            Decision::ApprovePublish { at } => Some(at),
            Decision::ApproveOnly | Decision::Reject => None,
        };
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated: Option<Submission> = sqlx::query_as(
            "UPDATE submissions SET status = ?, moderator_id = ?, decision_ts = ?, scheduled_ts = ?
             WHERE submission_id = ? AND status = 'pending' RETURNING *",
        )
        .bind(decision.status())
        .bind(moderator)
        .bind(now)
        .bind(scheduled)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(s) = &updated {
            sqlx::query(
                "INSERT INTO admin_action_logs (action_type, admin_user_id, target_user_id, submission_id, action_ts)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(decision.action())
            .bind(moderator)
            .bind(s.user_id)
            .bind(id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(updated)
    }

    pub async fn next_scheduled(&self) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT min(scheduled_ts) FROM submissions WHERE status = 'scheduled'",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    /// Takes one due publication and marks it `publishing` before anything is sent.
    pub async fn claim_due(&self, now: i64) -> Result<Option<Submission>> {
        Ok(sqlx::query_as(
            "UPDATE submissions SET status = 'publishing'
             WHERE submission_id = (SELECT submission_id FROM submissions
                                    WHERE status = 'scheduled' AND scheduled_ts <= ?
                                    ORDER BY scheduled_ts LIMIT 1)
             RETURNING *",
        )
        .bind(now)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn mark_published(&self, id: &str, channel_message_id: i64, now: i64) -> Result<()> {
        sqlx::query(
            "UPDATE submissions SET status = 'published', message_id_in_channel = ?, decision_ts = ?
             WHERE submission_id = ?",
        )
        .bind(channel_message_id)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Python's error handler: count the attempt, retry after a delay while under
    /// `max_attempts`, otherwise give up and keep the error text.
    pub async fn publication_failed(
        &self,
        id: &str,
        error: &str,
        now: i64,
        max_attempts: i64,
        retry_delay: i64,
    ) -> Result<RetryOutcome> {
        let attempt: i64 = sqlx::query_scalar(
            "UPDATE submissions SET publication_retry_count = publication_retry_count + 1,
                status = CASE WHEN publication_retry_count + 1 < ?1 THEN 'scheduled' ELSE 'publication_failed' END,
                scheduled_ts = CASE WHEN publication_retry_count + 1 < ?1 THEN ?2 ELSE scheduled_ts END,
                publication_error_message = CASE WHEN publication_retry_count + 1 < ?1
                    THEN publication_error_message ELSE ?3 END
             WHERE submission_id = ?4 RETURNING publication_retry_count",
        )
        .bind(max_attempts)
        .bind(now + retry_delay)
        .bind(error)
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(if attempt < max_attempts {
            RetryOutcome::Retry { attempt }
        } else {
            RetryOutcome::Failed { attempt }
        })
    }

    pub async fn recover(&self, now: i64, delay: i64) -> Result<Recovery> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let interrupted: Vec<Submission> = sqlx::query_as(
            "UPDATE submissions SET status = 'publication_failed',
                publication_error_message = 'interrupted by a restart while sending'
             WHERE status = 'publishing' RETURNING *",
        )
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query("UPDATE submissions SET status = 'scheduled', scheduled_ts = ? WHERE status = 'approved'")
            .bind(now + delay)
            .execute(&mut *tx)
            .await?;
        let undelivered: Vec<Submission> = sqlx::query_as(
            "SELECT * FROM submissions WHERE status = 'pending' AND message_id_in_admin_chat IS NULL
             ORDER BY submission_ts",
        )
        .fetch_all(&mut *tx)
        .await?;
        let pending: i64 =
            sqlx::query_scalar("SELECT count(*) FROM submissions WHERE status = 'pending'")
                .fetch_one(&mut *tx)
                .await?;
        tx.commit().await?;
        Ok(Recovery {
            interrupted,
            undelivered,
            pending,
        })
    }

    /// Monthly statistics by submission time, as in `statistics_service.py`.
    pub async fn month_counts(&self, start: i64, end: i64) -> Result<MonthCounts> {
        let (total, approved, published, rejected, unique_users): (i64, i64, i64, i64, i64) =
            sqlx::query_as(
                "SELECT count(*),
                count(*) FILTER (WHERE status IN ('approved', 'accepted_not_published', 'published',
                                                  'scheduled', 'publishing')),
                count(*) FILTER (WHERE status = 'published'),
                count(*) FILTER (WHERE status = 'rejected'),
                count(DISTINCT user_id)
             FROM submissions WHERE submission_ts >= ? AND submission_ts < ?",
            )
            .bind(start)
            .bind(end)
            .fetch_one(&self.pool)
            .await?;
        let (new_users, blocked_users): (i64, i64) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE registration_ts >= ? AND registration_ts < ?),
                    count(*) FILTER (WHERE is_blocked)
             FROM users",
        )
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await?;
        let admins = sqlx::query_as(
            "SELECT s.moderator_id, u.username, count(*) AS decisions
             FROM submissions s LEFT JOIN users u ON u.user_id = s.moderator_id
             WHERE s.submission_ts >= ? AND s.submission_ts < ? AND s.moderator_id IS NOT NULL
             GROUP BY s.moderator_id ORDER BY s.moderator_id",
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.pool)
        .await?;
        Ok(MonthCounts {
            total,
            approved,
            published,
            rejected,
            unique_users,
            new_users,
            blocked_users,
            admins,
        })
    }
}
