# Fake-only recovery — storage/process/IPC slices, full acceptance blocked

No hardware capability, NVML dependency, native device adapter, arbitrary backend,
elevation, workload, reset, automatic replay or production-root provisioning was
added. The original probe and power-trial crates remain byte-identical. Their
55 tests are regressions, not evidence of independent Windows recovery.

## Implemented boundary

- `autotune-recovery-contract`: fixed-field canonical JSON, strict decoding,
  domain-separated SHA-256, portable HMAC vectors, bounded length-prefixed MACed
  frames, per-direction sequence checking, synthetic manifest/approval/value
  policy, permission/deadline types, and the frozen storage v1 envelope, preparation,
  consent/arm/recovery, append-only call/notice, nonce and terminal history contract.
- `autotune-recovery-core`: pure one-lane policy with distinct readiness stages,
  explicit getter chain, two actual capture samples, capture-trace binding,
  publication-before-dispatch, original-permit return/idle retirement,
  dispatch-origin watchdogs, frozen cleanup budget, bounded restore count,
  separated baseline verification, raw call ledger, RAM cleanup on storage
  timeout, original-anchor clock policy and independent API/readback/publication
  evidence. Inputs here are modeled observations, not OS attestations.
- `autotune-recovery-windows`: CNG/clock/isolation diagnostics plus actual protected
   test-root opening, ownership/custody, real staged publication, conservative scanner
   and guardian-owned storage-thread port. These APIs linked and ran on the existing
   GNU toolchain. `require_arm_support()` still rejects; storage is not arm authority.
- `autotune-recovery-windows::{process,ipc}`: measured fixed role images, suspended
  launch/explicit inherited handle lists, restricted custody, named-pipe peer checks,
  CNG mutual authentication and poisoned one-use channels. The independent worker
  policy enforces preparation, finite call order, receipts, deadlines and single use.
- `autotune-recovery-harness`: help/platform diagnostics plus fixed verifier,
  guardian, coordinator and worker roles. `--fake-roundtrip` runs the complete
  preparation/capture/seal/approve/arm/candidate/restore/verify path through the
   real guardian policy, Windows storage and authenticated worker; the harness-owned
   witness retains state across worker death and selected entry barriers. Internal roles
  require a validated inherited bootstrap, not a user-provided executable/key.

The [isolated storage/journal v1 contract](windows-storage-v1.md) is frozen and
implemented, including preparation before manifest. Process/bootstrap types and
handshake golden vectors now exist, and a fake-only guardian drives the reducer
through the real process/IPC/storage ports for normal/error/missing-candidate
and selected cancellation/storage-failure paths. Complete field/security review,
bounded actor/control integration, parser exploration, exhaustive barriers
and the complete asynchronous fault/interleaving matrix remain
unfinished. No production adapter or hardware-armed guardian exists.
Cold snapshot history is always read-only, including apparently clean terminal
records: a late timed-out publication cannot unblock another trial. Two clean
sessions can roll over only within the original live ownership lifetime.

## Verification and proof tiers

