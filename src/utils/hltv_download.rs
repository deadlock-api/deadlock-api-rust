use bytes::Bytes;
use haste::broadcast::BroadcastFile;
use haste::demostream::DemoStream;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};
use std::io::Cursor;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::time::sleep;
use tracing::{error, trace};
use valveprotos::common::EDemoCommands;

const HLTV_PREFIX_URL: &str = "https://dist1-ord1.steamcontent.com/tv";

#[derive(Deserialize, Serialize)]
pub struct HltvFragment {
    pub match_id: u64,
    pub fragment_n: u64,
    pub fragment_contents: Bytes,
    pub fragment_type: FragmentType,
    pub is_confirmed_last_fragment: bool,
}

impl Debug for HltvFragment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HltvFragment")
            .field("match_id", &self.match_id)
            .field("fragment_n", &self.fragment_n)
            .field("fragment_type", &self.fragment_type)
            .finish()
    }
}

#[allow(unused)]
#[derive(Debug)]
pub enum HltvDownloadError {
    NetworkError(reqwest::Error),
    SyncNotAvailable(Option<reqwest::Error>),
    FragmentNotFound,
    TemporaryError,
    UnexpectedStatusCode(StatusCode),
    ReceiverDropped,
}

impl Display for HltvDownloadError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            HltvDownloadError::NetworkError(e) => write!(f, "Network error: {}", e),
            HltvDownloadError::SyncNotAvailable(Some(e)) => write!(f, "Sync not available: {}", e),
            HltvDownloadError::SyncNotAvailable(None) => write!(f, "Sync not available"),
            HltvDownloadError::FragmentNotFound => write!(f, "Fragment not found"),
            HltvDownloadError::TemporaryError => write!(f, "Temporary error"),
            HltvDownloadError::UnexpectedStatusCode(code) => {
                write!(f, "Unexpected status code: {}", code)
            }
            HltvDownloadError::ReceiverDropped => write!(f, "Receiver dropped"),
        }
    }
}

impl std::error::Error for HltvDownloadError {}

#[derive(Deserialize, Serialize)]
struct HltvSyncResponse {
    tick: u64,
    endtick: u64,
    maxtick: u64,
    rtdelay: f64,
    rcvage: f64,
    fragment: u64,
    signup_fragment: u64,
    tps: u64,
    keyframe_interval: u64,
    map: String,
    protocol: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub enum FragmentType {
    Full = 0,
    Delta = 1,
}

impl FragmentType {
    fn as_str(&self) -> &'static str {
        match self {
            FragmentType::Full => "full",
            FragmentType::Delta => "delta",
        }
    }
}

/// Downloads a match id, starting at the first `/full` fragment that the first `/sync` call says,
/// then /delta's afterwards.
///
/// The stream ends once either:
/// 1. A fragment with the `end` command chunk is identified
/// 2. `/sync` continues to 404 after at least 5 seconds of retries.
///
/// 404s of `/sync` are retried for at least 5 seconds. At the start of the download, they instead
/// have a 30s leniency to have a chance to startup.
///
/// 404s of fragments (`/<fragment>/full` and `/<fragment>/delta`) are continuously retried for as
/// long as the `/sync` endpoint stays alive.
///
/// General logic:
///
/// 1. Get `/sync` once with the leniency period. If the leniency period ends without a valid
///    `/sync`, return Err. Otherwise, return the receiver.
/// 2. Get the `/<fragment>/full` fragment at whatever starting fragment `/sync` responded with.
/// 3. Get the `/<fragment>/delta` fragment counting up from the first fragment. The first fragment
///    number should be called for *both* `/full` and `/delta`. Retry fragment
///    requests when they 404 with 1s in between calls.
/// 4. Stop once `check_fragment_has_end_command` returns true for a given fragment, or once `/sync` is confirmed to be gone as specified above.
/// 5. `is_confirmed_last_fragment` is only set if `check_fragment_has_end_command` was actually confirmed.
///
/// Fragment contents are the entire HTTP Get body of them.
///
/// Here are some sample valid urls of the /sync and fragments:
/// https://dist1-ord1.steamcontent.com/tv/17915135/sync
/// https://dist1-ord1.steamcontent.com/tv/17915135/48/full
/// https://dist1-ord1.steamcontent.com/tv/17915135/48/delta
/// https://dist1-ord1.steamcontent.com/tv/17915135/49/delta
/// ...
pub async fn download_match_mpsc(
    match_id: u64,
) -> Result<Receiver<HltvFragment>, HltvDownloadError> {
    let (sender, receiver) = channel::<HltvFragment>(100);

    let sync_url = format!("{}/{}/sync", HLTV_PREFIX_URL, match_id);
    let http_client = reqwest::Client::new();
    let sync_response: HltvSyncResponse = get_initial_sync(&http_client, &sync_url).await?;

    let fragment_start = sync_response.fragment;

    let sync_url_clone = sync_url.clone();
    let sender_clone = sender.clone();

    tokio::spawn(async move {
        if let Err(e) = fragment_fetching_loop(
            &http_client,
            HLTV_PREFIX_URL.to_string(),
            match_id,
            fragment_start,
            sender_clone,
            sync_url_clone,
        )
        .await
        {
            error!("Error in fragment fetching loop: {:?}", e);
        }
    });

    Ok(receiver)
}

