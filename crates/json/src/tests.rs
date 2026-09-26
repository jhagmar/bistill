use super::*;

fn val(text: &str) -> Value {
    parse(text.as_bytes()).unwrap_or_else(|err| panic!("{err}"))
}

fn bad(text: &str) -> Error {
    parse(text.as_bytes()).expect_err(text)
}

#[test]
fn parses_scalars_and_containers() {
    assert_eq!(val("null"), Value::Null);
    assert_eq!(val("true"), Value::Bool(true));
    assert_eq!(val("false"), Value::Bool(false));
    assert_eq!(val("\"hi\""), Value::String("hi".to_owned()));
    assert_eq!(val("0"), Value::Number("0".to_owned()));
    assert_eq!(val("-12"), Value::Number("-12".to_owned()));
    assert_eq!(val("1.50"), Value::Number("1.50".to_owned()));
    assert_eq!(val("1e+2"), Value::Number("1e+2".to_owned()));
    assert_eq!(val("8E-1"), Value::Number("8E-1".to_owned()));
    assert_eq!(val("[]"), Value::Array(vec![]));
    assert_eq!(val("[1, 2]"), Value::Array(vec![val("1"), val("2")]));
    assert_eq!(val("{}"), Value::Object(vec![]));
    assert_eq!(
        val("{\"b\":1,\"a\":2,\"b\":3}"),
        Value::Object(vec![("a".to_owned(), val("2")), ("b".to_owned(), val("3")),])
    );
    assert_eq!(val(" \n\t\rnull "), Value::Null);
}

