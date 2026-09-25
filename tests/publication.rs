mod common;
use common::*;
use serde_json::{Value, json};
use std::time::Duration;
use tg_bot_predlozhka::{
    db::Decision,
    model::{Submission, status},
    publish, time,
};
use wiremock::{Mock, ResponseTemplate, matchers};

async fn approve(e: &Env, s: &Submission) {
    e.app
        .db
        .decide(
            &s.submission_id,
            Decision::ApprovePublish { at: time::now() },
            ADMIN,
            time::now(),
        )
        .await
        .unwrap()
        .unwrap();
}

async fn channel_posts(e: &Env) -> Vec<(String, Value)> {
    calls(&e.server)
        .await
        .into_iter()
        .filter(|(_, b)| b["chat_id"] == CHANNEL)
        .collect()
}

async fn reload(e: &Env, s: &Submission) -> Submission {
    e.app
        .db
        .submission(&s.submission_id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn text_is_published_with_authorship() {
    // P2, P4, F1: copyMessage ignores `caption` for text, so the author line was lost.
    let e = setup().await;
    let s = submit(&e, USER, text(2, USER, "<пост>"), true).await;
    approve(&e, &s).await;
    publish::publish_due(&e.bot, &e.app).await.unwrap();
    let posts = channel_posts(&e).await;
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].0, "SendMessage");
    assert_eq!(posts[0].1["text"], "&lt;пост&gt;\n\nАвтор: @user42");
    assert_eq!(posts[0].1["parse_mode"], "HTML");
    let s = reload(&e, &s).await;
    assert_eq!(
        (s.status.as_str(), s.message_id_in_channel),
        (status::PUBLISHED, Some(123))
    );
}

#[tokio::test]
async fn anonymous_post_gets_footer_and_hashtags() {
    // P2
    let e = setup_with(&[
        ("FOOTER_TEXT", "<a href=\"https://t.me/x\">Канал</a>"),
        ("HASHTAGS", "#a #b"),
    ])
    .await;
    let s = submit(&e, USER, text(2, USER, "пост"), false).await;
    approve(&e, &s).await;
    publish::publish_due(&e.bot, &e.app).await.unwrap();
    assert_eq!(
        channel_posts(&e).await[0].1["text"],
        "пост\n\n<a href=\"https://t.me/x\">Канал</a>\n\n#a #b"
    );
}

#[tokio::test]
async fn author_without_username_is_named_by_first_name() {
    // P2
    let e = setup().await;
    let s = submit(&e, USER, text(2, USER, "пост"), true).await;
    sqlx::query("UPDATE users SET username = NULL, first_name = '<Имя>'")
        .execute(e.app.db.pool())
        .await
        .unwrap();
    approve(&e, &s).await;
    publish::publish_due(&e.bot, &e.app).await.unwrap();
    assert_eq!(
        channel_posts(&e).await[0].1["text"],
        "пост\n\nАвтор: &lt;Имя&gt;"
    );
}

#[tokio::test]
async fn media_is_copied_with_new_caption() {
    // P3, test_publish_media_copies_to_channel
    let e = setup().await;
    let s = submit(
        &e,
        USER,
        photo(2, USER, 10, json!({"caption":"фото"})),
        true,
    )
    .await;
    approve(&e, &s).await;
    publish::publish_due(&e.bot, &e.app).await.unwrap();
    let posts = channel_posts(&e).await;
    assert_eq!(posts[0].0, "CopyMessage");
    assert_eq!(
        (
            posts[0].1["from_chat_id"].as_i64(),
            posts[0].1["message_id"].as_i64()
        ),
        (Some(USER), Some(2))
    );
    assert_eq!(posts[0].1["caption"], "фото\n\nАвтор: @user42");
}

