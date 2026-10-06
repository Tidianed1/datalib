//! What a download owes, and the loop that fetches it.
//!
//! Upstream *lists* records, each at a version: an update time, a
//! newest-reply stamp, a hash of its listing entry, a date a day is
//! final from. What we *hold* is recorded beside the record, as the
//! version its content satisfies (`held_version` in the table's
//! `_bookkeeping` sidecar). What is *owed* is the difference, asked of
//! the store each time and never stored: docs/dev/plans/sync_state.md.
//!
//! The listing can come from anywhere: a delta's answer, an
//! enumeration's pages, a calendar, the rows of another table, or the
//! diff between two commits of a store. [`drain`] fetches what is owed
//! in batches, writes each batch in one transaction with the versions
//! it satisfied, and records what did not come.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context, Result};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::raw_store::Sealer;
use crate::run_problems::RunProblems;
use crate::stop::StopFlag;

/// A record upstream lists, at the version it lists it at. `None` is a
/// listing with no version: the record only has to be held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub key: String,
    pub version: Option<String>,
}

impl Listed {
    pub fn new(key: impl Into<String>, version: Option<impl Into<String>>) -> Self {
        Self {
            key: key.into(),
            version: version.map(Into::into),
        }
    }
}

/// Of `listed`, the records `table` does not hold at that version:
/// those with no sidecar row, and those whose `held_version` differs.
/// Order is the listing's.
pub async fn owed(pool: &SqlitePool, table: &str, listed: Vec<Listed>) -> Result<Vec<Listed>> {
    let held = held_versions(pool, table, listed.iter().map(|l| l.key.as_str())).await?;
    Ok(listed
        .into_iter()
        .filter(|l| match held.get(&l.key) {
            None => true,
            Some(held) => *held != l.version,
        })
        .collect())
}

/// `key → held_version` for the keys of `table` that have a sidecar row.
pub async fn held_versions<'k>(
    pool: &SqlitePool,
    table: &str,
    keys: impl IntoIterator<Item = &'k str>,
) -> Result<HashMap<String, Option<String>>> {
    let keys: Vec<&str> = keys.into_iter().collect();
    let mut out = HashMap::with_capacity(keys.len());
    for chunk in keys.chunks(crate::bulk::SQL_CHUNK) {
        // Audited: `table` is a provider's `&'static str`; the
        // placeholders are one `?` per key, every key bound.
        let sql = format!(
            "SELECT id, held_version FROM {table}_bookkeeping WHERE id IN ({})",
            vec!["?"; chunk.len()].join(",")
        );
        let mut q = sqlx::query_as::<_, (String, Option<String>)>(sqlx::AssertSqlSafe(sql));
        for key in chunk {
            q = q.bind(*key);
        }
        let rows = q
            .fetch_all(pool)
            .await
            .with_context(|| format!("read what {table} holds"))?;
        out.extend(rows);
    }
    Ok(out)
}

/// Record that `table`'s content for `key` now satisfies `version`: in
/// the transaction that wrote the content. Clears the record's attempts
/// and its fetch problem.
pub async fn hold(
    tx: &mut Transaction<'_, Sqlite>,
    table: &str,
    key: &str,
    version: Option<&str>,
) -> Result<()> {
    crate::doltlite_raw::record_object_attempt(tx, table, key, None).await?;
    // Audited: `table` is a provider's `&'static str`; values bound.
    let sql = format!("UPDATE {table}_bookkeeping SET held_version = ? WHERE id = ?");
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(version)
        .bind(key)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("hold {table}={key}"))?;
    Ok(())
}

/// Upstream no longer has `key`: its sidecar row and its fetch problem
/// go, in the transaction that removes its content.
pub async fn forget(tx: &mut Transaction<'_, Sqlite>, table: &str, key: &str) -> Result<()> {
    // Audited: `table` is a provider's `&'static str`; the key is bound.
    let sql = format!("DELETE FROM {table}_bookkeeping WHERE id = ?");
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(key)
        .execute(&mut **tx)
        .await
        .with_context(|| format!("forget {table}={key}"))?;
    crate::prune::forget_problems_in_tx(tx, table, "?", std::slice::from_ref(&key.to_string()))
        .await
}

