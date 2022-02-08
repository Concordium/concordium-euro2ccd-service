use crate::{
    config::{BITFINEX_URL, INITIAL_RETRY_INTERVAL, MAX_RETRIES},
    prometheus,
};
use num_rational::BigRational;
use reqwest::Url;
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
    Test(Url),
}

/**
 * Wrapper for a request function, to continous attempts, with exponential
 * backoff.
 */
async fn request_with_backoff<'a, Fut: 'a, T, Input>(
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
            log::warn!("Unable to retrieve from bitfinex: {:?}.", e);
            return None;
        }
    };

    if resp.status().is_success() {
        // Bitfinex api specifies that a successful status means the response is a json
        // array with a single float number.
        match resp.json::<Vec<f64>>().await {
            Ok(v) if v.len() == 1 => {
                let raw_rate = v[0];
                log::debug!("Raw exchange rate CCD/EUR polled from bitfinex: {:?}", raw_rate);
                return Some(raw_rate);
            }
            Ok(arr) => {
                log::error!(
                    "Unexpected response from the exchange. Expected an array of length 1, got \
                     array of length {}.",
                    arr.len()
                )
            }
            Err(err) => {
                log::error!(
                    "Unable to parse response from bitfinex as JSON (Breaking API): {}",
                    err
                )
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
async fn request_exchange_rate_test(client: reqwest::Client, url: Url) -> Option<f64> {
    let resp = match client.get(url.clone()).send().await {
        Ok(o) => o,
        Err(e) => {
            log::warn!("Unable to retrieve from test exchange: {:?}", e);
            return None;
        }
    };
    if resp.status().is_success() {
        match resp.json::<Vec<f64>>().await {
            Ok(v) => {
                let raw_rate = v[0];
                log::debug!("Raw exchange rate CCD/EUR polled from test exchange: {:?}", raw_rate);
                return Some(raw_rate);
            }
            Err(err) => {
                log::error!("Unable to parse response from test exchange as JSON: {}", err)
            }
        };
    } else {
        log::error!("Error response from test exchange: {}", resp.status());
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
    stats: prometheus::Stats,
    request_fn: impl Fn(reqwest::Client) -> Fut + Clone,
    rate_history_mutex: Arc<Mutex<VecDeque<BigRational>>>,
    pull_interval: u32,
    max_rates_saved: usize,
    client: reqwest::Client,
) where
    Fut: Future<Output = Option<f64>> + 'static, {
    let mut interval = interval(Duration::from_secs(pull_interval.into()));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

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

        log::info!("New exchange rate polled: {:?}", raw_rate);
        stats.update_read_rate(raw_rate);

        let rate = match BigRational::from_float(raw_rate) {
            Some(r) => r,
            None => {
                log::error!("Unable to convert rate to rational: {}", raw_rate);
                continue;
            }
        };
        {
            let mut rates = rate_history_mutex.lock().unwrap();
            rates.push_back(rate);
            if rates.len() > max_rates_saved {
                rates.pop_front();
            }
        }; // drop lock
    }
}

/**
 * Get the new MicroCCD/Euro exchange rate
 */
pub async fn pull_exchange_rate(
    stats: prometheus::Stats,
    exchange: Exchange,
    rate_history: Arc<Mutex<VecDeque<BigRational>>>,
    pull_interval: u32,
    max_rates_saved: usize,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    match exchange {
        Exchange::Bitfinex => {
            exchange_rate_getter(
                stats,
                request_exchange_rate_bitfinex,
                rate_history,
                pull_interval,
                max_rates_saved,
                client,
            )
            .await
        }
        Exchange::Test(url) => {
            exchange_rate_getter(
                stats,
                |client| request_exchange_rate_test(client, url.clone()),
                rate_history,
                pull_interval,
                max_rates_saved,
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
