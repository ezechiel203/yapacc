// yapacc — typed errors and the exit-code contract
// Copyright (C) 2026 Alexandre BETRY
// SPDX-License-Identifier: GPL-2.0-only
//
//! One error type for the whole command surface, so scripts can rely on
//! exit codes instead of parsing messages.
//!
//! | code | kind          | meaning                                             |
//! |------|---------------|-----------------------------------------------------|
//! | 0    | —             | success                                             |
//! | 1    | `Failed`      | the operation ran and failed                        |
//! | 2    | `Usage`       | bad arguments (same code clap uses)                 |
//! | 3    | `Refused`     | a gate or policy said no; nothing was changed       |
//! | 4    | `NotFound`    | the package, record or plan does not exist          |
//! | 5    | `Integrity`   | a hash, signature or chain check failed             |
//! | 6    | `Unavailable` | something the command needs is not available: a     |
//! |      |               | tool or component, a file this user may not read    |
//!
//! Older modules still return `Result<_, String>`; those convert into
//! `Kind::Failed` through `From<String>`, so `?` keeps working while they
//! are migrated one by one.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Failed,
    Usage,
    Refused,
    NotFound,
    Integrity,
    Unavailable,
}

impl Kind {
    pub fn exit_code(self) -> i32 {
        match self {
            Kind::Failed => 1,
            Kind::Usage => 2,
            Kind::Refused => 3,
            Kind::NotFound => 4,
            Kind::Integrity => 5,
            Kind::Unavailable => 6,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Error {
    pub kind: Kind,
    pub message: String,
}

impl Error {
    pub fn new(kind: Kind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub fn usage(m: impl Into<String>) -> Self {
        Self::new(Kind::Usage, m)
    }
    pub fn refused(m: impl Into<String>) -> Self {
        Self::new(Kind::Refused, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(Kind::NotFound, m)
    }
    pub fn integrity(m: impl Into<String>) -> Self {
        Self::new(Kind::Integrity, m)
    }
    pub fn unavailable(m: impl Into<String>) -> Self {
        Self::new(Kind::Unavailable, m)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::new(Kind::Failed, message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::new(Kind::Failed, message)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(Kind::Failed, e.to_string())
    }
}

/// Lets `?` carry a typed error through the older `Result<_, String>`
/// call chains (the kind is lost there; the message is preserved).
impl From<Error> for String {
    fn from(e: Error) -> Self {
        e.message
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Print the error (if any) and return the process exit code.
pub fn report(res: Result<()>) -> i32 {
    match res {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            e.kind.exit_code()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_are_stable() {
        // This table is a public contract (docs/ARCHITECTURE.md); changing a
        // number breaks scripts.
        assert_eq!(Kind::Failed.exit_code(), 1);
        assert_eq!(Kind::Usage.exit_code(), 2);
        assert_eq!(Kind::Refused.exit_code(), 3);
        assert_eq!(Kind::NotFound.exit_code(), 4);
        assert_eq!(Kind::Integrity.exit_code(), 5);
        assert_eq!(Kind::Unavailable.exit_code(), 6);
    }

    #[test]
    fn string_errors_convert_to_failed() {
        fn legacy() -> std::result::Result<(), String> {
            Err("boom".into())
        }
        fn typed() -> Result<()> {
            legacy()?;
            Ok(())
        }
        let e = typed().unwrap_err();
        assert_eq!(e.kind, Kind::Failed);
        assert_eq!(e.to_string(), "boom");
    }
}
