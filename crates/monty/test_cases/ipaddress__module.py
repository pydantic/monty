import ipaddress
from ipaddress import IPv4Address, IPv4Network, IPv6Address, IPv6Network, ip_address, ip_network

# === addresses ===
_a = ip_address('192.168.1.1')
assert isinstance(_a, IPv4Address)
assert repr(_a) == "IPv4Address('192.168.1.1')"
assert str(_a) == '192.168.1.1'
assert int(_a) == 3232235777
assert (_a.version, _a.max_prefixlen) == (4, 32)
assert _a.packed == b'\xc0\xa8\x01\x01'
assert (_a.exploded, _a.compressed) == ('192.168.1.1', '192.168.1.1')
assert _a.reverse_pointer == '1.1.168.192.in-addr.arpa'
assert (_a.is_private, _a.is_global, _a.is_multicast, _a.is_loopback) == (True, False, False, False)
assert (_a.is_reserved, _a.is_link_local, _a.is_unspecified) == (False, False, False)
assert repr(_a.ipv6_mapped) == "IPv6Address('::ffff:192.168.1.1')"
assert _a == IPv4Address('192.168.1.1')
assert _a == IPv4Address(3232235777)
assert _a == IPv4Address(b'\xc0\xa8\x01\x01')
assert _a != '192.168.1.1'
assert _a != IPv6Address('::1')
assert hash(_a) == hash(IPv4Address(3232235777))
assert {_a: 'home'}[IPv4Address('192.168.1.1')] == 'home'
assert _a < IPv4Address('192.168.1.2')
assert IPv4Address('9.0.0.1') < IPv4Address('10.0.0.1')
assert sorted([ip_address('10.0.0.2'), ip_address('10.0.0.1'), ip_address('9.0.0.1')]) == [
    IPv4Address('9.0.0.1'),
    IPv4Address('10.0.0.1'),
    IPv4Address('10.0.0.2'),
]
assert _a + 1 == IPv4Address('192.168.1.2')
assert _a - 1 == IPv4Address('192.168.1.0')
assert bool(IPv4Address('0.0.0.0'))
assert IPv4Address(True) == IPv4Address('0.0.0.1')
assert str(ip_address(5)) == '0.0.0.5'
assert str(ip_address(2**32)) == '::1:0:0'

for _text, _props in [
    ('0.0.0.0', (False, True, False, False, False, False, True)),
    ('10.0.0.1', (False, True, False, False, False, False, False)),
    ('100.64.1.1', (False, False, False, False, False, False, False)),
    ('127.0.0.1', (False, True, False, False, True, False, False)),
    ('169.254.1.1', (False, True, False, False, False, True, False)),
    ('192.0.0.9', (False, False, True, False, False, False, False)),
    ('192.0.0.8', (False, True, False, False, False, False, False)),
    ('224.0.0.1', (True, False, True, False, False, False, False)),
    ('240.0.0.1', (False, True, False, True, False, False, False)),
    ('255.255.255.255', (False, True, False, True, False, False, False)),
    ('8.8.8.8', (False, False, True, False, False, False, False)),
]:
    _ip = ip_address(_text)
    assert (
        _ip.is_multicast,
        _ip.is_private,
        _ip.is_global,
        _ip.is_reserved,
        _ip.is_loopback,
        _ip.is_link_local,
        _ip.is_unspecified,
    ) == _props

# === IPv6 addresses ===
_b = ip_address('2001:0db8:0000:0000:0000:0000:0000:0001')
assert isinstance(_b, IPv6Address)
assert repr(_b) == "IPv6Address('2001:db8::1')"
assert str(_b) == '2001:db8::1'
assert int(_b) == 42540766411282592856903984951653826561
assert (_b.version, _b.max_prefixlen) == (6, 128)
assert _b.packed == b' \x01\r\xb8\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01'
assert _b.exploded == '2001:0db8:0000:0000:0000:0000:0000:0001'
assert _b.compressed == '2001:db8::1'
assert _b.reverse_pointer == '1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa'
assert (_b.ipv4_mapped, _b.sixtofour, _b.teredo, _b.scope_id) == (None, None, None, None)
assert _b.is_private and not _b.is_global
assert IPv6Address(1) == ip_address('::1')
assert IPv6Address(2**128 - 1) == ip_address('ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff')
assert ip_address('::1') + 2**64 == IPv6Address('::1:0:0:0:1')
assert ip_address('::1') != ip_address('::1%x')
for _text, _compressed in [
    ('::', '::'),
    ('0:0:0:0:0:0:0:0', '::'),
    ('0::0', '::'),
    ('1::', '1::'),
    ('::1:0:0:0:0', '0:0:0:1::'),
    ('1:0:0:0:0:0:0:1', '1::1'),
    ('::a:b', '::a:b'),
    ('1:2:3:4:5:6:7:8', '1:2:3:4:5:6:7:8'),
    ('2002:c000:0204::', '2002:c000:204::'),
    ('::ffff:1.2.3.4', '::ffff:1.2.3.4'),
    ('::ffff:102:304', '::ffff:1.2.3.4'),
]:
    assert str(ip_address(_text)) == _compressed

