use crate::routes::v1::matches::types::MatchIdQuery;
use crate::utils::hltv_download::download_match_mpsc;
use axum::extract::Path;
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Sse};
use futures::Stream;
use itertools::Itertools;
use tracing::info;

pub async fn stream_hltv_match(match_id: u64) -> impl Stream<Item = serde_json::Result<Event>> {
    #[allow(clippy::unwrap_used)]
    let mut recv = download_match_mpsc(match_id).await.unwrap();

    async_stream::stream! {
        while let Some(fragment) = recv.recv().await {
            info!("Sending Fragment: {:?}", fragment);
            let event = Event::default()
                .data(fragment.fragment_contents.iter().map(|e| e.to_string()).join(","));
            yield Ok(event);
        }
    }
}

#[utoipa::path(
    get,
    path = "/{match_id}/live/demo",
    params(MatchIdQuery),
    responses(
        (status = OK),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = INTERNAL_SERVER_ERROR)
    ),
    tags = ["Matches"],
    summary = "Live Demo",
    description = "Streams the live demo of a match."
)]
pub async fn live_demo(Path(MatchIdQuery { match_id }): Path<MatchIdQuery>) -> impl IntoResponse {
    let stream = stream_hltv_match(match_id).await;
    Sse::new(stream).keep_alive(KeepAlive::default())
}
