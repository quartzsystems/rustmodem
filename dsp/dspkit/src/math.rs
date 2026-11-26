// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Elementary functions that give the same bits on every machine.
//!
//! The signal processing must be deterministic: the same octets in give the
//! same octets and the same events out, on the Windows machine that took a
//! capture and on the Linux runner that replays it. The sine, exponential
//! and logarithm of the system's mathematics library are not held to that:
//! two libraries may differ in the last bit, and a last bit can move a
//! sample across a decision value of G.711 or a level across a threshold.
//!
//! So nothing in the signal processing calls them. These are written with
//! additions, multiplications and divisions alone, which IEEE 754 defines
//! exactly, and so are the same everywhere. They are `const`, which lets
//! tables be built when the crate is compiled. They are for building tables
//! and coefficients, not for the tick path: each takes some tens of
//! operations. The square root is not here because IEEE 754 defines it
//! exactly too, and `f32::sqrt` may be used anywhere.

/// Pi.
pub const PI: f64 = std::f64::consts::PI;

/// Two pi: the radians in a turn.
pub const TAU: f64 = std::f64::consts::TAU;

const LN_2: f64 = std::f64::consts::LN_2;
const LN_10: f64 = std::f64::consts::LN_10;

/// The largest whole number not above `x`, for an `x` that fits an `i64`.
pub const fn floor(x: f64) -> f64 {
    let truncated = x as i64 as f64;
    if truncated > x {
        truncated - 1.0
    } else {
        truncated
    }
}

/// The whole number nearest to `x`, for an `x` that fits an `i64`.
pub const fn round(x: f64) -> f64 {
    floor(x + 0.5)
}

/// The sine of `x` radians.
pub const fn sin(x: f64) -> f64 {
    // Bring the angle into a quarter turn either side of zero, where the
    // series converges in a few terms.
    let turns = x / TAU;
    let mut r = (turns - round(turns)) * TAU;
    if r > PI / 2.0 {
        r = PI - r;
    } else if r < -PI / 2.0 {
        r = -PI - r;
    }
    // x - x^3/3! + x^5/5! - ...
    let square = r * r;
    let mut term = r;
    let mut sum = r;
    let mut n = 1;
    while n <= 12 {
        term = -term * square / ((2 * n) as f64 * (2 * n + 1) as f64);
        sum += term;
        n += 1;
    }
    sum
}

/// The cosine of `x` radians.
pub const fn cos(x: f64) -> f64 {
    sin(x + PI / 2.0)
}

/// e to the power `x`.
pub const fn exp(x: f64) -> f64 {
    // e^x = 2^k e^r, with r no more than half of ln 2 from zero.
    let k = round(x / LN_2);
    let r = x - k * LN_2;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut n = 1;
    while n <= 18 {
        term = term * r / n as f64;
        sum += term;
        n += 1;
    }
    let mut k = k as i64;
    while k > 0 {
        sum *= 2.0;
        k -= 1;
    }
    while k < 0 {
        sum /= 2.0;
        k += 1;
    }
    sum
}

/// The natural logarithm of `x`. Zero and negative numbers, which have
/// none, give a large negative number: a level of nothing is very far
/// down, and no caller wants a NaN from a silent line.
pub const fn ln(x: f64) -> f64 {
    // A NaN is neither, and is nothing too.
    if x.is_nan() || x <= 1.0e-300 {
        return -690.0;
    }
    // x = m 2^k with m between the square roots of a half and of two.
    let mut m = x;
    let mut k = 0i32;
    while m > std::f64::consts::SQRT_2 {
        m /= 2.0;
        k += 1;
    }
    while m < std::f64::consts::FRAC_1_SQRT_2 {
        m *= 2.0;
        k -= 1;
    }
    // ln m = 2 (t + t^3/3 + t^5/5 + ...) with t = (m - 1) / (m + 1), which
    // is within 0.172 of zero.
    let t = (m - 1.0) / (m + 1.0);
    let square = t * t;
    let mut power = t;
    let mut sum = t;
    let mut n = 1;
    while n <= 14 {
        power *= square;
        sum += power / (2 * n + 1) as f64;
        n += 1;
    }
    2.0 * sum + k as f64 * LN_2
}

/// The logarithm to the base ten of `x`.
pub const fn log10(x: f64) -> f64 {
    ln(x) / LN_10
}

/// Ten to the power `x`.
pub const fn pow10(x: f64) -> f64 {
    exp(x * LN_10)
}

/// The arctangent of `y / x` in radians, in the quadrant of the point
/// `(x, y)`: between minus pi and pi. The origin gives zero.
pub const fn atan2(y: f64, x: f64) -> f64 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let ax = if x < 0.0 { -x } else { x };
    let ay = if y < 0.0 { -y } else { y };
    // The angle in the first octant, from the smaller over the larger.
    let (small, large) = if ay > ax { (ax, ay) } else { (ay, ax) };
    let mut angle = atan_unit(small / large);
    if ay > ax {
        angle = PI / 2.0 - angle;
    }
    if x < 0.0 {
        angle = PI - angle;
    }
    if y < 0.0 { -angle } else { angle }
}

