use crate::context::AppState;
use crate::error::{APIError, APIResult};
use crate::routes::v1::matches::live_url::spectate_match;
use crate::routes::v1::matches::types::MatchIdQuery;
use async_stream::try_stream;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{IntoResponse, Sse};
use bytes::Bytes;
use futures::{Stream, TryStreamExt};
use haste::bitreader::BitReader;
use haste::broadcast::{BroadcastHttp, BroadcastHttpClientError};
use haste::demofile::DEMO_RECORD_BUFFER_SIZE;
use haste::demostream::{DecodeCmdError, DemoStream, ReadCmdError, ReadCmdHeaderError};
use haste::entities::{DeltaHeader, EntityError};
use haste::entityclasses::EntityClasses;
use haste::fielddecoder::FieldDecodeContext;
use haste::flattenedserializers::{FlattenedSerializerContainer, FlattenedSerializersError};
use haste::instancebaseline::INSTANCE_BASELINE_TABLE_NAME;
use haste::parser::{Context, DEFAULT_FULL_PACKET_INTERVAL, DEFAULT_TICK_INTERVAL, ParseError};
use haste::stringtables::StringTableError;
use prost::Message;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use valveprotos::common::{
    CsvcMsgCreateStringTable, CsvcMsgPacketEntities, CsvcMsgServerInfo, CsvcMsgUpdateStringTable,
    CsvcMsgUserCommands, EDemoCommands, SvcMessages,
};
use valveprotos::deadlock::{
    CCitadelUserCmdPb, CCitadelUserMessageDamage, CCitadelUserMessageObjectiveMask,
    CCitadelUserMsgHeroKilled, CMsgBulletImpact, CMsgFireBullets, CitadelUserMessageIds,
    ECitadelGameEvents,
};

async fn demo_stream(
    match_id: u64,
) -> impl Stream<Item = Result<Bytes, BroadcastHttpClientError<reqwest::Error>>> {
    let client = reqwest::Client::new();
    try_stream! {
        let mut demofile = BroadcastHttp::start_streaming(
            client,
            format!("https://dist1-ord1.steamcontent.com/tv/{match_id}"),
        ).await?;
        while let Some(chunk) = demofile.next_packet().await {
            info!("Received chunk");
            yield chunk?;
        }
    }
}

#[utoipa::path(
    get,
    path = "/{match_id}/demo/live",
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
pub(super) async fn live_demo(
    Path(MatchIdQuery { match_id }): Path<MatchIdQuery>,
    State(AppState { steam_client, .. }): State<AppState>,
) -> APIResult<impl IntoResponse> {
    spectate_match(&steam_client, match_id).await?;

    // Wait for the demo to be available
    tryhard::retry_fn(|| steam_client.live_demo_exists(match_id))
        .retries(60)
        .fixed_backoff(Duration::from_millis(500))
        .await?;

    let stream = demo_stream(match_id).await;
    Ok(Body::from_stream(stream))
}

