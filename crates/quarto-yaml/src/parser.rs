//! YAML parser that builds YamlWithSourceInfo trees.

use crate::{Error, Result, SourceInfo, YamlHashEntry, YamlWithSourceInfo};
use std::cell::Cell;
use yaml_rust2::Yaml;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};

/// The handle of the standard YAML tags, written `!!str`, `!!int`, … in source.
const STANDARD_TAG_HANDLE: &str = "tag:yaml.org,2002:";

/// Parse YAML from a string, producing a YamlWithSourceInfo tree.
///
/// This parses a single YAML document. If the input contains multiple documents,
/// only the first one will be parsed.
///
/// # Example
///
/// ```rust
/// use quarto_yaml::parse;
///
/// let yaml = parse("title: My Document").unwrap();
/// assert!(yaml.is_hash());
/// ```
///
/// # Errors
///
/// Returns an error if the YAML is invalid or if parsing fails.
pub fn parse(content: &str) -> Result<YamlWithSourceInfo> {
    parse_impl(content, None, None)
}

/// Parse YAML from a string with an associated filename.
///
/// The filename is included in source location information for better
/// error reporting.
///
/// # Example
///
/// ```rust
/// use quarto_yaml::parse_file;
///
/// let yaml = parse_file("title: My Document", "config.yaml").unwrap();
/// // Filename tracking will be added in a future update
/// assert!(yaml.source_info.end_offset() > 0);
/// ```
///
/// # Errors
///
/// Returns an error if the YAML is invalid or if parsing fails.
pub fn parse_file(content: &str, filename: &str) -> Result<YamlWithSourceInfo> {
    parse_impl(content, Some(filename), None)
}

/// Parse YAML that was extracted from a parent document.
///
/// This function is used when parsing YAML that is a substring of a larger
/// document (e.g., YAML frontmatter extracted from a .qmd file). The resulting
/// YamlWithSourceInfo will have Substring mappings that track back to the
/// parent document.
///
/// # Arguments
///
/// * `content` - The YAML string to parse
/// * `parent` - Source information for the parent document from which this YAML was extracted
///
/// # Example
///
/// ```rust,no_run
/// use quarto_yaml::{parse_with_parent, SourceInfo};
/// use quarto_source_map::{FileId, Location, Range};
///
/// // Create parent source info for a .qmd file
/// let parent = SourceInfo::from_range(
///     FileId(1),
///     Range {
///         start: Location { offset: 0, row: 0, column: 0 },
///         end: Location { offset: 1000, row: 50, column: 0 },
///     }
/// );
///
/// // Parse YAML frontmatter (extracted from parent document at offset 10-50)
/// let yaml_content = "title: My Document\nauthor: John";
/// let yaml = parse_with_parent(yaml_content, parent).unwrap();
///
/// // The yaml now has Substring mappings tracking back to the parent
/// ```
///
/// # Errors
///
/// Returns an error if the YAML is invalid or if parsing fails.
pub fn parse_with_parent(content: &str, parent: SourceInfo) -> Result<YamlWithSourceInfo> {
    parse_impl(content, None, Some(parent))
}

/// Derive the [`quarto_source_map::FileId`] used by [`parse_file`]
/// from a filename string.
///
/// This is the same hash that `parse_file` (and `parse_impl` below)
/// computes when no explicit parent SourceInfo is supplied. Exposed
/// so callers building a [`quarto_source_map::SourceContext`] for
/// ariadne rendering can look up the right `FileId` for a given
/// on-disk file without re-hashing inline.
///
/// Stability: this is part of the public API and is relied upon by
/// the diagnostic layer to bind file content to FileIds at render
/// time. Don't change the hash recipe without bumping consumers.
pub fn file_id_for_filename(filename: &str) -> quarto_source_map::FileId {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    filename.hash(&mut hasher);
    quarto_source_map::FileId(hasher.finish() as usize)
}

fn parse_impl(
    content: &str,
    filename: Option<&str>,
    parent: Option<SourceInfo>,
) -> Result<YamlWithSourceInfo> {
    // If parent is not provided but filename is, create a parent SourceInfo for the file
    let parent = parent.or_else(|| {
        filename.map(|name| {
            let file_id = file_id_for_filename(name);

            // Create SourceInfo for the entire file content
            use quarto_source_map::{Location, Range};
            SourceInfo::from_range(
                file_id,
                Range {
                    start: Location {
                        offset: 0,
                        row: 0,
                        column: 0,
                    },
                    end: Location {
                        offset: content.len(),
                        row: content.lines().count().saturating_sub(1),
                        column: content.lines().last().map_or(0, |l| l.len()),
                    },
                },
            )
        })
    });

    let mut parser = Parser::new_from_str(content);
    let mut builder = YamlBuilder::new(content, parent);

    parser
        .load(&mut builder, false) // false = single document only
        .map_err(Error::from)?;

    builder.result()
}

/// Helper function to create a contiguous span from start to end positions.
/// This is used for entry_span which should cover from key start to value end.
fn create_contiguous_span(start_info: &SourceInfo, end_info: &SourceInfo) -> SourceInfo {
    // Extract the actual start and end offsets, handling the different SourceInfo variants
    match (start_info, end_info) {
        (
            SourceInfo::Original {
                file_id: start_file,
                start_offset: start,
                ..
            },
            SourceInfo::Original {
                file_id: end_file,
                end_offset: end,
                ..
            },
        ) => {
            // Both are Original from the same file - create a single Original span
            assert_eq!(
                start_file, end_file,
                "Key and value must be from the same file"
            );
            SourceInfo::original(*start_file, *start, *end)
        }
        (
            SourceInfo::Substring {
                parent: start_parent,
                start_offset: start,
                ..
            },
            SourceInfo::Substring {
                end_offset: end, ..
            },
        ) => {
            // Both are Substrings - they should have the same parent
            // Use the first parent (they should be equivalent even if not the same Rc)
            SourceInfo::substring((**start_parent).clone(), *start, *end)
        }
        _ => {
            // Mixed types or Concat - fall back to combine which creates a Concat
            // This shouldn't happen in normal YAML parsing but handle it gracefully
            start_info.combine(end_info)
        }
    }
}

/// Builder that implements MarkedEventReceiver to construct YamlWithSourceInfo.
struct YamlBuilder<'a> {
    /// The source text being parsed
    source: &'a str,

    /// Whether character indices are also byte offsets.
    source_is_ascii: bool,

    /// Current `(character index, byte offset)` in `source`.
    cursor: Cell<(usize, usize)>,

    /// Optional parent source info for substring tracking
    parent: Option<SourceInfo>,

    /// Stack of nodes being constructed
    stack: Vec<BuildNode>,

    /// The completed root node
    root: Option<YamlWithSourceInfo>,
}

/// A node being constructed during parsing.
enum BuildNode {
    /// Building a sequence
    Sequence {
        /// Byte offset of the opening marker.
        start_offset: usize,
        items: Vec<YamlWithSourceInfo>,
    },

    /// Building a mapping
    Mapping {
        /// Byte offset of the opening marker.
        start_offset: usize,
        entries: Vec<(YamlWithSourceInfo, Option<YamlWithSourceInfo>)>,
    },
}

impl<'a> YamlBuilder<'a> {
    fn new(source: &'a str, parent: Option<SourceInfo>) -> Self {
        Self {
            source,
            source_is_ascii: source.is_ascii(),
            cursor: Cell::new((0, 0)),
            parent,
            stack: Vec::new(),
            root: None,
        }
    }

    /// Convert a marker's character index to a byte offset.
    ///
    /// Despite its documentation, `Marker::index()` counts characters, while
    /// `SourceInfo` spans must contain byte offsets.
    ///
    /// A cursor avoids a lookup table. It can move backward because some
    /// events, such as a key after `MappingStart`, have an earlier marker.
    fn byte_offset(&self, marker: &Marker) -> usize {
        self.byte_offset_of_char(marker.index())
    }

    /// Convert a character index to a byte offset, clamping at the source end.
    fn byte_offset_of_char(&self, target: usize) -> usize {
        if self.source_is_ascii {
            return target.min(self.source.len());
        }

        let bytes = self.source.as_bytes();
        let is_boundary = |i: usize| i >= bytes.len() || (bytes[i] & 0xC0) != 0x80;

        let (mut chars, mut offset) = self.cursor.get();

        // Stop at the source end to clamp out-of-range markers.
        while chars < target && offset < bytes.len() {
            offset += 1;
            while !is_boundary(offset) {
                offset += 1;
            }
            chars += 1;
        }

        while chars > target {
            offset -= 1;
            while !is_boundary(offset) {
                offset -= 1;
            }
            chars -= 1;
        }

        self.cursor.set((chars, offset));
        offset
    }

    fn collection_end(&self, end_marker: &Marker, close: u8) -> usize {
        let end = self.byte_offset(end_marker);
        if self.source.as_bytes().get(end) == Some(&close) {
            end + 1
        } else {
            end
        }
    }

    fn result(self) -> Result<YamlWithSourceInfo> {
        self.root.ok_or_else(|| Error::ParseError {
            message: "No YAML document found".into(),
            location: None,
        })
    }

