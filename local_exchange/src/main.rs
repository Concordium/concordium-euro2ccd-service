use log::info;
use std::sync::{Arc, Mutex};
use structopt::StructOpt;
use warp::{Filter, Reply};

#[derive(Debug, StructOpt)]
struct Config {
    #[structopt(
        long = "port",
        default_value = "8111",
        help = "Port on whic
h the server will listen on.",
        env = "LOCAL_EXCHANGE_PORT"
    )]
    port: u16,
}

/// A small binary that simulates an identity verifier that always verifies an
/// identity, and returns a verified attribute list.
#[tokio::main]
async fn main() {
    env_logger::init();

    let app = Config::clap()
        .setting(clap::AppSettings::ArgRequiredElseHelp)
        .global_setting(clap::AppSettings::ColoredHelp);
    let matches = app.get_matches();
    let opt = Config::from_clap(&matches);

    let rate: Arc<Mutex<f64>> = Arc::new(Mutex::new(0.0));

    let serve_rate = warp::get().and(warp::path!("rate")).map(move || {
        let mut rate_unlocked = rate.lock().unwrap();
        *rate_unlocked = *rate_unlocked + 1000000f64;

        info!("Received request for rate, returning {}", *rate_unlocked);

        let mut resp: Vec<f64> = Vec::new();
        resp.push(*rate_unlocked);

        warp::reply::json(&resp).into_response()
    });

    warp::serve(serve_rate).run(([0, 0, 0, 0], opt.port)).await;
}
