//! One-request transport and explicit uncertainty. Dropping a send future cannot retract submitted work.
#![forbid(unsafe_code)]

use crate::protocol::{ProtocolError, REQUEST_HEADER_SIZE, RESPONSE_HEADER_SIZE, Request, Response};
use std::{future::Future, io};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    NotSent,
    MayHaveRun,
}

#[derive(Debug)]
pub enum TransportFailure {
    Io(io::Error),
    Timeout,
    Busy,
    Missing,
    /// Native evidence proves this helper load left the process.
    Departed,
    ProcessChanged,
    WrongServer,
    Protocol(ProtocolError),
}
#[derive(Debug)]
pub struct TransportError {
    pub delivery: Delivery,
    pub failure: TransportFailure,
}
impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.failure {
            TransportFailure::Io(error) => write!(f, "Communication with the helper failed: {error}"),
            TransportFailure::Timeout => f.write_str("The helper did not respond in time."),
            TransportFailure::Busy => f.write_str("The helper is busy. Wait for the current operation to finish."),
            TransportFailure::Departed => f.write_str("The helper left Last Epoch. Waiting for an offline character."),
            TransportFailure::Missing => f.write_str("The helper is unavailable. Reconnect before continuing."),
            TransportFailure::ProcessChanged => f.write_str("The game process changed. Reconnect before continuing."),
            TransportFailure::WrongServer => f.write_str("The helper connection belongs to another process."),
            TransportFailure::Protocol(error) => write!(f, "The helper returned an invalid response: {error}"),
        }?;
        if self.delivery == Delivery::MayHaveRun {
            f.write_str(" The request may have reached the game; do not automatically retry.")?;
        }
        Ok(())
    }
}
impl std::error::Error for TransportError {}

pub trait Transport {
    /// Immutable target identity, checked by the session before any process access.
    fn target(&self) -> &crate::process::ProcessIdentity;
    /// Owner-only shutdown, including native proof that the module left and its
    /// dispatcher was restored. A protocol ACK alone cannot confirm departure.
    fn unload(&mut self) -> impl Future<Output = Result<(), TransportError>> + Send {
        async {
            Err(TransportError {
                delivery: Delivery::NotSent,
                failure: TransportFailure::Io(io::Error::other("Transport cannot verify helper departure")),
            })
        }
    }
    /// Implementations must bind server identity before sending and never replay a submitted request.
    fn send(&mut self, request: &Request) -> impl Future<Output = Result<Response, TransportError>> + Send;
}

#[derive(Debug)]
pub enum RequestReadError {
    Io(io::Error),
    Protocol(ProtocolError),
}
impl std::fmt::Display for RequestReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "request read: {error}"),
            Self::Protocol(error) => write!(f, "request read: {error}"),
        }
    }
}
impl std::error::Error for RequestReadError {}

/// Reads one declared bounded frame. The caller owns its whole-read deadline,
/// client identity and any admission; no ownership/queue action occurs here.
/// Following stream data is not part of this frame. A server can separately
/// refuse already buffered excess before submission, never undo accepted work.
pub async fn read_request<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Request, RequestReadError> {
    let mut header = [0; REQUEST_HEADER_SIZE];
    stream.read_exact(&mut header).await.map_err(RequestReadError::Io)?;
    // The header is bounded before its announced tail is allocated.
    let header = Request::decode_header(&header).map_err(RequestReadError::Protocol)?;
    let mut body = vec![0; header.body_len()];
    stream.read_exact(&mut body).await.map_err(RequestReadError::Io)?;
    header.decode_body(&body).map_err(RequestReadError::Protocol)
}

/// Generic framed exchange. The first write attempt is the conservative uncertainty boundary;
/// even a write error may follow partial delivery. Header bounds are checked before allocating.
pub async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, request: &Request) -> Result<Response, TransportError> {
    let result = async {
        stream.write_all(&request.encode()).await.map_err(TransportFailure::Io)?;
        let mut header = [0; RESPONSE_HEADER_SIZE];
        stream.read_exact(&mut header).await.map_err(TransportFailure::Io)?;
        let (code, size) = Response::decode_header(&header).map_err(TransportFailure::Protocol)?;
        let mut payload = vec![0; size];
        stream.read_exact(&mut payload).await.map_err(TransportFailure::Io)?;
        Ok(Response { code, payload })
    }
    .await;
    result.map_err(|failure| TransportError {
        delivery: Delivery::MayHaveRun,
        failure,
    })
}