_mapped = ip_address('::ffff:1.2.3.4')
assert _mapped.ipv4_mapped == IPv4Address('1.2.3.4')
assert _mapped.exploded == '0000:0000:0000:0000:0000:ffff:1.2.3.4'
assert _mapped.reverse_pointer == '4.0.3.0.2.0.1.0.f.f.f.f.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.ip6.arpa'
assert (_mapped.is_private, _mapped.is_global) == (False, True)
assert ip_address('::ffff:10.0.0.1').is_private
assert ip_address('2002:c000:0204::').sixtofour == IPv4Address('192.0.2.4')
assert ip_address('2001:0:4136:e378:8000:63bf:3fff:fdd2').teredo == (
    IPv4Address('65.54.227.120'),
    IPv4Address('192.0.2.45'),
)
_scoped = ip_address('fe80::1%eth0')
assert repr(_scoped) == "IPv6Address('fe80::1%eth0')"
assert (_scoped.scope_id, int(_scoped)) == ('eth0', 338288524927261089654018896841347694593)
assert _scoped.is_link_local
for _text, _props in [
    ('::', (False, True, False, True, False, False, True, False)),
    ('::1', (False, True, False, True, True, False, False, False)),
    ('fe80::1', (False, True, False, False, False, True, False, False)),
    ('fc00::1', (False, True, False, False, False, False, False, False)),
    ('fec0::1', (False, False, True, False, False, False, False, True)),
    ('ff02::1', (True, False, True, False, False, False, False, False)),
    ('2001:1::1', (False, False, True, False, False, False, False, False)),
    ('2001:1::3', (False, True, False, False, False, False, False, False)),
    ('1:2:3:4:5:6:7:8', (False, False, True, True, False, False, False, False)),
    ('64:ff9b:1::1', (False, True, False, True, False, False, False, False)),
    ('2606:4700::1111', (False, False, True, False, False, False, False, False)),
]:
    _ip = ip_address(_text)
    assert (
        _ip.is_multicast,
        _ip.is_private,
        _ip.is_global,
        _ip.is_reserved,
        _ip.is_loopback,
        _ip.is_link_local,
        _ip.is_unspecified,
        _ip.is_site_local,
    ) == _props

