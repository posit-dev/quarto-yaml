# Switch YAML backend from yaml-rust2 to saphyr

## Overview

Replace `yaml-rust2 0.11` with the maintained successor `saphyr` /
`saphyr-parser` (0.0.11, 2026-07-11) across both crates. Branch
`switch-to-saphyr`, based on `flow-collection-close-spans` (PR #12, which
stacks on PR #11) because the migration rewrites the same span code paths and
must preserve the tests those PRs added.

(braid is not installed in this environment, so no strand was filed; this
plan file is the tracking document.)

## Findings that shaped the design (verified empirically against 0.0.11)

Probe program: scratchpad `probe/` (parses a battery of inputs, prints every
event with its span).

- `Marker::index()` is still **char-based** (getter doc says bytes; scanner
  increments once per char). The char→byte table from PR #11 **stays**.
- `SpannedEventReceiver` supplies a full `Span { start, end }` per event:
  - Plain scalar span = exact content (folded lines included, trailing
    comment excluded) — matches our `plain_scalar_len`.
  - Quoted scalar span includes the quotes — matches `quoted_scalar_len`.
  - Block scalar span starts at first content char (header excluded) but
    **includes trailing newline/blank lines**, which our semantics exclude →
    trim trailing whitespace for Literal/Folded styles.
  - Flow `SequenceEnd`/`MappingEnd` span covers the closer; `span.end` is
    past `]`/`}` — natively what PR #12 hand-rolled (`collection_end`).
  - Block `MappingStart` points at the first key (yaml-rust2 pointed at the
    first `:`). Keep first-key anchoring as belt-and-suspenders via `min()`.
  - Scalar spans do **not** include tag/anchor properties → keep
    `find_tag_span`'s backward walk.
  - Empty scalar (e.g. `key:`) arrives as value `"~"` with an empty span.
- Value type: `saphyr::YamlOwned` (no lifetime params, fits the owned-data
  design): `Value(ScalarOwned::{Null,Boolean,Integer,FloatingPoint(OrderedFloat<f64>),String})`,
  `Sequence(Vec)`, `Mapping(LinkedHashMap)`, `Tagged`, `Alias`, `BadValue`,
  `Representation`. Accessors: `as_str/as_bool/as_integer/as_floating_point/
  as_vec/as_mapping/as_mapping_get/...`.
- `Yaml::Real(String)` has no equivalent — floats become
  `FloatingPoint(OrderedFloat<f64>)`. `saphyr::parse_core_schema_fp` is
  exactly our `is_yaml_float` rule set and returns the `f64`.
- We keep our own scalar resolution (`resolve_scalar` & friends) rather than
  `ScalarOwned::parse_from_cow_and_metadata`, to keep the precisely-tested
  core-schema behavior (hex/octal edge cases, overflow→string, BadValue on
  tag mismatch) under our control. We do not use `YamlOwned::Tagged`; tags
  stay in `YamlWithSourceInfo::tag` as today.

## Checklist

### Phase 1: quarto-yaml

- [x] Workspace + crate Cargo.toml: drop `yaml-rust2`, add `saphyr`,
      `saphyr-parser` (0.0.11)
- [x] `parser.rs`: port `YamlBuilder` to `SpannedEventReceiver`; spans from
      event `Span`s (char→byte converted); delete `compute_scalar_len`,
      `quoted_scalar_len`, `plain_scalar_len`, `block_scalar_len`,
      `collection_end`; trim trailing whitespace for block scalars; keep
      `find_tag_span`; `resolve_*` return `YamlOwned`
- [x] `yaml_with_source_info.rs`: `yaml: YamlOwned`, doc updates
- [x] `error.rs`: `From<saphyr_parser::ScanError>`
- [x] `lib.rs`: re-export `saphyr` types consumers need; doc updates
- [x] Benches: `YamlOwned::load_from_str`, memory-estimate arms for
      `YamlOwned`
- [x] Tests green in quarto-yaml (span tests from PRs #4/#11/#12 must pass
      unchanged except `Real`→`FloatingPoint` adjustments)

### Phase 2: quarto-yaml-validation

- [x] Cargo.toml: `yaml-rust2` → `saphyr`
- [x] Mechanical port of `Yaml::*` patterns (~280 sites): `String/Integer/
      Boolean/Null` → `Value(Scalar::…)`, `Array`→`Sequence`,
      `Hash`→`Mapping`, `Real(s)`→`FloatingPoint`, `as_i64`→`as_integer`,
      `as_f64`→`as_floating_point`
- [x] Tests green in quarto-yaml-validation

### Phase 3: docs & wrap-up

- [x] READMEs, `YAML-1.2-REQUIREMENT.md`, crate docs: yaml-rust2 → saphyr
- [x] `cargo test --workspace`, clippy, fmt
- [x] Commit(s) + PR (stacked on PR #12)

## Details

- Mapping span start = `min(byte(MappingStart.span.start), first key start)`:
  flow mappings anchor at `{` (before the first key), block mappings at the
  first key regardless of where the start marker points.
- Sequence span = `byte(SequenceStart.span.start) .. byte(SequenceEnd.span.end)`.
- Float semantics change: `Yaml::Real` stored the source string; saphyr
  stores the parsed `f64`. Validator code that parsed `Real` strings now
  reads the float directly; `1e3`-style spellings lose their source
  spelling in the value (still recoverable via the span).
- `Event::DocumentStart` gained a `bool` (explicit `---`) payload.
- yaml-rust2's `TScalarStyle` → saphyr's `ScalarStyle` (same variants).
- Scalar event payloads are `Cow<str>` / `Option<Cow<Tag>>` now.

## Outcome (2026-08-01)

All phases complete on branch `switch-to-saphyr` (based on
`flow-collection-close-spans`). Workspace: 349 tests + 5 doctests green,
clippy clean, fmt applied, both benches run.

Implementation notes beyond the original plan:

- saphyr's span **end** markers for quoted scalars and flow-collection end
  events are not recorded until the scanner has skipped trailing spaces and
  `# comment` on the line, so they overshoot. Consequently:
  - Quoted scalars are still measured with the escape-aware
    `quoted_scalar_len` walker (restored) instead of the event span end.
  - Collections use the end event's span **start** (which sits on the
    closer, exactly like yaml-rust2's end marker) plus PR #12's
    opener/closer check (`collection_end`), rather than `span.end`.
  - Plain-scalar spans and block-scalar spans (after a trailing-whitespace
    trim) come straight from the event span; `plain_scalar_len` and
    `block_scalar_len` are deleted.
- `test_get_hash_number_invalid_real` and `test_yaml_to_json_value_invalid_real`
  relied on `Yaml::Real` holding an unparseable string, a state that cannot
  exist with `FloatingPoint(f64)`; the former now uses a string value, the
  latter was folded into the infinity/NaN tests.
- `yaml_type_name`/`yaml_to_json_value` gained arms for saphyr's
  `Representation` and `Tagged` variants (unreachable from our parser).
- The scaling bench's flat-array "superlinear" flag fires on main too
  (ratio Δ26% vs the bench's 25% threshold); overhead ratios are slightly
  lower with saphyr. Not a regression.
