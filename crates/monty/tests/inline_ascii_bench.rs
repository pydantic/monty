use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

#[test]
#[ignore = "manual release benchmark"]
fn measure() {
    for (kind, text) in [
        ("ascii3", "abc".to_owned()),
        ("ascii15", "abcdefghijklmno".to_owned()),
        ("unicode3", "éx".to_owned()),
        ("unicode15", "éééééééa".to_owned()),
        ("heap_ascii", "a".repeat(4096)),
        ("heap_unicode", "é🐍".repeat(2048)),
    ] {
        for (operation, body, operations) in [
            (
                "len",
                "total = 0\nfor _ in range(3000):\n    total += len(text)\ntotal",
                3000,
            ),
            (
                "isascii",
                "total = 0\nfor _ in range(3000):\n    total += text.isascii()\ntotal",
                3000,
            ),
            ("index", "for _ in range(3000):\n    result = text[-1]\nresult", 3000),
            (
                "iterate",
                "total = 0\nfor _ in range(500):\n    for c in text:\n        total += 1\ntotal",
                500,
            ),
            (
                "construct",
                "for _ in range(3000):\n    result = ''.join([text, ''])\nresult",
                3000,
            ),
            (
                "construct_len",
                "total = 0\nfor _ in range(3000):\n    result = ''.join([text, ''])\n    total += len(result)\ntotal",
                3000,
            ),
        ] {
            let mut runner = MontyRun::new(
                body.to_owned(),
                "bench.py",
                vec!["text".to_owned()],
                CompileOptions::default(),
            )
            .unwrap();
            let mut samples = Vec::new();
            for _ in 0..7 {
                let start = Instant::now();
                let mut count = 0;
                while start.elapsed() < Duration::from_millis(100) {
                    black_box(runner.run_no_limits(vec![MontyObject::string(text.clone())]).unwrap());
                    count += 1;
                }
                samples.push(start.elapsed().as_secs_f64() * 1e9 / f64::from(count) / f64::from(operations));
            }
            samples.sort_by(f64::total_cmp);
            println!("BENCH,{kind},{operation},{:.3}", samples[3]);
        }
    }
}
