use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::inventory::Environment;
use crate::sensitive::Sensitive;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub scaleway: ScalewayLocations,
    pub bastion: BastionDefaults,
    pub ssh: SshIdentity,
    pub tags: TagConventions,
    pub db: DatabaseRules,
    pub naming: NamingRules,
    pub cache: CachePolicy,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CachePolicy {
    pub ttl_seconds: u64,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self { ttl_seconds: 300 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScalewayLocations {
    pub zones: Vec<String>,
    pub regions: Vec<String>,
}

impl Default for ScalewayLocations {
    fn default() -> Self {
        Self {
            zones: vec![
                "fr-par-1".to_owned(),
                "fr-par-2".to_owned(),
                "fr-par-3".to_owned(),
            ],
            regions: vec!["fr-par".to_owned()],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BastionDefaults {
    pub user: String,
    pub fallback_port: u16,
}

impl Default for BastionDefaults {
    fn default() -> Self {
        Self {
            user: "bastion".to_owned(),
            fallback_port: 61000,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshIdentity {
    pub user: String,
    /// Private key passed to ssh with -i; ssh's own defaults apply when unset.
    pub key: Option<String>,
}

impl Default for SshIdentity {
    fn default() -> Self {
        Self {
            user: "root".to_owned(),
            key: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TagConventions {
    pub port_forward_enabled: String,
    pub port_forward_prefix: String,
    pub env_prefix: String,
    pub mysql: String,
    pub master: String,
}

impl Default for TagConventions {
    fn default() -> Self {
        Self {
            port_forward_enabled: "EnablePortForward:true".to_owned(),
            port_forward_prefix: "PortForward:".to_owned(),
            env_prefix: "Env:".to_owned(),
            mysql: "Mysql".to_owned(),
            master: "Master".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseRules {
    /// Scaleway project holding the database password secrets.
    pub secret_project_id: Option<String>,
    /// Secret name pattern; {db}, {user} and {env} expand uppercased.
    pub secret_name_template: String,
    /// Database user; defaults to the OS user with dots replaced by underscores.
    pub user: Option<String>,
    pub default_env: Environment,
    /// Prefixes stripped from the database key, e.g. "db-" so that
    /// db-matched-article-1 resolves the MATCHED-ARTICLE secret.
    pub strip_prefixes: Vec<String>,
}

impl Default for DatabaseRules {
    fn default() -> Self {
        Self {
            secret_project_id: None,
            secret_name_template: "{db}-{user}-PWD-{env}".to_owned(),
            user: None,
            default_env: Environment::Prod,
            strip_prefixes: vec!["db-".to_owned()],
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NamingRules {
    /// Prefixes stripped from resource names for display and matching.
    pub strip_prefixes: Vec<String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing config {}", path.display()))
    }
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub secret_key: Sensitive,
    pub default_project_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ScwProfile {
    secret_key: Option<String>,
    default_project_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ScwConfigFile {
    #[serde(flatten)]
    base: ScwProfile,
    active_profile: Option<String>,
    #[serde(default)]
    profiles: HashMap<String, ScwProfile>,
}

impl Credentials {
    pub fn load(path: &Path) -> Result<Self> {
        let mut file = ScwConfigFile::default();
        if path.exists() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("reading scaleway config {}", path.display()))?;
            file = serde_yaml::from_str(&raw)
                .with_context(|| format!("parsing scaleway config {}", path.display()))?;
        }
        Self::resolve(file, |name| env::var(name).ok()).with_context(|| {
            format!(
                "no scaleway secret key found: set SCW_SECRET_KEY or configure {}",
                path.display()
            )
        })
    }

    fn resolve(mut file: ScwConfigFile, env: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let profile_name = env("SCW_PROFILE").or(file.active_profile);
        let profile = profile_name
            .as_deref()
            .and_then(|name| file.profiles.remove(name))
            .unwrap_or_default();

        let secret_key = env("SCW_SECRET_KEY")
            .or(profile.secret_key)
            .or(file.base.secret_key);
        let default_project_id = env("SCW_DEFAULT_PROJECT_ID")
            .or(profile.default_project_id)
            .or(file.base.default_project_id);

        let Some(secret_key) = secret_key else {
            bail!("missing secret key");
        };

        Ok(Self {
            secret_key: Sensitive::new(secret_key),
            default_project_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("scwx-test-{name}-{}", std::process::id()));
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn missing_config_yields_defaults() {
        let config = Config::load(Path::new("/nonexistent/scwx.toml")).unwrap();
        assert_eq!(config.bastion.fallback_port, 61000);
        assert_eq!(config.ssh.user, "root");
        assert_eq!(config.scaleway.zones.len(), 3);
        assert!(config.db.secret_project_id.is_none());
    }

    #[test]
    fn partial_config_keeps_defaults_for_missing_sections() {
        let path = write_temp(
            "partial",
            r#"
[db]
secret_project_id = "11111111-2222-3333-4444-555555555555"

[naming]
strip_prefixes = ["platform-ingestor-"]
"#,
        );
        let config = Config::load(&path).unwrap();
        fs::remove_file(&path).unwrap();

        assert_eq!(
            config.db.secret_project_id.as_deref(),
            Some("11111111-2222-3333-4444-555555555555")
        );
        assert_eq!(config.db.secret_name_template, "{db}-{user}-PWD-{env}");
        assert_eq!(config.naming.strip_prefixes, ["platform-ingestor-"]);
        assert_eq!(config.bastion.user, "bastion");
    }

    #[test]
    fn unknown_config_key_is_rejected() {
        let path = write_temp("unknown", "[bastion]\nuserr = \"x\"\n");
        let result = Config::load(&path);
        fs::remove_file(&path).unwrap();
        assert!(result.is_err());
    }

    fn parse_scw(yaml: &str) -> ScwConfigFile {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn credentials_prefer_active_profile_over_base() {
        let file = parse_scw(
            r#"
secret_key: base-key
default_project_id: base-project
active_profile: work
profiles:
  work:
    secret_key: work-key
"#,
        );
        let credentials = Credentials::resolve(file, |_| None).unwrap();

        assert_eq!(credentials.secret_key.expose(), "work-key");
        assert_eq!(
            credentials.default_project_id.as_deref(),
            Some("base-project")
        );
    }

    #[test]
    fn credentials_prefer_env_over_file() {
        let file = parse_scw("secret_key: base-key\n");
        let credentials = Credentials::resolve(file, |name| {
            (name == "SCW_SECRET_KEY").then(|| "env-key".to_owned())
        })
        .unwrap();

        assert_eq!(credentials.secret_key.expose(), "env-key");
    }

    #[test]
    fn env_can_select_the_profile() {
        let file = parse_scw(
            r#"
secret_key: base-key
profiles:
  staging:
    secret_key: staging-key
"#,
        );
        let credentials = Credentials::resolve(file, |name| {
            (name == "SCW_PROFILE").then(|| "staging".to_owned())
        })
        .unwrap();

        assert_eq!(credentials.secret_key.expose(), "staging-key");
    }

    #[test]
    fn missing_secret_key_is_an_error() {
        let result = Credentials::resolve(ScwConfigFile::default(), |_| None);
        assert!(result.is_err());
    }
}
