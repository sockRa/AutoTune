# Live-evidence session envelope v1 (phase 2)

**Status: October 28 owner-resolved timing and metric-slot fixes implemented;
focused parent verification passed and independent static review approved. Uncommitted.
A validated envelope is not live evidence, consent, admission or
qualification.** This page documents the typed `autotune.live-session` v1
envelope that sits on top of the [phase-1 primitives](live-evidence.md). It was
authorized by the local
[phase-2 envelope spec](../_bmad-output/implementation-artifacts/spec-e3-live-evidence-envelope.md);
that artifact is git-ignored and may be absent from a fresh clone.

The envelope module defines field names, fixed order, observation encoding,
provenance binding, one session digest and cross-field validation. Every value
is injected by the caller. There is no live producer, no OS clock read, no
process launch, no NVML/CUDA/PTX call, no consent/approval authority, no
baseline statistics and no release-gate closure. Phase-1 bytes, error variants
and digest construction are unchanged.

## Field order

Top level, exactly 13 keys in this order:

`schema`, `version`, `provenance`, `identity`, `artifacts`, `manifest`,
`clocks`, `lifecycle`, `dispatches`, `telemetry`, `integrity`, `result`,
`digest`.

`schema` is exactly `autotune.live-session` (public `SCHEMA`); `version` is
exactly `1` (public `VERSION`); `provenance` is exactly `live_observed` or
`simulated_fixture`. `dispatches` and `telemetry` are arrays; every other section
is an object.

Section fields, exact order. `Obs<T>` means either a known `T` or
`{"unavailable":"<reason>"}`; `Hex64` is 64 lowercase hex; `Ratio` is the exact
`{"num":"<i64>","den":"<u64>"}` pair.

| Section | Field | Type |
| --- | --- | --- |
| `identity` | `nonce` | text, 1..=128 printable ASCII |
| | `nvml_uuid`, `cuda_uuid`, `pci_bus_id`, `context_device`, `boot` | `Obs<text>` |
| | `uuid_match`, `pci_match` | `match` \| `mismatch` \| `unknown` |
| `artifacts` | `source_commit` | exactly 40 lowercase hex |
| | `worker_sha256`, `lockfile_sha256`, `sampler_sha256`, `ptx_sha256` | `Obs<Hex64>` |
| | `kernel_entry`, `generator_version`, `oracle_version` | text |
| `manifest` | `manifest_digest`, `profile_digest` | `Obs<Hex64>` |
| | `shape`, `stop_policy`, `approval_ref` | `Obs<text>` |
| | `seed` | `Obs<u64>` |
| | `control_writes` | `none` |
| `clocks` | `qpc_frequency` | `u64 > 0` |
| | `qpc_epoch_ticks`, `interrupt_ticks` | `u64` |
| | `unbiased_ticks` | `Obs<u64>` |
| | `resolution_ns` | `u64 > 0` |
| | `utc_context` | `Obs<text>` |
| | `discontinuities` | `u64` |
| `lifecycle` | `requested_ns`, `launch_ns`, `ready_ns`, `start_ns`, `stop_requested_ns`, `exit_ns` | `Obs<u64>` |
| | `empty_job` | `observed_empty` \| `observed_nonempty` \| `unknown` |
| `dispatches[]` | `j` | `u64` |
| | `role` | `warmup` \| `measured` |
| | `shape` | text |
| | `input_digest` | `Hex64` |
| | `submit_ns`, `query_ns`, `readback_ns`, `check_ns` | `Obs<u64>` |
| | `return_code` | `i64` |
| | `checked_lanes`, `mismatch`, `nonfinite`, `work` | `u64` |
| | `output_digest` | `Obs<Hex64>` |
| | `retention` | `digest_only` \| `full_failure` \| `full_success` |
| `telemetry[]` | `metric`, `unit`, `device` | text |
| | `value` | `Obs<i64 or Ratio>` |
| | `slot` | `u64` |
| | `query_start_ns`, `query_end_ns`, `receipt_ns` | `Obs<u64>` |
| | `internal_age` | `unknown` \| `assessed` |
| | `vendor_code` | `Obs<text>` |
| `integrity` | `capacity_bytes` | `u64 > 0` |
| | `observed_bytes`, `failed`, `late`, `rejected`, `overflow`, `expected_slots`, `observed_slots`, `raw_records` | `u64` |
| | `writer_status` | `bounded` \| `unbounded` |
| `result` | `admission` | `admitted` \| `blocked` \| `not_qualified` |
| | `correctness` | `passed` \| `failed` \| `not_checked` |
| | `worker_exit`, `sampler_exit` | `Obs<i64>` |
| | `empty_job` | `observed` \| `not_observed` \| `unknown` |
| | `native_completion` | `unknown` |
| | `coverage` | `complete` \| `incomplete` \| `unknown` |
| | `limitations` | array of reason codes |
| | `verdict` | `blocked` \| `invalid` \| `inconclusive` \| `usable` |

