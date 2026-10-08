# Scan errors carry a real source location

Strand: `qy-scan-error-location-f9r4yvxq`. Downstream: q2 `bd-x30aq7ae`.

## Overview

YAML syntax errors (yaml-rust2 `ScanError`) reach callers as
`Error::ParseError { location: None, .. }`. The only position is text in
`message` (`"... at byte N line L column C"`). That text has two problems:
`N` counts chars, not bytes, and `L`/`C` are relative to the parsed string,
not to the enclosing file. This plan builds the location inside `parse_impl`,
where the builder can still convert the marker to bytes and map it through
`parent`. It also strips the position suffix from the message.

## Reproduction (2026-10-08, at 038f898)

Scratch crate (path dep on `crates/quarto-yaml`) calling `parse_with_parent`
with `parent = SourceInfo::original(FileId(7), 4, 4 + body.len())`, i.e. the
body sits after a `---\n` line:

| case | body | result | correct file byte |
|---|---|---|---|
| ascii | `title: x\nauthor: [a, b\nformat: html\n` | `"illegal placement of ':' indicator at byte 29 line 3 column 7"`, `location: None` | 33 (`:` of `format:`) |
| non-ASCII | `title: "héllo ✓"\n` + same | `"... at byte 37 line 3 column 7"`, `location: None` | 44. The marker index is 37 **chars**, which is 40 bytes into the body (é is +1 byte, ✓ is +2) |
| EOF | `title: x\nauthor: [a, b` | `"while parsing a flow sequence, expected ',' or ']' at byte 22 line 3 column 1"` | marker index 22 == `body.len()`, so it sits at EOF |
| `parse_file("a: [1\nb: 2\n", "x.yml")` | | `"... at byte 7 line 2 column 2"`, `location: None` | |
| `parse("")` | | `"No YAML document found"`, `location: None` | |

This confirms all three symptoms in the strand. It also confirms that the
marker points exactly at the offending character, so a 1-char span starting
there is meaningful.

## Assessment

- `parse_impl` (`crates/quarto-yaml/src/parser.rs:151-153`) calls
  `parser.load(&mut builder, false).map_err(Error::from)?`. The builder holds
  everything needed: `byte_offset_of_char` (char→byte, clamped at source end)
  and `make_source_info_at_offset` (Substring of `parent`, or `Original` with
  `FileId(0)` when there is no parent). The `&mut` borrow ends when `load`
  returns, so `builder` can be used in the `map_err` closure.
- `parse_file` already synthesises a parent `Original(file_id_for_filename(name), 0, len)`.
  Errors mapped through it therefore get the right FileId with no special
  case.
- `byte_offset_of_char` keeps a cursor that may sit anywhere when the error
  fires. It walks backwards as well as forwards, so this is safe.
- Every yaml-rust2 failure, including parser-level ones such as "did not find
  expected key", comes through as a `ScanError` with a marker. One code path
  covers them all.
- Other `Error` constructions in this crate:
  - `YamlBuilder::result()` (`parser.rs:311`) gives "No YAML document found"
    with `location: None`. Same gap.
  - `UnexpectedEof` and `InvalidStructure` are **never constructed** in
    `quarto-yaml`. They are public variants only. Nothing to fix.
- `quarto-yaml-validation` builds `quarto_yaml::Error::ParseError` only by
  hand in tests (`error.rs:641-667`, `location: None`, and it asserts only
  on `contains("YAML parsing error")`). Nothing depends on the old message
  text. The parser tests at `parser.rs:2039` and `2106` only check
  `is_err()`.
- yaml-rust2 0.11.0 (locked) and 0.11.1 have identical `ScanError` /
  `Display`.

## Design decisions

Status 2026-10-08: all accepted (2: remove `From<ScanError>`, note migration in release notes).

1. **Span shape: one char at the marker, zero-width at EOF.** The end is the
   byte length of the char at the start offset. The strand allows a
   zero-width span. I prefer one char because ariadne-style renderers
   underline it visibly, and it costs only a `chars().next()` lookup.
