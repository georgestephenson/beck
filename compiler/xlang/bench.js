'use strict';
// Numbers here are float64. Every value the four benchmarks reach is below 2^53
// (500000500000 is the largest), so the arithmetic is exact — but it is not integer arithmetic,
// and it is not checked: JS has no overflow to detect.
const add=(a,b)=>a+b, sub=(a,b)=>a-b, mul=(a,b)=>a*b;
const div=(a,b)=>Math.trunc(a/b), rem=(a,b)=>a%b;

function fib(n){ return n<2 ? n : add(fib(sub(n,1)), fib(sub(n,2))); }

function sum_to(n,acc){ while(n>0){ acc=add(acc,n); n=sub(n,1);} return acc; }

function escapes(cr,ci){
  let zrzr=0, zi=0, zizi=0;
  for(let z=0; z<50; z++){
    const zr=zrzr-zizi+cr, nzi=2*zr*zi+ci, nzrzr=zr*zr, nzizi=nzi*nzi;
    if(nzrzr+nzizi>4) return 1;
    zrzr=nzrzr; zi=nzi; zizi=nzizi;
  }
  return 0;
}

function image(size){
  let acc=0;
  for(let y=0;y<size;y++){
    const ci=2*y/size-1;
    for(let x=0;x<size;x++){ acc=add(acc, escapes(2*x/size-1.5, ci)); }
  }
  return acc;
}

function xor_from(a,b,weight,acc){
  while(!(a===0&&b===0)){
    acc=add(acc, mul(weight, rem(add(rem(a,2),rem(b,2)),2)));
    a=div(a,2); b=div(b,2); weight=mul(weight,2);
  }
  return acc;
}

function xor_sweep(n,acc){
  while(n>0){ acc=add(acc, xor_from(rem(n,256), rem(mul(n,7),256), 1, 0)); n=sub(n,1); }
  return acc;
}

const A = process.argv.slice(2).map(Number);
const RUNS = 11;
function bench(name, f){
  // Warm the JIT first: an unwarmed V8 measures the interpreter, which is not what anybody means
  // by "how fast is Node".
  for(let i=0;i<3;i++) f();
  const ts=[]; let out=0;
  for(let i=0;i<RUNS;i++){ const s=process.hrtime.bigint(); out=f(); ts.push(Number(process.hrtime.bigint()-s)/1e6); }
  ts.sort((a,b)=>a-b);
  console.log(`${name}\t${ts[RUNS>>1].toFixed(4)}\t${out}`);
}
bench('fib', ()=>fib(A[0]));
bench('sum_to', ()=>sum_to(A[1],0));
bench('image', ()=>image(A[2]));
bench('xor_sweep', ()=>xor_sweep(A[3],0));