Host baseline: Windows 11 Pro, OS version 10.0.26200/build 26200, Rust/Cargo 1.99.1 GNU,
LLVM-MinGW 20261006 / LLD 23.1.3; `windows-sys = 0.61.2`.
Use the README session-only environment; no persistent changes or installations.

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo run -p autotune-recovery-harness -- --platform-check
cargo run -p autotune-recovery-harness -- --fake-roundtrip normal
```

### 2026-10-24 selected blocked-call control integration

The worker I/O lane now services authenticated Revoke/StorageFailed while the
external fake oracle is blocked off-thread. A bounded two-message control queue
and atomic oracle-active gate preserve ordering: accepted controls and their
zero-operation acknowledgements precede FakeResult; only the same worker's exact
ReturnAndIdle retires the original permit. No successor device operation or
replacement worker is admitted by a control acknowledgement. The blocked fixture
releases its late effect only after those actual worker acknowledgements, counted
separately as `in_call_controls_acknowledged` in the report.

| P1 case / matrix subcase | Actual direct test / observation |
| --- | --- |
| Watchdog while candidate blocked; W12/W20/W21 | `blocked_candidate_watchdog_keeps_lane_pending_until_same_worker_returns`: one in-call Revoke ack, original completion before restore intent, immutable cleanup deadline |
| Authenticated heartbeat source ceases; W12/W20 | `blocked_candidate_real_heartbeat_loss_revokes_before_return_without_rebasing_cleanup`: only owned coordinator suspended before validation; serviced 300 ms drain window orders heartbeat loss before watchdog; Heartbeat notice retained, one in-call ack |
| Publisher fails while candidate blocked; W08/W21/W23 | `blocked_candidate_storage_failure_notifies_in_call_then_serializes_ram_cleanup`: actual Closed failure, two in-call acks, exact B restored in RAM, no publication success or cold completion invention |
| Publisher stalls while candidate blocked; W08/W21/W23 | `blocked_candidate_storage_stall_quarantines_late_publication_and_uses_ram_cleanup`: unchanged 100 ms ceiling expires; only notification/return barrier releases the sole publisher late; quarantine and cold unresolved exclusion persist |

The real-pipe unit test
`authenticated_in_call_controls_do_not_retire_dispatch_or_admit_a_successor`
also runs in the ordinary workspace suite. The three new fixed CLI cases are
`heartbeat-loss-during-blocked-candidate`,
`storage-failure-during-blocked-candidate` and
`storage-stall-during-blocked-candidate`.

Build the round-trip test with `cargo test -p autotune-recovery-harness --test
roundtrip --no-run --locked`, then directly execute its printed executable outside
Cargo's restrictive job with `blocked_candidate --ignored --test-threads=1`.
This does not select the separate opt-in console test or create a visible console.

Final verification: formatting check, strict locked workspace Clippy, locked
workspace **137 passing / 14 ignored**; all **13/13** direct serial round trips;
three diagnostic-enabled repeats of the four blocked tests **12/12**.
Quiet direct transport/lifecycle tests also passed **5/5**, with console closure
left ignored and never selected. Original
crate sources and Cargo.toml/Cargo.lock have no diff against HEAD. README and the
existing .gitignore were not edited in this continuation.

Failures retained, not waived: an earlier 9/10 full direct run rejected a worker
receipt with Contract(Binding) in missing-restore; an intermediate blocked suite
passed 3/4 with a dispatch-authorization failure before report generation. Another
3/4 run exposed the initial heartbeat fixture racing its last buffered heartbeat
against watchdog; suspending before validation plus the serviced drain window
corrected fixture ordering without weakening its Heartbeat-notice assertion.
Historical and current end-to-end timing/reliability remain unqualified despite
the final passing runs. Logs, hashes and failure notes:
`C:\Users\manni\AppData\Local\Temp\opencode\recovery-async-control-ed81c291`.

This is selected P1 evidence only. Blocked-call coordinator cancellation/IPC loss,
all phase/lease/budget/publication boundaries, independent alerts, production-root
and cross-user qualification, console closure and live/cold validation review
remain incomplete. The harness task stays unchecked. G1–G7 and P2/P3 remain open;
no physical-restoration, hardware readiness or power-loss claim follows.

### 2026-10-20 full fake round trip (direct-run evidence)

`--fake-roundtrip normal|error-after-change|missing-candidate` runs real fixed
guardian/worker/coordinator processes, authenticated pipes, the reducer/journal
adapter and the protected Windows store with an injected synthetic device. All
three completed successfully when the test executable was run outside Cargo's
breakaway-restricted job:

- normal: baseline 247123 mW, target 242123 mW, candidate API 0, exact readback
  242123, one restore API 0, two baseline reads 247123 separated by 414 ms,
  observed restoration and durable publication, cold scan read-only.
- error-after-change: candidate API -7 retained, candidate readback skipped as
  designed, one restore, both baseline reads exact, status not promoted to success.
- missing-candidate: zero candidate entries, no candidate API/readback, conservative
  restore still executed and verified, cold scan read-only.

Journal sequence was 45–47 per run; the final stored state was `RestoredVerified`,
with the manifest's two captured readings equal to the fresh synthetic baseline and
default 215000 retained only as context. Process identities were distinct and the
image hashes matched. The fixture root is retained. No hardware was accessed.

Cargo places test processes in a job that forbids guardian breakaway. The normal
suite therefore verifies that rejection explicitly (`Win32(5)`, no device report),
and the three full round-trip tests are `#[ignore]`d for direct execution. This is
an unsupported-host blocker, not a survival pass.

