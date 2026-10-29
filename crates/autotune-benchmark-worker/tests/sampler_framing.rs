use autotune_benchmark_worker::sampler::{
    budget::Budget,
    framing::{Framer, Phase, ReadFault, ReadStep, Source},
};
struct Script {
    bytes: Vec<u8>,
    position: usize,
    chunk: usize,
    error_at: Option<usize>,
}
impl Source for Script {
    fn read(&mut self, output: &mut [u8]) -> ReadStep {
        let n = output
            .len()
            .min(self.chunk)
            .min(self.bytes.len() - self.position);
        output[..n].copy_from_slice(&self.bytes[self.position..self.position + n]);
        self.position += n;
        ReadStep {
            reported: n,
            error: self.error_at.filter(|at| self.position >= *at).map(|_| 995),
        }
    }
}
fn reader() -> Framer {
    Framer::new(Budget::new(1024 * 1024).unwrap()).unwrap()
}
#[test]
fn split_prefix_and_body_preserve_exact_bytes_without_requiring_atomic_reads() {
    let bytes = [9u32.to_le_bytes().as_slice(), b"hello"].concat();
    for chunk in 1..=9 {
        let mut source = Script {
            bytes: bytes.clone(),
            position: 0,
            chunk,
            error_at: None,
        };
        let packet = reader().read(&mut source);
        assert_eq!(packet.bytes(), bytes);
        assert!(packet.rejection().is_none());
    }
}
#[test]
fn oversize_prefix_is_retained_and_its_claimed_body_is_never_drained() {
    let bytes = [u32::MAX.to_le_bytes().as_slice(), b"not to be read"].concat();
    let mut source = Script {
        bytes,
        position: 0,
        chunk: 100,
        error_at: None,
    };
    let packet = reader().read(&mut source);
    assert_eq!(source.position, 4);
    assert_eq!(packet.bytes(), u32::MAX.to_le_bytes());
    assert_eq!(packet.rejection().unwrap().fault, ReadFault::Length);
}
#[test]
fn partial_prefix_and_body_eof_never_acquire_padded_zero_bytes() {
    for bytes in [vec![1, 2], [10u32.to_le_bytes().as_slice(), b"ab"].concat()] {
        let mut source = Script {
            bytes: bytes.clone(),
            position: 0,
            chunk: 2,
            error_at: None,
        };
        let packet = reader().read(&mut source);
        assert_eq!(packet.bytes(), bytes);
        let rejection = packet.rejection().unwrap();
        assert_eq!(rejection.fault, ReadFault::Eof);
        assert_eq!(
            rejection.phase,
            if bytes.len() < 4 {
                Phase::Prefix
            } else {
                Phase::Body
            }
        );
    }
}
#[test]
fn an_os_error_with_reported_bytes_retains_them_and_never_accepts_the_frame() {
    let bytes = [6u32.to_le_bytes().as_slice(), b"ab"].concat();
    let mut source = Script {
        bytes: bytes.clone(),
        position: 0,
        chunk: 4,
        error_at: Some(6),
    };
    let packet = reader().read(&mut source);
    assert_eq!(packet.bytes(), bytes);
    let rejection = packet.rejection().unwrap();
    assert_eq!(rejection.fault, ReadFault::Os(995));
    assert_eq!(rejection.reported, 2);
}
#[test]
fn exhausted_body_budget_still_retains_the_independently_reserved_prefix() {
    let budget = Budget::new(100).unwrap();
    let mut framer = Framer::new(budget.clone()).unwrap();
    let _full = budget.reserve(96).unwrap();
    let mut source = Script {
        bytes: [20u32.to_le_bytes().as_slice(), b"unused"].concat(),
        position: 0,
        chunk: 100,
        error_at: None,
    };
    let packet = framer.read(&mut source);
    assert_eq!(source.position, 4);
    assert_eq!(packet.bytes(), 20u32.to_le_bytes());
    assert_eq!(packet.rejection().unwrap().fault, ReadFault::Capacity);
}
#[test]
fn rejected_stream_is_sticky_and_never_reads_a_successor_frame() {
    let mut framer = reader();
    let mut source = Script {
        bytes: [u32::MAX.to_le_bytes().as_slice(), b"following data"].concat(),
        position: 0,
        chunk: 100,
        error_at: None,
    };
    let first = framer.read(&mut source);
    assert!(first.rejection().is_some());
    let second = framer.read(&mut source);
    assert_eq!(source.position, 4);
    assert!(second.bytes().is_empty());
    assert_eq!(second.rejection().unwrap().fault, ReadFault::Stopped);
}
#[test]
fn queue_fault_slot_preserves_first_packet_without_overwrite_or_waiting() {
    use autotune_benchmark_worker::sampler::framing::FaultSlot;
    let slot = FaultSlot::new();
    assert!(slot.store(vec![1, 2]).is_ok());
    assert_eq!(slot.store(vec![3]), Err(vec![3]));
    assert_eq!(slot.take().unwrap(), Some(vec![1, 2]));
    assert_eq!(slot.take().unwrap(), None);
}

#[test]
fn partial_failed_write_keeps_cumulative_count_and_never_retries_successor_bytes() {
    use autotune_benchmark_worker::sampler::framing::{write_packet, Sink};
    struct Partial {
        calls: usize,
    }
    impl Sink for Partial {
        fn write(&mut self, _: &[u8]) -> ReadStep {
            self.calls += 1;
            ReadStep {
                reported: 2,
                error: (self.calls == 2).then_some(995),
            }
        }
    }
    let mut sink = Partial { calls: 0 };
    let rejected = write_packet(&mut sink, b"abcdef").unwrap_err();
    assert_eq!(sink.calls, 2);
    assert_eq!(rejected.reported, 4);
    assert_eq!(rejected.phase, Phase::Write);
    assert_eq!(rejected.fault, ReadFault::Os(995));
}
