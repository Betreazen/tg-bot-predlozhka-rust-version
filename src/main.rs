use anyhow::{Context, Result, bail};
use std::{fs::File, path::Path, sync::Arc, time::Duration};
use teloxide::{prelude::*, types::AllowedUpdate, update_listeners::Polling};
use tg_bot_predlozhka::{
    bot::{App, handle_callback, handle_message},
    config::Config,
    db::Database,
    import, publish,
    text::Texts,
};

// Bot::new has a 17-second HTTP deadline; the long-poll wait must stay below it.
const POLLING_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_DRAIN: Duration = Duration::from_secs(30);

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let token = std::env::var("BOT_TOKEN").unwrap_or_default();
    if let Err(error) = run().await {
        let message = format!("{error:#}");
        let token = token.trim().trim_matches(['"', '\'']);
        eprintln!(
            "{}",
            if token.is_empty() {
                message
            } else {
                message.replace(token, "[REDACTED]")
            }
        );
        std::process::exit(1);
    }
}

enum Mode {
    Run,
    Check,
    Import(String),
}

async fn run() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new("info"))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => Mode::Run,
        ["--version"] => {
            println!("tg-bot-predlozhka {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        ["--check"] => Mode::Check,
        ["--import", dir] => Mode::Import((*dir).to_owned()),
        _ => bail!("usage: tg-bot-predlozhka [--check | --import <dir> | --version]"),
    };

    let config = Config::from_env()?;
    let texts = Texts::load(&config.messages_path)?;
    let _instance = lock_instance(&config.data_dir)?;
    let db = Database::open(&config.data_dir).await?;
    match mode {
        Mode::Check => {
            db.check().await?;
            db.close().await;
            println!("configuration, texts and database: ok");
            Ok(())
        }
        Mode::Import(dir) => {
            let summary = import::run(&db, Path::new(&dir)).await?;
            db.close().await;
            println!("{summary}");
            Ok(())
        }
        Mode::Run => serve(config, db, texts).await,
    }
}

/// Two processes on one database would also poll with one token (409 Conflict).
fn lock_instance(data_dir: &Path) -> Result<File> {
    std::fs::create_dir_all(data_dir).with_context(|| format!("create {}", data_dir.display()))?;
    let file = File::create(data_dir.join(".instance.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(_) => bail!("another instance is using {}", data_dir.display()),
    }
}

async fn serve(config: Config, db: Database, texts: Texts) -> Result<()> {
    let bot = Bot::new(&config.token);
    let me = bot.get_me().await.context("getMe")?;
    let mut app = App::new(config, db, texts);
    app.username = me.username.clone();
    let app = Arc::new(app);

    publish::startup(&bot, &app).await?;
    let worker = tokio::spawn(publish::run(bot.clone(), app.clone()));

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(handle_message))
        .branch(Update::filter_callback_query().endpoint(handle_callback));
    let errors_app = app.clone();
    let mut dispatcher = Dispatcher::builder(bot.clone(), handler)
        .dependencies(dptree::deps![app.clone()])
        .worker_queue_size(16)
        .error_handler(Arc::new(move |error: anyhow::Error| {
            let app = errors_app.clone();
            async move {
                tracing::error!(error = %app.redact(&format!("{error:#}")), "update failed");
            }
        }))
        .build();
    let listener = Polling::builder(bot.clone())
        .timeout(POLLING_TIMEOUT)
        .limit(50)
        .allowed_updates(vec![AllowedUpdate::Message, AllowedUpdate::CallbackQuery])
        .build();
    let shutdown = dispatcher.shutdown_token();
    // reqwest errors carry the URL with the token, so the cause is not logged.
    let polling_errors = Arc::new(|_error: teloxide::RequestError| async {
        tracing::warn!("Telegram polling failed; retrying with backoff");
    });
    tracing::info!(version = env!("CARGO_PKG_VERSION"), bot = ?me.username, "bot started");
    let polling = dispatcher.try_dispatch_with_listener(listener, polling_errors);
    tokio::pin!(polling);
    let result = tokio::select! {
        r = &mut polling => r.context("dispatcher stopped"),
        _ = stop_signal() => {
            let _drain = shutdown.shutdown();
            tokio::time::timeout(SHUTDOWN_DRAIN, &mut polling)
                .await
                .context("shutdown drain timeout")?
                .context("dispatcher stopped")
        }
    };
    worker.abort();
    let _ = worker.await;
    app.db.close().await;
    tracing::info!("bot stopped");
    result
}

async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
