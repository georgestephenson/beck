//! Sine and cosine, computed here rather than asked of the host.
//!
//! # Why a language implements these itself
//!
//! `docs/10-decisions.md` D3 rests the whole data tier on one property: folding a log twice gives
//! the same state, on any machine, for as long as the log is kept. Purity does not buy it.
//! IEEE 754 requires `+`, `-`, `*`, `/` and `sqrt` to be correctly rounded and requires **nothing
//! at all** of `sin` and `cos`, so two C libraries — or two versions of one — may answer different
//! last bits for the same double. A fold that computes a sine and reaches the host's libm is a
//! fold whose replay depends on the machine it lands on, which is the one thing D3 needs never to
//! happen.
//!
//! So the answer here is a function of the argument and of nothing else, in two senses:
//!
//! * **No rounded floating-point operation.** The argument is taken apart into the `m` and `e` of
//!   `m·2^e`, everything between is exact integer arithmetic on fixed-point limbs, and the single
//!   rounding is the last step, performed here. Determinism is then not an argument about what a
//!   target contracts, fuses or keeps in a wider register: there is no floating-point arithmetic
//!   to contract.
//! * **Correctly rounded**, so the answer is a fact about the mathematics rather than a fact about
//!   this file. Any other correctly-rounded implementation returns the same bits, which is what
//!   makes replaying an old log a property of the log rather than of the compiler version that
//!   wrote it. A 1-ulp-accurate vendored implementation would be deterministic too, and would
//!   quietly make every future improvement to it a wire-format change.
//!
//! # How
//!
//! Three steps, and the first is where the work is.
//!
//! 1. **Reduction**, exactly, by `reduce`. `|x| = m·2^e` with `m` a 53-bit integer, so
//!    `|x|·2/π` is an integer times a fixed number of bits of `2/π` — `TWO_OVER_PI` is 1472 of
//!    them, and the eight limbs the exponent selects are the only ones that can affect either the
//!    quadrant or the fraction. This is Payne and Hanek's reduction with no floating-point step in
//!    it: `n` is the nearest integer, `f = |x|·2/π − n` is in `[-½, ½]`, and `r = f·π/2` is in
//!    `[-π/4, π/4]`. Nothing here is approximate except the truncation of `2/π`, which is 2^-394
//!    down at the worst exponent.
//! 2. **The series**, by `sine` and `cosine`: `sin r / r` and `cos r` by Horner over the tables below, in
//!    the same fixed point.
//! 3. **The rounding**, by `assemble`: round to nearest, ties to even, and build the `f64`'s
//!    bits.
//!
//! # The error bound, and why it is enough
//!
//! Every step carries an absolute error under 2^-250: the truncation of `2/π` (2^-394), the
//! truncation of the reduced argument (2^-256), one truncation per Horner multiply (30 of them),
//! and one per coefficient. The smallest a sine or cosine of a double can be — other than the
//! arguments below 2^-27, which are answered exactly and never reach here — is about 2^-61, which
//! is the worst case of argument reduction in this format. So the relative error is under 2^-189,
//! where the hardest binary64 argument known needs about 2^-120 to round correctly.
//!
//! That bound is an argument; `beck-prim/tests/transcendentals.rs` is the check. It computes
//! sines and cosines a second time, at 1024 bits, by a deliberately different route — no window
//! into `2/π`, no coefficient table, no Horner — and asserts this module's answer is the nearest
//! double to that one.

/// A non-negative fixed-point number: `l[0]` is the integer part and `l[1..]` are 256 bits of
/// fraction, so the value is `l[0] + Σ l[i]·2^(-64i)`.
///
/// An array rather than a newtype because every operation on it is a free function over limbs, and
/// because the tables below are more readable as literals than as constructor calls.
type Fix = [u64; 5];

const ONE: Fix = [1, 0, 0, 0, 0];

/// The low half of a `u128`.
const LO: u128 = u64::MAX as u128;

