use autotune_measurement::{
    repeat::*, Identity, Metric, Policy as RecordingPolicy, Reading, Recorder, UnavailableReason,
};
use serde_json::{json, Value};

fn recorder(id: &str) -> Recorder {
    Recorder::new(
        Identity::new(id, "synthetic", "v1", 42, "GPU-SIM", "pci").unwrap(),
        RecordingPolicy::new(3, 10).unwrap(),
    )
}

fn provided(entries: &[(&str, &str)]) -> Metadata {
    Metadata::Provided {
        entries: entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    }
}

fn context() -> Context {
    Context {
        conditions: provided(&[("ambient", "caller assertion")]),
        settings: Metadata::Unavailable {
            reason: "not supplied".into(),
        },
        versions: provided(&[("driver", "synthetic version")]),
    }
}

fn ledger(capacity: usize, entries: usize) -> Ledger {
    Ledger::new(Policy::new(capacity, entries).unwrap())
}

fn category_mut(context: &mut Context, category: Category) -> &mut Metadata {
    match category {
        Category::Conditions => &mut context.conditions,
        Category::Settings => &mut context.settings,
        Category::Versions => &mut context.versions,
    }
}

fn add(r: &mut Recorder, metric: Metric, reading: Reading, observation: u64, receipt: u64) {
    r.append(&r.identity().clone(), metric, reading, observation, receipt)
        .unwrap();
}

// Every retention fixture checks exact source preservation and deterministic export.
fn retain(ledger: &mut Ledger, r: &Recorder, context: Context) {
    let original = r.to_json().unwrap();
    ledger.append(r, context.clone()).unwrap();
    let run = ledger.runs().last().unwrap();
    assert_eq!(run.recorder_json(), original);
    assert_eq!(run.identity(), r.identity());
    assert_eq!(run.context(), &context);
    assert_eq!(ledger.assessment(), Assessment::NotEvaluated);
    assert_eq!(ledger.to_json().unwrap(), ledger.to_json().unwrap());
    assert_eq!(r.to_json().unwrap(), original);
}

#[test]
fn empty_ledger_export_is_exact_and_deterministic() {
    let l = ledger(2, 1);
    let expected = concat!(
        "{\"schema_version\":1,\"provenance\":\"simulated\",",
        "\"time_basis\":\"run_relative_monotonic_nanoseconds\",",
        "\"assessment\":\"not_evaluated\",",
        "\"policy\":{\"run_capacity\":2,\"maximum_metadata_entries_per_category\":1},",
        "\"had_rejections\":false,\"runs\":[]}"
    );
    assert_eq!(l.to_json().unwrap(), expected);
    assert_eq!(l.to_json().unwrap(), expected);
}

#[test]
fn interleaved_snapshot_preserves_large_integers_and_mixed_evidence_losslessly() {
    let time = (1u64 << 53) + 1;
    let mut source = Recorder::new(
        Identity::new("large", "synthetic", "v1", u64::MAX, "GPU-SIM", "pci").unwrap(),
        RecordingPolicy::new(4, 10).unwrap(),
    );
    for (metric, reading, observation, receipt) in [
        (
            Metric::Temperature,
            Reading::Available { value: i64::MIN },
            time,
            time,
        ),
        (
            Metric::Power,
            Reading::Unavailable {
                reason: UnavailableReason::QueryFailed,
            },
            time + 1,
            time + 1,
        ),
        (
            Metric::Temperature,
            Reading::Available { value: i64::MAX },
            time + 2,
            time + 13,
        ),
        (
            Metric::Power,
            Reading::Available { value: i64::MAX },
            u64::MAX - 11,
            u64::MAX,
        ),
    ] {
        add(&mut source, metric, reading, observation, receipt);
    }
    let original = source.to_json().unwrap();
    let mut l = ledger(1, 1);
    retain(&mut l, &source, context());
    let export = l.to_json().unwrap();
    let decoded: Value = serde_json::from_str(&export).unwrap();
    let round_trip: Value =
        serde_json::from_str(&serde_json::to_string(&decoded).unwrap()).unwrap();
    assert_eq!(round_trip, decoded);
    let embedded = round_trip["runs"][0]["recorder_json"].as_str().unwrap();
    assert_eq!(embedded, original);
    let recording: Value = serde_json::from_str(embedded).unwrap();
    assert_eq!(
        recording["identity"]["workload_seed"].as_u64(),
        Some(u64::MAX)
    );
    assert_eq!(
        recording["samples"][0]["observation_ns"].as_u64(),
        Some(time)
    );
    assert_eq!(recording["samples"][0]["receipt_ns"].as_u64(), Some(time));
    assert_eq!(
        recording["samples"][0]["reading"]["value"].as_i64(),
        Some(i64::MIN)
    );
    assert_eq!(
        recording["samples"][2]["reading"]["value"].as_i64(),
        Some(i64::MAX)
    );
    assert_eq!(
        recording["samples"][3]["observation_ns"].as_u64(),
        Some(u64::MAX - 11)
    );
    assert_eq!(
        recording["samples"][3]["receipt_ns"].as_u64(),
        Some(u64::MAX)
    );
    assert_eq!(
        recording["samples"][3]["reading"]["value"].as_i64(),
        Some(i64::MAX)
    );
    assert_eq!(
        recording["samples"],
        json!([
            {"metric":"temperature", "unit":"degC", "reading":{"status":"available", "value":i64::MIN}, "observation_ns":time, "receipt_ns":time, "freshness":"fresh"},
            {"metric":"power", "unit":"mW", "reading":{"status":"unavailable", "reason":"query_failed"}, "observation_ns":time + 1, "receipt_ns":time + 1, "freshness":"fresh"},
            {"metric":"temperature", "unit":"degC", "reading":{"status":"available", "value":i64::MAX}, "observation_ns":time + 2, "receipt_ns":time + 13, "freshness":"stale"},
            {"metric":"power", "unit":"mW", "reading":{"status":"available", "value":i64::MAX}, "observation_ns":u64::MAX - 11, "receipt_ns":u64::MAX, "freshness":"stale"}
        ])
    );
    assert_eq!(source.to_json().unwrap(), original);
}

