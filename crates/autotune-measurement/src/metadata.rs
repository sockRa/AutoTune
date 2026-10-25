//! Exact selected assertion strings, not baseline comparability or completeness.
use crate::{repeat, Provenance};
use serde::Serialize;
use std::collections::BTreeSet;

/// Namespaces in a retained repeat context; no approved key vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Conditions,
    Settings,
    Versions,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Field {
    category: Category,
    key: String,
}

impl Field {
    pub fn new(category: Category, key: &str) -> Result<Self, Error> {
        if key.trim().is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
            return Err(Error::InvalidKey);
        }
        Ok(Self {
            category,
            key: key.into(),
        })
    }

    pub fn category(&self) -> Category {
        self.category
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidKey,
    DuplicateField { field: Field },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

/// Explicit ordered selection; empty is valid and never means a successful check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Selection {
    fields: Vec<Field>,
}

impl Selection {
    pub fn new(fields: Vec<Field>) -> Result<Self, Error> {
        let mut seen = BTreeSet::new();
        for field in &fields {
            if !seen.insert(field) {
                return Err(Error::DuplicateField {
                    field: field.clone(),
                });
            }
        }
        Ok(Self { fields })
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Evidence {
    Provided { value: String },
    KeyAbsent,
    CategoryUnavailable { reason: String },
}

/// Equal/different describes two provided assertion strings only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Equal,
    Different,
    Unknown,
}

#[derive(Debug, Serialize)]
pub struct Observation {
    field: Field,
    left: Evidence,
    right: Evidence,
    outcome: Outcome,
}

impl Observation {
    pub fn field(&self) -> &Field {
        &self.field
    }

    pub fn left(&self) -> &Evidence {
        &self.left
    }

    pub fn right(&self) -> &Evidence {
        &self.right
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparability {
    NotAssessed,
}

/// Owned source snapshots retain identity, context and exact nested recorder JSON.
/// A Run does not carry its ledger's rejection state; no such state is inferred.
#[derive(Debug, Serialize)]
pub struct Report {
    schema_version: u32,
    provenance: Provenance,
    comparability: Comparability,
    left_run_json: String,
    right_run_json: String,
    selection: Selection,
    observations: Vec<Observation>,
}

impl Report {
    pub fn left_run_json(&self) -> &str {
        &self.left_run_json
    }

    pub fn right_run_json(&self) -> &str {
        &self.right_run_json
    }

    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }

    pub fn comparability(&self) -> Comparability {
        self.comparability
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

fn evidence(run: &repeat::Run, field: &Field) -> Evidence {
    let context = run.context();
    let metadata = match field.category {
        Category::Conditions => &context.conditions,
        Category::Settings => &context.settings,
        Category::Versions => &context.versions,
    };
    match metadata {
        repeat::Metadata::Provided { entries } => match entries.get(&field.key) {
            Some(value) => Evidence::Provided {
                value: value.clone(),
            },
            None => Evidence::KeyAbsent,
        },
        repeat::Metadata::Unavailable { reason } => Evidence::CategoryUnavailable {
            reason: reason.clone(),
        },
    }
}

/// Observe only selected assertions; does not parse recordings or assess runs.
pub fn report(
    left: &repeat::Run,
    right: &repeat::Run,
    selection: Selection,
) -> Result<Report, serde_json::Error> {
    let observations = selection
        .fields
        .iter()
        .map(|field| {
            let left = evidence(left, field);
            let right = evidence(right, field);
            let outcome = match (&left, &right) {
                (Evidence::Provided { value: a }, Evidence::Provided { value: b }) => {
                    if a == b {
                        Outcome::Equal
                    } else {
                        Outcome::Different
                    }
                }
                _ => Outcome::Unknown,
            };
            Observation {
                field: field.clone(),
                left,
                right,
                outcome,
            }
        })
        .collect();
    Ok(Report {
        schema_version: 1,
        provenance: Provenance::Simulated,
        comparability: Comparability::NotAssessed,
        left_run_json: serde_json::to_string(left)?,
        right_run_json: serde_json::to_string(right)?,
        selection,
        observations,
    })
}
