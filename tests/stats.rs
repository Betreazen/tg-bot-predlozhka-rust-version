mod common;
use common::*;
use serde_json::json;
use tg_bot_predlozhka::{
    db::{AdminDecisions, MonthCounts},
    stats,
    text::Texts,
};

fn texts() -> Texts {
    Texts::load("messages.json".as_ref()).unwrap()
}

#[test]
fn renders_like_python() {
    // T2, T4, test_current_month_counts_and_rates
    let counts = MonthCounts {
        total: 3,
        approved: 2,
        published: 1,
        rejected: 1,
        unique_users: 2,
        new_users: 1,
        blocked_users: 0,
        admins: vec![
            AdminDecisions {
                moderator_id: 5,
                username: Some("mod".into()),
                decisions: 2,
            },
            AdminDecisions {
                moderator_id: 6,
                username: None,
                decisions: 1,
            },
        ],
    };
    assert_eq!(
        stats::render(&texts(), &counts, 2026, 9),
        "📊 <b>Статистика за Сентябрь 2026</b>\n\n📥 Предложено: 3\n✅ Принято: 2\n📢 Опубликовано: 1\n❌ Отклонено: 1\n\n\
         👥 Уникальных пользователей: 2\n🆕 Новых пользователей: 1\n🚫 Заблокированных: 0\n\n\
         📈 <b>Показатели:</b>\n• Одобрение: 66.7%\n• Публикация: 50.0%\n• Отклонение: 33.3%\n\n\
         📋 <b>По администраторам:</b>\n@mod: 2 решений\n@ID:6: 1 решений"
    );
}

#[test]
fn empty_month_has_no_rates_block() {
    // test_empty_month_has_zero_rates
    let out = stats::render(&texts(), &MonthCounts::default(), 2026, 1);
    assert!(out.starts_with("📊 <b>Статистика за Январь 2026</b>"));
    assert!(!out.contains("Показатели") && !out.contains("По администраторам"));
}

#[test]
fn percentages_follow_python_round_and_repr() {
    // round(x, 1) prints `0.0`, but publication_rate is the integer 0 without approvals.
    assert_eq!(
        stats::rates(4, 0, 0, 4),
        ("0.0".into(), "0".into(), "100.0".into())
    );
    // Exact ties round half to even: round(6.25, 1) == 6.2.
    assert_eq!(stats::rates(16, 1, 1, 0).0, "6.2");
    assert_eq!(
        stats::rates(3, 3, 2, 0),
        ("100.0".into(), "66.7".into(), "0.0".into())
    );
}

#[test]
fn navigation_hides_next_for_the_current_month() {
    // T5
    let t = texts();
    let current = stats::keyboard(&t, 2026, 1, (2026, 1));
    let data: Vec<_> = current
        .inline_keyboard
        .concat()
        .into_iter()
        .map(|b| format!("{:?}", b.kind))
        .collect();
    assert_eq!(data.len(), 1);
    assert!(data[0].contains("stats:2025:12"));
    let past = stats::keyboard(&t, 2025, 12, (2026, 1));
    let rows: Vec<Vec<String>> = past
        .inline_keyboard
        .iter()
        .map(|r| r.iter().map(|b| b.text.clone()).collect())
        .collect();
    assert_eq!(
        rows,
        [
            vec![
                "◀️ Предыдущий месяц".to_string(),
                "Следующий месяц ▶️".into()
            ],
            vec!["📅 Текущий месяц".into()]
        ]
    );
    assert!(format!("{:?}", past.inline_keyboard[0][1].kind).contains("stats:2026:1"));
}

#[tokio::test]
async fn stats_command_is_for_admins_only() {
    // T1
    let e = setup().await;
    msg(&e, text(1, USER, "/stats")).await;
    assert_eq!(
        calls_to(&e.server, "SendMessage").await[0]["text"],
        "⛔️ Только для администраторов"
    );
    msg(&e, group_text(2, ADMIN, "/stats")).await;
    let sent = calls_to(&e.server, "SendMessage").await;
    assert!(
        sent[1]["text"]
            .as_str()
            .unwrap()
            .starts_with("📊 <b>Статистика за ")
    );
    assert_eq!(sent[1]["chat_id"], ADMIN_CHAT);
    msg(&e, group_text(3, ADMIN, "/stats@other_bot")).await;
    assert_eq!(
        calls_to(&e.server, "SendMessage").await.len(),
        2,
        "command for another bot"
    );
}

#[tokio::test]
async fn navigation_edits_the_message() {
    // T5
    let e = setup().await;
    cb(&e, admin_cb(ADMIN, "stats:2025:3", json!({"text":"old"}))).await;
    let edit = calls_to(&e.server, "EditMessageText").await.pop().unwrap();
    assert!(
        edit["text"]
            .as_str()
            .unwrap()
            .starts_with("📊 <b>Статистика за Март 2025</b>")
    );
    cb(&e, admin_cb(USER, "stats:2025:3", json!({"text":"old"}))).await;
    assert_eq!(
        calls_to(&e.server, "AnswerCallbackQuery")
            .await
            .pop()
            .unwrap()["text"],
        "⛔️ Только для администраторов"
    );
}
