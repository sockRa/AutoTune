//! Harness-owned fake device. A completed oracle effect is deliberately NOT a
//! worker completion: late effects remain visible after that worker disappears.
use autotune_recovery_contract::{process::*, *};

pub struct Witness {
    profile: FakeProfile,
    current_mw: u32,
    calls: Vec<WitnessCall>,
    pending: bool,
    manifest: Option<Manifest>,
}
impl Witness {
    pub fn new(profile: FakeProfile) -> Result<Self> {
        profile.validate()?;
        Ok(Self {
            current_mw: profile.baseline_mw,
            profile,
            calls: vec![],
            pending: false,
            manifest: None,
        })
    }
    pub fn calls(&self) -> &[WitnessCall] {
        &self.calls
    }
    pub fn current_mw(&self) -> u32 {
        self.current_mw
    }
    pub fn seal(&mut self, manifest: Manifest) -> Result<()> {
        manifest.validate()?;
        if self.pending
            || self.manifest.is_some()
            || manifest.preparation.device != self.profile.device
            || manifest.runtime != self.profile.runtime
            || manifest.capture.readings_mw != [self.profile.baseline_mw; 2]
            || manifest.capture.minimum_mw != self.profile.minimum_mw
            || manifest.capture.maximum_mw != self.profile.maximum_mw
        {
            return Err(Rejection::Binding);
        }
        self.manifest = Some(manifest);
        Ok(())
    }
    /// Record entry while retaining the unfinished effect, exposing overlap
    /// instead of letting a worker kill silently erase it.
    pub fn enter(&mut self, proof: &DispatchProof, invocation: FakeInvocation) -> Result<()> {
        let p = &invocation.permit;
        let expected_write = match p.operation {
            Operation::CandidateOnce => {
                Some(self.manifest.as_ref().ok_or(Rejection::Binding)?.target_mw)
            }
            Operation::RestoreBaseline => Some(
                self.manifest
                    .as_ref()
                    .ok_or(Rejection::Binding)?
                    .capture
                    .baseline()?,
            ),
            _ => None,
        };
        if self.pending
            || self.calls.len() >= 64
            || p != &proof.permit
            || invocation.device != self.profile.device
            || invocation.entry_ms < p.dispatched_ms
            || self.calls.last().is_some_and(|c| {
                c.invocation.permit.session != p.session
                    || c.invocation.permit.operation_id >= p.operation_id
            })
            || invocation.write_mw != expected_write
            || self.manifest.as_ref().is_some_and(|m| {
                m.digest().ok() != Some(p.digest) || m.preparation.session != p.session
            })
        {
            return Err(Rejection::Binding);
        }
        self.calls.push(WitnessCall {
            invocation,
            effect_ms: None,
            result: None,
            worker_lost_before_effect: false,
        });
        self.pending = true;
        Ok(())
    }
    /// Release one synthetic effect at a harness barrier. `worker_lost` marks
    /// oracle-only evidence and can never retire the guardian's pending permit.
    pub fn release(&mut self, now: u64, worker_lost: bool) -> Result<RawResult> {
        if !self.pending {
            return Err(Rejection::Binding);
        }
        let call = self.calls.last_mut().ok_or(Rejection::Binding)?;
        if now < call.invocation.entry_ms {
            return Err(Rejection::Binding);
        }
        let result = match call.invocation.permit.operation {
            Operation::Enumerate | Operation::Identity => {
                RawResult::Identity(self.profile.device.clone())
            }
            Operation::Runtime => RawResult::Runtime(self.profile.runtime.clone()),
            Operation::Bounds => RawResult::Bounds {
                minimum_mw: self.profile.minimum_mw,
                maximum_mw: self.profile.maximum_mw,
            },
            Operation::Current => RawResult::Value(self.current_mw),
            Operation::CandidateOnce | Operation::RestoreBaseline => {
                self.current_mw = call.invocation.write_mw.ok_or(Rejection::Binding)?;
                RawResult::Api {
                    status: if call.invocation.permit.operation == Operation::CandidateOnce
                        && self.profile.behavior == FakeBehavior::ErrorAfterChange
                    {
                        -7
                    } else {
                        0
                    },
                }
            }
        };
        call.effect_ms = Some(now);
        call.result = Some(result.clone());
        call.worker_lost_before_effect = worker_lost;
        self.pending = false;
        Ok(result)
    }
}
