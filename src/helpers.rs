use crate::prometheus::increment_bounded_times;
use anyhow::{anyhow, Result};
use concordium_rust_sdk::types::ExchangeRate;
use fraction::Fraction;

pub fn convert_f64_to_exchange_rate(value: f64) -> Result<ExchangeRate> {
    let frac = Fraction::from(value as f32); // reduce precision, to reduce size of num/denom, to avoid overflow.
    let numerator = match frac.numer() {
        Some(e) => e,
        None => return Err(anyhow!("Conversion failed: unable to get numerator")),
    };
    let denominator = match frac.denom() {
        Some(e) => e,
        None => return Err(anyhow!("Conversion failed: unable to get denominator")),
    };
    Ok(ExchangeRate {
        numerator:   *numerator,
        denominator: *denominator,
    })
}

pub fn bound_exchange_rate_change(
    current_exchange_rate: ExchangeRate,
    new_exchange_rate: ExchangeRate,
    max_change: u8,
) -> Result<ExchangeRate> {
    let current =
        (current_exchange_rate.numerator as f64) / (current_exchange_rate.denominator as f64);
    let new = (new_exchange_rate.numerator as f64) / (new_exchange_rate.denominator as f64);

    if !current.is_finite() || !new.is_finite() {
        return Err(anyhow!("Converting exchange rates to float resulted in {}, {}", current, new));
    }

    // is the new rate an increase?
    let increase;

    let diff = if current > new {
        increase = false;
        current - new
    } else {
        increase = true;
        new - current
    };

    let max_change_concrete = (current / 100f64) * (max_change as f64);
    log::debug!("Allowed change is {:?} ({} %).", max_change_concrete, max_change);

    if !max_change_concrete.is_finite() {
        return Err(anyhow!("Calculating maximum change resulted in {}", max_change_concrete));
    }

    if diff > max_change_concrete {
        increment_bounded_times();
        let bounded = if increase {
            current + max_change_concrete
        } else {
            current - max_change_concrete
        };
        log::warn!("New exchange rate was outside allowed range, bounding it to {}", bounded);

        if !bounded.is_finite() {
            return Err(anyhow!("Bounded value resulted in {}", bounded));
        }

        return convert_f64_to_exchange_rate(bounded);
    }
    Ok(new_exchange_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_simple_decrease() {
        let current_rate = ExchangeRate {
            numerator:   7,
            denominator: 10,
        }; // .7
        let new_rate = ExchangeRate {
            numerator:   1,
            denominator: 10,
        }; // .1
        let result = ExchangeRate {
            numerator:   7,
            denominator: 20,
        }; // .35
        let max_change = 50u8; // 50 %
        match bound_exchange_rate_change(current_rate, new_rate, max_change) {
            Ok(bounded) => {
                assert_eq!(bounded.numerator, result.numerator);
                assert_eq!(bounded.denominator, result.denominator);
            }
            Err(e) => panic!("{}", e),
        }
    }

    #[test]
    fn bound_simple_increase() {
        let current_rate = ExchangeRate {
            numerator:   7,
            denominator: 10,
        }; // .7
        let new_rate = ExchangeRate {
            numerator:   2,
            denominator: 1,
        }; // 2
        let result = ExchangeRate {
            numerator:   21,
            denominator: 20,
        }; // 1.05
        let max_change = 50u8; // 50 %
        match bound_exchange_rate_change(current_rate, new_rate, max_change) {
            Ok(bounded) => {
                assert_eq!(bounded.numerator, result.numerator);
                assert_eq!(bounded.denominator, result.denominator);
            }
            Err(e) => panic!("{}", e),
        }
    }

    #[test]
    fn bound_large() {
        let current_rate = ExchangeRate {
            numerator:   269873210673,
            denominator: 250000000000000000,
        };
        let new_rate = ExchangeRate {
            numerator:   1,
            denominator: 1,
        };
        let result = ExchangeRate {
            numerator:   5451439,
            denominator: 4999999913984,
        };
        let max_change = 1u8;
        match bound_exchange_rate_change(current_rate, new_rate, max_change) {
            Ok(bounded) => {
                assert_eq!(bounded.numerator, result.numerator);
                assert_eq!(bounded.denominator, result.denominator);
            }
            Err(e) => panic!("{}", e),
        }
    }

    #[test]
    fn bound_larger() {
        let current_rate = ExchangeRate {
            numerator:   13902531941473,
            denominator: 12500000000000000000,
        };
        let new_rate = ExchangeRate {
            numerator:   3,
            denominator: 1,
        };
        let result = ExchangeRate {
            numerator:   244201,
            denominator: 217391300608,
        };
        let max_change = 1u8;
        match bound_exchange_rate_change(current_rate, new_rate, max_change) {
            Ok(bounded) => {
                assert_eq!(bounded.numerator, result.numerator);
                assert_eq!(bounded.denominator, result.denominator);
            }
            Err(e) => panic!("{}", e),
        }
    }
}
