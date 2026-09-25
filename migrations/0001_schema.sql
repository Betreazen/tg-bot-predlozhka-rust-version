-- Mirrors the Python bot's Alembic 001_initial_schema. Timestamps are Unix seconds (UTC).
CREATE TABLE IF NOT EXISTS users (
    user_id INTEGER PRIMARY KEY,
    username TEXT,
    first_name TEXT,
    last_name TEXT,
    is_blocked INTEGER NOT NULL DEFAULT 0,
    admin_note TEXT,
    total_submissions_count INTEGER NOT NULL DEFAULT 0 CHECK (total_submissions_count >= 0),
    registration_ts INTEGER NOT NULL,
    last_interaction_ts INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_users_username ON users(username);
CREATE INDEX IF NOT EXISTS ix_users_is_blocked ON users(is_blocked);
CREATE INDEX IF NOT EXISTS ix_users_registration_ts ON users(registration_ts);

CREATE TABLE IF NOT EXISTS submissions (
    submission_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    submission_ts INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected', 'published',
        'accepted_not_published', 'publication_failed', 'scheduled', 'publishing')),
    moderator_id INTEGER,
    decision_ts INTEGER,
    show_authorship INTEGER NOT NULL DEFAULT 0,
    message_id_in_admin_chat INTEGER,
    message_id_in_channel INTEGER,
    user_message_id INTEGER,
    user_chat_id INTEGER,
    has_media INTEGER NOT NULL DEFAULT 0,
    media_type TEXT,
    media_file_id TEXT,
    -- HTML, rendered from the user's message entities.
    text_content TEXT,
    -- JSON array of album parts; NULL for a single message.
    album TEXT,
    scheduled_ts INTEGER,
    publication_error_message TEXT,
    publication_retry_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS ix_submissions_user_id ON submissions(user_id);
CREATE INDEX IF NOT EXISTS ix_submissions_status ON submissions(status);
CREATE INDEX IF NOT EXISTS ix_submissions_submission_ts ON submissions(submission_ts);
CREATE INDEX IF NOT EXISTS ix_submissions_moderator_id ON submissions(moderator_id);
CREATE INDEX IF NOT EXISTS ix_submissions_scheduled_ts ON submissions(scheduled_ts);

CREATE TABLE IF NOT EXISTS admin_action_logs (
    log_id INTEGER PRIMARY KEY AUTOINCREMENT,
    action_type TEXT NOT NULL,
    admin_user_id INTEGER NOT NULL,
    target_user_id INTEGER,
    submission_id TEXT REFERENCES submissions(submission_id) ON DELETE CASCADE,
    action_ts INTEGER NOT NULL,
    additional_context TEXT
);
CREATE INDEX IF NOT EXISTS ix_admin_action_logs_submission_id ON admin_action_logs(submission_id);
CREATE INDEX IF NOT EXISTS ix_admin_action_logs_admin_user_id ON admin_action_logs(admin_user_id);
CREATE INDEX IF NOT EXISTS ix_admin_action_logs_action_ts ON admin_action_logs(action_ts);

-- Replaces aiogram's Redis FSM storage. Submissions happen in private chats only,
-- so the user id identifies the dialogue.
CREATE TABLE IF NOT EXISTS dialogues (
    user_id INTEGER PRIMARY KEY,
    state TEXT NOT NULL,
    draft TEXT
);

PRAGMA user_version = 1;