`--fake-roundtrip` exposes no hardware or arbitrary-backend mode; it is fake-only.

Known integration gaps (not accepted): the full
heartbeat/lease/cancel/storage-stall interleaving matrix remains open.
Selected witness/worker-kill and blocked-call cases are recorded below; console
closure remains undelivered on this host.

### 2026-10-23 selected cancellation/storage-failure integration

Two additional fixed `--fake-roundtrip` cases exercise actual guardian/worker
processes and the existing authenticated IPC/reducer/storage ports:

- `cancel-after-candidate`: cancellation after candidate return closes admission,
  delivers the guardian's frozen `Revoke` epoch, skips optional readback and
  explicitly restores/verifies the captured baseline. The cancel notice remains
  in the terminal journal.
- `storage-failure-after-candidate`: cancellation triggers a real publication;
  the sole storage executor fails at `PublicationStep::Closed`, after replacement
  and readback but before its acknowledgement. The guardian sends `Revoke` then
  `StorageFailed`, uses exact ordered RAM-only result receipts and performs one
  explicit baseline restore plus two separated reads. It refuses terminal closure,
  reports `RecoveryRequired`/`publication_verified: false` and exits 2. Cold opening
  remains reconciliation-only with no restore calls invented in the disk ledger.

The worker's reported recovery window matches the journal's frozen epoch in both
tests. This is not evidence for an indefinitely stalled storage thread, worker
loss, late effects, all fault barriers, physical durability or native quiescence.
The five direct-run round-trip tests passed outside Cargo's restrictive job;
the regular suite passes 129 tests with those five explicitly ignored. Formatting
and warnings-as-errors Clippy passed. Full infrastructure acceptance stays blocked.

### 2026-10-23 external witness and entry barriers

Actual worker roles now use the guardian-owned harness witness for every fake
getter/setter, through the same authenticated pipe. Worker-local device state is
no longer the real-process oracle. Witness entry/effect evidence is separate from
worker return/idle and publication; reports retain exact permits, synthetic device,
write arguments, timestamps, results and whether worker loss preceded the effect.
The witness survives worker loss, **not guardian loss**. No native fencing follows.

| W/M subcase | Actual test / observation | Tier / disposition |
| --- | --- | --- |
| W12/W20, M18/M19 | `worker_killed_at_candidate_entry_retains_late_effect_and_pending_without_restore`: owned worker killed and observed exited at the entry barrier; witness then applies T. Original candidate stays pending, no API completion/readback/restore/terminal success; cold startup remains read-only with unresolved intent and consumed nonce | P1 selected pass; remaining kill/disconnect/expiry edges blocked |
| W12/W21, M18 | `blocked_candidate_watchdog_keeps_lane_pending_until_same_worker_returns`: oracle effect blocks until guardian watchdog fault releases its barrier. Actor keeps sampling/reducing; no successor before same-worker exact completion; then explicit B restore and separated reads | P1 selected pass; not a general bounded actor-loop claim or in-call Revoke delivery |
| W14, M07/M23 | `unrelated_fresh_baseline_is_restored_by_external_witness_not_default_or_history`: fresh B=231457, T=226457, explicit restore to B, two separated reads; existing B=247123 regressions also use witness | P1 selected pass; full drift/restore-failure matrix remains open |
| Oracle misuse | `witness_preserves_unfinished_effect_rejects_overlap_and_never_releases_twice`, `witness_rejects_wrong_identity_permit_replay_and_raw_write_without_manifest` | P0 pass, no OS/native conclusion |

All eight ignored round-trip tests were executed directly outside Cargo's
restrictive job and passed. They remain ignored in the ordinary Cargo suite;
that suite separately checks fail-closed guardian launch rejection. Reproduce by
compiling `cargo test -p autotune-recovery-harness --test roundtrip --locked --no-run`
then invoking Cargo's exact printed test executable with
`--ignored --nocapture --test-threads=1`. Fixed CLI cases are
`--fake-roundtrip worker-killed-during-candidate` (exit 2, recovery required),
`--fake-roundtrip blocked-candidate-returns` and
`--fake-roundtrip alternate-baseline` (synthetic restoration only).

