/**
 * Collect&Go-prijzen in Google Sheets, via de Prijsspoor-API.
 *
 * Installeren: in je spreadsheet Extensies > Apps Script, plak dit bestand in Code.gs en
 * sla op. Daarna werken in elke cel:
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
 * opnieuw ophalen, kies dan in het menu "Collect&Go" > "Prijzen vernieuwen".
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

function onOpen() {
  SpreadsheetApp.getUi()
    .createMenu('Collect&Go')
    .addItem('Prijzen vernieuwen', 'vernieuwPrijzen')
    .addToUi();
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
  const tekst = String(waarde || '');
  const inTekst = tekst.match(/https?:\/\/(?:www\.)?collectandgo\.be\/\S+/i);
  if (inTekst) return inTekst[0];

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
  const rich = sheet.getRange(m[2] + m[3]).getRichTextValue();
  if (!rich) return null;
  const urls = [rich.getLinkUrl()].concat(rich.getRuns().map(function (run) { return run.getLinkUrl(); }));
  return urls.find(function (u) { return u && /collectandgo\.be/i.test(u); }) || null;
}

function haalPrijsOp_(url) {
  const cache = CacheService.getDocumentCache();
  // Eigen functies mogen properties enkel lezen; "Prijzen vernieuwen" zet een nieuwe versie.
  const versie = PropertiesService.getDocumentProperties().getProperty('cg_versie') || '0';
  const sleutel = 'cg:' + versie + ':' +
    Utilities.base64Encode(Utilities.computeDigest(Utilities.DigestAlgorithm.MD5, url));
  const bewaard = cache.get(sleutel);
  if (bewaard) return JSON.parse(bewaard);

  const resp = UrlFetchApp.fetch(PRIJSSPOOR_API + '?url=' + encodeURIComponent(url), {
    muteHttpExceptions: true,
  });
  const code = resp.getResponseCode();
  let data;
  if (code === 200) {
    data = JSON.parse(resp.getContentText());
  } else if (code === 404) {
    data = { error: 'niet gevonden op C&G' };
  } else if (code === 400) {
    data = { error: 'geen productlink' };
  } else {
    // Tijdelijke fout: niet bewaren, de volgende berekening probeert opnieuw.
    return { error: 'C&G niet bereikbaar (' + code + ')' };
  }
  cache.put(sleutel, JSON.stringify(data), CACHE_SECONDS);
  return data;
}

function euro_(x) {
  return '€ ' + x.toFixed(2).replace('.', ',');
}
