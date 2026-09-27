//! Against a mock HTTP server: what we send, and what we make of what comes back.

use serana_domain::prices::{AssetId, PriceError, PriceSource, Quote};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

fn assets() -> Vec<AssetId> {
    vec![
        AssetId::new("bitcoin"),
        AssetId::new("ethereum"),
        AssetId::new("cow-protocol"),
    ]
}

fn markets_body() -> serde_json::Value {
    // Deliberately in CoinGecko's own ranking order, which is not the order we asked in.
    serde_json::json!([
        { "id": "bitcoin", "symbol": "btc", "current_price": 84753.0,
          "price_change_percentage_24h": 0.90091 },
        { "id": "ethereum", "symbol": "eth", "current_price": 2692.68,
          "price_change_percentage_24h": 0.38441 },
        { "id": "cow-protocol", "symbol": "cow", "current_price": 0.160295,
          "price_change_percentage_24h": 4.57522 },
    ])
}

async fn coingecko(server: &MockServer, api_key: &str) -> CoinGecko {
    CoinGecko::new(CoinGeckoConfig {
        api_key: api_key.to_owned(),
        api_base: server.uri(),
        ..CoinGeckoConfig::default()
    })
    .unwrap()
}

async fn defillama(server: &MockServer) -> DefiLlama {
    DefiLlama::new(DefiLlamaConfig {
        api_base: server.uri(),
        ..DefiLlamaConfig::default()
    })
    .unwrap()
}

#[tokio::test]
async fn coingecko_is_asked_for_usd_and_the_24_hour_move() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/coins/markets"))
        .and(query_param("vs_currency", "usd"))
        .and(query_param("ids", "bitcoin,ethereum,cow-protocol"))
        .and(query_param("price_change_percentage", "24h"))
        .respond_with(ResponseTemplate::new(200).set_body_json(markets_body()))
        .mount(&server)
        .await;

    let quotes = coingecko(&server, "")
        .await
        .quotes(&assets())
        .await
        .unwrap();
    assert_eq!(quotes.len(), 3);
}

#[tokio::test]
async fn quotes_come_back_in_the_order_they_were_asked_for() {
    // CoinGecko answers in its own ranking order; the digest shows the watchlist in the
    // order the person wrote it.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(markets_body()))
        .mount(&server)
        .await;

    let asked = vec![AssetId::new("cow-protocol"), AssetId::new("bitcoin")];
    let quotes = coingecko(&server, "").await.quotes(&asked).await.unwrap();
    assert_eq!(
        quotes.iter().map(|q| q.symbol.as_str()).collect::<Vec<_>>(),
        vec!["COW", "BTC"]
    );
}

#[tokio::test]
async fn the_ticker_comes_from_the_source_rather_than_a_table_we_keep() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(markets_body()))
        .mount(&server)
        .await;

    let quotes = coingecko(&server, "")
        .await
        .quotes(&assets())
        .await
        .unwrap();
    assert_eq!(quotes[2].symbol, "COW", "upper-cased from the response");
    assert_eq!(quotes[2].usd, 0.160295);
    assert_eq!(quotes[2].change_24h, Some(4.57522));
}

#[tokio::test]
async fn an_asset_the_source_could_not_price_is_left_out_rather_than_shown_as_free() {
    // A zero here reads as "COW is worthless", which is the worst possible rendering of
    // "CoinGecko did not answer for it".
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "id": "bitcoin", "symbol": "btc", "current_price": 84753.0 },
            { "id": "cow-protocol", "symbol": "cow", "current_price": null },
        ])))
        .mount(&server)
        .await;

    let quotes = coingecko(&server, "")
        .await
        .quotes(&assets())
        .await
        .unwrap();
    assert_eq!(quotes.len(), 1, "{quotes:?}");
    assert_eq!(quotes[0].symbol, "BTC");
}

