use autotune_benchmark_worker::{
    evidence::{Digest, Domain},
    sampler::manifest::profile_bytes,
};
#[test]
fn readonly_profile_bytes_are_pinned_before_workload_profile_reuse() {
    assert_eq!(
        Digest::of(Domain::Profile, profile_bytes().as_bytes()).to_hex(),
        "d9ef3a0a5f1a138740a3ec657aac4d3b80515a910394dd8cf80aab2ead4bc6d9"
    );
}
