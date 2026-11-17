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

fn json_error(status: StatusCode, msg: &str) -> Result<Response<Body>, Error> {
    let body = serde_json::json!({ "error": msg }).to_string();
    Ok(Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::Text(body))?)
}

pub async fn handler(req: Request) -> Result<Response<Body>, Error> {
    let raw_url = match env::var("TURSO_DATABASE_URL") {
        Ok(val) if !val.trim().is_empty() => val.trim().to_string(),
        _ => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "TURSO_DATABASE_URL belum diatur di Vercel"),
    };

    let auth_token = match env::var("TURSO_AUTH_TOKEN") {
        Ok(val) if !val.trim().is_empty() => val.trim().to_string(),
        _ => return json_error(StatusCode::INTERNAL_SERVER_ERROR, "TURSO_AUTH_TOKEN belum diatur di Vercel"),
    };

    // Otomatis konversi libsql:// ke https:// agar kompatibel dengan HTTP/Hrana client
    let db_url = raw_url.replace("libsql://", "https://");

    let db = match Builder::new_remote(db_url, auth_token).build().await {
        Ok(d) => d,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("Gagal build DB: {}", e)),
    };

    let conn = match db.connect() {
        Ok(c) => c,
        Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("Gagal connect DB: {}", e)),
    };

    if let Err(e) = conn
        .execute(
            "CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                content TEXT NOT NULL,
                control_status TEXT NOT NULL,
                is_completed BOOLEAN NOT NULL DEFAULT 0
            )",
            (),
        )
        .await
    {
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("Gagal buat tabel: {}", e));
    }

    let parsed_url = Url::parse(&req.uri().to_string())
        .unwrap_or_else(|_| Url::parse("http://localhost/api/entries").unwrap());
    let params: HashMap<String, String> = parsed_url.query_pairs().into_owned().collect();

    match req.method().as_str() {
        "GET" => {
            let mut rows = match conn
                .query(
                    "SELECT id, content, control_status, is_completed FROM entries ORDER BY id DESC",
                    (),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => return json_error(StatusCode::INTERNAL_SERVER_ERROR, &format!("Gagal query: {}", e)),
            };

            let mut entries = Vec::new();
            while let Ok(Some(row)) = rows.next().await {
                entries.push(Entry {
                    id: row.get::<i64>(0).unwrap_or(0),
                    content: row.get::<String>(1).unwrap_or_default(),
                    control_status: row.get::<String>(2).unwrap_or_default(),
                    is_completed: row.get::<i64>(3).unwrap_or(0) != 0,
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
                .header("Content-Type", "application/json")
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
                .header("Content-Type", "application/json")
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
                .header("Content-Type", "application/json")
                .body(Body::Text("{\"status\":\"deleted\"}".to_string()))?)
        }
        _ => json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed"),
    }
}