//! Measurement of the proposed 100 ms publication ceiling on this host.
//! Diagnostic evidence only; it does not relax the ceiling or claim durability.
#![cfg(windows)]
#[path = "../../autotune-recovery-core/tests/support/journal.rs"]
mod support;
use autotune_recovery_contract::{process::FakeBehavior, Operation};
use autotune_recovery_windows::storage::*;
use std::time::{Duration, Instant};

#[test]
fn record_publication_latencies_against_the_proposed_ceiling() {
    use std::{cell::RefCell, rc::Rc};
    let root = TestRoot::provision_fixture().unwrap();
    let store = Rc::new(RefCell::new(Store::open(&root).unwrap()));
    let initial = store.borrow().scan().active.clone();
    let times = Rc::new(RefCell::new(Vec::new()));
    let writer = Rc::clone(&store);
    let observed = Rc::clone(&times);
    let mut driver = support::Driver::start(247_123, FakeBehavior::Normal, initial, move |next| {
        let started = Instant::now();
        let sequence = next.sequence();
        writer.borrow_mut().publish(next).unwrap();
        observed.borrow_mut().push((sequence, started.elapsed()));
    });
    driver.capture_seal_arm();
    driver.call(Operation::CandidateOnce, 501);
    driver.call(Operation::Current, 502);
    driver.recover();
    driver.close();
    let mut worst = Duration::ZERO;
    for (sequence, elapsed) in times.borrow().iter() {
        println!("publication {sequence}: {elapsed:?}");
        worst = worst.max(*elapsed);
    }
    println!("worst publication: {worst:?}; proposed ceiling: 100ms");
    assert!(worst > Duration::ZERO);
}
