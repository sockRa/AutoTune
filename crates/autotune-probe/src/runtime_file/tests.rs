use super::*;
struct Fake {
    data: Vec<u8>,
    position: usize,
    reads: u32,
    identities: u32,
    first: Io<Identity>,
    last: Io<Identity>,
    fail_read: Option<u32>,
    over_read: bool,
    fragment: usize,
}
fn id(bytes: u64) -> Identity {
    Identity {
        volume_serial: 1,
        index: 2,
        bytes,
        last_write: 3,
        attributes: 0x20,
        file_type: 1,
        file_type_error: None,
    }
}
impl Fake {
    fn new(data: Vec<u8>) -> Self {
        let first = Ok(id(data.len() as u64));
        Self {
            data,
            position: 0,
            reads: 0,
            identities: 0,
            first,
            last: first,
            fail_read: None,
            over_read: false,
            fragment: usize::MAX,
        }
    }
}
impl Source for Fake {
    fn identity(&mut self) -> Io<Identity> {
        self.identities += 1;
        if self.identities == 1 {
            self.first
        } else {
            self.last
        }
    }
    fn read(&mut self, buffer: &mut [u8]) -> Io<usize> {
        let index = self.reads;
        self.reads += 1;
        if self.fail_read == Some(index) {
            return Err(Some(5));
        }
        if self.over_read {
            return Ok(buffer.len() + 1);
        }
        let n = buffer
            .len()
            .min(self.data.len() - self.position)
            .min(self.fragment);
        buffer[..n].copy_from_slice(&self.data[self.position..self.position + n]);
        self.position += n;
        Ok(n)
    }
}
fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[test]
fn exact_short_and_multi_chunk_hashes_match_independent_golden_vectors() {
    for (data, expected) in [
        (
            b"abc".to_vec(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        (
            vec![b'a'; 1_000_000],
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0",
        ),
    ] {
        let mut source = Fake::new(data);
        let mut report = Report::default();
        let mut checks = 0;
        report.measure(&mut source, |_| {
            checks += 1;
            true
        });
        assert!(report.complete());
        assert_eq!(report.bytes, source.data.len() as u64);
        assert_eq!(hex(report.sha256.unwrap()), expected);
        assert_eq!(source.identities, 2);
        assert_eq!(checks, source.reads * 2 + 4);
        assert_eq!(report.last_read, Some(Ok(0)));
    }
}
#[test]
fn every_guard_boundary_blocks_successors_and_retains_returned_raw_facts() {
    // Metadata, two reads (payload/EOF), final metadata: eight guard calls.
    for boundary in 0..8 {
        let mut source = Fake::new(b"abc".to_vec());
        let mut report = Report::default();
        let mut count = 0;
        report.measure(&mut source, |_| {
            let i = count;
            count += 1;
            i != boundary
        });
        assert_eq!(report.fault, Some(Fault::Guard));
        assert_eq!(report.sha256, None);
        assert_eq!(count, boundary + 1);
        assert_eq!(
            source.identities,
            if boundary == 0 {
                0
            } else if boundary < 7 {
                1
            } else {
                2
            }
        );
        assert_eq!(
            source.reads,
            match boundary {
                0..=2 => 0,
                3..=4 => 1,
                _ => 2,
            }
        );
        if boundary >= 3 {
            assert!(report.last_read.is_some());
            assert_eq!(report.bytes, 3);
        }
        let reads = source.reads;
        report.measure(&mut source, |_| panic!("no retry"));
        assert_eq!(source.reads, reads);
    }
}
#[test]
fn every_read_error_keeps_exact_os_code_and_partial_byte_count() {
    for failed in 0..4 {
        let mut source = Fake::new(vec![7; CHUNK_BYTES * 2 + 1]);
        source.fail_read = Some(failed);
        let mut report = Report::default();
        report.measure(&mut source, |_| true);
        assert_eq!(report.fault, Some(Fault::Read));
        assert_eq!(report.last_read, Some(Err(Some(5))));
        assert_eq!(source.reads, failed + 1);
        assert_eq!(
            report.bytes,
            match failed {
                0 => 0,
                1 => CHUNK_BYTES as u64,
                2 => (CHUNK_BYTES * 2) as u64,
                _ => (CHUNK_BYTES * 2 + 1) as u64,
            }
        );
        assert_eq!(report.sha256, None);
        assert_eq!(report.after, None);
    }
}
#[test]
fn metadata_error_change_and_unavailable_type_cannot_supply_a_digest() {
    for mode in 0..9 {
        let mut source = Fake::new(b"abc".to_vec());
        let mut expected = Fault::Changed;
        match mode {
            0 => {
                source.first = Err(Some(32));
                expected = Fault::Metadata;
            }
            1 => {
                source.last = Err(None);
                expected = Fault::Metadata;
            }
            2 => source.last = Ok(Identity { index: 99, ..id(3) }),
            3 => {
                source.last = Ok(Identity {
                    volume_serial: 99,
                    ..id(3)
                })
            }
            4 => {
                source.last = Ok(Identity {
                    last_write: 99,
                    ..id(3)
                })
            }
            5 => {
                source.first = Ok(Identity {
                    attributes: 0x10,
                    ..id(3)
                });
                expected = Fault::FileKind;
            }
            6 => {
                source.first = Ok(Identity {
                    attributes: 0x400,
                    ..id(3)
                });
                expected = Fault::FileKind;
            }
            7 => {
                source.first = Ok(Identity {
                    file_type: 3,
                    ..id(3)
                });
                expected = Fault::FileKind;
            }
            _ => {
                source.first = Ok(Identity {
                    file_type: 0,
                    file_type_error: Some(6),
                    ..id(3)
                });
                expected = Fault::FileKind;
            }
        }
        let mut report = Report::default();
        report.measure(&mut source, |_| true);
        assert_eq!(report.fault, Some(expected));
        assert_eq!(report.sha256, None);
        assert_eq!(report.before, Some(source.first));
        if source.identities == 2 {
            assert_eq!(report.after, Some(source.last));
        }
    }
}
#[test]
fn empty_oversize_truncated_extra_and_malicious_counts_fail_closed() {
    for mode in 0..5 {
        let mut source = Fake::new(b"abc".to_vec());
        let expected = match mode {
            0 => {
                source.first = Ok(id(0));
                Fault::Size
            }
            1 => {
                source.first = Ok(id(MAX_FILE_BYTES + 1));
                Fault::Size
            }
            2 => {
                source.first = Ok(id(4));
                Fault::Truncated
            }
            3 => {
                source.first = Ok(id(2));
                Fault::Extra
            }
            _ => {
                source.over_read = true;
                Fault::Protocol
            }
        };
        let mut report = Report::default();
        report.measure(&mut source, |_| true);
        assert_eq!(report.fault, Some(expected));
        assert!(!report.complete());
        assert!(source.reads <= 2);
        assert_eq!(report.sha256, None);
    }
}
#[test]
fn fragmented_reads_have_a_fixed_call_ceiling_and_no_retry() {
    let mut source = Fake::new(vec![7; MAX_READS as usize + 1]);
    source.fragment = 1;
    let mut report = Report::default();
    report.measure(&mut source, |_| true);
    assert_eq!(source.reads, MAX_READS);
    assert_eq!(report.fault, Some(Fault::ReadCount));
    assert_eq!(report.bytes, u64::from(MAX_READS));
    assert_eq!(report.sha256, None);
    report.measure(&mut source, |_| panic!("no retry"));
    assert_eq!(source.reads, MAX_READS);
}
#[test]
fn file_identity_match_requires_complete_equal_before_and_after_evidence() {
    let mut source = Fake::new(b"abc".to_vec());
    let mut report = Report::default();
    assert!(!report.matches_identity(id(3)));
    report.measure(&mut source, |_| true);
    assert!(report.matches_identity(id(3)));
    for other in [
        Identity {
            volume_serial: 2,
            ..id(3)
        },
        Identity { index: 3, ..id(3) },
        id(4),
        Identity {
            last_write: 4,
            ..id(3)
        },
    ] {
        assert!(!report.matches_identity(other));
    }
}
#[test]
fn post_read_unwind_keeps_raw_return_and_never_commits_partial_digest() {
    let mut source = Fake::new(b"abc".to_vec());
    let mut report = Report::default();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        report.measure(&mut source, |s| {
            if matches!(s, Stage::ReadAfter(0)) {
                panic!("clock callback");
            }
            true
        })
    }));
    assert!(result.is_err());
    assert_eq!(report.last_read, Some(Ok(3)));
    assert_eq!(report.bytes, 3);
    assert_eq!(report.sha256, None);
    report.measure(&mut source, |_| panic!("no retry after unwind"));
    assert_eq!(source.reads, 1);
}
