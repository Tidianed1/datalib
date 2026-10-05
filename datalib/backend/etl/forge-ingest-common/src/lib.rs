//! What the forge providers' downloads (github, gitlab) share. A forge
//! sync reads the account it runs as, discovers every change request —
//! pull request or merge request — the person is on through a handful
//! of searches, and fetches each one with its comments. [`sync`] is that
//! run; a [`Forge`] is what one forge does differently.

pub mod client;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use datalib_etl::download_problems::RunProblem;
use datalib_etl::download_run::DownloadRun;
use datalib_etl::progress::Progress;
use datalib_etl::stop::StopFlag;
use datalib_time::IsoOffsetTimestamp;
use serde::Serialize;
use serde_json::Value;
use sqlx::SqlitePool;

pub use client::{ForgeClient, ForgeError, LATCHKEY_TIMEOUT, PER_PAGE};

/// A change request a search listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The repository or project.
    pub container: String,
    pub number: u32,
    /// When the forge last saw it change, from the listing; empty when
    /// the listing did not say, or the change request was named
    /// directly. Empty never matches what the store holds.
    pub updated_at: String,
}

#[async_trait]
pub trait Forge: Sync {
    type Summary: Default + Serialize + Send;
    /// `PR` or `MR`, in logs.
    const ITEM: &'static str;
    /// What goes between container and number: `#` or `!`.
    const SIGIL: char;
    /// The table a change request's own record lands in. A fetch that
    /// could not get one whole is a failed attempt on its row there.
    const ITEM_TABLE: &'static str;
    /// This provider's `scope_config` record. Its discovery scopes share
    /// one, because `refresh_window_days` is one knob for all of them;
    /// the per-scope cursors stay in `sync_scope_state`.
    const SCOPE_CONFIG_KEY: &'static str;

    fn pool(&self) -> &SqlitePool;

    /// Where the account the run authenticates as is read.
    fn self_url(&self) -> String;

    async fn store_self(&self, me: &Value) -> Result<()>;

    async fn search(
        &self,
        client: &ForgeClient,
        scope: &str,
        me: &Value,
        since: Option<&str>,
    ) -> Result<Vec<Value>>;

    /// `since` as a search takes it, from the shared policy's RFC 3339
    /// stamp.
    fn since_param(&self, stamp: String) -> String {
        stamp
    }

    /// A search result as a change request; `None` for one that names
    /// none.
    fn listed(&self, item: &Value) -> Option<Listed>;

    /// A change request's id in [`Self::ITEM_TABLE`]: container, sigil,
    /// number, which [`split_item_key`] reads back.
    fn item_key(&self, container: &str, number: u32) -> String;

    /// Whether the store holds any change request yet. An empty one
    /// discovers everything, whatever the cursors say.
    async fn any_stored(&self) -> Result<bool>;

    /// The `updated_at` of every change request held, so one the listing
    /// shows unchanged is not fetched again. Empty for a forge whose
    /// listing is not trusted for that.
    async fn stored_updated_at(&self) -> Result<HashMap<(String, u32), String>> {
        Ok(HashMap::new())
    }

    /// Fetch one change request and everything under it. Returns what
    /// it could not do, one line each — empty when it got all of it.
    /// `Err` is for the store.
    async fn fetch_one(
        &self,
        client: &ForgeClient,
        cr: &Listed,
        summary: &mut Self::Summary,
    ) -> Result<Vec<String>>;

    fn record_skipped(&self, _summary: &mut Self::Summary) {}

    fn record_requests(&self, summary: &mut Self::Summary, requests: u64);
}

