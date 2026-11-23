//! Exact world coordinates of unbounded range and precision.
//!
//! A [`Dyadic`] number is `m · 2^e` with an arbitrary-length integer mantissa `m`.
//! Addition and subtraction are exact, so the camera and the stroke anchors can live
//! anywhere on the canvas, at any zoom depth, without ever losing precision. Only the
//! *difference* between two nearby positions is ever converted to floating point.

use num_bigint::{BigInt, Sign};
use num_traits::{ToPrimitive, Zero};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// `m · 2^e`, exact.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Dyadic {
    pub m: BigInt,
    pub e: i32,
}

impl Dyadic {
    pub fn zero() -> Self {
        Self { m: BigInt::zero(), e: 0 }
    }

    pub fn new(m: impl Into<BigInt>, e: i32) -> Self {
        Self { m: m.into(), e }
    }

    /// Exact conversion of a finite f64 (every finite f64 is a dyadic rational).
    pub fn from_f64(x: f64) -> Self {
        if x == 0.0 || !x.is_finite() {
            return Self::zero();
        }
        let bits = x.to_bits();
        let sign = if bits >> 63 == 1 { -1i64 } else { 1 };
        let exp_bits = ((bits >> 52) & 0x7ff) as i32;
        let frac = (bits & ((1u64 << 52) - 1)) as i64;
        let (mant, exp) = if exp_bits == 0 {
            (frac, -1074)
        } else {
            (frac | (1i64 << 52), exp_bits - 1075)
        };
        Self { m: BigInt::from(sign * mant), e: exp }
    }

    pub fn is_zero(&self) -> bool {
        self.m.is_zero()
    }

    /// `self · 2^k`, exact and O(1).
    pub fn mul_pow2(&self, k: i32) -> Self {
        Self { m: self.m.clone(), e: self.e + k }
    }

    pub fn add(&self, o: &Self) -> Self {
        let e = self.e.min(o.e);
        Self { m: self.mantissa_at(e) + o.mantissa_at(e), e }
    }

    pub fn sub(&self, o: &Self) -> Self {
        let e = self.e.min(o.e);
        Self { m: self.mantissa_at(e) - o.mantissa_at(e), e }
    }

    /// Mantissa when re-expressed with exponent `e` (`e <= self.e`, exact).
    fn mantissa_at(&self, e: i32) -> BigInt {
        debug_assert!(e <= self.e);
        &self.m << ((self.e - e) as u64)
    }

    /// `floor(self / 2^e)`: the integer number of `2^e` units.
    pub fn floor_at(&self, e: i32) -> BigInt {
        match self.e.cmp(&e) {
            Ordering::Greater | Ordering::Equal => &self.m << ((self.e - e) as u64),
            Ordering::Less => &self.m >> ((e - self.e) as u64),
        }
    }

    /// Drops precision below `2^e_min` (rounding toward -inf). Keeps mantissas bounded.
    pub fn truncate(&mut self, e_min: i32) {
        if self.e < e_min {
            self.m >>= (e_min - self.e) as u64;
            self.e = e_min;
        }
        if self.m.is_zero() {
            self.e = e_min;
        }
    }

    /// Nearest f64 (relative error ~2^-53, saturates to ±inf / 0 when out of range).
    pub fn to_f64(&self) -> f64 {
        if self.m.is_zero() {
            return 0.0;
        }
        let bits = self.m.bits() as i64;
        let (m, e) = if bits > 62 {
            let shift = bits - 62;
            (&self.m >> (shift as u64), self.e as i64 + shift)
        } else {
            (self.m.clone(), self.e as i64)
        };
        ldexp(m.to_i64().unwrap() as f64, e)
    }

    /// Exact product.
    pub fn mul(&self, o: &Self) -> Self {
        Self { m: &self.m * &o.m, e: self.e + o.e }
    }

    /// `log2 |self|`, also for values far outside the f64 range (−∞ for zero).
    pub fn log2_abs(&self) -> f64 {
        if self.m.is_zero() {
            return f64::NEG_INFINITY;
        }
        let bits = self.m.bits() as i64;
        let shift = (bits - 53).max(0);
        let top = (&self.m >> (shift as u64)).to_i64().unwrap_or(1).unsigned_abs() as f64;
        top.log2() + (shift + self.e as i64) as f64
    }

