total = 0
for i in range(20):
    rows = fetch_rows(i)
    for row in rows:
        total += row['amount'] * row['quantity']
total
