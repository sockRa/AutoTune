use autotune_measurement::{
    metadata::*, repeat, Identity, Metric, Policy, Reading, Recorder, UnavailableReason,
};
use serde_json::{json, Value};

fn provided(entries: &[(&str, &str)]) -> repeat::Metadata {
    repeat::Metadata::Provided {
        entries: entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn context(metadata: repeat::Metadata) -> repeat::Context {
    repeat::Context {
        conditions: metadata.clone(),
        settings: metadata.clone(),
        versions: metadata,
    }
}

fn recorder(id: &str) -> Recorder {
    Recorder::new(
        Identity::new(id, "synthetic", "v1", u64::MAX, "GPU-SIM", "pci").unwrap(),
        Policy::new(3, 10).unwrap(),
    )
}

fn ledger(left: repeat::Context, right: repeat::Context) -> repeat::Ledger {
    let mut ledger = repeat::Ledger::new(repeat::Policy::new(3, 10).unwrap());
    ledger.append(&recorder("left"), left).unwrap();
    ledger.append(&recorder("right"), right).unwrap();
    ledger
}

fn field(category: Category, key: &str) -> Field {
    Field::new(category, key).unwrap()
}

fn selected(fields: Vec<Field>) -> Selection {
    Selection::new(fields).unwrap()
}

fn observe(ledger: &repeat::Ledger, fields: Vec<Field>) -> Report {
    let before = ledger.to_json().unwrap();
    let report = report(&ledger.runs()[0], &ledger.runs()[1], selected(fields)).unwrap();
    assert_eq!(ledger.to_json().unwrap(), before);
    assert_eq!(report.comparability(), Comparability::NotAssessed);
    assert_eq!(report.to_json().unwrap(), report.to_json().unwrap());
    let rebuilt = report_from_same_inputs(ledger, report.selection().clone());
    assert_eq!(report.to_json().unwrap(), rebuilt.to_json().unwrap());
    assert_eq!(
        report.left_run_json(),
        serde_json::to_string(&ledger.runs()[0]).unwrap()
    );
    assert_eq!(
        report.right_run_json(),
        serde_json::to_string(&ledger.runs()[1]).unwrap()
    );
    report
}

fn report_from_same_inputs(ledger: &repeat::Ledger, selection: Selection) -> Report {
    report(&ledger.runs()[0], &ledger.runs()[1], selection).unwrap()
}

#[test]
fn unknown_evidence_routes_each_category_independently() {
    let ledger = ledger(
        repeat::Context {
            conditions: repeat::Metadata::Unavailable {
                reason: "conditions unknown".into(),
            },
            settings: provided(&[("other", "settings")]),
            versions: provided(&[("key", "version")]),
        },
        repeat::Context {
            conditions: provided(&[("other", "conditions")]),
            settings: repeat::Metadata::Unavailable {
                reason: "settings unknown".into(),
            },
            versions: repeat::Metadata::Unavailable {
                reason: "versions unknown".into(),
            },
        },
    );
    let report = observe(
        &ledger,
        vec![
            field(Category::Conditions, "key"),
            field(Category::Settings, "key"),
            field(Category::Versions, "key"),
        ],
    );
    for observation in report.observations() {
        assert_eq!(observation.outcome(), Outcome::Unknown);
    }
    assert_eq!(
        report.observations()[0].left(),
        &Evidence::CategoryUnavailable {
            reason: "conditions unknown".into()
        }
    );
    assert_eq!(report.observations()[0].right(), &Evidence::KeyAbsent);
    assert_eq!(report.observations()[1].left(), &Evidence::KeyAbsent);
    assert_eq!(
        report.observations()[1].right(),
        &Evidence::CategoryUnavailable {
            reason: "settings unknown".into()
        }
    );
    assert_eq!(
        report.observations()[2].left(),
        &Evidence::Provided {
            value: "version".into()
        }
    );
    assert_eq!(
        report.observations()[2].right(),
        &Evidence::CategoryUnavailable {
            reason: "versions unknown".into()
        }
    );
}

#[test]
fn cross_ledger_same_run_id_retains_distinct_full_identities_without_equating_runs() {
    let left = recorder("same-id");
    let right = Recorder::new(
        Identity::new(
            "same-id",
            "other-workload",
            "v2",
            17,
            "other-device",
            "other-pci",
        )
        .unwrap(),
        Policy::new(1, 20).unwrap(),
    );
    let mut a = repeat::Ledger::new(repeat::Policy::new(1, 1).unwrap());
    let mut b = repeat::Ledger::new(repeat::Policy::new(1, 1).unwrap());
    a.append(&left, context(provided(&[("key", "left")])))
        .unwrap();
    b.append(&right, context(provided(&[("key", "right")])))
        .unwrap();
    let selection = selected(vec![field(Category::Settings, "key")]);
    let report = report(&a.runs()[0], &b.runs()[0], selection).unwrap();
    assert_eq!(report.observations()[0].outcome(), Outcome::Different);
    assert_eq!(
        report.observations()[0].left(),
        &Evidence::Provided {
            value: "left".into()
        }
    );
    assert_eq!(
        report.observations()[0].right(),
        &Evidence::Provided {
            value: "right".into()
        }
    );
    for (snapshot, source) in [
        (report.left_run_json(), &left),
        (report.right_run_json(), &right),
    ] {
        let snapshot: Value = serde_json::from_str(snapshot).unwrap();
        assert_eq!(
            snapshot["identity"],
            serde_json::to_value(source.identity()).unwrap()
        );
        assert_eq!(snapshot["recorder_json"], source.to_json().unwrap());
    }
    assert_eq!(report.comparability(), Comparability::NotAssessed);
}

#[test]
fn selector_validation_matches_retained_metadata_key_validation() {
    let mut keys = vec![
        "".into(),
        " ".into(),
        "\nkey".into(),
        "a\u{85}".into(),
        "key\0".into(),
        " é ".into(),
    ];
    keys.extend([
        "a".repeat(256),
        "a".repeat(257),
        "é".repeat(128),
        "é".repeat(129),
    ]);
    for key in keys {
        let mut ledger = repeat::Ledger::new(repeat::Policy::new(1, 1).unwrap());
        let retained = ledger.append(&recorder("parity"), context(provided(&[(&key, "value")])));
        assert_eq!(
            Field::new(Category::Settings, &key).is_ok(),
            retained.is_ok(),
            "key {key:?}"
        );
    }
}

#[test]
fn exact_export_binds_ordered_fields_assertions_and_both_full_run_snapshots() {
    let ledger = ledger(
        context(provided(&[("driver", "v1"), ("ambient", "20")])),
        context(provided(&[("driver", "v2"), ("ambient", "20")])),
    );
    let fields = vec![
        field(Category::Versions, "driver"),
        field(Category::Conditions, "ambient"),
        field(Category::Settings, "absent"),
    ];
    let report = observe(&ledger, fields.clone());
    assert_eq!(report.selection().fields(), fields);
    assert_eq!(
        report.observations()[0].field().category(),
        Category::Versions
    );
    assert_eq!(report.observations()[0].field().key(), "driver");
    let expected = json!({
        "schema_version": 1,
        "provenance": "simulated",
        "comparability": "not_assessed",
        "left_run_json": serde_json::to_string(&ledger.runs()[0]).unwrap(),
        "right_run_json": serde_json::to_string(&ledger.runs()[1]).unwrap(),
        "selection": { "fields": fields },
        "observations": [
            {"field":{"category":"versions","key":"driver"},
             "left":{"status":"provided","value":"v1"},
             "right":{"status":"provided","value":"v2"},"outcome":"different"},
            {"field":{"category":"conditions","key":"ambient"},
             "left":{"status":"provided","value":"20"},
             "right":{"status":"provided","value":"20"},"outcome":"equal"},
            {"field":{"category":"settings","key":"absent"},
             "left":{"status":"key_absent"},"right":{"status":"key_absent"},
             "outcome":"unknown"}
        ]
    });
    assert_eq!(
        serde_json::from_str::<Value>(&report.to_json().unwrap()).unwrap(),
        expected
    );
}

#[test]
fn all_nine_evidence_pairs_preserve_each_side_and_unknown_is_not_equality() {
    let states = [
        provided(&[("key", "assertion")]),
        provided(&[("other", "value")]),
        repeat::Metadata::Unavailable {
            reason: "unobserved".into(),
        },
    ];
    let evidence = [
        Evidence::Provided {
            value: "assertion".into(),
        },
        Evidence::KeyAbsent,
        Evidence::CategoryUnavailable {
            reason: "unobserved".into(),
        },
    ];
    for (i, left) in states.iter().enumerate() {
        for (j, right) in states.iter().enumerate() {
            let ledger = ledger(context(left.clone()), context(right.clone()));
            let report = observe(&ledger, vec![field(Category::Settings, "key")]);
            let observation = &report.observations()[0];
            assert_eq!(observation.left(), &evidence[i]);
            assert_eq!(observation.right(), &evidence[j]);
            assert_eq!(
                observation.outcome(),
                if i == 0 && j == 0 {
                    Outcome::Equal
                } else {
                    Outcome::Unknown
                }
            );
        }
    }
}

#[test]
fn unavailable_reasons_remain_independent_exact_and_never_compared() {
    let ledger = ledger(
        context(repeat::Metadata::Unavailable {
            reason: " left \"原因\" ".into(),
        }),
        context(repeat::Metadata::Unavailable {
            reason: "right \\ unavailable".into(),
        }),
    );
    let report = observe(&ledger, vec![field(Category::Versions, "driver")]);
    let observation = &report.observations()[0];
    assert_eq!(observation.outcome(), Outcome::Unknown);
    assert_eq!(
        observation.left(),
        &Evidence::CategoryUnavailable {
            reason: " left \"原因\" ".into()
        }
    );
    assert_eq!(
        observation.right(),
        &Evidence::CategoryUnavailable {
            reason: "right \\ unavailable".into()
        }
    );
    let export: Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
    assert_eq!(
        export["observations"][0]["left"]["reason"],
        " left \"原因\" "
    );
    assert_eq!(
        export["observations"][0]["right"]["reason"],
        "right \\ unavailable"
    );
}

#[test]
fn category_namespaces_do_not_alias_or_auto_select_unrequested_differences() {
    let left = repeat::Context {
        conditions: provided(&[("key", "conditions"), ("ignored", "a")]),
        settings: provided(&[("key", "settings")]),
        versions: provided(&[("key", "versions")]),
    };
    let mut right = left.clone();
    right.conditions = provided(&[("key", "conditions"), ("ignored", "b")]);
    right.versions = provided(&[("key", "new")]);
    let ledger = ledger(left, right);
    let report = observe(
        &ledger,
        vec![
            field(Category::Settings, "key"),
            field(Category::Versions, "key"),
            field(Category::Conditions, "key"),
        ],
    );
    assert_eq!(report.observations().len(), 3);
    assert_eq!(
        report
            .observations()
            .iter()
            .map(Observation::outcome)
            .collect::<Vec<_>>(),
        vec![Outcome::Equal, Outcome::Different, Outcome::Equal]
    );
    for (observation, value) in
        report
            .observations()
            .iter()
            .zip(["settings", "versions", "conditions"])
    {
        assert_eq!(
            observation.left(),
            &Evidence::Provided {
                value: value.into()
            }
        );
    }
}

#[test]
fn keys_and_values_use_exact_case_whitespace_and_unicode_without_numeric_parsing() {
    let ledger = ledger(
        context(provided(&[
            ("Key", "1"),
            (" key ", " v "),
            ("é", "é"),
            ("k", "10"),
        ])),
        context(provided(&[
            ("key", "1"),
            (" key ", "v"),
            ("é", "e\u{301}"),
            ("k", "010"),
        ])),
    );
    let report = observe(
        &ledger,
        vec![
            field(Category::Conditions, "Key"),
            field(Category::Conditions, "key"),
            field(Category::Conditions, " key "),
            field(Category::Conditions, "é"),
            field(Category::Conditions, "k"),
            field(Category::Conditions, "e\u{301}"),
        ],
    );
    assert_eq!(
        report
            .observations()
            .iter()
            .map(Observation::outcome)
            .collect::<Vec<_>>(),
        vec![
            Outcome::Unknown,
            Outcome::Unknown,
            Outcome::Different,
            Outcome::Different,
            Outcome::Different,
            Outcome::Unknown
        ]
    );
}

#[test]
fn empty_selection_retains_sources_without_auto_fields_or_success() {
    let ledger = ledger(
        context(provided(&[("a", "1")])),
        context(provided(&[("b", "2")])),
    );
    let report = observe(&ledger, vec![]);
    assert!(report.selection().fields().is_empty());
    assert!(report.observations().is_empty());
    assert_eq!(report.comparability(), Comparability::NotAssessed);
}

#[test]
fn same_run_missing_keys_and_unavailable_categories_are_still_unknown() {
    let ledger = ledger(
        context(provided(&[("present", "yes")])),
        context(repeat::Metadata::Unavailable {
            reason: "unknown".into(),
        }),
    );
    for run in ledger.runs() {
        let report = report(
            run,
            run,
            selected(vec![field(Category::Conditions, "absent")]),
        )
        .unwrap();
        assert_eq!(report.observations()[0].outcome(), Outcome::Unknown);
        assert_eq!(report.left_run_json(), report.right_run_json());
    }
}

#[test]
fn duplicate_exact_fields_reject_but_categories_case_and_whitespace_are_distinct() {
    let f = field(Category::Settings, "key");
    assert_eq!(
        Selection::new(vec![f.clone(), f.clone()]),
        Err(Error::DuplicateField { field: f })
    );
    assert!(Selection::new(vec![
        field(Category::Settings, "key"),
        field(Category::Versions, "key"),
        field(Category::Settings, "Key"),
        field(Category::Settings, " key ")
    ])
    .is_ok());
}

#[test]
fn selector_validation_enforces_utf8_bytes_nonblank_and_control_boundaries() {
    for key in ["", " ", "\t", "\nkey", "key\0", "a\u{7f}", "a\u{85}"] {
        assert_eq!(
            Field::new(Category::Conditions, key),
            Err(Error::InvalidKey)
        );
    }
    for key in [
        "a".repeat(256),
        "é".repeat(128),
        format!(" {} ", "a".repeat(254)),
    ] {
        assert_eq!(field(Category::Conditions, &key).key(), key);
    }
    for key in ["a".repeat(257), "é".repeat(129)] {
        assert_eq!(
            Field::new(Category::Conditions, &key),
            Err(Error::InvalidKey)
        );
    }
}

#[test]
fn owned_report_preserves_exact_nested_json_large_numbers_and_source_independence() {
    let mut source = recorder("left \"esc\\原因");
    let identity = source.identity().clone();
    source
        .append(
            &identity,
            Metric::Temperature,
            Reading::Available { value: i64::MIN },
            u64::MAX - 11,
            u64::MAX,
        )
        .unwrap();
    let original = source.to_json().unwrap();
    let mut ledger = repeat::Ledger::new(repeat::Policy::new(3, 10).unwrap());
    ledger
        .append(&source, context(provided(&[("key", "\"値\\")])))
        .unwrap();
    ledger
        .append(&recorder("right"), context(provided(&[("key", "\"値\\")])))
        .unwrap();
    let report = observe(&ledger, vec![field(Category::Versions, "key")]);
    let export = report.to_json().unwrap();
    source
        .append(
            &identity,
            Metric::Power,
            Reading::Unavailable {
                reason: UnavailableReason::QueryFailed,
            },
            u64::MAX,
            u64::MAX,
        )
        .unwrap();
    ledger
        .append(&recorder("later"), context(provided(&[("key", "changed")])))
        .unwrap();
    drop(ledger);
    assert_eq!(report.to_json().unwrap(), export);
    let outer: Value = serde_json::from_str(&export).unwrap();
    let run: Value = serde_json::from_str(outer["left_run_json"].as_str().unwrap()).unwrap();
    assert_eq!(run["recorder_json"], original);
    assert_eq!(run["identity"]["workload_seed"].as_u64(), Some(u64::MAX));
    let recording: Value = serde_json::from_str(run["recorder_json"].as_str().unwrap()).unwrap();
    assert_eq!(
        recording["samples"][0]["reading"]["value"].as_i64(),
        Some(i64::MIN)
    );
    assert!(recording["samples"][0]
        .to_string()
        .contains(&u64::MAX.to_string()));
    assert_eq!(
        report.observations()[0].left(),
        &Evidence::Provided {
            value: "\"値\\".into()
        }
    );
}

#[test]
fn recorder_and_ledger_rejections_do_not_turn_metadata_observations_into_baselines() {
    let mut source = recorder("rejected");
    assert!(source
        .append(
            &source.identity().clone(),
            Metric::Power,
            Reading::Available { value: -1 },
            0,
            0
        )
        .is_err());
    let mut ledger = repeat::Ledger::new(repeat::Policy::new(2, 1).unwrap());
    let context = context(provided(&[("key", "value")]));
    ledger.append(&source, context.clone()).unwrap();
    ledger.append(&recorder("right"), context.clone()).unwrap();
    assert!(ledger.append(&recorder("excess"), context).is_err());
    let report = observe(&ledger, vec![field(Category::Conditions, "key")]);
    assert_eq!(report.observations()[0].outcome(), Outcome::Equal);
    let run: Value = serde_json::from_str(report.left_run_json()).unwrap();
    let recording: Value = serde_json::from_str(run["recorder_json"].as_str().unwrap()).unwrap();
    assert_eq!(recording["had_rejections"], true);
    assert!(ledger.had_rejections());
    assert_eq!(ledger.assessment(), repeat::Assessment::NotEvaluated);
    assert_eq!(report.comparability(), Comparability::NotAssessed);
}
