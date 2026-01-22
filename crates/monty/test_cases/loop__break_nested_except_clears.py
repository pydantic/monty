# When breaking from nested except handlers, ALL exception states must be cleared.
# After the loop completes via break, execution should continue normally.

# Test 1: break from depth 2 should reach code after loop
def test_break():
    for i in range(1):
        try:
            raise ValueError('outer')
        except:
            try:
                raise TypeError('inner')
            except:
                break  # Should clear BOTH exceptions
    return 'ok'


assert test_break() == 'ok', 'break from nested except should reach return'


# Test 2: break from depth 3 should also work
def test_break_depth3():
    for i in range(1):
        try:
            raise ValueError('level1')
        except:
            try:
                raise TypeError('level2')
            except:
                try:
                    raise RuntimeError('level3')
                except:
                    break  # Should clear ALL THREE exceptions
    return 'deep'


assert test_break_depth3() == 'deep', 'break from 3-deep except should reach return'


# Test 3: verify exception stack is empty after break
def test_empty_stack():
    result = []
    for i in range(1):
        try:
            raise ValueError('outer')
        except:
            try:
                raise TypeError('inner')
            except:
                result.append('breaking')
                break
    result.append('after')
    return result


assert test_empty_stack() == ['breaking', 'after'], 'should execute code after break'
