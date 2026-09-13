use comsat_store::{CreateWatch, Watch, empty_object};
use comsat_types::Query;

use crate::{AppResult, ComsatApp};

use super::{search::parse_source_ids, types::WatchAddOptions};

const DEFAULT_WATCH_INTERVAL_SECONDS: u64 = 300;

pub(super) async fn watch_add(app: &ComsatApp, options: WatchAddOptions) -> AppResult<Watch> {
    let now = app.now();
    let watch_id = options
        .watch_id
        .unwrap_or_else(|| generated_watch_id(&options.query, now));
    let source_ids =
        parse_source_ids(&options.source).map_err(comsat_store::StoreError::Validation)?;
    app.store()?
        .create_watch(CreateWatch {
            tenant_id: app.tenant_id.clone(),
            watch_id,
            query: Query {
                text: options.query,
                limit: options.limit,
                since: None,
                until: None,
            },
            source_ids,
            interval_seconds: options
                .interval_seconds
                .unwrap_or(DEFAULT_WATCH_INTERVAL_SECONDS),
            cursor: empty_object(),
            enabled: true,
            created_at_epoch_seconds: now,
            first_due_epoch_seconds: None,
        })
        .await
        .map_err(Into::into)
}

pub(super) fn parse_since(value: &str, now: i64) -> Result<i64, String> {
    if let Some(days) = value.strip_suffix('d') {
        let days = days
            .parse::<i64>()
            .map_err(|_| "since days must be a number like 30d".to_string())?;
        if days < 0 {
            return Err("since days must be nonnegative".to_string());
        }
        return Ok(now.saturating_sub(days.saturating_mul(86_400)));
    }
    value
        .parse::<i64>()
        .map_err(|_| "since must be an epoch second or a duration like 30d".to_string())
}

fn generated_watch_id(query: &str, now: i64) -> String {
    let slug = query
        .bytes()
        .filter_map(|byte| match byte {
            b'a'..=b'z' | b'0'..=b'9' => Some(byte as char),
            b'A'..=b'Z' => Some(byte.to_ascii_lowercase() as char),
            b' ' | b'-' | b'_' => Some('-'),
            _ => None,
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "watch" } else { &slug };
    format!("{slug}-{now}")
}
