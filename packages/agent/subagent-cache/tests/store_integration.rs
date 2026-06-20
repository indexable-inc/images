//! Integration test for the storage layer against a real Postgres.
//!
//! Skipped unless `SUBAGENT_CACHE_TEST_DATABASE_URL` points at a throwaway
//! database (the only thing that can exercise the generated `tsvector` column
//! and `ts_rank`). Run locally with e.g.
//!   `SUBAGENT_CACHE_TEST_DATABASE_URL=postgres://localhost/sac_test` cargo test
//! The bootstrap is idempotent, so the test applies the schema twice.

use deadpool_postgres::{Manager, Pool};
use subagent_cache::store;
use subagent_cache::types::{FileDep, PopulateRequest};
use tokio_postgres::NoTls;

fn dep(path: &str, hash: &str) -> FileDep {
    FileDep { path: path.to_owned(), hash: hash.to_owned() }
}

#[tokio::test]
async fn populate_then_recall_roundtrip() {
    let Ok(url) = std::env::var("SUBAGENT_CACHE_TEST_DATABASE_URL") else {
        eprintln!("skipping: SUBAGENT_CACHE_TEST_DATABASE_URL not set");
        return;
    };

    let pg_config: tokio_postgres::Config = url.parse().expect("parse url");
    let pool: Pool = Pool::builder(Manager::new(pg_config, NoTls))
        .build()
        .expect("pool");
    pool.get()
        .await
        .expect("connect")
        .batch_execute("DROP TABLE IF EXISTS subagent_cache")
        .await
        .expect("drop");

    // Idempotent: bootstrap twice.
    store::bootstrap(&pool).await.expect("bootstrap 1");
    store::bootstrap(&pool).await.expect("bootstrap 2");

    let req = PopulateRequest {
        agent_type: "explore".into(),
        prompt: "how does the room turn lifecycle work".into(),
        findings: "the lifecycle is ...".into(),
        agent_def_hash: "xxh64:1111111111111111".into(),
        model: Some("claude-sonnet".into()),
        file_deps: vec![dep("crates/room/server/src/lib.rs", "xxh64:abc")],
    };
    store::populate(&pool, &req, 7).await.expect("populate");

    // Same key upserts in place (no duplicate row).
    store::populate(&pool, &req, 7).await.expect("re-populate");

    // Recall on overlapping keywords, matching agent_type + persona.
    let rows = store::recall(
        &pool,
        store::RecallParams {
            prompt: "room turn lifecycle",
            agent_type: "explore",
            agent_def_hash: "xxh64:1111111111111111",
            floor: 0.0,
            top_k: 3,
        },
    )
    .await
    .expect("recall");
    assert_eq!(rows.len(), 1, "exactly one live entry");
    assert_eq!(rows[0].findings, "the lifecycle is ...");
    assert_eq!(rows[0].file_deps.len(), 1);

    // Wrong persona => filtered out.
    let none = store::recall(
        &pool,
        store::RecallParams {
            prompt: "room turn lifecycle",
            agent_type: "explore",
            agent_def_hash: "xxh64:ffff",
            floor: 0.0,
            top_k: 3,
        },
    )
    .await
    .expect("recall persona");
    assert!(none.is_empty(), "persona mismatch must not recall");
}
