use crate::worker::Identity;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub session: [u8; 16],
    pub run: [u8; 16],
    pub device: Identity,
    pub clock_token: [u8; 16],
    pub qpc_frequency: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub clock_token: [u8; 16],
    pub qpc_before: u64,
    pub qpc_after: u64,
    pub interrupt_100ns: u64,
    pub unbiased_100ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    Binding,
    Epoch,
    Order,
    Regression,
    Overflow,
    Bracket,
    Disagreement,
    Suspend,
}

/// Floor elapsed time and ceil uncertainty with checked u128 intermediates.
pub fn ticks_to_ns(ticks: u64, frequency: u64, ceil: bool) -> Result<u64, ClockError> {
    if frequency == 0 {
        return Err(ClockError::Binding);
    }
    let numerator = u128::from(ticks)
        .checked_mul(1_000_000_000)
        .ok_or(ClockError::Overflow)?;
    let divisor = u128::from(frequency);
    let ns = if ceil {
        numerator.div_ceil(divisor)
    } else {
        numerator / divisor
    };
    u64::try_from(ns).map_err(|_| ClockError::Overflow)
}

pub(super) struct Clock {
    binding: Binding,
    origin: Anchor,
    last: Anchor,
    rejected: Option<ClockError>,
}

impl Clock {
    pub fn new(binding: Binding, anchor: Anchor) -> Result<Self, ClockError> {
        if binding.session == [0; 16]
            || binding.run == [0; 16]
            || !binding.device.valid()
            || binding.clock_token == [0; 16]
            || binding.qpc_frequency == 0
        {
            return Err(ClockError::Binding);
        }
        let clock = Self {
            binding,
            origin: anchor,
            last: anchor,
            rejected: None,
        };
        clock.validate_anchor(anchor)?;
        Ok(clock)
    }

    fn validate_anchor(&self, anchor: Anchor) -> Result<(), ClockError> {
        if anchor.clock_token != self.binding.clock_token {
            return Err(ClockError::Epoch);
        }
        let ticks = anchor
            .qpc_after
            .checked_sub(anchor.qpc_before)
            .ok_or(ClockError::Order)?;
        if ticks_to_ns(ticks, self.binding.qpc_frequency, true)? > super::limits().clock_bracket_ns
        {
            return Err(ClockError::Bracket);
        }
        Ok(())
    }

    fn elapsed(&self, earlier: Anchor, later: Anchor) -> Result<u64, ClockError> {
        if later.qpc_before < earlier.qpc_before || later.qpc_after < earlier.qpc_after {
            return Err(ClockError::Regression);
        }
        let interrupt = later
            .interrupt_100ns
            .checked_sub(earlier.interrupt_100ns)
            .ok_or(ClockError::Regression)?
            .checked_mul(100)
            .ok_or(ClockError::Overflow)?;
        let unbiased = later
            .unbiased_100ns
            .checked_sub(earlier.unbiased_100ns)
            .ok_or(ClockError::Regression)?
            .checked_mul(100)
            .ok_or(ClockError::Overflow)?;
        let lower = ticks_to_ns(
            later.qpc_before.saturating_sub(earlier.qpc_after),
            self.binding.qpc_frequency,
            false,
        )?;
        let upper = ticks_to_ns(
            later
                .qpc_after
                .checked_sub(earlier.qpc_before)
                .ok_or(ClockError::Regression)?,
            self.binding.qpc_frequency,
            true,
        )?;
        let limits = super::limits();
        if interrupt.abs_diff(unbiased) > limits.suspend_divergence_ns {
            return Err(ClockError::Suspend);
        }
        if interrupt < lower.saturating_sub(limits.clock_agreement_ns)
            || interrupt
                > upper
                    .checked_add(limits.clock_agreement_ns)
                    .ok_or(ClockError::Overflow)?
        {
            return Err(ClockError::Disagreement);
        }
        Ok(interrupt)
    }

    pub fn observe(&mut self, anchor: Anchor) -> Result<u64, ClockError> {
        if let Some(error) = self.rejected {
            return Err(error);
        }
        let result = self
            .validate_anchor(anchor)
            .and_then(|()| self.elapsed(self.last, anchor))
            .and_then(|_| self.elapsed(self.origin, anchor));
        match result {
            Ok(ns) => {
                self.last = anchor;
                Ok(ns)
            }
            Err(error) => {
                self.rejected = Some(error);
                Err(error)
            }
        }
    }
}
