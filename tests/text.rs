use tg_bot_predlozhka::text::{Texts, fill, plain};

#[test]
fn shipped_catalog_loads_with_python_texts() {
    let t = Texts::load("messages.json".as_ref()).unwrap();
    assert_eq!(t.user.welcome.button, "📝 Предложить контент");
    assert_eq!(t.admin.buttons.approve_publish, "✅ Принять и опубликовать");
    assert_eq!(t.statistics.navigation.prev_month, "◀️ Предыдущий месяц");
    assert!(t.admin.bot_started.starts_with("🤖"));
}

#[test]
fn missing_key_is_rejected_at_startup() {
    let json = std::fs::read_to_string("messages.json").unwrap();
    let broken = json.replace("\"rejected\":", "\"rejected_typo\":");
    let err = Texts::parse(&broken).err().unwrap();
    assert!(format!("{err:#}").contains("rejected"), "{err:#}");
}

#[test]
fn fill_substitutes_in_one_pass() {
    assert_eq!(fill("{a} и {b}", &[("a", "{b}"), ("b", "2")]), "{b} и 2");
    assert_eq!(fill("лимит {limit}", &[("limit", "2")]), "лимит 2");
    assert_eq!(fill("{unknown} {", &[]), "{unknown} {");
}

#[test]
fn plain_strips_tags_for_alerts() {
    assert_eq!(
        plain("⚠️ <b>Лимит превышен</b>\n\nМаксимум 2 &lt;в день&gt; &amp; всё"),
        "⚠️ Лимит превышен\n\nМаксимум 2 <в день> & всё"
    );
}
