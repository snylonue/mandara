//! Durable, instance-scoped storage for the plugin state import.

use std::sync::Arc;

use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::{AsyncConnection, RunQueryDsl};
use mandara_plugin::state::{MAX_STATE_BYTES, MAX_STATE_ENTRIES};
use mandara_plugin::{StateStore, StateStoreError};

use crate::db::DieselDb;
use crate::schema::plugin_state;

/// The namespace is selected by the server, never passed by the guest.
pub(super) struct DbPluginState {
    db: DieselDb,
    instance: String,
    rt: tokio::runtime::Handle,
}

impl DbPluginState {
    const MAX_STATE_VALUE_BYTES: usize = 256 * 1024;

    pub(super) fn new(db: DieselDb, instance: String) -> Arc<Self> {
        Arc::new(Self {
            db,
            instance,
            rt: tokio::runtime::Handle::current(),
        })
    }
}

fn storage_error(error: anyhow::Error) -> StateStoreError {
    error
        .downcast::<StateStoreError>()
        .unwrap_or_else(StateStoreError::Backend)
}

impl StateStore for DbPluginState {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StateStoreError> {
        self.rt
            .block_on(async {
                let mut conn = self.db.get().await?;
                Ok(plugin_state::table
                    .find((&self.instance, key))
                    .select(plugin_state::value)
                    .first(&mut conn)
                    .await
                    .optional()?)
            })
            .map_err(storage_error)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), StateStoreError> {
        if value.len() > Self::MAX_STATE_VALUE_BYTES {
            return Err(StateStoreError::QuotaExceeded);
        }
        self.rt
            .block_on(async {
                let mut conn = self.db.get().await?;
                conn.transaction::<_, anyhow::Error, _>(async |conn| {
                    // Write first to acquire SQLite's writer lock before checking
                    // quotas. A rejected write rolls back, preserving the old value.
                    diesel::insert_into(plugin_state::table)
                        .values((
                            plugin_state::instance_id.eq(&self.instance),
                            plugin_state::key.eq(key),
                            plugin_state::value.eq(value),
                        ))
                        .on_conflict((plugin_state::instance_id, plugin_state::key))
                        .do_update()
                        .set(plugin_state::value.eq(value))
                        .execute(&mut *conn)
                        .await?;
                    let (count, bytes): (i64, i64) = plugin_state::table
                        .filter(plugin_state::instance_id.eq(&self.instance))
                        .select((
                            diesel::dsl::count_star(),
                            diesel::dsl::sql::<diesel::sql_types::BigInt>(
                                "coalesce(sum(length(CAST(key AS BLOB)) + length(value)), 0)",
                            ),
                        ))
                        .first(conn)
                        .await?;
                    if count > MAX_STATE_ENTRIES as i64 || bytes > MAX_STATE_BYTES as i64 {
                        return Err(StateStoreError::QuotaExceeded.into());
                    }
                    Ok(())
                })
                .await
            })
            .map_err(storage_error)
    }