/// `|x|` — everything but the sign bit.
const MAG: u64 = !(1 << 63);
/// The bits of `+∞`, which every non-finite magnitude is at or above.
const INFINITY: u64 = 0x7FF0_0000_0000_0000;
/// The bits of 2^-27, below which the series is not needed. See [`sin`].
const TINY: u64 = 0x3E40_0000_0000_0000;

/// π/2, to 256 bits of fraction.
///
/// Together with `TWO_OVER_PI` these are the only two constants in this module that a person could
/// get wrong by typing, so neither was typed:
/// `beck-prim/tests/transcendentals.rs::the_constants_are_what_they_claim_to_be` recomputes π by
/// Machin's formula, checks it against the hexadecimal expansion of π that every Blowfish
/// implementation carries, checks the leading limb against `std::f64::consts::FRAC_PI_2`, and
/// checks that the two constants multiply to one.
#[rustfmt::skip]
const PI_OVER_2: Fix = [0x0000000000000001, 0x921FB54442D18469, 0x898CC51701B839A2, 0x52049C1114CF98E8, 0x04177D4C76273644];

/// The first 1472 bits of the fraction of 2/π.
///
/// Long enough that eight consecutive limbs of it cover the reduction at every exponent a double
/// has — `reduce` is where the two ends of that window are argued.
#[rustfmt::skip]
const TWO_OVER_PI: [u64; 23] = [
    0xA2F9836E4E441529, 0xFC2757D1F534DDC0, 0xDB6295993C439041,
    0xFE5163ABDEBBC561, 0xB7246E3A424DD2E0, 0x06492EEA09D1921C,
    0xFE1DEB1CB129A73E, 0xE88235F52EBB4484, 0xE99C7026B45F7E41,
    0x3991D639835339F4, 0x9C845F8BBDF9283B, 0x1FF897FFDE05980F,
    0xEF2F118B5A0A6D1F, 0x6D367ECF27CB09B7, 0x4F463F669E5FEA2D,
    0x7527BAC7EBE5F17B, 0x3D0739F78A5292EA, 0x6BFB5FB11F8D5D08,
    0x56033046FC7B6BAB, 0xF0CFBC209AF4361D, 0xA9E391615EE61B08,
    0x6599855F14A06840, 0x8DFFD8804D732746,
];

/// `1/(2k+1)!`, the coefficients of `sin r / r`, without their alternating signs.
#[rustfmt::skip]
const SIN_C: [Fix; 30] = [
    [0x0000000000000001, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000],
    [0x0000000000000000, 0x2AAAAAAAAAAAAAAA, 0xAAAAAAAAAAAAAAAA, 0xAAAAAAAAAAAAAAAA, 0xAAAAAAAAAAAAAAAA],
    [0x0000000000000000, 0x0222222222222222, 0x2222222222222222, 0x2222222222222222, 0x2222222222222222],
    [0x0000000000000000, 0x000D00D00D00D00D, 0x00D00D00D00D00D0, 0x0D00D00D00D00D00, 0xD00D00D00D00D00D],
    [0x0000000000000000, 0x00002E3BC74AAD8E, 0x671F5583911CA002, 0xE3BC74AAD8E671F5, 0x583911CA002E3BC7],
    [0x0000000000000000, 0x0000006B99159FD5, 0x138E3F9D1F92E0DF, 0x71C7880ADCBC46DA, 0xAAB1643C04A7FBE3],
    [0x0000000000000000, 0x00000000B092309D, 0x43684BE51C198E91, 0xD7B4269D9BABDFA2, 0x38E3994206980D1A],
    [0x0000000000000000, 0x0000000000D73F9F, 0x399DC0F88EC32B58, 0x774657F48F5EAF63, 0x83ED943C0C38CCDC],
    [0x0000000000000000, 0x000000000000CA96, 0x3B81856A53593028, 0xCBBB8D7FF53BA468, 0xD621D08B83CF4484],
    [0x0000000000000000, 0x0000000000000097, 0xA4DA340A0AB92650, 0xF61DBDCB3A5ABF5B, 0xA0D03143C6BF7BB5],
    [0x0000000000000000, 0x0000000000000000, 0x5C6E3BDB73D5C62F, 0xBC51BF3B9B914861, 0x21E81D5FDB4AD15C],
    [0x0000000000000000, 0x0000000000000000, 0x002EC368262C7033, 0xB2F70E09BAFEC4F3, 0x09ADFE08CA4C8B09],
    [0x0000000000000000, 0x0000000000000000, 0x000013F3CCDD165F, 0xA8D4E44A419776F1, 0x0B893FFF294C1301],
    [0x0000000000000000, 0x0000000000000000, 0x0000000746AC70B7, 0x33A8C82A6863C575, 0x09DCD281D2798B54],
    [0x0000000000000000, 0x0000000000000000, 0x00000000024B3F31, 0x686B15AF57C61CEE, 0xCDE2523ACCDEBB20],
    [0x0000000000000000, 0x0000000000000000, 0x000000000000A1A6, 0x973C1FADE2170F72, 0x37D35FE1C89DB179],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000027, 0x3024A9BA1AA36A70, 0x59BFF52E844FAA1B],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x086E2CE38B6C8F94, 0x19E3FAD3F0311D9D],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x00019EC8D1C94E85, 0xAF4C78B15C3D89D2],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x00000047A6512692, 0xEB37804111DABAD3],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x000000000B2F30E1, 0xCE812063F12E7E8D],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x00000000000195DB, 0x45257E5122DCBAE5],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000034, 0x7970E4440C8F1C05],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0636A382849FAE6D],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000AD21786FF584],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000001161872BF7],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x00000000019D4F10],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x00000000000023A0],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000002],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000],
];

