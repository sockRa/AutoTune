# Windows recovery proposal — acceptance and proof boundaries

**Specified only; not implemented or executed.** All device calls below are fake. This document extends, not replaces, [M01–M32](../../../specs/spec-guarded-power-limit-experiment/failure-tests.md). The completed simulator's 55 passing tests are prior reported evidence, not a pass for this matrix.

## Common oracle

For every trace assert: one owner; one immutable manifest; consumed nonce and acknowledged candidate intent before candidate dispatch; one candidate maximum; at most two restores; at most one outstanding device operation (including getters); no restore before trustworthy return/idle of every earlier permission; no candidate permission after admission closes; no expiry/recovery-budget extension; exact synthetic identity/value; distinct API/readback/restoration/storage results; unresolved exclusion after restart. Trace entry and return separately from dispatch/acknowledgement. Real host scheduling is not deterministic; use fake-call barriers and bounded eventual checks with recorded timestamps, not exact scheduler latency assertions.

| Tier | Evidence allowed | Evidence not established |
| --- | --- | --- |
| P0 deterministic reducer/fault model | Explored state transitions and boundary behavior | Windows isolation, ACLs, durability or native quiescence |
| P1 actual Windows processes/storage with fake workers | Observed console/job survival, IPC rejection, file/ACL/lock behavior and process-crash recovery on recorded host | OS crash persistence, controller-cache honesty or GPU restoration |
| P2 separately authorized disposable OS-crash environment | Observed recovery artifacts after tested VM/OS crash cases | Physical power-loss guarantee or equivalent behavior on user's storage |
| P3 hardware qualification | None in this milestone | Any real setter access, baseline restoration, safe values or stability |

`evidence_durable` from the completed memory model must not be reused as a claim of physical durability. New evidence records publication verification and the applicable proof tier explicitly.

## Infrastructure acceptance matrix

