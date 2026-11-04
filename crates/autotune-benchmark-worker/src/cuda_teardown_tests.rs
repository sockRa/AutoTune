// No actual library is loaded/released. Opaque fixture handles are only forgotten.
static CLOSE_NATIVE: AtomicUsize = AtomicUsize::new(0);
static CLOSE_FAIL: AtomicUsize = AtomicUsize::new(usize::MAX);
static CLOSE_LIBRARY: AtomicUsize = AtomicUsize::new(0);
fn close_status() -> i32 {
    let i = CLOSE_NATIVE.fetch_add(1, Ordering::SeqCst);
    if i == CLOSE_FAIL.load(Ordering::SeqCst) {
        -44
    } else {
        0
    }
}
unsafe extern "system" fn close_handle(handle: Handle) -> i32 {
    assert!(!handle.is_null());
    close_status()
}
unsafe extern "system" fn close_buffer(buffer: u64) -> i32 {
    assert_eq!(buffer, 64);
    close_status()
}
fn close_fixture(library: bool) -> CudaBackend {
    CLOSE_NATIVE.store(0, Ordering::SeqCst);
    CLOSE_FAIL.store(usize::MAX, Ordering::SeqCst);
    CLOSE_LIBRARY.store(0, Ordering::SeqCst);
    let mut b = fixture();
    b.api = Api::fake(|name| {
        Some(match name {
            "cuStreamDestroy_v2" | "cuModuleUnload" | "cuCtxDestroy_v2" => {
                Box::new(close_handle as HandleCall)
            }
            "cuMemFree_v2" => Box::new(close_buffer as Free),
            _ => return symbols(name),
        })
    })
    .unwrap();
    if library {
        b._library = Some(ManuallyDrop::new(unsafe { Library::from_raw(1) }));
    }
    b
}
fn fake_library_close(library: Library) -> Result<(), Option<i32>> {
    let _raw = library.into_raw();
    CLOSE_LIBRARY.fetch_add(1, Ordering::SeqCst);
    Ok(())
}
#[derive(Default)]
struct CloseRecorded {
    before: Vec<TeardownOperation>,
    raw: Vec<(TeardownOperation, TeardownReturn)>,
    reject_before: Option<TeardownOperation>,
    reject_after: Option<TeardownOperation>,
}
impl TeardownObserver for CloseRecorded {
    fn before(&mut self, op: TeardownOperation) -> Result<(), Fault> {
        self.before.push(op);
        if self.reject_before == Some(op) {
            Err(invalid("cleanup observer before"))
        } else {
            Ok(())
        }
    }
    fn returned(&mut self, op: TeardownOperation, raw: TeardownReturn) -> Result<(), Fault> {
        self.raw.push((op, raw));
        if self.reject_after == Some(op) {
            Err(invalid("cleanup observer after"))
        } else {
            Ok(())
        }
    }
}
#[test]
fn observed_teardown_reports_cuda_and_library_results_separately_then_closes_once() {
    let _lock = serial();
    let mut b = close_fixture(true);
    let mut o = CloseRecorded::default();
    b.close_with_release(&mut o, fake_library_close).unwrap();
    assert_eq!(
        o.before,
        vec![
            TeardownOperation::Stream,
            TeardownOperation::Buffer(0),
            TeardownOperation::Buffer(1),
            TeardownOperation::Buffer(2),
            TeardownOperation::Buffer(3),
            TeardownOperation::Module,
            TeardownOperation::Context,
            TeardownOperation::Library
        ]
    );
    assert_eq!(o.raw.len(), 8);
    assert!(o.raw[..7]
        .iter()
        .all(|(_, raw)| *raw == TeardownReturn::Cuda(0)));
    assert_eq!(
        o.raw[7].1,
        TeardownReturn::Library {
            released: true,
            error_code: None
        }
    );
    assert_eq!(CLOSE_NATIVE.load(Ordering::SeqCst), 7);
    assert_eq!(CLOSE_LIBRARY.load(Ordering::SeqCst), 1);
    assert!(b.state == State::Closed);
    assert!(b._library.is_none());
    assert_eq!(
        b.setup_resources(),
        SetupResources {
            context: false,
            module: false,
            function: false,
            stream: false,
            buffers: [false; 4]
        }
    );
    assert!(b.close_observed(&mut CloseRecorded::default()).is_err());
    drop(b);
    assert_eq!(CLOSE_NATIVE.load(Ordering::SeqCst), 7);
    assert_eq!(CLOSE_LIBRARY.load(Ordering::SeqCst), 1);
}
#[test]
fn every_cleanup_pre_or_post_rejection_prevents_successor_and_retry() {
    let _lock = serial();
    let mut b = close_fixture(true);
    let mut o = CloseRecorded::default();
    b.close_with_release(&mut o, fake_library_close).unwrap();
    let sequence = o.before;
    for (i, op) in sequence.into_iter().enumerate() {
        for after in [false, true] {
            let mut b = close_fixture(true);
            let mut o = CloseRecorded::default();
            if after {
                o.reject_after = Some(op)
            } else {
                o.reject_before = Some(op)
            };
            assert!(b.close_with_release(&mut o, fake_library_close).is_err());
            assert!(b.state == State::Poisoned);
            let expected = if i == 7 { 7 } else { i + usize::from(after) };
            assert_eq!(CLOSE_NATIVE.load(Ordering::SeqCst), expected);
            assert_eq!(
                CLOSE_LIBRARY.load(Ordering::SeqCst),
                usize::from(i == 7 && after)
            );
            assert_eq!(o.raw.len(), i + usize::from(after));
            assert!(b
                .close_with_release(&mut CloseRecorded::default(), fake_library_close)
                .is_err());
            drop(b);
            assert_eq!(CLOSE_NATIVE.load(Ordering::SeqCst), expected);
        }
    }
}
#[test]
fn every_native_cleanup_error_retains_uncertain_tokens_without_duplicate_release() {
    let _lock = serial();
    for failed in 0..7 {
        let mut b = close_fixture(true);
        CLOSE_FAIL.store(failed, Ordering::SeqCst);
        let mut o = CloseRecorded::default();
        let fault = b
            .close_with_release(&mut o, fake_library_close)
            .unwrap_err();
        assert_eq!(fault.raw_status, -44);
        assert_eq!(o.raw.last().unwrap().1, TeardownReturn::Cuda(-44));
        assert_eq!(o.raw.len(), failed + 1);
        assert_eq!(CLOSE_NATIVE.load(Ordering::SeqCst), failed + 1);
        let r = b.setup_resources();
        assert_eq!(r.stream, failed == 0);
        assert!(r.context);
        assert_eq!(r.module, failed <= 5);
        for i in 0..4 {
            assert_eq!(r.buffers[i], i + 1 >= failed);
        }
        assert!(b._library.is_some());
        assert_eq!(CLOSE_LIBRARY.load(Ordering::SeqCst), 0);
        assert!(b.close().is_err());
        drop(b);
        assert_eq!(CLOSE_NATIVE.load(Ordering::SeqCst), failed + 1);
    }
}
#[test]
fn library_failure_and_unknown_error_stay_retained_and_nonidle_cleanup_never_enters() {
    let _lock = serial();
    for code in [Some(5), None] {
        let mut b = close_fixture(true);
        let mut o = CloseRecorded::default();
        let fault = b
            .close_with_release(&mut o, |library| {
                let _ = library.into_raw();
                CLOSE_LIBRARY.fetch_add(1, Ordering::SeqCst);
                Err(code)
            })
            .unwrap_err();
        assert_eq!(fault.operation, "FreeLibrary");
        assert_eq!(
            o.raw.last().unwrap().1,
            TeardownReturn::Library {
                released: false,
                error_code: code
            }
        );
        assert!(b._library.is_some());
        assert!(b.state == State::Poisoned);
        assert_eq!(b.buffers, [0; 4]);
        assert!(b.close().is_err());
        drop(b);
        assert_eq!(CLOSE_LIBRARY.load(Ordering::SeqCst), 1);
    }
    for state in [State::InFlight, State::Poisoned, State::Closed] {
        let mut b = close_fixture(false);
        b.state = state;
        let mut o = CloseRecorded::default();
        assert!(b.close_observed(&mut o).is_err());
        assert!(o.before.is_empty());
        assert_eq!(CLOSE_NATIVE.load(Ordering::SeqCst), 0);
    }
    let mut b = close_fixture(false);
    b.state = State::Complete;
    let mut o = CloseRecorded::default();
    b.close_observed(&mut o).unwrap();
    assert_eq!(o.raw.len(), 7);
    assert_eq!(CLOSE_LIBRARY.load(Ordering::SeqCst), 0);
}
