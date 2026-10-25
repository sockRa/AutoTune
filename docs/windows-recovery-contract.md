# Windows fake-worker recovery — implementation contract

Implementation contract for the separate recovery crates. Adopted AD-1–AD-12 and their safety gates remain binding. Native adapters, machine-wide provisioning and P2/P3 tests are outside this milestone.

Implementation progress (2026-10-20): the isolated **storage/journal v1 contract**
is frozen and implemented, including preparation before manifest, protected
relative opening, stable lock/custody, real publication/scanning and a quarantined
storage thread. The reducer↔journal adapter and a real-process
`--fake-roundtrip` path now exercise normal, error-after-change and
missing-candidate variants end to end. Live publication validates the pinned
ancestry, active chain, lineage, stage identity, backup and bounded namespace;
cold startup retains the full history scan (see [storage contract](windows-storage-v1.md)).
An external fake witness and selected worker-kill/blocked-call barriers now exist;
the exhaustive interleaving matrix and console-close evidence remain unfinished.
The harness refuses hardware arming. See [current evidence](windows-recovery.md).
Rules below remain requirements, not a claim of complete infrastructure acceptance.

2026-10-23 continuation: the real round-trip path now sends authenticated
`Revoke`/`StorageFailed` notifications before successor dispatch/receipts and
supports ordered RAM-only result receipts after publication failure. Fixed fake
profiles add cancellation after candidate return and a publisher failure at its
`Closed` barrier. These are narrowly tested P1 subcases, not complete asynchronous
cancellation, heartbeat, blocked-call or storage-stall integration. Worker status
now exposes its actual pending permit and frozen recovery window.

2026-10-23 witness continuation: additive fake-only `FakeInvocation`, `WitnessCall`,
`Message::FakeCall` and `Message::FakeResult` carry oracle traffic over the existing
authenticated worker channel. They never authorize entry or retire a permit.
The worker checks its immutable policy and chooses T/B before emitting entry;
the I/O lane requires the exact outstanding Dispatch and permits one oracle call
before the exact worker ReturnAndIdle. The guardian-owned harness witness retains
synthetic state/entry/effect traces across worker termination, validates sealed
write arguments, and can release a late effect after confirmed owned worker exit.
It is not independent of guardian loss and is not a native fence. The existing
portable worker-local fake backend remains only for P0 tests; actual role calls
now use the external witness, including getters. RoundTripReport adds witness
calls/current state, separately from API and worker-completion evidence.

Fixed cases add `worker-killed-during-candidate`, `blocked-candidate-returns` and
`alternate-baseline`. A watchdog fault releases the blocked synthetic effect
barrier, not the guardian pending slot; only the same authenticated worker's
completion permits cleanup. Watchdog/heartbeat reduction continues during Dispatch
RPC. At that checkpoint revocation was delivered only after RPC return; the
2026-10-24 continuation below adds selected in-call delivery. No arbitrary backend,
new peer, reconnect or replacement worker
was added. These are selected P1 W12/W14/W20/W21 cases, not complete acceptance.

2026-10-24 rejection/diagnostic investigation: after authentication, guardian errors
now attempt the existing typed `Rejected` response at the role boundary, including
errors before readiness. The launcher preserves the reason as `RemoteRejection`
and labels handshake/readiness/request/final-receive transport boundaries. This is
diagnostic data, not authority; unavailable delivery remains a failure, with no
retry/reconnect or success report. A fixed `startup-owner-conflict` case holds the
same fixture lock while launching the guardian and must reject before readiness.

Opt-in `AUTOTUNE_RECOVERY_DIAG` observations are buffered in RAM (1024 events,
256 KiB maximum) and written only after role execution returns. Overflow is marked
explicitly. These optional logs can be lost on termination and are not journal or
durability evidence. This removes synchronous diagnostic file opens/writes from
the timed actor path; the 100 ms publication ceiling and every other guard remain
unchanged. Original intermittent end-to-end reliability remains unqualified; see
the exact repeated results and retained failures in [the evidence](windows-recovery.md).

## 2026-10-24 selected asynchronous blocked-call control

The sole authenticated worker I/O thread now runs the fake oracle off-thread and
services a separate bounded queue of two control messages while the same Dispatch
is outstanding. Only `Revoke` and `StorageFailed` can enter this queue; their
authenticated zero-operation Heartbeat acknowledgements never retire the permit.
Controls accepted while the oracle is active are drained before FakeResult, with
an atomic gate preventing late controls from leaking into a successor RPC.
Failure/overflow closes progression; no reconnect, retransmit or replacement lane.
The worker processes these notifications while waiting for the oracle result,
retaining its entry and exact permit until ReturnAndIdle after the result.

The blocked fixture releases its synthetic effect only after actual worker control
acknowledgement. Additive fake profiles cover real coordinator heartbeat loss,
publisher failure at Closed, and a Closed-barrier stall past the unchanged 100 ms
deadline. The stalled sole publisher is released late by the same fixture barrier,
remains quarantined, and cannot close the session. Reports add the count
`in_call_controls_acknowledged` independently of idle, API and journal evidence.
These are selected P1 W08/W12/W20/W21/W23 subcases, not the exhaustive matrix.
Blocked-call coordinator cancellation/IPC loss, lease/budget edges and all prior
root/console/reliability/hardware blockers remain open.

