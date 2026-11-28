use rusqlite::Connection;

use super::StoreError;

struct Migration {
    version: u32,
    name: &'static str,
    sql: &'static str,
}

/// Every migration, in order. To change the schema, add a file to
/// `migrations/` with the next number and add one line here. Never edit a
/// migration that has shipped.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "schedule",
        sql: include_str!("../../migrations/0001_schedule.sql"),
    },
    Migration {
        version: 2,
        name: "library_cache",
        sql: include_str!("../../migrations/0002_library_cache.sql"),
    },
    Migration {
        version: 3,
        name: "drop_cursors",
        sql: include_str!("../../migrations/0003_drop_cursors.sql"),
    },
    Migration {
        version: 4,
        name: "library_sources",
        sql: include_str!("../../migrations/0004_library_sources.sql"),
    },
    Migration {
        version: 5,
        name: "substitutions",
        sql: include_str!("../../migrations/0005_substitutions.sql"),
    },
    Migration {
        version: 6,
        name: "viewer_settings",
        sql: include_str!("../../migrations/0006_viewer_settings.sql"),
    },
    Migration {
        version: 7,
        name: "music_videos",
        sql: include_str!("../../migrations/0007_music_videos.sql"),
    },
    Migration {
        version: 8,
        name: "songs",
        sql: include_str!("../../migrations/0008_songs.sql"),
    },
];

/// Applies the migrations the database has not seen yet. Each one runs in its
/// own transaction together with its `schema_migrations` row, so a failure
/// leaves the database at the last complete version.
pub(super) fn run(conn: &mut Connection) -> Result<(), StoreError> {
    let action = "running migrations";
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version       INTEGER PRIMARY KEY,
            name          TEXT    NOT NULL,
            applied_at_ms INTEGER NOT NULL
        )",
    )
    .map_err(StoreError::database(action))?;

    let current: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(StoreError::database(action))?;
    let supported = MIGRATIONS.last().map_or(0, |m| m.version);
    if current > supported {
        return Err(StoreError::NewerSchema {
            found: current,
            supported,
        });
    }

    for migration in MIGRATIONS.iter().filter(|m| m.version > current) {
        let tx = conn.transaction().map_err(StoreError::database(action))?;
        tx.execute_batch(migration.sql)
            .map_err(StoreError::database(action))?;
        tx.execute(
            "INSERT INTO schema_migrations (version, name, applied_at_ms)
             VALUES (?1, ?2, CAST(strftime('%s', 'now') AS INTEGER) * 1000)",
            (migration.version, migration.name),
        )
        .map_err(StoreError::database(action))?;
        tx.commit().map_err(StoreError::database(action))?;
    }
    Ok(())
}

/// The schema version the newest migration brings a database to.
pub(super) fn latest_version() -> u32 {
    MIGRATIONS.last().map_or(0, |m| m.version)
}

#[cfg(test)]
pub(super) fn names() -> Vec<(u32, &'static str)> {
    MIGRATIONS.iter().map(|m| (m.version, m.name)).collect()
}
