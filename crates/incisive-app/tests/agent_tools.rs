//! Phase 3 T2: the literature tools, against wiremock fixtures (no network)
//! and the paper cache (`TEST_DATABASE_URL`; skipped when unset).

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_client::{ContentBlock, StopReason, Usage};
use incisive_app::agent::Literature;
use incisive_app::agent::agent_loop::{AgentSpec, ResultCaps, run_agent};
use incisive_app::agent::llm::{LlmCall, OutputSchema, ScriptedLlm};
use incisive_app::agent::tools::lit::{FULL_TEXT_CHARS, LitTools};
use incisive_app::agent::tools::{ToolError, ToolRegistry};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::lit::europepmc::EuropePmc;
use incisive_app::lit::http::{AllowlistedClient, OaPolicy};
use incisive_app::lit::pubmed::{PubMed, parse_elink};
use incisive_app::runs::RunMode;
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::store::create_run;
use incisive_app::runs::worker::CancelFlag;
use incisive_app::spend::{CapsConfig, Prices, reserve};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const QUESTION: &str =
    "In adults having colorectal resection, does ICG angiography reduce anastomotic leak?";
const ESEARCH: &str = "/entrez/eutils/esearch.fcgi";
const ESUMMARY: &str = "/entrez/eutils/esummary.fcgi";
const EFETCH: &str = "/entrez/eutils/efetch.fcgi";
const ELINK: &str = "/entrez/eutils/elink.fcgi";
const EPMC_SEARCH: &str = "/europepmc/webservices/rest/search";
const FULLTEXT: &str = "/europepmc/webservices/rest/PMC13375015/fullTextXML";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/lit/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn literature(server: &MockServer, key: Option<&str>) -> Arc<Literature> {
    let oa = OaPolicy::default();
    let client = AllowlistedClient::new("Incisive/test", oa.clone())
        .with_base_override(server.uri())
        .with_retry_backoff(Duration::from_millis(1));
    Arc::new(Literature {
        pubmed: PubMed::new(
            client.clone(),
            key.map(|k| SecretString::from(k.to_string())),
        )
        .with_min_interval(Duration::ZERO),
        epmc: EuropePmc::new(client, oa),
    })
}

async fn mount(server: &MockServer, route: &str, status: u16, body: String) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

async fn requests_to(server: &MockServer, route: &str) -> Vec<wiremock::Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == route)
        .collect()
}

fn param(r: &wiremock::Request, k: &str) -> Option<String> {
    r.url
        .query_pairs()
        .find(|(n, _)| n == k)
        .map(|(_, v)| v.into_owned())
}

fn tools(server: &MockServer, pool: &PgPool) -> ToolRegistry {
    LitTools::new(
        literature(server, None),
        pool.clone(),
        Uuid::new_v4(),
        QUESTION,
    )
    .registry()
}

async fn run(reg: &ToolRegistry, name: &str, input: Value) -> Result<Value, ToolError> {
    let out = reg.get(name).expect("tool").run(&input).await?;
    Ok(serde_json::from_str(&out).expect("tool output is JSON"))
}

fn input_err(r: Result<Value, ToolError>) -> String {
    match r {
        Err(ToolError::Input(m)) => m,
        other => panic!("expected an input error, got {other:?}"),
    }
}

// ---------- schemas ----------

/// Every object in a schema has `additionalProperties: false` and lists
/// all its properties as required; no range keywords (structured outputs
/// and strict tools reject `minimum`, R-FA1 smoke).
fn assert_strict(v: &Value, at: &str) {
    if let Some(o) = v.as_object() {
        for k in [
            "minimum",
            "maximum",
            "minItems",
            "maxItems",
            "minLength",
            "maxLength",
        ] {
            assert!(!o.contains_key(k), "{at}: {k}");
        }
        if o.get("type") == Some(&json!("object")) {
            assert_eq!(o.get("additionalProperties"), Some(&json!(false)), "{at}");
            let mut props: Vec<&String> = o["properties"].as_object().unwrap().keys().collect();
            let mut req: Vec<&str> = o["required"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_str().unwrap())
                .collect();
            props.sort();
            req.sort();
            assert_eq!(props, req, "{at}: every property required");
        }
        for (k, child) in o {
            assert_strict(child, &format!("{at}.{k}"));
        }
    } else if let Some(a) = v.as_array() {
        for c in a {
            assert_strict(c, at);
        }
    }
}