#[test]
fn string_escapes_and_surrogates() {
    assert_eq!(
        val(r#""\"\\\/\b\f\n\r\t""#).as_str(),
        Some("\"\\/\u{0008}\u{000c}\n\r\t")
    );
    assert_eq!(val(r#""\u0041""#).as_str(), Some("A"));
    assert_eq!(val(r#""\uD83D\uDE00""#).as_str(), Some("😀"));
    assert_eq!(val("\"é\"").as_str(), Some("é"));
    assert!(bad(r#""\uD800""#).message.contains("lone surrogate"));
    assert!(bad(r#""\uDEAD""#).message.contains("lone surrogate"));
    assert!(bad(r#""\uD800\uD800""#).message.contains("surrogate"));
    assert!(bad(r#""\uD800\""#).message.contains("lone surrogate"));
    assert!(bad("\"\\u12").message.contains("truncated unicode"));
    assert!(bad(r#""\u12GX""#).message.contains("invalid unicode"));
    assert!(bad(r#""\"#).message.contains("truncated escape"));
    assert!(bad(r#""\q""#).message.contains("invalid escape"));
    assert!(bad("\"\u{0001}\"").message.contains("control"));
    assert!(bad("\"abc").message.contains("unterminated"));
}

#[test]
fn rejects_invalid_text() {
    assert!(bad("").message.contains("expected a value"));
    assert!(bad("n").message.contains("expected a value"));
    assert!(bad("nullx").message.contains("expected a value"));
    assert!(bad("+1").message.contains("expected a value"));
    assert!(bad("-").message.contains("invalid number"));
    assert!(bad("1.").message.contains("invalid number"));
    assert!(bad("1e").message.contains("invalid number"));
    assert!(bad("1e+").message.contains("invalid number"));
    assert!(bad("01").message.contains("trailing"));
    assert!(bad("1 2").message.contains("trailing"));
    assert!(bad("//").message.contains("expected a value"));
    assert!(bad("[1,]").message.contains("expected a value"));
    assert!(bad("[1").message.contains("expected ',' or ']'"));
    assert!(bad("[1 2]").message.contains("expected ',' or ']'"));
    assert!(bad("{1:2}").message.contains("expected a string"));
    assert!(bad("{\"a\" 1}").message.contains("expected ':'"));
    assert!(bad("{\"a\":1,}").message.contains("expected a string"));
    assert!(bad("{\"a\":1").message.contains("expected ',' or '}'"));
    let err = bad("[\n  ,]");
    assert_eq!((err.line, err.column), (2, 3));
    assert!(err.to_string().contains("line 2"));
    let _dyn: &dyn std::error::Error = &err;
}

#[test]
fn limits_and_utf8() {
    let mut deep = "[".repeat(128);
    deep.push('0');
    deep.push_str(&"]".repeat(128));
    assert!(parse(deep.as_bytes()).is_ok());
    let mut deeper = "[".repeat(129);
    deeper.push('0');
    deeper.push_str(&"]".repeat(129));
    assert!(bad(&deeper).message.contains("nesting"));
    let mut deep_obj = String::new();
    for _ in 0..129 {
        deep_obj.push_str("{\"a\":");
    }
    deep_obj.push('0');
    deep_obj.push_str(&"}".repeat(129));
    assert!(bad(&deep_obj).message.contains("nesting"));

    let mut huge = vec![b'0'; MAX_BYTES + 1];
    huge[0] = b'1';
    let err = parse(&huge).unwrap_err();
    assert_eq!(err.offset, 0);
    assert!(err.message.contains("8 MiB"));

    let err = parse(b"\n\xff").unwrap_err();
    assert_eq!((err.line, err.column, err.offset), (2, 1, 1));
    assert!(err.message.contains("UTF-8"));
    let err = parse(b" [\xff").unwrap_err();
    assert_eq!((err.line, err.column, err.offset), (1, 3, 2));
    let err = parse(&[0xff]).unwrap_err();
    assert_eq!((err.line, err.column), (1, 1));
}

#[test]
fn writes_compact_text() {
    let cases = [
        ("null", "null"),
        ("true", "true"),
        ("false", "false"),
        (" 1.0 ", "1.0"),
        ("[1, 2]", "[1,2]"),
        ("{\"b\":1,\"a\":2}", "{\"b\":1,\"a\":2}"),
        (r#""a\"b\\c""#, r#""a\"b\\c""#),
    ];
    for (text, compact) in cases {
        let value = val(text);
        assert_eq!(to_vec(&value), compact.as_bytes());
        assert_eq!(parse(&to_vec(&value)).unwrap(), value);
    }
    let fancy = Value::String("\u{0001}\u{0008}\u{000c}\n\r\t\"\\é".to_owned());
    assert_eq!(
        String::from_utf8(to_vec(&fancy)).unwrap(),
        "\"\\u0001\\b\\f\\n\\r\\t\\\"\\\\é\""
    );
    assert_eq!(to_vec(&Value::Array(vec![])), b"[]");
    assert_eq!(to_vec(&Value::Object(vec![])), b"{}");
    assert_eq!(to_vec(&Value::Bool(false)), b"false");
}

#[test]
fn getters_and_integers() {
    let object = val(r#"{"n":1.0,"s":"a","a":[true],"z":0e-1}"#);
    assert_eq!(object.get("n").and_then(Value::as_i64), Some(1));
    assert_eq!(object.get("n").and_then(Value::as_u64), Some(1));
    assert_eq!(object.get("n").and_then(Value::as_f64), Some(1.0));
    assert_eq!(object.get("n").and_then(Value::as_number), Some("1.0"));
    assert_eq!(object.get("s").and_then(Value::as_str), Some("a"));
    assert_eq!(
        object
            .get("a")
            .and_then(|v| v.get_index(0))
            .and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(object.get("z").and_then(Value::as_i64), Some(0));
    assert!(object.get("missing").is_none());
    assert!(object.get_index(0).is_none());
    assert!(object.as_bool().is_none());
    assert!(Value::Null.as_number().is_none());
    assert!(Value::Null.as_str().is_none());
    assert!(Value::Null.as_array().is_none());
    assert!(Value::Null.as_object().is_none());
    assert!(Value::Bool(true).as_i64().is_none());
    assert!(val("[1]").get_index(1).is_none());
    assert_eq!(val("10e-1").as_i64(), Some(1));
    assert_eq!(val("1e2").as_u64(), Some(100));
    assert_eq!(val("1e+2").as_i64(), Some(100));
    assert!(val("1e-2").as_i64().is_none());
    assert!(val("1.5").as_i64().is_none());
    assert!(val("-1").as_u64().is_none());
    assert_eq!(val("-0").as_u64(), Some(0));
    assert_eq!(val("9223372036854775807").as_i64(), Some(i64::MAX));
    assert!(val("9223372036854775808").as_i64().is_none());
    assert_eq!(val("-9223372036854775808").as_i64(), Some(i64::MIN));
    assert!(val("-9223372036854775809").as_i64().is_none());
    assert_eq!(val("18446744073709551615").as_u64(), Some(u64::MAX));
    assert!(val("18446744073709551616").as_u64().is_none());
    assert_eq!(val("1e309").as_number(), Some("1e309"));
    assert!(val("1e309").as_f64().is_none());
    assert!(val("1e309").as_i64().is_none());
    assert_eq!(format!("{:?}", Value::Null.clone()), "Null");
    assert_eq!(format!("{:?}", bad("x").clone()), format!("{:?}", bad("x")));
}

#[test]
fn integer_text_that_is_not_a_json_number() {
    let long = "9".repeat(39);
    let cases = [
        "",
        "-",
        "1.",
        "01",
        "+1",
        "1.2a",
        "1e",
        "1e+",
        "1e-",
        "1e9999999999",
        "1.0e2147483647",
        long.as_str(),
    ];
    for text in cases {
        assert!(Value::Number(text.to_owned()).as_i64().is_none(), "{text}");
    }
    assert!(Value::Number("nope".to_owned()).as_f64().is_none());
}
