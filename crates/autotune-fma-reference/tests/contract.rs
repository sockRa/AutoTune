use autotune_fma_reference::{
    input::{block_digest, DispatchIndex, Error as InputError, Lane, Recipe, Shape, Steps},
    profile::{Error as ProfileError, Phase, Profile, ValidatedProfile, PROFILE_ID},
    reference::{check, expected_integer, FailureKind},
};

const S: u64 = 1_000_000_000;
const MIB: u64 = 1_048_576;

fn j(value: u32) -> DispatchIndex {
    DispatchIndex::new(value).unwrap()
}
fn recipe(lanes: u32) -> Recipe {
    Recipe::new(0, Shape::new(lanes, 4096).unwrap())
}
fn expected_outputs(recipe: Recipe, dispatch: DispatchIndex) -> Vec<f32> {
    recipe
        .lanes()
        .map(|lane| expected_integer(lane, recipe.shape().steps(), dispatch) as f32)
        .collect()
}
fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn approved_profile_matches_independently_transcribed_design() {
    let validated = ValidatedProfile::new(Profile::approved_design()).unwrap();
    let p = validated.profile();
    assert_eq!(p.id, PROFILE_ID);
    assert_eq!(p.acceptance_runs, 7);
    assert_eq!(p.maximum_dispatches_per_run, 4096);
    for (i, (lanes, steps, seed)) in [
        (65_536, 4096, 0),
        (262_144, 16_384, 1),
        (262_144, 65_536, 2),
    ]
    .into_iter()
    .enumerate()
    {
        let plan = p.pilot[i];
        assert_eq!(
            (plan.shape.lanes(), plan.shape.steps().value(), plan.seed),
            (lanes, steps, seed)
        );
        assert_eq!(
            (plan.warmup_dispatches, plan.measured_dispatches),
            (128, 256)
        );
        assert_eq!(plan.envelope_ns, 45 * S);
        assert_eq!(plan.phase_caps_ns, [10 * S, 10 * S, 5 * S, 10 * S, 3 * S]);
    }
    let plan = p.acceptance;
    assert_eq!(
        (plan.shape.lanes(), plan.shape.steps().value(), plan.seed),
        (262_144, 65_536, 0)
    );
    assert_eq!(
        (plan.warmup_dispatches, plan.measured_dispatches),
        (1024, 2048)
    );
    assert_eq!(plan.envelope_ns, 75 * S);
    assert_eq!(plan.phase_caps_ns, [15 * S, 10 * S, 15 * S, 30 * S, 3 * S]);
    assert_eq!(validated.totals().pilot_reservation_ns, 163 * S);
    assert_eq!(validated.totals().acceptance_reservation_ns, 553 * S);
    assert_eq!(validated.totals().ordinary_acceptance_metric_slots, 14_400);
    macro_rules! limits {
        ($($field:ident = $expected:expr),+ $(,)?) => {$(assert_eq!(p.limits.$field, $expected, stringify!($field));)+};
    }
    limits!(
        pilot_session_ns = 180 * S,
        acceptance_session_ns = 600 * S,
        preflight_ns = 10 * S,
        finalization_ns = 10 * S,
        shutdown_reserve_ns = 8 * S,
        temperature_stop_degc = 75,
        power_stop_mw = 200_000,
        reentry_temperature_degc = 55,
        reentry_utilization_percent = 10,
        reentry_observations = 4,
        controller_tick_ns = 10_000_000,
        dispatch_completion_ns = 100_000_000,
        cycle_ns = 250_000_000,
        cooperative_stop_ns = S,
        exit_observation_ns = 3 * S,
        worker_commit_bytes = 1024 * MIB,
        sampler_commit_bytes = 256 * MIB,
        controller_commit_bytes = 512 * MIB,
        processes_per_job = 1,
        worker_cpu_ns_per_run = 45 * S,
        sampler_cpu_ns_per_session = 30 * S,
        controller_cpu_ns_per_session = 60 * S,
        application_gpu_bytes = 16 * MIB,
        free_gpu_headroom_bytes = 512 * MIB,
        frame_bytes = 65_536,
        ordinary_queue_frames = 64,
        control_queue_frames = 4,
        sample_capacity = 32_768,
        event_capacity = 65_536,
        metadata_bytes = 4 * MIB,
        evidence_ram_bytes = 32 * MIB,
        writer_queue_bytes = 8 * MIB,
        failure_snapshot_bytes = 4 * MIB,
        serialized_bytes = 128 * MIB,
        log_bytes = 16 * MIB,
        cadence_ns = 250_000_000,
        metric_count = 6,
        query_bracket_ns = 50_000_000,
        transport_age_ns = 100_000_000,
        observation_gap_ns = 750_000_000,
        clock_bracket_ns = 1_000_000,
        clock_agreement_ns = 5_000_000,
        suspend_divergence_ns = 5_000_000,
        minimum_metric_observations = 8,
        minimum_quarter_observations = 4,
        drift_window_runs = 3,
        relative_mad_basis_points = 200,
        drift_basis_points = 300,
        individual_excursion_basis_points = 500,
        transition_temperature_degc = 2,
        transition_clock_basis_points = 500,
        minimum_gpu_utilization_percent = 80,
        minimum_submit_completion_percent = 70,
    );
}

