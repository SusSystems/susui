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

/// Find all `.drv` store paths whose derivation name matches any of the given patterns.
///
/// Each pattern is a derivation name (e.g. `"susui-0.1.0"`). The function searches
/// for store paths matching `/nix/store/%-<name>.drv`.
///
/// Returns `Vec<(drv_path, drv_name, drv_hash)>` where:
/// - `drv_path` is the full `/nix/store/...` path
/// - `drv_name` is the derivation name (e.g. `"susui-0.1.0"`)
/// - `drv_hash` is the 32-char nix hash prefix
pub fn find_drvs_by_name(name_patterns: &[String]) -> Vec<(String, String, String)> {
    match find_drvs_by_name_inner(name_patterns) {
        Ok(results) => results,
        Err(e) => {
            tracing::debug!("nixdb: failed to query drvs by name: {}", e);
            Vec::new()
        }
    }
}

fn find_drvs_by_name_inner(
    name_patterns: &[String],
) -> Result<Vec<(String, String, String)>, rusqlite::Error> {
    if name_patterns.is_empty() {
        return Ok(Vec::new());
    }

    let conn = Connection::open_with_flags(
        NIX_DB_PATH,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut results = Vec::new();

    for pattern in name_patterns {
        let like_pattern = format!("/nix/store/%-{}.drv", pattern);
        let mut stmt = conn.prepare(
            "SELECT path FROM ValidPaths WHERE path LIKE ?",
        )?;
        let rows = stmt.query_map([&like_pattern], |row| row.get::<_, String>(0))?;

        for path in rows.flatten() {
            // Parse: /nix/store/<32-char-hash>-<name>.drv
            let fname = path.split('/').next_back().unwrap_or("");
            if fname.len() > 33 && fname.as_bytes()[32] == b'-' {
                let hash = fname[..32].to_string();
                let name = fname[33..].trim_end_matches(".drv").to_string();
                results.push((path, name, hash));
            }
        }
    }

    Ok(results)
}

/// Find the stdenv derivation referenced by each of the given `.drv` paths.
///
/// Queries the `Refs` table to find stdenv references (e.g. `*-stdenv-linux.drv`).
/// Returns `HashMap<drv_path, stdenv_hash>` where `stdenv_hash` is the 8-char
/// nix store hash prefix from the stdenv derivation path.
pub fn find_stdenv_for_drvs(drv_paths: &[String]) -> HashMap<String, String> {
    match find_stdenv_for_drvs_inner(drv_paths) {
        Ok(results) => results,
        Err(e) => {
            tracing::debug!("nixdb: failed to query stdenv for drvs: {}", e);
            HashMap::new()
        }
    }
}

fn find_stdenv_for_drvs_inner(
    drv_paths: &[String],
) -> Result<HashMap<String, String>, rusqlite::Error> {
    if drv_paths.is_empty() {
        return Ok(HashMap::new());
    }

    let conn = Connection::open_with_flags(
        NIX_DB_PATH,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut results: HashMap<String, String> = HashMap::new();

    for chunk in drv_paths.chunks(100) {
        let placeholders: String = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT vp1.path, vp2.path \
             FROM Refs r \
             JOIN ValidPaths vp1 ON r.referrer = vp1.id \
             JOIN ValidPaths vp2 ON r.reference = vp2.id \
             WHERE vp1.path IN ({}) \
               AND (vp2.path LIKE '%-stdenv-linux.drv' OR vp2.path LIKE '%-stdenv-darwin.drv') \
               AND vp2.path NOT LIKE '%bootstrap%'",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows.flatten() {
            let (drv_path, stdenv_path) = row;
            // Parse hash from /nix/store/<32-char-hash>-stdenv-linux.drv
            let fname = stdenv_path.split('/').next_back().unwrap_or("");
            if fname.len() > 32 {
                let hash = &fname[..8];
                results.insert(drv_path, hash.to_string());
            }
        }
    }

    Ok(results)
}

/// Find all output store paths derived from the given `.drv` paths.
///
/// Returns a map of `drv_path → Vec<(output_path, registration_time)>`.
pub fn find_outputs_for_drvs(drv_paths: &[String]) -> HashMap<String, Vec<(String, i64)>> {
    match find_outputs_for_drvs_inner(drv_paths) {
        Ok(results) => results,
        Err(e) => {
            tracing::debug!("nixdb: failed to query outputs for drvs: {}", e);
            HashMap::new()
        }
    }
}

fn find_outputs_for_drvs_inner(
    drv_paths: &[String],
) -> Result<HashMap<String, Vec<(String, i64)>>, rusqlite::Error> {
    if drv_paths.is_empty() {
        return Ok(HashMap::new());
    }

    let conn = Connection::open_with_flags(
        NIX_DB_PATH,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut results: HashMap<String, Vec<(String, i64)>> = HashMap::new();

    // Query in batches to avoid huge IN clauses
    for chunk in drv_paths.chunks(100) {
        let placeholders: String = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT deriver, path, registrationTime FROM ValidPaths \
             WHERE deriver IN ({})",
            placeholders
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::types::ToSql> = chunk
            .iter()
            .map(|s| s as &dyn rusqlite::types::ToSql)
            .collect();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        for row in rows.flatten() {
            let (deriver, path, reg_time) = row;
            results.entry(deriver).or_default().push((path, reg_time));
        }
    }

    Ok(results)
}