/// What one sync is asked to do.
pub struct SyncOptions<'a> {
    /// Discovery scopes, as the forge's search takes them.
    pub scopes: &'a [String],
    /// On a non-empty store, only look again at change requests updated
    /// in the last N days; 0 is unbounded.
    pub refresh_window_days: u32,
    /// Safety cap on how many are fetched (`None` = unbounded).
    pub max_items: Option<usize>,
    /// Change requests named directly. When any are, discovery is
    /// skipped and only these are fetched.
    pub targets: &'a [(String, u32)],
    /// Ignore the per-scope cursors, for a full backfill.
    pub full_sync: bool,
    /// The run's pinned clock. A scope's cursor is stamped with it and
    /// its first search's floor is measured from it, so both are
    /// request parameters a replayed run asks again.
    pub now: &'a IsoOffsetTimestamp,
    /// Raised when the step is asked to stop.
    pub stop: &'a StopFlag,
    pub sleep_between: Duration,
    pub progress: &'a Progress,
    /// The run's knobs, as `sync_runs` records them.
    pub run_config: Value,
}

pub async fn sync<F: Forge>(
    forge: &F,
    client: &ForgeClient,
    opts: SyncOptions<'_>,
) -> Result<F::Summary> {
    let _ = datalib_etl::latchkey::ensure_curl_router();
    let pool = forge.pool();
    let run = DownloadRun::start(pool, &opts.run_config).await?;

    // Diff the scope-affecting params against the ones that produced the
    // current cursors. `None` (fresh store, or one written before
    // `sync_scope_config` existed) means no adjustment — see the module
    // docs on `scope_config`.
    let scope_cfg = datalib_etl::scope_state::refresh_window_blob(opts.refresh_window_days);
    let prior_scope_cfg = datalib_etl::scope_config::load_or_none(pool, F::SCOPE_CONFIG_KEY).await;

    let mut summary = F::Summary::default();

    // `Ok(true)` when the run covered everything the config asks:
    // every scope searched and every change request it listed fetched.
    // Only then has it satisfied `refresh_window_days`; see
    // `scope_config`.
    let work = async {
        let (me, _) = client.get(&forge.self_url()).await?;
        if !me.is_object() {
            anyhow::bail!("{} returned non-object", forge.self_url());
        }
        forge.store_self(&me).await?;

        let discovery = if opts.targets.is_empty() {
            let full = opts.full_sync || !forge.any_stored().await?;
            let state = datalib_etl::doltlite_raw::load_scope_state(pool).await?;
            Some(
                discover(
                    forge,
                    client,
                    &me,
                    &opts,
                    &state,
                    full,
                    prior_scope_cfg.as_ref(),
                )
                .await,
            )
        } else {
            None
        };
        let keys: Vec<Listed> = match &discovery {
            Some(found) => with_retries(found.keys.clone(), failed_items(forge).await?),
            // Named directly: no listing, so nothing to compare against —
            // always fetched, and nothing else is.
            None => opts
                .targets
                .iter()
                .map(|(container, number)| Listed {
                    container: container.clone(),
                    number: *number,
                    updated_at: String::new(),
                })
                .collect(),
        };
        let cut = opts.max_items.is_some_and(|cap| keys.len() > cap);
        let keys: Vec<Listed> = keys
            .into_iter()
            .take(opts.max_items.unwrap_or(usize::MAX))
            .collect();
        tracing::info!(count = keys.len(), "{}s to fetch", F::ITEM);

        // One scan of what is held, so the per-item comparison is O(1).
        // This is what lets an interrupted run resume cheaply: the
        // listing still names everything, and the ones already fetched
        // are skipped.
        let stored = if opts.full_sync {
            HashMap::new()
        } else {
            forge.stored_updated_at().await?
        };

        opts.progress.set_length(Some(keys.len() as u64));
        for cr in &keys {
            if opts.stop.requested() {
                break;
            }
            opts.progress.inc(1);
            opts.progress
                .set_message(&format!("{}{}{}", cr.container, F::SIGIL, cr.number));
            let unchanged = !cr.updated_at.is_empty()
                && stored.get(&(cr.container.clone(), cr.number)) == Some(&cr.updated_at);
            if unchanged {
                forge.record_skipped(&mut summary);
            } else {
                let shortfalls = forge.fetch_one(client, cr, &mut summary).await?;
                // After a stop every request fails at once; that is not
                // something the change request did.
                if !shortfalls.is_empty() && !opts.stop.requested() {
                    let detail = shortfalls.join("; ");
                    tracing::warn!(
                        container = %cr.container, number = cr.number, detail,
                        "{} not fetched whole", F::ITEM,
                    );
                    record_failure(
                        pool,
                        F::ITEM_TABLE,
                        &forge.item_key(&cr.container, cr.number),
                        &detail,
                    )
                    .await?;
                }
            }
            if opts.sleep_between > Duration::ZERO {
                tokio::time::sleep(opts.sleep_between).await;
            }
        }

        // A stopped run did not get through what it listed: its cursors
        // and its listing problems stay as the last finished run left
        // them.
        if opts.stop.requested() {
            return Ok(false);
        }
        let Some(found) = discovery else {
            return Ok(false);
        };
        // A capped run fetched only part of what it listed; moving its
        // cursors would skip the rest for good.
        if !cut {
            for (scope, at) in &found.new_state {
                datalib_etl::doltlite_raw::upsert_scope_state(pool, scope, at).await?;
            }
        }
        datalib_etl::download_problems::report_run(pool, &found.problems).await;
        Ok::<bool, anyhow::Error>(found.problems.is_empty() && !cut)
    };

    let result = work.await;
    forge.record_requests(&mut summary, client.request_count());
    // Record the config only once this run has actually satisfied it. A
    // skipped scope, a cut, a stop or a targets-only run leaves the
    // prior blob in place so the next run re-plans the widening.
    datalib_etl::scope_config::store_if_satisfied(
        pool,
        F::SCOPE_CONFIG_KEY,
        &scope_cfg,
        matches!(result, Ok(true)),
    )
    .await;
    run.finish(&result, &summary).await;
    result?;
    Ok(summary)
}

