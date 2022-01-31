pub const MAX_TIME_CHECK_SUBMISSION: u64 = 120; // seconds
pub const MAX_RETRIES: u64 = 5; // When attempting to reach exchange
pub const INITIAL_RETRY_INTERVAL: u64 = 10; // seconds, when attempting to reach exchange. (This gets doubled each unsuccessful try)
pub const MAXIMUM_RATES_SAVED: u64 = 30; // In rate_history.
pub const BITFINEX_URL: &str = "https://api-pub.bitfinex.com/v2/calc/fx";
pub const LOCAL_URL: &str = "http://127.0.0.1:8111/rate";

pub const CHECK_SUBMISSION_STATUS_INTERVAL: u64 = 5; // seconds
pub const RETRY_SUBMISSION_INTERVAL: u64 = 10; // seconds
pub const UPDATE_EXPIRY_OFFSET: u64 = 100; // seconds
