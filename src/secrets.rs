use anyhow::{Context, Result, ensure};
use base64::Engine;
use serde::Deserialize;

use crate::inventory::Environment;
use crate::scw::Client;

pub fn secret_name(template: &str, db: &str, user: &str, env: Environment) -> String {
    template
        .replace("{db}", &db.to_uppercase())
        .replace("{user}", &user.to_uppercase())
        .replace("{env}", &env.as_str().to_uppercase())
}

#[derive(Debug, Deserialize)]
struct SecretList {
    secrets: Vec<Secret>,
}

#[derive(Debug, Deserialize)]
struct Secret {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SecretVersion {
    data: String,
}

pub fn access_secret(
    client: &Client,
    region: &str,
    project_id: &str,
    name: &str,
) -> Result<String> {
    let list: SecretList = client
        .get_json(
            &format!("/secret-manager/v1beta1/regions/{region}/secrets"),
            &[
                ("project_id", project_id.to_owned()),
                ("name", name.to_owned()),
            ],
        )
        .with_context(|| format!("listing secrets named {name}"))?;
    ensure!(!list.secrets.is_empty(), "no secret named {name}");

    let secret_id = &list.secrets[0].id;
    let version: SecretVersion = client
        .get_json(
            &format!(
                "/secret-manager/v1beta1/regions/{region}/secrets/{secret_id}/versions/latest/access"
            ),
            &[],
        )
        .with_context(|| format!("accessing secret {name}"))?;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&version.data)
        .with_context(|| format!("decoding secret {name}"))?;
    let value = String::from_utf8(bytes).with_context(|| format!("secret {name} is not utf-8"))?;
    Ok(value.trim_end_matches(['\n', '\r']).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_name_uppercases_every_placeholder() {
        let name = secret_name(
            "{db}-{user}-PWD-{env}",
            "matched-article",
            "mael_lepetit",
            Environment::Prod,
        );
        assert_eq!(name, "MATCHED-ARTICLE-MAEL_LEPETIT-PWD-PROD");
    }

    #[test]
    fn secret_name_with_a_different_template_shape() {
        let name = secret_name("db/{env}/{db}/{user}", "search", "jo", Environment::Beta);
        assert_eq!(name, "db/BETA/SEARCH/JO");
    }
}
