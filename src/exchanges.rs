use crate::prometheus::update_rate;
use anyhow::{anyhow, Result};
use serde_json::json;
use std::future::Future;

#[derive(Copy, Clone)]
pub enum Exchange {
    Bitfinex,
    Local,
}

const MAX_RETRIES: u64 = 5;

async fn request_with_backoff<Fut, T, Input>(
    input: Input,
    request_fn: impl Fn(Input) -> Fut,
    initial_delay: u64,
    max_retries: u64,
) -> Option<T>
where
    Input: Clone,
    Fut: Future<Output = Option<T>>, {
    let mut timeout = initial_delay;
    let mut retries = max_retries;
    loop {
        if let Some(i) = request_fn(input.clone()).await {
            return Some(i);
        }

        log::warn!("Request not succesful. Waiting for {} seconds until trying again", timeout);

        if retries == 0 {
            return None;
        }
        retries -= 1;
        tokio::time::sleep(tokio::time::Duration::from_secs(timeout)).await;
        timeout *= 2;
    }
}

const INITIAL_RETRY_INTERVAL: u64 = 10; // seconds
const BITFINEX_URL: &str = "https://api-pub.bitfinex.com/v2/calc/fx";

async fn request_exchange_rate_bitfinex(client: reqwest::Client) -> Option<f64> {
    // TODO: replace ADA with CCD
    let params = json!({"ccy1": "EUR", "ccy2": "ADA"});

    let resp = match client.post(BITFINEX_URL).json(&params).send().await {
        Ok(o) => o,
        Err(e) => {
            log::warn!("Unable to retrieve from bitfinex: {:#?}.", e);
            return None;
        }
    };

    if resp.status().is_success() {
        // Bitfinex api specifies that a successful status means the response is a json
        // array with a single float number.
        match resp.json::<Vec<f64>>().await {
            Ok(v) => {
                let raw_rate = v[0];
                log::debug!("Raw exchange rate CCD/EUR polled from bitfinex: {:#?}", raw_rate);
                return Some(raw_rate);
            }
            Err(_) => {
                log::error!("Unable to parse response from bitfinex as JSON (Breaking API)")
            }
        };
    } else {
        log::error!("Error response from bitfinex: {:?}", resp.status());
    };
    None
}

const LOCAL_URL: &str = "http://127.0.0.1:8111/rate";

async fn get_local_exchange_rate(client: reqwest::Client) -> Option<f64> {
    let resp = match client.get(LOCAL_URL).send().await {
        Ok(o) => o,
        Err(e) => {
            log::warn!("Unable to retrieve from local: {:#?}", e);
            return None;
        }
    };
    if resp.status().is_success() {
        match resp.json::<Vec<f64>>().await {
            Ok(v) => {
                let raw_rate = v[0];
                log::debug!("Raw exchange rate CCD/EUR polled from local: {:#?}", raw_rate);
                return Some(raw_rate);
            }
            Err(_) => {
                log::error!("Unable to parse response from local as JSON")
            }
        };
    } else {
        log::error!("Error response from local: {:?}", resp.status());
    };
    None
}

/**
 * Get the new MicroCCD/Euro exchange rate
 */
pub async fn pull_exchange_rate(exchange: Exchange) -> Result<f64> {
    let client = reqwest::Client::new();
    let ccd_rate_opt = match exchange {
        Exchange::Bitfinex => {
            request_with_backoff(
                client,
                request_exchange_rate_bitfinex,
                INITIAL_RETRY_INTERVAL,
                MAX_RETRIES,
            )
            .await
        }
        Exchange::Local => {
            request_with_backoff(
                client,
                get_local_exchange_rate,
                INITIAL_RETRY_INTERVAL,
                MAX_RETRIES,
            )
            .await
        }
    };

    let ccd_rate = match ccd_rate_opt {
        Some(i) => i,
        None => return Err(anyhow!("Max retries exceeded, unable to pull exchange rate")),
    };
    update_rate(ccd_rate);
    // We multiply with 1/1000000 MicroCCD/CCD
    let micro_per_ccd = 1000000f64;
    let micro_ccd_rate = ccd_rate / micro_per_ccd;
    Ok(micro_ccd_rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[tokio::test]
    async fn test_ping_bitfinex() {
        let client = reqwest::Client::new();
        assert!(request_exchange_rate_bitfinex(client).await.is_some())
    }

    #[tokio::test]
    async fn test_backoff_lower_bound() {
        let dummy_req = |_c: Option<()>| futures::future::ready::<Option<()>>(None);

        let start = Instant::now();
        request_with_backoff(None, dummy_req, 10, 1).await;
        let duration = start.elapsed();
        assert!(duration <= std::time::Duration::from_secs(30)); // 10 + 20
    }

    #[tokio::test]
    async fn test_backoff_upper_bound() {
        let dummy_req = |_c: Option<()>| futures::future::ready::<Option<()>>(None);

        let start = Instant::now();
        request_with_backoff(None, dummy_req, 10, 2).await;
        let duration = start.elapsed();
        assert!(duration >= std::time::Duration::from_secs(30)); // 10 + 20
    }
}
