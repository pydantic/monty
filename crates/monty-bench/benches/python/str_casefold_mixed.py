# Isolate casefold on mostly ASCII German text with Unicode expansions.
text = 'Bitte API-Anfragen zur Straße prüfen und anschließend die Verbindung zum Gateway überprüfen.\n' * 1000
expected = 'bitte api-anfragen zur strasse prüfen und anschliessend die verbindung zum gateway überprüfen.\n' * 1000
for _ in range(20):
    result = text.casefold()
assert result == expected
