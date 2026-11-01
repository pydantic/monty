# String scan fixes

Interned ASCII indexing previously rescanned the literal to classify it on every access.
Walking all positions of a length-n literal therefore scanned O(n²) bytes.
The interner now records ASCII status when constructing an entry, including snapshot loading.
Heap strings already had this flag.

Unicode endpoint access now reads the first or last UTF-8 character directly.
Previously each access counted the whole string before selecting the character.
Unbounded search and affix checks skip character-range conversion.
Contiguous slices copy their UTF-8 range without collecting decoded characters.
Explicit Unicode bounds still scan; cursors and sparse indices belong to the follow-up PR.
String iteration already retained a byte position and is unchanged.

The benchmarks cover interned ASCII, runtime ASCII, and Unicode endpoints.
Use Flying Circus with saved workers to compare the two stages:

```sh
flying-circus --diff --samples 50 --memory-samples 10 \
  -r baseline=/path/to/main/monty -r candidate=/path/to/scan-fixes/monty \
  --benchmark crates/monty-bench/benches/python/string_random_access_interned_ascii.py \
  --benchmark crates/monty-bench/benches/python/string_random_access_ascii.py \
  --benchmark crates/monty-bench/benches/python/string_end_access_unicode.py
```

To check scaling, generate literal strings of 1,024, 2,048, 4,096 and 8,192 ASCII characters,
then sum `ord(text[i])` for every character. Fixture construction must be outside request timing.
A complete indexed walk should scale linearly after the fix.
The first PR adds no per-string cursor, length cache or checkpoint table.
