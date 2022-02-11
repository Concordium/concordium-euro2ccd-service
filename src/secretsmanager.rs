use anyhow::{bail, Context};
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_secretsmanager::{Client, Region};
use crypto_common::types::KeyPair;
use std::path::PathBuf;

pub async fn get_governance_from_aws(
    region: String,
    secret_names: Vec<String>,
) -> anyhow::Result<Vec<KeyPair>> {
    log::debug!("Loading keys from AWS secret manager!");
    let region_provider = RegionProviderChain::first_try(Region::new(region)).or_default_provider();
    let shared_config = aws_config::from_env().region(region_provider).load().await;

    let client = Client::new(&shared_config);

    let mut kps: Vec<KeyPair> = Vec::new();
    for secret in secret_names {
        let resp = client.get_secret_value().secret_id(secret).send().await?;
        let raw_secret = match resp.secret_string() {
            Some(s) => s,
            None => bail!("Secret string was not present"),
        };
        let additional_key = serde_json::from_str::<KeyPair>(raw_secret)
            .context("Could not read keys from secret {}.")?;
        kps.push(additional_key);
    }
    Ok(kps)
}

pub fn get_governance_from_file(key_paths: &[PathBuf]) -> anyhow::Result<Vec<KeyPair>> {
    log::warn!("loading keys from file");
    key_paths
        .iter()
        .map(|p| {
            serde_json::from_reader(std::fs::File::open(p).context("Could not open file.")?)
                .context("Could not read keys from file.")
        })
        .collect::<anyhow::Result<_>>()
}
