//! Compile-once loop and dictionary microbenchmarks, with identical Python sources.
use std::{hint::black_box, time::Instant};

use monty::MontyRun;
use monty_types::CompileOptions;
use pyo3::{prelude::*, types::PyDict};
#[cfg(feature = "worker-alloc")]
#[global_allocator]
static ALLOCATOR: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;
fn main() {
    Python::attach(|py| {
        let cases = [
            (
                "empty_loop",
                "def hot():\n    for _ in range(200_000):\n        pass\n    return 42\nresult = hot()",
                42,
            ),
            (
                "decrement",
                "def hot():\n    n = 200_000\n    while n:\n        n -= 1\n    return n\nresult = hot()",
                0,
            ),
            (
                "dict_string",
                "def hot():\n    d = {'x': 1, 'y': 2, 'z': 3}\n    total = 0\n    for _ in range(100_000):\n        total += d['x'] + d['y'] + d['z']\n    return total\nresult = hot()",
                600_000,
            ),
            (
                "dict_integer",
                "def hot():\n    d = {1: 1, 2: 2, 3: 3}\n    total = 0\n    for _ in range(100_000):\n        total += d[1] + d[2] + d[3]\n    return total\nresult = hot()",
                600_000,
            ),
            (
                "class_fallback",
                "class Record:\n    x = 1\ndef hot():\n    r = Record()\n    total = 0\n    for _ in range(100_000):\n        total += r.x\n    return total\nresult = hot()",
                100_000,
            ),
        ];
        println!("case,interpreter,sample,ns");
        for (name, source, expected) in cases {
            let mut monty = MontyRun::new(
                format!("{source}\nresult"),
                "bench.py",
                vec![],
                CompileOptions::default(),
            )
            .unwrap();
            let builtins = py.import("builtins").unwrap();
            let code = builtins.call_method1("compile", (source, "bench.py", "exec")).unwrap();
            let globals = PyDict::new(py);
            for sample in 0..35 {
                for first in [true, false] {
                    let cpython = first == (sample % 2 == 0);
                    let start = Instant::now();
                    if cpython {
                        builtins.call_method1("exec", (&code, &globals)).unwrap();
                        let ns = start.elapsed().as_nanos();
                        assert_eq!(
                            globals.get_item("result").unwrap().unwrap().extract::<i64>().unwrap(),
                            expected
                        );
                        if sample >= 5 {
                            println!("{name},cpython,{sample},{ns}");
                        }
                    } else {
                        let result = black_box(monty.run_no_limits(vec![]).unwrap());
                        let ns = start.elapsed().as_nanos();
                        assert_eq!(result, monty_types::MontyObject::int(expected));
                        if sample >= 5 {
                            println!("{name},monty,{sample},{ns}");
                        }
                    }
                }
            }
        }
    });
}
