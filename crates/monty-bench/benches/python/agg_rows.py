# Group-by aggregation over rows of dicts, the core loop of a data-analysis agent:
# str-keyed dict get/insert, `sorted` with a lambda key and a filtered generator `sum`.

regions = ['north', 'south', 'east', 'west']
segments = ['consumer', 'smb', 'enterprise']
channels = ['organic', 'paid', 'referral', 'email', 'social']
rows = []
for i in range(1_000):
    rows.append(
        {
            'order_id': i,
            'region': regions[i % 4],
            'segment': segments[i % 3],
            'channel': channels[i % 5],
            'amount': (i * 37) % 500 + 1,
            'quantity': i % 7 + 1,
        }
    )

checksum = 0
for _ in range(10):
    revenue = {}
    orders = {}
    for row in rows:
        key = row['region'] + '/' + row['segment']
        value = row['amount'] * row['quantity']
        revenue[key] = revenue.get(key, 0) + value
        orders[key] = orders.get(key, 0) + 1
    ranked = sorted(revenue.items(), key=lambda kv: kv[1], reverse=True)
    top_key, top_revenue = ranked[0]
    big = sum(1 for row in rows if row['amount'] > 400 and row['quantity'] >= 3)
    checksum += top_revenue + big + top_revenue // orders[top_key]
assert checksum == 909_800
