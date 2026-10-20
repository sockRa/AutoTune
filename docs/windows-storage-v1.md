# Frozen isolated Windows storage contract v1

Frozen for the **fake-only, isolated storage/ownership slice**, 2026-10-19.
Authoritative types: `autotune-recovery-contract::storage`. Implementation:
`autotune-recovery-windows::{storage,storage_executor}`. This does not freeze
the unfinished process/bootstrap/IPC contracts or qualify a production root.
Changes to this disk format require a storage-version change and fail-closed
migration design, not permissive decoding. Prior draft test artifacts are retained
but need not decode under this frozen schema. No migration/replay/repair exists.

## Bytes and field inventory

Disk files are complete bounded canonical UTF-8 JSON, fixed struct order, integer
units, explicit nulls, no maps/floats. Strict decoding plus re-encoding rejects
unknown/duplicate fields, missing optional nulls, alternate encodings and trailing
data. Maximum **envelope** size is 4 MiB, including checksum. Checksums are
integrity evidence, never authentication or execution permission.

| Type | Ordered fields |
| --- | --- |
| `JournalEnvelope` | `payload`, `checksum` |
| `JournalPayload::Anchor` | `version`, `store` (unpredictable 256-bit store ID) |
| `StorageSnapshot` | `version`, `store`, `sequence`, `previous`, `consumed_nonces`, `sessions` |
| `StoredSession` | `preparation`, `manifest`, `previous_terminal`, `nonce_consumed`, `consent`, `arm_commit`, `recovery`, `state`, `notices`, `calls` |
| `StoredConsent` | `approval` (exact `SimulatedApproval`), `boot_ms`, `utc_ms` |
| `ArmCommit` | `boot_ms`, `utc_ms` |
| `RecoveryWindow` | `trigger_ms`, `deadline_ms` |
| `StoredNotice` | `sequence`, `boot_ms`, `utc_ms`, `code` |
| `StoredCall` | `permit` (full immutable `Permission`), `intent_sequence`, `intent_utc_ms`, `completion` |
| `StoredCompletion` | `sequence`, `received_utc_ms`, `outcome` (atomic `Completion`) |
| Shared `FileIdentity` (port/custody evidence) | `volume` (u64), `id` (16 bytes) |

The shared existing types supply full preparation, synthetic manifest, process/
binary/device identity, immutable policy version, raw API results and actual
worker boot-clock entry/return observations. Guardian result-receipt UTC is not
relabeled as worker entry UTC. Missing values remain tagged unavailable, not zero.
No bootstrap key, approval secret, command path or native adapter is stored.

Storage version is **1**. Digest domains use the shared length-delimited
`AutoTune/recovery/v1\0` SHA-256 function: `journal-v1` for payloads,
`stored-terminal-v1` for complete terminal sessions, `stored-capture-v1` for the
ordered preparation call ledger, `storage-file-identity-v1` for canonical shared
file identity/manifest lock binding. `previous` is the preceding envelope checksum.
The legacy `Snapshot` type from the initial P0 foundation is **not accepted** by
the Windows port. All disk serialization uses these new types, not a port-owned
serializer. There is no CRC/rename/stdout shortcut to success.

Executable vectors in `crates/autotune-recovery-contract/tests/storage.rs`:

- Anchor with store `[1;32]`: checksum
  `0af0b6c4042917feb91f325b87355080266bd235e501967859a88f0b537c1b75`.
- The fixed preparation snapshot: canonical **payload** length 1149 bytes,
  checksum `e91f36d9876cf3be60847b3e1fce3875496d6bd5c2069cce47875c1280e8ae38`.
  This vector was independently calculated with Python `json`/`hashlib`.

## Preparation, history and admission

Provisioning creates an explicit empty-store anchor and stable `owner.lock`.
Normal opening never initializes either. The first snapshot, and the first
snapshot of each later session, is `Preparing`: real preparation capsule,
no fabricated manifest/B, no consumed approval, arm/recovery window, calls or
notices. Only separately published preparation-bound getter intents/results can
follow. Manifest sealing binds the prior completed capture ledger and its exact
two current-limit values/return timestamps; preparation cannot authorize setters.
The guardian/worker must still validate the full finite prerequisite chain.
The validator requires the explicit enumeration/identity/runtime/bounds/current
chain before a new setter intent; records of failed checks are retained but cannot
satisfy that chain. In storage tests these call observations/timestamps are
synthetic fixtures, not real-worker capture or timing evidence.

Global publication sequence increments exactly once per snapshot. Intents and
results have distinct increasing publication sequences; later calls cannot
precede completion of earlier ones. An intent cannot be rewritten, a completed
result cannot be replaced, notices cannot be erased, and per-session immutable
preparation/manifest/consent/arm commitment cannot drift. Raw late returns remain
evidence, not renewed authority. Historical grants remain immutable if a later
worker observation tightens the cleanup window.

