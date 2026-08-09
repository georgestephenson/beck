// Beck's integer semantics exactly: `checked_*`, in a safe language.
use std::hint::black_box;
use std::time::Instant;

fn add(a: i64, b: i64) -> i64 { a.checked_add(b).expect("overflow") }
fn sub(a: i64, b: i64) -> i64 { a.checked_sub(b).expect("overflow") }
fn mul(a: i64, b: i64) -> i64 { a.checked_mul(b).expect("overflow") }
fn div(a: i64, b: i64) -> i64 { a.checked_div(b).expect("overflow") }
fn rem(a: i64, b: i64) -> i64 { a.checked_rem(b).expect("overflow") }

fn fib(n: i64) -> i64 { if n < 2 { n } else { add(fib(sub(n,1)), fib(sub(n,2))) } }

fn sum_to(mut n: i64, mut acc: i64) -> i64 {
    while n > 0 { acc = add(acc, n); n = sub(n, 1); }
    acc
}

fn escapes(cr: f64, ci: f64) -> i64 {
    let (mut zrzr, mut zi, mut zizi) = (0.0f64, 0.0f64, 0.0f64);
    for _ in 0..50 {
        let zr = zrzr - zizi + cr;
        let nzi = 2.0 * zr * zi + ci;
        let nzrzr = zr * zr;
        let nzizi = nzi * nzi;
        if nzrzr + nzizi > 4.0 { return 1; }
        zrzr = nzrzr; zi = nzi; zizi = nzizi;
    }
    0
}

fn image(size: i64) -> i64 {
    let mut acc = 0i64;
    for y in 0..size {
        let ci = 2.0 * y as f64 / size as f64 - 1.0;
        for x in 0..size {
            let cr = 2.0 * x as f64 / size as f64 - 1.5;
            acc = add(acc, escapes(cr, ci));
        }
    }
    acc
}

fn xor_from(mut a: i64, mut b: i64, mut weight: i64, mut acc: i64) -> i64 {
    while !(a == 0 && b == 0) {
        acc = add(acc, mul(weight, rem(add(rem(a,2), rem(b,2)), 2)));
        a = div(a,2); b = div(b,2); weight = mul(weight,2);
    }
    acc
}

fn xor_sweep(mut n: i64, mut acc: i64) -> i64 {
    while n > 0 {
        acc = add(acc, xor_from(rem(n,256), rem(mul(n,7),256), 1, 0));
        n = sub(n,1);
    }
    acc
}

fn report(name: &str, mut ts: Vec<f64>, answer: i64) {
    ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!("{name}\t{:.4}\t{answer}", ts[ts.len()/2]);
}

fn main() {
    let a: Vec<i64> = std::env::args().skip(1).map(|s| s.parse().unwrap()).collect();
    let (nfib, nsum, nimg, nxor) = (a[0], a[1], a[2], a[3]);
    const RUNS: usize = 11;
    let mut out = 0i64;

    let mut ts = Vec::new();
    for _ in 0..RUNS { let s = Instant::now(); out = fib(black_box(nfib)); ts.push(s.elapsed().as_secs_f64()*1e3); }
    report("fib", ts, out);
    let mut ts = Vec::new();
    for _ in 0..RUNS { let s = Instant::now(); out = sum_to(black_box(nsum), 0); ts.push(s.elapsed().as_secs_f64()*1e3); }
    report("sum_to", ts, out);
    let mut ts = Vec::new();
    for _ in 0..RUNS { let s = Instant::now(); out = image(black_box(nimg)); ts.push(s.elapsed().as_secs_f64()*1e3); }
    report("image", ts, out);
    let mut ts = Vec::new();
    for _ in 0..RUNS { let s = Instant::now(); out = xor_sweep(black_box(nxor), 0); ts.push(s.elapsed().as_secs_f64()*1e3); }
    report("xor_sweep", ts, out);
}
