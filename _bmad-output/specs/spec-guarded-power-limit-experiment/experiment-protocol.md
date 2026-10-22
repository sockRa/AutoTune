# Guarded power-limit trial protocol

**Draft for review; not an executable runbook or hardware authorization.** No live values have been captured for this proposal. All numeric policy below is proposed and requires review.

## 1. Evidence and isolation

Completed milestones established read-only enumeration and modern P0 graphics/memory offset reads on an RTX 2070 SUPER. They did not establish write access, restoration, undervolting, V/F access or other P-state behavior. Prior observations were 240000 mW current, 215000 mW default and 107500–240000 mW bounds, driver 617.42. None is a runtime input to this experiment.

Keep the existing probe and its production call allowlist unchanged. Proposed future layout: a separate `crates/autotune-power-trial` tool with consent/state-machine, journal, guardian and narrowly scoped NVML modules. Do not add a write flag, write trait method or callable setter to `autotune-probe`; no write-capable dependency may enter its dependency graph. The new tool defaults to non-mutating preparation or mocks, never live mutation on bare invocation. This design adds no crate or implementation yet.

Reproduce the probe's restricted System32 NVML loading and checked FFI lifetimes in the separate adapter. Only the original documented `nvmlDeviceSetPowerManagementLimit(device, unsigned int limit_mW)` may be bound as a setter. Use `nvmlDeviceGetPowerManagementLimit` for matching readback and `nvmlDeviceGetPowerManagementLimitConstraints` for bounds. Power usage and enforced/effective limits are not substitutes for the configured-limit getter. No alternate v2/scope API or command-line fallback after failure.

The pinned NVIDIA header documents root/admin permissions and non-persistence across reboot/driver unload. That does not prove this GeForce supports the write or that reboot restores the captured setting. Do not enable persistence, elevate automatically, unload the driver or reboot as a restore operation. A denied call ends the candidate trial, not an invitation to escalate/retry.

## 2. Consent gates

1. **Now:** human reviews this design, including assumptions and residual recovery risk. No hardware action follows implicitly.
2. **Later:** implement and verify mocks and Windows process/storage behavior without NVML setters. Present evidence and unresolved limitations for review.
3. **Preparation:** only under user direction, perform fresh read-only preflight and prepare the exact manifest. This session does not run that preflight.
4. **Execution approval:** user explicitly approves that exact manifest and its risks. A general “continue,” design approval, old report, checked-in JSON or generic `--yes` is insufficient.
5. **Arm:** authenticate single-use approval, independently revalidate preflight, persist recovery intent and receive guardian acknowledgement. Only then is one candidate write permitted.

Approval is bound to a canonical manifest digest, unpredictable experiment nonce and local authenticated operator interaction. No self-authorizing report/journal import. Persist nonce consumption before mutation; fail closed if consumption cannot be made durable. Approval expires at the earlier of its explicit UTC expiry or 60 seconds of monotonic elapsed time since final baseline capture. Wall-clock rollback must not extend it; discontinuity invalidates it. Slow review means a new capture, manifest and approval, not an extended token.

### Required immutable manifest

| Group | Exact information shown to and approved by user |
|---|---|
| Contract | Experiment ID/nonce, schema, spec/protocol digest, executable/guardian/worker digests, single-use approval digest and expiry |
| Device | Full UUID, normalized PCI domain:bus:device.function, PCI vendor/device/subsystem IDs, model; index is diagnostic only |
| Environment | Windows/build, driver and NVML versions, API/header source, trusted loaded DLL path, requested privilege level |
| Capture | Two current-limit readings and timestamps, exact baseline `B`, fresh minimum/maximum, optional default with availability; live provenance, never simulated |
| Mutation | Exact `T` and `B - T` in integer mW and displayed W, setter name, one candidate attempt, matching readback getter |
| Recovery | Exact `B`, same setter/getter, two maximum restore attempts, restoration verification and all timers below |
| Supervision | Guardian identity/readiness, parent/worker identities, protected journal location and ACL expectations, exclusive ownership checks, confirmation other tuning/profile tools are stopped |
| Risk | Display/work disruption, driver/GPU loss, baseline itself unqualified, possible unsuccessful restoration, no guaranteed crash/reboot recovery; supervised escalation procedure |

