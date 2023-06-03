#![feature(test)]

extern crate test;

use test::{black_box, Bencher};

use monty::Executor;

#[bench]
fn loop_mod_13(bench: &mut Bencher) {
    let code = r#"
v = ''
for i in range(100):
    if i % 13 == 0:
        v += 'x'
len(v)
"#;

    let ex = Executor::new(code, "test.py", &[]).unwrap();

    bench.iter(|| {
        black_box(ex.run(vec![]).unwrap());
    });
}
