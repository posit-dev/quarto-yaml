//! Integration tests for ValidationDiagnostic
//!
//! Tests JSON structure, text output, and overall integration.

use quarto_source_map::SourceContext;
use quarto_yaml_validation::{Schema, ValidationDiagnostic, validate};
use serde_json::Value;

/// Helper to create a SourceContext with a test file
fn create_test_context(filename: &str, content: &str) -> SourceContext {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut ctx = SourceContext::new();

    // Compute FileId from filename hash (same as quarto-yaml)
    let mut hasher = DefaultHasher::new();
    filename.hash(&mut hasher);
    let file_id = quarto_source_map::FileId(hasher.finish() as usize);

    ctx.add_file_with_id(file_id, filename.to_string(), Some(content.to_string()));
    ctx
}

#[test]
fn test_json_structure_type_mismatch() {
    // Create a schema with nested object expecting age to be a number
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

    // Create invalid document with string instead of number for age
    let doc_content = r#"age: "not a number""#;
    let doc = quarto_yaml::parse_file(doc_content, "test.yaml").unwrap();

    // Create SourceContext
    let source_ctx = create_test_context("test.yaml", doc_content);

    // Validate (should fail)
    let registry = quarto_yaml_validation::SchemaRegistry::new();
    let result = validate(&doc, &schema, &registry, &source_ctx);

    assert!(result.is_err(), "Validation should fail for type mismatch");

    let error = result.unwrap_err();
    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);

    // Test JSON structure
    let json = diagnostic.to_json();

    // Check error_kind is structured (not just a string)
    assert!(
        json.get("error_kind").is_some(),
        "Should have error_kind field"
    );
    assert!(
        json["error_kind"].is_object(),
        "error_kind should be an object"
    );
    assert_eq!(json["error_kind"]["type"], "TypeMismatch");
    assert_eq!(json["error_kind"]["data"]["expected"], "number");
    assert_eq!(json["error_kind"]["data"]["got"], "string");

    // Check error code
    assert_eq!(json["code"], "Q-1-11");

    // Check message is present for convenience
    assert!(json.get("message").is_some());
    assert!(
        json["message"]
            .as_str()
            .unwrap()
            .contains("Expected number")
    );

    // Check instance_path points to "age" property
    assert!(json["instance_path"].is_array());
    let instance_path = json["instance_path"].as_array().unwrap();
    assert_eq!(instance_path.len(), 1);
    assert_eq!(instance_path[0]["type"], "Key");
    assert_eq!(instance_path[0]["value"], "age");

    // Check schema_path
    assert!(json["schema_path"].is_array());

    // Check source_range has filename (not file_id!)
    let source_range = json.get("source_range").expect("Should have source_range");
    assert_eq!(source_range["filename"], "test.yaml");
    assert!(source_range["start_offset"].is_number());
    assert!(source_range["end_offset"].is_number());
    assert!(source_range["start_line"].is_number());
    assert!(source_range["start_column"].is_number());
    assert!(source_range["end_line"].is_number());
    assert!(source_range["end_column"].is_number());

    // Verify line numbers are 1-indexed
    assert!(source_range["start_line"].as_u64().unwrap() >= 1);
    assert!(source_range["start_column"].as_u64().unwrap() >= 1);
}

