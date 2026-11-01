# English subtitle search fixture

`subtitles_en.txt` contains original fictional dialogue written for this benchmark.
It follows the short dialogue turns and timestamps of subtitles; no OpenSubtitles or Rebar text is included.
The fixture and generator use the repository's MIT license.
Regenerate it with `python3 crates/monty-bench/benches/python/data/generate_subtitles.py`.

The file contains 41,991 UTF-8 bytes and 41,951 characters, including 40 non-ASCII characters
(0.095%).
Its 480 cues repeat 24 original dialogue lines with distinct timestamps.
This small, repetitive fixture measures string operations; it does not represent natural-language diversity.

`../subtitle_search.py` finds 80 mentions of `Alex Rivera` and takes a 48-character excerpt at each match.
It validates the match count, character-position checksum, `count()` and final `rfind()` position.
This models an agent locating mentions and retrieving nearby text from a transcript.
The workload is inspired by subtitle searches in Rebar, but uses our own text, pattern and expected results.
