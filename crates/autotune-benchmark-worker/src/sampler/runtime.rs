//! Independent raw sleep-including shutdown accounting and resource policy.
//! Inputs are observations supplied by the OS adapter or explicit CPU fixtures.
fn limits() -> autotune_fma_reference::profile::Limits {
    autotune_fma_reference::profile::Profile::approved_design().limits
}
/// Capture receipt clocks AFTER the observed OS calls. An anchor taken before
/// wait/accounting could falsely credit an exit returned past its deadline.
pub trait HostSource {
    fn resources(&mut self) -> ResourceObservation;
    fn process(&mut self) -> Option<super::supervisor::ProcessObservation>;
    fn sampler_resources(&mut self) -> Option<ResourceObservation> {
        None
    }
    fn watchdog(&mut self) -> u64;
    fn anchor(&mut self) -> Option<crate::monitor::Anchor>;
}
pub struct HostCapture {
    pub resources: ResourceObservation,
    pub process: Option<super::supervisor::ProcessObservation>,
    pub sampler_resources: Option<ResourceObservation>,
    pub watchdog_100ns: u64,
    pub anchor: Option<crate::monitor::Anchor>,
}
pub fn observe_host(source: &mut impl HostSource) -> HostCapture {
    let resources = source.resources();
    let process = source.process();
    let sampler_resources = source.sampler_resources();
    let watchdog_100ns = source.watchdog();
    let anchor = source.anchor();
    HostCapture {
        resources,
        process,
        sampler_resources,
        watchdog_100ns,
        anchor,
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Launch,
    TransportReceive,
    BootstrapSend,
    QuerySend,
    StopSend,
    StopEvent,
    JobTerminate,
    TransportCancel,
}
/// Cancellation is a request; a finished worker is observed separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IoObservation {
    pub lane: super::wire::Lane,
    pub reader: bool,
    pub finished: bool,
    pub cancellation: Result<(), u32>,
}
impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::TransportReceive => "transport_receive",
            Self::BootstrapSend => "bootstrap_send",
            Self::QuerySend => "query_send",
            Self::StopSend => "stop_send",
            Self::StopEvent => "stop_event",
            Self::JobTerminate => "job_terminate",
            Self::TransportCancel => "transport_cancel",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchdogError {
    Regression,
    Overflow,
    Untrusted,
}
pub struct Watchdog {
    origin: u64,
    last: u64,
    now: u64,
    trusted: bool,
    stop: Option<u64>,
}
impl Watchdog {
    pub fn new(origin_100ns: u64) -> Result<Self, WatchdogError> {
        Ok(Self {
            origin: origin_100ns,
            last: origin_100ns,
            now: 0,
            trusted: true,
            stop: None,
        })
    }
    pub fn observe(&mut self, raw_100ns: u64) -> Result<u64, WatchdogError> {
        if !self.trusted {
            return Err(WatchdogError::Untrusted);
        }
        let result = if raw_100ns < self.last {
            Err(WatchdogError::Regression)
        } else {
            raw_100ns
                .checked_sub(self.origin)
                .and_then(|n| n.checked_mul(100))
                .ok_or(WatchdogError::Overflow)
        };
        match result {
            Ok(n) => {
                self.last = raw_100ns;
                self.now = n;
                Ok(n)
            }
            Err(e) => {
                self.trusted = false;
                Err(e)
            }
        }
    }
    pub fn stop(&mut self, first_ns: u64) {
        self.stop.get_or_insert(first_ns.min(self.now));
    }
    pub fn capture_due(&self) -> bool {
        !self.trusted || self.now >= limits().preflight_ns
    }
    pub fn force_due(&self) -> bool {
        !self.trusted
            || self
                .stop
                .is_some_and(|n| self.now >= n.saturating_add(limits().cooperative_stop_ns))
    }
    pub fn observation_expired(&self) -> bool {
        !self.trusted
            || self
                .stop
                .is_some_and(|n| self.now >= n.saturating_add(limits().exit_observation_ns))
    }
    pub fn finalization_expired(&self) -> bool {
        let p = limits();
        let end = p.preflight_ns.saturating_add(p.shutdown_reserve_ns);
        !self.trusted
            || self.now
                >= self
                    .stop
                    .map_or(end, |n| n.saturating_add(p.shutdown_reserve_ns).min(end))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceObservation {
    pub commit_bytes: Option<u64>,
    pub user_100ns: Option<u64>,
    pub kernel_100ns: Option<u64>,
    pub memory_error: Option<u32>,
    pub time_error: Option<u32>,
}
impl ResourceObservation {
    pub fn within_limits(self) -> bool {
        let p = limits();
        self.within(p.controller_commit_bytes, p.controller_cpu_ns_per_session)
    }
    pub fn within_sampler_limits(self) -> bool {
        let p = limits();
        self.within(p.sampler_commit_bytes, p.sampler_cpu_ns_per_session)
    }
    fn within(self, commit: u64, cpu: u64) -> bool {
        if self.memory_error.is_some() || self.time_error.is_some() {
            return false;
        }
        self.commit_bytes.is_some_and(|n| n < commit)
            && self
                .user_100ns
                .zip(self.kernel_100ns)
                .and_then(|(u, k)| u.checked_add(k))
                .and_then(|n| n.checked_mul(100))
                .is_some_and(|n| n < cpu)
    }
}

/// An exact operator invocation, not an approval inference from JSON. The
/// operating contract still requires separately retained human consent.
pub struct ExecutionRequest {
    pub expected_manifest: crate::evidence::Digest,
    pub manifest_path: std::path::PathBuf,
}
impl ExecutionRequest {
    pub fn parse(args: &[String]) -> Result<Self, super::wire::WireError> {
        if args.len() != 4
            || args[1] != "--execute-readonly-manifest"
            || args[3].is_empty()
            || args[3].len() > 32768
            || args[3].contains('\0')
        {
            return Err(super::wire::WireError::Binding);
        }
        Ok(Self {
            expected_manifest: crate::evidence::Digest::parse(&args[2])
                .map_err(|_| super::wire::WireError::Binding)?,
            manifest_path: args[3].clone().into(),
        })
    }
    pub fn bind(
        &self,
        manifest: &super::manifest::CaptureManifest,
        controller_sha256: [u8; 32],
    ) -> Result<(), super::wire::WireError> {
        if self.expected_manifest != manifest.digest()
            || manifest.draft().controller != controller_sha256
        {
            return Err(super::wire::WireError::Binding);
        }
        Ok(())
    }
}
