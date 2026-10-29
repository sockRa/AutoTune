# Independent Sampler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Connect a bounded, independently supervised read-only sampler to the existing monitor and evidence interfaces.

**Architecture:** Typed canonical messages cross private bounded Windows pipes; a pure supervisor owns scheduling/stop decisions and a separate OS adapter owns handles and nonblocking observations. A serial child uses the existing selective NVML getter; no benchmark worker or compute is launched.

**Tech Stack:** Existing Rust 1.99.1 workspace, locked offline dependencies, windows-sys 0.61.2, existing probe/monitor/codec/profile.

**Spec:** `docs/superpowers/specs/2026-10-29-e3-independent-sampler-design.md`; original local integration/profile/live-schema contracts.

## Global constraints

- Owner approved the design and explicitly waived further routine questions on 2026-10-29: `ja sluta fråga mig ... non-stop ... slutföra roadmappen`.
- Execute inline in the existing checkout under the retained dirty-main preference. Preserve preceding modifications. The owner's later 2026-10-29 instruction authorizes stage/commit/push when a coherent roadmap milestone is completed and verified; no execution consent follows.
- Use CPU-only tests and compile-only native/process paths. Separate exact approval remains necessary for fake-process acceptance, NVML capture and GPU workloads.
- All operating values use `Profile::approved_design().limits`; no numeric policy revision or synthetic completion claim.
- Keep phase-1 codec/envelope bytes and converter/monitor/probe semantics unchanged, apart from exposing the existing clock validation type and adding matched description evidence.
- This checklist/execution log and ROADMAP.md are the only progress records. Store verification transcripts/source hashes under `target/sampler-verification/` (ignored build output).

## Review focus

- Wrong role/run/sequence/clock or repeated message must poison a channel without accepting a successor — Task 1.
- Oversize prefixes/escaped metadata and serializer temporaries must not bypass allocation accounting — Tasks 1/4.
- A pending getter or failed clock cannot freeze stop/escalation; late results cannot upgrade terminal state — Task 2.
- EOF/done/terminate success without retained-handle exit and job accounting must remain unknown — Tasks 2/3.
- Actual getter-start, original Ticket issue and controller receipt must remain distinct through export — Tasks 2/4.

### Task 1: typed wire and bounded frame channel

Files: create `src/sampler/{wire,wire_model}.rs` under the benchmark-worker crate;
register via existing `src/sampler.rs`; test `tests/sampler_protocol.rs`.

Interfaces: `wire::{Binding, Lane, Message, Frame, Channel, WireError}`.
Binding owns session/run/manifest/clock and original monitor Binding. Message is
Bootstrap/Ready/Query/MetricRecord/Stop/StopAck/RoleFinished with typed bodies;
Frame owns binding/lane/sequence/message. `Channel::new(binding,lane)`;
`encode(&mut self,message:&Message)->Result<Vec<u8>,WireError>`;
`decode(&mut self,frame:&[u8])->Result<Message,WireError>`. One Channel per
direction/lane; failed send is caller-fatal, decode/encode rejection poisons it.
Body schemas/key orders and independent golden frames live beside the model.

- [x] Write protocol tests for exact canonical frame/prefix and lossless extreme integers; round-trip all typed bodies and raw unavailable fields; reject malformed lengths, noncanonical/duplicate/unknown keys, wrong bindings/lanes/sequences, oversized metadata and invalid units/value contradictions.
- [x] Run explicit `cargo test -p autotune-benchmark-worker --test sampler_protocol --locked --offline`; observe missing API failure.
- [x] Implement frame/model parsing and encoding with preallocation byte checks, exact ordered key helpers, bounded canonical model complexity and no unchecked count-based allocation. Use existing converter to reject malformed raw fields.
- [x] Repeat protocol tests to green, then adjacent converter/monitor/evidence/envelope CPU targets.

### Task 2: supervisor and query-clock validation

Files: create `src/sampler/supervisor.rs`, test `tests/sampler_controller.rs`;
expose existing `monitor::clock::Clock`.

