//! Environment-driven configuration.

use secrecy::SecretString;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEnv {
    Dev,
    Prod,
}

#[derive(Debug)]
#[allow(dead_code)] // anthropic_api_key and claude_model are consumed by the claude-client wiring in a later task
pub struct Config {
    pub anthropic_api_key: SecretString,
    pub claude_model: String,
    pub database_url: SecretString,
    pub app_env: AppEnv,
    pub port: u16,
}

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

        let anthropic_api_key = SecretString::from(required("ANTHROPIC_API_KEY")?);
        let claude_model = required("CLAUDE_MODEL")?;
        let database_url = SecretString::from(required("DATABASE_URL")?);

        let app_env = match lookup("APP_ENV").as_deref() {
            None | Some("dev") => AppEnv::Dev,
            Some("prod") => AppEnv::Prod,
            Some(other) => {
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

        Ok(Config {
            anthropic_api_key,
            claude_model,
            database_url,
            app_env,
            port,
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

    fn full() -> Vec<(&'static str, &'static str)> {
        vec![
            ("ANTHROPIC_API_KEY", "sk-test-123"),
            ("CLAUDE_MODEL", "claude-test"),
            ("DATABASE_URL", DB),
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
    fn port_defaults_to_8080_and_env_to_dev() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.app_env, AppEnv::Dev);
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
    fn debug_redacts_secrets() {
        let c = Config::from_lookup(lookup(&full())).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("sk-test-123"));
        assert!(!dbg.contains("hunter2pw"));
    }
}
