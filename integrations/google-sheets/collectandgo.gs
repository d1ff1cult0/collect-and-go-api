/**
 * Collect&Go- en bol.com-prijzen in Google Sheets, via de Prijsspoor-API.
 *
 * Werkt op twee manieren (zie README.md):
 * - als add-on voor de hele organisatie (Marketplace, privé), of
 * - in één spreadsheet: Extensies > Apps Script, plak dit bestand in Code.gs en sla op.
 *
 * Prijzen kleuren: Extensies > Prijsspoor > Prijzen controleren kleurt op alle tabbladen elke
 * prijs naast een Collect&Go- of bol.com-link groen of rood (zie controleer_ hieronder). Daarna
 * gebeurt dat ook vanzelf telkens de spreadsheet geopend wordt.
 *
 * Formules, in elke cel:
 *
 *   =CGPRIJS(O45)                          de huidige prijs van het product in O45
 *   =CGPRIJS(O45; "eenheidsprijs")         prijs per kg/l (enkel Collect&Go)
 *   =CGCHECK(I45; O45; H45)                "✓" als I45 de huidige prijs is voor H45 stuks
 *
 * O45 mag een link zijn, of tekst met een link erachter (zoals "STELLA ARTOIS pils ...").
 * Velden voor CGPRIJS: prijs (standaard), eenheidsprijs, eenheid, korting, korting_vanaf,
 * naam, beschikbaar, laatst_gecontroleerd, laatst_gewijzigd, vorige_prijs.
 *
 * Google herberekent een eigen functie enkel als haar invoer wijzigt. Wil je de prijzen
 * opnieuw ophalen, kies dan Extensies > Prijsspoor > Formules vernieuwen.
 */

const PRIJSSPOOR_API = 'https://c-en-g.vtk.be/api/v1/price';
const CACHE_SECONDS = 60 * 60; // Collect&Go wordt om de zes uur gescraped, bol.com op aanvraag

/**
 * Huidige prijs (of een ander veld) van het Collect&Go- of bol.com-product waar een cel naar linkt.
 *
 * @param {A1} cel De cel met de link of de linktekst, bv. O45.
 * @param {"prijs"} veld Optioneel: prijs, eenheidsprijs, eenheid, korting, korting_vanaf, naam, beschikbaar, laatst_gecontroleerd, laatst_gewijzigd, vorige_prijs.
 * @return De gevraagde waarde, of een korte uitleg als het niet lukt.
 * @customfunction
 */
function CGPRIJS(cel, veld) {
  const url = linkUitCel_(cel);
  if (!url) return 'geen Collect&Go- of bol.com-link';
  const data = haalPrijsOp_(url);
  if (data.error) return data.error;
  const p = data.price || {};
  switch (String(veld || 'prijs').toLowerCase()) {
    case 'prijs':
      return p.price != null ? p.price : 'geen prijs';
    case 'eenheidsprijs':
      return p.unit_price != null ? p.unit_price : '';
    case 'eenheid':
      return p.unit || '';
    case 'korting':
      return p.offer ? p.offer.price : '';
    case 'korting_vanaf':
      return p.offer ? p.offer.min_quantity : '';
    case 'naam':
      return data.product.name;
    case 'beschikbaar':
      return data.product.is_available;
    case 'laatst_gecontroleerd':
      return data.last_updated_at ? new Date(data.last_updated_at) : '';
    case 'laatst_gewijzigd':
      return data.last_changed_at ? new Date(data.last_changed_at) : '';
    case 'vorige_prijs':
      return data.previous_price != null ? data.previous_price : '';
    default:
      return 'onbekend veld: ' + veld;
  }
}

/**
 * Klopt de prijs in de spreadsheet nog?
 *
 * @param {16.45} prijs De prijs in je spreadsheet, bv. I45.
 * @param {A1} cel De cel met de link of de linktekst, bv. O45.
 * @param {3} aantal Optioneel: hoeveel stuks je koopt, bv. H45. Een hoeveelheidskorting ("vanaf 3 st.") telt enkel als dit aantal groot genoeg is.
 * @return "✓" als de prijs klopt, anders "✗" met de huidige prijs.
 * @customfunction
 */
