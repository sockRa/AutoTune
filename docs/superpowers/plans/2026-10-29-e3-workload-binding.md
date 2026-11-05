# E3 workload manifest and Start binding

Engineering continuation for package 2, E3.1/E3.2/E4.1, under the owner's
continuous software-implementation direction. Read ROADMAP.md and the retained
first-benchmark integration, qualification-profile and live-evidence contracts.
Those contracts and all approved numeric values remain unchanged.

The next integrated source boundary is a separate pilot workload manifest and
worker Start vocabulary. It must not turn the readonly sampler manifest into
workload consent or weaken the existing simulated worker's native rejection.
Actual sampler fixtures, native loads and GPU execution retain their separate
execution approvals; the previously requested fake-only matrix is still pending.

## Initial implementation boundary

- Immutable pilot declaration: exact nonzero session and three independent run
  IDs, original clock token, exact raw NVML UUID/PCI, source commit/inventory,
  lockfile/controller/worker/sampler/PTX/protocol digests, approval reference,
  fixed output root, and explicit digests of starting-settings/environment
  evidence. The latter bind caller-declared artifacts, not their truth or the
  sufficiency of required facts. No launch/admission or baseline verdict follows.
- Serialize exactly the approved three pilot RunPlans, preserving seeds 0/1/2,
  shape/K, warm-up/measured counts and phase/envelope caps. Acceptance repeats
  require a separately qualified pilot and are not admitted by this manifest.
- Reuse the existing canonical codec and digest domains. Pilot profile bytes
  carry their own scope plus the unchanged numeric limits and three RunPlans;
  readonly sampler profile bytes must remain byte-identical.
- Reserve bounded manifest/codec storage before owned model construction;
  reject large strings/capacities and oversized/canonical/schema contradictions.
  Private validated fields prevent caller mutation after freezing.
- A bound run declaration keeps the original manifest/clock/device/artifact
  identity and run origin. It computes phase deadlines through the existing
  approved RunPlan, without rebasing or granting a Start permission. The later
  worker protocol/controller must validate real readiness, safety and consent.

Fixed manifest key order: version, scope, session, runs, clock_token, raw_uuid,
raw_pci, source_commit, source_tree, lockfile, controller, worker, sampler, ptx,
protocol, profile, starting_evidence, environment_evidence, approval_ref,
output_root. RunPlan keys: run, seed, lanes, steps, warmup_dispatches,
measured_dispatches, envelope_ns, phase_caps_ns. Digests bind canonical bytes;
there is no approved=true or native execution token.

## Tasks and evidence

### Worker handshake continuation

The next source slice implements real encoded worker-role frames and the child
handshake state boundary; OS transport/dispatch composition follows. It does
not add a native executable or a native admission token. Bootstrap/Ready/Start
are ordinary-lane messages with explicit controller-to-worker or reverse
direction; Stop/StopAck/RoleFinished use the reserved control lane. Each lane
and direction owns a sequence. Any decode/encode/transport-capacity rejection
poisons that channel; no successor recovery. Encoded frames/decoded message
ownership retain shared RAM charges until dropped/transferred.

Frame keys: version, role, lane, direction, sequence, binding, kind, body.
Binding includes session/run/device, manifest/profile/worker/sampler digests,
original clock epoch/token/frequency, run origin and the frozen full RunPlan.
Bootstrap carries exact manifest bytes; Ready carries observed identity/context,
image digest, driver/capability/prepared-lane assertions and a raw anchor. These
are self-reports, not physical mapping/execution proof. Start carries its issued
anchor and can occur once after a matching Ready. Stop carries its first reason
and trigger/cooperative/exit times capped at original run/session ceilings.
RoleFinished records cleanup attempted/raw return/unknown, never OS exit.

The handshake gate retains original-epoch ordering and first terminal state;
duplicate Start, premature Start, invalid clocks or late Start close admission.
Its Start intent uses a conservative warm-up bound from controller issue time,
including transport delay, capped by the existing cumulative phase boundary.
It never extends any approved phase cap or truncates required counts. Parent
safety/re-entry/required-fact/consent validation remains separately required;
gate acceptance alone is not native execution permission. Dispatch records,
failure-vector transport, independent process supervision and parent controller
composition are still required before a runnable benchmark can be claimed.

- [x] Implement bounded immutable pilot declaration and exact profile/plan
  encoding, with public API tests for source/artifact/device/run binding,
  budget exhaustion, tampering and unchanged readonly profile bytes.
- [x] Add strict reading and original-epoch bound-run phase calculations;
  round-trip exact declared plans and reject scope/plan alterations.
- [ ] Continue to actual worker Start/Ready/Dispatch transport and controller
  composition; keep native admission unavailable until prerequisites exist.
- [ ] Independent review and source-bound CPU/compile checks at a coherent
  implementation milestone; update ROADMAP.md and this log without claiming
  process execution, real correctness or useful-baseline acceptance.

## Execution log

- 2026-11-04 worker entry/finalization composition: the opt-in worker_entry::entry
  now adopts parsed fixed worker Arguments once through Context, starts the split
  independent control owner and freezes bootstrap setup before composing actual
  observed CUDA preparation, Ready/encoded Start, a required future admission
  callback, producer/failure-vector export, explicit cleanup, control completion,
  bounded output/control drain and separate I/O cancellation/completion. No main
  invokes it and no NativeAdmission issuer is added. Unsafe exact setup consent
  remains external and must hold before invocation; the callback after Start
  cannot authorize earlier setup retroactively. Outcome retains Context, startup
  and preparation failures, raw split/sender/event-clone causes, rejected owners,
  admission reason, control report, native owners and finalization/I/O evidence.
  A continuous ClockChain spans all main-thread captures without a new epoch;
  WindowsClock clone copies immutable token/frequency only. worker_finish::State
  uses the already frozen producer exit or original Shared window and cannot
  manufacture new grace. Cleanup result/validation, completion queue receipt,
  output/control drain, cancellation result and actual I/O-thread completion are
  separate. Late supplemental completion cannot clear timeout, and unknown host
  pending cannot pass completion. I/O polling observes retained JoinHandles only,
  with no cancellation retry or blocking join. Nine CPU cases cover original
  deadlines, truthful native/validation/I/O status, ordering, retained first errors,
  budget/binding rejection, continuous-clock faults and actual injected-producer
  cleanup. A tenth feature-library test forces entry monomorphization/linking
  through black_box of its function pointer and NEVER invokes it. Initial Result
  API and test-module-placement errors were fixed; review also distinguished
  missing deadline from missing clock. Final source-bound evidence/publication:
  ROADMAP.md; earlier verification attempts remain separate. No real entry,
  endpoint adoption, control/I/O threads, DLL/native/GPU execution or independent
  review. Denial/failure before producer Start retains the prepared or partial
  native owner and reports absent cleanup; speculative pre-Start teardown is not
  introduced or qualified. Before wiring a fixed executable, complete parent
  facts/consent and pre-setup authority, fresh Start/safety/re-entry and admission
  issuance, pre-Start resource disposition, aggregate raw export and real process/
  native qualification. Full automatic benchmark/search/validation remains the
  goal; E3 partial/Gate B unmet. Actions remains removed and disabled.

- 2026-11-04 original shared stop/drain and concrete receive: Shared now retains
  the actual original trigger separately from its clipped raw cooperative cap.
  State, native/local stop and setup expiry publish one monotonic earliest
  trigger; full binding and unresolved/invalid control remain explicit. Producer
  polls that exact original window directly, alongside injected control, and
  retains each earlier observed window in eight prepaid inline slots plus an
  overflow owner. Later stops cannot renew the earliest window. A known shared
  stop now permits only the existing submitted dispatch's finite query/readback/
  check/hash under original stream/cycle/phase/cooperative ceilings, then terminal
  stopped reporting and explicit teardown; unknown/invalid control still freezes
  native progression. History exhaustion stops without eviction. CallTrace now
  marks every native host entry/return in the same serial Shared gate. Host-call
  pending and GPU-stream pending remain separate; only query/readback can use
  begin_drain after stop, and unmatched returns never clear unknown pending.
  Native raw returns are retained before pending completion/post-clock checking.
  Session owns its ordinary decoder and concrete Windows poll_native receive
  bridge, retaining OS error/EOF/idle/raw packet and original clock facts. Idle
  polls enforce the frozen setup deadline, and bracketed Start receipt is consumed
  once rather than treated as a second overlapping clock observation. Thirteen
  added CPU cases cover clipped/nonzero-epoch windows, earlier/later triggers,
  completed/pending drain, original expiry/no query, bounded history, actual
  encoded raw reception, missing/regressing clocks, OS/EOF, host-vs-stream pending
  and unresolved/mismatched calls. Receive history is prepaid for two observations
  per approved control tick across the full original run envelope; a complete
  simulated run retains constant charged memory and expires at the original end.
  Review corrected its initial setup-only capacity before the final manifest;
  earlier worker-drain-final evidence remains separately retained. Initial Clippy
  caught a collapsible condition;
  corrected before final checks. Exact final verification/publication: ROADMAP.md.
  A focused name-filtered command ran runtime tests but filtered control tests;
  the later explicit control/trace command ran all of those cases. Native pipe
  polling is compile/link only; no actual pipe/process/native/GPU execution or
  independent-agent review. Fixed executable/Context/control/native composition,
  final output/control/I/O drain, aggregate raw export, complete parent facts/
  consent, workload admission issuance and real qualification remain open. No
  reset, replacement, retry or physical-quiescence claim. Actions stays disabled.

