//! Posts meetup announcements and reminders on an `EventBridge` Scheduler cron.

use std::env;

use discord::events::{DiscordEvents, Webhook, parse_rfc3339};
use discord::reminders;
use lambda_http::lambda_runtime::{LambdaEvent, run, service_fn};
use lambda_http::{Error, tracing};
use serde::Deserialize;
use shared::ssm::Parameter;

/// The schedule's input: `{"scheduled_time": "<aws.scheduler.scheduled-time>"}`.
#[derive(Deserialize)]
struct Trigger {
    scheduled_time: String,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    let guild_id = env::var("DISCORD_GUILD_ID")?;
    let interval = 60 * env::var("REMINDER_INTERVAL_MINUTES")?.parse::<i64>()?;
    let config = aws_config::load_from_env().await;
    let ssm = aws_sdk_ssm::Client::new(&config);
    let events = DiscordEvents::new(
        guild_id.clone(),
        Parameter::new(ssm.clone(), env::var("DISCORD_BOT_TOKEN_PARAMETER")?),
    )?;
    let webhook = Webhook::new(Parameter::new(
        ssm,
        env::var("DISCORD_ANNOUNCEMENTS_WEBHOOK_PARAMETER")?,
    ))?;

    run(service_fn(|event: LambdaEvent<Trigger>| {
        let (events, webhook, guild_id) = (&events, &webhook, &guild_id);
        async move {
            let run_at = parse_rfc3339(&event.payload.scheduled_time)
                .ok_or_else(|| format!("bad scheduled_time {:?}", event.payload.scheduled_time))?;
            let posted = reminders::run(events, webhook, guild_id, run_at, interval).await?;
            tracing::info!(posted, "meetup reminders done");
            Ok::<_, Error>(())
        }
    }))
    .await
}
