use anyhow::{Context, Result};
use num_rational::BigRational;
use num_traits::ToPrimitive;
use prometheus::{Encoder, Gauge, GaugeVec, IntCounter, IntGauge, Registry, TextEncoder};
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
    /// The last exchange rate read from bitfinex.
    exchange_rate_read:           GaugeVec,
    /// The last exchange rate read from bitfinex.
    exchange_rate_updated:        Gauge,
    /// Number of times an update has been outside the warning threshold.
    warning_threshold_violations: IntCounter,
    /// Number of times we failed to submit an update.
    /// Resets to 0 upon successful submission.
    update_attempts:              IntGauge,
    /// A boolean gauge that indicates whether the service is in
    /// dry_run/protected mode (1) or not (0).
    protected:                    IntGauge,
    /// Number of times we failed to write to the database:
    failed_database_updates:      IntCounter,
}

pub const EXCHANGE_LABEL: &str = "exchange";
pub const COINGECKO_LABEL: &str = "coin_gecko";
pub const LIVECOINWATCH_LABEL: &str = "live_coin_watch";
pub const COINMARKETCAP_LABEL: &str = "coin_market_cap";

impl Stats {
    pub fn update_read_rate(&self, rate: f64, label: &str) {
        match self.exchange_rate_read.get_metric_with_label_values(&[label]) {
            Ok(metric) => metric.set(rate),
            Err(e) => log::warn!("Unable to update read rate to {}, on label {}, due to: {}", rate, label, e),
        }
    }

    pub fn update_updated_rate(&self, rate: &BigRational) {
        match rate.to_f64() {
            Some(f) => self.exchange_rate_updated.set(f),
            None => log::warn!("Unable to convert updated rate {}/{} to float for Prometheus", rate.numer(), rate.denom()),
        }
    }

    pub fn increment_warning_threshold_violations(&self) { self.warning_threshold_violations.inc() }

    pub fn increment_update_attempts(&self) { self.update_attempts.inc() }

    pub fn reset_update_attempts(&self) { self.update_attempts.set(0) }

    pub fn set_protected(&self) { self.protected.set(1); }

    pub fn increment_failed_database_updates(&self) { self.failed_database_updates.inc() }
}

pub async fn initialize() -> anyhow::Result<(Registry, Stats)> {
    let registry = Registry::new();

    let exchange_rate_read_opts = prometheus::Opts::new("exchange_rate_read", "Last polled exchange rate.");
    let exchange_rate_read = GaugeVec::new(exchange_rate_read_opts, &["Source"])?;

    let exchange_rate_updated = Gauge::new("exchange_rate_updated", "Last updated exchange rate.")?;
    let warning_threshold_violations = IntCounter::new(
        "warning_threshold_violations",
        "Amount of times an update has been outside the warning threshold.",
    )?;
    let update_attempts =
        IntGauge::new("failed_submissions", "Amount of times submitting an update has failed.")?;
    let protected = IntGauge::new(
        "in_protected_mode",
        "Whether the service is in protected (1) mode or not (0).",
    )?;
    let failed_database_updates = IntCounter::new(
        "failed_database_updates",
        "Amount of times writing to the database has failed.",
    )?;
    registry.register(Box::new(exchange_rate_read.clone()))?;
    registry.register(Box::new(exchange_rate_updated.clone()))?;
    registry.register(Box::new(warning_threshold_violations.clone()))?;
    registry.register(Box::new(update_attempts.clone()))?;
    registry.register(Box::new(protected.clone()))?;
    registry.register(Box::new(failed_database_updates.clone()))?;
    Ok((registry, Stats {
        exchange_rate_read,
        exchange_rate_updated,
        warning_threshold_violations,
        update_attempts,
        protected,
        failed_database_updates,
    }))
}