Closed enum spellings. Any other spelling is a typed rejection. Two spellings
exist only to be rejected when used: `full_success` retention and `unbounded`
writer status.

| Enum | Values |
| --- | --- |
| `Provenance` | `live_observed`, `simulated_fixture` |
| `MatchState` | `match`, `mismatch`, `unknown` |
| `ControlWrites` | `none` |
| `EmptyJobLifecycle` | `observed_empty`, `observed_nonempty`, `unknown` |
| `Role` | `warmup`, `measured` |
| `Retention` | `digest_only`, `full_failure`, `full_success` (rejected) |
| `InternalAge` | `unknown`, `assessed` |
| `WriterStatus` | `bounded`, `unbounded` (rejected) |
| `Admission` | `admitted`, `blocked`, `not_qualified` |
| `Correctness` | `passed`, `failed`, `not_checked` |
| `ResultEmptyJob` | `observed`, `not_observed`, `unknown` |
| `NativeCompletion` | `unknown` |
| `Coverage` | `complete`, `incomplete`, `unknown` |
| `Verdict` | `blocked`, `invalid`, `inconclusive`, `usable` |

## Observation encoding

A field is either its known value or an object with the single exact key
`unavailable`:

```json
"boot":{"unavailable":"not_sampled"}
```

A reason code is ASCII `[a-z0-9_.-]{1,64}`. It is validated when constructed and
when decoded; an empty object, an extra key, an empty code, an uppercase code or
a code with a space is a typed rejection. Absence is never inferred from an
empty string, zero or `null`, and an unavailable digest is never a zero-filled
hex string.

Known values use the phase-1 canonical forms:

- unsigned and signed integers are quoted canonical decimal strings;
- ratios are the exact `{"num":"<i64>","den":"<u64>"}` pair with `den > 0` and
  are never reduced;
- a `Hex64` is exactly 64 lowercase hexadecimal characters (the phase-1
  `Digest` parser enforces the form, so `Hex64` is a type alias rather than a
  second parser);
- text is canonical JSON text with phase-1 escaping.

Text rules: known text must not be empty and must not contain any
`char::is_control` character. Raw non-ASCII is allowed for ordinary text
fields. `nonce` is additionally 1..=128 printable ASCII (`0x20..=0x7e`), and
`source_commit` is exactly 40 lowercase hex characters. Text comparison is
byte-for-byte; there is no normalization, case folding or trimming.

`value` in a telemetry entry is either an `i64` observation or a ratio
observation. `limitations` is an array of reason codes. The arrays
`dispatches` and `telemetry` keep caller order.

## Complete example

This is the exact regression fixture from `tests/envelope.rs`, with the pinned
digest appended last. It is an injected `live_observed` label, not real evidence.

