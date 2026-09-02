//! Hand-rolled `PRAGMA user_version`-based schema migrations.
//!
//! No migration crate dependency: `MIGRATIONS` (see [`crate::archive_db::
//! schema`]) starts from a from-scratch schema with few migrations
//! expected soon, so a small runner is simpler to audit than vendoring one.

use rusqlite::Connection;

use super::schema::MIGRATIONS;

/// Applies every migration after the connection's current `user_version`,
/// in order, inside a single transaction. Idempotent: re-running on an
/// already-migrated connection is a no-op.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let current_version = usize::try_from(current_version).unwrap_or(0);

    if current_version >= MIGRATIONS.len() {
        return Ok(());
    }

    let tx = conn.transaction()?;
    for migration_sql in &MIGRATIONS[current_version..] {
        tx.execute_batch(migration_sql)?;
    }
    let new_version = MIGRATIONS.len();
    // PRAGMA doesn't support bound parameters; the value is our own
    // trusted constant (migration count), never user input.
    tx.execute_batch(&format!("PRAGMA user_version = {new_version}"))?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_creates_every_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        for table in super::super::schema::TABLE_NAMES {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "table `{table}` should exist after migrate()");
        }
    }

    #[test]
    fn migrate_sets_user_version_to_migration_count() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
    }

    #[test]
    fn migrate_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        // Re-running must not error (e.g. from re-issuing `CREATE TABLE`
        // without `IF NOT EXISTS`) and must not change the version.
        migrate(&mut conn).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version as usize, MIGRATIONS.len());
    }
}
