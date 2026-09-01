use renuxa_server::{AppState, app};
use sqlx::postgres::PgPoolOptions;
use std::{env, sync::Arc};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhowless::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "renuxa_server=info,tower_http=info".into()),
        )
        .json()
        .init();

    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://renuxa:renuxa@127.0.0.1:5432/renuxa".into());
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    let state = AppState {
        db: pool,
        jwt_secret: Arc::from(
            env::var("JWT_SECRET").unwrap_or_else(|_| "development-only-change-me".into()),
        ),
        http: reqwest::Client::new(),
    };
    let addr = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "Renuxa API listening");
    axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

mod anyhowless {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
}
