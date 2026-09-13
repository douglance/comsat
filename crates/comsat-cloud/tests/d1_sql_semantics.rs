#![cfg(not(target_arch = "wasm32"))]

#[path = "support/d1_sql.rs"]
mod d1_sql_support;
use d1_sql_support::*;
#[test]
fn source_watch_list_and_delete_use_shared_schema() -> rusqlite::Result<()> {
    let connection = database()?;
    upsert_source(&connection, "tenant-a", "github", true, 10)?;
    upsert_source(&connection, "tenant-a", "github", false, 20)?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    create_watch(&connection, "tenant-a", "watch-b", 20)?;
    assert_eq!(source_enabled(&connection, "tenant-a", "github")?, 0);
    assert_eq!(
        list_watch_ids(&connection, "tenant-a")?,
        vec!["watch-a", "watch-b"]
    );
    assert_eq!(delete_watch(&connection, "tenant-a", "watch-a")?, 1);
    assert_eq!(delete_watch(&connection, "tenant-a", "missing")?, 0);
    assert_eq!(list_watch_ids(&connection, "tenant-a")?, vec!["watch-b"]);
    Ok(())
}
#[test]
fn create_complete_and_history_are_tenant_scoped() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    create_watch(&connection, "tenant-b", "watch-b", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 40)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let outcome = complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &[record("github", "record-a", "https://example.com/a")],
            delivery_id: None,
        },
    )?;
    assert_eq!(outcome, CompleteOutcome::new(1, 1, 1, 1, 1));
    assert_eq!(
        history_for_tenant(&connection, "tenant-a")?,
        vec!["record-a"]
    );
    assert_eq!(
        history_for_watch(&connection, "tenant-a", "watch-a")?,
        vec!["record-a"]
    );
    assert!(history_for_tenant(&connection, "tenant-b")?.is_empty());
    assert_eq!(
        watch_next_due(&connection, "tenant-a", "watch-a")?,
        Some(90)
    );
    assert_eq!(watch_lease_owner(&connection, "tenant-a", "watch-a")?, None);
    Ok(())
}
#[test]
fn duplicate_start_uses_original_idempotent_run() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 40)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-1",
        "same-key",
        "worker-a",
        20,
    )?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-2",
        "same-key",
        "worker-a",
        20,
    )?;
    assert_eq!(
        idempotent_run_id(&connection, "tenant-a", "watch-a", "same-key")?,
        "run-1"
    );
    assert_eq!(watch_run_count(&connection, "tenant-a", "watch-a")?, 1);
    Ok(())
}
#[test]
fn stale_complete_after_lease_loss_writes_nothing() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 25)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let outcome = complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &[record("github", "record-a", "https://example.com/a")],
            delivery_id: None,
        },
    )?;
    assert_eq!(outcome, CompleteOutcome::new(0, 0, 0, 0, 0));
    assert!(history_for_tenant(&connection, "tenant-a")?.is_empty());
    assert_eq!(
        run_status(&connection, "tenant-a", "watch-a", "run-a")?,
        "running"
    );
    assert_eq!(
        watch_lease_owner(&connection, "tenant-a", "watch-a")?,
        Some("worker-a".into())
    );
    Ok(())
}
#[test]
fn failed_run_still_records_observations_and_clears_lease() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 80)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let outcome = complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "failed",
            error: Some("web timeout"),
            records: &[record("github", "record-a", "https://example.com/a")],
            delivery_id: None,
        },
    )?;
    assert_eq!(outcome, CompleteOutcome::new(1, 1, 1, 1, 1));
    assert_eq!(
        run_status(&connection, "tenant-a", "watch-a", "run-a")?,
        "failed"
    );
    assert_eq!(
        run_error(&connection, "tenant-a", "watch-a", "run-a")?,
        Some("web timeout".into())
    );
    assert_eq!(
        history_for_tenant(&connection, "tenant-a")?,
        vec!["record-a"]
    );
    assert_eq!(watch_lease_owner(&connection, "tenant-a", "watch-a")?, None);
    Ok(())
}

