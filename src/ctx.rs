//! Runtime context: paths and policy, read from the environment once per call.

use std::path::PathBuf;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct Ctx {
    /// `MainFlightyDatabase.db`. Env `FLIGHTY_DB` overrides.
    pub db_path: PathBuf,
    /// Legacy `Flighty.sqlite` (token fallback).
    pub legacy_db_path: PathBuf,
    /// `/Applications/Flighty.app/Contents/Info.plist`. Env `FLIGHTY_APP_PLIST` overrides.
    pub app_plist: PathBuf,
    /// `FLIGHTY_READ_ONLY=1`: no writes of any kind, including remove.
    pub read_only: bool,
    /// `FLIGHTY_ALLOW_REMOVE=1`: enables the experimental remove.
    pub allow_remove: bool,
    /// `FLIGHTY_OWNER_USER_ID`.
    pub owner_override: Option<String>,
}

/// Operation classes; see `Ctx::guard`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Read,
    Write,
    Destructive,
}

fn flag(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref().map(str::trim),
        Some("1") | Some("true") | Some("yes")
    )
}

impl Ctx {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let container = home.join("Library/Containers/com.flightyapp.flighty/Data");
        Ctx {
            db_path: std::env::var_os("FLIGHTY_DB")
                .map(PathBuf::from)
                .unwrap_or_else(|| container.join("Documents/MainFlightyDatabase.db")),
            legacy_db_path: container.join("Documents/Flighty.sqlite"),
            app_plist: std::env::var_os("FLIGHTY_APP_PLIST")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/Applications/Flighty.app/Contents/Info.plist")),
            read_only: flag("FLIGHTY_READ_ONLY"),
            allow_remove: flag("FLIGHTY_ALLOW_REMOVE"),
            owner_override: std::env::var("FLIGHTY_OWNER_USER_ID")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        }
    }

    /// Test/helper constructor pointing at a specific DB, all policy off.
    pub fn with_db(db_path: impl Into<PathBuf>) -> Self {
        let db_path = db_path.into();
        Ctx {
            legacy_db_path: db_path.with_extension("legacy-missing"),
            db_path,
            app_plist: PathBuf::from("/nonexistent/Info.plist"),
            read_only: false,
            allow_remove: false,
            owner_override: None,
        }
    }

    /// Whether a class of operation is available at all (used for MCP tool registration).
    pub fn allows(&self, class: Class) -> bool {
        match class {
            Class::Read => true,
            Class::Write => !self.read_only,
            Class::Destructive => !self.read_only && self.allow_remove,
        }
    }

    /// Policy check every mutating op calls first. Read-only overrides everything.
    pub fn guard(&self, class: Class) -> Result<()> {
        match class {
            Class::Read => Ok(()),
            _ if self.read_only => Err(Error::Refused(
                "read-only mode (FLIGHTY_READ_ONLY=1): writes are disabled".into(),
            )),
            Class::Write => Ok(()),
            Class::Destructive if !self.allow_remove => Err(Error::Refused(
                "remove is disabled; set FLIGHTY_ALLOW_REMOVE=1 to enable it (experimental)".into(),
            )),
            Class::Destructive => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_overrides_remove() {
        let mut c = Ctx::with_db("/x");
        c.read_only = true;
        c.allow_remove = true;
        assert!(c.guard(Class::Destructive).is_err());
        assert!(c.guard(Class::Write).is_err());
        assert!(c.guard(Class::Read).is_ok());
        assert!(!c.allows(Class::Destructive));
    }

    #[test]
    fn remove_needs_opt_in() {
        let c = Ctx::with_db("/x");
        assert!(c.guard(Class::Write).is_ok());
        assert!(c.guard(Class::Destructive).is_err());
    }
}
