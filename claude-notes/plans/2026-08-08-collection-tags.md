# Capture YAML tags on sequences and mappings

GitHub issue: https://github.com/posit-dev/quarto-yaml/issues/14
Braid strand: `qy-8pf7s9ot`

## Overview

`quarto_yaml::parse*` only attaches YAML tags to scalar nodes. Tags on
sequences and mappings arrive from yaml-rust2 in the
`Event::SequenceStart(_, tag)` / `Event::MappingStart(_, tag)` events but are
discarded (`crates/quarto-yaml/src/parser.rs`, `on_event`). Downstream, this
makes `!prefer` / `!concat` on arrays and maps a silent no-op in Quarto 2's
config merge (q2 strand `bd-43lc07w1`; an `#[ignore]`d regression test is
checked in there as
`crates/quarto-core/tests/integration/custom_project_type.rs::user_prefer_replaces_extension_array`).

Goal: attach `(suffix, tag_span)` to sequence and mapping nodes exactly the
way scalars already get it, with correct source spans.

## Assessment (verified 2026-08-08)

The issue's description is accurate against `main` (post-0.1.1). Verified in
`crates/quarto-yaml/src/parser.rs`:

- `Event::Scalar(value, style, _anchor_id, tag)` — captures the tag, locates
  its span with `find_tag_span` (backward whitespace-token walk from the value
  marker, skipping anchors), attaches via `new_scalar_with_tag`.
- `Event::SequenceStart(_anchor_id, _tag)` (parser.rs:504) — tag ignored.
- `Event::MappingStart(_anchor_id, _tag)` (parser.rs:535) — tag ignored.
- `YamlWithSourceInfo.tag: Option<(String, SourceInfo)>` is a public field and
  shape-agnostic; only the constructors are scalar-biased
  (`new_scalar_with_tag` exists; `new_array` / `new_hash` hardcode
  `tag: None`).
- The existing test `test_parse_scalar_with_concat_tag` (parser.rs:1591)
  explicitly documents the current (broken) behavior and will be updated.

### Marker positions (empirically probed against yaml-rust2 0.11)

Probed `SequenceStart` / `MappingStart` markers for every relevant shape
(scratch program, `Parser::load` with a `MarkedEventReceiver`):

| Input shape | Start marker points at | Backward walk from marker finds tag? |
|---|---|---|
| `k: !prefer [a, b]` (flow seq) | `[` | yes |
| `k: !prefer {x: 1}` (flow map) | `{` | yes |
| `k: !prefer\n  - a` (block seq) | first `-` | yes (walk crosses the newline) |
| `k: !prefer\n  x: 1` (block map) | **colon after the first key** | **no** — walk finds `x` first |
| root-level `!prefer\n- a` | first `-` | yes |
| `&nm !prefer` / `!prefer &nm` before a block seq | first `-` | yes (anchor-skipping loop handles both orders) |
| nested `- !prefer\n    - a` | inner `-` | yes |
| `k: [!prefer [a], b]` | inner `[` | yes |
| `k: !prefer []` (empty flow) | `[` | yes |
| `k: !!seq\n  - a` (standard tag) | first `-` | yes |

Key consequence: **block mappings are the one shape where the Start marker is
useless for tag discovery** (it sits after the first key). But `MappingEnd`
already computes the node's span start as the *first key's* start offset
(parser.rs:583-588), and walking backward from that offset finds the tag in
every probed case. So: locate the tag span at `SequenceEnd` / `MappingEnd`
time, walking back from the same offset the node span starts at.

## Design

1. **Stash the raw tag on the build frame.** Add `tag: Option<Tag>` (the
   yaml-rust2 `Tag`) to both `BuildNode::Sequence` and `BuildNode::Mapping`;
   populate it in `SequenceStart` / `MappingStart`.

2. **Generalize `find_tag_span`.** Change its parameter from
   `value_marker: &Marker` to a byte offset (`before_offset: usize`); the
   scalar call site passes `self.byte_offset(&marker)` as before. No behavior
   change for scalars — the walk itself is untouched.

3. **Locate and attach at End events.** In `SequenceEnd` / `MappingEnd`,
   compute the tag info from the stashed tag:
   - walk-back origin = the node's span start (`start_offset` for sequences
     and flow mappings; first key's start for block mappings — i.e. exactly
     the offset each branch already computes for `source_info`);
   - on walk failure, reuse the scalar fallback: span of length
     `1 + suffix.len()` anchored at the span start, clamped to the source
     (wrong but in-bounds; consumers can still slice).
   - store `(t.suffix.clone(), span)` — same convention as scalars, including
     for standard (`!!seq`) and verbatim tags.