    fn delete(&self, key: &str) -> Result<bool, StateStoreError> {
        self.rt
            .block_on(async {
                let mut conn = self.db.get().await?;
                Ok(
                    diesel::delete(plugin_state::table.find((&self.instance, key)))
                        .execute(&mut conn)
                        .await?
                        != 0,
                )
            })
            .map_err(storage_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diesel_async::SimpleAsyncConnection;
    use std::path::PathBuf;

    async fn fixture() -> (PathBuf, DieselDb) {
        let dir = std::env::temp_dir().join(format!("mandara-state-{}", uuid::Uuid::new_v4()));
        crate::db::run_migrations(&dir.join("test.db"))
            .await
            .unwrap();
        let dir = dir.canonicalize().unwrap();
        let db = crate::db::connect_diesel(&dir.join("test.db")).unwrap();
        db.get().await.unwrap().batch_execute(
            "INSERT INTO plugin_instances (id, wasm_file) VALUES ('first','hello.wasm'), ('second','hello.wasm');",
        ).await.unwrap();
        (dir, db)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn state_is_binary_scoped_and_durable() {
        let (dir, db) = fixture().await;
        let first = DbPluginState::new(db.clone(), "first".into());
        let second = DbPluginState::new(db.clone(), "second".into());
        tokio::task::spawn_blocking(move || {
            assert_eq!(first.get("目录").unwrap(), None);
            first.set("目录", &[0, 255, 128]).unwrap();
            assert_eq!(first.get("目录").unwrap(), Some(vec![0, 255, 128]));
            assert_eq!(second.get("目录").unwrap(), None);
            second.set("目录", &[]).unwrap();
            assert_eq!(second.get("目录").unwrap(), Some(vec![]));
            first.set("目录", &[254, 0]).unwrap();
            for key in [String::new(), "目录".repeat(1024)] {
                first.set(&key, &[42]).unwrap();
                assert_eq!(first.get(&key).unwrap(), Some(vec![42]));
                assert!(first.delete(&key).unwrap());
            }
        })
        .await
        .unwrap();
        drop(db);
        // A new pool and state handle simulate opening the persisted DB again.
        let db = crate::db::connect_diesel(&dir.join("test.db")).unwrap();
        let first = DbPluginState::new(db.clone(), "first".into());
        let second = DbPluginState::new(db.clone(), "second".into());
        tokio::task::spawn_blocking(move || {
            assert_eq!(first.get("目录").unwrap(), Some(vec![254, 0]));
            assert_eq!(second.get("目录").unwrap(), Some(vec![]));
            assert!(first.delete("目录").unwrap());
            assert!(!first.delete("目录").unwrap());
            assert_eq!(first.get("目录").unwrap(), None);
        })
        .await
        .unwrap();
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn value_size_limit_is_enforced_by_sqlite_store() {
        let (dir, db) = fixture().await;
        let state = DbPluginState::new(db.clone(), "first".into());
        tokio::task::spawn_blocking(move || {
            let bytes = vec![1; DbPluginState::MAX_STATE_VALUE_BYTES];
            state.set("key", &bytes).unwrap();
            let oversized = vec![2; DbPluginState::MAX_STATE_VALUE_BYTES + 1];
            for key in ["key", "missing"] {
                assert!(matches!(
                    state.set(key, &oversized),
                    Err(StateStoreError::QuotaExceeded)
                ));
            }
            assert_eq!(state.get("key").unwrap(), Some(bytes));
            assert_eq!(state.get("missing").unwrap(), None);
        })
        .await
        .unwrap();
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quotas_rollback_and_delete_reclaims_space() {
        let (dir, db) = fixture().await;
        let first = DbPluginState::new(db.clone(), "first".into());
        tokio::task::spawn_blocking(move || {
            let large = vec![1; DbPluginState::MAX_STATE_VALUE_BYTES];
            for i in 0..63 {
                first.set(&format!("bulk:{i}"), &large).unwrap();
            }
            first.set("keep", &[7]).unwrap();
            assert!(matches!(
                first.set("keep", &large),
                Err(StateStoreError::QuotaExceeded)
            ));
            assert_eq!(first.get("keep").unwrap(), Some(vec![7]));
            assert!(matches!(
                first.set("extra", &large),
                Err(StateStoreError::QuotaExceeded)
            ));
            assert_eq!(first.get("extra").unwrap(), None);
            assert!(first.delete("bulk:0").unwrap());
            first.set("keep", &large).unwrap();
        })
        .await
        .unwrap();
        let mut conn = db.get().await.unwrap();
        let sql: String = (0..MAX_STATE_ENTRIES)
            .map(|i| format!("INSERT INTO plugin_state VALUES('second','key:{i}',X'');"))
            .collect();
        conn.batch_execute(&sql).await.unwrap();
        drop(conn);
        let second = DbPluginState::new(db.clone(), "second".into());
        tokio::task::spawn_blocking(move || {
            assert!(matches!(
                second.set("one-too-many", &[]),
                Err(StateStoreError::QuotaExceeded)
            ));
            assert_eq!(second.get("one-too-many").unwrap(), None);
            second.set("key:0", &[2]).unwrap();
            assert!(second.delete("key:1").unwrap());
            second.set("one-too-many", &[]).unwrap();
        })
        .await
        .unwrap();
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_writes_do_not_overshoot_quota() {
        let (dir, db) = fixture().await;
        let state = DbPluginState::new(db.clone(), "first".into());
        let seed = state.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = vec![0; DbPluginState::MAX_STATE_VALUE_BYTES];
            for i in 0..62 {
                seed.set(&format!("seed:{i}"), &bytes).unwrap();
            }
        })
        .await
        .unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut jobs = Vec::new();
        for key in ["race:a", "race:b"] {
            let state = state.clone();
            let barrier = barrier.clone();
            jobs.push(tokio::task::spawn_blocking(move || {
                barrier.wait();
                state.set(key, &vec![0; DbPluginState::MAX_STATE_VALUE_BYTES])
            }));
        }
        let mut successes = 0;
        for job in jobs {
            match job.await.unwrap() {
                Ok(()) => successes += 1,
                Err(StateStoreError::QuotaExceeded) => {}
                other => panic!("unexpected write result: {other:?}"),
            }
        }
        assert_eq!(successes, 1);
        drop(state);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    struct NoImages;
    impl mandara_plugin::ImageStore for NoImages {
        fn store_image(
            &self,
            _: &[u8],
            _: &str,
        ) -> Result<String, mandara_plugin::ImageStoreError> {
            panic!("state test does not store images")
        }
    }

    async fn service(db: DieselDb) -> super::super::plugins::PluginService {
        let runtime = Arc::new(
            mandara_plugin::PluginRuntime::new(
                mandara_plugin::FetchPolicy::default(),
                Arc::new(NoImages),
            )
            .unwrap(),
        );
        let wasm = runtime
            .compile(
                "hello.wasm",
                include_bytes!("../../../mandara-plugin/tests/fixtures/hello.wasm"),
            )
            .unwrap();
        super::super::plugins::PluginService::new(db, runtime, vec![wasm])
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plugin_information_uses_state_only_after_registration() {
        let (dir, db) = fixture().await;
        let first = DbPluginState::new(db.clone(), "first".into());
        let second = DbPluginState::new(db.clone(), "second".into());
        let plugins = service(db.clone()).await;
        tokio::task::spawn_blocking(move || {
            // Constructing the compiled-file registry must not call exports.
            assert_eq!(first.get("demo:metadata-called").unwrap(), None);
            assert_eq!(second.get("demo:metadata-called").unwrap(), None);
            first.set("demo:name", b"first-instance").unwrap();
            second.set("demo:name", b"second-instance").unwrap();
            first.set("demo:no-default", &[]).unwrap();
        })
        .await
        .unwrap();
        let infos = plugins.infos().await.unwrap();
        assert_eq!(
            infos.iter().find(|i| i.id == "first").unwrap().name,
            "first-instance"
        );
        assert_eq!(
            infos.iter().find(|i| i.id == "second").unwrap().name,
            "second-instance"
        );
        let (fields, config) = plugins.config_schema("first").await.unwrap();
        assert_eq!(config, serde_json::json!({}));
        assert!(fields[0].required);
        assert!(fields[0].default.is_none());
        // Missing required fields are rejected only when saving configuration.
        assert!(
            plugins
                .set_config("first", serde_json::json!({}))
                .await
                .is_err()
        );
        plugins
            .set_config("first", serde_json::json!({"site-name":"站点"}))
            .await
            .unwrap();
        plugins.register("new", "hello.wasm").await.unwrap();
        assert_eq!(
            plugins.config_schema("new").await.unwrap().1,
            serde_json::json!({})
        );
        let state = DbPluginState::new(db.clone(), "new".into());
        tokio::task::spawn_blocking(move || {
            assert_eq!(state.get("demo:metadata-called").unwrap(), Some(vec![]));
        })
        .await
        .unwrap();
        assert!(plugins.register("new", "hello.wasm").await.is_err());
        drop(plugins);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn service_binds_instance_state_and_unregister_cascades() {
        let (dir, db) = fixture().await;
        let plugins = service(db.clone()).await;
        assert_eq!(
            plugins
                .get_chapter("first", "state:key", 1)
                .await
                .unwrap()
                .unwrap()
                .content,
            "Ok(())"
        );
        assert_eq!(
            plugins
                .get_chapter("second", "state:key", 0)
                .await
                .unwrap()
                .unwrap()
                .content,
            "Ok(None)"
        );
        let expected = plugins
            .get_chapter("first", "state:key", 0)
            .await
            .unwrap()
            .unwrap()
            .content;
        assert!(expected.starts_with("Ok(Some("));
        plugins
            .set_config("first", serde_json::json!({"site-name":"新的配置"}))
            .await
            .unwrap();
        plugins.set_enabled("first", false).await.unwrap();
        assert_eq!(
            plugins
                .get_chapter("first", "state:key", 0)
                .await
                .unwrap()
                .unwrap()
                .content,
            expected
        );
        drop(plugins);
        // Loading a replacement compiled component keeps the instance namespace.
        let plugins = service(db.clone()).await;
        assert_eq!(
            plugins
                .get_chapter("first", "state:key", 0)
                .await
                .unwrap()
                .unwrap()
                .content,
            expected
        );
        let stale = DbPluginState::new(db.clone(), "first".into());
        plugins.unregister("first").await.unwrap();
        tokio::task::spawn_blocking(move || {
            assert_eq!(stale.get("key").unwrap(), None);
            assert!(stale.set("key", &[1]).is_err());
        })
        .await
        .unwrap();
        assert_eq!(
            plugin_state::table
                .filter(plugin_state::instance_id.eq("first"))
                .count()
                .get_result::<i64>(&mut db.get().await.unwrap())
                .await
                .unwrap(),
            0
        );
        plugins.register("first", "hello.wasm").await.unwrap();
        assert_eq!(
            plugins
                .get_chapter("first", "state:key", 0)
                .await
                .unwrap()
                .unwrap()
                .content,
            "Ok(None)"
        );
        drop(plugins);
        drop(db);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