/// What one fetch of one owed record came to.
#[derive(Debug)]
pub enum Outcome<T> {
    /// Fetched: `store` writes it, and the record is held at the
    /// version it was listed at.
    Got(T),
    /// Upstream no longer has it: `store` removes it, and nothing holds
    /// it any more.
    Gone,
    /// It did not come. The record stays owed, with one more attempt and
    /// this as its problem.
    Failed(String),
}

/// One owed record and what its fetch came to.
#[derive(Debug)]
pub struct Fetched<T> {
    pub listed: Listed,
    pub outcome: Outcome<T>,
}

/// Why a whole batch did not come back.
#[derive(Debug)]
pub enum BatchError {
    /// This batch failed; every record in it stays owed with one more
    /// attempt, and the loop goes on.
    Batch(anyhow::Error),
    /// Nothing after this will fare better (a refused credential, the
    /// retry loop giving up): the loop ends, as a `phase:` row.
    Terminal(anyhow::Error),
}

/// What `store` returns: a future that may borrow the transaction and
/// the batch it was given. `Box::pin(async move { .. })`.
pub type StoreFut<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

/// What the loop is working for.
pub struct Loop<'a> {
    pub pool: &'a SqlitePool,
    /// The table whose sidecar holds the versions, and whose key the
    /// records are listed by.
    pub table: &'static str,
    /// Names the `phase:` row when the loop ends early.
    pub phase: &'a str,
    pub stop: &'a StopFlag,
    pub found: &'a RunProblems,
    pub sealer: Option<&'a Sealer>,
    /// Records per fetch.
    pub batch: usize,
    /// Batches in a row that came to nothing before the loop gives up
    /// on this run; `0` for never.
    pub failures_in_a_row: usize,
}

/// How a [`drain`] ended.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drained {
    pub got: usize,
    pub gone: usize,
    pub failed: usize,
    /// Owed records the loop did not reach: it was stopped, gave up, or
    /// met a terminal error.
    pub left: usize,
}

