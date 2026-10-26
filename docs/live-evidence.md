# Canonical live-evidence primitives (phase 1)

**Status: implemented, independently reviewed, locally verified and committed as
`c1cc27e637c8c113a219a93250543fe437f08d49` (no push). Not a live producer,
session envelope, consent channel or qualification result.** This page documents
the phase-1 byte/integer/digest layer of the proposed `autotune.live-session` v1
format. It was authorized by the local
[phase-1 scope](../_bmad-output/specs/spec-e3-first-real-benchmark/live-evidence-codec-scope.md)
and implemented under
[the phase-1 spec](../_bmad-output/implementation-artifacts/spec-e3-live-evidence-primitives.md).
Those local artifacts are git-ignored and may be absent from a fresh clone.

The codec produces and validates bytes only. Every value is injected by the
caller. A passing round trip never creates live evidence, promotes provenance,
grants consent or approval, or authorizes native work.

## Representation

Documents are compact UTF-8 JSON in fixed model order with no whitespace. The
value model is:

| Model value | Canonical bytes |
| --- | --- |
| typed null | `null` |
| boolean | `true` / `false` |
| text | JSON string with the escaping below |
| unsigned integer | quoted canonical decimal, e.g. `"0"`, `"18446744073709551615"` |
| signed integer | quoted canonical decimal, e.g. `"-9223372036854775808"` |
| ratio | `{"num":"<i64>","den":"<u64>"}` with `den > 0` |
| array | `[value,...]` in caller order |
| object | `{"key":value,...}` in insertion order |

Objects keep insertion order and the writer never sorts. A duplicate key is a
typed insertion error. The exact-field helper requires exactly the supplied field
names in the supplied order, so later field mapping can reject unknown, missing
and reordered fields without a second parser. `Object::without` removes one
top-level field only: nested fields with the same name are untouched, and an
absent key is a silent no-op that returns an equal object rather than an error.

Text and keys are compared byte for byte. There is no Unicode normalization, case
folding, whitespace trimming or escape canonicalization on top of the rules above,
so two strings that differ only in normalization form are different bytes and
therefore different documents.

`decode` returns any `Value`, including a scalar or array root. Phase-2 live-session
documents must be root objects; that shape rule belongs to the envelope layer and
the codec does not enforce it.

Integers are always quoted decimal strings. Canonical form has no `+`, no leading
zeros and no negative zero; `u64` and `i64` extremes survive losslessly. A ratio
is the exact two-field object above with a positive denominator. Ratios are exact
pairs and are never reduced: `{"num":"2","den":"4"}` and `{"num":"1","den":"2"}`
are valid, distinct values.

Text escapes `"` and `\`, uses `\b`, `\f`, `\n`, `\r` and `\t` for those
controls, uses lowercase `\u00xx` for other controls below `0x20`, and emits `/`
and non-ASCII characters as raw UTF-8.

The writer has no depth limit of its own: it trusts the depth of the caller-built
value and will encode any nesting it is given. The reader enforces 32 containers,
so a deeper caller-built value encodes successfully but does not decode.

Worked example: a root object with one field `t` whose text is `a"b\c`, a newline
and the control byte `0x01` encodes to `{"t":"a\"b\\c\n\u0001"}`, which is exactly
23 UTF-8 bytes (all ASCII). That payload's session digest is the 23-byte row in the
vector table below.

## Digest convention

A digest is SHA-256 over:

1. the fixed domain prefix,
2. one `0x00` byte,
3. the exact canonical payload bytes, excluding the payload's own digest field.

| Domain | Prefix |
| --- | --- |
| manifest | `autotune.live-evidence.manifest.v1` |
| profile | `autotune.live-evidence.profile.v1` |
| session | `autotune.live-evidence.session.v1` |
| raw artifact | `autotune.live-evidence.raw-artifact.v1` |

Hex form is exactly 64 lowercase characters. Parsing rejects uppercase, wrong
length and non-hex text. A digest binds bytes, not truth, consent or authority.

`Digest::of_value(domain, &Value)` is the canonical entry point: it hashes
`encode(value)`, so the digest can only ever cover canonical bytes.
`Digest::of(domain, payload)` still takes arbitrary bytes for callers that already
hold an exact canonical payload; it does not validate or normalize them.

Pinned golden vectors (independently computed; prefix + `0x00` + payload):

