use tg_bot_predlozhka::{
    db::{Database, Decision, NewSubmission, Profile, RetryOutcome},
    model::{Draft, Step, status},
};

const T0: i64 = 1_780_000_000;
const DAY: i64 = 86_400;

async fn actions(db: &Database, kind: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM admin_action_logs WHERE action_type = ?")
        .bind(kind)
        .fetch_one(db.pool())
        .await
        .unwrap()
}

async fn open() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path()).await.unwrap();
    (dir, db)
}

fn profile(id: i64, username: Option<&str>) -> Profile<'_> {
    Profile {
        user_id: id,
        username,
        first_name: Some("Имя"),
        last_name: None,
    }
}

fn new_submission(user: i64, message_id: i64) -> NewSubmission {
    NewSubmission {
        user_id: user,
        user_chat_id: user,
        user_message_id: message_id,
        show_authorship: true,
        text_content: Some("текст".into()),
        media: None,
        album: None,
    }
}

async fn submit(db: &Database, user: i64, now: i64) -> String {
    db.touch_user(&profile(user, Some("u")), now).await.unwrap();
    db.create_submission(&new_submission(user, 1), now, now - 10, 2)
        .await
        .unwrap()
        .expect("under limit")
        .submission_id
}

#[tokio::test]
async fn profile_update_does_not_clobber_known_fields_with_none() {
    let (_d, db) = open().await;
    db.touch_user(&profile(1, Some("alice")), T0).await.unwrap();
    let user = db
        .touch_user(
            &Profile {
                user_id: 1,
                username: None,
                first_name: None,
                last_name: Some("L"),
            },
            T0 + 5,
        )
        .await
        .unwrap();
    assert_eq!(user.username.as_deref(), Some("alice"));
    assert_eq!(user.first_name.as_deref(), Some("Имя"));
    assert_eq!(user.last_name.as_deref(), Some("L"));
    assert_eq!(
        (user.registration_ts, user.last_interaction_ts),
        (T0, T0 + 5)
    );
}

#[tokio::test]
async fn blocking_unknown_user_reports_failure() {
    let (_d, db) = open().await;
    assert!(!db.set_blocked(5, true).await.unwrap());
    db.touch_user(&profile(5, None), T0).await.unwrap();
    assert!(db.set_blocked(5, true).await.unwrap());
    assert!(db.user(5).await.unwrap().unwrap().is_blocked);
    assert!(db.set_blocked(5, false).await.unwrap());
    assert!(!db.user(5).await.unwrap().unwrap().is_blocked);
}

