mod common;
use common::*;
use serde_json::json;
use tg_bot_predlozhka::{
    bot::{handle_callback, handle_message},
    model::{Step, status},
    time,
};

async fn msg(e: &Env, m: teloxide::types::Message) {
    handle_message(e.bot.clone(), m, e.app.clone())
        .await
        .unwrap();
}
async fn cb(e: &Env, c: teloxide::types::CallbackQuery) {
    handle_callback(e.bot.clone(), c, e.app.clone())
        .await
        .unwrap();
}
async fn submissions(e: &Env) -> Vec<tg_bot_predlozhka::model::Submission> {
    sqlx::query_as("SELECT * FROM submissions ORDER BY submission_ts")
        .fetch_all(e.app.db.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn start_creates_user_and_greets() {
    // U1, test_user_flow::test_start_creates_user_and_greets
    let e = setup().await;
    msg(&e, text(1, USER, "/start")).await;
    let sent = calls_to(&e.server, "SendMessage").await;
    let t = &e.app.texts.user.welcome;
    assert_eq!(sent[0]["text"], format!("{}\n\n{}", t.title, t.text));
    assert_eq!(sent[0]["parse_mode"], "HTML");
    assert_eq!(buttons(&sent[0]), ["suggest_content"]);
    let user = e.app.db.user(USER).await.unwrap().unwrap();
    assert_eq!(user.username.as_deref(), Some("user42"));
}

#[tokio::test]
async fn full_text_submission_reaches_moderators_with_escaped_html() {
    // U2, U7–U9, M1; F2 — `<` and `&` used to break the card.
    let e = setup().await;
    msg(&e, text(1, USER, "/start")).await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    assert_eq!(
        e.app.db.dialogue(USER).await.unwrap().unwrap().0,
        Step::WaitingForContent
    );
    msg(&e, text(2, USER, "a < b & c")).await;
    cb(&e, user_cb(USER, "confirm_content")).await;
    cb(&e, user_cb(USER, "authorship_yes")).await;

    let sent = calls_to(&e.server, "SendMessage").await;
    let texts: Vec<&str> = sent.iter().map(|b| b["text"].as_str().unwrap()).collect();
    let u = &e.app.texts.user;
    assert_eq!(buttons(&sent[1]), ["cancel_submission"]);
    assert_eq!(texts[1], u.submission_prompt);
    assert_eq!(texts[2], u.submission_received);
    assert_eq!(buttons(&sent[2]), ["confirm_content", "cancel_submission"]);
    assert_eq!(texts[3], u.authorship_question);
    assert_eq!(buttons(&sent[3]), ["authorship_yes", "authorship_no"]);
    let card = &sent[4];
    assert_eq!(card["chat_id"], ADMIN_CHAT);
    let body = card["text"].as_str().unwrap();
    assert!(body.starts_with("📨 <b>Новое предложение #1</b>"), "{body}");
    assert!(
        body.contains("@user42") && body.contains("✍️ <b>Авторство:</b> Указать"),
        "{body}"
    );
    assert!(body.contains("a &lt; b &amp; c"), "{body}");
    let s = &submissions(&e).await[0];
    assert!(
        body.ends_with(&format!("\n\n[ID: {}]", &s.submission_id[..8])),
        "{body}"
    );
    assert_eq!(
        buttons(card),
        [
            format!("adm_app_pub:{}", s.submission_id),
            format!("adm_app:{}", s.submission_id),
            format!("adm_rej:{}", s.submission_id),
            format!("adm_blk:{USER}")
        ]
    );
    assert_eq!(texts[5], u.submission_accepted);
    assert_eq!(
        (
            s.status.as_str(),
            s.show_authorship,
            s.message_id_in_admin_chat
        ),
        (status::PENDING, true, Some(123))
    );
    assert!(e.app.db.dialogue(USER).await.unwrap().is_none());
}

#[tokio::test]
async fn formatting_of_user_text_is_kept() {
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    let bold: teloxide::types::Message = serde_json::from_value(message_json(
        2,
        USER,
        json!({"text":"жирный текст","entities":[{"type":"bold","offset":0,"length":6}]}),
    ))
    .unwrap();
    msg(&e, bold).await;
    let (_, draft) = e.app.db.dialogue(USER).await.unwrap().unwrap();
    assert_eq!(draft.unwrap().text.as_deref(), Some("<b>жирный</b> текст"));
}

#[tokio::test]
async fn cancel_clears_state_at_any_step() {
    // U3
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, text(2, USER, "x")).await;
    cb(&e, user_cb(USER, "cancel_submission")).await;
    assert!(e.app.db.dialogue(USER).await.unwrap().is_none());
    let sent = calls_to(&e.server, "SendMessage").await;
    assert_eq!(
        sent.last().unwrap()["text"],
        e.app.texts.user.submission_cancelled
    );
}

#[tokio::test]
async fn blocked_user_gets_plain_alert() {
    // U2, F4, test_blocked_user_cannot_start_submission
    let e = setup().await;
    msg(&e, text(1, USER, "/start")).await;
    e.app.db.set_blocked(USER, true).await.unwrap();
    cb(&e, user_cb(USER, "suggest_content")).await;
    let answers = calls_to(&e.server, "AnswerCallbackQuery").await;
    assert_eq!(answers[0]["show_alert"], true);
    assert_eq!(
        answers[0]["text"],
        "🚫 Внимание\n\nВаш аккаунт был заблокирован модератором."
    );
    assert!(e.app.db.dialogue(USER).await.unwrap().is_none());
}

#[tokio::test]
async fn blocking_can_be_disabled() {
    // M15
    let e = setup_with(&[("ENABLE_BLOCKING", "false")]).await;
    msg(&e, text(1, USER, "/start")).await;
    e.app.db.set_blocked(USER, true).await.unwrap();
    cb(&e, user_cb(USER, "suggest_content")).await;
    assert_eq!(
        e.app.db.dialogue(USER).await.unwrap().unwrap().0,
        Step::WaitingForContent
    );
}

async fn submit_text(e: &Env, id: i32) {
    cb(e, user_cb(USER, "suggest_content")).await;
    msg(e, text(id, USER, "t")).await;
    cb(e, user_cb(USER, "confirm_content")).await;
    cb(e, user_cb(USER, "authorship_no")).await;
}

#[tokio::test]
async fn daily_limit_blocks_the_third_submission() {
    // R1, R2, test_rate_limited_user_cannot_start
    let e = setup().await;
    msg(&e, text(1, USER, "/start")).await;
    submit_text(&e, 2).await;
    submit_text(&e, 3).await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    let answers = calls_to(&e.server, "AnswerCallbackQuery").await;
    let last = answers.last().unwrap();
    assert_eq!(last["show_alert"], true);
    assert!(
        last["text"]
            .as_str()
            .unwrap()
            .starts_with("⚠️ Лимит превышен\n\nВы можете отправить максимум 2 предложения")
    );
    assert_eq!(submissions(&e).await.len(), 2);
    assert_eq!(
        e.app
            .db
            .user(USER)
            .await
            .unwrap()
            .unwrap()
            .total_submissions_count,
        2
    );
}

#[tokio::test]
async fn limit_is_rechecked_when_submitting() {
    // R2: a flow started under the limit cannot exceed it at the last step.
    let e = setup().await;
    msg(&e, text(1, USER, "/start")).await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, text(2, USER, "t")).await;
    cb(&e, user_cb(USER, "confirm_content")).await;
    let (step, draft) = e.app.db.dialogue(USER).await.unwrap().unwrap();
    assert_eq!(step, Step::WaitingForAuthorship);
    let now = time::now();
    for i in 0..2 {
        e.app
            .db
            .create_submission(
                &tg_bot_predlozhka::db::NewSubmission {
                    user_id: USER,
                    user_chat_id: USER,
                    user_message_id: 100 + i,
                    show_authorship: false,
                    text_content: None,
                    media: None,
                    album: None,
                },
                now,
                now - 60,
                2,
            )
            .await
            .unwrap()
            .unwrap();
    }
    // Inserting ends the dialogue; put the half-finished flow back.
    e.app
        .db
        .set_dialogue(USER, step, draft.as_ref())
        .await
        .unwrap();
    cb(&e, user_cb(USER, "authorship_yes")).await;
    let last = calls_to(&e.server, "AnswerCallbackQuery")
        .await
        .pop()
        .unwrap();
    assert_eq!(last["show_alert"], true);
    assert!(last["text"].as_str().unwrap().contains("Лимит превышен"));
    assert_eq!(submissions(&e).await.len(), 2);
    assert!(e.app.db.dialogue(USER).await.unwrap().is_none());
}

