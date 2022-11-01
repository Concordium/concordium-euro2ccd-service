# Unreleased changes

# 0.5.0
 - Support for protocol 5.
 - The service now uses V2 GRPC node API.

# 0.4.1
 - Bump the SDK to fix a JSON parsing error that would sometimes lead to block
   summary parsing errors.

# 0.4.0

 - Compatibility with node version 4.

# 0.3.2

## Fixed
 - show updated rates in prometheus again (after first update)
 - error when getting blockSummary to a delegation node

# 0.3.1

## Fixed
 - Fix assumption that coinMarketCap response's status always has a error_message field

# 0.3.0

## Added

 - Allow multiple sources, update is the median of the medians of each sources history

# 0.2.2

## Fixed
-   Rejects negative readings from exchanges
