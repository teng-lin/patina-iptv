//! Series rows vary across Ministra versions. Preserve provider coordinates separately
//! from presentation numbering; a row without a command is never itself playable.
use crate::{HttpResponse, ProviderError};
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeriesRow {
    pub id: String,
    pub title: String,
    pub video_id: Option<String>,
    pub episode: bool,
    pub cmd: Option<String>,
    pub season: bool,
    pub number: u32,
    pub episodes: Vec<u32>,
    pub plot: Option<String>,
    pub duration_secs: Option<u32>,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|v| match v {
            Value::String(s) => Some(s.trim().to_owned()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
}

fn number(v: &Value, key: &str) -> Option<u32> {
    text(v, key)?.parse().ok()
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).is_some_and(|v| v == true || v == 1 || v == "1")
}

pub fn parse_series_rows(response: &HttpResponse) -> Result<Vec<SeriesRow>, ProviderError> {
    let body = crate::http::json_body(response)?;
    let js = body.get("js").unwrap_or(&body);
    let values = js.get("data").unwrap_or(js);
    let rows = match values {
        Value::Array(rows) => rows,
        _ => {
            return Err(ProviderError::InvalidBody(
                "Series list is not an array".into(),
            ));
        }
    };
    let mut out = Vec::new();
    for v in rows.iter().take(20_001) {
        let Some(id) = text(v, "id") else { continue };
        let embedded = v.get("series").is_some_and(Value::is_array);
        let episode = flag(v, "is_episode")
            || number(v, "episode_num").is_some()
            || number(v, "series_number").is_some();
        let season =
            flag(v, "is_season") || (!episode && !embedded && v.get("season_number").is_some());
        let ordinal = number(
            v,
            if season || embedded {
                "season_number"
            } else {
                "episode_num"
            },
        )
        .or_else(|| number(v, "series_number"))
        .or_else(|| number(v, "number"))
        .or_else(|| {
            embedded.then(|| text(v, "name")).flatten().and_then(|s| {
                s.split(|c: char| !c.is_ascii_digit())
                    .rfind(|s| !s.is_empty())
                    .and_then(|n| n.parse().ok())
            })
        })
        .unwrap_or(1);
        let episodes = v
            .get("series")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| match v {
                        Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
                        Value::String(s) => s.parse().ok(),
                        _ => None,
                    })
                    .filter(|n| *n > 0)
                    .take(20_000)
                    .collect()
            })
            .unwrap_or_default();
        out.push(SeriesRow {
            id,
            title: text(v, "name")
                .or_else(|| text(v, "title"))
                .unwrap_or_else(|| ordinal.to_string()),
            cmd: text(v, "cmd"),
            video_id: text(v, "video_id"),
            episode,
            season,
            number: ordinal,
            episodes,
            plot: text(v, "description"),
            duration_secs: number(v, "duration_secs")
                .or_else(|| number(v, "duration"))
                .or_else(|| number(v, "time").map(|m| m.saturating_mul(60))),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_provider_coordinates_and_embedded_episode_numbers() {
        let rows = parse_series_rows(&HttpResponse::ok(
            r#"{
  "js": {
    "data": [
      {
        "id": "season-key",
        "is_season": "1",
        "season_number": 3
      },
      {
        "id": "file-key",
        "cmd": "ffmpeg /media/7",
        "series": [
          2,
          "4",
          0,
          "invalid"
        ]
      }
    ]
  }
}"#,
        ))
        .unwrap();
        assert!(rows[0].season);
        assert_eq!(rows[0].number, 3);
        assert_eq!(rows[1].episodes, vec![2, 4]);
        assert_eq!(rows[1].id, "file-key");
    }
    #[test]
    fn rejects_malformed_payload_instead_of_reporting_empty_series() {
        assert!(parse_series_rows(&HttpResponse::ok(r#"{"js":{"error":"missing"}}"#)).is_err());
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    #[test]
    fn embedded_catalog_numbers_survive_normalization() {
        let parsed=crate::stalker::parse_vod_page(&HttpResponse::ok(r#"{"js":{"data":[{"id":"7","name":"Generated serial","cmd":"/media/generated","series":[1,"3",0]}]}}"#)).unwrap();
        let item = &parsed.items.items[0];
        assert!(item.is_series);
        assert_eq!(item.episode_numbers, vec![1, 3]);
    }
    #[test]
    fn native_series_catalog_needs_no_playback_command() {
        let parsed = crate::stalker::parse_series_page(&HttpResponse::ok(
            r#"{"js":{"data":[{"id":"9","name":"Generated native serial","category_id":"3"}]}}"#,
        ))
        .unwrap();
        assert_eq!(parsed.items.items.len(), 1);
        assert_eq!(parsed.items.items[0].provider_id.as_deref(), Some("9"));
    }
}
