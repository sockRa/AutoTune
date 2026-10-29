use autotune_benchmark_worker::sampler::budget::{Budget, BudgetError};

#[test]
fn concurrent_owners_share_one_limit_and_release_exact_reservations() {
    let budget = Budget::new(100).unwrap();
    let a = budget.reserve(60).unwrap();
    assert!(matches!(
        budget.clone().reserve(41),
        Err(BudgetError::Exhausted)
    ));
    let b = budget.reserve(40).unwrap();
    assert_eq!(budget.used(), 100);
    drop(a);
    assert_eq!(budget.used(), 40);
    let c = budget.reserve(60).unwrap();
    drop(b);
    drop(c);
    assert_eq!(budget.used(), 0);
    assert!(matches!(
        budget.reserve(usize::MAX),
        Err(BudgetError::Exhausted)
    ));
}

#[test]
fn invalid_resource_limits_never_create_an_unbounded_ledger() {
    assert!(Budget::new(0).is_err());
    assert!(Budget::new(33 * 1024 * 1024).is_err());
}

#[test]
fn preallocation_reservation_can_shrink_but_cannot_grow_past_its_charge() {
    let budget = Budget::new(100).unwrap();
    let mut permit = budget.reserve(100).unwrap();
    permit.shrink_to(40).unwrap();
    assert_eq!(budget.used(), 40);
    assert!(permit.shrink_to(41).is_err());
    assert_eq!(budget.used(), 40);
    drop(permit);
    assert_eq!(budget.used(), 0);
}

#[test]
fn reserved_control_partition_outlives_parent_transport_until_last_frame_drops() {
    let budget = Budget::new(100).unwrap();
    let control = budget.partition(60).unwrap();
    let pending = control.reserve(60).unwrap();
    assert!(budget.reserve(41).is_err());
    drop(control);
    assert_eq!(budget.used(), 60);
    drop(pending);
    assert_eq!(budget.used(), 0);
}
#[test]
fn transferring_a_nested_reservation_cannot_charge_an_unrelated_capture() {
    let root = Budget::new(100).unwrap();
    let nested = root.partition(60).unwrap();
    let foreign = Budget::new(100).unwrap();
    let permit = nested.reserve(20).unwrap();
    assert!(permit.is_within(&root));
    assert!(permit.is_within(&nested));
    assert!(!permit.is_within(&foreign));
    assert_eq!(permit.bytes(), 20);
}
