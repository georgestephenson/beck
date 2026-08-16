//! What `beck_prim::math` claims, checked against something that is not `beck_prim::math`.
//!
//! The claim is not "close to the host's libm" — that is the property the module exists to stop
//! depending on. It is **the nearest double to the true value**, which is a statement about the
//! mathematics and therefore checkable without a second implementation to agree with.
//!
//! So [`reference`] computes sines and cosines a second time and every part of it is chosen to be
//! a different route to the same number: π comes from the Bailey–Borwein–Plouffe series rather
//! than from Machin's formula, the argument is reduced by binary long division rather than by a
//! window into the bits of 2/π, the series is a term recurrence rather than Horner over a
//! coefficient table, and it all happens at 1408 bits rather than 256. A bug that survives both is
//! a bug in the mathematics rather than in either program.
//!
//! [`the_host_libm_would_fail_this`] is the other half, and it is the one worth reading: a test
//! that passes for a reason unrelated to what it guards is `docs/82` §82.10's pattern. It asserts
//! that the answers here are ones the platform's own `sin` does *not* give, so a change that
//! quietly went back to `f64::sin` would turn this file red rather than green.

use beck_prim::math;

/// How many arguments each property is checked at.
///
/// The reference below is four orders of magnitude slower than the thing it checks — it reduces
/// bit by bit and multiplies 42 limbs at a time — so the sample is sized to keep
/// `cargo test --workspace` quick and opened up when the same suite is run in release. Both sizes
/// walk the same sequence, so the smaller one is a prefix of the larger rather than a different
/// test.
const SAMPLE: usize = if cfg!(debug_assertions) { 300 } else { 4000 };

#[test]
fn every_answer_is_the_nearest_double_to_the_true_one() {
    let mut checked = 0;
    for x in arguments().take(SAMPLE) {
        let (want_sin, want_cos) = reference::sin_cos(x);
        assert_eq!(
            math::sin(x).to_bits(),
            want_sin.to_bits(),
            "sin({x:e}) [{:016X}]: {:e} is not the nearest double to the true value {want_sin:e}",
            x.to_bits(),
            math::sin(x),
        );
        assert_eq!(
            math::cos(x).to_bits(),
            want_cos.to_bits(),
            "cos({x:e}) [{:016X}]: {:e} is not the nearest double to the true value {want_cos:e}",
            x.to_bits(),
            math::cos(x),
        );
        checked += 1;
    }
    assert_eq!(checked, SAMPLE);
}

#[test]
fn the_host_libm_would_fail_this() {
    // The whole point of the module is that the answer does not come from the platform, and a
    // test that both implementations pass would not notice a change back to the platform's. So
    // count the arguments where the two differ, and require that there are some: this is the
    // assertion that would have gone red before `beck_prim::math` existed.
    let mut differ = 0;
    let mut example = (0.0, 0.0, 0.0);
    for x in arguments().take(SAMPLE) {
        if math::sin(x).to_bits() != x.sin().to_bits() {
            differ += 1;
            example = (x, math::sin(x), x.sin());
        }
        if math::cos(x).to_bits() != x.cos().to_bits() {
            differ += 1;
            example = (x, math::cos(x), x.cos());
        }
    }
    assert!(
        differ > 0,
        "this platform's libm agrees with `beck_prim::math` everywhere in the sample, so nothing \
         here would notice a change back to it — widen the sample rather than deleting the test"
    );
    println!(
        "{differ} of {} answers differ from this platform's libm; one of them is {:e}, where the \
         correctly-rounded answer is {:?} and the platform says {:?}",
        SAMPLE * 2,
        example.0,
        example.1,
        example.2
    );
}

