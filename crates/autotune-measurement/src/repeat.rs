//! Bounded synthetic provenance retention, not baseline or comparability assessment.
use crate::{Identity, Provenance, Recorder, TimeBasis};
use serde::Serialize;
use std::collections::BTreeMap;

/// Caller-selected count bounds, not allocation requests or product defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Policy {
    run_capacity: usize,
    maximum_metadata_entries_per_category: usize,
}

impl Policy {
    pub fn new(
        run_capacity: usize,
        maximum_metadata_entries_per_category: usize,
    ) -> Result<Self, Error> {
        if run_capacity == 0 {
            return Err(Error::ZeroRunCapacity);
        }
        if maximum_metadata_entries_per_category == 0 {
            return Err(Error::ZeroMetadataCapacity);
        }
        Ok(Self {
            run_capacity,
            maximum_metadata_entries_per_category,
        })
    }
}

/// Partial caller assertions with exact, case-sensitive keys; no approved vocabulary.
/// Validation occurs at append, so invalid context sets the ledger rejection marker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Metadata {
    Provided { entries: BTreeMap<String, String> },
    Unavailable { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Context {
    pub conditions: Metadata,
    pub settings: Metadata,
    pub versions: Metadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Conditions,
    Settings,
    Versions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Key,
    Value,
    Reason,
}

#[derive(Debug)]
pub enum Error {
    ZeroRunCapacity,
    ZeroMetadataCapacity,
    CapacityReached,
    DuplicateRunId,
    EmptyProvidedMetadata { category: Category },
    MetadataCapacityReached { category: Category },
    InvalidMetadata { category: Category, field: Field },
    Serialization(serde_json::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialization(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Assessment {
    NotEvaluated,
}

/// Immutable snapshot, including the exact original recorder JSON string.
#[derive(Debug, Serialize)]
pub struct Run {
    identity: Identity,
    context: Context,
    recorder_json: String,
}

impl Run {
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn recorder_json(&self) -> &str {
        &self.recorder_json
    }
}

/// Append-only owned evidence. Times remain relative to each individual run.
#[derive(Debug, Serialize)]
pub struct Ledger {
    schema_version: u32,
    provenance: Provenance,
    time_basis: TimeBasis,
    assessment: Assessment,
    policy: Policy,
    had_rejections: bool,
    runs: Vec<Run>,
}

impl Ledger {
    pub fn new(policy: Policy) -> Self {
        Self {
            schema_version: 1,
            provenance: Provenance::Simulated,
            time_basis: TimeBasis::RunRelativeMonotonicNanoseconds,
            assessment: Assessment::NotEvaluated,
            policy,
            had_rejections: false,
            runs: Vec::new(),
        }
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn assessment(&self) -> Assessment {
        self.assessment
    }

    pub fn had_rejections(&self) -> bool {
        self.had_rejections
    }

    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn append(&mut self, recorder: &Recorder, context: Context) -> Result<(), Error> {
        let result = self.try_append(recorder, context);
        if result.is_err() {
            self.had_rejections = true;
        }
        result
    }

    fn try_append(&mut self, recorder: &Recorder, context: Context) -> Result<(), Error> {
        if self.runs.len() >= self.policy.run_capacity {
            return Err(Error::CapacityReached);
        }
        if self
            .runs
            .iter()
            .any(|run| run.identity.run_id == recorder.identity().run_id)
        {
            return Err(Error::DuplicateRunId);
        }
        for (category, metadata) in [
            (Category::Conditions, &context.conditions),
            (Category::Settings, &context.settings),
            (Category::Versions, &context.versions),
        ] {
            self.validate(category, metadata)?;
        }
        let recorder_json = recorder.to_json().map_err(Error::Serialization)?;
        self.runs.push(Run {
            identity: recorder.identity().clone(),
            context,
            recorder_json,
        });
        Ok(())
    }

    fn validate(&self, category: Category, metadata: &Metadata) -> Result<(), Error> {
        let validate_string = |field, value: &str| {
            if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
                Err(Error::InvalidMetadata { category, field })
            } else {
                Ok(())
            }
        };
        match metadata {
            Metadata::Provided { entries } => {
                if entries.is_empty() {
                    return Err(Error::EmptyProvidedMetadata { category });
                }
                if entries.len() > self.policy.maximum_metadata_entries_per_category {
                    return Err(Error::MetadataCapacityReached { category });
                }
                for (key, value) in entries {
                    validate_string(Field::Key, key)?;
                    validate_string(Field::Value, value)?;
                }
            }
            Metadata::Unavailable { reason } => validate_string(Field::Reason, reason)?,
        }
        Ok(())
    }
}
