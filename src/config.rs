use anyhow::{Context, Result, bail, ensure};
use jiff::tz::TimeZone;
use std::{collections::HashSet, path::PathBuf, str::FromStr};

// Deliberately no Debug: configuration owns the bot credential.
pub struct Config {
    pub token: String,
    pub channel_id: i64,
    pub admin_chat_id: i64,
    pub error_chat_id: i64,
    pub admin_ids: HashSet<i64>,
    pub data_dir: PathBuf,
    pub messages_path: PathBuf,
    pub submissions_per_day: i64,
    pub timezone: TimeZone,
    pub publication_delay_seconds: i64,
    pub max_file_size_mb: u64,
    pub footer_text: String,
    pub hashtags: String,
    pub require_confirmation: bool,
    pub enable_blocking: bool,
    pub max_retry_attempts: i64,
    pub retry_delay_seconds: i64,
}

/// Values from an `.env` edited on Windows end with `\r`, and dotenv-style files
/// may quote them; Python tolerated both, so we do too.
fn clean(raw: &str) -> &str {
    let s = raw.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return inner.trim();
        }
    }
    s
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::parse(|key| std::env::var(key).ok())
    }

    pub fn parse(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let env = Env(get);
        let token = env.required("BOT_TOKEN")?;
        let admin_chat_id = env.number("ADMIN_CHAT_ID", None)?;
        Ok(Self {
            token,
            channel_id: env.number("CHANNEL_ID", None)?,
            admin_chat_id,
            error_chat_id: env.number("ERROR_CHAT_ID", Some(admin_chat_id))?,
            admin_ids: env.admin_ids()?,
            data_dir: env
                .value("DATA_DIR")
                .unwrap_or_else(|| "data".into())
                .into(),
            messages_path: env
                .value("MESSAGES_PATH")
                .unwrap_or_else(|| "messages.json".into())
                .into(),
            submissions_per_day: env.positive("SUBMISSIONS_PER_DAY", 2)?,
            timezone: env.timezone()?,
            publication_delay_seconds: env.number::<u32>("PUBLICATION_DELAY_MINUTES", Some(2))?
                as i64
                * 60,
            max_file_size_mb: env.positive("MAX_FILE_SIZE_MB", 200)? as u64,
            footer_text: env.verbatim("FOOTER_TEXT"),
            hashtags: env.verbatim("HASHTAGS"),
            require_confirmation: env.flag("REQUIRE_CONFIRMATION")?,
            enable_blocking: env.flag("ENABLE_BLOCKING")?,
            max_retry_attempts: env.positive("MAX_RETRY_ATTEMPTS", 2)?,
            retry_delay_seconds: env.number::<u32>("RETRY_DELAY_SECONDS", Some(30))? as i64,
        })
    }

    pub fn is_admin(&self, user_id: i64) -> bool {
        self.admin_ids.contains(&user_id)
    }
}

struct Env<F>(F);

impl<F: Fn(&str) -> Option<String>> Env<F> {
    fn value(&self, key: &str) -> Option<String> {
        (self.0)(key)
            .map(|v| clean(&v).to_owned())
            .filter(|v| !v.is_empty())
    }

    /// Footer and hashtags may legitimately be empty.
    fn verbatim(&self, key: &str) -> String {
        (self.0)(key)
            .map(|v| clean(&v).to_owned())
            .unwrap_or_default()
    }

    fn required(&self, key: &str) -> Result<String> {
        self.value(key)
            .with_context(|| format!("{key} is required"))
    }

    /// `default: None` makes the value required.
    fn number<T: FromStr>(&self, key: &str, default: Option<T>) -> Result<T> {
        match (self.value(key), default) {
            (Some(raw), _) => raw.parse().ok().with_context(|| format!("invalid {key}")),
            (None, Some(default)) => Ok(default),
            (None, None) => bail!("{key} is required"),
        }
    }

    fn positive(&self, key: &str, default: i64) -> Result<i64> {
        let n = self.number(key, Some(default))?;
        ensure!(n > 0, "{key} must be positive");
        Ok(n)
    }

    fn flag(&self, key: &str) -> Result<bool> {
        match self.value(key).map(|v| v.to_ascii_lowercase()).as_deref() {
            None | Some("true" | "1" | "yes") => Ok(true),
            Some("false" | "0" | "no") => Ok(false),
            Some(_) => bail!("invalid {key}: expected true or false"),
        }
    }

    fn admin_ids(&self) -> Result<HashSet<i64>> {
        let raw = self.value("ADMIN_IDS").unwrap_or_default();
        raw.split([',', ';'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| {
                part.parse::<i64>().ok().with_context(|| {
                    format!("invalid ADMIN_IDS entry {part:?}: expected integer user IDs")
                })
            })
            .collect()
    }

    fn timezone(&self) -> Result<TimeZone> {
        let name = self
            .value("TIMEZONE")
            .unwrap_or_else(|| "Europe/Moscow".into());
        TimeZone::get(&name).with_context(|| format!("invalid TIMEZONE {name:?}"))
    }
}
