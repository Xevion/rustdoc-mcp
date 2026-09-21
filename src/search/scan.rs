//! Byte offsets of a JSON document's members, without materializing its values.
//!
//! Locating an item in rustdoc JSON costs a full parse of the enclosing crate, which
//! for `core.json` retains over a hundred megabytes for the life of the process. A
//! scan records where each member of one object begins and ends, so a single item can
//! be deserialized from its own byte range later.
//!
//! serde_json does the parsing. `RawValue` already reports each member's own text
//! without decoding it, so nothing here re-implements JSON.

// A standalone primitive that crate loading does not route through yet.
#![allow(dead_code)]

use serde_json::value::RawValue;
use std::collections::HashMap;
use std::ops::Range;

/// Where one member of a scanned object lives in the source bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Member {
    /// The member's key, with any escapes decoded.
    pub key: String,
    /// The value's text, with no surrounding whitespace.
    pub value: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ScanError {
    #[error("malformed JSON: {0}")]
    Malformed(String),
    #[error("no top-level key named `{key}`")]
    KeyNotFound { key: String },
    #[error("`{key}` is not an object")]
    NotAnObject { key: String },
    #[error("a member's text did not lie within the input")]
    RangeEscapedInput,
}

/// Byte ranges of every member of the top-level object member named `object_key`.
///
/// Members come back in document order. A repeated key keeps its last occurrence,
/// which is what any JSON reader does with one.
pub(crate) fn scan_object_members(json: &[u8], object_key: &str) -> Result<Vec<Member>, ScanError> {
    // Owned keys, because a borrowed one cannot be unescaped in place and so
    // fails outright on any key carrying an escape.
    let document: HashMap<String, &RawValue> =
        serde_json::from_slice(json).map_err(|error| ScanError::Malformed(error.to_string()))?;
    let target = document
        .get(object_key)
        .ok_or_else(|| ScanError::KeyNotFound {
            key: object_key.to_string(),
        })?;
    let members: HashMap<String, &RawValue> =
        serde_json::from_str(target.get()).map_err(|_| ScanError::NotAnObject {
            key: object_key.to_string(),
        })?;

    let base = json.as_ptr() as usize;
    let mut scanned = members
        .into_iter()
        .map(|(key, value)| {
            let text = value.get();
            // Recovering an offset from the borrow is not a documented guarantee,
            // so a value that does not lie inside the input is an error, never a
            // wild range handed on to a caller.
            let start = (text.as_ptr() as usize)
                .checked_sub(base)
                .filter(|start| start + text.len() <= json.len())
                .ok_or(ScanError::RangeEscapedInput)?;
            Ok(Member {
                key,
                value: start..start + text.len(),
            })
        })
        .collect::<Result<Vec<_>, ScanError>>()?;
    // A map forgets the order the document had; the offsets remember it.
    scanned.sort_unstable_by_key(|member| member.value.start);
    Ok(scanned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::check;
    use rstest::rstest;

    /// Key and value text of each member, for compact comparison in cases.
    fn members(json: &str) -> Result<Vec<(String, String)>, ScanError> {
        let b = json.as_bytes();
        scan_object_members(b, "index").map(|ms| {
            ms.into_iter()
                .map(|m| (m.key, String::from_utf8(b[m.value].to_vec()).unwrap()))
                .collect()
        })
    }

    fn ok(json: &str) -> Vec<(String, String)> {
        members(json).expect("should scan")
    }

    fn pairs(want: Vec<(&str, &str)>) -> Vec<(String, String)> {
        want.into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[rstest]
    #[case(r#"{"index":{}}"#, vec![])]
    #[case(r#"{"index":{"1":{}}}"#, vec![("1", "{}")])]
    #[case(r#"{"index":{"1":1}}"#, vec![("1", "1")])]
    #[case(r#"{"index":{"a":"b"}}"#, vec![("a", r#""b""#)])]
    #[case(r#"{"index":{"a":true,"b":false,"c":null}}"#, vec![("a","true"),("b","false"),("c","null")])]
    #[case(r#"{"index":{"a":[1,2,3]}}"#, vec![("a", "[1,2,3]")])]
    #[case(r#"{"index":{"a":{"b":{"c":[{}]}}}}"#, vec![("a", r#"{"b":{"c":[{}]}}"#)])]
    #[case(r#"{"index":{"a":1,"b":2}}"#, vec![("a","1"),("b","2")])]
    #[case(r#"{"paths":{"z":0},"index":{"a":1}}"#, vec![("a","1")])]
    #[case(r#"{"index":{"a":1},"paths":{"z":0}}"#, vec![("a","1")])]
    #[case(r#"{"index2":{"q":9},"index":{"a":1}}"#, vec![("a","1")])]
    fn scans_members(#[case] json: &str, #[case] want: Vec<(&str, &str)>) {
        check!(ok(json) == pairs(want));
    }

    /// Whitespace is legal anywhere between tokens and must not widen a value range.
    #[rstest]
    #[case("{ \"index\" : { \"a\" : 1 } }", vec![("a","1")])]
    #[case("{\n\"index\"\n:\n{\n\"a\"\n:\n1\n}\n}", vec![("a","1")])]
    #[case("{\t\"index\":\t{\t\"a\":\t1\t}\t}", vec![("a","1")])]
    #[case("{\r\n\"index\":{\"a\":\r\n1\r\n}}", vec![("a","1")])]
    #[case(r#"{"index":{"a":  [ 1 , 2 ]  }}"#, vec![("a","[ 1 , 2 ]")])]
    fn whitespace_does_not_leak_into_ranges(#[case] json: &str, #[case] want: Vec<(&str, &str)>) {
        check!(ok(json) == pairs(want));
    }

    /// String contents must never be read as structure.
    #[rstest]
    #[case(r#"{"index":{"a":"}"}}"#, r#""}""#)]
    #[case(r#"{"index":{"a":"{"}}"#, r#""{""#)]
    #[case(r#"{"index":{"a":"]["}}"#, r#""][""#)]
    #[case(r#"{"index":{"a":"a,b:c"}}"#, r#""a,b:c""#)]
    #[case(r#"{"index":{"a":"\""}}"#, r#""\"""#)]
    #[case(r#"{"index":{"a":"\\"}}"#, r#""\\""#)]
    #[case(r#"{"index":{"a":"\\\""}}"#, r#""\\\"""#)]
    #[case(r#"{"index":{"a":"\\\\"}}"#, r#""\\\\""#)]
    #[case(r#"{"index":{"a":"x\\"}}"#, r#""x\\""#)]
    #[case(r#"{"index":{"a":"\u0022"}}"#, r#""\u0022""#)]
    #[case(r#"{"index":{"a":"\u005C"}}"#, r#""\u005C""#)]
    #[case(r#"{"index":{"a":"\u007B\u007D"}}"#, r#""\u007B\u007D""#)]
    #[case(r#"{"index":{"a":"\n\t\r\b\f\/"}}"#, r#""\n\t\r\b\f\/""#)]
    #[case(r#"{"index":{"a":"\uD83D\uDE00"}}"#, r#""\uD83D\uDE00""#)]
    fn strings_are_opaque_to_structure(#[case] json: &str, #[case] want_value: &str) {
        let got = ok(json);
        check!(got.len() == 1);
        check!(got[0].1 == want_value);
    }

    /// A nested `index` key belongs to an inner object and must not be mistaken
    /// for the top-level one.
    #[rstest]
    #[case(r#"{"paths":{"index":{"wrong":1}},"index":{"right":1}}"#, "right")]
    #[case(r#"{"index":{"right":1},"paths":{"index":{"wrong":1}}}"#, "right")]
    #[case(r#"{"a":[{"index":{"wrong":1}}],"index":{"right":1}}"#, "right")]
    fn only_the_top_level_key_matches(#[case] json: &str, #[case] want_key: &str) {
        let got = ok(json);
        check!(got.len() == 1);
        check!(got[0].0 == want_key);
    }

    /// Numbers span exactly their own text.
    #[rstest]
    #[case("0")]
    #[case("-0")]
    #[case("1")]
    #[case("-1")]
    #[case("1.5")]
    #[case("-1.5")]
    #[case("1e10")]
    #[case("1E10")]
    #[case("1e+10")]
    #[case("1e-10")]
    #[case("1.5e10")]
    #[case("123456789012345678901234567890")]
    fn numbers_are_bounded_exactly(#[case] number: &str) {
        let json = format!(r#"{{"index":{{"a":{number}}}}}"#);
        check!(ok(&json)[0].1 == number);
    }

    /// Inputs that must be rejected rather than silently mis-scanned.
    #[rstest]
    #[case::empty("")]
    #[case::whitespace_only("   ")]
    #[case::top_level_array("[]")]
    #[case::top_level_number("1")]
    #[case::top_level_string(r#""index""#)]
    #[case::unclosed_top(r#"{"index":{"a":1}"#)]
    #[case::unclosed_string(r#"{"index":{"a":"x}}"#)]
    #[case::unterminated_escape(r#"{"index":{"a":"x\"}}"#)]
    #[case::missing_colon(r#"{"index"{"a":1}}"#)]
    #[case::missing_comma(r#"{"index":{"a":1"b":2}}"#)]
    #[case::trailing_comma_object(r#"{"index":{"a":1,}}"#)]
    #[case::trailing_comma_array(r#"{"index":{"a":[1,]}}"#)]
    #[case::leading_comma(r#"{"index":{,"a":1}}"#)]
    #[case::bare_key(r#"{index:{"a":1}}"#)]
    #[case::single_quotes("{'index':{'a':1}}")]
    #[case::trailing_data(r#"{"index":{"a":1}} trailing"#)]
    #[case::trailing_brace(r#"{"index":{"a":1}}}"#)]
    #[case::leading_zero(r#"{"index":{"a":01}}"#)]
    #[case::bare_decimal(r#"{"index":{"a":.5}}"#)]
    #[case::trailing_decimal(r#"{"index":{"a":1.}}"#)]
    #[case::plus_number(r#"{"index":{"a":+1}}"#)]
    #[case::nan(r#"{"index":{"a":NaN}}"#)]
    #[case::infinity(r#"{"index":{"a":Infinity}}"#)]
    #[case::truthy(r#"{"index":{"a":tru}}"#)]
    #[case::raw_newline_in_string("{\"index\":{\"a\":\"x\ny\"}}")]
    #[case::raw_tab_in_string("{\"index\":{\"a\":\"x\ty\"}}")]
    #[case::short_unicode_escape(r#"{"index":{"a":"\u12"}}"#)]
    #[case::bad_unicode_escape(r#"{"index":{"a":"\uZZZZ"}}"#)]
    #[case::unknown_escape(r#"{"index":{"a":"\x"}}"#)]
    #[case::value_missing(r#"{"index":{"a":}}"#)]
    #[case::key_missing(r#"{"index":{:1}}"#)]
    #[allow(clippy::literal_string_with_formatting_args)]
    fn malformed_input_is_rejected(#[case] json: &str) {
        check!(members(json).is_err(), "should reject {json:?}");
    }

    #[rstest]
    #[case::absent(r#"{"paths":{"a":1}}"#)]
    #[case::empty_doc("{}")]
    fn missing_key_is_reported(#[case] json: &str) {
        check!(let Err(ScanError::KeyNotFound { .. }) = members(json));
    }

    #[rstest]
    #[case(r#"{"index":[]}"#)]
    #[case(r#"{"index":1}"#)]
    #[case(r#"{"index":"x"}"#)]
    #[case(r#"{"index":null}"#)]
    fn non_object_target_is_reported(#[case] json: &str) {
        check!(let Err(ScanError::NotAnObject { .. }) = members(json));
    }

    /// Duplicate keys are legal JSON, and the last occurrence is the one that counts.
    #[test]
    fn duplicate_member_keys_keep_the_last() {
        check!(ok(r#"{"index":{"a":1,"a":2}}"#) == pairs(vec![("a", "2")]));
    }

    /// A repeated target key mirrors serde_json, where the last one wins.
    #[test]
    fn last_duplicate_target_wins() {
        check!(ok(r#"{"index":{"a":1},"index":{"b":2}}"#) == pairs(vec![("b", "2")]));
    }

    #[test]
    fn empty_key_is_a_valid_member() {
        check!(ok(r#"{"index":{"":1}}"#) == pairs(vec![("", "1")]));
    }

    /// An escaped key still names the target, as it would for any JSON reader.
    #[test]
    fn target_key_may_be_escaped() {
        check!(ok(r#"{"\u0069ndex":{"a":1}}"#) == pairs(vec![("a", "1")]));
    }

    #[test]
    fn multibyte_content_keeps_ranges_aligned() {
        check!(ok(r#"{"index":{"kéy":"välue ☃"}}"#) == pairs(vec![("kéy", r#""välue ☃""#)]));
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let scanned = scan_object_members(b"{\"index\":{\"a\":\"\xff\"}}", "index");
        check!(let Err(ScanError::Malformed(_)) = scanned);
    }

    /// Values are skipped without recursion, so depth is neither capped nor a
    /// stack hazard, even well past what a materializing parser would allow.
    #[rstest]
    #[case(100)]
    #[case(1_000)]
    #[case(100_000)]
    fn deep_nesting_is_handled_without_overflowing(#[case] depth: usize) {
        let deep = format!(
            r#"{{"index":{{"a":{}{}}}}}"#,
            "[".repeat(depth),
            "]".repeat(depth)
        );
        check!(members(&deep).is_ok());
    }

    /// Every truncation of a valid document must error, never panic.
    #[test]
    fn every_prefix_of_a_valid_document_is_handled() {
        let full = r#"{"index":{"1":{"a":[1,{"b":"x\"y"}],"c":null}},"paths":{"1":"z"}}"#;
        for cut in 0..full.len() {
            let _ = scan_object_members(&full.as_bytes()[..cut], "index");
        }
    }

    /// Every single-byte corruption must error or scan, never panic.
    #[test]
    fn single_byte_mutations_never_panic() {
        let full = r#"{"index":{"1":{"a":[1,{"b":"x\"y"}],"c":null}}}"#;
        for pos in 0..full.len() {
            for byte in [b'{', b'}', b'[', b']', b'"', b'\\', b':', b',', b'0', b' '] {
                let mut bytes = full.as_bytes().to_vec();
                bytes[pos] = byte;
                let _ = scan_object_members(&bytes, "index");
            }
        }
    }
}

/// Differential tests: serde_json decides what the scan is allowed to accept,
/// and where each value begins and ends.
#[cfg(test)]
mod fuzz {
    use super::*;
    use assert2::check;
    use proptest::prelude::*;
    use serde_json::{Map, Value};

    /// Strings deliberately admit quotes, backslashes, control characters and
    /// astral-plane text, since those drive every interesting escape path.
    fn arb_string() -> impl Strategy<Value = String> {
        prop::string::string_regex("(?s).{0,12}").unwrap()
    }

    fn arb_value() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            any::<f64>()
                .prop_filter("json has no non-finite numbers", |f| f.is_finite())
                .prop_map(Value::from),
            arb_string().prop_map(Value::String),
        ];
        leaf.prop_recursive(4, 24, 3, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..3).prop_map(Value::Array),
                prop::collection::hash_map(arb_string(), inner, 0..3)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        })
    }

    /// A document shaped like rustdoc's: a target object beside unrelated keys.
    fn arb_document() -> impl Strategy<Value = (String, Map<String, Value>)> {
        (
            prop::collection::hash_map(arb_string(), arb_value(), 0..4),
            prop::collection::hash_map(arb_string(), arb_value(), 0..2),
            any::<bool>(),
        )
            .prop_map(|(index, others, pretty)| {
                let mut root = Map::new();
                for (k, v) in others {
                    if k != "index" {
                        root.insert(k, v);
                    }
                }
                let index: Map<String, Value> = index.into_iter().collect();
                root.insert("index".to_string(), Value::Object(index.clone()));
                let text = if pretty {
                    serde_json::to_string_pretty(&Value::Object(root)).unwrap()
                } else {
                    serde_json::to_string(&Value::Object(root)).unwrap()
                };
                (text, index)
            })
    }

    proptest! {
        /// Every member's range must deserialize back to the value serde_json saw.
        ///
        /// The expectation comes from serde_json reading the same text, not from the
        /// value it was generated from: serde_json's float parsing does not agree
        /// with `str::parse`, and that difference is not the scan's to answer for.
        #[test]
        fn ranges_round_trip_through_serde_json((text, _) in arb_document()) {
            let bytes = text.as_bytes();
            let members = scan_object_members(bytes, "index").expect("valid document");
            let got: std::collections::HashMap<String, Value> = members
                .into_iter()
                .map(|m| {
                    let value = serde_json::from_slice(&bytes[m.value]).expect("valid slice");
                    (m.key, value)
                })
                .collect();
            let truth: Value = serde_json::from_slice(bytes).expect("valid document");
            let Value::Object(want) = &truth["index"] else {
                return Err(TestCaseError::fail("index is not an object"));
            };
            let want: std::collections::HashMap<String, Value> =
                want.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            prop_assert_eq!(got, want);
        }

        /// Arbitrary bytes must never panic, and must never be accepted unless
        /// serde_json agrees the document is valid JSON.
        #[test]
        fn arbitrary_bytes_are_never_accepted_wrongly(raw in prop::collection::vec(any::<u8>(), 0..256)) {
            if scan_object_members(&raw, "index").is_ok() {
                prop_assert!(serde_json::from_slice::<Value>(&raw).is_ok());
            }
        }

        /// A restricted alphabet reaches the structural paths that random bytes
        /// almost never hit. It carries no exponent, whose overflow is a
        /// disagreement about representability rather than about structure.
        #[test]
        fn json_shaped_soup_agrees_with_serde_json(
            text in prop::string::string_regex(r#"[\{\}\[\]",:\\0-9a-du \n\t.-]{0,120}"#).unwrap()
        ) {
            let bytes = text.as_bytes();
            let scanned = scan_object_members(bytes, "index");
            let parsed = serde_json::from_slice::<Value>(bytes);
            if scanned.is_ok() {
                prop_assert!(parsed.is_ok(), "accepted invalid JSON: {:?}", text);
            }
            // The converse: a valid document carrying an `index` object must scan.
            if let Ok(Value::Object(root)) = &parsed
                && let Some(Value::Object(_)) = root.get("index")
            {
                prop_assert!(scanned.is_ok(), "rejected valid document: {:?}", text);
            }
        }

        /// Truncating a valid document must error rather than mis-scan or panic.
        #[test]
        fn truncation_never_mis_scans((text, _) in arb_document(), cut in 0usize..400) {
            let bytes = text.as_bytes();
            let cut = cut.min(bytes.len());
            if !text.is_char_boundary(cut) {
                return Ok(());
            }
            if scan_object_members(&bytes[..cut], "index").is_ok() {
                // Accepting a prefix is only legal when it is the whole document.
                prop_assert_eq!(cut, bytes.len());
            }
        }
    }

    /// Inputs drawn from the classic failure modes of hand-written JSON readers.
    ///
    /// Each one is judged against serde_json rather than against a hand-written
    /// expectation, so the pair can never drift apart silently.
    #[rstest::rstest]
    #[case::form_feed_is_not_whitespace(b"{\"index\":{\"a\":\x0C1}}")]
    #[case::vertical_tab_is_not_whitespace(b"{\"index\":{\"a\":\x0B1}}")]
    #[case::raw_tab_in_string(b"{\"index\":{\"a\":\"x\ty\"}}")]
    #[case::raw_nul_in_string(b"{\"index\":{\"a\":\"x\x00y\"}}")]
    #[case::raw_control_in_string(b"{\"index\":{\"a\":\"x\x01y\"}}")]
    #[case::utf8_bom(b"\xEF\xBB\xBF{\"index\":{\"a\":1}}")]
    #[case::invalid_utf8_key(b"{\"index\":{\"\xFF\":1}}")]
    #[case::exponent_underflow(b"{\"index\":{\"a\":1e-400}}")]
    #[case::huge_integer(b"{\"index\":{\"a\":123456789012345678901234567890}}")]
    #[case::surrogate_pair(b"{\"index\":{\"a\":\"\\ud83d\\ude00\"}}")]
    #[case::trailing_backslash(b"{\"index\":{\"a\":\"\\\"}}")]
    #[case::three_backslashes(b"{\"index\":{\"a\":\"\\\\\\\"}}")]
    #[case::escaped_backslash_at_end(b"{\"index\":{\"a\":\"x\\\\\"}}")]
    #[case::braces_inside_strings(b"{\"index\":{\"a\":\"{{{[\",\"b\":\"]}}}\"}}")]
    #[case::non_object_members(
        b"{\"index\":{\"a\":null,\"b\":12.5e-3,\"c\":\"s\",\"d\":true,\"e\":[]}}"
    )]
    #[case::escaped_keys(b"{\"index\":{\"a\\\"b\":1,\"c\\\\d\":2}}")]
    #[case::trailing_content(b"{\"index\":{\"a\":1}} x")]
    #[case::leading_plus(b"{\"index\":{\"a\":+1}}")]
    #[case::leading_zero(b"{\"index\":{\"a\":01}}")]
    #[case::trailing_comma(b"{\"index\":{\"a\":1,}}")]
    #[case::duplicate_keys(b"{\"index\":{\"a\":1,\"a\":2}}")]
    #[case::empty_input(b"")]
    #[case::whitespace_only(b"   ")]
    #[case::truncated_string(b"{\"index\":{\"a\":\"abc")]
    fn agrees_with_serde_json_on_known_traps(#[case] input: &[u8]) {
        let scanned = scan_object_members(input, "index");
        let oracle = serde_json::from_slice::<Value>(input);
        match (&scanned, &oracle) {
            (Ok(_), Err(e)) => panic!("accepted what serde_json rejected: {e}"),
            (Err(e), Ok(_)) => panic!("rejected what serde_json accepted: {e}"),
            _ => {}
        }
        let (Ok(members), Ok(truth)) = (scanned, oracle) else {
            return;
        };
        let Value::Object(want) = &truth["index"] else {
            return;
        };
        check!(members.len() == want.len());
        // Ranges must march forward without overlapping, or a slice belongs to
        // some other member than the one it is filed under.
        for pair in members.windows(2) {
            check!(pair[0].value.end <= pair[1].value.start);
        }
        for member in &members {
            let got: Value =
                serde_json::from_slice(&input[member.value.clone()]).expect("slice parses");
            check!(
                want.get(&member.key) == Some(&got),
                "mismatch at {}",
                member.key
            );
        }
    }

    /// Well-formed JSON whose content serde_json will not turn into a value.
    ///
    /// A member is located by its structure, and its contents are read only when
    /// something asks for them. Each of these is bounded exactly and then fails as
    /// an ordinary parse error confined to that one member, which is the point:
    /// one unreadable item cannot spoil the ranges of its neighbours.
    #[rstest::rstest]
    #[case(b"{\"index\":{\"a\":1e400}}", b"1e400")]
    #[case(b"{\"index\":{\"a\":\"\\ud800\"}}", b"\"\\ud800\"")]
    #[case(b"{\"index\":{\"a\":\"\\udc00\"}}", b"\"\\udc00\"")]
    fn unrepresentable_content_is_bounded_but_not_judged(
        #[case] input: &[u8],
        #[case] want: &[u8],
    ) {
        let members = scan_object_members(input, "index").expect("structure is valid");
        check!(members.len() == 1);
        check!(&input[members[0].value.clone()] == want);
        check!(serde_json::from_slice::<Value>(input).is_err());
        check!(serde_json::from_slice::<Value>(&input[members[0].value.clone()]).is_err());
    }

    /// The real rustdoc JSON this exists for, checked value by value.
    #[rstest::rstest]
    #[case("std.json")]
    #[case("core.json")]
    #[case("alloc.json")]
    fn scans_real_rustdoc_json(#[case] file: &str) {
        let Some(path) = sysroot_json(file) else {
            eprintln!("no rust-docs-json component, skipping");
            return;
        };
        let bytes = std::fs::read(&path).expect("read sysroot json");
        let started = std::time::Instant::now();
        let members = scan_object_members(&bytes, "index").expect("scan rustdoc json");
        eprintln!(
            "{file}: {} bytes, {} members, scanned in {:?}",
            bytes.len(),
            members.len(),
            started.elapsed()
        );
        let truth: Value = serde_json::from_slice(&bytes).expect("parse rustdoc json");
        let Value::Object(truth) = &truth["index"] else {
            panic!("index is not an object");
        };

        check!(members.len() == truth.len());
        for member in &members {
            let scanned: Value =
                serde_json::from_slice(&bytes[member.value.clone()]).expect("member slice parses");
            check!(
                truth.get(&member.key) == Some(&scanned),
                "mismatch at {}",
                member.key
            );
        }

        // The whole point: one item materializes from its own range.
        let first = &members[0];
        let item: rustdoc_types::Item =
            serde_json::from_slice(&bytes[first.value.clone()]).expect("Item from range");
        check!(item.id.0.to_string() == first.key);
    }

    fn sysroot_json(file: &str) -> Option<std::path::PathBuf> {
        let output = std::process::Command::new("rustc")
            .args(["--print", "sysroot"])
            .output()
            .ok()?;
        let root = String::from_utf8(output.stdout).ok()?;
        let path = std::path::Path::new(root.trim())
            .join("share/doc/rust/json")
            .join(file);
        path.exists().then_some(path)
    }
}
