//! Validation diagnostic with structured error information.
//!
//! This module provides `ValidationDiagnostic`, a wrapper around `DiagnosticMessage`
//! that preserves all validation-specific structure (instance paths, schema paths,
//! source ranges) for machine-readable JSON output while delegating text rendering
//! to `DiagnosticMessage`.

use crate::error::ValidationError;
use quarto_error_reporting::{DiagnosticMessage, DiagnosticMessageBuilder};
use quarto_source_map::{SourceContext, SourceInfo};
use serde::{Deserialize, Serialize};

/// A validation diagnostic with structured error information.
///
/// This type preserves all validation-specific structure (instance paths,
/// schema paths, source ranges) while delegating rendering to DiagnosticMessage.
///
/// # Example
///
/// ```ignore
/// let vd = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
///
/// // Machine-readable JSON
/// println!("{}", serde_json::to_string_pretty(&vd.to_json())?);
///
/// // Human-readable text with ariadne
/// eprintln!("{}", vd.to_text(&source_ctx));
/// ```
#[derive(Debug, Clone)]
pub struct ValidationDiagnostic {
    /// Structured error kind - machine readable
    pub kind: crate::error::ValidationErrorKind,

    /// The validation error code (Q-1-xxx)
    pub code: String,

    /// Path through the YAML instance where the error occurred
    /// Example: ["format", "html", "toc"]
    pub instance_path: Vec<PathSegment>,

    /// Path through the schema that was being validated
    /// Example: ["properties", "format", "properties", "html", "properties", "toc"]
    pub schema_path: Vec<String>,

    /// Source location with filename and byte offsets/line numbers
    pub source_range: Option<SourceRange>,

    /// Author-supplied hint override from the schema's `errorMessage`
    /// annotation. When present, it replaces the auto-generated hint.
    pub custom_hint: Option<String>,

    /// Internal: DiagnosticMessage for text rendering
    diagnostic: DiagnosticMessage,
}

/// A segment in an instance path (object key or array index)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum PathSegment {
    /// Object property key
    Key(String),
    /// Array index
    Index(usize),
}

/// Source range with filename and both offset and line/column positions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    /// Filename (human-readable, not a file_id)
    pub filename: String,

    /// Start byte offset in the file
    pub start_offset: usize,

    /// End byte offset in the file
    pub end_offset: usize,

    /// Start line number (1-indexed)
    pub start_line: usize,

    /// Start column number (1-indexed)
    pub start_column: usize,

    /// End line number (1-indexed)
    pub end_line: usize,

    /// End column number (1-indexed)
    pub end_column: usize,
}

impl ValidationDiagnostic {
    /// Get human-readable message (lazily generated from kind)
    pub fn message(&self) -> String {
        self.kind.message()
    }

    /// Get hints. An author-supplied `errorMessage` override (if any) replaces
    /// the auto-generated hint; otherwise the hint is derived from the kind.
    pub fn hints(&self) -> Vec<String> {
        Self::effective_hints(&self.kind, self.custom_hint.as_deref())
    }

    /// Compute the effective hints: the authored override if present, else the
    /// auto-generated hints for this error kind.
    fn effective_hints(
        kind: &crate::error::ValidationErrorKind,
        custom_hint: Option<&str>,
    ) -> Vec<String> {
        match custom_hint {
            Some(hint) => vec![hint.to_string()],
            None => Self::suggest_fixes_from_kind(kind),
        }
    }

