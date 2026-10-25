#![cfg(windows)]
#[path = "../../autotune-recovery-contract/tests/support/mod.rs"]
mod support;
use autotune_recovery_contract::{storage::*, *};
use autotune_recovery_windows::{storage::*, storage_executor::StorageExecutor};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

fn opening(previous: &JournalEnvelope) -> JournalEnvelope {
    JournalEnvelope::new(JournalPayload::Snapshot(StorageSnapshot {
        version: STORAGE_VERSION,
        store: previous.store(),
        sequence: previous.sequence() + 1,
        previous: previous.checksum,
        consumed_nonces: vec![],
        sessions: vec![StoredSession {
            preparation: support::preparation(),
            manifest: None,
            previous_terminal: None,
            nonce_consumed: false,
            consent: None,
            arm_commit: None,
            recovery: None,
            state: StoredState::Preparing,
            notices: vec![],
            calls: vec![],
        }],
    }))
    .unwrap()
}
fn next(previous: &JournalEnvelope, edit: impl FnOnce(&mut StorageSnapshot)) -> JournalEnvelope {
    let JournalPayload::Snapshot(mut snapshot) = previous.payload.clone() else {
        panic!("snapshot expected");
    };
    snapshot.sequence += 1;
    snapshot.previous = previous.checksum;
    edit(&mut snapshot);
    JournalEnvelope::new(JournalPayload::Snapshot(snapshot)).unwrap()
}
fn publish(store: &mut Store, edit: impl FnOnce(&mut StorageSnapshot)) {
    let n = next(&store.scan().active, edit);
    store.publish(n).unwrap();
}
fn captured_store() -> (PathBuf, Store) {
    let root = TestRoot::provision_fixture().unwrap();
    let mut store = Store::open(&root).unwrap();
    store.publish(opening(&store.scan().active)).unwrap();
    capture_and_arm(&mut store, 247_123);
    (root, store)
}
fn capture_and_arm(store: &mut Store, baseline: u32) {
    let JournalPayload::Snapshot(snapshot) = &store.scan().active.payload else {
        panic!()
    };
    let index = snapshot.sessions.len() - 1;
    let prep = snapshot.sessions[index].preparation.clone();
    let operations = [
        (
            Operation::Enumerate,
            RawResult::Identity(prep.device.clone()),
            0,
        ),
        (
            Operation::Identity,
            RawResult::Identity(prep.device.clone()),
            0,
        ),
        (
            Operation::Runtime,
            RawResult::Runtime(support::runtime()),
            0,
        ),
        (
            Operation::Bounds,
            RawResult::Bounds {
                minimum_mw: 100_000,
                maximum_mw: 300_000,
            },
            0,
        ),
        (Operation::Current, RawResult::Value(baseline), 0),
        (Operation::Current, RawResult::Value(baseline), 500),
    ];
    for (i, (operation, result, time)) in operations.into_iter().enumerate() {
        let time = time + prep.created_ms;
        publish(store, |s| {
            s.sessions[index].calls.push(StoredCall {
                permit: Permission {
                    session: prep.session,
                    boot: prep.boot,
                    digest: prep.digest().unwrap(),
                    generation: 1,
                    operation_id: i as u64 + 1,
                    operation,
                    dispatched_ms: time,
                    watchdog_ms: time + 1000,
                    expiry_ms: time + 1000,
                    recovery_trigger_ms: None,
                },
                intent_sequence: s.sequence,
                intent_utc_ms: time,
                completion: None,
            });
        });
        publish(store, |s| {
            s.sessions[index].calls.last_mut().unwrap().completion = Some(StoredCompletion {
                sequence: s.sequence,
                received_utc_ms: time,
                outcome: Completion::Returned {
                    entry_ms: time,
                    return_ms: time,
                    result,
                },
            });
        });
    }
    publish(store, |s| {
        let mut m = support::manifest(baseline);
        m.preparation = prep.clone();
        m.preparation_digest = prep.digest().unwrap();
        m.capture.sampled_ms = [prep.created_ms, prep.created_ms + 500];
        m.approval_expiry_utc_ms = prep.created_ms + 60_000;
        m.capture.trace = s.sessions[index].capture_trace().unwrap();
        s.sessions[index].manifest = Some(m);
        s.sessions[index].state = StoredState::ManifestReady;
    });
    publish(store, |s| {
        s.sessions[index].nonce_consumed = true;
        s.sessions[index].consent = Some(StoredConsent {
            approval: support::approval(s.sessions[index].manifest.as_ref().unwrap()),
            boot_ms: prep.created_ms + 500,
            utc_ms: prep.created_ms + 500,
        });
        s.consumed_nonces.push(prep.nonce);
    });
    publish(store, |s| {
        s.sessions[index].state = StoredState::Armed;
        s.sessions[index].arm_commit = Some(ArmCommit {
            boot_ms: prep.created_ms + 500,
            utc_ms: prep.created_ms + 500,
        });
    });
    let final_digest = match &store.scan().active.payload {
        JournalPayload::Snapshot(s) => s.sessions[index]
            .manifest
            .as_ref()
            .unwrap()
            .digest()
            .unwrap(),
        _ => unreachable!(),
    };
    for (operation, result) in [
        (
            Operation::Enumerate,
            RawResult::Identity(prep.device.clone()),
        ),
        (
            Operation::Identity,
            RawResult::Identity(prep.device.clone()),
        ),
        (Operation::Runtime, RawResult::Runtime(support::runtime())),
        (
            Operation::Bounds,
            RawResult::Bounds {
                minimum_mw: 100_000,
                maximum_mw: 300_000,
            },
        ),
        (Operation::Current, RawResult::Value(baseline)),
    ] {
        let time = prep.created_ms + 500;
        publish(store, |s| {
            let id = s.sessions[index].calls.last().unwrap().permit.operation_id + 1;
            s.sessions[index].calls.push(StoredCall {
                permit: Permission {
                    session: prep.session,
                    boot: prep.boot,
                    digest: final_digest,
                    generation: 1,
                    operation_id: id,
                    operation,
                    dispatched_ms: time,
                    watchdog_ms: time + 1000,
                    expiry_ms: time + 1000,
                    recovery_trigger_ms: None,
                },
                intent_sequence: s.sequence,
                intent_utc_ms: time,
                completion: None,
            });
        });
        publish(store, |s| {
            s.sessions[index].calls.last_mut().unwrap().completion = Some(StoredCompletion {
                sequence: s.sequence,
                received_utc_ms: time,
                outcome: Completion::Returned {
                    entry_ms: time,
                    return_ms: time,
                    result,
                },
            })
        });
    }
}
fn mutation_intent(previous: &JournalEnvelope) -> JournalEnvelope {
    next(previous, |s| {
        let session = &mut s.sessions[0];
        let m = session.manifest.as_ref().unwrap();
        session.state = StoredState::MutationPossible;
        session.calls.push(StoredCall {
            permit: Permission {
                session: m.preparation.session,
                boot: m.preparation.boot,
                digest: m.digest().unwrap(),
                generation: 1,
                operation_id: session.calls.last().unwrap().permit.operation_id + 1,
                operation: Operation::CandidateOnce,
                dispatched_ms: 501,
                watchdog_ms: 1501,
                expiry_ms: 1501,
                recovery_trigger_ms: None,
            },
            intent_sequence: s.sequence,
            intent_utc_ms: 501,
            completion: None,
        });
    })
}

