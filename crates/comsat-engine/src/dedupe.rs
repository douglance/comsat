use std::collections::BTreeSet;

use comsat_types::Record;
use serde_json::Value;

#[derive(Debug, Default, Clone)]
pub struct DedupeSet {
    seen: BTreeSet<String>,
}

impl DedupeSet {
    pub fn insert(&mut self, record: &Record) -> bool {
        let keys = dedupe_keys(record);
        if keys.iter().any(|key| self.seen.contains(key)) {
            return false;
        }
        self.seen.extend(keys);
        true
    }
}

fn dedupe_keys(record: &Record) -> Vec<String> {
    let mut keys = vec![
        format!("source:{}:{}", record.source.as_str(), record.id.as_str()),
        format!("url:{}", record.url),
    ];
    if let Some(native_id) = metadata_string(&record.metadata, "native_id") {
        keys.push(format!("native:{}:{native_id}", record.source.as_str()));
    }
    if let Some(native_id) = metadata_string(&record.metadata, "id") {
        keys.push(format!("native:{}:{native_id}", record.source.as_str()));
    }
    keys
}

fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}
