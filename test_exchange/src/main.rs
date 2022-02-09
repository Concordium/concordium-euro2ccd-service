use log::info;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, RwLock},
};
use structopt::StructOpt;
use warp::{Filter, Reply};

#[derive(Debug, StructOpt)]
struct Config {
    #[structopt(
        long = "port",
        default_value = "8111",
        help = "Port on which the server will listen on.",
        env = "TEST_EXCHANGE_PORT"
    )]
    port:         u16,
    #[structopt(
        long = "resort-value",
        default_value = "0.5",
        help = "Value to resort to if value queue is empty.",
        env = "TEST_EXCHANGE_RESORT_VALUE"
    )]
    resort_value: f64,
}

#[tokio::main]
async fn main() {
    let mut log_builder = env_logger::Builder::new();
    log_builder.filter_module(module_path!(), log::LevelFilter::Info);
    log_builder.init();

    let app = Config::clap()
        .setting(clap::AppSettings::ArgRequiredElseHelp)
        .global_setting(clap::AppSettings::ColoredHelp);
    let matches = app.get_matches();
    let opt = Config::from_clap(&matches);

    info!("Starting at port {}!", opt.port);

    let rates = Arc::new(Mutex::new(VecDeque::<serde_json::Value>::new()));
    let resort_value = Arc::new(RwLock::new(opt.resort_value));

    let resort_serve = resort_value.clone();
    let rates_serve = rates.clone();
    let serve_rate = warp::get().and(warp::path!("rate")).map(move || {
        let mut rates_unlocked = rates_serve.lock().unwrap();
        let rate = match rates_unlocked.pop_front() {
            Some(v) => v,
            None => serde_json::json!(*resort_serve.read().unwrap()),
        };

        info!("Received request for rate, returning {}", rate);
        warp::reply::json(&vec![rate]).into_response()
    });

    let rates_add = rates.clone();
    let add_rates = warp::post().and(warp::body::json()).and(warp::path!("add")).map(
        move |new_rates: Vec<serde_json::Value>| {
            info!("Received new rates {:?}", new_rates);

            let mut rates_unlocked = rates_add.lock().unwrap();
            rates_unlocked.extend(new_rates);

            warp::reply::reply()
        },
    );

    let change_resort =
        warp::put().and(warp::path!("update-resort" / f64)).map(move |new_resort: f64| {
            info!("Received new resort value {:?}", new_resort);
            let mut resort_unlocked = resort_value.write().unwrap();
            *resort_unlocked = new_resort;
            warp::reply::reply()
        });

    let reset_rates = warp::put().and(warp::path!("reset")).map(move || {
        let mut rates_unlocked = rates.lock().unwrap();
        rates_unlocked.clear();
        info!("Cleared all values");
        warp::reply::reply()
    });

    warp::serve(serve_rate.or(reset_rates).or(add_rates).or(change_resort))
        .run(([0, 0, 0, 0], opt.port))
        .await;
}
