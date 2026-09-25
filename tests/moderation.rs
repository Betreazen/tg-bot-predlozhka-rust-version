mod common;
use common::*;
use serde_json::{Value, json};
use tg_bot_predlozhka::model::status;

fn keyboard(id: &str) -> Value {
    json!([[{"text":"a","callback_data":format!("adm_app_pub:{id}")}],
           [{"text":"b","callback_data":format!("adm_app:{id}")},{"text":"c","callback_data":format!("adm_rej:{id}")}],
           [{"text":"d","callback_data":format!("adm_blk:{USER}")}]])
}

async fn pending(e: &Env) -> String {
    submit(e, USER, text(2, USER, "t"), true)
        .await
        .submission_id
}

async fn last_answer(e: &Env) -> Value {
    calls_to(&e.server, "AnswerCallbackQuery")
        .await
        .pop()
        .unwrap()
}

#[tokio::test]
async fn non_admin_cannot_moderate() {
    // M4, test_non_admin_cannot_moderate
    let e = setup().await;
    let id = pending(&e).await;
    for data in [
        format!("adm_rej:{id}"),
        format!("adm_blk:{USER}"),
        format!("adm_conf_pub:{id}"),
    ] {
        cb(&e, admin_cb(USER, &data, text_card(keyboard(&id)))).await;
        let a = last_answer(&e).await;
        assert_eq!(
            (a["text"].as_str(), a["show_alert"].as_bool()),
            (Some("⛔️ Только для администраторов"), Some(true))
        );
    }
    assert_eq!(
        e.app.db.submission(&id).await.unwrap().unwrap().status,
        status::PENDING
    );
    assert!(!e.app.db.user(USER).await.unwrap().unwrap().is_blocked);
}

#[tokio::test]
async fn malformed_or_unknown_submission_ids() {
    // M5
    let e = setup().await;
    cb(
        &e,
        admin_cb(ADMIN, "adm_rej:not-a-uuid", text_card(json!([]))),
    )
    .await;
    assert_eq!(
        last_answer(&e).await["text"],
        "❌ Некорректный идентификатор"
    );
    cb(
        &e,
        admin_cb(
            ADMIN,
            &format!("adm_rej:{}", uuid::Uuid::new_v4()),
            text_card(json!([])),
        ),
    )
    .await;
    assert_eq!(last_answer(&e).await["text"], "❌ Предложка не найдена");
}

#[tokio::test]
async fn approve_publish_asks_for_confirmation_first() {
    // M6, M7
    let e = setup().await;
    let id = pending(&e).await;
    cb(
        &e,
        admin_cb(
            ADMIN,
            &format!("adm_app_pub:{id}"),
            text_card(keyboard(&id)),
        ),
    )
    .await;
    let edit = calls_to(&e.server, "EditMessageReplyMarkup")
        .await
        .pop()
        .unwrap();
    assert_eq!(
        buttons(&edit),
        [format!("adm_conf_pub:{id}"), format!("adm_cancel_pub:{id}")]
    );
    assert_eq!(
        last_answer(&e).await["text"],
        e.app.texts.admin.confirm_approve_publish
    );
    assert_eq!(
        e.app.db.submission(&id).await.unwrap().unwrap().status,
        status::PENDING
    );

    e.app.db.set_blocked(USER, true).await.unwrap();
    cb(
        &e,
        admin_cb(ADMIN, &format!("adm_cancel_pub:{id}"), text_card(json!([]))),
    )
    .await;
    let edit = calls_to(&e.server, "EditMessageReplyMarkup")
        .await
        .pop()
        .unwrap();
    assert_eq!(
        buttons(&edit),
        [
            format!("adm_app_pub:{id}"),
            format!("adm_app:{id}"),
            format!("adm_rej:{id}"),
            format!("adm_unblk:{USER}")
        ]
    );
    assert_eq!(last_answer(&e).await["text"], "❌ Отменено");
}

#[tokio::test]
async fn confirmed_publication_is_scheduled_and_reported() {
    // M9, M12, M13, test_approve_publish_publishes_to_channel
    let e = setup().await;
    let id = pending(&e).await;
    let before = tg_bot_predlozhka::time::now();
    cb(
        &e,
        admin_cb(ADMIN, &format!("adm_conf_pub:{id}"), text_card(json!([]))),
    )
    .await;
    let s = e.app.db.submission(&id).await.unwrap().unwrap();
    assert_eq!(
        (s.status.as_str(), s.moderator_id),
        (status::SCHEDULED, Some(ADMIN))
    );
    let at = s.scheduled_ts.unwrap();
    assert!((before + 120..=before + 122).contains(&at));

    let to_user = calls_to(&e.server, "SendMessage").await.pop().unwrap();
    assert_eq!(to_user["chat_id"], USER);
    assert_eq!(
        to_user["text"],
        e.app.texts.notifications.approved_and_published
    );
    assert_eq!(
        to_user["reply_markup"]["inline_keyboard"][0][0]["text"],
        "📝 Предложить ещё контент"
    );
    assert_eq!(buttons(&to_user), ["suggest_content"]);

    let cleared = calls_to(&e.server, "EditMessageReplyMarkup")
        .await
        .pop()
        .unwrap();
    assert!(cleared.get("reply_markup").is_none());
    let edited = calls_to(&e.server, "EditMessageText").await.pop().unwrap();
    let body = edited["text"].as_str().unwrap();
    assert!(body.starts_with("📨 <b>Новое предложение #1</b>\n\nt\n\n━━━━━━━━━━━━━━━\n✅ <b>Решение:</b> Принято и запланировано к публикации\n👤 <b>Модератор:</b> @user99\n🕒 <b>Время:</b> "), "{body}");
    assert_eq!(edited["parse_mode"], "HTML");
    assert_eq!(
        last_answer(&e).await["text"],
        "✅ Принято и запланировано к публикации"
    );

    cb(
        &e,
        admin_cb(ADMIN, &format!("adm_conf_pub:{id}"), text_card(json!([]))),
    )
    .await;
    let a = last_answer(&e).await;
    assert_eq!(
        (a["text"].as_str(), a["show_alert"].as_bool()),
        (
            Some(e.app.texts.admin.already_processed.as_str()),
            Some(true)
        )
    );
}

