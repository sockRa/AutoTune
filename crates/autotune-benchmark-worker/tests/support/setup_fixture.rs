use super::a;
use autotune_benchmark_worker::{
    evidence::Digest,
    workload::{
        child_options::{self, Variable},
        nvml_trace::FileCheck,
        setup_model::{Entry, Operation, Output, Return},
        wire::WorkerBinding,
    },
};
use autotune_probe::runtime_file::{FileRecord, Identity, Kind, PathReturn, Report, Stage};
pub(super) struct Fixture {
    pub(super) entries: Vec<Entry>,
    pub(super) checks: Vec<FileCheck>,
    pub(super) file: FileRecord,
    pub(super) environment: child_options::Facts,
    pub(super) ready: autotune_benchmark_worker::workload::wire::Message,
}
pub(super) fn env(v: Variable) -> child_options::Read {
    let mut units = [0; 16];
    for (a, b) in units.iter_mut().zip(v.expected().encode_utf16()) {
        *a = b;
    }
    child_options::Read {
        length: v.expected().len() as u32,
        error: None,
        units,
    }
}
pub(super) fn push(entries: &mut Vec<Entry>, operation: Operation, output: Output) {
    entries.push(Entry {
        operation,
        before: Some(a(0)),
        after: Some(a(0)),
        attempted: true,
        raw: Some(Return { status: 0, output }),
    });
}
pub(super) fn retime(f: &mut Fixture) {
    for (i, e) in f.entries.iter_mut().enumerate() {
        let t = if i < 2 {
            2000 + i as u64 * 200
        } else {
            6000 + (i as u64 - 2) * 200
        };
        e.before = Some(a(t));
        e.after = Some(a(t + 100));
    }
}
pub(super) fn fixture(bind: WorkerBinding, worker: Digest, lanes: u32) -> Fixture {
    use autotune_benchmark_worker::workload::wire::Message;
    let mut entries = Vec::new();
    for v in Variable::ALL {
        push(
            &mut entries,
            Operation::Environment(v),
            Output::Environment(env(v)),
        );
    }
    for (op, out) in [
        (Operation::Initialize, Output::None),
        (Operation::DriverVersion, Output::Integer(12090)),
        (Operation::ModuleLoadingMode, Output::Integer(1)),
        (Operation::DeviceCount, Output::Integer(1)),
        (Operation::Device(0), Output::Integer(42)),
        (
            Operation::Uuid(42),
            Output::Uuid(bind.monitor().device.uuid),
        ),
    ] {
        push(&mut entries, op, out);
    }
    let d = bind.monitor().device;
    for (attribute, n) in [
        (50, d.domain as i32),
        (33, d.bus as i32),
        (34, d.device as i32),
        (75, 7),
        (76, 5),
    ] {
        push(
            &mut entries,
            Operation::Attribute {
                device: 42,
                attribute,
            },
            Output::Integer(n),
        );
    }
    for (op, out) in [
        (Operation::ContextCreate, Output::HandleIsNull(false)),
        (Operation::ContextDevice, Output::Integer(42)),
        (Operation::ModuleLoad, Output::HandleIsNull(false)),
        (Operation::Function, Output::HandleIsNull(false)),
        (
            Operation::MemoryInfo,
            Output::Memory {
                free: 1 << 30,
                total: 1 << 30,
            },
        ),
    ] {
        push(&mut entries, op, out);
    }
    for i in 0..4 {
        push(
            &mut entries,
            Operation::Allocate(i),
            Output::AllocationIsZero(false),
        );
    }
    push(
        &mut entries,
        Operation::StreamCreate,
        Output::HandleIsNull(false),
    );
    let stages = [
        Stage::DirectoryBefore,
        Stage::DirectoryAfter,
        Stage::OpenBefore,
        Stage::OpenAfter,
        Stage::MetadataBefore,
        Stage::MetadataAfter,
        Stage::ReadBefore(0),
        Stage::ReadAfter(0),
        Stage::ReadBefore(1),
        Stage::ReadAfter(1),
        Stage::FinalMetadataBefore,
        Stage::FinalMetadataAfter,
        Stage::LibraryLoadBefore,
        Stage::LibraryLoadAfter,
        Stage::ModulePathBefore,
        Stage::ModulePathAfter,
        Stage::LoadedOpenBefore,
        Stage::LoadedOpenAfter,
        Stage::LoadedMetadataBefore,
        Stage::LoadedMetadataAfter,
    ];
    let checks = stages
        .into_iter()
        .enumerate()
        .map(|(i, stage)| FileCheck {
            stage,
            anchor: Some(a(3000 + i as u64 * 100)),
        })
        .collect();
    let id = Identity {
        volume_serial: 1,
        index: 7,
        bytes: 3,
        last_write: 123,
        attributes: 0x20,
        file_type: 1,
        file_type_error: None,
    };
    let path: Vec<_> = "C:\\Windows\\System32\\nvcuda.dll".encode_utf16().collect();
    let sha = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22,
        0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00,
        0x15, 0xad,
    ];
    let file = FileRecord {
        kind: Kind::Cuda,
        requested: Some(path.clone()),
        loaded: Some(path.clone()),
        directory: Some(PathReturn {
            length: 19,
            error: None,
        }),
        open: Some(Ok(())),
        report: Report {
            before: Some(Ok(id)),
            after: Some(Ok(id)),
            reads: 2,
            last_read: Some(Ok(0)),
            bytes: 3,
            sha256: Some(sha),
            fault: None,
        },
        module_path: Some(PathReturn {
            length: path.len() as u32,
            error: None,
        }),
        loaded_open: Some(Ok(())),
        loaded_identity: Some(Ok(id)),
        fault: None,
        held: true,
        bound: true,
    };
    let ready = Message::Ready {
        anchor: a(100_000),
        identity: d,
        context: d,
        worker_digest: worker,
        driver_version: 12090,
        compute_major: 7,
        compute_minor: 5,
        prepared_lanes: lanes,
    };
    let mut f = Fixture {
        entries,
        checks,
        file,
        environment: child_options::Facts {
            cache_disable: Some(env(Variable::CacheDisable)),
            module_loading: Some(env(Variable::ModuleLoading)),
        },
        ready,
    };
    retime(&mut f);
    f
}