#[test]
fn actual_fixture_lock_and_replacement_preserve_ids_bytes_baseline_and_nonce() {
    let (root, mut store) = captured_store();
    assert!(Store::open(&root).is_err());
    let original = store.scan().active_identity;
    let custody = store.duplicate_custody().unwrap();
    assert!(store.same_lock_object(&custody));
    attest_custody(custody.as_handle(), custody.identity).unwrap();
    let result = store
        .publish(mutation_intent(&store.scan().active))
        .unwrap();
    assert_ne!(result.active_identity, original);
    assert!(result.read_only_required);
    let JournalPayload::Snapshot(s) = &result.active.payload else {
        panic!();
    };
    assert_eq!(
        s.sessions[0]
            .manifest
            .as_ref()
            .unwrap()
            .capture
            .baseline()
            .unwrap(),
        247_123
    );
    assert_eq!(s.consumed_nonces, [support::preparation().nonce]);
    drop(store);
    assert!(Store::open(&root).is_err()); // SAME OS file object survives original owner closure.
    drop(custody);
    let mut reopened = Store::open(&root).unwrap();
    assert!(reopened.scan().read_only_required);
    assert_eq!(reopened.scan().active, result.active);
    assert!(reopened.reconciliation_only());
    let same = next(&result.active, |_| {});
    assert!(matches!(
        reopened.publish(same),
        Err(StorageError::ReadOnlyReconciliation)
    ));
    println!("retained fixture {}", root.display());
}