```json
{"schema":"autotune.live-session","version":"1","provenance":"live_observed","identity":{"nonce":"nonce-1","nvml_uuid":"GPU-0000","cuda_uuid":"GPU-0000","pci_bus_id":"0000:01:00.0","context_device":"0","boot":{"unavailable":"not_sampled"},"uuid_match":"match","pci_match":"match"},"artifacts":{"source_commit":"0123456789abcdef0123456789abcdef01234567","worker_sha256":"0000000000000000000000000000000000000000000000000000000000000000","lockfile_sha256":"1111111111111111111111111111111111111111111111111111111111111111","sampler_sha256":{"unavailable":"not_built"},"ptx_sha256":"2222222222222222222222222222222222222222222222222222222222222222","kernel_entry":"fma_kernel","generator_version":"gen-1","oracle_version":"oracle-1"},"manifest":{"manifest_digest":"3333333333333333333333333333333333333333333333333333333333333333","profile_digest":"4444444444444444444444444444444444444444444444444444444444444444","shape":"1024x1","stop_policy":"pilot180s","approval_ref":{"unavailable":"no_approval"},"seed":"42","control_writes":"none"},"clocks":{"qpc_frequency":"10000000","qpc_epoch_ticks":"100","interrupt_ticks":"7","unbiased_ticks":{"unavailable":"unsupported"},"resolution_ns":"100","utc_context":"2026-10-26T00:00:00Z","discontinuities":"0"},"lifecycle":{"requested_ns":"1000","launch_ns":"1100","ready_ns":{"unavailable":"not_observed"},"start_ns":"1200","stop_requested_ns":"1300","exit_ns":"1400","empty_job":"observed_nonempty"},"dispatches":[{"j":"0","role":"warmup","shape":"8x8","input_digest":"5555555555555555555555555555555555555555555555555555555555555555","submit_ns":"1210","query_ns":{"unavailable":"not_sampled"},"readback_ns":"1220","check_ns":"1230","return_code":"0","checked_lanes":"16","mismatch":"0","nonfinite":"0","work":"100","output_digest":"7777777777777777777777777777777777777777777777777777777777777777","retention":"digest_only"},{"j":"1","role":"measured","shape":"8x8","input_digest":"8888888888888888888888888888888888888888888888888888888888888888","submit_ns":"1210","query_ns":{"unavailable":"not_sampled"},"readback_ns":"1220","check_ns":"1230","return_code":"0","checked_lanes":"32","mismatch":"0","nonfinite":"0","work":"200","output_digest":"6666666666666666666666666666666666666666666666666666666666666666","retention":"digest_only"}],"telemetry":[{"metric":"power","unit":"W","device":"GPU-0000","value":"200","slot":"0","query_start_ns":"2000","query_end_ns":"2100","receipt_ns":"2200","internal_age":"assessed","vendor_code":"0x0"},{"metric":"temp","unit":"C","device":"GPU-0000","value":{"num":"1","den":"2"},"slot":"1","query_start_ns":{"unavailable":"not_sampled"},"query_end_ns":{"unavailable":"not_sampled"},"receipt_ns":"2300","internal_age":"unknown","vendor_code":{"unavailable":"no_vendor_code"}}],"integrity":{"capacity_bytes":"4096","observed_bytes":"1024","failed":"0","late":"0","rejected":"0","overflow":"0","expected_slots":"2","observed_slots":"2","raw_records":"2","writer_status":"bounded"},"result":{"admission":"admitted","correctness":"passed","worker_exit":"0","sampler_exit":{"unavailable":"not_run"},"empty_job":"not_observed","native_completion":"unknown","coverage":"complete","limitations":[],"verdict":"usable"},"digest":"2ffe9f82352de2704cd1aa49ec98c7215327517e44bb068e1ffe22b8c6a5795c"}
```

## Digest construction

The session digest is

```text
SHA-256( "autotune.live-evidence.session.v1" || 0x00 || canonical(payload) )
```

where `payload` is the envelope object without `digest`. Encoding validates the
whole model, builds the 12-key payload, computes
`Digest::of_value(Domain::Session, &payload)`, appends `digest` and re-encodes.
Decoding requires `digest` to be the last top-level key, recomputes the digest
over the remaining object and compares before parsing any section. A section
edit without resealing therefore fails on `digest`, not on the section. A
mismatch is a typed rejection and no value escapes. The digest binds bytes, not
truth, consent or authority.

## Validation rules

