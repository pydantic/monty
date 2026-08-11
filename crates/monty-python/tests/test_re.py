from __future__ import annotations

import re
import sys

import pytest
from conftest import RunMonty

from pydantic_monty import MontyRuntimeError


def test_re_module(monty_run: RunMonty):
    assert monty_run('import re') is None


def test_re_compile(monty_run: RunMonty):
    code = """
import re
pattern = re.compile(r'\\d+')
matches = pattern.findall('There are 24 hours in a day and 365 days in a year.')
"""
    assert monty_run(code) is None


supported_flags = [
    (['re.I', 're.IGNORECASE'], re.IGNORECASE),
    (['re.M', 're.MULTILINE'], re.MULTILINE),
    (['re.S', 're.DOTALL'], re.DOTALL),
]
if sys.version_info >= (3, 11):
    supported_flags.append((['re.NOFLAG'], re.NOFLAG))


@pytest.mark.parametrize(
    'flags,target',
    supported_flags,
    ids=str,
)
def test_re_constant(monty_run: RunMonty, flags: list[str], target: int):
    code = f'import re; ({",".join(flags)},)'
    output = monty_run(code)
    assert all(map(lambda orig: orig == target, output))


def test_re_compile_repr(monty_run: RunMonty):
    code = r"""
import re
pattern = re.compile(r'\d+', re.IGNORECASE | re.DOTALL)
pattern
"""
    assert monty_run(code) == r"re.compile('\\d+', re.IGNORECASE|re.DOTALL)"


def test_re_match_repr(monty_run: RunMonty):
    code = """
import re
pattern = re.compile(r'\\d+')
pattern.match('123abc')
"""
    assert monty_run(code) == "<re.Match object; span=(0, 3), match='123'>"


def test_re_match_groups(monty_run: RunMonty):
    code = """
import re
pattern = re.compile(r'(\\d+)-(\\w+)')
match = pattern.match('123-abc')
match.groups()
"""
    assert monty_run(code) == ('123', 'abc')


def test_re_substitution(monty_run: RunMonty):
    code = """
import re
pattern = re.compile(r'\\s+')
result = pattern.sub('-', 'This is a test.')
result
"""
    assert monty_run(code) == 'This-is-a-test.'


def test_re_error_handling(monty_run: RunMonty):
    code = """
import re
try:
    pattern = re.compile(r'[')
except Exception as e:
    error_message = str(e)
error_message
"""
    output = monty_run(code)
    assert 'Parsing error at position 1: Invalid character class' in output


def test_re_error_upcast(monty_run: RunMonty):
    code = """
import re
re.compile(r'[')
"""
    with pytest.raises(MontyRuntimeError) as exc_info:
        monty_run(code)
    if sys.version_info >= (3, 13):
        assert type(exc_info.value.exception()) is re.PatternError
    else:
        assert type(exc_info.value.exception()) is re.error
    assert 'Parsing error at position 1: Invalid character class' in str(exc_info.value)