/// The arctangent of a `t` between zero and one.
const fn atan_unit(t: f64) -> f64 {
    // Halve the angle twice with atan t = 2 atan(t / (1 + sqrt(1 + t^2))),
    // which leaves an argument under 0.2 for the series. The square root
    // is Newton's, to keep this `const`.
    let t = t / (1.0 + sqrt_newton(1.0 + t * t));
    let t = t / (1.0 + sqrt_newton(1.0 + t * t));
    let square = t * t;
    let mut power = t;
    let mut sum = t;
    let mut n = 1;
    while n <= 12 {
        power = -power * square;
        sum += power / (2 * n + 1) as f64;
        n += 1;
    }
    4.0 * sum
}

/// The square root of an `x` between one and two, by Newton's method.
const fn sqrt_newton(x: f64) -> f64 {
    let mut root = 0.5 * (1.0 + x);
    let mut n = 0;
    while n < 6 {
        root = 0.5 * (root + x / root);
        n += 1;
    }
    root
}

/// The modified Bessel function of the first kind and order zero, which
/// the Kaiser window is made of.
pub const fn bessel_i0(x: f64) -> f64 {
    // The sum over k of ((x / 2)^k / k!)^2.
    let half = x / 2.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut k = 1;
    while k <= 40 {
        term = term * half / k as f64;
        sum += term * term;
        k += 1;
    }
    sum
}

/// sin(pi x) / (pi x), and one at zero.
pub const fn sinc(x: f64) -> f64 {
    if x > -1.0e-9 && x < 1.0e-9 {
        1.0
    } else {
        sin(PI * x) / (PI * x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance * b.abs().max(1.0)
    }

    #[test]
    fn they_agree_with_the_library() {
        let mut x = -40.0f64;
        while x < 40.0 {
            assert!(close(sin(x), x.sin(), 1e-13), "sin {x}");
            assert!(close(cos(x), x.cos(), 1e-13), "cos {x}");
            assert!(close(exp(x), x.exp(), 1e-13), "exp {x}");
            assert!(close(pow10(x / 4.0), 10f64.powf(x / 4.0), 1e-13), "pow10");
            x += 0.137;
        }
        let mut x = 1.0e-12f64;
        while x < 1.0e12 {
            assert!(close(ln(x), x.ln(), 1e-13), "ln {x}");
            assert!(close(log10(x), x.log10(), 1e-13), "log10 {x}");
            x *= 1.37;
        }
        let mut angle = -3.1f64;
        while angle < 3.1 {
            for radius in [0.001, 1.0, 250.0] {
                let (y, x) = (radius * angle.sin(), radius * angle.cos());
                assert!(close(atan2(y, x), angle, 1e-12), "atan2 {angle}");
            }
            angle += 0.0173;
        }
        let mut x = 0.0f64;
        while x < 12.0 {
            // Against the series summed the long way.
            let mut sum = 0.0;
            let mut term = 1.0f64;
            for k in 1..80 {
                sum += term * term;
                term *= x / 2.0 / k as f64;
            }
            assert!(close(bessel_i0(x), sum, 1e-12), "i0 {x}");
            x += 0.31;
        }
    }

    #[test]
    fn the_edges_are_what_they_should_be() {
        assert_eq!(sin(0.0), 0.0);
        assert!(close(sin(PI / 2.0), 1.0, 1e-15));
        assert!(close(cos(0.0), 1.0, 1e-15));
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(ln(1.0), 0.0);
        assert_eq!(sinc(0.0), 1.0);
        assert!(sinc(1.0).abs() < 1e-15);
        assert_eq!(atan2(0.0, 0.0), 0.0);
        assert!(close(atan2(1.0, 0.0), PI / 2.0, 1e-15));
        assert!(close(atan2(0.0, -1.0), PI, 1e-15));
        assert!(close(atan2(-1.0, -1.0), -3.0 * PI / 4.0, 1e-15));
        // Nothing has no logarithm, and is not a NaN.
        assert!(ln(0.0) < -600.0);
        assert!(ln(-1.0) < -600.0);
        assert!(ln(f64::NAN) < -600.0);
        assert_eq!(floor(-0.5), -1.0);
        assert_eq!(floor(2.0), 2.0);
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(-2.4), -2.0);
    }

    #[test]
    fn they_can_build_a_table_at_compile_time() {
        const TABLE: [f64; 4] = [sin(0.0), sin(PI / 6.0), exp(1.0), log10(1000.0)];
        assert!(close(TABLE[1], 0.5, 1e-15));
        assert!(close(TABLE[2], std::f64::consts::E, 1e-15));
        assert!(close(TABLE[3], 3.0, 1e-15));
    }
}
