# Euro to CCD service

Service that pulls exchange rate from services, and uses them to perform microCCD/euro chain updates on the concordium blockchain.

##  Run
To run for testing, use:

`cargo run`:

##  Build
To build for production, use:

`cargo build --release`:

## Parameters:
Explanations of all parameters can be seen by using the help flag, i.e. `cargo run -- --help` or `./euro2ccd-service --help`:

- `secret-names` (environment variable: `EUR2CCD_SERVICE_SECRET_NAMES`): Comma separated names of the secrets on AWS, where the governance keys are stored. The service expects one keypair, in the form of a JSON object, per secret.
- `aws-region` (environment variable: `EUR2CCD_SERVICE_AWS_REGION`): The aws region to request the secret, containing the governance keys, from. (default: eu-central-1)
- `node` (environment variable: `EUR2CCD_SERVICE_NODE`): Comma separated ip and port of the node(s), to pull data from and to send the chain updates to. (ex. http://localhost:10000).
- `rpc-token` (environment variable: `EUR2CCD_SERVICE_RPC_TOKEN`): GRPC interface access token for accessing the node. (default: rpcadmin) 
- `log-level` (environment variable: `EUR2CCD_SERVICE_LOG_LEVEL`): Determines the log level, defaults to outputting info messages (and higher priorities).
- `prometheus-port` (environment variable: `EUR2CCD_SERVICE_PROMETHEUS_PORT`): Port at which prometheus is served. (default: 8112)
- `database-url` (environment variable: `EUR2CCD_SERVICE_DATABASE_URL`): MySQL connection url, where every reading and update is inserted at. (Optional)
- `pull-interval` (environment variable: `EUR2CCD_SERVICE_PULL_INTERVAL`): How often to read the exchange rate from each source (In seconds). (default: 60 seconds)
- `max-rates-saved` (environment variable: `EUR2CCD_SERVICE_MAX_RATES_SAVED`): How many exchange rates should be saved at a time from each source (and used to determine the update value). (default: 60) 
- `update-interval` (environment variable: `EUR2CCD_SERVICE_UPDATE_INTERVAL`): How often to update the exchange rate on chain (In seconds). (default: 1800 seconds)
- `warning-increase-threshold` (environment variable: `EUR2CCD_SERVICE_WARNING_INCREASE_THRESHOLD`): Determines the threshold where an update increasing the exchange rate triggers a warning, specified in percentages. (default: 30%)
- `halt-increase-threshold` (environment variable: `EUR2CCD_SERVICE_HALT_INCREASE_THRESHOLD`): Determines the threshold where an update increasing the exchange rate triggers a halt, specified in percentages.  (default: 100%)
- `warning-decrease-threshold` (environment variable: `EUR2CCD_SERVICE_WARNING_DECREASE_THRESHOLD`): Determines the threshold where an update decreasing the exchange rate triggers a warning, specified in percentages. (default: 15%)
- `halt-decrease-threshold` (environment variable: `EUR2CCD_SERVICE_HALT_DECREASE_THRESHOLD`): Determines the threshold where an update decreasing the exchange rate triggers a halt, specified in percentages.  (default: 50%)
- `coin-gecko` (environment variable:  `EUR2CCD_SERVICE_COIN_MARKET_CAP`): If this flag is set, the service will use Coin Gecko as a source.
- `live-coin-watch` (environment variable:  `EUR2CCD_SERVICE_LIVE_COIN_WATCH`): If this flag is set, the service will use Live Coin Watch as a source. The value is expected to be an API key for the site.
- `coin-market-cap` (environment variable:  `EUR2CCD_SERVICE_COIN_MARKET_CAP`): If this flag is set, the service will use Coin Market Cap as a source. The value is expected to be an API key for the site.
- `bitfinex` (environment variable:  `EUR2CCD_SERVICE_BITFINEX`): If this flag is set, the service will use Bitfinex as a source.
 
- `dry-run` (environment variable: `EUR2CCD_DRY_RUN`): Configures the service to only poll and compute the updates it would have done
without performing them. Instead they are logged at INFO level.
- `test-source` (environment variable: `EUR2CCD_SERVICE_TEST_SOURCE`): Comma separated URLs, which the service will add to its list of sources. (See /test-exchange for an example implementation)
- `local-keys` (environment variable: `EUR2CCD_SERVICE_LOCAL_KEYS`): Comma separated names of files, which the service will attempt to read keys from, instead of from secrets on AWS. (Expects the files to contain arrays of keys)


## Forced dry run
If the halt thresholds are violated, the service will enter dry run mode. After Restarting the service, it will forcibly enter dry run mode again.

To disable this forced dry run, remove the `update.lockfile` at:
```
/var/lib/concordium-eur2ccd-service/update.lockfile
```