    fn push_complete(&mut self, node: YamlWithSourceInfo) {
        if self.stack.is_empty() {
            // This is the root
            self.root = Some(node);
            return;
        }

        // Add to the parent node
        match self.stack.last_mut().unwrap() {
            BuildNode::Sequence { items, .. } => {
                items.push(node);
            }
            BuildNode::Mapping { entries, .. } => {
                if let Some((_, value)) = entries.last_mut() {
                    if value.is_none() {
                        *value = Some(node);
                    } else {
                        // This is a new key
                        entries.push((node, None));
                    }
                } else {
                    // First key
                    entries.push((node, None));
                }
            }
        }
    }

    fn make_source_info(&self, marker: &Marker, len: usize) -> SourceInfo {
        let start_offset = self.byte_offset(marker);
        let end_offset = start_offset + len;

        if let Some(ref parent) = self.parent {
            // We're parsing a substring - create a Substring mapping
            SourceInfo::substring(parent.clone(), start_offset, end_offset)
        } else {
            // We're parsing an original file - create an Original mapping.
            // SourceInfo records byte offsets only; rows and columns are
            // derived from them (and the file's content) by SourceContext at
            // render time, so there is nothing to compute here.
            SourceInfo::original(
                quarto_source_map::FileId(0), // Dummy FileId for now
                start_offset,
                end_offset,
            )
        }
    }

    fn make_source_info_at_offset(&self, start_offset: usize, len: usize) -> SourceInfo {
        let end_offset = start_offset + len;

        if let Some(ref parent) = self.parent {
            // We're parsing a substring - create a Substring mapping
            SourceInfo::substring(parent.clone(), start_offset, end_offset)
        } else {
            // We're parsing an original file - create an Original mapping.
            // Offsets are all SourceInfo carries; see make_source_info above.
            SourceInfo::original(quarto_source_map::FileId(0), start_offset, end_offset)
        }
    }

    /// Compute how many bytes of source a scalar occupies.
    ///
    /// The scalar's *value* is not a reliable measure: quotes are not part of
    /// it, escapes are already decoded, and line breaks in multi-line scalars
    /// have been folded away. So the extent is measured against the source
    /// text, using the style to know what to look for. Quoted scalars include
    /// their quotes, which is what a diagnostic wants to underline.
    fn compute_scalar_len(&self, marker: &Marker, value: &str, style: TScalarStyle) -> usize {
        let start = self.byte_offset(marker);
        let Some(rest) = self.source.get(start..) else {
            return 0;
        };

        let len = match style {
            TScalarStyle::Plain => plain_scalar_len(rest, value),
            TScalarStyle::SingleQuoted => quoted_scalar_len(rest, b'\'').unwrap_or(value.len()),
            TScalarStyle::DoubleQuoted => quoted_scalar_len(rest, b'"').unwrap_or(value.len()),
            // For block scalars the marker points at the first content
            // character, so the `|`/`>` header is not covered by the span.
            TScalarStyle::Literal | TScalarStyle::Folded => block_scalar_len(rest, marker.col()),
        };

        len.min(rest.len())
    }

    /// Find the source extent of the tag preceding a scalar value.
    ///
    /// When yaml-rust2 emits a Scalar event with a tag, the marker points to the
    /// start of the VALUE, not the tag, and the tag's source spelling is not
    /// recoverable from the parsed `Tag` (`!!str`, `!str` and
    /// `!<tag:yaml.org,2002:str>` can all arrive with suffix `"str"`). So we
    /// look at the whitespace-delimited tokens before the value: a tag is a
    /// single token starting with `!`.
    ///
    /// For example, in "key: !expr x + 1", if marker points to "x", we need to
    /// find "!expr" which comes before it.
    ///
    /// Returns the byte offset of the '!' character and the tag's byte length.
    fn find_tag_span(&self, value_marker: &Marker) -> Option<(usize, usize)> {
        let mut before = &self.source[..self.byte_offset(value_marker)];

        // A node can carry both a tag and an anchor, in either order
        // (`!tag &name value` and `&name !tag value` are both valid), so walk
        // back over the properties instead of looking only at the token next to
        // the value.
        loop {
            // Back up over the whitespace before the value, then over the
            // token itself.
            let token_end = before.trim_end().len();
            let token_start = before[..token_end]
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map_or(0, |(i, c)| i + c.len_utf8());
            let token = &before[token_start..token_end];

            // In flow context a property can be glued to the opening indicator
            // (`[!tag a]`), which is not part of the property itself.
            let property = token.trim_start_matches(['[', '{', ',']);
            let offset = token_start + (token.len() - property.len());

            if property.starts_with('!') {
                return Some((offset, property.len()));
            }
            if !property.starts_with('&') {
                // Not a node property, so we've walked past the tag without
                // finding it.
                return None;
            }
            before = &before[..token_start];
        }
    }

    /// Create SourceInfo for a tag at a specific byte offset.
    fn make_tag_source_info(&self, tag_start_offset: usize, tag_len: usize) -> SourceInfo {
        let end_offset = tag_start_offset + tag_len;

        if let Some(ref parent) = self.parent {
            // We're parsing a substring - create a Substring mapping
            SourceInfo::substring(parent.clone(), tag_start_offset, end_offset)
        } else {
            // We're parsing an original file - create an Original mapping
            // For row/column, we'd need to scan the source, but for now use approximations
            SourceInfo::original(quarto_source_map::FileId(0), tag_start_offset, end_offset)
        }
    }
}

impl<'a> MarkedEventReceiver for YamlBuilder<'a> {
    fn on_event(&mut self, ev: Event, marker: Marker) {
        match ev {
            Event::Nothing => {}

            Event::StreamStart => {}
            Event::StreamEnd => {}
            Event::DocumentStart => {}
            Event::DocumentEnd => {}

            Event::Scalar(value, style, _anchor_id, tag) => {
                // Capture tag information if present
                let tag_info = tag.as_ref().map(|t| {
                    // The marker points to the start of the VALUE, not the tag
                    // We need to find where the tag actually is in the source
                    if let Some((tag_offset, tag_len)) = self.find_tag_span(&marker) {
                        let tag_source_info = self.make_tag_source_info(tag_offset, tag_len);
                        (t.suffix.clone(), tag_source_info)
                    } else {
                        // Fallback: if we can't find the tag, point at the value
                        // and guess the length from the suffix. That's wrong,
                        // but it stays inside the source, so consumers can
                        // still slice with it.
                        let guess = 1 + t.suffix.len();
                        let len =
                            guess.min(self.source.len().saturating_sub(self.byte_offset(&marker)));
                        let tag_source_info = self.make_source_info(&marker, len);
                        (t.suffix.clone(), tag_source_info)
                    }
                });

                // Compute source info for the value itself
                // The marker points to the start of the value
                let len = self.compute_scalar_len(&marker, &value, style);
                let source_info = self.make_source_info(&marker, len);

                // Create the Yaml value
                let yaml = resolve_scalar(&value, style, tag.as_ref());
                let node = YamlWithSourceInfo::new_scalar_with_tag(yaml, source_info, tag_info);

                self.push_complete(node);
            }

            Event::SequenceStart(_anchor_id, _tag) => {
                self.stack.push(BuildNode::Sequence {
                    start_offset: self.byte_offset(&marker),
                    items: Vec::new(),
                });
            }

            Event::SequenceEnd => {
                let build_node = self.stack.pop().expect("SequenceEnd without SequenceStart");

                if let BuildNode::Sequence {
                    start_offset,
                    items,
                } = build_node
                {
                    // SequenceEnd points at `]` for flow sequences.
                    let end = self.collection_end(&marker, b']');
                    let source_info = self
                        .make_source_info_at_offset(start_offset, end.saturating_sub(start_offset));

                    // Build the Yaml::Array
                    let yaml_items: Vec<Yaml> = items.iter().map(|n| n.yaml.clone()).collect();
                    let yaml = Yaml::Array(yaml_items);

                    let node = YamlWithSourceInfo::new_array(yaml, source_info, items);
                    self.push_complete(node);
                } else {
                    panic!("Expected Sequence build node");
                }
            }

            Event::MappingStart(_anchor_id, _tag) => {
                self.stack.push(BuildNode::Mapping {
                    start_offset: self.byte_offset(&marker),
                    entries: Vec::new(),
                });
            }

            Event::MappingEnd => {
                let build_node = self.stack.pop().expect("MappingEnd without MappingStart");

                if let BuildNode::Mapping {
                    start_offset,
                    entries,
                } = build_node
                {
                    // Build the hash entries
                    let mut hash_entries = Vec::new();
                    let mut yaml_pairs = Vec::new();

                    for (key, value) in entries {
                        let value = value.expect("Mapping entry without value");

                        // Create YamlHashEntry
                        let key_span = key.source_info.clone();
                        let value_span = value.source_info.clone();

                        // Entry span from key start to value end
                        // Create a contiguous span (not a Concat) from key start to value end
                        let entry_span = create_contiguous_span(&key_span, &value_span);

                        hash_entries.push(YamlHashEntry::new(
                            key.clone(),
                            value.clone(),
                            key_span,
                            value_span,
                            entry_span,
                        ));

                        yaml_pairs.push((key.yaml.clone(), value.yaml.clone()));
                    }

                    // MappingEnd points at `}` for flow mappings.
                    let end = self.collection_end(&marker, b'}');
                    let source_info = if self.source.as_bytes().get(start_offset) == Some(&b'{') {
                        self.make_source_info_at_offset(
                            start_offset,
                            end.saturating_sub(start_offset),
                        )
                    } else if let Some(first_entry) = hash_entries.first() {
                        let first_key_start = first_entry.key.source_info.start_offset();
                        self.make_source_info_at_offset(
                            first_key_start,
                            end.saturating_sub(first_key_start),
                        )
                    } else {
                        self.make_source_info_at_offset(
                            start_offset,
                            end.saturating_sub(start_offset),
                        )
                    };

                    // Build the Yaml::Hash
                    let yaml = Yaml::Hash(yaml_pairs.into_iter().collect());

                    let node = YamlWithSourceInfo::new_hash(yaml, source_info, hash_entries);
                    self.push_complete(node);
                } else {
                    panic!("Expected Mapping build node");
                }
            }

            Event::Alias(_anchor_id) => {
                // For now, we don't support aliases
                // We could add support later by tracking anchors
                let source_info = self.make_source_info(&marker, 0);
                let node = YamlWithSourceInfo::new_scalar(Yaml::Null, source_info);
                self.push_complete(node);
            }
        }
    }
}

