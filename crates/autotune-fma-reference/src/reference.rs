//! Integer closed-form oracle and bounded checking of caller-supplied outputs.
use crate::input::{DispatchIndex, Lane, Recipe, Steps};

/// Validated types bound every intermediate below 2^24 (and far below i32::MAX).
/// This is deliberately not the prospective kernel's repeated FMA algorithm.
pub fn expected_integer(lane: Lane, steps: Steps, dispatch: DispatchIndex) -> i32 {
    4096 * i32::from(lane.base())
        + steps.value() as i32 * i32::from(lane.a()) * i32::from(lane.b())
        + dispatch.value() as i32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Mismatch,
    NonFinite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failure {
    pub lane_index: u32,
    pub expected_integer: i32,
    pub actual_bits: u32,
    pub kind: FailureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LengthMismatch {
    pub expected: u32,
    pub actual: usize,
}

impl std::fmt::Display for LengthMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "expected {} lanes, received {}",
            self.expected, self.actual
        )
    }
}
impl std::error::Error for LengthMismatch {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckReport {
    recipe: Recipe,
    dispatch: DispatchIndex,
    checked_lanes: u32,
    mismatches: u32,
    nonfinite: u32,
    first_failure: Option<Failure>,
}

impl CheckReport {
    pub fn recipe(self) -> Recipe {
        self.recipe
    }
    pub fn dispatch(self) -> DispatchIndex {
        self.dispatch
    }
    pub fn checked_lanes(self) -> u32 {
        self.checked_lanes
    }
    /// All unequal lanes, including the nonfinite subset.
    pub fn mismatches(self) -> u32 {
        self.mismatches
    }
    pub fn nonfinite(self) -> u32 {
        self.nonfinite
    }
    pub fn first_failure(self) -> Option<Failure> {
        self.first_failure
    }
    /// Agreement with the CPU reference only; not evidence of GPU execution.
    pub fn matches_reference(self) -> bool {
        self.mismatches == 0
    }
}

/// Checks every supplied lane without allocating or mutating the source.
/// Length failure precedes scanning; a hash/exit status is never an input substitute.
pub fn check(
    recipe: Recipe,
    dispatch: DispatchIndex,
    output: &[f32],
) -> Result<CheckReport, LengthMismatch> {
    if output.len() != recipe.shape().lanes() as usize {
        return Err(LengthMismatch {
            expected: recipe.shape().lanes(),
            actual: output.len(),
        });
    }
    let mut report = CheckReport {
        recipe,
        dispatch,
        checked_lanes: recipe.shape().lanes(),
        mismatches: 0,
        nonfinite: 0,
        first_failure: None,
    };
    for (index, (lane, actual)) in recipe.lanes().zip(output).enumerate() {
        let expected = expected_integer(lane, recipe.shape().steps(), dispatch);
        if !actual.is_finite() || *actual != expected as f32 {
            let kind = if actual.is_finite() {
                FailureKind::Mismatch
            } else {
                report.nonfinite += 1;
                FailureKind::NonFinite
            };
            report.mismatches += 1;
            report.first_failure.get_or_insert(Failure {
                lane_index: index as u32,
                expected_integer: expected,
                actual_bits: actual.to_bits(),
                kind,
            });
        }
    }
    Ok(report)
}

/// One bounded run's inputs and independent integer bases, prepared once.
/// No dispatch-output cache; checking does not allocate or hash inputs.
pub struct PreparedReference {
    recipe: Recipe,
    a: Vec<f32>,
    b: Vec<f32>,
    x_base: Vec<i32>,
    expected_base: Vec<i32>,
}

impl PreparedReference {
    pub fn new(recipe: Recipe) -> Self {
        let capacity = recipe.shape().lanes() as usize;
        let mut prepared = Self {
            recipe,
            a: Vec::with_capacity(capacity),
            b: Vec::with_capacity(capacity),
            x_base: Vec::with_capacity(capacity),
            expected_base: Vec::with_capacity(capacity),
        };
        let zero = DispatchIndex::new(0).expect("zero is a valid tag");
        for lane in recipe.lanes() {
            prepared.a.push(f32::from(lane.a()));
            prepared.b.push(f32::from(lane.b()));
            prepared.x_base.push(4096 * i32::from(lane.base()));
            prepared
                .expected_base
                .push(expected_integer(lane, recipe.shape().steps(), zero));
        }
        prepared
    }
    pub fn recipe(&self) -> Recipe {
        self.recipe
    }
    pub fn a(&self) -> &[f32] {
        &self.a
    }
    pub fn b(&self) -> &[f32] {
        &self.b
    }
    pub fn fill_x(&self, dispatch: DispatchIndex, x: &mut [f32]) -> Result<(), LengthMismatch> {
        self.check_length(x.len())?;
        for (value, base) in x.iter_mut().zip(&self.x_base) {
            *value = (*base + dispatch.value() as i32) as f32;
        }
        Ok(())
    }
    fn check_length(&self, actual: usize) -> Result<(), LengthMismatch> {
        if actual == self.a.len() {
            Ok(())
        } else {
            Err(LengthMismatch {
                expected: self.recipe.shape().lanes(),
                actual,
            })
        }
    }
    pub fn check(
        &self,
        dispatch: DispatchIndex,
        output: &[f32],
    ) -> Result<CheckReport, LengthMismatch> {
        self.check_length(output.len())?;
        let mut report = CheckReport {
            recipe: self.recipe,
            dispatch,
            checked_lanes: self.recipe.shape().lanes(),
            mismatches: 0,
            nonfinite: 0,
            first_failure: None,
        };
        for (index, (base, actual)) in self.expected_base.iter().zip(output).enumerate() {
            let expected = *base + dispatch.value() as i32;
            if !actual.is_finite() || *actual != expected as f32 {
                let kind = if actual.is_finite() {
                    FailureKind::Mismatch
                } else {
                    report.nonfinite += 1;
                    FailureKind::NonFinite
                };
                report.mismatches += 1;
                report.first_failure.get_or_insert(Failure {
                    lane_index: index as u32,
                    expected_integer: expected,
                    actual_bits: actual.to_bits(),
                    kind,
                });
            }
        }
        Ok(report)
    }
}