Nonce consumption includes exact simulated approval and its original two-clock
observation; it must be present in a **prior** publication before an arm commitment
can be added. Arm commitment is frozen by the guardian, not receipt by the
coordinator. Its publication/ack must complete within the original ceilings.
Candidate intent requires consumed approval and the sealed manifest, at most one
candidate reservation. Restore grants require the frozen trigger/deadline and
are bounded by it at insertion. No raw setter value is accepted.

Limits: 256 retained sessions/nonces, 1024 namespace artifacts, 4 MiB/envelope,
one pending operation, one candidate and two restores. All history and consumed
nonces carry forward. New session opening requires the preceding complete terminal
digest. Terminal no-write forbids an entered/uncertain setter; terminal restoration
requires explicit restore and two exact fresh B reads at least 250 ms apart within
the recovery budget, no pending call. API failures/notices remain in the ledger.
No compaction, nonce reset, default/history fallback or infinite retry exists.

## Approved fixture boundary and concrete Windows opening policy

Only explicit random `recovery-storage-<64 hex digits>` fixture directories under
the approved `%LOCALAPPDATA%\Temp\opencode` scratch parent are provisioned.
There is no production-root API, installation, ACL change outside fresh fixtures,
elevation, alternate-root retry or machine-wide common-root claim.

1. Reject UNC, drive-relative, non-fixed/non-NTFS, ambiguous components, streams,
   separators in leaves and unexpected root names. `GetDriveTypeW` is advisory;
   filesystem and volume identity are checked **by handle** with
   `GetVolumeInformationByHandleW`/`FileIdInfo`.
2. Open the drive anchor then each component with user-mode **`NtCreateFile`**,
   `OBJECT_ATTRIBUTES.RootDirectory`, `OBJ_DONT_REPARSE`,
   `FILE_OPEN_REPARSE_POINT`, synchronous non-alertable and explicit directory/
   non-directory options. No canonicalized string is treated as security proof.
3. Hold every outer ancestor without write/delete sharing. Observe its owner/
   DACL by `GetSecurityInfo`, reject invalid/null security, and pin volume/file ID,
   type and reparse status. The outer scratch parent has permissive inherited
   sandbox ACEs on this host; it is **not** treated as a protected production root.
   Sibling creation is not authority over the already opened, pinned chain.
4. The fresh fixture anchor and **every leaf** have an atomically supplied,
   protected DACL and current-token owner. `OpenProcessToken`/`GetTokenInformation`
   supply the operator SID. `GetSecurityInfo`, `GetSecurityDescriptorControl`,
   `GetAce` and SID checks accept exactly three explicit full-access allow ACEs:
   operator, SYSTEM, Administrators; no inherited/deny/object/callback/unknown
   principal/ACE, null DACL or broad grant. This restrictive fixture policy is
   intentionally not a generalized Windows effective-access engine.
5. `GetFileInformationByHandleEx` checks `FileAttributeTagInfo`, `FileStandardInfo`
   and `FileIdInfo`: no reparse/device/directory leaf, pending deletion, hardlink
   (`NumberOfLinks != 1`), unexpected identity or changed volume. IDs are 64-bit
   volume serial plus 128-bit file ID. Directory ancestry remains held.
   Handle-based `FileStreamInfo` accepts only the unnamed data stream on leaves
   and no named data stream on the protected fixture root. Existing file/directory
   ADS are rejected; colon filtering alone is not treated as stream validation.

**Replace-compatible sharing refinement:** the protected fixture root allows
read/write sharing but not delete sharing; `ReplaceFileW` needs parent-directory
write access. Its strict DACL excludes untrusted writers/reparse modification.
Outer ancestors still deny write/delete sharing. File leaves allow replacement-
compatible sharing and are closed when required. A prototype read-only-shared
fixture directory produced Windows error 1175; no error was ignored or declared
durable. Same-account malicious code, administrators/SYSTEM, kernel/storage and
mount-manager compromise remain outside this boundary. Cross-user/production
ancestor-policy qualification remains G2, not inferred from these tests.

## Exclusive lifetime ownership

Open the pre-existing stable `owner.lock` with sharing **zero** before scanning.
Never replace/delete it, steal by PID or recreate a missing lock. `DuplicateHandle`
creates non-inheritable custody with only `FILE_READ_ATTRIBUTES | READ_CONTROL |
SYNCHRONIZE`; the worker performs no lock I/O. Local tests also use
`CompareObjectHandles`, not just matching file IDs. Cross-process tests duplicate
into a retained harness-owned process handle and attest identity/ACL in that child.

`attest_custody` requires a **trusted** handoff. Matching a file ID alone does not
authenticate a bootstrap or prove a peer. The future process port must bind the
creation identity, rights and actual duplicate to its measured authenticated child.
No coordinator duplicate or lock-stealing fallback is provided.

## Publication and actual-artifact scanner

One `Store` owner and one optional guardian-owned storage thread:

1. Revalidate root/ancestry and require the current active file to equal the
   already validated in-memory active chain (identity + exact bytes). Cold startup
   still scans every contiguous predecessor; live publication does not reread all
   history on every call (see latency note below).