/// What a sine costs, and the shape that cost has to keep.
///
/// **Gate the shape, print the rate** ([`docs/64`](../../../../docs/64-compile-speed-report.md)
/// §64.1): a threshold on nanoseconds would flake on a shared machine, but the property worth
/// holding does not involve a clock at all — **the cost of a sine must not grow with the size of
/// its argument**. Reducing `10^300` the obvious way is a loop over the exponent, and the
/// reference in this file is exactly that, at a thousand steps a call; the window into 2/π is why
/// the answer is a fixed amount of work at every exponent a double has.
///
/// The rate it prints is the number `docs/adr/0031` quotes, and
/// `cargo test -p beck-prim --release --test transcendentals -- --nocapture` is the command.
#[test]
fn a_sine_costs_the_same_at_every_size() {
    let at = |scale: f64, ours: bool| {
        let xs: Vec<f64> = (0..20_000)
            .map(|i| (i as f64 * 0.000_063 + 0.5) * scale)
            .collect();
        // One pass to warm, one to time.
        let run = || {
            let start = std::time::Instant::now();
            let mut acc = 0.0;
            for &x in &xs {
                acc += if ours { math::sin(x) } else { x.sin() };
            }
            (start.elapsed(), acc)
        };
        run();
        let (elapsed, acc) = run();
        assert!(acc.is_finite());
        elapsed.as_secs_f64() / xs.len() as f64
    };
    let small = at(1.0, true);
    let huge = at(1e300, true);
    // What the platform's own answer costs, printed beside it rather than claimed: the ratio is
    // the reason `docs/08` §8.5.4 schedules a fast path in front of this one.
    let platform = at(1.0, false);
    println!(
        "sin costs {:.0} ns at an ordinary argument and {:.0} ns at 10^300; this platform's own \
         libm costs {:.0} ns, so the exact path is {:.0}x it",
        small * 1e9,
        huge * 1e9,
        platform * 1e9,
        small / platform
    );
    assert!(
        huge < small * 5.0,
        "the reduction should be a fixed window rather than a walk down the exponent: \
         {:.0} ns against {:.0} ns",
        huge * 1e9,
        small * 1e9
    );
}

#[test]
fn what_is_not_a_number_has_no_sine() {
    for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(math::sin(x).is_nan(), "sin({x})");
        assert!(math::cos(x).is_nan(), "cos({x})");
    }
}

