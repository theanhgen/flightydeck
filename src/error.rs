//! Errors with stable exit codes. Messages must never contain the auth token.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// Bad user input, caught before any network call. Exit 2.
    #[error("{0}")]
    BadInput(String),
    /// Flighty not installed / not signed in / DB missing / schema unusable / token expired / owner unresolved. Exit 3.
    #[error("{0}")]
    NotReady(String),
    /// The Flighty API returned an error or an unexpected body. Exit 4.
    #[error("{0}")]
    Api(String),
    /// Refused by policy (read-only mode, remove not enabled, missing --yes). Exit 5.
    #[error("{0}")]
    Refused(String),
    /// Not found (flight, airport, friend). Exit 6.
    #[error("{0}")]
    NotFound(String),
    /// Anything else. Exit 1.
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Other(_) => 1,
            Error::BadInput(_) => 2,
            Error::NotReady(_) => 3,
            Error::Api(_) => 4,
            Error::Refused(_) => 5,
            Error::NotFound(_) => 6,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Error::Other(_) => "error",
            Error::BadInput(_) => "bad_input",
            Error::NotReady(_) => "not_ready",
            Error::Api(_) => "api",
            Error::Refused(_) => "refused",
            Error::NotFound(_) => "not_found",
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        let msg = e.to_string();
        if msg.contains("database is locked") || msg.contains("busy") {
            Error::Other("Flighty is syncing its database; retry in a moment".into())
        } else if msg.contains("unable to open database file") {
            Error::NotReady(format!(
                "can't open the Flighty database ({msg}). If the Flighty Mac app is installed and \
                 signed in, give your terminal app Full Disk Access in System Settings > Privacy & \
                 Security, then reopen the terminal"
            ))
        } else if msg.contains("no such column") || msg.contains("no such table") {
            Error::NotReady(format!(
                "Flighty's database layout changed ({msg}); update flightydeck"
            ))
        } else {
            Error::Other(format!("database error: {msg}"))
        }
    }
}
