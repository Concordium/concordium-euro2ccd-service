use crate::{
    config::{BITFINEX_URL, INITIAL_RETRY_INTERVAL, MAX_RETRIES, COINGECKO_URL, COINMARKETCAP_URL, LIVECOINWATCH_URL},
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
use crypto_common::*;

#[derive(Clone)]
pub enum Exchange {
    Bitfinex,
    Test(Url),
    CoinGecko,
    LiveCoinWatch(String), // param is api key
    CoinMarketCap(String), // param is api key
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
    let params = json!({"ccy1": "EUR", "ccy2": "CCD"});
    let request = client.post(BITFINEX_URL).json(&params);
    let parser = |v: Vec<f64>| Some(v[0]);
    request_exchange_rate_core(request, parser, "bitfinex").await
}

#[derive(SerdeDeserialize)]
struct CoinGeckoResponseInner {
    eur: f64
}
#[derive(SerdeDeserialize)]
struct CoinGeckoResponse {
    concordium: CoinGeckoResponseInner
}

async fn request_exchange_rate_coingecko(client: reqwest::Client) -> Option<f64> {
    let parser = |v: CoinGeckoResponse| Some(v.concordium.eur);
    request_exchange_rate_core(client.get(COINGECKO_URL), parser, "Coin Gecko").await
}

#[derive(SerdeDeserialize)]
struct LiveCoinWatchResponse {
    rate: f64
}

async fn request_exchange_rate_livecoinwatch(client: reqwest::Client, api_key: String) -> Option<f64> {
    let params = json!({"currency":"EUR","code":"CCD","meta":false});
    let request = client.post(LIVECOINWATCH_URL).json(&params).header("x-api-key", api_key);
    let parser = |v: LiveCoinWatchResponse| Some(v.rate);
    request_exchange_rate_core(request, parser, "LiveCoinWatch").await
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponsePrice {
    // TODO: add other fields like volume and change
    price: f64
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponseEur {
    #[serde(rename = "EUR")]
    eur: CoinMarketCapResponsePrice
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponseInfo {
    // TODO: add other fields about CCD
    quote: CoinMarketCapResponseEur
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponseData {
    #[serde(rename = "CCD")]
    ccd: Vec<CoinMarketCapResponseInfo>
}

#[derive(SerdeDeserialize)]
struct CoinMarketCapResponse {
    // TODO: add status structure
    data: CoinMarketCapResponseData
}

async fn request_exchange_rate_coinmarketcap(client: reqwest::Client, api_key: String) -> Option<f64> {
    let request = client.get(COINMARKETCAP_URL).header("X-CMC_PRO_API_KEY", api_key);
    let parser = |v: CoinMarketCapResponse| Some(v.data.ccd[0].quote.eur.price);
    request_exchange_rate_core(request, parser, "CoinMarketCap").await
}

async fn request_exchange_rate_core<ResponseFormat: for<'de> crypto_common::SerdeDeserialize<'de>>(request: reqwest::RequestBuilder, parser: impl Fn(ResponseFormat) -> Option<f64>, name: &str) -> Option<f64>  {
    let resp = match request.send().await {
        Ok(o) => o,
        Err(e) => {
            log::warn!("Unable to retrieve from {}: {:?}", name, e);
            return None;
        }
    };
    if resp.status().is_success() {
        match resp.json::<ResponseFormat>().await {
            Ok(v) => {
                match parser(v) {
                    Some(val) => {
                        if val < 0.0 {
                            log::error!("Exchange rate from  {} is negative: {}", name, val);
                            return None;
                        }
                        log::debug!("Raw exchange rate CCD/EUR polled from {}: {:?}", name, val);
                        return Some(val);
                    },
                    None => return None
                }
            }
            Err(err) => {
                log::error!("Unable to parse response from {} as JSON: {}", name, err)
            }
        };
    } else {
        log::error!("Error response from {}: {}", name, resp.status());
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
    mut database_conn: Option<mysql::PooledConn>,
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
        if let Some(ref mut conn) = database_conn {
            if let Err(e) = crate::database::write_read_rate(conn, raw_rate) {
                stats.increment_failed_database_updates();
                log::error!("Unable to INSERT new reading: {}, due to: {}", raw_rate, e)
            };
        }
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
    database_conn: Option<mysql::PooledConn>,
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
                database_conn,
            )
            .await
        }
        Exchange::Test(url) => {
            exchange_rate_getter(
                stats,
                |client| request_exchange_rate_core(client.get(url.clone()), |v: Vec<f64>| Some(v[0]), "Test exchange"),
                rate_history,
                pull_interval,
                max_rates_saved,
                client,
                database_conn,
            )
            .await
        }
        Exchange::CoinGecko => {
            exchange_rate_getter(
                stats,
                request_exchange_rate_coingecko,
                rate_history,
                pull_interval,
                max_rates_saved,
                client,
                database_conn,
            )
                .await
        }
        Exchange::LiveCoinWatch(api_key) => {
            exchange_rate_getter(
                stats,
                |client| request_exchange_rate_livecoinwatch(client, api_key.clone()),
                rate_history,
                pull_interval,
                max_rates_saved,
                client,
                database_conn,
            )
                .await
        }
        Exchange::CoinMarketCap(api_key) => {
            exchange_rate_getter(
                stats,
                |client| request_exchange_rate_coinmarketcap(client, api_key.clone()),
                rate_history,
                pull_interval,
                max_rates_saved,
                client,
                database_conn,
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
    #[ignore]
    async fn test_ping_coingecko() {
        let client = reqwest::Client::new();
        assert!(request_exchange_rate_coingecko(client).await.is_some())
    }

    #[tokio::test]
    #[ignore]
    async fn test_ping_bitfinex() {
        let client = reqwest::Client::new();
        assert!(request_exchange_rate_bitfinex(client).await.is_some())
    }

    #[tokio::test]
    #[ignore]
    async fn test_ping_livecoinwatch() {
        let client = reqwest::Client::new();
        // TODO Load api_key from parameter
        let api_key = "INSERT KEY".to_string();
        let result = request_exchange_rate_livecoinwatch(client, api_key).await;
        println!("{:?}", result);
        assert!(result.is_some())
    }

    #[tokio::test]
    #[ignore]
    async fn test_ping_coinmarketcap() {
        let client = reqwest::Client::new();
        // TODO Load api_key from parameter
        let api_key = "INSERT KEY".to_string();
        let result = request_exchange_rate_coinmarketcap(client, api_key).await;
        println!("{:?}", result);
        assert!(result.is_some())
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