#[test]
fn actual_two_session_rollover_preserves_terminal_link_and_cumulative_nonces() {
    let (root, mut store) = captured_store();
    publish(&mut store, |s| {
        s.sessions[0].state = StoredState::AbortedNoWrite
    });
    publish(&mut store, |s| {
        let previous = s.sessions[0].terminal_digest().unwrap();
        let mut p = support::preparation();
        p.nonce = [81; 32];
        p.session = [82; 32];
        p.created_ms = 1000;
        s.sessions.push(StoredSession {
            preparation: p,
            manifest: None,
            previous_terminal: Some(previous),
            nonce_consumed: false,
            consent: None,
            arm_commit: None,
            recovery: None,
            state: StoredState::Preparing,
            notices: vec![],
            calls: vec![],
        });
    });
    capture_and_arm(&mut store, 230_000);
    publish(&mut store, |s| {
        s.sessions[1].state = StoredState::AbortedNoWrite
    });
    let final_record = store.scan().active.clone();
    assert!(!store.scan().read_only_required);
    let JournalPayload::Snapshot(s) = &final_record.payload else {
        panic!()
    };
    assert_ne!(s.sessions[0].manifest, s.sessions[1].manifest);
    assert_eq!(s.consumed_nonces, [support::preparation().nonce, [81; 32]]);
    let mut replay = s.clone();
    replay.sequence += 1;
    replay.previous = final_record.checksum;
    let previous = replay.sessions[1].terminal_digest().unwrap();
    replay.sessions.push(StoredSession {
        preparation: support::preparation(),
        manifest: None,
        previous_terminal: Some(previous),
        nonce_consumed: false,
        consent: None,
        arm_commit: None,
        recovery: None,
        state: StoredState::Preparing,
        notices: vec![],
        calls: vec![],
    });
    assert!(JournalEnvelope::new(JournalPayload::Snapshot(replay)).is_err());
    drop(store);
    let reopened = Store::open(&root).unwrap();
    assert_eq!(reopened.scan().active, final_record);
    drop(reopened);
    // Recompute integrity after malicious history alteration: a valid checksum is NOT authority.
    let mut tampered = s.clone();
    tampered.consumed_nonces.remove(0);
    let payload = JournalPayload::Snapshot(tampered);
    let checksum = digest(b"journal-v1", &canonical(&payload).unwrap());
    fs::write(
        root.join("journal.json"),
        canonical(&JournalEnvelope { payload, checksum }).unwrap(),
    )
    .unwrap();
    assert!(Store::open(&root).is_err());
}

#[test]
fn unexpected_identical_bytes_leaf_id_swap_is_not_a_valid_publication() {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{GetSecurityInfo, SetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        },
        Storage::FileSystem::{READ_CONTROL, WRITE_DAC},
    };
    let (root, mut store) = captured_store();
    let next = mutation_intent(&store.scan().active);
    let clone = root.join("unexpected-copy.json");
    fs::copy(root.join("journal.json"), &clone).unwrap();
    let source = fs::File::open(root.join("journal.json")).unwrap();
    let destination = fs::OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .open(&clone)
        .unwrap();
    let mut descriptor = std::ptr::null_mut();
    let mut acl = std::ptr::null_mut();
    // SAFETY: handles are fresh fixture-owned files; descriptor/ACL lives until copied.
    unsafe {
        assert_eq!(
            GetSecurityInfo(
                source.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor
            ),
            0
        );
        assert_eq!(
            SetSecurityInfo(
                destination.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl,
                std::ptr::null_mut()
            ),
            0
        );
        LocalFree(descriptor);
    }
    drop(source);
    drop(destination);
    fs::rename(clone, root.join("journal.json")).unwrap();
    assert!(matches!(
        store.publish(next.clone()),
        Err(StorageError::Invalid("unexpected active replacement"))
    ));
    assert!(matches!(store.publish(next), Err(StorageError::Poisoned)));
}

