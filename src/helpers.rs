use anyhow::{anyhow, Result};
use concordium_rust_sdk::types::ExchangeRate;
use fraction::Fraction;

pub fn convert_fraction_to_exchange_rate(frac: Fraction) -> Result<ExchangeRate> {
    let numerator = match frac.numer() {
        Some(e) => e,
        None => return Err(anyhow!("unable to get numerator")),
    };
    let denominator = match frac.denom() {
        Some(e) => e,
        None => return Err(anyhow!("unable to get denominator")),
    };
    Ok(ExchangeRate {
        numerator:   *numerator,
        denominator: *denominator,
    })
}
