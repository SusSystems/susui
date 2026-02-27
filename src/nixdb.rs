use std::collections::HashMap;

use rusqlite::{Connection, OpenFlags};

const NIX_DB_PATH: &str = "/nix/var/nix/db/db.sqlite";

/// Build duration results from the Nix SQLite database.
pub struct BuildDurations {
    /// store_path → duration in seconds
    pub by_store_path: HashMap<String, u64>,
    /// drv_path → duration in seconds (uses max duration across outputs)
    pub by_drv_path: HashMap<String, u64>,
}

/// Look up build durations from the Nix SQLite database.
///
/// Computes `duration = output.registrationTime - deriver.registrationTime`
/// for each store path. Only returns entries where duration > 0 (locally-built
/// paths). Substituted/imported paths yield 0 and are excluded.
pub fn lookup_build_durations(
    store_paths: &[&str],
    drv_paths: &[&str],
) -> BuildDurations {
    match lookup_inner(store_paths, drv_paths) {
        Ok(durations) => durations,
        Err(e) => {
            tracing::debug!("nixdb: failed to query build durations: {}", e);
            BuildDurations {
                by_store_path: HashMap::new(),
                by_drv_path: HashMap::new(),
            }
        }
    }
}

fn lookup_inner(
    store_paths: &[&str],
    drv_paths: &[&str],
) -> Result<BuildDurations, rusqlite::Error> {
    let conn = Connection::open_with_flags(
        NIX_DB_PATH,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut by_store_path = HashMap::new();
    let mut by_drv_path: HashMap<String, u64> = HashMap::new();

    // Query by output store paths
    if !store_paths.is_empty() {
        let placeholders: String = store_paths.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT out.path, out.registrationTime - drv.registrationTime \
             FROM ValidPaths out \
             JOIN ValidPaths drv ON drv.path = out.deriver \
             WHERE out.path IN ({}) \
               AND out.deriver IS NOT NULL AND out.deriver != ''",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = store_paths
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for (path, dur) in rows.flatten() {
            if dur > 0 {
                by_store_path.insert(path, dur as u64);
            }
        }
    }

    // Query by derivation paths — keyed by deriver (drv_path) for matching
    if !drv_paths.is_empty() {
        let placeholders: String = drv_paths.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT out.deriver, out.registrationTime - drv.registrationTime \
             FROM ValidPaths out \
             JOIN ValidPaths drv ON drv.path = out.deriver \
             WHERE out.deriver IN ({}) \
               AND out.deriver IS NOT NULL AND out.deriver != ''",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = drv_paths
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for (deriver, dur) in rows.flatten() {
            if dur > 0 {
                // Use max duration across outputs (a drv can have multiple outputs)
                let entry = by_drv_path.entry(deriver).or_insert(0);
                if dur as u64 > *entry {
                    *entry = dur as u64;
                }
            }
        }
    }

    Ok(BuildDurations {
        by_store_path,
        by_drv_path,
    })
}