#[test]
fn forty_record_completion_uses_fixed_statement_count() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 80)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let records = (0..40)
        .map(|index| {
            record(
                "github",
                &format!("record-{index}"),
                &format!("https://example.com/{index}"),
            )
        })
        .collect::<Vec<_>>();

    let outcome = complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &records,
            delivery_id: None,
        },
    )?;

    assert_eq!(completion_statement_count(false), 7);
    assert_eq!(outcome, CompleteOutcome::new(1, 40, 40, 40, 40));
    assert_eq!(history_for_tenant(&connection, "tenant-a")?.len(), 40);
    Ok(())
}

#[test]
fn duplicate_records_count_only_new_rows() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 80)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let duplicate = record("github", "record-a", "https://example.com/a");
    let records = vec![duplicate.clone(), duplicate];

    let outcome = complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &records,
            delivery_id: None,
        },
    )?;

    assert_eq!(outcome, CompleteOutcome::new(1, 1, 1, 1, 1));
    assert_eq!(
        history_for_tenant(&connection, "tenant-a")?,
        vec!["record-a"]
    );
    Ok(())
}

#[test]
fn delivery_payload_contains_only_exact_new_watch_records() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 80)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let first = [record("github", "same", "https://example.com/same")];
    let outcome = complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &first,
            delivery_id: Some("delivery-a"),
        },
    )?;
    assert_eq!(
        outcome,
        CompleteOutcome::new(1, 1, 1, 1, 1).with_delivery(1)
    );

    claim_watch(&connection, "tenant-a", "worker-b", 90, 130)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-b",
        "idem-b",
        "worker-b",
        91,
    )?;
    let second = [
        record("github", "same", "https://example.com/same"),
        record("github", "new", "https://example.com/new"),
    ];
    complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-b",
            lease_owner: "worker-b",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &second,
            delivery_id: Some("delivery-b"),
        },
    )?;

    assert_eq!(
        delivery_payload_record_ids(&connection, "tenant-a", "delivery-a")?,
        vec!["same"]
    );
    assert_eq!(
        delivery_payload_record_ids(&connection, "tenant-a", "delivery-b")?,
        vec!["new"]
    );
    Ok(())
}

#[test]
fn stale_or_duplicate_delivery_completion_writes_no_outbox() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 25)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let records = [record("github", "record-a", "https://example.com/a")];
    let stale = complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &records,
            delivery_id: Some("stale-delivery"),
        },
    )?;
    assert_eq!(stale, CompleteOutcome::new(0, 0, 0, 0, 0));
    assert_eq!(delivery_count(&connection, "tenant-a")?, 0);

    create_watch(&connection, "tenant-b", "watch-b", 40)?;
    claim_watch(&connection, "tenant-b", "worker-b", 40, 80)?;
    start_run(
        &connection,
        "tenant-b",
        "watch-b",
        "run-b",
        "idem-b",
        "worker-b",
        41,
    )?;
    let duplicate = [
        record("github", "dup", "https://example.com/dup"),
        record("github", "dup", "https://example.com/dup"),
    ];
    let duplicate_outcome = complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-b",
            watch_id: "watch-b",
            run_id: "run-b",
            lease_owner: "worker-b",
            finished_at: 50,
            status: "succeeded",
            error: None,
            records: &duplicate,
            delivery_id: Some("duplicate-delivery"),
        },
    )?;
    assert_eq!(
        duplicate_outcome,
        CompleteOutcome::new(1, 1, 1, 1, 1).with_delivery(1)
    );
    assert_eq!(
        delivery_payload_record_ids(&connection, "tenant-b", "duplicate-delivery")?,
        vec!["dup"]
    );
    Ok(())
}

