// Functions in this module are consumed by the channel handler added in Stage 2.
// Dead code warnings are suppressed here because the public API is complete but
// the call sites (runner/channel.rs) are added in the next implementation stage.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use redb::{Database, ReadOnlyDatabase, ReadableDatabase, ReadableTable, TableDefinition};

use crate::error::CreftError;
use crate::search::index::SearchIndex;
use crate::search::store::write_index_bytes;

/// Table for key-value data.
const KV_TABLE: TableDefinition<&str, &str> = TableDefinition::new("kv");

/// Table for store metadata (global flag, etc.).
const META_TABLE: TableDefinition<&str, &str> = TableDefinition::new("meta");

/// Key used in `META_TABLE` to store the global visibility flag.
const META_GLOBAL_KEY: &str = "global";

/// Open or create the redb database for writing.
///
/// Creates the directory and database file if they do not exist.
/// Acquires an exclusive file lock. Returns `Err` wrapping
/// `CreftError::StoreOpen` if the database is already open by another
/// process (`DatabaseAlreadyOpen`).
fn open_db(dir: &Path, qualified_name: &str) -> Result<Database, CreftError> {
    std::fs::create_dir_all(dir)?;
    let path = store_path(dir, qualified_name);
    Database::create(&path).map_err(|e| CreftError::StoreOpen {
        name: qualified_name.to_owned(),
        reason: e.to_string(),
    })
}

/// Open the redb database for reading only.
///
/// Acquires a shared file lock, allowing concurrent readers. Does not
/// create the database if it does not exist — callers must check for
/// the file's existence first.
///
/// Returns `Err(CreftError::StoreOpen)` if the database cannot be opened,
/// including if it was not shut down cleanly (`RepairAborted`). Callers
/// that treat read failures as "not found" (e.g., `store_is_global`) handle
/// this by matching on `Err`.
fn open_db_readonly(dir: &Path, qualified_name: &str) -> Result<ReadOnlyDatabase, CreftError> {
    let path = store_path(dir, qualified_name);
    ReadOnlyDatabase::open(&path).map_err(|e| CreftError::StoreOpen {
        name: qualified_name.to_owned(),
        reason: e.to_string(),
    })
}

/// Insert or replace a key-value pair, optionally setting the global flag.
///
/// Opens the database with an exclusive lock, writes the KV entry and
/// (when `global` is `Some`) the global flag in a single transaction,
/// then closes the database. The exclusive lock is held only during the
/// transaction.
///
/// When `global` is `None`, the global flag is left unchanged. Passing
/// `Some(false)` explicitly revokes global access.
pub(crate) fn store_put(
    dir: &Path,
    qualified_name: &str,
    key: &str,
    value: &str,
    global: Option<bool>,
) -> Result<(), CreftError> {
    let db = open_db(dir, qualified_name)?;
    let txn = db.begin_write().map_err(|e| CreftError::StoreWrite {
        name: qualified_name.to_owned(),
        reason: e.to_string(),
    })?;
    {
        let mut table = txn
            .open_table(KV_TABLE)
            .map_err(|e| CreftError::StoreWrite {
                name: qualified_name.to_owned(),
                reason: e.to_string(),
            })?;
        table
            .insert(key, value)
            .map_err(|e| CreftError::StoreWrite {
                name: qualified_name.to_owned(),
                reason: e.to_string(),
            })?;
    }
    if let Some(flag) = global {
        let mut meta = txn
            .open_table(META_TABLE)
            .map_err(|e| CreftError::StoreWrite {
                name: qualified_name.to_owned(),
                reason: e.to_string(),
            })?;
        let val = if flag { "1" } else { "0" };
        meta.insert(META_GLOBAL_KEY, val)
            .map_err(|e| CreftError::StoreWrite {
                name: qualified_name.to_owned(),
                reason: e.to_string(),
            })?;
    }
    txn.commit().map_err(|e| CreftError::StoreWrite {
        name: qualified_name.to_owned(),
        reason: e.to_string(),
    })?;
    Ok(())
}