| ID / inherited matrix | Fault or interleaving | Required evidence / rejection | AD |
| --- | --- | --- | --- |
| W01 / M15, M17 | Launcher exits; coordinator killed before/after arm; actual coordinator console closed; coordinator job terminated and last kill-on-close handle closed | Out-of-job detached guardian and worker remain live; pre-dispatch rejection or bounded fake recovery without coordinator | 3, 10 |
| W02 / M15, M17 | Host job allows breakaway, forbids it, uses nested constraints; fake readiness with unknown job/console state | Allowed job-free case independently verified; all remaining/unknown job cases refuse arm. Process-group flags alone never pass | 3 |
| W03 / M17, M18 | Guardian dies while fake candidate/getter/restore is blocked, before dispatch, after return and during terminal publication | No automatic replacement/replay; worker-held duplicate lock preserves exclusion while worker lives; restart remains read-only and blocked by uncertain journal | 4, 9, 10 |
| W04 / M11, M28 | Concurrent owners in two processes/sessions, same root via path aliases; lock file substitution; stale PID | One stable exclusive handle wins; loser performs no fake operation, cannot retry alternate root or steal lock; test actual file IDs | 4, 5 |
| W05 / M11, M14 | Wrong owner/DACL, broad inherited ACE, root/ancestor junction swap, leaf reparse, hardlink, alternate stream, missing anchor/lock, different volume/network root | Reject before fake mutation; retain artifacts; never initialize a missing active store. Tests requiring unavailable privileges are blocked, not simulated passes | 5 |
| W06 / M11–M14 | Crash/short write/fault before, during and after stage creation, write, flush, replacement, reopen, second flush, readback and close | Use actual filesystem artifacts and process termination barriers, not six labels for one memory fault; saved baseline retained, ambiguous snapshots block; zero candidate without accepted publication | 6 |
| W07 / M12, M14, M29 | Orphan stage, missing active file, conflicting backups, valid old closed snapshot plus newer partial intent, modified nonce history, unsupported schema or bounded-record overflow | Scanner does not pick convenient old success; read-only failure, no setter replay, no automatic deletion/repair. Missing namespace metadata is a limitation of P1 | 4, 6 |
| W08 / M13, M15 | Storage executor stalls indefinitely before candidate, after possible mutation, at restore intent and terminal close; cancellation and expiry race late storage acknowledgement | Guardian timers stay responsive; no pre-entry dispatch; after mutation safe fake cleanup proceeds from RAM without storage wait. No second storage publisher, no durable success; late ack cannot reopen admission | 6, 8, 10 |
| W09 / M01, M02, M16 | Pipe squatter; wrong SID/logon; same-user wrong process; stale PID/creation identity; remote client; wrong binary measurement; bootstrap handle leak | Fail peer/capability checks before arm; explicit inherited handle inventory; no approval secret in argv/env/files/report; no remote fallback | 3, 7 |
| W10 / M01, M02, M16 | Wrong MAC/role/key/session/digest, duplicate/gap sequence, oversized/truncated/unknown frame, reordered command, reconnect after arm | Shared golden vectors and parser fuzz/property tests; reject with zero new permissions; uncertain sends never retransmitted; heartbeat flooding cannot starve deadline handling | 2, 7, 8 |
| W11 / M03, M16, M19 | Exact admission/lease/watchdog/heartbeat/recovery ties; delayed arm ack, old queued grant, overflow, UTC jump, injected suspend and unbiased-clock divergence | Absolute ceilings and first recovery trigger preserved; deadlines win ties; worker performs entry-time rejection. Native check-to-call scheduling gap remains explicitly unproven | 8, 9 |
| W12 / M18, M19 | Fake call hangs then returns just before/at/after recovery deadline; worker killed/disconnected after dispatch but before entry acknowledgement | One outstanding operation persists; zero parallel restore or getter; same-worker matching return+idle permits only within-budget cleanup, never new candidate. Lost completion remains unresolved | 7, 9, 10 |
| W13 / M15, M18, M19 | Cancel at every publication/dispatch/entry/return edge; double cancel; IPC loss at each edge; stale candidate generation | Close admission once, preserve immutable cleanup budget; accepted-but-not-entered command is possible mutation until resolved; cancel never disables necessary cleanup | 6, 8–10 |
| W14 / M20–M28 | Error before/after fake mutation; exact/off-by-one/clamped/unavailable readback; identity/runtime/bounds drift; restore failures; second-restore budget edges | Exact freshly captured synthetic B (at least two unrelated values), no default/history fallback, one candidate/two restores, two separated exact reads, failures retained. Changed prerequisites block blind restore | 1, 9, 10, 12 |
| W15 / M29, M30 | All processes lost; startup reads baseline/default/unknown; different session/boot; old journal imported as approval | Read-only reconciliation only, saved failure history retained, zero implicit writes or new lease; no human-reconciliation CLI is invented in this slice | 6, 10 |
| W16 / M31, M32 | Build graph/source review; unsupported platform; bare/help/live-flag parsing; original regression suites | Existing probe and mock behavior unchanged, no dependency into new infrastructure, no callable hardware symbols/loader/elevation/commands, every device report synthetic | 1, 11 |
| W17 / M13, M17, M26 | Coordinator gone plus storage/notification failure, failed terminal close, observed baseline with API error | Alert attempted without coordinator, delivery uncertainty recorded; restoration, API and durability remain independent; next trial blocked despite observed B | 6, 12 |
| W18 / M01–M03, M08–M10 | Missing/generic/wrong-operator approval, mutation of every valid nested manifest leaf, independent experiment nonce; delta 999/1000/5000/5001, T>=B, invalid required/default-only readings; symbols lost at readback/restore/verification | New shared validator/worker retains full source semantics, zero unauthorized permission, no silent target choice or API fallback; fake approval never called trusted human consent | 1, 2, 7, 9 |
| W19 / M12, M14, M28 | Two clean sessions with different manifests, replay first nonce, crash at every rollover edge, capacity ceilings | Explicit prior-terminal linkage, complete nonce carry-forward, expected backup membership; no compaction, reset or new session on ambiguity/exhaustion | 4, 6, 8 |
| W20 / M15–M19 | Cancel then old-generation atomic ReturnAndIdle; expired-before-entry rejection; guardian dies at restore send/receive/pre-entry/post-entry; earlier worker-only fault | Retire exact original permit once, distinguish NotEntered from Returned; no candidate reissue or old-generation new authority; effective cleanup deadline never later than first observed trigger+5 s; no new call after observed guardian loss | 7–10 |
| W21 / M11, M13, M18, M19 | Stall cleanup-intent publication; delay entry acknowledgement; pre-arm getter hangs; saturate ordinary ingress/maximum store under cancellation | Healthy storage acknowledges before dispatch; explicit failure or capped 100 ms timeout permits RAM-only safe cleanup without further wait, never beyond recovery expiry. Dispatch-origin 1000 ms device watchdog never renews and binds worker entry too; separate completion/control capacity, no admission or evidence loss on overflow | 6, 8, 9 |
| W22 / M11–M14 | Expected stage-to-active file-ID replacement vs unrelated leaf-ID swap; guardian loss with blocked storage thread; kill during worker lock handoff | Correct replacement accepted with compatible leaf sharing/closure; unexpected ID rejected; no independently surviving application publisher, custody verified before capture; kernel-I/O persistence limits remain G3 | 3–6 |
| W23 / M03, M15, M19 | Restore grant delayed after guardian-only storage fault; late arm acknowledgement; normal recovery misses arm+3 s then cleanup succeeds | Grant binds frozen boot-relative trigger/deadline, no receipt-relative rebase; worker may tighten, never extend; missed normal deadline remains in final evidence | 7, 8, 12 |
| W24 / M01, M06, M11, M18 | Start from inherited bootstrap only; capture before final manifest; missing preparation publication; nested validation getter attempt; preflight permit arrives after watchdog | Transport/capture/manifest/arm readiness are distinct; preparation permits fake getters only and binds capture lineage into sealed manifest. Every validation getter has its own finite trace/intent; no recursive/hidden calls, fabricated manifest or late preflight entry | 2, 6–9 |

