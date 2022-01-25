use crate::helpers::convert_fraction_to_exchange_rate;
use anyhow::Result;
use concordium_rust_sdk::types::ExchangeRate;
use fraction::Fraction;
use serde_json::json;

#[derive(Copy, Clone)]
pub enum Exchange {
    Bitfinex,
    Local,
}

const RETRY_BITFINEX_INTERVAL: u64 = 10; // seconds
const BITFINEX_URL: &str = "https://api-pub.bitfinex.com/v2/calc/fx";

async fn request_exchange_rate_bitfinex(client: reqwest::Client) -> Fraction {
    // TODO: replace ADA with CCD
    let params = json!({"ccy1": "EUR", "ccy2": "ADA"});

    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(RETRY_BITFINEX_INTERVAL));
    loop {
        interval.tick().await;

        let resp = match client.post(BITFINEX_URL).json(&params).send().await {
            Ok(o) => o,
            Err(e) => {
                log::warn!("Unable to retrieve from bitfinex: {:#?}", e);
                continue;
            }
        };

        if resp.status().is_success() {
            // Bitfinex api speficies that a succesful status means the response is a json
            // array with a single float number.
            match resp.json::<Vec<f64>>().await {
                Ok(v) => {
                    let raw_rate = v[0];
                    log::debug!("Raw exchange rate CCD/EUR polled from bitfinex: {:#?}", raw_rate);
                    return Fraction::from(raw_rate);
                }
                Err(_) => {
                    log::error!("Unable to parse response from bitfinex as JSON (Breaking API)")
                }
            };
        } else {
            log::error!("Error response from bitfinex: {:?}", resp.status());
        };
    }
}

const LOCAL_URL: &str = "http://127.0.0.1:8111/rate";

async fn get_local_exchange_rate(client: reqwest::Client) -> Fraction {
    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(RETRY_BITFINEX_INTERVAL));
    loop {
        interval.tick().await;

        let resp = match client.get(LOCAL_URL).send().await {
            Ok(o) => o,
            Err(e) => {
                log::warn!("Unable to retrieve from local: {:#?}", e);
                continue;
            }
        };
        if resp.status().is_success() {
            // Bitfinex api speficies that a succesful status means the response is a json
            // array with a single float number.
            match resp.json::<Vec<f64>>().await {
                Ok(v) => {
                    let raw_rate = v[0];
                    log::debug!("Raw exchange rate CCD/EUR polled from local: {:#?}", raw_rate);
                    return Fraction::from(raw_rate);
                }
                Err(_) => {
                    log::error!("Unable to parse response from local as JSON")
                }
            };
        } else {
            log::error!("Error response from local: {:?}", resp.status());
        };
    }
}

/**
 * Get the new MicroCCD/Euro exchange rate
 */
pub async fn pull_exchange_rate(exchange: Exchange) -> Result<ExchangeRate> {
    let client = reqwest::Client::new();
    let ccd_rate = match exchange {
        Exchange::Bitfinex => request_exchange_rate_bitfinex(client).await,
        Exchange::Local => get_local_exchange_rate(client).await,
    };
    // We multiply with 1/1000000 MicroCCD/CCD
    let micro_per_ccd = Fraction::new(1u64, 1000000u64);
    let micro_ccd_rate = ccd_rate * micro_per_ccd;
    convert_fraction_to_exchange_rate(micro_ccd_rate)
}