#[test]
fn a_small_argument_is_its_own_sine_and_a_zero_keeps_its_sign() {
    // `sin x - x` is `x³/6`, which below 2^-27 cannot reach the next double down. The zeros are
    // the case that makes it more than a shortcut: `sin(-0.0)` is `-0.0` in every library there
    // is, and an implementation that returned `0.0` would be visible to `1/sin(x)`.
    assert_eq!(math::sin(0.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(math::sin(-0.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(math::cos(0.0), 1.0);
    assert_eq!(math::cos(-0.0), 1.0);
    for e in 28..=1080 {
        let x = 2f64.powi(-e);
        if x == 0.0 {
            break;
        }
        assert_eq!(math::sin(x), x, "sin(2^-{e})");
        assert_eq!(math::sin(-x), -x, "sin(-2^-{e})");
        assert_eq!(math::cos(x), 1.0, "cos(2^-{e})");
    }
}

#[test]
fn the_answers_at_the_arguments_anybody_can_look_up() {
    // Decimal expansions that predate every implementation of anything here. They are the check on
    // the check: a reference that agreed with the module and with nothing else would be two
    // programs sharing one misunderstanding.
    for (x, want) in [
        (
            1.0f64,
            "0.8414709848078965066525023216302989996225630607983710",
        ),
        (
            0.5f64,
            "0.4794255386042030002732879352155713880818033679406006",
        ),
        (
            2.0f64,
            "0.9092974268256816953960198659117448427022549714478902",
        ),
        (
            100.0f64,
            "-0.5063656411097587936172439237679365261504908445341462",
        ),
    ] {
        assert_eq!(
            math::sin(x).to_bits(),
            nearest(want).to_bits(),
            "sin({x}) against its published expansion"
        );
    }
    for (x, want) in [
        (
            1.0f64,
            "0.5403023058681397174009366074429766037323104206179222",
        ),
        (
            0.5f64,
            "0.8775825618903727161162815826038296519916451971097440",
        ),
        (
            2.0f64,
            "-0.4161468365471423869975682295007621897660007710755448",
        ),
    ] {
        assert_eq!(
            math::cos(x).to_bits(),
            nearest(want).to_bits(),
            "cos({x}) against its published expansion"
        );
    }
}

/// The nearest double to a decimal literal, which is what Rust's own parser answers.
fn nearest(decimal: &str) -> f64 {
    decimal.parse().expect("a decimal expansion")
}

/// The arguments every property here is checked at.
///
/// Four kinds, because they fail in different places: ordinary sized arguments exercise the
/// series, arguments spread over the whole exponent range exercise the window into 2/π at both
/// ends, arguments just above and below a multiple of π/2 are where the reduced argument is small
/// enough for its precision to matter, and a handful of the largest doubles are where a reduction
/// that dropped a limb would show.
fn arguments() -> impl Iterator<Item = f64> {
    let mut state = 0x243F_6A88_85A3_08D3u64;
    let mut i = 0u64;
    std::iter::from_fn(move || {
        loop {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            i += 1;
            let x = match i % 4 {
                0 => (state >> 11) as f64 / (1u64 << 53) as f64 * 6.5,
                1 => (state >> 11) as f64 / (1u64 << 53) as f64 * 1.0e9,
                // A double either side of k·π/2, for a small integer k.
                2 => {
                    let k = (state % 4096) as f64;
                    let near = k * std::f64::consts::FRAC_PI_2;
                    f64::from_bits(near.to_bits() ^ (state >> 60))
                }
                // Anything at all, at any exponent, which includes the very large.
                _ => f64::from_bits(state & 0x7FEF_FFFF_FFFF_FFFF),
            };
            if x.is_finite() && x.abs() >= 1e-8 {
                return Some(x);
            }
        }
    })
}

/// Sine and cosine again, by a different route and at 1408 bits.
///
/// Everything here is a plain big *integer* holding `value · 2^SCALE`, so there is no fixed-point
/// type with a convention to get wrong — a shift is a shift and a comparison is a comparison.
mod reference {
    /// Limbs, most significant first. Wide enough for the largest double shifted up by [`SCALE`],
    /// which is 2432 bits, plus the room binary long division needs above it.
    const N: usize = 42;
    /// Fraction bits. It has to exceed the 1024 the reduction of the largest double consumes by
    /// enough to leave the answer meaningful, and 384 bits of margin is far more than enough.
    const SCALE: u32 = 1408;

    type B = [u64; N];

    pub fn sin_cos(x: f64) -> (f64, f64) {
        let bits = x.to_bits();
        let negative = bits >> 63 == 1;
        let mag = bits & !(1 << 63);
        let (quadrant, neg_r, r) = reduce(mag);
        let (s, c) = series(&r);
        let sin = round(&if quadrant % 2 == 0 { s } else { c }, {
            let n = match quadrant {
                0 => neg_r,
                1 => false,
                2 => !neg_r,
                _ => true,
            };
            n != negative
        });
        let cos = round(
            &if quadrant % 2 == 0 { c } else { s },
            match quadrant {
                0 => false,
                1 => !neg_r,
                2 => true,
                _ => neg_r,
            },
        );
        (sin, cos)
    }

    /// `|x| = n·π/2 + r`, by subtracting shifted copies of π/2 — binary long division, written out.
    fn reduce(mag: u64) -> (u32, bool, B) {
        let m = (mag & ((1 << 52) - 1)) | (1 << 52);
        let e = (mag >> 52) as i32 - 1075;
        let mut v = shl(&from_u64(m), (e + SCALE as i32) as u32);

        let half = half_pi();
        let mut h = half;
        let mut steps = 0u32;
        while cmp(&shl(&h, 1), &v) != std::cmp::Ordering::Greater {
            h = shl(&h, 1);
            steps += 1;
        }
        let mut n = 0u32;
        for j in (0..=steps).rev() {
            if cmp(&h, &v) != std::cmp::Ordering::Greater {
                v = sub(&v, &h);
                // Only the bottom two bits of the quotient survive into the quadrant.
                n = (n + if j == 0 {
                    1
                } else if j == 1 {
                    2
                } else {
                    0
                }) % 4;
            }
            h = shr(&h, 1);
        }
        // `v` is now in `[0, π/2)`; the nearest multiple is the next one up when it is past π/4.
        let quarter = shr(&half, 1);
        if cmp(&v, &quarter) == std::cmp::Ordering::Greater {
            ((n + 1) % 4, true, sub(&half, &v))
        } else {
            (n, false, v)
        }
    }

    /// `(sin r, cos r)` for `0 ≤ r ≤ π/4`, by the term recurrence rather than by a table.
    fn series(r: &B) -> (B, B) {
        let y = mul(r, r);
        let one = shl(&from_u64(1), SCALE);
        let (mut s, mut c) = (*r, one);
        let (mut ts, mut tc) = (*r, one);
        for k in 0u64.. {
            ts = div(&mul(&ts, &y), (2 * k + 2) * (2 * k + 3));
            tc = div(&mul(&tc, &y), (2 * k + 1) * (2 * k + 2));
            let (a, b) = if k % 2 == 0 {
                (sub(&s, &ts), sub(&c, &tc))
            } else {
                (add(&s, &ts), add(&c, &tc))
            };
            s = a;
            c = b;
            // Both series alternate with shrinking terms, so what is left is smaller than the last
            // term taken. 2^-600 is far below what a double can see.
            if bits(&tc) + 600 < SCALE {
                break;
            }
        }
        (s, c)
    }

    /// `b·2^-SCALE`, rounded to the nearest double, ties to even.
    fn round(b: &B, negative: bool) -> f64 {
        let sign = u64::from(negative) << 63;
        let len = bits(b);
        if len == 0 {
            return f64::from_bits(sign);
        }
        let top = len - 1;
        let shift = top as i32 - 52;
        assert!(shift > 0, "the reference does not reach subnormals");
        let mut m = window(b, shift);
        if bit(b, shift - 1) == 1 && (m & 1 == 1 || below(b, shift - 1)) {
            m += 1;
        }
        let mut exponent = shift;
        if m == 1 << 53 {
            m >>= 1;
            exponent += 1;
        }
        let biased = exponent - SCALE as i32 + 1075;
        assert!((1..=2046).contains(&biased), "outside the normal range");
        f64::from_bits(sign | ((biased as u64) << 52) | (m - (1 << 52)))
    }

    /// π, by Bailey, Borwein and Plouffe: `Σ 16^-k (4/(8k+1) - 2/(8k+4) - 1/(8k+5) - 1/(8k+6))`.
    ///
    /// A different formula from the Machin series `beck_prim::math`'s own constants come from, and
    /// a different shape of computation — four small divisions and a shift per term, no
    /// reciprocal and no Newton iteration.
    fn pi() -> B {
        let one = shl(&from_u64(1), SCALE);
        let mut acc = [0u64; N];
        for k in 0u64.. {
            let t = shr(&one, 4 * k as u32);
            if bits(&t) == 0 {
                break;
            }
            acc = add(&acc, &div(&shl(&t, 2), 8 * k + 1));
            acc = sub(&acc, &div(&shl(&t, 1), 8 * k + 4));
            acc = sub(&acc, &div(&t, 8 * k + 5));
            acc = sub(&acc, &div(&t, 8 * k + 6));
        }
        acc
    }

    fn half_pi() -> B {
        use std::sync::OnceLock;
        static ONCE: OnceLock<B> = OnceLock::new();
        *ONCE.get_or_init(|| shr(&pi(), 1))
    }

    // -- the arithmetic, which is a plain big unsigned integer and nothing more ------------------

    fn from_u64(x: u64) -> B {
        let mut b = [0u64; N];
        b[N - 1] = x;
        b
    }

    fn add(a: &B, b: &B) -> B {
        let mut out = [0u64; N];
        let mut carry = 0u128;
        for i in (0..N).rev() {
            let v = a[i] as u128 + b[i] as u128 + carry;
            out[i] = v as u64;
            carry = v >> 64;
        }
        out
    }

    fn sub(a: &B, b: &B) -> B {
        let mut out = [0u64; N];
        let mut borrow = false;
        for i in (0..N).rev() {
            let (v, b1) = a[i].overflowing_sub(b[i]);
            let (v, b2) = v.overflowing_sub(u64::from(borrow));
            out[i] = v;
            borrow = b1 || b2;
        }
        assert!(!borrow, "the reference subtracted below zero");
        out
    }

    fn cmp(a: &B, b: &B) -> std::cmp::Ordering {
        a.iter().cmp(b.iter())
    }

    fn shl(a: &B, k: u32) -> B {
        let (limbs, bits) = ((k / 64) as usize, k % 64);
        let mut out = [0u64; N];
        for (i, limb) in out.iter_mut().enumerate() {
            let from = i + limbs;
            if from >= N {
                continue;
            }
            let mut v = a[from] << bits;
            if bits > 0 && from + 1 < N {
                v |= a[from + 1] >> (64 - bits);
            }
            *limb = v;
        }
        out
    }

    fn shr(a: &B, k: u32) -> B {
        let (limbs, bits) = ((k / 64) as usize, k % 64);
        let mut out = [0u64; N];
        for i in (0..N).rev() {
            if i < limbs {
                continue;
            }
            let from = i - limbs;
            let mut v = a[from] >> bits;
            if bits > 0 && from > 0 {
                v |= a[from - 1] << (64 - bits);
            }
            out[i] = v;
        }
        out
    }

    /// `(a·b) >> SCALE`, which is the product of the two values they stand for.
    fn mul(a: &B, b: &B) -> B {
        let mut wide = vec![0u128; 2 * N + 1];
        for i in 0..N {
            if a[i] == 0 {
                continue;
            }
            for j in 0..N {
                if b[j] == 0 {
                    continue;
                }
                let p = a[i] as u128 * b[j] as u128;
                wide[i + j] += p >> 64;
                wide[i + j + 1] += p & (u64::MAX as u128);
            }
        }
        let mut out = vec![0u64; 2 * N + 1];
        let mut carry = 0u128;
        for k in (0..2 * N + 1).rev() {
            let v = wide[k] + carry;
            out[k] = v as u64;
            carry = v >> 64;
        }
        // `out[k]` carries the place value `2^(64(2N-1-k))`, so the limb the answer starts at is
        // the one whose place is `2^(64(N-1))` after the shift down by SCALE.
        let start = N - SCALE as usize / 64;
        let mut top = [0u64; N];
        top.copy_from_slice(&out[start..start + N]);
        assert!(
            out[..start].iter().all(|&l| l == 0),
            "the reference's multiplication overflowed"
        );
        top
    }

    fn div(a: &B, d: u64) -> B {
        let mut out = [0u64; N];
        let mut rem = 0u128;
        for i in 0..N {
            let cur = (rem << 64) | a[i] as u128;
            out[i] = (cur / d as u128) as u64;
            rem = cur % d as u128;
        }
        out
    }

    /// How many bits the value occupies, counted from its lowest.
    fn bits(a: &B) -> u32 {
        for (i, &limb) in a.iter().enumerate() {
            if limb != 0 {
                return ((N - 1 - i) * 64) as u32 + 64 - limb.leading_zeros();
            }
        }
        0
    }

    fn bit(a: &B, i: i32) -> u64 {
        let q = i.div_euclid(64);
        if q < 0 || q >= N as i32 {
            return 0;
        }
        (a[N - 1 - q as usize] >> i.rem_euclid(64)) & 1
    }

    fn window(a: &B, i: i32) -> u64 {
        let at = |q: i32| -> u64 {
            if q < 0 || q >= N as i32 {
                0
            } else {
                a[N - 1 - q as usize]
            }
        };
        let (q, s) = (i.div_euclid(64), i.rem_euclid(64) as u32);
        if s == 0 {
            at(q)
        } else {
            (at(q) >> s) | (at(q + 1) << (64 - s))
        }
    }

    fn below(a: &B, i: i32) -> bool {
        (0..i).any(|k| bit(a, k) == 1)
    }

    #[cfg(test)]
    mod tests {
        /// The reference's own π, against the expansion every Blowfish implementation carries.
        #[test]
        fn the_reference_computes_pi() {
            let pi = super::pi();
            // `pi` holds `π·2^SCALE`, so the limb above the point is 3 and the two below it are
            // the leading hexadecimal digits.
            let point = super::N - (super::SCALE as usize / 64);
            assert_eq!(pi[point - 1], 3);
            assert_eq!(pi[point], 0x243F_6A88_85A3_08D3);
            assert_eq!(pi[point + 1], 0x1319_8A2E_0370_7344);
            assert_eq!(pi[point + 2], 0xA409_3822_299F_31D0);
        }
    }
}