/// Fetch `owed` in batches of `Loop::batch`: `fetch` asks upstream for
/// one batch and says what each record came to; `store` writes a
/// batch's content in the transaction it is given, which then also
/// records what is held, what is gone and what failed, commits, and
/// tells the sealer. A record `fetch` leaves out of its answer is read
/// as failed. The loop ends early on a stop (nothing recorded for the
/// batch under way), on `Loop::failures_in_a_row` fruitless batches in
/// a row, or on a terminal error; the last two are a `phase:` row and
/// `cut_short`, and the run goes on.
pub async fn drain<T, F, Fut, S>(
    l: &Loop<'_>,
    owed: Vec<Listed>,
    mut fetch: F,
    mut store: S,
) -> Result<Drained>
where
    T: Send + Sync,
    F: FnMut(Vec<Listed>) -> Fut,
    Fut: Future<Output = std::result::Result<Vec<Fetched<T>>, BatchError>>,
    S: for<'a> FnMut(&'a mut Transaction<'static, Sqlite>, &'a [Fetched<T>]) -> StoreFut<'a>,
{
    let mut done = Drained::default();
    let total = owed.len();
    let batch = l.batch.max(1);
    let mut fruitless = 0usize;
    let mut batches = owed.into_iter().peekable();
    while batches.peek().is_some() {
        if l.stop.requested() {
            break;
        }
        let this: Vec<Listed> = batches.by_ref().take(batch).collect();
        let answered = match fetch(this.clone()).await {
            Ok(answered) => answered,
            Err(_) if l.stop.requested() => break,
            Err(BatchError::Batch(e)) => {
                let said = format!("{e:#}");
                let mut tx = l.pool.begin().await.context("begin a failed batch")?;
                for listed in &this {
                    crate::doltlite_raw::record_object_error(&mut tx, l.table, &listed.key, &said)
                        .await?;
                }
                tx.commit().await.context("commit a failed batch")?;
                done.failed += this.len();
                fruitless += 1;
                if l.failures_in_a_row > 0 && fruitless >= l.failures_in_a_row {
                    give_up(l, &done, total, &said);
                    break;
                }
                continue;
            }
            Err(BatchError::Terminal(e)) => {
                give_up(l, &done, total, &format!("{e:#}"));
                break;
            }
        };
        let answered = with_the_unanswered(this, answered);
        let mut tx = l.pool.begin().await.context("begin a batch")?;
        store(&mut tx, &answered).await?;
        let mut fruitful = false;
        for f in &answered {
            match &f.outcome {
                Outcome::Got(_) => {
                    hold(&mut tx, l.table, &f.listed.key, f.listed.version.as_deref()).await?;
                    done.got += 1;
                    fruitful = true;
                }
                Outcome::Gone => {
                    forget(&mut tx, l.table, &f.listed.key).await?;
                    done.gone += 1;
                    fruitful = true;
                }
                Outcome::Failed(said) => {
                    crate::doltlite_raw::record_object_error(&mut tx, l.table, &f.listed.key, said)
                        .await?;
                    done.failed += 1;
                }
            }
        }
        tx.commit().await.context("commit a batch")?;
        if let Some(sealer) = l.sealer {
            sealer.wrote(answered.len() as u64).await;
        }
        if fruitful {
            fruitless = 0;
        } else {
            fruitless += 1;
            if l.failures_in_a_row > 0 && fruitless >= l.failures_in_a_row {
                let said = answered
                    .iter()
                    .find_map(|f| match &f.outcome {
                        Outcome::Failed(said) => Some(said.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                give_up(l, &done, total, &said);
                break;
            }
        }
    }
    done.left = total - done.got - done.gone - done.failed;
    Ok(done)
}

fn give_up(l: &Loop<'_>, done: &Drained, total: usize, said: &str) {
    let left = total - done.got - done.gone - done.failed;
    l.found.phase(
        l.phase,
        format!(
            "stopped after {} failed in a row; {} fetched, {left} left for the next run: {said}",
            l.failures_in_a_row.max(1).min(done.failed.max(1)),
            done.got
        ),
    );
    l.found.cut_short();
}

/// Every record of `asked` with its outcome, the ones `fetch` said
/// nothing about as failed.
fn with_the_unanswered<T>(asked: Vec<Listed>, answered: Vec<Fetched<T>>) -> Vec<Fetched<T>> {
    let mut out = answered;
    for listed in asked {
        if !out.iter().any(|f| f.listed.key == listed.key) {
            out.push(Fetched {
                listed,
                outcome: Outcome::Failed("the fetch said nothing about it".to_string()),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "things";

    async fn store_at(dir: &tempfile::TempDir) -> SqlitePool {
        let ddl = [
            "CREATE TABLE IF NOT EXISTS things (id TEXT PRIMARY KEY, body TEXT NULL)",
            &crate::doltlite_raw::bookkeeping_ddl_for(T),
        ];
        let ddl: Vec<&str> = ddl.iter().map(|s| &**s).collect();
        crate::doltlite_raw::open(&dir.path().join("o.doltlite_db"), &ddl)
            .await
            .unwrap()
    }

    fn listed(key: &str, version: Option<&str>) -> Listed {
        Listed::new(key, version)
    }

    async fn bodies(pool: &SqlitePool) -> Vec<(String, Option<String>)> {
        sqlx::query_as("SELECT id, body FROM things ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    async fn problems(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT scope_key FROM problems ORDER BY scope_key")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    fn a_loop<'a>(
        pool: &'a SqlitePool,
        stop: &'a StopFlag,
        found: &'a RunProblems,
        batch: usize,
        budget: usize,
    ) -> Loop<'a> {
        Loop {
            pool,
            table: T,
            phase: "things",
            stop,
            found,
            sealer: None,
            batch,
            failures_in_a_row: budget,
        }
    }

    fn write_bodies<'a>(
        tx: &'a mut Transaction<'static, Sqlite>,
        fetched: &'a [Fetched<String>],
    ) -> StoreFut<'a> {
        Box::pin(async move {
            for f in fetched {
                match &f.outcome {
                    Outcome::Got(body) => {
                        sqlx::query("INSERT INTO things (id, body) VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET body = excluded.body")
                        .bind(&f.listed.key)
                        .bind(body)
                        .execute(&mut **tx)
                        .await?;
                    }
                    Outcome::Gone => {
                        sqlx::query("DELETE FROM things WHERE id = ?")
                            .bind(&f.listed.key)
                            .execute(&mut **tx)
                            .await?;
                    }
                    Outcome::Failed(_) => {}
                }
            }
            Ok(())
        })
    }

    /// The whole rule in one place: a record is owed until its content
    /// is held at the version it is listed at, and again once the
    /// listing moves on. A version of `None` only asks that it be held.
    #[tokio::test]
    async fn a_record_is_owed_until_held_at_its_listed_version() {
        let d = tempfile::tempdir().unwrap();
        let pool = store_at(&d).await;
        let stop = StopFlag::new();
        let found = RunProblems::unwritten();
        let l = a_loop(&pool, &stop, &found, 10, 0);
        let listing = || {
            vec![
                listed("picard", Some("v1")),
                listed("riker", Some("v1")),
                listed("data", None),
            ]
        };
        let first = owed(&pool, T, listing()).await.unwrap();
        assert_eq!(first.len(), 3);

        let drained = drain(
            &l,
            first,
            |batch| async move {
                Ok(batch
                    .into_iter()
                    .map(|listed| Fetched {
                        outcome: Outcome::Got(format!("log of {}", listed.key)),
                        listed,
                    })
                    .collect())
            },
            write_bodies,
        )
        .await
        .unwrap();
        assert_eq!(
            drained,
            Drained {
                got: 3,
                ..Default::default()
            }
        );
        assert!(owed(&pool, T, listing()).await.unwrap().is_empty());

        // The listing moves one record on.
        let later = vec![
            listed("picard", Some("v2")),
            listed("riker", Some("v1")),
            listed("data", None),
        ];
        assert_eq!(
            owed(&pool, T, later).await.unwrap(),
            [listed("picard", Some("v2"))]
        );
        pool.close().await;
    }

    /// A batch is one call with an outcome per record. A record that
    /// failed stays owed with an attempt and a problem row; one that is
    /// gone loses its content and holds nothing; one the fetch said
    /// nothing about is a failure, not a success. All of it lands in the
    /// batch's one transaction.
    #[tokio::test]
    async fn each_record_of_a_batch_has_its_own_outcome() {
        let d = tempfile::tempdir().unwrap();
        let pool = store_at(&d).await;
        let stop = StopFlag::new();
        let found = RunProblems::unwritten();
        let l = a_loop(&pool, &stop, &found, 10, 0);
        let listing = vec![
            listed("picard", Some("v1")),
            listed("riker", Some("v1")),
            listed("worf", Some("v1")),
            listed("data", Some("v1")),
        ];
        drain(
            &l,
            listing.clone(),
            |batch| async move {
                Ok(batch
                    .into_iter()
                    .filter_map(|listed| {
                        let outcome = match listed.key.as_str() {
                            "picard" => Outcome::Got("log".to_string()),
                            "riker" => Outcome::Failed("HTTP 500".to_string()),
                            "worf" => Outcome::Gone,
                            _ => return None,
                        };
                        Some(Fetched { listed, outcome })
                    })
                    .collect())
            },
            write_bodies,
        )
        .await
        .unwrap();
        assert_eq!(
            bodies(&pool).await,
            [
                ("data".to_string(), None),
                ("picard".to_string(), Some("log".to_string())),
                ("riker".to_string(), None)
            ],
            "a failed record keeps the id-only stub every fetch problem has"
        );
        assert_eq!(
            owed(&pool, T, listing.clone())
                .await
                .unwrap()
                .iter()
                .map(|l| l.key.as_str())
                .collect::<Vec<_>>(),
            ["riker", "worf", "data"]
        );
        assert_eq!(
            problems(&pool).await,
            ["things:data", "things:riker"],
            "a failure is a row a person sees; a gone record is not"
        );
        let attempts: Vec<(String, i64)> =
            sqlx::query_as("SELECT id, attempt_count FROM things_bookkeeping ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            attempts,
            [
                ("data".to_string(), 1),
                ("picard".to_string(), 1),
                ("riker".to_string(), 1)
            ]
        );

        // The next run gets riker; its problem row goes with its attempt.
        drain(
            &l,
            owed(&pool, T, listing.clone()).await.unwrap(),
            |batch| async move {
                Ok(batch
                    .into_iter()
                    .map(|listed| Fetched {
                        outcome: Outcome::Got("log".to_string()),
                        listed,
                    })
                    .collect())
            },
            write_bodies,
        )
        .await
        .unwrap();
        assert!(problems(&pool).await.is_empty());
        assert!(owed(&pool, T, listing).await.unwrap().is_empty());
        pool.close().await;
    }

    /// Fruitless batches in a row end the run's loop with one `phase:`
    /// row and leave the rest owed; a terminal error does the same at
    /// once. A stop records nothing for the batch under way. The records
    /// not reached are counted as left.
    #[tokio::test]
    async fn the_loop_gives_up_as_a_phase_row_and_leaves_the_rest_owed() {
        let d = tempfile::tempdir().unwrap();
        let pool = store_at(&d).await;
        let stop = StopFlag::new();
        let found = RunProblems::unwritten();
        let l = a_loop(&pool, &stop, &found, 1, 2);
        let listing: Vec<Listed> = ["a", "b", "c", "d"]
            .iter()
            .map(|k| listed(k, Some("v1")))
            .collect();

        let drained = drain(
            &l,
            listing.clone(),
            |_| async { Err(BatchError::Batch(anyhow::anyhow!("HTTP 503"))) },
            write_bodies,
        )
        .await
        .unwrap();
        assert_eq!(
            drained,
            Drained {
                failed: 2,
                left: 2,
                ..Default::default()
            }
        );
        let phases = found.run_problems();
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].key(), "phase:things");
        assert!(phases[0].detail.contains("2 left"), "{}", phases[0].detail);
        assert_eq!(problems(&pool).await, ["things:a", "things:b"]);

        let found = RunProblems::unwritten();
        let l = a_loop(&pool, &stop, &found, 1, 0);
        let drained = drain(
            &l,
            listing.clone(),
            |_| async { Err(BatchError::Terminal(anyhow::anyhow!("HTTP 401"))) },
            write_bodies,
        )
        .await
        .unwrap();
        assert_eq!(drained.left, 4);
        assert_eq!(found.run_problems().len(), 1);

        let found = RunProblems::unwritten();
        let l = a_loop(&pool, &stop, &found, 1, 0);
        let stop_at = stop.clone();
        let drained = drain(
            &l,
            listing,
            move |batch| {
                let stop_at = stop_at.clone();
                async move {
                    stop_at.request();
                    Err(BatchError::Batch(anyhow::anyhow!(
                        "interrupted asking for {}",
                        batch[0].key
                    )))
                }
            },
            write_bodies,
        )
        .await
        .unwrap();
        assert_eq!(drained.left, 4, "a stop is not a failure of any record");
        assert!(found.run_problems().is_empty());
        pool.close().await;
    }
}
