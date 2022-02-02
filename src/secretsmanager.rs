use crate::config::AWS_REGION;
use anyhow::{anyhow, Context, Result};
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_secretsmanager::{Client, Region};
use crypto_common::types::KeyPair;
use std::path::PathBuf;

pub async fn get_governance_from_aws(secret_names: Vec<String>) -> Result<Vec<KeyPair>> {
    let region_provider =
        RegionProviderChain::first_try(Region::new(AWS_REGION)).or_default_provider();
    let shared_config = aws_config::from_env().region(region_provider).load().await;

    let client = Client::new(&shared_config);

    let mut kps: Vec<KeyPair> = Vec::new();
    for secret in secret_names {
        let resp = client.get_secret_value().secret_id(secret).send().await?;
        let raw_secret = match resp.secret_string() {
            Some(s) => s,
            None => return Err(anyhow!("Secret string was not present")),
        };
        match serde_json::from_str::<Vec<KeyPair>>(raw_secret).context("Could not read keys from secret.") {
            Ok(mut kp) => kps.append(&mut kp),
            Err(e) => return Err(e),
        };
    }
    Ok(kps)
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
