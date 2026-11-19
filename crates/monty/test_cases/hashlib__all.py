import hashlib

# === known digests ===
assert hashlib.md5(b'').hexdigest() == 'd41d8cd98f00b204e9800998ecf8427e'
assert hashlib.md5(b'abc').hexdigest() == '900150983cd24fb0d6963f7d28e17f72'
assert hashlib.sha1(b'').hexdigest() == 'da39a3ee5e6b4b0d3255bfef95601890afd80709'
assert hashlib.sha1(b'abc').hexdigest() == 'a9993e364706816aba3e25717850c26c9cd0d89d'
assert hashlib.sha224(b'abc').hexdigest() == '23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7'
assert hashlib.sha256(b'').hexdigest() == 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
assert hashlib.sha256(b'abc').hexdigest() == 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
assert (
    hashlib.sha256(b'abc').digest()
    == b'\xbax\x16\xbf\x8f\x01\xcf\xeaAA@\xde]\xae"#\xb0\x03a\xa3\x96\x17z\x9c\xb4\x10\xffa\xf2\x00\x15\xad'
)
assert (
    hashlib.sha384(b'abc').hexdigest()
    == 'cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7'
)
assert (
    hashlib.sha512(b'abc').hexdigest()
    == 'ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f'
)
assert hashlib.sha3_224(b'abc').hexdigest() == 'e642824c3f8cf24ad09234ee7d3c766fc9a3a5168d0c94ad73b46fdf'
assert hashlib.sha3_256(b'abc').hexdigest() == '3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532'
assert (
    hashlib.sha3_384(b'abc').hexdigest()
    == 'ec01498288516fc926459f58e2c6ad8df9b473cb0fc08c2596da7cf0e49be4b298d88cea927ac7f539f1edf228376d25'
)
assert (
    hashlib.sha3_512(b'abc').hexdigest()
    == 'b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0'
)
assert hashlib.shake_128(b'abc').hexdigest(16) == '5881092dd818bf5cf8a3ddb793fbcba7'
assert hashlib.shake_128(b'abc').digest(5) == b'X\x81\t-\xd8'
assert hashlib.shake_256(b'abc').hexdigest(16) == '483366601360a8771c6863080cc4114d'
assert hashlib.shake_128(b'abc').digest(0) == b''
assert hashlib.shake_128(b'abc').hexdigest(0) == ''
assert hashlib.shake_128(b'abc').digest(True) == b'X'
assert hashlib.shake_128(b'abc').digest(length=3) == b'X\x81\t'
assert hashlib.shake_128(b'abc').hexdigest(length=3) == '588109'
# a squeeze longer than the rate keeps permuting
long_shake = hashlib.shake_128(b'abc').hexdigest(200)
assert len(long_shake) == 400
assert long_shake[:32] == '5881092dd818bf5cf8a3ddb793fbcba7'
assert long_shake[-16:] == '4818cb006aa5b4cd'
assert hashlib.shake_128(b'abc').digest(200)[:5] == hashlib.shake_128(b'abc').digest(5)
assert (
    hashlib.blake2b(b'abc').hexdigest()
    == 'ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923'
)
assert hashlib.blake2s(b'abc').hexdigest() == '508c5e8c327c14e2e1a72ba34eeb452f37458b209ed63a294d999b4c86675982'
assert hashlib.blake2b(b'').hexdigest()[:16] == '786a02f742015903'
assert hashlib.blake2s(b'').hexdigest()[:16] == '69217a3079908094'

