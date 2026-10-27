//! Unpaywall lookup: is there a free full-text copy of this DOI?
//!
//! The request carries `?email=` (Unpaywall requires a contact address). The
//! answer is `best_oa_location.url_for_pdf`, else `best_oa_location.url`, and
//! only an https URL is ever returned. That URL is the one place this module
//! calls [`OaPolicy::allow_once`], so the HTTP client will fetch it exactly
//! once. Nothing but the parsed API response reaches the grant.
//!
//! PDFs are not parsed in Phase 2: a PDF-only result counts as "abstract only"
//! to the caller.

use std::fmt;

use serde::Deserialize;
use url::Url;

use super::http::{AllowlistedClient, LitError, OaPolicy};

const BASE: &str = "https://api.unpaywall.org/v2/";

pub struct Unpaywall {
    client: AllowlistedClient,
    oa: OaPolicy,
    email: String,
}

impl fmt::Debug for Unpaywall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Unpaywall").finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Response {
    best_oa_location: Option<Location>,
}

#[derive(Deserialize)]
struct Location {
    url_for_pdf: Option<String>,
    url: Option<String>,
}

impl Unpaywall {
    /// `oa` must be the policy shared with `client` (clones share state).
    pub fn new(client: AllowlistedClient, oa: OaPolicy, email: String) -> Self {
        Self { client, oa, email }
    }

    /// The best free full-text URL for `doi`, or `None` (not open access,
    /// unknown DOI, or no https location). Registers the URL for one fetch.
    pub async fn best_oa(&self, doi: &str) -> Result<Option<String>, LitError> {
        let doi = doi.trim();
        if doi.is_empty() {
            return Ok(None);
        }
        let mut u = Url::parse(BASE).expect("static unpaywall url");
        // `set_path` percent-encodes anything that is not path-safe (`?`, `#`, spaces).
        u.set_path(&format!("/v2/{doi}"));
        u.query_pairs_mut().append_pair("email", &self.email);
        let body: Response = match self.client.get_json(u.as_str()).await {
            Ok(b) => b,
            // Unpaywall answers 404 for a DOI it does not know.
            Err(LitError::Http(404)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(loc) = body.best_oa_location else {
            return Ok(None);
        };
        let url = [loc.url_for_pdf, loc.url]
            .into_iter()
            .flatten()
            .find(|c| is_https(c));
        if let Some(url) = &url {
            // The one grant site: a URL parsed from Unpaywall's own response.
            self.oa.allow_once(url);
        }
        Ok(url)
    }
}

fn is_https(raw: &str) -> bool {
    Url::parse(raw).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some())
}
