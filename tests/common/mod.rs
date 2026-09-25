#![allow(dead_code)]
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use teloxide::{
    Bot,
    types::{CallbackQuery, Message},
};
use tg_bot_predlozhka::{bot::App, config::Config, db::Database, text::Texts};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

pub const USER: i64 = 42;
pub const ADMIN: i64 = 99;
pub const CHANNEL: i64 = -1001;
pub const ADMIN_CHAT: i64 = -1002;
pub const ERROR_CHAT: i64 = -1003;

pub struct Env {
    pub _dir: tempfile::TempDir,
    pub server: MockServer,
    pub app: Arc<App>,
    pub bot: Bot,
}

pub fn sent_message(id: i64) -> Value {
    json!({"ok":true,"result":{"message_id":id,"date":0,"chat":{"id":1,"type":"private"},"text":"ok"}})
}

pub async fn mock(server: &MockServer, api_method: &str, body: Value) {
    Mock::given(method("POST"))
        .and(path(format!("/bot123:fake/{api_method}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .with_priority(2)
        .mount(server)
        .await;
}

pub async fn setup() -> Env {
    setup_with(&[]).await
}

pub async fn setup_with(overrides: &[(&str, &str)]) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sent_message(123)))
        .mount(&server)
        .await;
    for m in ["AnswerCallbackQuery"] {
        mock(&server, m, json!({"ok":true,"result":true})).await;
    }
    mock(
        &server,
        "CopyMessages",
        json!({"ok":true,"result":[{"message_id":201},{"message_id":202}]}),
    )
    .await;
    mock(
        &server,
        "SendMediaGroup",
        json!({"ok":true,"result":[sent_message(301)["result"], sent_message(302)["result"]]}),
    )
    .await;
    let data_dir = dir.path().to_str().unwrap().to_owned();
    let mut env: HashMap<&str, &str> = HashMap::from([
        ("BOT_TOKEN", "123:fake"),
        ("CHANNEL_ID", "-1001"),
        ("ADMIN_CHAT_ID", "-1002"),
        ("ERROR_CHAT_ID", "-1003"),
        ("ADMIN_IDS", "99"),
        ("DATA_DIR", &data_dir),
    ]);
    env.extend(overrides.iter().copied());
    let config = Config::parse(|k| env.get(k).map(|v| v.to_string())).unwrap();
    let db = Database::open(&config.data_dir).await.unwrap();
    let texts = Texts::load("messages.json".as_ref()).unwrap();
    let mut app = App::new(config, db, texts);
    app.username = Some("predlozhka_test_bot".into());
    // Tests flush albums explicitly.
    app.album_delay = Duration::from_secs(3600);
    let bot = Bot::new("123:fake").set_api_url(server.uri().parse().unwrap());
    Env {
        _dir: dir,
        server,
        app: Arc::new(app),
        bot,
    }
}

fn from(id: i64) -> Value {
    json!({"id":id,"is_bot":false,"first_name":"Имя","username":format!("user{id}")})
}

fn private(id: i64) -> Value {
    json!({"id":id,"type":"private","first_name":"Имя"})
}

pub fn message_json(id: i32, user: i64, extra: Value) -> Value {
    let mut m =
        json!({"message_id":id,"date":1_780_000_000,"chat":private(user),"from":from(user)});
    m.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    m
}

pub fn text(id: i32, user: i64, text: &str) -> Message {
    serde_json::from_value(message_json(id, user, json!({"text":text}))).unwrap()
}

pub fn group_text(id: i32, user: i64, text: &str) -> Message {
    serde_json::from_value(json!({"message_id":id,"date":0,"chat":{"id":ADMIN_CHAT,"type":"supergroup","title":"a"},"from":from(user),"text":text})).unwrap()
}

pub fn photo(id: i32, user: i64, size: u64, extra: Value) -> Message {
    let mut m = message_json(
        id,
        user,
        json!({"photo":[{"file_id":"small","file_unique_id":"s","width":1,"height":1,"file_size":1},
                        {"file_id":format!("photo{id}"),"file_unique_id":format!("u{id}"),"width":9,"height":9,"file_size":size}]}),
    );
    m.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(m).unwrap()
}

