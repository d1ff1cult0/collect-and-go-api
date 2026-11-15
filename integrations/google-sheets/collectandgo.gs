/**
 * Collect&Go-prijzen in Google Sheets, via de Prijsspoor-API.
 *
 * Werkt op twee manieren (zie README.md):
 * - als add-on voor de hele organisatie (Marketplace, privé), of
 * - in één spreadsheet: Extensies > Apps Script, plak dit bestand in Code.gs en sla op.
 *
 * Daarna werken in elke cel:
 *
 *   =CGPRIJS(O45)                          de huidige prijs van het product in O45
 *   =CGPRIJS(O45; "eenheidsprijs")         prijs per kg/l
 *   =CGCHECK(I45; O45)                     "✓" als I45 de huidige prijs is, anders wat C&G nu vraagt
 *
 * O45 mag een link zijn, of tekst met een link erachter (zoals "STELLA ARTOIS pils ...").
 * Velden voor CGPRIJS: prijs (standaard), eenheidsprijs, eenheid, korting, korting_vanaf,
 * naam, beschikbaar, laatst_gecontroleerd, laatst_gewijzigd, vorige_prijs.
 *
 * Google herberekent een eigen functie enkel als haar invoer wijzigt. Wil je de prijzen
 * opnieuw ophalen, kies dan Extensies > Prijsspoor > Formules vernieuwen.

 * Zonder formules: Extensies > Prijsspoor > Prijzen controleren kleurt de prijzen groen of
 * rood (zie controleerPrijzen hieronder).
 */

const PRIJSSPOOR_API = 'https://c-en-g.vtk.be/api/v1/price';
const CACHE_SECONDS = 60 * 60; // de prijzen worden om de zes uur gescraped

/**
 * Huidige Collect&Go-prijs (of een ander veld) van het product waar een cel naar linkt.
 *
 * @param {A1} cel De cel met de link of de linktekst, bv. O45.
 * @param {"prijs"} veld Optioneel: prijs, eenheidsprijs, eenheid, korting, korting_vanaf, naam, beschikbaar, laatst_gecontroleerd, laatst_gewijzigd, vorige_prijs.
 * @return De gevraagde waarde, of een korte uitleg als het niet lukt.
 * @customfunction
 */
