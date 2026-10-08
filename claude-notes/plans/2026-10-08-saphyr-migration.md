# Switch the YAML backend from yaml-rust2 to saphyr-parser

Strand: `qy-fr4j7ft8`. Related: `qy-block-scalar-utf8-drift-7dccrmto` (the
bug that prompted this). Status 2026-10-08: proposal reviewed; go-ahead given for
Phase 1 as one PR (parser swap, keep `yaml_rust2::Yaml`), with the
yaml-rust2 removal as a follow-up. Work is on branch `saphyr-parser`.

Investigation artifacts live next to this file in
`2026-10-08-saphyr-migration-investigation/`:

- `probe/`: a standalone crate (not a workspace member) with two binaries.
  `probe` prints the event stream of yaml-rust2 0.11.1 and saphyr-parser
  0.1.0 side by side for ~55 inputs, flagging rows whose start markers
  differ. `qydump` prints the quarto-yaml 0.4.0 tree (byte spans, span
  text, content provenance, tags) for inputs given on the command line.
- `probe-output.txt`, `qydump-output.txt`: the runs this plan cites.

## Overview

quarto-yaml drives `yaml_rust2::parser::Parser` through a
`MarkedEventReceiver` and converts each event's `Marker` into byte spans
(`crates/quarto-yaml/src/parser.rs`). The scanner bug in the related strand
(block-scalar content past the lookahead buffer advances `index` by bytes,
not chars) shifts every marker after a non-ASCII block scalar. It is present
in yaml-rust2 0.11.1 and still in 0.13.0 (`scanner.rs:1778-1779` of the
published 0.13.0). yaml-rust2's README now says the crate "will receive only
basic maintenance and keep a stable API", pointing to saphyr for new work.
saphyr-parser 0.1.0 has the fix (`scanner.rs:1867`, `line_buffer.chars().count()`).

Goal: take source locations from saphyr-parser instead, without changing
what quarto-yaml exposes except where the old output was wrong, and with a
repeatable way to see every location difference before and after.

## Findings

### The two libraries

| | yaml-rust2 | saphyr-parser / saphyr |
|---|---|---|
| version used / latest | 0.11 in `Cargo.toml` (0.11.1 locked) / 0.13.0 | 0.1.0 (released 2026-09-19; local clone is `v0.1.0-5-ga9c9154`, the only parser change since is a one-line doc comment) |
| maintenance | "basic maintenance, stable API" (README) | active: 56 commits in 2026-07, releases 0.0.11, 0.0.12, 0.1.0 since July |
| lineage | yaml-rust fork | same author, fork of yaml-rust2; same scanner and parser structure, so error messages and most marker placements are identical |
| yaml-test-suite | runner present; suite not shipped in the crate | 402/402 pass locally (`cargo test --test yaml-test-suite` in the clone) |
| deps (normal) | arraydeque, encoding_rs, hashlink | arraydeque, thiserror. `no_std`. The `saphyr` value crate adds hashlink, ordered-float, encoding_rs |
| MSRV / edition | 1.65 | 1.85 / 2024. quarto-yaml is already edition 2024 and CI uses stable |
| event API | `Event` (owned `String`s), `MarkedEventReceiver::on_event(Event, Marker)` | `Event<'input>` (`Cow<'input, str>`, `Option<Cow<'input, Tag>>`), `SpannedEventReceiver<'input>::on_event(Event<'input>, Span)` |
| location per event | one `Marker` (the start) | `Span { start: Marker, end: Marker }`, end exclusive |
| other API deltas | `TScalarStyle` | `ScalarStyle`; `DocumentStart(bool)` (explicit `---` or not); `Tag::is_yaml_core_schema()`; `ScanError::new_str` |

Note: the clone the user prepared is at `./saphyr`, not
`external-sources/saphyr` (that directory is empty). The probe uses the
published crates.io 0.1.0, which is identical to the clone's `parser/src`
apart from the doc comment.

### How each library communicates locations

