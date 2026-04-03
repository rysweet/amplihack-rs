//! Multi-source news collector — transforms web-search results into structured articles.
//!
//! Ports Python `amplihack/evaluation/multi_source_collector.py`.

use serde::{Deserialize, Serialize};

use crate::error::EvalError;

/// A structured news article extracted from a web-search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewsArticle {
    pub url: String,
    pub title: String,
    pub content: String,
    pub published: String,
}

impl NewsArticle {
    pub fn new(
        url: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
        published: impl Into<String>,
    ) -> Result<Self, EvalError> {
        let url = url.into();
        let title = title.into();
        let content = content.into();
        let published = published.into();

        if url.is_empty() {
            return Err(EvalError::config("article url must not be empty"));
        }
        if title.is_empty() {
            return Err(EvalError::config("article title must not be empty"));
        }
        if content.is_empty() {
            return Err(EvalError::config("article content must not be empty"));
        }
        if published.is_empty() {
            return Err(EvalError::config("article published date must not be empty"));
        }
        Ok(Self {
            url,
            title,
            content,
            published,
        })
    }
}

/// Required fields in each source entry of the web-search payload.
const REQUIRED_FIELDS: &[&str] = &["url", "title", "content", "published"];

/// Transform a web-search JSON payload into a list of [`NewsArticle`]s.
///
/// The input `value` must contain a `"sources"` array where each element is
/// an object with keys `url`, `title`, `content`, and `published`.
pub fn collect_news(value: &serde_json::Value) -> Result<Vec<NewsArticle>, EvalError> {
    let sources = value
        .get("sources")
        .and_then(|v| v.as_array())
        .ok_or_else(|| EvalError::config("websearch payload must contain a 'sources' array"))?;

    let mut articles = Vec::with_capacity(sources.len());
    for (idx, src) in sources.iter().enumerate() {
        for &field in REQUIRED_FIELDS {
            if src.get(field).and_then(|v| v.as_str()).unwrap_or("").is_empty() {
                return Err(EvalError::config(format!(
                    "source[{idx}] missing required field '{field}'"
                )));
            }
        }

        let article = NewsArticle {
            url: src["url"].as_str().unwrap_or_default().to_string(),
            title: src["title"].as_str().unwrap_or_default().to_string(),
            content: src["content"].as_str().unwrap_or_default().to_string(),
            published: src["published"].as_str().unwrap_or_default().to_string(),
        };
        articles.push(article);
    }

    Ok(articles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collect_news_happy_path() {
        let data = json!({
            "sources": [
                {
                    "url": "https://example.com/1",
                    "title": "Article 1",
                    "content": "Body of article 1",
                    "published": "2024-01-15"
                },
                {
                    "url": "https://example.com/2",
                    "title": "Article 2",
                    "content": "Body of article 2",
                    "published": "2024-01-16"
                }
            ]
        });
        let articles = collect_news(&data).unwrap();
        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].title, "Article 1");
        assert_eq!(articles[1].url, "https://example.com/2");
    }

    #[test]
    fn collect_news_missing_sources_key() {
        let data = json!({"results": []});
        assert!(collect_news(&data).is_err());
    }

    #[test]
    fn collect_news_missing_field() {
        let data = json!({
            "sources": [{"url": "http://a", "title": "", "content": "c", "published": "d"}]
        });
        let err = collect_news(&data).unwrap_err();
        assert!(err.to_string().contains("title"));
    }

    #[test]
    fn collect_news_empty_sources() {
        let data = json!({"sources": []});
        let articles = collect_news(&data).unwrap();
        assert!(articles.is_empty());
    }

    #[test]
    fn news_article_new_validates() {
        assert!(NewsArticle::new("http://a", "t", "c", "d").is_ok());
        assert!(NewsArticle::new("", "t", "c", "d").is_err());
        assert!(NewsArticle::new("http://a", "", "c", "d").is_err());
        assert!(NewsArticle::new("http://a", "t", "", "d").is_err());
        assert!(NewsArticle::new("http://a", "t", "c", "").is_err());
    }

    #[test]
    fn news_article_serde_roundtrip() {
        let article = NewsArticle::new("http://x", "T", "C", "2024-01-01").unwrap();
        let json = serde_json::to_string(&article).unwrap();
        let restored: NewsArticle = serde_json::from_str(&json).unwrap();
        assert_eq!(article, restored);
    }
}
