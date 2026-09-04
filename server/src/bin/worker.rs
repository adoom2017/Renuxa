use renuxa_server::{AppState, worker};
use sqlx::postgres::PgPoolOptions;
use std::{env, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt().json().init();
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://renuxa:renuxa@127.0.0.1:5432/renuxa".into());
    let state = AppState {
        db: PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await?,
        jwt_secret: Arc::from(
            env::var("JWT_SECRET").unwrap_or_else(|_| "development-only-change-me".into()),
        ),
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
    };
    loop {
        if let Err(error) = worker::run_cycle(&state).await {
            tracing::error!(%error, "worker cycle failed");
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}
