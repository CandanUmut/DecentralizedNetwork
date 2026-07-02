// src/messaging.rs

use libp2p::request_response::{
    Behaviour as RequestResponseBehaviour, Config, Event as RequestResponseEvent, Message as RequestResponseMessage,
    ProtocolSupport, Codec,
};
use async_trait::async_trait;
use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::{io, iter};
use libp2p::identity::PeerId;
// messaging.rs
use tokio::sync::mpsc;
use std::sync::Arc;
use tokio::sync::Mutex;

pub type MessageReceiver = Arc<Mutex<mpsc::Receiver<(String, String)>>>;
pub type MessageSender = mpsc::Sender<(String, String)>;

pub struct Messaging {
    pub sender: MessageSender,
    pub receiver: MessageReceiver,
}

impl Messaging {
    /// Initializes a new messaging system with a sender and receiver
    pub fn new(buffer_size: usize) -> Self {
        let (sender, receiver) = mpsc::channel(buffer_size);
        Messaging {
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }
}



/// Define the messaging protocol
#[derive(Debug, Clone)]
pub struct MessagingProtocol;

/// Implement AsRef<str> for MessagingProtocol to satisfy Codec's requirement
impl AsRef<str> for MessagingProtocol {
    fn as_ref(&self) -> &str {
        "/messaging/1.0.0" // Unique protocol identifier
    }
}

/// Codec for the messaging protocol
#[derive(Debug, Clone, Default)]
pub struct MessagingCodec;

#[async_trait]
impl Codec for MessagingCodec {
    type Protocol = MessagingProtocol;
    type Request = Vec<u8>;
    type Response = Vec<u8>;

    async fn read_request<T: AsyncRead + Unpin + Send>(
        &mut self,
        _: &MessagingProtocol,
        io: &mut T,
    ) -> io::Result<Self::Request> {
        let mut buf = Vec::new();
        io.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    async fn read_response<T: AsyncRead + Unpin + Send>(
        &mut self,
        _: &MessagingProtocol,
        io: &mut T,
    ) -> io::Result<Self::Response> {
        let mut buf = Vec::new();
        io.read_to_end(&mut buf).await?;
        Ok(buf)
    }

    async fn write_request<T: AsyncWrite + Unpin + Send>(
        &mut self,
        _: &MessagingProtocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()> {
        io.write_all(&req).await?;
        Ok(())
    }

    async fn write_response<T: AsyncWrite + Unpin + Send>(
        &mut self,
        _: &MessagingProtocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()> {
        io.write_all(&res).await?;
        Ok(())
    }
}

/// Function to create the messaging behaviour
pub fn create_messaging_behavior() -> RequestResponseBehaviour<MessagingCodec> {
    let protocol = (MessagingProtocol, ProtocolSupport::Full);
    let config = Config::default();
    RequestResponseBehaviour::with_codec(
        MessagingCodec::default(),
        iter::once(protocol),
        config,
    )
}

/// Example function to handle messaging events
/// Example function to handle messaging events
pub fn handle_request_response_event(
    behaviour: &mut RequestResponseBehaviour<MessagingCodec>,
    event: RequestResponseEvent<Vec<u8>, Vec<u8>>,
) {
    match event {
        RequestResponseEvent::Message {
            peer,
            message,
            connection_id: _,
        } => match message {
            RequestResponseMessage::Request {
                request,
                channel,
                ..
            } => {
                println!(
                    "Received request from {:?}: {:?}",
                    peer,
                    String::from_utf8_lossy(&request)
                );
                // Example: Respond with a simple acknowledgment
                let response = b"Message received!".to_vec();
                if let Err(err) = behaviour.send_response(channel, response) {
                    eprintln!("Failed to send response: {:?}", err);
                }
            }
            RequestResponseMessage::Response { response, .. } => {
                println!(
                    "Received response: {:?}",
                    String::from_utf8_lossy(&response)
                );
            }
        },
        RequestResponseEvent::OutboundFailure {
            peer,
            request_id,
            error,
            connection_id: _,
        } => {
            eprintln!(
                "Failed to send request {:?} to {:?}: {:?}",
                request_id, peer, error
            );
        }
        RequestResponseEvent::InboundFailure {
            peer,
            request_id,
            error,
            connection_id: _,
        } => {
            eprintln!(
                "Inbound request {:?} from {:?} failed: {:?}",
                request_id, peer, error
            );
        }
        RequestResponseEvent::ResponseSent {
            peer,
            request_id,
            connection_id: _,
        } => {
            println!(
                "Response sent to {:?} for request {:?}",
                peer, request_id
            );
        }
    }
}
