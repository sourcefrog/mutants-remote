//! Watch a CloudWatch Logs stream.

use aws_config::SdkConfig;
use aws_sdk_cloudwatchlogs::{
    primitives::event_stream::EventReceiver,
    types::{StartLiveTailResponseStream, error::StartLiveTailResponseStreamError},
};
use tracing::{error, info, warn};

#[derive(Debug)]
pub(crate) struct LogTail {
    // logs_client: aws_sdk_cloudwatchlogs::Client,
    // log_group_name: String,
    pub log_stream_name: String,
    response_stream: EventReceiver<StartLiveTailResponseStream, StartLiveTailResponseStreamError>,
}

impl LogTail {
    pub(crate) async fn new(
        sdk_config: &SdkConfig,
        log_group_arn: &str,
        log_stream_name: &str,
    ) -> Self {
        let logs_client = aws_sdk_cloudwatchlogs::Client::new(sdk_config);
        // TODO: Might need to renew the tail after it times out, after 3h.
        let live_tail = logs_client
            .start_live_tail()
            .log_group_identifiers(log_group_arn.to_owned()) // TODO: Maybe should be the ARN?
            .log_stream_names(log_stream_name.to_owned())
            .send()
            .await
            .unwrap();
        let response_stream = live_tail.response_stream;

        LogTail {
            // logs_client,
            // log_group_name: log_group_name.to_string(),
            log_stream_name: log_stream_name.to_string(),
            response_stream,
        }
    }

    /// Get one page of log events.
    // TODO: Maybe return message times too?
    pub(crate) async fn more_log_events(&mut self) -> Option<Vec<String>> {
        // info!("Fetching log events from CloudWatch Logs");
        loop {
            match self.response_stream.recv().await {
                Ok(None) => {
                    info!("Log stream ended");
                    return None;
                }
                Ok(Some(StartLiveTailResponseStream::SessionStart(start))) => {
                    info!(?start, "Starting live tailing");
                    continue;
                }
                Ok(Some(StartLiveTailResponseStream::SessionUpdate(update))) => {
                    if update.session_metadata().is_some_and(|m| m.sampled) {
                        warn!("Logs were sampled");
                    }
                    return Some(
                        update
                            .session_results()
                            .iter()
                            .filter_map(|e| e.message.clone())
                            .collect(),
                    );
                }
                Ok(Some(_unknown)) => {
                    warn!("Unknown live tailing response");
                    continue;
                }
                Err(err) => {
                    error!("Error fetching log events: {}", err);
                    continue;
                }
            }
        }
    }
}