    /// Create a new ValidationDiagnostic from a ValidationError
    ///
    /// # Arguments
    ///
    /// * `error` - The validation error to convert
    /// * `source_ctx` - Source context for resolving file names and line/column positions
    ///
    /// # Example
    ///
    /// ```ignore
    /// let error = ValidationError::new("Expected number, got string", path);
    /// let vd = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
    /// ```
    pub fn from_validation_error(error: &ValidationError, source_ctx: &SourceContext) -> Self {
        // Build the diagnostic message for text rendering
        let diagnostic = Self::build_diagnostic_message(error, source_ctx);

        // Extract source range with filename
        let source_range = error
            .yaml_node
            .as_ref()
            .and_then(|node| Self::extract_source_range(&node.source_info, source_ctx));

        // Convert instance path segments
        let instance_path = error
            .instance_path
            .segments()
            .iter()
            .map(|seg| match seg {
                crate::error::PathSegment::Key(k) => PathSegment::Key(k.clone()),
                crate::error::PathSegment::Index(i) => PathSegment::Index(*i),
            })
            .collect();

        Self {
            kind: error.kind.clone(),
            code: error.error_code().to_string(),
            instance_path,
            schema_path: error.schema_path.segments().to_vec(),
            source_range,
            custom_hint: error.custom_hint.clone(),
            diagnostic,
        }
    }

    /// Render as JSON for machine consumption
    ///
    /// # Example
    ///
    /// ```ignore
    /// let json = vd.to_json();
    /// println!("{}", serde_json::to_string_pretty(&json)?);
    /// ```
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;

        let mut obj = json!({
            "error_kind": self.kind,  // Structured, machine-readable
            "code": self.code,
            "instance_path": self.instance_path,
            "schema_path": self.schema_path,
        });

        if let Some(range) = &self.source_range {
            obj["source_range"] = json!(range);
        }

        // Include human-readable fields for convenience
        obj["message"] = json!(self.kind.message());

        let hints = Self::effective_hints(&self.kind, self.custom_hint.as_deref());
        if !hints.is_empty() {
            obj["hints"] = json!(hints);
        }

