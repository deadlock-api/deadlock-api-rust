use cached::TimedCache;
use cached::proc_macro::cached;
use tracing::debug;

use crate::services::assets::types::{AssetsHero, AssetsRanks};

/// Client for interacting with the Deadlock assets API
#[derive(Clone)]
pub(crate) struct AssetsClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl AssetsClient {
    pub(crate) fn new(base_url: String, http_client: reqwest::Client) -> Self {
        Self {
            base_url,
            http_client,
        }
    }

    /// Fetch heroes from the assets API
    pub(crate) async fn fetch_heroes(&self) -> reqwest::Result<Vec<AssetsHero>> {
        debug!("Fetching heroes from assets API");
        fetch_heroes_cached(&self.http_client, &self.base_url).await
    }

    /// Fetch ranks from the assets API
    pub(crate) async fn fetch_ranks(&self) -> reqwest::Result<Vec<AssetsRanks>> {
        debug!("Fetching ranks from assets API");
        fetch_ranks_cached(&self.http_client, &self.base_url).await
    }

    /// Find a hero ID by name
    pub(crate) async fn fetch_hero_id_from_name(
        &self,
        hero_name: &str,
    ) -> reqwest::Result<Option<u32>> {
        debug!("Finding hero ID for name: {hero_name}");
        self.fetch_heroes().await.map(|heroes| {
            heroes
                .iter()
                .find(|h| h.name.to_lowercase() == hero_name.to_lowercase())
                .map(|h| h.id)
        })
    }

    /// Find a hero name by ID
    pub(crate) async fn fetch_hero_name_from_id(
        &self,
        hero_id: u32,
    ) -> reqwest::Result<Option<String>> {
        debug!("Finding hero name for ID: {hero_id}");
        self.fetch_heroes()
            .await
            .map(|heroes| heroes.into_iter().find(|h| h.id == hero_id).map(|h| h.name))
    }

    /// Validate if a hero ID exists
    pub(crate) async fn validate_hero_id(&self, hero_id: u32) -> bool {
        match self.fetch_heroes().await {
            Ok(heroes) => heroes.iter().any(|h| h.id == hero_id),
            Err(_) => false,
        }
    }
}

// Private cached helper functions
#[cached(
    ty = "TimedCache<u8, Vec<AssetsHero>>",
    create = "{ TimedCache::with_lifespan(std::time::Duration::from_secs(60 * 60)) }",
    result = true,
    convert = "{ 0 }"
)]
async fn fetch_heroes_cached(
    http_client: &reqwest::Client,
    base_url: &str,
) -> reqwest::Result<Vec<AssetsHero>> {
    http_client
        .get(format!("{base_url}/v2/heroes"))
        .send()
        .await?
        .json()
        .await
}

#[cached(
    ty = "TimedCache<u8, Vec<AssetsRanks>>",
    create = "{ TimedCache::with_lifespan(std::time::Duration::from_secs(60 * 60)) }",
    result = true,
    convert = "{ 0 }",
    sync_writes = "default"
)]
async fn fetch_ranks_cached(
    http_client: &reqwest::Client,
    base_url: &str,
) -> reqwest::Result<Vec<AssetsRanks>> {
    http_client
        .get(format!("{base_url}/v2/ranks"))
        .send()
        .await?
        .json()
        .await
}
