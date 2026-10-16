//! Small numeric helpers shared by the stages.

/// `fromDB` at 0x563fc: `pow(10, db / 20)`.
pub fn from_db(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Value after a round trip through C's `%.<prec>f` text formatting
/// (stages exchanged data through text files).
pub fn via_fixed(x: f64, prec: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.prec$}").parse().unwrap_or(x)
}

/// Value after a round trip through `std::ostream << double` with default
/// flags, i.e. `%g` with 6 significant digits.
pub fn via_g6(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.5e}").parse().unwrap_or(x)
}

/// `cvttsd2si`-style conversion to `u32` with the saturation the original's
/// unsigned conversion idiom produced.
pub fn sat_u32(d: f64) -> u32 {
    if d.is_nan() || d <= 0.0 {
        0
    } else if d >= u32::MAX as f64 {
        u32::MAX
    } else {
        d as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips() {
        assert_eq!(via_g6(1.584893192461113), 1.58489);
        assert_eq!(via_g6(0.000123456789), 0.000123457);
        assert_eq!(via_fixed(-23.456, 2), -23.46);
        assert_eq!(via_fixed(-0.001, 2), 0.0);
    }
}