# === the same stream through every block boundary ===
# MD5, SHA-1 and SHA-256 use 64-byte blocks, SHA-512 and BLAKE2b 128, BLAKE2s
# 64, SHA3-256 136 and SHAKE128 168, so a run over 0..300 bytes crosses each.
_hex = '0123456789abcdef'
every_byte = b''
for _i in range(512):
    every_byte += bytes.fromhex(_hex[(_i % 256) // 16] + _hex[_i % 16])
samples = {
    0: ('d41d8cd9', 'da39a3ee', 'e3b0c442', 'cf83e135', 'a7ffc6f8', '786a02f7', '69217a30', '7f9c2ba4'),
    7: ('9aa461e1', '6dc86f11', '57355ac3', 'b7c0b47f', '59b1add3', '8f945ba7', '598001fa', '4652c0ae'),
    56: ('51fdd1ac', '636e2ec6', 'da2ae4d6', '8b12b2f6', 'd192f596', '26cca012', 'e290dd27', '79504532'),
    63: ('48a62952', '6d942da0', '29af2686', '9dc9c559', 'ba7af58d', 'd10bf9a1', 'e57cb794', 'e8c2c5be'),
    70: ('5f1f5f64', 'c2488792', '5767d69a', '10164cfd', '97a26b0e', '45813f44', '657b09f3', 'b4d20a68'),
    112: ('d1fec2ac', 'e4ce142d', '09373f12', 'c5fbd731', '575f1807', '877fd652', '81dcc067', 'c258563e'),
    126: ('80749be0', 'a271f715', '5dda7cb7', '2681bf91', 'ee257791', 'e0721e02', '38c410f5', '05316363'),
    133: ('8b683106', '54152ac7', 'aacb65e7', '3f80b7bf', '721f0e93', 'e59b9987', 'f29b1b1a', '31ec6b81'),
    140: ('780c43f8', '25fb08a7', 'b4a4e5d6', '406a5382', '3a81a47e', '79fe2fe1', '7410d42d', 'd9d1e1f6'),
    168: ('0ad93860', '9af88ed1', '7f7193dd', '1ee6ca08', '369a33ba', '5ce1042a', '715c99c7', 'f15277eb'),
    175: ('cc91c083', '5ada1346', 'b7e3c3ea', '857be648', 'c913434c', 'd5e93873', '38c53dfb', '48dffc47'),
    294: ('34dc9a63', 'ac2369f5', 'fd9d87b9', 'd5b3efe8', '755352de', '96f47647', '01a5b895', 'f27cbc47'),
}
for n, expected in samples.items():
    data = every_byte[:n]
    got = (
        hashlib.md5(data).hexdigest()[:8],
        hashlib.sha1(data).hexdigest()[:8],
        hashlib.sha256(data).hexdigest()[:8],
        hashlib.sha512(data).hexdigest()[:8],
        hashlib.sha3_256(data).hexdigest()[:8],
        hashlib.blake2b(data).hexdigest()[:8],
        hashlib.blake2s(data).hexdigest()[:8],
        hashlib.shake_128(data).hexdigest(4),
    )
    assert got == expected

# === incremental updates equal one-shot hashing ===
for name in (
    'md5',
    'sha1',
    'sha224',
    'sha256',
    'sha384',
    'sha512',
    'sha3_224',
    'sha3_384',
    'sha3_512',
    'blake2b',
    'blake2s',
):
    whole = hashlib.new(name, every_byte).hexdigest()
    h = hashlib.new(name)
    for i in range(0, 512, 37):
        h.update(every_byte[i : i + 37])
    assert h.hexdigest() == whole
    # the digest does not consume the state
    assert h.hexdigest() == whole
    assert h.digest().hex() == whole
    h.update(b'')
    assert h.hexdigest() == whole
    assert hashlib.new(name, every_byte[:10]).update(every_byte[10:]) is None
h = hashlib.shake_256()
h.update(every_byte[:200])
h.update(every_byte[200:])
assert h.hexdigest(10) == hashlib.shake_256(every_byte).hexdigest(10)

# === copy() ===
h = hashlib.sha256(b'abc')
h2 = h.copy()
h.update(b'x')
assert h.hexdigest() == hashlib.sha256(b'abcx').hexdigest()
assert h2.hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert h2 is not h
assert type(h2) is type(h)
hb = hashlib.blake2b(b'abc', digest_size=16, key=b'k')
hb2 = hb.copy()
hb.update(b'more')
assert hb2.hexdigest() == hashlib.blake2b(b'abc', digest_size=16, key=b'k').hexdigest()
assert hb.hexdigest() == hashlib.blake2b(b'abcmore', digest_size=16, key=b'k').hexdigest()
assert hb2.digest_size == 16
sh = hashlib.shake_128(b'abc')
sh2 = sh.copy()
sh.update(b'x')
assert sh2.hexdigest(4) == hashlib.shake_128(b'abc').hexdigest(4)

# === attributes ===
sizes = {
    'md5': (16, 64),
    'sha1': (20, 64),
    'sha224': (28, 64),
    'sha256': (32, 64),
    'sha384': (48, 128),
    'sha512': (64, 128),
    'sha3_224': (28, 144),
    'sha3_256': (32, 136),
    'sha3_384': (48, 104),
    'sha3_512': (64, 72),
    'shake_128': (0, 168),
    'shake_256': (0, 136),
    'blake2b': (64, 128),
    'blake2s': (32, 64),
}
for name, (digest_size, block_size) in sizes.items():
    h = getattr(hashlib, name)()
    assert h.name == name
    assert h.digest_size == digest_size
    assert h.block_size == block_size
    assert hashlib.new(name).name == name
    if digest_size:
        assert len(h.digest()) == digest_size
        assert len(h.hexdigest()) == 2 * digest_size

# === repr and type ===
assert repr(hashlib.sha256()).startswith('<sha256 _hashlib.HASH object @ 0x')
assert repr(hashlib.md5()).startswith('<md5 _hashlib.HASH object @ 0x')
assert repr(hashlib.sha3_256()).startswith('<sha3_256 _hashlib.HASH object @ 0x')
assert repr(hashlib.shake_128()).startswith('<shake_128 _hashlib.HASHXOF object @ 0x')
assert repr(hashlib.blake2b()).startswith('<_blake2.blake2b object at 0x')
assert repr(hashlib.blake2s()).startswith('<_blake2.blake2s object at 0x')
assert str(type(hashlib.sha256())) == "<class '_hashlib.HASH'>"
assert str(type(hashlib.sha3_512())) == "<class '_hashlib.HASH'>"
assert str(type(hashlib.shake_256())) == "<class '_hashlib.HASHXOF'>"
assert str(type(hashlib.blake2b())) == "<class '_blake2.blake2b'>"
assert str(type(hashlib.blake2s())) == "<class '_blake2.blake2s'>"
assert type(hashlib.blake2b()) is hashlib.blake2b
assert isinstance(hashlib.blake2s(), hashlib.blake2s)
assert not isinstance(hashlib.blake2s(), hashlib.blake2b)
assert type(hashlib.sha256()) is type(hashlib.md5())
assert type(hashlib.shake_128()) is not type(hashlib.sha256())
h = hashlib.sha256()
assert h == h
assert h != hashlib.sha256()
assert hash(h) == hash(h)
assert bool(h)
assert str(h) == repr(h)

# === blake2 class constants ===
assert hashlib.blake2b.MAX_DIGEST_SIZE == 64
assert hashlib.blake2b.MAX_KEY_SIZE == 64
assert hashlib.blake2b.SALT_SIZE == 16
assert hashlib.blake2b.PERSON_SIZE == 16
assert hashlib.blake2s.MAX_DIGEST_SIZE == 32
assert hashlib.blake2s.MAX_KEY_SIZE == 32
assert hashlib.blake2s.SALT_SIZE == 8
assert hashlib.blake2s.PERSON_SIZE == 8
assert hashlib.blake2b().MAX_DIGEST_SIZE == 64
assert hashlib.blake2s().SALT_SIZE == 8

# === blake2 parameters ===
assert hashlib.blake2b(b'abc', digest_size=16).hexdigest() == 'cf4ab791c62b8d2b2109c90275287816'
assert hashlib.blake2b(b'abc', digest_size=1).hexdigest() == '6b'
assert hashlib.blake2b(b'abc', digest_size=1).hexdigest() != hashlib.blake2b(b'abc').hexdigest()[:2]
assert hashlib.blake2s(b'abc', digest_size=16).hexdigest() == 'aa4938119b1dc7b87cbad0ffd200d0ae'
assert (
    hashlib.blake2b(b'abc', key=b'k', salt=b's', person=b'p').hexdigest()
    == 'e84dbf3bcc0834d7de3820e9d6fe51d90805be8ffd9502b9d3b00fa5250b715c7bc891befc79967a50ca512ea11c7b91dc2b8bb1a1e22b4556587039de9ba31a'
)
assert hashlib.blake2b(key=b'k').hexdigest()[:16] == 'a393a0e4093eea8b'
assert hashlib.blake2b(b'x' * 128, key=b'k').hexdigest()[:2] == '47'
assert hashlib.blake2b(key=b'k' * 64).hexdigest() == hashlib.blake2b(b'', key=b'k' * 64).hexdigest()
assert hashlib.blake2s(key=b'k' * 32, salt=b's' * 8, person=b'p' * 8).digest_size == 32
assert hashlib.blake2b(fanout=0).hexdigest()[:8] == 'af166f0b'
assert hashlib.blake2b(last_node=True).hexdigest()[:16] == '05cc8cc53183c6fb'
assert hashlib.blake2b(depth=2).hexdigest()[:16] == 'e53fa85be194c204'
assert (
    hashlib.blake2b(
        fanout=1, depth=1, leaf_size=0, node_offset=0, node_depth=0, inner_size=0, last_node=False
    ).hexdigest()[:16]
    == '786a02f742015903'
)
assert hashlib.blake2b(usedforsecurity=False).hexdigest()[:16] == '786a02f742015903'
assert hashlib.blake2b(data=b'x').hexdigest()[:16] == '0909377ad35110ca'
assert hashlib.blake2b(string=b'x').hexdigest()[:16] == '0909377ad35110ca'
assert hashlib.blake2b(digest_size=True).digest_size == 1
assert hashlib.blake2b(digest_size=5).digest_size == 5
assert len(hashlib.blake2b(digest_size=5).digest()) == 5
assert hashlib.blake2b(b'x', digest_size=5).name == 'blake2b'
assert (
    hashlib.blake2b(
        node_offset=2**64 - 1,
        fanout=255,
        depth=255,
        leaf_size=2**32 - 1,
        node_depth=255,
        inner_size=64,
        last_node=True,
        key=b'k' * 64,
        salt=b's' * 16,
        person=b'p' * 16,
        digest_size=1,
    ).hexdigest()
    == 'ea'
)
assert (
    hashlib.blake2s(
        node_offset=2**48 - 1,
        fanout=255,
        depth=255,
        leaf_size=2**32 - 1,
        node_depth=255,
        inner_size=32,
        last_node=True,
        key=b'k' * 32,
        salt=b's' * 8,
        person=b'p' * 8,
        digest_size=1,
    ).hexdigest()
    == '97'
)
assert hashlib.blake2s(node_offset=2**48 - 1).hexdigest()[:8] == 'ddcb74ee'
assert hashlib.blake2s(leaf_size=2**32 - 1).hexdigest()[:8] == '55276ea8'
assert hashlib.blake2b(last_node='x').hexdigest() == hashlib.blake2b(last_node=True).hexdigest()
assert hashlib.blake2b(last_node=[]).hexdigest() == hashlib.blake2b().hexdigest()

# === new() ===
assert hashlib.new('sha256', b'abc').hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert hashlib.new('SHA256', b'abc').hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert hashlib.new('sha-256').name == 'sha256'
assert hashlib.new('SHA-512').name == 'sha512'
assert hashlib.new('Md5').name == 'md5'
assert hashlib.new('sha3-256').name == 'sha3_256'
assert hashlib.new('shake128').name == 'shake_128'
assert hashlib.new('blake2b512', b'a').hexdigest() == hashlib.blake2b(b'a').hexdigest()
assert hashlib.new('blake2s256').name == 'blake2s'
assert hashlib.new(name='sha256', data=b'abc').hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert hashlib.new('sha256', string=b'abc').hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert hashlib.new('sha256', b'abc', usedforsecurity=False).hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert hashlib.new('shake_128', b'abc').hexdigest(4) == '5881092d'
assert hashlib.new('blake2b', digest_size=16).hexdigest() == 'cae66941d9efbd404e4d88758ea67670'
assert hashlib.new('blake2b', b'a', digest_size=16).hexdigest() == '27c35e6e9373877f29e562464e46497e'
assert hashlib.new('blake2b', string=b'a').hexdigest()[:8] == '333fcb4e'
assert hashlib.new('blake2s', b'abc', key=b'k').hexdigest() == hashlib.blake2s(b'abc', key=b'k').hexdigest()
assert hashlib.new('blake2b', usedforsecurity=False).hexdigest()[:8] == '786a02f7'
assert type(hashlib.new('blake2s')) is hashlib.blake2s

# === constructor keywords ===
assert hashlib.sha256(data=b'abc').hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert hashlib.sha256(string=b'abc').hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert hashlib.sha256(b'abc', usedforsecurity=False).hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert hashlib.sha256(usedforsecurity='x').hexdigest()[:8] == 'e3b0c442'
assert hashlib.sha256(b'a', usedforsecurity=[]).hexdigest()[:8] == 'ca978112'
assert hashlib.sha256(usedforsecurity=False, data=b'a').hexdigest()[:8] == 'ca978112'
assert hashlib.md5(usedforsecurity=False).hexdigest() == 'd41d8cd98f00b204e9800998ecf8427e'

# === algorithms ===
guaranteed = {
    'blake2b',
    'blake2s',
    'md5',
    'sha1',
    'sha224',
    'sha256',
    'sha384',
    'sha3_224',
    'sha3_256',
    'sha3_384',
    'sha3_512',
    'sha512',
    'shake_128',
    'shake_256',
}
assert hashlib.algorithms_guaranteed == guaranteed
assert guaranteed.issubset(hashlib.algorithms_available)
assert type(hashlib.algorithms_guaranteed) is set
assert type(hashlib.algorithms_available) is set
assert hashlib.algorithms_guaranteed is not hashlib.algorithms_available
for name in guaranteed:
    assert hashlib.new(name).name == name

# === pbkdf2_hmac ===
assert (
    hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1000).hex()
    == '0a38253555ce37f5c72a6b703f996814ebf241f203af146e93dcdeb031c5567e'
)
assert hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1000, 16).hex() == '0a38253555ce37f5c72a6b703f996814'
assert hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1000, dklen=16).hex() == '0a38253555ce37f5c72a6b703f996814'
assert (
    hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, None).hex()
    == '6f4ad8c78ec365c060e648eb694ee40dea58484b0371fbd61715ac4410b7380a'
)
assert hashlib.pbkdf2_hmac(hash_name='sha256', password=b'pw', salt=b'salt', iterations=1, dklen=4).hex() == '6f4ad8c7'
assert hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', True, 4).hex() == '6f4ad8c7'
assert hashlib.pbkdf2_hmac('sha256', b'pw', b'salt', 1, True).hex() == '6f'
assert hashlib.pbkdf2_hmac('SHA-256', b'pw', b'salt', 3, 4).hex() == '3c671234'
assert (
    hashlib.pbkdf2_hmac('sha256', b'', b'', 1).hex()
    == 'f7ce0b653d2d72a4108cf5abe912ffdd777616dbbb27a70e8204f3ae2d0f6fad'
)
assert hashlib.pbkdf2_hmac('md5', b'pw', b'salt', 2).hex() == '4be3d687bfd2830f9cc9cab3c7e5dad2'
assert hashlib.pbkdf2_hmac('sha1', b'pw', b'salt', 2).hex() == '133a0b823b029801576d5a38793387e88064dd5f'
assert (
    hashlib.pbkdf2_hmac('sha3_256', b'pw', b'salt', 2).hex()
    == '7707a0647b20a494f286cd484e35b19e86abd7b79c3dfb23592460ee3ea8512d'
)
assert (
    hashlib.pbkdf2_hmac('sha224', b'pw', b'salt', 3, 40).hex()
    == '26ada39be31f195500c2ef7890d39b8a230230af9d54791eac49e368259d681bbef6a6b4b27e41ca'
)
assert (
    hashlib.pbkdf2_hmac('sha384', b'pw', b'salt', 3, 40).hex()
    == 'b9a2cf5af8187f5db8f65515496af1cad62f721fe138e5d1f2d33c18ef0d7daaeb4c87a604005452'
)
assert (
    hashlib.pbkdf2_hmac('sha3_512', b'pw' * 100, b'salt', 3, 40).hex()
    == 'f0b6b44b3c8a05267307fe0f47d17680ea2c4d776caae33a79c8d1fa49c8864a16d118cf58fc9127'
)
assert (
    hashlib.pbkdf2_hmac('blake2s', b'pw', b'salt', 3, 40).hex()
    == 'e58fc154da8b4ed854821c83557c3b2b2135b8246c3f79ea2db33021b99953095f35a46f2fb2609e'
)
# a password longer than the block size is hashed first, and a key longer than one digest spans blocks
assert (
    hashlib.pbkdf2_hmac('sha512', b'pw' * 100, b'salt', 3, 100).hex()
    == 'a079e5d89560f76f030ad7362265da74343cf70efed407534085b8dc409a0ff276ad6f572083dabacb59c246462c20643af553913f25780390388dea19ac2d2417113c46f4e4553628402f4ccfd1a33a1f9260c6da9c2dc972d730443ea5a591d8711b2f'
)