#[test]
fn profile_version_and_every_category_are_frozen() {
    let mut p = Profile::approved_design();
    p.id.push_str("-other");
    assert_eq!(
        ValidatedProfile::new(p),
        Err(ProfileError::UnsupportedVersion)
    );
    let mutations: &[fn(&mut Profile)] = &[
        |p| p.pilot[0].seed = 1,
        |p| p.pilot.swap(0, 1),
        |p| p.acceptance.shape = Shape::new(1, 4096).unwrap(),
        |p| p.acceptance.warmup_dispatches -= 1,
        |p| p.acceptance_runs -= 1,
        |p| p.limits.temperature_stop_degc += 1,
        |p| p.limits.power_stop_mw += 1,
        |p| p.limits.worker_commit_bytes += 1,
        |p| p.limits.free_gpu_headroom_bytes += 1,
        |p| p.limits.processes_per_job = 2,
        |p| p.limits.frame_bytes += 1,
        |p| p.limits.event_capacity += 1,
        |p| p.limits.cadence_ns += 1,
        |p| p.limits.clock_agreement_ns += 1,
        |p| p.limits.relative_mad_basis_points += 1,
        |p| p.limits.minimum_gpu_utilization_percent -= 1,
    ];
    for mutate in mutations {
        let mut p = Profile::approved_design();
        mutate(&mut p);
        assert!(ValidatedProfile::new(p).is_err());
    }
}

#[test]
fn profile_zero_and_overflow_are_typed_errors() {
    let mut p = Profile::approved_design();
    p.limits.cadence_ns = 0;
    assert_eq!(ValidatedProfile::new(p), Err(ProfileError::ZeroBudget));
    let mut p = Profile::approved_design();
    p.acceptance.phase_caps_ns = [u64::MAX; 5];
    assert_eq!(
        ValidatedProfile::new(p),
        Err(ProfileError::ArithmeticOverflow)
    );
    let mut p = Profile::approved_design();
    p.limits.preflight_ns = u64::MAX;
    assert_eq!(
        ValidatedProfile::new(p),
        Err(ProfileError::ArithmeticOverflow)
    );
}

#[test]
fn profile_budget_failures_do_not_rewrite_the_input() {
    type BudgetCase = (fn(&mut Profile), ProfileError);
    let cases: &[BudgetCase] = &[
        (
            |p| p.acceptance.measured_dispatches = u32::MAX,
            ProfileError::DispatchCapacityExceeded,
        ),
        (
            |p| p.acceptance.envelope_ns = S,
            ProfileError::PhaseBudgetExceeded,
        ),
        (
            |p| p.limits.pilot_session_ns = S,
            ProfileError::SessionBudgetExceeded,
        ),
        (
            |p| p.limits.application_gpu_bytes = 1,
            ProfileError::MemoryBudgetExceeded,
        ),
        (
            |p| p.limits.evidence_ram_bytes = 1,
            ProfileError::MemoryBudgetExceeded,
        ),
        (
            |p| p.limits.sample_capacity = 1,
            ProfileError::SampleCapacityExceeded,
        ),
    ];
    for (mutate, reason) in cases {
        let mut p = Profile::approved_design();
        mutate(&mut p);
        let source = p.clone();
        assert_eq!(ValidatedProfile::new(p.clone()), Err(*reason));
        assert_eq!(p, source);
    }
}