Show a human-readable summary and full manifest before approval. Any changed device, value, bounds, binary, privilege, timer or contract digest invalidates approval. Guardian readiness may precede approval but cannot grant write permission; its armed acknowledgement comes only after validation and durable intent.

Consent explicitly authorizes restoration of `B` following cancellation, parent death, candidate/readback failure and uncertain setter outcome within the active recovery session. Cancelling prohibits the candidate or ends it; it does not cancel cleanup already needed. No approval authorizes writing to an identity-mismatched GPU.

## 3. Device and preflight invariants

- Enumerate all devices, select by exact UUID, then independently verify full PCI identity on the resolved handle. Require exactly one match. Missing/ambiguous identity blocks. Never use a cached index or name alone.
- Acquire one protected per-device AutoTune ownership lock, covering preparation, writes and recovery. Reject any unresolved journal. The lock excludes other AutoTune instances, **not external tuning software**.
- Require the operator to stop other tuning tools, services and automatic profile application. Record inspection and confirmation. NVML provides no assumed global exclusive tuning lock; drift detection is defense-in-depth, not proof of isolation. Uncertain ownership blocks the trial.
- Read current limit twice, 500 ms apart, with successful identity and bounds checks. Both must be exactly `B`. Reject failed/sentinel/zero/overflow/inverted or implausible values; never fabricate values. Require `min <= B <= max` and `min < max`. Default is context only and may be unavailable without changing `B`.
- Independently re-read identity, current and bounds immediately before the candidate. Require unchanged identity, `current == B` and identical bounds. Any drift invalidates consent; do not overwrite it or quietly rebase the journal.
- Verify required getter/setter symbols, trustworthy getter semantics, guardian readiness, journal durability and approved privileges before arming. Symbol presence is not write verification. Read-only access cannot pre-prove restoration; the first write necessarily carries residual risk.

### Proposed candidate envelope

Use checked integer mW arithmetic, no floating-point setters:

`T = B - delta`, `1000 <= delta <= 5000`, `min <= T < B <= max`.

The exact `delta` and `T` must be selected and approved, never filled by the program from an example. If no such reduction fits, stop. No increase above the baseline, clamp, rounding, adaptive search or exploratory candidate retries. Returned bounds constrain the API, not hardware safety. Quantization or unexpected readback fails this trial and triggers restoration; a new target needs a new review/approval.

Illustration only: **if** a future approved capture were `B = 240000`, a separately approved `delta = 5000` would mean `T = 235000` and restore `240000`, never `215000`. This is not an approved experiment.

## 4. Guardian and expiry

Use an independent guardian process, not a thread or a `Drop` callback. The coordinator never holds a callable setter. The guardian owns the only serialized write channel through a narrowly scoped native worker; the worker accepts only authenticated, sequenced commands for this manifest's `T` once or `B` for recovery. Reject arbitrary values, stale generations and commands from other sessions.

The guardian must survive coordinator death, console closure and coordinator-owned job teardown. Demonstrate that on Windows with fake workers before hardware approval; do not assume child-process creation achieves independence. Keep the timer/control loop out of blocking FFI. Use process handles plus creation identity, authenticated private IPC and restrictive ACLs rather than PID alone. Guardian and worker binaries/privileges must match the approved manifest.

| Proposed timer/budget | Rule |
|---|---|
| Capture/approval freshness | At most 60 s from final baseline capture to candidate entry |
| Guardian lease | 10 s monotonic from arm acknowledgement; immutable, no renewal |
| Candidate admission | Start only in first 2 s after arm and while approval is still valid |
| Native operation watchdog | 1000 ms per call; timeout means unknown completion, not cancellation |
| Heartbeat | Coordinator every 250 ms; 1000 ms absence triggers recovery |
| Normal recovery | Start immediately after candidate readback, no dwell; request by arm + 3 s or trigger failure |
| Restore verification | Two fresh configured-limit reads 250 ms apart |
| Restore calls | At most two total, serialized; no unbounded retries/backoff |
| Recovery observation budget | 5 s from recovery trigger, not a fresh candidate lease |

