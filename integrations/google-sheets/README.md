# Collect&Go-prijzen in Google Sheets

`collectandgo.gs` voegt twee formules toe aan een spreadsheet die de Prijsspoor-API gebruiken:

| Formule | Geeft |
|---|---|
| `=CGCHECK(I45; O45)` | `✓` als I45 de huidige C&G-prijs is, `✓ (vanaf 3 st.)` als het de hoeveelheidskorting is, anders `✗ C&G: € 16,45` |
| `=CGPRIJS(O45)` | de huidige prijs (een getal, dus bruikbaar in berekeningen) |
| `=CGPRIJS(O45; "eenheidsprijs")` | ook: `eenheid`, `korting`, `korting_vanaf`, `naam`, `beschikbaar`, `laatst_gecontroleerd`, `laatst_gewijzigd`, `vorige_prijs` |

`O45` mag een link zijn, tekst met een link erachter (Invoegen > Link), of tekst waarin een
collectandgo.be-link staat. Cellen zonder Collect&Go-link geven een lege cel (`CGCHECK`) of
`geen Collect&Go-link` (`CGPRIJS`).

## Installeren

1. Open de spreadsheet en kies **Extensies > Apps Script**.
2. Vervang de inhoud van `Code.gs` door die van `collectandgo.gs` en klik op **Opslaan**.
3. Herlaad de spreadsheet. Er verschijnt een menu **Collect&Go**.
4. Zet in een vrije kolom naast de eerste productregel `=CGCHECK(I45; O45)` en sleep die naar
   beneden.

De eerste keer dat je het menu gebruikt, vraagt Google om toestemming voor het script.

## Goed om te weten

- Google herberekent een eigen formule alleen als haar invoer verandert. Nieuwe prijzen haal je
  op via **Collect&Go > Prijzen vernieuwen**. Antwoorden worden een uur bewaard; Prijsspoor zelf
  wordt om de 6 uur bijgewerkt.
- Een link naar een product dat Prijsspoor nog niet kent, wordt de eerste keer opgezocht. Dat
  duurt hooguit enkele seconden.
- Staat jullie Prijsspoor elders, pas dan `PRIJSSPOOR_API` bovenaan het script aan.
