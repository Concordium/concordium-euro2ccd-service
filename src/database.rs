use concordium_rust_sdk::types::ExchangeRate;
use mysql::{params, prelude::Queryable, Opts, Pool, PooledConn};

const READ_RATE_STATEMENT: &str =
    "insert into read_values (value, timestamp) values (:value, :timestamp)";
const UPDATE_RATE_STATEMENT: &str = "insert into updates (numerator, denominator, timestamp) \
                                     values (:numerator, :denominator, :timestamp)";

pub fn establish_connection_pool(url: &str) -> mysql::Result<Pool> {
    Pool::new(Opts::from_url(url)?)
}

pub fn write_read_rate(conn: &mut PooledConn, value: f64) -> mysql::Result<()> {
    let statement = conn.prep(READ_RATE_STATEMENT)?;
    conn.exec_drop(statement, params! {
        "timestamp" => chrono::offset::Utc::now().naive_utc(),
        "value" => value,
    })
}

pub fn write_update_rate(conn: &mut PooledConn, value: ExchangeRate) -> mysql::Result<()> {
    let statement = conn.prep(UPDATE_RATE_STATEMENT)?;
    conn.exec_drop(statement, params! {
        "timestamp" => chrono::offset::Utc::now().naive_utc(),
        "numerator" => value.numerator,
        "denominator" => value.denominator,
    })
}
