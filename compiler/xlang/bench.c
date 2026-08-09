/* The four benchmarks of measure_native.rs, ported faithfully.
 *
 * `sum_to`, `xor_sweep`, `rows`, `columns` and `escapes` are tail-recursive in the Beck source and
 * are loops here — that is what a tail call is, and every port below does the same, so the
 * comparison is about the arithmetic rather than about who has proper tail calls. `fib` stays
 * recursive, because tree recursion is the point of that one.
 *
 * CHECKED=1 gives Beck's integer semantics: an operation with no representable answer fails
 * instead of wrapping. Inline functions rather than macros — `int64_t n = f(n)` reads the
 * uninitialised `n`, which is what a macro that declares a temporary does when it nests.
 *
 * The sizes come from argv so the optimiser cannot fold a pure function of a literal to its answer,
 * which is what it did to `fib` and `sum_to` the first time.  */
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <time.h>

static int trapped = 0;

#if CHECKED
static inline int64_t ADD(int64_t a, int64_t b) { int64_t r; if (__builtin_add_overflow(a,b,&r)) trapped=1; return r; }
static inline int64_t SUB(int64_t a, int64_t b) { int64_t r; if (__builtin_sub_overflow(a,b,&r)) trapped=1; return r; }
static inline int64_t MUL(int64_t a, int64_t b) { int64_t r; if (__builtin_mul_overflow(a,b,&r)) trapped=1; return r; }
static inline int64_t DIV(int64_t a, int64_t b) { if (b==0 || (a==INT64_MIN && b==-1)) { trapped=1; return 0; } return a/b; }
static inline int64_t REM(int64_t a, int64_t b) { if (b==0 || (a==INT64_MIN && b==-1)) { trapped=1; return 0; } return a%b; }
#else
static inline int64_t ADD(int64_t a, int64_t b) { return a+b; }
static inline int64_t SUB(int64_t a, int64_t b) { return a-b; }
static inline int64_t MUL(int64_t a, int64_t b) { return a*b; }
static inline int64_t DIV(int64_t a, int64_t b) { return a/b; }
static inline int64_t REM(int64_t a, int64_t b) { return a%b; }
#endif

static int64_t fib(int64_t n) {
    if (n < 2) return n;
    return ADD(fib(SUB(n,1)), fib(SUB(n,2)));
}

static int64_t sum_to(int64_t n, int64_t acc) {
    while (n > 0) { acc = ADD(acc, n); n = SUB(n, 1); }
    return acc;
}

static int64_t escapes(double cr, double ci) {
    double zrzr = 0.0, zi = 0.0, zizi = 0.0;
    for (int64_t z = 0; z < 50; z++) {
        double zr = zrzr - zizi + cr;
        double nzi = 2.0 * zr * zi + ci;
        double nzrzr = zr * zr;
        double nzizi = nzi * nzi;
        if (nzrzr + nzizi > 4.0) return 1;
        zrzr = nzrzr; zi = nzi; zizi = nzizi;
    }
    return 0;
}

static int64_t image(int64_t size) {
    int64_t acc = 0;
    for (int64_t y = 0; y < size; y++) {
        double ci = 2.0 * (double)y / (double)size - 1.0;
        for (int64_t x = 0; x < size; x++) {
            double cr = 2.0 * (double)x / (double)size - 1.5;
            acc = ADD(acc, escapes(cr, ci));
        }
    }
    return acc;
}

static int64_t xor_from(int64_t a, int64_t b, int64_t weight, int64_t acc) {
    while (!(a == 0 && b == 0)) {
        acc = ADD(acc, MUL(weight, REM(ADD(REM(a,2), REM(b,2)), 2)));
        a = DIV(a,2); b = DIV(b,2); weight = MUL(weight,2);
    }
    return acc;
}

static int64_t xor_sweep(int64_t n, int64_t acc) {
    while (n > 0) {
        acc = ADD(acc, xor_from(REM(n,256), REM(MUL(n,7),256), 1, 0));
        n = SUB(n,1);
    }
    return acc;
}

static double now_ms(void) {
    struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e3 + t.tv_nsec / 1e6;
}

static int cmp(const void *a, const void *b) {
    double x = *(const double*)a, y = *(const double*)b;
    return (x > y) - (x < y);
}

#define RUNS 11
static void report(const char *name, double *ts, int64_t answer) {
    qsort(ts, RUNS, sizeof(double), cmp);
    printf("%s\t%.4f\t%lld\n", name, ts[RUNS/2], (long long)answer);
}

int main(int argc, char **argv) {
    /* Opaque to the optimiser. */
    if (argc != 5) { fprintf(stderr, "usage: %s <fib> <sum_to> <image> <xor_sweep>\n", argv[0]); return 2; }
    int64_t nfib = atoll(argv[1]), nsum = atoll(argv[2]);
    int64_t nimg = atoll(argv[3]), nxor = atoll(argv[4]);
    double ts[RUNS]; int64_t a = 0; double s;
    /* `volatile` per iteration, because these functions are pure and their argument is loop
       invariant: without it the optimiser hoists the call out of the timing loop and four of the
       five runs measure an empty block. That is what the first version of this file did, and the
       tell was `fib(30)` in 0.0000 ms. */
    volatile int64_t v;

    for (int i = 0; i < RUNS; i++) { v = nfib; s = now_ms(); a = fib(v);          ts[i] = now_ms() - s; }
    report("fib", ts, a);
    for (int i = 0; i < RUNS; i++) { v = nsum; s = now_ms(); a = sum_to(v, 0);    ts[i] = now_ms() - s; }
    report("sum_to", ts, a);
    for (int i = 0; i < RUNS; i++) { v = nimg; s = now_ms(); a = image(v);        ts[i] = now_ms() - s; }
    report("image", ts, a);
    for (int i = 0; i < RUNS; i++) { v = nxor; s = now_ms(); a = xor_sweep(v, 0); ts[i] = now_ms() - s; }
    report("xor_sweep", ts, a);
    if (trapped) printf("TRAPPED\n");
    return 0;
}
