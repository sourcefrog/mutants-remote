//! Watch a CloudWatch Logs stream.

use aws_config::SdkConfig;
use aws_sdk_cloudwatchlogs::types::OutputLogEvent;
use tracing::{error, info};

#[derive(Debug, Clone)]
pub(crate) struct LogTail {
    logs_client: aws_sdk_cloudwatchlogs::Client,
    log_group_name: String,
    pub log_stream_name: String,
    next_token: Option<String>,
}

impl LogTail {
    pub(crate) fn new(sdk_config: &SdkConfig, log_group_name: &str, log_stream_name: &str) -> Self {
        let logs_client = aws_sdk_cloudwatchlogs::Client::new(sdk_config);
        LogTail {
            logs_client,
            log_group_name: log_group_name.to_string(),
            log_stream_name: log_stream_name.to_string(),
            next_token: None,
        }
    }

    /// Get one page of log events.
    pub(crate) async fn get_log_events(&mut self) -> Vec<OutputLogEvent> {
        info!("Fetching log events from CloudWatch Logs");
        // TODO: Could use pagination or log tailing
        match self
            .logs_client
            .get_log_events()
            .log_group_name(self.log_group_name.clone())
            .log_stream_name(self.log_stream_name.clone())
            .start_from_head(true)
            .set_next_token(self.next_token.clone())
            .send()
            .await
        {
            Ok(response) => {
                self.next_token = response.next_forward_token;
                response.events.unwrap()
            }
            Err(err) => {
                error!("Error fetching log events: {}", err);
                Vec::new() // TODO: Maybe return an error?
            }
        }
    }
}
