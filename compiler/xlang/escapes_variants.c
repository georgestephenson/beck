/* When Beck's mandelbrot is slower than C's, is it the code generation or the *semantics*?
 *
 * This is the diagnostic that answered it once and should answer it again whenever the number
 * moves. Four variants of one loop, all in C so the compiler is held fixed and only the semantics
 * change:
 *
 *   plain  — IEEE doubles and `>`, which is what every other language in the table does.
 *   key    — the comparison through `beck_core`'s order key instead of `>`. **What beck-llvm
 *            emits today**: a real is normalised where a signed zero or a NaN is observable, which
 *            for this loop is the one comparison.
 *   norm   — every float *result* normalised as well. What beck-llvm emitted before docs/93 §93.5,
 *            and the 3x it cost is why it does not any more.
 *   both   — norm and key together, which is the same thing plus the comparison.
 *
 * If `key` lands on Beck's number, the code generation is at parity with clang and what is left is
 * the price of docs/32 §32.2's structural equality on reals.  */
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static inline double norm(double x) { return x == 0.0 ? 0.0 : x; }

static inline int64_t key(double x) {
    int64_t b; memcpy(&b, &x, 8);
    int64_t mask = (b >> 63) | INT64_MIN;
    return b ^ mask;
}

#define BODY(N, GT)                                                        \
    double zrzr = 0.0, zi = 0.0, zizi = 0.0;                               \
    for (int64_t z = 0; z < 50; z++) {                                     \
        double zr    = N(N(zrzr - zizi) + cr);                             \
        double nzi   = N(N(N(2.0 * zr) * zi) + ci);                        \
        double nzrzr = N(zr * zr);                                         \
        double nzizi = N(nzi * nzi);                                       \
        if (GT(N(nzrzr + nzizi), 4.0)) return 1;                           \
        zrzr = nzrzr; zi = nzi; zizi = nzizi;                              \
    }                                                                      \
    return 0;

#define ID(x) (x)
#define GT_PLAIN(a,b) ((a) > (b))
#define GT_KEY(a,b)   ((uint64_t)key(a) > (uint64_t)key(b))

static int64_t escapes_plain(double cr, double ci) { BODY(ID,   GT_PLAIN) }
static int64_t escapes_key  (double cr, double ci) { BODY(ID,   GT_KEY)   }
static int64_t escapes_norm (double cr, double ci) { BODY(norm, GT_PLAIN) }
static int64_t escapes_both (double cr, double ci) { BODY(norm, GT_KEY)   }

static int64_t image(int64_t size, int64_t (*esc)(double,double)) {
    int64_t acc = 0;
    for (int64_t y = 0; y < size; y++) {
        double ci = 2.0 * (double)y / (double)size - 1.0;
        for (int64_t x = 0; x < size; x++)
            acc += esc(2.0 * (double)x / (double)size - 1.5, ci);
    }
    return acc;
}

static double now_ms(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec*1e3 + t.tv_nsec/1e6; }
static int cmp(const void*a,const void*b){ double x=*(const double*)a,y=*(const double*)b; return (x>y)-(x<y); }

int main(int argc, char **argv) {
    int64_t n = argc > 1 ? atoll(argv[1]) : 96;
    volatile int64_t v;
    struct { const char *name; int64_t (*f)(double,double); } vs[] = {
        {"plain", escapes_plain}, {"key", escapes_key},
        {"norm", escapes_norm},   {"both", escapes_both},
    };
    for (unsigned k = 0; k < 4; k++) {
        double ts[11]; int64_t a = 0;
        for (int i = 0; i < 11; i++) { v = n; double s = now_ms(); a = image(v, vs[k].f); ts[i] = now_ms()-s; }
        qsort(ts, 11, sizeof(double), cmp);
        printf("%-6s\t%.4f ms\t%lld\n", vs[k].name, ts[5], (long long)a);
    }
    return 0;
}
