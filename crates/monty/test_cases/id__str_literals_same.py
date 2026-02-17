# With string interning, identical literals have the same id
id('hello') == id('hello')
# Return=True
