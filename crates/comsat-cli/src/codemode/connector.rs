//! COMSAT's Code Mode connector.
//!
//! Incurs drives rollback by asking the connector to compensate each applied
//! action in reverse order. Retrieval needs no compensation because it changes
//! nothing; the one mutating tool that can be undone, `watch_add`, is
//! compensated by deleting the watch it created. Anything else reports that it
//! did not compensate, so a rollback never claims more than it did.

use async_trait::async_trait;
use incurs_codemode::{Connector, ConnectorDescription, IncurConnector, ToolContext};
use serde_json::{Value, json};

const WATCH_ADD: &str = "watch_add";
const WATCH_DELETE: &str = "watch_delete";

pub struct ComsatConnector {
    inner: IncurConnector,
}

impl ComsatConnector {
    pub const fn new(inner: IncurConnector) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl Connector for ComsatConnector {
    async fn describe(&self) -> Result<ConnectorDescription, String> {
        self.inner.describe().await
    }

    async fn execute(
        &self,
        method: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<Value, String> {
        self.inner.execute(method, arguments, context).await
    }

    async fn revert(
        &self,
        method: &str,
        arguments: Value,
        result: Value,
        context: &ToolContext,
    ) -> Result<bool, String> {
        if method != WATCH_ADD {
            return Ok(false);
        }
        let Some(watch_id) = created_watch_id(&arguments, &result) else {
            return Ok(false);
        };
        self.inner
            .execute(WATCH_DELETE, json!({ "watch": watch_id }), context)
            .await?;
        Ok(true)
    }

    async fn pass_ended(&self, execution_id: &str, status: &str) {
        self.inner.pass_ended(execution_id, status).await;
    }

    async fn execution_ended(&self, execution_id: &str, status: &str) {
        self.inner.execution_ended(execution_id, status).await;
    }
}

/// The watch identity to compensate, preferring what the store reported.
fn created_watch_id(arguments: &Value, result: &Value) -> Option<String> {
    for value in [result, arguments] {
        if let Some(watch_id) = value.get("watch_id").and_then(Value::as_str) {
            return Some(watch_id.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::created_watch_id;

    #[test]
    fn prefers_the_identity_the_store_reported() {
        let id = created_watch_id(
            &json!({"watch_id": "requested"}),
            &json!({"watch_id": "stored"}),
        );

        assert_eq!(id.as_deref(), Some("stored"));
    }

    #[test]
    fn falls_back_to_the_requested_identity() {
        let id = created_watch_id(&json!({"watch_id": "requested"}), &json!(null));

        assert_eq!(id.as_deref(), Some("requested"));
    }

    #[test]
    fn reports_nothing_to_compensate_without_an_identity() {
        assert!(created_watch_id(&json!({"query": "rust"}), &json!({})).is_none());
    }
}
