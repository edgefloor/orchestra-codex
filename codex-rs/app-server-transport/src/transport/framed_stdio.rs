use super::CHANNEL_CAPACITY;
use super::ConnectionOrigin;
use super::TransportEvent;
use super::forward_incoming_message;
use super::next_connection_id;
use super::serialize_outgoing_message;
use crate::outgoing_message::QueuedOutgoingMessage;
use codex_app_server_protocol::InitializeParams;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCRequest;
use std::io::Error;
use std::io::ErrorKind;
use std::io::Result as IoResult;
use tokio::io;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::debug;
use tracing::error;
use tracing::info;

pub const MAX_FRAME_BYTES: usize = 64 * 1024;

pub async fn start_framed_stdio_connection(
    transport_event_tx: mpsc::Sender<TransportEvent>,
    stdio_handles: &mut Vec<JoinHandle<()>>,
    initialize_client_name_tx: oneshot::Sender<String>,
) -> IoResult<()> {
    let connection_id = next_connection_id();
    let (writer_tx, mut writer_rx) = mpsc::channel::<QueuedOutgoingMessage>(CHANNEL_CAPACITY);
    let writer_tx_for_reader = writer_tx.clone();
    transport_event_tx
        .send(TransportEvent::ConnectionOpened {
            connection_id,
            origin: ConnectionOrigin::Stdio,
            writer: writer_tx,
            disconnect_sender: None,
        })
        .await
        .map_err(|_| Error::new(ErrorKind::BrokenPipe, "processor unavailable"))?;

    let transport_event_tx_for_reader = transport_event_tx.clone();
    stdio_handles.push(tokio::spawn(async move {
        let mut stdin = io::stdin();
        let mut initialize_client_name_tx = Some(initialize_client_name_tx);

        loop {
            match read_frame(&mut stdin).await {
                Ok(Some(payload)) => {
                    if let Some(client_name) = framed_initialize_client_name(&payload)
                        && let Some(initialize_client_name_tx) = initialize_client_name_tx.take()
                    {
                        let _ = initialize_client_name_tx.send(client_name);
                    }
                    if !forward_incoming_message(
                        &transport_event_tx_for_reader,
                        &writer_tx_for_reader,
                        connection_id,
                        &payload,
                    )
                    .await
                    {
                        break;
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    error!("failed reading framed stdin: {err}");
                    break;
                }
            }
        }

        let _ = transport_event_tx_for_reader
            .send(TransportEvent::ConnectionClosed { connection_id })
            .await;
        debug!("framed stdin reader finished");
    }));

    stdio_handles.push(tokio::spawn(async move {
        let mut stdout = io::stdout();
        while let Some(queued_message) = writer_rx.recv().await {
            let Some(json) = serialize_outgoing_message(queued_message.message) else {
                continue;
            };
            if let Err(err) = write_frame(&mut stdout, json.as_bytes()).await {
                error!("failed writing framed stdout: {err}");
                break;
            }
            if let Some(write_complete_tx) = queued_message.write_complete_tx {
                let _ = write_complete_tx.send(());
            }
        }
        info!("framed stdout writer exited");
    }));

    Ok(())
}

async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> IoResult<Option<String>> {
    let mut length = [0_u8; 4];
    if reader.read(&mut length[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut length[1..]).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("frame length {length} is outside 1..={MAX_FRAME_BYTES}"),
        ));
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    String::from_utf8(body).map(Some).map_err(|error| {
        Error::new(
            ErrorKind::InvalidData,
            format!("frame is not valid UTF-8: {error}"),
        )
    })
}

async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), payload: &[u8]) -> IoResult<()> {
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "frame length {} is outside 1..={MAX_FRAME_BYTES}",
                payload.len()
            ),
        ));
    }
    writer
        .write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(payload).await?;
    writer.flush().await
}

fn framed_initialize_client_name(payload: &str) -> Option<String> {
    let message = serde_json::from_str::<JSONRPCMessage>(payload).ok()?;
    let JSONRPCMessage::Request(JSONRPCRequest { method, params, .. }) = message else {
        return None;
    };
    if method != "initialize" {
        return None;
    }
    let params = serde_json::from_value::<InitializeParams>(params?).ok()?;
    Some(params.client_info.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_round_trip_preserves_json_bytes() {
        let (mut writer, mut reader) = tokio::io::duplex(MAX_FRAME_BYTES + 4);
        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        write_frame(&mut writer, payload).await.unwrap();
        assert_eq!(
            read_frame(&mut reader).await.unwrap().unwrap().as_bytes(),
            payload
        );
    }

    #[tokio::test]
    async fn rejects_oversized_frames_before_allocating_the_body() {
        let (mut writer, mut reader) = tokio::io::duplex(4);
        writer
            .write_all(&((MAX_FRAME_BYTES as u32) + 1).to_be_bytes())
            .await
            .unwrap();
        let error = read_frame(&mut reader).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn distinguishes_clean_eof_from_truncated_frames() {
        let (writer, mut reader) = tokio::io::duplex(8);
        drop(writer);
        assert!(read_frame(&mut reader).await.unwrap().is_none());

        let (mut writer, mut reader) = tokio::io::duplex(8);
        writer.write_all(&[0, 0]).await.unwrap();
        drop(writer);
        let error = read_frame(&mut reader).await.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn rejects_oversized_outbound_payloads() {
        let (mut writer, _reader) = tokio::io::duplex(8);
        let error = write_frame(&mut writer, &vec![b'x'; MAX_FRAME_BYTES + 1])
            .await
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }
}