/// Resolve a scalar to a `Yaml` value the way the YAML 1.2 spec says to.
///
/// Three things decide the type, in this order:
///
/// 1. An explicit standard tag (`!!int`, `!!str`, …). An explicit tag *is* the
///    node's type, so it wins over everything else: `!!int "7"` is the integer
///    7 and `!!str 5` is the string `"5"`. A value that does not fit its tag
///    (`!!int abc`) becomes `Yaml::BadValue`, matching yaml-rust2's loader.
/// 2. The scalar's style. Quoting is precisely how YAML says "this is text,
///    not a number", so any non-plain scalar — quoted or block — is a string.
/// 3. Failing both, the core schema resolves the plain text (see
///    [`resolve_plain_scalar`]).
///
/// Application-specific tags (`!expr`, `!path`, …) do not affect resolution.
/// Every tag, standard or not, is reported in [`YamlWithSourceInfo::tag`] with
/// its own span, for consumers to interpret.
fn resolve_scalar(value: &str, style: TScalarStyle, tag: Option<&Tag>) -> Yaml {
    if let Some(suffix) = tag.and_then(standard_tag_suffix) {
        return resolve_tagged_scalar(value, suffix);
    }

    if style != TScalarStyle::Plain {
        return Yaml::String(value.to_string());
    }

    resolve_plain_scalar(value)
}

/// The suffix of a standard (`tag:yaml.org,2002:`) tag, if this is one.
///
/// The `!!x` shorthand arrives with the standard handle and suffix `x`; the
/// verbatim form `!<tag:yaml.org,2002:x>` arrives with an empty handle and the
/// whole URI as the suffix.
fn standard_tag_suffix(tag: &Tag) -> Option<&str> {
    if tag.handle == STANDARD_TAG_HANDLE {
        Some(&tag.suffix)
    } else if tag.handle.is_empty() {
        tag.suffix.strip_prefix(STANDARD_TAG_HANDLE)
    } else {
        None
    }
}

/// Resolve a scalar carrying an explicit standard tag, e.g. `!!int 5`.
///
/// Standard tags that don't apply to scalars (`!!map`, `!!binary`, …) leave the
/// text alone; a value that contradicts its tag becomes `Yaml::BadValue`.
fn resolve_tagged_scalar(value: &str, suffix: &str) -> Yaml {
    match suffix {
        "str" => Yaml::String(value.to_string()),
        "bool" => match value {
            "true" | "True" | "TRUE" => Yaml::Boolean(true),
            "false" | "False" | "FALSE" => Yaml::Boolean(false),
            _ => Yaml::BadValue,
        },
        "int" => match parse_core_schema_int(value) {
            Some(i) => Yaml::Integer(i),
            None => Yaml::BadValue,
        },
        "float" => {
            if is_yaml_float(value) {
                Yaml::Real(value.to_string())
            } else {
                Yaml::BadValue
            }
        }
        "null" => match value {
            "null" | "Null" | "NULL" | "~" | "" => Yaml::Null,
            _ => Yaml::BadValue,
        },
        _ => Yaml::String(value.to_string()),
    }
}

/// Parse an integer in one of the YAML 1.2 core schema's three `!!int` forms:
/// `[-+]?[0-9]+`, `0o[0-7]+` or `0x[0-9a-fA-F]+`.
///
/// The prefixed forms are lower-case and unsigned — `0X1f` and `-0x1f` are
/// strings — and `i64::from_str_radix` accepts a sign the schema doesn't, so
/// the digit runs are checked by hand rather than left to the parse. A value
/// that matches a form but overflows `i64` is `None`, like any non-match.
fn parse_core_schema_int(value: &str) -> Option<i64> {
    if let Some(digits) = value.strip_prefix("0x") {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return i64::from_str_radix(digits, 16).ok();
    }
    if let Some(digits) = value.strip_prefix("0o") {
        if digits.is_empty() || !digits.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            return None;
        }
        return i64::from_str_radix(digits, 8).ok();
    }
    value.parse::<i64>().ok()
}

/// Resolve an untagged plain scalar with the YAML 1.2 core schema.
///
/// This handles type inference: integers (in the three core-schema forms —
/// see [`parse_core_schema_int`]), floats, booleans, null, and strings. Note
/// that the core schema recognises only `true`/`false` (and their `True`,
/// `TRUE` spellings) as booleans — YAML 1.1's `yes`, `no`, `on` and `off` are
/// plain strings in 1.2.
fn resolve_plain_scalar(value: &str) -> Yaml {
    // Try to parse as integer
    if let Some(i) = parse_core_schema_int(value) {
        return Yaml::Integer(i);
    }

    // Try to parse as float, including the YAML 1.2 core-schema float
    // spellings (`.inf`, `-.inf`, `+.inf`, `.nan`, and case variants) that
    // Rust's `f64::from_str` does not accept.
    if is_yaml_float(value) {
        return Yaml::Real(value.to_string());
    }

    // Check for boolean
    match value {
        "true" | "True" | "TRUE" => {
            return Yaml::Boolean(true);
        }
        "false" | "False" | "FALSE" => {
            return Yaml::Boolean(false);
        }
        "null" | "Null" | "NULL" | "~" | "" => {
            return Yaml::Null;
        }
        _ => {}
    }

    // Default to string
    Yaml::String(value.to_string())
}

/// Length in bytes of a quoted scalar in the source, quotes included.
///
/// `rest` must start at the opening quote. Returns `None` if it doesn't, or if
/// the closing quote is missing (which yaml-rust2 would have rejected already).
fn quoted_scalar_len(rest: &str, quote: u8) -> Option<usize> {
    let bytes = rest.as_bytes();
    if bytes.first() != Some(&quote) {
        return None;
    }

    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && quote == b'"' {
            // Escapes only exist in double-quoted scalars, where the escaped
            // byte is always ASCII, so skipping it can't land mid-character.
            i += 2;
        } else if bytes[i] == quote {
            // A single-quoted scalar escapes its quote by doubling it.
            if quote == b'\'' && bytes.get(i + 1) == Some(&quote) {
                i += 2;
            } else {
                return Some(i + 1);
            }
        } else {
            i += 1;
        }
    }

    None
}

/// Length in bytes of a plain scalar in the source.
///
/// A plain scalar can span lines, and the parser folds those breaks into single
/// spaces (or newlines, for blank lines), so the value is shorter than its
/// source. Since plain scalars have no escapes, the value can be walked back
/// onto the source character by character, letting a folded space match a run
/// of source whitespace. If the two ever fail to line up we fall back to the
/// value's length rather than guess.
fn plain_scalar_len(rest: &str, value: &str) -> usize {
    // Bytes of `rest` consumed so far, and the end of the last content
    // character matched (which excludes any trailing fold whitespace).
    let mut consumed = 0;
    let mut end = 0;

    for ch in value.chars() {
        let tail = &rest[consumed..];

        // A space or newline in the value may be a folded line break, which is
        // a whole run of whitespace in the source.
        if matches!(ch, ' ' | '\n') {
            let run: usize = tail
                .chars()
                .take_while(|c| c.is_whitespace())
                .map(char::len_utf8)
                .sum();
            if run > 0 && tail[..run].contains('\n') {
                consumed += run;
                continue;
            }
        }

        if !tail.starts_with(ch) {
            return value.len();
        }
        consumed += ch.len_utf8();
        end = consumed;
    }

    end
}

/// Length in bytes of a block scalar's content in the source.
///
/// `rest` starts at the block's first content character, which sits at column
/// `indent`; the block continues over following lines while they are blank or
/// indented at least that far. Trailing blank lines are left out of the span.
fn block_scalar_len(rest: &str, indent: usize) -> usize {
    let mut consumed = 0;
    let mut end = 0;
    let mut first = true;

    while consumed < rest.len() {
        let line_end = rest[consumed..]
            .find('\n')
            .map_or(rest.len(), |i| consumed + i);
        let line = &rest[consumed..line_end];
        let blank = line.trim().is_empty();

        if !first && !blank && line.len() - line.trim_start().len() < indent {
            break;
        }
        if !blank {
            end = consumed + line.trim_end().len();
        }

        consumed = (line_end + 1).min(rest.len());
        first = false;
    }

    end
}

