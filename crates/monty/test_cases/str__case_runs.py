# Mixed conversion must retain Unicode expansions between maximal ASCII runs.
cases = (
    ('Straße STRAẞE', 'STRASSE STRAẞE', 'strasse strasse'),
    ('İıßẞﬃΣσς中🐍', 'İISSẞFFIΣΣΣ中🐍', 'i̇ıssssffiσσσ中🐍'),
    ('𐐨𐐀', '𐐀𐐀', '𐐨𐐨'),
    ('ΐΰᾀͅ', 'Ϊ́Ϋ́ἈΙΙ', 'ΐΰἀιι'),
    ('中文代理请求', '中文代理请求', '中文代理请求'),
    ('ßaﬃbİcΣd', 'SSAFFIBİCΣD', 'ssaffibi̇cσd'),
)
ascii_text = ''.join(chr(i) for i in range(128))
for prefix in ('', 'a', 'Api-ID_123', ascii_text, 'AbCd' * 100, '中文' * 200, 'éa' * 200):
    for text, upper, folded in cases:
        mixed = prefix + text + prefix
        assert mixed.upper() == prefix.upper() + upper + prefix.upper()
        assert mixed.casefold() == prefix.casefold() + folded + prefix.casefold()

for size in (7, 8, 9, 15, 16, 17, 31, 32, 33, 1024):
    text = ('a' * size + 'ß' + 'B' * size + '中' + 'C' * size + 'ﬃ') * 3
    assert text.upper() == ('A' * size + 'SS' + 'B' * size + '中' + 'C' * size + 'FFI') * 3
    assert text.casefold() == ('a' * size + 'ss' + 'b' * size + '中' + 'c' * size + 'ffi') * 3