/// `1/(2k)!`, the coefficients of `cos r`, without their alternating signs.
///
/// The tail of both tables is zero: `1/59!` is below 2^-256, so a coefficient that far down has
/// nothing left to say in this fixed point. They are kept at a common length because the Horner
/// loop walks both, and a table that stopped where its own coefficients vanished would make the
/// two lengths a thing to keep in agreement.
#[rustfmt::skip]
const COS_C: [Fix; 30] = [
    [0x0000000000000001, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000],
    [0x0000000000000000, 0x8000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000],
    [0x0000000000000000, 0x0AAAAAAAAAAAAAAA, 0xAAAAAAAAAAAAAAAA, 0xAAAAAAAAAAAAAAAA, 0xAAAAAAAAAAAAAAAA],
    [0x0000000000000000, 0x005B05B05B05B05B, 0x05B05B05B05B05B0, 0x5B05B05B05B05B05, 0xB05B05B05B05B05B],
    [0x0000000000000000, 0x0001A01A01A01A01, 0xA01A01A01A01A01A, 0x01A01A01A01A01A0, 0x1A01A01A01A01A01],
    [0x0000000000000000, 0x0000049F93EDDE27, 0xD71CBBC05B4FA999, 0xE392D8777C170B65, 0x559F4E943337D2C7],
    [0x0000000000000000, 0x00000008F76C77FC, 0x6C4BDAA26D4C3D67, 0xF425F600E7BA5B3C, 0xE38EC85A55B8AA52],
    [0x0000000000000000, 0x000000000C9CBA54, 0x603E4E905D6F8A2E, 0xFD1F2754668C46D4, 0xBAEBAF84B75400EF],
    [0x0000000000000000, 0x00000000000D73F9, 0xF399DC0F88EC32B5, 0x8774657F48F5EAF6, 0x383ED943C0C38CCD],
    [0x0000000000000000, 0x0000000000000B41, 0x3C31DCBECBBDD802, 0x4435161554BC33CC, 0xEF73A807C0362E79],
    [0x0000000000000000, 0x0000000000000007, 0x950AE900808941EA, 0x72B4AFE3C2EAEFF7, 0xC80A68DCFD232C95],
    [0x0000000000000000, 0x0000000000000000, 0x04338E5B6DFE14A5, 0x143242DFCCE3B1D5, 0xDEA1D2CA2CE07DE1],
    [0x0000000000000000, 0x0000000000000000, 0x0001F2CF01972F57, 0x7CCA4B4067CA9D8A, 0x20673FEB086DDB20],
    [0x0000000000000000, 0x0000000000000000, 0x000000C4742FE352, 0x72CD1C790285D358, 0x0A4A33B132D1B1F6],
    [0x0000000000000000, 0x0000000000000000, 0x0000000042862898, 0xD42174DCF171470D, 0x52A350A9353B32B0],
    [0x0000000000000000, 0x0000000000000000, 0x000000000013932C, 0x5047D60E60CADED4, 0xC2989C574B187DB4],
    [0x0000000000000000, 0x0000000000000000, 0x000000000000050D, 0x34B9E0FD6F10B87B, 0x91BE9AFF0E44ED8B],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000001, 0x2710231C0FD7A13F, 0x8A2B4AF9D6B70C88],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x003BF30652185952, 0x560D71A254E4EB7D],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x00000AEA565CE061, 0xD57489E9B8527627],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x00000001CA8ED42A, 0x12AE3001A07244AB],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000442BD4, 0x9D4C37A0DB136489],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000939, 0x58D81FF63527ECF9],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000001, 0x240804F65951062C],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x002123680D6DFE4C],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x000003766DEDC259],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000055915E62],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x000000000007A763],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x00000000000000A2],
    [0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000, 0x0000000000000000],
];