Verification: formatting and warnings-as-errors workspace Clippy passed;
locked workspace tests passed **131 tests, 8 ignored**, including all original 55.
All 18 original-crate files match the pre-continuation backup by SHA-256;
workspace manifests/lockfile are unchanged. Direct suites passed repeatedly, but
one repeat failed three cases with `Win32(109)` (broken pipe); two subsequent
diagnostic-enabled eight-case runs passed. The intermittent failure is unresolved,
not waived or counted as acceptance. Guardian round-trip failures now send their
typed rejection to the launcher before exit to aid diagnosis; no guard was relaxed.

The full W01–W24 matrix remains **unaccepted**: console closure, production-root/
cross-user qualification, exhaustive kill/return/storage/cancel interleavings,
in-call notification delivery and alert integration remain incomplete. Existing
live/cold publication validation asymmetry still needs review. No P2/P3 work or
G4–G7 qualification was performed; physical restoration is not claimed.

### 2026-10-24 Win32(109) investigation — reliability still blocked

Two concrete defects were identified, without relaxing any guard:

- A guardian error before readiness dropped the authenticated channel without
  reporting its cause. A deterministic competing-owner fixture reproduced the
  same `Context("guardian readiness receive", Win32(109))` symptom. The role now
  attempts `Rejected` for guardian errors both before and after readiness; the
  launcher retains the original reason rather than replacing it with a generic
  policy string. The startup regression now observes the exact lock rejection,
  `Storage(Nt(-1073741757))` (`STATUS_SHARING_VIOLATION`). A missing-restore test
  separately retains the original worker entry guard failure and exit 2.
- Optional diagnostics synchronously opened/appended files inside timed actor
  work. Concurrent diagnostic runs recorded real `Storage` failures and refused
  dispatch; they were not merely broken-pipe teardown races. Diagnostics now use
  bounded RAM capture and post-role buffered output. A unit regression checks no
  file exists before explicit flush, and that overflow is reported. Journal
  publication, expiry, ownership, IPC and worker assertions are unchanged.

Two focused real-pipe tests retain complete MACed frames after the writing handle
closes, for both client and server writers. They did **not** reproduce the proposed
buffered-data-loss explanation; they do not prove peer-process/native fencing.
The historical three failures cannot be conclusively attributed to one root cause
because their original guardian diagnostics were absent. No transport race is
declared fixed on speculation. A worker receipt rejection and startup deadlines
also occurred during intermediate diagnostic runs and remain retained evidence.

Exact completed repetition results (no test retries or changed expectations):

| Build / run | Result |
| --- | --- |
| Initial eight-case suite, six serial runs | 48 passed / 0 failed |
| Initial eight-case suite, one two-thread run | 8 passed / 0 failed |
| Typed-rejection build, three ten-case serial runs | 30 passed / 0 failed |
| Typed-rejection build, two ten-case two-thread runs | 10/10 passed; then 6 passed / 4 failed |
| Extra synchronous diagnostic instrumentation, one two-thread run | 1 passed / 9 failed; readiness deadlines and publication refusal explicitly observed |
| Buffered-diagnostic build, two serial runs and one two-thread run | 30 passed / 0 failed |
| Final rebuilt binary, serial then two-thread run | 10/10 passed; 10/10 passed (diagnostics enabled for the latter) |

An interrupted buffered run has no completed suite summary and is not counted as
a pass. Its `buffered.5072.log` still records a genuine publication timeout:
request at 8516747 ms, refused dispatch at 8517047 ms, `faults=[Storage]` and
`pending=false`. The ceiling was not extended to accommodate it. Final passing
repeats therefore do **not** close the host-latency/reliability blocker.

Final formatting and warnings-as-errors workspace Clippy passed. Locked workspace
tests passed **134 tests, 10 ignored**, including all original 55. The two new
direct rejection regressions remain ignored under Cargo's restrictive job and
were exercised in every ten-case direct suite. Original crate sources and the
parent's `.gitignore` were preserved; no Git mutation or remote operation occurred.