#[tokio::test]
async fn album_is_published_as_a_media_group() {
    // N1
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, photo(11, USER, 10, json!({"media_group_id":"g"}))).await;
    msg(
        &e,
        photo(
            12,
            USER,
            10,
            json!({"media_group_id":"g","caption":"вторая"}),
        ),
    )
    .await;
    e.app.finish_album(&e.bot, "g").await.unwrap();
    cb(&e, user_cb(USER, "confirm_content")).await;
    cb(&e, user_cb(USER, "authorship_yes")).await;
    let s = all_submissions(&e).await.pop().unwrap();
    approve(&e, &s).await;
    publish::publish_due(&e.bot, &e.app).await.unwrap();
    let posts = channel_posts(&e).await;
    assert_eq!(posts[0].0, "SendMediaGroup");
    let media = posts[0].1["media"].as_array().unwrap();
    assert_eq!(media.len(), 2);
    assert_eq!(
        (media[0]["type"].as_str(), media[0]["media"].as_str()),
        (Some("photo"), Some("photo11"))
    );
    assert!(media[0].get("caption").is_none());
    assert_eq!(media[1]["caption"], "вторая\n\nАвтор: @user42");
    assert_eq!(media[1]["parse_mode"], "HTML");
    assert_eq!(reload(&e, &s).await.message_id_in_channel, Some(301));
}

#[tokio::test]
async fn failures_are_retried_then_reported_to_the_error_chat() {
    // P5, test_retry_exhausted_marks_failed_and_notifies_admin; decision Q2
    let e = setup().await;
    Mock::given(matchers::method("POST"))
        .and(matchers::body_partial_json(json!({"chat_id": CHANNEL})))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({"ok":false,"error_code":400,"description":"Bad Request: <chat> not found"}),
        ))
        .with_priority(1)
        .mount(&e.server)
        .await;
    let s = submit(&e, USER, text(2, USER, "пост"), true).await;
    approve(&e, &s).await;
    publish::publish_due(&e.bot, &e.app).await.unwrap();
    let r = reload(&e, &s).await;
    assert_eq!(
        (r.status.as_str(), r.publication_retry_count),
        (status::SCHEDULED, 1)
    );
    assert!(r.scheduled_ts.unwrap() >= time::now() + 29);
    assert!(
        calls_to(&e.server, "SendMessage")
            .await
            .iter()
            .all(|b| b["chat_id"] != ERROR_CHAT)
    );

    sqlx::query("UPDATE submissions SET scheduled_ts = 0")
        .execute(e.app.db.pool())
        .await
        .unwrap();
    publish::publish_due(&e.bot, &e.app).await.unwrap();
    let r = reload(&e, &s).await;
    assert_eq!(
        (r.status.as_str(), r.publication_retry_count),
        (status::PUBLICATION_FAILED, 2)
    );
    let report = calls_to(&e.server, "SendMessage")
        .await
        .into_iter()
        .find(|b| b["chat_id"] == ERROR_CHAT)
        .unwrap();
    let body = report["text"].as_str().unwrap();
    assert!(body.starts_with("⚠️ <b>Ошибка публикации</b>"), "{body}");
    assert!(body.contains(&format!("📋 Предложка ID: {}", s.submission_id)));
    assert!(body.contains("👤 Пользователь: @user42"));
    assert!(
        body.contains("&lt;chat&gt; not found"),
        "error text is escaped: {body}"
    );
    assert!(body.ends_with("Попытка 2 из 2"));
}

#[tokio::test]
async fn cancelled_or_decided_elsewhere_is_not_published() {
    // P1, test_publish_skipped_if_not_scheduled
    let e = setup().await;
    let s = submit(&e, USER, text(2, USER, "пост"), true).await;
    approve(&e, &s).await;
    sqlx::query("UPDATE submissions SET status = 'accepted_not_published'")
        .execute(e.app.db.pool())
        .await
        .unwrap();
    publish::publish_due(&e.bot, &e.app).await.unwrap();
    assert!(channel_posts(&e).await.is_empty());
}