2. Reserve stage/backup capacity; create a new same-directory stage with `FILE_CREATE`
   and the explicit protected descriptor. Write **all** bounded envelope bytes.
3. `File::sync_all` invokes real Windows **`FlushFileBuffers`**. Measure the stage
   file ID after writing; close it, reopen/read/check exact identity and bytes,
   then close incompatible leaf handles.
4. **`ReplaceFileW`**, same volume, flags **0**, retains a uniquely named predecessor
   `backup-<20-digit sequence>-<checksum>.json`. No ignored ACL/merge errors or
   unsupported `REPLACEFILE_WRITE_THROUGH` promise.
5. Reopen active handle, require the measured stage ID, audit security/type again,
   flush again, seek/read exact bytes. Require backup ID/bytes to equal the prior
   active object. Close, then check the bounded namespace (no stage, exact expected
   artifact names/count) and the new active bytes/identity before returning success.

The scanner uses handle-based `GetFileInformationByHandleEx` directory enumeration,
bounded buffers and relative opens. Cold startup requires active plus stable lock and every
contiguous predecessor down to the original anchor. File names, store ID, checksum,
sequence, prefix history, immutable fields, terminal/nonce linkage and record order
must agree. It streams predecessor envelopes rather than retaining 1024 snapshots
in RAM. An orphan stage (even complete), unknown file, missing active/backup,
conflicting/newer artifact, oversized file, unsupported schema or hardlink rejects.
No older clean record is selected; nothing is removed/repaired/replayed.

### In-session validation and the 100 ms publication ceiling

Full-history revalidation on every publication is **O(history)** and exceeded the
proposed 100 ms ceiling at ~14 snapshots; measured worst latency grew to ~1 s by
47 snapshots. A full fake round trip needs ~45 publications. The live publisher now
checks the pinned ancestry, the active file's exact identity/bytes against the
validated in-memory chain, then the new snapshot's `follows` lineage, the exact
stage-to-active identity, expected backup bytes and a bounded namespace (no stage,
exact expected artifact count/names). Cold startup still performs the complete
scan above. This makes publication roughly constant (observed worst ~30 ms at 47
publications) but is a deliberate in-session/ cold-start asymmetry requiring
architecture review; it does not claim OS-crash or power-loss durability.

### Cold startup versus original live ownership

`Scan.read_only_required` reports unresolved **data**. `Store.reconciliation_only`
is stronger: **all recovered snapshot history, even a valid terminal, is read-only**.
Only an initial anchor or the original live owner can publish. Clean two-session
rollover works within that same ownership lifetime.

Reason: a stalled terminal publisher may finish after the actor's deadline. A cold
scanner cannot prove the old guardian received an on-time acknowledgement merely
because terminal bytes look valid. It must not let that late write unblock a new
trial. There is deliberately no resume/bypass/human-reconciliation CLI. Supporting
fresh sessions after cold clean-history opening needs a separately reviewed
acknowledgement/reconciliation design; do not weaken this fail-closed policy.

## Thread timeout and shutdown

`StorageExecutor` owns a single bounded request/reply lane, original lock custody
and duplicate ancestry pins. Its thread owns **all** publisher handles. Request
deadline is boot interrupt time at submission +100 ms, capped by the guardian's
already frozen cleanup deadline; no receipt-relative rebase. `poll()` is nonblocking
and deadlines win ties. Failure/timeout quarantines the lane; no second request,
replacement publisher or late-success promotion. The old writer may finish, and
late completion is recorded only as such. RAM device cleanup remains the guardian's
job under its existing quiescence/budget rules, not a function of disk availability.

Orderly `shutdown`/Drop joins the sole thread before relinquishing original custody
and ancestry. Joining a stalled writer can block and must not run on the actor loop.
There is no independently surviving storage process. Process death closes these
application handles/threads; residual kernel I/O and namespace/cache persistence
remain G3, not a fencing or power-loss guarantee.

## Observed tests and limits

`crates/autotune-recovery-windows/tests/storage.rs` runs actual isolated NTFS
directories, broad/null/inherited ACL faults, a real junction, hardlinks, file and
directory ADS, root rename
denial, cross-process custody/exclusion, expected/unexpected file-ID replacement,
two-session nonce history, capacity limits, torn/corrupt artifacts and timed-out
storage threads. It kills only its own child test processes at **ten** distinct
publication barriers (including a real half-written stage), plus process loss with
a blocked storage thread before/after replacement. These are before/after API
barriers, **not** instrumentation inside the kernel's flush/replace implementation.

An actual late terminal publication is tested: valid terminal data remains cold-
startup read-only after timeout. Fixtures/artifacts are retained, never auto-repaired.
All values/identities/approvals are synthetic; no device operation is executed by
the storage port. See `windows-recovery.md` for results, hashes and W/M mapping.

Unavailable other-user/wrong-owner setups, production-root provisioning, native
quiescence/physical entry, actual OS crash and physical power loss are **not passes**.
The guardian/worker process/IPC/bootstrap and reducer-to-storage adaptation remain
separate integration work. G1–G7 and hardware admission remain closed.
