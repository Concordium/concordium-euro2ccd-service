use concordium_rust_sdk::types::{BlockSummary, ExchangeRate, UpdateKeysIndex};
use crypto_common::{base16_encode_string, types::KeyPair};
use num_bigint::BigInt;
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{CheckedDiv, ToPrimitive, Zero};
use std::{collections::VecDeque, ops::Div};

/**
 * Given keypairs and a BlockSummary, find the corresponding index for each
 * keypair, on the chain. Aborts if any keypair is not registered to sign
 * CCD/euro rate updates.
 */
pub fn get_signer(
    kps: Vec<KeyPair>,
    summary: &BlockSummary,
) -> anyhow::Result<Vec<(UpdateKeysIndex, KeyPair)>> {
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
pub fn compute_average(rates: &[BigRational]) -> Option<BigRational> {
    rates
        .iter()
        .fold(BigRational::zero(), |a, b| a + b)
        .checked_div(&BigRational::from_integer(rates.len().into()))
}

/**
 * Compute the median of the rates stored in the given VeqDeque.
 * Returns None if the queue is empty.
 */
pub fn compute_median(rates: &VecDeque<BigRational>) -> Option<BigRational> {
    let len = rates.len();
    if len == 0 {
        return None;
    }
    let mut rate_vec = rates.iter().cloned().collect::<Vec<BigRational>>();
    rate_vec.sort();
    if len.is_odd() {
        Some(rate_vec[(len - 1) / 2].clone())
    } else {
        compute_average(&rate_vec[(len - 1) / 2..len / 2])
    }
}

/**
 * Convert a BigRational type into an exchange rate.
 * 1. Check if the BigRational can be translated directly (both bigints are
 * u64)
 * 2. Divide the numerator and denominator each by 2.
 * Repeat until 1. succeeds.
 */
pub fn convert_big_fraction_to_exchange_rate(target: &BigRational) -> ExchangeRate {
    let mut numerator: BigInt = target.numer().clone();
    let mut denominator: BigInt = target.denom().clone();
    loop {
        // Check if the bigints can fit into u64's.
        if let (Some(p), Some(q)) = (numerator.to_u64(), denominator.to_u64()) {
            return ExchangeRate {
                numerator:   p,
                denominator: q,
            };
        };
        numerator = numerator.div(2);
        denominator = denominator.div(2);
    }
}

/**
 * Calculates the relative change from the current to the change, in
 * percentages.
 */
pub fn relative_change(current: &BigRational, change: &BigRational) -> BigRational {
    if current > change {
        (current - change) * BigRational::from_integer(100.into()) / current
    } else {
        (change - current) * BigRational::from_integer(100.into()) / current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use num_traits::Signed;

    #[test]
    fn test_compute_average() {
        let mut v = Vec::new();
        v.push(BigRational::new(1.into(), 1.into()));
        v.push(BigRational::new(9u32.into(), 1u32.into()));
        v.push(BigRational::new(5u32.into(), 1u32.into()));
        v.push(BigRational::new(9u32.into(), 1u32.into()));
        assert_eq!(compute_average(&v), Some(BigRational::new(6u32.into(), 1u32.into())))
        // 24 / 4 = 6
    }

    #[test]
    fn test_relative_change() {
        assert_eq!(
            relative_change(
                &BigRational::from_integer(50.into()),
                &BigRational::from_integer(40.into())
            ),
            BigRational::from_integer(20.into())
        );
        assert_eq!(
            relative_change(
                &BigRational::from_integer(50.into()),
                &BigRational::from_integer(30.into())
            ),
            BigRational::from_integer(40.into())
        );
        assert_eq!(
            relative_change(
                &BigRational::from_integer(50.into()),
                &BigRational::from_integer(75.into())
            ),
            BigRational::from_integer(50.into())
        );
        assert_eq!(
            relative_change(
                &BigRational::from_integer(100.into()),
                &BigRational::from_integer(50.into())
            ),
            BigRational::from_integer(50.into())
        );
        assert_eq!(
            relative_change(
                &BigRational::from_integer(50.into()),
                &BigRational::from_integer(100.into())
            ),
            BigRational::from_integer(100.into())
        );
        assert_eq!(
            relative_change(
                &BigRational::from_integer(100.into()),
                &BigRational::from_integer(200.into())
            ),
            BigRational::from_integer(100.into())
        );
        assert_eq!(
            relative_change(
                &BigRational::from_integer(200.into()),
                &BigRational::from_integer(100.into())
            ),
            BigRational::from_integer(50.into())
        );
    }

    #[test]
    fn test_compute_median() {
        let mut v = VecDeque::new();
        v.push_back(BigRational::new(1.into(), 1.into()));
        v.push_back(BigRational::new(9u32.into(), 1u32.into()));
        v.push_back(BigRational::new(5u32.into(), 1u32.into()));
        v.push_back(BigRational::new(9u32.into(), 1u32.into()));
        assert_eq!(compute_median(&v), Some(BigRational::new(7u32.into(), 1u32.into())))
        // (5 + 9) / 2 = 7
    }

    #[test]
    fn test_compute_median_2() {
        let mut v = VecDeque::new();
        v.push_back(BigRational::new(20.into(), 1.into()));
        v.push_back(BigRational::new(100u32.into(), 9u32.into()));
        v.push_back(BigRational::new(1u32.into(), 12u32.into()));
        v.push_back(BigRational::new(1u32.into(), 100u32.into()));
        assert_eq!(compute_median(&v), Some(BigRational::new(403u32.into(), 72u32.into())))
    }

    #[test]
    fn test_compute_median_odd() {
        let mut v = VecDeque::new();
        v.push_back(BigRational::new(20000.into(), 1.into()));
        v.push_back(BigRational::new(20.into(), 1.into()));
        v.push_back(BigRational::new(100u32.into(), 9u32.into()));
        v.push_back(BigRational::new(1u32.into(), 12u32.into()));
        v.push_back(BigRational::new(1u32.into(), 100u32.into()));
        assert_eq!(compute_median(&v), Some(BigRational::new(100u32.into(), 9u32.into())))
    }

    fn test_convert_u64(num: u64, den: u64) {
        let result =
            convert_big_fraction_to_exchange_rate(&BigRational::new(num.into(), den.into()));
        assert_eq!(num, result.numerator);
        assert_eq!(den, result.denominator);
    }

    fn test_convert_u128(num: u128, den: u128) {
        let input = BigRational::new(num.into(), den.into());
        let result = convert_big_fraction_to_exchange_rate(&input);
        assert!(
            (input - BigRational::new(result.numerator.into(), result.denominator.into())).abs()
                < BigRational::new(1.into(), 100000000u64.into())
        );
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
        );
    }

    #[test]
    fn test_convert_128_2() {
        test_convert_u128(
            6730672262010705765392518838235123u128,
            12417307353238580889556877312u128,
        );
    }

    #[test]
    fn test_convert_128_3() {
        test_convert_u128(78784731800983935460904371u128, 57712362587357708288u128);
    }

    #[test]
    fn test_convert_128_4() {
        test_convert_u128(96961673726254741664712289u128, 64926407910777421824u128);
    }
}
