use std::path::Path;
use std::process::Command;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub async fn run(socket_path: &Path) -> Result<()> {
    let stream = match UnixStream::connect(socket_path).await {
        Ok(s) => s,
        Err(_) => {
            start_daemon()?;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            UnixStream::connect(socket_path).await?
        }
    };

    let (reader, mut writer) = stream.into_split();
    let mut socket_lines = BufReader::new(reader).lines();
    let stdin = tokio::io::stdin();
    let mut stdin_lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    loop {
        tokio::select! {
            line = stdin_lines.next_line() => {
                match line? {
                    Some(line) => {
                        let mut msg = line;
                        msg.push('\n');
                        writer.write_all(msg.as_bytes()).await?;
                    }
                    None => break,
                }
            }
            line = socket_lines.next_line() => {
                match line? {
                    Some(line) => {
                        let mut msg = line;
                        msg.push('\n');
                        stdout.write_all(msg.as_bytes()).await?;
                        stdout.flush().await?;
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}

fn start_daemon() -> Result<()> {
    let exe = std::env::var_os("SHELLSUGGEST_BIN")
        .map(Into::into)
        .unwrap_or(std::env::current_exe()?);
    Command::new(exe)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}
