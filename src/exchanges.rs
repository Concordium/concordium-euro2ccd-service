use crate::helpers::convert_f64_to_exchange_rate;
use crate::prometheus::push_rate;
use anyhow::Result;
use concordium_rust_sdk::types::ExchangeRate;
use serde_json::json;
use std::future::Future;

#[derive(Copy, Clone)]
pub enum Exchange {
    Bitfinex,
    Local,
}

// TODO: stop the backoff after some number of tries (when multiple sources are
// added)
async fn request_with_backoff<Fut, T>(
    client: reqwest::Client,
    request_fn: impl Fn(reqwest::Client) -> Fut,
    initial_delay: u64,
) -> T
where
    Fut: Future<Output = Option<T>>, {
    let mut timeout = initial_delay;
    loop {
        if let Some(i) = request_fn(client.clone()).await {
            return i;
        }

        log::warn!("Request not succesful. Waiting for {} seconds until trying again", timeout);
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
        // Bitfinex api speficies that a succesful status means the response is a json
        // array with a single float number.
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
pub async fn pull_exchange_rate(exchange: Exchange) -> Result<ExchangeRate> {
    let client = reqwest::Client::new();
    let ccd_rate = match exchange {
        Exchange::Bitfinex => {
            request_with_backoff(client, request_exchange_rate_bitfinex, INITIAL_RETRY_INTERVAL)
                .await
        }
        Exchange::Local => {
            request_with_backoff(client, get_local_exchange_rate, INITIAL_RETRY_INTERVAL).await
        }
    };
    push_rate(ccd_rate);
    // We multiply with 1/1000000 MicroCCD/CCD
    let micro_per_ccd = 1000000f64;
    let micro_ccd_rate = ccd_rate / micro_per_ccd;
    convert_f64_to_exchange_rate(micro_ccd_rate)
}