#[tokio::test]
async fn five_tools_with_strict_compatible_schemas() {
    let server = MockServer::start().await;
    let pool = lazy_pool(&SecretString::from(
        "postgres://u@127.0.0.1:9/x".to_string(),
    ))
    .unwrap();
    let reg = LitTools::new(literature(&server, None), pool, Uuid::new_v4(), QUESTION).registry();
    let defs = reg.defs();
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "search_pubmed",
            "search_europepmc",
            "get_abstracts",
            "get_full_text",
            "find_related"
        ]
    );
    for d in &defs {
        assert!(!d.description.is_empty());
        assert_strict(&d.input_schema, &d.name);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

// ---------- searches ----------

#[tokio::test]
async fn search_pubmed_returns_count_and_items() {
    let Some(pool) = pool().await else { return };
    let server = MockServer::start().await;
    mount(&server, ESEARCH, 200, fixture("pubmed_esearch_icg.json")).await;
    mount(&server, ESUMMARY, 200, fixture("pubmed_esummary_icg.json")).await;
    let reg = tools(&server, &pool);
    let q = "(indocyanine green[tiab] OR ICG[tiab]) AND anastomotic leak[tiab]";
    let out = run(
        &reg,
        "search_pubmed",
        json!({"query": q, "max_results": 20}),
    )
    .await
    .unwrap();
    assert_eq!(out["count"], 126);
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 5, "the ids esummary knows");
    assert_eq!(items[0]["pmid"], "40042831");
    for i in items {
        let o = i.as_object().unwrap();
        for k in ["pmid", "title", "year", "journal", "pub_types"] {
            assert!(o.contains_key(k), "{k}");
        }
        assert!(!o.contains_key("abstract"));
    }
    let r = &requests_to(&server, ESEARCH).await[0];
    assert_eq!(param(r, "term").as_deref(), Some(q));
    assert_eq!(param(r, "retmax").as_deref(), Some("20"));
    assert_eq!(param(r, "tool").as_deref(), Some("incisive"));
}

