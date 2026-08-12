use crate::config::Config;
use crate::inventory::{Environment, Resource, ResourceKind};

pub fn is_database(resource: &Resource, config: &Config) -> bool {
    match resource.kind {
        ResourceKind::Rdb => true,
        ResourceKind::Baremetal => resource.is_mysql(&config.tags),
        ResourceKind::Instance | ResourceKind::Redis | ResourceKind::Lb => false,
    }
}

/// Short database key: display name without the env segment, configured db
/// prefixes and shard suffix. `platform-ingestor-prod-search-2` -> `search`.
/// The key names both the mysql schema and the password secret.
pub fn db_key(resource: &Resource, config: &Config, env: Environment) -> String {
    let mut key = resource.display_name(&config.naming).to_owned();
    if let Some(rest) = key.strip_prefix(&format!("{env}-")) {
        key = rest.to_owned();
    }
    if let Some(rest) = config
        .db
        .strip_prefixes
        .iter()
        .find_map(|prefix| key.strip_prefix(prefix))
    {
        key = rest.to_owned();
    }
    if let Some(position) = key.rfind('-')
        && !key[position + 1..].is_empty()
        && key[position + 1..].chars().all(|c| c.is_ascii_digit())
    {
        key.truncate(position);
    }
    key
}

/// Unique database keys for the configured default env, for shell completion.
pub fn database_keys(resources: &[&Resource], config: &Config) -> Vec<String> {
    let env = config.db.default_env;
    let mut keys: Vec<String> = resources
        .iter()
        .filter(|resource| is_database(resource, config))
        .filter(|resource| resource.env(&config.tags) == Some(env))
        .map(|resource| db_key(resource, config, env))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(name: &str, tags: &[&str]) -> Resource {
        Resource {
            kind: ResourceKind::Baremetal,
            id: "id".to_owned(),
            name: name.to_owned(),
            zone: "fr-par-1".to_owned(),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            endpoint_ip: None,
            endpoint_port: None,
        }
    }

    fn config() -> Config {
        let mut config = Config::default();
        config.naming.strip_prefixes = vec!["platform-ingestor-".to_owned()];
        config
    }

    #[test]
    fn db_key_strips_prefix_env_and_shard() {
        let r = resource("platform-ingestor-prod-search-2", &[]);
        assert_eq!(db_key(&r, &config(), Environment::Prod), "search");
    }

    #[test]
    fn db_key_keeps_inner_digits_and_foreign_env() {
        let r = resource("platform-ingestor-db-matched-article-1", &[]);
        assert_eq!(db_key(&r, &config(), Environment::Prod), "matched-article");

        let r = resource("platform-ingestor-beta-saga", &[]);
        assert_eq!(db_key(&r, &config(), Environment::Prod), "beta-saga");
        assert_eq!(db_key(&r, &config(), Environment::Beta), "saga");
    }

    #[test]
    fn only_rdb_and_mysql_tagged_baremetal_are_databases() {
        let config = config();
        let mut rdb = resource("a", &[]);
        rdb.kind = ResourceKind::Rdb;
        assert!(is_database(&rdb, &config));

        assert!(is_database(&resource("b", &["Mysql"]), &config));
        assert!(!is_database(&resource("c", &[]), &config));

        let mut instance = resource("d", &["Mysql"]);
        instance.kind = ResourceKind::Instance;
        assert!(!is_database(&instance, &config));
    }

    #[test]
    fn database_keys_are_unique_and_sorted() {
        let config = config();
        let shard1 = resource(
            "platform-ingestor-db-matched-article-1",
            &["Mysql", "Env:Prod"],
        );
        let shard4 = resource(
            "platform-ingestor-db-matched-article-4",
            &["Mysql", "Env:Prod"],
        );
        let mut search = resource("platform-ingestor-prod-search", &["Env:Prod"]);
        search.kind = ResourceKind::Rdb;
        let beta = resource("platform-ingestor-beta-inbox", &["Mysql", "Env:Beta"]);
        let resources = vec![&shard1, &shard4, &search, &beta];

        assert_eq!(
            database_keys(&resources, &config),
            ["matched-article", "search"]
        );
    }
}
