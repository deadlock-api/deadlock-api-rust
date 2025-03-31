use crate::routes::v1::matches::types::MatchIdQuery;
use crate::utils::hltv_download::{HltvDownloadError, download_match_mpsc};
use async_stream::try_stream;
use axum::body::{Body, Bytes};
use axum::extract::Path;
use axum::response::IntoResponse;
use futures::Stream;
use tracing::info;

pub async fn stream_hltv_match(
    match_id: u64,
) -> impl Stream<Item = Result<Bytes, HltvDownloadError>> {
    try_stream! {
        let mut recv = download_match_mpsc(match_id).await?;
        while let Some(fragment) = recv.recv().await {
            info!("Sending Fragment: {:?}", fragment);
            yield fragment.fragment_contents;
        }
    }
}

#[utoipa::path(
    get,
    path = "/{match_id}/live/demo",
    params(MatchIdQuery),
    responses(
        (status = OK, body = [u8], description = "Live demo stream."),
        (status = BAD_REQUEST, description = "Provided parameters are invalid."),
        (status = INTERNAL_SERVER_ERROR)
    ),
    tags = ["Matches"],
    summary = "Live Demo",
    description = "Streams the live demo of a match."
)]
pub async fn live_demo(Path(MatchIdQuery { match_id }): Path<MatchIdQuery>) -> impl IntoResponse {
    let stream = stream_hltv_match(match_id).await;
    Body::from_stream(stream)
}