#[test]
fn actual_scanner_rejects_torn_extra_missing_history_and_unknown_schema_without_repair() {
    for fault in [
        "stage",
        "truncated-active",
        "missing-active",
        "missing-backup",
        "schema",
        "hardlink",
        "ads",
        "directory-ads",
    ] {
        let (root, store) = captured_store();
        drop(store);
        match fault {
            "stage" => fs::write(root.join("stage-orphan.json"), b"partial").unwrap(),
            "truncated-active" => fs::write(root.join("journal.json"), b"{\"payload\":").unwrap(),
            "missing-active" => {
                fs::rename(root.join("journal.json"), root.join("saved-active.json")).unwrap()
            }
            "missing-backup" => {
                let leaf = fs::read_dir(&root)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("backup-00000000000000000000-")
                    })
                    .unwrap();
                fs::rename(leaf, root.join("unexplained-anchor.json")).unwrap();
            }
            "schema" => {
                let bytes = fs::read(root.join("journal.json")).unwrap();
                let text = String::from_utf8(bytes).unwrap().replacen(
                    "\"version\":1",
                    "\"version\":99",
                    1,
                );
                fs::write(root.join("journal.json"), text).unwrap();
            }
            "hardlink" => {
                fs::hard_link(root.join("journal.json"), root.join("extra-link.json")).unwrap()
            }
            "ads" => fs::write(
                root.join("journal.json:unexpected"),
                b"unexplained named stream",
            )
            .unwrap(),
            "directory-ads" => fs::write(
                format!("{}:unexpected", root.display()),
                b"unexplained directory stream",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let before: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(Store::open(&root).is_err(), "{fault}");
        let after: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(before, after, "scanner modified {fault}");
    }
}

#[test]
fn actual_root_acl_and_missing_lock_reject_before_any_publication() {
    let root = TestRoot::provision_fixture().unwrap();
    fs::rename(root.join("owner.lock"), root.join("saved.lock")).unwrap();
    assert!(Store::open(&root).is_err());
    let root = TestRoot::provision_fixture().unwrap();
    // Replace protected file by a normally inherited file: root inheritance produces
    // inherited ACEs and unprotected DACL, so the strict per-object audit must reject.
    fs::rename(root.join("owner.lock"), root.join("saved.lock")).unwrap();
    fs::write(root.join("owner.lock"), b"").unwrap();
    assert!(Store::open(&root).is_err());
    assert!(TestRoot::open(&root.join("..")).is_err());
    assert!(TestRoot::open(Path::new("\\\\server\\share")).is_err());
}

#[test]
fn actual_namespace_and_snapshot_resource_limits_fail_closed() {
    let root = TestRoot::provision_fixture().unwrap();
    for n in 0..ARTIFACT_LIMIT {
        fs::write(root.join(format!("unexplained-{n:04}.json")), b"").unwrap();
    }
    assert!(matches!(
        Store::open(&root),
        Err(StorageError::Invalid("artifact capacity"))
    ));
    let root = TestRoot::provision_fixture().unwrap();
    fs::write(root.join("journal.json"), vec![b' '; SNAPSHOT_LIMIT + 1]).unwrap();
    assert!(matches!(
        Store::open(&root),
        Err(StorageError::Invalid("read limit/identity"))
    ));
}

struct SavedDacl {
    file: fs::File,
    descriptor: *mut std::ffi::c_void,
    acl: *mut windows_sys::Win32::Security::ACL,
}
impl Drop for SavedDacl {
    fn drop(&mut self) {
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                Authorization::{SetSecurityInfo, SE_FILE_OBJECT},
                DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
            },
        };
        // SAFETY: descriptor owns the ACL until restoration completes; file remains live.
        unsafe {
            assert_eq!(
                SetSecurityInfo(
                    self.file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    self.acl,
                    std::ptr::null_mut()
                ),
                0
            );
            LocalFree(self.descriptor);
        }
    }
}
fn set_test_dacl(path: &Path, sddl: Option<&str>) -> SavedDacl {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
                SetSecurityInfo, SE_FILE_OBJECT,
            },
            GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION,
        },
        Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL, WRITE_DAC,
        },
    };
    let file = fs::OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .unwrap();
    let mut old = std::ptr::null_mut();
    let mut acl = std::ptr::null_mut();
    // SAFETY: fixture-owned live handle, outputs allocated security descriptor and borrowed ACL.
    unsafe {
        assert_eq!(
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut old
            ),
            0
        );
    }
    let saved = SavedDacl {
        file,
        descriptor: old,
        acl,
    };
    let mut replacement = std::ptr::null_mut();
    let mut new_acl = std::ptr::null_mut();
    if let Some(sddl) = sddl {
        let text: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
        let mut present = 0;
        let mut defaulted = 0;
        // SAFETY: valid terminated literal SDDL, allocation retained until SetSecurityInfo copies it.
        unsafe {
            assert_ne!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    text.as_ptr(),
                    1,
                    &mut replacement,
                    std::ptr::null_mut()
                ),
                0
            );
            assert_ne!(
                GetSecurityDescriptorDacl(replacement, &mut present, &mut new_acl, &mut defaulted),
                0
            );
        }
    }
    // SAFETY: only this fresh isolated fixture's DACL is changed; null intentionally tests null-DACL rejection.
    unsafe {
        assert_eq!(
            SetSecurityInfo(
                saved.file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_acl,
                std::ptr::null_mut()
            ),
            0
        );
        if !replacement.is_null() {
            LocalFree(replacement);
        }
    }
    saved
}