## Platform and dependency selection

| Item | Selected baseline / evidence |
| --- | --- |
| Initial host | Observed Windows 10.0.26200.0; acceptance claims limited to recorded tested builds |
| Rust | Existing edition 2021 / Rust 1.99.1 GNU; no compiler installation/change |
| Linker | Existing portable LLVM-MinGW 20261006 / LLD 23.1.3; retain README session-only overrides |
| Binding | Target-Windows `windows-sys = "=0.61.2"`, MIT OR Apache-2.0, MSRV 1.71; verify GNU linking before depending on success |
| Binding features | Foundation, Security/Authorization/Cryptography, FileSystem, Threading/JobObjects/Pipes/IO, SystemInformation/WindowsProgramming; Console and user-mode NT file opening features only as needed by reviewed ports |

Research on 2026-10-19 checked [registry metadata](https://crates.io/api/v1/crates/windows-sys), [published manifest](https://docs.rs/crate/windows-sys/0.61.2/source/Cargo.toml) and generated API sources. `windows-link ^0.2.1` matches the existing lock resolution; no new GPU dependency. License selection for AutoTune itself remains unchanged.

## Shared contract ownership and validation

- `autotune-recovery-contract` is the sole owner of preparation, identity, full immutable manifest, consent, permission, completion, bootstrap, journal and evidence types. Strict decoding rejects unknown/duplicate fields, unsupported versions, oversize and inconsistent typed data. Neither old crate depends on it.
- Use fixed-field struct serialization into UTF-8 canonical bytes, no unordered maps or floats. Decode then compare with canonical re-encoding to reject alternate encodings. Keep role/session/digest/op/sequence and all body bytes inside the authenticated transcript; the wire prefix is u32 little-endian body length. A frame including authentication data cannot exceed 64 KiB.
- SHA-256 uses a domain-separation prefix per artifact class. Preparation digest and final-manifest digest are different domains. Existing portable manifest structs may guide field inventory but are not imported or changed.
- Freeze executable golden-vector tests for canonical bytes, lengths, digests, permission binding, MAC transcript and malformed/duplicate/unknown input before independent units integrate. One shared encoder serves every role.
- Preserve the protocol's full manifest field inventory, checked delta 1000–5000 mW and exact integer targets. All identities, baselines and vendor/runtime observations in this milestone are synthetic. Full report/summary and exact simulated approval are distinct from actual trusted human consent.
- Shared proposed constants: 100 ms publication timeout, 1000 ms dispatch-origin operation watchdog, 60 s capture freshness, 10 s lease, 2 s candidate admission, 3 s normal recovery request, 250/1000 ms heartbeat, 5 s first-trigger recovery budget, 250 ms verification separation; retain one candidate and two restores maximum. Preserve AD-8 queue/store ceilings.

## Readiness and authority

1. Transport bootstrap verifies inherited trust envelope and peer process identity before authentication; no arbitrary binary/backend override.
2. Capture readiness requires a published preparation capsule and worker custody of the exact exclusive lock object. Only explicit serialized fake getter operations are admissible.
3. Capture twice at least 500 ms apart; record actual sample timestamps and delay rather than fabricating exact timing. Host scheduling delays never extend freshness. Reject invalid/drifting readings; no historical/default fallback.
4. Seal final manifest against preparation/capture lineage and require exact simulated approval; preparation cannot authorize mutation or restore.
5. Arm only after consumed nonce, unresolved state and guardian validation are published. Candidate permission uses fixed expiry ceilings; lease starts at guardian acknowledgement commit, never receipt.

## Bootstrap, peers and IPC

- Launcher/guardian use a measured absolute executable path and explicit inheritable-handle lists. Create children suspended, assign coordinator job/retain process handles, provision bootstrap, then resume. Bootstrap child gets only its channel inputs and restricted guardian query/synchronize handle; worker also gets lifetime ownership custody. No secrets in argv, environment, journal or reports.
- Bootstrap envelope contains version, role, session, pipe locator, per-channel random 256-bit key, expected guardian/child process creation identities, measured binaries and rights/custody metadata. Child validates OS objects against these trusted inherited inputs, never unauthenticated server claims.
- Guardian owns first-instance, local-only, overlapped named pipes with explicit logon-SID DACL and minimal client rights. Verify both server/client PID against retained handle, creation time, token/logon/integrity context and measured executable. A PID query alone is insufficient.
- Use Windows CNG system RNG and HMAC-SHA256, constant-time tag comparison, fresh mutual challenges, role/direction/session/version/process-bound handshake. Sequence validation precedes coalescing or generation handling. Any uncertain command send retains its pending slot; no retransmission/reconnect authority.
- One atomic `ReturnAndIdle` binds original permit/generation with `NotEntered(reason)` or `Returned(entry, return, result)`. After authentication/session/sequence checks, accept closed-generation completion only for the outstanding operation. Retiring an unused candidate permit never reissues authority.

## Clock observations

- Sample precise interrupt time, unbiased interrupt time and UTC in brackets; keep original anchor and uncertainty. Supported clock APIs must exist or startup rejects. Monotonic deadlines use interrupt time including sleep; UTC remains an independent earlier expiry.
- Proposed conservative fake-infrastructure policy: maximum sampling bracket 2 ms, cumulative original-anchor UTC deviation 20 ms, suspend-divergence tolerance 2 ms. Reject regressions, excessive uncertainty or deviations; never add tolerance to a deadline or rebase the anchor. Near UTC expiry, reject within observed sampling uncertainty. These thresholds are policy, not Windows accuracy guarantees.
- Inject discontinuity/suspend observations for deterministic tests; do not change system time or suspend the host. Actual sampling cannot prove detection of every small or cancelling adjustment. Preserve G5's native check-to-call gap.
- Every permission carries dispatch-origin watchdog and applicable candidate/cleanup expiry in the common boot-clock domain. Timeout/revocation wins ties. Earlier worker-observed faults only tighten recovery expiry; never use permission receipt as a new origin.

## Secure storage and ownership

- No production root is provisioned. Test roots are explicit isolated fixtures under approved local storage; evidence cannot claim these prove machine-wide cross-user security. Startup on missing/insecure/unsupported root/anchor/lock rejects rather than initializes.
- Review a component-by-component opening protocol from a trusted local NTFS anchor, with held ancestry handles and user-mode `NtCreateFile.RootDirectory`/`OBJ_DONT_REPARSE` as appropriate. Reject separators/streams in leaves, reparse traversal, unexpected types/hardlinks, volume/identity changes and broad write/delete/create rights. Check owner/DACL by handle at creation and opening, not just canonicalized strings. If protection cannot be established, block rather than downgrade.
- OS-managed trusted ancestors and explicit test-root security must be distinguished. Same-account malicious software, administrators/SYSTEM and hostile kernel/storage are outside protection. User-mode NT calls do not authorize kernel code, driver loading or elevation.
- Stable `owner.lock` uses share-mode zero and is never replaced/deleted to steal ownership. Worker-ready attests a duplicate of the same file object before capture. Worker survives coordinator loss and retains custody across blocked operations; coordinator never inherits it.
- Guardian-owned storage thread publishes full bounded snapshot via fresh stage, complete write, flush, same-volume replace with retained backup, reopened expected stage identity, flush/readback. Account for replacement-compatible leaf sharing without relaxing ancestry checks. Any ambiguous stage/backup/history/nonce state blocks read-only startup.
- Reserve/journal every explicit getter/setter intent and result. Await healthy publication acknowledgement before progression. Before possible mutation, failure aborts; after possible mutation, publication failure or capped timeout enables safe RAM-only cleanup while the stalled writer stays quarantined. No durable closure after storage failure.
- Snapshots retain global record sequence, per-session immutable manifests, prior terminal linkage and cumulative consumed nonce history; only clean terminal state permits a new session. No automatic compaction or unexplained-artifact deletion; capacity exhaustion rejects new sessions.

## Process and call lifecycle

- Ordinary fixed roles are detached (including the coordinator, still suspended
  and assigned to its dedicated kill-on-close job before resume). Test subprocess
  wrappers use `CREATE_NO_WINDOW`. A trusted inherited bootstrap `console_test`
  flag selects only explicit owned-console diagnostics; the guardian remains
  detached and workers reject this flag. Only opted-in coordinator/verifier
  diagnostics use `CREATE_NEW_CONSOLE`; normal suites never request them.
- Guardian detaches and verifies no console/any-job membership before ready and arm. Enclosing jobs that deny breakaway cause rejection; no policy modification/service/elevation workaround. Coordinator job teardown cannot contain guardian/worker.
- One worker lane, one pending operation, explicit finite getter-validation chain and no hidden recursive calls. Missing API availability is a fake injected flag here. Validate required identity/bounds/runtime/ownership results before subsequent permission, not through untraced internal calls.
- Watchdog/cancel/IPC loss/worker exit never establish quiescence. Same-worker exact completion can retire its permit; no replacement/parallel restore while unresolved. Guardian loss permits already-entered return only, no successor after observed loss.
- Witness owned by the test harness retains synthetic state/late effects across worker termination. It is an oracle, not an alternative source of execution permission or native quiescence evidence.

## Implementation evidence

Map W01–W24 and applicable M01–M32 to actual test names/artifact hashes and pass/failure/blocker. Required blocked cases prevent complete infrastructure acceptance. Report P0/P1 separately, never infer P2/P3, and preserve G1–G7. No physical power-loss, actual OS crash, GPU driver or hardware behavior is tested.

Research references: [user-mode NtCreateFile](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntcreatefile), [object no-reparse](https://learn.microsoft.com/en-us/windows/win32/api/ntdef/ns-ntdef-_object_attributes), [handle security](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-getsecurityinfo), [clock semantics](https://learn.microsoft.com/en-us/windows/win32/sysinfo/interrupt-time), [process creation](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw). Architecture companion retains checked process/job/pipe/flush/replace/CNG references. Documentation fit is not implementation verification.
