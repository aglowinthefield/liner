use liner::{
    build_router,
    cache::Cache,
    config::Config,
    db,
    llm::Llm,
    ratelimit::RateLimitConfig,
    sources::{fanart::Fanart, lastfm::Lastfm},
    ApiDoc, AppState,
};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "serve".to_string());

    let pool = db::create_pool(&config.database_url).await?;
    db::run_migrations(&pool).await?;

    if mode == "key" {
        return key_command(&pool).await;
    }

    match mode.as_str() {
        "serve" => serve(config, pool).await,
        "worker" => worker(config, pool).await,
        other => anyhow::bail!(
            "unknown command `{other}`, expected `serve`, `worker`, or `key create <name> [limit]`"
        ),
    }
}

async fn app_state(config: &Config, pool: sqlx::PgPool) -> anyhow::Result<AppState> {
    Ok(AppState {
        pool,
        cache: Cache::connect(&config.redis_url).await?,
        mb_api_url: config.musicbrainz_api_url.clone(),
        wiki: Default::default(),
        lastfm: config.lastfm_api_key.as_deref().map(Lastfm::production),
        fanart: config.fanart_api_key.as_deref().map(Fanart::production),
        listenbrainz: Default::default(),
        cover_art: Default::default(),
        llm: config.llm_api_key.as_deref().map(|key| {
            Llm::new(
                &config.llm_base_url,
                key,
                &config.llm_model,
                config.llm_daily_artist_cap,
            )
        }),
        rate: RateLimitConfig {
            anon_per_min: config.rate_limit_anon_per_min,
            key_per_min: config.rate_limit_key_per_min,
            trust_proxy: config.trust_proxy,
        },
    })
}

async fn serve(config: Config, pool: sqlx::PgPool) -> anyhow::Result<()> {
    let state = app_state(&config, pool).await?;
    let app = build_router(state).merge(
        Scalar::with_url("/scalar", ApiDoc::openapi())
            .custom_html(include_str!("../res/scalar.html")),
    );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    tracing::info!(
        "liner listening on 0.0.0.0:{}, docs at /scalar",
        config.port
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

async fn worker(config: Config, pool: sqlx::PgPool) -> anyhow::Result<()> {
    let state = app_state(&config, pool).await?;
    liner::worker::run(state).await;
    Ok(())
}

/// `liner key create <name> [per-minute limit]` prints a new API key once and stores its hash.
async fn key_command(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    match args.as_slice() {
        [sub, name, rest @ ..] if sub == "create" => {
            let limit: Option<i32> = rest.first().map(|l| l.parse()).transpose()?;
            let key = format!(
                "lk_{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            sqlx::query(
                "INSERT INTO api_keys (name, key_hash, rate_limit_per_min) VALUES ($1, $2, $3)",
            )
            .bind(name)
            .bind(liner::ratelimit::hash_key(&key))
            .bind(limit)
            .execute(pool)
            .await?;
            println!("{key}");
            Ok(())
        }
        _ => anyhow::bail!("usage: liner key create <name> [per-minute limit]"),
    }
}
