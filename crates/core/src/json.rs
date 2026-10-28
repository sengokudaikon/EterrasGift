//! JSON objects whose keys people may type by hand: a known key is matched whatever its ASCII case.

use serde_json::{Map, Value};

/// The value of `key`, matched whatever its case; the first spelling when there are several.
pub fn find<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value)
}

/// Removes `key`, matched whatever its case, and returns its value; only the first spelling when there are several.
pub fn take(object: &mut Map<String, Value>, key: &str) -> Option<Value> {
    let name = object.keys().find(|name| name.eq_ignore_ascii_case(key))?.clone();
    object.remove(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_match_whatever_their_case_and_take_removes_one_spelling_at_a_time() {
        let mut object = serde_json::json!({ "AutoConnect": true, "autoconnect": false }).as_object().unwrap().clone();
        assert_eq!(find(&object, "autoConnect"), Some(&Value::Bool(true)));
        assert_eq!(take(&mut object, "AUTOCONNECT"), Some(Value::Bool(true)));
        assert_eq!(take(&mut object, "autoConnect"), Some(Value::Bool(false)));
        assert_eq!(take(&mut object, "autoConnect"), None);
    }
}
