use crate::{
    config::{
        BITFINEX_URL, COINGECKO_URL, COINMARKETCAP_URL, INITIAL_RETRY_INTERVAL, LIVECOINWATCH_URL,
        MAX_RETRIES,
    },
    prometheus,
};
use anyhow::anyhow;
use num_rational::BigRational;
use reqwest::Url;
use serde::Deserialize as SerdeDeserialize;
use serde_json::json;
use std::{
    collections::VecDeque,
    fmt,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::time::{interval, sleep, Duration};

pub struct RateHistory {
    pub rates:                  VecDeque<BigRational>,
    pub last_reading_timestamp: i64,
}

#[derive(Clone)]
pub enum Source {
    Bitfinex,
    /// Only used for testing, assumes the url accepts a GET request, and serves
    /// a json response, consisting of a list with a number value. i.e. [1.0]
    /// The label is used to differentiate between different test sources in
    /// logs and database entries.
    Test {
        url:   Url,
        label: String,
    },
    CoinGecko,
    LiveCoinWatch(String), // param is api key
    CoinMarketCap(String), // param is api key
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Source::Bitfinex => write!(f, "bitfinex"),
            Source::LiveCoinWatch(_) => write!(f, "live_coin_watch"),
            Source::CoinMarketCap(_) => write!(f, "coin_market_cap"),
            Source::CoinGecko => write!(f, "coin_gecko"),
            Source::Test {
                label,
                ..
            } => write!(f, "{}", label),
        }
    }
}

trait RequestExchangeRate: fmt::Display {
    /**
     * Pulls the exchange rate using the provided client from the given
     * source.
     */
    fn get_request(&self, client: reqwest::Client) -> reqwest::RequestBuilder;
    /**
     * Takes the raw response, and extracts the exchange rate
     */
    fn parse_response(&self, response_bytes: &[u8]) -> anyhow::Result<f64>;
}

impl RequestExchangeRate for Source {
    fn get_request(&self, client: reqwest::Client) -> reqwest::RequestBuilder {
        match self {
            Source::Bitfinex => {
                client.post(BITFINEX_URL).json(&json!({"ccy1": "CCD", "ccy2": "EUR"}))
            }
            Source::LiveCoinWatch(api_key) => client
                .post(LIVECOINWATCH_URL)
                .json(&json!({"currency":"EUR","code":"CCD","meta":false}))
                .header("x-api-key", api_key),
            Source::CoinMarketCap(api_key) => {
                client.get(COINMARKETCAP_URL).header("X-CMC_PRO_API_KEY", api_key)
            }
            Source::CoinGecko => client.get(COINGECKO_URL),
            Source::Test {
                url,
                ..
            } => client.get(url.clone()),
        }
    }

    fn parse_response(&self, response_bytes: &[u8]) -> anyhow::Result<f64> {
        match self {
            Source::Bitfinex
            | Source::Test {
                ..
            } => serde_json::from_slice::<Vec<f64>>(response_bytes)?
                .first()
                .copied()
                .ok_or_else(|| anyhow!("Unexpected missing value")),
            Source::LiveCoinWatch(_) => {
                Ok(serde_json::from_slice::<LiveCoinWatchResponse>(response_bytes)?.rate)
            }
            Source::CoinMarketCap(_) => {
                let response = serde_json::from_slice::<CoinMarketCapResponse>(response_bytes)?;
                if response.status.error_code != 0 {
                    return Err(anyhow!(response.status.error_message.unwrap_or(format!(
                        "No error message, but the code was {}",
                        response.status.error_code
                    ))));
                }
                Ok(response.data.ccd.quote.eur.price)
            }
            Source::CoinGecko => {
                Ok(serde_json::from_slice::<CoinGeckoResponse>(response_bytes)?.concordium.eur)
            }
        }
    }
}

/**
 * Wrapper for a request function, for continous attempts, with exponential
 * backoff.
 * on_fail is invoked after every failed attempt of the request, but only if
 * there are any retries left.
 */
async fn request_with_backoff<'a, Fut: 'a, T>(
    request_fn: impl Fn() -> Fut,
    on_fail: impl Fn(u64),
    initial_delay: u64,
    max_retries: u64,
) -> Option<T>
where
    Fut: Future<Output = Option<T>>, {
    let mut timeout = initial_delay;
    let mut retries = max_retries;
    loop {
        if let Some(i) = request_fn().await {
            return Some(i);
        }

        if retries == 0 {
            return None;
        }

        on_fail(timeout);

        retries -= 1;
        sleep(Duration::from_secs(timeout)).await;
        timeout *= 2;
    }
}

/**
 * Auxillary function for requesting exchange rate.
 * Handles common behaviour among functions for requesting exchange rate.
 * The parser should handle converting the JSON response body into an
 * exchange rate, and its parameter specifies the expected JSON format.
 */
async fn request_exchange_rate(source: &Source, client: reqwest::Client) -> Option<f64> {
    let resp = match source.get_request(client).send().await {
        Ok(o) => o,
        Err(e) => {
            log::warn!("{}: Unable to send request: {}", source, e);
            return None;
        }
    };
    if resp.status().is_success() {
        match resp.bytes().await {
            Ok(bytes) => match source.parse_response(&bytes) {
                Ok(val) => {
                    if val < 0.0 {
                        log::error!("{}: Exchange rate is negative: {}", source, val);
                        return None;
                    }
                    log::debug!("{}: Raw exchange rate CCD in EUR polled: {}", source, val);
                    return Some(val);
                }
                Err(err) => {
                    log::error!("{}: Unable to parse response: {}", source, err)
                }
            },
            Err(err) => {
                log::error!("{}: Unable to read response bytes: {}", source, err)
            }
        }
    } else {
        log::error!("{}: unsuccessful response: {}", source, resp.status());
    };
    None
}