#[test]
fn full_json_contract_is_simulated_and_not_evaluated() {
    let r = recorder("run");
    let mut l = ledger(2, 1);
    let policy = *l.policy();
    retain(&mut l, &r, context());
    let expected = format!(
        concat!(
            "{{\"schema_version\":1,\"provenance\":\"simulated\",",
            "\"time_basis\":\"run_relative_monotonic_nanoseconds\",",
            "\"assessment\":\"not_evaluated\",",
            "\"policy\":{{\"run_capacity\":2,\"maximum_metadata_entries_per_category\":1}},",
            "\"had_rejections\":false,\"runs\":[{{\"identity\":{},",
            "\"context\":{{\"conditions\":{{\"status\":\"provided\",\"entries\":{{\"ambient\":\"caller assertion\"}}}},",
            "\"settings\":{{\"status\":\"unavailable\",\"reason\":\"not supplied\"}},",
            "\"versions\":{{\"status\":\"provided\",\"entries\":{{\"driver\":\"synthetic version\"}}}}}},",
            "\"recorder_json\":{}}}]}}"
        ),
        serde_json::to_string(r.identity()).unwrap(),
        serde_json::to_string(&r.to_json().unwrap()).unwrap(),
    );
    assert_eq!(l.to_json().unwrap(), expected);
    assert_eq!(l.policy(), &policy);
    let value: Value = serde_json::from_str(&expected).unwrap();
    assert_eq!(value["assessment"], "not_evaluated");
    assert_eq!(value["runs"][0]["recorder_json"], r.to_json().unwrap());
}

#[test]
fn distinct_runs_preserve_changed_identity_context_and_independent_epochs() {
    let mut first = recorder("first");
    add(
        &mut first,
        Metric::Power,
        Reading::Available { value: 7 },
        100,
        100,
    );
    let mut second = Recorder::new(
        Identity::new("second", "other", "v2", 99, "GPU-OTHER", "other-pci").unwrap(),
        RecordingPolicy::new(1, 2).unwrap(),
    );
    add(
        &mut second,
        Metric::Power,
        Reading::Available { value: 9 },
        0,
        0,
    );
    let mut changed = context();
    changed.conditions = provided(&[("different", "conditions")]);
    let mut l = ledger(3, 1);
    retain(&mut l, &first, context());
    retain(&mut l, &second, changed);
    // A distinct ID with otherwise identical identity is also accepted.
    retain(&mut l, &recorder("third"), context());
    assert_eq!(l.runs()[0].identity(), first.identity());
    assert_eq!(l.runs()[1].identity(), second.identity());
    assert_eq!(l.runs().len(), 3);
    assert!(!l.had_rejections());
}