#[test]
fn test_json_structure_missing_property() {
    // Schema requiring "name" property
    let schema_yaml = quarto_yaml::parse(
        r#"
object:
  properties:
    name:
      string: {}
    age:
      number: {}
  required:
    - name
"#,
    )
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    // Document missing "name"
    let doc_content = r#"age: 25"#;
    let doc = quarto_yaml::parse_file(doc_content, "person.yaml").unwrap();

    let source_ctx = create_test_context("person.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let result = validate(&doc, &schema, &registry, &source_ctx);
    assert!(result.is_err());

    let error = result.unwrap_err();
    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
    let json = diagnostic.to_json();

    // Check structured error_kind
    assert_eq!(json["error_kind"]["type"], "MissingRequiredProperty");
    assert_eq!(json["error_kind"]["data"]["property"], "name");

    // Check error code
    assert_eq!(json["code"], "Q-1-10");

    // Check hints are present
    assert!(json.get("hints").is_some());
    let hints = json["hints"].as_array().unwrap();
    assert!(!hints.is_empty());
    assert!(hints[0].as_str().unwrap().contains("name"));
}

#[test]
fn test_json_structure_nested_path() {
    // Schema with nested structure
    let schema_yaml = quarto_yaml::parse(
        r#"
object:
  properties:
    user:
      object:
        properties:
          name:
            string: {}
          email:
            string:
              pattern: "^[^@]+@[^@]+\\.[^@]+$"
"#,
    )
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    // Document with invalid email
    let doc_content = r#"
user:
  name: "John"
  email: "invalid-email"
"#;
    let doc = quarto_yaml::parse_file(doc_content, "config.yaml").unwrap();

    let source_ctx = create_test_context("config.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let result = validate(&doc, &schema, &registry, &source_ctx);
    assert!(result.is_err());

    let error = result.unwrap_err();
    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
    let json = diagnostic.to_json();

    // Check instance_path shows nested structure
    let instance_path = json["instance_path"].as_array().unwrap();
    assert_eq!(instance_path.len(), 2);
    assert_eq!(instance_path[0]["type"], "Key");
    assert_eq!(instance_path[0]["value"], "user");
    assert_eq!(instance_path[1]["type"], "Key");
    assert_eq!(instance_path[1]["value"], "email");

    // Check source_range points to the email value
    let source_range = &json["source_range"];
    assert_eq!(source_range["filename"], "config.yaml");
    // Line should be around 4 (0-indexed: line 3)
    assert!(source_range["start_line"].as_u64().unwrap() >= 3);
}

#[test]
fn test_text_output_has_ariadne() {
    // Create schema and invalid document
    let schema_yaml = quarto_yaml::parse(
        r#"
number:
  minimum: 1
  maximum: 100
"#,
    )
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    let doc_content = r#"count: 500"#;
    let doc = quarto_yaml::parse_file(doc_content, "data.yaml").unwrap();

    let source_ctx = create_test_context("data.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let result = validate(&doc, &schema, &registry, &source_ctx);
    assert!(result.is_err());

    let error = result.unwrap_err();
    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);

    // Test text output
    let text = diagnostic.to_text(&source_ctx);

    // Should have ariadne box-drawing characters
    assert!(
        text.contains("─") || text.contains("│") || text.contains("╭") || text.contains("╯"),
        "Should have ariadne box-drawing characters"
    );

    // Should have filename
    assert!(text.contains("data.yaml"), "Should contain filename");

    // Should have error code
    assert!(text.contains("Q-1-"), "Should contain error code");

    // Should have line:column reference
    assert!(text.contains(":1:"), "Should contain line:column reference");
}

#[test]
fn test_json_round_trip_serialization() {
    // Test that JSON output is valid and can be parsed
    let schema_yaml = quarto_yaml::parse(
        r#"
string:
  minLength: 5
"#,
    )
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    let doc_content = r#"name: "ab""#;
    let doc = quarto_yaml::parse_file(doc_content, "test.yaml").unwrap();

    let source_ctx = create_test_context("test.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let result = validate(&doc, &schema, &registry, &source_ctx);
    assert!(result.is_err());

    let error = result.unwrap_err();
    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
    let json = diagnostic.to_json();

    // Serialize to string and parse back
    let json_str = serde_json::to_string_pretty(&json).unwrap();
    let parsed: Value = serde_json::from_str(&json_str).unwrap();

    // Verify key fields are preserved
    assert_eq!(parsed["code"], json["code"]);
    assert_eq!(parsed["message"], json["message"]);
    assert_eq!(parsed["error_kind"], json["error_kind"]);
}

#[test]
fn test_multiple_errors_same_file() {
    // Schema with multiple constraints
    let schema_yaml = quarto_yaml::parse(
        r#"
object:
  properties:
    name:
      string:
        minLength: 3
    age:
      number:
        minimum: 0
        maximum: 150
  required:
    - name
    - age
"#,
    )
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    // Document with only age (missing name)
    let doc_content = r#"age: 25"#;
    let doc = quarto_yaml::parse_file(doc_content, "user.yaml").unwrap();

    let source_ctx = create_test_context("user.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let result = validate(&doc, &schema, &registry, &source_ctx);
    assert!(result.is_err());

    // For now, we only get one error (first failure)
    // But the architecture supports multiple errors
    let error = result.unwrap_err();
    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
    let json = diagnostic.to_json();

    // Verify the error has proper source_range pointing to same file
    assert_eq!(json["source_range"]["filename"], "user.yaml");
}

#[test]
fn test_custom_error_message_overrides_pattern_hint() {
    // A string schema with a non-obvious pattern plus an authored errorMessage.
    let custom_hint = r#"Must be "naive" or a standard time zone in the form Area/Location"#;
    let schema_yaml = quarto_yaml::parse(&format!(
        r#"
string:
  pattern: "^(naive|UTC)$"
  errorMessage: '{custom_hint}'
"#
    ))
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    let doc_content = r#"PST"#;
    let doc = quarto_yaml::parse_file(doc_content, "tz.yaml").unwrap();
    let source_ctx = create_test_context("tz.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let result = validate(&doc, &schema, &registry, &source_ctx);
    assert!(
        result.is_err(),
        "Validation should fail for pattern mismatch"
    );

    let error = result.unwrap_err();
    assert_eq!(
        error.custom_hint.as_deref(),
        Some(custom_hint),
        "custom_hint should be populated from the schema's errorMessage"
    );

    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);

    // The authored message replaces the generic pattern hint.
    let hints = diagnostic.hints();
    assert_eq!(
        hints,
        vec![custom_hint.to_string()],
        "the authored errorMessage should be the only hint"
    );
    assert!(
        !hints
            .iter()
            .any(|h| h.contains("matches the expected format")),
        "the generic pattern hint must not appear"
    );

    // The factual primary message is left intact.
    assert!(
        diagnostic.message().contains("does not match pattern"),
        "primary message should still report the factual failure, got: {}",
        diagnostic.message()
    );

    // JSON output carries the authored hint too.
    let json = diagnostic.to_json();
    assert_eq!(json["hints"][0], custom_hint);

    // Text output includes the authored message.
    let text = diagnostic.to_text(&source_ctx);
    assert!(
        text.contains(custom_hint),
        "text output should include the authored errorMessage, got:\n{}",
        text
    );
}

#[test]
fn test_custom_error_message_applies_to_any_failure_at_node() {
    // The override should apply to whatever failure occurs at the annotated
    // node, not just pattern mismatches. Here a type mismatch (number, not
    // string) trips the same authored message.
    let schema_yaml = quarto_yaml::parse(
        r#"
string:
  pattern: "^[a-z]+$"
  errorMessage: 'must be a lowercase identifier'
"#,
    )
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    let doc_content = r#"42"#;
    let doc = quarto_yaml::parse_file(doc_content, "id.yaml").unwrap();
    let source_ctx = create_test_context("id.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let result = validate(&doc, &schema, &registry, &source_ctx);
    assert!(result.is_err());

    let error = result.unwrap_err();
    assert_eq!(
        error.custom_hint.as_deref(),
        Some("must be a lowercase identifier")
    );

    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
    assert_eq!(
        diagnostic.hints(),
        vec!["must be a lowercase identifier".to_string()]
    );
}

#[test]
fn test_no_custom_error_message_uses_generic_hint() {
    // Without errorMessage, the generic hint is still produced.
    let schema_yaml = quarto_yaml::parse(
        r#"
string:
  pattern: "^[0-9]+$"
"#,
    )
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    let doc_content = r#"abc"#;
    let doc = quarto_yaml::parse_file(doc_content, "p.yaml").unwrap();
    let source_ctx = create_test_context("p.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let error = validate(&doc, &schema, &registry, &source_ctx).unwrap_err();
    assert_eq!(error.custom_hint, None);

    let diagnostic = ValidationDiagnostic::from_validation_error(&error, &source_ctx);
    assert_eq!(
        diagnostic.hints(),
        vec!["Check that the string matches the expected format?".to_string()]
    );
}

#[test]
fn test_custom_error_message_innermost_node_wins() {
    // An object property's own errorMessage should win over the outer
    // object's — the override binds to the schema node where the failure
    // occurs.
    let schema_yaml = quarto_yaml::parse(
        r#"
object:
  errorMessage: 'outer object message'
  properties:
    tz:
      string:
        pattern: "^UTC$"
        errorMessage: 'inner tz message'
"#,
    )
    .unwrap();
    let schema = Schema::from_yaml(&schema_yaml).unwrap();

    let doc_content = "tz: PST";
    let doc = quarto_yaml::parse_file(doc_content, "doc.yaml").unwrap();
    let source_ctx = create_test_context("doc.yaml", doc_content);
    let registry = quarto_yaml_validation::SchemaRegistry::new();

    let error = validate(&doc, &schema, &registry, &source_ctx).unwrap_err();
    assert_eq!(error.custom_hint.as_deref(), Some("inner tz message"));
}