/// Returns `true` if `value` is a YAML 1.2 core-schema float.
fn is_yaml_float(value: &str) -> bool {
    match value {
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => true,
        "-.inf" | "-.Inf" | "-.INF" => true,
        ".nan" | ".NaN" | ".NAN" => true,
        _ => value.bytes().any(|b| b.is_ascii_digit()) && value.parse::<f64>().is_ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scalar() {
        let yaml = parse("hello").unwrap();
        assert!(yaml.is_scalar());
        assert_eq!(yaml.yaml.as_str(), Some("hello"));
    }

    #[test]
    fn test_parse_integer() {
        let yaml = parse("42").unwrap();
        assert!(yaml.is_scalar());
        assert_eq!(yaml.yaml.as_i64(), Some(42));
    }

    #[test]
    fn test_parse_yaml_float_special_forms() {
        // YAML 1.2 core-schema float spellings must resolve to Yaml::Real,
        // not Yaml::String. See tidyverse/data-dict#47.
        for (text, expected) in [
            (".inf", f64::INFINITY),
            ("+.inf", f64::INFINITY),
            (".Inf", f64::INFINITY),
            (".INF", f64::INFINITY),
            ("+.INF", f64::INFINITY),
            ("-.inf", f64::NEG_INFINITY),
            ("-.Inf", f64::NEG_INFINITY),
            ("-.INF", f64::NEG_INFINITY),
        ] {
            let yaml = parse(text).unwrap();
            assert!(
                matches!(yaml.yaml, Yaml::Real(_)),
                "{text:?} should parse as Yaml::Real, got {:?}",
                yaml.yaml
            );
            assert_eq!(
                yaml.yaml.as_f64(),
                Some(expected),
                "{text:?} should evaluate to {expected}"
            );
        }

        for text in [".nan", ".NaN", ".NAN"] {
            let yaml = parse(text).unwrap();
            assert!(
                matches!(yaml.yaml, Yaml::Real(_)),
                "{text:?} should parse as Yaml::Real, got {:?}",
                yaml.yaml
            );
            assert!(
                yaml.yaml.as_f64().unwrap().is_nan(),
                "{text:?} should evaluate to NaN"
            );
        }

        // Bare `inf` / `nan` (no leading dot) are NOT YAML floats — they stay strings.
        for text in ["inf", "nan", "infinity"] {
            let yaml = parse(text).unwrap();
            assert!(
                matches!(yaml.yaml, Yaml::String(_)),
                "{text:?} should stay a Yaml::String, got {:?}",
                yaml.yaml
            );
        }
    }

    #[test]
    fn test_parse_boolean() {
        let yaml = parse("true").unwrap();
        assert!(yaml.is_scalar());
        assert_eq!(yaml.yaml.as_bool(), Some(true));
    }

    #[test]
    fn test_parse_array() {
        let yaml = parse("[1, 2, 3]").unwrap();
        assert!(yaml.is_array());
        assert_eq!(yaml.len(), 3);

        let items = yaml.as_array().unwrap();
        assert_eq!(items[0].yaml.as_i64(), Some(1));
        assert_eq!(items[1].yaml.as_i64(), Some(2));
        assert_eq!(items[2].yaml.as_i64(), Some(3));
    }

    #[test]
    fn test_parse_hash() {
        let yaml = parse("title: My Document\nauthor: John Doe").unwrap();
        assert!(yaml.is_hash());
        assert_eq!(yaml.len(), 2);

        let title = yaml.get_hash_value("title").unwrap();
        assert_eq!(title.yaml.as_str(), Some("My Document"));

        let author = yaml.get_hash_value("author").unwrap();
        assert_eq!(author.yaml.as_str(), Some("John Doe"));
    }

    #[test]
    fn test_nested_structure() {
        let yaml = parse(
            r#"
project:
  title: My Project
  authors:
    - Alice
    - Bob
"#,
        )
        .unwrap();

        assert!(yaml.is_hash());

        let project = yaml.get_hash_value("project").unwrap();
        assert!(project.is_hash());

        let authors = project.get_hash_value("authors").unwrap();
        assert!(authors.is_array());
        assert_eq!(authors.len(), 2);
    }

    #[test]
    fn test_source_info_tracking() {
        let yaml = parse("title: My Document").unwrap();

        // Check that source info is present
        // Note: row/column are 0-indexed in the new system
        assert!(yaml.source_info.start_offset() < yaml.source_info.end_offset());

        let title = yaml.get_hash_value("title").unwrap();
        // Verify the title value has a valid range
        assert!(title.source_info.start_offset() < title.source_info.end_offset());
    }

    #[test]
    fn test_parse_with_filename() {
        let yaml = parse_file("title: Test", "config.yaml").unwrap();
        assert!(yaml.source_info.end_offset() > 0);

        // Verify that we're now using Substring mapping for files
        match &yaml.source_info {
            SourceInfo::Substring { .. } => {
                // Expected: Substring mapping to parent file
            }
            _ => panic!("Expected Substring mapping for file parsing"),
        }
    }

    #[test]
    fn test_parse_with_parent_simple() {
        use quarto_source_map::{FileId, Location, Range};

        // Simulate extracting YAML from a .qmd file at offset 100-150
        let parent = SourceInfo::from_range(
            FileId(42),
            Range {
                start: Location {
                    offset: 100,
                    row: 5,
                    column: 0,
                },
                end: Location {
                    offset: 150,
                    row: 8,
                    column: 0,
                },
            },
        );

        let yaml_content = "title: My Document\nauthor: John";
        let yaml = parse_with_parent(yaml_content, parent).unwrap();

        // Verify root has Substring mapping
        match &yaml.source_info {
            SourceInfo::Substring { parent: p, .. } => {
                // Parent should point to our original parent
                match p.as_ref() {
                    SourceInfo::Original { file_id, .. } => {
                        assert_eq!(file_id.0, 42);
                    }
                    _ => panic!("Expected parent to have Original mapping"),
                }
            }
            _ => panic!("Expected Substring mapping"),
        }
    }

    #[test]
    fn test_parse_with_parent_nested() {
        use quarto_source_map::{FileId, Location, Range};

        // Parent file
        let parent = SourceInfo::from_range(
            FileId(1),
            Range {
                start: Location {
                    offset: 0,
                    row: 0,
                    column: 0,
                },
                end: Location {
                    offset: 500,
                    row: 20,
                    column: 0,
                },
            },
        );

        let yaml_content = r#"
project:
  title: My Project
  authors:
    - Alice
    - Bob
"#;
        let yaml = parse_with_parent(yaml_content, parent).unwrap();

        // Get nested values
        let project = yaml
            .get_hash_value("project")
            .expect("project key not found");
        let title = project
            .get_hash_value("title")
            .expect("title key not found");
        let authors = project
            .get_hash_value("authors")
            .expect("authors key not found");

        // All should have Substring mappings
        assert!(matches!(project.source_info, SourceInfo::Substring { .. }));
        assert!(matches!(title.source_info, SourceInfo::Substring { .. }));
        assert!(matches!(authors.source_info, SourceInfo::Substring { .. }));

        // Array elements should also have Substring mappings
        if let Some(items) = authors.as_array() {
            assert_eq!(items.len(), 2);
            assert!(matches!(items[0].source_info, SourceInfo::Substring { .. }));
            assert!(matches!(items[1].source_info, SourceInfo::Substring { .. }));
        } else {
            panic!("Expected array for authors");
        }
    }

    #[test]
    fn test_substring_offset_tracking() {
        use quarto_source_map::{FileId, Location, Range};

        // Parent document
        let parent_content = "---\ntitle: Test\nauthor: John\n---\n\nDocument content";
        let parent = SourceInfo::from_range(
            FileId(1),
            Range {
                start: Location {
                    offset: 0,
                    row: 0,
                    column: 0,
                },
                end: Location {
                    offset: parent_content.len(),
                    row: 4,
                    column: 0,
                },
            },
        );

        // YAML frontmatter (offset 4-31 in parent)
        let yaml_content = "title: Test\nauthor: John";
        let yaml = parse_with_parent(yaml_content, parent).unwrap();

        // Get title value
        let title = yaml.get_hash_value("title").expect("title not found");

        // Verify the title has a valid substring range
        match &title.source_info {
            SourceInfo::Substring { start_offset, .. } => {
                // Offset should be relative to the yaml_content string
                assert!(*start_offset < yaml_content.len());
            }
            _ => panic!("Expected Substring mapping for title"),
        }

        // Check that range makes sense
        assert!(title.source_info.start_offset() < title.source_info.end_offset());
    }

    #[test]
    fn test_parse_anonymous_no_substring() {
        // Parse without filename or parent - should use Original mapping
        let yaml = parse("title: Test").unwrap();

        match &yaml.source_info {
            SourceInfo::Original { file_id, .. } => {
                assert_eq!(file_id.0, 0); // Anonymous FileId
            }
            _ => panic!("Expected Original mapping for anonymous parse"),
        }
    }

    /// Helper function to resolve a SourceInfo through the mapping chain to get
    /// the absolute offset in the original file.
    fn resolve_to_original_offset(info: &SourceInfo) -> (usize, quarto_source_map::FileId) {
        match info {
            SourceInfo::Original {
                file_id,
                start_offset,
                ..
            } => (*start_offset, *file_id),
            SourceInfo::Substring {
                parent,
                start_offset,
                ..
            } => {
                let (parent_offset, file_id) = resolve_to_original_offset(parent);
                (parent_offset + start_offset, file_id)
            }
            _ => panic!("Unsupported mapping type for offset resolution"),
        }
    }

    #[test]
    fn test_hash_key_and_value_locations() {
        // Test that we can track both key and value locations in YAML hashes
        let yaml_content = "hello: world\nfoo: bar\ncount: 42";
        let yaml = parse(yaml_content).unwrap();

        assert!(yaml.is_hash());
        let entries = yaml.as_hash().expect("Should be a hash");

        // Test 1: Verify "hello" key and "world" value locations
        let hello_entry = entries
            .iter()
            .find(|e| e.key.yaml.as_str() == Some("hello"))
            .expect("Should have 'hello' key");

        // Verify key location
        assert_eq!(hello_entry.key.yaml.as_str(), Some("hello"));
        let key_offset = hello_entry.key_span.start_offset();
        let key_str = &yaml_content[key_offset..key_offset + 5];
        assert_eq!(key_str, "hello", "Key location should point to 'hello'");

        // Verify value location
        assert_eq!(hello_entry.value.yaml.as_str(), Some("world"));
        let value_offset = hello_entry.value_span.start_offset();
        let value_str = &yaml_content[value_offset..value_offset + 5];
        assert_eq!(value_str, "world", "Value location should point to 'world'");

        // Verify they are different locations
        assert_ne!(
            key_offset, value_offset,
            "Key and value should have different offsets"
        );

        // Test 2: Verify "foo" key and "bar" value locations
        let foo_entry = entries
            .iter()
            .find(|e| e.key.yaml.as_str() == Some("foo"))
            .expect("Should have 'foo' key");

        let foo_key_offset = foo_entry.key_span.start_offset();
        let foo_key_str = &yaml_content[foo_key_offset..foo_key_offset + 3];
        assert_eq!(foo_key_str, "foo", "Key location should point to 'foo'");

        let bar_value_offset = foo_entry.value_span.start_offset();
        let bar_value_str = &yaml_content[bar_value_offset..bar_value_offset + 3];
        assert_eq!(bar_value_str, "bar", "Value location should point to 'bar'");

        // Test 3: Verify "count" key and "42" value locations
        let count_entry = entries
            .iter()
            .find(|e| e.key.yaml.as_str() == Some("count"))
            .expect("Should have 'count' key");

        let count_key_offset = count_entry.key_span.start_offset();
        let count_key_str = &yaml_content[count_key_offset..count_key_offset + 5];
        assert_eq!(
            count_key_str, "count",
            "Key location should point to 'count'"
        );

        assert_eq!(count_entry.value.yaml.as_i64(), Some(42));
        let count_value_offset = count_entry.value_span.start_offset();
        let count_value_str = &yaml_content[count_value_offset..count_value_offset + 2];
        assert_eq!(count_value_str, "42", "Value location should point to '42'");

        // Test 4: Verify entry spans include both key and value
        // The entry span should start at the key and end after the value
        assert!(
            hello_entry.entry_span.start_offset() <= key_offset,
            "Entry span should start at or before the key"
        );
        assert!(
            hello_entry.entry_span.end_offset() >= value_offset + 5,
            "Entry span should end at or after the value"
        );
    }

    #[test]
    fn test_qmd_frontmatter_extraction() {
        use quarto_source_map::{FileId, Location, Range};

        // Simulate a realistic .qmd file
        let qmd_content = r#"---
title: "My Research Paper"
author: "Jane Smith"
date: "2024-01-15"
format:
  html:
    theme: cosmo
    toc: true
  pdf:
    documentclass: article
---

# Introduction

This is my research paper with some **bold** text.

## Methods

We used the following approach...
"#;

        // Extract YAML frontmatter using regex (simple approach - just for testing)
        let re = regex::Regex::new(r"(?s)^---\n(.*?)\n---").unwrap();
        let captures = re
            .captures(qmd_content)
            .expect("Failed to find YAML frontmatter");

        let yaml_match = captures.get(1).expect("No YAML content found");
        let yaml_start = yaml_match.start();
        let yaml_end = yaml_match.end();
        let yaml_content = yaml_match.as_str();

        // Create parent SourceInfo for the entire .qmd file
        let parent = SourceInfo::from_range(
            FileId(123), // Simulated FileId for test.qmd
            Range {
                start: Location {
                    offset: 0,
                    row: 0,
                    column: 0,
                },
                end: Location {
                    offset: qmd_content.len(),
                    row: qmd_content.lines().count().saturating_sub(1),
                    column: qmd_content.lines().last().unwrap_or("").len(),
                },
            },
        );

        // Create parent SourceInfo for just the YAML portion
        let yaml_parent = SourceInfo::substring(parent.clone(), yaml_start, yaml_end);

        // Parse the YAML with parent tracking
        let yaml = parse_with_parent(yaml_content, yaml_parent).unwrap();

        // Verify the YAML was parsed correctly
        assert!(yaml.is_hash());
        let title = yaml.get_hash_value("title").expect("title not found");
        assert_eq!(title.yaml.as_str(), Some("My Research Paper"));

        // Verify that the title's location maps back through the substring chain
        match &title.source_info {
            SourceInfo::Substring {
                parent: p,
                start_offset,
                ..
            } => {
                // The offset should be within the YAML content
                assert!(*start_offset < yaml_content.len());

                // The parent should be another Substring pointing to the .qmd file
                match p.as_ref() {
                    SourceInfo::Substring {
                        parent: grandparent,
                        start_offset: yaml_offset,
                        ..
                    } => {
                        // This should point to the original .qmd file
                        assert_eq!(*yaml_offset, yaml_start);

                        // Grandparent should be the Original .qmd file
                        match grandparent.as_ref() {
                            SourceInfo::Original { file_id, .. } => {
                                assert_eq!(file_id.0, 123);
                            }
                            _ => panic!("Expected Original mapping for .qmd file"),
                        }
                    }
                    _ => panic!("Expected Substring mapping for YAML within .qmd"),
                }
            }
            _ => panic!("Expected Substring mapping for title"),
        }

        // Verify nested structures also have correct mappings
        let format = yaml.get_hash_value("format").expect("format not found");
        assert!(format.is_hash());

        let html = format.get_hash_value("html").expect("html not found");
        assert!(html.is_hash());

        let theme = html.get_hash_value("theme").expect("theme not found");
        assert_eq!(theme.yaml.as_str(), Some("cosmo"));

        // The theme value should also have Substring mapping through the chain
        match &theme.source_info {
            SourceInfo::Substring { .. } => {
                // Good - it has substring mapping
            }
            _ => panic!("Expected Substring mapping for deeply nested theme value"),
        }

        // Verify that the 'toc' boolean value is correctly located
        let toc = html.get_hash_value("toc").expect("toc not found");
        assert_eq!(toc.yaml.as_bool(), Some(true));

        // Calculate where "true" appears in the original .qmd file
        let toc_true_in_qmd = qmd_content
            .find("toc: true")
            .expect("toc: true not found in qmd");
        let toc_value_offset = toc_true_in_qmd + "toc: ".len();

        // The toc value should be located within the YAML frontmatter region
        assert!(
            toc_value_offset >= yaml_start && toc_value_offset < yaml_end,
            "toc value offset {} should be within YAML range {}-{}",
            toc_value_offset,
            yaml_start,
            yaml_end
        );

        // ===== NOW TEST OFFSET RESOLUTION =====

        // Test 1: Verify the title value resolves to correct position in .qmd file
        let (resolved_title_offset, resolved_file_id) =
            resolve_to_original_offset(&title.source_info);
        assert_eq!(
            resolved_file_id.0, 123,
            "Title should resolve to FileId 123"
        );

        // Extract the exact string at the resolved position
        let title_expected = "\"My Research Paper\""; // YAML parser includes quotes
        let resolved_title_str =
            &qmd_content[resolved_title_offset..resolved_title_offset + title_expected.len()];
        assert_eq!(
            resolved_title_str, title_expected,
            "Resolved title offset should point to exactly '{}'",
            title_expected
        );

        // Test 2: Verify the theme value "cosmo" resolves correctly
        let (resolved_cosmo_offset, resolved_file_id) =
            resolve_to_original_offset(&theme.source_info);
        assert_eq!(
            resolved_file_id.0, 123,
            "Theme should resolve to FileId 123"
        );

        // Extract the exact string at the resolved position
        let cosmo_expected = "cosmo";
        let resolved_cosmo_str =
            &qmd_content[resolved_cosmo_offset..resolved_cosmo_offset + cosmo_expected.len()];
        assert_eq!(
            resolved_cosmo_str, cosmo_expected,
            "Resolved theme offset should point to exactly '{}'",
            cosmo_expected
        );

        // Test 3: Verify the author value resolves correctly
        let author = yaml.get_hash_value("author").expect("author not found");
        assert_eq!(author.yaml.as_str(), Some("Jane Smith"));

        let (resolved_author_offset, resolved_file_id) =
            resolve_to_original_offset(&author.source_info);
        assert_eq!(
            resolved_file_id.0, 123,
            "Author should resolve to FileId 123"
        );

        // Extract the exact string at the resolved position
        let author_expected = "\"Jane Smith\""; // YAML parser includes quotes
        let resolved_author_str =
            &qmd_content[resolved_author_offset..resolved_author_offset + author_expected.len()];
        assert_eq!(
            resolved_author_str, author_expected,
            "Resolved author offset should point to exactly '{}'",
            author_expected
        );

        // Test 4: Verify the YAML root offset resolution
        let (resolved_yaml_offset, _) = resolve_to_original_offset(&yaml.source_info);

        // The resolved position should be within the YAML frontmatter
        assert!(
            resolved_yaml_offset >= yaml_start && resolved_yaml_offset < yaml_end,
            "YAML root offset {} should be within YAML content range {}-{}",
            resolved_yaml_offset,
            yaml_start,
            yaml_end
        );

        // Extract and verify the exact string - the YAML root should start at the first key
        let yaml_root_expected = "title: \"My Research P"; // First key and start of value
        let resolved_yaml_str =
            &qmd_content[resolved_yaml_offset..resolved_yaml_offset + yaml_root_expected.len()];
        assert_eq!(
            resolved_yaml_str, yaml_root_expected,
            "Resolved YAML root offset should point to exactly '{}'",
            yaml_root_expected
        );

        // Test 5: Verify nested hash entry offsets
        let pdf = format.get_hash_value("pdf").expect("pdf not found");
        let documentclass = pdf
            .get_hash_value("documentclass")
            .expect("documentclass not found");
        assert_eq!(documentclass.yaml.as_str(), Some("article"));

        let (resolved_article_offset, resolved_file_id) =
            resolve_to_original_offset(&documentclass.source_info);
        assert_eq!(
            resolved_file_id.0, 123,
            "Documentclass should resolve to FileId 123"
        );

        // Extract the exact string at the resolved position
        let article_expected = "article";
        let resolved_article_str =
            &qmd_content[resolved_article_offset..resolved_article_offset + article_expected.len()];
        assert_eq!(
            resolved_article_str, article_expected,
            "Resolved documentclass offset should point to exactly '{}'",
            article_expected
        );

        // Test 6: Verify that hash entry key spans resolve correctly
        if let Some(entries) = yaml.as_hash() {
            for entry in entries {
                let (entry_key_start, entry_file_id) = resolve_to_original_offset(&entry.key_span);
                assert_eq!(
                    entry_file_id.0, 123,
                    "Entry key should resolve to FileId 123"
                );

                // All top-level keys should be within the YAML frontmatter region
                assert!(
                    entry_key_start >= yaml_start && entry_key_start < yaml_end,
                    "Entry key at offset {} should be within YAML range {}-{}",
                    entry_key_start,
                    yaml_start,
                    yaml_end
                );

                // Verify the key actually points to the key string
                let key_str = entry.key.yaml.as_str().unwrap_or("");
                if !key_str.is_empty() && entry_key_start + key_str.len() <= qmd_content.len() {
                    let resolved_key_str =
                        &qmd_content[entry_key_start..entry_key_start + key_str.len()];
                    assert_eq!(
                        resolved_key_str, key_str,
                        "Entry key '{}' should resolve to exact position",
                        key_str
                    );
                }
            }
        }

        // All tests passed - offset resolution works correctly through the double-substring chain!
    }

    #[test]
    fn test_object_source_range_starts_at_first_key() {
        let yaml_content = "title: \"My Research Paper\"\nauthor: \"John Doe\"\n";
        let parsed = parse_file(yaml_content, "test.yaml").expect("parse failed");

        // The root should be an object
        assert!(parsed.is_hash());

        // Check the SourceInfo of the object
        let source_info = &parsed.source_info;

        // The object should span from offset 0 (start of "title") to the end
        // NOT from offset 5 (the colon)
        assert_eq!(
            source_info.start_offset(),
            0,
            "Object should start at offset 0 (beginning of first key), not at the colon"
        );

        // The end should be at the end of the content
        assert_eq!(
            source_info.end_offset(),
            yaml_content.len(),
            "Object should end at end of content"
        );
    }

    // =========== TAG TESTS ===========

    #[test]
    fn test_parse_scalar_with_tag() {
        let yaml = parse("key: !expr x + 1").unwrap();
        let value = yaml.get_hash_value("key").expect("key not found");

        assert!(value.tag.is_some());
        let (tag_suffix, _tag_source) = value.tag.as_ref().unwrap();
        assert_eq!(tag_suffix, "expr");
    }

    #[test]
    fn test_parse_scalar_with_prefer_tag() {
        let yaml = parse("theme: !prefer cosmo").unwrap();
        let value = yaml.get_hash_value("theme").expect("theme not found");

        assert!(value.tag.is_some());
        let (tag_suffix, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag_suffix, "prefer");
        assert_eq!(value.yaml.as_str(), Some("cosmo"));
    }

    #[test]
    fn test_parse_scalar_with_concat_tag() {
        let yaml = parse("items: !concat [a, b]").unwrap();
        // Note: !concat on a sequence - the tag is on the sequence itself
        let value = yaml.get_hash_value("items").expect("items not found");

        // The tag is currently only captured for scalars, not sequences
        // This test documents current behavior
        assert!(value.is_array());
    }

    #[test]
    fn test_parse_scalar_with_md_tag() {
        let yaml = parse("description: !md \"**bold** text\"").unwrap();
        let value = yaml
            .get_hash_value("description")
            .expect("description not found");

        assert!(value.tag.is_some());
        let (tag_suffix, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag_suffix, "md");
    }

    #[test]
    fn test_parse_scalar_with_str_tag() {
        let yaml = parse("title: !str \"My Title\"").unwrap();
        let value = yaml.get_hash_value("title").expect("title not found");

        assert!(value.tag.is_some());
        let (tag_suffix, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag_suffix, "str");
    }

    #[test]
    fn test_parse_scalar_with_path_tag() {
        let yaml = parse("file: !path ./data/file.csv").unwrap();
        let value = yaml.get_hash_value("file").expect("file not found");

        assert!(value.tag.is_some());
        let (tag_suffix, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag_suffix, "path");
    }

    #[test]
    fn test_parse_scalar_with_glob_tag() {
        let yaml = parse("sources: !glob \"*.qmd\"").unwrap();
        let value = yaml.get_hash_value("sources").expect("sources not found");

        assert!(value.tag.is_some());
        let (tag_suffix, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag_suffix, "glob");
    }

    #[test]
    fn test_combined_tag_with_underscore_works() {
        // Combined tags like !prefer_md ARE supported using underscore as separator.
        // This is the recommended syntax for combining merge ops with interpretation hints.
        let result = parse("title: !prefer_md \"**My Title**\"");
        assert!(result.is_ok(), "Combined tags with underscore should parse");

        let yaml = result.unwrap();
        let value = yaml.get_hash_value("title").unwrap();
        let (tag, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag, "prefer_md");
    }

    #[test]
    fn test_combined_tag_with_comma_not_supported() {
        // Note: Combined tags like !prefer,md are NOT supported by standard YAML parsers.
        // The comma is not valid in a tag without proper URI escaping.
        // Use underscore instead: !prefer_md
        let result = parse("title: !prefer,md \"**My Title**\"");
        assert!(result.is_err(), "Combined tags with comma should not parse");
    }

    #[test]
    fn test_tag_source_info_location() {
        let yaml_content = "key: !expr value";
        let yaml = parse(yaml_content).unwrap();
        let value = yaml.get_hash_value("key").expect("key not found");

        let (_, tag_source) = value.tag.as_ref().expect("tag should be present");

        // The tag should start at "!" (offset 5)
        let tag_start = tag_source.start_offset();
        assert_eq!(&yaml_content[tag_start..tag_start + 1], "!");

        // The tag should cover "!expr" (5 characters)
        let tag_len = tag_source.end_offset() - tag_source.start_offset();
        assert_eq!(tag_len, 5); // "!expr"
    }

    #[test]
    fn test_no_tag_when_absent() {
        let yaml = parse("key: value").unwrap();
        let value = yaml.get_hash_value("key").expect("key not found");

        assert!(value.tag.is_none());
    }

    #[test]
    fn test_alternative_tag_separator_syntaxes() {
        // Test which separators work for combined tags

        // Underscore separator - should work
        let result = parse("title: !prefer_md test");
        assert!(result.is_ok(), "Underscore separator should work");
        let yaml = result.unwrap();
        let value = yaml.get_hash_value("title").unwrap();
        let (tag, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag, "prefer_md");

        // Dash/hyphen separator - should work
        let result = parse("title: !prefer-md test");
        assert!(result.is_ok(), "Dash separator should work");
        let yaml = result.unwrap();
        let value = yaml.get_hash_value("title").unwrap();
        let (tag, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag, "prefer-md");

        // Dot separator - should work
        let result = parse("title: !prefer.md test");
        assert!(result.is_ok(), "Dot separator should work");
        let yaml = result.unwrap();
        let value = yaml.get_hash_value("title").unwrap();
        let (tag, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag, "prefer.md");

        // Colon separator - works
        let result = parse("title: !prefer:md test");
        assert!(result.is_ok(), "Colon separator should work");
        let yaml = result.unwrap();
        let value = yaml.get_hash_value("title").unwrap();
        let (tag, _) = value.tag.as_ref().unwrap();
        assert_eq!(tag, "prefer:md");

        // Exclamation mark separator - does NOT work (treated as handle)
        let result = parse("title: !md!prefer test");
        assert!(
            result.is_err(),
            "Bang separator should not work (treated as YAML handle)"
        );
    }

    #[test]
    fn test_multiple_tagged_values() {
        let yaml = parse(
            r#"
title: !str "Plain Title"
description: !md "**Bold** description"
file: !path ./data.csv
"#,
        )
        .unwrap();

        let title = yaml.get_hash_value("title").expect("title not found");
        assert_eq!(title.tag.as_ref().map(|(t, _)| t.as_str()), Some("str"));

        let desc = yaml
            .get_hash_value("description")
            .expect("description not found");
        assert_eq!(desc.tag.as_ref().map(|(t, _)| t.as_str()), Some("md"));

        let file = yaml.get_hash_value("file").expect("file not found");
        assert_eq!(file.tag.as_ref().map(|(t, _)| t.as_str()), Some("path"));
    }

    // =========== SCALAR RESOLUTION TESTS ===========

    /// Parse `source` and return the value of its single `key` entry.
    fn parse_value(source: &str) -> YamlWithSourceInfo {
        parse(source)
            .unwrap_or_else(|e| panic!("{source:?} should parse: {e}"))
            .get_hash_value("key")
            .unwrap_or_else(|| panic!("{source:?} should have a `key` entry"))
            .clone()
    }

    /// The source text a node's span covers.
    fn span_text(source: &str, node: &YamlWithSourceInfo) -> String {
        source[node.source_info.start_offset()..node.source_info.end_offset()].to_string()
    }

    #[test]
    fn test_quoted_scalars_are_strings() {
        // Quoting is how YAML says "this is text": a quoted scalar never
        // resolves to a number, bool, or null. See posit-dev/quarto-yaml#2.
        for (source, expected) in [
            ("key: \"1\"", "1"),
            ("key: '1'", "1"),
            ("key: \"\"", ""),
            ("key: ''", ""),
            ("key: 'null'", "null"),
            ("key: \"~\"", "~"),
            ("key: \"true\"", "true"),
            ("key: 'false'", "false"),
            ("key: \"1.5\"", "1.5"),
            ("key: \".inf\"", ".inf"),
        ] {
            let value = parse_value(source);
            assert_eq!(
                value.yaml,
                Yaml::String(expected.to_string()),
                "{source:?} should resolve to the string {expected:?}"
            );
        }
    }

    #[test]
    fn test_block_scalars_are_strings() {
        // Block scalars are strings too, including the chomped forms whose
        // value has no trailing newline to give them away.
        for source in [
            "key: |-\n  true",
            "key: >-\n  42",
            "key: |\n  null\n",
            "key: >\n  1.5\n",
        ] {
            let value = parse_value(source);
            assert!(
                matches!(value.yaml, Yaml::String(_)),
                "{source:?} should resolve to a string, got {:?}",
                value.yaml
            );
        }
    }

    #[test]
    fn test_plain_booleans_follow_yaml_12_core_schema() {
        // The 1.2 core schema recognises only `true`/`false`; YAML 1.1's
        // `yes`, `no`, `on`, `off` are plain strings.
        for text in ["true", "True", "TRUE"] {
            assert_eq!(
                parse(text).unwrap().yaml.as_bool(),
                Some(true),
                "{text:?} should be true"
            );
        }
        for text in ["false", "False", "FALSE"] {
            assert_eq!(
                parse(text).unwrap().yaml.as_bool(),
                Some(false),
                "{text:?} should be false"
            );
        }
        for text in [
            "yes", "Yes", "YES", "no", "No", "NO", "on", "On", "ON", "off", "Off", "OFF", "y", "n",
        ] {
            let yaml = parse(text).unwrap();
            assert_eq!(
                yaml.yaml,
                Yaml::String(text.to_string()),
                "{text:?} is a string in YAML 1.2, got {:?}",
                yaml.yaml
            );
        }
    }

    #[test]
    fn test_plain_integers_follow_yaml_12_core_schema() {
        // The core schema's `!!int` matches `[-+]?[0-9]+`, `0x[0-9a-fA-F]+`
        // and `0o[0-7]+`.
        for (text, expected) in [
            ("42", 42),
            ("-7", -7),
            ("+7", 7),
            ("0x1f", 31),
            ("0xFF", 255),
            ("0o17", 15),
            ("0o0", 0),
        ] {
            assert_eq!(
                parse(text).unwrap().yaml.as_i64(),
                Some(expected),
                "{text:?} should be the integer {expected}"
            );
        }
        // The prefixed forms are lower-case and unsigned, there is no binary
        // form, and a value that overflows i64 falls back to a string.
        for text in [
            "0X1f",
            "-0x1f",
            "+0x1f",
            "0b101",
            "0x",
            "0o",
            "0o8",
            "0o18",
            "0xg1",
            "0o+7",
            "0x-1f",
            "0xFFFFFFFFFFFFFFFF",
        ] {
            let yaml = parse(text).unwrap();
            assert_eq!(
                yaml.yaml,
                Yaml::String(text.to_string()),
                "{text:?} should stay a string, got {:?}",
                yaml.yaml
            );
        }
        // Quoting still wins: a quoted hex spelling is text.
        assert_eq!(
            parse_value("key: \"0x1f\"").yaml,
            Yaml::String("0x1f".to_string())
        );
    }

    #[test]
    fn test_explicit_standard_tags_are_honoured() {
        // An explicit tag *is* the node's type, so it overrides both implicit
        // resolution and the quoting style.
        for (source, expected) in [
            ("key: !!str 5", Yaml::String("5".to_string())),
            ("key: !!str true", Yaml::String("true".to_string())),
            ("key: !!int \"7\"", Yaml::Integer(7)),
            ("key: !!int -7", Yaml::Integer(-7)),
            ("key: !!int 0x1f", Yaml::Integer(31)),
            ("key: !!int 0o17", Yaml::Integer(15)),
            ("key: !!float \"1.5\"", Yaml::Real("1.5".to_string())),
            ("key: !!bool \"true\"", Yaml::Boolean(true)),
            ("key: !!bool 'false'", Yaml::Boolean(false)),
            ("key: !!null \"~\"", Yaml::Null),
            // The verbatim spelling of the same tags.
            (
                "key: !<tag:yaml.org,2002:str> 5",
                Yaml::String("5".to_string()),
            ),
            ("key: !<tag:yaml.org,2002:int> \"7\"", Yaml::Integer(7)),
        ] {
            assert_eq!(
                parse_value(source).yaml,
                expected,
                "{source:?} should resolve to {expected:?}"
            );
        }
    }

    #[test]
    fn test_standard_tag_that_does_not_fit_its_value() {
        // A value contradicting its tag is a bad value, as in yaml-rust2's own
        // loader — we don't silently re-type it.
        for source in [
            "key: !!int abc",
            "key: !!int 0X1f",
            "key: !!int 0xFFFFFFFFFFFFFFFF",
            "key: !!float abc",
            "key: !!bool maybe",
            "key: !!null 5",
        ] {
            assert_eq!(
                parse_value(source).yaml,
                Yaml::BadValue,
                "{source:?} should resolve to BadValue"
            );
        }
    }

    #[test]
    fn test_application_tags_do_not_change_resolution() {
        // Application-specific tags are reported in `tag` for consumers to
        // interpret; they leave resolution alone.
        let value = parse_value("key: !expr 42");
        assert_eq!(value.yaml, Yaml::Integer(42));
        assert_eq!(value.tag.as_ref().map(|(t, _)| t.as_str()), Some("expr"));

        // …but the quoting style still applies under an application tag.
        let value = parse_value("key: !expr \"42\"");
        assert_eq!(value.yaml, Yaml::String("42".to_string()));
        assert_eq!(value.tag.as_ref().map(|(t, _)| t.as_str()), Some("expr"));
    }

    // =========== SCALAR SPAN TESTS ===========

    #[test]
    fn test_quoted_scalar_spans_cover_the_whole_scalar() {
        // Spans start at the opening quote, so they have to end at the closing
        // one: `"1"` is three bytes of source even though its value is one.
        for (source, expected) in [
            ("key: \"1\"", "\"1\""),
            ("key: 'null'", "'null'"),
            ("key: \"true\"", "\"true\""),
            ("key: \"\"", "\"\""),
            ("key: ''", "''"),
            // Escapes: the value is shorter than the source.
            ("key: \"a\\nb\"", "\"a\\nb\""),
            ("key: \"esc \\\" quote\"", "\"esc \\\" quote\""),
            ("key: 'it''s'", "'it''s'"),
            // A trailing comment is not part of the scalar.
            ("key: \"x\" # comment", "\"x\""),
            // Multi-byte characters.
            ("key: \"日本 語\"", "\"日本 語\""),
            // A quoted scalar can be folded over several lines.
            ("key: \"multi\n  line\"", "\"multi\n  line\""),
        ] {
            let value = parse_value(source);
            assert_eq!(
                span_text(source, &value),
                expected,
                "span of the value in {source:?}"
            );
        }
    }

    #[test]
    fn test_plain_scalar_spans() {
        for (source, expected) in [
            ("key: 42", "42"),
            ("key: hello world", "hello world"),
            ("key: two  spaces", "two  spaces"),
            ("key: x # comment", "x"),
            // Folded over lines: the source is longer than the value.
            ("key: plain\n  continued", "plain\n  continued"),
            ("key: a \n  b", "a \n  b"),
            ("key: a\n\n  b", "a\n\n  b"),
            // Flow context: the scalar stops at the closing bracket.
            ("key: [a, b]", "a"),
        ] {
            let value = parse_value(source);
            let value = value.as_array().map_or(value.clone(), |i| i[0].clone());
            assert_eq!(
                span_text(source, &value),
                expected,
                "span of the value in {source:?}"
            );
        }
    }

    #[test]
    fn test_block_scalar_spans_cover_their_content() {
        // The marker points at the first content character, so the `|`/`>`
        // header is outside the span, but the content lines are all inside it.
        for (source, expected) in [
            ("key: |\n  a\n  b\n", "a\n  b"),
            ("key: |-\n  a\n  b\nnext: 1\n", "a\n  b"),
            ("key: >\n  folded\n  text\n", "folded\n  text"),
            // Blank lines inside the block belong to it; trailing ones don't.
            ("key: >-\n  a\n\n  b\n\nnext: 1\n", "a\n\n  b"),
            ("key: |\n  a\n\n", "a"),
            // A less-indented line ends the block.
            ("key: |\n  a\n# comment\nnext: 1\n", "a"),
        ] {
            let value = parse_value(source);
            assert_eq!(
                span_text(source, &value),
                expected,
                "span of the value in {source:?}"
            );
        }
    }

    #[test]
    fn test_char_index_to_byte_offset_conversion() {
        // Exercise forward, backward, repeated, and out-of-range queries.
        let sources = [
            "",
            "plain ascii",
            "é",
            "a é b — c 𝄞 d",
            "🇬🇧 flags, ç cedillas, and\nnewlines\n",
        ];

        for source in sources {
            let builder = YamlBuilder::new(source, None);
            let expected: Vec<usize> = source
                .char_indices()
                .map(|(byte, _)| byte)
                .chain([source.len()])
                .collect();

            let n = expected.len();
            let order = (0..n)
                .chain((0..n).rev())
                .chain((0..n).map(|i| (i * 7) % n))
                .chain([n, n + 5, 0, n + 1, n / 2]);

            for i in order {
                assert_eq!(
                    builder.byte_offset_of_char(i),
                    expected.get(i).copied().unwrap_or(source.len()),
                    "byte offset of char {i} in {source:?}"
                );
            }
        }
    }

    #[test]
    fn test_spans_are_byte_offsets_after_multibyte_characters() {
        // Regression test for posit-dev/quarto-yaml#9.
        let source = "first: é\nsecond: \"quoted\"\n";
        let parsed = parse(source).unwrap();
        let entries = parsed.as_hash().expect("should be a hash");

        assert_eq!(
            span_text(source, &entries[0].value),
            "é",
            "span of the value of `first`"
        );
        let second = &entries[1];
        assert_eq!(
            &source[second.key_span.start_offset()..second.key_span.end_offset()],
            "second",
            "span of the key `second`"
        );
        assert_eq!(
            span_text(source, &second.value),
            "\"quoted\"",
            "span of the value of `second`"
        );
    }

    #[test]
    fn test_collection_spans_are_byte_offsets_after_multibyte_characters() {
        let source = "dash: \"—\"\nlist: [é, deux]\nmap: {a: ç}\nafter: end\n";
        let parsed = parse(source).unwrap();

        let list = parsed.get_hash_value("list").expect("list not found");
        assert_eq!(span_text(source, list), "[é, deux]");
        let items = list.as_array().expect("list should be an array");
        assert_eq!(span_text(source, &items[0]), "é");
        assert_eq!(span_text(source, &items[1]), "deux");

        let map = parsed.get_hash_value("map").expect("map not found");
        let entries = map.as_hash().expect("map should be a hash");
        assert_eq!(span_text(source, &entries[0].value), "ç");

        let after = parsed.get_hash_value("after").expect("after not found");
        assert_eq!(span_text(source, after), "end");

        assert_eq!(parsed.source_info.start_offset(), 0);
        assert_eq!(parsed.source_info.end_offset(), source.len());
    }

    #[test]
    fn test_flow_collection_spans_include_the_closing_bracket() {
        let src = "list: [a, deux]\nmap: {a: deux}";
        let parsed = parse(src).unwrap();

        let list = parsed.get_hash_value("list").expect("list not found");
        let (s, e) = (
            list.source_info.start_offset(),
            list.source_info.end_offset(),
        );
        assert_eq!(&src[s..e], "[a, deux]");

        let map = parsed.get_hash_value("map").expect("map not found");
        let (s, e) = (map.source_info.start_offset(), map.source_info.end_offset());
        assert_eq!(&src[s..e], "{a: deux}");
    }

    #[test]
    fn test_tag_span_after_multibyte_characters() {
        let source = "unit: \"µs\"\nvalue: !expr x + 1\n";
        let parsed = parse(source).unwrap();
        let value = parsed.get_hash_value("value").expect("value not found");
        let (suffix, tag_span) = value.tag.as_ref().expect("tag should be present");
        assert_eq!(suffix, "expr");
        assert_eq!(
            &source[tag_span.start_offset()..tag_span.end_offset()],
            "!expr"
        );
        assert_eq!(span_text(source, value), "x + 1");
    }

    #[test]
    fn test_quoted_key_spans() {
        let source = "\"quoted key\": v";
        let entries = parse(source).unwrap();
        let entries = entries.as_hash().expect("should be a hash");
        assert_eq!(entries.len(), 1);
        assert_eq!(
            &source[entries[0].key_span.start_offset()..entries[0].key_span.end_offset()],
            "\"quoted key\""
        );
    }

    #[test]
    fn test_standard_tag_span_covers_both_bangs() {
        let source = "key: !!str 5";
        let value = parse_value(source);
        let (suffix, tag_span) = value.tag.as_ref().expect("tag should be present");
        assert_eq!(suffix, "str");
        assert_eq!(
            &source[tag_span.start_offset()..tag_span.end_offset()],
            "!!str"
        );
    }

    #[test]
    fn test_verbatim_tag_span() {
        let source = "key: !<tag:yaml.org,2002:str> 5";
        let value = parse_value(source);
        let (_, tag_span) = value.tag.as_ref().expect("tag should be present");
        assert_eq!(
            &source[tag_span.start_offset()..tag_span.end_offset()],
            "!<tag:yaml.org,2002:str>"
        );
    }

    #[test]
    fn test_tag_span_in_flow_sequence() {
        let source = "key: [!expr 1]";
        let items = parse_value(source);
        let items = items.as_array().expect("should be an array");
        let (_, tag_span) = items[0].tag.as_ref().expect("tag should be present");
        assert_eq!(
            &source[tag_span.start_offset()..tag_span.end_offset()],
            "!expr"
        );
    }

    #[test]
    fn test_tag_span_with_an_anchor_in_the_way() {
        // A node can carry a tag and an anchor in either order, so the tag is
        // not always the token next to the value.
        for (source, expected) in [
            ("key: !expr &name value", "!expr"),
            ("key: &name !expr value", "!expr"),
            ("key: !!str &name 5", "!!str"),
            (
                "key: &name !<tag:example.com,2000:x> value",
                "!<tag:example.com,2000:x>",
            ),
            ("key: [&a !expr 1]", "!expr"),
            ("key: [!expr &a 1]", "!expr"),
        ] {
            let value = parse_value(source);
            let value = value.as_array().map_or(value.clone(), |i| i[0].clone());
            let (_, tag_span) = value
                .tag
                .as_ref()
                .unwrap_or_else(|| panic!("{source:?} should have a tag"));
            assert_eq!(
                &source[tag_span.start_offset()..tag_span.end_offset()],
                expected,
                "tag span in {source:?}"
            );
        }
    }

    #[test]
    fn test_spans_stay_within_the_source() {
        // Whatever the shape of the input, no span may run past the end of the
        // source — consumers slice the source with these offsets.
        for source in [
            "key:",
            "key: |\n",
            "key: >\n",
            "[a, 'b', \"c\"]",
            "- 1\n- \"2\"\n",
            "{}",
            "[]",
            "key: !!str",
            "key: !expr",
            "key: !!str &name 5",
            "key: &name !!str",
            "!!str 5",
            "日本: 語",
            "é: [ü, \"ß\"]",
            "key: — !expr —",
            "key: |\n  日本\n",
        ] {
            let parsed = parse(source).unwrap_or_else(|e| panic!("{source:?} should parse: {e}"));
            assert_spans_within(source, &parsed);
        }
    }

    fn assert_spans_within(source: &str, node: &YamlWithSourceInfo) {
        let (start, end) = (
            node.source_info.start_offset(),
            node.source_info.end_offset(),
        );
        assert!(
            start <= end && end <= source.len(),
            "span {start}..{end} of {:?} is outside {source:?}",
            node.yaml
        );
        if let Some((suffix, tag_span)) = &node.tag {
            let (start, end) = (tag_span.start_offset(), tag_span.end_offset());
            assert!(
                start <= end && end <= source.len(),
                "tag span {start}..{end} of !{suffix} is outside {source:?}"
            );
        }
        if let Some(items) = node.as_array() {
            for item in items {
                assert_spans_within(source, item);
            }
        }
        if let Some(entries) = node.as_hash() {
            for entry in entries {
                assert_spans_within(source, &entry.key);
                assert_spans_within(source, &entry.value);
            }
        }
    }
}