2. **`From<ScanError> for Error`: remove it.** It is the trap that caused this
   bug: it cannot know the parent or do char→byte conversion. Nothing in
   either crate uses it once `parse_impl` changes. Removing it is a breaking
   API change, but this release is breaking anyway (0.4.0, see below). The
   alternative is to keep it location-less with `info()` only. That silently
   drops all position information for anyone who uses it, which is worse
   than a compile error. *Decided: remove.*

   **Who could be using it:** only code that calls yaml-rust2 directly,
   pins the same major version (0.11), and uses `?` to convert a
   `ScanError` into `quarto_yaml::Error`. quarto-yaml does not re-export
   `yaml_rust2` or `ScanError`, so plain `parse*` callers are unaffected.
   On crates.io (checked 2026-10-08), the only reverse dependency is
   `quarto-yaml-validation`, which doesn't use it.

   **Migration:** the breakage is a compile error at each `?` site. A
   one-line `map_err` reproduces the old behaviour exactly:
   `.map_err(|e| quarto_yaml::Error::ParseError { message: e.to_string(), location: None })`.
   `#[deprecated]` has no effect on trait impls, so a soft deprecation
   period isn't possible: we either keep the impl or remove it. If we
   remove it, put the one-liner in the release notes.
3. **Empty document location:** a zero-width span at offset 0 of the content,
   mapped through parent. For `parse_with_parent` this points at the start of
   the frontmatter body. The alternative is to span the whole content
   (`0..len`). I lean towards zero-width at 0, because an empty body has no
   extent to underline anyway.
4. **Display:** keep `"Parse error: {message}"`, where `message` is now just
   `info()`. Remove the three TODO blocks and the `if let Some(_loc)` no-ops.
   Add a doc comment on `Error` saying the location is deliberately not
   rendered: rendering needs a `SourceContext`, so consumers should render
   through quarto-error-reporting. Existing Display tests keep passing
   unchanged.
5. **Version: 0.4.0** for both crates (shared workspace version; bump
   `[workspace.package] version` and the `quarto-yaml` workspace dep). Under
   0.x semver, removing `From<ScanError>` makes this a breaking change. A
   populated `location` and the changed message text are behavioural changes
   that q2 pattern-matches on. If we keep `From` (decision 2), 0.3.1 could be
   argued, but 0.4.0 is still the honest choice.

## Checklist

### Tests first (`crates/quarto-yaml/tests/scan_errors.rs`)
- [x] `parse_with_parent`, ASCII body after `---\n`: `location.resolve_byte_range()` start == file byte of `:` in `format:` (33); message == `"illegal placement of ':' indicator"` exactly.
- [x] Non-ASCII (`title: "héllo ✓"`): start == 44 and `&file[start..end] == ":"`.
- [x] EOF (unclosed flow seq as last line): location is in bounds, `start == end == parent_end`, no panic.
- [x] `parse_file`: `location.root_file_id() == Some(file_id_for_filename(name))`, and the start offset is correct.
- [x] `parse` (no parent): `Original` with `FileId(0)` and the correct offset.
- [x] Message contains no `" at byte "` / `" line "` suffix (covered by the exact-equality asserts above).
- [x] Empty document: `location.is_some()` with the shape from decision 3.
- [x] Confirm the new tests fail on current code.

### Implementation
- [x] Add `YamlBuilder::scan_error(&self, err: ScanError) -> Error`: compute `start = byte_offset(err.marker())`, `len = source[start..].chars().next().map_or(0, char::len_utf8)`, then `make_source_info_at_offset(start, len)`; message is `err.info().to_owned()`.
- [x] `parse_impl`: `.map_err(|e| builder.scan_error(e))?`.
- [x] `result()`: give "No YAML document found" a location (decision 3).
- [x] `error.rs`: remove or adjust `From<ScanError>` (decision 2); clean up Display TODOs and add a doc comment (decision 4).
- [x] `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --check`.

### Release
- [x] Bump the workspace version to 0.4.0 (both places in root `Cargo.toml`) and refresh `Cargo.lock`.
- [x] Branch (`scan-error-location`) and PR
- [x] Merge (CI published 0.4.0 on 2026-10-08)
- [x] Record the published version in the strand comment so q2 `bd-x30aq7ae` can bump; close the strand.
