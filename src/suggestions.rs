use crate::storage::Settings;
use serde::Serialize;
use std::collections::HashSet;

#[derive(Clone, Serialize)]
pub struct Suggestion {
    pub title: String,
    pub url: String,
    pub kind: &'static str,
}

/// Search local metadata only. Passwords, usernames and form values never enter
/// the address-bar index. Private queries exclude regular browsing history.
pub fn search(
    settings: &Settings,
    sites: &[String],
    query: &str,
    private: bool,
) -> Vec<Suggestion> {
    let query = query.trim().to_lowercase();
    if query.chars().count() < 2 || query.len() > 512 {
        return Vec::new();
    }
    let terms: Vec<_> = query.split_whitespace().collect();
    let mut candidates = Vec::new();
    let mut add = |title: &str, input: &str, kind: &'static str, priority: i32, recent: usize| {
        let Ok(url) = url::Url::parse(input) else {
            return;
        };
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return;
        }
        let host = url.host_str().unwrap().to_lowercase();
        let title = if title.trim().is_empty() {
            &host
        } else {
            title
        };
        let searchable = format!("{} {}", title.to_lowercase(), url.as_str().to_lowercase());
        if !terms.iter().all(|term| searchable.contains(term)) {
            return;
        }
        let score = priority
            + if host == query {
                200
            } else if host.starts_with(&query) {
                100
            } else if host.contains(&query) {
                60
            } else {
                0
            };
        candidates.push((
            score,
            recent,
            Suggestion {
                title: title.to_owned(),
                url: url.to_string(),
                kind,
            },
        ));
    };
    for entry in &settings.bookmarks {
        add(&entry.title, &entry.url, "bookmark", 600, 0);
    }
    for site in sites {
        if let Ok(url) = url::Url::parse(site) {
            if let Some(host) = url.host_str() {
                add(host, site, "login", 550, 0);
            }
        }
    }
    if !private {
        for (recent, entry) in settings.history.iter().enumerate() {
            add(&entry.title, &entry.url, "history", 0, recent);
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|(_, _, item)| seen.insert(item.url.clone()).then_some(item))
        .take(8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Entry;
    fn entry(title: &str, url: &str) -> Entry {
        Entry {
            title: title.into(),
            url: url.into(),
            ..Entry::default()
        }
    }
    #[test]
    fn searches_imported_history_beyond_the_recent_panel() {
        let mut settings = Settings::default();
        settings
            .history
            .push(entry("Zoho old visit", "https://workplace.zoho.com/"));
        for i in 0..400 {
            settings
                .history
                .push(entry("Other page", &format!("https://example.org/{i}")));
        }
        assert_eq!(
            search(&settings, &[], "zoho", false)[0].url,
            "https://workplace.zoho.com/"
        );
    }
    #[test]
    fn private_queries_keep_saved_sites_but_exclude_regular_history() {
        let mut settings = Settings::default();
        settings
            .bookmarks
            .push(entry("Zoho bookmark", "https://www.zoho.com/"));
        settings.history.push(entry(
            "Zoho confidential visit",
            "https://example.org/zoho-history",
        ));
        let results = search(
            &settings,
            &["https://accounts.zoho.com".into()],
            "zoho",
            true,
        );
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.kind != "history"));
    }
    #[test]
    fn deduplicates_websites_and_rejects_nonweb_or_credential_urls() {
        let mut settings = Settings::default();
        settings
            .bookmarks
            .push(entry("Zoho", "https://accounts.zoho.com/"));
        settings
            .history
            .push(entry("Zoho", "https://accounts.zoho.com/"));
        let results = search(
            &settings,
            &[
                "https://accounts.zoho.com".into(),
                "android://zoho@app.test/".into(),
                "https://user:secret@zoho.example/".into(),
            ],
            "zoho",
            false,
        );
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, "bookmark");
    }
    #[test]
    fn ranking_is_bounded_and_saved_sites_precede_history() {
        let mut settings = Settings::default();
        for i in 0..20 {
            settings
                .history
                .push(entry("Zoho", &format!("https://example.org/zoho/{i}")));
        }
        let results = search(
            &settings,
            &["https://accounts.zoho.com".into()],
            "zoho",
            false,
        );
        assert_eq!(results.len(), 8);
        assert_eq!(results[0].kind, "login");
        let json = serde_json::to_value(&results[0]).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 3);
        assert!(json.get("password").is_none() && json.get("username").is_none());
        assert!(search(&settings, &[], " ", false).is_empty());
    }
}
