use crate::{
    certificate_resolver::get_client_with_specific_certificate,
    config::{
        BITFINEX_CERTIFICATE_LOCATION, BITFINEX_URL, INITIAL_RETRY_INTERVAL, MAXIMUM_RATES_SAVED,
        MAX_RETRIES,
    },
    helpers::{compute_average, within_allowed_deviation},
    prometheus,
};
use anyhow::Result;
use num_rational::BigRational;
use serde_json::json;
use std::{
    collections::VecDeque,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::time::{interval, sleep, Duration};

#[derive(Clone)]
pub enum Exchange {
    Bitfinex,
    Test(String),
}

/**
 * Wrapper for a request function, to continous attempts, with exponential
 * backoff.
 */
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

/**
 * Request current exchange rate from bitfinex.
 */
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

/**
 * Get exchange rate from test exchange. (Should only be used for testing)
 */
async fn request_exchange_rate_test(client: reqwest::Client, url: String) -> Option<f64> {
    let resp = match client.get(url).send().await {
        Ok(o) => o,
        Err(e) => {
            log::warn!("Unable to retrieve from test exchange: {:#?}", e);
            return None;
        }
    };
    if resp.status().is_success() {
        match resp.json::<Vec<f64>>().await {
            Ok(v) => {
                let raw_rate = v[0];
                log::debug!("Raw exchange rate CCD/EUR polled from test exchange: {:#?}", raw_rate);
                return Some(raw_rate);
            }
            Err(_) => {
                log::error!("Unable to parse response from test exchange as JSON")
            }
        };
    } else {
        log::error!("Error response from test exchange: {:?}", resp.status());
    };
    None
}

/**
 * Function that continously pulls the exchange rate using request_fn, and
 * updates the given rates_mutex. Ensures that new rates doesn't deviate
 * outside allowed range. Ensures that old rates are discarded, when the
 * queue exceeds max size.
 */
async fn exchange_rate_getter<Fut>(
    request_fn: impl Fn(reqwest::Client) -> Fut + Clone,
    rate_history_mutex: Arc<Mutex<VecDeque<BigRational>>>,
    max_deviation: u8,
    pull_interval: u64,
    client: reqwest::Client,
) where
    Fut: Future<Output = Option<f64>>, {
    let mut interval = interval(Duration::from_secs(pull_interval));
    let mut first_time = true;

    loop {
        interval.tick().await;
        log::debug!("Polling for exchange rate");

        let raw_rate = match request_with_backoff(
            client.clone(),
            request_fn.clone(),
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
                continue;
            }
        };
        let mut rates = rate_history_mutex.lock().unwrap();

        // First time rates are empty, so we just add the exchange rate:
        if first_time {
            rates.push_back(rate);
            first_time = false;
            continue;
        }

        let current_average = match compute_average(rates.clone()) {
            Some(r) => r,
            None => {
                log::error!("Unable to compute average to rational");
                continue;
            }
        };
        if within_allowed_deviation(&current_average, &rate, max_deviation) {
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

fn build_client(exchange: Exchange) -> Result<reqwest::Client> {
    match exchange {
        Exchange::Bitfinex => get_client_with_specific_certificate(BITFINEX_CERTIFICATE_LOCATION),
        Exchange::Test(_) => Ok(reqwest::Client::new()),
    }
}

/**
 * Get the new MicroCCD/Euro exchange rate
 */
pub async fn pull_exchange_rate(
    exchange: Exchange,
    rate_history: Arc<Mutex<VecDeque<BigRational>>>,
    max_deviation: u8,
    pull_interval: u64,
) -> Result<()> {
    let client = match build_client(exchange.clone()) {
        Ok(c) => c,
        Err(e) => {
            log::error!("Error while building client: {:#?}", e);
            return Ok(());
        }
    };

    match exchange {
        Exchange::Bitfinex => {
            exchange_rate_getter(
                request_exchange_rate_bitfinex,
                rate_history,
                max_deviation,
                pull_interval,
                client,
            )
            .await
        }
        Exchange::Test(url) => {
            exchange_rate_getter(
                |client| request_exchange_rate_test(client, url.clone()),
                rate_history,
                max_deviation,
                pull_interval,
                client,
            )
            .await
        }
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