    /// Fractional part in [0, 1) as f64 (value minus floor).
    pub fn fract_f64(&self) -> f64 {
        if self.e >= 0 {
            return 0.0;
        }
        let int = self.floor_at(0);
        self.sub(&Dyadic::new(int, 0)).to_f64()
    }
}

impl PartialEq for Dyadic {
    fn eq(&self, o: &Self) -> bool {
        self.sub(o).m.is_zero()
    }
}
impl Eq for Dyadic {}

impl PartialOrd for Dyadic {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Dyadic {
    fn cmp(&self, o: &Self) -> Ordering {
        match self.sub(o).m.sign() {
            Sign::Minus => Ordering::Less,
            Sign::NoSign => Ordering::Equal,
            Sign::Plus => Ordering::Greater,
        }
    }
}

/// `x · 2^e` without intermediate overflow of `2^e`.
pub fn ldexp(mut x: f64, mut e: i64) -> f64 {
    while e > 1000 {
        x *= 2f64.powi(1000);
        e -= 1000;
        if x.is_infinite() {
            return x;
        }
    }
    while e < -1000 {
        x *= 2f64.powi(-1000);
        e += 1000;
        if x == 0.0 {
            return x;
        }
    }
    x * 2f64.powi(e as i32)
}

/// A point in world space.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldPos {
    pub x: Dyadic,
    pub y: Dyadic,
}

impl WorldPos {
    pub fn new(x: Dyadic, y: Dyadic) -> Self {
        Self { x, y }
    }
    pub fn origin() -> Self {
        Self::default()
    }
    pub fn add(&self, o: &Self) -> Self {
        Self::new(self.x.add(&o.x), self.y.add(&o.y))
    }
    pub fn sub(&self, o: &Self) -> Self {
        Self::new(self.x.sub(&o.x), self.y.sub(&o.y))
    }
    pub fn mul_pow2(&self, k: i32) -> Self {
        Self::new(self.x.mul_pow2(k), self.y.mul_pow2(k))
    }
    pub fn truncate(&mut self, e_min: i32) {
        self.x.truncate(e_min);
        self.y.truncate(e_min);
    }
    pub fn to_f64(&self) -> [f64; 2] {
        [self.x.to_f64(), self.y.to_f64()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f64_roundtrip_exact() {
        for x in [0.0, 1.0, -1.5, 1e-300, 1.234_567_890_123_456_7, -7e250, 5e-324] {
            assert_eq!(Dyadic::from_f64(x).to_f64(), x, "{x}");
        }
    }

    #[test]
    fn add_sub_exact_across_huge_scales() {
        let big = Dyadic::new(1, 400); // 2^400
        let tiny = Dyadic::new(3, -400); // 3 * 2^-400
        let s = big.add(&tiny);
        assert_eq!(s.sub(&big), tiny);
        assert_eq!(s.sub(&tiny), big);
        assert!((s.sub(&big).to_f64() / ldexp(3.0, -400) - 1.0).abs() < 1e-15);
    }

    #[test]
    fn floor_at_negative() {
        let v = Dyadic::from_f64(-2.5);
        assert_eq!(v.floor_at(0), BigInt::from(-3));
        assert_eq!(v.floor_at(-1), BigInt::from(-5));
        assert_eq!(v.fract_f64(), 0.5);
    }

    #[test]
    fn to_f64_large_mantissa() {
        let v = Dyadic::new(BigInt::from(1) << 2000u32, -2000);
        assert_eq!(v.to_f64(), 1.0);
        assert_eq!(Dyadic::new(1, 5000).to_f64(), f64::INFINITY);
        assert_eq!(Dyadic::new(1, -5000).to_f64(), 0.0);
    }

    #[test]
    fn ordering() {
        assert!(Dyadic::from_f64(-1.0) < Dyadic::new(1, -900));
        assert_eq!(Dyadic::new(2, 0), Dyadic::new(1, 1));
    }

    #[test]
    fn log2_abs_far_out_of_f64_range() {
        let d = Dyadic::new(BigInt::from(3), 2000);
        assert!((d.log2_abs() - (2000.0 + 3f64.log2())).abs() < 1e-9);
        let d = Dyadic::new(BigInt::from(-1) << 3000u32, -10);
        assert!((d.log2_abs() - 2990.0).abs() < 1e-9);
        assert_eq!(Dyadic::zero().log2_abs(), f64::NEG_INFINITY);
        let p = Dyadic::from_f64(1.5).mul(&Dyadic::from_f64(-0.25));
        assert_eq!(p.to_f64(), -0.375);
    }
}
