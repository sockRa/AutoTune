//! Immutable sampler-only scope. A digest/reference binds a declaration, not
//! actual human consent or the truth of source/artifact assertions.
use super::{
    wire::{validate_complexity, WireBinding, WireError},
    wire_model::{exact, get, hex, object, string, unhex},
};
use crate::{
    evidence::{decode, Digest, Domain, Value},
    monitor::Binding,
    worker::DiscoveryBinding,
};

#[derive(Clone, Debug)]
pub struct Draft {
    pub session: [u8; 16],
    pub run: [u8; 16],
    pub clock_token: [u8; 16],
    pub raw_uuid: String,
    pub raw_pci: String,
    pub source_commit: String,
    pub source_tree: [u8; 32],
    pub lockfile: [u8; 32],
    pub controller: [u8; 32],
    pub sampler: [u8; 32],
    pub protocol: [u8; 32],
    pub approval_ref: String,
    pub output_root: String,
}
pub struct CaptureManifest {
    draft: Draft,
    discovery: DiscoveryBinding,
    canonical: String,
    digest: Digest,
    profile_digest: Digest,
}
impl CaptureManifest {
    pub fn freeze(draft: Draft) -> Result<Self, WireError> {
        let discovery = DiscoveryBinding::parse(&draft.raw_uuid, &draft.raw_pci)
            .map_err(|_| WireError::Binding)?;
        if draft.session == [0; 16]
            || draft.run == [0; 16]
            || draft.clock_token == [0; 16]
            || [
                draft.source_tree,
                draft.lockfile,
                draft.controller,
                draft.sampler,
                draft.protocol,
            ]
            .contains(&[0; 32])
            || draft.source_commit.len() != 40
            || !draft
                .source_commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || draft.approval_ref.trim().is_empty()
            || draft.approval_ref.len() > 1024
            || !valid_root(&draft.output_root)
        {
            return Err(WireError::Binding);
        }
        let profile_digest = Digest::of(Domain::Profile, profile_bytes().as_bytes());
        let text = |s: &str| Value::Text(s.into());
        let canonical = object([
            ("version", text("1")),
            ("scope", text("readonly_sampler")),
            ("session", text(&hex(&draft.session))),
            ("run", text(&hex(&draft.run))),
            ("clock_token", text(&hex(&draft.clock_token))),
            ("raw_uuid", text(&draft.raw_uuid)),
            ("raw_pci", text(&draft.raw_pci)),
            ("source_commit", text(&draft.source_commit)),
            ("source_tree", text(&hex(&draft.source_tree))),
            ("lockfile", text(&hex(&draft.lockfile))),
            ("controller", text(&hex(&draft.controller))),
            ("sampler", text(&hex(&draft.sampler))),
            ("profile", text(&profile_digest.to_hex())),
            ("protocol", text(&hex(&draft.protocol))),
            ("approval_ref", text(&draft.approval_ref)),
            ("output_root", text(&draft.output_root)),
        ])
        .to_canonical();
        let digest = Digest::of(Domain::Manifest, canonical.as_bytes());
        Ok(Self {
            draft,
            discovery,
            canonical,
            digest,
            profile_digest,
        })
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, WireError> {
        if bytes.len() > 16384 {
            return Err(WireError::Length);
        }
        validate_complexity(bytes)?;
        let value = decode(bytes, 16384).map_err(|_| WireError::Canonical)?;
        let o = exact(
            &value,
            &[
                "version",
                "scope",
                "session",
                "run",
                "clock_token",
                "raw_uuid",
                "raw_pci",
                "source_commit",
                "source_tree",
                "lockfile",
                "controller",
                "sampler",
                "profile",
                "protocol",
                "approval_ref",
                "output_root",
            ],
        )?;
        if string(get(o, "version"))? != "1" || string(get(o, "scope"))? != "readonly_sampler" {
            return Err(WireError::Binding);
        }
        let result = Self::freeze(Draft {
            session: unhex(get(o, "session"))?,
            run: unhex(get(o, "run"))?,
            clock_token: unhex(get(o, "clock_token"))?,
            raw_uuid: string(get(o, "raw_uuid"))?.into(),
            raw_pci: string(get(o, "raw_pci"))?.into(),
            source_commit: string(get(o, "source_commit"))?.into(),
            source_tree: unhex(get(o, "source_tree"))?,
            lockfile: unhex(get(o, "lockfile"))?,
            controller: unhex(get(o, "controller"))?,
            sampler: unhex(get(o, "sampler"))?,
            protocol: unhex(get(o, "protocol"))?,
            approval_ref: string(get(o, "approval_ref"))?.into(),
            output_root: string(get(o, "output_root"))?.into(),
        })?;
        if result.canonical.as_bytes() != bytes {
            return Err(WireError::Binding);
        }
        Ok(result)
    }
    pub fn canonical(&self) -> &str {
        &self.canonical
    }
    pub fn digest(&self) -> Digest {
        self.digest
    }
    pub fn profile_digest(&self) -> Digest {
        self.profile_digest
    }
    pub fn draft(&self) -> &Draft {
        &self.draft
    }
    pub fn selection(&self) -> super::supervisor::Selection {
        super::supervisor::Selection {
            raw_uuid: self.draft.raw_uuid.clone(),
            raw_pci: self.draft.raw_pci.clone(),
            sampler_digest: self.draft.sampler,
        }
    }
    pub fn binding(&self, frequency: u64) -> Result<WireBinding, WireError> {
        if frequency == 0 {
            return Err(WireError::Binding);
        }
        Ok(WireBinding {
            monitor: Binding {
                session: self.draft.session,
                run: self.draft.run,
                device: self.discovery.identity,
                clock_token: self.draft.clock_token,
                qpc_frequency: frequency,
            },
            manifest_digest: *self.digest.as_bytes(),
        })
    }
}
fn valid_root(root: &str) -> bool {
    let b = root.as_bytes();
    if !(4..=2048).contains(&b.len())
        || !b[0].is_ascii_alphabetic()
        || b[1] != b':'
        || b[2] != b'\\'
    {
        return false;
    }
    root[3..].split('\\').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.ends_with([' ', '.'])
            && !part.bytes().any(|b| b < 32 || b"/:*?\"<>|".contains(&b))
    })
}
/// Exact sampler-relevant profile plus every frozen resource/acceptance limit.
/// It identifies approved design policy, never measured qualification or consent.
pub fn profile_bytes() -> String {
    let profile = autotune_fma_reference::profile::Profile::approved_design();
    let p = profile.limits;
    macro_rules! limits {($($field:ident),*$(,)?)=>{object([$( (stringify!($field),Value::U64(p.$field as u64)) ),*])};}
    object([
        ("id", Value::Text(profile.id)),
        ("scope", Value::Text("readonly_sampler".into())),
        (
            "limits",
            limits![
                pilot_session_ns,
                acceptance_session_ns,
                preflight_ns,
                finalization_ns,
                shutdown_reserve_ns,
                temperature_stop_degc,
                power_stop_mw,
                reentry_temperature_degc,
                reentry_utilization_percent,
                reentry_observations,
                controller_tick_ns,
                dispatch_completion_ns,
                cycle_ns,
                cooperative_stop_ns,
                exit_observation_ns,
                worker_commit_bytes,
                sampler_commit_bytes,
                controller_commit_bytes,
                processes_per_job,
                worker_cpu_ns_per_run,
                sampler_cpu_ns_per_session,
                controller_cpu_ns_per_session,
                application_gpu_bytes,
                free_gpu_headroom_bytes,
                frame_bytes,
                ordinary_queue_frames,
                control_queue_frames,
                sample_capacity,
                event_capacity,
                metadata_bytes,
                evidence_ram_bytes,
                writer_queue_bytes,
                failure_snapshot_bytes,
                serialized_bytes,
                log_bytes,
                cadence_ns,
                metric_count,
                query_bracket_ns,
                transport_age_ns,
                observation_gap_ns,
                clock_bracket_ns,
                clock_agreement_ns,
                suspend_divergence_ns,
                minimum_metric_observations,
                minimum_quarter_observations,
                drift_window_runs,
                relative_mad_basis_points,
                drift_basis_points,
                individual_excursion_basis_points,
                transition_temperature_degc,
                transition_clock_basis_points,
                minimum_gpu_utilization_percent,
                minimum_submit_completion_percent
            ],
        ),
    ])
    .to_canonical()
}