#[tokio::test]
async fn approve_only_on_a_media_card_edits_the_caption() {
    // M10
    let e = setup().await;
    let id = pending(&e).await;
    let card = json!({"photo":[{"file_id":"p","file_unique_id":"p","width":1,"height":1}],"caption":"карточка"});
    cb(&e, admin_cb(ADMIN, &format!("adm_app:{id}"), card)).await;
    assert_eq!(
        e.app.db.submission(&id).await.unwrap().unwrap().status,
        status::ACCEPTED_NOT_PUBLISHED
    );
    let edited = calls_to(&e.server, "EditMessageCaption")
        .await
        .pop()
        .unwrap();
    assert!(
        edited["caption"]
            .as_str()
            .unwrap()
            .starts_with("карточка\n\n━━━━━━━━━━━━━━━\n✅ <b>Решение:</b> Принято без публикации")
    );
    assert!(calls_to(&e.server, "EditMessageText").await.is_empty());
    let to_user = calls_to(&e.server, "SendMessage").await.pop().unwrap();
    assert_eq!(to_user["text"], e.app.texts.notifications.approved_only);
    assert_eq!(
        to_user["reply_markup"]["inline_keyboard"][0][0]["text"],
        "📝 Предложить контент"
    );
    assert_eq!(last_answer(&e).await["text"], "✅ Принято без публикации");
}

#[tokio::test]
async fn reject_notifies_the_author() {
    // M11, test_reject_notifies_author
    let e = setup().await;
    let id = pending(&e).await;
    cb(
        &e,
        admin_cb(ADMIN, &format!("adm_rej:{id}"), text_card(json!([]))),
    )
    .await;
    assert_eq!(
        e.app.db.submission(&id).await.unwrap().unwrap().status,
        status::REJECTED
    );
    let to_user = calls_to(&e.server, "SendMessage").await.pop().unwrap();
    assert_eq!(to_user["text"], e.app.texts.notifications.rejected);
    assert_eq!(
        to_user["reply_markup"]["inline_keyboard"][0][0]["text"],
        "🔄 Попробовать снова"
    );
    assert_eq!(last_answer(&e).await["text"], "❌ Отклонено");
    let logged: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_action_logs WHERE action_type = 'reject' AND admin_user_id = 99 AND target_user_id = 42")
        .fetch_one(e.app.db.pool())
        .await
        .unwrap();
    assert_eq!(logged, 1);
}

#[tokio::test]
async fn block_and_unblock_from_the_card() {
    // M14, F8, test_block_user_from_card
    let e = setup().await;
    let id = pending(&e).await;
    cb(
        &e,
        admin_cb(ADMIN, &format!("adm_blk:{USER}"), text_card(keyboard(&id))),
    )
    .await;
    assert!(e.app.db.user(USER).await.unwrap().unwrap().is_blocked);
    assert_eq!(
        calls_to(&e.server, "SendMessage").await.pop().unwrap()["text"],
        e.app.texts.notifications.user_blocked
    );
    assert_eq!(
        last_answer(&e).await["text"],
        "🚫 Пользователь заблокирован"
    );
    let edit = calls_to(&e.server, "EditMessageReplyMarkup")
        .await
        .pop()
        .unwrap();
    assert_eq!(buttons(&edit).last().unwrap(), &format!("adm_unblk:{USER}"));
    assert_eq!(
        edit["reply_markup"]["inline_keyboard"][2][0]["text"],
        e.app.texts.admin.buttons.unblock_user
    );

    cb(
        &e,
        admin_cb(ADMIN, &format!("adm_unblk:{USER}"), text_card(json!([]))),
    )
    .await;
    assert!(!e.app.db.user(USER).await.unwrap().unwrap().is_blocked);
    assert_eq!(
        calls_to(&e.server, "SendMessage").await.pop().unwrap()["text"],
        e.app.texts.notifications.user_unblocked
    );
    assert_eq!(
        last_answer(&e).await["text"],
        "✅ Пользователь разблокирован"
    );
}

#[tokio::test]
async fn blocking_an_unknown_user_fails() {
    let e = setup().await;
    cb(&e, admin_cb(ADMIN, "adm_blk:777", text_card(json!([])))).await;
    let a = last_answer(&e).await;
    assert_eq!(
        (a["text"].as_str(), a["show_alert"].as_bool()),
        (Some("❌ Ошибка блокировки"), Some(true))
    );
}

#[tokio::test]
async fn blocked_status_shows_on_new_cards() {
    // M1: blocked_indicator
    let e = setup().await;
    let s = submit(&e, USER, text(2, USER, "t"), false).await;
    e.app.db.set_blocked(USER, true).await.unwrap();
    tg_bot_predlozhka::admin::present(&e.bot, &e.app, &s)
        .await
        .unwrap();
    let card = calls_to(&e.server, "SendMessage").await.pop().unwrap();
    assert!(
        card["text"]
            .as_str()
            .unwrap()
            .contains("🚫 <b>ПОЛЬЗОВАТЕЛЬ ЗАБЛОКИРОВАН</b>")
    );
    assert_eq!(buttons(&card).last().unwrap(), &format!("adm_unblk:{USER}"));
}
