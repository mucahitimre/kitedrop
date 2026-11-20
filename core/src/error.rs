use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Protocol-level failures. [`Error::code`] gives the stable string used on the wire and in
/// test vectors, [`Error::http_status`] the HTTP status a listener answers with (see "Error codes"
/// in `docs/protocol.md`).
#[derive(Debug, Error)]
pub enum Error {
    #[error("not a kitedrop envelope")]
    BadMagic,
    #[error("unsupported protocol version or suite")]
    UnsupportedVersion,
    #[error("sender is not a group member")]
    UnknownSender,
    #[error("envelope is addressed to another device")]
    WrongRecipient,
    #[error("decryption or authentication failed")]
    DecryptFailed,
    #[error("stream ended before the final chunk")]
    Truncated,
    #[error("data after the final chunk")]
    TrailingData,
    #[error("record larger than allowed")]
    RecordTooLarge,
    #[error("content size does not match the header")]
    SizeMismatch,
    #[error("malformed message: {0}")]
    Malformed(&'static str),
    #[error("bad signature")]
    BadSignature,
    #[error("device is not a group member")]
    NotMember,
    #[error("challenge unknown, expired, bound to another device or seq not increasing")]
    ChallengeInvalid,
    #[error("request timestamp outside the allowed window")]
    StaleRequest,
    #[error("pairing code unknown, expired or used")]
    CodeInvalid,
    #[error("content created outside the replay window")]
    Expired,
    #[error("group version does not chain onto the local one")]
    GroupConflict,
    #[error("not found")]
    NotFound,
    #[error("not enough disk space")]
    StorageFull,
    #[error("too many requests")]
    RateLimited,
    /// Local only: pairing would move a device that has other members into another group.
    #[error("both devices are already in groups with other devices")]
    AlreadyInGroup,
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("storage: {0}")]
    Db(String),
    #[error("network: {0}")]
    Network(String),
    /// Error answer from a peer: its wire code and message.
    #[error("peer answered {code}: {message}")]
    Remote { code: String, message: String },
    #[error("{0}")]
    Internal(String),
}

impl Error {
    pub fn code(&self) -> &str {
        match self {
            Error::BadMagic => "bad_magic",
            Error::UnsupportedVersion => "unsupported_version",
            Error::UnknownSender => "unknown_sender",
            Error::WrongRecipient => "wrong_recipient",
            Error::DecryptFailed => "decrypt_failed",
            Error::Truncated => "truncated",
            Error::TrailingData => "trailing_data",
            Error::RecordTooLarge => "record_too_large",
            Error::SizeMismatch => "size_mismatch",
            Error::Malformed(_) => "malformed",
            Error::BadSignature => "bad_signature",
            Error::NotMember => "not_member",
            Error::ChallengeInvalid => "challenge_invalid",
            Error::StaleRequest => "stale_request",
            Error::CodeInvalid => "code_invalid",
            Error::Expired => "expired",
            Error::GroupConflict => "group_conflict",
            Error::NotFound => "not_found",
            Error::StorageFull => "storage_full",
            Error::RateLimited => "rate_limited",
            Error::AlreadyInGroup => "already_in_group",
            Error::Io(e) if is_disk_full(e) => "storage_full",
            Error::Io(_) => "io",
            Error::Db(_) => "internal",
            Error::Network(_) => "network",
            Error::Remote { code, .. } => code,
            Error::Internal(_) => "internal",
        }
    }

    pub fn http_status(&self) -> u16 {
        match self.code() {
            "not_member" | "bad_signature" | "challenge_invalid" | "stale_request"
            | "code_invalid" | "unknown_sender" => 403,
            "group_conflict" => 409,
            "not_found" => 404,
            "storage_full" => 507,
            "rate_limited" => 429,
            "internal" | "io" | "network" => 500,
            _ => 400,
        }
    }

    /// Whether retrying the same request later can succeed (network trouble, peer busy or
    /// short of space). Protocol rejections are final.
    pub fn is_transient(&self) -> bool {
        matches!(
            self.code(),
            "network" | "rate_limited" | "storage_full" | "internal" | "io"
        )
    }

    /// Rebuilds an error from a peer's `{ "error": { "code", "message" } }` answer.
    pub fn from_remote(code: &str, message: &str) -> Self {
        Error::Remote {
            code: code.to_owned(),
            message: message.to_owned(),
        }
    }
}

fn is_disk_full(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::StorageFull
}

impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Error::Malformed("json")
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Db(e.to_string())
    }
}