- 2026-11-04 worker preparation/Ready/Start composition: worker_runtime::Session
  now composes the original bootstrap Gate, exact frozen Shared setup binding,
  retained producer preparation, fresh Ready, one output sequence and strict
  owned encoded Start. Ready and all subsequent producer reports share the same
  Channel; failed handoff retains the original frame and forbids retry. Original
  setup observations, late receipts and clock faults remain in a prepaid bounded
  EntryClock. Native setup's final anchor can be inherited without resetting the
  continuous producer clock. Peer-issued Start is checked against the original
  epoch separately from the worker's later post-Ready observation, preserving a
  legitimate cross-process issue/queue-return race without clock rewinding.
  Producer::prepare_retained returns the original driver and both callback owners
  on every recoverable failure; the actual preparation path uses it. The opt-in
  worker_native::prepare now checks worker role/full bootstrap/shared deadline
  before loading, composes observed CUDA open with Session preparation, and
  retains SetupFailure or preparation owners and the raw setup ledger. Successful
  CUDA capability/context checks supply Ready facts; no compute or consent is
  implied. Shared stop now also closes the per-call producer compute trace.
  Nine CPU cases cover the encoded Ready/first report sequence, racing Start,
  Ready-send failure/no retry, late raw receipt/equality, invalid/duplicate Start,
  inherited-clock regression, stop, returned owner/budget failures and native
  fixture rejection. Initial compile corrected Ready capability integer widths;
  the first sequence test incorrectly expected a warm-up PhaseEntered, corrected
  to the existing protocol's Ready then DispatchRecord. Final verification and
  publication: ROADMAP.md. Native setup composition is compile/link only, with
  no loader/process/GPU execution or independent-agent review. Actual executable
  adoption, concrete pipe/control event loop and controlled in-flight stop/drain,
  aggregate raw export, complete parent facts/consent, admission issuance and
  qualification remain open. A shared stop currently freezes this Session without
  entering more native calls and retains any in-flight producer uncertainty; it
  is not an executed stop/drain result or a complete worker entry. Full automatic
  benchmark/search/validation remains the goal. Actions stays removed/disabled.

- 2026-11-04 producer-bound teardown: Driver close now requires a portable
  per-call observer, and the actual CUDA Driver bridge delegates to
  close_observed. Producer cleanup owns eight inline prepaid raw entries and
  uses its existing continuous Context clock, already frozen exit deadline and
  original stop window for every release and final observation. No new clock
  epoch, allocation or grace is introduced at cleanup entry. Actual close result
  stays separate from clock/deadline/protocol validation; even returned success
  cannot pass without the required observed releases. CUDA and OS library
  outcomes, pending/mismatched returns and overflow stay distinct and retained.
  Optional exact bind_control attaches the independent Shared state before
  Start; native Start requires this state and completed setup, but still has no
  workload NativeAdmission issuer. Cleanup closes the work lane, checks its raw
  cooperative cap at every boundary and serializes releases with begin_cleanup/
  complete. Existing unresolved shared pending state is neither cleared nor
  bypassed. Unwind retains raw/pending evidence, marks observer uncertainty and
  the producer's one-shot attempt prevents retry. Six new CPU cases cover all
  release/final clock boundaries against original exit/cooperative caps, every
  CUDA/library failure, unobserved-success rejection, mismatch/panic, independent
  shared expiry without mailbox delivery, unresolved pending and rebind rejection.
  Actual CUDA Driver forwarding is checked by the feature-library ABI fake.
  Initial focused compile caught the old one-argument Driver close test and was
  corrected; focused tests and Clippy then passed. Exact final verification and
  publication are recorded in ROADMAP.md. Actual worker entry must still bind
  setup/control/preparation/Ready/Start and driver dispatch admission, finish
  required parent facts/consent, export raw owners and qualify real execution.
  No DLL/native/GPU/owned-process execution; Actions stays removed/disabled.

- 2026-11-04 bound CUDA setup observation: `cuda_setup_trace::Ledger` now
  implements the actual adapter's SetupObserver against the full worker binding,
  original shared epoch/frequency, control state's already frozen setup deadline
  and serial pending/stop gate. It reserves all 146 possible ABI-call entries
  before startup (64-device maximum), retains each attempted call/raw return and
  rejected/overflow evidence, and closes successors on clock/control/native/
  protocol/capacity failure. Raw return ownership precedes the post-clock hook;
  unwind closes admission without clearing unresolved native pending state.
  A late setup return now shares `Shared::expire_setup` with Ready completion,
  tightening grace from the original deadline even before control next ticks.
  No synthetic clock anchor or late-return-based new grace is introduced.
  Eight typed-fake tests compose the real setup engine and bound observer,
  including all 40 stop/missing-clock boundaries, the full 64-device ledger,
  every native failure, equality/late returns, nonzero epoch, raw-return panic,
  mismatched pending return, capacity/regression/completed-state and constructor
  budget/binding rejection. Initial checks caught observer Drop borrow lifetimes
  and a div_ceil test lint; both were corrected before final verification.
  Exact source-bound verification/publication is recorded in ROADMAP.md.
  This does not compose loader ownership/export or the actual executable entry,
  Ready, preparation, parent facts/consent, Start or native-admission issuance.
  Next bind explicit teardown to the producer's already frozen exit deadline
  and shared cooperative cap, then compose the worker path. No DLL/native/GPU
  or owned-process execution; Actions remains removed and disabled.

- 2026-11-04 observed CUDA teardown: `close_observed` now exposes a before/return
  pair around stream destroy, each of four buffer frees, module unload, context
  destroy and the consuming library close. CUDA status and OS library result/
  optional code are distinct types; no CUDA zero is fabricated for FreeLibrary.
  A successful native release clears its stored token before a post-clock hook;
  any observer/native/library failure leaves the session poisoned and retains
  remaining/uncertain tokens with no retry. Failed DLL close reconstructs only
  the retained opaque owner, without a second OS call or a validity claim.
  Legacy close uses the same path with an unobserved observer. Ready/Complete
  remain prerequisites; in-flight, failed and already-closed states never enter
  teardown. Four ABI-fake cases cover all eight pre/post boundaries, every CUDA
  cleanup failure, library failure with known/unknown OS code, cleared/retained
  tokens, rejected retry, no implicit Drop calls and normal observed-idle close.
  The fixture HMODULE compile mismatch was corrected to the installed library's
  isize handle type. No actual DLL load/release, CUDA/process/GPU execution or
  independent-agent review is claimed. Final source-bound evidence/publication:
  ROADMAP.md. Next bind startup/teardown observers to original worker clocks,
  shared control and the already frozen producer exit deadline; cleanup must
  not acquire a fresh grace window on entry. The producer's direct Driver close
  still uses the unobserved compatibility path until that composition exists.
  Actual worker entry, full required facts/consent, admission and aggregate live
  export/qualification remain open. Actions stays removed and disabled.

- 2026-11-04 observed CUDA startup and retained failure owner: the opt-in actual
  CUDA adapter now supplies `open_observed` with per-call pre/return callbacks
  for initialization/version, full unique UUID/PCI/capability selection, context
  device verification, fixed PTX/function loading, headroom, four buffers and
  stream creation. Status and integer/UUID/memory outputs or opaque-token
  presence reach the observer before post-clock validation. Any observer/native
  failure aborts before a successor call. The original backend/library and raw
  nonzero resource tokens remain privately owned by SetupFailure, including
  failures after successful context/allocation creation. There is no automatic
  cleanup or safe-retry inference; nonzero failed outputs do not prove valid
  resources. Legacy unsafe open delegates to this same setup with an unobserved
  observer, retaining its old execution/consent restrictions. Four ABI-fake tests
  cover success order, every pre/post rejection over all 20 one-device calls,
  signed native failure at every step through the actual owner-transfer path,
  ambiguity/count/context/headroom rejection and no implicit teardown/compute.
  The initial Clippy large-error lint is explicitly allowed to avoid fallible
  allocation after native entry. Final evidence/publication is in ROADMAP.md.
  No loader, driver, process or GPU execution or independent-agent review is
  claimed. A bound clock/control setup observer, observed teardown and actual
  worker entry/complete required environment/consent facts still must compose
  this source before native execution. Workload NativeAdmission has no issuer.
  GitHub Actions stays removed and disabled; local verification only.