Parent death, cancellation, heartbeat loss, lease expiry, call timeout, journal failure or candidate/readback error closes candidate admission permanently and triggers recovery. No new candidate after expiry, even if the coordinator resumes. Heartbeats cannot extend the lease. A recovery-only permission for the approved `B` remains for the bounded cleanup budget after lease expiry; expiration must not disable needed restoration. A missed 3 s normal-recovery deadline is recorded even if subsequent recovery works.

Serialize all worker commands. Before any recovery setter, establish that no candidate call can subsequently run or complete late. Invalidate queued commands, fence the command generation, and prove the previous worker is quiescent. Killing a process or cancelling a future **does not prove** an in-flight driver operation cannot finish. If quiescence cannot be established, do not race a restore worker against it: retain the journal, signal `recovery_required`, and escalate. The specific Windows mechanism and its limits require review before hardware execution.

Sleep/resume, clock discontinuity or guardian restart never renews admission; recover if safe, otherwise escalate. Guardian failure, kernel/driver hangs, power loss and simultaneous process loss remain outside guaranteed automatic recovery. Timers are detection/action deadlines, not promises that native calls return or the physical setting is restored within 10 s.

## 5. Recovery journal

This is a runtime safety journal, separate from BMAD's `.memlog.md`. Use a versioned, single-active-experiment record in a user-approved local persistent directory, not temporary storage, a network share or stdout. Protect directory/files/IPC with operator/guardian-only ACLs appropriate to approved privileges; reject symlinks/reparse redirection, insecure ownership and untrusted imported paths. Do not log approval secrets. Treat UUID/PCI as private when exporting evidence.

Record immutable manifest/digest, nonce consumption, full identity, `B`, `T`, bounds, versions, privileges, capture times, process/boot identity, lease/budget, state/sequence, each call's intent and result, raw vendor statuses, exact getter results, UTC and monotonic times, failure reasons, guardian acknowledgements and verification evidence. An unavailable value is typed unavailable, never zero. Call history preserves failures even if later restoration succeeds.

Each state replacement must be flushed and atomically published, with integrity/sequence checks and retained prior valid recovery data. Review and test actual Windows file flushing/replacement/ordering semantics; merely writing JSON or renaming a temp file is not proof of crash durability. The candidate cannot enter until `mutation_possible` and consumed consent are durable and independently validated by the guardian. Persist `restore_pending` before restore entry and results after each return. A crash between intent and outcome is always “may have mutated,” regardless of reported setter failure.

If storage fails before possible mutation, abort with zero setters. After possible mutation, retained in-memory guardian state must still drive authorized restoration; do not allow logging failure to skip cleanup. If restored hardware cannot be durably recorded, report that distinction and block further trials until reconciliation. Never report durable success because stdout was written.

Missing, corrupt, conflicting, unsupported-schema or partially written unresolved journals block new trials. Checksums detect damage, not malicious authorized-user modification; restrictive ACLs and manifest validation are also required. A journal is data, never authority to execute a command, choose a DLL or replay a setter.

## 6. State transitions and execution

| State | Admission and next action |
|---|---|
| `prepared` | Fresh capture and immutable proposal only; no setters |
| `consented` | Exact single-use approval verified; no setter yet |
| `armed` | Durable recovery state, consumed nonce and guardian acknowledgement; revalidate preflight |
| `mutation_possible` | Durable intent precedes candidate entry; from here every uncertain exit requires recovery |
| `candidate_observed` | Record candidate return and immediate exact getter result independently; enter recovery regardless of result |
| `restore_pending` | Candidate admission revoked; validate identity/bounds and quiescence; restore only `B` |
| `restored_verified` | Explicit restore attempted and two successful exact reads of `B`, matching identity and no pending native calls; durable close required |
| `aborted_no_write` | Evidence shows no candidate/native setter entered and no pending command; no write qualification |
| `recovery_required` | Restoration or its durable evidence is unresolved; retain journal, block new trials, alert operator |