#[test]
fn phase_deadlines_cap_early_late_and_overflowing_entries() {
    let p = Profile::approved_design();
    assert_eq!(
        p.acceptance
            .phase_deadline_ns(Phase::Setup, 100 * S, 105 * S),
        Ok(115 * S)
    );
    assert_eq!(
        p.acceptance
            .phase_deadline_ns(Phase::Setup, 100 * S, 120 * S),
        Ok(125 * S)
    );
    assert_eq!(
        p.acceptance
            .phase_deadline_ns(Phase::Setup, 100 * S, 130 * S),
        Ok(125 * S)
    );
    assert_eq!(
        p.acceptance.phase_deadline_ns(Phase::Setup, S, 0),
        Err(ProfileError::EntryBeforeRunOrigin)
    );
    assert_eq!(
        p.acceptance
            .phase_deadline_ns(Phase::Exit, u64::MAX, u64::MAX),
        Err(ProfileError::ArithmeticOverflow)
    );
    for (phase, expected) in [
        (Phase::Reentry, 15),
        (Phase::Setup, 25),
        (Phase::Warmup, 40),
        (Phase::Measured, 70),
        (Phase::Exit, 73),
    ] {
        assert_eq!(
            p.acceptance.phase_deadline_ns(phase, 0, expected * S),
            Ok(expected * S)
        );
    }
}

#[test]
fn counter_digest_matches_documented_and_independent_dotnet_vectors() {
    for (seed, block, expected) in [
        (
            0,
            0,
            "ea8e7f1ac9ed65c9c188474d5326a44e576076c94c3358d881a04faf3b3c9b19",
        ),
        (
            0,
            1,
            "6544d7c51c8201ae30c451de5ccb9b07ca92349c84259e3886ec3139db0a5a75",
        ),
        (
            1,
            1,
            "9121105770fc2fdeb2a80453fc44895c1a2e17b60d1d6d12707796014571a8eb",
        ),
        (
            u64::MAX,
            u64::MAX,
            "e318c363a3c68377a79dc877eead3d24b96baeff6e2da0f548d3a501a5554c76",
        ),
    ] {
        assert_eq!(hex(block_digest(seed, block)), expected);
    }
}

#[test]
fn documented_lanes_and_oracle_values_are_exact() {
    let r = recipe(3);
    for (i, a, b, base, small, large) in [
        (0, -3, -7, -114, -380_928, 909_313),
        (1, -2, -5, -19, -36_864, 577_537),
        (2, 2, -5, -120, -532_480, -1_146_879),
    ] {
        let lane = r.lane(i).unwrap();
        assert_eq!((lane.a(), lane.b(), lane.base()), (a, b, base));
        assert_eq!(
            expected_integer(lane, Steps::new(4096).unwrap(), j(0)),
            small
        );
        assert_eq!(
            expected_integer(lane, Steps::new(65_536).unwrap(), j(1)),
            large
        );
        assert_eq!(
            lane.inputs(j(1)),
            [
                f32::from(a),
                f32::from(b),
                (4096 * i32::from(base) + 1) as f32
            ]
        );
    }
}