- 2026-11-04 strict parent response reader: `starting_read::Receipt` now checks
  the canonical exporter schema, exact parent scope/declaration/digest, original
  epoch/frequency and parent-entry/child-entry/bootstrap-receipt clock chain.
  Serialized anchor tokens are restored from the frozen scope, not zero-filled
  in exposed facts. The immutable 10 s read deadline is distinct from chunk
  arrival allowance; equality rejects. Fixed ABI raw widths/buffer shapes,
  operation order, enum spellings and nested runtime schema are checked. A wider
  bounded structure scanner supports this 2 MiB response while old wire limits
  remain unchanged. Codec scratch is reserved before decoding; embedded runtime
  complexity is bounded before serde allocation. The nested Snapshot is read
  through the existing strict ReportedSnapshot parser, source/digest/request
  consistency is checked and the resulting reported artifact remains owned for
  later reference/readability checks. Raw failed facts, absence, bad readings,
  simulated source and reported native claims are not promoted to qualification.
  Session::read_report is available only after verified chunk delivery and
  rechecks its immutable digest; failed or incomplete reception exposes no report.
  Nine added CPU cases exercise actual exporter/reader/session composition,
  bound/tampered/nested schemas, absent/raw failure facts, max 3,077-call ledger,
  memory/complexity bounds and nonzero-epoch/deadline equality. Initial compile
  caught the old u16 complexity-counter width; the widened implementation keeps
  existing u16 caller bounds and adds the response's explicit wider bound.
  Exact final evidence is retained in ROADMAP.md. No process/native/GPU execution
  or independent-agent review is claimed. Hosted d2e9cae run 41883529717 passed
  both platforms before the owner's explicit retirement of all Actions workflows
  in 0b342c7; do not recreate workflows or expect hosted checks on later pushes.