Normal path is one setter to `T`, immediate configured-limit readback, then immediate setter to `B` and two verification reads. No workload or intentional holding interval. Only successful candidate return **and** exact readback `T` constitute a successful write/readback observation. A setter error may still change state; record the error, query if safe, and restore. A successful setter without reliable readback is not qualification.

For restore, re-resolve and verify UUID plus PCI identity and fresh bounds. Require original bounds unchanged and `B` still valid; changed identity/runtime/bounds or competing owner means no blind write. Report recovery-required rather than substituting a new value. Recovery does not require current to equal `T`, since partial mutation or clamping is possible; unexpected current is retained as evidence and reviewed for ownership conflicts.

Always explicitly attempt the approved `B` restore after a possible candidate write, even if a getter already reports `B`, provided identity, ownership and quiescence checks pass. Before each restore attempt durably record intent when storage works. After return, read configured limit; retain return status and unavailable/error outcomes. Two exact reads separated by 250 ms, valid identity and a quiescent worker establish observed restoration. A failed restore return followed by those reads may establish observed baseline restoration, but never hides the failed API call or makes the candidate trial successful.

Permit a second restore setter only after a completed first attempt with unsuccessful verification, while original identity/bounds/ownership and recovery budget remain valid. No retry on an unresolved blocked call. Missing/failed/mismatched verification, exhausted attempts or deadline failure is `recovery_required`. `nvmlShutdown` releases client resources; it is never restoration.

Success evidence includes immutable approval linkage, all actual calls/statuses/values, immediate candidate comparison, both final baseline reads, timing violations and a durable terminal journal. Report separately: candidate write, candidate readback, hardware restoration, evidence durability, overall trial result, simulated/live provenance and typed failures. The probe's own report and `not_tested` flags remain unchanged. Even a successful trial supports only this exact device/control/value/driver/session round trip, not offset tuning or stability.

## 7. Crash/reboot and manual recovery

- An active surviving guardian may recover under the original exact cleanup consent and budget; it does not need the crashed coordinator to reconfirm.
- If guardian/worker/driver is lost or recovery expires, notify the operator with experiment ID, UUID/PCI, captured `B`, last known/unknown state and journal path. Stop new experiments. No repeated blind writes or hidden reset/reboot fallback.
- Future startup checks unresolved journals **read-only**. Re-enumerate identity and read current/bounds; no startup service, scheduled task or automatic replay. Driver reset/reboot may change current to a boot default or another tool's profile, not `B`.
- Readback of `B` after restart may support read-only reconciliation, not proof the original restore API succeeded. Keep original failure evidence; explicit human reconciliation is required to unblock.
- A later recovery-only setter requires separate exact user approval after fresh matching identity/current/bounds and ownership checks. Its target is saved `B`, not freshly redefined “original”; if `B` is invalid, stop for human decision rather than clamp. Changed PCI identity after reboot requires investigation and a revised approved recovery design, not relaxed matching.
- Before live approval, agree a device-specific supervised escalation procedure, including saving work and what to do if the display or driver becomes unusable. Power cycling/reboot may be a human emergency action, but is not automatic restoration or proof of it. No recovery command is executed in this design session.

## 8. Checked source and live-entry blockers

Checked 2026-10-17: [NVIDIA maintained header, 13.4.92/API 13](https://github.com/NVIDIA/go-nvml/blob/6b38eea7ed6854b19001f44231e9385661e90c46/pkg/nvml/nvml.h), declarations/comments for the original power-limit setter, configured-limit getter, constraints and default. Original setter documents integer mW, admin permission and non-persistence. GeForce writable support and restoration are still unverified. The checked header is authoritative here; two guessed API-page URLs returned 404.

Hardware remains blocked until design review, reviewed implementation, passing mock matrix, demonstrated Windows guardian/storage behavior, assessed blocked-call limitations, stopped competing tools, approved manual recovery, fresh manifest and explicit exact experiment approval. Missing a feasible safety prerequisite means stop, not weaken the invariant silently.
