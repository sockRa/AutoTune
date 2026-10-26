use autotune_benchmark_worker::evidence::{
    canonical::{AccessError, FieldError, Object, ObjectError, Ratio, RatioError, Value},
    decode,
    digest::{Digest, DigestError, Domain},
    encode,
    parse::{ParseError, MAX_DEPTH},
};

const SESSION_A1: &str = "af8aacad2d10f122084a16cfd7f84fbab66e39b43afbca3c0c715d9f8dd61d9a";
const MANIFEST_A1: &str = "ee94e6fb273b02fd4c6bf2c1127989d49624770a310b0987afa0308e40e9228e";
const PROFILE_A1: &str = "16fb94aa41263c0421a2df29d590dd2a4bbea05278ae42ac8371054b6ebb2169";
const RAW_A1: &str = "d80a361d2be4c63e3961e4d37abc5c901b50f4768ca1e12b845d11e48866ab5b";
const SESSION_EMPTY: &str = "c5dd725be0ec0ec15a7fdf435537919680d4c29b5cdb416693361d8e43539d12";

fn text(value: &str) -> Value {
    Value::Text(value.to_owned())
}

fn object(entries: &[(&str, Value)]) -> Object {
    let mut object = Object::new();
    for (key, value) in entries {
        object.insert(*key, value.clone()).unwrap();
    }
    object
}

