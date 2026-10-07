//! `serde_json::Value` to and from Firestore `Value`.
//!
//! The `_at` rule (T-201b): any string whose object key ends in `_at` is
//! parsed as RFC 3339 and emitted as a Firestore `timestampValue`, so the TTL
//! backstop can fire. A parse failure is a bug (`BadResponse`).

use serde_json::{json, Map, Value};

use crate::gcp_http::GcpError;

/// Convert a top-level JSON object into Firestore `fields`.
///
/// # Errors
///
/// Returns `BadResponse` if the value is not a top-level object, or a string
/// under an `_at` key fails to parse as RFC 3339.
pub fn to_fields(v: &Value) -> Result<Map<String, Value>, GcpError> {
    let obj = v.as_object().ok_or(GcpError::BadResponse)?;
    let mut out = Map::new();
    for (k, val) in obj {
        out.insert(k.clone(), to_value(k, val)?);
    }
    Ok(out)
}

/// Convert Firestore `fields` back into a JSON object.
///
/// # Errors
///
/// Returns `BadResponse` on a malformed Firestore value.
pub fn from_fields(fields: &Map<String, Value>) -> Result<Value, GcpError> {
    let mut out = Map::new();
    for (k, val) in fields {
        out.insert(k.clone(), from_value(val)?);
    }
    Ok(Value::Object(out))
}

fn to_value(key: &str, v: &Value) -> Result<Value, GcpError> {
    match v {
        Value::Null => Ok(json!({ "nullValue": null })),
        Value::Bool(b) => Ok(json!({ "booleanValue": b })),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(json!({ "integerValue": i.to_string() }))
            } else {
                Ok(json!({ "doubleValue": n.as_f64().unwrap_or(0.0) }))
            }
        }
        Value::String(s) => {
            if key.ends_with("_at") {
                let dt =
                    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                        .map_err(|_| GcpError::BadResponse)?;
                let formatted = dt
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|_| GcpError::BadResponse)?;
                Ok(json!({ "timestampValue": formatted }))
            } else {
                Ok(json!({ "stringValue": s }))
            }
        }
        Value::Array(items) => {
            let values: Vec<Value> = items
                .iter()
                .map(|item| to_value(key, item))
                .collect::<Result<_, _>>()?;
            Ok(json!({ "arrayValue": { "values": values } }))
        }
        Value::Object(map) => {
            let fields = to_fields(&Value::Object(map.clone()))?;
            Ok(json!({ "mapValue": { "fields": fields } }))
        }
    }
}

fn from_value(v: &Value) -> Result<Value, GcpError> {
    if let Some(_) = v.get("nullValue") {
        return Ok(Value::Null);
    }
    if let Some(b) = v.get("booleanValue") {
        return Ok(Value::Bool(b.as_bool().ok_or(GcpError::BadResponse)?));
    }
    if let Some(s) = v.get("integerValue").and_then(|x| x.as_str()) {
        let n: i64 = s.parse().map_err(|_| GcpError::BadResponse)?;
        return Ok(Value::Number(n.into()));
    }
    if let Some(f) = v.get("doubleValue").and_then(serde_json::Value::as_f64) {
        return Ok(serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null));
    }
    if let Some(s) = v.get("timestampValue").and_then(|x| x.as_str()) {
        return Ok(Value::String(s.to_owned()));
    }
    if let Some(s) = v.get("stringValue").and_then(|x| x.as_str()) {
        return Ok(Value::String(s.to_owned()));
    }
    if let Some(arr) = v.get("arrayValue") {
        let values = arr
            .get("values")
            .and_then(|x| x.as_array())
            .ok_or(GcpError::BadResponse)?;
        let items = values
            .iter()
            .map(from_value)
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Value::Array(items));
    }
    if let Some(map) = v.get("mapValue") {
        let fields = map
            .get("fields")
            .and_then(|x| x.as_object())
            .ok_or(GcpError::BadResponse)?;
        return from_fields(fields);
    }
    Err(GcpError::BadResponse)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inv_2_ttl_fields_are_timestamp_values() {
        let v = json!({
            "name": "x",
            "expires_at": "2026-10-05T00:00:00Z",
            "nested": { "created_at": "2026-10-05T00:00:00Z" },
        });
        let fields = to_fields(&v).unwrap();
        assert!(fields["expires_at"].get("timestampValue").is_some());
        assert!(fields["name"].get("stringValue").is_some());
        assert!(fields["nested"]["mapValue"]["fields"]["created_at"]
            .get("timestampValue")
            .is_some());
    }

    #[test]
    fn inv_2_record_round_trips_unchanged() {
        let v = json!({
            "user_id": "123e4567-e89b-12d3-a456-426614174000",
            "created_at": "2026-10-05T00:00:00Z",
            "is_admin": false,
            "count": 3,
            "ratio": 1.5,
            "tags": ["a", "b"],
            "nested": { "x": null },
        });
        let fields = to_fields(&v).unwrap();
        let back = from_fields(&fields).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn integer_value_is_a_string_on_the_wire() {
        let v = json!({ "count": 3 });
        let fields = to_fields(&v).unwrap();
        assert_eq!(fields["count"]["integerValue"], "3");
    }

    #[test]
    fn bad_timestamp_is_bad_response() {
        let v = json!({ "expires_at": "not-a-time" });
        assert_eq!(to_fields(&v), Err(GcpError::BadResponse));
    }

    /// CFG-1: the `config/classifiers` document holds only its three fields.
    #[test]
    fn cfg_1_config_document_fields_on_the_wire() {
        let cfg = json!({
            "gemini_enabled": true,
            "jev_enabled": false,
            "updated_at": "2026-10-05T00:00:00Z",
        });
        let fields = to_fields(&cfg).unwrap();
        let keys: Vec<&String> = fields.keys().collect();
        assert_eq!(keys.len(), 3);
        assert!(fields.contains_key("gemini_enabled"));
        assert!(fields.contains_key("jev_enabled"));
        assert!(fields.contains_key("updated_at"));
        // updated_at is a timestamp so the TTL/ordering rules hold.
        assert!(fields["updated_at"].get("timestampValue").is_some());
    }

    proptest::proptest! {
        #[test]
        fn firestore_value_round_trip_property(v in arb_json()) {
            let fields = to_fields(&v).unwrap();
            let back = from_fields(&fields).unwrap();
            assert_eq!(back, v);
        }
    }

    fn arb_json() -> impl proptest::strategy::Strategy<Value = Value> {
        use proptest::prelude::*;
        // A string under an `_at` key must be a valid RFC 3339 timestamp
        // (the `_at` rule, T-201b), so every generated string is a canonical
        // timestamp. The field ranges are real (month 01-12, day 01-28, hour
        // 00-23 and so on): `[0-9]{2}` would let the generator build
        // `2026-13-45T25:00:00Z`, which `to_fields` refuses as a bad response
        // and the round trip never gets to see.
        let timestamp = "[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|1[0-9]|2[0-8])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9]Z"
            .prop_map(|s| Value::String(s.clone()));
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|i| Value::Number(i.into())),
            any::<f64>().prop_map(|f| {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }),
            timestamp,
        ];
        let tree = leaf.prop_recursive(4, 16, 4, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                proptest::collection::hash_map("[a-z_]{1,8}", inner, 0..4)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        });
        // `to_fields` requires a top-level object.
        proptest::collection::hash_map("[a-z_]{1,8}", tree, 1..4)
            .prop_map(|m| Value::Object(m.into_iter().collect()))
    }
}
