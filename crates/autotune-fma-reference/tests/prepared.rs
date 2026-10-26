use autotune_fma_reference::{input::*, reference::*};

#[test]
fn prepared_parity_for_all_steps_tags_and_failures() {
    for k in (4096..=65536).step_by(4096) {
        for seed in [0, 1, 2, u64::MAX] {
            let recipe = Recipe::new(seed, Shape::new(257, k).unwrap());
            let prepared = PreparedReference::new(recipe);
            for tag in [0, 1, 4095] {
                let dispatch = DispatchIndex::new(tag).unwrap();
                let mut output: Vec<_> = recipe
                    .lanes()
                    .map(|lane| expected_integer(lane, recipe.shape().steps(), dispatch) as f32)
                    .collect();
                let mut x = vec![0.0; output.len()];
                prepared.fill_x(dispatch, &mut x).unwrap();
                for (i, lane) in recipe.lanes().enumerate() {
                    assert_eq!(
                        [prepared.a()[i], prepared.b()[i], x[i]],
                        lane.inputs(dispatch)
                    );
                }
                assert_eq!(
                    prepared.check(dispatch, &output),
                    check(recipe, dispatch, &output)
                );
                for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25] {
                    output[256] = bad;
                    let bits: Vec<_> = output.iter().map(|v| v.to_bits()).collect();
                    assert_eq!(
                        prepared.check(dispatch, &output),
                        check(recipe, dispatch, &output)
                    );
                    assert_eq!(bits, output.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
                }
                assert_eq!(
                    prepared.check(dispatch, &output[..256]),
                    check(recipe, dispatch, &output[..256])
                );
                assert!(prepared.fill_x(dispatch, &mut x[..256]).is_err());
                let stale = DispatchIndex::new((tag + 1) % 4096).unwrap();
                let output: Vec<_> = recipe
                    .lanes()
                    .map(|lane| expected_integer(lane, recipe.shape().steps(), stale) as f32)
                    .collect();
                assert_eq!(prepared.check(dispatch, &output).unwrap().mismatches(), 257);
            }
        }
    }
}

#[test]
fn prepared_full_approved_pilot_and_acceptance_shapes() {
    for (seed, lanes, k) in [
        (0, 65536, 4096),
        (1, 262144, 16384),
        (2, 262144, 65536),
        (0, 262144, 65536),
    ] {
        let recipe = Recipe::new(seed, Shape::new(lanes, k).unwrap());
        let prepared = PreparedReference::new(recipe);
        for tag in [0, 4095] {
            let dispatch = DispatchIndex::new(tag).unwrap();
            let mut output: Vec<_> = recipe
                .lanes()
                .map(|lane| expected_integer(lane, recipe.shape().steps(), dispatch) as f32)
                .collect();
            assert_eq!(
                prepared.check(dispatch, &output),
                check(recipe, dispatch, &output)
            );
            output[lanes as usize - 1] = f32::NAN;
            let report = prepared.check(dispatch, &output).unwrap();
            assert_eq!(report.checked_lanes(), lanes);
            assert_eq!(report.first_failure().unwrap().lane_index, lanes - 1);
        }
    }
}
