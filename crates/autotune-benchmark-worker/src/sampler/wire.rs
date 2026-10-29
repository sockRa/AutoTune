//! Bounded canonical frames. One channel per direction/lane, no replay/recovery.
//! Encoding reserves a sequence; uncertain delivery requires ending the session.
pub use super::wire_model::Message;
use super::wire_model::{exact, get, hex, object, string, uint, unhex};
use crate::{
    evidence::{decode, Value},
    monitor::Binding,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WireBinding {
    pub monitor: Binding,
    pub manifest_digest: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Ordinary,
    Control,
}
impl Lane {
    fn name(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Control => "control",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WireError {
    Binding,
    Lane,
    Length,
    Canonical,
    Body,
    Sequence,
    Poisoned,
    Capacity,
}

pub struct Channel {
    binding: WireBinding,
    lane: Lane,
    next: u64,
    poisoned: bool,
}
fn frame_limit() -> usize {
    autotune_fma_reference::profile::Profile::approved_design()
        .limits
        .frame_bytes as usize
}
impl Channel {
    /// Transport/codec failures can reject a packet before decoding. Those
    /// failures close this lane just as a rejected canonical message does.
    pub(super) fn poison(&mut self) {
        self.poisoned = true;
    }
    pub fn new(binding: WireBinding, lane: Lane) -> Result<Self, WireError> {
        let m = binding.monitor;
        if m.session == [0; 16]
            || m.run == [0; 16]
            || m.clock_token == [0; 16]
            || m.qpc_frequency == 0
            || !m.device.valid()
            || binding.manifest_digest == [0; 32]
        {
            return Err(WireError::Binding);
        }
        Ok(Self {
            binding,
            lane,
            next: 0,
            poisoned: false,
        })
    }
    fn finish<T>(&mut self, result: Result<T, WireError>) -> Result<T, WireError> {
        match result {
            Ok(value) => {
                match self.next.checked_add(1) {
                    Some(next) => self.next = next,
                    None => {
                        self.poisoned = true;
                        return Err(WireError::Sequence);
                    }
                }
                Ok(value)
            }
            Err(error) => {
                self.poisoned = true;
                Err(error)
            }
        }
    }
    pub fn encode(&mut self, message: &Message) -> Result<Vec<u8>, WireError> {
        if self.poisoned {
            return Err(WireError::Poisoned);
        }
        let result = self.encode_inner(message);
        self.finish(result)
    }
    fn encode_inner(&self, message: &Message) -> Result<Vec<u8>, WireError> {
        if message.lane() != self.lane {
            return Err(WireError::Lane);
        }
        // Fixed structural overhead is bounded above; variable strings checked
        // before cloning. This deliberately rejects near-limit payloads rather
        // than allocating first and discovering oversize after serialization.
        let bound = message
            .text_bound()?
            .checked_add(4096)
            .ok_or(WireError::Length)?;
        if bound > frame_limit() {
            return Err(WireError::Length);
        }
        let b = self.binding.monitor;
        let body = message.body(self.binding)?;
        let value = object([
            ("version", Value::Text("1".into())),
            ("session", Value::Text(hex(&b.session))),
            ("run", Value::Text(hex(&b.run))),
            ("role", Value::Text("sampler".into())),
            ("lane", Value::Text(self.lane.name().into())),
            ("sequence", Value::U64(self.next)),
            (
                "manifest_digest",
                Value::Text(hex(&self.binding.manifest_digest)),
            ),
            ("clock_token", Value::Text(hex(&b.clock_token))),
            ("frequency", Value::U64(b.qpc_frequency)),
            (
                "device",
                object([
                    ("uuid", Value::Text(hex(&b.device.uuid))),
                    ("domain", Value::U64(b.device.domain.into())),
                    ("bus", Value::U64(b.device.bus.into())),
                    ("device", Value::U64(b.device.device.into())),
                ]),
            ),
            ("kind", Value::Text(message.kind().into())),
            ("body", body),
        ]);
        let encoded = value.to_canonical();
        let length = encoded.len().checked_add(4).ok_or(WireError::Length)?;
        if length > frame_limit() {
            return Err(WireError::Length);
        }
        let mut frame = Vec::with_capacity(length);
        frame.extend_from_slice(&(length as u32).to_le_bytes());
        frame.extend_from_slice(encoded.as_bytes());
        Ok(frame)
    }
    pub fn decode(&mut self, frame: &[u8]) -> Result<Message, WireError> {
        if self.poisoned {
            return Err(WireError::Poisoned);
        }
        let result = self.decode_inner(frame);
        self.finish(result)
    }
    fn decode_inner(&self, frame: &[u8]) -> Result<Message, WireError> {
        let length = Self::checked_length(
            frame
                .get(..4)
                .ok_or(WireError::Length)?
                .try_into()
                .map_err(|_| WireError::Length)?,
        )?;
        if length != frame.len() {
            return Err(WireError::Length);
        }
        // Reject container/key explosions before the generic codec constructs
        // its owned tree. Quoted braces/commas do not count as structure.
        validate_complexity(&frame[4..])?;
        let value = decode(&frame[4..], frame_limit() - 4).map_err(|_| WireError::Canonical)?;
        let o = exact(
            &value,
            &[
                "version",
                "session",
                "run",
                "role",
                "lane",
                "sequence",
                "manifest_digest",
                "clock_token",
                "frequency",
                "device",
                "kind",
                "body",
            ],
        )?;
        let b = self.binding.monitor;
        if string(get(o, "version"))? != "1" || string(get(o, "role"))? != "sampler" {
            return Err(WireError::Binding);
        }
        if string(get(o, "lane"))? != self.lane.name() {
            return Err(WireError::Lane);
        }
        let device = exact(get(o, "device"), &["uuid", "domain", "bus", "device"])?;
        if unhex::<16>(get(o, "session"))? != b.session
            || unhex::<16>(get(o, "run"))? != b.run
            || unhex::<32>(get(o, "manifest_digest"))? != self.binding.manifest_digest
            || unhex::<16>(get(o, "clock_token"))? != b.clock_token
            || uint(get(o, "frequency"))? != b.qpc_frequency
            || unhex::<16>(get(device, "uuid"))? != b.device.uuid
            || uint(get(device, "domain"))? != u64::from(b.device.domain)
            || uint(get(device, "bus"))? != u64::from(b.device.bus)
            || uint(get(device, "device"))? != u64::from(b.device.device)
        {
            return Err(WireError::Binding);
        }
        if uint(get(o, "sequence"))? != self.next {
            return Err(WireError::Sequence);
        }
        let message = Message::parse(string(get(o, "kind"))?, get(o, "body"), self.binding)?;
        if message.lane() != self.lane {
            return Err(WireError::Lane);
        }
        Ok(message)
    }
    /// Call on the four-byte prefix before allocating/reading the body.
    pub fn checked_length(prefix: [u8; 4]) -> Result<usize, WireError> {
        let length = u32::from_le_bytes(prefix) as usize;
        if !(4..=frame_limit()).contains(&length) {
            return Err(WireError::Length);
        }
        Ok(length)
    }
}

pub(super) fn validate_complexity(bytes: &[u8]) -> Result<(), WireError> {
    let (mut quoted, mut escaped, mut structures) = (false, false, 0u16);
    for b in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *b == b'\\' {
                escaped = true;
            } else if *b == b'"' {
                quoted = false;
            }
        } else if *b == b'"' {
            quoted = true;
        } else if matches!(*b, b'{' | b'[' | b',' | b':') {
            structures = structures.checked_add(1).ok_or(WireError::Capacity)?;
            if structures > 256 {
                return Err(WireError::Capacity);
            }
        }
    }
    Ok(())
}