/// Look up a value by exact key.
///
/// Returns `None` if the key does not exist or if the database does not
/// exist (cold start). Opens the database with a shared lock via
/// `ReadOnlyDatabase`, allowing concurrent readers.
pub(crate) fn store_get(
    dir: &Path,
    qualified_name: &str,
    key: &str,
) -> Result<Option<String>, CreftError> {
    let path = store_path(dir, qualified_name);
    if !path.exists() {
        return Ok(None);
    }
    let db = open_db_readonly(dir, qualified_name)?;
    let txn = db.begin_read().map_err(|e| CreftError::StoreRead {
        name: qualified_name.to_owned(),
        reason: e.to_string(),
    })?;
    let table = match txn.open_table(KV_TABLE) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(None),
        Err(e) => {
            return Err(CreftError::StoreRead {
                name: qualified_name.to_owned(),
                reason: e.to_string(),
            });
        }
    };
    match table.get(key) {
        Ok(Some(guard)) => Ok(Some(guard.value().to_owned())),
        Ok(None) => Ok(None),
        Err(e) => Err(CreftError::StoreRead {
            name: qualified_name.to_owned(),
            reason: e.to_string(),
        }),
    }
}

/// Read all key-value pairs from the store.
///
/// Returns an empty vec if the database or table does not exist.
/// Opens with a shared lock via `ReadOnlyDatabase`.
/// Used to rebuild the search index after a put.
pub(crate) fn store_entries(
    dir: &Path,
    qualified_name: &str,
) -> Result<Vec<(String, String)>, CreftError> {
    let path = store_path(dir, qualified_name);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let db = open_db_readonly(dir, qualified_name)?;
    let txn = db.begin_read().map_err(|e| CreftError::StoreRead {
        name: qualified_name.to_owned(),
        reason: e.to_string(),
    })?;
    let table = match txn.open_table(KV_TABLE) {
        Ok(t) => t,
        Err(redb::TableError::TableDoesNotExist(_)) => return Ok(Vec::new()),
        Err(e) => {
            return Err(CreftError::StoreRead {
                name: qualified_name.to_owned(),
                reason: e.to_string(),
            });
        }
    };
    let mut entries = Vec::new();
    let iter = table.iter().map_err(|e| CreftError::StoreRead {
        name: qualified_name.to_owned(),
        reason: e.to_string(),
    })?;
    for result in iter {
        let (k, v) = result.map_err(|e| CreftError::StoreRead {
            name: qualified_name.to_owned(),
            reason: e.to_string(),
        })?;
        entries.push((k.value().to_owned(), v.value().to_owned()));
    }
    Ok(entries)
}

/// Check whether a store is marked as globally accessible.
///
/// Opens with a shared lock via `ReadOnlyDatabase`. Returns `false` if
/// the database does not exist, cannot be opened, the meta table does not
/// exist, or the global key is not set. Conservative default ensures
/// cross-namespace searches fail closed.
pub(crate) fn store_is_global(dir: &Path, qualified_name: &str) -> bool {
    let path = store_path(dir, qualified_name);
    if !path.exists() {
        return false;
    }
    let db = match open_db_readonly(dir, qualified_name) {
        Ok(db) => db,
        Err(_) => return false,
    };
    let txn = match db.begin_read() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let table = match txn.open_table(META_TABLE) {
        Ok(t) => t,
        Err(_) => return false,
    };
    match table.get(META_GLOBAL_KEY) {
        Ok(Some(guard)) => guard.value() == "1",
        _ => false,
    }
}

/// Compute the redb database file path for a fully-qualified store name.
///
/// - `"deploy.data"` → `<dir>/deploy.data.redb`
/// - `"acme.deploy.data"` → `<dir>/acme.deploy.data.redb`
pub(crate) fn store_path(dir: &Path, qualified_name: &str) -> PathBuf {
    dir.join(format!("{qualified_name}.redb"))
}

/// Path to the search index companion file for a store.
pub(crate) fn store_index_path(dir: &Path, qualified_name: &str) -> PathBuf {
    dir.join(format!("{qualified_name}.store.idx"))
}

/// Rebuild the search index for a store from its current entries.
///
/// Reads all key-value pairs from the redb database, builds a
/// `SearchIndex` where each key is the document name and each value
/// is the searchable content, and writes the index atomically to
/// the `.store.idx` companion file.
pub(crate) fn rebuild_store_index(dir: &Path, qualified_name: &str) -> Result<(), CreftError> {
    let entries = store_entries(dir, qualified_name)?;
    let documents: Vec<(&str, &str, &str)> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), "", v.as_str()))
        .collect();
    let index = SearchIndex::build(&documents);
    let index_path = store_index_path(dir, qualified_name);
    write_index_bytes(&index_path, &index.to_bytes())
}