#[test]
fn actual_broad_null_and_inherited_acl_rejection_is_by_handle() {
    for leaf in [None, Some("journal.json"), Some("owner.lock")] {
        for acl in [None, Some("D:P(A;OICI;FA;;;WD)")] {
            let root = TestRoot::provision_fixture().unwrap();
            let path = leaf.map_or_else(|| root.clone(), |leaf| root.join(leaf));
            let saved = set_test_dacl(&path, acl);
            assert!(
                Store::open(&root).is_err(),
                "insecure DACL accepted on {leaf:?}"
            );
            drop(saved);
            assert!(Store::open(&root).is_ok(), "DACL restoration failed");
        }
    }
}

#[test]
fn actual_junction_root_and_ancestor_rename_are_rejected() {
    use windows_sys::Win32::{
        Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT},
        System::IO::DeviceIoControl,
    };
    let root = TestRoot::provision_fixture().unwrap();
    let target = TestRoot::provision_fixture().unwrap();
    let store = Store::open(&root).unwrap();
    assert!(
        fs::rename(&root, root.with_extension("moved")).is_err(),
        "held root allowed rename"
    );
    drop(store);
    fs::rename(root.join("journal.json"), target.join("saved-journal.json")).unwrap();
    fs::rename(root.join("owner.lock"), target.join("saved-owner.lock")).unwrap();
    let substitute: Vec<u16> = format!("\\??\\{}", target.display())
        .encode_utf16()
        .collect();
    let print: Vec<u16> = target.to_str().unwrap().encode_utf16().collect();
    let data_len = 8 + (substitute.len() + 1 + print.len() + 1) * 2;
    let mut data = Vec::new();
    data.extend(0xa0000003u32.to_le_bytes());
    data.extend((data_len as u16).to_le_bytes());
    data.extend(0u16.to_le_bytes());
    data.extend(0u16.to_le_bytes());
    data.extend(((substitute.len() * 2) as u16).to_le_bytes());
    data.extend((((substitute.len() + 1) * 2) as u16).to_le_bytes());
    data.extend(((print.len() * 2) as u16).to_le_bytes());
    for c in substitute.into_iter().chain([0]).chain(print).chain([0]) {
        data.extend(c.to_le_bytes());
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&root)
        .unwrap();
    let mut returned = 0;
    // SAFETY: well-formed mount-point reparse buffer, exclusively harness-owned empty directory;
    // target is another fresh fixture in the same approved scratch parent, never a system path.
    assert_ne!(
        unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                0x900a4,
                data.as_ptr().cast(),
                data.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        },
        0,
        "junction creation requires no elevation: {}",
        std::io::Error::last_os_error()
    );
    drop(file);
    assert!(matches!(
        TestRoot::open(&root),
        Err(StorageError::Nt(_)) | Err(StorageError::Invalid(_))
    ));
    // Include the junction as an intermediate path to exercise component-wise rejection.
    assert!(TestRoot::open(&root.join("child")).is_err());
    assert!(fs::read(target.join("journal.json"))
        .unwrap()
        .starts_with(b"{\"payload\""));
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn child(root: &Path, kind: &str, step: &str) -> (ChildGuard, mpsc::Receiver<String>) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    std::os::windows::process::CommandExt::creation_flags(
        &mut command,
        windows_sys::Win32::System::Threading::CREATE_NO_WINDOW,
    );
    command
        .args(["--exact", "storage_child", "--nocapture"])
        .env("AUTO_STORAGE_ROOT", root)
        .env("AUTO_STORAGE_KIND", kind)
        .env("AUTO_STORAGE_STEP", step)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().unwrap();
    let mut created = windows_sys::Win32::Foundation::FILETIME::default();
    let mut exited = windows_sys::Win32::Foundation::FILETIME::default();
    let mut kernel = windows_sys::Win32::Foundation::FILETIME::default();
    let mut user = windows_sys::Win32::Foundation::FILETIME::default();
    // SAFETY: this retained Child handle belongs to a process created by this test.
    assert_ne!(
        unsafe {
            windows_sys::Win32::System::Threading::GetProcessTimes(
                child.as_raw_handle(),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )
        },
        0
    );
    let creation = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
    println!("owned_process harness_pid={} child_pid={} creation_100ns={creation} kind={kind} step={step} fixture={}",std::process::id(),child.id(),root.display());
    let output = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    (ChildGuard(child), rx)
}
fn barrier(rx: &mpsc::Receiver<String>) {
    // Startup constructs complete fixture history under real host scheduling.
    // This harness-only wait is not a device/publication/recovery deadline.
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let line = rx
            .recv_timeout(end.saturating_duration_since(Instant::now()))
            .expect("child barrier timed out");
        if line.contains("STORAGE-BARRIER") {
            break;
        }
    }
}
#[test]
fn actual_process_crashes_at_every_publication_step_retain_baseline_and_uncertainty() {
    for step in [
        "StageCreated",
        "PartialWrite",
        "Written",
        "FirstFlush",
        "BeforeReplace",
        "Replaced",
        "Reopened",
        "SecondFlush",
        "Readback",
        "Closed",
    ] {
        let root = TestRoot::provision_fixture().unwrap();
        let (mut child, rx) = child(&root, "publish", step);
        barrier(&rx);
        assert!(Store::open(&root).is_err());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        if matches!(
            step,
            "Replaced" | "Reopened" | "SecondFlush" | "Readback" | "Closed"
        ) {
            let scanned = Store::open(&root).unwrap();
            assert!(scanned.scan().read_only_required);
            let JournalPayload::Snapshot(s) = &scanned.scan().active.payload else {
                panic!()
            };
            assert_eq!(
                s.sessions[0].manifest.as_ref().unwrap().capture.readings_mw,
                [247_123; 2]
            );
            assert!(s.sessions[0].calls.last().unwrap().completion.is_none());
        } else {
            assert!(Store::open(&root).is_err(), "must retain orphan {step}");
        }
        println!("crash {step}: {}", root.display());
    }
}

