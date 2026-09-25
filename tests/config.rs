use std::collections::HashMap;
use tg_bot_predlozhka::config::Config;

fn parse(pairs: &[(&str, &str)]) -> anyhow::Result<Config> {
    let mut env: HashMap<&str, &str> = HashMap::from([
        ("BOT_TOKEN", "123:fake"),
        ("CHANNEL_ID", "-1001"),
        ("ADMIN_CHAT_ID", "-1002"),
        ("ADMIN_IDS", "7"),
    ]);
    env.extend(pairs.iter().copied());
    Config::parse(|key| env.get(key).map(|v| v.to_string()))
}

#[test]
fn defaults_match_python_config_json() {
    let c = parse(&[]).unwrap();
    assert_eq!(c.channel_id, -1001);
    assert_eq!(c.admin_chat_id, -1002);
    assert_eq!(c.error_chat_id, -1002, "errors fall back to the admin chat");
    assert_eq!(c.submissions_per_day, 2);
    assert_eq!(c.timezone.iana_name(), Some("Europe/Moscow"));
    assert_eq!(c.publication_delay_seconds, 120);
    assert_eq!(c.max_file_size_mb, 200);
    assert_eq!(c.footer_text, "");
    assert_eq!(c.hashtags, "");
    assert!(c.require_confirmation);
    assert!(c.enable_blocking);
    assert_eq!(c.max_retry_attempts, 2);
    assert_eq!(c.retry_delay_seconds, 30);
    assert_eq!(c.messages_path.to_str(), Some("messages.json"));
}

#[test]
fn required_values_must_be_present() {
    for key in ["BOT_TOKEN", "CHANNEL_ID", "ADMIN_CHAT_ID"] {
        let err = parse(&[(key, "  ")]).err().expect(key);
        assert!(format!("{err:#}").contains(key), "{err:#}");
    }
}

#[test]
fn values_saved_on_windows_or_quoted_still_parse() {
    let c = parse(&[
        ("BOT_TOKEN", "\"123:fake\"\r"),
        ("CHANNEL_ID", "-1005\r"),
        ("ERROR_CHAT_ID", "'-1009'"),
        ("PUBLICATION_DELAY_MINUTES", " 3\r"),
        ("REQUIRE_CONFIRMATION", "false\r"),
    ])
    .unwrap();
    assert_eq!(c.token, "123:fake");
    assert_eq!(c.channel_id, -1005);
    assert_eq!(c.error_chat_id, -1009);
    assert_eq!(c.publication_delay_seconds, 180);
    assert!(!c.require_confirmation);
}

#[test]
fn admin_ids_accept_commas_semicolons_and_spaces() {
    let c = parse(&[("ADMIN_IDS", " 1, 2;3 ,, ")]).unwrap();
    let mut ids: Vec<_> = c.admin_ids.iter().copied().collect();
    ids.sort();
    assert_eq!(ids, [1, 2, 3]);
    assert!(c.is_admin(2));
    assert!(!c.is_admin(4));
    assert!(parse(&[("ADMIN_IDS", "")]).unwrap().admin_ids.is_empty());
}

#[test]
fn invalid_admin_id_is_an_error_like_python() {
    let err = parse(&[("ADMIN_IDS", "1,abc")]).err().unwrap();
    assert!(format!("{err:#}").contains("ADMIN_IDS"));
}

#[test]
fn invalid_numbers_booleans_and_timezone_are_errors() {
    assert!(parse(&[("SUBMISSIONS_PER_DAY", "two")]).is_err());
    assert!(parse(&[("SUBMISSIONS_PER_DAY", "0")]).is_err());
    assert!(parse(&[("ENABLE_BLOCKING", "maybe")]).is_err());
    assert!(parse(&[("TIMEZONE", "Mars/Olympus")]).is_err());
    assert!(parse(&[("ENABLE_BLOCKING", "0")]).is_ok());
}

#[test]
fn footer_and_hashtags_are_kept_verbatim() {
    let c = parse(&[("FOOTER_TEXT", "<b>Канал</b>"), ("HASHTAGS", "#a #b")]).unwrap();
    assert_eq!(c.footer_text, "<b>Канал</b>");
    assert_eq!(c.hashtags, "#a #b");
}
