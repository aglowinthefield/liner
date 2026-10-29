use liner::{build_router, cache::Cache, config::Config, db, ApiDoc, AppState};
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::from_env();
    let mode = std::env::args().nth(1).unwrap_or_else(|| "serve".to_string());

    let pool = db::create_pool(&config.database_url).await?;
    db::run_migrations(&pool).await?;

    match mode.as_str() {
        "serve" => serve(config, pool).await,
        "worker" => worker().await,
        other => anyhow::bail!("unknown command `{other}`, expected `serve` or `worker`"),
    }
}

async fn serve(config: Config, pool: sqlx::PgPool) -> anyhow::Result<()> {
    let cache = Cache::connect(&config.redis_url).await?;
    let state = AppState { pool, cache, mb_api_url: config.musicbrainz_api_url };
    let app = build_router(state)
        .merge(Scalar::with_url("/scalar", ApiDoc::openapi()).custom_html(include_str!("../res/scalar.html")));

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    tracing::info!("liner listening on 0.0.0.0:{}, docs at /scalar", config.port);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn worker() -> anyhow::Result<()> {
    // The job queue lands with the enrichment step; until then the worker idles.
    tracing::info!("liner worker started (no jobs yet)");
    std::future::pending::<()>().await;
    Ok(())
}