#[test]
fn actual_cross_process_duplicate_lock_custody_outlives_original_owner() {
    let root = TestRoot::provision_fixture().unwrap();
    let store = Store::open(&root).unwrap();
    let (mut child, rx) = child(&root, "custody", "");
    barrier(&rx);
    let remote = store.duplicate_custody_into(child.0.as_handle()).unwrap();
    let text = format!(
        "{} {} {}\n",
        remote.handle,
        remote.identity.volume,
        remote
            .identity
            .id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    child
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    barrier(&rx);
    drop(store);
    assert!(Store::open(&root).is_err());
    child
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"release\n")
        .unwrap();
    assert!(child.0.wait().unwrap().success());
    assert!(Store::open(&root).is_ok());
}

#[test]
fn actual_process_exit_also_stops_the_guardian_owned_storage_thread() {
    for step in ["FirstFlush", "Replaced"] {
        let root = TestRoot::provision_fixture().unwrap();
        let (mut child, rx) = child(&root, "thread-publish", step);
        barrier(&rx);
        assert!(Store::open(&root).is_err());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        if step == "FirstFlush" {
            // Typed scanner rejection means lock acquisition succeeded after process exit.
            assert!(matches!(
                Store::open(&root),
                Err(StorageError::Invalid("unexplained stage/artifact"))
            ));
        } else {
            assert!(Store::open(&root).unwrap().scan().read_only_required);
        }
    }
}

