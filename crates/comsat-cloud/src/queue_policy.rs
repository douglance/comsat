use comsat_store::StoreError;

pub const fn should_retry(error: &StoreError) -> bool {
    match error {
        StoreError::Backend(_) => true,
        StoreError::Conflict(_) | StoreError::NotFound(_) | StoreError::Validation(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::should_retry;
    use comsat_store::StoreError;

    #[test]
    fn backend_outages_retry_while_obsolete_claims_do_not() {
        assert!(should_retry(&StoreError::Backend("D1 unavailable".into())));
        assert!(!should_retry(&StoreError::Conflict("lease expired".into())));
        assert!(!should_retry(&StoreError::NotFound("watch deleted".into())));
        assert!(!should_retry(&StoreError::Validation("invalid job".into())));
    }
}
