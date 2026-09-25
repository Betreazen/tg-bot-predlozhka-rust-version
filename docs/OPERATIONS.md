# Эксплуатация и переход с Python-версии

## Устройство данных

Всё состояние бота хранится в `DATA_DIR/bot.db` (SQLite, режим WAL):

| Таблица | Содержимое |
|---|---|
| `users` | пользователи, блокировка, счётчик предложений, заметка (из Python-версии) |
| `submissions` | предложения, статусы, id карточки и поста, время публикации, повторы |
| `admin_action_logs` | журнал решений модераторов |
| `dialogues` | незавершённые диалоги подачи (в Python они жили в Redis) |

Время хранится в секундах Unix (UTC). Текст предложений хранится как HTML, с разметкой
пользователя. Схему создаёт и обновляет сам бот при запуске или `--check`. Номер схемы
записан в `PRAGMA user_version`; с базой более новой версии бот не запустится.

Статусы предложений: `pending`, `scheduled` (ждёт публикации или повтора), `publishing`
(отправляется прямо сейчас), `published`, `accepted_not_published`, `rejected`,
`publication_failed`. `approved` встречается только в данных, импортированных из Python:
при запуске такие предложения планируются к публикации.

## Переход с Python-версии

Нужны доступ к серверу Python-версии и собранный бинарник Rust-версии (см. [DEPLOY.md](DEPLOY.md)).
Ниже `<pg>` — контейнер PostgreSQL, а `DB_USER`/`DB_NAME` берутся из `.env` Python-версии.

1. **Установка.** `bash deploy/install.sh .` и заполненный `/etc/tg-bot-predlozhka/bot.env`
   (токен, `CHANNEL_ID`, `ADMIN_CHAT_ID`, `ERROR_CHAT_ID`, `ADMIN_IDS` — из `.env`
   Python-версии). Сервис пока не запускать.
2. **Резервная копия, пока всё работает.** `pg_dump -Fc` базы, `.env`, `logs/`.
3. **Остановить Python-бота** (только контейнер бота, PostgreSQL оставить):
   `docker compose stop bot`. С этого момента новых данных в PostgreSQL не появится.
4. **Выгрузка** трёх таблиц в JSON:

   ```bash
   mkdir -p export
   for t in users submissions admin_action_logs; do
     docker exec <pg> psql -U "$DB_USER" -d "$DB_NAME" -Atc \
       "select coalesce(json_agg(t), '[]') from $t t" > "export/$t.json"
   done
   ```

5. **Импорт** в пустой каталог данных от сервисного пользователя:

   ```bash
   set -a; . /etc/tg-bot-predlozhka/bot.env; set +a
   chown -R tg-bot-predlozhka: export
   DATA_DIR=/var/lib/tg-bot-predlozhka MESSAGES_PATH=/opt/tg-bot-predlozhka/messages.json \
     runuser -u tg-bot-predlozhka -p -- /opt/tg-bot-predlozhka/bot --import export
   ```

   Импорт идёт одной транзакцией. Если есть неизвестная колонка, неизвестный статус или
   битое значение, импорт целиком отменяется. В непустую базу импорт не пишет. Бот
   печатает сводку.

6. **Сверка.** Та же сводка, посчитанная в PostgreSQL, должна совпасть строка в строку:

   ```sql
   select format('users=%s blocked=%s notes=%s total_submissions_count=%s submissions=%s admin_action_logs=%s statuses=%s',
     (select count(*) from users), (select count(*) from users where is_blocked),
     (select count(admin_note) from users), (select coalesce(sum(total_submissions_count), 0) from users),
     (select count(*) from submissions), (select count(*) from admin_action_logs),
     (select coalesce(string_agg(status || ':' || n, ',' order by status collate "C"), '')
        from (select status::text as status, count(*) as n from submissions group by 1) s));
   ```

7. **Проверка и запуск.** Выполнить `--check` (как в [DEPLOY.md](DEPLOY.md)), затем
   `systemctl enable --now tg-bot-predlozhka`. Дальше проверить журнал, сообщение
   «🤖 Бот запущен» в админ-чате и `/start` у бота.

Что **не** переносится. Незавершённые диалоги из Redis: пользователь просто нажмёт
«Предложить контент» ещё раз. Счётчики дневного лимита из Redis тоже не переносятся,
Rust-версия считает лимит по таблице предложений. Предложения, отправленные в тот же день
до переключения, учитываются.

Предложения в статусе `pending`, чья карточка когда-то не дошла до админ-чата, при первом
запуске будут показаны модераторам заново.

## Откат на Python-версию

1. `systemctl disable --now tg-bot-predlozhka`. Два процесса с одним токеном работать не
   должны, поэтому сначала останавливается Rust.
2. `docker compose start bot` в каталоге Python-версии. PostgreSQL остался в состоянии на
   момент шага 3.
3. Данные, появившиеся в Rust-версии после переключения, в PostgreSQL не попадут. Их
   можно посмотреть в `bot.db` (`submissions`, `users`) и при необходимости перенести
   вручную. Старую резервную копию поверх более новых данных не восстанавливайте.

## Диагностика

- `journalctl -u tg-bot-predlozhka` — журнал. Токен в нём не появляется: сетевые ошибки
  приходят от teloxide без URL, а все остальные сообщения проходят через маскировку.
- `Telegram polling failed; retrying with backoff` — сеть или Telegram недоступны. Бот сам
  повторит попытку.
- `publication failed` с `Retry` — будет повтор. С `Failed` — в чат ошибок ушло сообщение
  «Ошибка публикации».
- Сообщение «Публикация прервана» в чате ошибок означает, что процесс остановился посреди
  отправки в канал. Автоматического повтора нет, чтобы не опубликовать дважды. Проверьте
  канал; если поста там нет, опубликуйте вручную.
- `another instance is using …` — второй процесс на том же каталоге данных, например
  `--check` при запущенном сервисе.
