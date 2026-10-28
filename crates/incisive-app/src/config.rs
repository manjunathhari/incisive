//! Environment-driven configuration.

use secrecy::SecretString;
use std::fmt;

use crate::agent::llm::AgentModels;
use crate::runs::RunsConfig;
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
}

/// DEV ONLY. Placeholder prices (USD per million tokens), not real rates. Prod must
/// set `MODEL_PRICES` explicitly.
const DEV_MODEL_PRICES: &str = r#"{
  "claude-haiku-5-5": {"input": 1.0, "output": 5.0},
  "claude-sonnet-5-5": {"input": 3.0, "output": 15.0}
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
    const PROD_PRICES: &str = r#"{"claude-haiku-5-5": {"input": 1, "output": 5}, "claude-sonnet-5-5": {"input": 3, "output": 15}, "claude-test": {"input": 3, "output": 15}}"#;

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

    #[test]
    fn debug_redacts_secrets() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("sk-test-123"));
        assert!(!dbg.contains("hunter2pw"));
    }
}
