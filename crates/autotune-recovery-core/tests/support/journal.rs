#![allow(dead_code)]
#[path = "../../../autotune-recovery-contract/tests/support/mod.rs"]
pub mod fixtures;
use autotune_recovery_contract::{process::*, storage::*, *};
use autotune_recovery_core::{journal::*, worker::*};
use std::collections::VecDeque;

/// Shared test driver only. All authority/state transitions are production core
/// and disk-contract code. Clock and fake worker calls are injected P0 evidence.
pub struct Driver<P: FnMut(JournalEnvelope)> {
    pub actor: JournaledGuardian,
    pub worker: WorkerPolicy,
    pub manifest: Option<Manifest>,
    pub publications: usize,
    pub last_dispatch: Option<DispatchProof>,
    pub baseline: u32,
    pub now: u64,
    publish: P,
}
impl<P: FnMut(JournalEnvelope)> Driver<P> {
    pub fn start(
        baseline: u32,
        behavior: FakeBehavior,
        anchor: JournalEnvelope,
        publish: P,
    ) -> Self {
        let prep = fixtures::preparation();
        let profile = FakeProfile {
            device: prep.device.clone(),
            runtime: fixtures::runtime(),
            baseline_mw: baseline,
            minimum_mw: 100_000,
            maximum_mw: 300_000,
            default_mw: Some(215_000),
            behavior,
        };
        let worker = WorkerPolicy::new(
            profile,
            prep.session,
            prep.boot,
            [8; 32],
            prep.operator.clone(),
        )
        .unwrap();
        let actor = JournaledGuardian::new(prep.clone(), anchor, 0, 0).unwrap();
        let mut driver = Self {
            actor,
            worker,
            manifest: None,
            publications: 0,
            last_dispatch: None,
            baseline,
            now: 0,
            publish,
        };
        let effects = driver
            .actor
            .start(prep.clone(), true, true, true, true)
            .unwrap();
        driver.effects(effects);
        driver
            .worker
            .prepare(prep, driver.actor.published().checksum)
            .unwrap();
        driver
    }
    pub fn effects(&mut self, effects: Vec<JournalEffect>) {
        let mut queue: VecDeque<_> = effects.into();
        while let Some(effect) = queue.pop_front() {
            match effect {
                JournalEffect::Publish(envelope) => {
                    assert!(
                        !self.actor.publication_failed(),
                        "no publisher after failure"
                    );
                    let checksum = envelope.checksum;
                    (self.publish)(*envelope);
                    self.publications += 1;
                    queue.extend(
                        self.actor
                            .acknowledge(checksum, self.now, self.now)
                            .unwrap(),
                    );
                }
                JournalEffect::Dispatch(proof) => {
                    assert!(
                        self.last_dispatch.is_none(),
                        "no overlapping device effects"
                    );
                    self.last_dispatch = Some(*proof);
                }
                JournalEffect::Alert(_) => (),
            }
        }
    }
    pub fn dispatch(&mut self, op: Operation, now: u64) -> (Permission, Entry) {
        self.now = now;
        let effects = self.actor.reserve(op, now, now).unwrap();
        self.effects(effects);
        let proof = self
            .last_dispatch
            .take()
            .expect("intent acknowledgement precedes dispatch");
        let permit = proof.permit.clone();
        let entry = self
            .worker
            .admit(proof, now, now, true)
            .unwrap()
            .unwrap_or_else(|o| panic!("{o:?}"));
        self.worker.revalidate_entry(&entry, now, true).unwrap();
        (permit, entry)
    }
    pub fn finish(&mut self, permit: Permission, entry: Entry, result: RawResult) {
        let outcome = self
            .worker
            .returned(entry, self.now, self.now, result)
            .unwrap();
        let effects = self
            .actor
            .completed(&permit, outcome, self.now, self.now)
            .unwrap();
        self.effects(effects);
        let envelope = self.actor.working();
        let JournalPayload::Snapshot(snapshot) = &envelope.payload else {
            panic!()
        };
        let completion = snapshot.sessions[0]
            .calls
            .last()
            .unwrap()
            .completion
            .as_ref()
            .unwrap();
        self.worker
            .recorded(&ResultReceipt {
                operation_id: permit.operation_id,
                sequence: completion.sequence,
                received_utc_ms: completion.received_utc_ms,
                publication: (!self.actor.publication_failed()).then_some(envelope.checksum),
            })
            .unwrap();
    }
    pub fn call(&mut self, op: Operation, now: u64) {
        let (permit, entry) = self.dispatch(op, now);
        let result = self.worker.fake_call(&entry).unwrap();
        self.finish(permit, entry, result);
    }
    pub fn chain(&mut self, now: u64) {
        for op in [
            Operation::Enumerate,
            Operation::Identity,
            Operation::Runtime,
            Operation::Bounds,
            Operation::Current,
        ] {
            self.call(op, now);
        }
    }
    pub fn capture_seal_only(&mut self) {
        self.chain(0);
        self.call(Operation::Current, 500);
        let mut manifest = fixtures::manifest(self.baseline);
        manifest.capture.trace = self.actor.capture_trace().unwrap();
        let worker_ack = self.worker.seal(manifest.clone()).unwrap();
        let effects = self.actor.seal(manifest.clone(), worker_ack).unwrap();
        self.effects(effects);
        self.manifest = Some(manifest);
    }
    pub fn capture_seal_arm(&mut self) {
        self.capture_seal_only();
        let manifest = self.manifest.clone().unwrap();
        let approval = fixtures::approval(&manifest);
        let effects = self.actor.approve(approval.clone(), 500, 500).unwrap();
        self.effects(effects);
        let JournalPayload::Snapshot(snapshot) = &self.actor.published().payload else {
            panic!()
        };
        let commit = snapshot.sessions[0].arm_commit.clone().unwrap();
        self.worker
            .arm(
                &ArmCapsule {
                    approval,
                    commit,
                    nonce_publication: self.actor.published().checksum,
                },
                500,
                500,
                true,
            )
            .unwrap();
        self.chain(500);
    }
    pub fn revoke_worker(&mut self) {
        let window = self.actor.guardian().recovery_window().unwrap();
        self.worker
            .revoke(self.actor.guardian().generation(), window)
            .unwrap();
    }
    pub fn recover(&mut self) {
        self.revoke_worker();
        self.chain(503);
        self.call(Operation::RestoreBaseline, 504);
        self.call(Operation::Current, 505);
        self.call(Operation::Current, 755);
    }
    pub fn close(&mut self) {
        let effects = self.actor.close().unwrap();
        self.effects(effects);
    }
}
pub fn anchor() -> JournalEnvelope {
    JournalEnvelope::new(JournalPayload::Anchor {
        version: STORAGE_VERSION,
        store: [17; 32],
    })
    .unwrap()
}
