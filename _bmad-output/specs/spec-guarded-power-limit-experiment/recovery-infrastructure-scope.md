# Recovery infrastructure — milestone boundary and companion adoption

## Authority and status

- This 2026-10-18 update adopts the architecture proposal and acceptance matrix as required spec companions, preserving AD-1–AD-12, W01–W24, G1–G7 and CAP-1–CAP-6.
- Adoption permits this documentation update only. It does not implement the proposal, accept every `[PROPOSED]` choice, execute tests, provision the machine or authorize an experiment.
- Architecture `status: final` means proposal preparation is complete, not Windows qualification. Its closing adoption invitation is historical; this spec now discovers it through `companions:`. The architecture remains owned by `bmad-architecture` and is not rewritten here.
- The completed mock-only milestone is human-approved and reports 55 passing tests (32 trial, 23 probe). These results were not rerun during adoption and do not establish Windows survival, IPC authentication, ACLs or physical storage durability.

## Companion interpretation

| Companion | Role in the contract |
| --- | --- |
| `experiment-protocol.md` | Future hardware protocol, exact consent/value/API constraints and base recovery invariants; not a current executable runbook |
| `failure-tests.md` | Inherited M01–M32 safety matrix; original “specified” heading is not a claim that the subsequent portable model was never implemented, nor that Windows/native coverage now passes |
| Adopted `ARCHITECTURE-SPINE.md` | Windows fake-only architecture and shared AD invariants; proposed choices retain their labels and evidence gates |
| Adopted `ACCEPTANCE-GATES.md` | W01–W24 infrastructure cases, P0–P3 proof boundaries and unmet G1–G7 prerequisites; specified, not executed |

All companions must be read. No original hardware safety invariant is relaxed by adoption. A substantive contradiction requires an explicit resolution, not choosing a convenient companion.

## Explicit refinements to the earlier protocol proposal

| Earlier statement | Current scoped interpretation |
| --- | --- |
| A future `autotune-power-trial` crate could contain native recovery components | That name now belongs to the completed independent portable simulator. Do not retrofit or rename it. The architecture's separate `autotune-recovery-*` names are a structural seed, not added crates |
| Per-device AutoTune ownership | Architecture proposes stronger single-active-trial global exclusion through one canonical common root and stable lock; this does not exclude external tuning tools |
| Guardian readiness before arming | Transport, capture, sealed-manifest and armed readiness are distinct; a durable preparation capsule allows only fake getters, never fabricated baseline/restoration authority |
| Flushed replacement with retained recovery data | AD-6 specifies staged publication, handle identity/backup lineage, nonce/session rollover and fail-closed ambiguity. Successful flush/replace/readback does not prove OS-crash namespace or physical power-loss persistence |
| Publish restore intent when storage works; preserve cleanup if storage fails | While healthy, await publication acknowledgement before progression. Explicit failure or capped 100 ms publication timeout after possible mutation permits otherwise-safe RAM-ledger cleanup without further storage wait; original recovery expiry still wins |
| Fence before restoration | One serialized operation lane includes all getters. The exact original permit must retire through authenticated `ReturnAndIdle`; uncertain dispatch/return, worker death or kill never permits a racing replacement worker |
| Full live manifest and operator approval | Current manifest/device/runtime values are explicitly synthetic, and fake operator interaction remains simulated. IPC peer authentication does not prove informed human hardware consent |

These refinements define the proposal's next milestone boundary, not observed guarantees or hardware approval. The original-only future power-limit setter, configured-limit getter and constraints API remain mandatory; no scope/v2, reset, command or privilege-escalation fallback is introduced.

## Next milestone: real Windows infrastructure, fake devices only

Status 2026-10-20: implemented and exercised end to end for the normal,
error-after-change and missing-candidate fake paths. Real fixed
guardian/worker/coordinator processes, authenticated pipes, protected test-root
storage and the reducer↔journal adapter reached explicit baseline restoration and
two separated verification reads. The external fake witness, worker-kill/blocked
call barriers and console-close delivery remain unimplemented; the harness still
refuses arming. Live publication now uses validated in-memory history plus exact
active/lineage checks, while cold startup retains the full scan; this asymmetry
requires architecture review. See [evidence](../../../docs/windows-recovery.md) and
[storage contract](../../../docs/windows-storage-v1.md).

- Guardian-first detached supervision must demonstrate survival of coordinator death, isolated console closure and coordinator job termination; unsupported job isolation refuses startup.

- Protected journal and exclusive lifetime ownership must use real file/ACL/IPC/process paths. Synthetic device state and the test witness survive worker termination so late fake effects remain observable.
- Authenticated private IPC, immutable expiry, bounded publication waits, serialized calls, cancellation, recovery failures and startup exclusion are tested through W01–W24 and inherited invariants.
- Preserve existing probe and simulator sources, behavior, fixtures, schemas and dependency separation. New infrastructure has no NVML loading, native setters, live option, arbitrary backend path, elevation, reset or workload path.
- All-call intent ordering, late completion, error-after-change, two distinct fresh synthetic baselines, exact readbacks, retained API failures and timing violations remain required.
- Test only isolated harness-owned processes/jobs/consoles and approved test directories. No user-console teardown, unrelated process termination, global clock/suspend changes, VM crash, reboot or physical power-loss test is implied.

## Required decisions before downstream work

| Item | Required disposition |
| --- | --- |
| Shared preparation/manifest/wire/journal/bootstrap schema and golden vectors | Resolve with one contract owner before independent units integrate; preserve stable AD IDs |
| Exact Windows build, native binding version/license/API coverage and toolchain | Select and verify before Windows implementation; no new package is selected by adoption |
| Ordered secure opening, ancestry protection, ACL/identity checks and handle rights | Review concrete mechanism before P1 acceptance; reject unprovable access rather than trust path strings |
| Clock tolerance, suspend observations and entry timing | Specify and test infrastructure behavior; retain native check-to-entry gap as G5 |
| Proposed publication/resource ceilings and common-root policy | Human review remains required; no production root is provisioned here |

## Proof limits and live gates

- P0 portable model evidence is not P1 Windows process-crash evidence. P1 cannot establish P2 OS-crash persistence or P3 hardware qualification.
- G1 guardian independence, G2 protected common ownership and G3 crash-consistent storage still need actual infrastructure evidence and stated environmental assumptions.
- G4 native call fencing and G5 strict physical entry timing are not proved by fake return/idle frames, process termination or timer polling. They remain live blockers even after fake-worker tests pass.
- G6 trusted exact human consent, binary measurement and external ownership, and G7 real privileges, fresh UUID/PCI/current/bounds, trustworthy setter/readback/restore semantics and approved manual recovery remain unresolved.
- Successful modern P0 offset reads do not prove writable support. Historical 240000 mW current and 215000 mW default are context only; any future trial captures fresh `B` and separately approves its exact target, duration, risks and recovery.
- No automatic journal replay, takeover, reset/reboot-as-restoration or recovery-only hardware action is authorized. An infeasible required safety gate means stop at fake-only infrastructure.
