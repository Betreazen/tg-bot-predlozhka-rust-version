use serde::{Deserialize, Serialize};

pub mod status {
    pub const PENDING: &str = "pending";
    pub const APPROVED: &str = "approved";
    pub const REJECTED: &str = "rejected";
    pub const PUBLISHED: &str = "published";
    pub const ACCEPTED_NOT_PUBLISHED: &str = "accepted_not_published";
    pub const PUBLICATION_FAILED: &str = "publication_failed";
    pub const SCHEDULED: &str = "scheduled";
    /// Claimed for sending. A row still here after a restart may already be in
    /// the channel, so it is never sent again automatically.
    pub const PUBLISHING: &str = "publishing";
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub user_id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub is_blocked: bool,
    pub admin_note: Option<String>,
    pub total_submissions_count: i64,
    pub registration_ts: i64,
    pub last_interaction_ts: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Submission {
    pub submission_id: String,
    pub user_id: i64,
    pub submission_ts: i64,
    pub status: String,
    pub moderator_id: Option<i64>,
    pub decision_ts: Option<i64>,
    pub show_authorship: bool,
    pub message_id_in_admin_chat: Option<i64>,
    pub message_id_in_channel: Option<i64>,
    pub user_message_id: Option<i64>,
    pub user_chat_id: Option<i64>,
    pub has_media: bool,
    pub media_type: Option<String>,
    pub media_file_id: Option<String>,
    pub text_content: Option<String>,
    pub album: Option<String>,
    pub scheduled_ts: Option<i64>,
    pub publication_error_message: Option<String>,
    pub publication_retry_count: i64,
}

impl Submission {
    pub fn album_parts(&self) -> Vec<AlbumPart> {
        self.album
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    WaitingForContent,
    WaitingForConfirmation,
    WaitingForAuthorship,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Photo,
    Video,
    Document,
    Audio,
    /// GIFs; the Python bot accepted them as documents.
    Animation,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Photo => "photo",
            Self::Video => "video",
            Self::Document => "document",
            Self::Audio => "audio",
            Self::Animation => "animation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumPart {
    pub message_id: i32,
    pub kind: MediaKind,
    pub file_id: String,
    /// HTML.
    pub caption: Option<String>,
}

/// What the user sent, kept between the content step and the authorship choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub chat_id: i64,
    pub message_id: i32,
    /// HTML rendered from the message entities.
    pub text: Option<String>,
    pub media: Option<(MediaKind, String)>,
    /// Empty for a single message.
    pub album: Vec<AlbumPart>,
}