Both use the same `Marker { index, line, col }`: `line` is 1-based, `col`
is 0-based. **`index` counts chars in both.** saphyr's doc comment
(changed on 2026-09-19 to say "in bytes", mirroring yaml-rust2 #79) is wrong:
every scanner increment is per `char` (`skip_blank`, `skip_n_non_blank`,
`line_buffer.chars().count()`, `skip_while_non_breakz` returns a char count),
and saphyr's own `parser/tests/span.rs` slices spans with
`input.chars().skip(start).take(end - start)`. The probe confirms it on
every non-ASCII input. So `YamlBuilder::byte_offset_of_char` and its cursor
stay. (A byte-offset API would be a reasonable upstream contribution later,
since `StrInput` holds a `&str` and could track bytes for free. Out of scope.)

Marker placement, from `probe-output.txt` (`!` rows). Positions are the
same in both unless stated:

| event | yaml-rust2 start | saphyr start | saphyr end |
|---|---|---|---|
| `MappingStart`, block | the `:` after the first key | the first key (after any anchor/tag) | = start |
| `MappingStart` / `SequenceStart`, flow | `{` / `[` | same | one past it |
| `MappingEnd` / `SequenceEnd`, block | start of the next token, usually the next line's first column | same | = start |
| `MappingEnd` / `SequenceEnd`, flow | `}` / `]` | same | one past it |
| `SequenceStart`, block | the first `-` | same | = start |
| `Scalar`, plain | first char | same | one past the last content char, also for multi-line (`"a\n  b\n\n  c"`) |
| `Scalar`, quoted | opening quote | same | one past the closing quote, also multi-line |
| `Scalar`, block (`\|`, `>`) | first content char | same | start of the line after the last line that belongs to the scalar. Includes the final newline, and the kept blank lines of `\|+`; excludes trailing comment/blank lines for `\|`/`\|-` |
| `Scalar`, block, empty body | at the next token, or at the `\|` header when at EOF | same | zero-length, or `\|`/`\|\n` at EOF |
| `Scalar`, empty (missing value) | at the **next token** (next key, next `- ` item, or EOF) | at the `:` of a block mapping entry; at the end of the `- ` line in a block sequence; in `{b c}` the span is the `}` | mostly = start |
| `Alias` | `*` | same | one past the name |
| tagged / anchored node | the node, never the property | same | same |
| `DocumentStart` | varies | varies (sometimes covers the first token) | ignored by quarto-yaml |
| `DocumentEnd` | varies | covers `...` / `---` when explicit | ignored |

`ScanError`: message strings and markers were identical in 12 of 13 error
inputs. The one difference is bad indentation (`a:\n  b: 1\n c: 2\n`):
yaml-rust2 reports char 12 (line 3, col 2), saphyr char 11 (line 3, col 1,
the start of the under-indented key). Both accept `a: {b c}` as
`{"b c": ""}`.

### Bugs saphyr fixes (observed)

1. **Block-scalar UTF-8 drift** (the related strand). `status: >\n  v2 — x\nbeads: y\n`:
   yaml-rust2 puts `beads` at 21, saphyr at 19. quarto-yaml 0.4.0 therefore
   spans `beads` as `ads: ` and `y` as empty, with no content provenance for
   either (`qydump-output.txt`). Two block scalars in a row accumulate; a
   40-char `é` line drifts by 39.
2. Empty-value markers point at the missing value's own position instead of
   the next key. This is an improvement but it moves spans; see risks.

### Pre-existing quarto-yaml bugs the probe exposed (backend-independent)

These come from `block_scalar_len` being given a marker that is already at
the next token, so they would survive a backend swap that keeps
quarto-yaml's own length derivation. saphyr's end marker makes them
trivially fixable (length 0).

- `a: |\nb: y\n`: the empty literal's span is `5..9` = `"b: y"`, the next
  entry. Same for `a: >\n  \nb: 1\n` (`"b: 1"`) and
  `k:\n  a: |\nj: 1\n` (`"j: 1"`). Filed as `qy-ky0yjkim`.
- `a: |\n\n`: content provenance reports `0..1`, which is inside the key.
  Needs checking with `strict-provenance`; may be a `Concat` offset
  reporting artifact rather than a wrong piece list.
- `|+` with trailing blank lines: span `"x"` only (`8..9`) while the value
  is `"x\n\n\n"` and provenance covers `8..12`. Consistent with the
  documented "trailing blank lines are left out", but the span and the
  provenance disagree about where the scalar ends.

### What quarto-yaml must keep doing

- Char index to byte offset conversion (`byte_offset_of_char`).
- Searching backwards for the tag spelling (`find_tag_span`): both parsers
  put the marker on the node and neither reports where the `!tag` was.
- Computing the mapping span from the first key for block mappings. The
  saphyr `MappingStart` marker already is the first key, so the
  `span_start` fallback in `MappingEnd` becomes dead for saphyr but can stay
  as defence.
- Scalar resolution (`resolve_scalar` and friends): quarto-yaml never used
  yaml-rust2's loader, so nothing changes here. saphyr's loader differs
  (it folds core-schema tags into values and wraps other tags in
  `Yaml::Tagged`), which is one reason not to adopt its value type now.
- The content-provenance lockstep walk. It only needs the scalar start and
  `marker.col()`; both are unchanged.

### Regression risk inventory

| risk | likelihood | how we see it |
|---|---|---|
| A marker that saphyr places differently in some construct the probe corpus lacks (e.g. `?` complex keys, `%TAG`, anchors on sequences, Windows line endings in flow scalars) | medium | golden-corpus diff (phase 0) over real Quarto YAML plus the yaml-test-suite inputs |
| Empty-value spans move from the next key to the `:` / end of `- ` line. Downstream diagnostics ("missing value", required fields) and `entry_span` shrink: `a:\n` goes from `"a:\n"` to `"a"` unless we extend the empty value past the colon | certain | golden diff; decide the rule in phase 1 |
| Accept/reject differences on malformed YAML. Both share a scanner, but saphyr has had fixes since the fork (comment intercepting plain scalars, `---` inside plain scalars, nested implicit flow mappings, `%` reserved directives, CR in literal blocks, 4H7K extra bracket). Some inputs yaml-rust2 accepted may now be rejected, and the reverse | low-medium | run both parsers over the corpus and list inputs where only one errors |
| Error marker differences (one seen: bad indentation off by one) | low | same |
| Value type: `YamlWithSourceInfo.yaml` is `yaml_rust2::Yaml`. quarto-yaml-validation has ~270 `Yaml::` variant uses; q2 (not on this machine) has an unknown number | n/a for the swap if we keep the type | see the value-type section |
| hashlink version: `yaml_rust2::yaml::Hash` is `hashlink 0.11::LinkedHashMap`. Any consumer that names hashlink must match | n/a if we keep yaml-rust2 0.11 for the type | `cargo tree -d` |
| Lifetimes: `Event<'input>` borrows the source. `YamlBuilder<'a>` already holds `&'a str`, so `impl SpannedEventReceiver<'a> for YamlBuilder<'a>` is direct; `Tag` is cloned into `BuildNode` as today | low | compiles or not |
| Benches use `yaml_rust2::YamlLoader` as the baseline | certain | keep yaml-rust2 as a dev-dependency, or switch the baseline to `saphyr::Yaml::load_from_str` |
| Performance | low | run `benches/scaling_overhead.rs` before and after |

### The value type (`Yaml`)

quarto-yaml's public API depends on yaml-rust2 only through
`YamlWithSourceInfo.yaml: yaml_rust2::Yaml` and the `new_*` constructors.

q2 (`~/rooms/room-5/q2` at `8ae461f1b`, measured 2026-10-08): depends on
`quarto-yaml = "0.4.0"` from crates.io in six crates; names `yaml_rust2`
in 75 files (quarto-core 72 lines, pampa 70, quarto-sass 16, quarto-config
7, others 11), with about 560 `Yaml::` variant matches; and pins
`hashlink = "0.11"` directly in a dozen crates with 380 use sites, so the
`Yaml::Hash` type identity (hashlink 0.11's `LinkedHashMap`) must not
change under it. 44 lines use `entry_span`/`value_span`.

Three options:

1. **Keep `yaml_rust2::Yaml` as the value type; use saphyr-parser only for
   events.** No public API change, nothing for quarto-yaml-validation or q2
   to do. yaml-rust2 stays as a dependency for a data enum with a stable API
   (`default-features = false` drops encoding_rs). Recommended for the swap.
   Also re-export it as `quarto_yaml::Yaml` (plus `Array`, `Hash`) so
   consumers can stop naming `yaml_rust2` at their own pace.
2. **Own `quarto_yaml::Yaml` enum** with yaml-rust2's shape (`Real(String)`,
   `Integer`, `String`, `Boolean`, `Array`, `Hash`, `Alias`, `Null`,
   `BadValue`) and accessors. Drops yaml-rust2 entirely. Breaking only in the
   type path; mechanical for consumers who already import through the
   re-export from option 1. A later strand.
3. **saphyr's `YamlOwned` / `MarkedYamlOwned`.** Different shape
   (`Value(ScalarOwned)`, `Representation`, `Tagged`, floats as
   `OrderedFloat<f64>`), and saphyr's README calls its API less stable.
   Would touch every `Yaml::` match downstream. Not proposed.

## Proposal

Three phases, each its own PR and release, each gated by the comparison
harness from phase 0. Phase 1 is the minimum that fixes the strand.

### Phase 0: comparison harness (the process)

The "process for understanding differences" is a three-way, corpus-driven
diff that we keep in the repo:

1. **Event probe** (exists, `investigation/probe`): yaml-rust2 vs saphyr
   events and markers. Used to reason about semantics; not a test.
2. **Golden tree dumps** (new): a dev-only dumper in `crates/quarto-yaml`
   (or `qydump` promoted) that prints every node's byte span, span text,
   content provenance (as `(start, end, length)` and the resolved text per
   piece), tag span, and the error for failing inputs. Snapshot the output
   for a corpus with `insta` (already a workspace dev-dependency). Capture
   the snapshots on `main` first, so the saphyr branch's `cargo insta
   review` shows exactly the location changes, each to be classified as
   fix, neutral, or regression in the PR description.
3. **Corpus**: the hand-written edge cases from the probe (~55);
   `crates/quarto-yaml-validation/test-fixtures/schemas/*.yml` (4,121 lines
   of real Quarto schema YAML, 169 block scalars, 15 non-ASCII lines in
   `definitions.yml`); and the 351 yaml-test-suite cases from the clone
   (`saphyr/parser/tests/yaml-test-suite/src`, `in.yaml` of each) for
   accept/reject and no-panic coverage. Real `.qmd` frontmatter from the
   Quarto docs would be a good addition if available.
4. **Accept/reject diff**: for the corpus, list inputs where exactly one
   backend errors, or where error messages or markers differ.

### Phase 1: backend swap, spans unchanged except for fixes (release 0.5.0)

- Replace `yaml-rust2` with `saphyr-parser = "0.1"` in `crates/quarto-yaml`
  for parsing. Keep `yaml-rust2 = "0.11"` for the `Yaml` type
  (`default-features = false`), and re-export `Yaml`, `Array`, `Hash` from
  `quarto_yaml`.
- `YamlBuilder`: `impl SpannedEventReceiver<'a>`; use `span.start`
  everywhere a `Marker` was used; `DocumentStart(_)`; `ScalarStyle`;
  `&*value` into `compute_scalar_provenance` and `resolve_scalar`;
  `tag.map(Cow::into_owned)`.
- Keep quarto-yaml's own scalar length derivation (`plain_scalar_len`,
  `quoted_scalar_len`, `block_scalar_len`) so spans do not change, with
  two deliberate exceptions, both fixes: the block-scalar drift, and the
  empty block scalar spanning the next entry (use `span.len() == 0` to
  short-circuit `block_scalar_len`).
- Empty missing values: the marker now sits on the `:`. Decide the span
  rule (see decisions) and pin it with tests.
- Tests: the two tests the related strand asks for (key/value after `>`
  and `|` with non-ASCII content; two such scalars in a row), span and
  provenance; the empty-block-scalar cases above; `strict-provenance`
  passes over the whole corpus.
- Update the README and the `lib.rs` docs (they name yaml-rust2), the
  benches' baseline, and `YAML-1.2-REQUIREMENT.md` in the validation crate.
- Release notes: dependency change, the two span fixes, the empty-value
  position change, the `quarto_yaml::Yaml` re-export.

### Phase 2: adopt saphyr's end markers (release 0.6.0)

Use `span.end` as the primary source of scalar and collection lengths and
keep the hand-written length functions only as a cross-check under
`strict-provenance` (assert they agree, or document each disagreement).
Behavioural changes to expect and decide on: block scalar spans gain their
final newline and `|+` kept blank lines; aliases get a `*name` span instead
of zero length; `collection_end` no longer needs to peek at `]`/`}`. This
phase is where the golden diff earns its keep.

### Phase 3 (optional, later): own `Yaml` type

Option 2 above, once q2 imports `quarto_yaml::Yaml`. Removes the last
yaml-rust2 dependency and the hashlink coupling.

## Decisions (2026-10-08, Carlos)

1. Value type: option 1 (keep `yaml_rust2::Yaml`, re-export it as
   `quarto_yaml::Yaml`), with README and rustdoc text steering consumers to
   the `quarto_yaml` paths. Phase 3 is the follow-up strand `qy-0qodo40x`.
2. Empty missing value span: zero-width just after the `:`
   (`YamlBuilder::scalar_start`; only for values, an empty *key* stays on
   its colon so tag lookup keeps working).
3. Block-scalar span rule: keep today's "through the last content char".
   Phase 2 is therefore not planned; `span.end` is used only to detect the
   empty block scalar (`qy-ky0yjkim`).
4. `saphyr-parser = "0.1"`, patch version pinned by `Cargo.lock`; periodic
   monitoring is the chore strand `qy-hpp43tsj`.
5. Golden corpus and dumper live in `crates/quarto-yaml/tests/`.
6. q2 is at `~/rooms/room-5/q2`; its dependent crates' tests are run
   against the branch with a `[patch.crates-io]` (results in the PR).

## Phase 1 outcome

Golden diff (yaml-rust2 baseline → saphyr), every change classified:

- Fix: spans and content provenance after non-ASCII block scalars. In
  `definitions.yml` the baseline had 3,687 scalars with `content=NONE`
  (a curly apostrophe in a block scalar near byte 17k shifted everything
  after it); now 0.
- Fix: empty block scalar no longer spans the next entry (`qy-ky0yjkim`).
- Fix: flow-pair mappings inside flow sequences (`[ {a: b}:c ]`) no longer
  extend to the closing `]`.
- Change (decision 2): missing values sit just after the `:`, or at the end
  of a `- ` line; `entry_span` of `a:` is `"a:"` instead of `"a:\n"`.
- Change: "did not find expected key" on bad indentation points at the
  start of the under-indented key, not at its colon.
- Change: yaml-test-suite cases 4H7K (extra `]`) and BS4K (comment
  intercepting a multi-line plain scalar) are now rejected, as the suite
  requires; two messages on already-rejected inputs changed. Disagreements
  with the suite went from 9 to 7; no new ones.
- Pre-existing, unchanged: 15 inputs the provenance walk cannot derive
  (`qy-0ongzhi7`); the golden tests are ignored under `strict-provenance`
  because of them.

## Checklist

### Phase 0: harness
- [x] Add the golden tree dumper and the insta-based corpus test (`crates/quarto-yaml/tests/golden.rs`)
- [x] Assemble the corpus (`tests/corpus/edge-cases`, `tests/corpus/yaml-test-suite` via `gen_corpus`, schema fixtures read in place)
- [x] Capture snapshots with the yaml-rust2 backend and commit them (first commit on the branch)
- [x] Accept/reject record: the `yaml_test_suite` snapshot header lists disagreements with the suite (9 under yaml-rust2)

### Phase 1: swap
- [x] File the confirmed discovered bug (`qy-ky0yjkim`, empty block scalar spans the next entry)
- [x] The `strict-provenance` golden run surfaced the real desyncs: filed as `qy-0ongzhi7`. (`|+` span vs provenance is by design under decision 3; `a: |\n\n` does not desync.)
- [x] Add `saphyr-parser` to the workspace deps; set `yaml-rust2` to `default-features = false`
- [x] Port `YamlBuilder` to `SpannedEventReceiver`
- [x] Re-export `Yaml`, `Array`, `Hash` from `quarto_yaml`
- [x] Decide and implement the empty-value span rule
- [x] Short-circuit `block_scalar_len` on zero-length spans
- [x] Tests from the related strand (`>`/`|` with non-ASCII, two in a row) and for the empty-value rule
- [x] Run the golden diff; classify every change (above and in the PR); update snapshots
- [x] `cargo test --workspace`, with and without `strict-provenance`; clippy; fmt
- [x] Benches: unchanged (they use `yaml_rust2::YamlLoader` as the memory baseline, which still builds without the `encoding` feature)
- [x] Docs: README, `lib.rs`, `YAML-1.2-REQUIREMENT.md`; release notes in the PR
- [x] Version 0.5.0
- [x] q2 dependent-crate tests against the branch (2026-10-08, `~/rooms/room-5/q2` at `8ae461f1b`, `[patch.crates-io]` to the branch): 19 test binaries, 11,717 passed / 145 failed with the branch vs 11,731 / 131 unpatched. The failure sets overlap almost entirely (pandoc "current working directory no longer exists", typst, julia, engine fixtures). The 16 tests failing only with the branch all pass when re-run against the branch single-threaded, so they are the same flakiness; the 2 failing only unpatched are too. No failure is attributable to the swap. Logs in the session scratchpad; not kept.
- [ ] PR, merge; record the release in the strands; close `qy-block-scalar-utf8-drift-7dccrmto` and `qy-ky0yjkim`

### Phase 2: end markers (not planned; decision 3 keeps today's span rules)

### Phase 3: own `Yaml` type (strand `qy-0qodo40x`, after q2 moves to the re-export)
- [ ] Define `quarto_yaml::Yaml` with yaml-rust2's shape and accessors
- [ ] Port quarto-yaml-validation
- [ ] Drop yaml-rust2