4. **API: tag-carrying construction.** Add a consuming builder
   `YamlWithSourceInfo::with_tag(self, tag: Option<(String, SourceInfo)>) -> Self`
   (works for all three shapes; parser uses it for the collection nodes).
   Keep `new_scalar_with_tag` as-is for compatibility.

5. **No resolution changes.** Collection tags do not affect how the node's
   `Yaml` value is built — `!!seq` / `!!map` are recorded like any other tag
   but not validated against the node shape (mirrors how application tags on
   scalars leave resolution alone). Documented as a non-goal.

6. **Node spans keep excluding the tag** — a scalar's span is the value, and
   the tag has its own span; collections stay consistent with that.

### Known limitation (parity with scalars)

A comment between the tag and the collection (`k: !prefer # note\n  - a`)
defeats the backward walk (nearest token is `note`), triggering the fallback
span. The scalar path has the same limitation today. Not fixed here; covered
by a test asserting the tag is still captured and its span stays in-bounds.

## Checklist

- [x] Add `tag: Option<Tag>` to `BuildNode::Sequence` / `BuildNode::Mapping`;
      capture in `SequenceStart` / `MappingStart`
- [x] Refactor `find_tag_span` to take a byte offset; update scalar call site
- [x] Extract shared helper producing `(suffix, SourceInfo)` from
      `(&Tag, walk_back_offset)` incl. the fallback; use it from all three
      shapes (`make_tag_info`)
- [x] Attach tag info in `SequenceEnd` / `MappingEnd` (walk back from the
      node's span start; block-mapping case uses first key's offset)
- [x] Add `YamlWithSourceInfo::with_tag` builder
- [x] Update `test_parse_scalar_with_concat_tag` (now
      `test_parse_concat_tag_on_flow_sequence`) to assert the tag is captured
- [x] New tests (see matrix below) — all shapes covered; 363 workspace tests
      pass, clippy clean
- [x] `cargo test --workspace`, `cargo clippy --workspace`, `cargo fmt`
- [ ] Close the strand; comment on GH issue #14 (after PR merge)
- [ ] Follow-up (separate PR per release process): version bump to 0.1.2 so
      q2 can depend on the fix

Comment-between-tag-and-node limitation filed as strand `qy-yqstv88w`
(discovered-from `qy-8pf7s9ot`); decision: keep parity with scalars for now.
API decision: `with_tag` builder (approved 2026-08-08).

## Test matrix

Tag suffix + exact tag span text (`&source[span]`), per shape:

- Block sequence: `k: !prefer\n  - a\n  - b` → suffix `prefer`, span `!prefer`
- Flow sequence: `k: !prefer [a, b]`
- Block mapping: `k: !prefer\n  x: 1` (the marker-at-colon case)
- Flow mapping: `k: !prefer {x: 1}`
- Root-level tagged collections: `!prefer\n- a`, `!prefer [a]`
- Anchor and tag in both orders: `k: &nm !prefer\n  - a`,
  `k: !prefer &nm\n  - a`
- Nested: `k:\n  - !prefer\n    - a` (tag on inner seq only),
  `k: [!prefer [a], b]`
- Empty flow collections: `k: !prefer []`, `k: !prefer {}`
- Standard tags on collections: `!!seq`, `!!map` recorded, value unchanged
- Tag separated by blank line: `k: !prefer\n\n  - a`
- Multibyte chars before the tag (byte-offset correctness), e.g.
  `dash: "—"\nlist: !prefer [é]`
- Comment-between fallback: `k: !prefer # note\n  - a` → tag present, span
  in-bounds (fallback semantics)
- Extend `test_spans_stay_within_the_source` inputs with tagged-collection
  shapes (its `assert_spans_within` already checks tag spans recursively)
- Untagged collections still have `tag: None`

## Open questions

1. `with_tag` builder vs. `new_array_with_tag` / `new_hash_with_tag`
   constructors — plan says builder; flag if you'd rather mirror the scalar
   constructor naming.
2. Version bump target: 0.1.2 (additive API + behavior fix; nothing
   breaking). Bump in a separate release PR per CLAUDE.md.
