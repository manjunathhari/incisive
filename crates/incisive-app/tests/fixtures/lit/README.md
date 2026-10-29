# Literature fixtures

Real responses recorded on 2026-10-28 with curl.

- `pubmed_*`: NCBI E-utilities (Task 6).
- `epmc_search_icg.json`: Europe PMC `search` (resultType=core, pageSize=10) for
  `(indocyanine green OR ICG) AND "anastomotic leak" AND colorectal`.
- `epmc_fulltext.xml`: Europe PMC `fullTextXML` for PMC13375015 (in the search above).
- `unpaywall_oa_pdf.json`: DOI 10.1007/s10151-026-03388-4 (has `url_for_pdf`).
- `unpaywall_oa_url_only.json`: DOI 10.1056/NEJMoa2034577 (`url_for_pdf` is null; `url` is set).
- `unpaywall_closed.json`: DOI 10.1016/j.surg.2019.05.025 (not open access).

Unpaywall was queried with the owner's email for the recording only; the saved
responses do not contain it.

- `pubmed_elink_neighbor_score.json`: **constructed, not recorded** (Phase 3
  T2: no live fetch). It follows the documented E-utilities elink JSON shape
  for `dbfrom=pubmed&cmd=neighbor_score&linkname=pubmed_pubmed&retmode=json`
  (`linksets[].linksetdbs[].links[]` of `{"id","score"}`, the source id listed
  first), using the ICG fixture ids so `pubmed_esummary_icg.json` serves the
  titles.