- 2026-11-04 hosted Linux gate failure for `c17ac64` and its correction: hosted
  [run 41882617874](https://github.com/sockRa/AutoTune/actions/runs/41882617874)
  completed **failed** on the Linux job: warning-denied Clippy rejected the unused
  `Anchor` import and the never-read `Session::clock` field on non-Windows targets
  (the same two cross-target warnings the isolated Linux `cargo check` had shown).
  The correction is `Session::observe`, which makes the original-epoch conversion
  portable and is contained in commit `d2e9cae`; that commit needs its own observed
  hosted run. The failure stays recorded and is not rewritten as a pass.

- 2026-11-04 parent-side starting launch handoff and portable epoch accessor:
  `workload::starting_launch::Handoff` (Windows only) composes the session, the
  frozen encoded starting frame and the one-shot starting sender: `send` makes at
  most one attempt, re-stores the exact unsent frame and first `SendFailure` on
  failure, and never re-acquires a sender; `poll` delegates to
  `Session::poll_process` so deadline, epoch conversion and sticky faults stay
  the session's. `Session::observe` exposes the original-epoch anchor conversion
  portably, returning `None` for a mismatched token, regression or missing
  observation instead of a restarted base, which removes the cross-target
  dead-code warnings without a target-specific allow. Three compile-only cases
  cover the handoff and session driver wiring; one portable case covers anchor
  conversion and rejection. No process is created, no sender touches a real pipe
  and no native entry, GPU execution or live qualification occurs. The initial
  slice stays recorded as `starting-handoff1` (482 tests). CPU-only verification
  binds baseline `c17ac64` plus unchanged before/after source inventories:
  **483 tests** (402 integration across 40 test binaries including 21
  reception/session/handoff cases, 32 probe ABI fakes, 24 worker feature-library
  fakes/static, 25 reference/prepared), formatting, warning-denied scoped Clippy
  and full-workspace/all-target/all-feature compile/link. Manifest
  `target/workload-verification/starting-handoff2-manifest.json`, SHA-256
  `add9dab0777591e4dcc149343514ca153a1118dcabc12abc68c28eb3c5b0321e`;
  cross-target `x86_64-unknown-linux-gnu` workspace/all-target check passes
  warning-free (`target/workload-verification/starting-handoff2-linux-manual.log`,
  isolated `target/linux-check` root). Both pinned matrix images remain
  byte-identical. No launched process, native entry, GPU execution or live
  qualification is claimed; the strict parent-side starting-response reader, the
  child worker entry/driver/setup, complete consent/environment facts, the
  Start/native admission issuer and aggregate live export remain open.

- 2026-11-04 parent-side starting session and original-epoch clock conversion:
  `workload::starting_session::Session` composes one started reception under the
  parent's original epoch. It binds the declaration/scope before any chunk,
  reserves the receiver capacity in the constructor, derives the arrival bound
  from the approved profile (preflight plus cooperative-stop and
  exit-observation allowances, capped by the pilot session) and exposes the
  launched process only by reference, so no process, native admission or
  qualification is created here. `poll_native` now converts the observed anchor
  to elapsed nanoseconds before any deadline/clock check (a raw QPC reading is no
  longer compared against an elapsed bound) and keeps lane/framing rejection, a
  closed peer and an OS error distinct from the receiver's sticky fault.
  `Session::settle` merges those supervision facts: a verified completion is
  checked first and cannot be withdrawn by a later transport observation, the
  first OS cause is retained, and a closed peer without completion becomes an
  explicit stopped outcome. Six added cases cover the verified payload/outcome,
  transport-fault retention without payload exposure, closed-peer stop,
  idle-deadline/missing-clock faults, scope/capacity rejection before reception
  and Windows compile-only driver wiring. CPU-only verification binds baseline
  `b8bb700` plus unchanged before/after source inventories: **481 tests** (400
  integration across 40 test binaries including 19 reception/session cases, 32
  probe ABI fakes, 24 worker feature-library fakes/static, 25 reference/prepared),
  formatting, warning-denied scoped Clippy and full-workspace/all-target/
  all-feature compile/link. Manifest
  `target/workload-verification/starting-session2-manifest.json`, SHA-256
  `7e961e87fe1478fb07439a4f19b1919fd15686a34a90f8bf1ae3b3e36fa8296c`;
  cross-target `x86_64-unknown-linux-gnu` workspace/all-target check passes
  (`target/workload-verification/starting-session2-linux-manual.log`, isolated
  `target/linux-check` root). The first verification attempt failed only the
  formatting gate (unformatted test edits) and stays recorded as
  `starting-session1-format`; the correction needs its own observed hosted run.
  Both pinned matrix images remain byte-identical. No launched process, native
  entry, GPU execution or live qualification is claimed; the child-side worker
  entry/driver/setup, complete consent/environment facts, the Start/native
  admission issuer and aggregate live export remain open.

- 2026-11-03 executable starting-mode routing and bounded response delivery:
  the fixed `autotune-pilot-sampler` main now dispatches only on the exact
  `--pilot-starting-v1` first token to `starting_entry::entry`; every other
  argument vector keeps the unchanged pilot sampler entry. `entry` parses the
  fixed 14-token starting argument mode (one extra token forces a strict
  rejection instead of silent truncation), adopts the exclusively inherited
  endpoints through `Context::from_starting_inherited`, and composes the actual
  entry. `execute` now builds the retained `Response` and hands it to the
  bounded `starting_delivery::Chunks` sender over the already-owned ordinary
  lane after the original stop/drain/control completion. The frame is
  `[u32 total][u32 header_len][canonical header][raw data]`, at most 64 KiB; the
  fixed response cap (2 MiB) needs at most 33 chunks. The header binds the exact
  declaration tag, scope digest, transfer digest, index/count, total and byte
  offset. The parent `Assembler` reserves the full response cap before any body
  allocation, requires a strictly increasing index/offset with exact declared
  lengths, verifies the assembled SHA-256 against the declared digest, and then
  verifies the payload still names the parent's scope digest and declaration
  tag before exposing canonical bytes. Gaps, duplicates, foreign
  declaration/scope, structural contradictions, digest mismatch and a missing
  final chunk remain sticky explicit faults; only a complete verified stream
  yields `Status::Complete`. A failed queue handoff retains its chunk index,
  sent count and cause and sends no successor. `Outcome::exit_code` is 0 only
  for clean trace/drain/control completion together with a complete drained
  delivery; it remains child orderly-completion, not qualification or a GPU
  fence. No native/live/hosted/process execution is claimed. CPU-only
  verification binds baseline `10550de` plus unchanged before/after source
  inventories: **462 tests** (381 integration including 12 new delivery/routing
  cases, 32 probe ABI fakes, 24 worker feature-library fakes/static, 25
  reference/prepared), formatting, warning-denied scoped Clippy and
  full-workspace/all-target/all-feature compile/link. Manifest
  `target/workload-verification/starting-delivery-final3-manifest.json`,
  SHA-256
  `b5e69b5fd4c1aacbd9b30ffa2f5cd683d43d46bd4a963006efca6d6f137a3b67`.
  Both pinned matrix images remain byte-identical. Parent-side pipe polling,
  multi-mode launch declaration selection, complete required consent facts and
  actual Windows/NVML acceptance remain open; no success qualification follows
  from the exit code.

- 2026-11-03 bounded starting response/export: `workload::starting_export::Response`
  now freezes one canonical `pilot_starting_settings_response` that binds the
  exact starting scope canonical/digest, declaration tag, original frequency/
  epoch, child first entry/receipt and 10 s preflight deadline. It retains the
  observed `SamplerInfo` runtime/raw UUID+PCI, the complete fixed 3077-call
  `nvml_trace::Ledger` (per-call before/after anchors, attempted flag, raw signed
  status and complete fixed output buffer, first fault, overflow and rejected
  return), the snapshot canonical/digest/source or an explicit missing snapshot
  plus block, all four serial operations' requested/attempted/returned/raw clocks,
  cleanup status, original stop/drain/control facts, the control completion/
  receipt/finished/fault/window/trigger/event/transport/frequency/rejected
  receipt, and native-failure/clock/control/event-clone/unreported-completion
  evidence. Foreign or stale scope/declaration/snapshot and mismatched reported
  identity are rejected before encoding; owned/codec capacity is reserved before
  model construction; the canonical bytes and `RawArtifact` digest are
  deterministic. `starting_entry::Outcome::response` composes this export from
  the actual retained entry. No peer delivery, wire response, executable routing
  or success exit code is added; this is raw retained evidence, not native
  execution/cleanup/GPU proof or a baseline verdict. CPU-only verification binds
  baseline `01c16cf9a679dddf7ad0f9e6ac4c54696d9b0f46`, 450 tests (369 integration
  including 9 new export cases, 32 probe ABI fakes, 24 worker feature-library
  fakes/static, 25 reference/prepared), formatting, warning-denied scoped Clippy
  and full-workspace/all-target/all-feature linking. Manifest
  `target/workload-verification/starting-export-final-manifest.json`, SHA-256
  `cc8f06f5aa45a5fd271693b7bc191f078dde9edc8a2465ca582e5ba10853069a`.
  No independent-agent review, native entry/load/watcher/process/GPU execution is
  claimed; both pinned matrix images remain byte-identical.

- 2026-11-02 per-ABI/per-call NVML stop/deadline barriers: the probe now has a
  fixed read-only `SamplerObserver` seam (`SamplerCall`/`SamplerOutput`/
  `SamplerReturn`) hooked before and after every internal startup/selection/
  description ABI call: init, bounded count, per-index handle/UUID/PCI and the
  three runtime text/version getters. `CallGuard` delivers the raw signed status
  and complete output buffer before the post-call clock hook; a false/rejected
  hook or nonzero native status permanently closes that observed invocation, so
  no successor ABI call is issued. The observed identity still requires the same
  exact unique UUID+PCI pair, null-handle rejection and 1024 count bound as the
  unobserved path, and initialized/selected ownership remains with the caller
  for explicit shutdown. `workload::nvml_trace::Ledger` records the fixed
  3077-entry bound with per-call before/after anchors, first fault, overflow,
  rejected return and a budget reservation; any fault stops the shared
  `NativeAdmission` gate. Both the pilot sampler entry and the starting entry
  compose this observer for Open/Describe, and reject runtime facts/faults
  before readiness. Eight probe cases (exact ABI order, stop-before call,
  stop-at-return, native status, exact identity, describe stop, missing symbol,
  status-0 unusable text) and eight Ledger cases (raw per-call anchors, stopped
  gate, deadline equality, clock loss/regression, unpaired/mismatched return,
  native status, fixed capacity/no eviction, construction bounds) were added.
  CPU-only verification binds baseline `fbd9f3078dc7e90dacfe787734c35be7e4127aea`,
  441 tests, formatting, warning-denied scoped Clippy and full-workspace
  all-target/all-feature linking (manifest in ROADMAP.md). No native load,
  process/GPU execution, final response/export or independent-agent review is
  claimed; the group-to-per-call gap is closed in source and CPU fakes only.

- 2026-11-02 starting serial entry composition: `starting_runtime::Trace` now
  retains logical Open/Describe/Read/Cleanup requests/entries/returns, original
  preflight history, first failure, signed cleanup status and bounded output/
  control drain. It composes the exact starting Shared scope rather than a run
  placeholder. Snapshot owns its own serial begin/return pairing and per-getter
  raw facts; its read lane closes on return. `starting_entry::execute` now joins
  the actual starting context, independent watcher, retained NVML selection/
  description, existing three-setting Snapshot collector and explicit cleanup
  in source. Initialized failed sessions and selected owners remain in the
  shared ManuallyDrop NativeOwner until explicit shutdown returns or no session
  was initialized; existing pilot owner behavior is extracted unchanged.
  Raw open failure/description/snapshot errors, typed clocks/event-clone errors,
  original completion on failed handoff and actual control report remain owned.
  Only already-finished control threads can be joined; no executable routing,
  final response, artifact export or success exit code is supplied yet.
  Seven new CPU/static tests cover operation order, raw clocks/returns, stopped/
  late groups, immutable cleanup, pending/grace/drain, scope/capacity/exhaustion,
  duplicate rejection and actual entry signature compilation. No native entry,
  watcher, process, GPU execution or independent-agent review is claimed.
  Important remaining contract gap: Open/Describe are logical probe API groups,
  not individual ABI-call traces. Selection/runtime helpers may perform several
  native calls internally. Per-call stop/deadline barriers must be audited and
  implemented before executable routing/live qualification; post-group clocks
  and an independent watcher do not prove no internal successor call occurred.
  Parent response/export and complete facts/consent/admission also remain open.
  Final source-bound verification/publication receipt is in ROADMAP.md.

- 2026-11-02 independent starting control: a separate scope-bound temporal state
  and actual Windows event/control-lane watcher now close serial read admission
  independently of NVML. No WorkerBinding/run is invented. Work expires at the
  original 10 s preflight boundary; first stop owns 1 s cooperative/3 s observation
  windows, clipped to the original session ceiling. Delayed detection uses the
  original boundary, and late/duplicate stops or native returns cannot renew it.
  Cleanup eligibility checks the original raw deadline and pending state but
  grants no execution authority. Completion keeps initialized/attempted/raw
  shutdown status separately, rejects initialized-without-successful-cleanup,
  pending/future/late commands, and retains first command/actual receipt.
  The watcher owns the split control port/event, real clock/frequency and bounded
  one-slot completion mailbox; event-only starting mode rejects/retains foreign
  control packets rather than interpreting pilot Stop/Start frames. No wire ack
  or final response is claimed yet. Readiness follows the first actual event/
  clock tick; each poll clock is retained before a separate completion receipt.
  Raw event/OS/clock/transport errors, rejected packets and pending race mailbox
  remain owned. Report retrieval joins only an already finished thread; Drop
  closes the gate without joining. Shared/mailbox reservations survive their
  independently retained owners. Eleven added CPU/static tests cover original
  deadlines, delayed/blocked returns, first facts, cleanup/pending/clock errors,
  epoch/overflow, full 13 s history under ordinary exhaustion, capacity ownership
  and actual Windows API compilation without starting the watcher. Review closed
  readiness, shared-accounting, poll-retention and completion-fact gaps before
  final checks. No independent-agent review or process/native/GPU execution.
  Starting context bc6ab35a15bfaec095abf4471f2e1d08ee54a3b6 passes hosted Linux/MSVC
  run 41465032149. Final source-bound evidence/publication: ROADMAP.md. Actual
  native-open/read/cleanup entry, bound response, executable routing, complete
  parent facts/consent, aggregate export and qualification remain open.

- 2026-11-02 starting child inherited-context continuation: actual Windows
  `Context` now shares endpoint/clock/image/pipe preparation through a private
  two-mode argument vocabulary, with pilot Bound remaining the default. A
  distinct starting context accepts only the starting bootstrap and fixed sampler
  image; it does not fabricate a manifest/run/WorkerBinding. Before any first
  packet, actual child entry must be within the original-epoch 10 s preflight
  ceiling; idle polls and final receipt cannot renew it. Epoch offset, invalid
  clocks and absolute interrupt-time overflow are checked. All existing raw
  packet, typed clock/frequency/OS errors, image lock/digest and transport shutdown
  failure owners remain shared. No native calls occur in context creation.
  Three added CPU/static cases cover these bounds and the typed actual Windows
  entry/parts APIs without invoking them. Existing pilot bootstrap cases pass.
  Independently supervised starting control/read/cleanup/response and executable
  routing remain unfinished; no native-admission issuer is added. Source-bound
  final verification/publication receipts are in ROADMAP.md. No process/native/
  GPU execution or independent-agent review is claimed. Previous starting-launch
  source c1e9588622813127c1bf6ea2a0cb762322888d52 now passes hosted Linux/MSVC run
  41463915362. Full Task 3, E3 and Gate B remain open.

- 2026-11-02 pre-manifest starting bootstrap/launch continuation: the separate
  immutable starting scope now has a fixed sampler launch declaration, bounded
  14-token argument mode and canonical first frame. No pilot run/manifest or
  WorkerBinding placeholder is used. Original parent entry is hashed into the
  declaration; child reading validates scope/epoch/frequency, entry/first/receipt
  chronology and the absolute original-epoch 10 s preflight ceiling. Strict
  re-encoding rejects alternate wrapper bytes. A distinct one-shot sender keeps
  exact unsent owners and first typed faults, and honors cancellation even when
  constructed before stop. The actual Windows launch core is shared through a
  sealed two-declaration vocabulary, preserving image lock/hash, suspended
  creation/job assignment, retained failures/process owners, resource/exit
  observations and cancellation. Starting declarations can only use the fixed
  sampler image and sampler limits/environment; pilot defaults remain intact.
  Child inherited-context/entry/control/readout-response composition is still
  unfinished, and the new flag is not yet routed by the executable. Per-process
  limits are not an aggregate session CPU proof. No native-admission issuer,
  process/native/GPU execution or independent-agent review is claimed.
  Final verification and publication receipt are recorded in ROADMAP.md. Initial
  compile-test errors (explicit impl-Trait generic argument, then Clippy type
  complexity) were corrected with a typed function alias before final checks.

- 2026-11-02 common actual child bootstrap: first pilot frame now binds role,
  declaration, parent setup entry and canonical manifest through the existing
  bounded sender/pipe path. Concrete Windows context composes exclusive inherited
  endpoint adoption, actual clock capture, own-image measurement/lock, pipe poll
  and strict role/manifest/argument/chronology checks. Original raw first packet,
  available image/clock/failure observations and I/O shutdown state remain owned.
  Pre-manifest temporal validation reuses unchanged clock rules without inventing
  device/session IDs; a bounded charged history preserves missing/overflow faults.
  Worker Gate::from_bootstrap keeps the original parent setup deadline and rejects
  the sampler role. Eight new CPU tests cover these seams, including actual shared
  framer/sender byte ownership. One initial misplaced-field compile error was
  corrected before verification. No independent-agent review or native execution
  is claimed. Baseline `ef98931` plus unchanged before/after source inventories
  passes **322 CPU-only tests** (256 integration, 22 probe ABI fakes, 19 worker
  library, 25 reference/prepared), scoped all-feature warning-denied Clippy,
  formatting and full-workspace/all-feature compile/link. Manifest
  `target/workload-verification/child-bootstrap-final-manifest.json`, SHA-256
  `19383dac24d733fa8893974a3eaf68007c7c43e9ed8e5ffe0c6cfed08ae3f999`.
  Concrete Windows endpoint/clock/image/pipe work is compiled, not executed;
  both pinned old matrix images still match. Pilot start `0fc3bce` passed hosted
  run 41411782871 on Linux and Windows/MSVC. Publication belongs in ROADMAP.md.
  Fixed mains/control/
  telemetry/native producer and complete parent admission/export remain Task 3.

- 2026-11-02 fixed pilot process startup: role/image/run/epoch argument declaration
  and strict child binding, with actual Windows background measurement/create/
  assign/resume source reusing existing sampler containment primitives. Role
  commit/user-CPU limits preserve the frozen profile. Original process/job/image
  owners, measured absolute path/hash, failure stage/raw code, creation/assignment,
  termination request and I/O shutdown observations remain independent. Post-create
  errors retain image locks as well as original process handles; late cancelled
  results remain observable and closed to ordinary work. A shared cancellation
  queue protects senders created before stop and preserves control availability.
  Worker child environment requests CUDA_CACHE_DISABLE=1 after NVIDIA documentation
  review; sampler retains its old empty environment. No parent/global environment
  modification, native/default-mode execution, benchmark permission or GPU fence.
  Six new CPU tests cover the declaration/binding/cancellation seam. Baseline
  `7d5f135` plus unchanged before/after source inventories passes **314 CPU-only
  tests** (248 integration, 22 probe ABI fakes, 19 worker library, 25 reference/
  prepared), scoped all-feature warning-denied Clippy, formatting and full-
  workspace/all-feature compile/link. Manifest
  `target/workload-verification/pilot-launch-verified-manifest.json`, SHA-256
  `4680803dc22df76e18797932410d58e527b8a3a21d582de94813297affc3c62d`.
  Source inspection retained actual image locks/path/hash on failures as well as
  live process handles and closed cancellation/owner-loss cases before final
  checks; no independent-agent review is claimed. Actual OS startup is compile/
  link only; no process/native/GPU test ran. Both pinned matrix images still
  match. Prior dispatch `b97670d` passed hosted run 41408825278 on Linux and MSVC.
  Publication receipt belongs in ROADMAP.md.
  Fixed child executable entries/bootstrap/control/telemetry and full parent
  admission/session export remain Task 3 obligations; NativeAdmission has no issuer.

- 2026-11-02 selected sampler settings dispatch: actual inherited-pipe poll now
  composes strict one-shot request parsing, child receipt/setup chronology,
  selected NVML collection under the independent shared stop gate, immutable
  Capture retention and response queueing. The first raw request remains owned;
  a duplicate returns its original owner. Capture/unsent frame/typed send reason
  remain retained after encoding/handoff failure, with no successor retry. The
  fixture path stays simulated; failed facts can be sent as supplemental raw
  evidence and cannot become readable current values. Child and parent setup
  ceilings only tighten. A red regression reproduced post-collection clock
  validation incorrectly restarting at receipt; final code carries the last
  getter clock into subsequent polls. Shared admission also closes on pending or
  replay failures, including direct native dispatch calls. Baseline `3394325`
  plus unchanged before/after source inventories passes **308 CPU-only tests**
  (242 integration, 22 probe ABI fakes, 19 worker library, 25 reference/prepared),
  scoped all-feature warning-denied Clippy, formatting and full-workspace/
  all-feature compile/link. Manifest `target/workload-verification/sampler-dispatch-verified-manifest.json`,
  SHA-256 `e092d280beaeb9204ad87fc43833a09b21e7b51151ba1767d94eba185b3bd871`.
  Six new dispatch/retention/chronology regressions pass, including the real red
  clock case after correction. Earlier pass manifests remain historical and
  do not bind the final source. Local source/contract inspection completed;
  no independent-agent review is claimed. Concrete Windows/NVML dispatch is
  compile/link only; both pinned matrix image hashes remain unchanged. Prior
  pipe bridge `943fb42` passed hosted run 41406262526 on Linux and Windows/MSVC.
  Publication receipt belongs in ROADMAP.md.
  No runtime loading, default native mode, fixed process entry/launch, compute
  admission issuer or executed native/process/GPU evidence is added. Full pilot
  telemetry/control startup, parent facts/safety/re-entry/consent and aggregate
  live session export remain Task 3 obligations.

- 2026-11-02 actual Windows pipe bridge continuation: connect encoded worker and
  settings frames to the existing bounded synchronous pipe workers. Sender is
  the producer's actual FrameSink; full binding/lane/direction metadata remains
  immutable without changing canonical wire bytes. Reserve the destination copy
  before allocation, return the exact original owner on failed handoff, and retain
  queued copies on partial OS writes. A one-shot Windows sender prevents reset;
  optional control ownership splits before sender creation to keep independent
  control polling off a blocking driver thread. Ordinary polling never consumes
  the split control receiver or its fault slots. Raw packet/clock/rejection and
  decoded result stay owned together through worker/settings parent decoding.
  Readonly sampler behavior and executable modes remain unchanged. Baseline
  `a433a3d` plus unchanged source inventories passes **302 CPU-only tests**
  (236 integration, 22 probe ABI fakes, 19 worker library, 25 reference/prepared),
  scoped all-feature warning-denied Clippy, formatting and full-workspace/
  all-feature compile/link. Manifest `target/workload-verification/windows-bridge-final-manifest.json`,
  SHA-256 `38e5d6292f0fc18e743ec098032fb847a4b2750d82c3c6dfbb18b45f9acca36f`.
  Six new handoff/ingress tests, one producer integration and one compile-time
  Send assertion cover this seam. Local source/contract inspection completed;
  no independent-agent review is claimed. Pinned old matrix images still match.
  Concrete Windows pipe behavior is compiled, not executed; no native/process/
  GPU run. Prior exchange `2ebad35` passed hosted run 41404479033 on both platforms.
  Fixed worker launch/entry, actual pilot-sampler dispatch, full parent facts/
  safety/re-entry/consent admission and aggregate live session export remain open;
  NativeAdmission still has no issuer. Publication receipt belongs in ROADMAP.md.

- 2026-11-02 settings exchange continuation: separate bounded
  `pilot_settings_sampler` request/response framing connects the selected
  current-setting collector and strict parent ReportedCapture reader. One
  attempt, exact full binding/request/artifact digests, preserved raw failures,
  sender-claimed provenance and sticky independent parent stop/deadline outcomes.
  Setup entry and exact relative/cumulative deadline travel in the request and
  constrain actual getter collection; issue/child/receipt chronology is assessed
  without rebasing the original epoch or granting a Start/settings freshness lease.
  Windows sampler dispatch, metadata/session export and full parent required-fact/
  safety/re-entry/consent admission remain Task 3 obligations. NativeAdmission
  remains unavailable. Baseline `3dddef2` plus unchanged source inventories passes
  **294 CPU-only tests** (229 integration, 22 probe ABI fakes, 18 worker library,
  25 reference/prepared), scoped all-feature warning-denied Clippy, formatting
  and full-workspace/all-feature compile/link. Manifest
  `target/workload-verification/settings-transport-closed-manifest.json`, SHA-256
  `0cf0bfe5a7ff135fb798e85c1794aa1f1f23df97c5a5524c025c1902467d57f6`.
  Seven new integration and one private collector-window test cover the seam.
  Local contract/source inspection tightened the child relative deadline and
  original issue bracket before final verification; no independent-agent review
  is claimed. Initial visibility compile and sandbox linker failures remain
  historical. Old pinned matrix images still match; no native/process/GPU run.
  Settings source `5a9b289` passed hosted run 41347349598. New publication evidence
  belongs in ROADMAP.md; full Task 3 and the roadmap remain incomplete.

- Final current-setting fact-source verification: baseline `8688ab7` plus exact
  sources, unchanged before/after inventories, **286 CPU-only tests** (222
  integration, 22 probe ABI fakes, 17 feature-library fakes, 25 reference/prepared),
  scoped all-feature warning-denied Clippy, formatting and full-workspace/
  all-feature compile/link pass. Six new artifact/value/clock/identity/owner tests,
  three collector-engine tests and three actual selected-getter ABI-fake tests
  cover the new seam. Manifest `target/workload-verification/settings-reviewed-manifest.json`,
  SHA-256 `b402c29035b42875ca5805dcc48e74eebe517df71b5f9114c59ef6e9bc00c52f`.
  Both old pinned matrix images still match; no DLL/native/process/GPU run.
  Producer `2ffba2f` passed Linux and Windows/MSVC in hosted run 41343779427.
  Settings facts are only part of required parent evidence: initial/runtime/
  environment facts, freshness/relative setup, safety/re-entry, exact consent,
  sampler response integration and Windows supervision remain open. Task 3 and
  the actual full roadmap remain incomplete; NativeAdmission still has no issuer.

- Current-setting review: Standards found zero hard violations/material smells.
  Spec found P2 post-read finalization-budget loss. Fixed by reserving complete
  owned model/canonical/codec capacity before the first getter and transferring
  it at finalization. The regression proves insufficient budget executes zero
  getters and budget pressure introduced during reads cannot discard their
  evidence. Final independent Spec review closed P2 with no remaining material
  issue; final Standards delta likewise passed. No native read or process ran.

- Parent fact-source continuation: add separate selected current-setting getters
  to the actual NvmlSampler for current power limit, modern graphics P0 offset
  and modern memory P0 offset. Each invocation uses one requested getter on the
  uniquely selected handle, no legacy fallback/setter or telemetry substitution.
  Existing six-metric sampler/readonly manifest/profile/protocol stays unchanged;
  these reads require a separately scoped workload-native capture path.
- `workload::settings::Capture` collects the three raw fields sequentially with
  original per-call anchors and stop/clock/device-loss closure, reserving bounded
  staging/model/codec storage first. Native collection takes exclusive access to
  the sampler and checks exact declared raw identity before getters. Explicit
  fixture construction cannot select native provenance. Observed wrong clock
  tokens, missing anchors, raw status/unit/symbol/reason and malformed scalar
  representations remain distinguishable in canonical evidence, not zero-filled.
- This is a per-run current-settings artifact bound to the already frozen
  manifest, not the pre-manifest initial starting-evidence artifact (using it
  there would create a digest cycle). Comparison covers only these three observed
  values within the same manifest/session/device/clock/source scope. It is no
  proof of unchanged complete settings, other P-states/V-F curve, writable support
  or ownership. Captured values are not a freshness lease at Start.
- Capture guards original clock validity/current-run cumulative setup bounds.
  The future parent must still enforce its frozen relative setup deadline,
  actual Start/safety/re-entry freshness, native peer/runtime/environment/initial
  evidence and independent exact consent. No new numeric freshness policy is
  invented for settings. NativeAdmission remains unissued; no GPU/native-read
  process was executed. Actual sampler request/response/export integration and
  complete parent admission/Windows chain remain Task 3 obligations.

- Final producer milestone verification: baseline `abc58f8` plus unchanged
  before/after source inventories passes **274 CPU-only tests** (216 integration,
  19 probe ABI fakes, 14 feature-library fakes, 25 reference/prepared), scoped
  all-feature warning-denied Clippy, workspace formatting and full-workspace/
  all-feature compile/link. Twelve producer tests include full encoded 128+256
  P1 to parent audit, P3 first failure, drained stop, original control windows,
  native-admission rejection, clocks, no retry and sticky/late export failures.
  One new typed ABI-fake library test exercises the actual CudaBackend Driver
  bridge. Manifest `target/workload-verification/producer-reviewed-manifest.json`,
  SHA-256 `27ae4e3783fb4762dd180c152420d224de03b56ae51cc593c3c56a4552394a6c`.
  First Clippy rejected unnecessary_unwrap and was corrected without lint
  suppression; earlier seven/nine/ten-test draft checks remain historical.
- Independent final Spec review closed both P2 findings with no remaining
  material defect; final Standards review found zero hard violations/material
  smells. Both original matrix image hashes still match. No native DLL, owned
  fixture process, GPU workload or tuning ran. Tracing `6ef2793` passed Linux and
  Windows/MSVC in hosted run 41340614086. Actual parent admission, Windows child/
  supervisor and live export/session composition remain Task 3, not a narrower
  replacement objective or completed E3/Gate B.

- Producer review: Standards axis found no hard violations/material smells.
  Spec axis identified two P2 issues: drain completion rebased exit grace, and
  capacity errors during failure export did not persist. Control now carries the
  original validated StopWindow (not a newly timed bool); child receipt retains
  its own raw anchor separately. Terminal/cleanup/export only tighten original
  stop/fault ceilings; late controls and cooperative expiry never grant native
  entry. Invalid control windows are retained, and continuous captures poll
  control even when the trace will reject that time before its callback.
- Export capacity/encode errors now retain a sticky fault and original output,
  preventing retry after RAM frees. Handed-off word count is separate from
  complete-export status; a full final handoff followed by a failed deadline/
  clock guard cannot report complete export. New regression cases exercise both
  issues, future control, delayed original stop and the expired cleanup cap.
  Native admission remains unissued; no hardware execution follows.

- Bound producer continuation scope: implement `workload::producer` using
  prepared input/reference bases and fixed x/output/sentinel buffers, the actual
  observed Driver seam (CudaBackend implementation), continuous original-epoch
  clock/control context and existing worker protocol. One validated handshake
  yields one non-cloneable StartObservation. Fixture start rejects Native; the
  separate NativeAdmission type has no issuer until the real parent admission
  chain exists. No new native executable, DLL loading or execution authority.
- Each advance submits at most one dispatch or polls one in-flight query and
  completes full readback/check/hash/reporting. Fixed 128+256 counts, shape/seed/K
  and phase bounds are preserved; measured entry is encoded before measured work.
  Completed warm-up transitions to the measurement cumulative boundary without
  reapplying its expired cap. Frames consume owned bounded RAM and transfer to a
  bounded sink; uncertain send retains the exact frame, poisons the channel and
  ends work without retry. Sent counts mean queue handoff, not peer/disk proof.
- Stop closes new work. In-flight query/readback and CPU checking may drain under
  unchanged original 100 ms stream/250 ms cycle/phase bounds (earlier than the
  1 s cooperative window); no successor kernel follows. Independent controller
  supervision remains mandatory for blocked native/sink calls. First end/fault,
  raw clocks including cross-dispatch regressions, partial summaries, readback
  lengths and actual call statuses remain retained. Successful vectors are
  reused only after full checking/hash; no replayable successful vector archive.
- First full failed output transfers its existing owned buffer/charge into the
  separately reserved 4 MiB failure pool, retaining exact bits and hash or its
  absence. Chunk export is bounded by original exit/run ceilings; clock/digest/
  transport failure keeps original bytes and an explicit incomplete export.
  Driver/OS/allocator overhead is not claimed bounded by host-buffer accounting.
  Explicit cleanup is one shot after admitted terminal known-idle work, records
  attempted before entry and raw return before post-clock, and never proves OS
  exit/native global quiescence. Native cleanup cannot bypass missing admission.
- Initial full-loop CPU fixtures pass seven tests, including full encoded P1 to
  parent audit, finite stop-drain, stale/partial/faulted outputs, uncertain sends,
  continuous clock failure and measurement phase entry. New largest-P3 failure,
  timeout and actual typed ABI bridge tests extend this verification. No fixture
  process/native driver/GPU execution; actual required-fact/safety/re-entry/
  exact-consent admission, Windows worker child/supervisor and live export remain
  Task 3. This is source toward the real pipeline, not E3/Gate B acceptance.

- Final actual-adapter tracing verification binds baseline `ae44192` plus source
  with identical before/after inventories: **261 CPU-only tests** (204 integration,
  19 probe ABI fakes, 13 feature-library fakes, 25 reference/prepared), scoped
  all-feature warning-denied Clippy, workspace formatting and full-workspace/
  all-feature compile/link pass. Eight portable trace tests and three new typed
  CUDA ABI-fake tests cover raw status/clock/control/fault retention, original
  binding, trace charges, actual callback order and poisoned adapter successors.
  Manifest `target/workload-verification/trace-reviewed-manifest.json`, SHA-256
  `1f69ce90c5e7081329eae7cb412d5c8245779fe3a03c4ba416edaa98cb4c2d94`.
  Earlier trace-final/pre-review checks remain historical; this manifest covers
  the reviewed binding correction. Initial test enum-shadowing/array-comparison
  compile failures and Clippy large-error/array-chunks/unused-import findings
  were fixed without weakening lint. Both pinned original images still match.
  No DLL loading/native/process/GPU tests. Assessment `e53c7e2` passed both hosted
  platforms in run 41339063933; this source has its own later publication scope.

- Independent tracing review: Standards found zero hard violations/material
  smells. Spec found a material attribution defect: the first draft could bind
  collected A calls to a B summary/channel. Fixed by retaining exact original
  WorkerBinding/phase/j, validating summary/pending assertions, replacing plain
  Message exposure with borrowed record inspection and `Channel::encode_traced`
  enforcing the pinned identity. Final Spec review closed P2 with no remaining
  material defect. The regression covers changed manifest with same clock,
  seed/j/phase, foreign channel poison and preserved raw owners. Continuous
  cross-trace clocks and frozen phase entry remain producer obligations.

- Actual-adapter tracing continuation: `CudaBackend` now has observed
  submit/query/readback methods; existing Backend methods delegate through a
  no-op observer and preserve existing behavior. Every upload, default-stream
  upload-completion query, launch, kernel query and readback is observed before
  entry/after raw return. An observer failure keeps the adapter poisoned and
  prevents successors. Default-stream NOT_READY still fails submit without
  launch/retry; only the kernel query may remain pending.
- `workload::trace::CallTrace` owns reserved raw calls, injected clock/control
  captures and sticky first faults. It retains raw status before post-clock
  capture, including native failure plus missing post-clock. It observes the
  unchanged original phase/stream/cycle limits where entries are known; the
  future producer must keep one frozen phase entry across dispatches, supply
  Input/Check/Hash host stages and preserve actual summary/output evidence.
  Partial traces alone are not whole-cycle or native qualification.
- A successful bind transfers the trace and charge into immutable TracedRecord,
  borrowed directly by the actual worker encoder. Failed nonempty-summary binds
  retain both raw owners; a pre-reserved boxed error avoids copying a large
  stack error. Fault and first unmatched return remain separate owned supplemental
  evidence, not silently serialized as an ordinary matched call. Future exporter
  must retain them. No new binary, native admission or executed native path.
- Integration reconciliation: existing CUDA submit performs a required
  default-stream upload-completion observation before launch. Added its explicit
  Operation kind and made parent audit require it; prior aggregate trace fixtures
  omitted this real call. Successful order is now Input/uploads/sentinel/upload
  completion/launch/kernel queries/readback/check/hash. Numeric policy and old
  handshake/sampler bytes remain unchanged. Full native producer/host buffers,
  retained summaries/failure export, admission and Windows composition remain
  Task 3; this milestone closes the real-adapter per-call tracing seam only.

- Parent report-assessment milestone reviewed and source-bound verified:
  independent Standards axis found zero hard violations/material smells; Spec
  axis found no material defect. The count-completion dimension is separate from
  later fault and all runtime/qualification obligations. Eight new tests pass;
  final suite totals 250 CPU-only tests (196 integration, 19 probe ABI fakes,
  10 feature-library fakes, 25 reference/prepared), plus formatting, scoped
  all-feature Clippy and full-workspace/all-feature compile/link. Unchanged source
  inventories bind baseline `64e5c00` plus sources in
  `target/workload-verification/assessment-final-manifest.json`, SHA-256
  `862f6ac8311d33af79d922c6f0b744164b60859f0fb7e7f884b0bdb98002e713`.
  Original pinned sampler image hashes still match; no process/native/GPU paths
  ran. Dispatch `8fb5de3` passed Linux and Windows/MSVC in hosted run 41338199629.
  Native producer/parent admission/Windows composition remain Task 3, not a
  substitute benchmark-only product or completed E3/Gate B.

- Parent report-assessment continuation scope: add `workload::assessment::Audit`
  over retained worker messages with bounded fixed metadata and no backend/OS
  calls. It checks original Start, exact j order and full bound counts; distinct
  Input/upload-A/B/X/sentinel/launch/query/readback/check/hash call order/statuses,
  original raw clock brackets/order and separate source/receipt/controller-tick
  streams, pending/readback/full-check assertions and exact reported work. The
  auditor borrows messages: its caller must first retain every owned raw frame,
  receipt and rejection; assessment is no archive or live truth claim.
- Measurement entry now has a worker-role `PhaseEntered` frame, emitted before
  measured work. Warm-up uses the existing conservative issued-Start bound;
  measurement freezes its relative cap at the reported actual entry, capped at
  the original cumulative boundary. Before entry is known only that immutable
  cumulative deadline can be assessed. Native producer must send entry before
  work and stop on uncertain send; independent controller supervision remains
  mandatory. Queued evidence never rebases time; a newer controller tick is
  considered when assessing an older entry/receipt. Controller ticks and delayed
  receive observations use independent original-epoch clocks, preventing a valid
  queued receipt from spuriously regressing the tick stream.
- Per-cycle enforcement includes all input/submit/readback/check/hash work at
  250 ms and first stream upload through returned completion at 100 ms, both
  under earlier frozen phase ceilings with equality losing. Missing, malformed,
  failed, skipped or duplicate evidence closes assessment, retaining counts and
  first fault; late evidence cannot resume it. All-reports-complete is a narrow
  dimension, never consent, coverage, physical identity, replayed successful GPU
  vectors, teardown/exit or useful baseline acceptance. Safety/re-entry/exact
  consent admission and native/Windows/controller composition remain Task 3.
- Initial assessment checks: eight CPU tests pass, including actual encoded full
  128+256 plan, explicit phase entry, retained message ownership, delayed source
  and receipt streams, query NOT_READY before completion, malformed calls/clocks,
  fixed metadata charges, silent-worker ticks and exact deadline equality. An
  upload-start budget regression was observed red before correction (the first
  draft counted from launch); final focused tests and scoped Clippy now pass.
  Early compile failure used the wrong existing Phase variant name and is fixed.

- Dispatch-transport milestone delivered: nine new CPU tests cover raw stage
  clocks/statuses/unknowns, exact bound phase/count/work, strict nested rejection,
  independent body golden, ownership/control compatibility, gaps/duplicates/hash
  uncertainty and full P3 1 MiB vectors through actual encoded worker frames.
  Independent Standards review found zero hard violations/material smells;
  Spec review found no material defect, including the final explicit upload,
  sentinel/launch kinds and boundary-test delta. No native/process/GPU paths ran.
- Final dispatch verification binds baseline `ca6efa1` plus unchanged before/after
  source inventories: 242 CPU-only tests (188 integration, 19 probe ABI fakes,
  10 feature-library fakes, 25 reference/prepared), formatting, scoped all-feature
  Clippy and full-workspace/all-feature compile/link pass. Manifest
  `target/workload-verification/dispatch-final-manifest.json`, SHA-256
  `d9d9386129d900bf0e7e25aa9f5ae1c13ffad3576a23d56488b5069ff3e5d83d`.
  Both original pinned matrix images remain unchanged. Handshake `4c4fdfb`
  now passed Linux and Windows/MSVC in hosted run 41337285143. Task 3 stays open
  for parent adjudication/admission, native production and Windows composition;
  decode/hash completion alone never closes correctness, process or Gate B.

- Dispatch continuation scope (November 1): extend the existing worker channel
  with bound DispatchRecord and FailureChunk bodies, keeping handshake/sampler
  schema bytes unchanged. Records retain raw optional pre/post anchors and signed
  statuses for input/submit/query/readback/check/hash, bound plan/index/phase,
  lengths/full-check diagnostics, exact verified work and optional raw SHA-256
  input/output digests. Missing/contradictory raw clocks remain observations for
  parent assessment, not an admission/result verdict. Passed vectors are not
  archived; the first full failing vector travels as ordered bounded chunks.
  Receiver reserves its full storage before allocation, retains partial bytes on
  gap/foreign/hash/transport failure and never claims OS/native completion.
  Per-message structural bounds (64 raw calls, 2048 words per chunk, 4096 JSON
  structures) exist inside the unchanged 64 KiB frame/RAM limits. Overflow must
  stop/preserve caller-owned trace; never truncate or merge query calls silently.
  These are representation bounds, not new polling/operating budgets. Sampler
  complexity remains 256. Parent sequence/lifecycle/clock/coverage adjudication,
  native producer, Windows adapter and exact consent remain Task 3 obligations.

- November 1 handshake milestone: canonical worker frames and child gate now
  implement the scoped continuation above. Nine CPU tests cover actual bytes,
  independent Start golden, binding/sequence/canonical rejection, sticky lanes,
  ownership/control reservations, raw statuses, clocks and terminal/deadline
  boundaries. Standards review found zero hard violations/material smells.
  Spec review found two timing defects: setup consumed unused re-entry allowance
  and future Stop triggers were accepted. Setup failure was reproduced before
  correction; setup deadline is now frozen at Prepare and checked at Ready/Start,
  and future triggers fail. Genuine late triggers keep expired original ceilings;
  the reviewer withdrew its earlier suggestion to reject all late triggers.
  Final independent static review found no remaining material defect.
- Final source-bound handshake verification: baseline `de18d39` plus sources,
  unchanged before/after inventories, 233 CPU-only tests (179 integration,
  19 probe ABI fakes, 10 library fakes, 25 reference/prepared), formatting,
  scoped all-feature Clippy and full-workspace/all-feature compile/link pass.
  Manifest `target/workload-verification/handshake-reviewed-manifest.json`,
  SHA-256 `4c512b76ac0a3bd0755b3dd6f55dab78046bda96d1de2dd2dbb887c2fdd6f0bc`.
  The earlier eight-test pre-review manifest remains historical. No fake process,
  native capture or GPU execution occurred. Pinned matrix images still match.
- Declaration milestone `1bfcac3` passed Linux and Windows/MSVC in hosted run
  40952952692. Handshake publication is a further software milestone, not
  completion of Task 3: Dispatch/failure-vector transport, parent admission,
  independent Windows supervision and exact consent still remain.

- Initial declaration slice delivered: immutable scoped pilot manifest, exact
  three-stage plans/profile bytes, strict canonical reading and bound-run
  calculations. Six workload tests plus one readonly compatibility regression
  pass. The initial missing-module failure was observed before implementation.
  Readonly profile digest was captured from the old implementation before
  extracting `limits_value`; the placeholder assertion failure was baseline
  capture, not a production defect or an independent digest derivation.
- Independent review: Spec axis found no material correctness/spec issue in
  this declaration-only boundary. Standards axis found zero hard violations
  and one optional low duplication smell in internal canonical/hex helpers;
  keeping manifest errors local avoids a broader codec refactor in this slice.
  No native/process/fixture commands were run by reviewers. Actual Start
  protocol/controller/native admission remain unfinished, as task 3 states.
- Source-bound final checks: baseline `3ed68d2` plus the declaration sources;
  unchanged before/after inventory, 224 CPU-only tests (170 integration, 19 probe
  ABI fakes, 10 feature-library fakes, 25 FMA reference/prepared), scoped
  all-target/all-feature warning-denied Clippy, workspace formatting and
  all-workspace/all-feature compile/link pass. Manifest
  `target/workload-verification/workload-final-manifest.json`, SHA-256
  `c2cb6f00268b6e5a26e854eb480ae92c0265f2046d0f2aa3070add8b528fd61f`.
  The first Clippy pass rejected manual_contains; corrected before final checks.
- Build isolation correction: early CPU checks reused the sampler build target
  and replaced its root fixture image. Recovered the exact matrix-pinned child
  from the existing Cargo cache (verified SHA-256), restored it without
  modifying its cached hardlinks, and copied both pinned images to
  `target/sampler-verification/pinned-matrix-2f932f31/`. Final checks use
  `target/workload-verification/build` exclusively. Both original matrix image
  hashes still match; no fixture was executed. This source now differs from
  the old matrix revision, so an approved old-matrix execution must first
  re-establish that exact reviewed source snapshot reversibly, preserving all
  newer/user-owned work, rather than skipping its guard or changing its digest.
- Start: clean `3ed68d2` checkout; previous turn delivered concrete preparation
  and publication progress. Exact fake-only execution matrix remains pending,
  not an authorization to run its branch. Implementation proceeds inline,
  preserving source/artifacts needed for that existing request; if approval
  arrives while this work is dirty, isolate only these agent-owned source
  changes reversibly before revalidating the already pinned fixture revision.

- 2026-11-03 hosted CI gate correction: the delivered starting delivery
  slice exposed two Linux-only build defects that failed hosted runs
  41497007596 (`fe97463`), 41497054618 (`01c16cf`), 41631462137 (`55ba2d8`)
  and 41631529381 (`10550de`) at workspace Clippy. `NvmlSampler::
  describe_observed` called the Windows-only `describe()` from its
  unsupported-platform branch, and the new delivery integration test
  referenced the Windows-only `starting_entry`. `describe_observed` is now
  itself Windows-gated (all its callers are Windows modules), and the token
  routing/signature cases plus their `OsString` import are Windows-gated.
  Cross-target `cargo check --workspace --all-targets --target
  x86_64-unknown-linux-gnu` now passes (log
  `target/workload-verification/linux-gate-fix2-linux-manual.log`, separate
  `target/linux-check` build root). Windows CPU-only verification re-ran as
  `linux-gate-fix2` (462 tests: 381 integration, 32 probe ABI fakes, 24
  feature-library fakes/static, 25 reference/prepared) with scoped Clippy,
  formatting and all-workspace/all-target/all-feature linking; manifest
  `target/workload-verification/linux-gate-fix2-manifest.json`. Hosted
  results are not retroactively rewritten; the fix needs its own observed
  run.

- 2026-11-03 parent-side starting reception: `workload::starting_receive::
  Receiver` now drives the strict assembler under the original epoch and
  absolute deadline, with a monotonic receive clock, explicit stop and a
  sticky first fault that stays distinguishable from chunk faults
  (`Clock`, `Deadline`, `Stopped`, `Assembly(..)`). A chunk arriving at the
  deadline is rejected; a stop or supervision fault hides every payload; a
  verified completion is retained and cannot be withdrawn by a stray later
  chunk, which is reported without recording a new fault. Capacity is
  reserved in the constructor, before any chunk is accepted. Nine new CPU
  cases cover exact payload exposure, post-completion retention, stop,
  deadline equality, clock regression, foreign chunk, duplicate gap, epoch
  deadline validation and capacity. Full CPU-only verification ran as
  `starting-receive2`: 471 tests (390 integration, 32 probe ABI fakes, 24
  feature-library fakes/static, 25 reference/prepared), scoped
  warning-denied Clippy, formatting and full-workspace/all-target/
  all-feature linking; manifest
  `target/workload-verification/starting-receive2-manifest.json`. Cross
  target `x86_64-unknown-linux-gnu` workspace/all-target check passes
  (`target/workload-verification/starting-receive2-linux-manual.log`).
  The first run failed red on an unused `Domain` import and the unused
  `epoch_ns` field; both were corrected before this manifest. Reception is
  transport-agnostic source: no parent process polling, live/native/GPU
  execution or qualification follows.

- 2026-11-03 concrete parent-side starting reception poll: `starting_receive::
  poll_native` now drives the strict receiver from one nonblocking owned
  inherited-pipe poll, keeping the OS cause, transport fault, idle/finished
  state and any rejected packet separate from the receiver's sticky fault.
  Idle polls advance the original-epoch deadline, a missing clock is a clock
  fault (never on-time), a closed peer or transport fault closes reception,
  and a non-ordinary lane or framing rejection is not accepted as a chunk.
  `Receiver::tick`/`clock_lost` expose the same rules to fixture tests.
  Four added cases cover idle deadline enforcement, idle clock regression,
  missing-clock semantics before and after a verified completion, and the
  compiled poll signature. Full CPU-only verification ran as
  `starting-receive3`: 475 tests (394 integration, 32 probe ABI fakes, 24
  feature-library fakes/static, 25 reference/prepared), scoped
  warning-denied Clippy, formatting and full-workspace/all-target/
  all-feature linking; manifest
  `target/workload-verification/starting-receive3-manifest.json`. Cross
  target Linux workspace/all-target check passes. The concrete pipe/OS path
  is compiled, not executed; no live/native/GPU verification follows.
