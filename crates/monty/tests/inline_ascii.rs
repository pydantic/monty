use monty::MontyRun;
use monty_types::CompileOptions;

#[test]
fn producing_operations_preserve_ascii_status() {
    let source = r"
def check(result, expected, ascii):
    assert result == expected
    assert result.isascii() == ascii
    assert len(result) == len(expected)
    assert list(result) == list(expected)

for source in ['abc', 'abcdefghijklmnop', 'éabc', 'éabcdefghijklmnop']:
    text = source + ''
    for result in [text[1:4], text[1:4:2], text[1:4][::-1]]:
        assert result.isascii()
    assert text[:1].isascii() == source[:1].isascii()
    assert text[::-1].isascii() == text.isascii()

def inline(text):
    return text + ''

a = inline('ab')
u = inline('é')
check(a + 'cd', 'abcd', True)
check(a + u, 'abé', False)
check(u + a, 'éab', False)
check(a * 3, 'ababab', True)
check(3 * u, 'ééé', False)
check(u * 0, '', True)
check(u * -1, '', True)
check(''.join(['ab', 'cd']), 'abcd', True)
check(''.join(['é', 'cd']), 'écd', False)
check('é'.join(['ab', 'cd']), 'abécd', False)
check('é'.join(['abc']), 'abc', True)
check('é'.join([]), '', True)
check(('éabc' + '')[0], 'é', False)
check(('éabc' + '')[-1], 'c', True)
long_ascii = inline('a' * 15)
check(long_ascii + 'b', 'aaaaaaaaaaaaaaab', True)
check(long_ascii + 'é', 'aaaaaaaaaaaaaaaé', False)

for source, ascii in [('a' * 32, True), ('éa' * 32, False)]:
    check(source + 'tail', source + 'tail', ascii)
    check(source * 3, source + source + source, ascii)
    check(''.join([source, source]), source * 2, ascii)
    check(source[:20], source[:20], ascii)
    check(source[::-1], source[::-1], ascii)
    check(source * 0, '', True)
    assert (source + 'é').isascii() == False
    assert (source + 'tail')[1:10].isascii() == source[1:10].isascii()
";
    let mut run = MontyRun::new(source.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    run.run_no_limits(vec![]).unwrap();
}