pub fn callback(user: i64, data: &str, message: Value) -> CallbackQuery {
    serde_json::from_value(
        json!({"id":"cb","from":from(user),"chat_instance":"c","data":data,"message":message}),
    )
    .unwrap()
}

/// A callback pressed on a message in the user's private chat.
pub fn user_cb(user: i64, data: &str) -> CallbackQuery {
    callback(
        user,
        data,
        json!({"message_id":500,"date":1,"chat":private(user),"text":"x"}),
    )
}

/// A callback pressed on the moderation card in the admin chat.
pub fn admin_cb(user: i64, data: &str, card: Value) -> CallbackQuery {
    let mut m =
        json!({"message_id":123,"date":1,"chat":{"id":ADMIN_CHAT,"type":"supergroup","title":"a"}});
    m.as_object_mut()
        .unwrap()
        .extend(card.as_object().unwrap().clone());
    callback(user, data, m)
}

/// `(method, body)` of every Bot API call so far.
pub async fn calls(server: &MockServer) -> Vec<(String, Value)> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| {
            let name = r.url.path().rsplit('/').next().unwrap().to_owned();
            let body = serde_json::from_slice(&r.body).unwrap_or_else(|_| multipart(&r.body));
            (name, body)
        })
        .collect()
}

pub async fn calls_to(server: &MockServer, api_method: &str) -> Vec<Value> {
    calls(server)
        .await
        .into_iter()
        .filter(|(m, _)| m == api_method)
        .map(|(_, b)| b)
        .collect()
}

pub fn buttons(body: &Value) -> Vec<String> {
    body["reply_markup"]["inline_keyboard"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|row| row.as_array().unwrap().iter())
        .map(|b| b["callback_data"].as_str().unwrap_or_default().to_owned())
        .collect()
}

pub async fn msg(e: &Env, m: Message) {
    tg_bot_predlozhka::bot::handle_message(e.bot.clone(), m, e.app.clone())
        .await
        .unwrap();
}

pub async fn cb(e: &Env, c: CallbackQuery) {
    tg_bot_predlozhka::bot::handle_callback(e.bot.clone(), c, e.app.clone())
        .await
        .unwrap();
}

pub async fn all_submissions(e: &Env) -> Vec<tg_bot_predlozhka::model::Submission> {
    sqlx::query_as("SELECT * FROM submissions ORDER BY submission_ts, rowid")
        .fetch_all(e.app.db.pool())
        .await
        .unwrap()
}

/// Drives the user flow with `content` and returns the stored submission.
pub async fn submit(
    e: &Env,
    user: i64,
    content: Message,
    authorship: bool,
) -> tg_bot_predlozhka::model::Submission {
    cb(e, user_cb(user, "suggest_content")).await;
    msg(e, content).await;
    cb(e, user_cb(user, "confirm_content")).await;
    cb(
        e,
        user_cb(
            user,
            if authorship {
                "authorship_yes"
            } else {
                "authorship_no"
            },
        ),
    )
    .await;
    all_submissions(e).await.pop().expect("submission created")
}

pub fn text_card(keyboard: Value) -> Value {
    json!({"text":"📨 Новое предложение #1\n\nt","entities":[{"type":"bold","offset":3,"length":20}],"reply_markup":{"inline_keyboard":keyboard}})
}

/// teloxide sends some methods (sendMediaGroup) as multipart forms.
fn multipart(body: &[u8]) -> Value {
    let body = String::from_utf8_lossy(body);
    let mut fields = serde_json::Map::new();
    for part in body.split("\r\n--") {
        let Some((head, value)) = part.split_once("\r\n\r\n") else {
            continue;
        };
        let Some(name) = head
            .split("name=\"")
            .nth(1)
            .and_then(|n| n.split('"').next())
        else {
            continue;
        };
        let value = value.trim_end_matches("\r\n");
        fields.insert(
            name.to_owned(),
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned())),
        );
    }
    Value::Object(fields)
}