#[test]
fn indexed_and_streamed_lanes_agree_across_blocks_and_seed_extremes() {
    for seed in [0, 1, u64::MAX] {
        let r = Recipe::new(seed, Shape::new(33, 4096).unwrap());
        let mut iterator = r.lanes();
        for index in 0..33 {
            assert_eq!(iterator.len(), (33 - index) as usize);
            assert_eq!(iterator.next(), Some(r.lane(index).unwrap()));
        }
        assert_eq!(iterator.next(), None);
        assert_eq!(r.lane(33), Err(InputError::LaneOutOfRange));
        assert_eq!(r.lane(u32::MAX), Err(InputError::LaneOutOfRange));
    }
    // seed0/block1 starts with little-endian word c5d74465, not 6544d7c5.
    let lane8 = recipe(9).lane(8).unwrap();
    assert_eq!((lane8.a(), lane8.b(), lane8.base()), (6, 7, 68));
}

#[test]
fn every_decoder_operand_and_base_is_bounded_and_high_bits_unused() {
    for word in 0..=u16::MAX {
        let lane = Lane::from_word(u32::from(word));
        assert!((1..=8).contains(&lane.a().abs()));
        assert!((1..=8).contains(&lane.b().abs()));
        assert_eq!(lane, Lane::from_word(u32::from(word) | 0xffff_0000));
    }
    assert_eq!(Lane::from_word(0x8000).base(), -128);
    assert_eq!(Lane::from_word(0x7f00).base(), 127);
}

#[test]
fn invalid_shape_steps_and_tags_fail_before_generation_or_allocation() {
    for lanes in [0, 262_145, u32::MAX] {
        assert_eq!(Shape::new(lanes, 4096), Err(InputError::InvalidLanes));
    }
    for steps in [0, 1, 4095, 4097, 65_537, u32::MAX] {
        assert_eq!(Steps::new(steps), Err(InputError::InvalidSteps));
        assert_eq!(Shape::new(1, steps), Err(InputError::InvalidSteps));
    }
    for dispatch in [4096, u32::MAX] {
        assert_eq!(
            DispatchIndex::new(dispatch),
            Err(InputError::InvalidDispatch)
        );
    }
    assert_eq!(
        Shape::new(262_144, 65_536)
            .unwrap()
            .application_buffer_bytes(),
        4 * MIB
    );
    assert_eq!(
        Shape::new(262_144, 65_536)
            .unwrap()
            .fma_steps_per_dispatch(),
        17_179_869_184
    );
}

#[test]
fn reference_extremes_are_fp32_exact_and_encode_the_dispatch() {
    for word in [
        0x0000, 0x00ff, 0x8000, 0x80ff, 0x7f00, 0x7fff, 0x0088, 0x8008,
    ] {
        let lane = Lane::from_word(word);
        for steps in [4096, 16_384, 65_536] {
            for dispatch in [0, 1, 3071, 4095] {
                let expected = expected_integer(lane, Steps::new(steps).unwrap(), j(dispatch));
                assert!(expected.abs() < (1 << 24));
                assert_eq!((expected as f32) as i32, expected);
                assert_eq!(expected.rem_euclid(4096), dispatch as i32);
                let independent = i64::from(lane.inputs(j(dispatch))[2] as i32)
                    + i64::from(steps) * i64::from(lane.a()) * i64::from(lane.b());
                assert_eq!(i64::from(expected), independent);
            }
        }
    }
}

#[test]
fn reference_checker_is_immutable_and_checks_the_entire_tail() {
    let r = recipe(17);
    let mut output = expected_outputs(r, j(3071));
    output[0] += 1.0;
    output[8] = f32::from_bits(0x7fc0_0123);
    output[16] = f32::NEG_INFINITY;
    let original_bits: Vec<_> = output.iter().map(|v| v.to_bits()).collect();
    let report = check(r, j(3071), &output).unwrap();
    assert_eq!(report.recipe(), r);
    assert_eq!(report.dispatch(), j(3071));
    assert_eq!(report.checked_lanes(), 17);
    assert_eq!(report.mismatches(), 3);
    assert_eq!(report.nonfinite(), 2);
    assert!(!report.matches_reference());
    let first = report.first_failure().unwrap();
    assert_eq!(first.lane_index, 0);
    assert_eq!(first.kind, FailureKind::Mismatch);
    assert_eq!(first.actual_bits, output[0].to_bits());
    assert_eq!(
        first.expected_integer,
        expected_integer(r.lane(0).unwrap(), r.shape().steps(), j(3071))
    );
    assert_eq!(
        output.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        original_bits
    );
    assert_eq!(check(r, j(3071), &output).unwrap(), report);
}

