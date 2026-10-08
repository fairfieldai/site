//! Runs on an `EventBridge` Scheduler cron: copies Discord's meetups to each
//! site, emails members, and posts announcements and reminders to Discord.

use std::env;
use std::sync::Arc;

use discord::events::{DiscordEvents, EventSource, Webhook};
use discord::mail::{Cognito, MailApi};
use discord::reminders;
use discord::site::{self, Mail, Site};
use lambda_http::lambda_runtime::{LambdaEvent, run, service_fn};
use lambda_http::{Error, tracing};
use serde::Deserialize;
use shared::events::DynamoEvents;
use shared::ssm::Parameter;
use shared::subscribers::DynamoSubscribers;
use shared::time::parse_rfc3339;

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
    let dynamodb = aws_sdk_dynamodb::Client::new(&config);
    let events = DiscordEvents::new(
        guild_id.clone(),
        Parameter::new(ssm.clone(), env::var("DISCORD_BOT_TOKEN_PARAMETER")?),
    )?;
    let webhook = Webhook::new(Parameter::new(
        ssm.clone(),
        env::var("DISCORD_ANNOUNCEMENTS_WEBHOOK_PARAMETER")?,
    ))?;

    // Every environment's site gets the events; only the one named by
    // EMAIL_TABLE_NAME (prod) emails its members.
    let email_table = env::var("EMAIL_TABLE_NAME").ok();
    let mut sites = Vec::new();
    for table in env::var("TABLE_NAMES")?.split(',') {
        let mail = if email_table.as_deref() == Some(table) {
            let mailer = MailApi::new(
                &env::var("MAIL_API_BASE_URL")?,
                &env::var("MAIL_FROM")?,
                Parameter::new(ssm.clone(), env::var("MAIL_API_KEY_PARAMETER")?),
            )?;
            Some(Mail {
                subscribers: Arc::new(DynamoSubscribers::new(dynamodb.clone(), table.into())),
                directory: Arc::new(Cognito::new(
                    aws_sdk_cognitoidentityprovider::Client::new(&config),
                    env::var("COGNITO_USER_POOL_ID")?,
                )),
                mailer: Arc::new(mailer),
                site_url: env::var("SITE_URL")?.trim_end_matches('/').into(),
            })
        } else {
            None
        };
        sites.push(Site {
            events: Arc::new(DynamoEvents::new(dynamodb.clone(), table.into())),
            mail,
        });
    }
    if email_table.is_some() && !sites.iter().any(|site| site.mail.is_some()) {
        return Err("EMAIL_TABLE_NAME must be one of TABLE_NAMES".into());
    }

    run(service_fn(|event: LambdaEvent<Trigger>| {
        let (events, webhook, guild_id, sites) = (&events, &webhook, &guild_id, &sites);
        async move {
            let run_at = parse_rfc3339(&event.payload.scheduled_time)
                .ok_or_else(|| format!("bad scheduled_time {:?}", event.payload.scheduled_time))?;
            let scheduled = events.scheduled_events().await?;
            let summary = site::run(sites, &scheduled, guild_id, run_at, interval).await;
            tracing::info!(?summary, "sites updated");
            let posted = reminders::post(&scheduled, webhook, guild_id, run_at, interval).await?;
            tracing::info!(posted, "meetup reminders done");
            Ok::<_, Error>(())
        }
    }))
    .await
}
