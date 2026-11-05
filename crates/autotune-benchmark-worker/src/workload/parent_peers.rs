//! Local launch/channel provenance composition. Not liveness, user consent,
//! fresh safety, mapped-image attestation or native admission.
#[cfg(any(windows, test))]
use super::launch_origin::Kind;
use super::{
    launch_origin::{self, Origin},
    parent_context::References,
};
use crate::evidence::Digest;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Context,
    Missing,
    Mixed,
    Binding,
    Cancelled,
    Aliased,
    Duplicate,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub expected: Origin,
    pub received: Result<Origin, launch_origin::Fault>,
    pub cancelled: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub context: Option<References>,
    pub starting_response: Option<Digest>,
    /// Starting, sampler, worker, each with all original local facts retained.
    pub peers: [Peer; 3],
    pub checks: [Result<(), Fault>; 3],
    pub result: Result<(), Fault>,
}
#[cfg(any(windows, test))]
pub(crate) fn assess(
    context: Option<References>,
    binding: Digest,
    images: [Digest; 2],
    starting_response: Option<Digest>,
    peers: [Peer; 3],
) -> Record {
    let kinds = [Kind::Starting, Kind::Sampler, Kind::Worker];
    let checks = std::array::from_fn(|i| {
        let p = peers[i];
        if p.cancelled {
            return Err(Fault::Cancelled);
        }
        let received = p.received.map_err(|f| match f {
            launch_origin::Fault::Missing => Fault::Missing,
            launch_origin::Fault::Mixed => Fault::Mixed,
        })?;
        let c = context.ok_or(Fault::Context)?;
        if received != p.expected
            || p.expected.kind() != kinds[i]
            || p.expected.binding() != if i == 0 { c.scope } else { binding }
            || p.expected.image() != images[usize::from(i == 2)]
        {
            return Err(Fault::Binding);
        }
        if i == 0 && (starting_response.is_none() || starting_response != c.starting_response) {
            return Err(Fault::Binding);
        }
        Ok(())
    });
    let result = if context.is_none() {
        Err(Fault::Context)
    } else if peers.iter().enumerate().any(|(i, p)| {
        peers[..i]
            .iter()
            .any(|o| o.expected.pid() == p.expected.pid())
    }) {
        Err(Fault::Aliased)
    } else {
        checks
            .iter()
            .copied()
            .find(Result::is_err)
            .unwrap_or(Ok(()))
    };
    Record {
        context,
        starting_response,
        peers,
        checks,
        result,
    }
}

#[cfg(windows)]
pub struct Processes<'a> {
    pub starting: &'a super::windows_process::Process<super::starting_bootstrap::Declaration>,
    pub sampler: &'a super::windows_process::Process,
    pub worker: &'a super::windows_process::Process,
    pub starting_session: &'a super::starting_session::Session,
    pub setup: &'a mut super::worker_setup_delivery::ReadyDelivery,
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::Domain;
    fn fixture() -> (References, [Peer; 3]) {
        let d = Digest::of(Domain::RawArtifact, b"model");
        let refs = References {
            manifest: d,
            scope: d,
            host: Some(d),
            starting: Some(d),
            starting_response: Some(d),
            worker_setup: d,
            current: Some(d),
            sampler_ready: Some(d),
            current_response: Some(d),
        };
        let peers = std::array::from_fn(|i| {
            let expected = Origin::new(
                [Kind::Starting, Kind::Sampler, Kind::Worker][i],
                d,
                d,
                d,
                i as u32 + 1,
                100,
            )
            .unwrap();
            Peer {
                expected,
                received: Ok(expected),
                cancelled: false,
            }
        });
        (refs, peers)
    }
    fn screen(r: References, p: [Peer; 3]) -> Record {
        assess(Some(r), r.manifest, [r.manifest; 2], r.starting_response, p)
    }
    #[test]
    fn exact_local_origins_preserve_context_and_all_three_results() {
        let (r, p) = fixture();
        let record = screen(r, p);
        assert_eq!(record.result, Ok(()));
        assert_eq!(record.checks, [Ok(()); 3]);
        assert_eq!(record.peers, p);
    }
    #[test]
    fn missing_mixed_cancelled_and_changed_source_are_distinct() {
        for i in 0..3 {
            for cause in 0..4 {
                let (r, mut p) = fixture();
                let expected = match cause {
                    0 => {
                        p[i].received = Err(launch_origin::Fault::Missing);
                        Fault::Missing
                    }
                    1 => {
                        p[i].received = Err(launch_origin::Fault::Mixed);
                        Fault::Mixed
                    }
                    2 => {
                        p[i].cancelled = true;
                        Fault::Cancelled
                    }
                    _ => {
                        p[i].received = Ok(p[(i + 1) % 3].expected);
                        Fault::Binding
                    }
                };
                let result = screen(r, p);
                assert_eq!(result.checks[i], Err(expected));
                assert_eq!(result.peers, p);
            }
        }
    }
    #[test]
    fn byte_equality_never_substitutes_for_context_or_delivered_starting_response() {
        let (r, p) = fixture();
        assert_eq!(
            assess(None, r.manifest, [r.manifest; 2], r.starting_response, p).result,
            Err(Fault::Context)
        );
        assert_eq!(
            assess(Some(r), r.manifest, [r.manifest; 2], None, p).result,
            Err(Fault::Binding)
        );
        let other = Digest::of(Domain::RawArtifact, b"other");
        assert_eq!(
            assess(Some(r), other, [r.manifest; 2], r.starting_response, p).result,
            Err(Fault::Binding)
        );
        assert_eq!(
            assess(Some(r), r.manifest, [other; 2], r.starting_response, p).result,
            Err(Fault::Binding)
        );
    }
    #[test]
    fn one_process_cannot_fill_two_roles() {
        let (r, mut p) = fixture();
        p[2].expected = p[1].expected;
        p[2].received = Ok(p[2].expected);
        assert_eq!(screen(r, p).result, Err(Fault::Aliased));
    }
}
