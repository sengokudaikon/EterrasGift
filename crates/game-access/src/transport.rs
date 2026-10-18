//! One-request transport and explicit uncertainty. Dropping a send future cannot retract submitted work.
use crate::protocol::{ProtocolError, RESPONSE_HEADER_SIZE, Request, Response};
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
        write!(f, "helper transport ({:?}): {:?}", self.delivery, self.failure)
    }
}
impl std::error::Error for TransportError {}

pub trait Transport {
    /// Immutable target identity, checked by the session before any process access.
    fn target(&self) -> &crate::process::ProcessIdentity;
    /// Implementations must bind server identity before sending and never replay a submitted request.
    fn send(&mut self, request: &Request) -> impl Future<Output = Result<Response, TransportError>> + Send;
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
