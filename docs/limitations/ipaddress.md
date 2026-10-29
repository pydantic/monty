# `ipaddress` module

`IPv4Address`, `IPv6Address`, `IPv4Network`, `IPv6Network`, `ip_address()`, `ip_network()`,
`summarize_address_range()`, `collapse_addresses()`, `v4_int_to_packed()`, `v6_int_to_packed()`, `IPV4LENGTH` and
`IPV6LENGTH` are available.

- **`AddressValueError` and `NetmaskValueError` are `ValueError` itself.** Monty's exception types are a fixed set,
    so the module binds both names to `ValueError`: `except ipaddress.AddressValueError` catches every `ValueError`,
    and the constructor errors are plain `ValueError`s with CPython's messages. `ip_address()` and `ip_network()`
    fall back to the other version on a parse error, as CPython does.
- **No interfaces.** `IPv4Interface`, `IPv6Interface`, `ip_interface()` and `get_mixed_type_key()` are absent.
- **The generators are iterators.** `hosts()`, `subnets()` and iterating a network return a lazy iterator whose type
    is `generator`; `address_exclude()`, `summarize_address_range()` and `collapse_addresses()` compute their result
    first and return a `list_iterator`. A `/32` or `/128` network's `hosts()` is the same lazy iterator, where CPython
    returns a `tuple_iterator`. `subnets()` and `supernet()` check their arguments when called; CPython's
    `subnets()` is a generator that fails when first iterated.
- **Error messages name the types with their module** (`'<' not supported between instances of 'ipaddress.IPv4Address' and 'int'`),
    where CPython writes `IPv4Address`; the version-mismatch errors (`1.2.3.4 and ::1 are not of the same version`)
    match. `type(ip).__name__` is `IPv4Address` on both.
- **Constructor arity errors count the arguments without `self`**: `IPv4Address('1.2.3.4', 5)` reports
    `IPv4Address.__init__() takes 1 positional argument but 2 were given`, one less than CPython on each side.
- **`copy.copy()` and `copy.deepcopy()` return the same object**, where CPython builds a new one; the values are
    immutable, so only `is` can tell.
- **Prefix arguments must fit 64 bits.** `subnets()` and `supernet()` raise `OverflowError` for a `prefixlen_diff` or
    `new_prefix` beyond that, where CPython works the value into its `ValueError` message.
- **Addresses and networks cannot cross the host boundary.** Return them as `str(value)` or `int(value)`.
- **`format()` is not implemented** on addresses: `f'{ip:#x}'` raises `TypeError`, where CPython formats the integer.
- **`__class__`, `__reduce__` and `__class_getitem__` are absent**, as on other Monty built-in types.
- **Class attributes are reached through instances only**: `IPv4Address.max_prefixlen` raises `AttributeError`,
    where CPython gives `32`.
