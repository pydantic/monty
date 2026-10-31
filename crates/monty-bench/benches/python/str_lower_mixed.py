# Isolate lowercasing of mostly ASCII German text containing umlauts.
text = (
    'Bitte API-Anfragen zuerst anhand der Anfrage-ID vergleichen und den Status des Dienstes prüfen. '
    'Danach die Verbindung zum Gateway überprüfen und die Antwort im Vorgang dokumentieren.\n'
) * 1000
expected = (
    'bitte api-anfragen zuerst anhand der anfrage-id vergleichen und den status des dienstes prüfen. '
    'danach die verbindung zum gateway überprüfen und die antwort im vorgang dokumentieren.\n'
) * 1000

for _ in range(20):
    lowered = text.lower()
assert lowered == expected
