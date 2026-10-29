use autotune_benchmark_worker::{
    monitor::{Anchor, Binding, Metric, Ticket},
    sampler::wire::{Channel, Lane, Message, WireBinding, WireError},
    worker::Identity,
};
use autotune_probe::report::{Failure, Field, Reason};

fn binding() -> WireBinding {
    WireBinding {
        monitor: Binding {
            session: [1; 16],
            run: [2; 16],
            device: Identity {
                uuid: [3; 16],
                domain: 0,
                bus: 1,
                device: 2,
            },
            clock_token: [4; 16],
            qpc_frequency: 1_000_000_000,
        },
        manifest_digest: [5; 32],
    }
}
fn anchor() -> Anchor {
    Anchor {
        clock_token: [4; 16],
        qpc_before: 10,
        qpc_after: 11,
        interrupt_100ns: 12,
        unbiased_100ns: 13,
    }
}
fn query() -> Message {
    Message::Query(Ticket {
        binding: binding().monitor,
        metric: Metric::Power,
        slot: u64::MAX,
        scheduled_ns: u64::MAX - 2,
        start_ns: u64::MAX - 1,
    })
}

// Removing binding/sequence checks would allow a stale or foreign child to
// refresh the real monitor. Expectations are independent literal mutations.
#[test]
fn canonical_query_and_monotonic_sequence() {
    let mut tx = Channel::new(binding(), Lane::Ordinary).unwrap();
    let mut rx = Channel::new(binding(), Lane::Ordinary).unwrap();
    let bytes = tx.encode(&query()).unwrap();
    assert_eq!(
        u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize,
        bytes.len()
    );
    let text = std::str::from_utf8(&bytes[4..]).unwrap();
    assert!(text.starts_with("{\"version\":\"1\",\"session\":\"01010101010101010101010101010101\",\"run\":\"02020202020202020202020202020202\",\"role\":\"sampler\",\"lane\":\"ordinary\",\"sequence\":\"0\","));
    assert!(text.ends_with("\"kind\":\"query\",\"body\":{\"metric\":\"power\",\"slot\":\"18446744073709551615\",\"scheduled_ns\":\"18446744073709551613\",\"issued_ns\":\"18446744073709551614\"}}"));
    match rx.decode(&bytes).unwrap() {
        Message::Query(t) => {
            assert_eq!(t.slot, u64::MAX);
            assert_eq!(t.binding, binding().monitor);
        }
        _ => panic!("wrong kind"),
    }
    assert_eq!(rx.decode(&bytes).unwrap_err(), WireError::Sequence);
    assert_eq!(
        rx.decode(&tx.encode(&query()).unwrap()).unwrap_err(),
        WireError::Poisoned
    );
}

#[test]
fn rejects_foreign_binding_lane_and_prefix_before_body() {
    let bytes = Channel::new(binding(), Lane::Ordinary)
        .unwrap()
        .encode(&query())
        .unwrap();
    let mut foreign = binding();
    foreign.monitor.run[0] = 7;
    assert_eq!(
        Channel::new(foreign, Lane::Ordinary)
            .unwrap()
            .decode(&bytes)
            .unwrap_err(),
        WireError::Binding
    );
    assert_eq!(
        Channel::new(binding(), Lane::Control)
            .unwrap()
            .decode(&bytes)
            .unwrap_err(),
        WireError::Lane
    );
    for prefix in [0u32, 3, 65537, u32::MAX] {
        assert_eq!(
            Channel::new(binding(), Lane::Ordinary)
                .unwrap()
                .decode(&prefix.to_le_bytes())
                .unwrap_err(),
            WireError::Length
        );
    }
    assert_eq!(
        Channel::new(binding(), Lane::Ordinary)
            .unwrap()
            .decode(&bytes[..bytes.len() - 1])
            .unwrap_err(),
        WireError::Length
    );
}

#[test]
fn unavailable_raw_field_and_actual_anchors_survive_wire() {
    let ticket = match query() {
        Message::Query(t) => t,
        _ => unreachable!(),
    };
    let msg = Message::MetricRecord {
        ticket,
        start: anchor(),
        end: anchor(),
        raw: Field::unavailable(
            "mW",
            Failure {
                reason: Reason::QueryFailed,
                vendor_status: Some(i32::MIN),
                detail: "quoted \"detail\"\n".into(),
            },
        ),
    };
    let bytes = Channel::new(binding(), Lane::Ordinary)
        .unwrap()
        .encode(&msg)
        .unwrap();
    match Channel::new(binding(), Lane::Ordinary)
        .unwrap()
        .decode(&bytes)
        .unwrap()
    {
        Message::MetricRecord {
            start, end, raw, ..
        } => {
            assert_eq!(start, anchor());
            assert_eq!(end, anchor());
            match raw.reading {
                autotune_probe::report::Reading::Unavailable { failure } => {
                    assert_eq!(failure.vendor_status, Some(i32::MIN));
                    assert_eq!(failure.detail, "quoted \"detail\"\n");
                }
                _ => panic!("lost unavailable"),
            }
        }
        _ => panic!("wrong kind"),
    }
}

