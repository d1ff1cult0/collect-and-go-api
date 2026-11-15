# Prijsspoor

Volgt de prijzen van **alle producten op [Collect&Go](https://www.collectandgo.be)** en bewaart hun
prijsgeschiedenis. Eén Rust-binary met:

- een **scraper** die standaard elke 6 uur het volledige assortiment ophaalt (±19.500 producten);
- een **publieke website** om producten te zoeken en hun prijsverloop te bekijken;
- een **JSON-API**, onder meer om met een Collect&Go-link de huidige prijs op te vragen, samen met
  wanneer die prijs het laatst gecontroleerd en het laatst gewijzigd is.

## Snel starten met Docker

```bash
docker compose up -d --build
```

De site draait dan op <http://localhost:8080>. De eerste scrape start meteen bij het opstarten
en duurt een paar minuten. Daarna volgt elke 6 uur een nieuwe. Alle data staat in SQLite in het
volume `prijsspoor-data` (`/data/cogo.db` in de container).

Zonder compose:

```bash
docker build -t prijsspoor .
docker run -d -p 8080:8080 -v prijsspoor-data:/data -e PUBLIC_BASE_URL=https://prijzen.example.be prijsspoor
```

## API

De volledige documentatie staat op `/api` van je installatie. Het belangrijkste endpoint:

```bash
curl -G http://localhost:8080/api/v1/price \
  --data-urlencode 'url=https://www.collectandgo.be/nl/assortiment/boni-walnootpitten-200g'
```

```json
{
  "product": { "id": "3074457345616678942", "article_number": "28856", "name": "BONI walnootpitten 200g", "...": "..." },
  "price": { "currency": "EUR", "price": 3.19, "unit_price": 15.95, "unit": "kg",
             "offer": { "price": 3.06, "min_quantity": 3, "unit_price": 15.3 }, "is_red_price": false },
  "last_updated_at": "2026-11-15T18:00:04Z",
  "last_changed_at": "2026-11-02T06:00:03Z",
  "previous_price": 3.29,
  "tracked_since": "2026-10-20T12:00:01Z"
}
```

- `last_updated_at`: laatste keer dat deze prijs op Collect&Go gezien is.
- `last_changed_at`: eerste waarneming van de huidige prijs, dus de laatste prijswijziging. Is
  dit gelijk aan `tracked_since`, dan is de prijs niet veranderd sinds het product gevolgd wordt.

Andere endpoints:

| Endpoint | Beschrijving |
|---|---|
| `GET /api/v1/products?q=&page=&per_page=&sort=&available=` | zoeken |
| `GET /api/v1/products/{id}` | één product (product-ID of artikelnummer) |
| `GET /api/v1/products/{id}/history` | alle prijsperiodes |
| `GET /api/v1/changes?days=7&direction=down\|up` | recente prijswijzigingen |
| `GET /api/v1/status` | statistieken en laatste scrapes |
| `POST /api/v1/admin/scrape` | scrape nu starten (`Authorization: Bearer $ADMIN_TOKEN`) |

## Configuratie

Alles via omgevingsvariabelen:

| Variabele | Standaard | Betekenis |
|---|---|---|
| `BIND_ADDR` | `0.0.0.0:8080` | adres van de webserver |
| `DATABASE_URL` | `sqlite://data/cogo.db` | SQLite-database (wordt aangemaakt) |
| `PUBLIC_BASE_URL` | `http://localhost:8080` | publieke URL, voor links in de API |
| `SCRAPE_INTERVAL_HOURS` | `6` | tijd tussen scrapes |
| `SCRAPE_ON_STARTUP` | `true` | meteen scrapen als de laatste scrape ouder is dan het interval |
| `SCHEDULER_ENABLED` | `true` | ingebouwde scheduler aan/uit (bv. als je extern met `cogo-prices scrape` werkt) |
| `SCRAPE_PAGE_SIZE` | `500` | producten per API-request (24–1000) |
| `SCRAPE_REQUEST_DELAY_MS` | `1500` | pauze tussen requests naar Collect&Go |
| `SCRAPE_MAX_PAGES` | `0` | enkel voor testen: stop na zoveel pagina's |
| `ADMIN_TOKEN` | leeg | zet dit om `POST /api/v1/admin/scrape` in te schakelen |
| `COGO_RELAY_URL` | leeg | adres van de relay, bv. `http://100.64.0.5:8787` (zie hieronder) |
| `COGO_RELAY_SECRET` | leeg | gedeeld geheim met de relay (minstens 16 tekens) |
| `COGO_RELAY_MODE` | `fallback` | `fallback`: rechtstreeks en enkel via de relay als Collect&Go blokkeert; `always`: altijd via de relay |
| `COGO_PROXY` | leeg | HTTP(S)- of SOCKS5-proxy voor alle rechtstreekse aanvragen |
| `COGO_API_KEY` | ingebouwd | `X-CG-APIKey` van Collect&Go; wordt zelf vernieuwd als hij geweigerd wordt |
| `RUST_LOG` | `info` | logniveau |

## Geblokkeerd door Collect&Go (HTTP 456)

Vanaf het IP van de meeste servers (datacenters) antwoordt Collect&Go op alles met HTTP 456
("Colruyt | AntiBot"). In de logs zie je dan `request mislukt ... HTTP 456`. Vanaf een gewone
thuisverbinding werkt hetzelfde verzoek wel. De oplossing is Collect&Go opvragen vanaf een
residentieel IP, op een van deze manieren:

**1. De relay thuis (aangeraden).** `relay/server.cjs` is een kleine dienst die Collect&Go
namens Prijsspoor opvraagt. Hij is overgenomen van de prijsbrowser van de VTK-website. Laat hem
draaien op een machine met een thuisverbinding (thuisserver, Raspberry Pi, NAS) en maak hem
bereikbaar voor de server, bij voorkeur via Tailscale of WireGuard:

```bash
# op de machine thuis
cd relay
cp .env.example .env          # RELAY_SECRET=$(openssl rand -hex 24), RELAY_BIND=<tailscale-ip>
docker compose up -d

# op de server, in .env naast docker-compose.yml
COGO_RELAY_URL=http://<tailscale-ip-thuis>:8787
COGO_RELAY_SECRET=<hetzelfde geheim>
```

**2. De relay naast Prijsspoor, via een residentiële proxy.** `COMPOSE_PROFILES=relay`,
`COGO_RELAY_URL=http://relay:8787` en `RELAY_PROXY=<proxy>`. Zonder proxy heeft dit geen zin:
de relay gebruikt dan hetzelfde geweigerde IP.

**3. Enkel een proxy.** Heb je thuis al een HTTP- of SOCKS5-proxy (bv. via Tailscale), dan
volstaat `COGO_PROXY=socks5://<ip>:1080`, zonder relay.

Met `COGO_RELAY_MODE=fallback` (standaard) probeert Prijsspoor eerst rechtstreeks. Bij een
blokkade gaat het meteen via de relay, en de volgende 30 minuten enkel via de relay. In
`GET /api/v1/status` staat onder `transport` welke weg gebruikt wordt.

Hoe de relay werkt:

1. Hij doet eerst een gewone HTTP-aanvraag vanaf zijn eigen IP (of `RELAY_PROXY`), net zoals
   Prijsspoor. Vanaf een thuisverbinding is dat bijna altijd genoeg.
2. Wordt een pagina van de website toch tegengehouden, dan gebruikt hij een echte Chromium
   (Playwright, met een venster op een virtueel scherm). Die houdt één Collect&Go-pagina open,
   haalt pagina's op vanuit die pagina en zit een wachtpagina van de botbeveiliging uit zoals
   een bezoeker. Lukt dat niet, dan antwoordt de relay tien minuten lang meteen 456.

Wat de relay bewust niet doet: captcha's oplossen, een browservingerafdruk of de
challenge-handtekening van de API namaken, of als open proxy werken. Hij vraagt een geheim,
opent enkel https-adressen van `collectandgo.be` (`RELAY_HOSTS`) en doet hoogstens twee
opvragingen tegelijk. Antwoorden gaan gecomprimeerd terug: een volledige scrape kost ±24 MB
upload vanaf de thuisverbinding.

Lokaal testen zonder Docker (Playwright moet geïnstalleerd zijn):

```bash
RELAY_SECRET=$(openssl rand -hex 24) RELAY_LISTEN=127.0.0.1 node relay/server.cjs
```

## Hoe het werkt

**Scrapen.** De Collect&Go-website laadt productlijsten via een HCL Commerce-zoekdienst achter
`apip.collectandgo.be` (store 90004, catalogus 10429), met een publieke API-key die de site zelf
meestuurt. `productview/bySearchTerm/*` levert het volledige assortiment in pagina's van 500 producten
(±40 requests, ±24 MB gecomprimeerd per ronde). Mislukte requests worden met exponentiële
backoff opnieuw geprobeerd. Bij de anti-botcode 456 gaat het, als die ingesteld is, via de relay.

**Prijsgeschiedenis.** Elke rij in `prices` is een periode met dezelfde prijs: `first_seen_at` is het
moment van de wijziging en `last_seen_at` de laatste bevestiging. Een nieuwe rij komt er alleen als
de prijs, eenheidsprijs, hoeveelheidskorting, referentieprijs of rode-prijsvlag verandert. Zo blijft
de database klein, ook na jaren scrapen om de 6 uur. Bedragen staan in eurocent.

**Van URL naar product.** Productlinks bevatten geen product-ID. Na elke scrape leest Prijsspoor
daarom de product-sitemaps en koppelt elke URL (NL en FR) aan een product via de productafbeelding.
Dat dekt het grootste deel van de producten. Voor een link die nog niet gekend is, haalt de API de
productpagina één keer op, leest het product-ID eruit en bewaart de koppeling. Links die geen product
blijken te zijn, worden een uur onthouden zodat ze Collect&Go niet blijven belasten.

## Ontwikkelen

```bash
cargo test
cargo run                      # serve: website + API + scheduler
cargo run -- scrape            # één scrape en stoppen
SCRAPE_MAX_PAGES=2 SCRAPE_PAGE_SIZE=100 cargo run -- scrape   # snelle testscrape
```

Inspiratie: [BelgianNoise/colruyt-products-scraper](https://github.com/BelgianNoise/colruyt-products-scraper).

Prijsspoor is een onafhankelijk project en hoort niet bij Colruyt Group.
