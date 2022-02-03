# Euro to CCD service

Service that pulls exchange rate from services, and uses them to perform microCCD/euro chain updates on the concordium blockchain.

##  Run
To run for testing, use:

`cargo run`:

##  Build
To build for production, use:

`cargo build --release`:

## Parameters:
Explainations of all parameters can be seen by using the help flag, i.e. `cargo run -- --help` or `./euro2ccd-service --help`:

- `secret-names` (environment variable: `EUR2CCD_SERVICE_SECRET_NAMES`): comma separated names of the secrets on AWS, where the governance keys are stored.
- `node` (environment variable: `EUR2CCD_SERVICE_NODE`): the ip and port of the node, to pull data from and to send the chain updates to (ex. http://localhost:10000).
- `log-level` (environment variable: `EUR2CCD_SERVICE_LOG_LEVEL`): determines the log level, disabled as default.
- `prometheus-port` (environment variable: `EUR2CCD_SERVICE_PROMETHEUS_PORT`): Port at which prometheus is served.
- `test` (environment variable: `EUR2CCD_SERVICE_TEST`): allows test settings (currently test-exchange  and local-keys).

Other parameters:
`max-deviation`, `pull-exchange-interval`,  `rpc-token`, `update-interval`, `conversion_threshold_denominator`.
