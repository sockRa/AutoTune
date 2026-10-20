mod support;
use autotune_recovery_contract::{storage::*, *};
fn anchor() -> JournalEnvelope {
    JournalEnvelope::new(JournalPayload::Anchor {
        version: STORAGE_VERSION,
        store: [1; 32],
    })
    .unwrap()
}
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
fn change(old: &JournalEnvelope, f: impl FnOnce(&mut StorageSnapshot)) -> JournalEnvelope {
    let JournalPayload::Snapshot(mut s) = old.payload.clone() else {
        panic!()
    };
    s.sequence += 1;
    s.previous = old.checksum;
    f(&mut s);
    JournalEnvelope::new(JournalPayload::Snapshot(s)).unwrap()
}
#[test]
fn frozen_anchor_bytes_digest_and_strict_preparation_round_trip() {
    let id = FileIdentity {
        volume: 7,
        id: [2; 16],
    };
    assert_eq!(
        canonical(&id).unwrap(),
        br#"{"volume":7,"id":[2,2,2,2,2,2,2,2,2,2,2,2,2,2,2,2]}"#
    );
    assert_eq!(
        id.digest().unwrap(),
        digest(b"storage-file-identity-v1", &canonical(&id).unwrap())
    );
    let anchor = anchor();
    assert_eq!(canonical(&anchor.payload).unwrap(),br#"{"Anchor":{"version":1,"store":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]}}"#);
    assert_eq!(
        anchor.checksum,
        [
            0x0a, 0xf0, 0xb6, 0xc4, 0x04, 0x29, 0x17, 0xfe, 0xb9, 0x1f, 0x32, 0x5b, 0x87, 0x35,
            0x50, 0x80, 0x26, 0x6b, 0xd2, 0x35, 0xe5, 0x01, 0x96, 0x78, 0x59, 0xa8, 0x8f, 0x0b,
            0x53, 0x7c, 0x1b, 0x75
        ]
    );
    let prep = opening(&anchor);
    prep.follows(&anchor).unwrap();
    assert!(prep.read_only_required());
    // Independent Python json/hashlib vector, fixed field order and compact UTF-8.
    assert_eq!(canonical(&prep.payload).unwrap().len(), 1149);
    assert_eq!(
        prep.checksum,
        [
            0xe9, 0x1f, 0x36, 0xd9, 0x87, 0x6c, 0xf3, 0xbe, 0x60, 0x84, 0x7b, 0x3e, 0x1f, 0xce,
            0x38, 0x75, 0x49, 0x6d, 0x6b, 0xd5, 0xc2, 0x06, 0x9c, 0xce, 0x47, 0x87, 0x5c, 0x12,
            0x80, 0xe8, 0xae, 0x38
        ]
    );
    let bytes = canonical(&prep).unwrap();
    let decoded: JournalEnvelope = decode(&bytes, SNAPSHOT_LIMIT).unwrap();
    assert_eq!(decoded, prep);
    let text = String::from_utf8(bytes).unwrap().replacen(
        "\"nonce_consumed\":false",
        "\"nonce_consumed\":false,\"extra\":0",
        1,
    );
    assert!(decode::<JournalEnvelope>(text.as_bytes(), SNAPSHOT_LIMIT).is_err());
}
#[test]
fn initial_publication_cannot_skip_preparation_or_invent_a_manifest() {
    let anchor = anchor();
    let prep = opening(&anchor);
    let bad = change(&prep, |s| {
        s.sessions[0].manifest = Some(support::manifest(247_123));
        s.sessions[0].state = StoredState::ManifestReady;
    });
    assert!(bad.follows(&prep).is_err());
    let JournalPayload::Snapshot(mut snapshot) = bad.payload.clone() else {
        panic!()
    };
    snapshot.sequence = 1;
    snapshot.previous = anchor.checksum;
    let bad = JournalEnvelope::new(JournalPayload::Snapshot(snapshot)).unwrap();
    assert!(bad.follows(&anchor).is_err());
}
#[test]
fn global_sequence_pending_result_and_immutable_intent_are_checked() {
    let a = anchor();
    let prep = opening(&a);
    let p = support::preparation();
    let intent = change(&prep, |s| {
        s.sessions[0].calls.push(StoredCall {
            permit: Permission {
                session: p.session,
                boot: p.boot,
                digest: p.digest().unwrap(),
                generation: 1,
                operation_id: 1,
                operation: Operation::Current,
                dispatched_ms: 0,
                watchdog_ms: 1000,
                expiry_ms: 1000,
                recovery_trigger_ms: None,
            },
            intent_sequence: s.sequence,
            intent_utc_ms: 0,
            completion: None,
        })
    });
    intent.follows(&prep).unwrap();
    let result = change(&intent, |s| {
        s.sessions[0].calls[0].completion = Some(StoredCompletion {
            sequence: s.sequence,
            received_utc_ms: 1,
            outcome: Completion::Returned {
                entry_ms: 0,
                return_ms: 1,
                result: RawResult::Value(247_123),
            },
        })
    });
    result.follows(&intent).unwrap();
    let changed = change(&result, |s| s.sessions[0].calls[0].permit.generation += 1);
    assert!(changed.follows(&result).is_err());
    let changed = change(&result, |s| {
        s.sessions[0].calls[0].completion.as_mut().unwrap().outcome = Completion::NotEntered {
            reason: "rewrite".into(),
        }
    });
    assert!(changed.follows(&result).is_err());
}
#[test]
fn terminal_rollover_is_linked_and_nonce_replay_or_history_erasure_rejects() {
    let a = anchor();
    let prep = opening(&a);
    let closed = change(&prep, |s| s.sessions[0].state = StoredState::AbortedNoWrite);
    closed.follows(&prep).unwrap();
    assert!(!closed.read_only_required());
    let rolled = change(&closed, |s| {
        let previous = s.sessions[0].terminal_digest().unwrap();
        let mut p = support::preparation();
        p.session = [31; 32];
        p.nonce = [32; 32];
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
    rolled.follows(&closed).unwrap();
    let erased = change(&rolled, |s| {
        s.sessions.remove(0);
        s.sessions[0].previous_terminal = None;
    });
    assert!(erased.follows(&rolled).is_err());
    let JournalPayload::Snapshot(mut duplicate) = rolled.payload.clone() else {
        panic!()
    };
    duplicate.sessions[1].preparation.nonce = duplicate.sessions[0].preparation.nonce;
    assert!(JournalEnvelope::new(JournalPayload::Snapshot(duplicate)).is_err());
}

#[test]
fn fault_and_timing_notices_are_append_only_even_with_a_recomputed_checksum() {
    let a = anchor();
    let prep = opening(&a);
    let fault = change(&prep, |s| {
        s.sessions[0].state = StoredState::RecoveryRequired;
        s.sessions[0].notices.push(StoredNotice {
            sequence: s.sequence,
            boot_ms: 5000,
            utc_ms: 1,
            code: NoticeCode::Clock,
        });
    });
    fault.follows(&prep).unwrap();
    let erased = change(&fault, |s| s.sessions[0].notices.clear());
    assert!(erased.follows(&fault).is_err());
    let changed = change(&fault, |s| s.sessions[0].notices[0].utc_ms = 6000);
    assert!(changed.follows(&fault).is_err());
}