#[tokio::test]
async fn unsupported_media_and_oversized_files_are_rejected() {
    // U4, U5, test_receive_oversized_photo_rejected
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    let voice: teloxide::types::Message = serde_json::from_value(message_json(
        2,
        USER,
        json!({"voice":{"file_id":"v","file_unique_id":"v","duration":1}}),
    ))
    .unwrap();
    msg(&e, voice).await;
    msg(&e, photo(3, USER, 201 * 1024 * 1024, json!({}))).await;
    let sent = calls_to(&e.server, "SendMessage").await;
    assert_eq!(sent[1]["text"], e.app.texts.user.unsupported_media);
    assert!(
        sent[2]["text"]
            .as_str()
            .unwrap()
            .contains("Максимальный размер файла: 200 МБ")
    );
    assert_eq!(
        e.app.db.dialogue(USER).await.unwrap().unwrap().0,
        Step::WaitingForContent
    );
}

#[tokio::test]
async fn unknown_file_size_is_accepted() {
    // teloxide reports a missing file_size as u32::MAX; Python treated it as 0.
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    let doc: teloxide::types::Message = serde_json::from_value(message_json(
        2,
        USER,
        json!({"document":{"file_id":"d","file_unique_id":"d"}}),
    ))
    .unwrap();
    msg(&e, doc).await;
    assert_eq!(
        e.app.db.dialogue(USER).await.unwrap().unwrap().0,
        Step::WaitingForConfirmation
    );
}

