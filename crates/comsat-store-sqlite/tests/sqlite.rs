use comsat_store::{
    ClaimDueWatch, ClaimPendingDelivery, CompleteDelivery, CompleteWatchRun, CreateWatch,
    DeleteWatch, HistoryRequest, ListDeliveries, StartWatchRun, Store, WatchRunDelivery,
    empty_object,
};
use comsat_store_sqlite::SqliteStore;
use comsat_types::{Query, Record, RecordId, SourceId};
use serde_json::json;

#[test]
fn opens_legacy_0001_database_and_second_open_preserves_delivery_columns() {
    let path = legacy_database_path();
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!("../../../migrations/0001_initial.sql"))
            .unwrap();
    }

    drop(SqliteStore::open(&path).unwrap());
    drop(SqliteStore::open(&path).unwrap());

    let connection = rusqlite::Connection::open(&path).unwrap();
    let columns = delivery_columns(&connection);
    assert!(columns.iter().any(|column| column == "payload_json"));
    assert!(
        columns
            .iter()
            .any(|column| column == "next_attempt_epoch_seconds")
    );
    assert!(columns.iter().any(|column| column == "attempts"));
    assert_eq!(migration_version_count(&connection), 1);
    let _ = std::fs::remove_file(path);
}

fn legacy_database_path() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "comsat-legacy-sqlite-{}-{nanos}.db",
        std::process::id()
    ))
}

