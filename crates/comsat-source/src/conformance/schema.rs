use std::collections::BTreeSet;

use serde_json::Value;

use super::ConformanceError;

/// Keywords whose value is a single schema. Draft 2019-09 also allows an array
/// of schemas for `items`, so single positions accept either shape.
const SINGLE_KEYWORDS: &[&str] = &[
    "additionalItems",
    "additionalProperties",
    "contains",
    "contentSchema",
    "else",
    "if",
    "items",
    "not",
    "propertyNames",
    "then",
    "unevaluatedItems",
    "unevaluatedProperties",
];

/// Keywords whose value is an array of schemas.
const ARRAY_KEYWORDS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];

/// Keywords whose value maps names to schemas.
const MAP_KEYWORDS: &[&str] = &[
    "$defs",
    "definitions",
    "dependentSchemas",
    "patternProperties",
    "properties",
];

/// Keywords that carry local references.
const REFERENCE_KEYWORDS: &[&str] = &["$ref", "$dynamicRef"];

/// Keywords that declare a plain-name fragment.
const ANCHOR_KEYWORDS: &[&str] = &["$anchor", "$dynamicAnchor"];

enum Position {
    Single,
    Array,
    Map,
}

/// Checks that a tool schema is internally consistent: every schema position
/// holds a schema, `$schema` appears only on a resource root, and every local
/// reference resolves inside its own resource.
///
/// Only known schema keywords are traversed, so data positions such as `const`,
/// `default`, `enum`, and `examples` are left alone.
pub(super) fn validate_schema_document(
    tool: &str,
    document: &Value,
) -> Result<(), ConformanceError> {
    walk(tool, document, &Resource::new(document), true)
}

/// One schema resource: the document a local reference resolves against.
struct Resource<'a> {
    root: &'a Value,
    anchors: BTreeSet<String>,
}

impl<'a> Resource<'a> {
    fn new(root: &'a Value) -> Self {
        let mut anchors = BTreeSet::new();
        collect_anchors(root, true, &mut anchors);
        Self { root, anchors }
    }

    /// Remote references are out of scope; only `#`-prefixed fragments are resolved.
    fn resolves(&self, reference: &str) -> bool {
        let Some(fragment) = reference.strip_prefix('#') else {
            return true;
        };
        if fragment.is_empty() {
            return true;
        }
        if fragment.starts_with('/') {
            return self.root.pointer(fragment).is_some();
        }
        self.anchors.contains(fragment)
    }
}

fn walk(
    tool: &str,
    node: &Value,
    resource: &Resource,
    is_resource_root: bool,
) -> Result<(), ConformanceError> {
    if node.is_boolean() {
        return Ok(());
    }
    let Some(map) = node.as_object() else {
        return Err(invalid(
            tool,
            "schema position must be an object or boolean",
        ));
    };
    if !is_resource_root && map.contains_key("$id") {
        return walk_members(tool, node, &Resource::new(node));
    }
    if !is_resource_root && map.contains_key("$schema") {
        return Err(invalid(tool, "nested $schema outside a resource root"));
    }
    walk_members(tool, node, resource)
}

fn walk_members(tool: &str, node: &Value, resource: &Resource) -> Result<(), ConformanceError> {
    let map = node.as_object().expect("checked by caller");
    check_references(tool, node, resource)?;
    for (keyword, value) in map {
        walk_member(tool, keyword, value, resource)?;
    }
    Ok(())
}

fn walk_member(
    tool: &str,
    keyword: &str,
    value: &Value,
    resource: &Resource,
) -> Result<(), ConformanceError> {
    let Some(position) = position(keyword) else {
        return Ok(());
    };
    for child in children(&position, value).map_err(|reason| invalid(tool, reason))? {
        walk(tool, child, resource, false)?;
    }
    Ok(())
}

fn check_references(tool: &str, node: &Value, resource: &Resource) -> Result<(), ConformanceError> {
    for keyword in REFERENCE_KEYWORDS {
        let Some(value) = node.get(keyword) else {
            continue;
        };
        let Some(reference) = value.as_str() else {
            return Err(invalid(tool, format!("{keyword} must be a string")));
        };
        if !resource.resolves(reference) {
            return Err(invalid(tool, format!("unresolved local ref {reference}")));
        }
    }
    Ok(())
}

fn collect_anchors(node: &Value, is_resource_root: bool, anchors: &mut BTreeSet<String>) {
    let Some(map) = node.as_object() else {
        return;
    };
    if !is_resource_root && map.contains_key("$id") {
        return;
    }
    for keyword in ANCHOR_KEYWORDS {
        if let Some(anchor) = node.get(keyword).and_then(Value::as_str) {
            anchors.insert(anchor.to_owned());
        }
    }
    for (keyword, value) in map {
        collect_member_anchors(keyword, value, anchors);
    }
}

fn collect_member_anchors(keyword: &str, value: &Value, anchors: &mut BTreeSet<String>) {
    let Some(position) = position(keyword) else {
        return;
    };
    for child in children(&position, value).unwrap_or_default() {
        collect_anchors(child, false, anchors);
    }
}

fn position(keyword: &str) -> Option<Position> {
    if SINGLE_KEYWORDS.contains(&keyword) {
        return Some(Position::Single);
    }
    if ARRAY_KEYWORDS.contains(&keyword) {
        return Some(Position::Array);
    }
    if MAP_KEYWORDS.contains(&keyword) {
        return Some(Position::Map);
    }
    None
}

fn children<'a>(position: &Position, value: &'a Value) -> Result<Vec<&'a Value>, &'static str> {
    match position {
        Position::Single if !value.is_array() => Ok(vec![value]),
        Position::Single | Position::Array => value
            .as_array()
            .map(|values| values.iter().collect())
            .ok_or("schema array position must hold an array"),
        Position::Map => value
            .as_object()
            .map(|entries| entries.values().collect())
            .ok_or("schema map position must hold an object"),
    }
}

fn invalid(tool: &str, reason: impl Into<String>) -> ConformanceError {
    ConformanceError::InvalidToolSchema {
        tool: tool.to_owned(),
        reason: reason.into(),
    }
}
