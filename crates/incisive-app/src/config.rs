//! Environment-driven configuration.

use secrecy::SecretString;
use std::fmt;

use crate::agent::llm::AgentModels;
use crate::runs::{Engine, QuickEngine, RunsConfig};
use crate::spend::Prices;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEnv {
    Dev,
    Prod,
}

#[derive(Debug)]
#[allow(dead_code)] // claude_model is consumed by the evidence agent in Phase 2
pub struct Config {
    /// Required in prod. Optional in dev: without it the gate's model call fails
    /// closed and only rule-decided questions get through.
    pub anthropic_api_key: Option<SecretString>,
    pub claude_model: String,
    pub database_url: SecretString,
    pub app_env: AppEnv,
    pub port: u16,
    /// IAP JWT audience. Required in prod; empty in dev when unset.
    pub iap_audience: String,
    /// Lower-cased email of the owner, bootstrapped at startup.
    pub owner_email: String,
    /// Cheap model for the medical-only gate. Defaults to `claude-haiku-5-5`.
    pub gate_model: String,
    /// USD per million tokens, per model (`MODEL_PRICES`). Required in prod.
    pub model_prices: Prices,
    /// Exchange rate (`USD_INR`). Required in prod; 88 in dev.
    pub usd_inr: f64,
    /// Whole-app monthly budget in paise, from `GLOBAL_MONTHLY_CAP_INR` rupees.
    pub global_monthly_cap_paise: i64,
    /// Per-depth run limits (`RUN_LIMITS`, optional; defaults otherwise).
    pub runs: RunsConfig,
    /// Contact address sent to Unpaywall (required there) and NCBI (`UNPAYWALL_EMAIL`).
    /// Required in prod; an obviously fake address in dev.
    pub unpaywall_email: String,
    /// Model per agent step (`AGENT_MODELS`, optional JSON; missing keys take defaults).
    pub agent_models: AgentModels,
    /// Engine for Quick runs (`QUICK_ENGINE`: `team`, the default, or
    /// `classic`, the Phase 2 pipeline). The owner's rollback switch.
    pub quick_engine: QuickEngine,
    /// Engine for Full runs (`FULL_ENGINE`: `team`, the default, or
    /// `classic`, the Phase 2 Focused pipeline). The owner's rollback switch.
    pub full_engine: Engine,
    /// Stage 4b: the completeness review after the Full writer
    /// (`COMPLETENESS`: `on`, the default, or `off`, the rollback switch).
    pub completeness: bool,
    /// Temporal T2/T3: the expert writer after a saved Full answer
    /// (`TEMPORAL`: `off`, the default, or `on`; `TEMPORAL_MAX_TOKENS`
    /// optional, 2,000–32,000, default 20,000).
    pub temporal: crate::agent::temporal_writer::TemporalSettings,
    /// Stage S1: the two Opus writers stream their replies (`STREAM`: `on`, the
    /// default, or `off`, which restores today's calls exactly).
    pub stream: bool,
    /// The introduction agent (`INTRO`: `off`, the default, or `on`;
    /// `INTRO_MODEL`, default `claude-sonnet-5-5`, priced when `INTRO=on`).
    pub intro: crate::runs::intro::IntroSettings,
    /// The knowledge graph (`GRAPH`: `off`, the default, or `on`; `GRAPH_MODEL`,
    /// priced when on in prod; `GRAPH_FROM`, a `YYYY-MM-DD` date, required when on).
    pub graph: crate::graph::settings::GraphSettings,
    /// The memory layer (`MEMORY`, default off; `GEMMA_URL`, `GEMMA_AUTH`,
    /// `EMBED_MODEL`, `MEMORY_TOPIC_MATCH`, `MEMORY_GATE_MATCH`).
    pub memory: crate::memory::settings::MemorySettings,
    /// The evidence chat (`CHAT`, default off; needs `MEMORY=on`;
    /// `CHAT_MODEL`, `CHAT_DAILY_LIMIT`, `CHAT_PRIOR`, `CHAT_MIN_SCORE`).
    pub chat: crate::chat::settings::ChatSettings,
    /// NCBI E-utilities key (`NCBI_API_KEY`, optional secret). With it PubMed
    /// is paced at 10 requests a second instead of 3. Redacted by `Debug`;
    /// never logged.
    pub ncbi_api_key: Option<SecretString>,
}

/// DEV ONLY. The official prices (USD per million tokens, platform.claude.com,
/// 29 Oct 2026) of every default model. Prod must set `MODEL_PRICES`
/// explicitly (the GitHub variable).
///
/// Haiku 5.5 costs more for prompts above 100k tokens; the rate here is the
/// one up to 100k. Incisive never sends Haiku a prompt that large (the gate
/// and Screen batches are a few thousand tokens), so this rate applies.
const DEV_MODEL_PRICES: &str = r#"{
  "claude-haiku-5-5": {"input": 0.10, "output": 0.50},
  "claude-sonnet-5-5": {"input": 2.0, "output": 10.0},
  "claude-opus-5-5": {"input": 4.0, "output": 20.0}
}"#;
/// DEV ONLY. `.invalid` can never resolve, so a dev run cannot pose as a real contact.
const DEV_UNPAYWALL_EMAIL: &str = "dev@example.invalid";
const DEV_USD_INR: f64 = 88.0;
const DEV_GLOBAL_MONTHLY_CAP_INR: i64 = 10_000;

