//! Error types for YAML parsing with source locations.

use crate::SourceInfo;
use std::fmt;

/// Result type alias for quarto-yaml operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during YAML parsing.
///
/// `Display` renders only the message. A location is a [`SourceInfo`] of
/// byte offsets, and turning it into a file, row and column needs a
/// `SourceContext`. To show it, convert the error to a diagnostic and render
/// it with quarto-error-reporting.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// YAML syntax error
    ParseError {
        message: String,
        location: Option<SourceInfo>,
    },

    /// Unexpected end of input
    UnexpectedEof { location: Option<SourceInfo> },

    /// Invalid YAML structure
    InvalidStructure {
        message: String,
        location: Option<SourceInfo>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ParseError { message, .. } => write!(f, "Parse error: {}", message),
            Error::UnexpectedEof { .. } => write!(f, "Unexpected end of input"),
            Error::InvalidStructure { message, .. } => {
                write!(f, "Invalid YAML structure: {}", message)
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use quarto_source_map::FileId;

    #[test]
    fn test_parse_error_display_no_location() {
        let error = Error::ParseError {
            message: "unexpected token".to_string(),
            location: None,
        };
        assert_eq!(error.to_string(), "Parse error: unexpected token");
    }

    #[test]
    fn test_parse_error_display_with_location() {
        let location = SourceInfo::original(FileId(0), 10, 20);
        let error = Error::ParseError {
            message: "invalid syntax".to_string(),
            location: Some(location),
        };
        // Display never renders the location; see the `Error` docs.
        assert_eq!(error.to_string(), "Parse error: invalid syntax");
    }

    #[test]
    fn test_unexpected_eof_display_no_location() {
        let error = Error::UnexpectedEof { location: None };
        assert_eq!(error.to_string(), "Unexpected end of input");
    }

    #[test]
    fn test_unexpected_eof_display_with_location() {
        let location = SourceInfo::original(FileId(0), 100, 100);
        let error = Error::UnexpectedEof {
            location: Some(location),
        };
        assert_eq!(error.to_string(), "Unexpected end of input");
    }

    #[test]
    fn test_invalid_structure_display_no_location() {
        let error = Error::InvalidStructure {
            message: "expected mapping".to_string(),
            location: None,
        };
        assert_eq!(
            error.to_string(),
            "Invalid YAML structure: expected mapping"
        );
    }

    #[test]
    fn test_invalid_structure_display_with_location() {
        let location = SourceInfo::original(FileId(0), 50, 60);
        let error = Error::InvalidStructure {
            message: "duplicate key".to_string(),
            location: Some(location),
        };
        assert_eq!(error.to_string(), "Invalid YAML structure: duplicate key");
    }

    #[test]
    fn test_error_is_std_error() {
        let error = Error::ParseError {
            message: "test".to_string(),
            location: None,
        };
        // Verify that Error implements std::error::Error
        fn assert_error<T: std::error::Error>(_: &T) {}
        assert_error(&error);
    }
}
