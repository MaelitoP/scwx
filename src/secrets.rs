//! Database passwords from Scaleway Secret Manager.

use anyhow::{Context, Result};
use base64::Engine;
use serde::Deserialize;

use crate::inventory::Environment;
use crate::scw::Client;
use crate::sensitive::Sensitive;

pub(crate) fn secret_name(template: &str, db: &str, user: &str, env: Environment) -> String {
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

pub(crate) fn read_secret(
    client: &Client,
    region: &str,
    project_id: &str,
    name: &str,
) -> Result<Sensitive> {
    let list: SecretList = client
        .fetch_json(
            &format!("/secret-manager/v1beta1/regions/{region}/secrets"),
            &[("project_id", project_id), ("name", name)],
        )
        .with_context(|| format!("listing secrets named {name}"))?;
    let secret_id = &list
        .secrets
        .first()
        .with_context(|| format!("no secret named {name}"))?
        .id;
    let version: SecretVersion = client
        .fetch_json(
            &format!(
                "/secret-manager/v1beta1/regions/{region}/secrets/{secret_id}/versions/latest/access"
            ),
            &[],
        )
        .with_context(|| format!("accessing secret {name}"))?;

    decode_secret(name, &version.data)
}

fn decode_secret(name: &str, data: &str) -> Result<Sensitive> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .with_context(|| format!("decoding secret {name}"))?;
    let value = String::from_utf8(bytes).with_context(|| format!("secret {name} is not utf-8"))?;
    Ok(Sensitive::new(
        value.trim_end_matches(['\n', '\r']).to_owned(),
    ))
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

    #[test]
    fn decode_trims_trailing_newlines_and_rejects_garbage() {
        // "hunter2\n" base64-encoded
        let decoded = decode_secret("S", "aHVudGVyMgo=").unwrap();
        assert_eq!(decoded.expose(), "hunter2");

        assert!(decode_secret("S", "not-base64!!!").is_err());
        // 0xFF 0xFE is valid base64 content but not utf-8
        assert!(decode_secret("S", "//4=").is_err());
    }
}