#[derive(thiserror::Error, Debug)]
enum StreamParseError {
    #[error(transparent)]
    Broadcast(#[from] BroadcastHttpClientError<reqwest::Error>),
    #[error(transparent)]
    ReadCmdHeader(#[from] ReadCmdHeaderError),
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    DecodeCmd(#[from] DecodeCmdError),
    #[error(transparent)]
    ReadCmd(#[from] ReadCmdError),
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    #[error(transparent)]
    Protobuf(#[from] prost::DecodeError),
    #[error(transparent)]
    ReadBits(#[from] haste::bitreader::BitReaderError),
    #[error(transparent)]
    Entity(#[from] EntityError),
    #[error(transparent)]
    StringTable(#[from] StringTableError),
    #[error(transparent)]
    Decompress(#[from] snap::Error),
    #[error(transparent)]
    FlattenedSerializers(#[from] FlattenedSerializersError),
    #[error(transparent)]
    SSEEvent(#[from] axum_core::Error),
}

#[derive(serde::Serialize)]
pub(crate) enum DemoEvent {
    UserCmd(CCitadelUserCmdPb),
    FireBullets(CMsgFireBullets),
    BulletImpact(CMsgBulletImpact),
    Damage(CCitadelUserMessageDamage),
    HeroKilled(CCitadelUserMsgHeroKilled),
    ObjectiveMask(CCitadelUserMessageObjectiveMask),
}

#[derive(serde::Serialize)]
struct SSEEvent<E: serde::Serialize> {
    tick: i32,
    event: E,
}

impl<E: serde::Serialize> TryInto<Event> for SSEEvent<E> {
    type Error = StreamParseError;

    fn try_into(self) -> Result<Event, Self::Error> {
        Event::default()
            .json_data(self)
            .map_err(StreamParseError::SSEEvent)
    }
}

async fn demo_event_stream(
    match_id: u64,
) -> Result<impl Stream<Item = Result<Event, StreamParseError>>, StreamParseError> {
    let client = reqwest::Client::new();
    let mut demo_stream = BroadcastHttp::start_streaming(
        client,
        format!("https://dist1-ord1.steamcontent.com/tv/{match_id}"),
    )
    .await?;
    Ok(try_stream! {
        let mut ctx = Context::default();
        let mut buf = vec![0; DEMO_RECORD_BUFFER_SIZE];
        let mut field_decode_ctx = FieldDecodeContext::default();
        loop {
            // Waiting for the next packet
            demo_stream.next_packet().await;
            loop{
            let Ok(cmd_header) = demo_stream.read_cmd_header() else{
                break;
            };

            ctx.prev_tick = ctx.tick;
            ctx.tick = cmd_header.tick;
            info!("Tick: {}, Cmd: {:?}", cmd_header.tick, cmd_header.cmd);
            match cmd_header.cmd{
                EDemoCommands::DemPacket | EDemoCommands::DemSignonPacket => {
                    let cmd_body = demo_stream.read_cmd(&cmd_header)?;
                    let cmd = BroadcastHttp::<reqwest::Client>::decode_cmd_packet(cmd_body)?;
                    let data = cmd.data.unwrap_or_default();
                    let mut br = BitReader::new(&data);

                    while br.num_bits_left() > 8 {
                        let command = br.read_ubitvar()?;
                        let size = br.read_uvarint32()? as usize;

                        let cmd_buf = &mut buf[..size];
                        br.read_bytes(cmd_buf)?;
                        let cmd_buf: &_ = cmd_buf;

                        match command {
                            c if c == SvcMessages::SvcCreateStringTable as u32 => {
                                let msg = CsvcMsgCreateStringTable::decode(cmd_buf)?;
                                let string_table = ctx.string_tables.create_string_table_mut(
                                    msg.name(),
                                    msg.user_data_fixed_size(),
                                    msg.user_data_size(),
                                    msg.user_data_size_bits(),
                                    msg.flags(),
                                    msg.using_varint_bitcounts(),
                                );

                                let string_data = if msg.data_compressed() {
                                    let sd = msg.string_data();
                                    let decompress_len = snap::raw::decompress_len(sd)?;
                                    snap::raw::Decoder::new().decompress(sd, &mut buf)?;
                                    &buf[..decompress_len]
                                } else {
                                    msg.string_data()
                                };

                                let mut br = BitReader::new(string_data);
                                string_table.parse_update(&mut br, msg.num_entries())?;

                                if string_table.name().eq(INSTANCE_BASELINE_TABLE_NAME) {
                                    if let Some(entity_classes) = ctx.entity_classes.as_ref() {
                                        ctx
                                            .instance_baseline
                                            .update(string_table, entity_classes.classes)?;
                                    }
                                }
                            }
                            c if c == SvcMessages::SvcUpdateStringTable as u32 => {
                                let msg = CsvcMsgUpdateStringTable::decode(cmd_buf)?;
                                debug_assert!(msg.table_id.is_some(), "invalid table id");
                                let table_id = msg.table_id() as usize;

                                // table_id was validated by has_table() check above
                                if !ctx.string_tables.has_table(table_id) {
                                    warn!("String table {table_id} not found");
                                    continue;
                                }
                                let Some(string_table) = ctx.string_tables.get_table_mut(table_id) else{
                                    warn!("String table {table_id} not found");
                                    continue;
                                };

                                let mut br = BitReader::new(msg.string_data());
                                string_table.parse_update(&mut br, msg.num_changed_entries())?;

                                if string_table.name().eq(INSTANCE_BASELINE_TABLE_NAME) {
                                    if let Some(entity_classes) = ctx.entity_classes.as_ref() {
                                        ctx
                                            .instance_baseline
                                            .update(string_table, entity_classes.classes)?;
                                    }
                                }

                                warn!("Send string table update");
                            }
                            c if c == SvcMessages::SvcPacketEntities as u32 => {
                                let msg = CsvcMsgPacketEntities::decode(cmd_buf)?;
                                let Some(entity_classes) = ctx.entity_classes.as_ref() else {
                                    warn!("Entity classes not initialized");
                                    continue;
                                };
                                let serializers = ctx
                                    .serializers
                                    .as_ref()
                                    .ok_or(ParseError::SerializersNotInitialized)?;
                                let instance_baseline = &ctx.instance_baseline;

                                let entity_data = msg.entity_data();
                                let mut br = BitReader::new(entity_data);

                                let mut entity_index: i32 = -1;
                                for _ in (0..msg.updated_entries()).rev() {
                                    // TODO(blukai): maybe try to make naming consistent with valve; see
                                    // https://github.com/taylorfinnell/csgo-demoinfo/blob/74960c07c387b080a0965c4fc33d69ccf9bfe6c8/demoinfogo/demofiledump.cpp#L1153C18-L1153C29
                                    // and CL_ParseDeltaHeader in engine/client.cpp
                                    entity_index += br.read_ubitvar()? as i32 + 1;

                                    let delta_header = DeltaHeader::from_bit_reader(&mut br)?;
                                    match delta_header {
                                        DeltaHeader::CREATE => {
                                            // Create the entity and get its index for later retrieval
                                            let created_entity_index = {
                                                ctx.entities.handle_create(
                                                    entity_index,
                                                    &mut field_decode_ctx,
                                                    &mut br,
                                                    entity_classes,
                                                    instance_baseline,
                                                    serializers,
                                                )?;
                                                entity_index
                                            };

                                            // Now safely get the entity we just created
                                            let entity = ctx.entities.get(&created_entity_index).ok_or(
                                                ParseError::EntityNotFoundAfterOperation {
                                                    index: created_entity_index,
                                                },
                                            )?;
                                            warn!("Send entity update");
                                        }
                                        DeltaHeader::DELETE => {
                                            let entity = ctx
                                                .entities
                                                .handle_delete(entity_index)?;
                                            warn!("Send entity update");
                                        }
                                        DeltaHeader::UPDATE => {
                                            // Update the entity and get its index for later retrieval
                                            let updated_entity_index = {
                                                ctx
                                                    .entities
                                                    .handle_update(entity_index, &mut field_decode_ctx, &mut br)?;
                                                entity_index
                                            };

                                            // Now safely get the entity we just updated
                                            let entity = ctx.entities.get(&updated_entity_index).ok_or(
                                                ParseError::EntityNotFoundAfterOperation {
                                                    index: updated_entity_index,
                                                },
                                            )?;
                                            warn!("Send entity update");
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            c if c == SvcMessages::SvcServerInfo as u32 => {
                                let msg = CsvcMsgServerInfo::decode(cmd_buf)?;
                                if let Some(tick_interval) = msg.tick_interval {
                                    ctx.tick_interval = tick_interval;
                                    let ratio = DEFAULT_TICK_INTERVAL / tick_interval;
                                    ctx.full_packet_interval = DEFAULT_FULL_PACKET_INTERVAL * ratio as i32;
                                    field_decode_ctx.tick_interval = tick_interval;
                                }
                            }
                            c if c == CitadelUserMessageIds::KEUserMsgDamage as u32 => {
                                let msg = CCitadelUserMessageDamage::decode(cmd_buf)?;
                                let event = SSEEvent{
                                    tick: ctx.tick,
                                    event: DemoEvent::Damage(msg)
                                };
                                yield Event::default().json_data(event)?;
                            }
                            c if c == CitadelUserMessageIds::KEUserMsgObjectiveMask as u32 => {
                                let msg = CCitadelUserMessageObjectiveMask::decode(cmd_buf)?;
                                    let event = SSEEvent{
                                        tick: ctx.tick,
                                        event: DemoEvent::ObjectiveMask(msg)
                                    };
                                yield Event::default().json_data(event)?;
                            }
                            c if c == CitadelUserMessageIds::KEUserMsgHeroKilled as u32 => {
                                let msg = CCitadelUserMsgHeroKilled::decode(cmd_buf)?;
                                    let event = SSEEvent{
                                        tick: ctx.tick,
                                        event: DemoEvent::HeroKilled(msg)
                                    };
                                yield Event::default().json_data(event)?;
                            }
                            c if c == SvcMessages::SvcUserCmds as u32 => {
                                let msg = CsvcMsgUserCommands::decode(cmd_buf)?;
                                for user_cmd in msg.commands.iter() {
                                    let Some(data) = user_cmd.data.as_ref() else{
                                        continue;
                                    };
                                    let msg = CCitadelUserCmdPb::decode(data.as_slice())?;
                                    let event = SSEEvent{
                                        tick: ctx.tick,
                                        event: DemoEvent::UserCmd(msg)
                                    };
                                    yield Event::default().json_data(event)?;
                                }
                            }
                            c if c == ECitadelGameEvents::GeFireBullets as u32 => {
                                let msg = CMsgFireBullets::decode(cmd_buf)?;
                                    let event = SSEEvent{
                                        tick: ctx.tick,
                                        event: DemoEvent::FireBullets(msg)
                                    };
                                yield Event::default().json_data(event)?;
                            }
                            c if c == ECitadelGameEvents::GeBulletImpact as u32 => {
                                let msg = CMsgBulletImpact::decode(cmd_buf)?;
                                    let event = SSEEvent{
                                        tick: ctx.tick,
                                        event: DemoEvent::BulletImpact(msg)
                                    };
                                yield Event::default().json_data(event)?;
                            }
                            c if c == EDemoCommands::DemSendTables as u32 || c == EDemoCommands::DemStringTables as u32 => {
                                if ctx.serializers.is_some() {
                                    continue;
                                }
                                let Ok(cmd) = BroadcastHttp::<reqwest::Client>::decode_cmd_send_tables(cmd_body) else {
                                    continue;
                                };
                                ctx.serializers = match FlattenedSerializerContainer::parse(cmd) {
                                    Ok(serializers) => Some(serializers),
                                    Err(e) => {
                                        continue;
                                    }
                                }
                            },
                            c if c == EDemoCommands::DemClassInfo as u32 || c == SvcMessages::SvcClassInfo as u32 => {
                        info!("Demo Class Info");
                                let Ok(cmd) = BroadcastHttp::<reqwest::Client>::decode_cmd_class_info(cmd_body) else{
                                        warn!("Failed to decode cmd class info");
                                    continue;
                                    };
                                ctx.entity_classes = Some(EntityClasses::parse(cmd));
                                if let Some(string_table) = ctx.string_tables.find_table(INSTANCE_BASELINE_TABLE_NAME)
                                {
                                    let Ok(entity_classes) = ctx
                                        .entity_classes
                                        .as_ref()
                                        .ok_or(ParseError::EntityClassesNotInitialized) else{
                                        warn!("Entity classes not initialized, skipping instance baseline update");
                                        continue;
                                    };
                                    if let Err(e) = ctx
                                        .instance_baseline
                                        .update(string_table, entity_classes.classes){
                                        error!("Failed to update instance baseline: {e}");
                                };
                                }
                            },
                            msg => {
                                if 340 == msg || 308 == msg {
                                    continue;
                                }
                                if (300..=359).contains(&msg) {
                                    info!("User Message: {msg:?}");
                                    continue;
                                }
                                if (101..=212).contains(&msg) {
                                    continue;
                                }
                                if 400 == msg {
                                    continue;
                                }
                                debug!("Skipping msg: {msg:?}");
                            }
                        }
                    }
                },
                EDemoCommands::DemSendTables | EDemoCommands::DemStringTables => {
                    info!("Send tables");
                    if ctx.serializers.is_some() {
                            warn!("Serializers already initialized, skipping send tables");
                        continue;
                    }

                    let Ok(cmd_body) = demo_stream.read_cmd(&cmd_header) else{
                          warn!("Failed to read cmd");
                          continue;
                        };
                    let Ok(cmd) = BroadcastHttp::<reqwest::Client>::decode_cmd_send_tables(cmd_body) else {
                        warn!("Failed to decode cmd send tables");
                        continue;
                    };
                    ctx.serializers = match FlattenedSerializerContainer::parse(cmd) {
                            Ok(serializers) => Some(serializers),
                            Err(e) => {
                                error!("Failed to parse serializers: {e}");
                                continue;
                            }
                        }
                },
                EDemoCommands::DemClassInfo => {
                        info!("Demo Class Info");
                    let cmd_body = demo_stream.read_cmd(&cmd_header)?;
                    let Ok(cmd) = BroadcastHttp::<reqwest::Client>::decode_cmd_class_info(cmd_body) else{
                            warn!("Failed to decode cmd class info");
                        continue;
                        };
                    ctx.entity_classes = Some(EntityClasses::parse(cmd));
                    if let Some(string_table) = ctx.string_tables.find_table(INSTANCE_BASELINE_TABLE_NAME)
                    {
                        let Ok(entity_classes) = ctx
                            .entity_classes
                            .as_ref()
                            .ok_or(ParseError::EntityClassesNotInitialized) else{
                            warn!("Entity classes not initialized, skipping instance baseline update");
                            continue;
                        };
                        if let Err(e) = ctx
                            .instance_baseline
                            .update(string_table, entity_classes.classes){
                            error!("Failed to update instance baseline: {e}");
                    };
                    }
                },
                header => {
                    debug!("Skipping cmd: {header:?}");
                    if let Err(e) = demo_stream.skip_cmd(&cmd_header){
                        error!("Failed to skip cmd: {e}");
                    }
                },
            }
            }
        }
    })
}

#[utoipa::path(
    get,
    path = "/{match_id}/demo/live/events",
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
pub(super) async fn live_demo_events(
    Path(MatchIdQuery { match_id }): Path<MatchIdQuery>,
    State(AppState { steam_client, .. }): State<AppState>,
) -> APIResult<impl IntoResponse> {
    spectate_match(&steam_client, match_id).await?;

    // Wait for the demo to be available
    tryhard::retry_fn(|| steam_client.live_demo_exists(match_id))
        .retries(60)
        .fixed_backoff(Duration::from_millis(500))
        .await?;

    let stream = demo_event_stream(match_id)
        .await
        .map_err(|e| APIError::internal(e.to_string()))?
        .inspect_err(|e| error!("Error in demo event stream: {e}"));

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
