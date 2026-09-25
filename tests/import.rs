use serde_json::json;
use std::path::Path;
use tg_bot_predlozhka::{db::Database, import, model::status};

fn write(
    dir: &Path,
    users: serde_json::Value,
    submissions: serde_json::Value,
    logs: serde_json::Value,
) {
    std::fs::write(dir.join("users.json"), users.to_string()).unwrap();
    std::fs::write(dir.join("submissions.json"), submissions.to_string()).unwrap();
    std::fs::write(dir.join("admin_action_logs.json"), logs.to_string()).unwrap();
}

/// Rows exactly as `psql -Atc "select coalesce(json_agg(t), '[]') from <table> t"` prints them.
fn postgres_export(dir: &Path) {
    write(
        dir,
        json!([
            {"user_id":42,"username":"alice","first_name":"Алиса","last_name":null,"is_blocked":true,
             "admin_note":"заметка","total_submissions_count":2,
             "registration_timestamp":"2026-06-22T08:50:12.123456","last_interaction_timestamp":"2026-09-25T10:00:00"},
            {"user_id":7,"username":null,"first_name":"B","last_name":"C","is_blocked":false,"admin_note":null,
             "total_submissions_count":0,"registration_timestamp":"2026-07-01T00:00:00","last_interaction_timestamp":"2026-07-01T00:00:00"}
        ]),
        json!([
            {"submission_id":"6f1c2a4e-0b7d-4d7e-9c55-3c1a8e0f9b21","user_id":42,"submission_timestamp":"2026-09-01T12:00:00.5",
             "status":"scheduled","moderator_id":7,"decision_timestamp":"2026-09-01T12:05:00","show_authorship":true,
             "message_id_in_admin_chat":10,"message_id_in_channel":null,"user_message_id":5,"user_chat_id":42,
             "has_media":false,"media_type":null,"media_file_id":null,"text_content":"a < b",
             "scheduled_publication_time":"2026-09-01T12:07:00","publication_error_message":null,"publication_retry_count":1},
            {"submission_id":"0a2b3c4d-1111-4222-8333-944455556666","user_id":42,"submission_timestamp":"2026-09-02T00:00:00",
             "status":"rejected","moderator_id":7,"decision_timestamp":null,"show_authorship":false,
             "message_id_in_admin_chat":null,"message_id_in_channel":null,"user_message_id":null,"user_chat_id":null,
             "has_media":true,"media_type":null,"media_file_id":null,"text_content":null,
             "scheduled_publication_time":null,"publication_error_message":null,"publication_retry_count":0}
        ]),
        json!([
            {"log_id":3,"action_type":"reject","admin_user_id":7,"target_user_id":42,
             "submission_id":"0a2b3c4d-1111-4222-8333-944455556666","action_timestamp":"2026-09-02T00:01:00","additional_context":null}
        ]),
    );
}

#[tokio::test]
async fn imports_every_row_and_reports_counts() {
    let data = tempfile::tempdir().unwrap();
    let export = tempfile::tempdir().unwrap();
    postgres_export(export.path());
    let db = Database::open(data.path()).await.unwrap();
    let summary = import::run(&db, export.path()).await.unwrap();
    assert_eq!(
        summary.to_string(),
        "users=2 blocked=1 notes=1 total_submissions_count=2 submissions=2 admin_action_logs=1 statuses=rejected:1,scheduled:1"
    );
    let alice = db.user(42).await.unwrap().unwrap();
    assert_eq!(
        (alice.is_blocked, alice.admin_note.as_deref()),
        (true, Some("заметка"))
    );
    assert_eq!(
        alice.registration_ts, 1_782_118_212,
        "fractional seconds are dropped"
    );
    let s = db
        .submission("6f1c2a4e-0b7d-4d7e-9c55-3c1a8e0f9b21")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(s.status, status::SCHEDULED);
    assert_eq!(
        s.text_content.as_deref(),
        Some("a &lt; b"),
        "Python stored plain text; Rust stores HTML"
    );
    assert_eq!(s.scheduled_ts, Some(1_788_264_420));
    assert_eq!(s.publication_retry_count, 1);
    // The next log continues after the imported ids.
    let next: i64 = sqlx::query_scalar(
        "INSERT INTO admin_action_logs (action_type, admin_user_id, action_ts) VALUES ('reject', 1, 0) RETURNING log_id",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(next, 4);
}

#[tokio::test]
async fn refuses_to_import_into_a_database_with_data() {
    let data = tempfile::tempdir().unwrap();
    let export = tempfile::tempdir().unwrap();
    postgres_export(export.path());
    let db = Database::open(data.path()).await.unwrap();
    import::run(&db, export.path()).await.unwrap();
    let err = import::run(&db, export.path()).await.err().unwrap();
    assert!(format!("{err:#}").contains("not empty"), "{err:#}");
}

#[tokio::test]
async fn unexpected_columns_or_values_abort_the_whole_import() {
    let data = tempfile::tempdir().unwrap();
    let export = tempfile::tempdir().unwrap();
    let db = Database::open(data.path()).await.unwrap();
    let user = json!({"user_id":1,"username":null,"first_name":null,"last_name":null,"is_blocked":false,"admin_note":null,
                      "total_submissions_count":0,"registration_timestamp":"2026-07-01T00:00:00",
                      "last_interaction_timestamp":"2026-07-01T00:00:00"});
    let mut extra = user.clone();
    extra["new_column"] = json!(1);
    write(export.path(), json!([extra]), json!([]), json!([]));
    assert!(import::run(&db, export.path()).await.is_err());

    let bad_status = json!([{"submission_id":"6f1c2a4e-0b7d-4d7e-9c55-3c1a8e0f9b21","user_id":1,
        "submission_timestamp":"2026-09-01T12:00:00","status":"weird","moderator_id":null,"decision_timestamp":null,
        "show_authorship":true,"message_id_in_admin_chat":null,"message_id_in_channel":null,"user_message_id":null,
        "user_chat_id":null,"has_media":false,"media_type":null,"media_file_id":null,"text_content":null,
        "scheduled_publication_time":null,"publication_error_message":null,"publication_retry_count":0}]);
    write(export.path(), json!([user]), bad_status, json!([]));
    assert!(import::run(&db, export.path()).await.is_err());
    assert!(
        db.user(1).await.unwrap().is_none(),
        "nothing is committed on failure"
    );
}
