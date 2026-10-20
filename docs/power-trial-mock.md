# Guarded power-limit trial: simulation evidence

This milestone implements a portable, deterministic Rust safety model only.
**It is not hardware qualification or authorization.** It never loads NVML,
binds a native setter, elevates, launches a command/workload or implements boot
integration. `autotune-probe` remains separate and read-only, including its
schema, fixtures and `write_verification: not_tested` fields. Bare invocation
of the **probe** is still live; do not confuse it with the new simulation CLI.

## Usage and output

Use the README's session-only linker environment if needed. No installation or
persistent environment changes are required.

```powershell
cargo run -p autotune-power-trial -- --help
cargo run -p autotune-power-trial -- --mock
cargo run -p autotune-power-trial -- --mock baseline-230000
cargo run -p autotune-power-trial -- --mock error-after-change
cargo run -p autotune-power-trial -- --mock late-completion
cargo run -p autotune-power-trial -- --mock storage-close
```

`--help` lists all fixtures. Bare invocation only shows help. `--mock` defaults
to `success`. JSON schema 1 includes the full immutable synthetic manifest,
SHA-256 of its canonical struct serialization (field order is defined by the
Rust struct), independent candidate API/readback, restore API history, final
verification reads, observed restoration, evidence durability, typed failures,
exact call-entry/return trace, and the model journal. All provenance is
`simulated`. No approval secret or real device identity is used. Reports and
journals are never imported as execution authority.

Exit codes: **0** successful simulated round trip (or help), **2** modeled
rejection/failure/unresolved recovery, **64** invalid options/scenarios,
including any live/recovery option. Identical fixtures produce identical bytes,
traces and exit codes. A failed API call remains a failure even if baseline
restoration is subsequently observed.

## Model boundaries

- `manifest.rs`: typed unavailable readings, full UUID/PCI/vendor/device/subsystem
  identity, environment/binary/operator/risk linkage, exact two synthetic capture
  observations 500 ms apart, checked bounds and 1000–5000 mW downward delta.
  The entire manifest is digest-bound; the engine owns an immutable clone.
  Consent is **simulated exact interaction**, not authentication. Nonces are
  fixed fixture identifiers, not unpredictable production secrets. Approval
  freshness ends at 60 s or UTC expiry, whichever is earlier. Any modeled UTC
  discontinuity invalidates it; no token renewal or silent rebase exists.
- `engine.rs`: serialized event processing and injected fake monotonic/UTC time.
  It models coordinator, independent guardian and worker roles, **not OS
  processes**. Trusted harness events model authenticated IPC; the
  `submit_candidate` ingress checks session digest and generation, and the
  typed candidate event independently checks the manifest digest and carries no
  arbitrary setter value. This is
  not a demonstration of IPC authentication, process identity or ACLs.
  Enumeration resolves exact UUID/PCI rather than index; all entries recheck
  identity, bounds, runtime and modeled ownership. A per-device owned flag is
  a lock abstraction, not a real/global lock. External tools are not excluded.
- All protocol timers are immutable: 10 s lease, first 2 s admission, 1000 ms
  watchdog, 250 ms heartbeat/1000 ms loss, normal recovery requested by arm+3 s,
  two verification reads 250 ms apart, 5 s recovery observation budget.
  Timer decisions win completion/admission ties. There is one candidate and
  at most two serialized baseline restores, with no dwell/workload.
- A pending native-model operation remains pending after cancellation, timeout
  or guardian death. A late return can establish quiescence within the original
  recovery budget; otherwise recovery stays unresolved. No restore races a
  blocked call. Late completion after the budget is recorded without claiming
  restored success. There is **no proven native/driver fencing mechanism**.
- Restoration always explicitly writes freshly captured `B`, even when a read
  already reports `B`. Fixtures independently restore 240000 and 230000 mW,
  never the contextual 215000 default. Restore status and exact readbacks are
  independent. A second restore requires completed unsuccessful verification,
  unchanged identity/bounds/runtime/ownership and remaining budget.
- `journal.rs`: memory-only ordered, checksummed, manifest-bound records with
  nonce consumption before candidate admission, intent before native entry,
  retained prior valid records and injected publication faults (before, partial
  write, flush, replacement, readback and after). Before/PartialWrite/Flush/
  Replace/Readback are labels for the same conservative implementation: retain
  the prior records, fail publication and mark storage failed. They do not
  create distinct partial filesystem artifacts or simulate filesystem ordering.
  After retains the published record but still reports storage failure.
  All physical durability gates remain closed. `intent:Restore` is the
  `restore_pending` publication. After possible mutation, storage faults cannot
  suppress safe cleanup, but they prevent durable success and another trial.
  An aborted pre-entry attempt can therefore leave an unresolved journal.
  The model store is trusted memory; checksums do not authenticate an authorized
  editor. Invalid/missing/redirection/ACL failure is represented by rejection or
  the journal's invalid flag, **not tested filesystem security**.
- Restart reconciliation only records supplied synthetic enumeration, bounds
  and current reading, identity/bounds matches and saved-baseline validity. It
  executes zero setters and cannot clear an unresolved journal. Even a current
  reading equal to `B` requires explicit human reconciliation and is not proof
  that an earlier restore API succeeded. No recovery-only setter is implemented;
  all such actions are rejected rather than borrowing old consent.

