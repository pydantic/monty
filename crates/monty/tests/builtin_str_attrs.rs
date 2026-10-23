use monty::MontyRun;
use monty_types::{CompileOptions, MontyObject};

#[test]
fn string_tables_handle_interned_and_computed_receivers() {
    let code = r"
for text in ['AbC def', ''.join(['AbC ', 'def'])]:
    assert text.upper() == 'ABC DEF'
    assert text.lower() == 'abc def'
    assert text.split() == ['AbC', 'def']
    assert text.find('def') == 4
    assert text.replace('def', 'ghi') == 'AbC ghi'
    assert text.encode() == b'AbC def'
    assert text.isascii()
    assert text.title() == 'Abc Def'
for separator in [', ', ''.join([',', ' '])]:
    assert separator.join(['one', 'two']) == 'one, two'
for template in ['{}: {name}', ''.join(['{}: ', '{name}'])]:
    assert template.format('key', name='value') == 'key: value'
assert 'Straße'.casefold() == 'strasse'
assert ''.join(['Stra', 'ße']).casefold() == 'strasse'
True
";
    let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}

#[test]
fn string_tables_preserve_errors_and_release_arguments() {
    let code = r"
for text in ['abc', ''.join(['a', 'bc'])]:
    try:
        text.upper([1])
    except TypeError:
        pass
    else:
        raise AssertionError('upper accepted an argument')
    try:
        text.split([1])
    except TypeError:
        pass
    else:
        raise AssertionError('split accepted a list')
    try:
        text.missing_method([1])
    except AttributeError:
        pass
    else:
        raise AssertionError('missing method succeeded')
    try:
        text.append([1])
    except AttributeError:
        pass
    else:
        raise AssertionError('non-string method succeeded')
    assert text.upper() == 'ABC'
True
";
    let mut run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    assert_eq!(run.run_no_limits(vec![]).unwrap(), MontyObject::bool(true));
}
