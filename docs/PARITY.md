# Соответствие Python-версии

Карта поведения Python-бота [tg-bot-predlozhka](https://github.com/Betreazen/tg-bot-predlozhka)
(коммит `12eccfe`) и того, как оно переносится в Rust. Ссылки вида `user_handlers.py:57`
ведут на файлы в `bot/`; тесты — в `tests/` того же репозитория.

Пометки в колонке «Rust»:

- **=** — переносится без изменений (тексты, кнопки, порядок действий);
- **fix** — в Python это ошибка, в Rust исправляется (список согласуется в спецификации);
- **new** — в Python нет, в Rust добавляется;
- **—** — в Python объявлено, но не подключено к интерфейсу; не переносится.

Колонка «Rust-тест» заполняется по мере реализации.

## Пользователь

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| U1 | `/start`: сброс состояния, создание/обновление пользователя (поля профиля не затираются `None`), приветствие `welcome.title + "\n\n" + welcome.text`, кнопка `welcome.button` → `suggest_content` | `user_handlers.py:36`, `user_service.py:22`; `test_user_flow::test_start_creates_user_and_greets`, `test_user_service::test_create_then_update_does_not_clobber_with_none` | = | |
| U2 | `suggest_content`: заблокированному — alert `notifications.user_blocked`; при исчерпанном лимите — alert `user.limit_exceeded` c `{limit}`; иначе состояние `waiting_for_content`, сообщение `submission_prompt` с кнопкой `submission_cancel_button` → `cancel_submission` | `user_handlers.py:72`; `test_blocked_user_cannot_start_submission`, `test_rate_limited_user_cannot_start` | = (alert без HTML-тегов — fix F4) | |
| U3 | `cancel_submission` в любом шаге подачи: сброс состояния, сообщение `submission_cancelled` | `user_handlers.py:117` | = | |
| U4 | Приём контента: текст, фото, видео, документ (в т.ч. GIF — он приходит и как документ), аудио. Прочее (голосовое, кружок, стикер, опрос…) — `unsupported_media` | `user_handlers.py:148` | = | |
| U5 | Размер файла > `max_file_size_mb` (200) — `file_too_large` c `{max_size}`; размер фото — по последнему (крупнейшему) варианту; неизвестный размер считается 0 | `user_handlers.py:164`; `test_media_submission::test_receive_oversized_photo_rejected` | = | |
| U6 | Черновик: `message_id`, `chat_id`, `text = text or caption` (без разметки), `has_media` | `user_handlers.py:182`; `test_receive_photo_marks_has_media` | fix F2: текст хранится как HTML с сохранением разметки | |
| U7 | `require_confirmation=true`: сообщение `submission_received` + кнопки `confirm_button` → `confirm_content`, `cancel_button` → `cancel_submission`; иначе сразу вопрос об авторстве | `user_handlers.py:190` | = | |
| U8 | Вопрос `authorship_question`, кнопки `authorship_yes` / `authorship_no` (по одной в ряд) | `user_handlers.py:132` | = | |
| U9 | Выбор авторства: повторная проверка блокировки (alert + сброс), атомарная проверка лимита (alert `limit_exceeded` + сброс), создание предложения `pending`, `total_submissions_count + 1`, отправка карточки админам; ответ `submission_accepted` или `error_occurred`, если карточка не дошла | `user_handlers.py:221`; `test_full_submission_reaches_moderators` | = | |
| U10 | Сообщения в шагах «подтверждение» и «авторство» игнорируются; кнопки из старых шагов без состояния не обрабатываются (крутится «часики») | aiogram без подходящего обработчика | = (fix F7: старые кнопки гасятся пустым ответом) | |
| U11 | Альбом: каждая часть обрабатывается отдельно, пользователь получает несколько запросов подтверждения, в предложение попадает одно сообщение | нет обработки `media_group_id` | new N1: альбом собирается целиком | |
| U12 | Команда, отправленная в шаге «жду контент» (кроме `/start`), становится содержимым предложения | порядок роутеров `main.py:98` | fix F6: команды обрабатываются раньше состояния | |
| U13 | Поток подачи работает и в группах (состояние по паре чат+пользователь) | aiogram FSM | fix F5: подача только в личке | |

## Лимит подач

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| R1 | `submissions_per_day` (2) в сутки по часовому поясу `rate_limits.timezone` (Europe/Moscow), сброс в локальную полночь | `rate_limit.py`; `test_try_acquire_enforces_daily_limit` | = (счёт по таблице предложений в SQLite, без Redis) | |
| R2 | Предварительная проверка при нажатии кнопки, окончательная — атомарно при создании; отказ не расходует слот | `rate_limit.py:96`; `test_rejected_acquire_rolls_back_counter`, `test_check_limit_reports_state` | = (проверка и вставка в одной транзакции) | |
| R3 | При недоступном Redis — подсчёт по БД | `test_db_fallback_when_redis_down` | не нужно: Redis нет, БД — единственный источник | |

## Модерация

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| M1 | Карточка в `ADMIN_CHAT_ID`: `admin.submission_header` (`{submission_number}` = `{total_submissions}` = счётчик пользователя, `{user_info}` = `@username` или `ID: <id>`, `{timestamp}` = время подачи UTC `%Y-%m-%d %H:%M`, `{blocked_status}`, `{note_section}` всегда пусто, `{authorship_info}`) + `"\n\n" + текст` + `"\n\n[ID: <8 символов uuid>]"` | `admin_handlers.py:172`; `test_present_text_submission_sends_card` | = | |
| M2 | Медиа — `copyMessage` с подписью-карточкой и кнопками; текст — `sendMessage` | `admin_handlers.py:228`; `test_present_media_uses_copy_message` | = для одиночных; new N1 для альбомов; fix F3 при подписи > 1024 символов | |
| M3 | Кнопки: `approve_publish` (`adm_app_pub:<uuid>`), ряд `approve_only` (`adm_app:`) + `reject` (`adm_rej:`), ряд `block_user`/`unblock_user` (`adm_blk:<user_id>`/`adm_unblk:`) | `admin_handlers.py:47` | = (те же callback_data) | |
| M4 | Любая админ-кнопка от не-админа — alert `⛔️ Только для администраторов`. Админы — `ADMIN_IDS` (разделители `,` и `;`, пробелы допустимы, мусор — ошибка запуска) | `admin_handlers.py:284`, `config.py:122`; `test_non_admin_cannot_moderate`, `test_config.py` | = | |
| M5 | Битый uuid — alert `❌ Некорректный идентификатор`; нет в БД — `❌ Предложка не найдена` | `admin_handlers.py:93` | = | |
| M6 | «Принять и опубликовать» → клавиатура заменяется на `confirm` (`adm_conf_pub:`) / `cancel` (`adm_cancel_pub:`), toast `confirm_approve_publish` | `admin_handlers.py:276` | = | |
| M7 | `adm_cancel_pub` → исходная клавиатура (с актуальной кнопкой блокировки), toast `❌ Отменено` | `admin_handlers.py:391` | = | |
| M8 | Решение только из `pending`; иначе alert `already_processed`. Параллельный клик в пределах блокировки Redis — alert `processing_by_another` | `decision_manager.py:57`; `test_second_decision_raises_already_decided`, `test_lock_held_raises_lock_not_acquired` | = по сути: атомарный `UPDATE … WHERE status='pending'`; второй клик всегда получает `already_processed` | |
| M9 | Подтверждённая публикация: статус `approved` → `scheduled` через `delay_minutes` (2), уведомление `approved_and_published` с кнопкой `📝 Предложить ещё контент`, подвал решения `Принято и запланировано к публикации`, toast `✅ Принято и запланировано к публикации` | `admin_handlers.py:318`; `test_approve_publish_publishes_to_channel` | = | |
| M10 | «Принять»: `accepted_not_published`, уведомление `approved_only` + `📝 Предложить контент`, подвал `Принято без публикации`, toast `✅ Принято без публикации` | `admin_handlers.py:431` | = | |
| M11 | «Отклонить»: `rejected`, уведомление `rejected` + `🔄 Попробовать снова`, подвал `Отклонено`, toast `❌ Отклонено` | `admin_handlers.py:485`; `test_reject_notifies_author` | = | |
| M12 | Подвал `admin.decision_made` (`{decision}`, `{moderator}` = username или id, `{timestamp}` = текущее время сервера (UTC) `%Y-%m-%d %H:%M`): клавиатура удаляется, к тексту/подписи карточки дописывается подвал; разметка карточки теряется | `admin_handlers.py:117` | = (fix F2: разметка сохраняется) | |
| M13 | Каждое решение пишется в `admin_action_logs` (`approve_publish`/`approve_only`/`reject`) | `decision_manager.py:108`; `test_make_decision_approves_and_logs` | = | |
| M14 | Блокировка/разблокировка с карточки: флаг в `users`, уведомление `user_blocked`/`user_unblocked`, toast `🚫 Пользователь заблокирован` / `✅ Пользователь разблокирован`; неизвестный пользователь — alert `❌ Ошибка блокировки`/`❌ Ошибка разблокировки`. Клавиатура карточки не меняется, в журнал не пишется | `admin_handlers.py:539`; `test_block_user_from_card` | = (fix F8: кнопка на карточке меняется на противоположную) | |
| M15 | `enable_blocking=false` отключает проверку блокировки у пользователя | `user_handlers.py:16` | = | |

## Публикация

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| P1 | Через `delay_minutes` проверка, что статус всё ещё `scheduled`, иначе пропуск | `publication_service.py:103`; `test_publish_skipped_if_not_scheduled` | = | |
| P2 | Подпись: текст + `"\n\nАвтор: @username"` (или `first_name`, или `Аноним`) при выбранном авторстве + `"\n\n" + footer_text` при `include_footer` + `"\n\n" + " ".join(hashtags)` при `include_hashtags` | `publication_service.py:158` | = | |
| P3 | `copyMessage` в канал с этой подписью; при ошибке — `sendMessage` одной подписью (медиа теряется); без подписи и без медиа — ошибка | `publication_service.py:198`; `test_publish_to_channel_copies_and_marks_published` | fix F1: текстовые предложения — `sendMessage` (у текста нет подписи, авторство терялось); запасной вариант «только текст вместо медиа» убран — ошибка идёт в повтор | |
| P4 | Успех: `message_id_in_channel`, статус `published` | `publication_service.py:226` | = | |
| P5 | Ошибка: `publication_retry_count + 1`; если меньше `max_retry_attempts` (2) — повтор через `retry_delay_seconds` (30); иначе `publication_failed`, текст ошибки в БД, сообщение `admin.publication_error` (попытка N из M) в админ-чат | `error_handler.py:38`; `test_retry_exhausted_marks_failed_and_notifies_admin` | = (время повтора хранится в БД и переживает рестарт) | |
| P6 | Сбой между отправкой в канал и отметкой в БД → после рестарта повторная публикация (дубль) | `recovery_service.py:76` | fix: статус `publishing` ставится до отправки; после рестарта такие записи не публикуются повторно, а помечаются `publication_failed` с сообщением админам «проверьте канал» | |

## Запуск и восстановление

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| S1 | `approved` без расписания → планируется заново | `recovery_service.py:57`; `test_recover_approved_gets_scheduled` | = | |
| S2 | `scheduled` с прошедшим временем → публикуется сразу, с будущим — по оставшемуся времени | `recovery_service.py:76`; `test_recover_overdue_scheduled_publishes_now`, `test_recover_future_scheduled_reschedules_task`, `test_recover_overdue_failure_marks_failed` | = | |
| S3 | `pending` > 10 → `⚠️ N предложений ожидают модерации` в админ-чат | `recovery_service.py:127`; `test_recover_pending_notifies_when_many` | = | |
| S4 | Предложение, чья карточка не дошла до админов, остаётся `pending` и больше не показывается | `user_handlers.py:273` (комментарий обещает показ при восстановлении) | fix F9: при запуске такие карточки отправляются повторно | |
| S5 | При каждом запуске два сообщения в админ-чат: `bot_restarted`, затем `bot_started` | `recovery_service.py:153`, `main.py:111`; `test_recover_pending_tasks_smoke_sends_restart_notice` | вопрос Q3 | |
| S6 | Необработанная ошибка в обработчике: лог + alert `❌ Произошла ошибка. Попробуйте позже.` на кнопке | `main.py:24` | = | |

## Статистика

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| T1 | `/stats` только для админов (иначе `⛔️ Только для администраторов`), в любом чате; месяц — текущий по `rate_limits.timezone` | `statistics_handlers.py:138` | = | |
| T2 | Текст: `title` (`Месяц ГГГГ`, русские названия), пустая строка, 4 строки по предложениям, пустая, 3 по пользователям; при `total > 0` — блок `rates`; при наличии решений — `admin_performance` и строки `admin_line` (`@username` или `@ID:<id>`) | `statistics_handlers.py:29`; `test_current_month_counts_and_rates`, `test_empty_month_has_zero_rates` | = | |
| T3 | Метрики за месяц по времени подачи: всего; принято = `approved`+`accepted_not_published`+`published`+`scheduled`; опубликовано; отклонено; уникальные авторы; новые пользователи (по регистрации); заблокированные (всего, не за месяц); решения по `moderator_id` | `statistics_service.py:53`; `test_admin_stats_resolves_username` | = (`publishing` считается как `scheduled`) | |
| T4 | Проценты `round(x, 1)`; «Публикация» = опубликовано/принято. Питоновский вывод: `50.0`, но `0`, когда принятых нет (целый ноль) | `statistics_service.py:124` | = (включая `0` против `0.0`) | |
| T5 | Навигация `stats:<год>:<месяц>`: «◀️ Предыдущий месяц» всегда; «Следующий месяц ▶️» и «📅 Текущий месяц» — только не для текущего; сообщение редактируется | `statistics_handlers.py:83` | = | |
| T6 | Ошибка — `❌ Ошибка получения статистики` | `statistics_handlers.py:167` | = | |

## Конфигурация и эксплуатация

| # | Поведение Python | Rust |
|---|---|---|
| C1 | `config/config.json` с подстановкой `${VAR}` + `.env`; `messages.json` — тексты | Настройки — переменные окружения с теми же значениями по умолчанию; тексты — тот же `messages.json` (проверка всех ключей при запуске) |
| C2 | Postgres + Redis, Alembic `001_initial_schema` | SQLite, встроенная миграция; импорт данных Postgres подкомандой |
| C3 | Логи в `logs/bot_YYYYMMDD.log` и stdout | stdout → journald; токен в логах маскируется |
| C4 | Docker `HEALTHCHECK` каждые 30 с запускает новый Python-процесс (≈ 0,42 CPU-с на прогон) | Не переносится (см. DESIGN.md, «Причина CPU») |
| C5 | `ERROR_CHAT_ID` обязателен, но нигде не используется | вопрос Q2 |

## Объявлено, но не подключено (не переносится)

- Заметки о пользователях: тексты `note_*`, кнопки `add_note`/`edit_note`, флаг `enable_user_notes`, колонка `users.admin_note` — обработчиков нет. Колонка переносится вместе с данными.
- Отмена запланированной публикации: `DecisionManager.cancel_publication`, `PublicationService.cancel_publication`, кнопка `cancel_publication` — не вызываются.
- Кнопки `retry` и `statistics`, callback `show_statistics` (обработчик есть, кнопки нет), `adm_cancel` (обработчик есть, кнопки нет).
- `config_reloaded`, `database_error`, `redis_error`, `ConfigLoader.reload`, `get_yearly_stats`, `enable_statistics`, `retention_years`, `reset_time`, `administrators` в `config.json` (пустой).