Encoding validates fields and cross-field rules before writing a byte. Decoding
applies the same validation after the digest check. The following describes the
required conditions after the 2026-10-28 owner resolution; violations reject.

- `observed_bytes <= capacity_bytes`; `observed_slots <= expected_slots`.
- `qpc_frequency > 0`, `resolution_ns > 0`, `capacity_bytes > 0`.
- dispatch `j` starts at `0` and is strictly increasing.
- dispatch `mismatch <= checked_lanes`; `nonfinite <= checked_lanes`.
- known lifecycle values are non-decreasing:
  `requested_ns <= launch_ns <= ready_ns <= start_ns <= stop_requested_ns <=
  exit_ns`.
- per dispatch, known `submit_ns <= query_ns <= readback_ns <= check_ns`; a
  known `submit_ns` is not before a known `start_ns`. In-flight checks after
  `stop_requested_ns` and supplemental checks after `exit_ns` remain retainable.
  `submit_ns` is not admission time, so there is no submit-after-stop rejection.
- per telemetry entry, known `query_start_ns <= query_end_ns <= receipt_ns`;
  `internal_age == assessed` requires all three to be known.
- `retention == digest_only` requires a known `output_digest`;
  `retention == full_failure` requires `mismatch > 0` or `nonfinite > 0`;
  `retention == full_success` is always rejected, on encode and on decode.
- telemetry `(metric, slot)` pairs are unique; their count equals
  `observed_slots`. Several metrics may share a numeric slot and each pair
  counts independently. `expected_slots` is an aggregate expected pair count,
  not a per-metric slot-index bound. Without a declared expected pair set, the
  codec cannot infer per-metric ranges or identify which expected pairs are missing.
- `writer_status == unbounded` is always rejected, on encode and on decode.
- `identity.uuid_match == match` requires known `nvml_uuid` and `cuda_uuid`;
  `pci_match == match` requires a known `pci_bus_id`.
- `lifecycle.empty_job` agrees with `result.empty_job`: `observed_empty` with
  `observed`, `observed_nonempty` with `not_observed`, `unknown` with `unknown`.
- `verdict == usable` requires `admission == admitted`,
  `correctness == passed`, `coverage == complete`, empty `limitations`,
  `worker_exit == 0`, a known-zero or unavailable `sampler_exit`, matching UUID
  and PCI identity, zero `failed`/`late`/`rejected`/`overflow` integrity
  counters, `observed_slots == expected_slots`, and at least one `measured`
  dispatch where every measured dispatch has `retention == digest_only`, a known
  `output_digest`, `return_code == 0`, `mismatch == 0`, `nonfinite == 0` and
  `checked_lanes > 0`.
- `AcceptPolicy::RequireLive` rejects `simulated_fixture` after digest
  verification. `AcceptPolicy::AllowSimulated` accepts either provenance without
  promoting it; a decoded `simulated_fixture` stays `simulated_fixture`.

Unknown, missing, extra and reordered keys are rejected by the exact ordered
field lists. The phase-1 reader rejects duplicate keys, non-canonical bytes,
numbers at a value position and over-deep or oversized documents before field
mapping begins.

The integration contract requires admission closure at stop while permitting
finite in-flight completion/readback, the admission/submission race and
supplemental late evidence. A stop request is not a completion cutoff. This
codec validates one snapshot, not terminal-history immutability: it does not
observe an earlier outcome or enforce append-only updates. Producers remain
responsible for retaining terminal invalid/inconclusive outcomes without
upgrading them when late evidence arrives. The late-check regression fixtures
retain those explicit outcomes; they do not prove temporal-history enforcement.

## API

Portable module `autotune_benchmark_worker::evidence::envelope`, re-exported at
`evidence::*`:

- `Envelope`, `Provenance`, `AcceptPolicy`, `Obs<T>`, `Reason`, `Hex64` and the
  section structs/enums in the tables above.
