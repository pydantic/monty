# When continuing from nested except handlers, ALL exception states must be cleared.
# After the loop completes its iterations, execution should continue normally.

# Test 1: continue from depth 2 should process all iterations
def test_continue():
    results = []
    for i in range(3):
        try:
            raise ValueError('outer')
        except:
            try:
                raise TypeError('inner')
            except:
                results.append(i)
                continue  # Should clear BOTH exceptions
    return results


assert test_continue() == [0, 1, 2], 'continue from nested except should process all iterations'


# Test 2: continue from depth 3 should also work
def test_continue_depth3():
    results = []
    for i in range(2):
        try:
            raise ValueError('level1')
        except:
            try:
                raise TypeError('level2')
            except:
                try:
                    raise RuntimeError('level3')
                except:
                    results.append(i)
                    continue  # Should clear ALL THREE exceptions
    return results


assert test_continue_depth3() == [0, 1], 'continue from 3-deep except should work'


# Test 3: continue runs else clause since loop completes normally
def test_continue_with_else():
    results = []
    for i in range(2):
        try:
            raise ValueError('outer')
        except:
            try:
                raise TypeError('inner')
            except:
                results.append(i)
                continue
    else:
        results.append('else')
    return results


assert test_continue_with_else() == [0, 1, 'else'], 'continue should allow else to run'