/// Helper function to get the initial `/sync` with a 30s leniency period.
async fn get_initial_sync(
    client: &reqwest::Client,
    sync_url: &str,
) -> Result<HltvSyncResponse, HltvDownloadError> {
    let start_time = Instant::now();

    loop {
        match client.get(sync_url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    let sync_response = resp
                        .json::<HltvSyncResponse>()
                        .await
                        .map_err(HltvDownloadError::NetworkError)?;
                    trace!("Got successful /sync response {sync_url}");
                    return Ok(sync_response);
                } else if resp.status() == StatusCode::NOT_FOUND
                    || resp.status() == StatusCode::METHOD_NOT_ALLOWED
                {
                    if Instant::now() - start_time >= Duration::from_secs(120) {
                        return Err(HltvDownloadError::SyncNotAvailable(
                            resp.error_for_status().err(),
                        ));
                    }
                    sleep(Duration::from_secs(10)).await;
                    continue;
                } else {
                    return Err(HltvDownloadError::UnexpectedStatusCode(resp.status()));
                }
            }
            Err(_) => {
                if Instant::now() - start_time >= Duration::from_secs(30) {
                    return Err(HltvDownloadError::SyncNotAvailable(None));
                }
                sleep(Duration::from_secs(1)).await;
                continue;
            }
        }
    }
}