Private logs, regression red/green output and artifact hashes are retained under
`C:\Users\manni\AppData\Local\Temp\opencode\recovery-pipe-race-investigation`.
In particular: `red.txt`, `after-parallel-2.txt`, `resumed-parallel-1.txt`,
`final-rebuilt-serial.txt`, `final-rebuilt-parallel.txt`, `workspace.txt` and
`manifest.json`. Failed iterations remain failures, not filtered acceptance.
The spec remains in progress with the prior console, asynchronous fault matrix,
root/security, alert and G4–G7 blockers. This is not readiness to push.

### 2026-10-20 publication-latency finding

Full-history revalidation on every snapshot grew linearly: the measured worst
publication exceeded the proposed 100 ms ceiling at ~14 snapshots and reached ~1 s
at 47. The live publisher now validates the pinned ancestry plus the active chain
against the in-memory validated history, then the new snapshot lineage, stage
identity, backup bytes and bounded namespace; cold startup still performs the full
scan. After the change the measured worst at 47 publications was ~30 ms, allowing
the round trip to finish under the 100 ms ceiling. This in-session/cold-start
asymmetry is documented in [the storage contract](windows-storage-v1.md) and is
flagged for architecture review; it is not a durability or power-loss claim.

### Earlier diagnostic verification

New tests are named below; **partial** means only the stated P0/P1 subcase ran,
not that the full W case passed. Actual Windows storage/lock/process-crash and
full fake-round-trip tests now supply P1 evidence. No W case is claimed fully
accepted: console closure, exhaustive worker-kill barriers and native fencing remain
open. No test was run against physical hardware. P2/P3 are not tested.

### 2026-10-20 verification

**129 workspace tests passed**, 3 ignored (direct-run full round trips). The
normal suite includes original 55, contract 16 (9 general, 2 process, 5 storage),
core 28 (16 guardian, 4 worker, 8 journal adapter), harness 12 (2 CLI, 5 transport,
4 journal integration, 1 round-trip rejection, 1 latency measurement), Windows
port 17 (3 diagnostics, 14 storage), and the round-trip file's rejection test.
Formatting and warnings-as-errors Clippy passed. All 18 files in the two original
crates remain byte-identical; their dependency trees match the pre-edit backup.

The three ignored full round-trip tests were then run directly from the compiled
test executable outside Cargo's job and all passed: normal, error-after-change and
missing-candidate (details above). No fake or physical setter is reachable from
the probe; all device values are synthetic.

The real transport tests cover one-use measured bootstrap, mutual pipe handshake,
pipe squatting, wrong peer/key/MAC, replay/sequence gaps, wrong role/digest,
oversized/truncated/unknown-field frames, failed-channel reuse and owned-job
closure with an authenticated sibling surviving outside that job. The full round
trip covers preparation, two separated captures, sealed manifest, exact simulated
approval, arm, candidate, recovery, explicit restore and terminal durability.

Cargo runs these tests inside a job that forbids breakaway: the normal suite
verifies explicit `CREATE_BREAKAWAY_FROM_JOB` rejection and zero permissions,
**not survival**. The compiled lifecycle test was also run directly outside that
job: guardian and worker retained their creation identities across coordinator
kill and coordinator-job closure; an expired getter and pre-manifest candidate
were rejected. Console `WM_CLOSE` was not delivered as a real coordinator closure
on this host; the test explicitly reported that W01 subcase **BLOCKED**, not passed.

To reproduce the additional outside-job check without changing host job policy:
compile with `cargo test -p autotune-recovery-harness --test transport_acceptance
--locked --no-run`, then invoke the **exact executable path printed by Cargo**
directly with `--exact guardian_survival_or_enclosing_job_rejection_is_explicit_without_mutation
--test-threads=1 --nocapture`. An unsupported enclosing job still rejects. Do not
guess the executable hash/path, install a service or relax isolation to force a pass.

Two worker regressions were demonstrated red then fixed: revocation before exact
approval/candidate admission must not create restore authority; a permit cannot
execute twice or relabel an entered call as `NotEntered`. No mutation of either
existing crate or hardware capability was needed for these corrections.

