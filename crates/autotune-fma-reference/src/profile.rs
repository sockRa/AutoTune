//! Frozen numeric design data and checked coherence, never execution admission.
use crate::input::{Shape, DISPATCH_CAPACITY};

pub const PROFILE_ID: &str = "e3-fma-qualification-v0.1";
const MS: u64 = 1_000_000;
const S: u64 = 1000 * MS;
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    UnsupportedVersion,
    ZeroBudget,
    ArithmeticOverflow,
    DispatchCapacityExceeded,
    PhaseBudgetExceeded,
    SessionBudgetExceeded,
    MemoryBudgetExceeded,
    SampleCapacityExceeded,
    FrozenDesignMismatch,
    EntryBeforeRunOrigin,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

fn add(left: u64, right: u64) -> Result<u64, Error> {
    left.checked_add(right).ok_or(Error::ArithmeticOverflow)
}
fn mul(left: u64, right: u64) -> Result<u64, Error> {
    left.checked_mul(right).ok_or(Error::ArithmeticOverflow)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Reentry,
    Setup,
    Warmup,
    Measured,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunPlan {
    pub shape: Shape,
    pub seed: u64,
    pub warmup_dispatches: u32,
    pub measured_dispatches: u32,
    pub envelope_ns: u64,
    /// Reentry, setup, warm-up, measurement, exit, in that order.
    pub phase_caps_ns: [u64; 5],
}

impl RunPlan {
    /// Pure calculation using the caller's FIRST entry time. Does not observe
    /// a clock, retain entry state, enforce a deadline or establish termination.
    /// Both absolute candidates must fit u64; overflow rejects even when the
    /// other candidate would be representable. Use a validated design plan.
    pub fn phase_deadline_ns(
        self,
        phase: Phase,
        run_origin_ns: u64,
        first_entry_ns: u64,
    ) -> Result<u64, Error> {
        if first_entry_ns < run_origin_ns {
            return Err(Error::EntryBeforeRunOrigin);
        }
        let index = phase as usize;
        let boundary = self.phase_caps_ns[..=index]
            .iter()
            .try_fold(0, |sum, cap| add(sum, *cap))?;
        Ok(add(run_origin_ns, boundary)?.min(add(first_entry_ns, self.phase_caps_ns[index])?))
    }
}

/// Exact named values, not a general-purpose tuning configuration or defaults
/// imported into the existing caller-supplied synthetic policies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub pilot_session_ns: u64,
    pub acceptance_session_ns: u64,
    pub preflight_ns: u64,
    pub finalization_ns: u64,
    pub shutdown_reserve_ns: u64,
    pub temperature_stop_degc: i64,
    pub power_stop_mw: u64,
    pub reentry_temperature_degc: i64,
    pub reentry_utilization_percent: u32,
    pub reentry_observations: u32,
    pub controller_tick_ns: u64,
    pub dispatch_completion_ns: u64,
    pub cycle_ns: u64,
    pub cooperative_stop_ns: u64,
    pub exit_observation_ns: u64,
    pub worker_commit_bytes: u64,
    pub sampler_commit_bytes: u64,
    pub controller_commit_bytes: u64,
    pub processes_per_job: u32,
    pub worker_cpu_ns_per_run: u64,
    pub sampler_cpu_ns_per_session: u64,
    pub controller_cpu_ns_per_session: u64,
    pub application_gpu_bytes: u64,
    pub free_gpu_headroom_bytes: u64,
    pub frame_bytes: u64,
    pub ordinary_queue_frames: u32,
    pub control_queue_frames: u32,
    pub sample_capacity: u64,
    pub event_capacity: u64,
    pub metadata_bytes: u64,
    pub evidence_ram_bytes: u64,
    pub writer_queue_bytes: u64,
    pub failure_snapshot_bytes: u64,
    pub serialized_bytes: u64,
    pub log_bytes: u64,
    pub cadence_ns: u64,
    pub metric_count: u32,
    pub query_bracket_ns: u64,
    pub transport_age_ns: u64,
    pub observation_gap_ns: u64,
    pub clock_bracket_ns: u64,
    pub clock_agreement_ns: u64,
    pub suspend_divergence_ns: u64,
    pub minimum_metric_observations: u32,
    pub minimum_quarter_observations: u32,
    pub drift_window_runs: u32,
    pub relative_mad_basis_points: u32,
    pub drift_basis_points: u32,
    pub individual_excursion_basis_points: u32,
    pub transition_temperature_degc: u32,
    pub transition_clock_basis_points: u32,
    pub minimum_gpu_utilization_percent: u32,
    pub minimum_submit_completion_percent: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub pilot: [RunPlan; 3],
    pub acceptance: RunPlan,
    pub acceptance_runs: u32,
    pub maximum_dispatches_per_run: u32,
    pub limits: Limits,
}

