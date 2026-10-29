use uuid::Uuid;

pub struct EntityInfo {
    pub name: String,
    pub artist_name: String,
}

/// Look up the name/title and artist of a MusicBrainz entity by type and MBID.
pub async fn lookup_entity(base_url: &str, entity_type: &str, mbid: Uuid) -> Option<EntityInfo> {
    let mb_type = match entity_type {
        "release_group" => "release-group",
        other => other,
    };

    let inc = match entity_type {
        "release_group" => "?inc=artist-credits&fmt=json",
        _ => "?fmt=json",
    };

    let url = format!("{base_url}/ws/2/{mb_type}/{mbid}{inc}");
    let resp = reqwest::get(&url).await.ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;

    let name = json.get("title")
        .or_else(|| json.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    let artist_name = json.get("artist-credit")
        .and_then(|v| v.as_array())
        .map(|credits| {
            credits.iter().map(|ac| {
                let artist_name = ac.get("name").or_else(|| ac.pointer("/artist/name")).and_then(|v| v.as_str()).unwrap_or_default();
                let join = ac.get("joinphrase").and_then(|v| v.as_str()).unwrap_or_default();
                format!("{artist_name}{join}")
            }).collect::<String>()
        })
        .unwrap_or_default();

    Some(EntityInfo { name, artist_name })
}

/// Look up just the name/title of a MusicBrainz entity by type and MBID.
pub async fn lookup_entity_name(base_url: &str, entity_type: &str, mbid: Uuid) -> Option<String> {
    lookup_entity(base_url, entity_type, mbid).await.map(|info| info.name)
}
