//! Typed protocol bodies and their fixed field order. No native observations.
use super::wire::{Lane, WireBinding, WireError};
use crate::{
    evidence::{Object, Value},
    monitor::{Anchor, Metric, Ticket},
};
use autotune_probe::report::{Failure, Field, Reading, Reason, SymbolPresence};

#[derive(Clone, Debug)]
pub enum Message {
    /// Canonical manifest bytes are separately digest-bound and validated by
    /// the capture layer before launch. This message confers no consent.
    Bootstrap {
        origin: Anchor,
        raw_uuid: String,
        raw_pci: String,
        manifest: String,
    },
    Ready {
        raw_uuid: String,
        raw_pci: String,
        sampler_digest: [u8; 32],
        frequency: u64,
        anchor: Anchor,
        runtime: String,
    },
    Query(Ticket),
    MetricRecord {
        ticket: Ticket,
        start: Anchor,
        end: Anchor,
        raw: Field,
    },
    Stop {
        reason: String,
        trigger_ns: u64,
        cooperative_ns: u64,
        exit_ns: u64,
    },
    StopAck {
        anchor: Anchor,
        pending: bool,
    },
    RoleFinished {
        anchor: Anchor,
        pending: bool,
        cleanup: String,
    },
}

pub(super) fn object(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut object = Object::new();
    for (key, value) in fields {
        object
            .insert(key, value)
            .expect("fixed unique protocol keys");
    }
    Value::Object(object)
}
fn text(value: impl Into<String>) -> Value {
    Value::Text(value.into())
}
pub(super) fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}
pub(super) fn unhex<const N: usize>(value: &Value) -> Result<[u8; N], WireError> {
    let bytes = string(value)?.as_bytes();
    if bytes.len() != N * 2 {
        return Err(WireError::Body);
    }
    let mut out = [0; N];
    for (index, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        let digit = |b: u8| match b {
            b'0'..=b'9' => Ok(b - b'0'),
            b'a'..=b'f' => Ok(b - b'a' + 10),
            _ => Err(WireError::Body),
        };
        out[index] = digit(pair[0])? * 16 + digit(pair[1])?;
    }
    Ok(out)
}
pub(super) fn exact<'a>(value: &'a Value, keys: &[&str]) -> Result<&'a Object, WireError> {
    let object = value.as_object().ok_or(WireError::Body)?;
    object.require_exact(keys).map_err(|_| WireError::Body)?;
    Ok(object)
}
pub(super) fn get<'a>(object: &'a Object, key: &str) -> &'a Value {
    object.get(key).expect("exact fields validated")
}
pub(super) fn string(value: &Value) -> Result<&str, WireError> {
    value.as_text().ok_or(WireError::Body)
}
pub(super) fn uint(value: &Value) -> Result<u64, WireError> {
    value.as_u64().map_err(|_| WireError::Body)
}
fn boolean(value: &Value) -> Result<bool, WireError> {
    match value {
        Value::Bool(b) => Ok(*b),
        _ => Err(WireError::Body),
    }
}