#[derive(Debug, PartialEq, Eq)]
pub enum ConfigError {
    Missing(&'static str),
    Invalid { var: &'static str, reason: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Missing(var) => {
                write!(f, "missing required environment variable {var}")
            }
            ConfigError::Invalid { var, reason } => {
                write!(f, "invalid value for {var}: {reason}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// The embedding server carries basic-auth credentials, so the URL must be
/// `https://`; plain `http://` is allowed only for `localhost` or `127.0.0.1`
/// (local development and tests).
fn gemma_url_allowed(u: &str) -> bool {
    let Ok(url) = url::Url::parse(u) else {
        return false;
    };
    match (url.scheme(), url.host_str()) {
        ("https", Some(_)) => true,
        ("http", Some(h)) => h == "localhost" || h == "127.0.0.1",
        _ => false,
    }
}

impl Config {
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let lookup = |var: &str| {
            get(var)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let required = |var: &'static str| lookup(var).ok_or(ConfigError::Missing(var));

        // Missing key: allowed only in dev (checked before anything else so a
        // missing key is still the first error reported outside dev).
        let anthropic_api_key = match lookup("ANTHROPIC_API_KEY") {
            Some(k) => Some(SecretString::from(k)),
            None if lookup("APP_ENV").as_deref() == Some("dev") => None,
            None => return Err(ConfigError::Missing("ANTHROPIC_API_KEY")),
        };
        let claude_model = required("CLAUDE_MODEL")?;
        let database_url = SecretString::from(required("DATABASE_URL")?);

        // Required: dev mode trusts a plain header, so it must never be a default.
        let app_env = match required("APP_ENV")?.as_str() {
            "dev" => AppEnv::Dev,
            "prod" => AppEnv::Prod,
            other => {
                return Err(ConfigError::Invalid {
                    var: "APP_ENV",
                    reason: format!("expected `dev` or `prod`, got `{other}`"),
                });
            }
        };

        let port = match lookup("PORT") {
            None => 8080,
            Some(p) => p.parse::<u16>().map_err(|e| ConfigError::Invalid {
                var: "PORT",
                reason: e.to_string(),
            })?,
        };

        let iap_audience = match (app_env, lookup("IAP_AUDIENCE")) {
            (_, Some(a)) => a,
            (AppEnv::Dev, None) => String::new(),
            (AppEnv::Prod, None) => return Err(ConfigError::Missing("IAP_AUDIENCE")),
        };
        let owner_email = required("OWNER_EMAIL")?.to_lowercase();

        let gate_model = lookup("GATE_MODEL").unwrap_or_else(|| "claude-haiku-5-5".to_string());

        let model_prices = match (app_env, lookup("MODEL_PRICES")) {
            (_, Some(json)) => Prices::from_json(&json).map_err(|e| ConfigError::Invalid {
                var: "MODEL_PRICES",
                reason: e
                    .to_string()
                    .trim_start_matches("invalid MODEL_PRICES: ")
                    .to_string(),
            })?,
            (AppEnv::Dev, None) => {
                Prices::from_json(DEV_MODEL_PRICES).expect("dev price table is valid")
            }
            (AppEnv::Prod, None) => return Err(ConfigError::Missing("MODEL_PRICES")),
        };

        // Every model the app calls must be priced, or spend cannot be recorded. In prod
        // that is a startup error; in dev it is only a warning (see main.rs).
        if app_env == AppEnv::Prod {
            for (name, model) in [("GATE_MODEL", &gate_model), ("CLAUDE_MODEL", &claude_model)] {
                if model_prices.get(model).is_none() {
                    return Err(ConfigError::Invalid {
                        var: "MODEL_PRICES",
                        reason: format!(
                            "no entry for {name} `{model}`; add it to MODEL_PRICES or change {name}"
                        ),
                    });
                }
            }
        }

        let agent_models = match lookup("AGENT_MODELS") {
            None => AgentModels::default(),
            Some(json) => AgentModels::from_json(&json).map_err(|reason| ConfigError::Invalid {
                var: "AGENT_MODELS",
                reason,
            })?,
        };
        if app_env == AppEnv::Prod {
            for (step, model) in agent_models.entries() {
                if model_prices.get(model).is_none() {
                    return Err(ConfigError::Invalid {
                        var: "MODEL_PRICES",
                        reason: format!(
                            "no entry for AGENT_MODELS {step} `{model}`; add it to MODEL_PRICES or change AGENT_MODELS"
                        ),
                    });
                }
            }
        }

        let quick_engine = match lookup("QUICK_ENGINE") {
            None => QuickEngine::default(),
            Some(v) => QuickEngine::parse(&v).ok_or_else(|| ConfigError::Invalid {
                var: "QUICK_ENGINE",
                reason: "expected `team` or `classic`".to_string(),
            })?,
        };

        let full_engine = match lookup("FULL_ENGINE") {
            None => Engine::default(),
            Some(v) => Engine::parse(&v).ok_or_else(|| ConfigError::Invalid {
                var: "FULL_ENGINE",
                reason: "expected `team` or `classic`".to_string(),
            })?,
        };

        let completeness = match lookup("COMPLETENESS").as_deref() {
            None | Some("on") => true,
            Some("off") => false,
            Some(_) => {
                return Err(ConfigError::Invalid {
                    var: "COMPLETENESS",
                    reason: "expected `on` or `off`".to_string(),
                });
            }
        };

        let memory = {
            use crate::memory::settings::MemorySettings;
            let on = match lookup("MEMORY").as_deref() {
                None | Some("off") => false,
                Some("on") => true,
                Some(_) => {
                    return Err(ConfigError::Invalid {
                        var: "MEMORY",
                        reason: "expected `on` or `off`".to_string(),
                    });
                }
            };
            let threshold = |var: &'static str, default: f32| match lookup(var) {
                None => Ok(default),
                Some(v) => v
                    .parse::<f32>()
                    .ok()
                    .filter(|n| (0.0..=1.0).contains(n))
                    .ok_or_else(|| ConfigError::Invalid {
                        var,
                        reason: "expected a number from 0.0 to 1.0".to_string(),
                    }),
            };
            let defaults = MemorySettings::default();
            let gemma_url = lookup("GEMMA_URL")
                .map(|u| u.trim().trim_end_matches('/').to_string())
                .filter(|u| !u.is_empty());
            if let Some(u) = &gemma_url
                && !gemma_url_allowed(u)
            {
                return Err(ConfigError::Invalid {
                    var: "GEMMA_URL",
                    reason: "expected an https:// URL (http only for localhost or 127.0.0.1)"
                        .to_string(),
                });
            }
            MemorySettings {
                on,
                gemma_url,
                gemma_auth: lookup("GEMMA_AUTH").map(SecretString::from),
                embed_model: lookup("EMBED_MODEL").unwrap_or(defaults.embed_model),
                topic_match: threshold("MEMORY_TOPIC_MATCH", defaults.topic_match)?,
                gate_match: threshold("MEMORY_GATE_MATCH", defaults.gate_match)?,
            }
        };

        let chat = {
            use crate::chat::settings::ChatSettings;
            let switch = |var: &'static str| match lookup(var).as_deref().map(str::trim) {
                None => Ok(None),
                Some("off") => Ok(Some(false)),
                Some("on") => Ok(Some(true)),
                Some(_) => Err(ConfigError::Invalid {
                    var,
                    reason: "expected `on` or `off`".to_string(),
                }),
            };
            let asked = switch("CHAT")?.unwrap_or(false);
            let prior_switch = switch("CHAT_PRIOR")?;
            let daily_limit = match lookup("CHAT_DAILY_LIMIT") {
                None => None,
                Some(v) => Some(
                    v.trim()
                        .parse::<i64>()
                        .ok()
                        .filter(|n| *n >= 1)
                        .ok_or_else(|| ConfigError::Invalid {
                            var: "CHAT_DAILY_LIMIT",
                            reason: "expected a whole number of 1 or more".to_string(),
                        })?,
                ),
            };
            let min_score = match lookup("CHAT_MIN_SCORE") {
                None => None,
                Some(v) => Some(
                    v.trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|n| (0.0..=1.0).contains(n))
                        .ok_or_else(|| ConfigError::Invalid {
                            var: "CHAT_MIN_SCORE",
                            reason: "expected a number from 0.0 to 1.0".to_string(),
                        })?,
                ),
            };
            // MEMORY=off must stay a safe rollback on its own: chat then
            // turns itself off with a warning instead of failing the boot.
            let on = asked && memory.on;
            if asked && !memory.on {
                tracing::warn!("CHAT=on is ignored because MEMORY is off");
            }
            let defaults = ChatSettings::default();
            ChatSettings {
                on,
                model: lookup("CHAT_MODEL")
                    .map(|m| m.trim().to_string())
                    .filter(|m| !m.is_empty())
                    .unwrap_or(defaults.model),
                daily_limit: daily_limit.unwrap_or(defaults.daily_limit),
                prior: on && prior_switch.unwrap_or(true),
                min_score: min_score.unwrap_or(defaults.min_score),
            }
        };

        let temporal = {
            use crate::agent::temporal_writer::{MAX_TOKENS_RANGE, TemporalSettings};
            let on = match lookup("TEMPORAL").as_deref() {
                None | Some("off") => false,
                Some("on") => true,
                Some(_) => {
                    return Err(ConfigError::Invalid {
                        var: "TEMPORAL",
                        reason: "expected `on` or `off`".to_string(),
                    });
                }
            };
            let max_tokens = match lookup("TEMPORAL_MAX_TOKENS") {
                None => TemporalSettings::default().max_tokens,
                Some(v) => v
                    .parse::<u32>()
                    .ok()
                    .filter(|n| MAX_TOKENS_RANGE.contains(n))
                    .ok_or_else(|| ConfigError::Invalid {
                        var: "TEMPORAL_MAX_TOKENS",
                        reason: format!(
                            "expected a whole number from {} to {}",
                            MAX_TOKENS_RANGE.start(),
                            MAX_TOKENS_RANGE.end()
                        ),
                    })?,
            };
            TemporalSettings {
                on,
                max_tokens,
                ..TemporalSettings::default()
            }
        };

        let stream = match lookup("STREAM").as_deref().map(str::trim) {
            None | Some("") | Some("on") => true,
            Some("off") => false,
            Some(_) => {
                return Err(ConfigError::Invalid {
                    var: "STREAM",
                    reason: "expected on or off".into(),
                });
            }
        };

        let intro = {
            use crate::runs::intro::IntroSettings;
            let on = match lookup("INTRO").as_deref() {
                None | Some("off") => false,
                Some("on") => true,
                Some(_) => {
                    return Err(ConfigError::Invalid {
                        var: "INTRO",
                        reason: "expected `on` or `off`".to_string(),
                    });
                }
            };
            let defaults = IntroSettings::default();
            let model = lookup("INTRO_MODEL").unwrap_or(defaults.model);
            if model.chars().any(char::is_whitespace) {
                return Err(ConfigError::Invalid {
                    var: "INTRO_MODEL",
                    reason: "expected a model id without spaces".to_string(),
                });
            }
            // Only when on: with `INTRO=off` the model is never called, and an
            // unpriced default must not stop a deploy that has it off.
            if on && app_env == AppEnv::Prod && model_prices.get(&model).is_none() {
                return Err(ConfigError::Invalid {
                    var: "MODEL_PRICES",
                    reason: format!(
                        "no entry for INTRO_MODEL `{model}`; add it to MODEL_PRICES or change INTRO_MODEL"
                    ),
                });
            }
            IntroSettings {
                on,
                model,
                ..defaults
            }
        };

        let graph = {
            use crate::graph::settings::GraphSettings;
            let on = match lookup("GRAPH").as_deref() {
                None | Some("off") => false,
                Some("on") => true,
                Some(_) => {
                    return Err(ConfigError::Invalid {
                        var: "GRAPH",
                        reason: "expected `on` or `off`".to_string(),
                    });
                }
            };
            let defaults = GraphSettings::default();
            let model = lookup("GRAPH_MODEL").unwrap_or(defaults.model);
            if model.chars().any(char::is_whitespace) {
                return Err(ConfigError::Invalid {
                    var: "GRAPH_MODEL",
                    reason: "expected a model id without spaces".to_string(),
                });
            }
            let from = match lookup("GRAPH_FROM") {
                None => None,
                Some(v) => Some(
                    Some(v.as_str())
                        .filter(|v| {
                            v.len() == 10
                                && v.bytes().enumerate().all(|(i, b)| {
                                    if i == 4 || i == 7 {
                                        b == b'-'
                                    } else {
                                        b.is_ascii_digit()
                                    }
                                })
                        })
                        .and_then(|v| chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").ok())
                        .ok_or_else(|| ConfigError::Invalid {
                            var: "GRAPH_FROM",
                            reason: "expected a date as YYYY-MM-DD".to_string(),
                        })?,
                ),
            };

            if on && from.is_none() {
                return Err(ConfigError::Invalid {
                    var: "GRAPH_FROM",
                    reason: "required when GRAPH=on (a date as YYYY-MM-DD)".to_string(),
                });
            }
            // Only when on, as for INTRO_MODEL.
            if on && app_env == AppEnv::Prod && model_prices.get(&model).is_none() {
                return Err(ConfigError::Invalid {
                    var: "MODEL_PRICES",
                    reason: format!(
                        "no entry for GRAPH_MODEL `{model}`; add it to MODEL_PRICES or change GRAPH_MODEL"
                    ),
                });
            }
            GraphSettings {
                on,
                model,
                from,
                ..GraphSettings::default()
            }
        };

        let usd_inr = match (app_env, lookup("USD_INR")) {
            (_, Some(v)) => {
                let rate = v.parse::<f64>().map_err(|e| ConfigError::Invalid {
                    var: "USD_INR",
                    reason: e.to_string(),
                })?;
                if !rate.is_finite() || rate <= 0.0 {
                    return Err(ConfigError::Invalid {
                        var: "USD_INR",
                        reason: "must be a positive number".to_string(),
                    });
                }
                rate
            }
            (AppEnv::Dev, None) => DEV_USD_INR,
            (AppEnv::Prod, None) => return Err(ConfigError::Missing("USD_INR")),
        };

        let global_cap_inr = match (app_env, lookup("GLOBAL_MONTHLY_CAP_INR")) {
            (_, Some(v)) => v.parse::<i64>().map_err(|e| ConfigError::Invalid {
                var: "GLOBAL_MONTHLY_CAP_INR",
                reason: format!("expected whole rupees: {e}"),
            })?,
            (AppEnv::Dev, None) => DEV_GLOBAL_MONTHLY_CAP_INR,
            (AppEnv::Prod, None) => return Err(ConfigError::Missing("GLOBAL_MONTHLY_CAP_INR")),
        };
        let global_monthly_cap_paise = match global_cap_inr.checked_mul(100) {
            Some(p) if p > 0 => p,
            _ => {
                return Err(ConfigError::Invalid {
                    var: "GLOBAL_MONTHLY_CAP_INR",
                    reason: "must be a positive number of rupees".to_string(),
                });
            }
        };

        let runs = match lookup("RUN_LIMITS") {
            None => RunsConfig::default(),
            Some(json) => RunsConfig::from_json(&json).map_err(|reason| ConfigError::Invalid {
                var: "RUN_LIMITS",
                reason,
            })?,
        };

        let unpaywall_email = match (app_env, lookup("UNPAYWALL_EMAIL")) {
            (_, Some(e)) => {
                if !e.contains('@') || e.chars().any(char::is_whitespace) {
                    return Err(ConfigError::Invalid {
                        var: "UNPAYWALL_EMAIL",
                        reason: "expected an email address".to_string(),
                    });
                }
                e
            }
            (AppEnv::Dev, None) => DEV_UNPAYWALL_EMAIL.to_string(),
            (AppEnv::Prod, None) => return Err(ConfigError::Missing("UNPAYWALL_EMAIL")),
        };

        Ok(Config {
            anthropic_api_key,
            claude_model,
            database_url,
            app_env,
            port,
            iap_audience,
            owner_email,
            gate_model,
            model_prices,
            usd_inr,
            global_monthly_cap_paise,
            runs,
            unpaywall_email,
            agent_models,
            quick_engine,
            full_engine,
            completeness,
            temporal,
            stream,
            intro,
            graph,
            memory,
            chat,
            ncbi_api_key: lookup("NCBI_API_KEY").map(SecretString::from),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    const DB: &str = "postgres://user:hunter2pw@host/db";
    /// Prices for the default gate model and the `CLAUDE_MODEL` used by `full()`.
    const PROD_PRICES: &str = r#"{"claude-haiku-5-5": {"input": 1, "output": 5}, "claude-sonnet-5-5": {"input": 3, "output": 15}, "claude-opus-5-5": {"input": 4, "output": 20}, "claude-test": {"input": 3, "output": 15}}"#;

    fn full() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ANTHROPIC_API_KEY", "sk-test-123"),
            ("CLAUDE_MODEL", "claude-test"),
            ("DATABASE_URL", DB),
            ("APP_ENV", "dev"),
            ("OWNER_EMAIL", "Owner@Example.com"),
        ]
    }

    #[test]
    fn missing_api_key_is_named() {
        let err = Config::from_lookup(lookup(&[("CLAUDE_MODEL", "m"), ("DATABASE_URL", DB)]))
            .unwrap_err();
        assert_eq!(err, ConfigError::Missing("ANTHROPIC_API_KEY"));
        assert!(err.to_string().contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn blank_value_counts_as_missing() {
        let err = Config::from_lookup(lookup(&[
            ("ANTHROPIC_API_KEY", "   "),
            ("CLAUDE_MODEL", "m"),
            ("DATABASE_URL", DB),
        ]))
        .unwrap_err();
        assert_eq!(err, ConfigError::Missing("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn api_key_optional_only_in_dev() {
        let without = |env: &'static str| -> Vec<(&'static str, &'static str)> {
            let mut v: Vec<_> = full()
                .into_iter()
                .filter(|(k, _)| *k != "ANTHROPIC_API_KEY" && *k != "APP_ENV")
                .collect();
            v.push(("APP_ENV", env));
            v
        };
        let dev = Config::from_lookup(lookup(&without("dev"))).unwrap();
        assert!(dev.anthropic_api_key.is_none());
        let mut prod = without("prod");
        prod.extend([
            ("IAP_AUDIENCE", "/projects/1/apps/2"),
            ("MODEL_PRICES", PROD_PRICES),
            ("USD_INR", "88"),
            ("GLOBAL_MONTHLY_CAP_INR", "10000"),
        ]);
        assert_eq!(
            Config::from_lookup(lookup(&prod)).unwrap_err(),
            ConfigError::Missing("ANTHROPIC_API_KEY")
        );
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert!(c.anthropic_api_key.is_some());
    }

    #[test]
    fn port_defaults_to_8080() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.app_env, AppEnv::Dev);
    }

    #[test]
    fn missing_app_env_is_error() {
        let pairs: Vec<_> = full()
            .into_iter()
            .filter(|(k, _)| *k != "APP_ENV")
            .collect();
        let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("APP_ENV"));
        let mut blank = pairs;
        blank.push(("APP_ENV", "  "));
        assert_eq!(
            Config::from_lookup(lookup(&blank)).unwrap_err(),
            ConfigError::Missing("APP_ENV")
        );
    }

    #[test]
    fn bad_port_is_invalid() {
        let mut pairs = full();
        pairs.push(("PORT", "abc"));
        let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { var: "PORT", .. }));
        assert!(err.to_string().starts_with("invalid value for PORT: "));
    }

    #[test]
    fn owner_email_required_and_lowercased() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.owner_email, "owner@example.com");
        let pairs: Vec<_> = full()
            .into_iter()
            .filter(|(k, _)| *k != "OWNER_EMAIL")
            .collect();
        let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("OWNER_EMAIL"));
    }

    #[test]
    fn iap_audience_required_in_prod_optional_in_dev() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.iap_audience, "");
        let mut prod = full();
        prod.push(("APP_ENV", "prod"));
        let err = Config::from_lookup(lookup(&prod)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("IAP_AUDIENCE"));
        prod.push(("IAP_AUDIENCE", "/projects/1/apps/2"));
        prod.push(("MODEL_PRICES", PROD_PRICES));
        prod.push(("USD_INR", "90"));
        prod.push(("GLOBAL_MONTHLY_CAP_INR", "5000"));
        assert_eq!(
            Config::from_lookup(lookup(&prod)).unwrap_err(),
            ConfigError::Missing("UNPAYWALL_EMAIL")
        );
        prod.push(("UNPAYWALL_EMAIL", "ops@example.org"));
        let c = Config::from_lookup(lookup(&prod)).unwrap();
        assert_eq!(c.iap_audience, "/projects/1/apps/2");
    }

    #[test]
    fn gate_model_defaults_and_can_be_overridden() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.gate_model, "claude-haiku-5-5");
        let mut pairs = full();
        pairs.push(("GATE_MODEL", " claude-gate-test "));
        let c = Config::from_lookup(lookup(&pairs)).unwrap();
        assert_eq!(c.gate_model, "claude-gate-test");
        let mut blank = full();
        blank.push(("GATE_MODEL", "  "));
        let c = Config::from_lookup(lookup(&blank)).unwrap();
        assert_eq!(c.gate_model, "claude-haiku-5-5");
    }

    #[test]
    fn spend_settings_have_dev_defaults() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.usd_inr, 88.0);
        assert_eq!(c.global_monthly_cap_paise, 1_000_000);
        assert!(c.model_prices.get("claude-haiku-5-5").is_some());
        assert!(c.model_prices.get("claude-sonnet-5-5").is_some());
    }

    #[test]
    fn spend_settings_required_in_prod() {
        let mut prod = full();
        prod.push(("APP_ENV", "prod"));
        prod.push(("IAP_AUDIENCE", "/projects/1/apps/2"));
        assert_eq!(
            Config::from_lookup(lookup(&prod)).unwrap_err(),
            ConfigError::Missing("MODEL_PRICES")
        );
        prod.push(("MODEL_PRICES", PROD_PRICES));
        assert_eq!(
            Config::from_lookup(lookup(&prod)).unwrap_err(),
            ConfigError::Missing("USD_INR")
        );
        prod.push(("USD_INR", "90"));
        assert_eq!(
            Config::from_lookup(lookup(&prod)).unwrap_err(),
            ConfigError::Missing("GLOBAL_MONTHLY_CAP_INR")
        );
        prod.push(("GLOBAL_MONTHLY_CAP_INR", "5000"));
        assert_eq!(
            Config::from_lookup(lookup(&prod)).unwrap_err(),
            ConfigError::Missing("UNPAYWALL_EMAIL")
        );
        prod.push(("UNPAYWALL_EMAIL", "ops@example.org"));
        let c = Config::from_lookup(lookup(&prod)).unwrap();
        assert_eq!(c.usd_inr, 90.0);
        assert_eq!(c.global_monthly_cap_paise, 500_000);
        assert!(c.model_prices.get("claude-haiku-5-5").is_some());
    }

    fn prod_with(prices: &'static str) -> Vec<(&'static str, &'static str)> {
        let mut prod = full();
        prod.extend([
            ("APP_ENV", "prod"),
            ("IAP_AUDIENCE", "/projects/1/apps/2"),
            ("MODEL_PRICES", prices),
            ("USD_INR", "90"),
            ("GLOBAL_MONTHLY_CAP_INR", "5000"),
        ]);
        prod
    }

    #[test]
    fn prod_requires_gate_model_priced() {
        // CLAUDE_MODEL (`claude-test`) is priced; the default gate model is not.
        let err = Config::from_lookup(lookup(&prod_with(
            r#"{"claude-test": {"input": 3, "output": 15}}"#,
        )))
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "MODEL_PRICES",
                ..
            }
        ));
        let msg = err.to_string();
        assert!(
            msg.contains("MODEL_PRICES") && msg.contains("GATE_MODEL"),
            "{msg}"
        );
        assert!(msg.contains("claude-haiku-5-5"), "{msg}");
    }

    #[test]
    fn prod_requires_claude_model_priced() {
        let err = Config::from_lookup(lookup(&prod_with(
            r#"{"claude-haiku-5-5": {"input": 1, "output": 5}}"#,
        )))
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("MODEL_PRICES") && msg.contains("CLAUDE_MODEL"),
            "{msg}"
        );
        assert!(msg.contains("claude-test"), "{msg}");
    }

    #[test]
    fn dev_allows_unpriced_models() {
        let mut pairs = full();
        pairs.push(("MODEL_PRICES", r#"{"other": {"input": 1, "output": 5}}"#));
        assert!(Config::from_lookup(lookup(&pairs)).is_ok());
    }

    #[test]
    fn bad_spend_settings_name_the_variable() {
        for (var, bad) in [
            ("MODEL_PRICES", "not json"),
            ("USD_INR", "abc"),
            ("USD_INR", "0"),
            ("USD_INR", "NaN"),
            ("GLOBAL_MONTHLY_CAP_INR", "ten"),
            ("GLOBAL_MONTHLY_CAP_INR", "0"),
            ("GLOBAL_MONTHLY_CAP_INR", "-5"),
            ("GLOBAL_MONTHLY_CAP_INR", "99999999999999999"),
        ] {
            let mut pairs = full();
            pairs.push((var, bad));
            let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { var: v, .. } if *v == var),
                "{var}={bad}: {err:?}"
            );
            assert!(err.to_string().contains(var));
        }
    }

    #[test]
    fn run_limits_json_parses_and_bad_value_names_variable() {
        use crate::runs::{DepthSettings, RunsConfig};
        // Unset: the Global Constraints defaults.
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.runs, RunsConfig::default());
        assert_eq!(
            c.runs.quick,
            DepthSettings {
                refinements: 1,
                screen_n: 20,
                use_n: 5,
                full_text: false,
                limit_paise: 1500,
                time_limit_s: 300,
                // Phase 3 (T3): additive; the Phase 2 pipeline ignores them.
                // Owner ruling R-S2a: ₹100; review: 6 minutes, writer 16k
                // tokens and 150 s kept for it.
                agent_limit_paise: 10000,
                agent_time_limit_s: 360,
                researcher_turns: 8,
                skeptic_turns: 0,
                researcher_max_tokens: 4000,
                skeptic_max_tokens: 4000,
                extractor_max_tokens: 4000,
                writer_max_tokens: 20000,
                writer_time_allowance_s: 150,
                review_time_allowance_s: 0,
                extractor_concurrency: 3,
            }
        );
        assert_eq!(
            c.runs.focused,
            DepthSettings {
                refinements: 3,
                screen_n: 100,
                use_n: 10,
                full_text: true,
                limit_paise: 4000,
                time_limit_s: 300,
                // Owner ruling R-S3a: ₹300, 10 minutes, 180 s for the review.
                agent_limit_paise: 30000,
                agent_time_limit_s: 600,
                researcher_turns: 15,
                skeptic_turns: 8,
                researcher_max_tokens: 4000,
                skeptic_max_tokens: 4000,
                extractor_max_tokens: 4000,
                writer_max_tokens: 20000,
                writer_time_allowance_s: 180,
                // Stage 3: the Full review phase (extractor ∥ skeptic).
                review_time_allowance_s: 180,
                extractor_concurrency: 3,
            }
        );

        // Partial override: only the named fields change.
        let mut pairs = full();
        pairs.push((
            "RUN_LIMITS",
            r#"{"focused": {"limit_paise": 6000, "screen_n": 150}, "quick": {"refinements": 2}}"#,
        ));
        let c = Config::from_lookup(lookup(&pairs)).unwrap();
        assert_eq!(c.runs.focused.limit_paise, 6000);
        assert_eq!(c.runs.focused.screen_n, 150);
        assert_eq!(c.runs.focused.use_n, 10);
        assert_eq!(c.runs.quick.refinements, 2);
        assert_eq!(c.runs.quick.limit_paise, 1500);
        assert_eq!(
            c.runs.for_mode(crate::runs::RunMode::Focused).limit_paise,
            6000
        );

        // Phase 3 fields override the same way, beside Phase 2 ones.
        let mut pairs = full();
        pairs.push((
            "RUN_LIMITS",
            r#"{"focused": {"researcher_turns": 12, "agent_limit_paise": 25000, "limit_paise": 4500}, "quick": {"writer_max_tokens": 6000}}"#,
        ));
        let c = Config::from_lookup(lookup(&pairs)).unwrap();
        assert_eq!(c.runs.focused.researcher_turns, 12);
        assert_eq!(c.runs.focused.agent_limit_paise, 25000);
        assert_eq!(c.runs.focused.limit_paise, 4500);
        assert_eq!(c.runs.focused.skeptic_turns, 8);
        assert_eq!(c.runs.quick.writer_max_tokens, 6000);
        assert_eq!(c.runs.quick.agent_limit_paise, 10000);

        for bad in [
            "not json",
            "[]",
            r#"{"deep": {}}"#,
            r#"{"quick": 5}"#,
            r#"{"quick": {"limit_paise": 0}}"#,
            r#"{"quick": {"limit_paise": -5}}"#,
            r#"{"quick": {"limit_paise": 1.5}}"#,
            r#"{"focused": {"time_limit_s": 0}}"#,
            r#"{"focused": {"screen_n": 0}}"#,
            r#"{"focused": {"use_n": 0}}"#,
            r#"{"focused": {"refinements": 300}}"#,
            r#"{"focused": {"full_text": "yes"}}"#,
            r#"{"focused": {"limit_rupees": 40}}"#,
            r#"{"focused": {"agent_limit_paise": 0}}"#,
            r#"{"quick": {"agent_time_limit_s": 0}}"#,
            r#"{"quick": {"researcher_turns": 0}}"#,
            r#"{"focused": {"researcher_turns": 51}}"#,
            r#"{"focused": {"skeptic_turns": 51}}"#,
            r#"{"focused": {"researcher_max_tokens": 0}}"#,
            r#"{"focused": {"writer_max_tokens": 64001}}"#,
            r#"{"quick": {"extractor_max_tokens": 0}}"#,
            r#"{"quick": {"skeptic_max_tokens": 100000}}"#,
            r#"{"quick": {"skeptic_turn": 2}}"#,
            // The writer's time must leave research some time.
            r#"{"quick": {"writer_time_allowance_s": 360}}"#,
            r#"{"focused": {"writer_time_allowance_s": 900}}"#,
            // Stage 3: the writer's and the review's time together must
            // leave research some time; the extractor runs 1 to 8 at once.
            r#"{"focused": {"review_time_allowance_s": 450}}"#,
            r#"{"focused": {"extractor_concurrency": 0}}"#,
            r#"{"focused": {"extractor_concurrency": 9}}"#,
        ] {
            let mut pairs = full();
            pairs.push(("RUN_LIMITS", bad));
            let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConfigError::Invalid {
                        var: "RUN_LIMITS",
                        ..
                    }
                ),
                "{bad}: {err:?}"
            );
            assert!(err.to_string().contains("RUN_LIMITS"), "{err}");
        }
    }

    #[test]
    fn unpaywall_email_required_in_prod() {
        // Dev: an obviously fake default.
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.unpaywall_email, "dev@example.invalid");
        // Prod: missing (or blank) is an error that names the variable.
        let mut prod = prod_with(PROD_PRICES);
        let err = Config::from_lookup(lookup(&prod)).unwrap_err();
        assert_eq!(err, ConfigError::Missing("UNPAYWALL_EMAIL"));
        assert!(err.to_string().contains("UNPAYWALL_EMAIL"));
        prod.push(("UNPAYWALL_EMAIL", "   "));
        assert_eq!(
            Config::from_lookup(lookup(&prod)).unwrap_err(),
            ConfigError::Missing("UNPAYWALL_EMAIL")
        );
        // Set: used as given (trimmed).
        prod.pop();
        prod.push(("UNPAYWALL_EMAIL", " ops@example.org "));
        let c = Config::from_lookup(lookup(&prod)).unwrap();
        assert_eq!(c.unpaywall_email, "ops@example.org");
        // Dev may override too; a value that is not an address names the variable.
        let mut dev = full();
        dev.push(("UNPAYWALL_EMAIL", "not-an-address"));
        let err = Config::from_lookup(lookup(&dev)).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "UNPAYWALL_EMAIL",
                ..
            }
        ));
        assert!(err.to_string().contains("UNPAYWALL_EMAIL"));
    }

    /// Stage 2 §0: the official prices (platform.claude.com, 29 Oct 2026),
    /// USD per million tokens, for every default model.
    #[test]
    fn dev_prices_are_the_official_rates() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        for (model, input, output) in [
            ("claude-opus-5-5", 4.0, 20.0),
            ("claude-sonnet-5-5", 2.0, 10.0),
            ("claude-haiku-5-5", 0.10, 0.50),
        ] {
            let p = c
                .model_prices
                .get(model)
                .unwrap_or_else(|| panic!("{model}"));
            assert_eq!(p.input_usd_per_mtok, input, "{model} input");
            assert_eq!(p.output_usd_per_mtok, output, "{model} output");
        }
        // Every default agent model is priced in dev too.
        for (role, model) in c.agent_models.entries() {
            assert!(c.model_prices.get(model).is_some(), "{role} `{model}`");
        }
    }

    /// Stage 2 §0: `QUICK_ENGINE` is `team` (default) or `classic`; anything
    /// else is refused at startup, naming the variable.
    #[test]
    fn quick_engine_switch() {
        use crate::runs::QuickEngine;
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.quick_engine, QuickEngine::Team);
        for (v, want) in [
            ("team", QuickEngine::Team),
            ("classic", QuickEngine::Classic),
            (" classic ", QuickEngine::Classic),
            ("   ", QuickEngine::Team),
        ] {
            let mut pairs = full();
            pairs.push(("QUICK_ENGINE", v));
            assert_eq!(
                Config::from_lookup(lookup(&pairs)).unwrap().quick_engine,
                want,
                "{v:?}"
            );
        }
        for bad in ["Team", "CLASSIC", "phase2", "team,classic", "1"] {
            let mut pairs = full();
            pairs.push(("QUICK_ENGINE", bad));
            let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        var: "QUICK_ENGINE",
                        ..
                    }
                ),
                "{bad}: {err:?}"
            );
            let msg = err.to_string();
            assert!(msg.contains("team") && msg.contains("classic"), "{msg}");
            assert!(!msg.contains(bad), "the value is not echoed: {msg}");
        }
        // Prod too.
        let mut prod = prod_with(PROD_PRICES);
        prod.push(("UNPAYWALL_EMAIL", "ops@example.org"));
        prod.push(("QUICK_ENGINE", "classic"));
        assert_eq!(
            Config::from_lookup(lookup(&prod)).unwrap().quick_engine,
            QuickEngine::Classic
        );
    }

    /// Stage 3 §0: `FULL_ENGINE` is `team` (default) or `classic`, checked
    /// like `QUICK_ENGINE`, and independent of it.
    #[test]
    fn full_engine_switch() {
        use crate::runs::{Engine, QuickEngine};
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.full_engine, Engine::Team);
        for (v, want) in [
            ("team", Engine::Team),
            ("classic", Engine::Classic),
            (" classic ", Engine::Classic),
            ("   ", Engine::Team),
        ] {
            let mut pairs = full();
            pairs.push(("FULL_ENGINE", v));
            let c = Config::from_lookup(lookup(&pairs)).unwrap();
            assert_eq!(c.full_engine, want, "{v:?}");
            assert_eq!(c.quick_engine, QuickEngine::Team, "Quick is its own switch");
        }
        for bad in ["Team", "CLASSIC", "phase2", "team,classic", "1"] {
            let mut pairs = full();
            pairs.push(("FULL_ENGINE", bad));
            let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        var: "FULL_ENGINE",
                        ..
                    }
                ),
                "{bad}: {err:?}"
            );
            let msg = err.to_string();
            assert!(!msg.contains(bad), "the value is not echoed: {msg}");
        }
        let mut prod = prod_with(PROD_PRICES);
        prod.push(("UNPAYWALL_EMAIL", "ops@example.org"));
        prod.push(("FULL_ENGINE", "classic"));
        prod.push(("QUICK_ENGINE", "team"));
        let c = Config::from_lookup(lookup(&prod)).unwrap();
        assert_eq!(
            (c.quick_engine, c.full_engine),
            (Engine::Team, Engine::Classic)
        );
    }

    #[test]
    fn memory_defaults_off() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert!(!c.memory.on);
        assert_eq!(c.memory.embed_model, "embeddinggemma");
        assert_eq!(c.memory.topic_match, 0.80);
        assert_eq!(c.memory.gate_match, 0.85);
        assert!(!c.memory.embeddings_enabled());
    }

    #[test]
    fn memory_on_parses() {
        let mut p = full();
        p.push(("MEMORY", " on "));
        assert!(Config::from_lookup(lookup(&p)).unwrap().memory.on);
    }

    #[test]
    fn memory_rejects_unknown() {
        let mut p = full();
        p.push(("MEMORY", "yes"));
        assert!(matches!(
            Config::from_lookup(lookup(&p)).unwrap_err(),
            ConfigError::Invalid { var: "MEMORY", .. }
        ));
    }

    #[test]
    fn memory_threshold_range() {
        for (var, bad) in [
            ("MEMORY_GATE_MATCH", "1.5"),
            ("MEMORY_GATE_MATCH", "-0.1"),
            ("MEMORY_TOPIC_MATCH", "abc"),
            ("MEMORY_TOPIC_MATCH", "2"),
        ] {
            let mut p = full();
            p.push((var, bad));
            match Config::from_lookup(lookup(&p)).unwrap_err() {
                ConfigError::Invalid { var: v, .. } => assert_eq!(v, var, "{bad}"),
                e => panic!("{bad}: {e:?}"),
            }
        }
        let mut p = full();
        p.push(("MEMORY_GATE_MATCH", "0.9"));
        p.push(("MEMORY_TOPIC_MATCH", "0"));
        let c = Config::from_lookup(lookup(&p)).unwrap();
        assert_eq!((c.memory.gate_match, c.memory.topic_match), (0.9, 0.0));
    }

    #[test]
    fn chat_defaults_off() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert!(!c.chat.on);
        assert_eq!(c.chat.model, "gemma3:4b");
        assert_eq!(c.chat.daily_limit, 30);
        assert!(!c.chat.prior);
        assert_eq!(c.chat.min_score, 0.45);
    }

    #[test]
    fn chat_invalid_value_is_config_error() {
        for (var, bad) in [
            ("CHAT", "yes"),
            ("CHAT_PRIOR", "maybe"),
            ("CHAT_DAILY_LIMIT", "abc"),
            ("CHAT_DAILY_LIMIT", "0"),
            ("CHAT_DAILY_LIMIT", "-3"),
        ] {
            let mut p = full();
            p.push(("MEMORY", "on"));
            p.push((var, bad));
            match Config::from_lookup(lookup(&p)).unwrap_err() {
                ConfigError::Invalid { var: v, .. } => assert_eq!(v, var, "{bad}"),
                e => panic!("{bad}: {e:?}"),
            }
        }
    }

    #[test]
    fn chat_on_with_memory_off_is_off() {
        let mut p = full();
        p.push(("CHAT", "on"));
        let c = Config::from_lookup(lookup(&p)).unwrap();
        assert!(!c.chat.on);
        assert!(!c.chat.prior, "no prior while chat is off");
        p.push(("MEMORY", "on"));
        assert!(Config::from_lookup(lookup(&p)).unwrap().chat.on);
    }

    #[test]
    fn chat_prior_defaults_on_when_chat_on() {
        let mut p = full();
        p.push(("MEMORY", "on"));
        p.push(("CHAT", " on "));
        assert!(Config::from_lookup(lookup(&p)).unwrap().chat.prior);
        p.push(("CHAT_PRIOR", "off"));
        assert!(!Config::from_lookup(lookup(&p)).unwrap().chat.prior);
        p.push(("CHAT_MODEL", "gemma3:12b"));
        p.push(("CHAT_DAILY_LIMIT", "5"));
        let c = Config::from_lookup(lookup(&p)).unwrap();
        assert_eq!(
            (c.chat.model.as_str(), c.chat.daily_limit),
            ("gemma3:12b", 5)
        );
    }

    #[test]
    fn chat_min_score_out_of_range_is_config_error() {
        for bad in ["1.5", "-0.1", "abc"] {
            let mut p = full();
            p.push(("CHAT_MIN_SCORE", bad));
            assert!(matches!(
                Config::from_lookup(lookup(&p)).unwrap_err(),
                ConfigError::Invalid {
                    var: "CHAT_MIN_SCORE",
                    ..
                }
            ));
        }
        let mut p = full();
        p.push(("CHAT_MIN_SCORE", "0.6"));
        assert_eq!(Config::from_lookup(lookup(&p)).unwrap().chat.min_score, 0.6);
    }

    #[test]
    fn embeddings_need_url_and_auth() {
        let mut p = full();
        p.push(("GEMMA_URL", "https://gemma.example.org/"));
        let c = Config::from_lookup(lookup(&p)).unwrap();
        assert!(!c.memory.embeddings_enabled());
        assert_eq!(
            c.memory.gemma_url.as_deref(),
            Some("https://gemma.example.org")
        );
        p.push(("GEMMA_AUTH", "tok"));
        assert!(
            Config::from_lookup(lookup(&p))
                .unwrap()
                .memory
                .embeddings_enabled()
        );
    }

    #[test]
    fn gemma_url_must_be_https_or_loopback() {
        for ok in [
            "https://gemma.example.org",
            "http://localhost:11434",
            "http://127.0.0.1:8080/",
            "https://x.example",
            "http://127.0.0.1",
        ] {
            let mut p = full();
            p.push(("GEMMA_URL", ok));
            assert!(Config::from_lookup(lookup(&p)).is_ok(), "{ok}");
        }
        for bad in [
            "http://gemma.example.org",
            "ftp://gemma.example.org",
            "gemma.example.org",
            "http://localhost@evil.example.org",
            "http://127.0.0.1.evil.example.org",
            "http://evil.example\\@localhost/",
            "http://localhost.evil.example",
            "ftp://x",
        ] {
            let mut p = full();
            p.push(("GEMMA_URL", bad));
            let err = Config::from_lookup(lookup(&p)).unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        var: "GEMMA_URL",
                        ..
                    }
                ),
                "{bad}"
            );
            assert!(!err.to_string().contains(bad));
        }
    }

    #[test]
    fn gemma_url_rejects_backslash_bypass() {
        assert!(!gemma_url_allowed("http://evil.example\\@localhost/"));
        assert!(!gemma_url_allowed("http://evil.example\\@127.0.0.1/"));
        assert!(gemma_url_allowed("https://x.example"));
        assert!(gemma_url_allowed("http://localhost:11434"));
        assert!(gemma_url_allowed("http://127.0.0.1"));
        assert!(!gemma_url_allowed("http://localhost.evil.example"));
        assert!(!gemma_url_allowed("ftp://x"));
        assert!(!gemma_url_allowed("https://"));
    }

    #[test]
    fn gemma_auth_not_in_debug() {
        let mut p = full();
        p.push(("GEMMA_AUTH", "s3cr3t-gemma-token"));
        let c = Config::from_lookup(lookup(&p)).unwrap();
        assert!(!format!("{:?}", c.memory).contains("s3cr3t-gemma-token"));
    }

    /// `STREAM` is `on` (default) or `off`, trimmed; anything else refuses at
    /// startup.
    #[test]
    fn stream_switch() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert!(c.stream, "on by default");
        for (v, want) in [
            ("on", true),
            ("off", false),
            (" off ", false),
            (" on", true),
        ] {
            let mut pairs = full();
            pairs.push(("STREAM", v));
            assert_eq!(
                Config::from_lookup(lookup(&pairs)).unwrap().stream,
                want,
                "{v}"
            );
        }
        for bad in ["true", "OFF", "1"] {
            let mut pairs = full();
            pairs.push(("STREAM", bad));
            assert!(matches!(
                Config::from_lookup(lookup(&pairs)),
                Err(ConfigError::Invalid { var: "STREAM", .. })
            ));
        }
    }

    /// `INTRO` is `off` (default) or `on`, trimmed; anything else refuses at
    /// startup. `INTRO_MODEL` defaults to Sonnet and must be priced in prod
    /// when `INTRO=on`.
    #[test]
    fn intro_switch() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert!(!c.intro.on, "off by default");
        assert_eq!(c.intro.model, "claude-sonnet-5-5");
        for (v, on) in [
            ("on", true),
            ("off", false),
            ("  on\n", true),
            (" off ", false),
        ] {
            let mut pairs = full();
            pairs.push(("INTRO", v));
            assert_eq!(
                Config::from_lookup(lookup(&pairs)).unwrap().intro.on,
                on,
                "{v}"
            );
        }
        for bad in ["ON", "true", "1", "o n"] {
            let mut pairs = full();
            pairs.push(("INTRO", bad));
            assert_eq!(
                Config::from_lookup(lookup(&pairs)).unwrap_err(),
                ConfigError::Invalid {
                    var: "INTRO",
                    reason: "expected `on` or `off`".to_string(),
                },
                "{bad}"
            );
        }
        let mut pairs = full();
        pairs.push(("INTRO_MODEL", "claude-opus-5-5"));
        assert_eq!(
            Config::from_lookup(lookup(&pairs)).unwrap().intro.model,
            "claude-opus-5-5"
        );
        let mut pairs = full();
        pairs.push(("INTRO_MODEL", "two words"));
        assert!(matches!(
            Config::from_lookup(lookup(&pairs)).unwrap_err(),
            ConfigError::Invalid {
                var: "INTRO_MODEL",
                ..
            }
        ));
        // Prod: priced when on; with it off an unpriced model stops nothing.
        let prod = |intro: &'static str, model: &'static str| {
            let mut v: Vec<_> = full()
                .into_iter()
                .filter(|(k, _)| *k != "APP_ENV")
                .collect();
            v.extend([
                ("APP_ENV", "prod"),
                ("IAP_AUDIENCE", "/projects/1/apps/2"),
                ("MODEL_PRICES", PROD_PRICES),
                ("USD_INR", "88"),
                ("GLOBAL_MONTHLY_CAP_INR", "10000"),
                ("UNPAYWALL_EMAIL", "a@example.com"),
                ("INTRO", intro),
                ("INTRO_MODEL", model),
            ]);
            Config::from_lookup(lookup(&v))
        };
        assert!(prod("on", "claude-sonnet-5-5").unwrap().intro.on);
        assert!(!prod("off", "unpriced").unwrap().intro.on);
        match prod("on", "unpriced").unwrap_err() {
            ConfigError::Invalid { var, reason } => {
                assert_eq!(var, "MODEL_PRICES");
                assert!(reason.contains("INTRO_MODEL"));
            }
            e => panic!("{e:?}"),
        }
    }

    /// `GRAPH` is `off` (default) or `on`; on needs `GRAPH_FROM` (a date);
    /// `GRAPH_MODEL` has no spaces and is priced in prod when on.
    #[test]
    fn graph_switch() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert!(!c.graph.on, "off by default");
        assert_eq!(c.graph.model, "claude-sonnet-5-5");
        assert_eq!(c.graph.from, None);
        let with = |extra: &[(&'static str, &'static str)]| {
            let mut p = full();
            p.extend_from_slice(extra);
            Config::from_lookup(lookup(&p))
        };
        assert!(!with(&[("GRAPH", "off")]).unwrap().graph.on);
        // Same trimming as INTRO: blanks around the value are ignored, and a
        // blank value is unset (off).
        assert!(!with(&[("GRAPH", "")]).unwrap().graph.on);
        assert!(!with(&[("GRAPH", "  \n")]).unwrap().graph.on);
        assert!(!with(&[("GRAPH", " off ")]).unwrap().graph.on);
        assert!(
            with(&[("GRAPH", " on "), ("GRAPH_FROM", " 2026-11-01\n")])
                .unwrap()
                .graph
                .on
        );
        assert!(with(&[("GRAPH", "on"), ("GRAPH_FROM", " ")]).is_err());
        let c = with(&[("GRAPH", "on"), ("GRAPH_FROM", "2026-11-01")]).unwrap();
        assert!(c.graph.on);
        assert_eq!(c.graph.from, chrono::NaiveDate::from_ymd_opt(2026, 11, 1));
        for bad in ["ON", "maybe", "true", "o n"] {
            assert_eq!(
                with(&[("GRAPH", bad)]).unwrap_err(),
                ConfigError::Invalid {
                    var: "GRAPH",
                    reason: "expected `on` or `off`".to_string(),
                },
                "{bad}"
            );
        }
        assert!(matches!(
            with(&[("GRAPH", "on")]).unwrap_err(),
            ConfigError::Invalid {
                var: "GRAPH_FROM",
                ..
            }
        ));
        for bad in [
            "yesterday",
            "2026-13-01",
            "01/11/2026",
            "2026-1-5",
            "2026-11-1",
            "+2026-11-01",
            "2026-11-01x",
        ] {
            assert!(
                matches!(
                    with(&[("GRAPH_FROM", bad)]).unwrap_err(),
                    ConfigError::Invalid {
                        var: "GRAPH_FROM",
                        ..
                    }
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            with(&[("GRAPH_MODEL", "two words")]).unwrap_err(),
            ConfigError::Invalid {
                var: "GRAPH_MODEL",
                ..
            }
        ));
        let prod = |model: &'static str| {
            let mut v: Vec<_> = full()
                .into_iter()
                .filter(|(k, _)| *k != "APP_ENV")
                .collect();
            v.extend([
                ("APP_ENV", "prod"),
                ("IAP_AUDIENCE", "/projects/1/apps/2"),
                ("MODEL_PRICES", PROD_PRICES),
                ("USD_INR", "88"),
                ("GLOBAL_MONTHLY_CAP_INR", "10000"),
                ("UNPAYWALL_EMAIL", "a@example.com"),
                ("GRAPH", "on"),
                ("GRAPH_FROM", "2026-11-01"),
                ("GRAPH_MODEL", model),
            ]);
            Config::from_lookup(lookup(&v))
        };
        assert!(prod("claude-sonnet-5-5").unwrap().graph.on);
        match prod("unpriced").unwrap_err() {
            ConfigError::Invalid { var, reason } => {
                assert_eq!(var, "MODEL_PRICES");
                assert!(reason.contains("GRAPH_MODEL"));
            }
            e => panic!("{e:?}"),
        }
    }

    /// Temporal T2: `TEMPORAL` is `off` (default) or `on`, trimmed;
    /// `TEMPORAL_MAX_TOKENS` is optional within its range; anything else is
    /// refused at startup without echoing the value. The model is Opus by
    /// default and switchable by `AGENT_MODELS`.
    #[test]
    fn temporal_switch() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert!(!c.temporal.on, "off by default");
        assert_eq!(c.temporal.max_tokens, 20_000);
        assert_eq!(c.agent_models.temporal, "claude-opus-5-5");
        for (v, want) in [("on", true), (" on ", true), ("off", false), ("  ", false)] {
            let mut pairs = full();
            pairs.push(("TEMPORAL", v));
            assert_eq!(
                Config::from_lookup(lookup(&pairs)).unwrap().temporal.on,
                want,
                "{v:?}"
            );
        }
        for bad in ["On", "true", "1", "o n"] {
            let mut pairs = full();
            pairs.push(("TEMPORAL", bad));
            let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        var: "TEMPORAL",
                        ..
                    }
                ),
                "{bad}"
            );
            assert!(!err.to_string().contains(bad));
        }
        let mut pairs = full();
        pairs.push(("TEMPORAL_MAX_TOKENS", "12000"));
        assert_eq!(
            Config::from_lookup(lookup(&pairs))
                .unwrap()
                .temporal
                .max_tokens,
            12_000
        );
        for bad in ["1999", "32001", "9k", "-1"] {
            let mut pairs = full();
            pairs.push(("TEMPORAL_MAX_TOKENS", bad));
            let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        var: "TEMPORAL_MAX_TOKENS",
                        ..
                    }
                ),
                "{bad}"
            );
        }
        let mut pairs = full();
        pairs.push(("AGENT_MODELS", r#"{"temporal": "claude-sonnet-5-5"}"#));
        assert_eq!(
            Config::from_lookup(lookup(&pairs))
                .unwrap()
                .agent_models
                .temporal,
            "claude-sonnet-5-5"
        );
    }

    /// Stage 4b: `COMPLETENESS` is `on` (default) or `off`, trimmed; anything
    /// else is refused at startup without echoing the value.
    #[test]
    fn completeness_switch() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert!(c.completeness, "on by default");
        assert_eq!(c.agent_models.completeness, "claude-sonnet-5-5");
        for (v, want) in [("on", true), ("off", false), (" off ", false), ("  ", true)] {
            let mut pairs = full();
            pairs.push(("COMPLETENESS", v));
            let c = Config::from_lookup(lookup(&pairs)).unwrap();
            assert_eq!(c.completeness, want, "{v:?}");
        }
        for bad in ["On", "OFF", "true", "0", "of f"] {
            let mut pairs = full();
            pairs.push(("COMPLETENESS", bad));
            let err = Config::from_lookup(lookup(&pairs)).unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::Invalid {
                        var: "COMPLETENESS",
                        ..
                    }
                ),
                "{bad}: {err:?}"
            );
            assert!(!err.to_string().contains(bad));
        }
        // The model switch: Opus by AGENT_MODELS.
        let mut pairs = full();
        pairs.push(("AGENT_MODELS", r#"{"completeness": "claude-opus-5-5"}"#));
        let c = Config::from_lookup(lookup(&pairs)).unwrap();
        assert_eq!(c.agent_models.completeness, "claude-opus-5-5");
    }

    /// The CI container smoke's exact prod environment (`.github/workflows/
    /// ci.yml`) parses: every default role priced, the engines defaulted.
    /// Stage 2's merge was once blocked by a CI env missing a price.
    #[test]
    fn the_ci_smoke_env_parses_in_prod() {
        let env = [
            ("APP_ENV", "prod"),
            ("ANTHROPIC_API_KEY", "sk-ant-FAKE-smoke-test-not-a-real-key"),
            ("CLAUDE_MODEL", "claude-sonnet-5-5"),
            ("GATE_MODEL", "claude-haiku-5-5"),
            ("OWNER_EMAIL", "owner@example.com"),
            ("UNPAYWALL_EMAIL", "ci@example.invalid"),
            (
                "MODEL_PRICES",
                r#"{"claude-haiku-5-5":{"input":0.1,"output":0.5},"claude-sonnet-5-5":{"input":2,"output":10},"claude-opus-5-5":{"input":4,"output":20}}"#,
            ),
            ("USD_INR", "88"),
            ("GLOBAL_MONTHLY_CAP_INR", "10000"),
            (
                "IAP_AUDIENCE",
                "/projects/0/locations/asia-south1/services/test",
            ),
            ("DATABASE_URL", "postgres://u:p@127.0.0.1:1/x"),
        ];
        let ci = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../.github/workflows/ci.yml"
        ))
        .unwrap();
        for (k, v) in &env {
            if *k != "DATABASE_URL" {
                assert!(
                    ci.contains(v),
                    "ci.yml no longer sets {k} as this test does"
                );
            }
        }
        let c = Config::from_lookup(lookup(&env)).unwrap();
        assert_eq!(c.full_engine, crate::runs::Engine::Team);
        // §X: the cache rates default from the input price.
        let opus = c.model_prices.get("claude-opus-5-5").unwrap();
        assert_eq!(opus.cache_write_usd_per_mtok, 5.0);
        // Per-model overrides parse in prod too.
        let mut prod = prod_with(
            r#"{"claude-haiku-5-5": {"input": 1, "output": 5}, "claude-sonnet-5-5": {"input": 3, "output": 15, "cache_write": 3.75, "cache_read": 0.3}, "claude-opus-5-5": {"input": 4, "output": 20}, "claude-test": {"input": 3, "output": 15}}"#,
        );
        prod.push(("UNPAYWALL_EMAIL", "ops@example.org"));
        let c = Config::from_lookup(lookup(&prod)).unwrap();
        assert_eq!(
            c.model_prices
                .get("claude-sonnet-5-5")
                .unwrap()
                .cache_read_usd_per_mtok,
            0.3
        );
    }

    #[test]
    fn debug_redacts_secrets() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("sk-test-123"));
        assert!(!dbg.contains("hunter2pw"));
    }
}