#[tokio::test]
async fn search_inputs_are_validated_before_any_request() {
    let server = MockServer::start().await;
    let pool = lazy_pool(&SecretString::from(
        "postgres://u@127.0.0.1:9/x".to_string(),
    ))
    .unwrap();
    let reg = LitTools::new(literature(&server, None), pool, Uuid::new_v4(), QUESTION).registry();
    for tool in ["search_pubmed", "search_europepmc"] {
        for input in [
            json!({"query": "", "max_results": 5}),
            json!({"query": "  ", "max_results": 5}),
            json!({"query": "icg", "max_results": 0}),
            json!({"query": "icg", "max_results": 51}),
            json!({"query": "icg"}),
            json!({"max_results": 5}),
            json!("icg"),
            json!({"query": "see https://evil.example", "max_results": 5}),
            json!({"query": "a\nb", "max_results": 5}),
            json!({"query": "x".repeat(1_001), "max_results": 5}),
            json!({"query": "(icg", "max_results": 5}),
        ] {
            let m = input_err(run(&reg, tool, input.clone()).await);
            assert!(!m.contains("evil"), "never echoes the input: {m}");
        }
        // The echo guard: eight consecutive words of the question never
        // leave for a database.
        let m = input_err(
            run(
                &reg,
                tool,
                json!({"query": "adults having colorectal resection does ICG angiography reduce anastomotic", "max_results": 5}),
            )
            .await,
        );
        assert!(m.contains("echo"), "{m}");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn search_europepmc_adds_pmcid_and_doi() {
    let server = MockServer::start().await;
    mount(&server, EPMC_SEARCH, 200, fixture("epmc_search_icg.json")).await;
    let pool = lazy_pool(&SecretString::from(
        "postgres://u@127.0.0.1:9/x".to_string(),
    ))
    .unwrap();
    let reg = LitTools::new(literature(&server, None), pool, Uuid::new_v4(), QUESTION).registry();
    let out = run(
        &reg,
        "search_europepmc",
        json!({"query": "TITLE:\"indocyanine green\" AND colorectal", "max_results": 3}),
    )
    .await
    .unwrap();
    assert_eq!(out["count"], 789);
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["pmid"], Value::Null, "a preprint without a PMID");
    assert_eq!(items[0]["pmcid"], "PMC13675078");
    assert_eq!(items[2]["pmcid"], "PMC13601099");
    assert_eq!(items[2]["doi"], "10.1007/s10151-026-03388-4");
    let r = &requests_to(&server, EPMC_SEARCH).await[0];
    assert_eq!(param(r, "pageSize").as_deref(), Some("3"));
}

#[tokio::test]
async fn a_database_failure_is_a_tool_error_without_the_url() {
    let Some(pool) = pool().await else { return };
    let server = MockServer::start().await;
    mount(&server, ESEARCH, 503, "down".into()).await;
    let lit = literature(&server, Some("SECRET-NCBI-KEY"));
    let reg = LitTools::new(lit, pool, Uuid::new_v4(), QUESTION).registry();
    match run(
        &reg,
        "search_pubmed",
        json!({"query": "icg", "max_results": 5}),
    )
    .await
    {
        Err(ToolError::Failed(m)) => {
            assert!(m.contains("could not be reached"), "{m}");
            assert!(!m.contains("SECRET") && !m.contains("http"), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

// ---------- abstracts ----------

#[tokio::test]
async fn get_abstracts_fetches_then_serves_from_the_cache() {
    let Some(pool) = pool().await else { return };
    let ids = ["41060645", "39979619", "35777031"];
    sqlx::query("DELETE FROM incisive.paper_cache WHERE pmid = ANY($1)")
        .bind(ids.map(String::from).to_vec())
        .execute(&pool)
        .await
        .unwrap();
    let server = MockServer::start().await;
    mount(
        &server,
        ESUMMARY,
        200,
        fixture("pubmed_esummary_hernia.json"),
    )
    .await;
    mount(&server, EFETCH, 200, fixture("pubmed_efetch_hernia.xml")).await;
    let reg = tools(&server, &pool);
    let input = json!({"pmids": ["41060645", "39979619", "35777031", "39979619", "123456789"]});
    let out = run(&reg, "get_abstracts", input.clone()).await.unwrap();
    let papers = out["papers"].as_array().unwrap();
    assert_eq!(papers.len(), 3, "duplicates collapsed");
    assert_eq!(out["not_found"], json!(["123456789"]));
    for p in papers {
        let o = p.as_object().unwrap();
        for k in [
            "pmid",
            "title",
            "journal",
            "year",
            "abstract",
            "pub_types",
            "doi",
            "pmcid",
            "retracted",
        ] {
            assert!(o.contains_key(k), "{k}");
        }
        assert!(p["abstract"].as_str().unwrap().len() > 100);
    }
    assert!(
        papers[1]["pub_types"]
            .as_array()
            .unwrap()
            .contains(&json!("Randomized Controlled Trial"))
    );
    assert_eq!(requests_to(&server, EFETCH).await.len(), 1);

    // Second call: all three are cached now; only the unknown id is looked
    // up again, and efetch is not called for the cached ones.
    let out2 = run(&reg, "get_abstracts", input).await.unwrap();
    assert_eq!(out2["papers"], out["papers"]);
    let efetch = requests_to(&server, EFETCH).await;
    assert_eq!(efetch.len(), 1, "esummary found nothing new, so no efetch");
    let summaries = requests_to(&server, ESUMMARY).await;
    assert_eq!(param(&summaries[1], "id").as_deref(), Some("123456789"));
}

#[tokio::test]
async fn get_abstracts_rejects_bad_id_lists() {
    let server = MockServer::start().await;
    let pool = lazy_pool(&SecretString::from(
        "postgres://u@127.0.0.1:9/x".to_string(),
    ))
    .unwrap();
    let reg = LitTools::new(literature(&server, None), pool, Uuid::new_v4(), QUESTION).registry();
    let many: Vec<String> = (1..=21).map(|i| i.to_string()).collect();
    for input in [
        json!({"pmids": []}),
        json!({"pmids": many}),
        json!({"pmids": ["12a"]}),
        json!({"pmids": ["1234567890"]}),
        json!({"pmids": [123]}),
        json!({"pmids": "123"}),
        json!({}),
    ] {
        input_err(run(&reg, "get_abstracts", input).await);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

// ---------- full text ----------

#[tokio::test]
async fn get_full_text_returns_sections_results_before_methods() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(FULLTEXT))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("epmc_fulltext.xml"), "application/xml"),
        )
        .mount(&server)
        .await;
    let pool = lazy_pool(&SecretString::from(
        "postgres://u@127.0.0.1:9/x".to_string(),
    ))
    .unwrap();
    let reg = LitTools::new(literature(&server, None), pool, Uuid::new_v4(), QUESTION).registry();
    let out = run(&reg, "get_full_text", json!({"pmcid": "pmc13375015"}))
        .await
        .unwrap();
    assert_eq!(out["pmcid"], "PMC13375015");
    let names: Vec<&str> = out["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    let pos = |n: &str| names.iter().position(|x| *x == n);
    assert_eq!(names[0], "abstract");
    if let (Some(r), Some(m)) = (pos("results"), pos("methods")) {
        assert!(r < m, "{names:?}");
    }
    let total: usize = out["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["text"].as_str().unwrap().chars().count())
        .sum();
    assert!(total <= FULL_TEXT_CHARS, "{total}");
    assert_eq!(out["truncated"], json!(total == FULL_TEXT_CHARS));

    for bad in ["13375015", "PMC", "PMC12345678901", "PMC1/../x"] {
        input_err(run(&reg, "get_full_text", json!({"pmcid": bad})).await);
    }
}

// ---------- related ----------

#[test]
fn elink_neighbours_parse_both_link_shapes() {
    let body: Value = serde_json::from_str(&fixture("pubmed_elink_neighbor_score.json")).unwrap();
    assert_eq!(
        parse_elink(&body, "40042831", 30).unwrap(),
        ["41871586", "40690925", "33820705", "38577071"],
        "the source id is left out; the pubmed_pubmed link set is used"
    );
    assert_eq!(parse_elink(&body, "40042831", 2).unwrap().len(), 2);
    let plain = json!({"linksets": [{"dbfrom": "pubmed", "ids": ["1"], "linksetdbs": [
        {"dbto": "pubmed", "linkname": "pubmed_pubmed", "links": ["1", "22", "x9", "22", "333"]}
    ]}]});
    assert_eq!(parse_elink(&plain, "1", 30).unwrap(), ["22", "333"]);
    // Ids may come as JSON numbers too (review fix 5).
    let numeric: Value = serde_json::from_str(&fixture("pubmed_elink_numeric_ids.json")).unwrap();
    assert_eq!(
        parse_elink(&numeric, "40042831", 30).unwrap(),
        ["41871586", "40690925", "33820705", "38577071"]
    );
    let none = json!({"linksets": [{"dbfrom": "pubmed", "ids": ["1"]}]});
    assert!(parse_elink(&none, "1", 30).unwrap().is_empty());
    assert!(parse_elink(&json!({"ERROR": "Invalid uid"}), "1", 30).is_err());
    assert!(parse_elink(&json!({}), "1", 30).is_err());
}

#[tokio::test]
async fn find_related_uses_elink_on_eutils_with_the_key_and_returns_items() {
    let Some(pool) = pool().await else { return };
    let server = MockServer::start().await;
    mount(
        &server,
        ELINK,
        200,
        fixture("pubmed_elink_neighbor_score.json"),
    )
    .await;
    mount(&server, ESUMMARY, 200, fixture("pubmed_esummary_icg.json")).await;
    let lit = literature(&server, Some("k-123"));
    let reg = LitTools::new(lit, pool, Uuid::new_v4(), QUESTION).registry();
    let out = run(
        &reg,
        "find_related",
        json!({"pmid": "40042831", "max_results": 3}),
    )
    .await
    .unwrap();
    let items = out["items"].as_array().unwrap();
    assert_eq!(out["count"], 3);
    assert_eq!(
        items
            .iter()
            .map(|i| i["pmid"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["41871586", "40690925", "33820705"]
    );
    assert!(items.iter().all(|i| i["title"].as_str().is_some()));
    let r = &requests_to(&server, ELINK).await[0];
    for (k, v) in [
        ("dbfrom", "pubmed"),
        ("db", "pubmed"),
        ("id", "40042831"),
        ("cmd", "neighbor_score"),
        ("linkname", "pubmed_pubmed"),
        ("retmode", "json"),
        ("api_key", "k-123"),
        ("tool", "incisive"),
    ] {
        assert_eq!(param(r, k).as_deref(), Some(v), "{k}");
    }
    for input in [
        json!({"pmid": "4004a", "max_results": 3}),
        json!({"pmid": "40042831", "max_results": 31}),
        json!({"pmid": "40042831"}),
    ] {
        input_err(run(&reg, "find_related", input).await);
    }
}

// ---------- through the loop ----------

#[tokio::test]
async fn abstracts_reach_the_model_wrapped_as_source() {
    let Some(pool) = pool().await else { return };
    sqlx::query("DELETE FROM incisive.paper_cache WHERE pmid = '9657784'")
        .execute(&pool)
        .await
        .unwrap();
    let server = MockServer::start().await;
    mount(
        &server,
        ESUMMARY,
        200,
        fixture("pubmed_esummary_hernia.json"),
    )
    .await;
    mount(&server, EFETCH, 200, fixture("pubmed_efetch_hernia.xml")).await;

    let email = format!("tools-{}@example.com", Uuid::new_v4());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap();
    let run_id = create_run(&pool, uid, "q", RunMode::Quick, 10_000)
        .await
        .unwrap()
        .unwrap();
    let user = incisive_app::identity::find_user(&pool, &email)
        .await
        .unwrap()
        .unwrap();
    let caps = CapsConfig {
        global_monthly_paise: i64::MAX / 4,
    };
    reserve(&pool, &user, &caps, 10_000, run_id, "run")
        .await
        .unwrap();
    let budget = Arc::new(RunBudget::new(
        run_id,
        uid,
        10_000,
        Instant::now(),
        Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap(),
        88.0,
    ));
    let u = Usage {
        input_tokens: 100,
        output_tokens: 10,
    };
    let llm = Arc::new(
        ScriptedLlm::new()
            .reply_blocks(
                vec![ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "get_abstracts".into(),
                    input: json!({"pmids": ["9657784"]}),
                }],
                Some(StopReason::ToolUse),
                u,
            )
            .reply_blocks(
                vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                Some(StopReason::EndTurn),
                u,
            )
            .reply_json(json!({"ok": true}), u),
    );
    let call = LlmCall::new(
        llm.clone(),
        budget,
        pool.clone(),
        CancelFlag::default(),
        "m-test",
        "researcher",
    );
    let reg = LitTools::new(literature(&server, None), pool, run_id, QUESTION)
        .registry_of(&["get_abstracts"]);
    let schema = OutputSchema::new(json!({
        "type": "object", "additionalProperties": false,
        "properties": {"ok": {"type": "boolean"}}, "required": ["ok"]
    }));
    let spec = AgentSpec {
        system: "s",
        user: "task",
        max_turns: 4,
        max_tokens: 500,
        effort: None,
        final_system: "f",
        final_instruction: "submit",
        final_schema: &schema,
        final_max_tokens: 100,
        caps: ResultCaps::default(),
    };
    let out = run_agent::<Value>(&call, &reg, &spec).await.unwrap();
    assert_eq!(out.output, json!({"ok": true}));
    let reqs = llm.requests();
    assert_eq!(reqs[0].tools.len(), 1);
    assert!(
        reqs[0].output_config.is_none(),
        "no effort: no output_config"
    );
    let ContentBlock::ToolResult {
        content, is_error, ..
    } = &reqs[1].messages[2].content[0]
    else {
        panic!("tool result")
    };
    assert!(!is_error);
    assert!(content.starts_with("<source id=\"t1-1\">"));
    let inner = content
        .strip_prefix("<source id=\"t1-1\">\n")
        .unwrap()
        .strip_suffix("\n</source>")
        .unwrap();
    let v: Value = serde_json::from_str(inner).unwrap();
    assert_eq!(v["papers"][0]["pmid"], "9657784");
    assert!(v["papers"][0]["abstract"].as_str().unwrap().len() > 100);
}
