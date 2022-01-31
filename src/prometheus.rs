use anyhow::{Context, Result};
use lazy_static::lazy_static;
use prometheus::{
    register_counter, register_gauge, Counter, Encoder, Gauge, Registry, TextEncoder,
};
use warp::{http::StatusCode, Filter};

lazy_static! {
    static ref EXCHANGE_RATE: Gauge =
        register_gauge!("exchange_rate", "Last polled exchange rate.").unwrap();
    static ref DROPPED_TIMES: Counter =
        register_counter!("rates_bounded", "amount of times exchange rate is being bounded.")
            .unwrap();
    pub static ref REGISTRY: Registry = Registry::new();
}

async fn handle_metrics() -> Result<String> {
    // Gather the metrics.
    let mut buffer = vec![];
    TextEncoder::new().encode(&REGISTRY.gather(), &mut buffer).context("cannot gather metrics")?;
    let response = String::from_utf8(buffer).context("cannot encode response as UTF-8")?;
    Ok(response)
}

pub async fn initialize_prometheus(port: u16) -> Result<()> {
    REGISTRY
        .register(Box::new(EXCHANGE_RATE.clone()))
        .expect("Unable to register exchange rate gauge");
    REGISTRY
        .register(Box::new(DROPPED_TIMES.clone()))
        .expect("Unable to register bounded times counter");

    let metrics_route = warp::path("metrics").then(move || async move {
        let res = handle_metrics().await;
        match res {
            Ok(v) => warp::reply::with_status(v, StatusCode::OK),
            Err(e) => {
                warp::reply::with_status(e.to_string() + ".\n", StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    });

    warp::serve(metrics_route).run(([0, 0, 0, 0], port)).await;
    Ok(())
}

pub fn update_rate(rate: f64) { EXCHANGE_RATE.set(rate) }

pub fn increment_dropped_times() { DROPPED_TIMES.inc() }