# === address errors ===
# `AddressValueError` and `NetmaskValueError` are caught as `ValueError` on both interpreters
for _bad, _msg in [
    ('1.2.3', "Expected 4 octets in '1.2.3'"),
    ('1.2.3.4.5', "Expected 4 octets in '1.2.3.4.5'"),
    ('256.1.1.1', "Octet 256 (> 255) not permitted in '256.1.1.1'"),
    ('01.2.3.4', "Leading zeros are not permitted in '01' in '01.2.3.4'"),
    ('1.2.3.a', "Only decimal digits permitted in 'a' in '1.2.3.a'"),
    ('', 'Address cannot be empty'),
    ('1..2.3', "Empty octet not permitted in '1..2.3'"),
    ('1.2.3.4444', "At most 3 characters permitted in '4444' in '1.2.3.4444'"),
    ('1.2.3.4/24', "Unexpected '/' in '1.2.3.4/24'"),
    (2**32, '4294967296 (>= 2**32) is not permitted as an IPv4 address'),
    (-1, '-1 (< 0) is not permitted as an IPv4 address'),
    (b'\x01\x02\x03', "b'\\x01\\x02\\x03' (len 3 != 4) is not permitted as an IPv4 address"),
    (1.5, "Expected 4 octets in '1.5'"),
    (None, "Expected 4 octets in 'None'"),
]:
    try:
        IPv4Address(_bad)
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == _msg
for _bad, _msg in [
    ('::1:', "Trailing ':' only permitted as part of '::' in '::1:'"),
    (':1::', "Leading ':' only permitted as part of '::' in ':1::'"),
    ('1::2::3', "At most one '::' permitted in '1::2::3'"),
    ('1:2:3:4:5:6:7:8:9', "Exactly 8 parts expected without '::' in '1:2:3:4:5:6:7:8:9'"),
    ('1:2:3:4:5:6:7', "Exactly 8 parts expected without '::' in '1:2:3:4:5:6:7'"),
    ('::g', "Only hex digits permitted in 'g' in '::g'"),
    ('::12345', "At most 4 characters permitted in '12345' in '::12345'"),
    ('1:2:3:4:5:6:1.2.3', "Expected 4 octets in '1.2.3' in '1:2:3:4:5:6:1.2.3'"),
    ('fe80::1%', 'Invalid IPv6 address: "\'fe80::1%\'"'),
    ('fe80::1%a%b', 'Invalid IPv6 address: "\'fe80::1%a%b\'"'),
    ('1', "At least 3 parts expected in '1'"),
    (':::', "At most one '::' permitted in ':::'"),
    ('x' * 46, "At most 45 characters expected in '" + 'x' * 46 + "'"),
    (2**128, '340282366920938463463374607431768211456 (>= 2**128) is not permitted as an IPv6 address'),
    (b'ab', "b'ab' (len 2 != 16) is not permitted as an IPv6 address"),
]:
    try:
        IPv6Address(_bad)
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == _msg
for _bad, _msg in [
    ('1.2.3', "'1.2.3' does not appear to be an IPv4 or IPv6 address"),
    ('abc', "'abc' does not appear to be an IPv4 or IPv6 address"),
    (-1, '-1 does not appear to be an IPv4 or IPv6 address'),
    (None, 'None does not appear to be an IPv4 or IPv6 address'),
    (b'\x01\x02\x03', "b'\\x01\\x02\\x03' does not appear to be an IPv4 or IPv6 address"),
]:
    try:
        ip_address(_bad)
        assert False, 'expected ValueError'
    except ValueError as e:
        assert str(e) == _msg
try:
    ip_address('1.2.3.4') < IPv6Address('::1')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '1.2.3.4 and ::1 are not of the same version'
try:
    ip_address('255.255.255.255') + 1
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == '4294967296 (>= 2**32) is not permitted as an IPv4 address'
try:
    ip_address('0.0.0.0') - 1
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == '-1 (< 0) is not permitted as an IPv4 address'
try:
    ip_address('1.2.3.4') + 1.5
    assert False, 'expected TypeError'
except TypeError:
    pass

# === networks ===
_n = ip_network('192.168.1.0/24')
assert isinstance(_n, IPv4Network)
assert repr(_n) == "IPv4Network('192.168.1.0/24')"
assert str(_n) == '192.168.1.0/24'
assert (_n.network_address, _n.broadcast_address) == (IPv4Address('192.168.1.0'), IPv4Address('192.168.1.255'))
assert (_n.netmask, _n.hostmask) == (IPv4Address('255.255.255.0'), IPv4Address('0.0.0.255'))
assert (_n.prefixlen, _n.num_addresses, _n.version, _n.max_prefixlen) == (24, 256, 4, 32)
assert _n.with_prefixlen == '192.168.1.0/24'
assert _n.with_netmask == '192.168.1.0/255.255.255.0'
assert _n.with_hostmask == '192.168.1.0/0.0.0.255'
assert (_n.compressed, _n.exploded) == ('192.168.1.0/24', '192.168.1.0/24')
assert (_n.is_private, _n.is_global, _n.is_multicast, _n.is_loopback) == (True, False, False, False)
assert ip_network('192.168.1.0/255.255.255.0') == _n
assert ip_network('192.168.1.0/0.0.0.255') == _n
assert IPv4Network(('192.168.1.0', 24)) == _n
assert IPv4Network(('192.168.1.0', '24')) == _n
assert IPv4Network(('192.168.1.0', '255.255.255.0')) == _n
assert IPv4Network(('192.168.1.1', 24), False) == _n
assert IPv4Network((3232235776, 24)) == _n
assert ip_network('192.168.1.1/24', strict=False) == _n
assert ip_network('10.0.0.1') == IPv4Network('10.0.0.1/32')
assert IPv4Network(167772160) == IPv4Network('10.0.0.0/32')
assert IPv4Network(b'\x0a\x00\x00\x00') == IPv4Network('10.0.0.0/32')
assert IPv4Network(('10.0.0.0',)) == IPv4Network('10.0.0.0/32')
assert _n != ip_network('192.168.1.0/25')
assert _n != '192.168.1.0/24'
assert hash(_n) == hash(IPv4Network('192.168.1.0/24'))
assert ip_network('10.0.0.0/8') < ip_network('10.0.0.0/16')
assert ip_network('10.0.0.0/8') < ip_network('11.0.0.0/8')
assert sorted([ip_network('10.0.0.0/16'), ip_network('10.0.0.0/8'), ip_network('9.0.0.0/8')]) == [
    IPv4Network('9.0.0.0/8'),
    IPv4Network('10.0.0.0/8'),
    IPv4Network('10.0.0.0/16'),
]

