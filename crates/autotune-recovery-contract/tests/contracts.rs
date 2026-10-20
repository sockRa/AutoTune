mod support;
use autotune_recovery_contract::*;
use support::*;

#[test]
fn canonical_golden_bytes_and_lengths() {
    let bytes = canonical(&RawResult::Api { status: -7 }).unwrap();
    assert_eq!(bytes, br#"{"Api":{"status":-7}}"#);
    let bytes = canonical(&Operation::CandidateOnce).unwrap();
    assert_eq!(bytes, b"\"CandidateOnce\"");
    let frame = frame();
    let wire = encode_frame(&frame, &[8; 32]).unwrap();
    assert_eq!(wire.len(), canonical(&frame).unwrap().len() + 36);
    assert_eq!(
        u32::from_le_bytes(wire[..4].try_into().unwrap()) as usize,
        wire.len() - 4
    );
    assert_eq!(wire[4..wire.len() - 32], canonical(&frame).unwrap());
}

#[test]
fn rfc4231_hmac_golden_vector_and_domain_separation() {
    // RFC 4231 case 3: 20 bytes aa followed by zero padding to our 32-byte key.
    let mut key = [0u8; 32];
    key[..20].fill(0xaa);
    assert_eq!(
        mac(&key, &[0xdd; 50]),
        [
            0x77, 0x3e, 0xa9, 0x1e, 0x36, 0x80, 0x0e, 0x46, 0x85, 0x4d, 0xb8, 0xeb, 0xd0, 0x91,
            0x81, 0xa7, 0x29, 0x59, 0x09, 0x8b, 0x3e, 0xf8, 0xc1, 0x22, 0xd9, 0x63, 0x55, 0x14,
            0xce, 0xd5, 0x65, 0xfe
        ]
    );
    assert_ne!(digest(b"preparation", b"x"), digest(b"manifest", b"x"));
}

#[test]
fn strict_decoder_rejects_duplicate_unknown_alternate_and_oversize() {
    for bytes in [
        br#"{"Api":{"status":1,"status":2}}"#.as_slice(),
        br#"{"Api":{"status":1,"extra":2}}"#,
        br#"{ "Api":{"status":1}}"#,
        br#"{"Api":{"status":1.0}}"#,
        br#"{"Api":{"status":1}} trailing"#,
    ] {
        assert!(decode::<RawResult>(bytes, FRAME_LIMIT).is_err());
    }
    assert_eq!(
        decode::<Operation>(b"\"Current\"", 1),
        Err(Rejection::Oversize)
    );
}

#[test]
fn authentication_sequence_and_complete_transcript_binding() {
    let wire = encode_frame(&frame(), &[8; 32]).unwrap();
    for i in 0..wire.len() {
        let mut damaged = wire.clone();
        damaged[i] ^= 1;
        let mut rx = Receiver::new(Role::Guardian, Role::Worker, [1; 32], [2; 32]);
        assert!(rx.receive(&damaged, &[8; 32]).is_err(), "byte {i}");
    }
    let mut rx = Receiver::new(Role::Guardian, Role::Worker, [1; 32], [2; 32]);
    assert!(rx.receive(&wire, &[9; 32]).is_err());
    assert!(rx.receive(&wire, &[8; 32]).is_ok());
    assert_eq!(rx.receive(&wire, &[8; 32]), Err(Rejection::Sequence));
    let mut gap = frame();
    gap.sequence = 3;
    assert_eq!(
        rx.receive(&encode_frame(&gap, &[8; 32]).unwrap(), &[8; 32]),
        Err(Rejection::Sequence)
    );
}

#[test]
fn malformed_prefix_truncation_and_oversize_never_decode() {
    let wire = encode_frame(&frame(), &[8; 32]).unwrap();
    for len in 0..wire.len() {
        assert!(
            Receiver::new(Role::Guardian, Role::Worker, [1; 32], [2; 32])
                .receive(&wire[..len], &[8; 32])
                .is_err()
        );
    }
    assert!(
        Receiver::new(Role::Guardian, Role::Worker, [1; 32], [2; 32])
            .receive(&vec![0; FRAME_LIMIT + 1], &[8; 32])
            .is_err()
    );
}

#[test]
fn fresh_baselines_and_checked_target_boundaries() {
    for b in [230_000, 247_123] {
        let mut m = manifest(b);
        assert!(m.validate().is_ok());
        assert_eq!(m.capture.baseline().unwrap(), b);
        for delta in [999, 1000, 5000, 5001] {
            m.delta_mw = delta;
            m.target_mw = b - delta;
            assert_eq!(m.validate().is_ok(), (1000..=5000).contains(&delta));
        }
        m = manifest(b);
        m.capture.default_mw = None;
        assert!(m.validate().is_ok());
        m.target_mw = b;
        assert!(m.validate().is_err());
        m = manifest(b);
        m.capture.readings_mw[1] += 1;
        assert!(m.validate().is_err());
    }
}

#[test]
fn exact_approval_binds_all_serialized_leaves() {
    let m = manifest(247_123);
    let a = approval(&m);
    assert!(a.validate(&m, 500, 500).is_ok());
    assert!(a.validate(&m, 60_500, 500).is_err());
    assert!(a.validate(&m, 500, 60_000).is_err());
    // Each serialized byte change either fails typed decoding/validation or changes digest.
    let bytes = canonical(&m).unwrap();
    for i in 0..bytes.len() {
        let mut mutation = bytes.clone();
        mutation[i] ^= 1;
        if let Ok(changed) = decode::<Manifest>(&mutation, SNAPSHOT_LIMIT) {
            assert!(a.validate(&changed, 500, 500).is_err(), "manifest byte {i}");
        }
    }
}

#[test]
fn permissions_bind_dispatch_origin_and_absolute_cleanup() {
    let p = Permission {
        session: [1; 32],
        boot: [5; 32],
        digest: [2; 32],
        generation: 1,
        operation_id: 1,
        operation: Operation::RestoreBaseline,
        dispatched_ms: 500,
        watchdog_ms: 1500,
        expiry_ms: 1500,
        recovery_trigger_ms: Some(0),
    };
    assert!(p.validate_entry(1499, &[5; 32], true).is_ok());
    assert_eq!(
        p.validate_entry(1500, &[5; 32], true),
        Err(Rejection::Expired)
    );
    assert!(p.validate_entry(500, &[5; 32], false).is_err());
    assert!(p.validate_entry(500, &[6; 32], true).is_err());
}

#[test]
fn journal_nonce_history_and_rollover_fail_closed() {
    let m = manifest(247_123);
    let session = SessionRecord {
        manifest: m.clone(),
        previous_terminal: None,
        calls: vec![],
        terminal: None,
        recovery_required: true,
    };
    let mut snapshot = Snapshot {
        version: VERSION,
        sequence: 1,
        previous: None,
        consumed_nonces: vec![m.preparation.nonce],
        sessions: vec![session.clone()],
    };
    assert!(snapshot.validate().is_ok());
    snapshot.sessions.push(session);
    snapshot.consumed_nonces.push(m.preparation.nonce);
    assert!(snapshot.validate().is_err());
    snapshot.sessions.truncate(1);
    snapshot.consumed_nonces.truncate(1);
    snapshot.consumed_nonces[0] = [99; 32];
    assert!(snapshot.validate().is_err());
}
