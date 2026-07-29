# Core-schema hex/octal integer resolution (GH #3, braid qy-3g25mtzw)

- GitHub issue: https://github.com/posit-dev/quarto-yaml/issues/3
- Braid strand: `qy-3g25mtzw`
- Affected code: `crates/quarto-yaml/src/parser.rs`

## Overview

The YAML 1.2 [core schema](https://yaml.org/spec/1.2.2/#103-core-schema)
resolves `!!int` from three regexes: `[-+]?[0-9]+` (decimal), `0o[0-7]+`
(octal), and `0x[0-9a-fA-F]+` (hex). Both of our resolution paths parse
integers with `i64::from_str`, which is decimal-only:

- `resolve_plain_scalar` (parser.rs:630): plain `0x1f` falls through to
  `String("0x1f")`. Should be `Integer(31)`.
- `resolve_tagged_scalar` (parser.rs:603): explicit `!!int 0x1f` fails the
  parse and becomes `Yaml::BadValue`. Should be `Integer(31)`.

yaml-rust2's own implicit resolution (`Yaml::from_str`) handles both prefixed
forms, so we currently diverge from both the spec and the library we wrap.
This was noticed while fixing #2 (PR #4, commit 53d325c), which reworked
scalar resolution but deliberately left this divergence for a follow-up.

## Findings

1. **Only two call sites are affected.** A workspace-wide grep for
   `parse::<i64>` / `from_str_radix` finds only the two calls in
   `parser.rs`. `quarto-yaml-validation` consumes already-resolved `Yaml`
   values (`as_i64`), so it inherits the fix for free.

2. **The core-schema forms are narrower than "whatever Rust parses".** The
   prefixes are lowercase-only (`0x`, `0o`) and unsigned. So `0X1f`, `-0x1f`,
   `+0x1f`, and `0b101` (no binary form in the core schema) are genuinely
   strings, and the issue explicitly asks that they stay that way.

3. **`i64::from_str_radix` is too permissive to use unguarded.** It accepts a
   leading `+`/`-`, so `from_str_radix("+7", 8)` succeeds — but `0o+7` must
   stay a string. The helper must validate the digit run itself (non-empty,
   only `[0-9a-fA-F]` / `[0-7]`) before calling `from_str_radix`.

4. **Decimal is already exactly right.** `i64::from_str` accepts precisely
   `[-+]?[0-9]+` (modulo overflow), so the decimal path needs no change.

5. **Overflow.** `0xFFFFFFFFFFFFFFFF` matches the `!!int` regex but exceeds
   `i64`. yaml-rust2 falls back to `String` for plain scalars in this case;
   our current decimal behavior does the same. Decision below.

## Decisions

- **One shared helper, used by both paths** (as the issue suggests):

  ```rust
  /// Parse an integer in one of the YAML 1.2 core schema's three forms:
  /// `[-+]?[0-9]+`, `0o[0-7]+`, `0x[0-9a-fA-F]+`.
  fn parse_core_schema_int(value: &str) -> Option<i64>
  ```

  - `0x` prefix (exact case): digit run must be non-empty and all
    `is_ascii_hexdigit`, then `i64::from_str_radix(digits, 16)`.
  - `0o` prefix (exact case): digit run non-empty and all in `'0'..='7'`,
    then `from_str_radix(digits, 8)`.
  - Otherwise: `value.parse::<i64>()` (unchanged decimal behavior).

- **Overflow → `None`**, falling out naturally from `from_str_radix`'s error.
  Consequences: plain overflow stays `String` (matches yaml-rust2 and our
  current decimal behavior); tagged `!!int` overflow stays `BadValue`
  (matches our current decimal behavior).

- **Tagged path diverges from yaml-rust2, deliberately.** yaml-rust2's loader
  also uses decimal-only parsing for explicit `!!int` tags, so after this fix
  `!!int 0x1f` is `Integer(31)` for us but `BadValue` for yaml-rust2. The
  spec is unambiguous that the tag's resolution rules are the core-schema
  regexes, and the issue asks for the shared-helper fix; spec wins over
  bug-for-bug parity here.

- **No float changes.** `is_yaml_float` is untouched; hex/octal floats don't
  exist in the core schema.

## Plan

- [x] Add `parse_core_schema_int` helper to `parser.rs` (near the other
      resolution helpers), with a doc comment citing the core-schema regexes.
- [x] Use it in `resolve_plain_scalar` (replacing the bare
      `value.parse::<i64>()`).
- [x] Use it in `resolve_tagged_scalar`'s `"int"` arm (`Some` → `Integer`,
      `None` → `BadValue`).
- [x] Update the doc comments on `resolve_plain_scalar` /
      `resolve_tagged_scalar` if they describe integer handling.
      (`resolve_plain_scalar` now points at the helper; `resolve_tagged_scalar`
      didn't describe integer forms, so it needed no change.)
- [x] Tests, following the existing table-driven style in `parser.rs`'s
      test module (cf. `test_plain_booleans_follow_yaml_12_core_schema`):
  - Plain positives: `0x1f` → 31, `0xFF` → 255 (mixed-case digits are
    legal), `0o17` → 15, `0o0` → 0.
  - Plain negatives (stay strings): `0X1f`, `-0x1f`, `+0x1f`, `0b101`,
    `0x` / `0o` (empty digit run), `0o18` / `0o8` (bad octal digit),
    `0xg1`, `0o+7` / `0x-1f` (sign after prefix),
    `0xFFFFFFFFFFFFFFFF` (overflow).
  - Quoting still wins: `"0x1f"` quoted → `String`.
  - Tagged: `!!int 0x1f` → 31, `!!int 0o17` → 15; `!!int 0X1f` and
    `!!int 0xFFFFFFFFFFFFFFFF` → `BadValue`.
  - Decimal regression guard: `42`, `-7`, `+7` still integers.
- [x] `cargo test` across the workspace (346 tests pass); `cargo clippy`
      clean.
- [ ] Close `qy-3g25mtzw` and open a PR referencing GH #3.

## Open questions

- None blocking. One judgment call worth confirming: the tagged-path
  divergence from yaml-rust2 (`!!int 0x1f` → `Integer`, not `BadValue`) —
  the plan follows the spec and the issue's framing, but flag it in the PR
  description.
