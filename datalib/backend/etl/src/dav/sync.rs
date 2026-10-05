//! Keeping one DAV collection (a calendar, an address book) in step with
//! the server: RFC 6578 `sync-collection` page by page, a fresh listing
//! when the server refuses the stored token, a provider's own query when
//! the server has no `sync-collection`, and `multiget` for what a listing
//! names without its data. Its one rule beyond the RFCs: only a whole
//! listing that came back complete may delete what it did not name.

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use tracing::info;

use super::{escape_xml, report, DavError, DavProps, DavResponse, Multistatus};
use crate::http::{HttpService, LatchkeySettings};

/// How many truncated `sync-collection` replies one listing follows
/// before giving up on this run; the token taken so far is kept.
pub const MAX_SYNC_ROUNDS: usize = 50;

/// How many resources one `multiget` names.
const MULTIGET_BATCH: usize = 100;

/// What differs between CalDAV and CardDAV on the wire.
#[derive(Debug, Clone, Copy)]
pub struct CollectionKind {
    pub service: HttpService,
    /// Declares the prefix `data_prop` and `multiget` use.
    pub ns_decl: &'static str,
    /// The property holding an object: `C:calendar-data`.
    pub data_prop: &'static str,
    /// The `multiget` REPORT's root element: `C:calendar-multiget`.
    pub multiget: &'static str,
    /// The Depth every REPORT on the collection is sent with.
    pub report_depth: &'static str,
}

impl CollectionKind {
    pub fn body_sync_collection(&self, prev_token: &str) -> String {
        super::body_sync_collection(prev_token, self.ns_decl, self.data_prop)
    }

    pub fn body_multiget(&self, hrefs: &[String]) -> String {
        let hrefs: String = hrefs
            .iter()
            .map(|h| format!("  <href>{}</href>\n", escape_xml(h)))
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<{root} xmlns="DAV:" {ns}>
  <prop>
    <getetag/>
    <{data}/>
  </prop>
{hrefs}</{root}>
"#,
            root = self.multiget,
            ns = self.ns_decl,
            data = self.data_prop,
        )
    }
}

/// The properties of one stored object: what `multiget` is for.
pub trait ObjectProps: DavProps {
    /// The object itself, `None` when the listing named it without.
    fn data(&self) -> Option<&str>;
}

/// One reply's worth of changes, ready to store.
#[derive(Debug)]
pub struct Page<P> {
    /// Objects with their data, fetched by `multiget` where the listing
    /// named them without it.
    pub changed: Vec<DavResponse<P>>,
    /// hrefs the server reports gone (404 or 410).
    pub deleted: Vec<String>,
    /// hrefs listed whose data `multiget` did not return either.
    pub unfetched: Vec<String>,
    /// The token to store once the page is applied; `None` stores none,
    /// as a query listing leaves nothing to resume from.
    pub token: Option<String>,
}

/// How a collection's listing ended, once [`CollectionSync::next_page`]
/// has returned `None`.
#[derive(Debug)]
pub struct Outcome {
    /// The listing named everything the collection holds, not only what
    /// changed since a token.
    whole: bool,
    listed: HashSet<String>,
    cut_short: Option<String>,
}

impl Outcome {
    /// Why the listing did not reach the end, if it did not.
    pub fn cut_short(&self) -> Option<&str> {
        self.cut_short.as_deref()
    }

    /// The stored hrefs to delete: those a whole, complete listing did
    /// not name. Absence from anything less says nothing.
    pub fn unlisted<'s>(&self, stored: impl IntoIterator<Item = &'s String>) -> Vec<&'s String> {
        if !self.whole || self.cut_short.is_some() {
            return Vec::new();
        }
        stored
            .into_iter()
            .filter(|href| !self.listed.contains(*href))
            .collect()
    }
}

enum Mode {
    Sync { token: String, rounds: usize },
    Query { body: String },
    Done,
}

/// Drives one collection's listing. Call [`Self::next_page`] until it
/// returns `None`, storing each page and its token as it comes, then
/// [`Self::finish`].
pub struct CollectionSync<'a> {
    kind: &'a CollectionKind,
    url: &'a str,
    latchkey: &'a LatchkeySettings,
    mode: Mode,
    /// The query that lists everything, for a server without
    /// `sync-collection`.
    fallback: Option<String>,
    whole: bool,
    listed: HashSet<String>,
    cut_short: Option<String>,
    /// The server refused a token this run, so the listing restarted
    /// from nothing; a second refusal fails rather than loop.
    token_refused: bool,
    requests: usize,
}