fn delivery_columns(connection: &rusqlite::Connection) -> Vec<String> {
    let mut statement = connection.prepare("PRAGMA table_info(deliveries)").unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn migration_version_count(connection: &rusqlite::Connection) -> i64 {
    connection
        .query_row(
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 2",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn history_is_tenant_isolated() {
    let store = SqliteStore::in_memory().unwrap();
    run_once(&store, "tenant-a", "watch", "owner-a", record("one")).await;
    run_once(&store, "tenant-b", "watch", "owner-b", record("two")).await;

    let a_history = store
        .history(HistoryRequest {
            tenant_id: "tenant-a".into(),
            watch_id: None,
            since_epoch_seconds: None,
            limit: 20,
        })
        .await
        .unwrap();

    assert_eq!(a_history.len(), 1);
    assert_eq!(a_history[0].record.id.as_str(), "one");
}

#[tokio::test]
async fn repeating_a_watch_run_does_not_duplicate_observations() {
    let store = SqliteStore::in_memory().unwrap();
    run_once(&store, "tenant", "watch", "owner", record("same")).await;

    create_watch(&store, "tenant", "watch-2", 30).await;
    claim_and_start(
        &store,
        ClaimStart::new(("tenant", "watch-2", "owner-2", "run-2", "idem-2"), 30),
    )
    .await;
    let second = store
        .complete_watch_run(CompleteWatchRun {
            tenant_id: "tenant".into(),
            watch_id: "watch-2".into(),
            run_id: "run-2".into(),
            lease_owner: "owner-2".into(),
            finished_at_epoch_seconds: 40,
            records: vec![record("same")],
            next_cursor: empty_object(),
            error: None,
            delivery: None,
        })
        .await
        .unwrap();

    assert_eq!(second.records_inserted, 0);
    assert_eq!(second.watch_records_inserted, 1);
}

#[tokio::test]
async fn due_claims_obey_interval_and_lease_fencing() {
    let store = SqliteStore::in_memory().unwrap();
    create_watch(&store, "tenant", "watch", 100).await;

    assert!(
        store
            .claim_due_watch(ClaimDueWatch {
                tenant_id: "tenant".into(),
                now_epoch_seconds: 99,
                lease_owner: "early".into(),
                lease_seconds: 30,
            })
            .await
            .unwrap()
            .is_none()
    );

    let claim = claim_and_start(
        &store,
        ClaimStart::new(("tenant", "watch", "owner", "run", "idem"), 100),
    )
    .await;
    assert_eq!(claim.lease_expires_epoch_seconds, 130);
    assert!(
        store
            .start_watch_run(StartWatchRun {
                tenant_id: "tenant".into(),
                watch_id: "watch".into(),
                run_id: "stale-run".into(),
                idempotency_key: "stale-idem".into(),
                lease_owner: "other".into(),
                started_at_epoch_seconds: 101,
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stale_worker_cannot_complete_or_clear_newer_lease() {
    let store = SqliteStore::in_memory().unwrap();
    create_watch(&store, "tenant", "watch", 10).await;
    claim_and_start(
        &store,
        ClaimStart::new(("tenant", "watch", "old-owner", "old-run", "old-idem"), 10),
    )
    .await;
    claim_and_start(
        &store,
        ClaimStart::new(("tenant", "watch", "new-owner", "new-run", "new-idem"), 50),
    )
    .await;

    let stale = store
        .complete_watch_run(CompleteWatchRun {
            tenant_id: "tenant".into(),
            watch_id: "watch".into(),
            run_id: "old-run".into(),
            lease_owner: "old-owner".into(),
            finished_at_epoch_seconds: 51,
            records: vec![record("stale")],
            next_cursor: empty_object(),
            error: None,
            delivery: None,
        })
        .await;
    assert!(stale.is_err());

    let history = store
        .history(HistoryRequest {
            tenant_id: "tenant".into(),
            watch_id: None,
            since_epoch_seconds: None,
            limit: 20,
        })
        .await
        .unwrap();
    assert!(history.is_empty());
}

#[tokio::test]
async fn deleting_a_watch_cascades_watch_history_links() {
    let store = SqliteStore::in_memory().unwrap();
    run_once(&store, "tenant", "watch", "owner", record("deleted-link")).await;

    assert!(
        store
            .delete_watch(DeleteWatch {
                tenant_id: "tenant".into(),
                watch_id: "watch".into(),
            })
            .await
            .unwrap()
    );
    let watch_history = store
        .history(HistoryRequest {
            tenant_id: "tenant".into(),
            watch_id: Some("watch".into()),
            since_epoch_seconds: None,
            limit: 20,
        })
        .await
        .unwrap();
    let tenant_history = store
        .history(HistoryRequest {
            tenant_id: "tenant".into(),
            watch_id: None,
            since_epoch_seconds: None,
            limit: 20,
        })
        .await
        .unwrap();

    assert!(watch_history.is_empty());
    assert_eq!(tenant_history.len(), 1);
}

#[tokio::test]
async fn completion_delivery_is_idempotent_and_tenant_isolated() {
    let store = SqliteStore::in_memory().unwrap();
    complete_with_delivery(
        &store,
        CompletionSeed::new(("tenant-a", "watch-a", "owner-a", "run-a", "idem-a"), 10)
            .records(vec![record("a-one")])
            .delivery_id("delivery-1"),
    )
    .await;
    complete_with_delivery(
        &store,
        CompletionSeed::new(("tenant-a", "watch-b", "owner-b", "run-b", "idem-b"), 20)
            .records(vec![record("a-two")])
            .delivery_id("delivery-1"),
    )
    .await;
    complete_with_delivery(
        &store,
        CompletionSeed::new(("tenant-b", "watch-a", "owner-c", "run-c", "idem-c"), 10)
            .records(vec![record("b-one")])
            .delivery_id("delivery-1"),
    )
    .await;

    let tenant_a = pending_deliveries(&store, "tenant-a").await;
    let tenant_b = pending_deliveries(&store, "tenant-b").await;

    assert_eq!(tenant_a.len(), 1);
    assert_eq!(tenant_a[0].delivery_id, "delivery-1");
    assert_eq!(tenant_a[0].watch_id, "watch-a");
    assert_eq!(payload_record_ids(&tenant_a[0]), vec!["a-one"]);
    assert_eq!(tenant_b.len(), 1);
    assert_eq!(tenant_b[0].delivery_id, "delivery-1");
    assert_eq!(payload_record_ids(&tenant_b[0]), vec!["b-one"]);
}

#[tokio::test]
async fn same_second_second_run_delivery_contains_only_new_watch_records() {
    let store = SqliteStore::in_memory().unwrap();
    create_watch(&store, "tenant", "watch", 1).await;
    claim_and_start(
        &store,
        ClaimStart::new(("tenant", "watch", "owner-1", "run-1", "idem-1"), 1),
    )
    .await;
    store
        .complete_watch_run(CompleteWatchRun {
            tenant_id: "tenant".into(),
            watch_id: "watch".into(),
            run_id: "run-1".into(),
            lease_owner: "owner-1".into(),
            finished_at_epoch_seconds: 2,
            records: vec![record("same")],
            next_cursor: empty_object(),
            error: None,
            delivery: Some(watch_delivery("first-delivery")),
        })
        .await
        .unwrap();
    claim_and_start(
        &store,
        ClaimStart::new(("tenant", "watch", "owner-2", "run-2", "idem-2"), 62),
    )
    .await;
    store
        .complete_watch_run(CompleteWatchRun {
            tenant_id: "tenant".into(),
            watch_id: "watch".into(),
            run_id: "run-2".into(),
            lease_owner: "owner-2".into(),
            finished_at_epoch_seconds: 2,
            records: vec![record("same"), record("new")],
            next_cursor: empty_object(),
            error: None,
            delivery: Some(watch_delivery("second-delivery")),
        })
        .await
        .unwrap();

    let deliveries = pending_deliveries(&store, "tenant").await;
    assert_eq!(deliveries.len(), 2);
    assert_eq!(payload_record_ids(&deliveries[0]), vec!["same"]);
    assert_eq!(payload_record_ids(&deliveries[1]), vec!["new"]);
}

#[tokio::test]
async fn completion_without_new_records_or_valid_lease_does_not_insert_delivery() {
    let store = SqliteStore::in_memory().unwrap();
    run_once(&store, "tenant", "watch-a", "owner-a", record("same")).await;
    claim_and_start(
        &store,
        ClaimStart::new(("tenant", "watch-a", "owner-b", "run-b", "idem-b"), 62),
    )
    .await;
    store
        .complete_watch_run(CompleteWatchRun {
            tenant_id: "tenant".into(),
            watch_id: "watch-a".into(),
            run_id: "run-b".into(),
            lease_owner: "owner-b".into(),
            finished_at_epoch_seconds: 63,
            records: vec![record("same")],
            next_cursor: empty_object(),
            error: None,
            delivery: Some(watch_delivery("no-new-records")),
        })
        .await
        .unwrap();

    create_watch(&store, "tenant", "watch-c", 50).await;
    claim_and_start(
        &store,
        ClaimStart::new(
            ("tenant", "watch-c", "old-owner", "old-run", "old-idem"),
            50,
        ),
    )
    .await;
    claim_and_start(
        &store,
        ClaimStart::new(
            ("tenant", "watch-c", "new-owner", "new-run", "new-idem"),
            90,
        ),
    )
    .await;
    assert!(
        store
            .complete_watch_run(CompleteWatchRun {
                tenant_id: "tenant".into(),
                watch_id: "watch-c".into(),
                run_id: "old-run".into(),
                lease_owner: "old-owner".into(),
                finished_at_epoch_seconds: 91,
                records: vec![record("stale")],
                next_cursor: empty_object(),
                error: None,
                delivery: Some(watch_delivery("stale-lease")),
            })
            .await
            .is_err()
    );

    assert!(pending_deliveries(&store, "tenant").await.is_empty());
}

#[tokio::test]
async fn expired_delivering_delivery_is_reclaimable_and_old_owner_cannot_complete() {
    let store = SqliteStore::in_memory().unwrap();
    complete_with_delivery(
        &store,
        CompletionSeed::new(("tenant-a", "watch", "owner", "run", "idem"), 1)
            .records(vec![record("reclaim")])
            .delivery_id("d1"),
    )
    .await;

    let old = claim_delivery(&store, "tenant-a", "old-worker", 10)
        .await
        .unwrap();
    assert_eq!(old.lease_expires_epoch_seconds, Some(70));
    let reclaimed = claim_delivery(&store, "tenant-a", "new-worker", 70)
        .await
        .unwrap();
    assert_eq!(reclaimed.lease_owner.as_deref(), Some("new-worker"));
    assert_eq!(reclaimed.attempts, 2);

    assert!(
        store
            .complete_delivery(CompleteDelivery {
                tenant_id: "tenant-a".into(),
                delivery_id: "d1".into(),
                lease_owner: "old-worker".into(),
                finished_at_epoch_seconds: 71,
                success: true,
                error: None,
            })
            .await
            .is_err()
    );
    let still_delivering = store
        .list_deliveries(list_deliveries("tenant-a", Some("delivering")))
        .await
        .unwrap();
    assert_eq!(
        still_delivering[0].lease_owner.as_deref(),
        Some("new-worker")
    );

    let completed = store
        .complete_delivery(CompleteDelivery {
            tenant_id: "tenant-a".into(),
            delivery_id: "d1".into(),
            lease_owner: "new-worker".into(),
            finished_at_epoch_seconds: 72,
            success: true,
            error: None,
        })
        .await
        .unwrap();
    assert_eq!(completed.status, "succeeded");
    assert_eq!(completed.delivered_at_epoch_seconds, Some(72));
}

#[tokio::test]
async fn deliveries_retry_then_fail_under_lease() {
    let store = SqliteStore::in_memory().unwrap();
    complete_with_delivery(
        &store,
        CompletionSeed::new(("tenant-a", "watch", "owner", "run", "idem"), 1)
            .records(vec![record("delivered")])
            .delivery_id("d1"),
    )
    .await;

    let first = claim_delivery(&store, "tenant-a", "worker", 10)
        .await
        .unwrap();
    assert_eq!(first.attempts, 1);
    let retry = store
        .complete_delivery(CompleteDelivery {
            tenant_id: "tenant-a".into(),
            delivery_id: "d1".into(),
            lease_owner: "worker".into(),
            finished_at_epoch_seconds: 20,
            success: false,
            error: Some("timeout".into()),
        })
        .await
        .unwrap();
    assert_eq!(retry.status, "pending");
    assert_eq!(retry.next_attempt_epoch_seconds, 320);

    claim_delivery(&store, "tenant-a", "worker", 320)
        .await
        .unwrap();
    store
        .complete_delivery(CompleteDelivery {
            tenant_id: "tenant-a".into(),
            delivery_id: "d1".into(),
            lease_owner: "worker".into(),
            finished_at_epoch_seconds: 321,
            success: false,
            error: Some("timeout".into()),
        })
        .await
        .unwrap();
    claim_delivery(&store, "tenant-a", "worker", 621)
        .await
        .unwrap();
    let failed = store
        .complete_delivery(CompleteDelivery {
            tenant_id: "tenant-a".into(),
            delivery_id: "d1".into(),
            lease_owner: "worker".into(),
            finished_at_epoch_seconds: 622,
            success: false,
            error: Some("timeout".into()),
        })
        .await
        .unwrap();
    assert_eq!(failed.status, "failed");
    assert!(
        store
            .claim_pending_delivery(claim_delivery_request("tenant-a", "worker", 923))
            .await
            .unwrap()
            .is_none()
    );
}

async fn run_once(store: &SqliteStore, tenant: &str, watch: &str, owner: &str, record: Record) {
    create_watch(store, tenant, watch, 1).await;
    claim_and_start(
        store,
        ClaimStart::new(
            (
                tenant,
                watch,
                owner,
                format!("{watch}-run"),
                format!("{watch}-idem"),
            ),
            1,
        ),
    )
    .await;
    store
        .complete_watch_run(CompleteWatchRun {
            tenant_id: tenant.into(),
            watch_id: watch.into(),
            run_id: format!("{watch}-run"),
            lease_owner: owner.into(),
            finished_at_epoch_seconds: 2,
            records: vec![record],
            next_cursor: empty_object(),
            error: None,
            delivery: None,
        })
        .await
        .unwrap();
}

fn watch_delivery(delivery_id: &str) -> WatchRunDelivery {
    WatchRunDelivery {
        delivery_id: delivery_id.into(),
        target: json!({"type":"webhook","url":"https://example.com/hook"}),
    }
}

async fn pending_deliveries(store: &SqliteStore, tenant: &str) -> Vec<comsat_store::Delivery> {
    store
        .list_deliveries(list_deliveries(tenant, Some("pending")))
        .await
        .unwrap()
}

fn payload_record_ids(delivery: &comsat_store::Delivery) -> Vec<String> {
    delivery
        .payload
        .get("records")
        .and_then(serde_json::Value::as_array)
        .expect("delivery payload has records")
        .iter()
        .map(|record| {
            record
                .get("id")
                .and_then(serde_json::Value::as_str)
                .expect("payload record has id")
                .to_owned()
        })
        .collect()
}

struct CompletionSeed {
    tenant: String,
    watch: String,
    owner: String,
    run: String,
    idem: String,
    now: i64,
    records: Vec<Record>,
    delivery_id: String,
}

impl CompletionSeed {
    fn new(identity: (&str, &str, &str, &str, &str), now: i64) -> Self {
        let (tenant, watch, owner, run, idem) = identity;
        Self {
            tenant: tenant.into(),
            watch: watch.into(),
            owner: owner.into(),
            run: run.into(),
            idem: idem.into(),
            now,
            records: vec![record("default")],
            delivery_id: "delivery".into(),
        }
    }

    fn records(mut self, records: Vec<Record>) -> Self {
        self.records = records;
        self
    }

    fn delivery_id(mut self, delivery_id: &str) -> Self {
        self.delivery_id = delivery_id.into();
        self
    }
}

async fn complete_with_delivery(store: &SqliteStore, seed: CompletionSeed) {
    create_watch(store, &seed.tenant, &seed.watch, seed.now).await;
    claim_and_start(
        store,
        ClaimStart::new(
            (
                seed.tenant.clone(),
                seed.watch.clone(),
                seed.owner.clone(),
                seed.run.clone(),
                seed.idem,
            ),
            seed.now,
        ),
    )
    .await;
    store
        .complete_watch_run(CompleteWatchRun {
            tenant_id: seed.tenant,
            watch_id: seed.watch,
            run_id: seed.run,
            lease_owner: seed.owner,
            finished_at_epoch_seconds: seed.now + 1,
            records: seed.records,
            next_cursor: empty_object(),
            error: None,
            delivery: Some(watch_delivery(&seed.delivery_id)),
        })
        .await
        .unwrap();
}

fn list_deliveries(tenant: &str, status: Option<&str>) -> ListDeliveries {
    ListDeliveries {
        tenant_id: tenant.into(),
        status: status.map(str::to_owned),
        limit: 20,
    }
}

fn claim_delivery_request(tenant: &str, owner: &str, now: i64) -> ClaimPendingDelivery {
    ClaimPendingDelivery {
        tenant_id: tenant.into(),
        now_epoch_seconds: now,
        lease_owner: owner.into(),
        lease_seconds: 60,
    }
}

async fn claim_delivery(
    store: &SqliteStore,
    tenant: &str,
    owner: &str,
    now: i64,
) -> Option<comsat_store::Delivery> {
    store
        .claim_pending_delivery(claim_delivery_request(tenant, owner, now))
        .await
        .unwrap()
}

async fn create_watch(store: &SqliteStore, tenant: &str, watch: &str, first_due: i64) {
    store
        .create_watch(CreateWatch {
            tenant_id: tenant.into(),
            watch_id: watch.into(),
            query: Query {
                text: "MCP OAuth".into(),
                limit: Some(10),
                since: None,
                until: None,
            },
            source_ids: vec![SourceId::new("github").unwrap()],
            interval_seconds: 60,
            cursor: empty_object(),
            enabled: true,
            created_at_epoch_seconds: 0,
            first_due_epoch_seconds: Some(first_due),
        })
        .await
        .unwrap();
}

struct ClaimStart {
    tenant: String,
    watch: String,
    owner: String,
    run: String,
    idem: String,
    now: i64,
}

impl ClaimStart {
    fn new(
        values: (
            impl Into<String>,
            impl Into<String>,
            impl Into<String>,
            impl Into<String>,
            impl Into<String>,
        ),
        now: i64,
    ) -> Self {
        let (tenant, watch, owner, run, idem) = values;
        Self {
            tenant: tenant.into(),
            watch: watch.into(),
            owner: owner.into(),
            run: run.into(),
            idem: idem.into(),
            now,
        }
    }
}

async fn claim_and_start(store: &SqliteStore, input: ClaimStart) -> comsat_store::ClaimedWatch {
    let claim = store
        .claim_due_watch(ClaimDueWatch {
            tenant_id: input.tenant.clone(),
            now_epoch_seconds: input.now,
            lease_owner: input.owner.clone(),
            lease_seconds: 30,
        })
        .await
        .unwrap()
        .unwrap();
    store
        .start_watch_run(StartWatchRun {
            tenant_id: input.tenant,
            watch_id: input.watch,
            run_id: input.run,
            idempotency_key: input.idem,
            lease_owner: input.owner,
            started_at_epoch_seconds: input.now,
        })
        .await
        .unwrap();
    claim
}

fn record(id: &str) -> Record {
    Record {
        id: RecordId::new(id).unwrap(),
        source: SourceId::new("github").unwrap(),
        kind: "issue".into(),
        url: format!("https://github.com/example/project/issues/{id}"),
        title: Some(id.into()),
        text: None,
        author: None,
        created_at: None,
        updated_at: None,
        metadata: json!({}),
    }
}