#[test]
fn correct_output_and_all_lengths_have_distinct_outcomes() {
    let r = recipe(8);
    let mut output = expected_outputs(r, j(0));
    let report = check(r, j(0), &output).unwrap();
    assert!(report.matches_reference());
    assert_eq!(
        (
            report.checked_lanes(),
            report.mismatches(),
            report.nonfinite()
        ),
        (8, 0, 0)
    );
    assert_eq!(report.first_failure(), None);
    for length in [0, 1, 7] {
        let error = check(r, j(0), &output[..length]).unwrap_err();
        assert_eq!((error.expected, error.actual), (8, length));
    }
    output.push(0.0);
    assert_eq!(check(r, j(0), &output).unwrap_err().actual, 9);
}

#[test]
fn nonfinite_sentinel_nan_and_infinities_preserve_failure_bits() {
    let r = recipe(1);
    for bits in [0x7fc0_0123, 0x7f80_0000, 0xff80_0000, 0x7f80_0001] {
        let report = check(r, j(0), &[f32::from_bits(bits)]).unwrap();
        assert_eq!((report.mismatches(), report.nonfinite()), (1, 1));
        let failure = report.first_failure().unwrap();
        assert_eq!(failure.actual_bits, bits);
        assert_eq!(failure.kind, FailureKind::NonFinite);
    }
}

#[test]
fn signed_zero_agrees_only_with_an_exact_zero_reference() {
    // Find a generated lane with base == -a*b, hence zero expected for K4096,j0.
    let r = recipe(4096);
    let index = r
        .lanes()
        .position(|lane| expected_integer(lane, r.shape().steps(), j(0)) == 0)
        .unwrap();
    let mut output = expected_outputs(r, j(0));
    output[index] = -0.0;
    assert!(check(r, j(0), &output).unwrap().matches_reference());
    output[index] = 0.0;
    assert!(check(r, j(0), &output).unwrap().matches_reference());
    output[0] = -0.0;
    assert!(!check(r, j(0), &output).unwrap().matches_reference());
}

#[test]
fn stale_dispatch_rejects_every_lane_and_matching_cross_run_data_is_not_authentication() {
    let r = recipe(33);
    let output = expected_outputs(r, j(0));
    for dispatch in [1, 3071, 4095] {
        let report = check(r, j(dispatch), &output).unwrap();
        assert_eq!(report.mismatches(), 33);
    }
    // A fresh identical recipe legitimately produces identical CPU-reference data.
    let other_run_recipe = Recipe::new(r.seed(), r.shape());
    assert!(check(other_run_recipe, j(0), &output)
        .unwrap()
        .matches_reference());
}

#[test]
fn all_decoded_lanes_have_exact_maximum_step_results() {
    let steps = Steps::new(65_536).unwrap();
    for word in 0..=u16::MAX {
        let lane = Lane::from_word(u32::from(word));
        for dispatch in [0, 4095] {
            let expected = expected_integer(lane, steps, j(dispatch));
            let independent = 4096_i64 * i64::from(lane.base())
                + 65_536_i64 * i64::from(lane.a()) * i64::from(lane.b())
                + i64::from(dispatch);
            assert_eq!(i64::from(expected), independent);
            assert_eq!((expected as f32) as i64, independent);
            assert_eq!(expected.rem_euclid(4096), dispatch as i32);
        }
    }
}

#[test]
fn maximum_shape_iterator_is_bounded_and_agrees_at_last_lane() {
    let shape = Shape::new(262_144, 65_536).unwrap();
    let r = Recipe::new(u64::MAX, shape);
    let mut iterator = r.lanes();
    assert_eq!(iterator.len(), 262_144);
    assert_eq!(iterator.next(), r.lane(0).ok());
    assert_eq!(iterator.last(), r.lane(262_143).ok());
    assert_eq!(r.lane(262_144), Err(InputError::LaneOutOfRange));
}

