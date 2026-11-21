//! Shared helpers for the integration tests.
#![allow(dead_code)]

use serde_json::Value;

/// Reads `tests/data/{provider}/{name}` (synthetic fixtures; the repo ignores `fixtures/` dirs).
pub fn fixture(provider: &str, name: &str) -> String {
    let path = format!(
        "{}/tests/data/{provider}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Values real panels put where something else was expected.
fn odd_values() -> Vec<Value> {
    serde_json::from_str(
        r#"[null, "", "N/A", "-", [], {}, -1, 0, 1e300, true, "1e999", "١٢٣", "  ", "0x10"]"#,
    )
    .unwrap()
}

/// Calls `f` with the body re-serialized once per (leaf, odd value) pair,
/// plus once per removed object key, and with a few truncations.
pub fn for_each_type_mutation(json: &str, mut f: impl FnMut(String)) {
    let root: Value = serde_json::from_str(json).unwrap();
    let mut paths = Vec::new();
    collect_paths(&root, &mut Vec::new(), &mut paths);
    for path in &paths {
        for odd in odd_values() {
            let mut v = root.clone();
            *at(&mut v, path) = odd;
            f(v.to_string());
        }
        if let Some((PathPart::Key(key), parent)) = path.split_last() {
            let mut v = root.clone();
            if let Some(map) = at(&mut v, parent).as_object_mut() {
                map.remove(key);
            }
            f(v.to_string());
        }
    }
    for cut in [
        1,
        json.len() / 3,
        json.len() / 2,
        json.len().saturating_sub(2),
    ] {
        if let Some(prefix) = json.get(..cut) {
            f(prefix.to_owned());
        }
    }
}

fn collect_paths(v: &Value, current: &mut Vec<PathPart>, out: &mut Vec<Vec<PathPart>>) {
    out.push(current.clone());
    match v {
        Value::Array(a) => {
            for (i, item) in a.iter().enumerate() {
                current.push(PathPart::Index(i));
                collect_paths(item, current, out);
                current.pop();
            }
        }
        Value::Object(o) => {
            for (k, item) in o {
                current.push(PathPart::Key(k.clone()));
                collect_paths(item, current, out);
                current.pop();
            }
        }
        _ => {}
    }
}

#[derive(Clone)]
enum PathPart {
    Index(usize),
    Key(String),
}

fn at<'a>(v: &'a mut Value, path: &[PathPart]) -> &'a mut Value {
    path.iter().fold(v, |v, p| match p {
        PathPart::Index(i) => &mut v[*i],
        PathPart::Key(k) => &mut v[k.as_str()],
    })
}
