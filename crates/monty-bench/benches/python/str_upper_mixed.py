# Isolate upper on mostly ASCII German text with Unicode expansions.
text = 'Bitte API-Anfragen zur Straße prüfen und anschließend die Verbindung zum Gateway überprüfen.\n' * 1000
expected = 'BITTE API-ANFRAGEN ZUR STRASSE PRÜFEN UND ANSCHLIESSEND DIE VERBINDUNG ZUM GATEWAY ÜBERPRÜFEN.\n' * 1000
for _ in range(20):
    result = text.upper()
assert result == expected
