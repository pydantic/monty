//! Empty-opcode dispatch slope. Build with --features worker-alloc.
use std::{env, ffi::CString, hint::black_box, time::Instant};

use monty::MontyRun;
use monty_types::MontyObject;
use pyo3::prelude::*;

#[cfg(feature = "worker-alloc")]
#[global_allocator]
static ALLOCATOR: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

fn main() {
    let samples: usize = env::var("SAMPLES").map_or(Ok(20), |s| s.parse()).unwrap();
    Python::attach(|py| {
        let mut cases = Vec::new();
        for count in [0, 10_000, 100_000, 1_000_000] {
            let mut monty = MontyRun::opcode_cost_benchmark(count);
            let code = format!(
                "import opcode, types\ndef base():\n    return 0\nprefix = bytes([opcode.opmap['NOP'], 0]) * {count}\nbody = base.__code__.co_code\ncode = base.__code__.replace(co_code=body[:2] + prefix + body[2:])\nhot = types.FunctionType(code, globals())"
            );
            let module = PyModule::from_code(py, &CString::new(code).unwrap(), c"nop.py", c"nop_bench").unwrap();
            let fun = module.getattr("hot").unwrap().unbind();
            for _ in 0..3 {
                assert_eq!(monty.run_no_limits(vec![]).unwrap(), MontyObject::int(0));
                assert_eq!(fun.call0(py).unwrap().extract::<i64>(py).unwrap(), 0);
            }
            cases.push((count, monty, fun));
        }
        println!("interpreter,nops,sample,ns");
        for sample in 0..samples {
            for offset in 0..cases.len() {
                let index = if sample % 2 == 0 {
                    offset
                } else {
                    cases.len() - offset - 1
                };
                let (count, monty, fun) = &mut cases[index];
                for first in [true, false] {
                    let cpython = first == (sample % 2 == 0);
                    let start = Instant::now();
                    if cpython {
                        let result = black_box(fun.call0(py).unwrap().extract::<i64>(py).unwrap());
                        let ns = start.elapsed().as_nanos();
                        assert_eq!(result, 0);
                        println!("cpython,{count},{sample},{ns}");
                    } else {
                        let result = black_box(monty.run_no_limits(vec![]).unwrap());
                        let ns = start.elapsed().as_nanos();
                        assert_eq!(result, MontyObject::int(0));
                        println!("monty,{count},{sample},{ns}");
                    }
                }
            }
        }
    });
}
