//! The contacts app's store: contacts a person made, and the handles they
//! linked to them. The one store under a data root that cannot be rebuilt
//! from anything, so it refuses a schema it cannot reach rather than
//! rebuilding, and every write is a commit a person can undo.
//!
//! Its one writer is the `datalib_contacts` applet. Nothing in the core
//! opens it; the core knows handles, never contacts.
//! `docs/dev/plans/contacts.md` has the design.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
pub use datalib_contact_schema::ContactKind;
use datalib_contact_schema::{ContactHandle, DatalibContact};
use datalib_etl::doltlite_raw;
use datalib_handle::Handle;
use datalib_store_meta::StoreKind;
use datalib_time::IsoOffsetTimestamp;
use serde::Serialize;
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use strum::{EnumString, IntoStaticStr, VariantArray};

/// Under the data root: one directory per app whose state a person
/// curates, so each can be managed or deleted on its own.
pub const CURATED_DIR: &str = "datalib_curated";
pub const APP_DIR: &str = "datalib_contacts";
pub const STORE_FILE: &str = "contacts.doltlite_db";

/// The `source_id` of every contact this app answers with: a contact is
/// one more account of a person, ranked above every source's.
pub const SOURCE_ID: &str = "datalib_contacts";

pub fn store_path(data_root: &Path) -> PathBuf {
    data_root.join(CURATED_DIR).join(APP_DIR).join(STORE_FILE)
}

const DDL: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS contacts (
        contact_id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        note TEXT,
        merged_into TEXT,
        created_at_utc TEXT NOT NULL,
        updated_at_utc TEXT NOT NULL,
        tz_offset TEXT NOT NULL
    )",
    // A handle belongs to one contact for all time; an address two people
    // share belongs to a group contact.
    "CREATE TABLE IF NOT EXISTS handles (
        handle TEXT PRIMARY KEY,
        contact_id TEXT NOT NULL,
        linked_how TEXT NOT NULL,
        linked_at_utc TEXT NOT NULL,
        tz_offset TEXT NOT NULL,
        stopped_working_by TEXT
    )",
    "CREATE INDEX IF NOT EXISTS handles_by_contact ON handles (contact_id)",
    "CREATE TABLE IF NOT EXISTS members (
        group_id TEXT NOT NULL,
        member_id TEXT NOT NULL,
        added_at_utc TEXT NOT NULL,
        tz_offset TEXT NOT NULL,
        PRIMARY KEY (group_id, member_id)
    )",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, IntoStaticStr, VariantArray)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum LinkedHow {
    Manual,
}

impl LinkedHow {
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContactSummary {
    pub contact_id: String,
    pub name: String,
    pub kind: String,
}

/// What linking a handle to a contact comes to, given who holds it now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkPlan {
    Insert,
    AlreadyLinked,
    HeldBy(String),
}

fn plan_link(holder: Option<&str>, target: &str) -> LinkPlan {
    match holder {
        None => LinkPlan::Insert,
        Some(h) if h == target => LinkPlan::AlreadyLinked,
        Some(h) => LinkPlan::HeldBy(h.to_string()),
    }
}

/// `2019`, `2019-06` or `2019-06-14`: as precise as the person knows.
fn is_partial_date(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    let digits = |p: &str, n: usize| p.len() == n && p.chars().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        [y] => digits(y, 4),
        [y, m] => digits(y, 4) && digits(m, 2) && ("01"..="12").contains(m),
        [y, m, d] => {
            digits(y, 4)
                && digits(m, 2)
                && digits(d, 2)
                && ("01"..="12").contains(m)
                && ("01"..="31").contains(d)
        }
        _ => false,
    }
}

