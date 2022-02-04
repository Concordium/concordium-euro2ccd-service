use anyhow::{Context, Result};
use prometheus::{Encoder, Gauge, IntCounter, IntGauge, Registry, TextEncoder};
use warp::{http::StatusCode, Filter};

async fn handle_metrics(registry: Registry) -> Result<String> {
    // Gather the metrics.
    let mut buffer = vec![];
    TextEncoder::new().encode(&registry.gather(), &mut buffer).context("cannot gather metrics")?;
    let response = String::from_utf8(buffer).context("cannot encode response as UTF-8")?;
    Ok(response)
}

pub async fn serve_prometheus(registry: Registry, port: u16) {
    let metrics_route = warp::path("metrics").then(move || {
        let registry = registry.clone();
        async move {
            let res = handle_metrics(registry).await;
            match res {
                Ok(v) => warp::reply::with_status(v, StatusCode::OK),
                Err(e) => warp::reply::with_status(
                    e.to_string() + ".\n",
                    StatusCode::INTERNAL_SERVER_ERROR,
                ),
            }
        }
    });
    warp::serve(metrics_route).run(([0, 0, 0, 0], port)).await;
}

#[derive(Debug, Clone)]
pub struct Stats {
    exchange_rate:   Gauge,
    dropped_times:   IntCounter,
    /// Number of times we failed to submit an update.
    /// Resets to 0 upon successful submission.
    update_attempts: IntGauge,
}

impl Stats {
    pub fn update_rate(&self, rate: f64) { self.exchange_rate.set(rate) }

    pub fn increment_dropped_times(&self) { self.dropped_times.inc() }

    pub fn increment_update_attempts(&self) { self.update_attempts.inc() }

    pub fn reset_update_attempts(&self) { self.update_attempts.set(0) }
}

pub async fn initialize() -> anyhow::Result<(Registry, Stats)> {
    let registry = Registry::new();
    let exchange_rate = Gauge::new("exchange_rate", "Last polled exchange rate.")?;
    let dropped_times =
        IntCounter::new("rates_bounded", "amount of times exchange rate has been bounded")?;
    let update_attempts =
        IntGauge::new("failed_submissions", "amount of times submitting an update has failed")?;
    registry.register(Box::new(exchange_rate.clone()))?;
    registry.register(Box::new(dropped_times.clone()))?;
    registry.register(Box::new(update_attempts.clone()))?;
    Ok((registry, Stats {
        exchange_rate,
        dropped_times,
        update_attempts,
    }))
}
