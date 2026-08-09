# Python's ints are arbitrary precision: no overflow to check, and more work per operation than a
# 64-bit machine integer. `//` is floor division and every operand here is non-negative, so it is
# Beck's truncating division on this input.
import sys, time

def fib(n):
    return n if n < 2 else fib(n-1) + fib(n-2)

def sum_to(n, acc):
    while n > 0:
        acc += n; n -= 1
    return acc

def escapes(cr, ci):
    zrzr = zi = zizi = 0.0
    for _ in range(50):
        zr = zrzr - zizi + cr
        nzi = 2.0*zr*zi + ci
        nzrzr = zr*zr
        nzizi = nzi*nzi
        if nzrzr + nzizi > 4.0:
            return 1
        zrzr, zi, zizi = nzrzr, nzi, nzizi
    return 0

def image(size):
    acc = 0
    for y in range(size):
        ci = 2.0*y/size - 1.0
        for x in range(size):
            acc += escapes(2.0*x/size - 1.5, ci)
    return acc

def xor_from(a, b, weight, acc):
    while not (a == 0 and b == 0):
        acc += weight * ((a % 2 + b % 2) % 2)
        a //= 2; b //= 2; weight *= 2
    return acc

def xor_sweep(n, acc):
    while n > 0:
        acc += xor_from(n % 256, (n*7) % 256, 1, 0)
        n -= 1
    return acc

A = [int(x) for x in sys.argv[1:]]
RUNS = 11
def bench(name, f):
    ts = []
    out = 0
    for _ in range(RUNS):
        s = time.perf_counter()
        out = f()
        ts.append((time.perf_counter()-s)*1e3)
    ts.sort()
    print(f"{name}\t{ts[RUNS//2]:.4f}\t{out}")

bench('fib', lambda: fib(A[0]))
bench('sum_to', lambda: sum_to(A[1], 0))
bench('image', lambda: image(A[2]))
bench('xor_sweep', lambda: xor_sweep(A[3], 0))