#[tokio::test]
async fn worker_publishes_when_woken() {
    let e = setup().await;
    let worker = tokio::spawn(publish::run(e.bot.clone(), e.app.clone()));
    let s = submit(&e, USER, text(2, USER, "пост"), true).await;
    approve(&e, &s).await;
    e.app.wake.notify_one();
    for _ in 0..100 {
        if reload(&e, &s).await.status == status::PUBLISHED {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    worker.abort();
    assert_eq!(reload(&e, &s).await.status, status::PUBLISHED);
}

#[tokio::test]
async fn startup_recovers_without_double_posting() {
    // S1, S2, S4, S5, P6
    let e = setup().await;
    let interrupted = submit(&e, USER, text(2, USER, "a"), true).await;
    approve(&e, &interrupted).await;
    e.app.db.claim_due(time::now()).await.unwrap().unwrap();
    let approved = submit(&e, USER, text(3, USER, "b"), true).await;
    sqlx::query("UPDATE submissions SET status = 'approved' WHERE submission_id = ?")
        .bind(&approved.submission_id)
        .execute(e.app.db.pool())
        .await
        .unwrap();
    let undelivered = submit(&e, 43, text(4, 43, "c"), false).await;
    sqlx::query("UPDATE submissions SET message_id_in_admin_chat = NULL WHERE submission_id = ?")
        .bind(&undelivered.submission_id)
        .execute(e.app.db.pool())
        .await
        .unwrap();
    e.server.reset().await;
    Mock::given(matchers::method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sent_message(777)))
        .mount(&e.server)
        .await;

    publish::startup(&e.bot, &e.app).await.unwrap();

    assert!(
        channel_posts(&e).await.is_empty(),
        "interrupted publication is never resent"
    );
    assert_eq!(
        reload(&e, &interrupted).await.status,
        status::PUBLICATION_FAILED
    );
    let sent = calls_to(&e.server, "SendMessage").await;
    let alert = sent.iter().find(|b| b["chat_id"] == ERROR_CHAT).unwrap();
    assert!(
        alert["text"]
            .as_str()
            .unwrap()
            .contains(&interrupted.submission_id)
    );
    let card = sent
        .iter()
        .find(|b| {
            b["chat_id"] == ADMIN_CHAT && b["text"].as_str().unwrap().contains("Новое предложение")
        })
        .unwrap();
    assert!(
        card["text"]
            .as_str()
            .unwrap()
            .contains(&undelivered.submission_id[..8])
    );
    assert_eq!(
        reload(&e, &undelivered).await.message_id_in_admin_chat,
        Some(777)
    );
    let started: Vec<_> = sent
        .iter()
        .filter(|b| b["text"] == e.app.texts.admin.bot_started)
        .collect();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0]["chat_id"], ADMIN_CHAT);
    let a = reload(&e, &approved).await;
    assert_eq!(a.status, status::SCHEDULED);
    assert!(a.scheduled_ts.unwrap() >= time::now() + 119);
}

#[tokio::test]
async fn many_pending_submissions_are_announced() {
    // S3, test_recover_pending_notifies_when_many
    let e = setup_with(&[("SUBMISSIONS_PER_DAY", "20")]).await;
    for i in 0..11 {
        submit(&e, USER, text(10 + i, USER, "x"), false).await;
    }
    publish::startup(&e.bot, &e.app).await.unwrap();
    let sent = calls_to(&e.server, "SendMessage").await;
    assert!(
        sent.iter()
            .any(|b| b["chat_id"] == ADMIN_CHAT
                && b["text"] == "⚠️ 11 предложений ожидают модерации")
    );
}

#[tokio::test]
async fn token_is_redacted_from_error_text() {
    let e = setup().await;
    assert_eq!(
        e.app.redact("GET https://api/bot123:fake/x failed"),
        "GET https://api/bot[REDACTED]/x failed"
    );
}

#[tokio::test]
async fn shutdown_waits_for_a_send_in_progress_and_blocks_new_claims() {
    // Aborting the worker mid-send left rows in `publishing` and a false
    // "check the channel" alert after every routine restart.
    let e = setup().await;
    let s = submit(&e, USER, text(2, USER, "пост"), true).await;
    approve(&e, &s).await;
    let guard = e.app.sending.lock().await;
    let task = {
        let (bot, app) = (e.bot.clone(), e.app.clone());
        tokio::spawn(async move { publish::publish_due(&bot, &app).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        reload(&e, &s).await.status,
        status::SCHEDULED,
        "no claim while shutdown holds the lock"
    );
    drop(guard);
    task.await.unwrap().unwrap();
    assert_eq!(reload(&e, &s).await.status, status::PUBLISHED);
}

#[tokio::test]
async fn pending_album_parts_are_flushed_on_shutdown() {
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, photo(11, USER, 10, json!({"media_group_id":"g"}))).await;
    e.app.flush_albums(&e.bot).await;
    assert_eq!(
        calls_to(&e.server, "SendMessage").await.last().unwrap()["text"],
        e.app.texts.user.submission_received
    );
}
