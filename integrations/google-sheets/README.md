# Prijsspoor voor Google Sheets

Controleert de Collect&Go- en bol.com-prijzen in een spreadsheet met de Prijsspoor-API
(`https://c-en-g.vtk.be`). De add-on leest de link in een cel, ook als die achter tekst zit
(Invoegen > Link), en vergelijkt de prijs. Dat kan zijn:

- een Collect&Go-productpagina (`.../nl/assortiment/<product>`);
- een Collect&Go-zoek- of categoriepagina waarop je een product hebt opengeklikt
  (`.../nl/zoek?searchTerm=...#pdp_<nummer>` of `.../nl/assortiment/suiker-pak#pdp_<nummer>`);
- een bol.com-product op de Belgische site (`https://www.bol.com/be/nl/p/<naam>/<nummer>/`).

## Gebruik

**Prijzen kleuren.** Kies *Extensies > Prijsspoor > Prijzen controleren (alle tabbladen)*. Op
elk tabblad krijgt elke prijs naast een Collect&Go- of bol.com-link een kleur:

| Kleur | Betekenis |
|---|---|
| groen | de prijs klopt (de prijs met hoeveelheidskorting enkel als je genoeg stuks neemt) |
| rood | de winkel vraagt nu iets anders, of de bol.com-link gaat niet naar bol.com/be/nl; uitleg en echte prijs staan in de notitie op de cel |
| oranje | het product is niet gevonden |

De prijscel is de cel op dezelfde rij, in de kolom van de dichtstbijzijnde kop
**"Prijs per eenheid"** of **"Prijs per stuk"** erboven, zoals in het budgetsjabloon. Met
*Kleuren wissen* komen de oorspronkelijke achtergronden van het tabblad terug en verdwijnen de
notities. *Prijzen controleren (dit tabblad)* doet enkel het actieve tabblad.

**Automatisch bij het openen.** Na de eerste keer *Prijzen controleren* gebeurt de controle
vanzelf, op alle tabbladen, telkens iemand de spreadsheet opent (hoogstens één keer per tien
minuten). De kleuren verschijnen na enkele seconden; cellen worden enkel herschreven als er iets
veranderde. Eén persoon die het aanzet volstaat voor iedereen: de controle draait met de rechten
van die persoon. Uit- of weer aanzetten: *Extensies > Prijsspoor > Automatisch controleren bij
openen*. Uitzetten geldt voor iedereen.