/// The open store. Holds the file's writer lock for as long as it lives;
/// [`Store::close`] before dropping it.
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let pool = doltlite_raw::open_curated(path, DDL, StoreKind::Contacts).await?;
        Ok(Self { pool })
    }

    pub async fn close(self) {
        self.pool.close().await;
    }

    /// The contact holding each of `handles`, by handle; a handle no
    /// contact holds is absent.
    pub async fn resolve(&self, handles: &[Handle]) -> Result<HashMap<String, DatalibContact>> {
        let mut out = HashMap::new();
        for h in handles {
            let holder: Option<String> =
                sqlx::query_scalar("SELECT contact_id FROM handles WHERE handle = ?")
                    .bind(h.as_str())
                    .fetch_optional(&self.pool)
                    .await
                    .context("resolve a handle")?;
            if let Some(contact) = match holder {
                Some(id) => self.contact(&id).await?,
                None => None,
            } {
                out.insert(h.as_str().to_string(), contact);
            }
        }
        Ok(out)
    }

    /// Contacts whose name contains `q`, ignoring case; every contact
    /// for an empty `q`. Merged-away contacts are left out.
    pub async fn search(&self, q: &str, limit: u32) -> Result<Vec<ContactSummary>> {
        let pattern = format!(
            "%{}%",
            q.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let rows = sqlx::query(
            "SELECT contact_id, name, kind FROM contacts \
              WHERE merged_into IS NULL AND name LIKE ? ESCAPE '\\' \
              ORDER BY name COLLATE NOCASE LIMIT ?",
        )
        .bind(pattern)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .context("search contacts")?;
        Ok(rows
            .iter()
            .map(|r| ContactSummary {
                contact_id: r.get("contact_id"),
                name: r.get("name"),
                kind: r.get("kind"),
            })
            .collect())
    }

    pub async fn contact(&self, contact_id: &str) -> Result<Option<DatalibContact>> {
        let Some(r) = sqlx::query(
            "SELECT contact_id, name, kind, note, created_at_utc, updated_at_utc \
               FROM contacts WHERE contact_id = ?",
        )
        .bind(contact_id)
        .fetch_optional(&self.pool)
        .await
        .context("read a contact")?
        else {
            return Ok(None);
        };
        let kind: String = r.get("kind");
        let mut contact = DatalibContact::new(
            SOURCE_ID,
            contact_id,
            ContactKind::parse(&kind).unwrap_or(ContactKind::Person),
        );
        contact.names = vec![r.get("name")];
        contact.note = r.get("note");
        contact.created_at = r.get("created_at_utc");
        contact.modified_at = r.get("updated_at_utc");
        contact.handles = sqlx::query(
            "SELECT handle, stopped_working_by FROM handles WHERE contact_id = ? \
              ORDER BY stopped_working_by IS NOT NULL, handle",
        )
        .bind(contact_id)
        .fetch_all(&self.pool)
        .await
        .context("read a contact's handles")?
        .iter()
        .filter_map(|h| {
            let mut linked = ContactHandle::of(Handle::parse(h.get("handle"))?);
            linked.stopped_working_by = h.get("stopped_working_by");
            Some(linked)
        })
        .collect();
        Ok(Some(contact))
    }

    /// A new contact holding `handles`. Refused, with nothing written, if
    /// any of them already belongs to someone.
    pub async fn create(
        &self,
        name: &str,
        kind: ContactKind,
        handles: &[Handle],
    ) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a contact needs a name");
        }
        let contact_id = uuid::Uuid::new_v4().to_string();
        let (now, tz) = IsoOffsetTimestamp::now_local().to_utc_and_offset();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO contacts (contact_id, kind, name, created_at_utc, updated_at_utc, tz_offset) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&contact_id)
        .bind(kind.as_str())
        .bind(name)
        .bind(&now)
        .bind(&now)
        .bind(&tz)
        .execute(&mut *tx)
        .await
        .context("insert a contact")?;
        for h in handles {
            match plan_link(holder(&mut tx, h).await?.as_deref(), &contact_id) {
                LinkPlan::Insert => insert_handle(&mut tx, h, &contact_id, &now, &tz).await?,
                LinkPlan::AlreadyLinked => {}
                LinkPlan::HeldBy(other) => {
                    bail!("{h} already belongs to {}", name_of(&mut tx, &other).await?)
                }
            }
        }
        tx.commit().await?;
        self.seal(&format!("contacts: new {} {name:?}", kind.as_str()))
            .await?;
        Ok(contact_id)
    }

    /// Link `handle` to `contact_id`. Linking it where it already is
    /// does nothing; linking a handle someone else holds is refused —
    /// unlink it there first.
    pub async fn link(&self, handle: &Handle, contact_id: &str) -> Result<()> {
        let (now, tz) = IsoOffsetTimestamp::now_local().to_utc_and_offset();
        let mut tx = self.pool.begin().await?;
        let name: Option<String> =
            sqlx::query_scalar("SELECT name FROM contacts WHERE contact_id = ?")
                .bind(contact_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(name) = name else {
            bail!("no contact {contact_id}");
        };
        match plan_link(holder(&mut tx, handle).await?.as_deref(), contact_id) {
            LinkPlan::AlreadyLinked => return Ok(()),
            LinkPlan::HeldBy(other) => {
                bail!(
                    "{handle} already belongs to {}",
                    name_of(&mut tx, &other).await?
                )
            }
            LinkPlan::Insert => insert_handle(&mut tx, handle, contact_id, &now, &tz).await?,
        }
        tx.commit().await?;
        self.seal(&format!("contacts: link {handle} to {name:?}"))
            .await
    }

    /// Returns whether the handle was linked to anyone.
    pub async fn unlink(&self, handle: &Handle) -> Result<bool> {
        let done = sqlx::query("DELETE FROM handles WHERE handle = ?")
            .bind(handle.as_str())
            .execute(&self.pool)
            .await
            .context("unlink a handle")?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        self.seal(&format!("contacts: unlink {handle}")).await?;
        Ok(true)
    }

    /// Mark a linked handle as no longer working by `by` (a partial
    /// date), or as working again with `None`.
    pub async fn set_stopped_working(&self, handle: &Handle, by: Option<&str>) -> Result<bool> {
        if let Some(by) = by {
            if !is_partial_date(by) {
                bail!("{by:?} is not a date: write 2019, 2019-06 or 2019-06-14");
            }
        }
        let done = sqlx::query("UPDATE handles SET stopped_working_by = ? WHERE handle = ?")
            .bind(by)
            .bind(handle.as_str())
            .execute(&self.pool)
            .await
            .context("mark a handle")?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        let what = by.map_or("works again".to_string(), |d| {
            format!("stopped working by {d}")
        });
        self.seal(&format!("contacts: {handle} {what}")).await?;
        Ok(true)
    }

    pub async fn rename(&self, contact_id: &str, name: &str) -> Result<bool> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a contact needs a name");
        }
        let (now, tz) = IsoOffsetTimestamp::now_local().to_utc_and_offset();
        let done = sqlx::query(
            "UPDATE contacts SET name = ?, updated_at_utc = ?, tz_offset = ? WHERE contact_id = ?",
        )
        .bind(name)
        .bind(&now)
        .bind(&tz)
        .bind(contact_id)
        .execute(&self.pool)
        .await
        .context("rename a contact")?;
        if done.rows_affected() == 0 {
            return Ok(false);
        }
        self.seal(&format!("contacts: rename {contact_id} to {name:?}"))
            .await?;
        Ok(true)
    }

    async fn seal(&self, msg: &str) -> Result<()> {
        doltlite_raw::commit_run(&self.pool, msg).await?;
        Ok(())
    }
}