#[test]
fn real_storage_thread_timeout_quarantines_late_publish_and_retains_ownership() {
    let root = TestRoot::provision_fixture().unwrap();
    let store = Store::open(&root).unwrap();
    let snapshot = opening(&store.scan().active);
    let (entered, rx) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let mut executor = StorageExecutor::start_with(store, move |step| {
        if step == PublicationStep::FirstFlush {
            entered.send(()).unwrap();
            wait.recv().unwrap();
        }
        Ok(())
    })
    .unwrap();
    executor.submit(snapshot.clone(), None).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        executor.submit(snapshot.clone(), None),
        Err(StorageError::Busy)
    );
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match executor.poll() {
            Err(StorageError::Timeout) => break,
            Ok(None) => {
                assert!(Instant::now() < end);
                std::thread::yield_now();
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(executor.quarantined());
    assert!(Store::open(&root).is_err());
    assert_eq!(executor.submit(snapshot, None), Err(StorageError::Poisoned));
    release.send(()).unwrap();
    while !executor.late_completion_observed() {
        let _ = executor.poll();
        assert!(Instant::now() < end);
        std::thread::yield_now();
    }
    assert!(Store::open(&root).is_err());
    executor.shutdown().unwrap();
    assert!(Store::open(&root).unwrap().scan().read_only_required);
}

#[test]
fn storage_child() {
    let Some(root) = std::env::var_os("AUTO_STORAGE_ROOT") else {
        return;
    };
    let kind = std::env::var("AUTO_STORAGE_KIND").unwrap();
    if kind == "custody" {
        println!("STORAGE-BARRIER ready");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        let fields: Vec<_> = line.split_whitespace().collect();
        let raw: usize = fields[0].parse().unwrap();
        let volume: u64 = fields[1].parse().unwrap();
        let mut id = [0u8; 16];
        for (i, b) in id.iter_mut().enumerate() {
            *b = u8::from_str_radix(&fields[2][i * 2..i * 2 + 2], 16).unwrap();
        }
        // SAFETY: test parent transferred exactly one DuplicateHandle-owned handle into this process.
        let custody = unsafe { OwnedHandle::from_raw_handle(raw as *mut std::ffi::c_void) };
        attest_custody(custody.as_handle(), FileIdentity { volume, id }).unwrap();
        println!("STORAGE-BARRIER custody");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_line(&mut String::new()).unwrap();
        drop(custody);
        return;
    }
    let mut store = Store::open(Path::new(&root)).unwrap();
    // This child owns the session from its initial anchor. Startup never resumes
    // an unresolved journal or turns imported state into permission.
    store.publish(opening(&store.scan().active)).unwrap();
    capture_and_arm(&mut store, 247_123);
    let step = std::env::var("AUTO_STORAGE_STEP").unwrap();
    let snapshot = mutation_intent(&store.scan().active);
    let barrier = move |current| {
        if format!("{current:?}") == step {
            println!("STORAGE-BARRIER {step}");
            std::io::stdout().flush().unwrap();
            let mut byte = [0];
            std::io::stdin().read_exact(&mut byte).unwrap();
        }
        Ok(())
    };
    if kind == "thread-publish" {
        let mut executor = StorageExecutor::start_with(store, barrier).unwrap();
        executor.submit(snapshot, None).unwrap();
        loop {
            let _ = executor.poll();
            std::thread::park_timeout(Duration::from_millis(10));
        }
    } else {
        store.publish_with(snapshot, barrier).unwrap();
    }
}

#[test]
fn late_terminal_write_after_timeout_cannot_authorize_a_cold_new_session() {
    let (root, store) = captured_store();
    let terminal = next(&store.scan().active, |s| {
        s.sessions[0].state = StoredState::AbortedNoWrite
    });
    let (entered, rx) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let mut executor = StorageExecutor::start_with(store, move |step| {
        if step == PublicationStep::Replaced {
            entered.send(()).unwrap();
            wait.recv().unwrap();
        }
        Ok(())
    })
    .unwrap();
    executor.submit(terminal, None).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match executor.poll() {
            Err(StorageError::Timeout) => break,
            Ok(None) => {
                assert!(Instant::now() < end);
                std::thread::yield_now();
            }
            other => panic!("{other:?}"),
        }
    }
    release.send(()).unwrap();
    while !executor.late_completion_observed() {
        let _ = executor.poll();
        assert!(Instant::now() < end);
        std::thread::yield_now();
    }
    assert!(matches!(executor.late_outcome(), Some(Ok(_))));
    executor.shutdown().unwrap();
    let mut cold = Store::open(&root).unwrap();
    assert!(
        !cold.scan().read_only_required,
        "record is structurally terminal"
    );
    assert!(
        cold.reconciliation_only(),
        "cold scan cannot infer an on-time actor ack"
    );
    let new = next(&cold.scan().active, |_| {});
    assert!(matches!(
        cold.publish(new),
        Err(StorageError::ReadOnlyReconciliation)
    ));
}
