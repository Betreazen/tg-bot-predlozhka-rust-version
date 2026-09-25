# Соответствие Python-версии

Карта поведения Python-бота [tg-bot-predlozhka](https://github.com/Betreazen/tg-bot-predlozhka)
(коммит `12eccfe`) и того, как оно переносится в Rust. Ссылки вида `user_handlers.py:57`
ведут на файлы в `bot/`; тесты — в `tests/` того же репозитория.

Пометки в колонке «Rust»:

- **=** — переносится без изменений (тексты, кнопки, порядок действий);
- **fix** — в Python это ошибка, в Rust исправляется (список согласуется в спецификации);
- **new** — в Python нет, в Rust добавляется;
- **—** — в Python объявлено, но не подключено к интерфейсу; не переносится.

Колонка «Rust-тест» — файл в `tests/` и имя теста.

## Пользователь

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| U1 | `/start`: сброс состояния, создание/обновление пользователя (поля профиля не затираются `None`), приветствие `welcome.title + "\n\n" + welcome.text`, кнопка `welcome.button` → `suggest_content` | `user_handlers.py:36`, `user_service.py:22`; `test_user_flow::test_start_creates_user_and_greets`, `test_user_service::test_create_then_update_does_not_clobber_with_none` | = | user_flow::start_creates_user_and_greets; db::profile_update_does_not_clobber_known_fields_with_none |
| U2 | `suggest_content`: заблокированному — alert `notifications.user_blocked`; при исчерпанном лимите — alert `user.limit_exceeded` c `{limit}`; иначе состояние `waiting_for_content`, сообщение `submission_prompt` с кнопкой `submission_cancel_button` → `cancel_submission` | `user_handlers.py:72`; `test_blocked_user_cannot_start_submission`, `test_rate_limited_user_cannot_start` | = (alert без HTML-тегов — fix F4) | user_flow::full_text_submission_reaches_moderators_with_escaped_html, blocked_user_gets_plain_alert, daily_limit_blocks_the_third_submission |
| U3 | `cancel_submission` в любом шаге подачи: сброс состояния, сообщение `submission_cancelled` | `user_handlers.py:117` | = | user_flow::cancel_clears_state_at_any_step |
| U4 | Приём контента: текст, фото, видео, документ (в т.ч. GIF — он приходит и как документ), аудио. Прочее (голосовое, кружок, стикер, опрос…) — `unsupported_media` | `user_handlers.py:148` | = | user_flow::unsupported_media_and_oversized_files_are_rejected |
| U5 | Размер файла > `max_file_size_mb` (200) — `file_too_large` c `{max_size}`; размер фото — по последнему (крупнейшему) варианту; неизвестный размер считается 0 | `user_handlers.py:164`; `test_media_submission::test_receive_oversized_photo_rejected` | = | user_flow::unsupported_media_and_oversized_files_are_rejected, unknown_file_size_is_accepted |
| U6 | Черновик: `message_id`, `chat_id`, `text = text or caption` (без разметки), `has_media` | `user_handlers.py:182`; `test_receive_photo_marks_has_media` | fix F2: текст хранится как HTML с сохранением разметки | user_flow::formatting_of_user_text_is_kept, full_text_submission_reaches_moderators_with_escaped_html |
| U7 | `require_confirmation=true`: сообщение `submission_received` + кнопки `confirm_button` → `confirm_content`, `cancel_button` → `cancel_submission`; иначе сразу вопрос об авторстве | `user_handlers.py:190` | = | user_flow::full_text_submission_reaches_moderators_with_escaped_html, confirmation_can_be_skipped |
| U8 | Вопрос `authorship_question`, кнопки `authorship_yes` / `authorship_no` (по одной в ряд) | `user_handlers.py:132` | = | user_flow::full_text_submission_reaches_moderators_with_escaped_html |
| U9 | Выбор авторства: повторная проверка блокировки (alert + сброс), атомарная проверка лимита (alert `limit_exceeded` + сброс), создание предложения `pending`, `total_submissions_count + 1`, отправка карточки админам; ответ `submission_accepted` или `error_occurred`, если карточка не дошла | `user_handlers.py:221`; `test_full_submission_reaches_moderators` | = | user_flow::full_text_submission_reaches_moderators_with_escaped_html, undelivered_card_tells_the_user_and_keeps_the_submission |
| U10 | Сообщения в шагах «подтверждение» и «авторство» игнорируются; кнопки из старых шагов без состояния не обрабатываются (крутится «часики») | aiogram без подходящего обработчика | = (fix F7: старые кнопки гасятся пустым ответом) | user_flow::messages_in_later_steps_are_ignored, stale_buttons_are_answered_silently |
| U11 | Альбом: каждая часть обрабатывается отдельно, пользователь получает несколько запросов подтверждения, в предложение попадает одно сообщение | нет обработки `media_group_id` | new N1: альбом собирается целиком | user_flow::album_becomes_one_submission, album_with_oversized_part_is_rejected_once |
| U12 | Команда, отправленная в шаге «жду контент» (кроме `/start`), становится содержимым предложения; текст вида `/r/foo` — тоже | порядок роутеров `main.py:98` | fix F6: `/start` и `/stats` обрабатываются раньше состояния; прочий текст с `/` — обычное содержимое, как в Python | user_flow::commands_are_not_taken_as_content, slash_text_that_is_not_a_known_command_is_content |
| U13 | Поток подачи работает и в группах (состояние по паре чат+пользователь) | aiogram FSM | fix F5: подача только в личке | user_flow::submission_flow_is_private_only |

## Лимит подач

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| R1 | `submissions_per_day` (2) в сутки по часовому поясу `rate_limits.timezone` (Europe/Moscow), сброс в локальную полночь | `rate_limit.py`; `test_try_acquire_enforces_daily_limit` | = (счёт по таблице предложений в SQLite, без Redis) | user_flow::daily_limit_blocks_the_third_submission; db::daily_limit_is_enforced_atomically_and_resets_next_day; time::daily_limit_resets_at_moscow_midnight |
| R2 | Предварительная проверка при нажатии кнопки, окончательная — атомарно при создании; отказ не расходует слот | `rate_limit.py:96`; `test_rejected_acquire_rolls_back_counter`, `test_check_limit_reports_state` | = (проверка и вставка в одной транзакции) | user_flow::limit_is_rechecked_when_submitting |
| R3 | При недоступном Redis — подсчёт по БД | `test_db_fallback_when_redis_down` | не нужно: Redis нет, БД — единственный источник | — |

## Модерация

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| M1 | Карточка в `ADMIN_CHAT_ID`: `admin.submission_header` (`{submission_number}` = `{total_submissions}` = счётчик пользователя, `{user_info}` = `@username` или `ID: <id>`, `{timestamp}` = время подачи UTC `%Y-%m-%d %H:%M`, `{blocked_status}`, `{note_section}` всегда пусто, `{authorship_info}`) + `"\n\n" + текст` + `"\n\n[ID: <8 символов uuid>]"` | `admin_handlers.py:172`; `test_present_text_submission_sends_card` | = | user_flow::full_text_submission_reaches_moderators_with_escaped_html; moderation::blocked_status_shows_on_new_cards |
| M2 | Медиа — `copyMessage` с подписью-карточкой и кнопками; текст — `sendMessage` | `admin_handlers.py:228`; `test_present_media_uses_copy_message` | = для одиночных; new N1 для альбомов; fix F3 при подписи > 1024 символов | user_flow::photo_card_is_copied_with_caption_and_keyboard, long_caption_card_is_sent_separately, album_becomes_one_submission |
| M3 | Кнопки: `approve_publish` (`adm_app_pub:<uuid>`), ряд `approve_only` (`adm_app:`) + `reject` (`adm_rej:`), ряд `block_user`/`unblock_user` (`adm_blk:<user_id>`/`adm_unblk:`) | `admin_handlers.py:47` | = (те же callback_data) | user_flow::full_text_submission_reaches_moderators_with_escaped_html |
| M4 | Любая админ-кнопка от не-админа — alert `⛔️ Только для администраторов`. Админы — `ADMIN_IDS` (разделители `,` и `;`, пробелы допустимы, мусор — ошибка запуска) | `admin_handlers.py:284`, `config.py:122`; `test_non_admin_cannot_moderate`, `test_config.py` | = | moderation::non_admin_cannot_moderate; config::admin_ids_accept_commas_semicolons_and_spaces, invalid_admin_id_is_an_error_like_python |
| M5 | Битый uuid — alert `❌ Некорректный идентификатор`; нет в БД — `❌ Предложка не найдена` | `admin_handlers.py:93` | = | moderation::malformed_or_unknown_submission_ids |
| M6 | «Принять и опубликовать» → клавиатура заменяется на `confirm` (`adm_conf_pub:`) / `cancel` (`adm_cancel_pub:`), toast `confirm_approve_publish` | `admin_handlers.py:276` | = | moderation::approve_publish_asks_for_confirmation_first |
| M7 | `adm_cancel_pub` → исходная клавиатура (с актуальной кнопкой блокировки), toast `❌ Отменено` | `admin_handlers.py:391` | = | moderation::approve_publish_asks_for_confirmation_first |
| M8 | Решение только из `pending`; иначе alert `already_processed`. Параллельный клик в пределах блокировки Redis — alert `processing_by_another` | `decision_manager.py:57`; `test_second_decision_raises_already_decided`, `test_lock_held_raises_lock_not_acquired` | = по сути: атомарный `UPDATE … WHERE status='pending'`; второй клик всегда получает `already_processed` | moderation::confirmed_publication_is_scheduled_and_reported; db::decision_is_taken_once_and_logged |
| M9 | Подтверждённая публикация: статус `approved` → `scheduled` через `delay_minutes` (2), уведомление `approved_and_published` с кнопкой `📝 Предложить ещё контент`, подвал решения `Принято и запланировано к публикации`, toast `✅ Принято и запланировано к публикации` | `admin_handlers.py:318`; `test_approve_publish_publishes_to_channel` | = | moderation::confirmed_publication_is_scheduled_and_reported |
| M10 | «Принять»: `accepted_not_published`, уведомление `approved_only` + `📝 Предложить контент`, подвал `Принято без публикации`, toast `✅ Принято без публикации` | `admin_handlers.py:431` | = | moderation::approve_only_on_a_media_card_edits_the_caption |
| M11 | «Отклонить»: `rejected`, уведомление `rejected` + `🔄 Попробовать снова`, подвал `Отклонено`, toast `❌ Отклонено` | `admin_handlers.py:485`; `test_reject_notifies_author` | = | moderation::reject_notifies_the_author |
| M12 | Подвал `admin.decision_made` (`{decision}`, `{moderator}` = username или id, `{timestamp}` = текущее время сервера (UTC) `%Y-%m-%d %H:%M`): клавиатура удаляется, к тексту/подписи карточки дописывается подвал; разметка карточки теряется | `admin_handlers.py:117` | = (fix F2: разметка сохраняется) | moderation::confirmed_publication_is_scheduled_and_reported, approve_only_on_a_media_card_edits_the_caption |
| M13 | Каждое решение пишется в `admin_action_logs` (`approve_publish`/`approve_only`/`reject`) | `decision_manager.py:108`; `test_make_decision_approves_and_logs` | = | moderation::reject_notifies_the_author; db::decision_is_taken_once_and_logged |
| M14 | Блокировка/разблокировка с карточки: флаг в `users`, уведомление `user_blocked`/`user_unblocked`, toast `🚫 Пользователь заблокирован` / `✅ Пользователь разблокирован`; неизвестный пользователь — alert `❌ Ошибка блокировки`/`❌ Ошибка разблокировки`. Клавиатура карточки не меняется, в журнал не пишется | `admin_handlers.py:539`; `test_block_user_from_card` | = (fix F8: кнопка на карточке меняется на противоположную) | moderation::block_and_unblock_from_the_card, blocking_an_unknown_user_fails |
| M15 | `enable_blocking=false` отключает проверку блокировки у пользователя | `user_handlers.py:16` | = | user_flow::blocking_can_be_disabled |

## Публикация

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| P1 | Через `delay_minutes` проверка, что статус всё ещё `scheduled`, иначе пропуск | `publication_service.py:103`; `test_publish_skipped_if_not_scheduled` | = | publication::cancelled_or_decided_elsewhere_is_not_published |
| P2 | Подпись: текст + `"\n\nАвтор: @username"` (или `first_name`, или `Аноним`) при выбранном авторстве + `"\n\n" + footer_text` при `include_footer` + `"\n\n" + " ".join(hashtags)` при `include_hashtags` | `publication_service.py:158` | = | publication::text_is_published_with_authorship, anonymous_post_gets_footer_and_hashtags, author_without_username_is_named_by_first_name |
| P3 | `copyMessage` в канал с этой подписью; при ошибке — `sendMessage` одной подписью (медиа теряется); без подписи и без медиа — ошибка | `publication_service.py:198`; `test_publish_to_channel_copies_and_marks_published` | fix F1: текстовые предложения — `sendMessage` (у текста нет подписи, авторство терялось); запасной вариант «только текст вместо медиа» убран — ошибка идёт в повтор | publication::text_is_published_with_authorship, media_is_copied_with_new_caption, album_is_published_as_a_media_group |
| P4 | Успех: `message_id_in_channel`, статус `published` | `publication_service.py:226` | = | publication::text_is_published_with_authorship |
| P5 | Ошибка: `publication_retry_count + 1`; если меньше `max_retry_attempts` (2) — повтор через `retry_delay_seconds` (30); иначе `publication_failed`, текст ошибки в БД, сообщение `admin.publication_error` (попытка N из M) в админ-чат | `error_handler.py:38`; `test_retry_exhausted_marks_failed_and_notifies_admin` | = (время повтора хранится в БД и переживает рестарт) | publication::failures_are_retried_then_reported_to_the_error_chat; db::failed_publication_is_retried_then_marked_failed |
| P6 | Сбой между отправкой в канал и отметкой в БД → после рестарта повторная публикация (дубль) | `recovery_service.py:76` | fix: статус `publishing` ставится до отправки; после рестарта такие записи не публикуются повторно, а помечаются `publication_failed` с сообщением админам «проверьте канал» | publication::startup_recovers_without_double_posting, shutdown_waits_for_a_send_in_progress_and_blocks_new_claims; db::approve_publish_schedules_and_claim_happens_once, recovery_never_resends_an_interrupted_publication |

## Запуск и восстановление

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| S1 | `approved` без расписания → планируется заново | `recovery_service.py:57`; `test_recover_approved_gets_scheduled` | = | publication::startup_recovers_without_double_posting |
| S2 | `scheduled` с прошедшим временем → публикуется сразу, с будущим — по оставшемуся времени | `recovery_service.py:76`; `test_recover_overdue_scheduled_publishes_now`, `test_recover_future_scheduled_reschedules_task`, `test_recover_overdue_failure_marks_failed` | = | publication::worker_publishes_when_woken, startup_recovers_without_double_posting |
| S3 | `pending` > 10 → `⚠️ N предложений ожидают модерации` в админ-чат | `recovery_service.py:127`; `test_recover_pending_notifies_when_many` | = | publication::many_pending_submissions_are_announced |
| S4 | Предложение, чья карточка не дошла до админов, остаётся `pending` и больше не показывается | `user_handlers.py:273` (комментарий обещает показ при восстановлении) | fix F9: при запуске такие карточки отправляются повторно | publication::startup_recovers_without_double_posting |
| S5 | При каждом запуске два сообщения в админ-чат: `bot_restarted`, затем `bot_started` | `recovery_service.py:153`, `main.py:111`; `test_recover_pending_tasks_smoke_sends_restart_notice` | одно сообщение `bot_started` (решение Q3) | publication::startup_recovers_without_double_posting |
| S6 | Необработанная ошибка в обработчике: лог + alert `❌ Произошла ошибка. Попробуйте позже.` на кнопке | `main.py:24` | = | user_flow::handler_errors_stop_the_spinner_with_an_alert |

## Статистика

| # | Поведение Python | Код / тест Python | Rust | Rust-тест |
|---|---|---|---|---|
| T1 | `/stats` только для админов (иначе `⛔️ Только для администраторов`), в любом чате; месяц — текущий по `rate_limits.timezone` | `statistics_handlers.py:138` | = | stats::stats_command_is_for_admins_only |
| T2 | Текст: `title` (`Месяц ГГГГ`, русские названия), пустая строка, 4 строки по предложениям, пустая, 3 по пользователям; при `total > 0` — блок `rates`; при наличии решений — `admin_performance` и строки `admin_line` (`@username` или `@ID:<id>`) | `statistics_handlers.py:29`; `test_current_month_counts_and_rates`, `test_empty_month_has_zero_rates` | = | stats::renders_like_python, empty_month_has_no_rates_block |
| T3 | Метрики за месяц по времени подачи: всего; принято = `approved`+`accepted_not_published`+`published`+`scheduled`; опубликовано; отклонено; уникальные авторы; новые пользователи (по регистрации); заблокированные (всего, не за месяц); решения по `moderator_id` | `statistics_service.py:53`; `test_admin_stats_resolves_username` | = (`publishing` считается как `scheduled`) | db::monthly_counts_match_python_definitions; time::month_range_matches_python_moscow_offset |
| T4 | Проценты `round(x, 1)`; «Публикация» = опубликовано/принято. Питоновский вывод: `50.0`, но `0`, когда принятых нет (целый ноль) | `statistics_service.py:124` | = (включая `0` против `0.0`) | stats::percentages_follow_python_round_and_repr |
| T5 | Навигация `stats:<год>:<месяц>`: «◀️ Предыдущий месяц» всегда; «Следующий месяц ▶️» и «📅 Текущий месяц» — только не для текущего; сообщение редактируется | `statistics_handlers.py:83` | = | stats::navigation_hides_next_for_the_current_month, navigation_edits_the_message |
| T6 | Ошибка — `❌ Ошибка получения статистики` | `statistics_handlers.py:167` | = | stats::network_errors_are_logged_without_the_token |

## Конфигурация и эксплуатация

| # | Поведение Python | Rust |
|---|---|---|
| C1 | `config/config.json` с подстановкой `${VAR}` + `.env`; `messages.json` — тексты | Настройки — переменные окружения с теми же значениями по умолчанию; тексты — тот же `messages.json` (проверка всех ключей при запуске) |
| C2 | Postgres + Redis, Alembic `001_initial_schema` | SQLite, встроенная миграция; импорт данных Postgres подкомандой |
| C3 | Логи в `logs/bot_YYYYMMDD.log` и stdout | stdout → journald; токен в логах маскируется |
| C4 | Docker `HEALTHCHECK` каждые 30 с запускает новый Python-процесс (≈ 0,42 CPU-с на прогон) | Не переносится (см. DESIGN.md, «Причина CPU») |
| C5 | `ERROR_CHAT_ID` обязателен, но нигде не используется | ошибки публикации идут в `ERROR_CHAT_ID`, по умолчанию — в админ-чат (решение Q2) |

## Объявлено, но не подключено (не переносится)

- Заметки о пользователях: тексты `note_*`, кнопки `add_note`/`edit_note`, флаг `enable_user_notes`, колонка `users.admin_note` — обработчиков нет. Колонка переносится вместе с данными.
- Отмена запланированной публикации: `DecisionManager.cancel_publication`, `PublicationService.cancel_publication`, кнопка `cancel_publication` — не вызываются.
- Кнопки `retry` и `statistics`, callback `show_statistics` (обработчик есть, кнопки нет), `adm_cancel` (обработчик есть, кнопки нет).
- `config_reloaded`, `database_error`, `redis_error`, `ConfigLoader.reload`, `get_yearly_stats`, `enable_statistics`, `retention_years`, `reset_time`, `administrators` в `config.json` (пустой).

## Прочие отличия, найденные сверкой после реализации

Все — исправления ошибок или безопасные улучшения; внешний вид сообщений не меняется.

- Текст длиннее 4096 символов вместе с заголовком карточки: Python не мог отправить карточку
  (предложение оставалось без модерации), Rust копирует сообщение пользователя и отправляет
  карточку ответом — как F3 для подписей медиа.
- Профиль (username, имя) обновляется при каждой подаче, а не только на `/start`: в карточке и
  в строке «Автор:» — актуальный username. Пользователь без записи в БД (не нажимал `/start`)
  тоже может подать предложение; в Python это заканчивалось ошибкой.
- Двойное нажатие кнопки авторства: в Python aiogram обрабатывал апдейты параллельно, и второе
  нажатие могло создать дубль. В Rust диалог удаляется в той же транзакции, что и вставка.
- Текст ошибки в сообщении о неудачной публикации экранируется (в Python `<` или `&` в тексте
  ошибки ломали само уведомление). Формулировки ошибок — teloxide, а не aiogram.