// ---------------------------------------------------------------------------------------------
// The two functions, and their three special cases.
// ---------------------------------------------------------------------------------------------

/// The sine of `x`, correctly rounded.
///
/// `NaN` for an argument that is not finite, which is what every C library answers and what the
/// evaluator answered when it was one: a sine of infinity has no value to be right about.
///
/// Below 2^-27 the answer is `x` itself, sign and all, and that is exact rather than a shortcut:
/// `x - sin x = x³/6` is under `x·2^-56` there, and the gap from `x` down to the midpoint below it
/// is at least `x·2^-54`, so the true sine cannot reach the next double. It is here because the
/// fixed point below has 256 bits of *absolute* precision and a subnormal argument has none of
/// them.
pub fn sin(x: f64) -> f64 {
    let bits = x.to_bits();
    let mag = bits & MAG;
    if mag >= INFINITY {
        return f64::NAN;
    }
    if mag < TINY {
        return x;
    }
    let (quadrant, neg_r, r) = reduce(mag);
    // sin(n·π/2 + r), by n mod 4. `r` carries its own sign, and the sine is odd in both `r` and
    // in `x`, so the two negations meet in the last line. Only one of the two series is ever
    // wanted, and which one is known before either is run.
    let (v, neg) = match quadrant {
        0 => (sine(&r), neg_r),
        1 => (cosine(&r), false),
        2 => (sine(&r), !neg_r),
        _ => (cosine(&r), true),
    };
    assemble(&v, neg != (bits >> 63 == 1))
}

/// The cosine of `x`, correctly rounded.
///
/// `NaN` for an argument that is not finite, and exactly `1.0` below 2^-27 — where `1 - cos x` is
/// under 2^-55 and the midpoint below `1.0` is 2^-54 away, so no argument that small has a cosine
/// that rounds anywhere else.
pub fn cos(x: f64) -> f64 {
    let bits = x.to_bits();
    let mag = bits & MAG;
    if mag >= INFINITY {
        return f64::NAN;
    }
    if mag < TINY {
        return 1.0;
    }
    let (quadrant, neg_r, r) = reduce(mag);
    // cos(n·π/2 + r), by n mod 4. The cosine is even in `x`, so nothing here reads its sign.
    let (v, neg) = match quadrant {
        0 => (cosine(&r), false),
        1 => (sine(&r), !neg_r),
        2 => (cosine(&r), true),
        _ => (sine(&r), neg_r),
    };
    assemble(&v, neg)
}

// ---------------------------------------------------------------------------------------------
// Reduction.
// ---------------------------------------------------------------------------------------------