#[tokio::test]
async fn photo_card_is_copied_with_caption_and_keyboard() {
    // M2, test_present_media_uses_copy_message
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, photo(2, USER, 1000, json!({"caption":"подпись"}))).await;
    cb(&e, user_cb(USER, "confirm_content")).await;
    cb(&e, user_cb(USER, "authorship_no")).await;
    let copies = calls_to(&e.server, "CopyMessage").await;
    assert_eq!(copies.len(), 1);
    let c = &copies[0];
    assert_eq!(
        (
            c["chat_id"].as_i64(),
            c["from_chat_id"].as_i64(),
            c["message_id"].as_i64()
        ),
        (Some(ADMIN_CHAT), Some(USER), Some(2))
    );
    let caption = c["caption"].as_str().unwrap();
    assert!(
        caption.contains("🎭 <b>Авторство:</b> Анонимно")
            && caption.contains("\n\nподпись\n\n[ID: "),
        "{caption}"
    );
    assert_eq!(buttons(c).len(), 4);
    let s = &submissions(&e).await[0];
    assert_eq!(
        (
            s.has_media,
            s.media_type.as_deref(),
            s.media_file_id.as_deref()
        ),
        (true, Some("photo"), Some("photo2"))
    );
}

#[tokio::test]
async fn long_caption_card_is_sent_separately() {
    // F3: header + caption over 1024 characters used to lose the submission.
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(
        &e,
        photo(2, USER, 1000, json!({"caption":"я".repeat(1000)})),
    )
    .await;
    cb(&e, user_cb(USER, "confirm_content")).await;
    cb(&e, user_cb(USER, "authorship_no")).await;
    let copy = &calls_to(&e.server, "CopyMessage").await[0];
    assert!(copy.get("caption").is_none() && copy.get("reply_markup").is_none());
    let card = calls_to(&e.server, "SendMessage")
        .await
        .into_iter()
        .find(|b| b["chat_id"] == ADMIN_CHAT)
        .unwrap();
    assert_eq!(buttons(&card).len(), 4);
    assert_eq!(card["reply_parameters"]["message_id"], 123);
}

#[tokio::test]
async fn album_becomes_one_submission() {
    // U11, N1
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(
        &e,
        photo(
            11,
            USER,
            10,
            json!({"media_group_id":"g1","caption":"<общая>"}),
        ),
    )
    .await;
    msg(&e, photo(12, USER, 10, json!({"media_group_id":"g1"}))).await;
    assert_eq!(
        calls_to(&e.server, "SendMessage").await.len(),
        1,
        "no prompt before the album is complete"
    );
    e.app.finish_album(&e.bot, "g1").await.unwrap();
    let sent = calls_to(&e.server, "SendMessage").await;
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1]["text"], e.app.texts.user.submission_received);
    cb(&e, user_cb(USER, "confirm_content")).await;
    cb(&e, user_cb(USER, "authorship_yes")).await;

    let copies = calls_to(&e.server, "CopyMessages").await;
    assert_eq!(copies[0]["message_ids"], json!([11, 12]));
    assert_eq!(copies[0]["chat_id"], ADMIN_CHAT);
    let card = calls_to(&e.server, "SendMessage")
        .await
        .into_iter()
        .find(|b| b["chat_id"] == ADMIN_CHAT)
        .unwrap();
    assert_eq!(card["reply_parameters"]["message_id"], 201);
    assert_eq!(buttons(&card).len(), 4);
    let s = &submissions(&e).await[0];
    let parts = s.album_parts();
    assert_eq!(
        parts.iter().map(|p| p.file_id.as_str()).collect::<Vec<_>>(),
        ["photo11", "photo12"]
    );
    assert_eq!(parts[0].caption.as_deref(), Some("&lt;общая&gt;"));
    assert_eq!(s.text_content.as_deref(), Some("&lt;общая&gt;"));
    assert_eq!(s.user_message_id, Some(11));
}

