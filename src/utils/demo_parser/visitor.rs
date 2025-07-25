use std::collections::HashSet;

use axum::response::sse::Event;
use haste::demostream::CmdHeader;
use haste::entities::{DeltaHeader, Entity};
use haste::parser::{Context, Visitor};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info};
use valveprotos::common::EDemoCommands;

use crate::utils::demo_parser::entity_events::{
    EntityType, EntityUpdateEvent, EntityUpdateEvents, GameRulesProxyEvent,
};
use crate::utils::demo_parser::error::DemoParseError;
use crate::utils::demo_parser::types::{DemoEvent, DemoEventPayload};

pub(crate) struct SendingVisitor {
    sender: UnboundedSender<Event>,
    subscribed_entities: Option<HashSet<EntityType>>,
    game_time: f32,
    tick_interval: f32,
    rules: GameRulesProxyEvent,
}

impl SendingVisitor {
    pub(crate) fn new(
        sender: UnboundedSender<Event>,
        subscribed_entities: Option<impl IntoIterator<Item = EntityType>>,
    ) -> Self {
        Self {
            sender,
            subscribed_entities: subscribed_entities.map(|iter| iter.into_iter().collect()),
            game_time: 0.0,
            tick_interval: 1.0 / 60.0,
            rules: GameRulesProxyEvent::default(),
        }
    }
}

impl Visitor for SendingVisitor {
    type Error = DemoParseError;

    async fn on_entity(
        &mut self,
        ctx: &Context,
        delta_header: DeltaHeader,
        entity: &Entity,
    ) -> Result<(), Self::Error> {
        let Some(entity_type) = EntityType::from_opt(entity) else {
            return Ok(());
        };

        if entity_type == EntityType::GameRulesProxy
            && let Some(rules) =
                GameRulesProxyEvent::from_entity_update(ctx, delta_header.into(), entity)
        {
            info!("Updating game rules");
            self.rules = rules;
        }

        if self
            .subscribed_entities
            .as_ref()
            .is_some_and(|e| !e.contains(&entity_type))
        {
            return Ok(());
        }

        let Some(entity_update) =
            EntityUpdateEvents::from_update(ctx, delta_header.into(), entity_type, entity)
        else {
            return Ok(());
        };

        let demo_event = DemoEvent {
            tick: ctx.tick(),
            game_time: self.game_time,
            event: DemoEventPayload::EntityUpdate {
                delta: delta_header.into(),
                entity_index: entity.index(),
                entity_type,
                entity_update,
            },
        };
        let sse_event = demo_event.try_into()?;
        self.sender.send(sse_event)?;
        Ok(())
    }

    async fn on_cmd(
        &mut self,
        ctx: &Context,
        cmd_header: &CmdHeader,
        _data: &[u8],
    ) -> Result<(), Self::Error> {
        if cmd_header.cmd == EDemoCommands::DemSyncTick {
            debug!("Updating tick interval");
            self.tick_interval = ctx.tick_interval();
        }
        Ok(())
    }

    async fn on_packet(
        &mut self,
        _ctx: &Context,
        _packet_type: u32,
        _data: &[u8],
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn on_tick_end(&mut self, ctx: &Context) -> Result<(), Self::Error> {
        #[allow(clippy::cast_precision_loss)]
        {
            let ticks = ctx.tick() - self.rules.total_paused_ticks.unwrap_or_default();
            let total_time = ticks as f32 * self.tick_interval;
            self.game_time = total_time - self.rules.game_start_time.unwrap_or_default();
        }

        let demo_event = DemoEvent {
            tick: ctx.tick(),
            game_time: self.game_time,
            event: DemoEventPayload::TickEnd,
        };
        self.sender.send(demo_event.try_into()?)?;
        Ok(())
    }
}
