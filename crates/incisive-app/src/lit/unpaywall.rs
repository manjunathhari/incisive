//! Unpaywall lookup: is there a free full-text copy of this DOI?
//!
//! The request carries `?email=` (Unpaywall requires a contact address). The
//! answer is an [`OaLink`]: the HTML link (`best_oa_location.url`, or
//! `url_for_landing_page` when `url` is absent) if there is an https one,
//! otherwise the PDF (`url_for_pdf`, or a `url` that is a PDF) with
//! `is_pdf = true`. Only an https URL is ever returned. That URL is the one place this module
//! calls [`OaPolicy::allow_once`], so the HTTP client will fetch it exactly
//! once. Nothing but the parsed API response reaches the grant.
//!
//! PDFs are not parsed in Phase 2: a link with `is_pdf` counts as "abstract only"
//! to the caller.

use std::fmt;

use serde::Deserialize;
use url::Url;

use super::http::{AllowlistedClient, LitError, OaPolicy};
use super::{OaLink, url_is_pdf};

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
    url_for_landing_page: Option<String>,
}

impl Unpaywall {
    /// `oa` must be the policy shared with `client` (clones share state).
    pub fn new(client: AllowlistedClient, oa: OaPolicy, email: String) -> Self {
        Self { client, oa, email }
    }

    /// The best free full-text link for `doi`, or `None` (not open access,
    /// unknown or malformed DOI, or no https location). Registers the URL for
    /// one fetch.
    pub async fn best_oa(&self, doi: &str) -> Result<Option<OaLink>, LitError> {
        let doi = doi.trim();
        // A DOI starts `10.`; `..` could climb out of the `/v2/` path.
        if !doi.starts_with("10.") || doi.contains("..") {
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
        let https = |u: Option<String>| u.filter(|c| is_https(c));
        let mut pdf = https(loc.url_for_pdf);
        let page = https(loc.url);
        // `url` is the HTML link unless it is a PDF (then it is the PDF link).
        let html = match page {
            Some(u) if url_is_pdf(&u) || pdf.as_deref() == Some(u.as_str()) => {
                pdf.get_or_insert(u);
                None
            }
            Some(u) => Some(u),
            None => https(loc.url_for_landing_page),
        };
        let link = match (html, pdf) {
            (Some(url), _) => Some(OaLink { url, is_pdf: false }),
            (None, Some(url)) => Some(OaLink { url, is_pdf: true }),
            (None, None) => None,
        };
        if let Some(link) = &link {
            // The one grant site: a URL parsed from Unpaywall's own response.
            self.oa.allow_once(&link.url);
        }
        Ok(link)
    }
}

fn is_https(raw: &str) -> bool {
    Url::parse(raw).is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some())
}