/// Reduce the magnitude `mag` of a normal double to `(n mod 4, r < 0, |r|)`, where
/// `|x| = n·π/2 + r` and `|r| ≤ π/4`.
///
/// The whole of it is integer arithmetic on a window of [`TWO_OVER_PI`], which is what makes it
/// exact. `|x| = m·2^e`, so the product `|x|·2/π` is `m` times a bit string, and two facts bound
/// how much of that string is needed:
///
/// * A limb whose contribution is a multiple of 4 changes neither `n mod 4` nor the fraction, so
///   everything above `j0` is dropped rather than computed and discarded.
/// * A limb 512 bits below that contributes under 2^-394 even at the largest exponent, which is
///   far below the 2^-256 the fraction is kept to.
///
/// Eight limbs sit between those two lines at every exponent a double has.
fn reduce(mag: u64) -> (u32, bool, Fix) {
    // A normal double: the implicit bit is put back, and the exponent is the one that makes
    // `m·2^e` the value rather than the one the format stores. Subnormals never arrive — `sin`
    // and `cos` answer everything below 2^-27 without coming here.
    let m = (mag & ((1 << 52) - 1)) | (1 << 52);
    let e = (mag >> 52) as i32 - 1075;

    // The first limb that can still matter, and the eight from there.
    let j0 = if e >= 2 { ((e - 2) / 64) as usize } else { 0 };
    let mut p = [0u64; 9];
    let mut carry = 0u128;
    for i in (0..8).rev() {
        let v = m as u128 * TWO_OVER_PI[j0 + i] as u128 + carry;
        p[i + 1] = v as u64;
        carry = v >> 64;
    }
    p[0] = carry as u64;
    // `p` is an integer; the value it stands for is `p·2^-t`, so bit `t` of it is the ones place.
    let t = 64 * (j0 as i32 + 8) - e;

    // The quadrant, and the fraction below the binary point. Reading a bit outside `p` answers
    // zero, which is what makes the small-exponent end need no case of its own.
    let n = (bit(&p, t) | (bit(&p, t + 1) << 1)) as u32;
    let mut frac: Fix = [0; 5];
    for (i, limb) in frac.iter_mut().enumerate().skip(1) {
        *limb = word(&p, t - 64 * i as i32);
    }

    // Round to nearest: the bit immediately below the point says which side of the integer the
    // argument falls, and reading it from `p` rather than from `frac` makes the test exact.
    if bit(&p, t - 1) == 1 {
        ((n + 1) & 3, true, mul(&sub(&ONE, &frac), &PI_OVER_2))
    } else {
        (n & 3, false, mul(&frac, &PI_OVER_2))
    }
}

/// Bit `i` of a big-endian limb array, counted from the least significant bit of its last limb.
///
/// Out of range is zero. That is not a defensive answer — it is the arithmetic one, since the
/// array is a finite window on a value whose other bits are not there — and it is what saves the
/// callers a case at each end.
fn bit(l: &[u64], i: i32) -> u64 {
    let q = i.div_euclid(64);
    if q < 0 || q >= l.len() as i32 {
        return 0;
    }
    (l[l.len() - 1 - q as usize] >> i.rem_euclid(64)) & 1
}

/// The 64 bits of a big-endian limb array starting at bit `i`, on [`bit`]'s numbering.
fn word(l: &[u64], i: i32) -> u64 {
    let at = |q: i32| -> u64 {
        if q < 0 || q >= l.len() as i32 {
            0
        } else {
            l[l.len() - 1 - q as usize]
        }
    };
    let q = i.div_euclid(64);
    let s = i.rem_euclid(64) as u32;
    if s == 0 {
        at(q)
    } else {
        (at(q) >> s) | (at(q + 1) << (64 - s))
    }
}

// ---------------------------------------------------------------------------------------------
// The series.
// ---------------------------------------------------------------------------------------------

/// `sin r` for `0 ≤ r ≤ π/4`, by Horner over [`SIN_C`].
fn sine(r: &Fix) -> Fix {
    let y = mul(r, r);
    mul(r, &horner(&SIN_C, &y))
}