impl<'a> CollectionSync<'a> {
    /// `sync-collection` from `token` (`None` or empty: list whole),
    /// falling back to `fallback` where the server cannot.
    pub fn new(
        kind: &'a CollectionKind,
        url: &'a str,
        token: Option<String>,
        fallback: Option<String>,
        latchkey: &'a LatchkeySettings,
    ) -> Self {
        let token = token.unwrap_or_default();
        Self {
            kind,
            url,
            latchkey,
            whole: token.is_empty(),
            mode: Mode::Sync { token, rounds: 0 },
            fallback,
            listed: HashSet::new(),
            cut_short: None,
            token_refused: false,
            requests: 0,
        }
    }

    /// One query naming everything in scope, with no token kept.
    pub fn query(
        kind: &'a CollectionKind,
        url: &'a str,
        body: String,
        latchkey: &'a LatchkeySettings,
    ) -> Self {
        Self {
            kind,
            url,
            latchkey,
            whole: true,
            mode: Mode::Query { body },
            fallback: None,
            listed: HashSet::new(),
            cut_short: None,
            token_refused: false,
            requests: 0,
        }
    }

    pub fn requests(&self) -> usize {
        self.requests
    }

    pub fn finish(self) -> Outcome {
        Outcome {
            whole: self.whole,
            listed: self.listed,
            cut_short: self.cut_short,
        }
    }

    pub async fn next_page<P: ObjectProps>(&mut self) -> Result<Option<Page<P>>> {
        loop {
            match std::mem::replace(&mut self.mode, Mode::Done) {
                Mode::Done => return Ok(None),
                Mode::Query { body } => {
                    let reply = self.report::<P>(&body).await.context("query REPORT")?;
                    let reply = Reply::of(reply, self.url);
                    if reply.truncated {
                        self.cut_short = Some(CUT_SHORT.to_string());
                    }
                    return self.page(reply, None).await.map(Some);
                }
                Mode::Sync { token, rounds } => {
                    if rounds == MAX_SYNC_ROUNDS {
                        self.cut_short = Some(format!(
                            "the listing was still unfinished after {MAX_SYNC_ROUNDS} pages"
                        ));
                        return Ok(None);
                    }
                    let body = self.kind.body_sync_collection(&token);
                    let refused = match self.report::<P>(&body).await {
                        Ok(ms) => {
                            let reply = Reply::of(ms, self.url);
                            let Some(next) = reply.token.clone() else {
                                bail!("sync-collection reply carried no sync-token");
                            };
                            match after_sync_page(&token, &next, reply.truncated) {
                                AfterPage::Done => {}
                                AfterPage::NextPage => {
                                    self.mode = Mode::Sync {
                                        token: next.clone(),
                                        rounds: rounds + 1,
                                    }
                                }
                                AfterPage::CutShort => {
                                    self.cut_short = Some(format!(
                                        "{CUT_SHORT}, and would not page past where it stopped"
                                    ))
                                }
                            }
                            return self.page(reply, Some(next)).await.map(Some);
                        }
                        Err(DavError::Http { status, .. }) => status,
                        Err(e) => return Err(e).context("sync-collection REPORT"),
                    };
                    match on_refusal(refused, &token, self.fallback.is_some(), self.token_refused) {
                        Refusal::ListWhole => {
                            self.token_refused = true;
                            info!(
                                event = "dav_sync_token_refused",
                                url = %self.url,
                                status = refused,
                                "the server refused the stored sync token; listing the collection whole"
                            );
                            self.whole = true;
                            self.listed.clear();
                            self.mode = Mode::Sync {
                                token: String::new(),
                                rounds: 0,
                            };
                        }
                        Refusal::Fallback => {
                            info!(
                                event = "dav_sync_collection_unsupported",
                                url = %self.url,
                                status = refused,
                                "the server does not support sync-collection; listing the collection with a query"
                            );
                            self.whole = true;
                            self.mode = Mode::Query {
                                body: self.fallback.take().unwrap_or_default(),
                            };
                        }
                        Refusal::Fail => bail!(
                            "sync-collection REPORT: http {refused}{}",
                            if token.is_empty() && matches!(refused, 403 | 405 | 501) {
                                " (the server does not support sync-collection)"
                            } else if self.token_refused {
                                " (the server refused a token again after listing from nothing)"
                            } else {
                                ""
                            }
                        ),
                    }
                }
            }
        }
    }