#[test]
fn writer_emits_canonical_text_escapes() {
    assert_eq!(text("a\"b\\c/d").to_canonical(), r#""a\"b\\c/d""#);
    assert_eq!(text("\u{8}\u{c}\n\r\t").to_canonical(), r#""\b\f\n\r\t""#);
    assert_eq!(text("\u{0}\u{1f}").to_canonical(), r#""\u0000\u001f""#);
    assert_eq!(text("é/").to_canonical(), "\"é/\"");
    assert_eq!(text("").to_canonical(), "\"\"");
}

#[test]
fn writer_preserves_order_and_encodes_every_type() {
    let document = object(&[
        ("b", Value::U64(u64::MAX)),
        ("a", Value::I64(i64::MIN)),
        ("ratio", Value::Ratio(Ratio::new(-3, 7).unwrap())),
        ("array", Value::Array(vec![Value::Null, Value::Bool(false)])),
        ("empty", Value::Object(Object::new())),
    ]);
    let value = Value::Object(document);
    let bytes = value.to_canonical();
    assert_eq!(
        bytes,
        "{\"b\":\"18446744073709551615\",\"a\":\"-9223372036854775808\",\
         \"ratio\":{\"num\":\"-3\",\"den\":\"7\"},\"array\":[null,false],\"empty\":{}}"
    );
    assert_eq!(value.to_canonical(), bytes);

    let decoded = decode(bytes.as_bytes(), bytes.len()).unwrap();
    assert_eq!(decoded.to_canonical(), bytes);
    let decoded = decoded.as_object().unwrap();
    assert_eq!(
        decoded.iter().map(|(key, _)| key).collect::<Vec<_>>(),
        vec!["b", "a", "ratio", "array", "empty"]
    );
}

#[test]
fn integers_survive_extremes_and_reject_noncanonical_text() {
    let max_u64 = Value::U64(u64::MAX).to_canonical();
    assert_eq!(max_u64, "\"18446744073709551615\"");
    assert_eq!(
        decode(max_u64.as_bytes(), 64).unwrap().as_u64().unwrap(),
        u64::MAX
    );

    let min_i64 = Value::I64(i64::MIN).to_canonical();
    assert_eq!(min_i64, "\"-9223372036854775808\"");
    assert_eq!(
        decode(min_i64.as_bytes(), 64).unwrap().as_i64().unwrap(),
        i64::MIN
    );

    let max_i64 = Value::I64(i64::MAX).to_canonical();
    assert_eq!(max_i64, "\"9223372036854775807\"");
    assert_eq!(
        decode(max_i64.as_bytes(), 64).unwrap().as_i64().unwrap(),
        i64::MAX
    );

    for (input, expected) in [
        ("", AccessError::Empty),
        ("+1", AccessError::Sign),
        ("-", AccessError::Digit),
        ("-1", AccessError::Sign),
        ("-0", AccessError::Sign),
        ("01", AccessError::LeadingZero),
        ("00", AccessError::LeadingZero),
        ("1.0", AccessError::Digit),
        (" 1", AccessError::Digit),
        ("1 ", AccessError::Digit),
        ("18446744073709551616", AccessError::Overflow),
    ] {
        assert_eq!(text(input).as_u64(), Err(expected), "u64 {input:?}");
    }
    for (input, expected) in [
        ("", AccessError::Empty),
        ("+1", AccessError::Sign),
        ("-", AccessError::Digit),
        ("-0", AccessError::NegativeZero),
        ("-00", AccessError::LeadingZero),
        ("-01", AccessError::LeadingZero),
        ("01", AccessError::LeadingZero),
        ("9223372036854775808", AccessError::Overflow),
        ("-9223372036854775809", AccessError::Overflow),
    ] {
        assert_eq!(text(input).as_i64(), Err(expected), "i64 {input:?}");
    }
    assert_eq!(Value::U64(1).as_u64(), Ok(1));
    assert_eq!(Value::U64(0).as_i64(), Ok(0));
    assert_eq!(Value::I64(-1).as_u64(), Err(AccessError::Sign));
    assert_eq!(Value::I64(7).as_u64(), Ok(7));
    assert_eq!(Value::I64(i64::MIN).as_i64(), Ok(i64::MIN));
    assert_eq!(Value::U64(u64::MAX).as_i64(), Err(AccessError::Overflow));
    assert_eq!(Value::Null.as_u64(), Err(AccessError::NotText));
    assert_eq!(Value::Null.as_i64(), Err(AccessError::NotText));
}

#[test]
fn ratio_accessor_validates_shape_and_denominator() {
    assert_eq!(Ratio::new(1, 0), Err(RatioError::Denominator));
    let ratio = Ratio::new(-7, 3).unwrap();
    assert_eq!(ratio.num(), -7);
    assert_eq!(ratio.den(), 3);

    let bytes = Value::Ratio(ratio).to_canonical();
    assert_eq!(bytes, "{\"num\":\"-7\",\"den\":\"3\"}");
    assert_eq!(
        decode(bytes.as_bytes(), 64).unwrap().as_ratio().unwrap(),
        ratio
    );

    let cases = [
        (
            vec![("num", text("-7")), ("den", text("0"))],
            AccessError::Denominator,
        ),
        (vec![("num", text("-7"))], AccessError::Fields),
        (
            vec![
                ("num", text("-7")),
                ("den", text("3")),
                ("extra", text("1")),
            ],
            AccessError::Fields,
        ),
        (
            vec![("den", text("3")), ("num", text("-7"))],
            AccessError::Fields,
        ),
        (
            vec![("num", text("+7")), ("den", text("3"))],
            AccessError::Sign,
        ),
        (
            vec![("num", Value::Bool(true)), ("den", text("3"))],
            AccessError::NotText,
        ),
    ];
    for (entries, expected) in cases {
        assert_eq!(Value::Object(object(&entries)).as_ratio(), Err(expected));
    }
    assert_eq!(text("1").as_ratio(), Err(AccessError::NotRatio));
    assert_eq!(Value::U64(1).as_ratio(), Err(AccessError::NotRatio));

    // A typed ratio round-trips through the accessor without re-parsing text.
    let typed = Value::Ratio(Ratio::new(-7, 3).unwrap());
    assert_eq!(typed.as_ratio(), Ok(ratio));
    assert_eq!(
        Value::Ratio(Ratio::new(0, 1).unwrap()).as_ratio(),
        Ok(Ratio::new(0, 1).unwrap())
    );
}

#[test]
fn exact_field_helper_requires_ordered_exact_keys() {
    let ordered = object(&[("a", text("1")), ("b", text("2"))]);
    assert_eq!(ordered.require_exact(&["a", "b"]), Ok(()));
    assert_eq!(ordered.require_exact(&["a"]), Err(FieldError::Count));
    assert_eq!(ordered.require_exact(&["a", "c"]), Err(FieldError::Name));
    assert_eq!(ordered.require_exact(&["b", "a"]), Err(FieldError::Name));
    assert_eq!(ordered.len(), 2);
    assert!(!ordered.is_empty());
    assert_eq!(ordered.get("b"), Some(&text("2")));
    assert_eq!(ordered.get("c"), None);

    let mut duplicate = Object::new();
    duplicate.insert("a", text("1")).unwrap();
    assert_eq!(
        duplicate.insert("a", text("2")),
        Err(ObjectError::DuplicateKey)
    );
    assert_eq!(encode(&Value::Object(duplicate)), "{\"a\":\"1\"}");
}

#[test]
fn structural_accessors_and_without_are_total() {
    assert_eq!(text("x").as_text(), Some("x"));
    assert_eq!(Value::U64(1).as_text(), None);
    assert_eq!(Value::Null.as_text(), None);

    let array = vec![text("1"), Value::Null];
    assert_eq!(
        Value::Array(array.clone()).as_array(),
        Some(array.as_slice())
    );
    assert_eq!(text("1").as_array(), None);

    let document = object(&[("a", text("1")), ("b", Value::Null)]);
    assert_eq!(Value::Object(document.clone()).as_object(), Some(&document));
    assert_eq!(Value::Array(array).as_object(), None);

    let present = document.without("a");
    assert_eq!(present, object(&[("b", Value::Null)]));
    let absent = document.without("missing");
    assert_eq!(absent, document);
    assert_eq!(absent.len(), document.len());

    // `without` is top-level only: a nested field with the same name survives.
    let nested = object(&[("outer", Value::Object(object(&[("a", text("1"))])))]);
    assert_eq!(encode(&Value::Object(nested.without("outer"))), "{}");
    assert_eq!(
        encode(&Value::Object(nested.without("a"))),
        "{\"outer\":{\"a\":\"1\"}}"
    );
}

#[test]
fn digest_vectors_are_pinned_and_domain_separated() {
    let payload = Value::Object(object(&[("a", text("1"))])).to_canonical();
    assert_eq!(payload, "{\"a\":\"1\"}");

    for (domain, expected) in [
        (Domain::Session, SESSION_A1),
        (Domain::Manifest, MANIFEST_A1),
        (Domain::Profile, PROFILE_A1),
        (Domain::RawArtifact, RAW_A1),
    ] {
        let digest = Digest::of(domain, payload.as_bytes());
        assert_eq!(digest.to_hex(), expected);
        assert_eq!(Digest::parse(expected), Ok(digest));
        assert_eq!(digest.as_bytes().len(), 32);
        assert_eq!(digest.to_hex().len(), 64);
        assert!(expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }
    assert_eq!(Digest::of(Domain::Session, b"{}").to_hex(), SESSION_EMPTY);

    let distinct = Domain::ALL
        .iter()
        .map(|domain| Digest::of(*domain, payload.as_bytes()).to_hex())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(distinct.len(), 4);
    assert_ne!(
        Digest::of(Domain::Session, b"{\"a\":\"1\"}"),
        Digest::of(Domain::Session, b"{\"a\":\"2\"}")
    );
}

#[test]
fn digest_of_value_is_the_canonical_entry_point() {
    // Session-domain vectors independently derived outside this crate from the
    // UTF-8 payload bytes (prefix + 0x00 + payload); the lengths are pinned too.
    let escaped = Value::Object(object(&[("t", text("a\"b\\c\n\u{1}"))]));
    let escaped_bytes = encode(&escaped);
    assert_eq!(escaped_bytes, "{\"t\":\"a\\\"b\\\\c\\n\\u0001\"}");
    assert_eq!(escaped_bytes.len(), 23);
    assert_eq!(
        Digest::of_value(Domain::Session, &escaped).to_hex(),
        "56787dab9110e2370187a1a08f4f2ddcccb1de627a499ade4f4a9187255c6ecf"
    );

    let extremes = Value::Object(object(&[(
        "a",
        Value::Array(vec![Value::U64(0), Value::U64(u64::MAX)]),
    )]));
    let extremes_bytes = encode(&extremes);
    assert_eq!(extremes_bytes, "{\"a\":[\"0\",\"18446744073709551615\"]}");
    assert_eq!(extremes_bytes.len(), 34);
    assert_eq!(
        Digest::of_value(Domain::Session, &extremes).to_hex(),
        "cd91a4421ce474cd2b4ab5808723377fadd44086e1c4cc003256d22723d50428"
    );

    let a1 = Value::Object(object(&[("a", text("1"))]));
    assert_eq!(Digest::of_value(Domain::Session, &a1).to_hex(), SESSION_A1);
    assert_eq!(
        Digest::of_value(Domain::Session, &a1),
        Digest::of(Domain::Session, encode(&a1).as_bytes())
    );
    assert_eq!(
        Digest::of_value(Domain::Session, &Value::Object(Object::new())).to_hex(),
        SESSION_EMPTY
    );

    // Derives: domains and digests are hashable and totally ordered.
    let mut domains = Domain::ALL;
    domains.sort();
    assert_eq!(domains, Domain::ALL);
    assert!(Domain::Manifest < Domain::Profile);
    let mut hashed = std::collections::HashSet::new();
    hashed.insert(Digest::of_value(Domain::Session, &a1));
    assert!(hashed.contains(&Digest::of(Domain::Session, b"{\"a\":\"1\"}")));
}

#[test]
fn digest_parse_requires_lowercase_64_hex() {
    assert_eq!(Digest::parse(SESSION_A1).unwrap().to_hex(), SESSION_A1);
    assert_eq!(
        Digest::parse(&SESSION_A1.to_uppercase()),
        Err(DigestError::Digit)
    );
    assert_eq!(Digest::parse(&SESSION_A1[..63]), Err(DigestError::Length));
    assert_eq!(
        Digest::parse(&format!("{SESSION_A1}0")),
        Err(DigestError::Length)
    );
    let mut invalid = SESSION_A1.to_owned();
    invalid.replace_range(0..1, "g");
    assert_eq!(Digest::parse(&invalid), Err(DigestError::Digit));
    assert_eq!(Digest::parse(""), Err(DigestError::Length));
}

#[test]
fn digest_field_is_excluded_from_its_own_payload() {
    let document = object(&[("a", text("1")), ("digest", text(SESSION_A1))]);
    let payload = Value::Object(document.without("digest")).to_canonical();
    assert_eq!(payload, "{\"a\":\"1\"}");
    assert_eq!(
        Digest::of(Domain::Session, payload.as_bytes()).to_hex(),
        SESSION_A1
    );
}

#[test]
fn reader_accepts_and_reencodes_canonical_documents() {
    let document = object(&[
        ("text", text("a\"b\\c/d\n\t\u{1}é")),
        ("null", Value::Null),
        ("bool", Value::Bool(true)),
        ("u64", Value::U64(u64::MAX)),
        ("i64", Value::I64(i64::MIN)),
        ("ratio", Value::Ratio(Ratio::new(-1, 2).unwrap())),
        ("array", Value::Array(vec![Value::Null, Value::Bool(false)])),
        ("empty", Value::Object(Object::new())),
    ]);
    let bytes = Value::Object(document).to_canonical();
    let decoded = decode(bytes.as_bytes(), bytes.len()).unwrap();
    assert_eq!(decoded.to_canonical(), bytes);

    for accepted in [
        "{\"a\":\"\\u0000\"}",
        "{\"a\":\"\\b\\f\\n\\r\\t\"}",
        "{\"a\":\"\\\"\\\\\"}",
        "[]",
        "{}",
        "null",
        "false",
    ] {
        let decoded = decode(accepted.as_bytes(), accepted.len()).unwrap();
        assert_eq!(decoded.to_canonical(), accepted);
    }
}

#[test]
fn reader_rejects_noncanonical_and_malformed_input() {
    let cases: &[(&str, ParseError)] = &[
        (" {\"a\":\"1\"}", ParseError::Syntax),
        ("{\"a\": \"1\"}", ParseError::Syntax),
        ("{\"a\":\"1\" }", ParseError::Syntax),
        ("{ \"a\":\"1\"}", ParseError::Syntax),
        ("{\"a\":\"1\",}", ParseError::Syntax),
        ("{\"a\" \"1\"}", ParseError::Syntax),
        ("{\"a\":\"\\u0031\"}", ParseError::Canonical),
        ("{\"a\":\"\\/\"}", ParseError::Canonical),
        ("{\"a\":\"\\u0041\"}", ParseError::Canonical),
        ("{\"a\":\"\\u000A\"}", ParseError::Canonical),
        ("{\"a\":\"1\",\"a\":\"2\"}", ParseError::DuplicateKey),
        ("{\"a\":1}", ParseError::Number),
        ("{\"a\":-1}", ParseError::Number),
        ("{\"a\":+1}", ParseError::Number),
        ("{\"a\":.5}", ParseError::Number),
        ("[1]", ParseError::Number),
        ("[+1]", ParseError::Number),
        ("[.5]", ParseError::Number),
        ("{\"\\u0061\":\"1\"}", ParseError::Canonical),
        ("{\"a\":\"1\",\"\\u0061\":\"2\"}", ParseError::DuplicateKey),
        ("{\"a\":\"\\u00E9\"}", ParseError::Canonical),
        ("{\"a\":\"1\"", ParseError::Truncated),
        ("{", ParseError::Truncated),
        ("{\"a\":\"1\",", ParseError::Truncated),
        ("{\"a\":\"1\"}}", ParseError::Trailing),
        ("{}x", ParseError::Trailing),
        ("", ParseError::Truncated),
        ("{\"a\":\"\\x\"}", ParseError::Escape),
        ("{\"a\":\"\\u12\"}", ParseError::Escape),
        ("{\"a\":\"\\ud800\"}", ParseError::Surrogate),
        ("{\"a\":\"\\ud800\\ud800\"}", ParseError::Surrogate),
        ("{\"a\":\"\\ud800\\u0041\"}", ParseError::Surrogate),
        ("{\"a\":\"\\udc00\"}", ParseError::Surrogate),
        ("{\"a\":\"x\ny\"}", ParseError::Control),
        ("{\"a\":\"x\ty\"}", ParseError::Control),
        ("{\"a\":tru}", ParseError::Syntax),
        ("nul", ParseError::Truncated),
        ("nulx", ParseError::Syntax),
        ("true false", ParseError::Trailing),
        ("{\"a\":\"1\"} {}", ParseError::Trailing),
    ];
    for (input, expected) in cases {
        assert_eq!(
            decode(input.as_bytes(), input.len()),
            Err(*expected),
            "input {input:?}"
        );
    }
    assert_eq!(
        decode(&[b'{', b'"', b'a', b'"', b':', b'"', 0xff, b'"', b'}'], 64),
        Err(ParseError::Utf8)
    );
    // A UTF-8 BOM is not a value-position byte.
    let bom = "\u{feff}{\"a\":\"1\"}";
    assert_eq!(decode(bom.as_bytes(), bom.len()), Err(ParseError::Syntax));
}

#[test]
fn reader_enforces_byte_bound_and_depth() {
    let input = "{\"a\":\"1\"}";
    assert_eq!(
        decode(input.as_bytes(), input.len() - 1),
        Err(ParseError::Oversize)
    );
    assert!(decode(input.as_bytes(), input.len()).is_ok());

    let accepted = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
    assert!(decode(accepted.as_bytes(), accepted.len()).is_ok());
    let rejected = format!("{}{}", "[".repeat(MAX_DEPTH + 1), "]".repeat(MAX_DEPTH + 1));
    assert_eq!(
        decode(rejected.as_bytes(), rejected.len()),
        Err(ParseError::Depth)
    );

    // Objects nest as {"a":{"a":...null...}} and obey the same bound.
    let object_nest =
        |depth: usize| format!("{}null{}", "{\"a\":".repeat(depth), "}".repeat(depth));
    let accepted = object_nest(MAX_DEPTH);
    assert!(decode(accepted.as_bytes(), accepted.len()).is_ok());
    let rejected = object_nest(MAX_DEPTH + 1);
    assert_eq!(
        decode(rejected.as_bytes(), rejected.len()),
        Err(ParseError::Depth)
    );

    // Mixed object/array nesting counts every container toward the same bound.
    let mixed_nest = |depth: usize| {
        let mut open = String::new();
        let mut close = String::new();
        for index in 0..depth {
            if index % 2 == 0 {
                open.push('[');
                close.insert(0, ']');
            } else {
                open.push_str("{\"a\":");
                close.insert(0, '}');
            }
        }
        format!("{open}null{close}")
    };
    let accepted = mixed_nest(MAX_DEPTH);
    assert!(decode(accepted.as_bytes(), accepted.len()).is_ok());
    let rejected = mixed_nest(MAX_DEPTH + 1);
    assert_eq!(
        decode(rejected.as_bytes(), rejected.len()),
        Err(ParseError::Depth)
    );
}