# membership, indexing and iteration
assert ip_address('192.168.1.77') in _n
assert ip_address('192.168.2.1') not in _n
assert ip_address('::1') not in _n
assert ip_network('192.168.1.0/25') not in _n
assert (_n[0], _n[1], _n[-1]) == (IPv4Address('192.168.1.0'), IPv4Address('192.168.1.1'), IPv4Address('192.168.1.255'))
assert _n[-256] == IPv4Address('192.168.1.0')
assert [str(h) for h in list(_n.hosts())[:3]] == ['192.168.1.1', '192.168.1.2', '192.168.1.3']
assert len(list(_n.hosts())) == 254
assert [str(h) for h in list(_n)[:3]] == ['192.168.1.0', '192.168.1.1', '192.168.1.2']
assert len(list(_n)) == 256
assert [str(h) for h in ip_network('10.0.0.0/31').hosts()] == ['10.0.0.0', '10.0.0.1']
assert [str(h) for h in ip_network('10.0.0.0/32').hosts()] == ['10.0.0.0']
assert [str(h) for h in ip_network('2001:db8::/127').hosts()] == ['2001:db8::', '2001:db8::1']
assert [str(h) for h in ip_network('2001:db8::/126').hosts()] == ['2001:db8::1', '2001:db8::2', '2001:db8::3']
assert type(_n.hosts()).__name__ == 'generator'
_hosts = _n.hosts()
assert iter(_hosts) is _hosts
assert next(_hosts) == IPv4Address('192.168.1.1')
assert [str(x) for _, x in zip(range(2), ip_network('::/0'))] == ['::', '::1']

# subnets and supernets
assert [str(x) for x in _n.subnets()] == ['192.168.1.0/25', '192.168.1.128/25']
assert [str(x) for x in _n.subnets(2)] == ['192.168.1.0/26', '192.168.1.64/26', '192.168.1.128/26', '192.168.1.192/26']
assert [str(x) for x in _n.subnets(new_prefix=26)] == [str(x) for x in _n.subnets(2)]
assert [str(x) for x in ip_network('10.0.0.0/32').subnets()] == ['10.0.0.0/32']
assert [str(x) for _, x in zip(range(2), ip_network('::/0').subnets())] == ['::/1', '8000::/1']
assert type(_n.subnets()).__name__ == 'generator'
assert str(_n.supernet()) == '192.168.0.0/23'
assert str(_n.supernet(2)) == '192.168.0.0/22'
assert str(_n.supernet(new_prefix=16)) == '192.168.0.0/16'
assert str(ip_network('0.0.0.0/0').supernet()) == '0.0.0.0/0'
assert str(ip_network('10.0.0.0/8').supernet(2)) == '8.0.0.0/6'

