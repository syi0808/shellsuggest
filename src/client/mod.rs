use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::config::Config;
use crate::protocol::{self, DaemonMessage};
use crate::runtime::QueryRuntime;

pub async fn run(config: &Config) -> Result<()> {
    let mut runtime = QueryRuntime::from_config(config)?;
    let stdin = tokio::io::stdin();
    let mut stdin_lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = stdin_lines.next_line().await? {
        let response = match protocol::parse_client_message(&line) {
            Ok(message) => runtime.handle_message(message),
            Err(err) => DaemonMessage::Error {
                message: format!("parse error: {err}"),
                request_id: 0,
            },
        };

        let mut message = protocol::encode_daemon_message(&response);
        message.push('\n');
        stdout.write_all(message.as_bytes()).await?;
        stdout.flush().await?;
    }

    Ok(())
}
