# Развёртывание без Docker (systemd)

Один бинарник, файл SQLite и unit systemd. Нет контейнерного runtime, PostgreSQL,
Redis и Python. Автозапуск после перезагрузки и перезапуск после падения
обеспечивает systemd (`Restart=always`).

## Требования

- Linux x86_64 с systemd и glibc ≥ 2.34 (Debian 12+, Ubuntu 22.04+).
- Исходящий HTTPS до `api.telegram.org`. Входящие порты не нужны.
- Локальный диск под `/var/lib/tg-bot-predlozhka`.
- Бот — администратор канала с правом публикации, участник админ-чата и чата ошибок.

## 1. Сборка бинарника

Компиляция требует больше памяти, чем сама работа бота. На маломощном сервере
собирайте на рабочей машине или в CI:

```bash
docker build --target build -t tg-bot-predlozhka:build .
id=$(docker create tg-bot-predlozhka:build)
docker cp "$id":/usr/local/bin/bot ./bot
docker rm "$id"
```

На Linux с Rust то же самое делает
`cargo build --release --locked && cp target/release/tg-bot-predlozhka bot`.

## 2. Установка

Скопируйте на сервер `bot`, `messages.json`, `.env.example` и каталог `deploy/`,
затем от root выполните:

```bash
bash deploy/install.sh .
```

Скрипт создаёт системного пользователя `tg-bot-predlozhka` и раскладывает файлы:

| Путь | Что там |
|---|---|
| `/opt/tg-bot-predlozhka/` | бинарник `bot` и `messages.json` |
| `/var/lib/tg-bot-predlozhka/` | `bot.db` (0700, владелец — сервисный пользователь) |
| `/etc/tg-bot-predlozhka/bot.env` | настройки (0600, root) |
| `/etc/systemd/system/tg-bot-predlozhka.service` | unit |

Существующий `bot.env` скрипт не перезаписывает и сервис не запускает. Заполните в
`bot.env` поля `BOT_TOKEN`, `CHANNEL_ID`, `ADMIN_CHAT_ID`, `ERROR_CHAT_ID` и `ADMIN_IDS`.
`DATA_DIR` и `MESSAGES_PATH` задаёт unit.

## 3. Проверка и запуск

Запускайте `--check` от сервисного пользователя, пока сервис остановлен:

```bash
set -a; . /etc/tg-bot-predlozhka/bot.env; set +a
DATA_DIR=/var/lib/tg-bot-predlozhka MESSAGES_PATH=/opt/tg-bot-predlozhka/messages.json \
  runuser -u tg-bot-predlozhka -p -- /opt/tg-bot-predlozhka/bot --check

systemctl enable --now tg-bot-predlozhka
systemctl status tg-bot-predlozhka
journalctl -u tg-bot-predlozhka -f
```

При запуске бот один раз пишет в админ-чат «🤖 Бот запущен». Если в журнале нет
`bot started` или после старта появляются строки уровня WARN или ERROR, проверьте токен,
ID чатов и права бота.

## Обновление

Соберите новый `bot`, повторите `bash deploy/install.sh .` и выполните
`systemctl restart tg-bot-predlozhka`. Бинарник заменяется атомарно, данные и
`bot.env` не затрагиваются. При остановке бот до 30 секунд дожидается текущих
обработчиков и отправки публикации, если она уже идёт. Откат — установить предыдущий
бинарник тем же способом.

## Ресурсы

```bash
systemctl show tg-bot-predlozhka -p MemoryCurrent -p MemoryPeak -p CPUUsageNSec -p NRestarts
```

Лимиты в unit стартовые: `MemoryMax=128M`, `CPUQuota=50%`, `TasksMax=64`.
Проверить перезапуск после падения можно так: `systemctl kill -s KILL tg-bot-predlozhka`.
Через 5–8 секунд должен появиться новый PID, а `NRestarts` должен вырасти.

## Резервная копия

Согласованная копия делается так: `systemctl stop tg-bot-predlozhka`, затем копируется
**весь** `/var/lib/tg-bot-predlozhka` (вместе с `bot.db-wal`), затем
`systemctl start tg-bot-predlozhka`. Копировать один `bot.db` у работающего процесса нельзя.

## Переход с Python-версии

Кратко: остановить Python-бота, выгрузить три таблицы PostgreSQL в JSON, выполнить
`bot --import` в пустой `DATA_DIR`, сверить счётчики, затем выполнить `--check` и
`systemctl enable --now`. Подробно, с откатом — в [OPERATIONS.md](OPERATIONS.md).

## Удаление

```bash
systemctl disable --now tg-bot-predlozhka
rm /etc/systemd/system/tg-bot-predlozhka.service && systemctl daemon-reload
rm -r /opt/tg-bot-predlozhka /etc/tg-bot-predlozhka   # /var/lib — вручную, после резервной копии
userdel tg-bot-predlozhka
```
