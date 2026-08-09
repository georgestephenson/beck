# Ruby Integers are arbitrary precision, like Python's. `/` on two Integers is floor division and
# every operand here is non-negative, so it is Beck's truncating division on this input.
def fib(n) = n < 2 ? n : fib(n-1) + fib(n-2)

def sum_to(n, acc)
  while n > 0 do acc += n; n -= 1 end
  acc
end

def escapes(cr, ci)
  zrzr = zi = zizi = 0.0
  50.times do
    zr = zrzr - zizi + cr
    nzi = 2.0*zr*zi + ci
    nzrzr = zr*zr
    nzizi = nzi*nzi
    return 1 if nzrzr + nzizi > 4.0
    zrzr, zi, zizi = nzrzr, nzi, nzizi
  end
  0
end

def image(size)
  acc = 0
  (0...size).each do |y|
    ci = 2.0*y/size - 1.0
    (0...size).each { |x| acc += escapes(2.0*x/size - 1.5, ci) }
  end
  acc
end

def xor_from(a, b, weight, acc)
  until a == 0 && b == 0
    acc += weight * ((a % 2 + b % 2) % 2)
    a /= 2; b /= 2; weight *= 2
  end
  acc
end

def xor_sweep(n, acc)
  while n > 0
    acc += xor_from(n % 256, (n*7) % 256, 1, 0)
    n -= 1
  end
  acc
end

a = ARGV.map(&:to_i)
RUNS = 11
def bench(name, runs)
  ts = []; out = 0
  runs.times { s = Process.clock_gettime(Process::CLOCK_MONOTONIC); out = yield; ts << (Process.clock_gettime(Process::CLOCK_MONOTONIC)-s)*1e3 }
  ts.sort!
  puts "#{name}\t%.4f\t#{out}" % ts[runs/2]
end
bench('fib', RUNS) { fib(a[0]) }
bench('sum_to', RUNS) { sum_to(a[1], 0) }
bench('image', RUNS) { image(a[2]) }
bench('xor_sweep', RUNS) { xor_sweep(a[3], 0) }
