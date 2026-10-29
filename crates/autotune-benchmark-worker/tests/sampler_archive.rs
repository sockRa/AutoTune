use autotune_benchmark_worker::evidence::{decode, Value};
use autotune_benchmark_worker::sampler::archive::{Archive, Artifact, IndexBinding, Sink};
use std::{collections::BTreeMap, io};
#[derive(Default)]
struct Memory {
    files: BTreeMap<&'static str, Vec<u8>>,
    fail: Option<&'static str>,
}
impl Sink for Memory {
    fn write(&mut self, name: &'static str, bytes: &[u8]) -> io::Result<()> {
        if self.fail == Some(name) {
            return Err(io::Error::other("injected disk failure"));
        }
        if self.files.contains_key(name) {
            return Err(io::Error::other("already exists"));
        }
        self.files.insert(name, bytes.to_vec());
        Ok(())
    }
}
fn binding() -> IndexBinding {
    IndexBinding {
        manifest: [1; 32],
        profile: [2; 32],
        source_tree: [3; 32],
        controller: [4; 32],
        sampler: [5; 32],
        protocol: [6; 32],
        session_digest: [7; 32],
    }
}
#[test]
fn fixed_archive_order_exact_bytes_and_index_excludes_its_own_hash() {
    let mut a = Archive::new(Memory::default());
    for (kind, bytes) in [
        (Artifact::Manifest, b"manifest".as_slice()),
        (Artifact::Profile, b"profile".as_slice()),
        (Artifact::Raw, b"raw\n".as_slice()),
        (Artifact::Session, b"session".as_slice()),
    ] {
        a.write(kind, bytes).unwrap();
    }
    let receipt = a.finish(binding()).unwrap();
    assert_eq!(receipt.primary_bytes, 26);
    assert!(receipt.total_bytes > receipt.primary_bytes);
    let files = &a.sink().files;
    assert_eq!(files.len(), 5);
    assert_eq!(files["raw-events.ndjson"], b"raw\n");
    let index = decode(&files["capture-index.json"], 65536).unwrap();
    let o = index.as_object().unwrap();
    assert_eq!(o.get("primary_bytes").unwrap().as_u64().unwrap(), 26);
    assert_eq!(
        o.get("index_scope").and_then(Value::as_text),
        Some("four_primary_artifacts_excludes_self")
    );
    assert_eq!(o.get("status").and_then(Value::as_text), Some("complete"));
    let entries = match o.get("artifacts").unwrap() {
        Value::Array(entries) => entries,
        _ => panic!("entries"),
    };
    assert_eq!(entries.len(), 4);
    let first = entries[0].as_object().unwrap();
    // Independently known SHA256("manifest"), not a second call to Archive.
    assert_eq!(
        first.get("sha256").and_then(Value::as_text),
        Some("05b3abf2579a5eb66403cd78be557fd860633a1fe2103c7642030defe32c657f")
    );
}
#[test]
fn missing_out_of_order_or_failed_write_never_produces_complete_index() {
    let mut a = Archive::new(Memory::default());
    assert!(a.write(Artifact::Raw, b"raw").is_err());
    assert!(a.finish(binding()).is_err());
    assert!(a.sink().files.is_empty());
    let mut a = Archive::new(Memory {
        fail: Some("raw-events.ndjson"),
        ..Memory::default()
    });
    a.write(Artifact::Manifest, b"manifest").unwrap();
    a.write(Artifact::Profile, b"profile").unwrap();
    assert!(a.write(Artifact::Raw, b"raw").is_err());
    assert!(a.write(Artifact::Raw, b"retry").is_err());
    assert!(a.finish(binding()).is_err());
    assert!(!a.sink().files.contains_key("capture-index.json"));
    assert_eq!(a.sink().files.len(), 2);
}
