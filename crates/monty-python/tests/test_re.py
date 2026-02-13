from typing import Any

import pytest
from inline_snapshot import snapshot

import pydantic_monty


def test_re_module():
    m = pydantic_monty.Monty("import re")
    output = m.run()
    assert output is None

def test_re_compile():
    code = """
import re
pattern = re.compile(r'\\d+')
matches = pattern.findall('There are 24 hours in a day and 365 days in a year.')
"""
    m = pydantic_monty.Monty(code)
    output = m.run()
    assert output is None

def test_re_match_repr():
    code = """
import re
pattern = re.compile(r'\\d+')
pattern.match('123abc')
"""
    m = pydantic_monty.Monty(code)
    output = m.run()
    assert output == "<re.Match object; span=(0, 3), match='123'>"

def test_re_match_groups():
    code = """
import re
pattern = re.compile(r'(\\d+)-(\\w+)')
match = pattern.match('123-abc')
match.groups()
"""
    m = pydantic_monty.Monty(code)
    output = m.run()
    assert output == ('123', 'abc')
    
def test_re_substitution():
    code = """
import re
pattern = re.compile(r'\\s+')
result = pattern.sub('-', 'This is a test.')
result
"""
    m = pydantic_monty.Monty(code)
    output = m.run()
    assert output == 'This-is-a-test.'
    
def test_re_error_handling():
    code = """
import re
try:
    pattern = re.compile(r'[')
except Exception as e:
    error_message = str(e)
error_message
"""
    m = pydantic_monty.Monty(code)
    output = m.run()
    error = """regex parse error:
    [
    ^
error: unclosed character class"""
    assert error in output

def test_re_resume():
    code = """
import re
pattern = re.compile(func())
matches = pattern.findall('Sample 123 text 456')
dump(matches)
"""
    m = pydantic_monty.Monty(code, external_functions=['func', "dump"])
    progress = m.start()
    assert isinstance(progress, pydantic_monty.MontySnapshot)
    
    assert progress.function_name == snapshot('func')
    assert progress.args == snapshot(())
    assert progress.kwargs == snapshot({})
    
    progress2 = progress.resume(return_value='\\d+')
    assert isinstance(progress2, pydantic_monty.MontySnapshot)
    
    result = progress2.resume(return_value=['123', '456'])
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(['123', '456'])

def test_re_persistence():
    code = """
import re
pattern = re.compile(r'\\w+')
dump()
matches = pattern.findall('Test 123!')
matches
"""
    m = pydantic_monty.Monty(code, external_functions=['dump'])
    progress = m.start()
    assert isinstance(progress, pydantic_monty.MontySnapshot)
    
    data = progress.dump()
    
    progress2 = pydantic_monty.MontySnapshot.load(data)
    
    result = progress2.resume(return_value=None)
    assert isinstance(result, pydantic_monty.MontyComplete)
    assert result.output == snapshot(['Test', '123'])