impl Profile {
    /// Explicitly selects owner-approved DESIGN values, not runtime approval.
    /// No Default: callers must consciously select this frozen profile version.
    pub fn approved_design() -> Self {
        fn plan(lanes: u32, steps: u32, seed: u64, acceptance: bool) -> RunPlan {
            RunPlan {
                shape: Shape::new(lanes, steps).expect("fixed valid design shape"),
                seed,
                warmup_dispatches: if acceptance { 1024 } else { 128 },
                measured_dispatches: if acceptance { 2048 } else { 256 },
                envelope_ns: if acceptance { 75 * S } else { 45 * S },
                phase_caps_ns: if acceptance {
                    [15 * S, 10 * S, 15 * S, 30 * S, 3 * S]
                } else {
                    [10 * S, 10 * S, 5 * S, 10 * S, 3 * S]
                },
            }
        }
        Self {
            id: PROFILE_ID.into(),
            pilot: [
                plan(65_536, 4096, 0, false),
                plan(262_144, 16_384, 1, false),
                plan(262_144, 65_536, 2, false),
            ],
            acceptance: plan(262_144, 65_536, 0, true),
            acceptance_runs: 7,
            maximum_dispatches_per_run: DISPATCH_CAPACITY,
            limits: Limits {
                pilot_session_ns: 180 * S,
                acceptance_session_ns: 600 * S,
                preflight_ns: 10 * S,
                finalization_ns: 10 * S,
                shutdown_reserve_ns: 8 * S,
                temperature_stop_degc: 75,
                power_stop_mw: 200_000,
                reentry_temperature_degc: 55,
                reentry_utilization_percent: 10,
                reentry_observations: 4,
                controller_tick_ns: 10 * MS,
                dispatch_completion_ns: 100 * MS,
                cycle_ns: 250 * MS,
                cooperative_stop_ns: S,
                exit_observation_ns: 3 * S,
                worker_commit_bytes: 1024 * MIB,
                sampler_commit_bytes: 256 * MIB,
                controller_commit_bytes: 512 * MIB,
                processes_per_job: 1,
                worker_cpu_ns_per_run: 45 * S,
                sampler_cpu_ns_per_session: 30 * S,
                controller_cpu_ns_per_session: 60 * S,
                application_gpu_bytes: 16 * MIB,
                free_gpu_headroom_bytes: 512 * MIB,
                frame_bytes: 64 * 1024,
                ordinary_queue_frames: 64,
                control_queue_frames: 4,
                sample_capacity: 32_768,
                event_capacity: 65_536,
                metadata_bytes: 4 * MIB,
                evidence_ram_bytes: 32 * MIB,
                writer_queue_bytes: 8 * MIB,
                failure_snapshot_bytes: 4 * MIB,
                serialized_bytes: 128 * MIB,
                log_bytes: 16 * MIB,
                cadence_ns: 250 * MS,
                metric_count: 6,
                query_bracket_ns: 50 * MS,
                transport_age_ns: 100 * MS,
                observation_gap_ns: 750 * MS,
                clock_bracket_ns: MS,
                clock_agreement_ns: 5 * MS,
                suspend_divergence_ns: 5 * MS,
                minimum_metric_observations: 8,
                minimum_quarter_observations: 4,
                drift_window_runs: 3,
                relative_mad_basis_points: 200,
                drift_basis_points: 300,
                individual_excursion_basis_points: 500,
                transition_temperature_degc: 2,
                transition_clock_basis_points: 500,
                minimum_gpu_utilization_percent: 80,
                minimum_submit_completion_percent: 70,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Totals {
    pub pilot_reservation_ns: u64,
    pub acceptance_reservation_ns: u64,
    pub ordinary_acceptance_metric_slots: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedProfile {
    profile: Profile,
    totals: Totals,
}

impl ValidatedProfile {
    /// Checks arithmetic/coherence AND exact frozen values. No permissive
    /// override, authentication, execution admission or hardware enforcement.
    pub fn new(profile: Profile) -> Result<Self, Error> {
        if profile.id != PROFILE_ID {
            return Err(Error::UnsupportedVersion);
        }
        let limits = profile.limits;
        if limits.cadence_ns == 0 || profile.acceptance_runs == 0 {
            return Err(Error::ZeroBudget);
        }
        let mut pilot_total = 0;
        for plan in profile.pilot.iter().chain([&profile.acceptance]) {
            if plan.warmup_dispatches == 0
                || plan.measured_dispatches == 0
                || plan.phase_caps_ns.contains(&0)
            {
                return Err(Error::ZeroBudget);
            }
            let dispatches = add(
                u64::from(plan.warmup_dispatches),
                u64::from(plan.measured_dispatches),
            )?;
            if dispatches > u64::from(profile.maximum_dispatches_per_run)
                || profile.maximum_dispatches_per_run > DISPATCH_CAPACITY
            {
                return Err(Error::DispatchCapacityExceeded);
            }
            let phase_total = plan.phase_caps_ns.iter().try_fold(0, |a, b| add(a, *b))?;
            if phase_total > plan.envelope_ns {
                return Err(Error::PhaseBudgetExceeded);
            }
            if plan.shape.application_buffer_bytes() > limits.application_gpu_bytes {
                return Err(Error::MemoryBudgetExceeded);
            }
        }
        for plan in &profile.pilot {
            pilot_total = add(pilot_total, plan.envelope_ns)?;
        }
        let overhead = add(
            add(limits.preflight_ns, limits.finalization_ns)?,
            limits.shutdown_reserve_ns,
        )?;
        let pilot_reservation_ns = add(pilot_total, overhead)?;
        let acceptance_reservation_ns = add(
            mul(
                profile.acceptance.envelope_ns,
                u64::from(profile.acceptance_runs),
            )?,
            overhead,
        )?;
        if pilot_reservation_ns > limits.pilot_session_ns
            || acceptance_reservation_ns > limits.acceptance_session_ns
        {
            return Err(Error::SessionBudgetExceeded);
        }
        if add(limits.writer_queue_bytes, limits.failure_snapshot_bytes)?
            > limits.evidence_ram_bytes
            || add(limits.metadata_bytes, limits.log_bytes)? > limits.serialized_bytes
        {
            return Err(Error::MemoryBudgetExceeded);
        }
        let slots = limits.acceptance_session_ns / limits.cadence_ns;
        let ordinary_acceptance_metric_slots = mul(slots, u64::from(limits.metric_count))?;
        if ordinary_acceptance_metric_slots > limits.sample_capacity {
            return Err(Error::SampleCapacityExceeded);
        }
        if profile != Profile::approved_design() {
            return Err(Error::FrozenDesignMismatch);
        }
        Ok(Self {
            profile,
            totals: Totals {
                pilot_reservation_ns,
                acceptance_reservation_ns,
                ordinary_acceptance_metric_slots,
            },
        })
    }
    pub fn profile(&self) -> &Profile {
        &self.profile
    }
    pub fn totals(&self) -> Totals {
        self.totals
    }
}