Interfaces: `Supervisor::new(binding,origin,manifest)->Result<Self,InitError>`;
`advance(anchor:Option<Anchor>,input:Input)->Vec<Effect>`; `terminal()` and
`report()`. Input = tick/typed frame/stop/transport failure/process observation;
Effect = send/signal stop/terminate/finalize. At most four effects per advance.
Process observations retain creation/wait/exit/termination/job facts separately.

- [x] Write injected tests for Ready gating, actual-start ordering, blocked pending query with continuing ticks, stop before Ready, boundary deadlines, clock failure immediate escalation, unknown exit and late supplemental evidence.
- [x] Observe missing supervisor API failure; implement using existing Controller and separate child Clock track. Source budgets from profile, preserve original epoch and first terminal outcome.
- [x] Run supervisor and adjacent protocol/converter/monitor tests to green.

### Task 3: matched sampler description and Windows containment

Files: `crates/autotune-probe/src/{nvml,nvml_tests}.rs`;
create `src/sampler/{child,windows}.rs`, fixed sampler and separately featured
fixture binary; tests `tests/sampler_process.rs` (ignored process acceptance).

Interfaces: `NvmlSampler::describe()->SamplerInfo` with raw UUID/PCI/runtime;
child serial getter loop consumes Message and emits Message; Windows process
adapter launches only the fixed measured binary, exposes nonblocking inbound
poll, bounded send, independent event stop, termination and retained-handle/job
observations. OS adapter consumes Task 1 wire and Task 2 observations.

- [x] Add ABI-fake description tests and compile-time/process-fake test cases (no execution); observe missing description failure using probe library tests only.
- [x] Implement description without another native loader or getter; build Windows private pipes/allowlist/job/binary holding, bounded I/O workers and event stop independent of pending getter.
- [x] Pass probe library tests, offline all-target compile/Clippy. Process fixtures stay ignored, production binaries unexecuted.

### Task 4: capture binding, evidence and fixed controller

Files: create `src/sampler/{manifest,producer}.rs`, fixed controller binary,
`tests/sampler_evidence.rs`; update worker Cargo.toml/lib and locked local edges only.

Interfaces: validated immutable CaptureManifest owns the design's exact identity,
artifact/profile/protocol/approval/output-leaf scope; producer consumes bound
supervisor/raw observations and emits existing Envelope plus fixed artifact index.
Global budget charges all owned/encoded temporary data before construction.

- [x] Write evidence tests for bounded manifest, sampler-only blocked/invalid exports, actual getter starts, distinct metric-slot counts, incomplete writer/index and simulated provenance.
- [x] Observe missing API failure, implement producer and fixed controller integration with no compute admission/native execution.
- [x] Pass focused CPU export tests and compile-only binaries.

### Task 5: integrated verification and review

- [x] Run installed Rust fmt; explicit CPU protocol/controller/evidence/converter/monitor/codec/envelope/worker and probe/reference tests; compile/link all workspace targets without execution; warning-denied Clippy for affected targets/features.
- [x] Retain command exit codes, source baseline/full working-tree hashes and logs. Do not claim any unexecuted host/hardware check.
- [x] Independent fresh whole-change review; correct material findings and verify regressions at authorized seams, explicitly retaining any native reproduction gap. Update shared docs/ROADMAP.md and this log with exact limits and next execution boundary.

## Execution log