#[tokio::test]
async fn album_with_oversized_part_is_rejected_once() {
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, photo(11, USER, 10, json!({"media_group_id":"g2"}))).await;
    msg(
        &e,
        photo(12, USER, 300 * 1024 * 1024, json!({"media_group_id":"g2"})),
    )
    .await;
    e.app.finish_album(&e.bot, "g2").await.unwrap();
    let sent = calls_to(&e.server, "SendMessage").await;
    assert_eq!(sent.len(), 2);
    assert!(
        sent[1]["text"]
            .as_str()
            .unwrap()
            .contains("Файл слишком большой")
    );
    assert_eq!(
        e.app.db.dialogue(USER).await.unwrap().unwrap().0,
        Step::WaitingForContent
    );
}

#[tokio::test]
async fn confirmation_can_be_skipped() {
    // U7 with require_confirmation = false
    let e = setup_with(&[("REQUIRE_CONFIRMATION", "false")]).await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, text(2, USER, "x")).await;
    assert_eq!(
        e.app.db.dialogue(USER).await.unwrap().unwrap().0,
        Step::WaitingForAuthorship
    );
}

#[tokio::test]
async fn messages_in_later_steps_are_ignored() {
    // U10
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, text(2, USER, "first")).await;
    msg(&e, text(3, USER, "second")).await;
    assert_eq!(calls_to(&e.server, "SendMessage").await.len(), 2);
    let (_, draft) = e.app.db.dialogue(USER).await.unwrap().unwrap();
    assert_eq!(draft.unwrap().text.as_deref(), Some("first"));
}

#[tokio::test]
async fn commands_are_not_taken_as_content() {
    // U12, F6
    let e = setup().await;
    cb(&e, user_cb(USER, "suggest_content")).await;
    msg(&e, text(2, USER, "/stats")).await;
    let sent = calls_to(&e.server, "SendMessage").await;
    assert_eq!(
        sent.last().unwrap()["text"],
        "⛔️ Только для администраторов"
    );
    assert_eq!(
        e.app.db.dialogue(USER).await.unwrap().unwrap().0,
        Step::WaitingForContent
    );
}

#[tokio::test]
async fn submission_flow_is_private_only() {
    // U13, F5
    let e = setup().await;
    msg(&e, group_text(1, USER, "/start")).await;
    assert!(calls_to(&e.server, "SendMessage").await.is_empty());
    assert!(e.app.db.user(USER).await.unwrap().is_none());
}

#[tokio::test]
async fn stale_buttons_are_answered_silently() {
    // U10, F7
    let e = setup().await;
    cb(&e, user_cb(USER, "confirm_content")).await;
    cb(&e, user_cb(USER, "authorship_yes")).await;
    let answers = calls_to(&e.server, "AnswerCallbackQuery").await;
    assert_eq!(answers.len(), 2);
    assert!(answers.iter().all(|a| a.get("text").is_none()));
    assert!(submissions(&e).await.is_empty());
}

#[tokio::test]
async fn undelivered_card_tells_the_user_and_keeps_the_submission() {
    // U9: the card could not be sent.
    let e = setup().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/bot123:fake/SendMessage"))
        .and(wiremock::matchers::body_partial_json(
            json!({"chat_id": ADMIN_CHAT}),
        ))
        .respond_with(wiremock::ResponseTemplate::new(400).set_body_json(
            json!({"ok":false,"error_code":400,"description":"Bad Request: chat not found"}),
        ))
        .with_priority(1)
        .mount(&e.server)
        .await;
    submit_text(&e, 2).await;
    let sent = calls_to(&e.server, "SendMessage").await;
    assert_eq!(
        sent.last().unwrap()["text"],
        e.app.texts.user.error_occurred
    );
    let s = &submissions(&e).await[0];
    assert_eq!(
        (s.status.as_str(), s.message_id_in_admin_chat),
        (status::PENDING, None)
    );
}
