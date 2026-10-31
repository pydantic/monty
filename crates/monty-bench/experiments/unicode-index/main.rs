//! Standalone policy experiments: includes lazy construction and cleanup, excludes VM dispatch.

use std::{hint::black_box, time::Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Scan,
    Length,
    Cursor,
    Sparse,
    Deltas,
    Adaptive,
}

struct Policy {
    name: &'static str,
    kind: Kind,
    stride: usize,
    trigger: usize,
    backtrack: usize,
    cursor: bool,
}

const POLICIES: &[Policy] = &[
    Policy {
        name: "scan",
        kind: Kind::Scan,
        stride: 0,
        trigger: 0,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "length",
        kind: Kind::Length,
        stride: 0,
        trigger: 0,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "cursor_forward",
        kind: Kind::Cursor,
        stride: 0,
        trigger: 0,
        backtrack: 0,
        cursor: true,
    },
    Policy {
        name: "cursor_back8",
        kind: Kind::Cursor,
        stride: 0,
        trigger: 0,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "cursor_back64",
        kind: Kind::Cursor,
        stride: 0,
        trigger: 0,
        backtrack: 64,
        cursor: true,
    },
    Policy {
        name: "sparse32_at1",
        kind: Kind::Sparse,
        stride: 32,
        trigger: 1,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "sparse8_at8",
        kind: Kind::Sparse,
        stride: 8,
        trigger: 8,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "sparse32_at8",
        kind: Kind::Sparse,
        stride: 32,
        trigger: 8,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "sparse128_at8",
        kind: Kind::Sparse,
        stride: 128,
        trigger: 8,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "sparse32_at64",
        kind: Kind::Sparse,
        stride: 32,
        trigger: 64,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "sparse32_cursor_at8",
        kind: Kind::Sparse,
        stride: 32,
        trigger: 8,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "full_at8",
        kind: Kind::Sparse,
        stride: 1,
        trigger: 8,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "full_at64",
        kind: Kind::Sparse,
        stride: 1,
        trigger: 64,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "deltas_at8",
        kind: Kind::Deltas,
        stride: 0,
        trigger: 8,
        backtrack: 0,
        cursor: false,
    },
    Policy {
        name: "adaptive8_1x",
        kind: Kind::Adaptive,
        stride: 8,
        trigger: 1,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "adaptive8_4x",
        kind: Kind::Adaptive,
        stride: 8,
        trigger: 4,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "adaptive32_1x",
        kind: Kind::Adaptive,
        stride: 32,
        trigger: 1,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "adaptive32_4x",
        kind: Kind::Adaptive,
        stride: 32,
        trigger: 4,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "adaptive8_16x",
        kind: Kind::Adaptive,
        stride: 8,
        trigger: 16,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "adaptive8_64x",
        kind: Kind::Adaptive,
        stride: 8,
        trigger: 64,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "adaptive32_16x",
        kind: Kind::Adaptive,
        stride: 32,
        trigger: 16,
        backtrack: 8,
        cursor: true,
    },
    Policy {
        name: "adaptive32_64x",
        kind: Kind::Adaptive,
        stride: 32,
        trigger: 64,
        backtrack: 8,
        cursor: true,
    },
];

// Matches the current non-ASCII get_char_at_index paths, including the -1 shortcut.
fn scan(text: &str, ascii: bool, index: i64) -> Option<char> {
    if ascii {
        let offset = if index < 0 {
            text.len().checked_sub(usize::try_from(index.unsigned_abs()).ok()?)?
        } else {
            usize::try_from(index).ok()?
        };
        text.as_bytes().get(offset).copied().map(char::from)
    } else if index == -1 {
        text.chars().next_back()
    } else if index < 0 {
        let length = text.chars().count();
        text.chars()
            .nth(length.checked_sub(usize::try_from(index.unsigned_abs()).ok()?)?)
    } else {
        text.chars().nth(usize::try_from(index).ok()?)
    }
}

struct Lookup<'a> {
    text: &'a str,
    ascii: bool,
    policy: &'static Policy,
    requests: usize,
    length: usize,
    stride_shift: u32,
    built: bool,
    scanned_bytes: usize,
    last: Option<(usize, usize)>,
    offsets: Vec<u32>,
    deltas: Vec<(u32, u32)>,
}

impl<'a> Lookup<'a> {
    fn new(text: &'a str, ascii: bool, policy: &'static Policy) -> Self {
        Self {
            text,
            ascii,
            policy,
            requests: 0,
            length: usize::MAX,
            stride_shift: policy.stride.trailing_zeros(),
            built: false,
            scanned_bytes: 0,
            last: None,
            offsets: Vec::new(),
            deltas: Vec::new(),
        }
    }

    fn len(&mut self) -> usize {
        if self.length == usize::MAX {
            self.length = self.text.chars().count();
        }
        self.length
    }

