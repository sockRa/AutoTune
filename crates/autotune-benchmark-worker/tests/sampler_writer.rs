use autotune_benchmark_worker::sampler::{
    archive::{Artifact, IndexBinding, Sink},
    budget::Budget,
    producer::BoundText,
    writer::Writer,
};
use std::{
    collections::BTreeMap,
    io,
    sync::{mpsc, Arc, Mutex},
};
struct Gated {
    files: Arc<Mutex<BTreeMap<&'static str, Vec<u8>>>>,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
    finished: mpsc::Sender<()>,
}
impl Sink for Gated {
    fn write(&mut self, name: &'static str, bytes: &[u8]) -> io::Result<()> {
        if name == "raw-events.ndjson" {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
        }
        self.files.lock().unwrap().insert(name, bytes.to_vec());
        Ok(())
    }
}
impl Drop for Gated {
    fn drop(&mut self) {
        let _ = self.finished.send(());
    }
}
#[test]
fn blocked_storage_does_not_block_enqueue_or_poll_and_receipt_follows_index_write() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let files = Arc::new(Mutex::new(BTreeMap::new()));
    let budget = Budget::new(8 * 1024 * 1024).unwrap();
    let sink = Gated {
        files: files.clone(),
        entered: entered_tx,
        release: release_rx,
        finished: finished_tx,
    };
    let mut writer = Writer::start(sink, budget.clone()).unwrap();
    for kind in [
        Artifact::Manifest,
        Artifact::Profile,
        Artifact::Raw,
        Artifact::Session,
    ] {
        writer
            .enqueue(kind, BoundText::copy("bounded bytes", &budget).unwrap())
            .unwrap();
    }
    writer
        .finish(IndexBinding {
            manifest: [1; 32],
            profile: [2; 32],
            source_tree: [3; 32],
            controller: [4; 32],
            sampler: [5; 32],
            protocol: [6; 32],
            session_digest: [7; 32],
        })
        .unwrap();
    entered_rx.recv().unwrap();
    assert!(writer.poll().is_none());
    assert!(!files.lock().unwrap().contains_key("capture-index.json"));
    release_tx.send(()).unwrap();
    finished_rx.recv().unwrap();
    assert!(writer.poll().unwrap().is_ok());
    assert!(files.lock().unwrap().contains_key("capture-index.json"));
}
#[test]
fn moving_output_to_writer_transfers_its_reservation_without_copying_bytes() {
    let source = Budget::new(100).unwrap();
    let writer = Budget::new(100).unwrap();
    let mut text = BoundText::copy("source bytes", &source).unwrap();
    assert_eq!(source.used(), 12);
    text.recharge(&writer).unwrap();
    assert_eq!(source.used(), 0);
    assert_eq!(writer.used(), 12);
    text.recharge(&writer).unwrap();
    assert_eq!(writer.used(), 12);
    drop(text);
    assert_eq!(writer.used(), 0);
}

#[test]
fn output_directory_setup_is_also_off_the_control_path() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let budget = Budget::new(8 * 1024 * 1024).unwrap();
    let mut writer = Writer::start_factory(
        move || -> io::Result<NeverWritten> {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Err(io::Error::other("directory unavailable"))
        },
        budget,
    )
    .unwrap();
    entered_rx.recv().unwrap();
    assert!(!writer.ready());
    assert!(writer.poll().is_none());
    release_tx.send(()).unwrap();
    // Dropping closes the command lane without waiting for blocked storage.
    writer.close();
}
struct NeverWritten;
impl Sink for NeverWritten {
    fn write(&mut self, _: &'static str, _: &[u8]) -> io::Result<()> {
        panic!("factory failed before any write")
    }
}

#[test]
fn storage_os_error_survives_as_bounded_incomplete_capture_without_a_complete_index() {
    use autotune_benchmark_worker::{evidence::Digest, sampler::writer::IncompleteCapture};
    struct FailedSink(mpsc::Sender<()>);
    impl Sink for FailedSink {
        fn write(&mut self, name: &'static str, _: &[u8]) -> io::Result<()> {
            assert_eq!(name, "manifest.json");
            Err(io::Error::from_raw_os_error(5))
        }
    }
    impl Drop for FailedSink {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
    let budget = Budget::new(8 * 1024 * 1024).unwrap();
    let (done, received) = mpsc::channel();
    let mut writer = Writer::start(FailedSink(done), budget.clone()).unwrap();
    writer
        .enqueue(
            Artifact::Manifest,
            BoundText::copy("manifest", &budget).unwrap(),
        )
        .unwrap();
    received.recv().unwrap();
    // Simulate a sink failing between controller poll and its next enqueue.
    // The failed send must not consume the original typed storage failure.
    assert!(writer
        .enqueue(
            Artifact::Profile,
            BoundText::copy("profile", &budget).unwrap()
        )
        .is_err());
    let failure = writer.poll().unwrap().unwrap_err();
    assert_eq!(failure.operation, "manifest.json");
    assert_eq!(failure.os_code, Some(5));
    let evidence = IncompleteCapture::retain(
        Digest::parse(&"ab".repeat(32)).unwrap(),
        Some(failure),
        Err(1168),
        false,
        || Some(BoundText::copy("raw failed capture\n", &budget).unwrap()),
        &budget,
    )
    .unwrap();
    let error = io::Error::other(evidence);
    let retained = error
        .get_ref()
        .unwrap()
        .downcast_ref::<IncompleteCapture>()
        .unwrap();
    assert_eq!(retained.failure.unwrap().os_code, Some(5));
    assert_eq!(retained.cancellation, Err(1168));
    assert!(!retained.writer_finished);
    assert!(error.to_string().contains("raw failed capture"));
    assert!(error.to_string().contains("os_code: Some(5)"));
    let raw_attempted = std::cell::Cell::new(false);
    assert!(IncompleteCapture::retain(
        retained.manifest,
        Some(failure),
        Err(1168),
        false,
        || {
            raw_attempted.set(true);
            None
        },
        &Budget::new(4095).unwrap(),
    )
    .is_err());
    assert!(!raw_attempted.get());
}