- `SCHEMA` and `VERSION` public constants.
- `encode_envelope(&Envelope) -> Result<String, EnvelopeError>`.
- `decode_envelope(&[u8], max_bytes, AcceptPolicy) -> Result<Envelope, EnvelopeError>`.
- `EnvelopeError` with `field() -> &'static str` and
  `kind() -> EnvelopeErrorKind`. Both accessors, the error and its kind are
  `Copy`/`Clone`/`Debug`/`PartialEq`/`Eq`. There is intentionally no `Display`
   or `std::error::Error` implementation in this slice.
- `Reason::new(&str) -> Result<Reason, ReasonError>` with `as_str()`;
  `ReasonError` is `Empty`, `TooLong` or `Character`.
- `Obs<T>` is `Known(T)` or `Unavailable(Reason)`, with `known`,
   `unavailable`, `as_known` and `is_known` helpers.

`field()` returns a static expected schema field name (or `envelope`), not an
arbitrary offending input key or indexed path. A shifted/unknown key reports the
expected key at that position; a trailing extra key reports the last expected
key. Errors inside arrays do not identify the array index. Phase-1 parse errors
are mapped to `envelope`/`Parse` without preserving parser detail.

`EnvelopeErrorKind` values: `Parse`, `Root`, `NotObject`, `NotArray`, `NotText`,
`FieldCount`, `FieldName`, `FieldOrder`, `ExtraField`, `DuplicateField`,
`Schema`, `Version`, `UnknownVariant`, `TextEmpty`, `TextControl`, `TextNonAscii`,
`TextLength`, `Integer`, `Ratio`, `Hex`, `Reason`, `DigestMismatch`,
`CrossField`, `Range`, `RetentionFullSuccess`, `WriterUnbounded`, `Provenance`.

No dependency is added and no CI definition changes: the phase-1 `sha2`
dependency and writer/reader are reused.

## Versioning

The `autotune.live-session` v1 owner-resolved fixes passed focused verification
and independent static review on 2026-10-28. Any later change to field names,
order, types, enum spellings, reason grammar,
digest construction or validation is a new schema version, not a v1 edit. A v1
reader must reject re-spelled v1 bytes and must not accept a different
`schema`/`version` pair. Adding an optional field is also a version change,
because v1 requires exactly 13 top-level keys and exact section key lists.

## Local verification

2026-10-28 closeout: the parent reproduced both newly added public
regressions with exit 101 before the production fix. The cutoff and aggregate
counting fixes, corrected historical expectations, encode/decode rejection cases
for undercounted and duplicate pairs, sparse slot IDs and racing submission
coverage are now patched. The original fixture input and pinned digest are
unchanged. Parent final verification passed all 26 envelope tests (none ignored
or filtered), warning-denied library/envelope Clippy and package formatting,
using the three focused commands below. Monitor `mon_B4SZGYXVD6K1JEP8`
reported `FINAL_EXIT=0`. Earlier formatting check caught one test-call line
wrap; it was corrected before this final run. No checks or temporary resources
were created by the implementation child.

Source binding: HEAD `3655e6a8800c11f7bf3d5d78dc63495619b401b5` plus the
uncommitted envelope changes. SHA-256:

| File | SHA-256 |
| --- | --- |
| `src/evidence/envelope/codec.rs` | `6f7b6f28156ab3f297f0aca45c2e2fae13c8af8667feb1645b2d0a9203be69d6` |
| `src/evidence/envelope/model.rs` | `7fcdbb389de74ed434dde8d6aa450d4c99530149e7c4d1d1436f2cec6b16d8c0` |
| `tests/envelope.rs` | `155d535ca4a28cadcd61f8685693573fc53fbb5e4ca20263d08d3892f8cd0fa0` |

Independent reviewer `st_01a183c1` (ChatGPT Subscription GPT-6.1 Sol) found no
code blockers and approved the bounded static review; it did not run commands
or independently derive the fixture digest. The parent ran the public codec
scenarios. This closes the two envelope defects, not E3, live integration or
any product release gate. No broad suite, process fixture or hardware ran.
The historical results below remain separate.