#[test]
fn oversized_escaped_detail_and_wrong_unit_never_encode() {
    let ticket = match query() {
        Message::Query(t) => t,
        _ => unreachable!(),
    };
    for raw in [
        Field::available("watts", 10),
        Field::unavailable("mW", Failure::new(Reason::QueryFailed, "\n".repeat(40_000))),
    ] {
        let msg = Message::MetricRecord {
            ticket,
            start: anchor(),
            end: anchor(),
            raw,
        };
        assert!(Channel::new(binding(), Lane::Ordinary)
            .unwrap()
            .encode(&msg)
            .is_err());
    }
}

#[test]
fn noncanonical_unknown_duplicate_and_wrong_role_reject() {
    let bytes = Channel::new(binding(), Lane::Ordinary)
        .unwrap()
        .encode(&query())
        .unwrap();
    let text = std::str::from_utf8(&bytes[4..]).unwrap();
    for bad in [
        text.replace("\"sampler\"", "\"worker\""),
        text.replace("\"version\":\"1\"", "\"version\":\"2\""),
        text.replace("\"version\":\"1\"", "\"version\":\"1\",\"extra\":null"),
        text.replace("\"version\":\"1\"", "\"version\":\"1\",\"version\":\"1\""),
        format!(" {text}"),
        text.replace("\"slot\":\"18446744073709551615\"", "\"slot\":\"01\""),
    ] {
        let mut frame = ((bad.len() + 4) as u32).to_le_bytes().to_vec();
        frame.extend(bad.as_bytes());
        assert!(Channel::new(binding(), Lane::Ordinary)
            .unwrap()
            .decode(&frame)
            .is_err());
    }
}

#[test]
fn full_query_golden_frame_is_independent_of_encoder() {
    let expected = concat!(
        "{\"version\":\"1\",\"session\":\"01010101010101010101010101010101\",",
        "\"run\":\"02020202020202020202020202020202\",\"role\":\"sampler\",\"lane\":\"ordinary\",\"sequence\":\"0\",",
        "\"manifest_digest\":\"0505050505050505050505050505050505050505050505050505050505050505\",",
        "\"clock_token\":\"04040404040404040404040404040404\",\"frequency\":\"1000000000\",",
        "\"device\":{\"uuid\":\"03030303030303030303030303030303\",\"domain\":\"0\",\"bus\":\"1\",\"device\":\"2\"},",
        "\"kind\":\"query\",\"body\":{\"metric\":\"power\",\"slot\":\"18446744073709551615\",\"scheduled_ns\":\"18446744073709551613\",\"issued_ns\":\"18446744073709551614\"}}"
    );
    let bytes = Channel::new(binding(), Lane::Ordinary)
        .unwrap()
        .encode(&query())
        .unwrap();
    assert_eq!(&bytes[4..], expected.as_bytes());
}

#[test]
fn ready_bootstrap_and_control_bodies_round_trip_without_losing_fields() {
    let messages = [
        Message::Bootstrap {
            origin: anchor(),
            raw_uuid: "GPU-example".into(),
            raw_pci: "0000:01:02.0".into(),
            manifest: "{}".into(),
        },
        Message::Ready {
            raw_uuid: "GPU-example".into(),
            raw_pci: "0000:01:02.0".into(),
            sampler_digest: [9; 32],
            frequency: 1_000_000_000,
            anchor: anchor(),
            runtime: "runtime evidence".into(),
        },
        Message::Stop {
            reason: "requested".into(),
            trigger_ns: 10,
            cooperative_ns: 20,
            exit_ns: 30,
        },
        Message::StopAck {
            anchor: anchor(),
            pending: true,
        },
        Message::RoleFinished {
            anchor: anchor(),
            pending: true,
            cleanup: "unknown".into(),
        },
    ];
    for message in messages {
        let bytes = Channel::new(binding(), message.lane())
            .unwrap()
            .encode(&message)
            .unwrap();
        let decoded = Channel::new(binding(), message.lane())
            .unwrap()
            .decode(&bytes)
            .unwrap();
        let reencoded = Channel::new(binding(), message.lane())
            .unwrap()
            .encode(&decoded)
            .unwrap();
        assert_eq!(reencoded, bytes);
        match decoded {
            Message::Ready {
                sampler_digest,
                frequency,
                anchor: a,
                runtime,
                ..
            } => {
                assert_eq!(sampler_digest, [9; 32]);
                assert_eq!(frequency, 1_000_000_000);
                assert_eq!(a, anchor());
                assert_eq!(runtime, "runtime evidence");
            }
            Message::Bootstrap {
                origin, manifest, ..
            } => {
                assert_eq!(origin, anchor());
                assert_eq!(manifest, "{}");
            }
            Message::Stop {
                trigger_ns,
                cooperative_ns,
                exit_ns,
                ..
            } => {
                assert_eq!((trigger_ns, cooperative_ns, exit_ns), (10, 20, 30));
            }
            Message::StopAck { pending, .. } | Message::RoleFinished { pending, .. } => {
                assert!(pending)
            }
            _ => panic!("wrong message"),
        }
    }
}