/**
 * Function that continously pulls the exchange rate, from the source
 * specified, and updates the given rates_history_mutex. Ensures that old
 * rates are discarded, when the queue exceeds max size.
 */
pub async fn pull_exchange_rate(
    stats: prometheus::Stats,
    source: Source,
    rate_history_mutex: Arc<Mutex<RateHistory>>,
    pull_interval: u32,
    max_rates_saved: usize,
    db_conn_pool: Option<mysql::Pool>,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();

    let mut interval = interval(Duration::from_secs(pull_interval.into()));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;
        log::debug!("{}: Polling for exchange rate", source);

        let raw_rate = match request_with_backoff(
            || request_exchange_rate(&source, client.clone()),
            |timeout: u64| {
                log::warn!(
                    "{}: Request not successful. Waiting for {} seconds until trying again",
                    source,
                    timeout
                );
                stats.increment_read_attempts(&source);
            },
            INITIAL_RETRY_INTERVAL,
            MAX_RETRIES,
        )
        .await
        {
            Some(i) => i,
            None => {
                log::error!("{}: Request failed. Retries exhausted", source);
                stats.increment_read_attempts(&source);
                continue;
            }
        };
        stats.reset_read_attempts(&source);

        if let Some(ref pool) = db_conn_pool {
            if let Err(e) = crate::database::write_read_rate(pool, raw_rate, &source) {
                stats.increment_failed_database_updates();
                log::error!("{}: Unable to INSERT new reading: {}, due to: {}", source, raw_rate, e)
            };
        }
        stats.update_read_rate(raw_rate, &source);

        let rate = match BigRational::from_float(raw_rate) {
            Some(r) => r.recip(), // Get the inverse value, to change units from EUR/CCD to CCD/EUR
            None => {
                log::error!("{}: Unable to convert rate to rational: {}", source, raw_rate);
                continue;
            }
        };
        log::info!("{}: New exchange rate polled: {}/{}", source, rate.numer(), rate.denom());
        {
            let mut rate_history = rate_history_mutex.lock().unwrap();
            rate_history.rates.push_back(rate);
            if rate_history.rates.len() > max_rates_saved {
                rate_history.rates.pop_front();
            }
            rate_history.last_reading_timestamp = chrono::offset::Utc::now().timestamp();
        } // drop lock
    }
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponsePrice {
    // Note: This object contains other fields like volume and change
    price: f64,
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponseEur {
    #[serde(rename = "EUR")]
    eur: CoinMarketCapResponsePrice,
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponseInfo {
    // Note: This object contains other fields with information about CCD
    quote: CoinMarketCapResponseEur,
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponseData {
    // 18031 is the id of CCD token on Coin market cap
    #[serde(rename = "18031")]
    ccd: CoinMarketCapResponseInfo,
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponseStatus {
    // This object also contains timestamp, elapsed and credit_count.
    error_code:    u16,
    error_message: Option<String>,
}

#[derive(SerdeDeserialize)]
pub struct CoinMarketCapResponse {
    data:   CoinMarketCapResponseData,
    status: CoinMarketCapResponseStatus,
}

#[derive(SerdeDeserialize)]
struct CoinGeckoResponseInner {
    eur: f64,
}
#[derive(SerdeDeserialize)]
pub struct CoinGeckoResponse {
    concordium: CoinGeckoResponseInner,
}

#[derive(SerdeDeserialize)]
pub struct LiveCoinWatchResponse {
    rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::Instant;

    #[tokio::test]
    #[ignore]
    async fn test_ping_coingecko() {
        let client = reqwest::Client::new();
        assert!(request_exchange_rate(&Source::CoinGecko, client).await.is_some())
    }

    #[tokio::test]
    #[ignore]
    async fn test_ping_bitfinex() {
        let client = reqwest::Client::new();
        assert!(request_exchange_rate(&Source::Bitfinex, client).await.is_some())
    }

    #[tokio::test]
    #[ignore]
    async fn test_ping_livecoinwatch() {
        let client = reqwest::Client::new();
        // TODO Load api_key from parameter
        let api_key = "INSERT KEY".to_string();
        let result = request_exchange_rate(&Source::LiveCoinWatch(api_key), client).await;
        println!("{:?}", result);
        assert!(result.is_some())
    }

    #[tokio::test]
    #[ignore]
    async fn test_ping_coinmarketcap() {
        let client = reqwest::Client::new();
        // TODO Load api_key from parameter
        let api_key = "INSERT KEY".to_string();
        let result = request_exchange_rate(&Source::CoinMarketCap(api_key), client).await;
        println!("{:?}", result);
        assert!(result.is_some())
    }

    #[tokio::test]
    async fn test_backoff_lower_bound() {
        let dummy_req = || futures::future::ready::<Option<()>>(None);

        let start = Instant::now();
        request_with_backoff(dummy_req, |_| {}, 10, 1).await;
        let duration = start.elapsed();
        assert!(duration <= std::time::Duration::from_secs(30)); // 10 + 20
    }

    #[tokio::test]
    async fn test_backoff_upper_bound() {
        let dummy_req = || futures::future::ready::<Option<()>>(None);

        let start = Instant::now();
        request_with_backoff(dummy_req, |_| {}, 10, 2).await;
        let duration = start.elapsed();
        assert!(duration >= std::time::Duration::from_secs(30)); // 10 + 20
    }
}