        obj
    }

    /// Render as text for human consumption (uses ariadne/tidyverse)
    ///
    /// # Example
    ///
    /// ```ignore
    /// let text = vd.to_text(&source_ctx);
    /// eprintln!("{}", text);
    /// ```
    pub fn to_text(&self, source_ctx: &SourceContext) -> String {
        self.diagnostic.to_text(Some(source_ctx))
    }

    /// Render as a single compact line, optimized for token-efficient
    /// consumption (e.g. feeding validation errors to an LLM).
    ///
    /// Format: `file:line:col [CODE] path: message (hint: ...)`
    ///
    /// The location prefix is omitted when no source range is available, and
    /// the path renders as `(root)` for top-level errors. Unlike [`to_text`],
    /// this drops the ariadne box drawing, source snippet, and the redundant
    /// schema-constraint line; hints are kept because they materially help a
    /// model propose a correct fix. Multiple hints are joined with `; `.
    ///
    /// [`to_text`]: Self::to_text
    ///
    /// # Example
    ///
    /// ```ignore
    /// for d in diagnostics {
    ///     println!("{}", d.to_compact());
    /// }
    /// ```
    pub fn to_compact(&self) -> String {
        let mut out = String::new();

        if let Some(range) = &self.source_range {
            out.push_str(&format!(
                "{}:{}:{} ",
                range.filename, range.start_line, range.start_column
            ));
        }

        out.push_str(&format!("[{}] ", self.code));

        let path = Self::instance_path_string(&self.instance_path);
        if path.is_empty() {
            out.push_str("(root): ");
        } else {
            out.push_str(&path);
            out.push_str(": ");
        }

        out.push_str(&self.message());

        let hints = self.hints();
        if !hints.is_empty() {
            out.push_str(&format!(" (Hint: {})", hints.join("; ")));
        }

        out
    }

    /// Render an instance path as a compact dotted/indexed string.
    ///
    /// e.g. `[Key("authors"), Index(0), Key("name")]` -> `authors[0].name`.
    fn instance_path_string(segments: &[PathSegment]) -> String {
        let mut out = String::new();
        for seg in segments {
            match seg {
                PathSegment::Key(k) => {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(k);
                }
                PathSegment::Index(i) => {
                    out.push_str(&format!("[{}]", i));
                }
            }
        }
        out
    }

    /// Helper: Build DiagnosticMessage for text rendering
    fn build_diagnostic_message(
        error: &ValidationError,
        _source_ctx: &SourceContext,
    ) -> DiagnosticMessage {
        let mut builder = DiagnosticMessageBuilder::error("YAML Validation Failed")
            .with_code(error.error_code())
            .problem(error.message());

        // Attach full SourceInfo for ariadne rendering
        if let Some(yaml_node) = &error.yaml_node {
            builder = builder.with_location(yaml_node.source_info.clone());
        }

        // Add human-readable details
        if !error.instance_path.is_empty() {
            builder = builder.add_detail(format!("At document path: `{}`", error.instance_path));
        } else {
            builder = builder.add_detail("At document root");
        }

        if !error.schema_path.is_empty() {
            builder = builder.add_info(format!("Schema constraint: {}", error.schema_path));
        }

        // Add hints (authored `errorMessage` override wins over generated ones)
        for hint in Self::effective_hints(&error.kind, error.custom_hint.as_deref()) {
            builder = builder.add_hint(hint);
        }

        builder.build()
    }

    /// Helper: Extract SourceRange from SourceInfo
    fn extract_source_range(
        source_info: &SourceInfo,
        source_ctx: &SourceContext,
    ) -> Option<SourceRange> {
        // Map the start of the range (offset 0 in SourceInfo coordinates)
        // This will handle Substring/Concat/Original correctly
        let start_mapped = source_info.map_offset(0, source_ctx)?;

        // Map the end of the range (length in SourceInfo coordinates)
        // For SourceInfo, the end offset is relative to the same base as start_offset
        let length = source_info.end_offset() - source_info.start_offset();
        let end_mapped = source_info.map_offset(length, source_ctx)?;

        // Get filename
        let file = source_ctx.get_file(start_mapped.file_id)?;

        Some(SourceRange {
            filename: file.path.clone(),
            start_offset: source_info.start_offset(),
            end_offset: source_info.end_offset(),
            start_line: start_mapped.location.row + 1, // 1-indexed
            start_column: start_mapped.location.column + 1, // 1-indexed
            end_line: end_mapped.location.row + 1,
            end_column: end_mapped.location.column + 1,
        })
    }

    // No longer needed - error codes come from ValidationErrorKind::error_code()

    /// Suggest fixes based on error kind
    fn suggest_fixes_from_kind(kind: &crate::error::ValidationErrorKind) -> Vec<String> {
        use crate::error::ValidationErrorKind;
        let mut hints = Vec::new();

        match kind {
            ValidationErrorKind::MissingRequiredProperty { property, .. } => {
                hints.push(format!(
                    "Add the `{}` property to your YAML document?",
                    property
                ));
            }
            ValidationErrorKind::TypeMismatch { expected, .. } => match expected.as_str() {
                "boolean" => {
                    hints.push("Use `true` or `false` (YAML 1.2 standard)?".to_string());
                }
                "number" => {
                    hints.push("Use a numeric value without quotes?".to_string());
                }
                "string" => {
                    hints.push(
                        "Ensure the value is a string (quoted if it contains special characters)?"
                            .to_string(),
                    );
                }
                "array" => {
                    hints.push(
                        "Use YAML array syntax: `[item1, item2]` or list format?".to_string(),
                    );
                }
                "object" => {
                    hints.push("Use YAML mapping syntax with key-value pairs?".to_string());
                }
                _ => {}
            },
            ValidationErrorKind::InvalidEnumValue { .. } => {
                hints.push("Check the schema for allowed values?".to_string());
            }
            ValidationErrorKind::StringPatternMismatch { .. } => {
                hints.push("Check that the string matches the expected format?".to_string());
            }
            ValidationErrorKind::NumberOutOfRange { .. }
            | ValidationErrorKind::NumberNotMultipleOf { .. } => {
                hints.push("Check the allowed value range in the schema?".to_string());
            }
            ValidationErrorKind::UnknownProperty { .. } => {
                hints.push(
                    "Check for typos in property names or remove unrecognized properties?"
                        .to_string(),
                );
            }
            ValidationErrorKind::ArrayItemsNotUnique => {
                hints.push("Remove duplicate items from the array?".to_string());
            }
            _ => {
                // No specific hints for other error kinds
            }
        }

        hints
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::InstancePath;

    #[test]
    fn test_path_segment_serialization() {
        let key = PathSegment::Key("format".to_string());
        let json = serde_json::to_value(&key).unwrap();
        assert_eq!(json["type"], "Key");
        assert_eq!(json["value"], "format");

        let index = PathSegment::Index(42);
        let json = serde_json::to_value(&index).unwrap();
        assert_eq!(json["type"], "Index");
        assert_eq!(json["value"], 42);
    }

    #[test]
    fn test_source_range_serialization() {
        let range = SourceRange {
            filename: "test.yaml".to_string(),
            start_offset: 10,
            end_offset: 20,
            start_line: 1,
            start_column: 5,
            end_line: 1,
            end_column: 15,
        };

        let json = serde_json::to_value(&range).unwrap();
        assert_eq!(json["filename"], "test.yaml");
        assert_eq!(json["start_offset"], 10);
        assert_eq!(json["end_offset"], 20);
        assert_eq!(json["start_line"], 1);
        assert_eq!(json["start_column"], 5);
    }

    #[test]
    fn test_error_code() {
        use crate::error::ValidationErrorKind;

        let error = ValidationError::new(
            ValidationErrorKind::MissingRequiredProperty {
                property: "author".to_string(),
                allowed: None,
                expected_type: None,
            },
            InstancePath::new(),
        );
        assert_eq!(error.error_code(), "Q-1-10");

        let error = ValidationError::new(
            ValidationErrorKind::TypeMismatch {
                expected: "number".to_string(),
                got: "string".to_string(),
            },
            InstancePath::new(),
        );
        assert_eq!(error.error_code(), "Q-1-11");

        let error = ValidationError::new(
            ValidationErrorKind::InvalidEnumValue {
                value: "foo".to_string(),
                allowed: vec!["html".to_string(), "pdf".to_string()],
            },
            InstancePath::new(),
        );
        assert_eq!(error.error_code(), "Q-1-12");

        let error = ValidationError::new(
            ValidationErrorKind::UnknownProperty {
                property: "foo".to_string(),
            },
            InstancePath::new(),
        );
        assert_eq!(error.error_code(), "Q-1-18");
    }

    #[test]
    fn test_instance_path_string() {
        assert_eq!(ValidationDiagnostic::instance_path_string(&[]), "");
        assert_eq!(
            ValidationDiagnostic::instance_path_string(&[PathSegment::Key("format".to_string())]),
            "format"
        );
        assert_eq!(
            ValidationDiagnostic::instance_path_string(&[
                PathSegment::Key("authors".to_string()),
                PathSegment::Index(0),
                PathSegment::Key("name".to_string()),
            ]),
            "authors[0].name"
        );
    }

    /// Strip ANSI SGR color codes and OSC-8 hyperlink sequences so the human
    /// (ariadne) rendering can be snapshotted in a clean, machine-independent
    /// form. The OSC-8 hyperlink embeds an absolute `file://` path, which is
    /// not portable across machines; stripping it leaves the visible
    /// `filename:line:col` text untouched.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\u{1b}' {
                out.push(c);
                continue;
            }
            match chars.peek() {
                // CSI (e.g. color): ESC [ ... <final byte in @..~>
                Some('[') => {
                    chars.next();
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if ('@'..='~').contains(&nc) {
                            break;
                        }
                    }
                }
                // OSC (e.g. hyperlink): ESC ] ... <BEL or ST (ESC \)>
                Some(']') => {
                    chars.next();
                    while let Some(nc) = chars.next() {
                        if nc == '\u{07}' {
                            break;
                        }
                        if nc == '\u{1b}' {
                            if let Some('\\') = chars.peek() {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn test_source_context(filename: &str, content: &str) -> SourceContext {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut ctx = SourceContext::new();
        let mut hasher = DefaultHasher::new();
        filename.hash(&mut hasher);
        let file_id = quarto_source_map::FileId(hasher.finish() as usize);
        ctx.add_file_with_id(file_id, filename.to_string(), Some(content.to_string()));
        ctx
    }

    /// Snapshot the three rendering variants (compact, JSON, human) for a
    /// single validation error, so changes to any format's wording/structure
    /// are caught in one place. The error is produced end-to-end through
    /// `validate()` so the source range is real.
    #[test]
    fn test_all_three_formats_snapshot() {
        use crate::{Schema, SchemaRegistry, validate};

        let schema_yaml = quarto_yaml::parse(
            r#"
object:
  properties:
    age:
      number:
        minimum: 0
        maximum: 100
"#,
        )
        .unwrap();
        let schema = Schema::from_yaml(&schema_yaml).unwrap();

        let doc_content = r#"age: "not a number""#;
        let doc = quarto_yaml::parse_file(doc_content, "test.yaml").unwrap();
        let source_ctx = test_source_context("test.yaml", doc_content);

        let registry = SchemaRegistry::new();
        let error = validate(&doc, &schema, &registry, &source_ctx)
            .expect_err("validation should fail for type mismatch");
        let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);

        let combined = format!(
            "=== compact ===\n{}\n\n=== json ===\n{}\n\n=== human (ANSI stripped) ===\n{}",
            diagnostic.to_compact(),
            serde_json::to_string_pretty(&diagnostic.to_json()).unwrap(),
            strip_ansi(&diagnostic.to_text(&source_ctx)),
        );

        insta::assert_snapshot!(combined);
    }

    #[test]
    fn test_to_compact() {
        use crate::error::ValidationErrorKind;

        let source_ctx = SourceContext::new();

        // Error at a nested path: location omitted (no yaml_node), code + path + message + hint.
        let mut path = InstancePath::new();
        path.push_key("format");
        path.push_key("html");
        let error = ValidationError::new(
            ValidationErrorKind::TypeMismatch {
                expected: "boolean".to_string(),
                got: "string".to_string(),
            },
            path,
        );
        let vd = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
        let compact = vd.to_compact();
        assert_eq!(
            compact,
            "[Q-1-11] format.html: Expected boolean, got string (Hint: Use `true` or `false` (YAML 1.2 standard)?)"
        );

        // Root-level error renders `(root)` rather than an empty path.
        let error = ValidationError::new(
            ValidationErrorKind::MissingRequiredProperty {
                property: "version".to_string(),
                allowed: None,
                expected_type: None,
            },
            InstancePath::new(),
        );
        let vd = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
        let compact = vd.to_compact();
        assert!(compact.starts_with("[Q-1-10] (root): Missing required property 'version'"));
        assert!(
            !compact.contains('\n'),
            "compact output must be a single line"
        );
    }

    #[test]
    fn test_suggest_fixes() {
        use crate::error::ValidationErrorKind;

        let kind = ValidationErrorKind::MissingRequiredProperty {
            property: "author".to_string(),
            allowed: None,
            expected_type: None,
        };
        let hints = ValidationDiagnostic::suggest_fixes_from_kind(&kind);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("author"));

        let kind = ValidationErrorKind::TypeMismatch {
            expected: "boolean".to_string(),
            got: "string".to_string(),
        };
        let hints = ValidationDiagnostic::suggest_fixes_from_kind(&kind);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("true"));

        let kind = ValidationErrorKind::TypeMismatch {
            expected: "number".to_string(),
            got: "string".to_string(),
        };
        let hints = ValidationDiagnostic::suggest_fixes_from_kind(&kind);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("numeric"));
    }
}