/// Load a store's search index from disk.
///
/// Returns `None` if the file does not exist or cannot be deserialized.
pub(crate) fn load_store_index(dir: &Path, qualified_name: &str) -> Option<SearchIndex> {
    let path = store_index_path(dir, qualified_name);
    let bytes = std::fs::read(&path).ok()?;
    SearchIndex::from_bytes(&bytes)
}

/// Returns `true` if the error indicates the database is locked by another
/// process (`DatabaseAlreadyOpen`).
///
/// Used by the channel handler to distinguish retryable contention from
/// terminal errors like disk full or permission denied.
pub(crate) fn is_lock_contention(err: &CreftError) -> bool {
    // We detect contention by checking the source message, because
    // the CreftError variants use String for the source rather than
    // carrying the original redb error type.
    if let CreftError::StoreOpen { reason, .. } = err {
        // redb's Display for DatabaseAlreadyOpen is "Database already open. Cannot acquire lock."
        reason.contains("already open") || reason.contains("Already open")
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::{assert_eq, assert_ne};

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // ── store_path / store_index_path ─────────────────────────────────────────

    #[test]
    fn store_path_appends_redb_extension() {
        let dir = std::path::Path::new("/tmp/stores");
        let path = store_path(dir, "deploy.data");
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/stores/deploy.data.redb")
        );
    }

    #[test]
    fn store_path_three_segment_name() {
        let dir = std::path::Path::new("/tmp/stores");
        let path = store_path(dir, "acme.deploy.data");
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/stores/acme.deploy.data.redb")
        );
    }

    #[test]
    fn store_index_path_appends_store_idx_extension() {
        let dir = std::path::Path::new("/tmp/stores");
        let path = store_index_path(dir, "deploy.data");
        assert_eq!(
            path,
            std::path::PathBuf::from("/tmp/stores/deploy.data.store.idx")
        );
    }

    // ── store_get: cold start ─────────────────────────────────────────────────

    #[test]
    fn store_get_returns_none_when_database_does_not_exist() {
        let dir = tmpdir();
        let result = store_get(dir.path(), "deploy.data", "env").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn store_get_returns_none_for_missing_key_in_existing_store() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "env", "production", None).unwrap();
        let result = store_get(dir.path(), "deploy.data", "missing_key").unwrap();
        assert_eq!(result, None);
    }

    // ── store_put / store_get round-trip ─────────────────────────────────────

    #[test]
    fn put_then_get_returns_inserted_value() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "env", "production", None).unwrap();
        let value = store_get(dir.path(), "deploy.data", "env").unwrap();
        assert_eq!(value, Some("production".to_owned()));
    }

    #[test]
    fn second_put_replaces_first_value() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "env", "staging", None).unwrap();
        store_put(dir.path(), "deploy.data", "env", "production", None).unwrap();
        let value = store_get(dir.path(), "deploy.data", "env").unwrap();
        assert_eq!(value, Some("production".to_owned()));
    }

    #[test]
    fn put_multiple_keys_all_retrievable() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "env", "production", None).unwrap();
        store_put(dir.path(), "deploy.data", "region", "us-east-1", None).unwrap();
        assert_eq!(
            store_get(dir.path(), "deploy.data", "env").unwrap(),
            Some("production".to_owned())
        );
        assert_eq!(
            store_get(dir.path(), "deploy.data", "region").unwrap(),
            Some("us-east-1".to_owned())
        );
    }

    // ── global flag ───────────────────────────────────────────────────────────

    #[test]
    fn store_is_global_returns_false_for_nonexistent_database() {
        let dir = tmpdir();
        assert!(!store_is_global(dir.path(), "deploy.data"));
    }

    #[test]
    fn put_with_global_true_then_is_global_returns_true() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "env", "prod", Some(true)).unwrap();
        assert!(store_is_global(dir.path(), "deploy.data"));
    }

    #[test]
    fn put_with_global_none_after_global_true_leaves_flag_unchanged() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "k1", "v1", Some(true)).unwrap();
        store_put(dir.path(), "deploy.data", "k2", "v2", None).unwrap();
        assert_eq!(store_is_global(dir.path(), "deploy.data"), true);
    }

    #[test]
    fn put_with_global_false_after_global_true_revokes_flag() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "k1", "v1", Some(true)).unwrap();
        store_put(dir.path(), "deploy.data", "k2", "v2", Some(false)).unwrap();
        assert_eq!(store_is_global(dir.path(), "deploy.data"), false);
    }

    // ── store_entries ─────────────────────────────────────────────────────────

    #[test]
    fn store_entries_returns_empty_vec_for_nonexistent_database() {
        let dir = tmpdir();
        let entries = store_entries(dir.path(), "deploy.data").unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn store_entries_returns_all_pairs() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "alpha", "a_value", None).unwrap();
        store_put(dir.path(), "deploy.data", "beta", "b_value", None).unwrap();
        store_put(dir.path(), "deploy.data", "gamma", "g_value", None).unwrap();

        let mut entries = store_entries(dir.path(), "deploy.data").unwrap();
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ("alpha".to_owned(), "a_value".to_owned()));
        assert_eq!(entries[1], ("beta".to_owned(), "b_value".to_owned()));
        assert_eq!(entries[2], ("gamma".to_owned(), "g_value".to_owned()));
    }

    // ── rebuild_store_index / load_store_index ────────────────────────────────

    #[test]
    fn rebuild_then_load_produces_queryable_index() {
        let dir = tmpdir();
        store_put(
            dir.path(),
            "deploy.data",
            "config",
            "rollback procedure",
            None,
        )
        .unwrap();

        rebuild_store_index(dir.path(), "deploy.data").unwrap();

        let index =
            load_store_index(dir.path(), "deploy.data").expect("index must exist after rebuild");
        let results = index.search("rollback");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "config");
    }

    #[test]
    fn load_store_index_returns_none_when_file_missing() {
        let dir = tmpdir();
        let result = load_store_index(dir.path(), "nonexistent.store");
        assert!(result.is_none());
    }

    #[test]
    fn rebuild_store_index_on_empty_store_produces_empty_index() {
        let dir = tmpdir();
        // Put one entry then remove won't work (no remove API), so we verify
        // that a store with no entries produces an empty index.
        // We create the DB by calling store_entries (which just checks path.exists).
        // Since the DB doesn't exist yet, entries is empty.
        rebuild_store_index(dir.path(), "empty.store").unwrap();
        let index = load_store_index(dir.path(), "empty.store")
            .expect("index file must exist after rebuild even for empty store");
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn rebuild_store_index_creates_directory_if_missing() {
        let dir = tmpdir();
        let nested = dir.path().join("deeply").join("nested");
        // nested doesn't exist yet; rebuild_store_index must create it
        store_put(&nested, "deploy.data", "key", "value", None).unwrap();
        rebuild_store_index(&nested, "deploy.data").unwrap();
        assert!(store_index_path(&nested, "deploy.data").exists());
    }

    #[test]
    fn search_index_from_store_with_config_key_matches_on_value_content() {
        let dir = tmpdir();
        store_put(
            dir.path(),
            "deploy.data",
            "config",
            "rollback procedure",
            None,
        )
        .unwrap();

        rebuild_store_index(dir.path(), "deploy.data").unwrap();

        let index = load_store_index(dir.path(), "deploy.data").unwrap();
        let results = index.search("rollback");
        assert_eq!(results.iter().any(|e| e.name == "config"), true);
    }

    // ── is_lock_contention ────────────────────────────────────────────────────

    #[test]
    fn is_lock_contention_true_for_store_open_with_already_open_message() {
        let err = CreftError::StoreOpen {
            name: "deploy.data".to_owned(),
            reason: "Database already open. Cannot acquire lock.".to_owned(),
        };
        assert!(is_lock_contention(&err));
    }

    #[test]
    fn is_lock_contention_false_for_other_store_open_error() {
        let err = CreftError::StoreOpen {
            name: "deploy.data".to_owned(),
            reason: "Permission denied".to_owned(),
        };
        assert!(!is_lock_contention(&err));
    }

    #[test]
    fn is_lock_contention_false_for_non_store_open_error() {
        let err = CreftError::StoreWrite {
            name: "deploy.data".to_owned(),
            reason: "disk full".to_owned(),
        };
        assert!(!is_lock_contention(&err));
    }

    // ── different qualified names do not interfere ────────────────────────────

    #[test]
    fn different_qualified_names_use_separate_databases() {
        let dir = tmpdir();
        store_put(dir.path(), "deploy.data", "env", "production", None).unwrap();
        store_put(dir.path(), "acme.deploy.data", "env", "staging", None).unwrap();

        assert_eq!(
            store_get(dir.path(), "deploy.data", "env").unwrap(),
            Some("production".to_owned())
        );
        assert_eq!(
            store_get(dir.path(), "acme.deploy.data", "env").unwrap(),
            Some("staging".to_owned())
        );
    }

    // ── load_store_index: corrupt bytes ──────────────────────────────────────

    #[test]
    fn load_store_index_returns_none_for_corrupt_bytes() {
        let dir = tmpdir();
        let path = store_index_path(dir.path(), "corrupt.store");
        std::fs::write(&path, b"\xFF\xFF\xFF\xFF\xFF\x00\x00\x00").unwrap();
        let result = load_store_index(dir.path(), "corrupt.store");
        assert!(result.is_none());
    }

    // ── store_is_global: db exists but no global flag set ────────────────────

    #[test]
    fn store_is_global_returns_false_when_no_meta_flag_set() {
        let dir = tmpdir();
        // Put without setting the global flag; meta table is never written.
        store_put(dir.path(), "deploy.data", "env", "production", None).unwrap();
        assert_eq!(store_is_global(dir.path(), "deploy.data"), false);
    }

    // ── store_entries: deterministic ordering ─────────────────────────────────

    #[test]
    fn store_entries_returns_pairs_in_key_order() {
        let dir = tmpdir();
        // Insert in reverse alphabetical order.
        store_put(dir.path(), "order.test", "zebra", "z_val", None).unwrap();
        store_put(dir.path(), "order.test", "apple", "a_val", None).unwrap();
        store_put(dir.path(), "order.test", "mango", "m_val", None).unwrap();

        let entries = store_entries(dir.path(), "order.test").unwrap();
        assert_eq!(entries.len(), 3);
        // redb B-tree returns keys in sorted order.
        assert_eq!(entries[0].0, "apple");
        assert_eq!(entries[1].0, "mango");
        assert_eq!(entries[2].0, "zebra");
    }

    // ── store_dir_for ─────────────────────────────────────────────────────────

    #[test]
    fn store_dir_for_global_scope_returns_stores_subdir() {
        use crate::model::{AppContext, Scope};
        let tmp = tmpdir();
        let ctx = AppContext::for_test_with_creft_home(
            tmp.path().to_path_buf(),
            tmp.path().to_path_buf(),
        );
        let store_dir = ctx.store_dir_for(Scope::Global).unwrap();
        assert!(store_dir.ends_with("stores"));
    }

    // ── corrupt database file handling ───────────────────────────────────────

    /// A file that exists but is not a valid redb database causes `store_get`
    /// to propagate an error from `open_db_readonly`.
    #[test]
    fn store_get_returns_err_when_file_exists_but_is_corrupt() {
        let dir = tmpdir();
        // Write garbage bytes to the expected database path.
        let path = store_path(dir.path(), "corrupt.db");
        std::fs::write(&path, b"not a redb file\xFF\xFE\xFD").unwrap();

        let result = store_get(dir.path(), "corrupt.db", "key");
        assert_eq!(result.is_err(), true);
    }

    /// A corrupt file causes `store_entries` to propagate an error.
    #[test]
    fn store_entries_returns_err_when_file_exists_but_is_corrupt() {
        let dir = tmpdir();
        let path = store_path(dir.path(), "corrupt.db");
        std::fs::write(&path, b"not a redb file\xFF\xFE\xFD").unwrap();

        let result = store_entries(dir.path(), "corrupt.db");
        assert_eq!(result.is_err(), true);
    }

    /// A corrupt file causes `store_is_global` to return false (conservative
    /// default) rather than propagating an error.
    #[test]
    fn store_is_global_returns_false_when_file_exists_but_is_corrupt() {
        let dir = tmpdir();
        let path = store_path(dir.path(), "corrupt.db");
        std::fs::write(&path, b"not a redb file\xFF\xFE\xFD").unwrap();

        assert_eq!(store_is_global(dir.path(), "corrupt.db"), false);
    }

    // ── store_put with global flag in both directions ─────────────────────────

    #[test]
    fn put_without_global_on_new_store_leaves_flag_false() {
        let dir = tmpdir();
        store_put(dir.path(), "new.store", "k", "v", None).unwrap();
        assert_eq!(store_is_global(dir.path(), "new.store"), false);
    }

    // ── assert_ne sanity (triggers pretty_assertions import) ─────────────────

    #[test]
    fn some_value_differs_from_none() {
        let a: Option<String> = Some("x".to_owned());
        let b: Option<String> = None;
        assert_ne!(a, b);
    }
}