function CGCHECK(prijs, cel, aantal) {
  const url = linkUitCel_(cel);
  if (!url) return '';
  return beoordeel_(prijs, aantal, haalPrijsOp_(url)).kort;
}

/**
 * Menu onder Extensies. `createAddonMenu` werkt zowel in een add-on als in een script in één
 * spreadsheet. In een add-on draait onOpen eerst zonder rechten (AuthMode.NONE): hier dus
 * enkel het menu maken, niets anders. Het automatisch controleren gebeurt in een aparte,
 * installeerbare trigger (automatischBijOpenen), want onOpen mag geen prijzen ophalen.
 */
function onOpen(e) {
  SpreadsheetApp.getUi()
    .createAddonMenu()
    .addItem('Prijzen controleren (alle tabbladen)', 'controleerAlles')
    .addItem('Prijzen controleren (dit tabblad)', 'controleerDitBlad')
    .addItem('Kleuren wissen (dit tabblad)', 'wisKleuren')
    .addSeparator()
    .addItem('Automatisch controleren bij openen: aanzetten', 'zetAutomatischAan')
    .addItem('Automatisch controleren bij openen: uitzetten', 'zetAutomatischUit')
    .addSeparator()
    .addItem('Formules vernieuwen', 'vernieuwPrijzen')
    .addItem('Hoe gebruik ik dit?', 'toonHulp')
    .addToUi();
}

/** Bij installatie als add-on: meteen het menu tonen. */
function onInstall(e) {
  onOpen(e);
}

/**
 * Uitleg. Een menu-item kiezen schakelt de add-on ook in voor deze spreadsheet; pas daarna
 * werken CGCHECK en CGPRIJS hier.
 */
function toonHulp() {
  SpreadsheetApp.getUi().alert(
    'Prijsspoor staat aan in deze spreadsheet',
    'Prijzen controleren kleurt op alle tabbladen elke prijs naast een Collect&Go- of\n' +
      'bol.com-link:\n' +
      '    groen = klopt, rood = anders (de echte prijs staat in de notitie),\n' +
      '    oranje = product niet gevonden.\n' +
      'Na de eerste keer gebeurt dat ook vanzelf telkens de spreadsheet geopend wordt\n' +
      '(uitzetten kan in het menu).\n\n' +
      'Een hoeveelheidskorting ("€ 4,05 vanaf 3 st.") telt enkel als de kolom "Eenheden"\n' +
      '(of "Aantal") op die rij minstens zoveel stuks toont.\n' +
      'bol.com-links moeten naar de Belgische site gaan: bol.com/be/nl/...\n' +
      'Een link naar bol.com/nl/... is altijd rood.\n\n' +
      'Of met formules, in een vrije kolom op de rij van een product:\n\n' +
      '=CGCHECK(I45; O45; H45)\n' +
      '    ✓ als de prijs in I45 de huidige prijs is voor H45 stuks,\n' +
      '    anders "✗ …" met de huidige prijs.\n\n' +
      '=CGPRIJS(O45)\n' +
      '    de huidige prijs als getal.\n\n' +
      '=CGPRIJS(O45; "laatst_gewijzigd")\n' +
      '    ook: eenheidsprijs, eenheid, korting, korting_vanaf, naam,\n' +
      '    beschikbaar, laatst_gecontroleerd, vorige_prijs.\n\n' +
      'O45 is de cel met de link (of tekst met een link erachter).\n' +
      'Formules opnieuw laten rekenen: Extensies > Prijsspoor > Formules vernieuwen.',
    SpreadsheetApp.getUi().ButtonSet.OK
  );
}