#[test]
fn skipped_iterator_elements_still_use_their_actual_block() {
    let r = recipe(33);
    for index in [1, 7, 8, 9, 17, 32] {
        assert_eq!(r.lanes().nth(index), r.lane(index as u32).ok());
    }
    let mut iterator = r.lanes();
    assert_eq!(iterator.next(), r.lane(0).ok());
    assert_eq!(iterator.nth(8), r.lane(9).ok());
    for (i, lane) in r.lanes().step_by(9).enumerate() {
        assert_eq!(Some(lane), r.lane((i * 9) as u32).ok());
    }
}

#[test]
fn pilot_phase_boundaries_and_early_entry_caps_are_independently_pinned() {
    let p = Profile::approved_design();
    for plan in p.pilot {
        for (phase, boundary, cap) in [
            (Phase::Reentry, 10, 10),
            (Phase::Setup, 20, 10),
            (Phase::Warmup, 25, 5),
            (Phase::Measured, 35, 10),
            (Phase::Exit, 38, 3),
        ] {
            assert_eq!(
                plan.phase_deadline_ns(phase, 100 * S, 100 * S),
                Ok((100 + cap) * S)
            );
            assert_eq!(
                plan.phase_deadline_ns(phase, 100 * S, (100 + boundary) * S),
                Ok((100 + boundary) * S)
            );
            assert_eq!(
                plan.phase_deadline_ns(phase, 100 * S, (101 + boundary) * S),
                Ok((100 + boundary) * S)
            );
        }
    }
}

#[test]
fn deadline_requires_both_absolute_candidates_to_be_representable() {
    let plan = Profile::approved_design().acceptance;
    let origin = u64::MAX - 20 * S;
    // Entry+10s fits, origin+25s does not: deliberately reject unsupported range.
    assert_eq!(
        plan.phase_deadline_ns(Phase::Setup, origin, origin),
        Err(ProfileError::ArithmeticOverflow)
    );
}

#[test]
fn every_permitted_step_multiple_preserves_integer_result_and_tag() {
    for multiple in 1..=16 {
        let k = multiple * 4096;
        let shape = Shape::new(8, k).unwrap();
        assert_eq!(shape.steps().value(), k);
        let r = Recipe::new(0, shape);
        for tag in [0, 1, 4095] {
            let output: Vec<_> = r
                .lanes()
                .map(|lane| {
                    let n = 4096_i64 * i64::from(lane.base())
                        + i64::from(k) * i64::from(lane.a()) * i64::from(lane.b())
                        + i64::from(tag);
                    assert_eq!(n.rem_euclid(4096), i64::from(tag));
                    assert_eq!((n as f32) as i64, n);
                    n as f32
                })
                .collect();
            assert!(check(r, j(tag), &output).unwrap().matches_reference());
        }
    }
}

#[test]
fn exact_check_rejects_adjacent_floats_fractional_values_and_subnormals() {
    let r = recipe(4096);
    let original = expected_outputs(r, j(0));
    let zero = original.iter().position(|v| *v == 0.0).unwrap();
    let adjacent = f32::from_bits(original[0].to_bits() + 1);
    for actual in [adjacent, original[0] + 0.5] {
        let mut output = original.clone();
        output[0] = actual;
        let report = check(r, j(0), &output).unwrap();
        assert_eq!(report.mismatches(), 1);
        assert_eq!(report.nonfinite(), 0);
        assert_eq!(
            report.first_failure().unwrap().actual_bits,
            actual.to_bits()
        );
    }
    for bits in [1, 0x8000_0001] {
        let mut output = original.clone();
        output[zero] = f32::from_bits(bits);
        let report = check(r, j(0), &output).unwrap();
        assert_eq!(report.mismatches(), 1);
        assert_eq!(report.nonfinite(), 0);
        assert_eq!(report.first_failure().unwrap().lane_index, zero as u32);
    }
}