/// `cos r` for `0 ≤ r ≤ π/4`, by Horner over [`COS_C`].
fn cosine(r: &Fix) -> Fix {
    let y = mul(r, r);
    horner(&COS_C, &y)
}

/// `Σ (-1)^k c[k]·y^k`, by Horner.
///
/// The coefficients are `1/(2k+1)!` and `1/(2k)!` with their signs left out, because the
/// alternation is the subtraction here and a signed fixed point would be a sign bit to carry
/// through every operation for one use. Each step subtracts `y·p` from a coefficient at least six
/// times larger than the one below it, so no partial result is ever negative.
///
/// Horner rather than a term recurrence because a recurrence divides by `(2k+2)(2k+3)` at every
/// step, and a 128-by-64 division is a called-out routine on every target this compiles for: the
/// tables trade 2 KiB of constants for a loop that only multiplies.
fn horner(c: &[Fix], y: &Fix) -> Fix {
    let mut p = c[c.len() - 1];
    for coefficient in c[..c.len() - 1].iter().rev() {
        p = sub(coefficient, &mul(y, &p));
    }
    p
}

// ---------------------------------------------------------------------------------------------
// Fixed-point arithmetic. Truncating, and the module documentation is where that is paid for.
// ---------------------------------------------------------------------------------------------

/// `a - b`, which every caller here knows to be non-negative.
fn sub(a: &Fix, b: &Fix) -> Fix {
    let mut out = [0u64; 5];
    let mut borrow = false;
    for i in (0..5).rev() {
        let (v, b1) = a[i].overflowing_sub(b[i]);
        let (v, b2) = v.overflowing_sub(u64::from(borrow));
        out[i] = v;
        borrow = b1 || b2;
    }
    debug_assert!(!borrow, "fixed-point subtraction went below zero");
    out
}

/// `a · b`, truncated to the same 256 fraction bits.
///
/// The high half of `a[i]·b[j]` carries the place value of limb `i+j-1` and the low half that of
/// `i+j`, so the accumulator is indexed one above the result and its first entry is the overflow
/// past the integer part — which nothing in this module produces, and which a debug build says so
/// about.
fn mul(a: &Fix, b: &Fix) -> Fix {
    // Seven places rather than ten: a partial product below `acc[6]` reaches the result only as a
    // carry into its last limb, and is worth under 2^-320 there — two orders below the truncation
    // this function already performs, and the module's error bound covers both together.
    let mut acc = [0u128; 7];
    for (i, &ai) in a.iter().enumerate() {
        if ai == 0 {
            continue;
        }
        for (j, &bj) in b.iter().enumerate() {
            if i + j > 6 {
                break;
            }
            let p = ai as u128 * bj as u128;
            acc[i + j] += p >> 64;
            if i + j < 6 {
                acc[i + j + 1] += p & LO;
            }
        }
    }
    let mut out = [0u64; 7];
    let mut carry = 0u128;
    for k in (0..7).rev() {
        let v = acc[k] + carry;
        out[k] = v as u64;
        carry = v >> 64;
    }
    debug_assert_eq!(carry, 0);
    debug_assert_eq!(out[0], 0, "fixed-point multiplication overflowed");
    let mut r = [0u64; 5];
    r.copy_from_slice(&out[1..6]);
    r
}

// ---------------------------------------------------------------------------------------------
// Rounding.
// ---------------------------------------------------------------------------------------------

