"""Extract sender details from emails in several scripts, redact them, and dedupe the senders.

Names arrive in Latin with accents, Cyrillic and CJK, sometimes as fullwidth or
decomposed Unicode and in different cases, so the same person appears under several
spellings. `unicodedata.normalize('NFKC', ...)` and `casefold()` collapse them. The
body redaction is plain `str.replace` after `re` finds the phone numbers, and the
expected answer is computed host-side with the same expressions.
"""

from __future__ import annotations

import re
import unicodedata

from pydantic_evals.evaluators import EqualsExpected

from evals.harness.task import Task

_HEADER = re.compile(r'^(?P<name>.+?) <(?P<email>[^>]+)>$')
_PHONE = re.compile(r'\+?\d[\d\s-]{7,}\d')

_EMAILS = [
    ('José Núñez <jose@example.com>', 'Hola, soy José Núñez. Llámame al +34 612 345 678 o escribe a jose@example.com.'),
    ('José Núñez <JOSE@example.com>', 'Segunda nota de José Núñez: el pedido llega el lunes.'),
    ('Владимир Петров <vladimir@example.ru>', 'Здравствуйте, Владимир Петров на связи, телефон +7 495 123-45-67.'),
    ('владимир петров <Vladimir@Example.ru>', 'Повторно: подтвердите встречу, тел. +7 495 123-45-67.'),
    ('山田太郎 <taro@example.jp>', '山田太郎です。電話は 03-1234-5678 です。taro@example.jp まで。'),
    ('Ｙａｍａｄａ Ｔａｒｏ <taro@example.jp>', 'Yamada Taro here, following up on the invoice.'),
    (
        'Zoë Müller <zoe@example.de>',
        'Hallo, Zoë Müller hier. Rückruf unter +49 30 123456 78.',  # codespell:ignore unter
    ),
    ('Zoë Müller <zoe@example.de>', 'Nochmals Zoë Müller: Rechnung anbei.'),
    ('Aoife Ní Bhriain <aoife@example.ie>', 'Dia dhuit, Aoife Ní Bhriain anseo, +353 1 234 5678.'),
    ('Nguyễn Văn An <an@example.vn>', 'Chào bạn, Nguyễn Văn An gửi báo giá, số 0912 345 678.'),
    ('NGUYỄN VĂN AN <AN@EXAMPLE.VN>', 'Nhắc lại: liên hệ an@example.vn.'),
    ('Ahmed El-Sayed <ahmed@example.eg>', 'Hi, Ahmed El-Sayed from Cairo. Call 010 1234 5678.'),
    ('Πέτρος Παπαδόπουλος <petros@example.gr>', 'Καλημέρα, Πέτρος Παπαδόπουλος, τηλ. +30 210 123 4567.'),
    ('Mei-Ling Chen <mei@example.tw>', 'Hello from Mei-Ling Chen, reachable on +886 2 2345 6789 or mei@example.tw.'),
    ('ｍｅｉ-ｌｉｎｇ ｃｈｅｎ <MEI@example.tw>', 'Mei-Ling Chen again, thanks for the quick reply.'),
]

EMAILS = [{'id': f'm{i + 1:02d}', 'from': sender, 'body': body} for i, (sender, body) in enumerate(_EMAILS)]


def _canonical(text: str) -> str:
    return unicodedata.normalize('NFKC', text).casefold()


def _expected() -> dict[str, object]:
    redacted: list[dict[str, str]] = []
    senders: dict[str, dict[str, str]] = {}
    for mail in EMAILS:
        match = _HEADER.match(mail['from'])
        assert match is not None
        name, email = match.group('name'), match.group('email')
        body = mail['body']
        for phone in _PHONE.findall(body):
            body = body.replace(phone, '[PHONE]')
        body = body.replace(email, '[EMAIL]').replace(name, '[NAME]')
        redacted.append({'id': mail['id'], 'body': body})
        key = _canonical(email)
        senders.setdefault(key, {'name': _canonical(name), 'email': key})
    return {'senders': sorted(senders.values(), key=lambda s: s['email']), 'redacted': redacted}


EXPECTED = _expected()

STUBS = '''
EMAILS: list[dict[str, str]] = []
"""Each has `id`, `from` (`Name <email>`) and `body`."""
'''

REFERENCE = """
import re
import unicodedata

header = re.compile(r'^(?P<name>.+?) <(?P<email>[^>]+)>$')
phone_re = re.compile(r'\\+?\\d[\\d\\s-]{7,}\\d')

def canonical(text):
    return unicodedata.normalize('NFKC', text).casefold()

redacted = []
senders = {}
for mail in EMAILS:
    match = header.match(mail['from'])
    if match is None:
        continue
    name = match.group('name')
    email = match.group('email')
    body = mail['body']
    for phone in phone_re.findall(body):
        body = body.replace(phone, '[PHONE]')
    body = body.replace(email, '[EMAIL]').replace(name, '[NAME]')
    redacted.append({'id': mail['id'], 'body': body})
    key = canonical(email)
    if key not in senders:
        senders[key] = {'name': canonical(name), 'email': key}

{'senders': sorted(senders.values(), key=lambda s: s['email']), 'redacted': redacted}
"""

TASK = Task(
    name='redact_pii',
    category='text',
    prompt=(
        'For each email in EMAILS, parse the sender name and address from the `from` header, which '
        'has the form `Name <email>`. Redact the body: replace every phone number (a run of at least '
        '9 characters made of digits, spaces and dashes, optionally starting with +) with [PHONE], '
        'then the sender address with [EMAIL], then the sender name exactly as written in the header '
        'with [NAME]. Then list the distinct senders: treat addresses as the same after Unicode NFKC '
        'normalisation and casefolding, keep the first-seen name normalised the same way, and sort by '
        'email. Return {"senders": [{"name", "email"}, ...], "redacted": [{"id", "body"}, ...]} with '
        'redacted in EMAILS order.'
    ),
    stubs=STUBS,
    tools={},
    inputs={'EMAILS': EMAILS},
    expected=EXPECTED,
    evaluators=(EqualsExpected(),),
    reference_solution=REFERENCE,
    traps=('unicodedata.normalize', 'str.casefold', 'named regex groups', 'Match | None under the type checker'),
)