New source-hash/dependency evidence is retained privately in the approved OpenCode
temporary directory. Raw diagnostic JSON includes actual user/logon SID, process
IDs, executable hashes and file identity; it must not be published without review.
The historical source-backup audit was retired to the local cleanup archive on
2026-10-29; see the [roadmap cleanup handoff](../ROADMAP.md) for its location.
No stage/commit/push occurred during that historical audit.

Prior foundation verification: **85 tests passed** (55 unchanged originals plus
9 contract, 16 core, 2 harness CLI, 3 Windows diagnostic tests), zero ignored.
Formatting and strict Clippy passed. `--platform-check` exited 2 as designed;
the current process reported `Isolation`, so it was not treated as a guardian.
Original source hashes and both dependency trees match the pre-edit backup.

Verification logs and hashes were recorded in the approved local temporary
directory during this run and are not committed. The final local run reported
129 passing tests, 3 ignored direct round trips and 18 preserved original files;
`manifest.json` marked the milestone not accepted, arming disabled and no hardware
accessed.
`manifest.json` identifies log/source/binary inventory hashes. `tests.log` SHA-256:
`74b6cd90c73d7d250f77e0cbb632bfd04c271f188672e21d67da6672e807ceaf`.
Prior foundation harness executable SHA-256:
`ae690ee322b2694321fa032bf3a1cdebd44379fac045ce07a1fb18788a693436`.
PowerShell 5 captures Cargo stderr as `NativeCommandError` records in these
logs even when Cargo exits 0; exit codes were checked explicitly.

## W01–W24 audit

Test paths abbreviations: `contract` =
`crates/autotune-recovery-contract/tests/contracts.rs`; `core` =
`crates/autotune-recovery-core/tests/reducer.rs`; `cli` =
`crates/autotune-recovery-harness/tests/cli.rs`; `storage` =
`crates/autotune-recovery-windows/tests/storage.rs`; `schema` =
`crates/autotune-recovery-contract/tests/storage.rs`.