**Hoeveelheidskorting.** Collect&Go geeft soms korting vanaf een aantal stuks ("€ 4,05/st vanaf
3 st"). Die prijs is enkel groen als je minstens zoveel stuks neemt. Het aantal komt uit de kolom
**"Eenheden"** op dezelfde koprij als de prijs, of anders **"Aantal"** (telkens de dichtstbijzijnde
links van de prijs). Staat er geen van beide, dan is de kortingsprijs rood.

**bol.com.** Enkel links naar de Belgische site tellen: `bol.com/be/nl/...`. Een link naar
`bol.com/nl/...` (de Nederlandse site, met andere prijzen) is meteen rood, zonder iets op te
vragen. Prijsspoor haalt bol.com-prijzen op wanneer de spreadsheet erom vraagt en bewaart ze een
uur.

**Formules.** Voor wie liever met formules werkt:

| Formule | Geeft |
|---|---|
| `=CGCHECK(I45; O45; H45)` | `✓`, `✓ (vanaf 3 st.)`, `✗ C&G: € 16,45`, `✗ bol: € 9,99` of `✗ korting pas vanaf 3 st., C&G: € 4,49` (H45 = aantal stuks, optioneel) |
| `=CGPRIJS(O45)` | de huidige prijs als getal |
| `=CGPRIJS(O45; "laatst_gewijzigd")` | ook: `eenheidsprijs`, `eenheid`, `korting`, `korting_vanaf`, `naam`, `beschikbaar`, `laatst_gecontroleerd`, `vorige_prijs` |

Google rekent eigen formules alleen opnieuw als hun invoer verandert. Voor verse prijzen kies je
*Extensies > Prijsspoor > Formules vernieuwen*. Antwoorden worden een uur bewaard; Prijsspoor
zelf wordt om de zes uur bijgewerkt.

## Installeren in één spreadsheet

1. Open de spreadsheet en kies *Extensies > Apps Script*.
2. Vervang de inhoud van `Code.gs` door `collectandgo.gs` en klik op *Opslaan*.
3. Herlaad de spreadsheet. Het menu staat onder *Extensies > (naam van het project)*.

Verwijder dit script weer zodra de add-on voor de organisatie geïnstalleerd is. Anders bestaan
`CGCHECK` en `CGPRIJS` twee keer.

## Installeren voor de hele organisatie (Workspace-beheerder)

Je publiceert het script als **privé Editor-add-on**, alleen zichtbaar binnen het domein, en
installeert het als beheerder voor iedereen. Google beoordeelt een privé-app niet.

Je hebt nodig:
- een Workspace-beheerdersaccount van het domein;
- een Google Cloud-project binnen de organisatie van het domein;
- minstens één screenshot van 1280×800 (bv. een budget na *Prijzen controleren*). Maak die door
  het script eerst in één spreadsheet te installeren (zie hierboven).

### 1. Het Apps Script-project

1. Ga naar <https://script.google.com> > *Nieuw project* en noem het **Prijsspoor**.
2. *Projectinstellingen* (tandwiel): vink *"appsscript.json"-manifestbestand weergeven in editor*
   aan en noteer het **Script-ID**.
3. Zet de code erin, op een van twee manieren:
   - **Met clasp** (aangeraden voor updates). Eerst eenmalig: zet de *Google Apps Script API* aan
     op <https://script.google.com/home/usersettings>. Daarna:
     ```bash
     npm install -g @google/clasp
     clasp login                       # met je VTK-account
     cd integrations/google-sheets
     cp .clasp.json.example .clasp.json # vul het Script-ID in
     clasp push --force                # zet collectandgo.gs en appsscript.json online
     ```
   - **Met de hand.** Plak `collectandgo.gs` in `Code.gs` en `appsscript.json` in
     `appsscript.json`, en sla op.

### 2. Het Google Cloud-project

1. Ga naar <https://console.cloud.google.com> > *Nieuw project*, bv. `prijsspoor-sheets`, met als
   *Organisatie* jullie domein.
2. *Google Auth Platform* (vroeger "OAuth-toestemmingsscherm"):
   - *Branding*: appnaam **Prijsspoor**, supportmail, het logo `assets/icon-128.png`, en als links
     `https://c-en-g.vtk.be/` (startpagina) en `https://c-en-g.vtk.be/privacy` (privacybeleid).
   - *Doelgroep*: **Intern**.
   - *Gegevenstoegang*: voeg de vier scopes uit `appsscript.json` toe.
3. Noteer het **projectnummer** (*Cloud-overzicht > Dashboard*).
4. Terug in Apps Script: *Projectinstellingen > Google Cloud Platform-project > Project wijzigen*
   en vul het projectnummer in.

### 3. Een versie publiceren

In Apps Script: *Implementeren > Nieuwe implementatie > type kiezen (tandwiel) > Add-on*, met als
beschrijving bv. "v1", en dan *Implementeren*. Noteer de **implementatie-ID**. Met clasp kan het
ook zo: `clasp version "v1"` en daarna `clasp deploy -V 1 -d "v1"`.

### 4. De Marketplace-vermelding

In het Cloud-project: *API's en services > Bibliotheek* > **Google Workspace Marketplace SDK** >
*Inschakelen*, en daarna *Beheren*.

*App-configuratie*:
- **Zichtbaarheid van de app**: *Privé* (alleen het domein). Dit kun je later niet meer wijzigen.
- **Installatie-instellingen**: *Alleen beheerdersinstallatie* (of ook individuele installatie
  als collega's hem zelf mogen toevoegen).
- **App-integratie**: niet *Google Workspace add-on*, maar alleen **Sheets add-on** aanvinken
  (dat is het editor-add-on-type). Vul het **Script-ID** in en bij *Script version* het
  **versienummer** uit stap 3 (bv. `1`), niet de implementatie-ID.
- **OAuth-scopes**: dezelfde vier als in `appsscript.json`.
- **Ontwikkelaarsgegevens**: naam, `https://c-en-g.vtk.be/` en een mailadres.
- *Opslaan*.

*Winkelvermelding*:
- naam **Prijsspoor**, korte beschrijving ("Controleer Collect&Go-prijzen in je budget") en een
  langere (de tabel onder *Gebruik* hierboven);
- categorie: *Bedrijfstools* of *Productiviteit*;
- afbeeldingen: `assets/icon-128.png`, `assets/icon-32.png`, `assets/banner-220x140.png` en je
  screenshot;
- links: gebruiksvoorwaarden en privacybeleid `https://c-en-g.vtk.be/privacy`, support
  `https://c-en-g.vtk.be/`;
- *Publiceren*. Een privé-app staat meteen in de Marketplace van het domein.

### 5. Installeren voor iedereen

Open <https://admin.google.com> > *Apps > Google Workspace Marketplace-apps > Apps-lijst* >
*App toevoegen* > *Interne app toevoegen* > **Prijsspoor** > *Beheerdersinstallatie*. Kies
*Iedereen in de organisatie* (of een organisatie-eenheid of groep) en accepteer de rechten.

De add-on kan ook vanuit Sheets: *Extensies > Add-ons > Add-ons downloaden* > tab *Intern* >
Prijsspoor > *Beheerdersinstallatie*.

Het kan tot 24 uur duren voor iedereen hem ziet.

### 6. Voor collega's

In elke spreadsheet één keer: *Extensies > Prijsspoor > Prijzen controleren (alle tabbladen)*.
Daarna gebeurt het vanzelf bij elk openen. Wie formules
wil gebruiken, kiest eerst één keer *Extensies > Prijsspoor > Hoe gebruik ik dit?*: Google maakt
`CGCHECK` en `CGPRIJS` in een spreadsheet pas beschikbaar nadat de add-on daar één keer gebruikt
is.

### Updates

1. Pas `collectandgo.gs` aan en doe `clasp push --force`.
2. *Implementeren > Implementaties beheren* > potlood > *Versie: Nieuwe versie* > *Implementeren*.
   Hangt de Marketplace-configuratie aan een versienummer in plaats van een implementatie-ID,
   zet dan ook daar het nieuwe nummer.

Gebruikers krijgen de nieuwe versie vanzelf.

**Nieuwe scope sinds het automatisch controleren.** `script.scriptapp` (installeerbare triggers)
is nieuw. Voeg hem toe in het Cloud-project (*Google Auth Platform > Gegevenstoegang*) en in de
Marketplace-configuratie (*App-configuratie > OAuth-scopes*), en publiceer de nieuwe versie. In
de Admin console moet de beheerder daarna de nieuwe rechten van Prijsspoor goedkeuren (*Apps >
Google Workspace Marketplace-apps > Prijsspoor*); tot dan blijft de oude versie actief.
Gebruikers die de add-on zelf installeerden, krijgen bij het volgende menu-item een
toestemmingsvenster.

## Bestanden

| Bestand | Wat |
|---|---|
| `collectandgo.gs` | de code (zowel add-on als script in één spreadsheet) |
| `appsscript.json` | manifest: scopes, en `UrlFetch` alleen naar `https://c-en-g.vtk.be/` (ook bol.com gaat langs Prijsspoor) |
| `.clasp.json.example`, `.claspignore` | voor `clasp push` |
| `assets/` | icoontjes en kaartbanner voor de Marketplace |
