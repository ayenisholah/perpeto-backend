use std::{env, process::ExitCode};

use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> ExitCode {
    let Ok(database_url) = env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL must be set");
        return ExitCode::FAILURE;
    };
    let pool = match PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
    {
        Ok(pool) => pool,
        Err(error) => {
            eprintln!("failed to connect to the database: {error}");
            return ExitCode::FAILURE;
        }
    };
    match sqlx::migrate!("../../migrations").run(&pool).await {
        Ok(()) => {
            println!("database migrations are up to date");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("migration failed: {error}");
            ExitCode::FAILURE
        }
    }
}
