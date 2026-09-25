# Спецификация Rust-версии

Статус: **одобрено 2026-09-25**. Поведение Python-версии и пометки `=`/`fix`/`new`
перечислены в [PARITY.md](PARITY.md); здесь — решения, которые нужно принять до кода.

## Цель

Заменить Python-стек (бот + Postgres + Redis в Docker) одним Rust-процессом под systemd
с SQLite. Пользователи и админы не должны заметить подмену: те же тексты, кнопки,
callback_data, лимиты и задержки. Все данные Postgres переносятся.

## Причина высокого CPU Python-версии (найдена)

Сам процесс бота за 11,5 суток потратил 125 CPU-с (0,013 % ядра). Остальные ~14 500 CPU-с
cgroup контейнера — это `HEALTHCHECK` из Dockerfile: каждые 30 с Docker запускает
`python -m bot.utils.health`. Каждый прогон поднимает интерпретатор, импортирует
SQLAlchemy/pydantic/redis и открывает соединения с Postgres и Redis. Замер на сервере:
**0,419 CPU-с на прогон**, 0,419 / 30 = 1,40 % ядра, наблюдаемое среднее 1,48 %. Часть CPU
Postgres и Redis тоже уходит на эти подключения и на их собственные healthcheck каждые 10 с.

Вывод для Rust: никаких периодических exec-проверок. За живостью следит systemd
(`Restart=always`), фоновые задачи — событийные, без опроса по таймеру.

## Архитектура

Один процесс, `tokio` current_thread, `teloxide` 0.17 long polling (таймаут 10 с — меньше
17-секундного HTTP-дедлайна teloxide, урок эталона). Скачивания и загрузки файлов нет: контент
пересылается по `message_id`/`file_id`, поэтому второй HTTP-клиент с долгим таймаутом не нужен.

| Модуль | Ответственность |
|---|---|
| `main.rs` | CLI, логирование, запуск диспетчера и планировщика, SIGTERM, маскировка токена |
| `config.rs` | Разбор переменных окружения (`trim`, CRLF, кавычки), без `Debug` |
| `text.rs` | Каталог `messages.json`, проверка всех ключей при старте, подстановка `{name}` |
| `db.rs` | SQLite: открытие, миграция, запросы |
| `user.rs` | Поток подачи: FSM, черновик, сборка альбома, лимит |
| `admin.rs` | Карточка модерации, решения, блокировка |
| `publish.rs` | Планировщик публикаций, повторы, восстановление при запуске |
| `stats.rs` | `/stats`, форматирование, навигация |
| `import.rs` | Импорт выгрузки Postgres |

Оценка объёма: 2–2,5 тыс. строк Rust без тестов.

### Крейты

Как в эталоне: `teloxide` 0.17 (`rustls`, без default), `tokio`, `sqlx` 0.8 (`sqlite`,
`runtime-tokio`), `serde`/`serde_json`, `tracing`/`tracing-subscriber`, `anyhow`, `uuid` (v4).
Новый: **`jiff`** — часовой пояс для суточного лимита и границ месяца в статистике (настройка
`TIMEZONE`, как в `config.json`; база tz — системная `/usr/share/zoneinfo`, на Windows —
встроенная). Для тестов: `wiremock`, `tempfile`, `tokio/test-util`.

### Хранилище: SQLite вместо Postgres + Redis

Файл `DATA_DIR/bot.db`, WAL, `synchronous=FULL`, одно соединение (нагрузка — единицы
запросов в минуту). Время — целые секунды Unix (UTC).

```sql
users(user_id INTEGER PRIMARY KEY, username, first_name, last_name,
      is_blocked INTEGER NOT NULL DEFAULT 0, admin_note TEXT,
      total_submissions_count INTEGER NOT NULL DEFAULT 0 CHECK (total_submissions_count >= 0),
      registration_ts INTEGER NOT NULL, last_interaction_ts INTEGER NOT NULL)

submissions(submission_id TEXT PRIMARY KEY,           -- uuid, как в Python
      user_id INTEGER NOT NULL REFERENCES users ON DELETE CASCADE,
      submission_ts INTEGER NOT NULL,
      status TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','published',
             'accepted_not_published','publication_failed','scheduled','publishing')),
      moderator_id INTEGER, decision_ts INTEGER, show_authorship INTEGER NOT NULL,
      message_id_in_admin_chat INTEGER, message_id_in_channel INTEGER,
      user_message_id INTEGER, user_chat_id INTEGER,
      has_media INTEGER NOT NULL, media_type TEXT, media_file_id TEXT,
      text_content TEXT,                                -- HTML (fix F2)
      album TEXT,                                       -- JSON частей альбома (new N1)
      scheduled_ts INTEGER, publication_error_message TEXT,
      publication_retry_count INTEGER NOT NULL DEFAULT 0)

admin_action_logs(log_id INTEGER PRIMARY KEY AUTOINCREMENT, action_type TEXT NOT NULL,
      admin_user_id INTEGER NOT NULL, target_user_id INTEGER,
      submission_id TEXT REFERENCES submissions ON DELETE CASCADE,
      action_ts INTEGER NOT NULL, additional_context TEXT)

dialogues(user_id INTEGER PRIMARY KEY, state TEXT NOT NULL, draft TEXT)  -- вместо FSM в Redis
```