pub(super) fn anchor_value(anchor: Anchor) -> Value {
    object([
        ("qpc_before", Value::U64(anchor.qpc_before)),
        ("qpc_after", Value::U64(anchor.qpc_after)),
        ("interrupt_100ns", Value::U64(anchor.interrupt_100ns)),
        ("unbiased_100ns", Value::U64(anchor.unbiased_100ns)),
    ])
}
fn anchor(value: &Value, binding: WireBinding) -> Result<Anchor, WireError> {
    let o = exact(
        value,
        &[
            "qpc_before",
            "qpc_after",
            "interrupt_100ns",
            "unbiased_100ns",
        ],
    )?;
    Ok(Anchor {
        clock_token: binding.monitor.clock_token,
        qpc_before: uint(get(o, "qpc_before"))?,
        qpc_after: uint(get(o, "qpc_after"))?,
        interrupt_100ns: uint(get(o, "interrupt_100ns"))?,
        unbiased_100ns: uint(get(o, "unbiased_100ns"))?,
    })
}
pub(super) fn metric_name(metric: Metric) -> &'static str {
    match metric {
        Metric::Temperature => "temperature",
        Metric::Power => "power",
        Metric::GraphicsClock => "graphics_clock",
        Metric::MemoryClock => "memory_clock",
        Metric::GpuUtilization => "gpu_utilization",
        Metric::MemoryUtilization => "memory_utilization",
    }
}
fn metric(value: &Value) -> Result<Metric, WireError> {
    Metric::ALL
        .into_iter()
        .find(|m| Some(metric_name(*m)) == value.as_text())
        .ok_or(WireError::Body)
}
fn ticket_value(ticket: Ticket) -> Value {
    object([
        ("metric", text(metric_name(ticket.metric))),
        ("slot", Value::U64(ticket.slot)),
        ("scheduled_ns", Value::U64(ticket.scheduled_ns)),
        ("issued_ns", Value::U64(ticket.start_ns)),
    ])
}
fn ticket(value: &Value, binding: WireBinding) -> Result<Ticket, WireError> {
    let o = exact(value, &["metric", "slot", "scheduled_ns", "issued_ns"])?;
    Ok(Ticket {
        binding: binding.monitor,
        metric: metric(get(o, "metric"))?,
        slot: uint(get(o, "slot"))?,
        scheduled_ns: uint(get(o, "scheduled_ns"))?,
        start_ns: uint(get(o, "issued_ns"))?,
    })
}
fn reason_name(reason: Reason) -> &'static str {
    match reason {
        Reason::RuntimeMissing => "runtime_missing",
        Reason::RuntimeLoadFailed => "runtime_load_failed",
        Reason::SymbolMissing => "symbol_missing",
        Reason::Unsupported => "unsupported",
        Reason::PermissionDenied => "permission_denied",
        Reason::QueryFailed => "query_failed",
        Reason::GpuLost => "gpu_lost",
        Reason::InvalidValue => "invalid_value",
        Reason::UnsupportedPlatform => "unsupported_platform",
    }
}
fn reason(value: &Value) -> Result<Reason, WireError> {
    [
        Reason::RuntimeMissing,
        Reason::RuntimeLoadFailed,
        Reason::SymbolMissing,
        Reason::Unsupported,
        Reason::PermissionDenied,
        Reason::QueryFailed,
        Reason::GpuLost,
        Reason::InvalidValue,
        Reason::UnsupportedPlatform,
    ]
    .into_iter()
    .find(|r| Some(reason_name(*r)) == value.as_text())
    .ok_or(WireError::Body)
}
fn field_value(field: &Field) -> Result<Value, WireError> {
    let symbol = match field.symbol_presence {
        SymbolPresence::Present => "present",
        SymbolPresence::Absent => "absent",
        SymbolPresence::NotChecked => "not_checked",
    };
    let reading = match &field.reading {
        Reading::Available { value } => object([
            ("status", text("available")),
            ("value", Value::I64(value.as_i64().ok_or(WireError::Body)?)),
        ]),
        Reading::Unavailable { failure } => object([
            ("status", text("unavailable")),
            ("reason", text(reason_name(failure.reason))),
            (
                "vendor_status",
                failure
                    .vendor_status
                    .map_or(Value::Null, |v| Value::I64(v.into())),
            ),
            ("detail", text(&failure.detail)),
        ]),
    };
    Ok(object([
        ("unit", text(&field.unit)),
        ("symbol", text(symbol)),
        ("reading", reading),
    ]))
}
fn field(value: &Value) -> Result<Field, WireError> {
    let o = exact(value, &["unit", "symbol", "reading"])?;
    let symbol_presence = match string(get(o, "symbol"))? {
        "present" => SymbolPresence::Present,
        "absent" => SymbolPresence::Absent,
        "not_checked" => SymbolPresence::NotChecked,
        _ => return Err(WireError::Body),
    };
    let reading_object = get(o, "reading").as_object().ok_or(WireError::Body)?;
    let status = reading_object
        .get("status")
        .and_then(Value::as_text)
        .ok_or(WireError::Body)?;
    let reading = match status {
        "available" => {
            let r = exact(get(o, "reading"), &["status", "value"])?;
            Reading::Available {
                value: get(r, "value")
                    .as_i64()
                    .map_err(|_| WireError::Body)?
                    .into(),
            }
        }
        "unavailable" => {
            let r = exact(
                get(o, "reading"),
                &["status", "reason", "vendor_status", "detail"],
            )?;
            let vendor_status = match get(r, "vendor_status") {
                Value::Null => None,
                v => Some(
                    i32::try_from(v.as_i64().map_err(|_| WireError::Body)?)
                        .map_err(|_| WireError::Body)?,
                ),
            };
            Reading::Unavailable {
                failure: Failure {
                    reason: reason(get(r, "reason"))?,
                    vendor_status,
                    detail: string(get(r, "detail"))?.into(),
                },
            }
        }
        _ => return Err(WireError::Body),
    };
    Ok(Field {
        unit: string(get(o, "unit"))?.into(),
        symbol_presence,
        reading,
    })
}

