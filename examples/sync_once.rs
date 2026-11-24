//! Run one sync job one time, then stop.
//!
//! ```sh
//! DATABASE_URL=postgres:///diogenes_dev cargo run --example sync_once -- groups
//! ```
//!
//! Jobs: `deputies`, `groups`, `roll_calls`. The configuration comes from the environment
//! (see `SyncConfig::from_env`).

#![allow(clippy::expect_used)]

use diogenes::camara::CamaraClient;
use diogenes::sync::{run_job, Job, SyncConfig};

#[tokio::main]
async fn main() {
    let name = std::env::args()
        .nth(1)
        .expect("give a job: deputies, groups or roll_calls");
    let job = Job::parse(&name).expect("unknown job");
    let url = std::env::var("DATABASE_URL").expect("set DATABASE_URL");
    let db = sqlx::PgPool::connect(&url).await.expect("connect");
    diogenes::migrate(&db).await.expect("migrate");
    let client = CamaraClient::new().expect("http client");
    match run_job(&db, &client, &SyncConfig::from_env(), job).await {
        Ok(Some(run)) => println!("run {run} finished; see diogenes_sync_run"),
        Ok(None) => println!("another process runs this job now"),
        Err(err) => {
            eprintln!("cannot record the run: {err}");
            std::process::exit(1);
        }
    }
}
