//! Runtime facts carried by starting response v2. Reported file/clock assertions
//! are not authenticated reads, mapped-page attestations or native admission.
use super::{
    nvml_trace::{FileCheck, Ledger},
    settings::{self, Block},
    starting::Scope,
};
use crate::{
    evidence::{Object, Value},
    monitor::clock::Clock,
    sampler::wire_model::{hex, object, unhex},
};
use autotune_probe::runtime_file::{
    Fault as ReadFault, FileRecord, Identity, ImageFault, Io, Kind, PathReturn, Report, Stage,
    MAX_CHECKS, MAX_FILE_BYTES, MAX_PATH_UNITS, MAX_READS,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoadError {
    pub known: bool,
    pub code: Option<i32>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    SimulatedFixture,
    NativeFilePath,
}
pub enum Input<'a> {
    Recorded {
        file: Option<&'a FileRecord>,
        load_error: Option<LoadError>,
        initialization: Option<i32>,
    },
    #[cfg(windows)]
    Native {
        file: Option<&'a autotune_probe::runtime_file::windows::Image>,
        load_error: Option<&'a autotune_probe::nvml::RuntimeLoadError>,
        initialization: Option<i32>,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Shape,
    Capacity,
    Missing,
    File,
    Clock,
    Deadline,
    Loader,
    Initialization,
    Overflow,
    Kind,
    Simulated,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reported {
    source: Source,
    file: Option<FileRecord>,
    load_error: Option<LoadError>,
    initialization: Option<i32>,
    checks: Vec<FileCheck>,
    overflow: Option<FileCheck>,
}
impl Reported {
    pub fn source(&self) -> Source {
        self.source
    }
    pub fn file(&self) -> Option<&FileRecord> {
        self.file.as_ref()
    }
    pub fn load_error(&self) -> Option<LoadError> {
        self.load_error
    }
    pub fn initialization(&self) -> Option<i32> {
        self.initialization
    }
    pub fn checks(&self) -> &[FileCheck] {
        &self.checks
    }
    pub fn overflow(&self) -> Option<FileCheck> {
        self.overflow
    }
    /// Consistency/readability screen only. It never proves actual reads,
    /// consent, peer identity, exclusive ownership, or hardware qualification.
    pub fn assess(&self, scope: &Scope) -> Result<(), Fault> {
        if self.source != Source::NativeFilePath {
            return Err(Fault::Simulated);
        }
        if self.overflow.is_some() {
            return Err(Fault::Overflow);
        }
        if self.load_error.is_some() {
            return Err(Fault::Loader);
        }
        if self.initialization != Some(0) {
            return Err(Fault::Initialization);
        }
        let file = self.file.as_ref().ok_or(Fault::Missing)?;
        if file.kind != Kind::Nvml {
            return Err(Fault::Kind);
        }
        let first = file.report.before.and_then(Result::ok).ok_or(Fault::File)?;
        if file.fault.is_some()
            || !file.bound
            || !file.held
            || file.requested.is_none()
            || file.loaded.is_none()
            || file.open != Some(Ok(()))
            || file.loaded_open != Some(Ok(()))
            || !file.report.complete()
            || !file.report.matches_identity(first)
            || file.loaded_identity != Some(Ok(first))
            || file.report.bytes != first.bytes
            || first.bytes == 0
            || first.bytes > MAX_FILE_BYTES
            || file.report.reads == 0
            || file.report.reads > MAX_READS
            || file.report.last_read != Some(Ok(0))
            || first.file_type != 1
            || first.file_type_error.is_some()
            || first.attributes & (0x10 | 0x400) != 0
            || !file.requested.as_ref().is_some_and(|p| absolute_path(p))
            || !file.loaded.as_ref().is_some_and(|p| absolute_path(p))
            || !file
                .directory
                .is_some_and(|p| p.error.is_none() && p.length > 0 && p.length < 4096)
            || !file.module_path.is_some_and(|p| {
                p.error.is_none()
                    && p.length > 0
                    && p.length < 4096
                    && file
                        .loaded
                        .as_ref()
                        .is_some_and(|v| v.len() == p.length as usize)
            })
        {
            return Err(Fault::File);
        }
        let prefix = [
            Stage::DirectoryBefore,
            Stage::DirectoryAfter,
            Stage::OpenBefore,
            Stage::OpenAfter,
            Stage::MetadataBefore,
            Stage::MetadataAfter,
        ];
        let suffix = [
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
        if self.checks.len() != 16 + file.report.reads as usize * 2 {
            return Err(Fault::Missing);
        }
        let expected = prefix
            .into_iter()
            .chain((0..file.report.reads).flat_map(|n| [Stage::ReadBefore(n), Stage::ReadAfter(n)]))
            .chain(suffix);
        if self.checks.iter().zip(expected).any(|(c, s)| c.stage != s) {
            return Err(Fault::Shape);
        }
        let d = scope.draft();
        let mut clock =
            Clock::temporal(d.frequency, d.clock_token, d.epoch).map_err(|_| Fault::Clock)?;
        let mut last = d.epoch;
        for c in &self.checks {
            let a = c.anchor.ok_or(Fault::Clock)?;
            if a.qpc_before < last.qpc_after {
                return Err(Fault::Clock);
            }
            let n = clock.observe(a).map_err(|_| Fault::Clock)?;
            last = a;
            if n >= autotune_fma_reference::profile::Profile::approved_design()
                .limits
                .preflight_ns
            {
                return Err(Fault::Deadline);
            }
        }
        Ok(())
    }
}
pub(super) fn value(input: Input<'_>, ledger: &Ledger) -> Result<Value, Block> {
    if ledger.file_checks().len() > MAX_CHECKS {
        return Err(Block::Capacity);
    }
    let (file, load_error, initialization, source) = match input {
        Input::Recorded {
            file,
            load_error,
            initialization,
        } => {
            if file.is_some_and(|f| {
                [&f.requested, &f.loaded]
                    .into_iter()
                    .flatten()
                    .any(|p| p.len() > MAX_PATH_UNITS)
            }) {
                return Err(Block::Capacity);
            }
            (
                file.cloned(),
                load_error,
                initialization,
                Source::SimulatedFixture,
            )
        }
        #[cfg(windows)]
        Input::Native {
            file,
            load_error,
            initialization,
        } => {
            let error = load_error
                .map(|e| {
                    use std::error::Error;
                    match e {
                        autotune_probe::nvml::RuntimeLoadError::LoadLibraryExW { .. } => {
                            Ok(LoadError {
                                known: true,
                                code: e
                                    .source()
                                    .and_then(|s| s.downcast_ref::<std::io::Error>())
                                    .and_then(std::io::Error::raw_os_error),
                            })
                        }
                        autotune_probe::nvml::RuntimeLoadError::LoadLibraryExWUnknown => {
                            Ok(LoadError {
                                known: false,
                                code: None,
                            })
                        }
                        _ => Err(Block::Scope),
                    }
                })
                .transpose()?;
            (
                file.map(|f| f.record()),
                error,
                initialization,
                Source::NativeFilePath,
            )
        }
    };
    if let Some(f) = &file {
        if [&f.requested, &f.loaded]
            .into_iter()
            .flatten()
            .any(|p| p.len() > MAX_PATH_UNITS)
        {
            return Err(Block::Capacity);
        }
    }
    if load_error.is_some_and(|e| !e.known && e.code.is_some()) {
        return Err(Block::Scope);
    }
    Ok(object([
        ("version", text("1")),
        (
            "source",
            text(match source {
                Source::SimulatedFixture => "simulated_fixture",
                Source::NativeFilePath => "native_file_path",
            }),
        ),
        ("file", file.as_ref().map(file_value).unwrap_or(Value::Null)),
        (
            "load_error",
            load_error
                .map(|e| object([("known", Value::Bool(e.known)), ("code", oi(e.code))]))
                .unwrap_or(Value::Null),
        ),
        ("initialization", oi(initialization)),
        (
            "file_checks",
            Value::Array(ledger.file_checks().iter().map(check_value).collect()),
        ),
        (
            "file_overflow",
            ledger
                .file_overflow()
                .as_ref()
                .map(check_value)
                .unwrap_or(Value::Null),
        ),
    ]))
}
pub(super) fn read(v: &Value) -> Result<Reported, Fault> {
    let o = obj(
        v,
        &[
            "version",
            "source",
            "file",
            "load_error",
            "initialization",
            "file_checks",
            "file_overflow",
        ],
    )?;
    if txt(get(o, "version")?)? != "1" {
        return Err(Fault::Shape);
    }
    let source = match txt(get(o, "source")?)? {
        "simulated_fixture" => Source::SimulatedFixture,
        "native_file_path" => Source::NativeFilePath,
        _ => return Err(Fault::Shape),
    };
    let entries = get(o, "file_checks")?.as_array().ok_or(Fault::Shape)?;
    if entries.len() > MAX_CHECKS {
        return Err(Fault::Capacity);
    }
    let file = optional(get(o, "file")?, read_file)?;
    let load_error = optional(get(o, "load_error")?, |v| {
        let o = obj(v, &["known", "code"])?;
        let e = LoadError {
            known: boolv(get(o, "known")?)?,
            code: opi(get(o, "code")?)?,
        };
        if !e.known && e.code.is_some() {
            return Err(Fault::Shape);
        }
        Ok(e)
    })?;
    let initialization = opi(get(o, "initialization")?)?;
    let checks = entries
        .iter()
        .map(read_check)
        .collect::<Result<Vec<_>, _>>()?;
    let overflow = optional(get(o, "file_overflow")?, read_check)?;
    Ok(Reported {
        source,
        file,
        load_error,
        initialization,
        checks,
        overflow,
    })
}
fn absolute_path(p: &[u16]) -> bool {
    !p.contains(&0)
        && p.len() >= 3
        && (((p[0] >= 65 && p[0] <= 90) || (p[0] >= 97 && p[0] <= 122))
            && p[1] == 58
            && (p[2] == 92 || p[2] == 47)
            || p[0] == 92 && p[1] == 92)
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}
fn oi(n: Option<i32>) -> Value {
    n.map(|n| Value::I64(n.into())).unwrap_or(Value::Null)
}
fn ou(n: Option<u32>) -> Value {
    n.map(|n| Value::U64(n.into())).unwrap_or(Value::Null)
}
fn path_value(p: &Option<Vec<u16>>) -> Value {
    p.as_ref()
        .map(|p| {
            text(&hex(&p
                .iter()
                .flat_map(|n| n.to_le_bytes())
                .collect::<Vec<_>>()))
        })
        .unwrap_or(Value::Null)
}
fn path_return(p: PathReturn) -> Value {
    object([
        ("length", Value::U64(p.length.into())),
        ("error", ou(p.error)),
    ])
}
fn io_value<T: Copy>(io: Io<T>, f: impl FnOnce(T) -> Value) -> Value {
    match io {
        Ok(v) => object([
            ("status", text("returned")),
            ("value", f(v)),
            ("error", Value::Null),
        ]),
        Err(e) => object([
            ("status", text("failed")),
            ("value", Value::Null),
            ("error", oi(e)),
        ]),
    }
}
fn identity(i: Identity) -> Value {
    object([
        ("volume_serial", Value::U64(i.volume_serial.into())),
        ("index", Value::U64(i.index)),
        ("bytes", Value::U64(i.bytes)),
        ("last_write", Value::U64(i.last_write)),
        ("attributes", Value::U64(i.attributes.into())),
        ("file_type", Value::U64(i.file_type.into())),
        ("file_type_error", ou(i.file_type_error)),
    ])
}
fn file_value(f: &FileRecord) -> Value {
    object([
        (
            "kind",
            text(match f.kind {
                Kind::Cuda => "cuda",
                Kind::Nvml => "nvml",
            }),
        ),
        ("requested_utf16le", path_value(&f.requested)),
        ("loaded_utf16le", path_value(&f.loaded)),
        (
            "directory",
            f.directory.map(path_return).unwrap_or(Value::Null),
        ),
        (
            "open",
            f.open
                .map(|i| io_value(i, |_| Value::Null))
                .unwrap_or(Value::Null),
        ),
        (
            "measurement",
            object([
                (
                    "before",
                    f.report
                        .before
                        .map(|i| io_value(i, identity))
                        .unwrap_or(Value::Null),
                ),
                (
                    "after",
                    f.report
                        .after
                        .map(|i| io_value(i, identity))
                        .unwrap_or(Value::Null),
                ),
                ("reads", Value::U64(f.report.reads.into())),
                (
                    "last_read",
                    f.report
                        .last_read
                        .map(|i| io_value(i, |n| Value::U64(n as u64)))
                        .unwrap_or(Value::Null),
                ),
                ("bytes", Value::U64(f.report.bytes)),
                (
                    "sha256",
                    f.report
                        .sha256
                        .map(|s| text(&hex(&s)))
                        .unwrap_or(Value::Null),
                ),
                (
                    "fault",
                    f.report
                        .fault
                        .map(|e| text(read_fault_name(e)))
                        .unwrap_or(Value::Null),
                ),
            ]),
        ),
        (
            "module_path",
            f.module_path.map(path_return).unwrap_or(Value::Null),
        ),
        (
            "loaded_open",
            f.loaded_open
                .map(|i| io_value(i, |_| Value::Null))
                .unwrap_or(Value::Null),
        ),
        (
            "loaded_identity",
            f.loaded_identity
                .map(|i| io_value(i, identity))
                .unwrap_or(Value::Null),
        ),
        (
            "fault",
            f.fault
                .map(|e| text(image_fault_name(e)))
                .unwrap_or(Value::Null),
        ),
        ("held", Value::Bool(f.held)),
        ("bound", Value::Bool(f.bound)),
    ])
}
fn check_value(c: &FileCheck) -> Value {
    let (name, index) = stage(c.stage);
    object([
        ("stage", text(name)),
        (
            "index",
            index.map(|n| Value::U64(n.into())).unwrap_or(Value::Null),
        ),
        (
            "anchor",
            c.anchor.map(settings::raw_anchor).unwrap_or(Value::Null),
        ),
    ])
}
fn stage(s: Stage) -> (&'static str, Option<u32>) {
    match s {
        Stage::DirectoryBefore => ("directory_before", None),
        Stage::DirectoryAfter => ("directory_after", None),
        Stage::OpenBefore => ("open_before", None),
        Stage::OpenAfter => ("open_after", None),
        Stage::MetadataBefore => ("metadata_before", None),
        Stage::MetadataAfter => ("metadata_after", None),
        Stage::ReadBefore(n) => ("read_before", Some(n)),
        Stage::ReadAfter(n) => ("read_after", Some(n)),
        Stage::FinalMetadataBefore => ("final_metadata_before", None),
        Stage::FinalMetadataAfter => ("final_metadata_after", None),
        Stage::LibraryLoadBefore => ("library_load_before", None),
        Stage::LibraryLoadAfter => ("library_load_after", None),
        Stage::ModulePathBefore => ("module_path_before", None),
        Stage::ModulePathAfter => ("module_path_after", None),
        Stage::LoadedOpenBefore => ("loaded_open_before", None),
        Stage::LoadedOpenAfter => ("loaded_open_after", None),
        Stage::LoadedMetadataBefore => ("loaded_metadata_before", None),
        Stage::LoadedMetadataAfter => ("loaded_metadata_after", None),
    }
}
fn read_check(v: &Value) -> Result<FileCheck, Fault> {
    let o = obj(v, &["stage", "index", "anchor"])?;
    let name = txt(get(o, "stage")?)?;
    let index = optional(get(o, "index")?, num::<u32>)?;
    let stages = [
        Stage::DirectoryBefore,
        Stage::DirectoryAfter,
        Stage::OpenBefore,
        Stage::OpenAfter,
        Stage::MetadataBefore,
        Stage::MetadataAfter,
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
    let s = if let Some(n) = index {
        match name {
            "read_before" => Stage::ReadBefore(n),
            "read_after" => Stage::ReadAfter(n),
            _ => return Err(Fault::Shape),
        }
    } else {
        stages
            .into_iter()
            .find(|s| stage(*s).0 == name)
            .ok_or(Fault::Shape)?
    };
    Ok(FileCheck {
        stage: s,
        anchor: settings::parse_anchor(get(o, "anchor")?).map_err(|_| Fault::Shape)?,
    })
}
fn read_fault_name(f: ReadFault) -> &'static str {
    match f {
        ReadFault::Guard => "guard",
        ReadFault::Metadata => "metadata",
        ReadFault::FileKind => "file_kind",
        ReadFault::Size => "size",
        ReadFault::Read => "read",
        ReadFault::ReadCount => "read_count",
        ReadFault::Protocol => "protocol",
        ReadFault::Truncated => "truncated",
        ReadFault::Extra => "extra",
        ReadFault::Changed => "changed",
    }
}
fn image_fault_name(f: ImageFault) -> &'static str {
    match f {
        ImageFault::Guard => "guard",
        ImageFault::Directory => "directory",
        ImageFault::Open => "open",
        ImageFault::Measurement => "measurement",
        ImageFault::ModulePath => "module_path",
        ImageFault::LoadedOpen => "loaded_open",
        ImageFault::LoadedMetadata => "loaded_metadata",
        ImageFault::Identity => "identity",
        ImageFault::Repeated => "repeated",
    }
}
fn obj<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Object, Fault> {
    let o = v.as_object().ok_or(Fault::Shape)?;
    o.require_exact(keys).map_err(|_| Fault::Shape)?;
    Ok(o)
}
fn get<'a>(o: &'a Object, key: &str) -> Result<&'a Value, Fault> {
    o.get(key).ok_or(Fault::Shape)
}
fn txt(v: &Value) -> Result<&str, Fault> {
    v.as_text().ok_or(Fault::Shape)
}
fn num<T: TryFrom<u64>>(v: &Value) -> Result<T, Fault> {
    T::try_from(v.as_u64().map_err(|_| Fault::Shape)?).map_err(|_| Fault::Shape)
}
fn boolv(v: &Value) -> Result<bool, Fault> {
    if let Value::Bool(b) = v {
        Ok(*b)
    } else {
        Err(Fault::Shape)
    }
}
fn optional<T>(v: &Value, f: impl FnOnce(&Value) -> Result<T, Fault>) -> Result<Option<T>, Fault> {
    if v == &Value::Null {
        Ok(None)
    } else {
        f(v).map(Some)
    }
}
fn opi(v: &Value) -> Result<Option<i32>, Fault> {
    optional(v, |v| {
        i32::try_from(v.as_i64().map_err(|_| Fault::Shape)?).map_err(|_| Fault::Shape)
    })
}
fn read_path(v: &Value) -> Result<Option<Vec<u16>>, Fault> {
    optional(v, |v| {
        let t = txt(v)?;
        if t.len() > MAX_PATH_UNITS * 4 || t.len() % 4 != 0 {
            return Err(Fault::Capacity);
        }
        let mut out = Vec::with_capacity(t.len() / 4);
        for chunk in t.as_bytes().as_chunks::<4>().0 {
            let s = std::str::from_utf8(chunk).map_err(|_| Fault::Shape)?;
            if !s
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Fault::Shape);
            }
            out.push(
                u16::from_str_radix(s, 16)
                    .map_err(|_| Fault::Shape)?
                    .swap_bytes(),
            );
        }
        Ok(out)
    })
}
fn read_path_return(v: &Value) -> Result<PathReturn, Fault> {
    let o = obj(v, &["length", "error"])?;
    Ok(PathReturn {
        length: num(get(o, "length")?)?,
        error: optional(get(o, "error")?, num::<u32>)?,
    })
}
fn read_io<T>(v: &Value, f: impl FnOnce(&Value) -> Result<T, Fault>) -> Result<Io<T>, Fault> {
    let o = obj(v, &["status", "value", "error"])?;
    match txt(get(o, "status")?)? {
        "returned" => {
            if get(o, "error")? != &Value::Null {
                return Err(Fault::Shape);
            }
            Ok(Ok(f(get(o, "value")?)?))
        }
        "failed" => {
            if get(o, "value")? != &Value::Null {
                return Err(Fault::Shape);
            }
            Ok(Err(opi(get(o, "error")?)?))
        }
        _ => Err(Fault::Shape),
    }
}
fn read_unit(v: &Value) -> Result<(), Fault> {
    if v == &Value::Null {
        Ok(())
    } else {
        Err(Fault::Shape)
    }
}
fn read_identity(v: &Value) -> Result<Identity, Fault> {
    let o = obj(
        v,
        &[
            "volume_serial",
            "index",
            "bytes",
            "last_write",
            "attributes",
            "file_type",
            "file_type_error",
        ],
    )?;
    Ok(Identity {
        volume_serial: num(get(o, "volume_serial")?)?,
        index: num(get(o, "index")?)?,
        bytes: num(get(o, "bytes")?)?,
        last_write: num(get(o, "last_write")?)?,
        attributes: num(get(o, "attributes")?)?,
        file_type: num(get(o, "file_type")?)?,
        file_type_error: optional(get(o, "file_type_error")?, num::<u32>)?,
    })
}
fn read_file(v: &Value) -> Result<FileRecord, Fault> {
    let o = obj(
        v,
        &[
            "kind",
            "requested_utf16le",
            "loaded_utf16le",
            "directory",
            "open",
            "measurement",
            "module_path",
            "loaded_open",
            "loaded_identity",
            "fault",
            "held",
            "bound",
        ],
    )?;
    let m = obj(
        get(o, "measurement")?,
        &[
            "before",
            "after",
            "reads",
            "last_read",
            "bytes",
            "sha256",
            "fault",
        ],
    )?;
    let fault = optional(get(m, "fault")?, |v| {
        [
            ReadFault::Guard,
            ReadFault::Metadata,
            ReadFault::FileKind,
            ReadFault::Size,
            ReadFault::Read,
            ReadFault::ReadCount,
            ReadFault::Protocol,
            ReadFault::Truncated,
            ReadFault::Extra,
            ReadFault::Changed,
        ]
        .into_iter()
        .find(|f| Some(read_fault_name(*f)) == v.as_text())
        .ok_or(Fault::Shape)
    })?;
    let image_fault = optional(get(o, "fault")?, |v| {
        [
            ImageFault::Guard,
            ImageFault::Directory,
            ImageFault::Open,
            ImageFault::Measurement,
            ImageFault::ModulePath,
            ImageFault::LoadedOpen,
            ImageFault::LoadedMetadata,
            ImageFault::Identity,
            ImageFault::Repeated,
        ]
        .into_iter()
        .find(|f| Some(image_fault_name(*f)) == v.as_text())
        .ok_or(Fault::Shape)
    })?;
    Ok(FileRecord {
        kind: match txt(get(o, "kind")?)? {
            "cuda" => Kind::Cuda,
            "nvml" => Kind::Nvml,
            _ => return Err(Fault::Shape),
        },
        requested: read_path(get(o, "requested_utf16le")?)?,
        loaded: read_path(get(o, "loaded_utf16le")?)?,
        directory: optional(get(o, "directory")?, read_path_return)?,
        open: optional(get(o, "open")?, |v| read_io(v, read_unit))?,
        report: Report {
            before: optional(get(m, "before")?, |v| read_io(v, read_identity))?,
            after: optional(get(m, "after")?, |v| read_io(v, read_identity))?,
            reads: num(get(m, "reads")?)?,
            last_read: optional(get(m, "last_read")?, |v| read_io(v, num::<usize>))?,
            bytes: num(get(m, "bytes")?)?,
            sha256: optional(get(m, "sha256")?, |v| unhex(v).map_err(|_| Fault::Shape))?,
            fault,
        },
        module_path: optional(get(o, "module_path")?, read_path_return)?,
        loaded_open: optional(get(o, "loaded_open")?, |v| read_io(v, read_unit))?,
        loaded_identity: optional(get(o, "loaded_identity")?, |v| read_io(v, read_identity))?,
        fault: image_fault,
        held: boolv(get(o, "held")?)?,
        bound: boolv(get(o, "bound")?)?,
    })
}
