import asyncio

# Open the pricing page
page = await open_page(url)

# Parse the HTML with BeautifulSoup
soup = beautiful_soup(page.html)

# Find the main content area that contains pricing information
# Let's look for tables or structured pricing data
pricing_tables = soup.find_all('table')

# Initialize a list to store all model pricing data
all_models = []

# Process each table found
for table in pricing_tables:
    # Get all rows in the table
    rows = table.find_all('tr')

    if len(rows) < 2:  # Skip tables without data rows
        continue

    # Get headers from the first row
    header_row = rows[0]
    headers = [th.get_text(strip=True) for th in header_row.find_all(['th', 'td'])]

    # Process data rows
    for row in rows[1:]:
        cells = row.find_all(['td', 'th'])
        if len(cells) < 2:
            continue

        # Extract cell values
        row_data = [cell.get_text(strip=True) for cell in cells]

        # Skip rows that might indicate deprecated models
        row_text = ' '.join(row_data).lower()
        if 'deprecated' in row_text or 'legacy' in row_text:
            continue

        # Create a dictionary for this model
        model_info = {}
        for i, value in enumerate(row_data):
            if i < len(headers):
                model_info[headers[i]] = value
            else:
                model_info[f'column_{i}'] = value

        if model_info:  # Only add if we have data
            all_models.append(model_info)

# Print the results
print(f'Found {len(all_models)} models with pricing data')
print('\nModel pricing information:')
for i, model in enumerate(all_models, 1):
    print(f'\n{i}. {model}')

all_models