function CGPRIJS(cel, veld) {
  const url = linkUitCel_(cel);
  if (!url) return 'geen Collect&Go-link';
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
 * Klopt de prijs in de spreadsheet nog met Collect&Go?
 *
 * @param {16.45} prijs De prijs in je spreadsheet, bv. I45.
 * @param {A1} cel De cel met de link of de linktekst, bv. O45.
 * @return "✓" als de prijs klopt, anders "✗ C&G: € x,xx".
 * @customfunction
 */
function CGCHECK(prijs, cel) {
  const url = linkUitCel_(cel);
  if (!url) return '';
  const data = haalPrijsOp_(url);
  if (data.error) return data.error;
  const huidig = data.price && data.price.price;
  if (huidig == null) return 'geen prijs';
  if (typeof prijs !== 'number') return 'C&G: ' + euro_(huidig);
  if (Math.abs(prijs - huidig) < 0.005) return '✓';
  // Hoeveelheidskorting: de prijs in de spreadsheet is misschien de kortingsprijs.
  const k = data.price.offer;
  if (k && Math.abs(prijs - k.price) < 0.005) return '✓ (vanaf ' + k.min_quantity + ' st.)';
  return '✗ C&G: ' + euro_(huidig);
}

/**
 * Menu onder Extensies. `createAddonMenu` werkt zowel in een add-on als in een script in één
 * spreadsheet. In een add-on draait onOpen eerst zonder rechten (AuthMode.NONE): hier dus
 * enkel het menu maken, niets anders.
 */
function onOpen(e) {
  SpreadsheetApp.getUi()
    .createAddonMenu()
    .addItem('Prijzen controleren (groen/rood)', 'controleerPrijzen')
    .addItem('Kleuren wissen', 'wisKleuren')
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
    'Prijzen controleren (groen/rood) kleurt elke prijs naast een Collect&Go-link:\n' +
      '    groen = klopt, rood = anders (de echte prijs staat in de notitie),\n' +
      '    oranje = product niet gevonden.\n\n' +
      'Of met formules, in een vrije kolom op de rij van een product:\n\n' +
      '=CGCHECK(I45; O45)\n' +
      '    ✓ als de prijs in I45 de huidige Collect&Go-prijs is,\n' +
      '    anders "✗ C&G: € …" met de huidige prijs.\n\n' +
      '=CGPRIJS(O45)\n' +
      '    de huidige prijs als getal.\n\n' +
      '=CGPRIJS(O45; "laatst_gewijzigd")\n' +
      '    ook: eenheidsprijs, eenheid, korting, korting_vanaf, naam,\n' +
      '    beschikbaar, laatst_gecontroleerd, vorige_prijs.\n\n' +
      'O45 is de cel met de Collect&Go-link (of tekst met een link erachter).\n' +
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
  SpreadsheetApp.getActive().toast(cellen.length + ' formules vernieuwd', 'Collect&Go');
}

// --- intern ---------------------------------------------------------------------------

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

function haalPrijsOp_(url) {
  return haalPrijzenOp_([url])[url];
}

/** Prijzen voor een lijst links: eerst uit de cache, de rest in één keer tegelijk. */
function haalPrijzenOp_(urls) {
  const cache = CacheService.getDocumentCache();
  // Eigen functies mogen properties enkel lezen; "Formules vernieuwen" zet een nieuwe versie.
  const versie = PropertiesService.getDocumentProperties().getProperty('cg_versie') || '0';
  const sleutel = function (url) {
    return 'cg:' + versie + ':' +
      Utilities.base64Encode(Utilities.computeDigest(Utilities.DigestAlgorithm.MD5, url));
  };
  const uit = {};
  const teHalen = [];
  urls.forEach(function (url) {
    if (url in uit) return;
    const bewaard = cache.get(sleutel(url));
    if (bewaard) uit[url] = JSON.parse(bewaard);
    else if (teHalen.indexOf(url) === -1) teHalen.push(url);
  });
  for (let i = 0; i < teHalen.length; i += 20) {
    const deel = teHalen.slice(i, i + 20);
    const antwoorden = UrlFetchApp.fetchAll(deel.map(function (url) {
      return { url: PRIJSSPOOR_API + '?url=' + encodeURIComponent(url), muteHttpExceptions: true };
    }));
    antwoorden.forEach(function (resp, k) {
      const url = deel[k];
      const code = resp.getResponseCode();
      let data;
      if (code === 200) data = JSON.parse(resp.getContentText());
      else if (code === 404) data = { error: 'niet gevonden op C&G' };
      else if (code === 400) data = { error: 'geen productlink' };
      else {
        // Tijdelijke fout: niet bewaren, de volgende keer opnieuw proberen.
        uit[url] = { error: 'C&G niet bereikbaar (' + code + ')' };
        return;
      }
      cache.put(sleutel(url), JSON.stringify(data), CACHE_SECONDS);
      uit[url] = data;
    });
  }
  return uit;
}

// --- groen/rood -----------------------------------------------------------------------

const KLEUR_OK = '#c9ecc6';
const KLEUR_FOUT = '#f7c6c0';
const KLEUR_ONBEKEND = '#fde3a7';
const NOTITIE = 'Prijsspoor: ';
const PRIJSKOP = /^\s*prijs\s+per\s+(eenheid|stuk)\s*$/i;

/**
 * Kleurt op het actieve blad elke prijs naast een Collect&Go-link: groen als ze klopt, rood
 * als Collect&Go nu iets anders vraagt (de echte prijs staat in de notitie), oranje als het
 * product niet gevonden werd. De prijscel is de cel op dezelfde rij, in de kolom van de
 * dichtstbijzijnde kop "Prijs per eenheid" of "Prijs per stuk" erboven.
 */
function controleerPrijzen() {
  const sheet = SpreadsheetApp.getActiveSheet();
  const range = sheet.getDataRange();
  const waarden = range.getValues();
  const opmaak = range.getRichTextValues();

  const items = [];
  let zonderKolom = 0;
  for (let r = 0; r < waarden.length; r++) {
    for (let c = 0; c < waarden[r].length; c++) {
      const url = linkUitRich_(opmaak[r][c]) || linkUitTekst_(waarden[r][c]);
      if (!url) continue;
      const kolom = prijsKolom_(waarden, r);
      if (kolom < 0) {
        zonderKolom++;
        continue;
      }
      items.push({ rij: r, kolom: kolom, url: url, prijs: waarden[r][kolom] });
    }
  }
  if (!items.length) {
    SpreadsheetApp.getUi().alert('Geen Collect&Go-links gevonden op dit blad.');
    return;
  }

  const prijzen = haalPrijzenOp_(items.map(function (x) { return x.url; }));
  const props = PropertiesService.getDocumentProperties();
  const sleutel = 'cg_kleuren_' + sheet.getSheetId();
  const origineel = JSON.parse(props.getProperty(sleutel) || '{}');
  const tijd = Utilities.formatDate(new Date(), 'Europe/Brussels', 'd/MM HH:mm');
  const telling = { ok: 0, fout: 0, onbekend: 0 };

  items.forEach(function (x) {
    const cel = sheet.getRange(x.rij + 1, x.kolom + 1);
    const a1 = cel.getA1Notation();
    if (!(a1 in origineel)) origineel[a1] = cel.getBackground();
    const data = prijzen[x.url];
    const p = data && !data.error && data.price;
    if (!p || p.price == null) {
      cel.setBackground(KLEUR_ONBEKEND).setNote(NOTITIE + ((data && data.error) || 'geen prijs') + ' (' + tijd + ')');
      telling.onbekend++;
      return;
    }
    const naam = data.product.name;
    if (typeof x.prijs === 'number' && Math.abs(x.prijs - p.price) < 0.005) {
      cel.setBackground(KLEUR_OK).setNote(NOTITIE + 'klopt, ' + naam + ' ' + euro_(p.price) + ' (' + tijd + ')');
      telling.ok++;
    } else if (typeof x.prijs === 'number' && p.offer && Math.abs(x.prijs - p.offer.price) < 0.005) {
      cel.setBackground(KLEUR_OK).setNote(NOTITIE + 'klopt met de hoeveelheidskorting vanaf ' +
        p.offer.min_quantity + ' st. (normaal ' + euro_(p.price) + ') (' + tijd + ')');
      telling.ok++;
    } else {
      let notitie = NOTITIE + 'Collect&Go vraagt nu ' + euro_(p.price) + ' voor ' + naam;
      if (p.offer) notitie += ', of ' + euro_(p.offer.price) + ' vanaf ' + p.offer.min_quantity + ' st.';
      if (!data.product.is_available) notitie += '. Momenteel niet bestelbaar.';
      cel.setBackground(KLEUR_FOUT).setNote(notitie + ' (' + tijd + ')');
      telling.fout++;
    }
  });
  props.setProperty(sleutel, JSON.stringify(origineel));

  let bericht = telling.ok + ' klopt, ' + telling.fout + ' verschilt';
  if (telling.onbekend) bericht += ', ' + telling.onbekend + ' niet gevonden';
  if (zonderKolom) bericht += ', ' + zonderKolom + ' links zonder kop "Prijs per eenheid/stuk" erboven';
  SpreadsheetApp.getActive().toast(bericht + '. Rode cellen: de echte prijs staat in de notitie.', 'Prijsspoor', 8);
}

/** Zet de oorspronkelijke achtergrond terug en verwijdert de Prijsspoor-notities. */
function wisKleuren() {
  const sheet = SpreadsheetApp.getActiveSheet();
  const props = PropertiesService.getDocumentProperties();
  const sleutel = 'cg_kleuren_' + sheet.getSheetId();
  const origineel = JSON.parse(props.getProperty(sleutel) || '{}');
  Object.keys(origineel).forEach(function (a1) {
    const cel = sheet.getRange(a1);
    cel.setBackground(origineel[a1] === '#ffffff' ? null : origineel[a1]);
    if (String(cel.getNote()).indexOf(NOTITIE) === 0) cel.setNote('');
  });
  props.deleteProperty(sleutel);
  SpreadsheetApp.getActive().toast(Object.keys(origineel).length + ' cellen hersteld', 'Prijsspoor');
}

function linkUitRich_(rich) {
  if (!rich) return null;
  const urls = [rich.getLinkUrl()].concat(rich.getRuns().map(function (run) { return run.getLinkUrl(); }));
  return urls.find(function (u) { return u && /collectandgo\.be/i.test(u); }) || null;
}

function linkUitTekst_(waarde) {
  const m = String(waarde || '').match(/https?:\/\/(?:www\.)?collectandgo\.be\/\S+/i);
  return m ? m[0] : null;
}

/** Kolom van de dichtstbijzijnde kop "Prijs per eenheid/stuk" boven rij r, of -1. */
function prijsKolom_(waarden, r) {
  for (let k = r - 1; k >= 0 && k >= r - 80; k--) {
    const kolom = waarden[k].findIndex(function (v) { return PRIJSKOP.test(String(v)); });
    if (kolom >= 0) return kolom;
  }
  return -1;
}

function euro_(x) {
  return '€ ' + x.toFixed(2).replace('.', ',');
}
