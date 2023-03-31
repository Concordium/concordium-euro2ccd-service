use concordium_rust_sdk::types::ExchangeRate;
use num_bigint::BigInt;
use num_integer::Integer;
use num_rational::BigRational;
use num_traits::{CheckedDiv, ToPrimitive, Zero};
use std::collections::VecDeque;

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
        compute_average(&rate_vec[(len - 1) / 2..(len / 2) + 1])
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
            return ExchangeRate::new_unchecked(p, q);
        };
        numerator /= 2;
        denominator /= 2;
        let gcd = numerator.gcd(&denominator);
        if gcd > 1.into() {
            numerator /= &gcd;
            denominator /= gcd;
        }
    }
}

/**
 * Calculates the relative change from the current to the new, in
 * percentages.
 */
pub fn relative_change(current: &BigRational, new: &BigRational) -> BigRational {
    if current > new {
        (current - new) * BigRational::from_integer(100.into()) / current
    } else {
        (new - current) * BigRational::from_integer(100.into()) / current
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
    fn test_compute_median_floats_1() {
        let median_part = BigRational::from_float(0.03878333).unwrap();
        let mut v = VecDeque::new();
        v.push_back(BigRational::from_float(0.03885685));
        v.push_back(BigRational::from_float(0.0388584));
        v.push_back(BigRational::from_float(0.0388584));
        v.push_back(BigRational::from_float(0.03878333));
        v.push_back(BigRational::from_float(0.03878333));
        v.push_back(BigRational::from_float(0.03878333));
        v.push_back(BigRational::from_float(0.03878333));
        v.push_back(BigRational::from_float(0.03878333));
        v.push_back(BigRational::from_float(0.03878333));
        v.push_back(BigRational::from_float(0.03893119));
        match v.into_iter().collect::<Option<VecDeque<_>>>().and_then(|rm| compute_median(&rm)) {
            Some(v) => assert_eq!(
                v,
                (median_part.clone() + median_part) / BigRational::from_integer(2.into())
            ),
            None => panic!("unexpeced missing result"),
        }
    }

    #[test]
    fn test_compute_median_floats_2() {
        let median_part_1 = BigRational::from_float(0.038753264437224634).unwrap();
        let median_part_2 = BigRational::from_float(0.0387291204482976).unwrap();
        let mut v = VecDeque::new();
        v.push_back(BigRational::from_float(0.03890143506317882));
        v.push_back(BigRational::from_float(0.038753264437224634));
        v.push_back(BigRational::from_float(0.03875608469063674));
        v.push_back(BigRational::from_float(0.0387291204482976));
        v.push_back(BigRational::from_float(0.03872209783880581));
        v.push_back(BigRational::from_float(0.03872864822122797));
        v.push_back(BigRational::from_float(0.038725175847088227));
        v.push_back(BigRational::from_float(0.03871499568024753));
        v.push_back(BigRational::from_float(0.03878131780389962));
        v.push_back(BigRational::from_float(0.03882990048880441));
        match v.into_iter().collect::<Option<VecDeque<_>>>().and_then(|rm| compute_median(&rm)) {
            Some(v) => {
                assert_eq!(v, (median_part_1 + median_part_2) / BigRational::from_integer(2.into()))
            }
            None => panic!("unexpeced missing result"),
        }
    }

    #[test]
    fn test_compute_median_floats_3() {
        let median_part_1 = BigRational::from_float(0.03828436591897897).unwrap();
        let median_part_2 = BigRational::from_float(0.03820941906671523).unwrap();
        let mut v = VecDeque::new();
        v.push_back(BigRational::from_float(0.03828436591897897));
        v.push_back(BigRational::from_float(0.03819855216150514));
        v.push_back(BigRational::from_float(0.03819348471351539));
        v.push_back(BigRational::from_float(0.03820941906671523));
        v.push_back(BigRational::from_float(0.03820393607375668));
        v.push_back(BigRational::from_float(0.03820735176404182));
        v.push_back(BigRational::from_float(0.03830744063881822));
        v.push_back(BigRational::from_float(0.0382987008046979));
        v.push_back(BigRational::from_float(0.03829543671546038));
        v.push_back(BigRational::from_float(0.03838764088740058));
        match v.into_iter().collect::<Option<VecDeque<_>>>().and_then(|rm| compute_median(&rm)) {
            Some(v) => {
                assert_eq!(v, (median_part_1 + median_part_2) / BigRational::from_integer(2.into()))
            }
            None => panic!("unexpeced missing result"),
        }
    }

    #[test]
    fn test_compute_median_of_medians() {
        let median_1 = BigRational::from_float(0.03878333).unwrap();
        let median_2_part_1 = BigRational::from_float(0.038753264437224634).unwrap();
        let median_2_part_2 = BigRational::from_float(0.0387291204482976).unwrap();
        let median_2 = (median_2_part_1 + median_2_part_2) / BigRational::from_integer(2.into());
        let median_3_part_1 = BigRational::from_float(0.03828436591897897).unwrap();
        let median_3_part_2 = BigRational::from_float(0.03820941906671523).unwrap();
        let median_3 = (median_3_part_1 + median_3_part_2) / BigRational::from_integer(2.into());
        // These values are the results from previous tests
        let mut v = VecDeque::new();
        v.push_back(median_1);
        v.push_back(median_2.clone());
        v.push_back(median_3);
        assert_eq!(compute_median(&v), Some(median_2))
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
        assert_eq!(num, result.numerator());
        assert_eq!(den, result.denominator());
    }

    fn test_convert_u128(num: u128, den: u128) {
        let input = BigRational::new(num.into(), den.into());
        let result = convert_big_fraction_to_exchange_rate(&input);
        assert!(
            (input - BigRational::new(result.numerator().into(), result.denominator().into())).abs()
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
