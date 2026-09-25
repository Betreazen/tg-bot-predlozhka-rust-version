use std::{
    path::Path,
    process::{Command, Output},
};

fn bot(args: &[&str], data: Option<&Path>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tg-bot-predlozhka"));
    cmd.args(args).env_clear().env("NO_COLOR", "1");
    if let Some(data) = data {
        cmd.envs([
            ("BOT_TOKEN", "123:fake"),
            ("CHANNEL_ID", "-1001"),
            ("ADMIN_CHAT_ID", "-1002"),
            ("ADMIN_IDS", "99"),
            (
                "MESSAGES_PATH",
                concat!(env!("CARGO_MANIFEST_DIR"), "/messages.json"),
            ),
        ]);
        cmd.env("DATA_DIR", data);
    }
    cmd.output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn version_needs_no_configuration() {
    let o = bot(&["--version"], None);
    assert!(o.status.success());
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        format!("tg-bot-predlozhka {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn missing_token_fails_before_touching_data() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let o = Command::new(env!("CARGO_BIN_EXE_tg-bot-predlozhka"))
        .arg("--check")
        .env_clear()
        .env("DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("BOT_TOKEN is required"),
        "{}",
        stderr(&o)
    );
    assert!(!data.exists());
}

#[test]
fn check_creates_the_schema_without_telegram() {
    let dir = tempfile::tempdir().unwrap();
    let o = bot(&["--check"], Some(dir.path()));
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        "configuration, texts and database: ok"
    );
    assert!(dir.path().join("bot.db").exists());
}

#[test]
fn unknown_arguments_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let o = bot(&["--frobnicate"], Some(dir.path()));
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--check"));
}

#[test]
fn import_prints_the_summary() {
    let dir = tempfile::tempdir().unwrap();
    let export = tempfile::tempdir().unwrap();
    std::fs::write(
        export.path().join("users.json"),
        r#"[{"user_id":1,"username":null,"first_name":"A","last_name":null,"is_blocked":false,"admin_note":null,
             "total_submissions_count":0,"registration_timestamp":"2026-07-01T00:00:00",
             "last_interaction_timestamp":"2026-07-01T00:00:00"}]"#,
    )
    .unwrap();
    std::fs::write(export.path().join("submissions.json"), "[]\n").unwrap();
    std::fs::write(export.path().join("admin_action_logs.json"), "[]\n").unwrap();
    let o = bot(
        &["--import", export.path().to_str().unwrap()],
        Some(dir.path()),
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        "users=1 blocked=0 notes=0 total_submissions_count=0 submissions=0 admin_action_logs=0 statuses="
    );
    let o = bot(&["--import"], Some(dir.path()));
    assert!(!o.status.success());
}

#[test]
fn second_instance_on_the_same_data_refuses_to_start() {
    let dir = tempfile::tempdir().unwrap();
    let lock = std::fs::File::create(dir.path().join(".instance.lock")).unwrap();
    lock.lock().unwrap();
    let o = bot(&["--check"], Some(dir.path()));
    assert!(!o.status.success());
    assert!(stderr(&o).contains("another instance"), "{}", stderr(&o));
}