/// The listing plus every change request an earlier run could not
/// fetch whole, which no listing may name again once the cursor has
/// moved past it. A retried one is fetched whatever its listed
/// `updated_at` says: the stored copy is the incomplete one.
fn with_retries(mut keys: Vec<Listed>, failed: Vec<(String, u32)>) -> Vec<Listed> {
    let failed: HashSet<(String, u32)> = failed.into_iter().collect();
    for cr in &mut keys {
        if failed.contains(&(cr.container.clone(), cr.number)) {
            cr.updated_at.clear();
        }
    }
    let listed: HashSet<(String, u32)> = keys
        .iter()
        .map(|cr| (cr.container.clone(), cr.number))
        .collect();
    keys.extend(failed.into_iter().filter(|key| !listed.contains(key)).map(
        |(container, number)| Listed {
            container,
            number,
            updated_at: String::new(),
        },
    ));
    keys.sort_by(|a, b| (&a.container, a.number).cmp(&(&b.container, b.number)));
    keys
}

/// The change requests whose last fetch failed. Read from the sidecar
/// alone: a change request that never fetched has no data row, because
/// its table's promoted columns cannot be null.
async fn failed_items<F: Forge>(forge: &F) -> Result<Vec<(String, u32)>> {
    use anyhow::Context as _;
    use sqlx::Row as _;
    let table = F::ITEM_TABLE;
    let sql =
        format!("SELECT id FROM {table}_bookkeeping WHERE last_error IS NOT NULL ORDER BY id");
    // Audited: `table` is a `&'static str` constant of the provider.
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .fetch_all(forge.pool())
        .await
        .with_context(|| format!("select the failed rows of {table}"))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id: String = row.try_get("id").context("failed row id")?;
        match split_item_key(&id, F::SIGIL) {
            Some(key) => out.push(key),
            None => tracing::warn!(table, id, "a failed row whose id names no change request"),
        }
    }
    Ok(out)
}