# relations
assert ip_network('10.0.0.0/8').overlaps(ip_network('10.1.0.0/16'))
assert ip_network('10.1.0.0/16').overlaps(ip_network('10.0.0.0/8'))
assert not ip_network('10.0.0.0/8').overlaps(ip_network('11.0.0.0/8'))
assert not ip_network('10.0.0.0/8').overlaps(ip_network('::/8'))
assert ip_network('10.1.0.0/16').subnet_of(ip_network('10.0.0.0/8'))
assert not ip_network('10.0.0.0/8').subnet_of(ip_network('10.1.0.0/16'))
assert ip_network('10.0.0.0/8').supernet_of(ip_network('10.1.0.0/16'))
assert [str(x) for x in ip_network('10.0.0.0/8').address_exclude(ip_network('10.1.0.0/16'))] == [
    '10.128.0.0/9',
    '10.64.0.0/10',
    '10.32.0.0/11',
    '10.16.0.0/12',
    '10.8.0.0/13',
    '10.4.0.0/14',
    '10.2.0.0/15',
    '10.0.0.0/16',
]
assert list(ip_network('10.0.0.0/8').address_exclude(ip_network('10.0.0.0/8'))) == []
assert ip_network('10.0.0.0/8').compare_networks(ip_network('10.0.0.0/16')) == -1
assert ip_network('10.0.0.0/8').compare_networks(ip_network('10.0.0.0/8')) == 0
assert ip_network('11.0.0.0/8').compare_networks(ip_network('10.0.0.0/8')) == 1

# IPv6 networks
_m = ip_network('2001:db8::/32')
assert isinstance(_m, IPv6Network)
assert repr(_m) == "IPv6Network('2001:db8::/32')"
assert (_m.network_address, _m.broadcast_address) == (
    IPv6Address('2001:db8::'),
    IPv6Address('2001:db8:ffff:ffff:ffff:ffff:ffff:ffff'),
)
assert str(_m.netmask) == 'ffff:ffff::'
assert str(_m.hostmask) == '::ffff:ffff:ffff:ffff:ffff:ffff'
assert (_m.prefixlen, _m.num_addresses) == (32, 2**96)
assert _m.exploded == '2001:0db8:0000:0000:0000:0000:0000:0000/32'
assert ip_network('::ffff:0:0/96').exploded == '0000:0000:0000:0000:0000:ffff:0000:0000/96'
assert str(ip_network('::ffff:0:0/96')) == '::ffff:0.0.0.0/96'
assert (_m.is_private, _m.is_global, _m.is_site_local) == (True, False, False)
assert ip_network('fec0::/10').is_site_local
assert ip_network('::/0').num_addresses == 2**128
assert ip_network('::/0')[2**128 - 1] == IPv6Address('ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff')
assert ip_network('2001:db8::/64')[2**64 - 1] == IPv6Address('2001:db8::ffff:ffff:ffff:ffff')
assert IPv6Network(('2001:db8::', 32)) == _m
assert ip_network('2001:db8::1/32', False) == _m
for _text, _props in [
    ('10.0.0.0/8', (False, True, False, False, False, False, False)),
    ('0.0.0.0/0', (False, False, True, False, False, False, False)),
    ('100.64.0.0/10', (False, False, False, False, False, False, False)),
    ('172.16.0.0/12', (False, True, False, False, False, False, False)),
    ('192.0.0.0/29', (False, True, False, False, False, False, False)),
    ('192.0.0.8/31', (False, False, True, False, False, False, False)),
    ('224.0.0.0/4', (True, False, True, False, False, False, False)),
    ('127.0.0.0/8', (False, True, False, False, True, False, False)),
    ('fe80::/10', (False, True, False, False, False, True, False)),
    ('2001::/23', (False, True, False, False, False, False, False)),
    ('2001:20::/28', (False, False, True, False, False, False, False)),
    ('::/0', (False, False, True, False, False, False, False)),
]:
    _net = ip_network(_text)
    assert (
        _net.is_multicast,
        _net.is_private,
        _net.is_global,
        _net.is_reserved,
        _net.is_loopback,
        _net.is_link_local,
        _net.is_unspecified,
    ) == _props