/// Round a fixed-point magnitude to the nearest double, ties to even, and give it `neg`'s sign.
///
/// The bits are assembled rather than multiplied out, so the last step of a function that performs
/// no floating-point arithmetic does not perform one either.
fn assemble(v: &Fix, neg: bool) -> f64 {
    let sign = u64::from(neg) << 63;
    // The value is the 320-bit integer these limbs spell, times 2^-256.
    let mut top: i32 = -1;
    for (i, &limb) in v.iter().enumerate() {
        if limb != 0 {
            top = (4 - i as i32) * 64 + (63 - limb.leading_zeros() as i32);
            break;
        }
    }
    if top < 0 {
        return f64::from_bits(sign);
    }
    // 53 significant bits, then the bit below them and whatever is below that.
    let shift = top - 52;
    debug_assert!(shift > 0, "a sine this small does not come out of `reduce`");
    let mut m = word(v, shift);
    if bit(v, shift - 1) == 1 && (m & 1 == 1 || any_below(v, shift - 1)) {
        m += 1;
    }
    let mut exponent = shift;
    if m == 1 << 53 {
        m >>= 1;
        exponent += 1;
    }
    // `m·2^(exponent-256)` with `m` in `[2^52, 2^53)`, which is the format's own decomposition.
    let biased = exponent - 256 + 1075;
    debug_assert!((1..=2046).contains(&biased), "outside the normal range");
    f64::from_bits(sign | ((biased as u64) << 52) | (m - (1 << 52)))
}