## Acceptance mapping

Tests exercise the actual engine and journal, not fabricated final reports.
`tests/safety.rs` asserts call serialization, exact identity/handle/value,
intent-before-entry, single candidate/two restores, revoked admission, two
separated reads before observed restoration, and unresolved exclusion.
`tests/cli.rs` executes every fixture twice and compares complete JSON/status.

| Matrix | Automated model evidence (`tests/safety.rs`, unless noted) | Remaining native/Windows gate |
|---|---|---|
| M01 | `m01_missing_generic_wrong_operator_replay_and_expiry` | Operator authentication, unpredictable nonce |
| M02 | `m02_every_serialized_manifest_field_is_bound_to_consent` | Trusted binary/contract measurement |
| M03 | `m03_capture_boundaries_and_clock_jumps_never_extend_consent` | Real monotonic clock, suspend/resume detection |
| M04–M05 | `m04_m05_exact_identity_resolution_not_index_or_name` | Native enumeration/handle identity semantics |
| M06 | `m06_capture_and_pre_entry_drift_reject_without_rebase` | Fresh getter/identity/bounds capture |
| M07–M08 | `m07_m08_fresh_baselines_and_optional_default` | Real getter/sentinel interpretation |
| M08–M09 | `m08_m09_checked_bounds_and_reductions` | Returned bounds are not qualified safety limits |
| M10, M20–M22 | `m10_m20_m21_m22_distinct_candidate_api_and_exact_readback` | Setter availability/permission, real configured-limit semantics |
| M11, M13 | `m11_m13_every_publication_fault_preserves_cleanup_and_exclusion` | Actual disk flush/atomic replacement/readback ordering, ACLs |
| M12, M14, M29–M30 | `m12_m14_m29_m30_restart_is_read_only_never_authority` | Crash/reboot identity, protected directory, manual reconciliation; no recovery-only execution implemented |
| M15, M19 | `m15_m19_cancel_parent_ipc_and_clock_interleavings_at_every_phase` | Independent guardian survival after console/job/parent teardown |
| M16 | `m16_fixed_lease_heartbeat_and_closed_generation` | Authenticated private IPC, process creation identity |
| M17–M18 | `m17_m18_unknown_native_completion_never_races_a_restore` | Guardian failure and native in-flight quiescence remain unsolved |
| M23–M26 | `m23_m24_m25_m26_restoration_is_exact_bounded_and_independent_of_api_success` | Physical restoration and trustworthy readbacks |
| M27–M28 | `m27_m28_no_blind_restore_after_identity_runtime_bounds_or_owner_change` | Actual protected ownership and stopped external tools |
| M31 | Existing unchanged probe unit/integration mutation guards; `m31_m32_mock_source_has_no_live_dependency_or_execution_path`; separate dependency trees | Continue preserving probe allowlist/schema |
| M32 | `tests/cli.rs::bare_and_help_are_nonmutating_and_live_options_are_rejected` | Live runtime/unsupported platform path is deliberately absent, not qualified |

Additional generated coverage: all six fault phases at all 14 normal-path
publications; each native-model operation with immediate/delayed/blocked/error
outcomes; cancellation/parent/IPC/clock events at every millisecond across the
normal operation and verification interval. This is bounded exploration, not a
formal proof of all interleavings. Corrupt records and unsupported schema are
rejected; trusted-memory flags stand in for missing/protected-path faults.
Publication faults are also crossed with cancellation at operation boundaries;
repeated cleanup triggers are tested not to restart successful verification or
consume a second restore attempt. Crash snapshots of every journal prefix are
reconciled read-only.

## Verification and live blockers

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo tree -p autotune-power-trial --locked
cargo tree -p autotune-probe --locked
cargo run -p autotune-power-trial -- --mock
```

The mock graph contains serde/JSON/SHA-256, no probe, loader or native bindings.
The probe keeps its own existing loader/getter graph. Test executables launch
only the new mock CLI; production simulation code launches no subprocesses.

Verified on 2026-10-17 with the README's Rust/GNU linker environment: formatting,
warnings-as-errors Clippy, locked workspace tests (32 new trial tests and all 23
existing probe tests), both dependency trees and default mock smoke passed.
All 39 CLI scenarios were compared twice. Preserved SHA-256 baseline hashes
matched all 9 probe files and 2 existing documentation files.

Review regressions additionally cover repeated/terminal guardian death, immutable
cleanup deadlines, incomplete-prefix and historical nonce corruption, symbol
loss at later call entries, immediate cleanup after candidate/storage failure,
recursive manifest-leaf consent mutations, multi-GPU reconciliation, invalid
low-baseline fixtures and independent early UTC approval expiry. All three BMAD
review layers completed; accepted findings were fixed and verified.

**All live gates remain closed:** reviewed hardware implementation, Windows
fake-worker process-survival/IPC/ACL/ownership tests, actual crash/durability
tests, assessed blocked-driver-call fencing, real privilege and getter/setter
semantics, stopped competing tools, approved device-specific manual recovery,
fresh read-only capture and full exact single-use hardware approval. No claim
is made about crash/power-loss survival, safe hardware limits or stability.