    fn build(&mut self) {
        let length = self.len();
        if matches!(self.policy.kind, Kind::Sparse | Kind::Adaptive) {
            self.offsets = Vec::with_capacity(length.div_ceil(self.policy.stride));
            let mut next_checkpoint = 0;
            for (index, (byte, _)) in self.text.char_indices().enumerate() {
                if index == next_checkpoint {
                    self.offsets.push(u32::try_from(byte).unwrap());
                    next_checkpoint += self.policy.stride;
                }
            }
        } else {
            let count = self.text.chars().filter(|c| !c.is_ascii()).count();
            self.deltas = Vec::with_capacity(count);
            let mut extra = 0;
            for (index, c) in self.text.chars().enumerate() {
                if !c.is_ascii() {
                    extra += c.len_utf8() - 1;
                    self.deltas
                        .push((u32::try_from(index + 1).unwrap(), u32::try_from(extra).unwrap()));
                }
            }
        }
        self.built = true;
    }

    fn get(&mut self, index: i64) -> Option<char> {
        if self.ascii || self.policy.kind == Kind::Scan {
            return scan(self.text, self.ascii, index);
        }
        // Cheap endpoints should neither build an index nor displace a useful cursor.
        if index == 0 {
            return self.text.chars().next();
        }
        if index == -1 {
            return self.text.chars().next_back();
        }
        self.requests += 1;
        if matches!(self.policy.kind, Kind::Sparse | Kind::Deltas) && self.requests == self.policy.trigger {
            self.build();
        }
        let target = if index < 0 {
            self.len().checked_sub(usize::try_from(index.unsigned_abs()).ok()?)?
        } else {
            usize::try_from(index).ok()?
        };
        if self.length != usize::MAX && target >= self.length {
            return None;
        }

        let indexed = self.built;
        if indexed && self.policy.kind == Kind::Deltas {
            let at = self
                .deltas
                .partition_point(|&(character, _)| character as usize <= target);
            let extra = if at == 0 { 0 } else { self.deltas[at - 1].1 as usize };
            return self.text.get(target + extra..)?.chars().next();
        }

        let mut anchor = if indexed {
            let slot = target >> self.stride_shift;
            (slot << self.stride_shift, *self.offsets.get(slot)? as usize)
        } else {
            (0, 0)
        };
        if self.policy.cursor {
            if let Some(last) = self.last {
                if last.0 <= target && last.0 >= anchor.0 {
                    anchor = last;
                } else if last.0 > target
                    && last.0 - target <= self.policy.backtrack
                    && last.0 - target < target - anchor.0
                {
                    anchor = last;
                }
            }
            if self.length != usize::MAX
                && self.length - target <= self.policy.backtrack
                && self.length - target < target.abs_diff(anchor.0)
            {
                anchor = (self.length, self.text.len());
            }
        }

        let (byte, character) = if target >= anchor.0 {
            let mut chars = self.text[anchor.1..].chars();
            let character = chars.nth(target - anchor.0)?;
            (self.text.len() - chars.as_str().len() - character.len_utf8(), character)
        } else {
            let mut chars = self.text[..anchor.1].chars();
            let character = chars.nth_back(anchor.0 - target - 1)?;
            (chars.as_str().len(), character)
        };
        if self.policy.cursor {
            self.last = Some((target, byte));
        }
        if self.policy.kind == Kind::Adaptive && !self.built && target.abs_diff(anchor.0) > 8 {
            self.scanned_bytes += byte.abs_diff(anchor.1);
            if self.scanned_bytes >= self.text.len() * self.policy.trigger {
                self.build();
            }
        }
        Some(character)
    }

    fn cache_bytes(&self) -> usize {
        self.offsets.capacity() * size_of::<u32>() + self.deltas.capacity() * size_of::<(u32, u32)>()
    }
}

fn text(kind: &str, length: usize) -> String {
    let phrase: Vec<char> = match kind {
        "german" => "Bitte API-Anfragen zuerst anhand der Anfrage-ID vergleichen und den Status des Dienstes prüfen. Danach das Gateway überprüfen und den Vorgang dokumentieren. ".chars().collect(),
        "chinese" => "首先检查服务状态，并记录 API request_id 和相关日志。确认配置无误后，再验证请求结果。".chars().collect(),
        _ => Vec::new(),
    };
    (0..length)
        .map(|i| match kind {
            "german" | "chinese" => phrase[i % phrase.len()],
            "latin1_10pct" if i % 10 == 0 => 'ü',
            "emoji_10pct" if i % 10 == 0 => '🐍',
            _ => char::from(b'a' + (i % 26) as u8),
        })
        .collect()
}