# === network errors ===
for _call, _exc, _msg in [
    (lambda: ip_network('10.0.0.1/8'), ValueError, '10.0.0.1/8 has host bits set'),
    (lambda: IPv4Network('10.0.0.1/8'), ValueError, '10.0.0.1/8 has host bits set'),
    (lambda: IPv6Network('2001:db8::1/32'), ValueError, '2001:db8::1/32 has host bits set'),
    (lambda: IPv4Network(('10.0.0.0', 33)), ValueError, '33 is not a valid netmask'),
    (lambda: IPv4Network(('10.0.0.0', -1)), ValueError, '-1 is not a valid netmask'),
    (lambda: IPv4Network('10.0.0.0/33'), ValueError, "'33' is not a valid netmask"),
    (lambda: IPv4Network('10.0.0.0/-1'), ValueError, "'-1' is not a valid netmask"),
    (lambda: IPv4Network('10.0.0.0/abc'), ValueError, "'abc' is not a valid netmask"),
    (lambda: IPv4Network('10.0.0.0/255.0.255.0'), ValueError, "'255.0.255.0' is not a valid netmask"),
    (lambda: IPv4Network('10.0.0.0/8/8'), ValueError, "Only one '/' permitted in '10.0.0.0/8/8'"),
    (lambda: IPv4Network('10.0.0.256/8'), ValueError, "Octet 256 (> 255) not permitted in '10.0.0.256'"),
    (lambda: IPv4Network('::1/128'), ValueError, "Expected 4 octets in '::1'"),
    (lambda: IPv6Network('2001:db8::/129'), ValueError, "'129' is not a valid netmask"),
    (lambda: IPv6Network('2001:db8::/ffff::'), ValueError, "'ffff::' is not a valid netmask"),
    (lambda: ip_network('10.0.0.0/8/8'), ValueError, "'10.0.0.0/8/8' does not appear to be an IPv4 or IPv6 network"),
    (lambda: ip_network('10.0.0.0/33'), ValueError, "'10.0.0.0/33' does not appear to be an IPv4 or IPv6 network"),
    (lambda: ip_network(None), ValueError, 'None does not appear to be an IPv4 or IPv6 network'),
    (
        lambda: ip_network('10.0.0.0/8') < ip_network('::/8'),
        TypeError,
        '10.0.0.0/8 and ::/8 are not of the same version',
    ),
    (lambda: _n[256], IndexError, 'address out of range'),
    (lambda: _n[-257], IndexError, 'address out of range'),
    (lambda: ip_network('::/0')[2**128], IndexError, 'address out of range'),
    (lambda: list(ip_network('10.0.0.0/8').subnets(-1)), ValueError, 'prefix length diff must be > 0'),
    (
        lambda: list(ip_network('10.0.0.0/8').subnets(25)),
        ValueError,
        'prefix length diff 33 is invalid for netblock 10.0.0.0/8',
    ),
    (lambda: list(ip_network('10.0.0.0/8').subnets(new_prefix=4)), ValueError, 'new prefix must be longer'),
    (
        lambda: list(ip_network('10.0.0.0/8').subnets(2, new_prefix=12)),
        ValueError,
        'cannot set prefixlen_diff and new_prefix',
    ),
    (
        lambda: ip_network('10.0.0.0/8').supernet(9),
        ValueError,
        'current prefixlen is 8, cannot have a prefixlen_diff of 9',
    ),
    (lambda: ip_network('10.0.0.0/8').supernet(new_prefix=9), ValueError, 'new prefix must be shorter'),
    (
        lambda: ip_network('10.0.0.0/8').supernet(2, new_prefix=4),
        ValueError,
        'cannot set prefixlen_diff and new_prefix',
    ),
    (
        lambda: ip_network('10.0.0.0/8').subnet_of(ip_network('::/8')),
        TypeError,
        '10.0.0.0/8 and ::/8 are not of the same version',
    ),
    (
        lambda: ip_network('10.0.0.0/8').subnet_of(ip_address('10.0.0.1')),
        TypeError,
        'Unable to test subnet containment between 10.0.0.0/8 and 10.0.0.1',
    ),
    (
        lambda: ip_network('10.0.0.0/8').supernet_of(5),
        TypeError,
        'Unable to test subnet containment between 5 and 10.0.0.0/8',
    ),
    (
        lambda: list(ip_network('10.0.0.0/8').address_exclude(ip_network('11.0.0.0/8'))),
        ValueError,
        '11.0.0.0/8 not contained in 10.0.0.0/8',
    ),
    (
        lambda: list(ip_network('10.0.0.0/8').address_exclude(ip_network('::/8'))),
        TypeError,
        '10.0.0.0/8 and ::/8 are not of the same version',
    ),
    (
        lambda: list(ip_network('10.0.0.0/8').address_exclude(ip_address('10.0.0.1'))),
        TypeError,
        '10.0.0.1 is not a network object',
    ),
    (
        lambda: ip_network('11.0.0.0/8').compare_networks(ip_network('::/8')),
        TypeError,
        '11.0.0.0/8 and ::/8 are not of the same type',
    ),
    (lambda: '10.0.0.1' in ip_network('10.0.0.0/8'), AttributeError, "'str' object has no attribute 'version'"),
    (lambda: 5 in ip_network('10.0.0.0/8'), AttributeError, "'int' object has no attribute 'version'"),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg

# the exception names exist and catch the constructor errors
try:
    IPv4Address('1.2.3')
    assert False, 'expected AddressValueError'
except ipaddress.AddressValueError as e:
    assert str(e) == "Expected 4 octets in '1.2.3'"
try:
    IPv4Network('1.2.3.0/33')
    assert False, 'expected NetmaskValueError'
except ipaddress.NetmaskValueError as e:
    assert str(e) == "'33' is not a valid netmask"

# === module functions ===
assert [str(x) for x in ipaddress.summarize_address_range(IPv4Address('192.0.2.0'), IPv4Address('192.0.2.130'))] == [
    '192.0.2.0/25',
    '192.0.2.128/31',
    '192.0.2.130/32',
]
assert [str(x) for x in ipaddress.summarize_address_range(IPv4Address('0.0.0.0'), IPv4Address('255.255.255.255'))] == [
    '0.0.0.0/0'
]
assert [str(x) for x in ipaddress.summarize_address_range(IPv6Address('::'), IPv6Address('::ff'))] == ['::/120']
assert [str(x) for x in ipaddress.collapse_addresses([IPv4Network('192.0.2.0/25'), IPv4Network('192.0.2.128/25')])] == [
    '192.0.2.0/24'
]
assert [
    str(x)
    for x in ipaddress.collapse_addresses(
        [IPv4Network('192.0.2.0/25'), IPv4Network('192.0.2.0/24'), IPv4Network('10.0.0.0/8')]
    )
] == ['10.0.0.0/8', '192.0.2.0/24']
assert [
    str(x)
    for x in ipaddress.collapse_addresses(
        [
            IPv4Address('1.1.1.1'),
            IPv4Address('1.1.1.2'),
            IPv4Address('1.1.1.3'),
            IPv4Network('1.1.1.4/32'),
            IPv4Address('1.1.1.0'),
        ]
    )
] == ['1.1.1.0/30', '1.1.1.4/32']
assert list(ipaddress.collapse_addresses([])) == []
assert ipaddress.v4_int_to_packed(16909060) == b'\x01\x02\x03\x04'
assert ipaddress.v6_int_to_packed(1) == b'\x00' * 15 + b'\x01'
assert (ipaddress.IPV4LENGTH, ipaddress.IPV6LENGTH) == (32, 128)
for _call, _exc, _msg in [
    (
        lambda: list(ipaddress.summarize_address_range(IPv4Address('1.0.0.1'), IPv4Address('1.0.0.0'))),
        ValueError,
        'last IP address must be greater than first',
    ),
    (
        lambda: list(ipaddress.summarize_address_range(IPv4Address('1.0.0.1'), IPv6Address('::1'))),
        TypeError,
        '1.0.0.1 and ::1 are not of the same version',
    ),
    (
        lambda: list(ipaddress.summarize_address_range(IPv4Network('1.0.0.0/8'), IPv4Address('1.0.0.0'))),
        TypeError,
        'first and last must be IP addresses, not networks',
    ),
    (
        lambda: list(ipaddress.collapse_addresses([IPv4Address('1.1.1.1'), IPv6Address('::1')])),
        TypeError,
        '::1 and 1.1.1.1 are not of the same version',
    ),
    (
        lambda: list(ipaddress.collapse_addresses([IPv4Network('1.0.0.0/8'), IPv6Network('::/8')])),
        TypeError,
        '::/8 and 1.0.0.0/8 are not of the same version',
    ),
    (lambda: ipaddress.v4_int_to_packed(2**32), ValueError, 'Address negative or too large for IPv4'),
    (lambda: ipaddress.v4_int_to_packed(-1), ValueError, 'Address negative or too large for IPv4'),
    (lambda: ipaddress.v6_int_to_packed(2**128), ValueError, 'Address negative or too large for IPv6'),
    (lambda: ipaddress.v4_int_to_packed('x'), AttributeError, "'str' object has no attribute 'to_bytes'"),
    (
        lambda: ip_address(),
        TypeError,
        "ip_address() missing 1 required positional argument: 'address'",
    ),
]:
    try:
        _call()
        assert False, 'expected an error'
    except _exc as e:
        assert str(e) == _msg
assert ip_network(address='1.2.3.0/24', strict=False) == IPv4Network('1.2.3.0/24')
assert ip_address(address='1.2.3.4') == IPv4Address('1.2.3.4')