#[test]
fn expired_delivering_delivery_is_reclaimable_and_old_owner_cannot_complete() -> rusqlite::Result<()>
{
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 80)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let records = [record("github", "reclaim", "https://example.com/reclaim")];
    complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &records,
            delivery_id: Some("delivery-a"),
        },
    )?;

    assert_eq!(
        claim_delivery(&connection, "tenant-a", "old-worker", 40, 70)?,
        Some("delivery-a".into())
    );
    assert_eq!(
        claim_delivery(&connection, "tenant-a", "new-worker", 70, 130)?,
        Some("delivery-a".into())
    );
    assert!(
        complete_delivery(
            &connection,
            ("tenant-a", "delivery-a", "old-worker"),
            71,
            true,
            None
        )
        .is_err()
    );
    assert_eq!(
        delivery_status(&connection, "tenant-a", "delivery-a")?,
        "delivering"
    );
    assert_eq!(
        complete_delivery(
            &connection,
            ("tenant-a", "delivery-a", "new-worker"),
            72,
            true,
            None
        )?,
        "succeeded"
    );
    assert_eq!(
        delivery_status(&connection, "tenant-a", "delivery-a")?,
        "succeeded"
    );
    Ok(())
}

#[test]
fn delivery_retry_cooldown_and_attempt_exhaustion() -> rusqlite::Result<()> {
    let connection = database()?;
    create_watch(&connection, "tenant-a", "watch-a", 10)?;
    claim_watch(&connection, "tenant-a", "worker-a", 10, 80)?;
    start_run(
        &connection,
        "tenant-a",
        "watch-a",
        "run-a",
        "idem-a",
        "worker-a",
        20,
    )?;
    let records = [record("github", "record-a", "https://example.com/a")];
    complete_run(
        &connection,
        CompleteInput {
            tenant_id: "tenant-a",
            watch_id: "watch-a",
            run_id: "run-a",
            lease_owner: "worker-a",
            finished_at: 30,
            status: "succeeded",
            error: None,
            records: &records,
            delivery_id: Some("delivery-a"),
        },
    )?;

    assert_eq!(
        claim_delivery(&connection, "tenant-a", "sender", 30, 90)?,
        Some("delivery-a".into())
    );
    assert_eq!(
        complete_delivery(
            &connection,
            ("tenant-a", "delivery-a", "sender"),
            40,
            false,
            Some("timeout")
        )?,
        "pending"
    );
    assert_eq!(
        delivery_next_attempt(&connection, "tenant-a", "delivery-a")?,
        340
    );
    assert_eq!(
        claim_delivery(&connection, "tenant-a", "sender", 339, 399)?,
        None
    );

    assert_eq!(
        claim_delivery(&connection, "tenant-a", "sender", 340, 400)?,
        Some("delivery-a".into())
    );
    assert_eq!(
        complete_delivery(
            &connection,
            ("tenant-a", "delivery-a", "sender"),
            341,
            false,
            Some("timeout")
        )?,
        "pending"
    );
    assert_eq!(
        claim_delivery(&connection, "tenant-a", "sender", 641, 701)?,
        Some("delivery-a".into())
    );
    assert_eq!(
        complete_delivery(
            &connection,
            ("tenant-a", "delivery-a", "sender"),
            642,
            false,
            Some("timeout")
        )?,
        "failed"
    );
    assert_eq!(
        delivery_status(&connection, "tenant-a", "delivery-a")?,
        "failed"
    );
    assert_eq!(
        claim_delivery(&connection, "tenant-a", "sender", 942, 1002)?,
        None
    );
    Ok(())
}

#[test]
fn oversized_record_batch_fails_before_database_execution() {
    assert!(
        oversized_records_error_message()
            .contains("serialized D1 completion records exceed 1048576 bytes")
    );
}

#[test]
fn completion_batch_uses_seven_statements_independent_of_record_count() {
    assert_eq!(completion_statement_count(false), 7);
    assert_eq!(completion_statement_count(true), 8);
}