| Case | Available evidence | Missing required evidence / blocker |
| --- | --- | --- |
| W01 | `transport_acceptance::guardian_survival_or_enclosing_job_rejection_is_explicit_without_mutation`: direct outside-job coordinator kill/job teardown survival, fixed process identities, zero mutation (P1 partial) | Actual console-close delivery blocked; launcher-loss and post-candidate cleanup still unaccepted |
| W02 | Actual Cargo-job breakaway denial and direct no-job launch, plus owned job closure/sibling survival (P1 partial) | Allowed-breakaway/nested-job matrix and repeat isolation before real arm |
| W03 | `journal_integration::actual_store_keeps_unknown_call_after_policy_worker_loss_without_restore_race`, `post_mutation...`; cross-process lock custody (P0/P1 partial) | Guardian/worker kill at every handoff/native-call barrier; external witness |
| W04 | `storage::actual_fixture_lock_and_replacement_preserve_ids_bytes_baseline_and_nonce`, `actual_cross_process_duplicate_lock_custody_outlives_original_owner` (P1 partial); same OS object/exclusive share mode | Cross-user/session/global production-root and full path-alias race matrix |
| W05 | Actual root/file owner/DACL checks, broad/null/inherited ACL rejection, root junction/rename denial, active hardlink and file/directory ADS rejection, local NTFS relative opening (P1 partial) | Other-user/wrong-owner setup, production ancestor policy, executable measurement and exhaustive races; scratch policy is not production G2 |
| W06 | `storage::actual_process_crashes_at_every_publication_step_retain_baseline_and_uncertainty`: ten distinct real before/after barriers, including half-written stage, flush/Replace/reopen/readback/close (P1 partial) | Kernel-internal failure points/actual flush errors and integrated guardian admission; P2/P3 not tested |
| W07 | `storage::actual_scanner_rejects_torn_extra_missing_history_and_unknown_schema_without_repair`, actual resource-limit test (P1 partial) | Exhaustive conflicting-backup/import fault permutations; OS-crash namespace persistence |
| W08 | `storage::real_storage_thread_timeout_quarantines_late_publish_and_retains_ownership`; `roundtrip::post_candidate_publisher_failure_delivers_ram_only_cleanup_and_blocks_cold_start` (P1 partial) | Actual fake-device cleanup interleaved with all storage fault/stall edges under guardian process |
| W09 | Real inherited bootstrap and token/logon/creation/image peer checks; wrong-peer/key rejection; first-instance squatter rejection; full round trip through authenticated fixed roles (P1 partial) | Complete inherited-handle audit, hostile cross-user/environment cases and full authority-channel negative matrix |
| W10 | Real hostile frame rejection and poisoned-channel reuse checks plus handshake vectors and canonical-parser tests; full authenticated round trip (P0/P1 partial) | Remote-client/peer race matrix, parser fuzzing and actor/control-flood integration |
| W11 | `core::expiry_wins_ties_and_no_late_dispatch_from_intent_ack`, `original_clock_anchor_detects_cumulative_drift_and_suspend`; `contract::permissions_bind_dispatch_origin_and_absolute_cleanup`; actual arm/freshness/expiry round trip (P1 partial) | All boundary combinations, worker-side UTC uncertainty and actor/control flood behavior |
| W12 | In-process lost completion and actual store unknown-call evidence; full round trip serializes every call (P0/P1 partial) | Actual blocked fake call/worker kill with external witness; no native fence |
| W13 | `core::old_generation_exact_atomic_return_retires_once_without_new_candidate`; `roundtrip::cancellation_after_candidate_delivers_revoke_and_restores_without_readback`; expired-getter paths (P0/P1 partial) | Exhaustive edge cancellation/disconnect interleavings, especially blocked RPC |
| W14 | `core` normal/error/missing/drift tests; real round trips for normal, error-after-change and missing-candidate with exact restored baseline (P0/P1 partial) | Full fault matrix with external witness, clamping/readback faults, second-restore edge cases under real processes |
| W15 | Real crash artifacts remain reconciliation-only; cold scanner publishes nothing, even for a clean-looking late terminal (P1 partial); CLI rejects replay | Full guardian/worker/witness all-process-loss reconciliation matrix |
| W16 | Original 55 regressions, source hashes/dependency trees, `cli::bare_help_and_invalid_live_backend_role_flags_do_not_start_workers` | Non-Windows target not built/run in this Windows-only environment; overall infrastructure incomplete |
| W17 | RAM-cleanup model plus `storage::late_terminal_write_after_timeout_cannot_authorize_a_cold_new_session` (P1 storage partial) | Independent guardian notice delivery/failure and integrated observed-B/API-error scenarios |
| W18 | Full-leaf consent/candidate-boundary tests and worker pre-approval/pre-candidate revocation plus single-use/false-no-entry regression tests (P0 partial); actual pre-manifest candidate rejection (P1 partial) | Complete manifest inventory review, hostile readings and later worker API-loss matrix |
| W19 | `storage::actual_two_session_rollover_preserves_terminal_link_and_cumulative_nonces`, schema replay/history tests, actual capacity rejection (P1 storage partial) | Crash at every rollover edge; no cold-clean-history publication without reviewed reconciliation |
| W20 | Old-generation retirement and permission entry tests; actual candidate/recovery/restore round trips with guardian-owned revocation epoch (P0/P1 partial) | Worker guardian-loss barriers, independent earlier-fault tightening and actual worker kill |
| W21 | Actual 100-ms capped storage timeout, retained lock, single quarantined publisher, late reply refusal; P0 publication-before-getter tests | Full cleanup-intent/getter stall and bounded actor ingress/control integration |
| W22 | Expected stage-to-active/backup IDs, `unexpected_identical_bytes_leaf_id_swap_is_not_a_valid_publication`, actual blocked storage-thread process death and restricted cross-process custody (P1 partial) | Full guardian/worker handoff death barriers and authenticated custody readiness; residual kernel I/O remains G3 |
| W23 | `core::missed_normal_recovery_is_retained_after_watchdog_and_late_return`, fixed cleanup permissions (P0 partial) | Delayed actual grants, worker tightening and late-arm scenarios |
| W24 | Actual inherited bootstrap/custody, preparation publication, finite worker chain, late-getter rejection; full real-process capture/seal/arm/round trip (P0/P1 partial) | External witness, all late-preflight cases and production-root qualification |

## Inherited M01–M32