/// Whether any bit strictly below `i` is set, on [`bit`]'s numbering — the sticky bit that decides
/// a tie.
fn any_below(l: &[u64], i: i32) -> bool {
    if i <= 0 {
        return false;
    }
    let n = l.len();
    let q = (i / 64) as usize;
    for k in 0..q.min(n) {
        if l[n - 1 - k] != 0 {
            return true;
        }
    }
    let s = (i % 64) as u32;
    q < n && s > 0 && l[n - 1 - q] & ((1 << s) - 1) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wide enough for [`TWO_OVER_PI`] with an integer limb in front of it.
    const W: usize = 24;
    type Big = [u64; W];

    /// The two constants, recomputed rather than compared against a copy of themselves.
    ///
    /// π comes out of Machin's `16·atan(1/5) - 4·atan(1/239)`, checked against the hexadecimal
    /// expansion of π that every Blowfish implementation carries as its P-array — a published
    /// constant older than anything here and derived from neither of these. 2/π then comes out of
    /// a Newton iteration, which is a different computation again, and the product of the two is
    /// what says they agree.
    #[test]
    fn the_constants_are_what_they_claim_to_be() {
        let pi = big_sub(
            &small_mul(&arctan_inverse(5), 16),
            &small_mul(&arctan_inverse(239), 4),
        );
        assert_eq!(
            pi[..10],
            [
                3,
                0x243F_6A88_85A3_08D3,
                0x1319_8A2E_0370_7344,
                0xA409_3822_299F_31D0,
                0x082E_FA98_EC4E_6C89,
                0x4528_21E6_38D0_1377,
                0xBE54_66CF_34E9_0C6C,
                0xC0AC_29B7_C97C_50DD,
                0x3F84_D5B5_B547_0917,
                0x9216_D5D9_8979_FB1B,
            ],
            "Machin's formula and the published expansion of π disagree"
        );

        let half = shift_right(&pi);
        assert_eq!(
            PI_OVER_2[..],
            half[..5],
            "PI_OVER_2 is not the leading limbs of π/2"
        );
        // Its leading limb, read as a double, is the constant the standard library ships.
        assert_eq!(
            f64::from_bits(0x3FF0_0000_0000_0000 | (half[1] >> 12)),
            std::f64::consts::FRAC_PI_2
        );

        // 2/π by Newton: `y ← y·(2 - (π/2)·y)`, from five decimal digits of a starting guess.
        let mut y = small_div(&from_u64(63662), 100_000);
        for _ in 0..12 {
            y = big_mul(&y, &big_sub(&from_u64(2), &big_mul(&half, &y)));
        }
        assert_eq!(y[0], 0, "2/π has no integer part");
        assert_eq!(y[1..], TWO_OVER_PI[..], "TWO_OVER_PI is not 2/π");

        // And the two together: a truncating multiplication makes this all ones rather than a
        // leading one, which is the whole of the difference between them.
        let product = big_mul(&half, &y);
        assert_eq!(product[0], 0);
        assert!(
            product[1..W - 1].iter().all(|&l| l == u64::MAX),
            "π/2 and 2/π do not multiply to one"
        );
    }

    /// The series coefficients, against the factorials they are.
    #[test]
    fn the_series_coefficients_are_the_reciprocals_of_the_factorials() {
        let mut sin = from_u64(1);
        let mut cos = from_u64(1);
        for k in 0..SIN_C.len() as u64 {
            if k > 0 {
                sin = small_div(&small_div(&sin, 2 * k), 2 * k + 1);
                cos = small_div(&small_div(&cos, 2 * k - 1), 2 * k);
            }
            assert_eq!(SIN_C[k as usize][..], sin[..5], "1/{}!", 2 * k + 1);
            assert_eq!(COS_C[k as usize][..], cos[..5], "1/{}!", 2 * k);
        }
        // The tail has to be zero, or the series would stop above the precision it claims.
        assert_eq!(SIN_C[SIN_C.len() - 1], [0; 5]);
        assert_eq!(COS_C[COS_C.len() - 1], [0; 5]);
    }

    /// `atan(1/n)`, by its alternating series, in the same fixed point.
    fn arctan_inverse(n: u64) -> Big {
        let mut power = small_div(&from_u64(1), n);
        let mut sum = [0u64; W];
        for k in 0u64.. {
            let term = small_div(&power, 2 * k + 1);
            if term.iter().all(|&l| l == 0) {
                return sum;
            }
            sum = if k % 2 == 0 {
                big_add(&sum, &term)
            } else {
                big_sub(&sum, &term)
            };
            power = small_div(&power, n * n);
        }
        unreachable!()
    }

    fn from_u64(x: u64) -> Big {
        let mut b = [0u64; W];
        b[0] = x;
        b
    }

    fn big_add(a: &Big, b: &Big) -> Big {
        let mut out = [0u64; W];
        let mut carry = 0u128;
        for i in (0..W).rev() {
            let v = a[i] as u128 + b[i] as u128 + carry;
            out[i] = v as u64;
            carry = v >> 64;
        }
        out
    }

    fn big_sub(a: &Big, b: &Big) -> Big {
        let mut out = [0u64; W];
        let mut borrow = false;
        for i in (0..W).rev() {
            let (v, b1) = a[i].overflowing_sub(b[i]);
            let (v, b2) = v.overflowing_sub(u64::from(borrow));
            out[i] = v;
            borrow = b1 || b2;
        }
        assert!(!borrow);
        out
    }

    fn big_mul(a: &Big, b: &Big) -> Big {
        let mut acc = [0u128; 2 * W + 2];
        for (i, &ai) in a.iter().enumerate() {
            for (j, &bj) in b.iter().enumerate() {
                let p = ai as u128 * bj as u128;
                acc[i + j] += p >> 64;
                acc[i + j + 1] += p & LO;
            }
        }
        let mut out = [0u64; 2 * W + 2];
        let mut carry = 0u128;
        for k in (0..2 * W + 2).rev() {
            let v = acc[k] + carry;
            out[k] = v as u64;
            carry = v >> 64;
        }
        assert_eq!(out[0], 0);
        let mut r = [0u64; W];
        r.copy_from_slice(&out[1..W + 1]);
        r
    }

    fn small_mul(a: &Big, m: u64) -> Big {
        let mut out = [0u64; W];
        let mut carry = 0u128;
        for i in (0..W).rev() {
            let v = a[i] as u128 * m as u128 + carry;
            out[i] = v as u64;
            carry = v >> 64;
        }
        assert_eq!(carry, 0);
        out
    }

    fn small_div(a: &Big, d: u64) -> Big {
        let mut out = [0u64; W];
        let mut rem = 0u128;
        for i in 0..W {
            let cur = (rem << 64) | a[i] as u128;
            out[i] = (cur / d as u128) as u64;
            rem = cur % d as u128;
        }
        out
    }

    fn shift_right(a: &Big) -> Big {
        let mut out = [0u64; W];
        let mut carry = 0u64;
        for i in 0..W {
            out[i] = (a[i] >> 1) | (carry << 63);
            carry = a[i] & 1;
        }
        out
    }
}
