pub const MAX_TIME_CHECK_SUBMISSION: u64 = 120; // seconds
pub const MAX_RETRIES: u64 = 5; // When attempting to reach exchange
pub const INITIAL_RETRY_INTERVAL: u64 = 10; // seconds, when attempting to reach exchange. (This gets doubled each
                                            // unsuccessful try)
pub const BITFINEX_URL: &str = "https://api-pub.bitfinex.com/v2/calc/fx";

pub const CHECK_SUBMISSION_STATUS_INTERVAL: u64 = 5; // seconds
pub const RETRY_SUBMISSION_INTERVAL: u64 = 10; // seconds
/// Expiry of the update instruction. This should be a bit less than
/// [MAX_TIME_CHECK_SUBMISSION].
pub const UPDATE_EXPIRY_OFFSET: u64 = 100; // seconds

pub const AWS_REGION: &str = "eu-central-1";