#[tokio::test]
async fn a_missing_24_hour_move_is_absent_rather_than_zero() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "id": "bitcoin", "symbol": "btc", "current_price": 84753.0 },
        ])))
        .mount(&server)
        .await;

    let quotes = coingecko(&server, "")
        .await
        .quotes(&assets())
        .await
        .unwrap();
    assert_eq!(quotes[0].change_24h, None);
}

#[tokio::test]
async fn a_demo_key_is_sent_when_there_is_one_and_not_when_there_is_not() {
    let keyed = MockServer::start().await;
    Mock::given(method("GET"))
        .and(header("x-cg-demo-api-key", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(markets_body()))
        .mount(&keyed)
        .await;
    assert!(
        coingecko(&keyed, "secret")
            .await
            .quotes(&assets())
            .await
            .is_ok()
    );

    // The keyless route is the default, and a blank key must not become a blank header.
    let bare = MockServer::start().await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::header_regex("x-cg-demo-api-key", ".*"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&bare)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(markets_body()))
        .mount(&bare)
        .await;
    assert!(coingecko(&bare, "").await.quotes(&assets()).await.is_ok());
}

#[tokio::test]
async fn being_rate_limited_says_so_rather_than_reporting_a_bare_number() {
    // 429 is what the keyless route does under load, and it is the whole reason the
    // fallback exists.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let error = coingecko(&server, "")
        .await
        .quotes(&assets())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("rate limited"), "{error}");
}

#[tokio::test]
async fn an_unreadable_body_is_a_failure_rather_than_an_empty_market() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>maintenance</html>"))
        .mount(&server)
        .await;
    assert!(
        coingecko(&server, "")
            .await
            .quotes(&assets())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn nothing_is_asked_for_an_empty_watchlist() {
    // A request with no ids is a request CoinGecko answers with the entire market.
    let server = MockServer::start().await;
    assert!(
        coingecko(&server, "")
            .await
            .quotes(&[])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn defillama_namespaces_the_ids_and_reports_no_24_hour_move() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/prices/current/coingecko:bitcoin,coingecko:ethereum,coingecko:cow-protocol",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "coins": {
                "coingecko:bitcoin": { "price": 84671.86, "symbol": "BTC", "confidence": 0.99 },
                "coingecko:ethereum": { "price": 2690.38, "symbol": "ETH", "confidence": 0.99 },
                "coingecko:cow-protocol": { "price": 0.16106, "symbol": "COW", "confidence": 0.99 },
            }
        })))
        .mount(&server)
        .await;

    let quotes = defillama(&server).await.quotes(&assets()).await.unwrap();
    assert_eq!(quotes.len(), 3);
    assert_eq!(quotes[2].symbol, "COW");
    assert_eq!(quotes[2].usd, 0.16106);
    assert_eq!(quotes[2].change_24h, None, "this route does not report it");
}