/// Main loop to fetch fragments and send them via the channel.
async fn fragment_fetching_loop(
    http_client: &reqwest::Client,
    prefix_url: String,
    match_id: u64,
    first_fragment_n: u64,
    sender: Sender<HltvFragment>,
    sync_url: String,
) -> Result<(), HltvDownloadError> {
    let mut sync_available = true;

    let mut fragment_n = first_fragment_n;

    let mut hard_retry = false;
    while sync_available {
        if hard_retry {
            let sync_response: HltvSyncResponse = get_initial_sync(http_client, &sync_url).await?;
            if sync_response.fragment > fragment_n {
                fragment_n = sync_response.fragment;
            }
        } else {
            // Check if /sync is still available
            sync_available = check_sync_availability(http_client, &sync_url).await;
            if !sync_available {
                break;
            }
        }

        let is_first_fragment = fragment_n == first_fragment_n;

        let fragment_types = if is_first_fragment {
            vec![FragmentType::Full, FragmentType::Delta]
        } else {
            vec![FragmentType::Delta]
        };

        for fragment_type in fragment_types {
            let mut retry_count = 0;
            loop {
                match download_match_fragment(
                    http_client,
                    prefix_url.clone(),
                    match_id,
                    fragment_n,
                    fragment_type,
                )
                .await
                {
                    Ok(fragment_contents) => {
                        let is_confirmed_last_fragment =
                            check_fragment_has_end_command(&fragment_contents);

                        let hltv_fragment = HltvFragment {
                            match_id,
                            fragment_n,
                            fragment_contents,
                            fragment_type,
                            is_confirmed_last_fragment,
                        };

                        sender
                            .send(hltv_fragment)
                            .await
                            .map_err(|_| HltvDownloadError::ReceiverDropped)?;

                        if is_confirmed_last_fragment {
                            return Ok(());
                        }

                        break;
                    }
                    Err(e) => match e {
                        HltvDownloadError::FragmentNotFound => {
                            // warn!("[{match_id} {fragment_n}] Got 404");
                            retry_count += 1;

                            // minimum 4 sec wait time
                            sleep(Duration::from_secs((2 * retry_count).max(4))).await;

                            if retry_count > 1 {
                                trace!("Retry #{retry_count} - checking sync availability...");
                                // Check if /sync is still available
                                sync_available =
                                    check_sync_availability(http_client, &sync_url).await;
                                if !sync_available {
                                    break;
                                } else if retry_count > 5 {
                                    error!("[{match_id} {fragment_n}] still 404 after 5 retries");
                                    hard_retry = true;
                                    break;
                                }
                            }
                            continue;
                        }
                        HltvDownloadError::NetworkError(e) => {
                            error!("[{match_id} {fragment_n}] Network error: {e:?}");
                            sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                        _ => {
                            return Err(e);
                        }
                    },
                }
            }

            if !sync_available {
                break;
            }
        }

        fragment_n += 1;
    }

    Ok(())
}

/// Checks if `/sync` is still available with a 5s retry period.
async fn check_sync_availability(http_client: &reqwest::Client, sync_url: &str) -> bool {
    let start_time = Instant::now();

    loop {
        match http_client.get(sync_url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    return true;
                } else if resp.status() == StatusCode::NOT_FOUND {
                    if Instant::now() - start_time >= Duration::from_secs(20) {
                        return false;
                    }
                    sleep(Duration::from_secs(2)).await;
                    continue;
                } else if resp.status() == StatusCode::METHOD_NOT_ALLOWED {
                    if Instant::now() - start_time >= Duration::from_secs(45) {
                        return false;
                    }
                    sleep(Duration::from_secs(20)).await;
                    continue;
                } else {
                    return false;
                }
            }
            Err(_) => {
                if Instant::now() - start_time >= Duration::from_secs(5) {
                    return false;
                }
                sleep(Duration::from_secs(2)).await;
                continue;
            }
        }
    }
}

/// Download a specific fragment from a match
///
/// Returns an error in case of a 404.
pub async fn download_match_fragment(
    http_client: &reqwest::Client,
    prefix_url: String,
    match_id: u64,
    fragment_n: u64,
    fragment_type: FragmentType,
) -> Result<Bytes, HltvDownloadError> {
    let fragment_url = format!(
        "{}/{}/{}/{}",
        prefix_url,
        match_id,
        fragment_n,
        fragment_type.as_str()
    );

    trace!("Downloading match fragment: {fragment_url}");
    let resp = http_client
        .get(&fragment_url)
        .send()
        .await
        .map_err(HltvDownloadError::NetworkError)?;

    if resp.status().is_success() {
        resp.bytes().await.map_err(HltvDownloadError::NetworkError)
    } else if resp.status() == StatusCode::NOT_FOUND {
        Err(HltvDownloadError::FragmentNotFound)
    } else if resp.status() == StatusCode::METHOD_NOT_ALLOWED {
        Err(HltvDownloadError::TemporaryError)
    } else {
        Err(HltvDownloadError::UnexpectedStatusCode(resp.status()))
    }
}

fn check_fragment_has_end_command(fragment_contents: &Bytes) -> bool {
    let cursor = Cursor::new(&fragment_contents[..]);

    let mut demo_file = BroadcastFile::start_reading(cursor);

    let mut count = 0;
    loop {
        match demo_file.read_cmd_header() {
            Ok(cmd_header) => {
                count += 1;
                if cmd_header.cmd == EDemoCommands::DemStop {
                    return true;
                }
                // cmd_header.cmd
                if let Err(e) = demo_file.skip_cmd(&cmd_header) {
                    error!(
                        "Got error skipping cmd body #{}, cmd type {:?}: {:?}",
                        count, cmd_header.cmd, e
                    );
                    return false;
                };
            }
            Err(err) => {
                if demo_file.is_at_eof().unwrap_or_default() {
                    // Tick rate is 60, so a delta file which has count < 60
                    // if count < 60 && fragment_type == FragmentType::Delta {
                    //     return true;
                    // }

                    return false;
                }
                error!("Got error processing fragmemt cmd #{}: {:?}", count, err);
                return false;
            }
        }
    }
}