/**
 * Haalt alle prijzen opnieuw op: een nieuwe cacheversie (oude antwoorden tellen niet meer)
 * en elke CGPRIJS/CGCHECK-formule even leegmaken en terugzetten, zodat Sheets ze herberekent.
 */
function vernieuwPrijzen() {
  PropertiesService.getDocumentProperties().setProperty('cg_versie', String(Date.now()));
  const cellen = [];
  for (const sheet of SpreadsheetApp.getActiveSpreadsheet().getSheets()) {
    const range = sheet.getDataRange();
    range.getFormulas().forEach(function (row, r) {
      row.forEach(function (f, c) {
        if (/CG(PRIJS|CHECK)\s*\(/i.test(f)) cellen.push([range.getCell(r + 1, c + 1), f]);
      });
    });
  }
  cellen.forEach(function (x) { x[0].setFormula(''); });
  SpreadsheetApp.flush();
  cellen.forEach(function (x) { x[0].setFormula(x[1]); });
  SpreadsheetApp.getActive().toast(cellen.length + ' formules vernieuwd', 'Prijsspoor');
}

// --- links ----------------------------------------------------------------------------

const LINK_COGO = /^https?:\/\/(?:[\w-]+\.)*collectandgo\.be\//i;
const LINK_BOL = /^https?:\/\/(?:www\.)?bol\.com\//i;
const LINK_IN_TEKST = /https?:\/\/(?:(?:[\w-]+\.)*collectandgo\.be|(?:www\.)?bol\.com)\/\S+/i;

function isLink_(u) {
  return !!u && (LINK_COGO.test(u) || LINK_BOL.test(u));
}

/**
 * Een bol.com-link moet naar de Belgische site gaan (bol.com/be/nl/...). Een link naar
 * bol.com/nl/... toont Nederlandse prijzen: meteen fout, zonder iets op te vragen.
 * Geeft de foutmelding, of null als de link in orde is (of geen bol.com-link is).
 */
function bolSiteFout_(url) {
  if (!LINK_BOL.test(url)) return null;
  const pad = url.replace(LINK_BOL, '/');
  if (/^\/be\/nl\//.test(pad)) return null;
  const m = /^\/([a-z]{2})\/([a-z]{2})\//i.exec(pad);
  return 'verkeerde bol.com-link: ' + (m ? 'bol.com/' + m[1] + '/' + m[2] : 'zonder /be/nl') +
    ' in plaats van de Belgische site bol.com/be/nl';
}

/**
 * Een eigen functie krijgt enkel de waarde van de cel, niet de link erachter. Daarom
 * zoeken we de cel op die de formule aanroept en lezen we de link uit het argument.
 */
function linkUitCel_(waarde) {
  const inTekst = linkUitTekst_(waarde);
  if (inTekst) return inTekst;

  const actief = SpreadsheetApp.getActiveRange();
  if (!actief) return null;
  const formule = actief.getFormula();
  // Eerste celverwijzing in de formule na CGPRIJS( of het tweede argument van CGCHECK(.
  const m = /CGCHECK\s*\(\s*[^;,]+[;,]\s*('?[^'!]+'?!)?\$?([A-Z]+)\$?(\d+)/i.exec(formule) ||
    /CGPRIJS\s*\(\s*('?[^'!]+'?!)?\$?([A-Z]+)\$?(\d+)/i.exec(formule);
  if (!m) return null;
  const sheet = m[1]
    ? SpreadsheetApp.getActiveSpreadsheet().getSheetByName(m[1].replace(/^'|'?!$/g, ''))
    : actief.getSheet();
  if (!sheet) return null;
  return linkUitRich_(sheet.getRange(m[2] + m[3]).getRichTextValue());
}

function linkUitRich_(rich) {
  if (!rich) return null;
  const urls = [rich.getLinkUrl()].concat(rich.getRuns().map(function (run) { return run.getLinkUrl(); }));
  return urls.find(isLink_) || null;
}

function linkUitTekst_(waarde) {
  const m = String(waarde || '').match(LINK_IN_TEKST);
  return m ? m[0] : null;
}

// --- de API ---------------------------------------------------------------------------

function haalPrijsOp_(url) {
  return haalPrijzenOp_([url])[url];
}

/**
 * Prijzen voor een lijst links: eerst uit de cache, de rest tegelijk. bol.com-links in kleine
 * groepjes, want Prijsspoor haalt die pagina's pas op wanneer erom gevraagd wordt.
 */
function haalPrijzenOp_(urls) {
  // In een trigger is er soms geen documentcache; de scriptcache werkt dan ook (de sleutel is de link).
  const cache = CacheService.getDocumentCache() || CacheService.getScriptCache();
  // Eigen functies mogen properties enkel lezen; "Formules vernieuwen" zet een nieuwe versie.
  const versie = PropertiesService.getDocumentProperties().getProperty('cg_versie') || '0';
  const sleutel = function (url) {
    return 'cg:' + versie + ':' +
      Utilities.base64Encode(Utilities.computeDigest(Utilities.DigestAlgorithm.MD5, url));
  };
  const uit = {};
  const cogo = [];
  const bol = [];
  urls.forEach(function (url) {
    if (url in uit || cogo.indexOf(url) >= 0 || bol.indexOf(url) >= 0) return;
    const siteFout = bolSiteFout_(url);
    if (siteFout) {
      uit[url] = { error: siteFout, fout: true };
      return;
    }
    const bewaard = cache.get(sleutel(url));
    if (bewaard) uit[url] = JSON.parse(bewaard);
    else (LINK_BOL.test(url) ? bol : cogo).push(url);
  });

  const haal = function (lijst, perKeer) {
    for (let i = 0; i < lijst.length; i += perKeer) {
      const deel = lijst.slice(i, i + perKeer);
      const antwoorden = UrlFetchApp.fetchAll(deel.map(function (url) {
        return { url: PRIJSSPOOR_API + '?url=' + encodeURIComponent(url), muteHttpExceptions: true };
      }));
      antwoorden.forEach(function (resp, k) {
        const url = deel[k];
        const code = resp.getResponseCode();
        let data;
        if (code === 200) data = JSON.parse(resp.getContentText());
        else if (code === 404) data = { error: 'niet gevonden op ' + winkel_(url) };
        else if (code === 400) data = { error: 'geen productlink' };
        else {
          // Tijdelijke fout: niet bewaren, de volgende keer opnieuw proberen.
          uit[url] = { error: winkel_(url) + ' niet bereikbaar (' + code + ')' };
          return;
        }
        cache.put(sleutel(url), JSON.stringify(data), CACHE_SECONDS);
        uit[url] = data;
      });
    }
  };
  haal(cogo, 20);
  haal(bol, 4);
  return uit;
}

function winkel_(url) {
  return LINK_BOL.test(url) ? 'bol.com' : 'C&G';
}

/**
 * Vergelijkt een prijs uit de spreadsheet met het antwoord van de API.
 * Geeft { status: 'ok' | 'fout' | 'onbekend', notitie, kort }: `notitie` voor de cel,
 * `kort` voor CGCHECK.
 *
 * Een hoeveelheidskorting (Collect&Go: "€ 4,05/st vanaf 3 st") telt enkel als `aantal` een
 * getal is van minstens zoveel stuks.
 */
function beoordeel_(prijs, aantal, data) {
  if (!data) return { status: 'onbekend', notitie: 'geen antwoord', kort: 'geen antwoord' };
  if (data.fout) return { status: 'fout', notitie: data.error, kort: '✗ ' + data.error };
  if (data.error) return { status: 'onbekend', notitie: data.error, kort: data.error };
  const p = data.price;
  const naam = data.product.name;
  const bol = data.source === 'bol.com';
  const winkel = bol ? 'bol.com' : 'Collect&Go';
  const afk = bol ? 'bol' : 'C&G';
  if (!p || p.price == null) {
    const t = data.product.is_available === false ? 'niet leverbaar, geen prijs' : 'geen prijs';
    return { status: 'onbekend', notitie: naam + ': ' + t + ' op ' + winkel, kort: t };
  }
  const nietBestelbaar = data.product.is_available === false ? '. Momenteel niet bestelbaar.' : '';
  const k = p.offer;
  const vanaf = k && k.min_quantity != null ? k.min_quantity : null;
  const vanafTekst = vanaf != null ? vanaf : '?';
  const heeftAantal = typeof aantal === 'number' && isFinite(aantal);
  const genoeg = k && vanaf != null && heeftAantal && aantal >= vanaf;

  if (typeof prijs !== 'number') {
    return {
      status: 'fout',
      notitie: 'geen prijs in de cel; ' + winkel + ' vraagt ' + euro_(p.price) + ' voor ' + naam + nietBestelbaar,
      kort: afk + ': ' + euro_(p.price),
    };
  }
  if (gelijk_(prijs, p.price)) {
    let t = 'klopt, ' + naam + ' ' + euro_(p.price) + ' op ' + winkel;
    if (genoeg) t += '. Met ' + aantal + ' st. kan het ook aan ' + euro_(k.price) + ' per stuk (vanaf ' + vanaf + ' st.)';
    return { status: 'ok', notitie: t + nietBestelbaar, kort: '✓' };
  }
  if (k && gelijk_(prijs, k.price)) {
    if (genoeg) {
      return {
        status: 'ok',
        notitie: 'klopt met de hoeveelheidskorting: ' + aantal + ' st., korting vanaf ' + vanaf +
          ' st. (normaal ' + euro_(p.price) + ')' + nietBestelbaar,
        kort: '✓ (vanaf ' + vanaf + ' st.)',
      };
    }
    const waarom = heeftAantal
      ? 'hier ' + (aantal === 1 ? 'staat er maar 1' : 'staan er maar ' + aantal)
      : 'het aantal stuks is onbekend (geen kolom "Eenheden" of "Aantal")';
    return {
      status: 'fout',
      notitie: euro_(k.price) + ' is de hoeveelheidskorting vanaf ' + vanafTekst +
        ' st., maar ' + waarom + '. De prijs per stuk is dan ' + euro_(p.price) + ' (' + naam + ')' + nietBestelbaar,
      kort: '✗ korting pas vanaf ' + vanafTekst + ' st., ' + afk + ': ' + euro_(p.price),
    };
  }
  let t = winkel + ' vraagt nu ' + euro_(p.price) + ' voor ' + naam;
  if (k) t += ', of ' + euro_(k.price) + ' per stuk vanaf ' + vanafTekst + ' st.';
  return { status: 'fout', notitie: t + nietBestelbaar, kort: '✗ ' + afk + ': ' + euro_(p.price) };
}

function gelijk_(a, b) {
  return Math.abs(a - b) < 0.005;
}

// --- groen/rood -----------------------------------------------------------------------

const KLEUR = { ok: '#c9ecc6', fout: '#f7c6c0', onbekend: '#fde3a7' };
const NOTITIE = 'Prijsspoor: ';
const PRIJSKOP = /^\s*prijs\s+per\s+(eenheid|stuk)\s*$/i;
const KOP_EENHEDEN = /^\s*eenheden\s*$/i;
const KOP_AANTAL = /^\s*aantal\s*$/i;

/** Menu: alle tabbladen. */
function controleerAlles() {
  const ss = SpreadsheetApp.getActiveSpreadsheet();
  controleer_(ss, ss.getSheets(), false);
  zetAutomatischAanEersteKeer_(ss);
}

/** Menu: enkel het actieve tabblad. */
function controleerDitBlad() {
  const ss = SpreadsheetApp.getActiveSpreadsheet();
  controleer_(ss, [ss.getActiveSheet()], false);
  zetAutomatischAanEersteKeer_(ss);
}

/**
 * Kleurt op de gegeven bladen elke prijs naast een Collect&Go- of bol.com-link: groen als ze
 * klopt, rood als de winkel nu iets anders vraagt (de echte prijs staat in de notitie), oranje
 * als het product niet gevonden werd.
 *
 * De prijscel is de cel op dezelfde rij, in de kolom van de dichtstbijzijnde kop
 * "Prijs per eenheid" of "Prijs per stuk" erboven. Het aantal stuks (voor een
 * hoeveelheidskorting) staat in de kolom "Eenheden" op diezelfde koprij, of anders "Aantal",
 * links van de prijs.
 *
 * `stil`: vanuit de trigger bij het openen, dus geen dialoogvensters.
 */
function controleer_(ss, sheets, stil) {
  const perBlad = [];
  let zonderKolom = 0;
  sheets.forEach(function (sheet) {
    const range = sheet.getDataRange();
    const waarden = range.getValues();
    const items = [];
    if (waarden.length > 1 || waarden[0].length > 1) {
      const opmaak = range.getRichTextValues();
      for (let r = 0; r < waarden.length; r++) {
        for (let c = 0; c < waarden[r].length; c++) {
          const url = linkUitRich_(opmaak[r][c]) || linkUitTekst_(waarden[r][c]);
          if (!url) continue;
          const kop = prijsKop_(waarden, r);
          if (!kop) {
            zonderKolom++;
            continue;
          }
          const aantalKolom = aantalKolom_(waarden[kop.rij], kop.kolom);
          items.push({
            rij: r,
            kolom: kop.kolom,
            url: url,
            prijs: waarden[r][kop.kolom],
            aantal: aantalKolom >= 0 ? waarden[r][aantalKolom] : null,
          });
        }
      }
    }
    if (items.length) perBlad.push({ sheet: sheet, range: range, items: items });
  });

  const aantalLinks = perBlad.reduce(function (n, b) { return n + b.items.length; }, 0);
  if (!aantalLinks) {
    if (!stil) {
      SpreadsheetApp.getUi().alert(sheets.length > 1
        ? 'Geen Collect&Go- of bol.com-links gevonden in deze spreadsheet.'
        : 'Geen Collect&Go- of bol.com-links gevonden op dit tabblad.');
    }
    return;
  }

  const urls = [];
  perBlad.forEach(function (b) { b.items.forEach(function (x) { urls.push(x.url); }); });
  const prijzen = haalPrijzenOp_(urls);

  const props = PropertiesService.getDocumentProperties();
  const tijd = Utilities.formatDate(new Date(), 'Europe/Brussels', 'd/MM HH:mm');
  const telling = { ok: 0, fout: 0, onbekend: 0 };
  const foutBladen = [];

  perBlad.forEach(function (b) {
    const sleutel = 'cg_kleuren_' + b.sheet.getSheetId();
    const origineel = JSON.parse(props.getProperty(sleutel) || '{}');
    // Eerst alles lezen, dan alles schrijven: Sheets bundelt de schrijfacties dan.
    const achtergronden = b.range.getBackgrounds();
    const notities = b.range.getNotes();
    let fouten = 0;
    b.items.forEach(function (x) {
      const oordeel = beoordeel_(x.prijs, x.aantal, prijzen[x.url]);
      telling[oordeel.status]++;
      if (oordeel.status === 'fout') fouten++;
      const kleur = KLEUR[oordeel.status];
      const notitie = NOTITIE + oordeel.notitie;
      const huidigeKleur = achtergronden[x.rij][x.kolom];
      const huidigeNotitie = String(notities[x.rij][x.kolom] || '');
      const cel = b.sheet.getRange(x.rij + 1, x.kolom + 1);
      const a1 = cel.getA1Notation();
      if (!(a1 in origineel)) {
        // Een kleur van een vorige controle is niet de oorspronkelijke achtergrond.
        const vanOns = huidigeNotitie.indexOf(NOTITIE) === 0;
        origineel[a1] = vanOns ? null : huidigeKleur;
      }
      // Niets veranderd: niet opnieuw schrijven (scheelt tijd en ruis in de versiegeschiedenis).
      if (huidigeKleur === kleur && huidigeNotitie.replace(/ \(\d+\/\d+ \d+:\d+\)$/, '') === notitie) return;
      cel.setBackground(kleur).setNote(notitie + ' (' + tijd + ')');
    });
    props.setProperty(sleutel, JSON.stringify(origineel));
    if (fouten) foutBladen.push(b.sheet.getName() + ' (' + fouten + ')');
  });

  let bericht = telling.ok + ' klopt, ' + telling.fout + ' verschilt';
  if (telling.onbekend) bericht += ', ' + telling.onbekend + ' niet gevonden';
  if (zonderKolom) bericht += ', ' + zonderKolom + ' links zonder kop "Prijs per eenheid/stuk" erboven';
  bericht += '.';
  if (foutBladen.length && sheets.length > 1) bericht += ' Rood op: ' + foutBladen.join(', ') + '.';
  if (telling.fout) bericht += ' De echte prijs staat in de notitie.';
  try {
    ss.toast(bericht, 'Prijsspoor', telling.fout ? 15 : 8);
  } catch (err) {
    // Geen venster om een melding in te tonen (bv. trigger zonder open spreadsheet).
  }
}

/** Zet de oorspronkelijke achtergrond terug en verwijdert de Prijsspoor-notities. */
function wisKleuren() {
  const sheet = SpreadsheetApp.getActiveSheet();
  const props = PropertiesService.getDocumentProperties();
  const sleutel = 'cg_kleuren_' + sheet.getSheetId();
  const origineel = JSON.parse(props.getProperty(sleutel) || '{}');
  Object.keys(origineel).forEach(function (a1) {
    const cel = sheet.getRange(a1);
    const kleur = origineel[a1];
    cel.setBackground(!kleur || kleur === '#ffffff' ? null : kleur);
    if (String(cel.getNote()).indexOf(NOTITIE) === 0) cel.setNote('');
  });
  props.deleteProperty(sleutel);
  SpreadsheetApp.getActive().toast(Object.keys(origineel).length + ' cellen hersteld', 'Prijsspoor');
}

/** De dichtstbijzijnde kop "Prijs per eenheid/stuk" boven rij r: { rij, kolom }, of null. */
function prijsKop_(waarden, r) {
  for (let k = r - 1; k >= 0 && k >= r - 80; k--) {
    const kolom = waarden[k].findIndex(function (v) { return PRIJSKOP.test(String(v)); });
    if (kolom >= 0) return { rij: k, kolom: kolom };
  }
  return null;
}

/**
 * De kolom met het aantal stuks op een koprij: "Eenheden" (variabele kosten: Aantal is daar
 * de hoeveelheid die je verbruikt), anders "Aantal". Telkens de dichtstbijzijnde links van
 * de prijskolom, want verderop staat soms nog een kolom "aantal" (bv. naast "Flesserke").
 */
function aantalKolom_(kopRij, prijsKolom) {
  for (const kop of [KOP_EENHEDEN, KOP_AANTAL]) {
    for (let c = prijsKolom - 1; c >= 0; c--) {
      if (kop.test(String(kopRij[c]))) return c;
    }
  }
  return -1;
}

function euro_(x) {
  return '€ ' + x.toFixed(2).replace('.', ',');
}

// --- automatisch bij openen -----------------------------------------------------------
//
// Een gewone onOpen mag niets ophalen. Daarom een installeerbare trigger per spreadsheet: die
// draait met de rechten van wie hem aanzette, telkens iemand met bewerkrechten de spreadsheet
// opent. Eén persoon volstaat dus voor iedereen. De trigger komt er vanzelf bij de eerste
// keer "Prijzen controleren", tenzij iemand hem eerder uitzette.

const AUTO = 'cg_auto'; // 'aan' of 'uit' in de documenteigenschappen
const AUTO_LAATST = 'cg_auto_laatst';
const AUTO_HANDLER = 'automatischBijOpenen';
const AUTO_PAUZE_MS = 10 * 60 * 1000;

/** De trigger: alle tabbladen controleren, hoogstens één keer per tien minuten. */
function automatischBijOpenen(e) {
  const ss = (e && e.source) || SpreadsheetApp.getActiveSpreadsheet();
  const props = PropertiesService.getDocumentProperties();
  if (props.getProperty(AUTO) === 'uit') return;
  // Openen meerdere mensen tegelijk, of hebben twee mensen een trigger: één controle volstaat.
  const lock = LockService.getDocumentLock();
  if (lock && !lock.tryLock(2000)) return;
  try {
    const laatst = Number(props.getProperty(AUTO_LAATST) || 0);
    if (Date.now() - laatst < AUTO_PAUZE_MS) return;
    props.setProperty(AUTO_LAATST, String(Date.now()));
  } finally {
    if (lock) lock.releaseLock();
  }
  try {
    ss.toast('Prijzen worden gecontroleerd…', 'Prijsspoor', 5);
  } catch (err) {
    // geen venster
  }
  controleer_(ss, ss.getSheets(), true);
}

function heeftTrigger_(ss) {
  return ScriptApp.getUserTriggers(ss).some(function (t) {
    return t.getHandlerFunction() === AUTO_HANDLER;
  });
}

function maakTrigger_(ss) {
  if (!heeftTrigger_(ss)) ScriptApp.newTrigger(AUTO_HANDLER).forSpreadsheet(ss).onOpen().create();
}

/** Na de eerste handmatige controle: automatisch aanzetten, tenzij iemand het uitzette. */
function zetAutomatischAanEersteKeer_(ss) {
  const props = PropertiesService.getDocumentProperties();
  if (props.getProperty(AUTO)) return;
  try {
    maakTrigger_(ss);
    props.setProperty(AUTO, 'aan');
    ss.toast('Vanaf nu controleert Prijsspoor de prijzen telkens deze spreadsheet geopend wordt. ' +
      'Uitzetten: Extensies > Prijsspoor.', 'Prijsspoor', 10);
  } catch (err) {
    // Bv. geen toestemming voor triggers: dan blijft het bij handmatig controleren.
    console.warn('automatisch controleren niet aangezet: ' + err);
  }
}

/** Menu: automatisch controleren aanzetten (ook om het opnieuw aan te zetten na "uit"). */
function zetAutomatischAan() {
  const ss = SpreadsheetApp.getActiveSpreadsheet();
  maakTrigger_(ss);
  PropertiesService.getDocumentProperties().setProperty(AUTO, 'aan');
  SpreadsheetApp.getUi().alert(
    'Automatisch controleren staat aan',
    'Telkens iemand deze spreadsheet opent, controleert Prijsspoor de prijzen op alle ' +
      'tabbladen (hoogstens één keer per tien minuten). De kleuren verschijnen na enkele ' +
      'seconden; de cellen worden enkel aangepast als er iets veranderde.',
    SpreadsheetApp.getUi().ButtonSet.OK
  );
}

/** Menu: uitzetten. Geldt voor iedereen, ook als iemand anders de trigger aanmaakte. */
function zetAutomatischUit() {
  const ss = SpreadsheetApp.getActiveSpreadsheet();
  ScriptApp.getUserTriggers(ss).forEach(function (t) {
    if (t.getHandlerFunction() === AUTO_HANDLER) ScriptApp.deleteTrigger(t);
  });
  PropertiesService.getDocumentProperties().setProperty(AUTO, 'uit');
  SpreadsheetApp.getUi().alert(
    'Automatisch controleren staat uit',
    'De prijzen worden niet meer gecontroleerd bij het openen van deze spreadsheet. ' +
      'Handmatig kan nog altijd via Extensies > Prijsspoor > Prijzen controleren.',
    SpreadsheetApp.getUi().ButtonSet.OK
  );
}
