#!/usr/bin/env bash
# Installs or updates the bot as a systemd service (no Docker). Run as root.
# Usage: install.sh <dir>   where <dir> holds the Linux binary `bot`,
# messages.json, .env.example and deploy/tg-bot-predlozhka.service.
# Does not start the service and never overwrites an existing bot.env.
set -euo pipefail
NAME=tg-bot-predlozhka
SRC=${1:-.}

id -u "$NAME" >/dev/null 2>&1 ||
    useradd --system --home-dir "/var/lib/$NAME" --no-create-home \
        --shell /usr/sbin/nologin "$NAME"

install -d -m 0755 "/opt/$NAME"
# Replace the binary atomically; a running process keeps its old inode.
install -m 0755 "$SRC/bot" "/opt/$NAME/bot.new"
mv -f "/opt/$NAME/bot.new" "/opt/$NAME/bot"
install -m 0644 "$SRC/messages.json" "/opt/$NAME/messages.json"

install -d -m 0700 -o "$NAME" -g "$NAME" "/var/lib/$NAME"
install -d -m 0700 "/etc/$NAME"
if [ ! -f "/etc/$NAME/bot.env" ]; then
    install -m 0600 "$SRC/.env.example" "/etc/$NAME/bot.env"
    echo "Created /etc/$NAME/bot.env: fill in BOT_TOKEN, chats and ADMIN_IDS."
fi

install -m 0644 "$SRC/deploy/$NAME.service" "/etc/systemd/system/$NAME.service"
systemctl daemon-reload
"/opt/$NAME/bot" --version
echo "Installed. First start: systemctl enable --now $NAME; update: systemctl restart $NAME"
