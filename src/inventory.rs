use std::fmt;
use std::str::FromStr;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::config::{NamingSection, TagsSection};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Instance,
    Baremetal,
    Rdb,
    Redis,
    Lb,
}

impl ResourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Instance => "instance",
            Self::Baremetal => "baremetal",
            Self::Rdb => "rdb",
            Self::Redis => "redis",
            Self::Lb => "lb",
        }
    }

    /// Kinds reachable with a plain SSH session.
    pub fn is_server(self) -> bool {
        match self {
            Self::Instance | Self::Baremetal => true,
            Self::Rdb | Self::Redis | Self::Lb => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Prod,
    Beta,
    Dev,
}

impl Environment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prod => "prod",
            Self::Beta => "beta",
            Self::Dev => "dev",
        }
    }
}

impl FromStr for Environment {
    type Err = UnknownEnvironment;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "prod" => Ok(Self::Prod),
            "beta" => Ok(Self::Beta),
            "dev" => Ok(Self::Dev),
            _ => Err(UnknownEnvironment),
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub struct UnknownEnvironment;

impl fmt::Display for UnknownEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unknown environment")
    }
}

impl std::error::Error for UnknownEnvironment {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub kind: ResourceKind,
    pub id: String,
    pub name: String,
    pub zone: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Private IP for baremetal/rdb/redis/lb targets; servers are reached by name.
    #[serde(default)]
    pub endpoint_ip: Option<String>,
    /// Service port advertised by the resource's endpoint (rdb/redis).
    #[serde(default)]
    pub endpoint_port: Option<u16>,
}

impl Resource {
    pub fn display_name(&self, naming: &NamingSection) -> String {
        let stripped = naming
            .strip_prefixes
            .iter()
            .find_map(|prefix| self.name.strip_prefix(prefix))
            .unwrap_or(&self.name);
        stripped.to_owned()
    }

    pub fn env(&self, tags: &TagsSection) -> Option<Environment> {
        self.tags
            .iter()
            .find_map(|tag| tag.strip_prefix(&tags.env_prefix))
            .and_then(|value| value.parse().ok())
    }

    pub fn port_forward_enabled(&self, tags: &TagsSection) -> bool {
        self.tags.contains(&tags.port_forward_enabled)
    }

    pub fn port_forward_port(&self, tags: &TagsSection) -> Option<u16> {
        self.tags
            .iter()
            .find_map(|tag| tag.strip_prefix(&tags.port_forward_prefix))
            .and_then(|value| value.parse().ok())
            .or(self.endpoint_port)
    }

    pub fn is_mysql(&self, tags: &TagsSection) -> bool {
        self.tags.contains(&tags.mysql)
    }

    pub fn is_master(&self, tags: &TagsSection) -> bool {
        self.tags.contains(&tags.master)
    }

    pub fn matches(&self, query: &str, naming: &NamingSection) -> bool {
        let query = query.to_ascii_lowercase();
        self.name.to_ascii_lowercase().contains(&query)
            || self
                .display_name(naming)
                .to_ascii_lowercase()
                .contains(&query)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bastion {
    pub ip: String,
    pub port: u16,
    pub zone: String,
}

impl Bastion {
    pub fn destination(&self, user: &str) -> String {
        format!("{user}@{}", self.ip)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inventory {
    pub resources: Vec<Resource>,
    pub bastion: Option<Bastion>,
}

impl Inventory {
    pub fn filtered(&self, env: Option<Environment>, tags: &TagsSection) -> Vec<&Resource> {
        self.resources
            .iter()
            .filter(|resource| env.is_none() || resource.env(tags) == env)
            .collect()
    }

    pub fn bastion(&self) -> anyhow::Result<&Bastion> {
        self.bastion
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no bastion-enabled vpc gateway found in the inventory"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naming(prefixes: &[&str]) -> NamingSection {
        NamingSection {
            strip_prefixes: prefixes.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn resource(name: &str, tags: &[&str]) -> Resource {
        Resource {
            kind: ResourceKind::Baremetal,
            id: "11111111-2222-3333-4444-555555555555".to_owned(),
            name: name.to_owned(),
            zone: "fr-par-2".to_owned(),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            endpoint_ip: None,
            endpoint_port: None,
        }
    }

    #[test]
    fn display_name_strips_the_first_matching_prefix() {
        let resource = resource("platform-ingestor-prod-perco-3", &[]);
        let naming = naming(&["platform-ingestor-", "other-"]);
        assert_eq!(resource.display_name(&naming), "prod-perco-3");
    }

    #[test]
    fn display_name_without_matching_prefix_is_the_full_name() {
        let resource = resource("standalone", &[]);
        assert_eq!(resource.display_name(&naming(&["platform-"])), "standalone");
    }

    #[test]
    fn env_parses_the_env_tag_case_insensitively() {
        let tags = TagsSection::default();
        assert_eq!(
            resource("a", &["Env:Prod"]).env(&tags),
            Some(Environment::Prod)
        );
        assert_eq!(
            resource("a", &["Env:beta"]).env(&tags),
            Some(Environment::Beta)
        );
        assert_eq!(resource("a", &["Env:Unknown"]).env(&tags), None);
        assert_eq!(resource("a", &[]).env(&tags), None);
    }

    #[test]
    fn port_forward_port_prefers_the_tag_over_the_endpoint() {
        let tags = TagsSection::default();
        let mut r = resource("a", &["PortForward:3306"]);
        r.endpoint_port = Some(6379);
        assert_eq!(r.port_forward_port(&tags), Some(3306));

        let mut r = resource("a", &[]);
        r.endpoint_port = Some(6379);
        assert_eq!(r.port_forward_port(&tags), Some(6379));

        assert_eq!(resource("a", &[]).port_forward_port(&tags), None);
    }

    #[test]
    fn invalid_port_forward_tag_falls_back_to_the_endpoint() {
        let tags = TagsSection::default();
        let mut r = resource("a", &["PortForward:not-a-port"]);
        r.endpoint_port = Some(5432);
        assert_eq!(r.port_forward_port(&tags), Some(5432));
    }

    #[test]
    fn matches_is_case_insensitive_on_full_and_display_name() {
        let resource = resource("platform-ingestor-prod-perco-3", &[]);
        let naming = naming(&["platform-ingestor-"]);
        assert!(resource.matches("PERCO", &naming));
        assert!(resource.matches("platform-ingestor-prod", &naming));
        assert!(!resource.matches("redis", &naming));
    }

    #[test]
    fn filtered_keeps_only_the_requested_env() {
        let tags = TagsSection::default();
        let inventory = Inventory {
            resources: vec![
                resource("prod-a", &["Env:Prod"]),
                resource("beta-a", &["Env:Beta"]),
                resource("untagged", &[]),
            ],
            bastion: None,
        };

        let prod = inventory.filtered(Some(Environment::Prod), &tags);
        assert_eq!(prod.len(), 1);
        assert_eq!(prod[0].name, "prod-a");

        assert_eq!(inventory.filtered(None, &tags).len(), 3);
    }
}
