use clap::AppSettings;
use structopt::StructOpt;
use std::collections::HashMap;
use serde_json::json;
use anyhow::Result;

use concordium_rust_sdk::{
    endpoints
};

#[derive(StructOpt)]
struct App {
    #[structopt(
        long = "node",
        help = "GRPC interface of the node(s).",
        default_value = "http://localhost:10000",
        use_delimiter = true,
        env = "EURO2CCD_SERVICE_NODES"
    )]
    endpoint:     Vec<endpoints::Endpoint>,
    #[structopt(
        long = "rpc-token",
        help = "GRPC interface access token for accessing all the nodes.",
        default_value = "rpcadmin",
        env = "EURO2CCD_SERVICE_RPC_TOKEN"
    )]
    token:        String,
    #[structopt(
        long = "log-level",
        default_value = "off",
        help = "Maximum log level.",
        env = "EURO2CCD_SERVICE_LOG_LEVEL"
    )]
    log_level:    log::LevelFilter
}

async fn pull_exchangeRate(client: reqwest::Client) -> Result<f64> {
    let params = json!({"ccy1": "BTC", "ccy2": "USD"});
    let req = client.post("https://api-pub.bitfinex.com/v2/calc/fx").json(&params);
    let resp = req.send().await?.json::<Vec<f64>>().await?;
    Ok(resp[0])
}

#[tokio::main]
async fn main() -> Result<()> {
    let app = {
        let app = App::clap()
        // .setting(AppSettings::ArgRequiredElseHelp)
            .global_setting(AppSettings::ColoredHelp);
        let matches = app.get_matches();
        App::from_clap(&matches)
    };

    let mut log_builder = env_logger::Builder::from_env("TRANSACTION_LOGGER_LOG");
    // only log the current module (main).
    log_builder.filter_module(module_path!(), app.log_level);
    log_builder.init();

    let client = reqwest::Client::new();
    log::debug!("Polling for exchange rate");
    let rate = pull_exchangeRate(client).await?;
    log::info!("New exchange rate polled: {:#?}", rate);
    Ok(())
}