/// `(container, number)` back out of an [`Forge::item_key`].
pub fn split_item_key(id: &str, sigil: char) -> Option<(String, u32)> {
    let (container, number) = id.rsplit_once(sigil)?;
    Some((container.to_string(), number.parse().ok()?))
}

async fn record_failure(pool: &SqlitePool, table: &str, id: &str, detail: &str) -> Result<()> {
    use anyhow::Context as _;
    let mut tx = pool.begin().await.context("begin the failure record")?;
    datalib_etl::doltlite_raw::record_object_error(&mut tx, table, id, detail).await?;
    tx.commit().await.context("commit the failure record")
}

/// What one discovery pass found.
struct Discovery {
    /// Unique by `(container, number)`, sorted by it, each with the
    /// newest `updated_at` any scope listed.
    keys: Vec<Listed>,
    /// Next-run cursor per scope. Only scopes that actually searched
    /// appear, so a failed scope keeps its old cursor and retries.
    new_state: HashMap<String, String>,
    /// One `listing:search <scope>` per scope whose search failed and
    /// was stepped over.
    problems: Vec<RunProblem>,
}

async fn discover<F: Forge>(
    forge: &F,
    client: &ForgeClient,
    me: &Value,
    opts: &SyncOptions<'_>,
    state: &HashMap<String, String>,
    full: bool,
    prior: Option<&Value>,
) -> Discovery {
    let mut newest: HashMap<(String, u32), String> = HashMap::new();
    let mut new_state: HashMap<String, String> = HashMap::new();
    let mut problems: Vec<RunProblem> = Vec::new();
    for scope in opts.scopes {
        if opts.stop.requested() {
            break;
        }
        let since = datalib_etl::scope_state::since_for_scope(
            opts.now,
            state,
            scope,
            opts.refresh_window_days,
            full,
            prior,
        )
        .map(|stamp| forge.since_param(stamp));
        tracing::info!(scope, since, "searching {}s", F::ITEM);
        let results = match forge.search(client, scope, me, since.as_deref()).await {
            Ok(v) => v,
            Err(e) => {
                let name = format!("search {scope}");
                let refused = e
                    .downcast_ref::<ForgeError>()
                    .is_some_and(ForgeError::refused);
                problems.push(if refused {
                    RunProblem::forbidden(&name, format!("{e:#}"))
                } else {
                    RunProblem::listing(&name, format!("{e:#}"))
                });
                continue;
            }
        };
        for listed in results.iter().filter_map(|item| forge.listed(item)) {
            let key = (listed.container, listed.number);
            match newest.get(&key) {
                Some(held) if *held >= listed.updated_at => {}
                _ => {
                    newest.insert(key, listed.updated_at);
                }
            }
        }
        new_state.insert(scope.clone(), opts.now.to_rfc3339_secs());
        tracing::info!(scope, count = results.len(), "scope done");
    }
    let mut keys: Vec<Listed> = newest
        .into_iter()
        .map(|((container, number), updated_at)| Listed {
            container,
            number,
            updated_at,
        })
        .collect();
    keys.sort_by(|a, b| (&a.container, a.number).cmp(&(&b.container, b.number)));
    Discovery {
        keys,
        new_state,
        problems,
    }
}

/// A change request's own record, or why there is none to store.
pub async fn get_change_request(client: &ForgeClient, url: &str) -> Result<Value, String> {
    match client.get(url).await {
        Ok((v, _)) if v.is_object() => Ok(v),
        Ok(_) => Err(format!("{url} returned something other than an object")),
        Err(e) => Err(e.to_string()),
    }
}

/// Walk a change request's whole list of one kind of child, or say why
/// it could not. An empty list from a failed request is
/// indistinguishable from "all deleted", and pruning on it would wipe
/// every comment the change request has: on `Err` the caller neither
/// stores nor prunes, and reports the line.
pub async fn walk_children(
    client: &ForgeClient,
    url: &str,
    what: &str,
) -> Result<Vec<Value>, String> {
    client
        .paginate(url)
        .await
        .map_err(|e| format!("could not list its {what}: {e}"))
}