#[test]
fn unavailable_categories_retain_reasons_without_entries() {
    let mut c = context();
    for category in [Category::Conditions, Category::Settings, Category::Versions] {
        *category_mut(&mut c, category) = Metadata::Unavailable {
            reason: format!("unknown {category:?}"),
        };
    }
    let mut l = ledger(1, 1);
    retain(&mut l, &recorder("run"), c);
    let value: Value = serde_json::from_str(&l.to_json().unwrap()).unwrap();
    for (key, reason) in [
        ("conditions", "unknown Conditions"),
        ("settings", "unknown Settings"),
        ("versions", "unknown Versions"),
    ] {
        assert_eq!(
            value["runs"][0]["context"][key],
            json!({"status":"unavailable", "reason":reason})
        );
    }
}

#[test]
fn invalid_strings_in_every_category_reject_atomically_and_stick() {
    for category in [Category::Conditions, Category::Settings, Category::Versions] {
        for field in [Field::Key, Field::Value, Field::Reason] {
            for invalid in [
                "".into(),
                " \u{2003}".into(),
                "a\nb".into(),
                "a\u{7f}b".into(),
                "a\u{85}b".into(),
                "é".repeat(128) + "x",
            ] {
                let mut l = ledger(2, 1);
                retain(&mut l, &recorder("accepted"), context());
                let before = serde_json::to_string(l.runs()).unwrap();
                let mut c = context();
                *category_mut(&mut c, category) = match field {
                    Field::Key => provided(&[(&invalid, "valid")]),
                    Field::Value => provided(&[("valid", &invalid)]),
                    Field::Reason => Metadata::Unavailable { reason: invalid },
                };
                assert!(
                    matches!(l.append(&recorder("later"), c), Err(Error::InvalidMetadata { category: actual, field: actual_field }) if actual == category && actual_field == field)
                );
                assert_eq!(serde_json::to_string(l.runs()).unwrap(), before);
                assert!(l.had_rejections());
                retain(&mut l, &recorder("later"), context());
                assert!(l.had_rejections());
                let value: Value = serde_json::from_str(&l.to_json().unwrap()).unwrap();
                assert_eq!(value["had_rejections"], true);
            }
        }
    }
}

#[test]
fn empty_and_overfull_provided_categories_reject_without_replacement() {
    for category in [Category::Conditions, Category::Settings, Category::Versions] {
        for empty in [true, false] {
            let mut l = ledger(2, 1);
            retain(&mut l, &recorder("accepted"), context());
            let before = serde_json::to_string(l.runs()).unwrap();
            let mut c = context();
            *category_mut(&mut c, category) = if empty {
                provided(&[])
            } else {
                provided(&[("a", "1"), ("b", "2")])
            };
            let error = l.append(&recorder("new"), c).unwrap_err();
            if empty {
                assert!(
                    matches!(error, Error::EmptyProvidedMetadata { category: actual } if actual == category)
                );
            } else {
                assert!(
                    matches!(error, Error::MetadataCapacityReached { category: actual } if actual == category)
                );
            }
            assert_eq!(serde_json::to_string(l.runs()).unwrap(), before);
            assert!(l.had_rejections());
        }
    }
}

#[test]
fn policy_zero_exact_limits_overflow_and_huge_capacity_without_preallocation() {
    assert!(matches!(Policy::new(0, 1), Err(Error::ZeroRunCapacity)));
    assert!(matches!(
        Policy::new(1, 0),
        Err(Error::ZeroMetadataCapacity)
    ));
    let huge = ledger(usize::MAX, usize::MAX);
    assert!(huge.runs().is_empty());
    assert!(!huge.had_rejections());
    let mut l = ledger(1, 2);
    let metadata = provided(&[("a", "1"), ("b", "2")]);
    retain(
        &mut l,
        &recorder("accepted"),
        Context {
            conditions: metadata.clone(),
            settings: metadata.clone(),
            versions: metadata,
        },
    );
    let before = serde_json::to_string(l.runs()).unwrap();
    assert!(matches!(
        l.append(&recorder("overflow"), context()),
        Err(Error::CapacityReached)
    ));
    assert_eq!(serde_json::to_string(l.runs()).unwrap(), before);
    assert!(l.had_rejections());
}

