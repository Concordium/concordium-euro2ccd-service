use crate::helpers::{compute_average, within_allowed_deviation};
use crate::prometheus;
use anyhow::Result;
use serde_json::json;
use std::future::Future;
use tokio::time::{interval, sleep, Duration};
use num_rational::BigRational;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Copy, Clone)]
pub enum Exchange {
    Bitfinex,
    Local,
}

const MAX_RETRIES: u64 = 5;
const PULL_RATE_INTERVAL: u64 = 10; // seconds
const MAX_DEVIATION_FROM_AVERAGE: u16 = 30; // percentage
const INITIAL_RETRY_INTERVAL: u64 = 10; // seconds
const BITFINEX_URL: &str = "https://api-pub.bitfinex.com/v2/calc/fx";
const MAXIMUM_RATES_SAVED: u64 = 30;

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

        log::warn!("Request not successful. Waiting for {} seconds until trying again", timeout);

        if retries == 0 {
            return None;
        }
        retries -= 1;
        sleep(Duration::from_secs(timeout)).await;
        timeout *= 2;
    }
}

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

async fn exchange_rate_getter<Fut>(
    request_fn: impl Fn(reqwest::Client) -> Fut + Copy,
    rates_mutex: Arc<Mutex<VecDeque<BigRational>>>,
) where
    Fut: Future<Output = Option<f64>>, {
    let mut interval = interval(Duration::from_secs(PULL_RATE_INTERVAL));
    let client = reqwest::Client::new();

    loop {
        interval.tick().await;
        log::debug!("Polling for exchange rate");

        let raw_rate = match request_with_backoff(
            client.clone(),
            request_fn,
            INITIAL_RETRY_INTERVAL,
            MAX_RETRIES,
        )
        .await
        {
            Some(i) => i,
            None => continue,
        };

        log::info!("New exchange rate polled: {:#?}", raw_rate);
        prometheus::update_rate(raw_rate);
        let rate = match BigRational::from_float(raw_rate) {
            Some(r) => r,
            None => {
                log::error!("Unable to convert rate to rational: {}", raw_rate);
                continue
            }
        };
        let mut rates = rates_mutex.lock().unwrap();
        let current_average = match compute_average(rates.clone()) {
            Some(r) => r,
            None => {
                log::error!("Unable to compute average to rational");
                continue
            }
        };
        if within_allowed_deviation(&current_average, &rate, MAX_DEVIATION_FROM_AVERAGE) {
            rates.push_back(rate);
            if rates.iter().map(|_| 1).sum::<u64>() > MAXIMUM_RATES_SAVED {
                rates.pop_front();
            }
        } else {
            prometheus::increment_dropped_times();
            log::warn!("Polled rate deviated too much, and has been dropped: {:#?}", rate);
        }
        log::debug!("Currently saved rates: {:#?}", *rates);
        drop(rates);
    }
}

/**
 * Get the new MicroCCD/Euro exchange rate
 */
pub async fn pull_exchange_rate(
    exchange: Exchange,
    rates: Arc<Mutex<VecDeque<BigRational>>>,
) -> Result<()> {
    match exchange {
        Exchange::Bitfinex => exchange_rate_getter(request_exchange_rate_bitfinex, rates).await,
        Exchange::Local => exchange_rate_getter(get_local_exchange_rate, rates).await,
    };
    Ok(())
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
