//! Pure injected policy: fixture eligibility never grants execution consent.
pub mod clock;
pub mod controller;
pub mod schedule;

pub use clock::{Anchor, Binding, ClockError};
pub use controller::{Controller, Report, Session, Stop, StopReason};
pub use schedule::{Metric, Record, Status, Ticket, Unit};

fn limits() -> autotune_fma_reference::profile::Limits {
    static LIMITS: std::sync::OnceLock<autotune_fma_reference::profile::Limits> =
        std::sync::OnceLock::new();
    *LIMITS.get_or_init(|| autotune_fma_reference::profile::Profile::approved_design().limits)
}
