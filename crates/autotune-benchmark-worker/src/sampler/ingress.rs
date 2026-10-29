//! Shared owned decode/retention handoff for pipe packets and CPU fixtures.
use super::{
    budget::Reservation,
    framing::{Packet, Phase, ReadFault, Rejection},
    supervisor::{Effect, Input, Supervisor},
    wire::{Channel, Lane, WireError},
};
use crate::monitor::Anchor;
pub struct Received {
    input: Input,
    charge: Reservation,
}
impl Received {
    pub fn decode(
        lane: Lane,
        packet: Packet,
        decoder: &mut Channel,
        codec: Result<Reservation, ()>,
    ) -> Self {
        if let Some(rejection) = packet.rejection() {
            decoder.poison();
            let (bytes, charge, _) = packet.into_parts();
            return Self {
                input: Input::Rejected {
                    lane,
                    bytes,
                    rejection,
                },
                charge,
            };
        }
        let parsed = match &codec {
            Ok(_) => decoder.decode(packet.bytes()),
            Err(_) => {
                decoder.poison();
                Err(WireError::Capacity)
            }
        };
        match parsed {
            Ok(message) => Self {
                input: Input::Message(message),
                charge: codec.expect("decode had a reservation"),
            },
            Err(error) => {
                let (bytes, charge, _) = packet.into_parts();
                let reported = bytes.len() as u64;
                Self {
                    input: Input::Rejected {
                        lane,
                        bytes,
                        rejection: Rejection {
                            fault: ReadFault::Wire(error),
                            phase: Phase::Decode,
                            reported,
                            queue_overflow: false,
                        },
                    },
                    charge,
                }
            }
        }
    }
    pub fn apply(self, supervisor: &mut Supervisor, receipt: Option<Anchor>) -> Vec<Effect> {
        supervisor.advance_reserved(receipt, self.input, self.charge)
    }
}