## Failure-test harness constraints

- A fake device-state witness, owned by the harness rather than the worker, records exact operation/identity/value entry/return and can release late completion after worker loss. It exposes overlap bugs hidden by deleting process-local state. Witness access is test-only, never an alternate production command channel.
- Actual guardian/worker IPC and journal port paths are exercised, not a second state machine manufactured in test code. Reducer behavior is compared to applicable completed-model invariants without changing the completed simulator.
- Maintain explicit fake-call barriers at publication, permission send, entry and return. Cover pairwise cancel/timeout/storage/parent events and generated traces; bounded exploration is not formal verification.
- Real console closure tests require an isolated owned console. Never close the user's working console or terminate unrelated jobs. Host process-crash tests kill only harness-created processes. Root/ACL changes stay inside dedicated test roots; privileged negative tests require separate authorization/environment.
- Use real OS time for process liveness and fake time for exhaustive deadline cases. Record OS build, filesystem, toolchain, binary digests, process/job identities, test IDs, artifact hashes and evidence tier; redact private identifiers on export. No unsafe environment-wide clock/suspend changes as a test shortcut.
- Native binding/CNG correctness, exact shared schemas/golden vectors and race-resistant directory opening require design/implementation review before P1 acceptance. Failures or prerequisites unavailable to the unelevated harness are reported as blocked.

## Prerequisites that this proposal cannot prove

| Gate | Missing evidence | Result |
| --- | --- | --- |
| G1 guardian independence | No Windows fake-worker implementation/test has run | Infrastructure not accepted yet; only stated supported launch envelope |
| G2 protected common ownership | Approved persistent root, ACL provisioning, other-user tests and namespace-race resistance | Per-user fake test directories do not qualify production root |
| G3 crash-consistent storage | Actual Windows ordering/crash artifacts; OS-crash namespace persistence; storage-cache guarantees | Flush/replace/readback is a proposed algorithm, not certified power-loss durability |
| G4 native quiescence | Documented and demonstrated completion/fence semantics if a worker/driver disappears | No such mechanism established; killing worker is not a fence; hardware trial blocked |
| G5 immutable physical entry | Clock discontinuity semantics and check-to-native-entry scheduling/suspend gap | Fake boundary tests do not prove hard real-time native admission |
| G6 trusted approval and isolation | Approved operator authentication, binary measurement, common trust/privilege boundary and stopped competing software | Pipe authentication alone is insufficient for exact informed human consent or global hardware exclusion |
| G7 hardware recovery | Fresh UUID/PCI/current/bounds, exact manifest/approval, privilege decision, supported setter/getter semantics and supervised manual recovery | Hardware writes and restoration remain unverified; historical 240 W / 215 W not usable |

If any required gate remains infeasible, stop at fake-only infrastructure. Design approval, passing tests or a general “continue” cannot waive a gate or authorize a real experiment.

## Checked platform sources

Checked online 2026-10-17; these document API behavior, not empirical guarantees on this host. Two initial guessed Microsoft API URLs returned 404; the corrected canonical links below were read successfully.

- [Job objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects): inheritance, nested jobs, breakaway policies and kill-on-close.
- [Process creation flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags): detachment and breakaway are separate mechanisms.
- [Explicit inherited handle list](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute): child handle allowlist, not broad inheritance.
- [Named-pipe security](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights): unsafe default exposure and generic-write/create-instance overlap.
- [CreateNamedPipeW](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-createnamedpipew): first instance, remote rejection and overlapped I/O.
- [Pipe client process identity](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeclientprocessid): PID query is only one component of peer verification.
- [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew): share exclusion, reparse flags, security and cache limitations.
- [FlushFileBuffers](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers): explicit flush API; no admin volume flush is proposed.
- [ReplaceFileW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew): backup/same-volume requirements, partial-failure states and **unsupported write-through flag**.
- [QueryInterruptTime](https://learn.microsoft.com/en-us/windows/win32/api/realtimeapiset/nf-realtimeapiset-queryinterrupttime) and [QueryUnbiasedInterruptTime](https://learn.microsoft.com/en-us/windows/win32/api/realtimeapiset/nf-realtimeapiset-queryunbiasedinterrupttime): non-wall-clock observations; unbiased time excludes sleep.
- [BCryptGenRandom](https://learn.microsoft.com/en-us/windows/win32/api/bcrypt/nf-bcrypt-bcryptgenrandom) and [BCryptCreateHash](https://learn.microsoft.com/en-us/windows/win32/api/bcrypt/nf-bcrypt-bcryptcreatehash): OS RNG and keyed HMAC support; integration/vector testing still required.
