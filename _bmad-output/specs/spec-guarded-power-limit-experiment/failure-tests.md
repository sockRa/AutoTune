# Mock-only acceptance and failure tests

**Specified, not implemented or executed.** No test here may load NVML, call a real setter, request elevation, reset hardware or start a workload. Mock success is not hardware qualification.

## Test harness requirements

- Inject fake NVML getters/setter with exact operation, handle, argument, entry/return and time traces; model a setter that fails after changing state or completes late.
- Inject monotonic/UTC clocks, process lifecycle/IPC, ownership locks and a journal store with crash, partial-write, flush and replace fault points. Use fake time, not long sleeps.
- Exercise the production state machine and private native resolver seams, not just preassembled reports. Windows process/storage acceptance uses fake workers and isolated test directories without a GPU runtime.
- For every case assert setter count, order, exact UUID/PCI/handle/value, journal intent-before-entry, admission revocation, final evidence/status and unresolved-journal blocking. Every synthetic report is marked simulated.
- Parameterize failure before/after every state publication and native-call entry/return. Explore interleavings of cancellation, expiry, heartbeat and call completion; no generated trace may admit a second candidate or false restoration.

## Acceptance matrix

| ID / capability | Injected condition | Required result |
|---|---|---|
| M01 / CAP-1 | Missing, generic, wrong digest, expired, reused or wrong-operator approval | Zero setters; no arm or candidate; precise rejection |
| M02 / CAP-1 | Change target, bounds, device, timing, binary/spec digest or privilege after consent | Zero setters; demand fresh manifest/approval |
| M03 / CAP-1,4 | 60 s capture boundary, UTC rollback/forward jump, monotonic deadline, sleep/resume | No extension; stale capture/discontinuity blocks candidate; post-mutation triggers recovery |
| M04 / CAP-2 | Duplicate UUID, absent UUID/PCI, same model on two devices | Zero setters; ambiguity never resolves by name/index |
| M05 / CAP-2 | Enumeration reordered while target retains UUID/PCI; handle now points at another device | Re-resolve exact pair; write only matching device or stop; never index zero by default |
| M06 / CAP-2 | Baseline changes between capture reads or before setter; bounds drift | Zero candidate setters; invalidate approval, never silently rebase |
| M07 / CAP-2 | Current 240000, default 215000; separate case current 230000 | Restore freshly captured current in each case, never hard-coded 240000/215000 |
| M08 / CAP-2 | Missing/failed/sentinel/zero/inverted bounds/current; default getter alone unavailable | Required-field faults block; default absence never replaces or invalidates valid baseline by itself |
| M09 / CAP-2 | `B` outside bounds, no room below `B`, delta 999/5001, `T == B`, increase, unsigned underflow/overflow | Zero setters; checked arithmetic, no rounding/clamping |
| M10 / CAP-2,5 | Required symbols absent, getter unsupported/denied, setter denied/unsupported | Preflight faults block; attempted setter failure preserves status and enters recovery because mutation is uncertain; no escalation/fallback |
| M11 / CAP-3 | Journal create/ACL/lock/flush/replace/readback failure before mutation; guardian cannot validate | Zero setters; no false arm; consumed approval cannot be replayed |
| M12 / CAP-3 | Crash after durable mutation intent but before result, including before native entry | Restart treats intent as uncertain, not no-write; active guardian safely reconciles/restores; later startup is read-only |
| M13 / CAP-3,5 | Journal fails after possible mutation, during restore intent or terminal close | In-memory guardian still attempts safe authorized restoration; retain failure and block new trials even if hardware reads `B` |
| M14 / CAP-3 | Truncated/corrupt/stale/unknown-schema journal, conflicting nonce, redirected path, insecure ACL or tampered baseline | No automatic setter replay; reject/block and require human reconciliation; no arbitrary path/command execution |
| M15 / CAP-4 | Parent dies, console/job closes, IPC disconnects, cancellation at every phase | Before entry: no candidate; after possible mutation: surviving guardian recovers without parent; fake Windows process test proves survival |
| M16 / CAP-4 | Heartbeat ceases for 1000 ms; heartbeats continue past 10 s lease; queued stale candidate | Recovery triggers; no renewal, late or duplicate candidate; queued generations rejected |
| M17 / CAP-4 | Guardian never ready, dies before arm or after mutation | Before arm: zero setters; after mutation: unresolved journal and visible recovery-required, not a cleanup guarantee |
| M18 / CAP-4 | Native worker hangs in getter or candidate setter; timeout followed by delayed completion | Watchdog remains responsive; no parallel restore or late candidate after a claimed restore; inability to prove quiescence stays recovery-required |
| M19 / CAP-4 | Expiry/cancel races with candidate dispatch, result and restore dispatch | One serialized decision; no candidate after closed admission; restore-only cleanup permission remains bounded beyond lease |
| M20 / CAP-5 | Candidate succeeds and getter exactly `T` | One candidate, immediate readback, explicit `B` restore and two baseline reads; no dwell or workload |
| M21 / CAP-5 | Setter returns error before change and separately error after change | Both record failure and restore safely; neither becomes successful candidate qualification |
| M22 / CAP-5 | Setter succeeds but readback clamps, remains `B`, is unavailable, invalid or differs by 1 mW | Fail exact candidate comparison and restore; no tolerance, rounding, second candidate or fabricated success |
| M23 / CAP-5 | Restore succeeds and two reads equal `B` 250 ms apart | Observed restoration only after both reads, matching identity, quiescence and durable terminal record |
| M24 / CAP-5 | First restore returns error, mismatch or failed getter; second completes successfully | At most two restore setters within budget, rechecked identity/bounds; retain first failure; distinguish restored baseline from failed trial |
| M25 / CAP-5 | Restore returns error but two valid fresh reads equal `B` | Record observed baseline restoration plus API failure; overall candidate/recovery API failure remains visible |
| M26 / CAP-5 | Both restore attempts fail, second verification drifts, deadline expires, or getter never becomes reliable | Recovery-required; no success close, infinite retries or default fallback |
| M27 / CAP-2,5 | GPU lost, UUID/PCI changes, bounds/runtime changes or competing owner appears after candidate | No blind restore to replacement/changed device; block qualification, preserve journal and escalate |
| M28 / CAP-2,4 | Second AutoTune starts; external tool/profile changes values | AutoTune lock rejects second owner; detected external drift aborts/requires recovery; tests never imply global lock coverage |
| M29 / CAP-3,5 | Reboot/driver reload leaves current at default, `B`, or unknown; saved `B` outside new bounds | Startup only reads; no reboot-as-restore claim or auto replay; matching `B` supports explicit reconciliation only; invalid `B` never clamped |
| M30 / CAP-1,5 | Recovery-only action without new exact approval after guardian/boot loss | Zero setters; valid separate recovery approval cannot authorize a new candidate |
| M31 / CAP-6 | Run all existing probe mocks/native fake-getter traces and mutation-boundary checks | Same schema/provenance/typed failures, opaque setter inspection and `not_tested`; zero executed setter operations and no write dependency |
| M32 / CAP-6 | Exercise missing/unsupported runtime and non-Windows live branch in future tool | Structured unavailable/unsupported result, no write fallback; mock state machine remains portable |

## Required evidence before live approval

1. All M01–M32 pass with deterministic traces and crash-point/interleaving coverage; implementation review resolves any changed invariant.
2. Formatting, Clippy and workspace tests pass without weakening the probe's existing mutation assertions. Inspect dependency graph and native-call allowlists separately for probe and trial tool.
3. Windows fake-worker tests demonstrate parent-death survival, restricted IPC/ACLs, single ownership, journal flush/replace ordering, expiry and no overlapping native calls. Document what mocked durability cannot prove about physical power loss.
4. No claim that process termination cancels an in-flight driver operation. Resolve the quiescence design or explicitly keep the hardware trial blocked.
5. Present the exact hardware manifest separately for user approval; neither test passage nor design approval is execution consent. No stress tests belong to this milestone.