| Domain | Payload | UTF-8 bytes | SHA-256 |
| --- | --- | --- | --- |
| session | `{"a":"1"}` | 9 | `af8aacad2d10f122084a16cfd7f84fbab66e39b43afbca3c0c715d9f8dd61d9a` |
| manifest | `{"a":"1"}` | 9 | `ee94e6fb273b02fd4c6bf2c1127989d49624770a310b0987afa0308e40e9228e` |
| profile | `{"a":"1"}` | 9 | `16fb94aa41263c0421a2df29d590dd2a4bbea05278ae42ac8371054b6ebb2169` |
| raw artifact | `{"a":"1"}` | 9 | `d80a361d2be4c63e3961e4d37abc5c901b50f4768ca1e12b845d11e48866ab5b` |
| session | `{}` | 2 | `c5dd725be0ec0ec15a7fdf435537919680d4c29b5cdb416693361d8e43539d12` |
| session | `{"t":"a\"b\\c\n\u0001"}` | 23 | `56787dab9110e2370187a1a08f4f2ddcccb1de627a499ade4f4a9187255c6ecf` |
| session | `{"a":["0","18446744073709551615"]}` | 34 | `cd91a4421ce474cd2b4ab5808723377fadd44086e1c4cc003256d22723d50428` |

The last two rows have a retained independent derivation that does not use the
Rust codec: `C:\Users\manni\AppData\Local\Temp\opencode\autotune-evidence-vector-derivation.ps1`,
SHA-256 `2d744ce6bd04584164d89990492ae9c834cd047f8dfd6527e1ecbe55c09a8a26`,
with its log
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-evidence-vector-derivation.log`,
SHA-256 `a6dd0e3b54b6c960e3086a1cdd523eae1524218d0390be21e646660d6409b128`.
The Rust tests pin these constants; they did not produce them.

`Object::without` removes a named top-level field before encoding, which is how a
document's own digest field is excluded from its payload.

## Versioning

The v1 bytes above are frozen. Any byte change to the canonical form, escaping,
digest construction or envelope field set requires a new `.v2` domain prefix and a
new schema ID; a v1 reader must never silently accept re-spelled v1 bytes.

## Strict reader

`evidence::parse::decode(input, max_bytes)` applies a caller-supplied byte bound
and a structural nesting bound of 32 containers. The byte bound is inclusive: an
input of exactly `max_bytes` bytes is accepted and only a longer input is
`Oversize`. It accepts only `{`, `[`, `"`, `true`, `false` and `null` at a value
position. Any digit, `-`, `+` or `.` at a value position is a number-token error.

The reader returns a typed error and no partially accepted document for:

| Rejection | Error |
| --- | --- |
| input longer than the caller's bound | `Oversize` |
| end of input inside a value, string or literal | `Truncated` |
| bytes after the top-level value | `Trailing` |
| more than 32 nested containers | `Depth` |
| invalid UTF-8 | `Utf8` |
| repeated object key | `DuplicateKey` |
| invalid escape character or `\u` hex | `Escape` |
| lone or malformed UTF-16 surrogate pair | `Surrogate` |
| raw control byte inside a string | `Control` |
| JSON number token at a value position | `Number` |
| unexpected structural byte, whitespace or literal | `Syntax` |
| accepted value whose re-encode differs from the input | `Canonical` |

A document is accepted only when re-encoding it reproduces the input bytes
exactly. Whitespace, `\u`-escaped ASCII, an escaped slash and other non-canonical
spellings are therefore rejected rather than normalized. Escaped controls that
the writer itself emits (for example `\u0000`) remain canonical. An escaped key
such as `{"\u0061":"1"}` is likewise `Canonical`, and a duplicate that only
appears after unescaping is still `DuplicateKey`. Uppercase `\u` hex is a valid
escape, so `{"a":"\u00E9"}` fails the canonicality check as `Canonical`, not as
`Escape`; `Escape` is reserved for an unknown escape character or a non-hex
digit. A leading UTF-8 BOM is not a value-position byte and is `Syntax`.

Because the generic reader cannot know which text fields are numeric, canonical
decimal form is enforced by typed accessors, which also accept their own typed
variants so a value the writer produced round-trips without re-parsing text:

- `Value::as_u64` accepts `U64(n)` and non-negative `I64(n)` directly, and parses
  canonical unsigned text otherwise. A negative `I64` is `Sign`.
- `Value::as_i64` accepts `I64(n)` and any `U64(n)` that fits, and parses canonical
  signed text otherwise. A `U64` above `i64::MAX` is `Overflow`.
- Text forms reject `+` and any other signed spelling with `Sign`, except that a
  bare `-` is `Digit` for both accessors and `-0` is `NegativeZero` for the signed
  accessor (the unsigned accessor reports `Sign`); leading zeros are
  `LeadingZero`; any other non-digit is `Digit`; out-of-range values are
  `Overflow`; empty text is `Empty`; non-text, non-typed values are `NotText`.
- `Value::as_ratio` accepts a `Ratio` directly, or requires the exact ordered
  `num`/`den` fields, canonical integers and a positive denominator.

Decoding a canonical integer or ratio yields text/object structure; the original
number is recovered with these accessors, not by assuming a typed decode.

## API

Portable module `autotune_benchmark_worker::evidence`, with no Windows, monitor,
clock, process or CUDA dependency:

- `canonical::{Value, Object, Ratio, encode}` and accessors/errors.
- `digest::{Domain, Digest}` with `Digest::of_value(domain, &Value)` as the
  canonical entry point, `Digest::of(domain, payload)` for exact raw bytes,
  `to_hex` and `parse`. `Domain` and `Digest` derive `Hash`, `PartialOrd` and
  `Ord`.
- `parse::{decode, ParseError, MAX_DEPTH}`.

The module re-exports the same names at `evidence::*`. No new dependency is
added: the existing `sha2` dependency is reused.

## Local verification

Date: 2026-10-26. Baseline HEAD `f44ccf01b0f0e5d77172ad7b72c7a52cd1e5033a`
with the then-uncommitted phase-1 working-tree changes (committed unchanged as
`c1cc27e637c8c113a219a93250543fe437f08d49`). Installed Rust 1.99.1 Windows
GNU, existing LLVM-MinGW, `--locked --offline`, fresh target
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-evidence-parent-final-20261026-target`.

Post-review parent run:

```text
cargo +stable fmt --all -- --check
cargo +stable clippy --workspace --all-targets --locked --offline -- -D warnings
cargo +stable test --workspace --no-run --locked --offline
cargo +stable test -p autotune-recovery-contract -p autotune-recovery-core \
  -p autotune-measurement -p autotune-fma-reference --locked --offline
cargo +stable test -p autotune-probe --lib --locked --offline
cargo +stable test -p autotune-probe --test probe --locked --offline -- \
  --skip every_matrix_scenario_is_deterministic_and_cli_matches_schema \
  --skip usage_help_and_default_mock_exit_behavior
cargo +stable test -p autotune-power-trial --lib --test safety --locked --offline
cargo +stable test -p autotune-benchmark-worker --lib --test worker \
  --test monitor --test evidence --locked --offline
cargo +stable clippy -p autotune-benchmark-worker --all-targets \
  --features native-cuda --locked --offline -- -D warnings
cargo +stable test -p autotune-benchmark-worker --all-targets \
  --features native-cuda --locked --offline --no-run
cargo +stable test -p autotune-benchmark-worker --lib \
  --features native-cuda --locked --offline
```

All pass first attempt: **256 selected CPU-only tests**, including all 13
`evidence` tests, 36 monitor, 8 worker integration, one worker library, 147
model/FMA/measurement, 12 probe library, 9 probe pure (two CLI tests filtered)
and 30 trial safety; four native-feature library typed/static tests pass
separately. Worker-process and trial-CLI targets were not selected and no
`--ignored` run occurred. The earlier dev-run manifest
`autotune-evidence-phase1-20261026-manifest.json`
(SHA-256 `189575d2fb70c2fa34008638a8f7de487e14bce257ee6ac617a7c55c6532604e`)
is historical: its test run preceded the `digest.rs` lint fix, so it is not the
post-patch result.

Source hashes for this pass:

| File | SHA-256 |
| --- | --- |
| `src/evidence/canonical.rs` | `bf2d57beca2aacffa40b102ced8d119e2ee21026c34f6c561901c9452a3b5e64` |
| `src/evidence/digest.rs` | `168f47b94c8218d43af1f55a42b7cf51269a3dd3616b64b6f671a34ae9817d85` |
| `src/evidence/parse.rs` | `6f66c26995d58d0b12ec1d3fb90b81c348a602a11129dd8cbdd2590fcee9282e` |
| `src/evidence/mod.rs` | `9df7991b9fc0f6ac000165c4a78e1bef6a9a1b81df482eaf6b7b737eebf1f7bf` |
| `src/lib.rs` | `50cdd7bb9be8d5c0b3ca3bbe57822510cc2c3daee001dc0a36ad7f956cda67fb` |
| `tests/evidence.rs` | `22f4903cd10a44c37d7307c9b9a3c261396f7b50b8d70615737111552623b271` |

Manifest:
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-evidence-parent-final-20261026-manifest.json`,
SHA-256 `2d86bf46d385d8835681757a30d417a2b73ee9803f25df7aae87fdb160996976`.
It binds the baseline, every command with logs/exit codes, identical before/after
hashes of 105 source/CI files and the independent vector derivation. This later
documentation/roadmap annotation changes no tested Rust/CI source. Not run:
hosted CI, native loading, GPU execution, OS clock reads, process fixtures or
`--ignored` tests. No staging, commit or push in the verified run.

## Limits and non-authority

This is a byte/digest layer only. It defines no session envelope fields, binding
or admission semantics, live producer, clock source, process or workload launch,
CUDA/NVML/PTX call, consent or approval channel, baseline/energy/search verdict or
release-gate closure. Phase 2 maps envelope fields separately and requires its own
approval; it must not change phase-1 bytes. Fixtures encoded here remain
`simulated_fixture`, and a valid digest or round trip is not a correctness,
baseline, GPU-quiescence or tuning claim.