fn requests(pattern: &str, length: usize, count: usize) -> Vec<i64> {
    let mut random = 42u64;
    let mut nearby = length / 2;
    (0..count)
        .map(|i| {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let index = match pattern {
                "sequential" => i % length,
                "reverse" => length - 1 - i % length,
                "repeat" => length / 2,
                "nearby" => {
                    nearby = (nearby + (random % 9) as usize + length - 4) % length;
                    nearby
                }
                "ends" => {
                    if i % 2 == 0 {
                        0
                    } else {
                        length - 1
                    }
                }
                _ => random as usize % length,
            };
            if i % 2 == 0 {
                index as i64
            } else {
                index as i64 - length as i64
            }
        })
        .collect()
}

fn validate() {
    let repeated = "prüfen ".repeat(50);
    for source in ["", "abc", "aé中🐍İΣ", repeated.as_str()] {
        let characters: Vec<_> = source.chars().collect();
        for policy in POLICIES {
            let mut cache = Lookup::new(source, source.is_ascii(), policy);
            for _ in 0..3 {
                for index in -(characters.len() as i64 + 2)..characters.len() as i64 + 2 {
                    let expected = if index < 0 {
                        characters.len().checked_sub(index.unsigned_abs() as usize)
                    } else {
                        Some(index as usize)
                    }
                    .and_then(|i| characters.get(i).copied());
                    assert_eq!(cache.get(index), expected, "{} {source:?} {index}", policy.name);
                }
            }
            assert_eq!(cache.get(i64::MIN), None);
            assert_eq!(cache.get(i64::MAX), None);
        }
    }
}

fn matrix() {
    validate();
    println!("corpus,characters,utf8_bytes,non_ascii,pattern,requests,policy,ns_per_lookup,cache_bytes");
    for corpus in ["ascii", "german", "latin1_10pct", "chinese", "emoji_10pct"] {
        for length in [256, 4096, 65536] {
            let source = text(corpus, length);
            let ascii = source.is_ascii();
            for pattern in ["random", "sequential", "reverse", "nearby", "repeat", "ends"] {
                for count in [1, 16, 256, 4096] {
                    let queries = requests(pattern, length, count);
                    let expected: u64 = queries
                        .iter()
                        .map(|&i| u64::from(scan(&source, ascii, i).unwrap() as u32))
                        .sum();
                    let batches = if count == 1 {
                        16
                    } else if count == 16 {
                        8
                    } else {
                        1
                    };
                    // Rotate policy order across samples to reduce ordering and thermal bias.
                    let mut samples = vec![Vec::new(); POLICIES.len()];
                    let mut bytes = vec![0; POLICIES.len()];
                    for sample in 0..7 {
                        for offset in 0..POLICIES.len() {
                            let slot = (offset + sample * 5) % POLICIES.len();
                            let start = Instant::now();
                            let mut checksum = 0;
                            for _ in 0..batches {
                                let mut cache = Lookup::new(black_box(&source), ascii, &POLICIES[slot]);
                                for &index in &queries {
                                    checksum += u64::from(black_box(&mut cache).get(black_box(index)).unwrap() as u32);
                                }
                                bytes[slot] = cache.cache_bytes();
                                black_box(cache);
                            }
                            let elapsed = start.elapsed().as_nanos() as f64 / (count * batches) as f64;
                            assert_eq!(checksum, expected * batches as u64);
                            samples[slot].push(elapsed);
                        }
                    }
                    for (slot, policy) in POLICIES.iter().enumerate() {
                        samples[slot].sort_by(f64::total_cmp);
                        println!(
                            "{corpus},{length},{},{},{pattern},{count},{},{:.3},{}",
                            source.len(),
                            source.chars().filter(|c| !c.is_ascii()).count(),
                            policy.name,
                            samples[slot][3],
                            bytes[slot]
                        );
                    }
                }
            }
        }
    }
}

fn memory(args: &[String]) {
    let policy = POLICIES.iter().find(|p| p.name == args[0]).unwrap();
    let source = text(&args[1], 4096);
    let count: usize = args[2].parse().unwrap();
    let strings = vec![source; 3000];
    let mut caches: Vec<_> = strings.iter().map(|s| Lookup::new(s, s.is_ascii(), policy)).collect();
    let queries = requests("random", 4096, count);
    let mut checksum = 0u64;
    for cache in &mut caches {
        for &query in &queries {
            checksum += u64::from(cache.get(query).unwrap() as u32);
        }
    }
    println!(
        "{},{},{},{},{}",
        strings.iter().map(String::len).sum::<usize>(),
        caches.iter().map(Lookup::cache_bytes).sum::<usize>(),
        size_of::<Lookup<'_>>() * caches.len(),
        strings.len(),
        checksum
    );
    black_box(caches);
    black_box(strings);
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|s| s == "memory") {
        memory(&args[1..]);
    } else if args.first().is_some_and(|s| s == "validate") {
        validate();
        println!("validated {} policies", POLICIES.len());
    } else {
        matrix();
    }
}
