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
        let value = |key: &str| {
            get(key)
                .map(|v| clean(&v).to_owned())
                .filter(|v| !v.is_empty())
        };
        let required = |key: &str| value(key).with_context(|| format!("{key} is required"));
        fn number<T: FromStr>(key: &str, raw: Option<String>, default: T) -> Result<T> {
            raw.map(|s| {
                s.parse::<T>()
                    .ok()
                    .with_context(|| format!("invalid {key}"))
            })
            .transpose()
            .map(|n| n.unwrap_or(default))
        }
        let positive = |key: &str, default: i64| -> Result<i64> {
            let n = number(key, value(key), default)?;
            ensure!(n > 0, "{key} must be positive");
            Ok(n)
        };
        let flag = |key: &str| -> Result<bool> {
            match value(key).map(|v| v.to_ascii_lowercase()).as_deref() {
                None | Some("true" | "1" | "yes") => Ok(true),
                Some("false" | "0" | "no") => Ok(false),
                Some(_) => bail!("invalid {key}: expected true or false"),
            }
        };

        let token = required("BOT_TOKEN")?;
        let channel_id = number("CHANNEL_ID", Some(required("CHANNEL_ID")?), 0)?;
        let admin_chat_id = number("ADMIN_CHAT_ID", Some(required("ADMIN_CHAT_ID")?), 0)?;
        let error_chat_id = number("ERROR_CHAT_ID", value("ERROR_CHAT_ID"), admin_chat_id)?;
        let mut admin_ids = HashSet::new();
        for part in value("ADMIN_IDS").unwrap_or_default().split([',', ';']) {
            let part = part.trim();
            if !part.is_empty() {
                admin_ids.insert(part.parse::<i64>().ok().with_context(|| {
                    format!("invalid ADMIN_IDS entry {part:?}: expected integer user IDs")
                })?);
            }
        }
        let tz_name = value("TIMEZONE").unwrap_or_else(|| "Europe/Moscow".into());
        let timezone =
            TimeZone::get(&tz_name).with_context(|| format!("invalid TIMEZONE {tz_name:?}"))?;

        Ok(Self {
            token,
            channel_id,
            admin_chat_id,
            error_chat_id,
            admin_ids,
            data_dir: value("DATA_DIR").unwrap_or_else(|| "data".into()).into(),
            messages_path: value("MESSAGES_PATH")
                .unwrap_or_else(|| "messages.json".into())
                .into(),
            submissions_per_day: positive("SUBMISSIONS_PER_DAY", 2)?,
            timezone,
            publication_delay_seconds: number(
                "PUBLICATION_DELAY_MINUTES",
                value("PUBLICATION_DELAY_MINUTES"),
                2u32,
            )? as i64
                * 60,
            max_file_size_mb: positive("MAX_FILE_SIZE_MB", 200)? as u64,
            footer_text: get("FOOTER_TEXT")
                .map(|v| clean(&v).to_owned())
                .unwrap_or_default(),
            hashtags: get("HASHTAGS")
                .map(|v| clean(&v).to_owned())
                .unwrap_or_default(),
            require_confirmation: flag("REQUIRE_CONFIRMATION")?,
            enable_blocking: flag("ENABLE_BLOCKING")?,
            max_retry_attempts: positive("MAX_RETRY_ATTEMPTS", 2)?,
            retry_delay_seconds: number("RETRY_DELAY_SECONDS", value("RETRY_DELAY_SECONDS"), 30u32)?
                as i64,
        })
    }

    pub fn is_admin(&self, user_id: i64) -> bool {
        self.admin_ids.contains(&user_id)
    }
}
