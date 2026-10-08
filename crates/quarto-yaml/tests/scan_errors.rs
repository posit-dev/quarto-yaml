//! Syntax errors carry a source location that maps to the enclosing file.

use quarto_source_map::FileId;
use quarto_yaml::{Error, SourceInfo, file_id_for_filename, parse, parse_file, parse_with_parent};

/// Parse `body` as if it were frontmatter following a `---\n` line, and
/// return the file text, the error message and the error's resolved
/// `(file_id, start, end)` byte range in the file.
fn frontmatter_error(body: &str) -> (String, String, (usize, usize, usize)) {
    let file = format!("---\n{body}---\n");
    let body_start = 4;
    let parent = SourceInfo::original(FileId(7), body_start, body_start + body.len());
    match parse_with_parent(body, parent).unwrap_err() {
        Error::ParseError {
            message,
            location: Some(location),
        } => {
            let range = location
                .resolve_byte_range()
                .expect("error location resolves to a byte range");
            (file, message, range)
        }
        other => panic!("expected a located ParseError, got {other:?}"),
    }
}

#[test]
fn scan_error_location_maps_to_file_offset() {
    let (file, message, (file_id, start, end)) =
        frontmatter_error("title: x\nauthor: [a, b\nformat: html\n");
    assert_eq!(message, "illegal placement of ':' indicator");
    assert_eq!(file_id, 7);
    assert_eq!(start, file.find("format:").unwrap() + "format".len());
    assert_eq!(&file[start..end], ":");
}

#[test]
fn scan_error_location_is_a_byte_offset_after_non_ascii() {
    let (file, message, (_, start, end)) =
        frontmatter_error("title: \"héllo ✓\"\nauthor: [a, b\nformat: html\n");
    assert_eq!(message, "illegal placement of ':' indicator");
    assert_eq!(start, file.find("format:").unwrap() + "format".len());
    assert_eq!(&file[start..end], ":");
}

#[test]
fn scan_error_at_eof_is_clamped_to_source_end() {
    let body = "title: x\nauthor: [a, b";
    let parent = SourceInfo::original(FileId(7), 4, 4 + body.len());
    let Error::ParseError {
        message,
        location: Some(location),
    } = parse_with_parent(body, parent).unwrap_err()
    else {
        panic!("expected a located ParseError");
    };
    assert_eq!(
        message,
        "while parsing a flow sequence, expected ',' or ']'"
    );
    let (_, start, end) = location.resolve_byte_range().unwrap();
    assert_eq!((start, end), (4 + body.len(), 4 + body.len()));
}

#[test]
fn scan_error_in_parse_file_uses_filename_file_id() {
    let content = "a: [1\nb: 2\n";
    let Error::ParseError {
        message,
        location: Some(location),
    } = parse_file(content, "config.yml").unwrap_err()
    else {
        panic!("expected a located ParseError");
    };
    assert!(!message.contains(" at byte "), "{message}");
    assert_eq!(
        location.root_file_id(),
        Some(file_id_for_filename("config.yml"))
    );
    let (_, start, end) = location.resolve_byte_range().unwrap();
    assert_eq!(&content[start..end], ":");
}

#[test]
fn scan_error_in_parse_without_parent() {
    let content = "a: [1\nb: 2\n";
    let Error::ParseError {
        location: Some(location),
        ..
    } = parse(content).unwrap_err()
    else {
        panic!("expected a located ParseError");
    };
    assert_eq!(location, SourceInfo::original(FileId(0), 7, 8));
}

#[test]
fn empty_document_error_is_located_at_content_start() {
    let parent = SourceInfo::original(FileId(7), 4, 4);
    let Error::ParseError {
        message,
        location: Some(location),
    } = parse_with_parent("", parent).unwrap_err()
    else {
        panic!("expected a located ParseError");
    };
    assert_eq!(message, "No YAML document found");
    assert_eq!(location.resolve_byte_range(), Some((7, 4, 4)));
}
