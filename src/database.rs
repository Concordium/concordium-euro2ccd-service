use crate::Source;
use concordium_rust_sdk::types::ExchangeRate;
use mysql::{params, prelude::Queryable, Opts, Pool, PooledConn};

const READ_RATE_STATEMENT: &str =
    "insert into read_values (value, timestamp, label) values (:value, :timestamp, :label)";
const UPDATE_RATE_STATEMENT: &str = "insert into updates (numerator, denominator, timestamp) \
                                     values (:numerator, :denominator, :timestamp)";
const CREATE_TABLES: &str = "CREATE TABLE IF NOT EXISTS read_values (value DOUBLE NOT NULL, \
                             timestamp DATETIME NOT NULL, label VARCHAR(15)); CREATE TABLE IF NOT \
                             EXISTS updates (numerator BIGINT UNSIGNED NOT NULL, denominator \
                             BIGINT UNSIGNED NOT NULL, timestamp DATETIME NOT NULL);";

const CHECK_FOR_LABEL: &str = "SELECT count(*) FROM information_schema.columns WHERE table_name = \
                               'read_values' AND column_name = 'label' and table_schema = \
                               DATABASE();";
// When we add the label column, it is assumed that all values are from v1, so
// we label them: bitfinex(v1)
const DEFAULT_LABEL: &str = "bitfinex(v1)";

pub fn establish_connection_pool(url: &str) -> mysql::Result<Pool> {
    Pool::new(Opts::from_url(url)?)
}

/// Creates the tables, we are inserting data into. (If they don't exist
/// already)
pub fn create_tables(conn: &mut PooledConn) -> anyhow::Result<()> {
    conn.query_drop(CREATE_TABLES)?;
    // The check for label should return 1/0 depending on the existance of the label
    // column
    match conn.query_first(CHECK_FOR_LABEL)? {
        Some(0) => Ok(conn.query_drop(format!(
            "ALTER TABLE read_values ADD COLUMN label VARCHAR(15) DEFAULT '{}';",
            DEFAULT_LABEL
        ))?),
        Some(_) => Ok(()),
        None => anyhow::bail!("Checking for label column returned no result"),
    }
}

pub fn write_read_rate(conn: &mut PooledConn, value: f64, label: &Source) -> mysql::Result<()> {
    let statement = conn.prep(READ_RATE_STATEMENT)?;
    conn.exec_drop(statement, params! {
        "timestamp" => chrono::offset::Utc::now().naive_utc(),
        "label" => label.to_string(),
        "value" => value,
    })
}

pub fn write_update_rate(conn: &mut PooledConn, value: ExchangeRate) -> mysql::Result<()> {
    let statement = conn.prep(UPDATE_RATE_STATEMENT)?;
    conn.exec_drop(statement, params! {
        "timestamp" => chrono::offset::Utc::now().naive_utc(),
        "numerator" => value.numerator(),
        "denominator" => value.denominator(),
    })
}