- Implementation milestone closeout (latest): Tasks 1–5 are complete for the software-only design scope, not executed Windows/native qualification. Full code-review skill Standards and Spec axes ran independently. Standards: zero hard violations, one optional low duplicated-resource-observation smell, not a correctness blocker. Spec: P1 failed-launch handle/cleanup loss and P2 storage-failure/cancellation evidence loss were fixed and re-reviewed without remaining material findings. Follow-up review confirmed failed-launch I/O cancellation snapshots and the corrected ordering of stop effects before supplemental evidence. Actual Win32 failure/handle ownership has no executed CPU seam: CPU regressions verify honest projection/retention only; original-handle native behavior remains a separately gated fixture acceptance requirement, not a fabricated red/green claim.
- Final source-bound checks: 217 CPU-only tests (163 integration, 19 probe ABI fakes, 10 feature-library fakes, 25 reference/prepared); scoped all-target/all-feature warning-denied Clippy, workspace formatting and workspace all-target/all-feature compile/link pass. Matching inventories/logs/artifact hashes: `target/sampler-verification/cleanup-review-manifest.json`, SHA-256 `083087660673f66d1ee4107b2efc63dd2dd9b0ea48d5e56b54bb0c4cc6ed30e1`. Earlier Clippy drafts failed on redundant generated fields/closure and nested match; corrected, with logs retained. Full 4 MiB metadata ceiling now has shared-ingress/reserved-ownership coverage; removed unused `wire::Queues` instead of counting a duplicate model as production coverage.
- Failure evidence closeout: post-spawn errors retain original process/job handles, original failure/assignment/optional creation/termination facts and four I/O shutdown observations; controller observes/retries the original scope through frozen bounds. Final-drain errors and later worker completion observations survive raw export. Writer initialization/export/enqueue/finish/deadline errors preserve typed failure plus cancellation/completion in bounded manifest-bound `IncompleteCapture`; optional raw serialization starts only after diagnostic metadata is reserved. A broken/blocked sink cannot promise durable diagnostics; unavailable raw/unfinished threads remain explicit. One CPU test triggers sink failure between poll and enqueue and retains its original OS code; another verifies no raw allocation attempt when diagnostic capacity is unavailable.
- Publication: owner authorized milestone commits/pushes. E1 maintenance separately committed as `17b142f`; sampler milestone publication follows with its revision and hosted result in ROADMAP.md. No sampler process fixture, real clock CLI, NVML, native GPU or workload executed. Next: source/artifact-bound process acceptance matrix and separate exact read-only consent manifest, then remaining worker/device/consent chain. Do not restart this software slice or imply E3/Gate B completion.
- Latest audit continuation: previous turn made progress; current turn reproduced/fixed terminal-channel gaps on framing/OS and codec-capacity failure, with failing ingress regressions then green. Final export now requires explicit exact manifest/profile/raw byte accounting, with a failing regression then green; session/index remain separately covered by the archive index. Original phase-1/envelope bytes and all approved numbers are unchanged.
- Aggregate-metadata diagnosis: the preceding full CPU log had an unresolved failure, reproduced with one exact test. Ranked causes were shared RAM exhaustion, frame bound, or dropped metadata marker. A temporary tagged probe measured 48 retained records, 33511072 charged RAM bytes and 17564928 monitor bytes: shared RAM fills before the 4 MiB metadata cap. Corrected the fixture to assert that earlier ceiling, sticky retention and no eviction; removed instrumentation. This is not proof of the independent metadata boundary. No production limit was raised and no test assertion was weakened to claim metadata acceptance.
- Current source-bound audit: `target/sampler-verification/sampler-audit-manifest.json`, SHA-256 `2284835547dff6906de122cbf5788cc7851e01f7bdfe9cf86465976866314e73`, binds unchanged before/after source inventories, baseline `eb8cdf51bdb4c55b6a272fe7b9004d99b26d6a2b`, seven successful commands and artifact/log hashes: 214 CPU tests (160 integration + 19 probe + 10 feature-library fakes + 25 reference/prepared), scoped all-feature Clippy, workspace formatting and all-workspace/all-feature compile/link. Windows fixture source now has five ignored cases, including three shared wire-engine scenarios; none ran. Whole-change review/cleanup audit and separate execution acceptance remain open. No commit/push yet; publish completed milestones under the owner's new instruction.
- Baseline `eb8cdf51bdb4c55b6a272fe7b9004d99b26d6a2b` plus preceding dirty bridge/cleanup/design; no competing status ledger.
- Continuation classification: the preceding goal turn made concrete source/test/documentation progress; it was not a wait or a no-progress turn. This continuation implements the fixed controller and writer toward the same full roadmap objective.
- Ruling: owner explicitly opts out of routine questions; design and inline execution accepted. Continue without another plan-approval prompt; exact execution consent contracts remain separate.
- Ruling: continue dirty main per recorded owner preference rather than recreating removed automation worktrees. Cost is mixed local changes; preserve baseline/source inventories and avoid commits.
- Ruling: use this checklist/log instead of Bash-specific SDD scaffolding on Windows and use ignored target output for command evidence. Cost is no compatibility with skill helper scripts; durable shared checklist remains available.
- Pre-flight: Task 1 typed messages feed Task 2 supervisor and Task 3 OS/child; Task 2 observations feed Task 4 producer. Task 3 description uses existing probe runtime type. Keep existing sampler converter exports at their current paths.
- Task 1 implementation/checks: typed canonical wire, exact binding/device/frequency/sequence validation, prefix-before-allocation, bounded complexity, ordinary/control queues and independent complete query golden vector are implemented. Missing-API RED, then 8 protocol tests GREEN. Whole-change review remains pending.
- Task 2 implementation/checks: supervisor composes the existing monitor/clock/converter; raw actual-start/receipt retention, one pending getter, sticky stop, independent missing-clock termination and explicit observation-expired input are implemented. Missing API RED, then 10 supervisor tests GREEN. Two behavioral RED regressions exposed late exit upgrade and missing-clock finalization; both pass after correction. A producer regression also prevents same-timestamp late lifecycle evidence from becoming primary launch/readiness.
- Task 3 in progress: `SamplerInfo` and Windows-only `NvmlSampler::describe` retain exact selected UUID/PCI and reuse the existing runtime getters (19 probe-library ABI-fake tests GREEN). New Windows sampler containment/private pipes/dedicated I/O threads and fixed native/featured-fake child sources compile/link; child gate has 2 CPU tests GREEN. Process acceptance was compiled only and remains ignored/unexecuted. Production controller, cleanup/transport finalization and final integration review remain unfinished.
- Task 4 in progress: immutable sampler-only manifest/profile encoding (2 CPU tests), shared/partitioned budget (4 CPU tests) and derived envelope/raw export (3 CPU tests) are implemented drafts. Fixed controller/artifact writer/index, source/protocol execution binding and full resource/serialization accounting still need integration and review. Do not claim a runnable qualified capture or complete bounded live chain.
- Ruling: add the already-locked `serde_json` package as a direct worker dependency to retain existing runtime-report bytes in Ready; lockfile changes only its local dependency edge, no new package/version/download. Cost is another explicit local dependency edge, not a new wire parser.
- Ruling: expose the existing `Clock` validator without changing its policy; use independent child-anchor tracking and original controller receipt. Cost is an expanded public validation API, not native admission.
- Ruling: keep Input/Effect message variants inline under scoped Clippy large-enum allowances; trace preallocation accounts their full size and effects remain bounded to four. Cost is larger stack values in exchange for avoiding uncharged per-event boxes.
- Ruling: separate root-budget partitions keep control frames/codec capacity reserved, and their parent charge outlives detached workers/frames. Supervisor and transport share the ordinary budget. Full end-to-end allocation/serialization audit remains required, including transfers of decoded messages and final writer output; CPU ledger tests alone do not prove all application allocations are charged.
- Environment findings: plain Cargo lacked PATH/dlltool support; the installed llvm-mingw toolchain from earlier project verification fixes linking, without installation. Explicit `+1.99.1` lacks rustfmt/Clippy components; existing `+stable` is also rustc 1.99.1 and provides them. `target/sampler-verification/Invoke-Check.ps1` records toolchain, arguments, exit and log hashes. The first description test draft incorrectly assumed mutex fixtures; corrected to the existing thread-local SCRIPT before the clean missing-method RED. Initial bad draft remains in the retained failed log.
- Verification scope: explicit CPU targets only; all-target/all-feature compile-only linking has passed at the preceding draft source. Formatting/Clippy/test evidence is refreshed at the checkpoint manifest; no unrecorded current-source full-workspace runtime claim. One Clippy draft failed on inline enums and constant chunks; scoped rationale and as_chunks correction passed the subsequent check. A warning from the producer reservation field was resolved by naming its retained ownership explicitly.
- Next exact work: complete `child_windows` cleanup/RoleFinished and transport/decoded-message ownership; implement fixed controller with exact manifest/artifact execution binding, independent shutdown watchdog/resource observations and bounded off-path artifact writer/index; finish producer allocation/raw-status/byte-accounting audit. Then run integrated verification and fresh whole-change review. No design/plan approval question is pending.
- Källbundet mellankontrollsteg: 177 selected CPU tests pass at unchanged before/after source inventories: 123 worker integration, 19 probe library, 25 reference/prepared, 10 feature-library typed/static/clock fakes. Scoped all-target/all-feature warning-denied Clippy, workspace fmt check and all-workspace/all-target/all-feature linking pass. Manifest `target/sampler-verification/checkpoint-manifest.json`, SHA-256 `5882fe35b0dba51ea488dd1a65297f3e3f6d20e40c2e8b664bbee9f1b27e883c`, binds baseline, all source hashes and seven successful command/log records. Earlier RED/environment/lint failures remain in command history. No final review or native/process execution follows from this checkpoint. Earlier commentary's 121-test count was a one-test arithmetic error; the preceding focused command had 120, and this expanded checkpoint has 177.
- Tasks 3/4 source continuation: fixed controller/runtime resource observations and exact invocation binding, asynchronous measured launch (shared ownership of one noninherited kernel job handle), bounded off-path directory setup/writer/archive/index, decoded-message ownership transfer, sticky child StopAck/RoleFinished state and bounded cleanup send-drain source are implemented. Writer tests genuinely block an injected storage sink while enqueue/poll remain responsive; no sleeps/native clocks are used. Two process acceptance tests remain ignored/compile-only. No production/fake sampler/controller executable was run.
- Ruling: use `shutdown_reserve_ns` (8 s) for the sampler-only closure deadline and trace reserve, not the separate `finalization_ns` (10 s) phase field. The watchdog test first failed at the 8 s boundary and caught the wrong profile-field use; fixed without changing any approved number. Cost if wrong is an overly early invalid/incomplete result, never an extended workload window.
- Ruling: move sampler image hashing/launch preparation off the deadline thread. `SamplerLaunch` holds the same single kernel job handle through Arc ownership wholly inside the controller process, with cancellation checks before creation/assignment/resume. No child inherits the job. Cost if wrong is unresolved startup/termination; real Windows acceptance is still required.
- Full injected capture RED/GREEN: a 10 s logical capture with 1001 host ticks, six metric schedules, query-send outcomes and observed fixture exit reached preflight but raw export failed Capacity. The old 2048-byte-per-event whole-output preallocation overestimated actual output beyond the writer budget. RawBuilder now charges real capacity growth, including concurrent old/new allocations during reallocation. The unchanged-budget complete fixture passes and exports a blocked simulated envelope, preserving expected 246 scheduled metric slots without padding. This is not a 10 s native run or a useful baseline.
- Source-bound runtime checkpoint: 190 selected CPU tests (136 integration, 19 probe, 25 reference/prepared, 10 feature-library fakes), affected-package all-target/all-feature warning-denied Clippy, workspace fmt and workspace all-target/all-feature linking pass. Unchanged before/after source hashes and seven command/log records: `target/sampler-verification/runtime-manifest.json`, SHA-256 `b12419d9b2a7952e6cfde7e5471f9c8e7d8e5f103fae359b15f8f7c22532ead4`. The archive golden SHA literal draft was corrected against independently computed .NET SHA256; its failed draft remains logged. No final review or native/process/hardware acceptance claim.
- Remaining exact audit work: (1) retain rejected/truncated prefix/body bytes and transport failure evidence rather than only a generic failure code; drain queued late records/EOF before finalizing, without extending original bounds. (2) retain each I/O worker's finished/cancel status; controller currently discards the finished bool. (3) replace the envelope's placeholder `observed_bytes=0` with exact declared byte scope and audit invalid timestamp projection and oversized rejected unit metadata. (4) audit launch failure/termination raw statuses, repeated watchdog force requests, all metadata/codec transfers and bounded effect routing. (5) current fixed fake child covers event hang/force only; implement the shared protocol engine's fake backend/controller matrix needed to observe actual independent ticks/blocked getters/IPC/export under separately approved Windows execution. (6) remove or integrate any unused pure queue duplication, then fresh whole-change review and source-bound verification. These are engineering implementation gaps, not owner-design questions or permission to run native code.
