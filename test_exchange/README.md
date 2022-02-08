# Test Exchange

Service which can emulate an exchange, and is to be used with the eur2ccd services test exchange parameter.
It maintains a queue of values, which is serves through the `/rate` endpoint. If the queue is empty, the value specified by the resort_value, is returned instead.

Has 2 parameters:

 * `port` (environment variable: `TEST_EXCHANGE_PORT`): Port at which the exchange is served.
 * `resort-value` (environment variable: `TEST_EXCHANGE_RESORT_VALUE`): The exchange rate, which is returned, when the queue is empty.

Has 3 endpoints:

 * `GET /rate`: get an exchange rate (this should pointed to by the eur2ccd service.
 * `POST /add`: Expects a body that is a json array of floats, which will be added to the queue of values, which is served on `/rate`.
 * `PUT /reset`: clears the queue of values, which is served on `/rate`.
 * `PUT /update-resort/:f64`: updates the resort value, which is served on `/rate`, when the queue is empty.

Example on how to add (using curl):
```console 
curl -d "@rates.json"  -H "Content-Type: application/json" -XPOST http://127.0.0.1:8111/add 
```
Where rates.json could be:
``` json
[0.6, 0.7, 1.0, 1.2]
```

How to update resort value to 0.1 (using curl):
```console 
curl -XPUT http://127.0.0.1:8111/update-resort/0.1 
```