/// Delete `table`'s rows under one change request that its fresh,
/// complete listing did not name. Scoped to that change request: the
/// endpoint enumerated its children and nothing else.
pub async fn prune_children(
    pool: &SqlitePool,
    table: &'static str,
    scope: &[(&str, &str)],
    keep: &std::collections::HashSet<String>,
) -> Result<usize> {
    let gone = datalib_etl::prune::prune_scope(pool, table, scope, keep).await?;
    if !gone.is_empty() {
        tracing::info!(
            event = "forge_children_pruned",
            table,
            scope = ?scope,
            removed = gone.len(),
            "the forge no longer lists these; deleting our copies",
        );
    }
    Ok(gone.len())
}

/// The account the store was synced as: its one `self_identity` row.
pub async fn load_self_identity(pool: &SqlitePool) -> Result<Option<Value>> {
    use anyhow::Context as _;
    use sqlx::Row as _;
    let row = sqlx::query(
        "SELECT json(payload) AS payload FROM self_identity \
         WHERE payload IS NOT NULL ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .context("select self_identity")?;
    let Some(row) = row else { return Ok(None) };
    let payload: Option<String> = row.try_get("payload").ok();
    Ok(payload.and_then(|s| serde_json::from_str(&s).ok()))
}

/// A loaded row's `payload` column, parsed; `None` for a row the load
/// steps over.
pub fn row_payload(row: &sqlx::sqlite::SqliteRow) -> Option<Value> {
    use sqlx::Row as _;
    let payload: String = row.try_get("payload").ok()?;
    serde_json::from_str(&payload).ok()
}

/// A payload's string field, owned. For the promoted columns of a raw row.
pub fn opt_str(payload: &Value, key: &str) -> Option<String> {
    payload.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// A payload's numeric `id`, as the text a raw row keys on. `what` names
/// the payload in the error.
pub fn numeric_id(payload: &Value, what: &str) -> Result<String> {
    payload
        .get("id")
        .and_then(|v| v.as_i64())
        .map(|n| n.to_string())
        .ok_or_else(|| anyhow::anyhow!("{what} missing id"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(container: &str, number: u32, updated_at: &str) -> Listed {
        Listed {
            container: container.to_string(),
            number,
            updated_at: updated_at.to_string(),
        }
    }

    /// A change request whose discussions would not list was stored
    /// whole otherwise, so its listed `updated_at` matches the store and
    /// the skip would step over it forever: the retry must clear it.
    #[test]
    fn a_failed_one_is_fetched_whether_or_not_the_listing_names_it() {
        let keys = vec![
            listed("starfleet/enterprise", 2, "2369-04-14T00:00:00Z"),
            listed("starfleet/enterprise", 1, "2369-04-14T00:00:00Z"),
        ];
        let failed = vec![
            ("starfleet/enterprise".to_string(), 2),
            ("starfleet/defiant".to_string(), 74205),
        ];
        assert_eq!(
            with_retries(keys, failed),
            vec![
                listed("starfleet/defiant", 74205, ""),
                listed("starfleet/enterprise", 1, "2369-04-14T00:00:00Z"),
                listed("starfleet/enterprise", 2, ""),
            ]
        );
    }

    #[test]
    fn an_item_key_splits_back_at_its_last_sigil() {
        assert_eq!(
            split_item_key("starfleet/enterprise#1701", '#'),
            Some(("starfleet/enterprise".to_string(), 1701))
        );
        assert_eq!(
            split_item_key("starfleet/enterprise!1701", '!'),
            Some(("starfleet/enterprise".to_string(), 1701))
        );
        assert_eq!(split_item_key("starfleet/enterprise", '#'), None);
        assert_eq!(split_item_key("starfleet/enterprise#NCC", '#'), None);
    }
}