    async fn report<P: DavProps>(&mut self, body: &str) -> Result<Multistatus<P>, DavError> {
        self.requests += 1;
        report(
            self.kind.service,
            self.url,
            self.kind.report_depth,
            body,
            self.latchkey,
        )
        .await
    }

    async fn page<P: ObjectProps>(
        &mut self,
        reply: Reply<P>,
        token: Option<String>,
    ) -> Result<Page<P>> {
        self.listed
            .extend(reply.present.iter().map(|r| r.href.clone()));
        let (mut changed, without): (Vec<_>, Vec<_>) = reply
            .present
            .into_iter()
            .partition(|r| r.props.data().is_some());
        let without: Vec<String> = without.into_iter().map(|r| r.href).collect();
        for chunk in without.chunks(MULTIGET_BATCH) {
            let ms = self
                .report::<P>(&self.kind.body_multiget(chunk))
                .await
                .context("multiget REPORT")?;
            changed.extend(
                ms.responses
                    .into_iter()
                    .filter(|r| r.status.is_none() && r.props.data().is_some()),
            );
        }
        let fetched: HashSet<&str> = changed.iter().map(|r| r.href.as_str()).collect();
        let unfetched = without
            .iter()
            .filter(|h| !fetched.contains(h.as_str()))
            .cloned()
            .collect();
        Ok(Page {
            changed,
            deleted: reply.deleted,
            unfetched,
            token,
        })
    }
}

const CUT_SHORT: &str = "the server stopped the listing short (507)";

/// One listing reply, sorted.
struct Reply<P> {
    /// Objects the reply names as present, the collection itself left out.
    present: Vec<DavResponse<P>>,
    deleted: Vec<String>,
    /// A 507 anywhere: RFC 6578 §3.6 puts it on the collection when the
    /// server stops a listing short, and no 507 means a whole reply.
    truncated: bool,
    token: Option<String>,
}

impl<P> Reply<P> {
    fn of(ms: Multistatus<P>, collection_url: &str) -> Self {
        let own = collection_url.trim_end_matches('/');
        let is_collection = |href: &str| {
            super::absolutize(collection_url, href).is_some_and(|u| u.trim_end_matches('/') == own)
        };
        let mut out = Reply {
            present: Vec::new(),
            deleted: Vec::new(),
            truncated: false,
            token: ms.sync_token,
        };
        for r in ms.responses {
            match r.status {
                Some(507) => out.truncated = true,
                Some(404 | 410) => out.deleted.push(r.href),
                _ if is_collection(&r.href) => {}
                _ => out.present.push(r),
            }
        }
        out
    }
}

#[derive(Debug, PartialEq, Eq)]
enum AfterPage {
    Done,
    NextPage,
    CutShort,
}

