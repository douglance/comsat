use comsat_store::StoreResult;
use rusqlite::Connection;

use crate::codec::map_sql;

const INITIAL: &str = include_str!("../../../migrations/0001_initial.sql");
const DELIVERIES: &str = include_str!("../../../migrations/0002_delivery_notifications.sql");

pub fn apply(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction().map_err(map_sql)?;
    transaction.execute_batch(INITIAL).map_err(map_sql)?;
    transaction
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY);",
        )
        .map_err(map_sql)?;
    let applied: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = 2)",
            [],
            |row| row.get(0),
        )
        .map_err(map_sql)?;
    if !applied {
        transaction.execute_batch(DELIVERIES).map_err(map_sql)?;
        transaction
            .execute("INSERT INTO schema_migrations(version) VALUES (2)", [])
            .map_err(map_sql)?;
    }
    transaction.commit().map_err(map_sql)
}
