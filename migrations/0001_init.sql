-- Producten zoals ze op Collect&Go staan. `id` is het Collect&Go "uniqueID" van het product.
CREATE TABLE products (
    id              TEXT PRIMARY KEY NOT NULL,
    part_number     TEXT NOT NULL,
    tech_article_id TEXT,
    name            TEXT NOT NULL,
    long_name       TEXT,
    brand           TEXT,
    content         TEXT,
    unit            TEXT,
    image_path      TEXT,
    image_asset     TEXT,
    offered_by      TEXT,
    is_available    INTEGER NOT NULL DEFAULT 0,
    is_ranged       INTEGER NOT NULL DEFAULT 0,
    promotions      TEXT NOT NULL DEFAULT '[]',
    search_text     TEXT NOT NULL,
    first_seen_at   TEXT NOT NULL,
    last_seen_at    TEXT NOT NULL,
    current_price_id INTEGER
);
CREATE INDEX products_image_asset ON products(image_asset);
CREATE INDEX products_part_number ON products(part_number);

-- Prijsgeschiedenis. Elke rij is een periode waarin een product dezelfde prijs had:
-- `first_seen_at` = eerste waarneming van deze prijs (moment van wijziging),
-- `last_seen_at`  = laatste waarneming van deze prijs.
-- Bedragen in eurocent.
CREATE TABLE prices (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    product_id       TEXT NOT NULL REFERENCES products(id),
    base_price       INTEGER NOT NULL,
    unit_price       INTEGER,
    offer_price      INTEGER,
    offer_quantity   INTEGER,
    offer_unit_price INTEGER,
    reference_price  INTEGER,
    is_red_price     INTEGER NOT NULL DEFAULT 0,
    first_seen_at    TEXT NOT NULL,
    last_seen_at     TEXT NOT NULL
);
CREATE INDEX prices_product_time ON prices(product_id, first_seen_at);
CREATE INDEX prices_first_seen ON prices(first_seen_at);

-- Koppeling Collect&Go-URL-pad (genormaliseerd: /nl/assortiment/<slug>) -> product.
CREATE TABLE product_urls (
    path       TEXT PRIMARY KEY NOT NULL,
    product_id TEXT NOT NULL,
    lang       TEXT NOT NULL,
    source     TEXT NOT NULL,      -- 'sitemap' of 'page'
    updated_at TEXT NOT NULL
);
CREATE INDEX product_urls_product ON product_urls(product_id);

-- Negatieve cache voor URL's die geen product bleken te zijn.
CREATE TABLE unresolved_urls (
    path       TEXT PRIMARY KEY NOT NULL,
    reason     TEXT NOT NULL,
    checked_at TEXT NOT NULL
);

CREATE TABLE scrape_runs (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at     TEXT NOT NULL,
    finished_at    TEXT,
    status         TEXT NOT NULL,   -- running | success | partial | failed
    products_seen  INTEGER NOT NULL DEFAULT 0,
    products_total INTEGER,
    new_products   INTEGER NOT NULL DEFAULT 0,
    prices_changed INTEGER NOT NULL DEFAULT 0,
    urls_mapped    INTEGER NOT NULL DEFAULT 0,
    error          TEXT
);
