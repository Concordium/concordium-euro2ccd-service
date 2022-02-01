use anyhow::Result;
use concordium_rust_sdk::types::{BlockSummary, ExchangeRate, UpdateKeysIndex};
use crypto_common::{base16_encode_string, types::KeyPair};
use num_rational::BigRational;
use num_traits::{identities::One, CheckedDiv, CheckedSub, Signed, ToPrimitive, Zero};
use std::collections::VecDeque;

/**
 * Given keypairs and a BlockSummary, find the corresponding index for each
 * keypair, on the chain. Aborts if any keypair is not registered to sign
 * CCD/euro rate updates.
 */
pub fn get_signer(
    kps: Vec<KeyPair>,
    summary: &BlockSummary,
) -> Result<Vec<(UpdateKeysIndex, KeyPair)>> {
    let update_keys = &summary.updates.keys.level_2_keys.keys;
    let update_key_indices = &summary.updates.keys.level_2_keys.micro_gtu_per_euro;

    // find the key indices to sign with
    let mut signer = Vec::new();
    for kp in kps {
        if let Some(i) = update_keys.iter().position(|public| public.public == kp.public.into()) {
            let idx = UpdateKeysIndex {
                index: i as u16,
            };
            if update_key_indices.authorized_keys.contains(&idx) {
                signer.push((idx, kp))
            } else {
                anyhow::bail!(
                    "The given key {} is not registered for the CCD/Eur rate update.",
                    base16_encode_string(&kp.public)
                );
            }
        } else {
            anyhow::bail!(
                "The given key {} is not registered for any level 2 updates.",
                base16_encode_string(&kp.public)
            );
        }
    }
    Ok(signer)
}

/**
 * Compute the average of the rates stored in the given VeqDeque.
 * Returns None if the queue is empty.
 */
pub fn compute_average(rates: VecDeque<BigRational>) -> Option<BigRational> {
    let len = rates.len();
    rates
        .into_iter()
        .fold(BigRational::zero(), |a, b| a + b)
        .checked_div(&BigRational::from_integer(len.into()))
}

/**
 * Determine whether the candidate is within the range:
 * [baseline - max_deviation % , baseline + max_deviation %]
 * N.B: max_deviation is expected to be given as percentages.
 */
pub fn within_allowed_deviation(
    baseline: &BigRational,
    candidate: &BigRational,
    max_deviation: u8,
) -> bool {
    let max_deviation = BigRational::new(max_deviation.into(), 100.into());
    !((baseline * (BigRational::one() + &max_deviation) <= *candidate)
        || (baseline * (BigRational::one() - max_deviation) >= *candidate))
}

/**
 * Return the value of low or high, which is closest to the target.
 */
fn get_closest(low: ExchangeRate, high: ExchangeRate, target: BigRational) -> ExchangeRate {
    let big_low = BigRational::new(low.numerator.into(), low.denominator.into());
    let big_high = BigRational::new(high.numerator.into(), high.denominator.into());
    let low_diff = target.checked_sub(&big_low);
    let high_diff = big_high.checked_sub(&target);
    match (low_diff, high_diff) {
        (None, Some(_)) => high,
        (Some(_), None) => low,
        (Some(l_d), Some(h_d)) if l_d > h_d => high,
        (Some(_), Some(_)) => low,
        (None, None) => panic!("Could not calculate difference for high nor low"),
    }
}

/**
 * Convert a BigRational type into an exchange rate.
 * 1. Check if the BigRational can be translated directly (both bigints are
 * u64)
 * 2. Traverse the Stern-Brocot tree until we reach a fraction which is
 * within epsilon of the target. If we unable to represent the next mediant
 * with u64, then we abort and return the limit closest to the target.
 */
pub fn convert_big_fraction_to_exchange_rate(
    target: BigRational,
    epsilon: BigRational,
) -> ExchangeRate {
    // Check if the bigints can fit into u64's.
    if let (Some(p), Some(q)) = (target.numer().to_u64(), target.denom().to_u64()) {
        return ExchangeRate {
            numerator:   p,
            denominator: q,
        };
    };

    // Otherwise use "Stern-Brocot approximation":
    let mut mediant: ExchangeRate;
    let mut low = ExchangeRate {
        numerator:   0,
        denominator: 1,
    };
    let mut high = ExchangeRate {
        numerator:   1,
        denominator: 0,
    };
    loop {
        let mediant_numer = low.numerator.checked_add(high.numerator);
        let mediant_denom = low.denominator.checked_add(high.denominator);
        mediant = match (mediant_numer, mediant_denom) {
            (Some(n), Some(d)) => ExchangeRate {
                numerator:   n,
                denominator: d,
            },
            (_, _) => return get_closest(low, high, target),
        };
        let big_mediant = BigRational::new(mediant.numerator.into(), mediant.denominator.into());
        if (&big_mediant - &target).abs() <= epsilon {
            return mediant;
        } else if big_mediant > target {
            high = mediant
        } else {
            low = mediant
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_take_average() {
        let mut v = VecDeque::new();
        v.push_back(BigRational::new(1.into(), 1.into()));
        v.push_back(BigRational::new(9u32.into(), 1u32.into()));
        v.push_back(BigRational::new(5u32.into(), 1u32.into()));
        v.push_back(BigRational::new(9u32.into(), 1u32.into()));
        assert_eq!(compute_average(v), Some(BigRational::new(6u32.into(), 1u32.into())))
        // 24 / 4 = 6
    }

    fn test_convert_u64(num: u64, den: u64) {
        let result = convert_big_fraction_to_exchange_rate(
            BigRational::new(num.into(), den.into()),
            BigRational::new(1.into(), 1000000000000u64.into()),
        );
        assert_eq!(num, result.numerator);
        assert_eq!(den, result.denominator);
    }

    fn test_convert_u128(num: u128, den: u128, res_num: u64, res_den: u64) {
        let result = convert_big_fraction_to_exchange_rate(
            BigRational::new(num.into(), den.into()),
            BigRational::new(1.into(), 1000000000000u64.into()),
        );
        assert_eq!(res_num, result.numerator);
        assert_eq!(res_den, result.denominator);
    }

    #[test]
    fn test_convert_1() { test_convert_u64(1, 101); }

    #[test]
    fn test_convert_2() { test_convert_u64(13902531941473u64, 12500000000000000000u64); }

    #[test]
    fn test_convert_3() { test_convert_u64(12500000000000000000u64, 13902531941473u64); }

    #[test]
    fn test_convert_4() { test_convert_u64(1, 2); }

    #[test]
    fn test_convert_128() {
        test_convert_u128(
            100000000000000000000000000000000000u128,
            200000000000000000000000000000000001u128,
            1,
            2,
        );
    }

    #[test]
    fn test_convert_128_2() {
        test_convert_u128(
            6730672262010705765392518838235123u128,
            12417307353238580889556877312u128,
            444549438399,
            820142,
        );
    }

    #[test]
    fn test_convert_128_3() {
        test_convert_u128(
            78784731800983935460904371u128,
            57712362587357708288u128,
            865205507352,
            633791,
        );
    }

    #[test]
    fn test_convert_128_4() {
        test_convert_u128(
            96961673726254741664712289u128,
            64926407910777421824u128,
            2905555397391,
            1945586,
        );
    }
}
