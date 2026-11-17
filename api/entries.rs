use libsql::Builder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use url::Url;
use vercel_runtime::{run, Body, Error, Request, Response, StatusCode};

#[derive(Serialize)]
struct Entry {
    id: i64,
    content: String,
    control_status: String,
    is_completed: bool,
}

#[derive(Deserialize)]
struct CreateEntry {
    content: String,
    control_status: String,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    run(handler).await
}

pub async fn handler(req: Request) -> Result<Response<Body>, Error> {
    let db_url = env::var("TURSO_DATABASE_URL").expect("TURSO_DATABASE_URL belum diatur");
    let auth_token = env::var("TURSO_AUTH_TOKEN").expect("TURSO_AUTH_TOKEN belum diatur");

    let db = Builder::new_remote(db_url, auth_token).build().await?;
    let conn = db.connect()?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            control_status TEXT NOT NULL,
            is_completed BOOLEAN NOT NULL DEFAULT 0
        )",
        (),
    )
    .await?;

    let parsed_url = Url::parse(&req.uri().to_string())
        .unwrap_or_else(|_| Url::parse("http://localhost/api/entries").unwrap());
    let params: HashMap<String, String> = parsed_url.query_pairs().into_owned().collect();

    match req.method().as_str() {
        "GET" => {
            let mut rows = conn
                .query(
                    "SELECT id, content, control_status, is_completed FROM entries ORDER BY id DESC",
                    (),
                )
                .await?;
            let mut entries = Vec::new();
            while let Some(row) = rows.next().await? {
                entries.push(Entry {
                    id: row.get::<i64>(0)?,
                    content: row.get::<String>(1)?,
                    control_status: row.get::<String>(2)?,
                    is_completed: row.get::<i64>(3)? != 0,
                });
            }
            let json = serde_json::to_string(&entries)?;
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "application/json")
                .body(Body::Text(json))?)
        }
        "POST" => {
            let body_str = std::str::from_utf8(req.body())?;
            let payload: CreateEntry = serde_json::from_str(body_str)?;
            conn.execute(
                "INSERT INTO entries (content, control_status) VALUES (?1, ?2)",
                libsql::params![payload.content, payload.control_status],
            )
            .await?;
            Ok(Response::builder()
                .status(StatusCode::CREATED)
                .body(Body::Text("{\"status\":\"created\"}".to_string()))?)
        }
        "PATCH" => {
            if let Some(id_str) = params.get("id") {
                if let Ok(id) = id_str.parse::<i64>() {
                    conn.execute(
                        "UPDATE entries SET is_completed = NOT is_completed WHERE id = ?1",
                        libsql::params![id],
                    )
                    .await?;
                }
            }
            Ok(Response::builder()
                .status(StatusCode::OK)
                .body(Body::Text("{\"status\":\"updated\"}".to_string()))?)
        }
        "DELETE" => {
            if let Some(id_str) = params.get("id") {
                if let Ok(id) = id_str.parse::<i64>() {
                    conn.execute(
                        "DELETE FROM entries WHERE id = ?1",
                        libsql::params![id],
                    )
                    .await?;
                }
            }
            Ok(Response::builder()
                .status(StatusCode::OK)
                .body(Body::Text("{\"status\":\"deleted\"}".to_string()))?)
        }
        _ => Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::Text("Method not allowed".to_string()))?),
    }
}