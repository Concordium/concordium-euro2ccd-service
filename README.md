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
- `pull_exchange_interval` (environment variable: `EUR2CCD_SERVICE_PULL_INTERVAL`): How often to read the exchange rate from exchange (In seconds). (default: 60 seconds)
- `max_rates_saved` (environment variable: `EUR2CCD_SERVICE_MAX_RATES_SAVED`): How many exchange rates should be saved at a time (and used to determine the update value). (default: 60) 
- `update_interval` (environment variable: `EUR2CCD_SERVICE_UPDATE_INTERVAL`): How often to update the exchange rate on chain (In seconds). (default: 1800 seconds)
- `warning_increase_threshold` (environment variable: `EUR2CCD_SERVICE_WARNING_INCREASE_THRESHOLD`): Determines the threshold where an update increasing the exchange rate triggers a warning, specified in percentages. (default: 30%)
- `halt_increase_threshold` (environment variable: `EUR2CCD_SERVICE_HALT_INCREASE_THRESHOLD`): Determines the threshold where an update increasing the exchange rate triggers a halt, specified in percentages.  (default: 100%)
- `warning_decrease_threshold` (environment variable: `EUR2CCD_SERVICE_WARNING_DECREASE_THRESHOLD`): Determines the threshold where an update decreasing the exchange rate triggers a warning, specified in percentages. (default: 15%)
- `halt_decrease_threshold` (environment variable: `EUR2CCD_SERVICE_HALT_DECREASE_THRESHOLD`): Determines the threshold where an update decreasing the exchange rate triggers a halt, specified in percentages.  (default: 50%)

- `dry-run` (environment variable: `EUR2CCD_DRY_RUN`): Configures the service to only poll and compute the updates it would have done
without performing them. Instead they are logged at INFO level.
- `test_exchange` (environment variable: `EUR2CCD_SERVICE_TEST_EXCHANGE`): If this parameter is given, the service will read exchanges from the given URL instead of at Bitfinex. (See /local_exchange for an example implementation)
- `local_keys` (environment variable: `EUR2CCD_SERVICE_LOCAL_KEYS`): Comma separated names of files, which the service will attempt to read keys from, instead of from secrets on AWS. (Expects the files to contain arrays of keys)
