use autotune_benchmark_worker::{
    monitor::{Anchor, Binding, Metric, Ticket},
    sampler::{
        framing::{Framer, ReadStep, Source},
        ingress::Received,
        supervisor::{Input, Selection, Supervisor},
        wire::{Channel, Lane, Message, WireBinding},
    },
    worker::DiscoveryBinding,
};
const UUID: &str = "GPU-03030303-0303-0303-0303-030303030303";
const PCI: &str = "0000:01:02.0";
fn binding() -> WireBinding {
    WireBinding {
        monitor: Binding {
            session: [1; 16],
            run: [2; 16],
            device: DiscoveryBinding::parse(UUID, PCI).unwrap().identity,
            clock_token: [4; 16],
            qpc_frequency: 1_000_000_000,
        },
        manifest_digest: [5; 32],
    }
}
fn anchor() -> Anchor {
    Anchor {
        clock_token: [4; 16],
        qpc_before: 0,
        qpc_after: 0,
        interrupt_100ns: 0,
        unbiased_100ns: 0,
    }
}
fn supervisor() -> Supervisor {
    Supervisor::new(
        binding(),
        anchor(),
        Selection {
            raw_uuid: UUID.into(),
            raw_pci: PCI.into(),
            sampler_digest: [6; 32],
        },
    )
    .unwrap()
}
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
fn frame() -> Vec<u8> {
    Channel::new(binding(), Lane::Ordinary)
        .unwrap()
        .encode(&Message::Query(Ticket {
            binding: binding().monitor,
            metric: Metric::Power,
            slot: 0,
            scheduled_ns: 0,
            start_ns: 0,
        }))
        .unwrap()
}
#[test]
fn wrong_binding_frame_keeps_all_bytes_even_when_ordinary_retention_is_full() {
    let mut s = supervisor();
    let b = s.budget();
    let bad = frame();
    let mut other = binding();
    other.monitor.run = [9; 16];
    let mut decoder = Channel::new(other, Lane::Ordinary).unwrap();
    let mut framer = Framer::new(b.clone()).unwrap();
    let packet = framer.read(&mut Bytes(&bad));
    let received = Received::decode(
        Lane::Ordinary,
        packet,
        &mut decoder,
        b.reserve(1024 * 1024).map_err(|_| ()),
    );
    let _remaining = b.reserve(32 * 1024 * 1024 - b.used()).unwrap();
    received.apply(&mut s, Some(anchor()));
    match &s.traces().last().unwrap().input {
        Input::Rejected { bytes, .. } => assert_eq!(bytes, &bad),
        _ => panic!("raw frame lost"),
    }
}
#[test]
fn codec_capacity_failure_keeps_raw_frame_instead_of_dropping_it() {
    let mut s = supervisor();
    let b = s.budget();
    let bytes = frame();
    let mut decoder = Channel::new(binding(), Lane::Ordinary).unwrap();
    let packet = Framer::new(b).unwrap().read(&mut Bytes(&bytes));
    Received::decode(Lane::Ordinary, packet, &mut decoder, Err(())).apply(&mut s, Some(anchor()));
    match &s.traces().last().unwrap().input {
        Input::Rejected { bytes: raw, .. } => assert_eq!(raw, &bytes),
        _ => panic!("capacity loss erased input"),
    }
    assert_eq!(
        decoder.decode(&bytes).unwrap_err(),
        autotune_benchmark_worker::sampler::wire::WireError::Poisoned
    );
}
#[test]
fn full_reported_frame_with_os_failure_is_never_decoded_as_a_valid_message() {
    struct Failing(Bytes<'static>);
    impl Source for Failing {
        fn read(&mut self, out: &mut [u8]) -> ReadStep {
            let mut step = self.0.read(out);
            if self.0 .0.is_empty() {
                step.error = Some(995);
            }
            step
        }
    }
    let mut s = supervisor();
    let b = s.budget();
    let packet = Framer::new(b.clone())
        .unwrap()
        .read(&mut Failing(Bytes(b"\x06\0\0\0{}")));
    let mut decoder = Channel::new(binding(), Lane::Ordinary).unwrap();
    Received::decode(
        Lane::Ordinary,
        packet,
        &mut decoder,
        b.reserve(1024 * 1024).map_err(|_| ()),
    )
    .apply(&mut s, Some(anchor()));
    assert!(matches!(
        s.traces().last().unwrap().input,
        Input::Rejected { .. }
    ));
    // An OS failure accompanying a complete frame is still terminal for the
    // decoder. Even a valid sequence-zero successor cannot reopen this lane.
    assert_eq!(
        decoder.decode(&frame()).unwrap_err(),
        autotune_benchmark_worker::sampler::wire::WireError::Poisoned
    );
}
