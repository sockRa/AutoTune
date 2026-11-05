mod delivery_tests {
    #[test]
    fn matching_outer_channel_cannot_rebind_an_artifact_from_another_run() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let other = binding(&b, 1);
        let artifact = artifact(other, &b, false);
        let chunks = Chunks::new(&artifact, &b).unwrap();
        let mut tx = channel(bind, &b);
        let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
        let mut rx = receiver(bind, &b);
        assert_eq!(
            rx.accept(raw(frame.as_bytes(), Some(a(1000)), &b)),
            Err(DeliveryFault::Payload(Fault::Binding))
        );
        assert_eq!(rx.payload(), artifact.canonical().as_bytes());
        assert!(rx.receipt().is_none());
        assert!(rx.ready().is_none());
    }
    use super::*;
    use autotune_benchmark_worker::{
        sampler::{
            framing::{Framer, ReadStep, Source},
            wire::Lane,
        },
        workload::{
            dispatch::Phase,
            transport::RawReceived,
            wire::{Channel, Direction, EncodedFrame, Message},
            worker_setup_delivery::{Chunks, Fault as DeliveryFault, Receiver},
        },
    };
    struct Bytes<'a>(&'a [u8]);
    impl Source for Bytes<'_> {
        fn read(&mut self, out: &mut [u8]) -> ReadStep {
            let n = out.len().min(self.0.len());
            out[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            ReadStep {
                reported: n,
                error: None,
            }
        }
    }
    fn raw(bytes: &[u8], receipt: Option<Anchor>, b: &Budget) -> RawReceived {
        RawReceived::new(
            Lane::Ordinary,
            Framer::new(b.clone()).unwrap().read(&mut Bytes(bytes)),
            receipt,
        )
    }
    fn artifact(binding: WorkerBinding, b: &Budget, large: bool) -> Artifact {
        let checks = vec![
            FileCheck {
                stage: Stage::ReadBefore(0),
                anchor: Some(a(200))
            };
            if large { 300 } else { 0 }
        ];
        Artifact::recorded(binding, facts(&[], &checks), b).unwrap()
    }
    fn receiver(binding: WorkerBinding, b: &Budget) -> Receiver {
        Receiver::new(binding, a(100), 100_000_000, b.clone()).unwrap()
    }
    fn channel(binding: WorkerBinding, b: &Budget) -> Channel {
        Channel::new(binding, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap()
    }
    fn ready(binding: WorkerBinding) -> Message {
        Message::Ready {
            anchor: a(1_000_000),
            identity: binding.monitor().device,
            context: binding.monitor().device,
            worker_digest: digest(9),
            driver_version: 12090,
            compute_major: 7,
            compute_minor: 5,
            prepared_lanes: 65536,
        }
    }
    fn modified(frame: &EncodedFrame, path: &[&str], replacement: Value) -> Vec<u8> {
        let v = decode(&frame.as_bytes()[4..], 64 * 1024).unwrap();
        let text = edit(&v, path, replacement).to_canonical();
        let mut bytes = ((text.len() + 4) as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|v| format!("{v:02x}")).collect()
    }
    #[test]
    fn setup_chunks_ready_and_following_messages_use_one_retained_decoder_sequence() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let artifact = artifact(bind, &b, true);
        let chunks = Chunks::new(&artifact, &b).unwrap();
        assert!(chunks.count() > 1);
        let mut tx = channel(bind, &b);
        let mut rx = receiver(bind, &b);
        for i in 0..chunks.count() {
            let frame = tx.encode(&chunks.message(i).unwrap()).unwrap();
            assert!(frame.as_bytes().len() <= 64 * 1024);
            rx.accept(raw(frame.as_bytes(), Some(a(1000 + i as u64 * 1000)), &b))
                .unwrap();
            if i + 1 < chunks.count() {
                assert!(rx.receipt().is_none());
            }
        }
        assert!(rx.ready().is_none());
        assert_eq!(rx.receipt().unwrap().canonical(), artifact.canonical());
        assert_eq!(rx.payload(), artifact.canonical().as_bytes());
        let frame = tx.encode(&ready(bind)).unwrap();
        rx.accept(raw(frame.as_bytes(), Some(a(2_000_000)), &b))
            .unwrap();
        assert_eq!(rx.arrivals().len(), chunks.count() as usize + 1);
        let mut delivered = rx.into_ready().ok().unwrap();
        assert!(delivered.receipt().reported().simulated);
        let frame = tx
            .encode(&Message::PhaseEntered {
                phase: Phase::Warmup,
                anchor: a(3_000_000),
            })
            .unwrap();
        let packet =
            raw(frame.as_bytes(), Some(a(3_000_100)), &b).decode_worker(delivered.decoder());
        assert!(matches!(packet.message(), Ok(Message::PhaseEntered { .. })));
    }
    #[test]
    fn ready_before_setup_is_sticky_and_retains_the_exact_rejected_packet() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let mut tx = channel(bind, &b);
        let frame = tx.encode(&ready(bind)).unwrap();
        let mut rx = receiver(bind, &b);
        assert_eq!(
            rx.accept(raw(frame.as_bytes(), Some(a(2_000_000)), &b)),
            Err(DeliveryFault::Protocol)
        );
        assert_eq!(
            rx.rejected().unwrap().raw().packet().bytes(),
            frame.as_bytes()
        );
        assert!(rx.receipt().is_none());
        assert!(rx.into_ready().is_err());
    }
    #[test]
    fn duplicate_or_skipped_chunk_index_keeps_partial_payload_and_first_fault() {
        for skip in [false, true] {
            let b = Budget::new(32 * 1024 * 1024).unwrap();
            let bind = binding(&b, 0);
            let artifact = artifact(bind, &b, true);
            let chunks = Chunks::new(&artifact, &b).unwrap();
            let mut tx = channel(bind, &b);
            let mut rx = receiver(bind, &b);
            if !skip {
                let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
                rx.accept(raw(frame.as_bytes(), Some(a(1000)), &b)).unwrap();
            }
            let before = rx.payload().to_vec();
            let frame = tx
                .encode(&chunks.message(if skip { 1 } else { 0 }).unwrap())
                .unwrap();
            assert_eq!(
                rx.accept(raw(frame.as_bytes(), Some(a(2000)), &b)),
                Err(DeliveryFault::Protocol)
            );
            assert_eq!(rx.payload(), before);
            assert_eq!(
                rx.rejected().unwrap().raw().packet().bytes(),
                frame.as_bytes()
            );
            rx.stop();
            assert_eq!(rx.fault(), Some(DeliveryFault::Protocol));
        }
    }
    #[test]
    fn changed_midstream_digest_cannot_replace_the_original_transfer() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let artifact = artifact(bind, &b, true);
        let chunks = Chunks::new(&artifact, &b).unwrap();
        let mut tx = channel(bind, &b);
        let mut rx = receiver(bind, &b);
        let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
        rx.accept(raw(frame.as_bytes(), Some(a(1000)), &b)).unwrap();
        let before = rx.payload().to_vec();
        let frame = tx.encode(&chunks.message(1).unwrap()).unwrap();
        let changed = modified(
            &frame,
            &["body", "digest"],
            Value::Text(digest(77).to_hex()),
        );
        assert_eq!(
            rx.accept(raw(&changed, Some(a(2000)), &b)),
            Err(DeliveryFault::Protocol)
        );
        assert_eq!(rx.payload(), before);
        assert_eq!(rx.rejected().unwrap().raw().packet().bytes(), changed);
    }
    #[test]
    fn complete_corrupt_payload_keeps_all_bytes_and_rejects_digest_before_ready() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let artifact = artifact(bind, &b, false);
        let chunks = Chunks::new(&artifact, &b).unwrap();
        assert_eq!(chunks.count(), 1);
        let mut tx = channel(bind, &b);
        let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
        let data = artifact.canonical().as_bytes();
        let mut bad = data.to_vec();
        bad[0] = b'[';
        let changed = modified(&frame, &["body", "data"], Value::Text(hex(&bad)));
        let mut rx = receiver(bind, &b);
        assert_eq!(
            rx.accept(raw(&changed, Some(a(1000)), &b)),
            Err(DeliveryFault::Digest)
        );
        assert_eq!(rx.payload(), bad);
        assert!(rx.receipt().is_none());
        assert!(rx.ready().is_none());
    }
    #[test]
    fn self_consistent_digest_does_not_make_a_bad_setup_schema_readable() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let artifact = artifact(bind, &b, false);
        let chunks = Chunks::new(&artifact, &b).unwrap();
        let mut tx = channel(bind, &b);
        let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
        let bad = artifact
            .canonical()
            .replacen("\"version\":\"1\"", "\"version\":\"9\"", 1);
        assert_ne!(bad, artifact.canonical());
        let v = decode(&frame.as_bytes()[4..], 64 * 1024).unwrap();
        let v = edit(&v, &["body", "data"], Value::Text(hex(bad.as_bytes())));
        let text = edit(
            &v,
            &["body", "digest"],
            Value::Text(Digest::of(Domain::RawArtifact, bad.as_bytes()).to_hex()),
        )
        .to_canonical();
        let mut bytes = ((text.len() + 4) as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(text.as_bytes());
        let mut rx = receiver(bind, &b);
        assert!(matches!(
            rx.accept(raw(&bytes, Some(a(1000)), &b)),
            Err(DeliveryFault::Payload(_))
        ));
        assert_eq!(rx.payload(), bad.as_bytes());
        assert!(rx.receipt().is_none());
    }
    #[test]
    fn raw_missing_foreign_regressing_and_deadline_equal_receipts_never_resume() {
        for mode in 0..4 {
            let b = Budget::new(32 * 1024 * 1024).unwrap();
            let bind = binding(&b, 0);
            let artifact = artifact(bind, &b, false);
            let chunks = Chunks::new(&artifact, &b).unwrap();
            let mut tx = channel(bind, &b);
            let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
            let mut foreign = a(1000);
            foreign.clock_token = [88; 16];
            let anchor = match mode {
                0 => None,
                1 => Some(a(50)),
                2 => Some(foreign),
                _ => Some(a(10_000_000_000)),
            };
            let mut rx = receiver(bind, &b);
            let expected = if mode == 3 {
                DeliveryFault::Deadline
            } else {
                DeliveryFault::Clock
            };
            assert_eq!(rx.accept(raw(frame.as_bytes(), anchor, &b)), Err(expected));
            assert_eq!(rx.rejected().unwrap().raw().receipt(), anchor);
            assert_eq!(rx.tick(Some(a(2000))), Err(expected));
            assert!(rx.payload().is_empty());
        }
    }
    #[test]
    fn malformed_chunk_metadata_widths_hex_and_role_cannot_enter_the_assembler() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let artifact = artifact(bind, &b, false);
        let chunks = Chunks::new(&artifact, &b).unwrap();
        let mut tx = channel(bind, &b);
        let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
        for (path, replacement) in [
            (vec!["body", "count"], Value::U64(0)),
            (vec!["body", "total"], Value::U64(u64::MAX)),
            (vec!["body", "index"], Value::U64(u32::MAX as u64 + 1)),
            (vec!["body", "data"], Value::Text("G0".into())),
            (vec!["role"], Value::Text("pilot_sampler".into())),
        ] {
            let changed = modified(&frame, &path, replacement);
            let mut rx = receiver(bind, &b);
            assert!(rx.accept(raw(&changed, Some(a(1000)), &b)).is_err());
            assert!(rx.payload().is_empty());
            assert!(rx.rejected().is_some());
        }
        let mut sampler =
            Channel::sampler(bind, Lane::Ordinary, Direction::FromWorker, b.clone()).unwrap();
        assert!(sampler.encode(&chunks.message(0).unwrap()).is_err());
        let mut control =
            Channel::new(bind, Lane::Control, Direction::FromWorker, b.clone()).unwrap();
        assert!(control.encode(&chunks.message(0).unwrap()).is_err());
    }
    #[test]
    fn stop_and_transport_fault_keep_received_bytes_and_do_not_restart() {
        for stop in [false, true] {
            let b = Budget::new(32 * 1024 * 1024).unwrap();
            let bind = binding(&b, 0);
            let artifact = artifact(bind, &b, true);
            let chunks = Chunks::new(&artifact, &b).unwrap();
            let mut tx = channel(bind, &b);
            let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
            let mut rx = receiver(bind, &b);
            rx.accept(raw(frame.as_bytes(), Some(a(1000)), &b)).unwrap();
            let before = rx.payload().to_vec();
            if stop {
                rx.stop()
            } else {
                rx.transport_failed(Some(5), Some(a(2000)));
            }
            let expected = if stop {
                DeliveryFault::Stopped
            } else {
                DeliveryFault::Transport
            };
            assert_eq!(rx.tick(Some(a(3000))), Err(expected));
            assert_eq!(rx.payload(), before);
            assert!(rx.receipt().is_none());
            assert_eq!(rx.received(), 1);
            if !stop {
                assert_eq!(rx.transport(), Some((Some(5), Some(a(2000)))));
            }
        }
    }
    #[test]
    fn allocation_failure_retains_complete_payload_and_returns_no_receipt() {
        let b = Budget::new(32 * 1024 * 1024).unwrap();
        let bind = binding(&b, 0);
        let tiny = Budget::new(1).unwrap();
        assert!(matches!(
            Receiver::new(bind, a(100), 100_000_000, tiny.clone()),
            Err(DeliveryFault::Capacity)
        ));
        assert_eq!(tiny.used(), 0);
        let artifact = artifact(bind, &b, false);
        assert!(matches!(
            Chunks::new(&artifact, &tiny),
            Err(DeliveryFault::Capacity)
        ));
        assert_eq!(tiny.used(), 0);
        let chunks = Chunks::new(&artifact, &b).unwrap();
        let mut tx = channel(bind, &b);
        let frame = tx.encode(&chunks.message(0).unwrap()).unwrap();
        let mut rx = receiver(bind, &b);
        let _pressure = b
            .reserve(32 * 1024 * 1024 - b.used() - 2 * 1024 * 1024)
            .unwrap();
        assert_eq!(
            rx.accept(raw(frame.as_bytes(), Some(a(1000)), &b)),
            Err(DeliveryFault::Payload(Fault::Capacity))
        );
        assert_eq!(rx.payload(), artifact.canonical().as_bytes());
        assert!(rx.receipt().is_none());
        assert!(rx.rejected().is_some());
        assert!(rx.into_ready().is_err());
    }
}
