use incurs_codemode::{ArtifactStore, ExecutionState, ExecutionStatus, RuntimeStore, Snippet};
use serde_json::json;

use super::SqliteCodeModeStore;

fn store() -> SqliteCodeModeStore {
    SqliteCodeModeStore::in_memory().expect("in-memory store")
}

fn execution(id: &str, updated_at: u64) -> ExecutionState {
    ExecutionState {
        id: id.to_owned(),
        code: "return 1;".to_owned(),
        status: ExecutionStatus::Completed,
        log: Vec::new(),
        result: Some(json!(1)),
        error: None,
        logs: Vec::new(),
        connectors: vec!["comsat".to_owned()],
        capabilities: None,
        events: Vec::new(),
        created_at: 1,
        updated_at,
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(future)
}

#[test]
fn executions_survive_a_new_store_over_the_same_database() {
    let path = std::env::temp_dir().join(format!("comsat-codemode-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&path);
    block_on(async {
        let first = SqliteCodeModeStore::open(&path).expect("open");
        first.put_execution(&execution("exec-1", 10)).await.unwrap();

        let second = SqliteCodeModeStore::open(&path).expect("reopen");
        let reloaded = second.get_execution("exec-1").await.unwrap().unwrap();

        assert_eq!(reloaded.id, "exec-1");
        assert_eq!(reloaded.result, Some(json!(1)));
    });
    let _ = std::fs::remove_file(&path);
}

#[test]
fn executions_list_newest_first_and_replace_in_place() {
    block_on(async {
        let store = store();
        store.put_execution(&execution("older", 10)).await.unwrap();
        store.put_execution(&execution("newer", 20)).await.unwrap();
        store.put_execution(&execution("older", 30)).await.unwrap();

        let ids: Vec<String> = store
            .list_executions()
            .await
            .unwrap()
            .into_iter()
            .map(|execution| execution.id)
            .collect();

        assert_eq!(ids, ["older", "newer"]);
    });
}

#[test]
fn deleting_an_execution_removes_its_artifacts() {
    block_on(async {
        let store = store();
        store.put_execution(&execution("exec-1", 1)).await.unwrap();
        let artifact = store.put("exec-1", &json!({"large": true})).await.unwrap();

        RuntimeStore::delete_execution(&store, "exec-1")
            .await
            .unwrap();

        assert!(store.get_execution("exec-1").await.unwrap().is_none());
        assert!(
            ArtifactStore::get(&store, "exec-1", &artifact.id)
                .await
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn artifacts_are_scoped_to_their_execution() {
    block_on(async {
        let store = store();
        let artifact = store.put("exec-1", &json!({"value": 7})).await.unwrap();

        assert_eq!(
            ArtifactStore::get(&store, "exec-1", &artifact.id)
                .await
                .unwrap(),
            Some(json!({"value": 7}))
        );
        assert!(
            ArtifactStore::get(&store, "exec-2", &artifact.id)
                .await
                .unwrap()
                .is_none(),
            "another execution must not read this artifact"
        );
    });
}

#[test]
fn snippets_round_trip_and_delete_reports_whether_one_existed() {
    block_on(async {
        let store = store();
        let snippet = Snippet {
            name: "daily".to_owned(),
            description: "daily digest".to_owned(),
            code: "return await comsat.search({text: 'rust'});".to_owned(),
            saved_at: 5,
            input_schema: None,
            connectors: vec!["comsat".to_owned()],
        };

        store.put_snippet(&snippet).await.unwrap();

        assert_eq!(store.get_snippet("daily").await.unwrap(), Some(snippet));
        assert_eq!(store.list_snippets().await.unwrap().len(), 1);
        assert!(store.delete_snippet("daily").await.unwrap());
        assert!(!store.delete_snippet("daily").await.unwrap());
    });
}