#[test]
fn exact_utf8_byte_limits_are_accepted_without_trimming_or_normalization() {
    let max = "é".repeat(128);
    let metadata = provided(&[(&max, &max)]);
    let mut l = ledger(1, 1);
    retain(
        &mut l,
        &recorder("run"),
        Context {
            conditions: metadata.clone(),
            settings: metadata,
            versions: Metadata::Unavailable { reason: max },
        },
    );
    let mut l = ledger(1, 4);
    let metadata = provided(&[("Key", "1"), ("key", "2"), (" key ", " 3 "), ("kéy", "4")]);
    retain(
        &mut l,
        &recorder("run"),
        Context {
            conditions: metadata.clone(),
            settings: metadata.clone(),
            versions: metadata,
        },
    );
}

#[test]
fn duplicate_ids_reject_same_and_changed_identity_without_replacing_evidence() {
    for changed in [false, true] {
        let mut l = ledger(2, 1);
        retain(&mut l, &recorder("same"), context());
        let before = serde_json::to_string(l.runs()).unwrap();
        let duplicate = if changed {
            Recorder::new(
                Identity::new("same", "other", "v9", 0, "other-gpu", "other-pci").unwrap(),
                RecordingPolicy::new(1, 1).unwrap(),
            )
        } else {
            recorder("same")
        };
        assert!(matches!(
            l.append(&duplicate, context()),
            Err(Error::DuplicateRunId)
        ));
        assert_eq!(serde_json::to_string(l.runs()).unwrap(), before);
        assert!(l.had_rejections());
        retain(&mut l, &recorder("new"), context());
        assert!(l.had_rejections());
    }
}

#[test]
fn empty_stale_missing_and_rejected_recorders_remain_exact_evidence() {
    let mut l = ledger(4, 1);
    retain(&mut l, &recorder("empty"), context());
    let mut stale = recorder("stale");
    add(
        &mut stale,
        Metric::Power,
        Reading::Available { value: 10 },
        0,
        11,
    );
    retain(&mut l, &stale, context());
    let mut missing = recorder("missing");
    add(
        &mut missing,
        Metric::Power,
        Reading::Unavailable {
            reason: UnavailableReason::NotSampled,
        },
        0,
        0,
    );
    retain(&mut l, &missing, context());
    let mut rejected = recorder("rejected");
    assert!(rejected
        .append(
            &rejected.identity().clone(),
            Metric::Power,
            Reading::Available { value: -1 },
            0,
            0
        )
        .is_err());
    retain(&mut l, &rejected, context());
    let source: Value = serde_json::from_str(l.runs()[3].recorder_json()).unwrap();
    assert_eq!(source["had_rejections"], true);
    assert!(!l.had_rejections());
    assert_eq!(l.assessment(), Assessment::NotEvaluated);
}

#[test]
fn snapshot_is_owned_and_later_source_changes_do_not_rewrite_old_runs() {
    let mut source = recorder("source");
    let mut c = context();
    let original_context = c.clone();
    let original_json = source.to_json().unwrap();
    let mut l = ledger(2, 1);
    retain(&mut l, &source, c.clone());
    c.conditions = provided(&[("changed", "after append")]);
    add(
        &mut source,
        Metric::Power,
        Reading::Available { value: 2 },
        0,
        0,
    );
    assert!(source
        .append(
            &source.identity().clone(),
            Metric::Power,
            Reading::Available { value: 3 },
            0,
            0
        )
        .is_err());
    assert_ne!(source.to_json().unwrap(), original_json);
    assert!(matches!(
        l.append(&source, c.clone()),
        Err(Error::DuplicateRunId)
    ));
    retain(&mut l, &recorder("later"), c);
    assert_eq!(l.runs()[0].recorder_json(), original_json);
    assert_eq!(l.runs()[0].context(), &original_context);
    assert!(l.had_rejections());
    drop(source);
    assert_eq!(l.runs()[0].recorder_json(), original_json);
    let final_export: Value = serde_json::from_str(&l.to_json().unwrap()).unwrap();
    assert_eq!(final_export["runs"][0]["recorder_json"], original_json);
    assert_eq!(
        final_export["runs"][0]["context"],
        serde_json::to_value(&original_context).unwrap()
    );
}

#[test]
fn metadata_insertion_order_and_repeated_exports_are_byte_identical() {
    let mut first = ledger(1, 2);
    let mut second = ledger(1, 2);
    let mut a = context();
    a.conditions = provided(&[("z", "last"), ("a", "first")]);
    let mut b = context();
    b.conditions = provided(&[("a", "first"), ("z", "last")]);
    retain(&mut first, &recorder("run"), a);
    retain(&mut second, &recorder("run"), b);
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
    assert!(first
        .to_json()
        .unwrap()
        .contains("\"entries\":{\"a\":\"first\",\"z\":\"last\"}"));
}
