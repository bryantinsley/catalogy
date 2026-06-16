//! Integration tests for the config-management API handlers.
//!
//! These exercise `config_get_handler` / `config_put_handler` directly (with
//! real `State`/`Json` extractors) so the actual lock/merge/disk-persist logic
//! and the path-validation error mapping are covered — not just serde shapes.
//!
//! NB: `Catalog::open` builds its own tokio runtime (see AGENTS.md "runtime
//! gotcha"), so it cannot run inside a `#[tokio::test]` runtime. We build the
//! state on the (sync) test thread and drive the async handlers with an
//! explicit `block_on`, ensuring the catalog's runtime drops in a sync context.

use std::sync::{Arc, RwLock};

use axum::extract::{Json, State};
use axum::http::StatusCode;

use catalogy_core::Config;
use catalogy_server::api;
use catalogy_server::app::{AppState, ProgressState};

/// Build an `AppState` backed by temp dirs: a fresh LanceDB catalog, a
/// non-existent (yet) config path, and a default in-memory config.
fn test_state(tmp: &tempfile::TempDir) -> Arc<AppState> {
    let catalog_path = tmp.path().join("catalog.lance");
    let catalog = Arc::new(
        catalogy_catalog::Catalog::open(catalog_path.to_str().unwrap()).expect("open catalog"),
    );
    let config_path = tmp.path().join("config.toml");
    Arc::new(AppState {
        catalog,
        search_engine: None,
        state_db_path: None,
        model_dir: tmp.path().join("models"),
        data_dir: tmp.path().to_path_buf(),
        progress: std::sync::Mutex::new(ProgressState::default()),
        config: Arc::new(RwLock::new(Config::default())),
        config_path,
    })
}

/// Run an async body on a fresh runtime that is dropped before `state`, so the
/// catalog's own runtime never drops inside this runtime's `block_on`.
fn run<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fut)
}

#[test]
fn config_get_returns_current_config() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(&tmp);

    let Json(cfg) = run(api::config_get_handler(State(state.clone()))).expect("get config");

    // Default config has no library paths configured.
    assert!(cfg.library.paths.is_empty());
}

#[test]
fn config_put_persists_and_is_readable_back() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(&tmp);

    // A real, existing directory is a valid library path.
    let lib = tempfile::tempdir().unwrap();
    let lib_path = lib.path().to_string_lossy().to_string();

    let mut update = Config::default();
    update.library.paths = vec![lib_path.clone()];

    let Json(resp) =
        run(api::config_put_handler(State(state.clone()), Json(update))).expect("put config");
    assert!(resp.ok);

    // In-memory state reflects the update.
    let Json(cfg) = run(api::config_get_handler(State(state.clone()))).expect("get config");
    assert_eq!(cfg.library.paths, vec![lib_path.clone()]);

    // And it was persisted to disk: a fresh load sees the path.
    assert!(state.config_path.exists(), "config file should be written");
    let on_disk = Config::from_file(&state.config_path.to_string_lossy())
        .expect("reload config from disk");
    assert_eq!(on_disk.library.paths, vec![lib_path]);
}

#[test]
fn config_put_rejects_nonexistent_library_path() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(&tmp);

    let mut update = Config::default();
    update.library.paths = vec!["/definitely/not/a/real/path/xyzzy".to_string()];

    let status = match run(api::config_put_handler(State(state.clone()), Json(update))) {
        Ok(_) => panic!("nonexistent path should be rejected"),
        Err((status, _msg)) => status,
    };
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Nothing should have been persisted on a rejected update.
    assert!(
        !state.config_path.exists(),
        "config file must not be written on validation failure"
    );
}