Индексы — те же, что в Alembic-миграции. Схема версионируется `PRAGMA user_version`.

Что было в Redis и куда уходит:

| Redis | Rust |
|---|---|
| FSM `fsm:<chat>:<user>:state/data` | таблица `dialogues`: переживает рестарт, как и в Redis |
| Счётчик `submission_count:<user>:<дата>` | подсчёт предложений пользователя с локальной полуночи в той же транзакции, что и вставка |
| Блокировка `decision_lock:<uuid>` | атомарный `UPDATE … WHERE status = 'pending'` |

### Перенос данных

Сейчас в Postgres: `users` 13, `submissions` 0, `admin_action_logs` 0. В Redis 4 ключа — только
незавершённые диалоги (`fsm:*:state`), счётчиков лимита нет.

1. После остановки Python-бота (Postgres ещё работает) каждая таблица выгружается в JSON:
   `psql -Atc "select coalesce(json_agg(t), '[]') from <table> t"` → `users.json`,
   `submissions.json`, `admin_action_logs.json`. Сторонних форматов и CSV-парсера не нужно.
2. `bot --import <dir>` в одной транзакции вставляет всё в SQLite, отказывается работать с
   непустой БД, переводит время в Unix-секунды, экранирует старый `text_content` в HTML,
   сохраняет `log_id`. Печатает только счётчики: строки по таблицам, заблокированные, заметки,
   сумму `total_submissions_count`, статусы.
3. Скрипт переключения считает те же агрегаты в Postgres и сравнивает. Расхождение — откат.
4. Незавершённые диалоги (4 шт.) не переносятся, как и в эталоне: пользователь нажмёт кнопку
   ещё раз. Суточных счётчиков нет, переносить нечего.

### Публикации и восстановление

Одна фоновая задача-планировщик. Берёт из БД ближайшее `scheduled_ts` среди `scheduled` и
спит до него или до сигнала `Notify` (новое решение). Когда публикаций нет, она ждёт сигнала
бесконечно и **CPU в простое не тратит**.

Публикация «не более одного раза» (fix P6):

1. `UPDATE status = 'publishing' WHERE status = 'scheduled'` — если строк 0, выходим (P1);
2. отправка в канал;
3. успех → `published` + `message_id_in_channel`; ошибка → `retry_count + 1` и либо
   `scheduled` с `scheduled_ts = now + retry_delay`, либо `publication_failed` +
   `admin.publication_error` (P5).

При запуске: `publishing` → `publication_failed` с текстом «бот перезапустился во время
публикации — проверьте канал» и сообщение админам (дубль невозможен, потеря видна);
`approved` → `scheduled` через `delay_minutes` (S1); `pending` без карточки → карточка
отправляется заново (F9); `pending` > 10 → предупреждение (S3).

### Альбомы (new N1)

В Python альбом ломается (U11). В Rust:

- Части с одним `media_group_id` в шаге «жду контент» копятся 1,5 с после первой части
  (Telegram присылает альбом одной пачкой), потом становятся одним черновиком: порядок по
  `message_id`, каждая часть с типом, `file_id` и своей подписью в HTML. Размер проверяется
  у каждой части. Пользователь получает один запрос подтверждения.
- Админ-чат: `copyMessages` (альбом остаётся альбомом, подписи и порядок сохраняются), затем
  карточка текстом с кнопками ответом на первую часть. Подвал решения дописывается к карточке.
- Канал: `sendMediaGroup` по сохранённым `file_id` и подписям. Авторство, подвал и хэштеги
  дописываются к подписи первой части с подписью, а если подписей нет — к первой части.

### Исправления ошибок Python-версии

Все исправления мелкие, каждое закрывается тестом. Внешний вид для пользователей не меняется,
кроме пунктов с пометкой «видно».