The original `autotune-power-trial/tests/safety.rs` exercises M01–M32 in its
portable model, with 32 trial tests total including CLI tests. Probe tests add
23. These remain unchanged and do not close the new infrastructure cases.

New partial P0 evidence: M01–M03 (approval/deadline/clock tests), M06–M09
(capture/baseline/checked target tests), M11/M13 (publication stall tests),
M16–M19 (pending, expiry and old-generation tests), M20/M23 (normal path),
M31 (unchanged originals/graph), M32 (CLI rejection; non-Windows run still
unavailable). M04/M05/M10/M12/M14/M15/M21/M22/M24–M30 have no complete new
infrastructure test. All inherited invariants remain binding; none is waived.

Additional actual storage subcases now cover M11–M14 (exclusive opening,
publication/crash/ambiguity and failed-publication exclusion), M28 (same-root
cooperating owner exclusion), M29/M30 (cold data never grants permission), plus
preparation and nonce rollover semantics. This is P1 storage evidence only, not
a claim of complete new M acceptance or a replacement for the original model.

## Gates and continuation

G1–G7 remain unmet. G1 now has read-only process survival plus a full fake
guardian/worker/coordinator round trip outside Cargo's job; console closure and
worker-kill recovery remain missing. G2 needs production common-root/cross-user qualification beyond protected scratch fixtures. G3 now
has observed process-crash storage evidence but no OS-crash namespace/cache or
power-loss proof. G4 native fencing and G5 physical
entry timing cannot be inferred from fake messages or polling. G6 trusted human
consent/external ownership and G7 real device privileges/recovery remain outside
this implementation. Model publication verification is not evidence durability.

Next integration must preserve the current round trip while adding an external
synthetic witness, worker-kill/blocked-call barriers, console-closure evidence and
the remaining fault matrix. The in-session publication-validation change must be
reviewed against AD-6 before acceptance. No storage record may generate permission
on startup.
Do not remove `require_arm_support()`'s
blocker until those mandatory paths and required W cases are implemented.
Unavailable cross-user/destructive tests must stay explicit blockers. No source
backup commit, staging, push or remote operation was performed.

## 2026-10-24 quiet-launch continuation

Ordinary guardian, worker, coordinator and verifier roles now use
`DETACHED_PROCESS`; the coordinator still starts suspended and enters its own
kill-on-close job before resume. Guardian/worker job rejection and explicit
inherited-handle lists are unchanged. Recovery test subprocess wrappers use
`CREATE_NO_WINDOW`, including the storage crash-test child. Only an explicit
console diagnostic carries the inherited `console_test` bootstrap flag; it never
gives a worker a console or detaches a guardian from its independence checks.

`opt_in_owned_console_closure_acceptance` is ignored in ordinary tests. Build it
with `cargo test -p autotune-recovery-harness --test transport_acceptance --no-run --locked`,
then invoke the printed executable outside Cargo's restrictive job with
`--ignored --exact opt_in_owned_console_closure_acceptance --nocapture`.
This explicitly creates/closes owned consoles and fails if delivery is blocked.
It was **not run in this continuation**; console closure remains a W01 blocker,
not a silently skipped pass. The normal direct lifecycle test exercises observe,
coordinator termination, job closure, expired getter and denied candidate only.

Verification: formatting check and warnings-as-errors workspace Clippy passed.
Locked workspace tests passed **136 tests, 11 ignored** (ten
direct round trips plus the new console opt-in). Direct round trips passed 10/10
serially; quiet direct transport/lifecycle tests passed 5/5 with the console test
ignored. Initial direct testing with a `CREATE_NO_WINDOW` coordinator failed
9/10 at the strict console/job readiness check; switching the coordinator to
`DETACHED_PROCESS` corrected that regression without weakening the check. These
results do not resolve historical intermittent timing/Win32(109) failures.
Original 18 crate files, Cargo.toml, Cargo.lock and the existing `.gitignore`
match the pre-edit continuation backup. No hardware access or Git mutation.

Backup and SHA-256 preservation evidence:
`C:\Users\manni\AppData\Local\Temp\opencode\recovery-quiet-launch-ba407778db8142c399ca3981afd5e50e`.
The exhaustive asynchronous matrix, independent alert integration, production
root/cross-user qualification, storage live/cold review and G1–G7 remain open.
