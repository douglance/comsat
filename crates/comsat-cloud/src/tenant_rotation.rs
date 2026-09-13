/// Bound empty-tenant probes as well as successful claims. Rotate the first
/// tenant each scheduled window so idle tenants cannot starve later tenants.
pub fn scheduled_tenants(tenants: &[String], now: i64, limit: usize) -> Vec<&str> {
    if tenants.is_empty() {
        return Vec::new();
    }
    let window = usize::try_from(now.max(0) / 300).unwrap_or_default();
    tenants
        .iter()
        .cycle()
        .skip(window % tenants.len())
        .take(limit.min(tenants.len()))
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::scheduled_tenants;

    #[test]
    fn bounds_empty_probes_and_rotates_without_duplicates() {
        let tenants = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(scheduled_tenants(&tenants, 0, 2), ["a", "b"]);
        assert_eq!(scheduled_tenants(&tenants, 300, 2), ["b", "c"]);
        assert_eq!(scheduled_tenants(&tenants, 600, 2), ["c", "a"]);
        assert_eq!(scheduled_tenants(&tenants, 600, 9), ["c", "a", "b"]);
        assert!(scheduled_tenants(&[], 0, 2).is_empty());
        assert!(scheduled_tenants(&tenants, 0, 0).is_empty());
    }
}