| # | Ошибка в Python | Исправление |
|---|---|---|
| F1 | Текстовое предложение публикуется через `copyMessage` c `caption`, а у текста подписи нет — авторство, подвал и хэштеги теряются (проверю на тестовом боте). Если копирование медиа не удалось, в канал уходит только текст без медиа | Текст → `sendMessage`; при ошибке медиа — повтор, а не «текст вместо медиа». **Видно:** у текстовых постов появляется «Автор:» |
| F2 | Текст пользователя вставляется в HTML без экранирования: `<` или `&` ломают карточку (предложение не доходит до админов). Разметка пользователя (жирный, ссылки) теряется в карточке и в канале | Текст хранится как HTML из entities (`html_text()`); разметка сохраняется, спецсимволы экранируются |
| F3 | Подпись-карточка к медиа длиннее 1024 символов → `copyMessage` падает, предложение не доходит | Медиа копируется без замены подписи, карточка идёт отдельным сообщением |
| F4 | В alert'ах видны теги: `🚫 <b>Внимание</b>…`, `⚠️ <b>Лимит превышен</b>…` | Теги убираются из текста alert'ов. **Видно** |
| F5 | Подача работает в группах, в том числе в админ-чате | Подача и `/start` только в личке; `/stats` — где угодно, как было |
| F6 | `/stats` или другая команда в шаге «жду контент» становится предложением | Команды обрабатываются раньше состояния |
| F7 | Кнопки из старых сообщений без состояния — бесконечные «часики» | Пустой `answerCallbackQuery` |
| F8 | После блокировки кнопка на карточке остаётся «Заблокировать» | Кнопка меняется на противоположную. **Видно** |
| F9 | Карточка, не дошедшая до админов, не показывается никогда (хотя комментарий обещает) | Повторная отправка при запуске |

Не меняется (паритет): время на карточке и в подвале решения — UTC, как в Python;
уведомление «опубликовано» уходит пользователю в момент решения, а не публикации;
`/stats` доступна админам в любом чате.

### Конфигурация

Переменные окружения. `.env` бинарник сам не читает — его подгружает systemd `EnvironmentFile`.

| Переменная | По умолчанию | Откуда в Python |
|---|---|---|
| `BOT_TOKEN`, `CHANNEL_ID`, `ADMIN_CHAT_ID`, `ADMIN_IDS` | обязательны | `.env` |
| `ERROR_CHAT_ID` | = `ADMIN_CHAT_ID` | `.env` |
| `DATA_DIR` | `./data` | — |
| `MESSAGES_PATH` | `messages.json` | `config/messages.json` |
| `SUBMISSIONS_PER_DAY` | 2 | `rate_limits.submissions_per_day` |
| `TIMEZONE` | `Europe/Moscow` | `rate_limits.timezone` |
| `PUBLICATION_DELAY_MINUTES` | 2 | `publication.delay_minutes` |
| `MAX_FILE_SIZE_MB` | 200 | `publication.max_file_size_mb` |
| `FOOTER_TEXT` | пусто = выкл. | `include_footer` + `footer_text` |
| `HASHTAGS` | пусто = выкл. | `include_hashtags` + `hashtags` |
| `REQUIRE_CONFIRMATION`, `ENABLE_BLOCKING` | `true` | `features.*` |
| `MAX_RETRY_ATTEMPTS`, `RETRY_DELAY_SECONDS` | 2, 30 | `error_handling.*` |
| `RUST_LOG` | `info` | `LOG_LEVEL` |

Значения на сервере совпадают со значениями по умолчанию (`config/` на сервере не отличается
от репозитория), поэтому в `bot.env` переносятся только токен, чаты и `ADMIN_IDS`.

### CLI

`bot` — запуск; `bot --check` — конфигурация, тексты, миграция и восстановление без Telegram;
`bot --import <dir>` — импорт выгрузки; `bot --version`. Экземпляр держит блокировку файла
`DATA_DIR/.instance.lock`: второй процесс на тех же данных не запустится.

## Тестирование

TDD. Telegram — `wiremock` (как `tests/handlers.rs` эталона): проверяются исходящие запросы.
Каждая строка PARITY.md получает Rust-тест. Отдельно: CRLF и кавычки в env, границы суток по
Москве, формат процентов (`50.0` против `0`), импорт на выгрузке с реальной структурой Postgres,
at-most-once, альбом, рестарт с `scheduled`/`publishing`/`approved`. Порог:
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, все тесты, `cargo audit`.

## Развёртывание

Как у эталона: бинарник из build-стадии Dockerfile (`rust:1.98.1-bookworm`), пользователь
`tg-bot-predlozhka`, `/opt/tg-bot-predlozhka/{bot,messages.json}`, `/var/lib/tg-bot-predlozhka`,
`/etc/tg-bot-predlozhka/bot.env`, unit с hardening и лимитами (`MemoryMax=128M`,
`CPUQuota=50%`). Переключение одним скриптом с автооткатом: бэкап (`pg_dump`, `.env`, `logs/`)
→ `docker compose stop bot` → выгрузка JSON → `--import` → сверка агрегатов → `--check` →
`systemctl enable --now` → проверки. Python-стек удаляется только после отдельного
подтверждения.

## Принятые решения (2026-09-25)

- **Q1.** Исправления F1–F9 и альбомы N1 — одобрены.
- **Q2.** Ошибки публикации и тревога «проверьте канал» идут в `ERROR_CHAT_ID`; если он не
  задан — в админ-чат. Переменная необязательна.
- **Q3.** При запуске одно сообщение `bot_started`.
- **Q4.** Чтение агрегатов боевой БД разрешено. Снято 2026-09-25: пользователей 13,
  заблокированных 0, заметок 0, сумма `total_submissions_count` 0.