fn after_sync_page(prev_token: &str, next_token: &str, truncated: bool) -> AfterPage {
    if !truncated {
        AfterPage::Done
    } else if next_token == prev_token {
        AfterPage::CutShort
    } else {
        AfterPage::NextPage
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// Drop the token and list from nothing.
    ListWhole,
    /// The server has no `sync-collection`: list with the query.
    Fallback,
    Fail,
}

/// What an HTTP error on `sync-collection` means. With a token, RFC 6578
/// answers one it no longer honours with 403 `valid-sync-token`; some
/// servers send 409 or 410. Without one, a 403 (RFC 3253's
/// `supported-report`), 405 or 501 says the REPORT itself is unsupported.
/// A server that refuses even the token it just handed out is not
/// restarted for again.
fn on_refusal(status: u16, token: &str, has_fallback: bool, refused_before: bool) -> Refusal {
    match status {
        403 | 409 | 410 if !token.is_empty() && !refused_before => Refusal::ListWhole,
        403 | 405 | 501 if token.is_empty() && has_fallback => Refusal::Fallback,
        _ => Refusal::Fail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct Props {
        data: Option<String>,
    }

    impl DavProps for Props {
        fn leaf(&mut self, name: &str, _parent: &str, text: String) {
            if name == "calendar-data" {
                self.data = Some(text);
            }
        }
    }

    fn response(href: &str, status: Option<u16>) -> DavResponse<Props> {
        DavResponse {
            href: href.into(),
            status,
            props: Props::default(),
        }
    }

    #[test]
    fn a_reply_sorts_present_deleted_and_truncated_and_drops_the_collection() {
        let ms = Multistatus {
            responses: vec![
                response("/cal/", None),
                response("/cal/a.ics", None),
                response("/cal/b.ics", Some(404)),
                response("/cal/c.ics", Some(410)),
                response("/cal/", Some(507)),
            ],
            sync_token: Some("t2".into()),
        };
        let reply = Reply::of(ms, "https://dav.test/cal");
        let present: Vec<&str> = reply.present.iter().map(|r| r.href.as_str()).collect();
        assert_eq!(present, vec!["/cal/a.ics"]);
        assert_eq!(reply.deleted, vec!["/cal/b.ics", "/cal/c.ics"]);
        assert!(reply.truncated);
        assert_eq!(reply.token.as_deref(), Some("t2"));
    }

    #[test]
    fn a_truncated_page_pages_on_only_while_its_token_moves() {
        assert_eq!(after_sync_page("t1", "t1", false), AfterPage::Done);
        assert_eq!(after_sync_page("t1", "t2", true), AfterPage::NextPage);
        assert_eq!(after_sync_page("t2", "t2", true), AfterPage::CutShort);
        assert_eq!(after_sync_page("", "t1", true), AfterPage::NextPage);
    }

    #[test]
    fn a_refused_token_lists_whole_and_an_unsupported_report_falls_back() {
        for status in [403, 409, 410] {
            assert_eq!(on_refusal(status, "t1", false, false), Refusal::ListWhole);
        }
        for status in [403, 405, 501] {
            assert_eq!(on_refusal(status, "", true, false), Refusal::Fallback);
            assert_eq!(on_refusal(status, "", false, false), Refusal::Fail);
        }
        assert_eq!(on_refusal(500, "t1", true, false), Refusal::Fail);
        assert_eq!(on_refusal(405, "t1", true, false), Refusal::Fail);
    }

    /// A server that refuses every token it hands out, even mid-way
    /// through a fresh listing, would otherwise restart that listing
    /// forever.
    #[test]
    fn a_second_refused_token_fails_instead_of_restarting() {
        assert_eq!(on_refusal(410, "t2", false, true), Refusal::Fail);
    }

    fn outcome(whole: bool, cut_short: Option<&str>) -> Outcome {
        Outcome {
            whole,
            listed: ["/cal/a.ics".to_string()].into(),
            cut_short: cut_short.map(str::to_string),
        }
    }

    /// Only a whole listing that reached its end may delete: a token's
    /// changes or a cut-short listing say nothing about what they omit.
    #[test]
    fn only_a_whole_complete_listing_names_what_to_delete() {
        let stored = ["/cal/a.ics".to_string(), "/cal/b.ics".to_string()];
        assert_eq!(outcome(true, None).unlisted(&stored), vec!["/cal/b.ics"]);
        assert!(outcome(false, None).unlisted(&stored).is_empty());
        assert!(outcome(true, Some(CUT_SHORT)).unlisted(&stored).is_empty());
    }

    #[test]
    fn a_multiget_names_each_href_escaped() {
        let kind = CollectionKind {
            service: HttpService::Caldav,
            ns_decl: r#"xmlns:C="urn:ietf:params:xml:ns:caldav""#,
            data_prop: "C:calendar-data",
            multiget: "C:calendar-multiget",
            report_depth: "1",
        };
        let body = kind.body_multiget(&["/cal/a&b.ics".into()]);
        assert!(
            body.contains("<C:calendar-multiget xmlns=\"DAV:\" xmlns:C="),
            "{body}"
        );
        assert!(body.contains("<href>/cal/a&amp;b.ics</href>"), "{body}");
        assert!(body.contains("<C:calendar-data/>"), "{body}");
    }
}