impl Message {
    pub fn lane(&self) -> Lane {
        match self {
            Self::Stop { .. } | Self::StopAck { .. } | Self::RoleFinished { .. } => Lane::Control,
            _ => Lane::Ordinary,
        }
    }
    pub(super) fn kind(&self) -> &'static str {
        match self {
            Self::Bootstrap { .. } => "bootstrap",
            Self::Ready { .. } => "ready",
            Self::Query(_) => "query",
            Self::MetricRecord { .. } => "metric_record",
            Self::Stop { .. } => "stop",
            Self::StopAck { .. } => "stop_ack",
            Self::RoleFinished { .. } => "role_finished",
        }
    }
    /// Checked escaped-string upper bound before cloning into the codec model.
    /// Every caller-owned byte is counted, including backslash/control expansion.
    pub(super) fn text_bound(&self) -> Result<usize, WireError> {
        let fields: Vec<&String> = match self {
            Self::Bootstrap {
                raw_uuid,
                raw_pci,
                manifest,
                ..
            } => vec![raw_uuid, raw_pci, manifest],
            Self::Ready {
                raw_uuid,
                raw_pci,
                runtime,
                ..
            } => vec![raw_uuid, raw_pci, runtime],
            Self::MetricRecord { raw, .. } => match &raw.reading {
                Reading::Available { .. } => vec![&raw.unit],
                Reading::Unavailable { failure } => vec![&raw.unit, &failure.detail],
            },
            Self::Stop { reason, .. } => vec![reason],
            Self::RoleFinished { cleanup, .. } => vec![cleanup],
            _ => vec![],
        };
        fields.into_iter().try_fold(0usize, |sum, s| {
            let escaped = s.bytes().try_fold(2usize, |n, b| {
                n.checked_add(match b {
                    b'"' | b'\\' => 2,
                    0..=31 => 6,
                    _ => 1,
                })
                .ok_or(WireError::Length)
            })?;
            sum.checked_add(escaped.max(s.capacity()))
                .ok_or(WireError::Length)
        })
    }
    pub(super) fn body(&self, binding: WireBinding) -> Result<Value, WireError> {
        let check_anchor = |a: Anchor| {
            if a.clock_token == binding.monitor.clock_token {
                Ok(())
            } else {
                Err(WireError::Binding)
            }
        };
        Ok(match self {
            Self::Bootstrap {
                origin,
                raw_uuid,
                raw_pci,
                manifest,
            } => {
                check_anchor(*origin)?;
                object([
                    ("origin", anchor_value(*origin)),
                    ("raw_uuid", text(raw_uuid)),
                    ("raw_pci", text(raw_pci)),
                    ("manifest", text(manifest)),
                ])
            }
            Self::Ready {
                raw_uuid,
                raw_pci,
                sampler_digest,
                frequency,
                anchor,
                runtime,
            } => {
                check_anchor(*anchor)?;
                object([
                    ("raw_uuid", text(raw_uuid)),
                    ("raw_pci", text(raw_pci)),
                    ("sampler_digest", text(hex(sampler_digest))),
                    ("frequency", Value::U64(*frequency)),
                    ("anchor", anchor_value(*anchor)),
                    ("runtime", text(runtime)),
                ])
            }
            Self::Query(t) => {
                if t.binding != binding.monitor {
                    return Err(WireError::Binding);
                }
                ticket_value(*t)
            }
            Self::MetricRecord {
                ticket,
                start,
                end,
                raw,
            } => {
                if ticket.binding != binding.monitor {
                    return Err(WireError::Binding);
                }
                check_anchor(*start)?;
                check_anchor(*end)?;
                // Device loss is valid raw transport evidence, but must cause the
                // supervisor to stop. Other contradictions reject the channel.
                match super::convert(*ticket, 0, 0, raw) {
                    Err(super::MappingError::DeviceLost) | Ok(_) => {}
                    Err(_) => return Err(WireError::Body),
                }
                object([
                    ("ticket", ticket_value(*ticket)),
                    ("start", anchor_value(*start)),
                    ("end", anchor_value(*end)),
                    ("raw", field_value(raw)?),
                ])
            }
            Self::Stop {
                reason,
                trigger_ns,
                cooperative_ns,
                exit_ns,
            } => {
                if reason.is_empty() || trigger_ns > cooperative_ns || cooperative_ns > exit_ns {
                    return Err(WireError::Body);
                }
                object([
                    ("reason", text(reason)),
                    ("trigger_ns", Value::U64(*trigger_ns)),
                    ("cooperative_ns", Value::U64(*cooperative_ns)),
                    ("exit_ns", Value::U64(*exit_ns)),
                ])
            }
            Self::StopAck { anchor, pending } => {
                check_anchor(*anchor)?;
                object([
                    ("anchor", anchor_value(*anchor)),
                    ("pending", Value::Bool(*pending)),
                ])
            }
            Self::RoleFinished {
                anchor,
                pending,
                cleanup,
            } => {
                check_anchor(*anchor)?;
                object([
                    ("anchor", anchor_value(*anchor)),
                    ("pending", Value::Bool(*pending)),
                    ("cleanup", text(cleanup)),
                ])
            }
        })
    }
    pub(super) fn parse(
        kind: &str,
        value: &Value,
        binding: WireBinding,
    ) -> Result<Self, WireError> {
        let message = match kind {
            "bootstrap" => {
                let o = exact(value, &["origin", "raw_uuid", "raw_pci", "manifest"])?;
                Self::Bootstrap {
                    origin: anchor(get(o, "origin"), binding)?,
                    raw_uuid: string(get(o, "raw_uuid"))?.into(),
                    raw_pci: string(get(o, "raw_pci"))?.into(),
                    manifest: string(get(o, "manifest"))?.into(),
                }
            }
            "ready" => {
                let o = exact(
                    value,
                    &[
                        "raw_uuid",
                        "raw_pci",
                        "sampler_digest",
                        "frequency",
                        "anchor",
                        "runtime",
                    ],
                )?;
                Self::Ready {
                    raw_uuid: string(get(o, "raw_uuid"))?.into(),
                    raw_pci: string(get(o, "raw_pci"))?.into(),
                    sampler_digest: unhex(get(o, "sampler_digest"))?,
                    frequency: uint(get(o, "frequency"))?,
                    anchor: anchor(get(o, "anchor"), binding)?,
                    runtime: string(get(o, "runtime"))?.into(),
                }
            }
            "query" => Self::Query(ticket(value, binding)?),
            "metric_record" => {
                let o = exact(value, &["ticket", "start", "end", "raw"])?;
                Self::MetricRecord {
                    ticket: ticket(get(o, "ticket"), binding)?,
                    start: anchor(get(o, "start"), binding)?,
                    end: anchor(get(o, "end"), binding)?,
                    raw: field(get(o, "raw"))?,
                }
            }
            "stop" => {
                let o = exact(
                    value,
                    &["reason", "trigger_ns", "cooperative_ns", "exit_ns"],
                )?;
                Self::Stop {
                    reason: string(get(o, "reason"))?.into(),
                    trigger_ns: uint(get(o, "trigger_ns"))?,
                    cooperative_ns: uint(get(o, "cooperative_ns"))?,
                    exit_ns: uint(get(o, "exit_ns"))?,
                }
            }
            "stop_ack" => {
                let o = exact(value, &["anchor", "pending"])?;
                Self::StopAck {
                    anchor: anchor(get(o, "anchor"), binding)?,
                    pending: boolean(get(o, "pending"))?,
                }
            }
            "role_finished" => {
                let o = exact(value, &["anchor", "pending", "cleanup"])?;
                Self::RoleFinished {
                    anchor: anchor(get(o, "anchor"), binding)?,
                    pending: boolean(get(o, "pending"))?,
                    cleanup: string(get(o, "cleanup"))?.into(),
                }
            }
            _ => return Err(WireError::Body),
        };
        message.body(binding)?; // Same semantic checks on input and output.
        Ok(message)
    }
}
