//! Local inherited-channel provenance, never a child-supplied wire assertion.
//! Copies are historical references, not process liveness or native authority.
use crate::evidence::Digest;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Worker,
    Sampler,
    Starting,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    kind: Kind,
    binding: Digest,
    declaration: Digest,
    image: Digest,
    pid: u32,
    creation: u64,
}
impl Origin {
    // Only fixed launch code can mint production provenance. No decoder or
    // public constructor can turn a matching frame/source label into this type.
    #[cfg(any(windows, test))]
    pub(crate) fn new(
        kind: Kind,
        binding: Digest,
        declaration: Digest,
        image: Digest,
        pid: u32,
        creation: u64,
    ) -> Option<Self> {
        (pid != 0 && creation != 0).then_some(Self {
            kind,
            binding,
            declaration,
            image,
            pid,
            creation,
        })
    }
    pub fn kind(self) -> Kind {
        self.kind
    }
    pub fn binding(self) -> Digest {
        self.binding
    }
    pub fn declaration(self) -> Digest {
        self.declaration
    }
    pub fn image(self) -> Digest {
        self.image
    }
    pub fn pid(self) -> u32 {
        self.pid
    }
    pub fn creation(self) -> u64 {
        self.creation
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Missing,
    Mixed,
}
/// Fixed prepaid state. A missing stamp or mixed source cannot be repaired by
/// later authentic packets; the first concrete origin remains available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Chain {
    first: Option<Origin>,
    missing: bool,
    mixed: bool,
    count: u32,
}
impl Chain {
    pub(crate) fn observe(&mut self, origin: Option<Origin>) {
        self.count = self.count.saturating_add(1);
        match origin {
            None => self.missing = true,
            Some(o) => match self.first {
                None => self.first = Some(o),
                Some(first) if first != o => self.mixed = true,
                _ => {}
            },
        }
    }
    pub(crate) fn result(self) -> Result<Origin, Fault> {
        if self.mixed {
            Err(Fault::Mixed)
        } else if self.missing || self.count == 0 {
            Err(Fault::Missing)
        } else {
            self.first.ok_or(Fault::Missing)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::Domain;
    fn origin(pid: u32) -> Origin {
        let d = Digest::of(Domain::RawArtifact, b"fixture");
        Origin::new(Kind::Worker, d, d, d, pid, 123).unwrap()
    }
    #[test]
    fn zero_process_identity_is_never_issued() {
        let d = origin(1).image();
        assert!(Origin::new(Kind::Worker, d, d, d, 0, 123).is_none());
        assert!(Origin::new(Kind::Worker, d, d, d, 1, 0).is_none());
    }
    #[test]
    fn empty_and_unstamped_chains_cannot_be_promoted() {
        let mut c = Chain::default();
        assert_eq!(c.result(), Err(Fault::Missing));
        c.observe(None);
        c.observe(Some(origin(1)));
        assert_eq!(c.result(), Err(Fault::Missing));
    }
    #[test]
    fn mixed_origins_are_sticky_even_when_the_first_source_returns() {
        let mut c = Chain::default();
        c.observe(Some(origin(1)));
        c.observe(Some(origin(2)));
        c.observe(Some(origin(1)));
        assert_eq!(c.result(), Err(Fault::Mixed));
        assert_eq!(c.first, Some(origin(1)));
    }
    #[test]
    fn same_process_with_changed_binding_or_creation_is_a_different_origin() {
        for change in 0..5 {
            let mut other = origin(1);
            let d = Digest::of(Domain::RawArtifact, b"other");
            match change {
                0 => other.creation += 1,
                1 => other.binding = d,
                2 => other.declaration = d,
                3 => other.image = d,
                _ => other.kind = Kind::Starting,
            }
            let mut c = Chain::default();
            c.observe(Some(origin(1)));
            c.observe(Some(other));
            assert_eq!(c.result(), Err(Fault::Mixed));
        }
    }
    #[test]
    fn matching_chain_needs_no_heap_or_success_claim_from_wire() {
        let mut c = Chain::default();
        for _ in 0..129 {
            c.observe(Some(origin(1)));
        }
        assert_eq!(c.result(), Ok(origin(1)));
        c.observe(None);
        assert_eq!(c.result(), Err(Fault::Missing));
    }
}