async fn holder(tx: &mut sqlx::SqliteConnection, h: &Handle) -> Result<Option<String>> {
    sqlx::query_scalar("SELECT contact_id FROM handles WHERE handle = ?")
        .bind(h.as_str())
        .fetch_optional(&mut *tx)
        .await
        .context("read who holds a handle")
}

async fn name_of(tx: &mut sqlx::SqliteConnection, contact_id: &str) -> Result<String> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM contacts WHERE contact_id = ?")
        .bind(contact_id)
        .fetch_optional(&mut *tx)
        .await
        .context("read a contact's name")?;
    Ok(name.unwrap_or_else(|| format!("contact {contact_id}")))
}

async fn insert_handle(
    tx: &mut sqlx::SqliteConnection,
    h: &Handle,
    contact_id: &str,
    now: &str,
    tz: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO handles (handle, contact_id, linked_how, linked_at_utc, tz_offset) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(h.as_str())
    .bind(contact_id)
    .bind(LinkedHow::Manual.as_str())
    .bind(now)
    .bind(tz)
    .execute(&mut *tx)
    .await
    .context("link a handle")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn email(s: &str) -> Handle {
        Handle::email(s).unwrap()
    }

    #[test]
    fn a_handle_is_linked_once_and_never_taken_silently() {
        assert_eq!(plan_link(None, "a"), LinkPlan::Insert);
        assert_eq!(plan_link(Some("a"), "a"), LinkPlan::AlreadyLinked);
        assert_eq!(plan_link(Some("b"), "a"), LinkPlan::HeldBy("b".into()));
    }

    #[test]
    fn stopped_working_takes_only_as_much_date_as_a_person_knows() {
        for ok in ["2019", "2019-06", "2019-06-14"] {
            assert!(is_partial_date(ok), "{ok}");
        }
        for bad in [
            "",
            "19",
            "2019-6",
            "2019-13",
            "2019-06-32",
            "2019-06-14T00:00",
            "June 2019",
        ] {
            assert!(!is_partial_date(bad), "{bad}");
        }
    }

    #[test]
    fn strum_spellings_round_trip() {
        for k in LinkedHow::VARIANTS {
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
    }

    /// The whole loop a chip drives: unresolved, created, resolved, and
    /// back to unresolved — each step a commit, on a real doltlite file.
    #[tokio::test]
    async fn create_link_resolve_unlink_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&store_path(dir.path())).await.unwrap();
        let riker = email("riker@enterprise.org");
        let tel = Handle::tel("+15550123456").unwrap();

        assert!(store
            .resolve(std::slice::from_ref(&riker))
            .await
            .unwrap()
            .is_empty());
        let id = store
            .create(
                "Will Riker",
                ContactKind::Person,
                std::slice::from_ref(&riker),
            )
            .await
            .unwrap();
        store.link(&tel, &id).await.unwrap();
        store.link(&tel, &id).await.unwrap();

        let got = store.resolve(&[riker.clone(), tel.clone()]).await.unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[riker.as_str()].name(), Some("Will Riker"));
        assert_eq!(got[tel.as_str()].key, id);
        assert_eq!(got[tel.as_str()].source_id, SOURCE_ID);

        let other = store
            .create("Thomas Riker", ContactKind::Person, &[])
            .await
            .unwrap();
        let err = store.link(&tel, &other).await.unwrap_err();
        assert!(
            err.to_string().ends_with("already belongs to Will Riker"),
            "{err}"
        );
        let err = store
            .create("Twin", ContactKind::Person, std::slice::from_ref(&riker))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already belongs"), "{err}");
        let names: Vec<String> = store
            .search("riker", 10)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            names,
            ["Thomas Riker", "Will Riker"],
            "the refused create left nothing behind"
        );

        assert!(store
            .set_stopped_working(&tel, Some("2019-06"))
            .await
            .unwrap());
        let c = store.contact(&id).await.unwrap().unwrap();
        assert_eq!(
            c.handles[0].handle.as_ref(),
            Some(&riker),
            "working handles first"
        );
        assert_eq!(c.handles[1].stopped_working_by.as_deref(), Some("2019-06"));

        assert!(store.unlink(&tel).await.unwrap());
        assert!(!store.unlink(&tel).await.unwrap());
        assert!(store.resolve(&[tel]).await.unwrap().is_empty());
        store.close().await;
    }

    /// A reader sees every edit as soon as it returns: each write is
    /// sealed onto `main`, not left on the writer's branch.
    #[tokio::test]
    async fn every_edit_reaches_main() {
        let dir = tempfile::tempdir().unwrap();
        let path = store_path(dir.path());
        let store = Store::open(&path).await.unwrap();
        store
            .create(
                "Deanna Troi",
                ContactKind::Person,
                &[email("troi@enterprise.org")],
            )
            .await
            .unwrap();
        let reader = doltlite_raw::open_reader(&path, None)
            .await
            .unwrap()
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM handles")
            .fetch_one(reader.pool())
            .await
            .unwrap();
        assert_eq!(n, 1);
        reader.close().await;
        store.close().await;
    }
}