Historical focused checks: 2026-10-26. Windows GNU, installed Rust 1.99.1 (`+stable`), existing
LLVM-MinGW, `--locked --offline`, fresh target
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-envelope-check-target`.

```text
cargo +stable test -p autotune-benchmark-worker --test envelope --locked --offline
cargo +stable clippy -p autotune-benchmark-worker --lib --test envelope --locked --offline -- -D warnings
```

Both pass: 24 envelope tests, no ignored tests; warning-denied Clippy is clean
for the library and the `envelope` target. `cargo +stable fmt -p
autotune-benchmark-worker -- --check` also passes. No workspace-wide test, other
suite, `--ignored` run, process fixture, native load, GPU/probe execution, OS
clock read, installation or system change occurred. No hosted run and no
hardware claim.

The fixture's pinned session digest is
`2ffe9f82352de2704cd1aa49ec98c7215327517e44bb068e1ffe22b8c6a5795c` over the
fixture payload without `digest`. It was produced once by this encoder, then
hard-coded in `tests/envelope.rs`; it is a regression constant, not an
independently derived vector.

Historical source hashes bound to the retained parent run (unchanged on
2026-10-27; the codec and tests have since changed):

| File | SHA-256 |
| --- | --- |
| `src/evidence/envelope/model.rs` | `7fcdbb389de74ed434dde8d6aa450d4c99530149e7c4d1d1436f2cec6b16d8c0` |
| `src/evidence/envelope/codec.rs` | `c3e157255892e07e1a24c252943b6cc183cc4b1a032a2b2330188998c6c63894` |
| `src/evidence/envelope/mod.rs` | `1e856087b012fbcd2c1f265ebdb2043033f34cc7ce84c959da358b17aadbbcae` |
| `src/evidence/mod.rs` | `4d0c8499c3a062cc9c743f96af1a7d257abaed9dab0a28c654681e242dcd9098` |
| `tests/envelope.rs` | `1b2f1a2ea6aeb1750af5e9d0d063f08354e7d7f22e242e160f7edb3ce40b1da5` |

The post-patch parent verification (workspace fmt, warning-denied workspace
all-target Clippy/linking, 280 selected CPU-only tests including all 24
`envelope` tests, feature Clippy/linking and four feature-library tests) is bound
to a retained source manifest: 117 repository files are byte-identical before and
after, and all 11 commands exited 0. Manifest
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-envelope-parent-final-20261026-manifest.json`,
SHA256 `2e9d96bee22e883a3222b363fa81cf1f6bf4734a6c819312fd899dc436788809`.
Its exact commands and counts are recorded in
[docs/ci.md](ci.md#live-evidence-session-envelope-phase-2--2026-10-26) and
[ROADMAP.md](../ROADMAP.md).

Evidence correction and closeout audit:
`C:\Users\manni\AppData\Local\Temp\opencode\autotune-envelope-closeout-20261027-companion.json`.
SHA256: `333b77f2ecdef234377d6e3a411c35505dd16c388e263a3491c0aaa35618d034`.
Read it together with the unchanged original manifest: the original's claim that
reviewers recomputed the current fixture digest is unsupported (their reviews
saw an older fixture). The current digest remains encoder-produced only. The
first parent attempt stopped because PowerShell treated Cargo stderr as an
error; the successful rerun reused partial compilation in that initially new
target. This is not an entirely pristine first-attempt run. Those historical
passing tests did not resolve the semantic findings. No Rust changes or tests were run during
the 2026-10-27 documentation/contract audit.

## Limits and non-authority

A decoded envelope is a validated byte structure. It is **not** live evidence,
not a consent or approval channel, not admission, not a qualification result,
not a baseline or tuning verdict, and not release-gate closure. `live_observed`
is an injected label; the codec cannot verify that a producer was live, that a
consent record existed or that the named device was used. A `usable` verdict only
means the listed structural conditions were satisfied for injected values. No
clock source, process or workload launch, CUDA/NVML/PTX call, energy
measurement, thermal attribution, optimizer or hardware qualification is
implemented or implied. Phase-1 canonical bytes, error variants and digest
construction are unchanged, and a v1 reader must not accept re-spelled v1 bytes.
