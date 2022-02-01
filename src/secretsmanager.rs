use crate::config::AWS_REGION;
use anyhow::{anyhow, Context, Result};
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_secretsmanager::{Client, Region};
use crypto_common::types::KeyPair;
use std::path::PathBuf;

pub async fn get_governance_from_aws(secret_name: &str) -> Result<Vec<KeyPair>> {
    let region_provider =
        RegionProviderChain::first_try(Region::new(AWS_REGION)).or_default_provider();
    let shared_config = aws_config::from_env().region(region_provider).load().await;

    let client = Client::new(&shared_config);
    let resp = client.get_secret_value().secret_id(secret_name).send().await?;
    let raw_secret = match resp.secret_string() {
        Some(s) => s,
        None => return Err(anyhow!("Secret string was not present")),
    };
    serde_json::from_str(raw_secret).context("Could not read keys from secret.")
}

pub fn get_governance_from_file(key_paths: Vec<PathBuf>) -> Result<Vec<KeyPair>> {
    log::warn!("loading test keys from file");
    let kps: Vec<KeyPair> = key_paths
        .iter()
        .map(|p| {
            serde_json::from_reader(std::fs::File::open(p).context("Could not open file.")?)
                .context("Could not read keys from file.")
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(kps)
}