#[tokio::test]
async fn daily_limit_is_enforced_atomically_and_resets_next_day() {
    let (_d, db) = open().await;
    db.touch_user(&profile(1, None), T0).await.unwrap();
    db.set_dialogue(1, Step::WaitingForAuthorship, None)
        .await
        .unwrap();
    for i in 0..2 {
        let s = db
            .create_submission(&new_submission(1, i), T0 + i, T0 - 100, 2)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.status, status::PENDING);
    }
    assert!(
        db.create_submission(&new_submission(1, 3), T0 + 3, T0 - 100, 2)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(db.count_since(1, T0 - 100).await.unwrap(), 2);
    assert_eq!(
        db.user(1).await.unwrap().unwrap().total_submissions_count,
        2
    );
    assert!(
        db.dialogue(1).await.unwrap().is_none(),
        "a created submission ends the dialogue"
    );
    // Next local day.
    assert!(
        db.create_submission(&new_submission(1, 4), T0 + DAY, T0 + DAY - 100, 2)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn dialogue_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let draft = Draft {
        chat_id: 1,
        message_id: 7,
        text: Some("<b>x</b>".into()),
        media: None,
        album: vec![],
    };
    {
        let db = Database::open(dir.path()).await.unwrap();
        db.set_dialogue(1, Step::WaitingForConfirmation, Some(&draft))
            .await
            .unwrap();
        db.close().await;
    }
    let db = Database::open(dir.path()).await.unwrap();
    assert_eq!(
        db.dialogue(1).await.unwrap(),
        Some((Step::WaitingForConfirmation, Some(draft)))
    );
    db.clear_dialogue(1).await.unwrap();
    assert_eq!(db.dialogue(1).await.unwrap(), None);
}

#[tokio::test]
async fn decision_is_taken_once_and_logged() {
    let (_d, db) = open().await;
    let id = submit(&db, 1, T0).await;
    let decided = db
        .decide(&id, Decision::Reject, 99, T0 + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (decided.status.as_str(), decided.moderator_id),
        (status::REJECTED, Some(99))
    );
    assert!(
        db.decide(&id, Decision::ApproveOnly, 98, T0 + 2)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(actions(&db, "reject").await, 1);
    assert_eq!(actions(&db, "approve_only").await, 0);
}

#[tokio::test]
async fn approve_publish_schedules_and_claim_happens_once() {
    let (_d, db) = open().await;
    let id = submit(&db, 1, T0).await;
    let s = db
        .decide(&id, Decision::ApprovePublish { at: T0 + 120 }, 99, T0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (s.status.as_str(), s.scheduled_ts),
        (status::SCHEDULED, Some(T0 + 120))
    );
    assert_eq!(actions(&db, "approve_publish").await, 1);
    assert_eq!(db.next_scheduled().await.unwrap(), Some(T0 + 120));
    assert!(
        db.claim_due(T0 + 119).await.unwrap().is_none(),
        "not due yet"
    );
    let claimed = db.claim_due(T0 + 120).await.unwrap().unwrap();
    assert_eq!(claimed.status, status::PUBLISHING);
    assert!(
        db.claim_due(T0 + 500).await.unwrap().is_none(),
        "never claimed twice"
    );
    assert_eq!(db.next_scheduled().await.unwrap(), None);
    db.mark_published(&id, 555, T0 + 121).await.unwrap();
    let s = db.submission(&id).await.unwrap().unwrap();
    assert_eq!(
        (s.status.as_str(), s.message_id_in_channel),
        (status::PUBLISHED, Some(555))
    );
}

#[tokio::test]
async fn failed_publication_is_retried_then_marked_failed() {
    let (_d, db) = open().await;
    let id = submit(&db, 1, T0).await;
    db.decide(&id, Decision::ApprovePublish { at: T0 }, 99, T0)
        .await
        .unwrap();
    db.claim_due(T0).await.unwrap().unwrap();
    let outcome = db
        .publication_failed(&id, "boom", T0 + 1, 2, 30)
        .await
        .unwrap();
    assert_eq!(outcome, RetryOutcome::Retry { attempt: 1 });
    assert_eq!(
        db.next_scheduled().await.unwrap(),
        Some(T0 + 31),
        "retry time is persisted"
    );
    db.claim_due(T0 + 31).await.unwrap().unwrap();
    let outcome = db
        .publication_failed(&id, "boom2", T0 + 32, 2, 30)
        .await
        .unwrap();
    assert_eq!(outcome, RetryOutcome::Failed { attempt: 2 });
    let s = db.submission(&id).await.unwrap().unwrap();
    assert_eq!(s.status, status::PUBLICATION_FAILED);
    assert_eq!(s.publication_error_message.as_deref(), Some("boom2"));
    assert_eq!(s.publication_retry_count, 2);
}

#[tokio::test]
async fn recovery_never_resends_an_interrupted_publication() {
    let (_d, db) = open().await;
    let interrupted = submit(&db, 1, T0).await;
    db.decide(&interrupted, Decision::ApprovePublish { at: T0 }, 99, T0)
        .await
        .unwrap();
    db.claim_due(T0).await.unwrap().unwrap();
    let approved = submit(&db, 2, T0).await;
    sqlx::query("UPDATE submissions SET status = 'approved' WHERE submission_id = ?")
        .bind(&approved)
        .execute(db.pool())
        .await
        .unwrap();
    let undelivered = submit(&db, 3, T0).await;
    let delivered = submit(&db, 4, T0).await;
    db.set_admin_message(&delivered, 10).await.unwrap();

    let r = db.recover(T0 + 1000, 120).await.unwrap();
    assert_eq!(
        r.interrupted
            .iter()
            .map(|s| s.submission_id.as_str())
            .collect::<Vec<_>>(),
        [interrupted.as_str()]
    );
    assert_eq!(
        db.submission(&interrupted).await.unwrap().unwrap().status,
        status::PUBLICATION_FAILED
    );
    let a = db.submission(&approved).await.unwrap().unwrap();
    assert_eq!(
        (a.status.as_str(), a.scheduled_ts),
        (status::SCHEDULED, Some(T0 + 1120))
    );
    assert_eq!(
        r.undelivered
            .iter()
            .map(|s| s.submission_id.as_str())
            .collect::<Vec<_>>(),
        [undelivered.as_str()]
    );
    assert_eq!(r.pending, 2);
}

#[tokio::test]
async fn monthly_counts_match_python_definitions() {
    let (_d, db) = open().await;
    // tests/test_statistics_service.py::test_current_month_counts_and_rates
    let ids = [
        submit(&db, 1, T0).await,
        submit(&db, 2, T0).await,
        submit(&db, 2, T0 + 1).await,
        submit(&db, 3, T0).await,
    ];
    db.decide(&ids[0], Decision::ApprovePublish { at: T0 }, 99, T0)
        .await
        .unwrap();
    db.claim_due(T0).await.unwrap();
    db.mark_published(&ids[0], 1, T0).await.unwrap();
    db.decide(&ids[1], Decision::ApproveOnly, 99, T0)
        .await
        .unwrap();
    db.decide(&ids[2], Decision::Reject, 98, T0).await.unwrap();
    submit(&db, 4, T0 + 40 * DAY).await; // next month
    db.set_blocked(3, true).await.unwrap();
    db.touch_user(&profile(99, Some("mod")), T0 - 40 * DAY)
        .await
        .unwrap();

    let c = db.month_counts(T0 - DAY, T0 + DAY).await.unwrap();
    assert_eq!((c.total, c.approved, c.published, c.rejected), (4, 2, 1, 1));
    assert_eq!((c.unique_users, c.new_users, c.blocked_users), (3, 3, 1));
    let admins: Vec<_> = c
        .admins
        .iter()
        .map(|a| (a.moderator_id, a.username.as_deref(), a.decisions))
        .collect();
    assert_eq!(admins, [(98, None, 1), (99, Some("mod"), 2)]);
}