/// A source that always fails, for driving the chain.
struct Broken(&'static str);

#[async_trait::async_trait]
impl PriceSource for Broken {
    fn name(&self) -> &str {
        self.0
    }
    async fn quotes(&self, _: &[AssetId]) -> Result<Vec<Quote>, PriceError> {
        Err(PriceError::new("429 (rate limited)"))
    }
}

/// A source that answers, so the chain has somewhere to fall back to.
struct Working(&'static str);

#[async_trait::async_trait]
impl PriceSource for Working {
    fn name(&self) -> &str {
        self.0
    }
    async fn quotes(&self, assets: &[AssetId]) -> Result<Vec<Quote>, PriceError> {
        Ok(assets
            .iter()
            .map(|asset| Quote {
                asset: asset.clone(),
                symbol: asset.as_str().to_uppercase(),
                usd: 1.0,
                change_24h: None,
            })
            .collect())
    }
}

#[tokio::test]
async fn the_chain_falls_through_to_the_next_source_and_says_which_answered() {
    let chain = Chain::new(vec![Box::new(Broken("first")), Box::new(Working("second"))]);
    let quotes = chain.quotes(&assets()).await.unwrap();

    assert_eq!(quotes.len(), 3);
    assert_eq!(chain.answered(), "second");
}

#[tokio::test]
async fn the_preferred_source_is_used_when_it_works() {
    let chain = Chain::new(vec![Box::new(Working("first")), Box::new(Broken("second"))]);
    chain.quotes(&assets()).await.unwrap();
    assert_eq!(chain.answered(), "first");
}

#[tokio::test]
async fn a_source_that_prices_nothing_is_treated_as_a_failure() {
    // An empty answer is this source not knowing the assets, not an empty market. The next
    // source may well know them.
    struct Silent;
    #[async_trait::async_trait]
    impl PriceSource for Silent {
        fn name(&self) -> &str {
            "silent"
        }
        async fn quotes(&self, _: &[AssetId]) -> Result<Vec<Quote>, PriceError> {
            Ok(Vec::new())
        }
    }

    let chain = Chain::new(vec![Box::new(Silent), Box::new(Working("second"))]);
    assert_eq!(chain.quotes(&assets()).await.unwrap().len(), 3);
    assert_eq!(chain.answered(), "second");
}

#[tokio::test]
async fn every_source_failing_reports_all_of_them() {
    // One line naming each attempt, because "prices unavailable" with two sources
    // configured tells whoever reads the log nothing about which to look at.
    let chain = Chain::new(vec![Box::new(Broken("first")), Box::new(Broken("second"))]);
    let error = chain.quotes(&assets()).await.unwrap_err();
    assert!(error.to_string().contains("first"), "{error}");
    assert!(error.to_string().contains("second"), "{error}");
}

#[tokio::test]
async fn searching_asks_coingecko_by_name_and_keeps_only_coins() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("query", "1inch"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "coins": [
                { "id": "1inch", "name": "1INCH", "symbol": "1INCH", "market_cap_rank": 233 },
                { "id": "1inch-yvault", "name": "1INCH yVault", "symbol": "YV1INCH",
                  "market_cap_rank": null },
            ],
            "exchanges": [{ "id": "1inch-exchange", "name": "1inch" }],
            "categories": [],
        })))
        .mount(&server)
        .await;

    let found = coingecko(&server, "").await.search("1inch").await.unwrap();
    assert_eq!(found.len(), 2, "exchanges are not assets");
    assert_eq!(found[0].id, AssetId::new("1inch"));
    assert_eq!(found[0].rank, Some(233));
    assert_eq!(found[1].rank, None);
}

#[tokio::test]
async fn a_ticker_padded_with_a_non_breaking_space_is_trimmed() {
    // CoinGecko really does send "\u{a0}1COW".
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "coins": [{ "id": "onecow", "name": "OneCOW", "symbol": "\u{a0}1COW",
                        "market_cap_rank": 3275 }],
        })))
        .mount(&server)
        .await;

    let found = coingecko(&server, "").await.search("1cow").await.unwrap();
    assert_eq!(found[0].symbol, "1COW");
}

#[tokio::test]
async fn an_empty_search_asks_nothing() {
    let server = MockServer::start().await;
    assert!(
        coingecko(&server, "")
            .await
            .search("  ")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn defillama_cannot_search_and_says_so() {
    let server = MockServer::start().await;
    assert!(defillama(&server).await.search("1inch").await.is_err());
}

#[tokio::test]
async fn the_chain_searches_with_the_first_source_that_can() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "coins": [{ "id": "1inch", "name": "1INCH", "symbol": "1INCH",
                        "market_cap_rank": 233 }],
        })))
        .mount(&server)
        .await;

    // DefiLlama first on purpose: it cannot search, so the chain has to move past it.
    let chain = Chain::new(vec![
        Box::new(defillama(&server).await),
        Box::new(coingecko(&server, "").await),
    ]);
    let found = chain.search("1inch").await.unwrap();
    assert_eq!(found[0].id, AssetId::new("1inch"));
}
