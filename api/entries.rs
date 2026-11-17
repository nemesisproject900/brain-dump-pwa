use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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

fn json_response(status: StatusCode, body: String) -> Result<Response<Body>, Error> {
    Ok(Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::Text(body))?)
}

async fn turso_exec(
    client: &Client,
    pipeline_url: &str,
    token: &str,
    sql: &str,
    args: Vec<Value>,
) -> Result<Value, String> {
    let payload = json!({
        "requests": [
            {
                "type": "execute",
                "stmt": {
                    "sql": sql,
                    "args": args
                }
            },
            { "type": "close" }
        ]
    });

    let res = client
        .post(pipeline_url)
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Request gagal: {}", e))?;

    let status = res.status();
    let text = res.text().await.map_err(|e| format!("Baca response gagal: {}", e))?;

    if !status.is_success() {
        return Err(format!("Turso HTTP {}: {}", status, text));
    }

    serde_json::from_str(&text).map_err(|e| format!("Parse JSON gagal: {}", e))
}

pub async fn handler(req: Request) -> Result<Response<Body>, Error> {
    let raw_url = match env::var("TURSO_DATABASE_URL") {
        Ok(val) if !val.trim().is_empty() => val.trim().to_string(),
        _ => return json_response(StatusCode::INTERNAL_SERVER_ERROR, r#"{"error":"TURSO_DATABASE_URL belum diatur"}"#.to_string()),
    };

    let token = match env::var("TURSO_AUTH_TOKEN") {
        Ok(val) if !val.trim().is_empty() => val.trim().to_string(),
        _ => return json_response(StatusCode::INTERNAL_SERVER_ERROR, r#"{"error":"TURSO_AUTH_TOKEN belum diatur"}"#.to_string()),
    };

    let base_url = raw_url.replace("libsql://", "https://").trim_end_matches('/').to_string();
    let pipeline_url = format!("{}/v2/pipeline", base_url);
    let client = Client::new();

    if let Err(e) = turso_exec(
        &client,
        &pipeline_url,
        &token,
        "CREATE TABLE IF NOT EXISTS entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            content TEXT NOT NULL,
            control_status TEXT NOT NULL,
            is_completed BOOLEAN NOT NULL DEFAULT 0
        )",
        vec![],
    )
    .await
    {
        return json_response(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e }).to_string());
    }

    let parsed_url = Url::parse(&req.uri().to_string())
        .unwrap_or_else(|_| Url::parse("http://localhost/api/entries").unwrap());
    let params: HashMap<String, String> = parsed_url.query_pairs().into_owned().collect();

    match req.method().as_str() {
        "GET" => {
            let data = match turso_exec(
                &client,
                &pipeline_url,
                &token,
                "SELECT id, content, control_status, is_completed FROM entries ORDER BY id DESC",
                vec![],
            )
            .await
            {
                Ok(v) => v,
                Err(e) => return json_response(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e }).to_string()),
            };

            let mut entries = Vec::new();
            if let Some(rows) = data["results"][0]["response"]["result"]["rows"].as_array() {
                for row in rows {
                    let id = row[0]["value"]
                        .as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .or_else(|| row[0]["value"].as_i64())
                        .unwrap_or(0);
                    let content = row[1]["value"].as_str().unwrap_or_default().to_string();
                    let control_status = row[2]["value"].as_str().unwrap_or_default().to_string();
                    let completed_val = row[3]["value"]
                        .as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .or_else(|| row[3]["value"].as_i64())
                        .unwrap_or(0);

                    entries.push(Entry {
                        id,
                        content,
                        control_status,
                        is_completed: completed_val != 0,
                    });
                }
            }

            json_response(StatusCode::OK, serde_json::to_string(&entries)?)
        }
        "POST" => {
            let body_str = std::str::from_utf8(req.body())?;
            let payload: CreateEntry = serde_json::from_str(body_str)?;
            let args = vec![
                json!({ "type": "text", "value": payload.content }),
                json!({ "type": "text", "value": payload.control_status }),
            ];

            if let Err(e) = turso_exec(
                &client,
                &pipeline_url,
                &token,
                "INSERT INTO entries (content, control_status) VALUES (?, ?)",
                args,
            )
            .await
            {
                return json_response(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e }).to_string());
            }

            json_response(StatusCode::CREATED, r#"{"status":"created"}"#.to_string())
        }
        "PATCH" => {
            if let Some(id_str) = params.get("id") {
                if let Ok(id) = id_str.parse::<i64>() {
                    let args = vec![json!({ "type": "integer", "value": id.to_string() })];
                    let _ = turso_exec(
                        &client,
                        &pipeline_url,
                        &token,
                        "UPDATE entries SET is_completed = NOT is_completed WHERE id = ?",
                        args,
                    )
                    .await;
                }
            }
            json_response(StatusCode::OK, r#"{"status":"updated"}"#.to_string())
        }
        "DELETE" => {
            if let Some(id_str) = params.get("id") {
                if let Ok(id) = id_str.parse::<i64>() {
                    let args = vec![json!({ "type": "integer", "value": id.to_string() })];
                    let _ = turso_exec(
                        &client,
                        &pipeline_url,
                        &token,
                        "DELETE FROM entries WHERE id = ?",
                        args,
                    )
                    .await;
                }
            }
            json_response(StatusCode::OK, r#"{"status":"deleted"}"#.to_string())
        }
        _ => json_response(StatusCode::METHOD_NOT_ALLOWED, r#"{"error":"Method not allowed"}"#.to_string()),
    }
}