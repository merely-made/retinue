//! Small static-page adapter for the public `nomadnetwork.node` destination.
//!
//! This module owns only the destination registration and the outer Reticulum
//! request/response envelope. Page bytes remain caller-supplied and opaque;
//! Micron parsing, MIME policy, and filesystem access belong above it.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::vec::Vec;
use core::fmt;
use std::io;
use std::time::Duration;

use crate::destination::DestinationName;
use crate::endpoint::{
    AcceptedResource, Endpoint, PayloadMode, ReceivedRequest, ResourceSession,
    ResourceTransferConfig,
};
use crate::hash::AddressHash;
use crate::identity::Identity;
use crate::request::Request;

/// The public Nomad Network node destination name.
pub const NODE_NAME: &str = "nomadnetwork";
/// The public Nomad Network node destination aspect.
pub const NODE_ASPECT: &str = "node";
/// The default path used by a node's index page.
pub const INDEX_PATH: &[u8] = b"/page/index.mu";

/// Hash a page path using the request envelope's wire-defined truncation.
pub fn page_path_hash(path: &[u8]) -> AddressHash {
    AddressHash::of(path)
}

/// A bounded policy for static pages and their Reticulum transfer.
#[derive(Clone, Copy, Debug)]
pub struct StaticPageConfig {
    /// Maximum body bytes accepted for one page.
    pub max_page_bytes: usize,
    /// Resource retry, request-window, and total-transfer policy.
    pub transfer: ResourceTransferConfig,
}

/// Client policy for one page fetch. `max_page_bytes` is checked after the
/// existing Resource receiver has reassembled and verified the response; it
/// is not a wire-level allocation ceiling.
#[derive(Clone, Copy, Debug)]
pub struct FetchPageConfig {
    pub max_page_bytes: usize,
    pub transfer: ResourceTransferConfig,
}

impl Default for FetchPageConfig {
    fn default() -> Self {
        Self {
            max_page_bytes: StaticPageConfig::default().max_page_bytes,
            transfer: StaticPageConfig::default().transfer,
        }
    }
}

impl Default for StaticPageConfig {
    fn default() -> Self {
        Self {
            max_page_bytes: 4 * 1024 * 1024,
            transfer: ResourceTransferConfig {
                timeout: Duration::from_secs(60),
                ..ResourceTransferConfig::default()
            },
        }
    }
}

/// Result of serving one registered page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServedPage {
    pub path_hash: AddressHash,
    pub body_bytes: usize,
    pub mode: PayloadMode,
}

/// A caller-owned static Nomad Network node.
#[derive(Clone, Debug)]
pub struct StaticNode {
    name: DestinationName,
    pages: BTreeMap<AddressHash, Vec<u8>>,
    config: StaticPageConfig,
    app_data: Vec<u8>,
}

impl Default for StaticNode {
    fn default() -> Self {
        Self::new()
    }
}

impl StaticNode {
    /// Create an empty `nomadnetwork.node` page registry.
    pub fn new() -> Self {
        Self {
            name: DestinationName::new(NODE_NAME, [NODE_ASPECT]),
            pages: BTreeMap::new(),
            config: StaticPageConfig::default(),
            app_data: Vec::new(),
        }
    }

    pub fn config(&self) -> StaticPageConfig {
        self.config
    }

    pub fn set_config(&mut self, config: StaticPageConfig) {
        self.config = config;
    }

    /// Replace the announce application data. It is not interpreted by this adapter.
    pub fn set_app_data(&mut self, app_data: Vec<u8>) {
        self.app_data = app_data;
    }

    pub fn name(&self) -> &DestinationName {
        &self.name
    }

    pub fn destination(&self, identity: &Identity) -> AddressHash {
        self.name.destination_hash(identity)
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Add or replace a page. Paths are opaque bytes with a conservative static
    /// path boundary: absolute, non-empty, and free of NUL bytes.
    pub fn insert_page(&mut self, path: &[u8], body: Vec<u8>) -> Result<(), PageError> {
        validate_path(path)?;
        if body.len() > self.config.max_page_bytes {
            return Err(PageError::BodyTooLarge {
                actual: body.len(),
                maximum: self.config.max_page_bytes,
            });
        }
        self.pages.insert(page_path_hash(path), body);
        Ok(())
    }

    /// Register this destination and announce it on an endpoint.
    pub fn register(&self, endpoint: &Endpoint) {
        endpoint.register_resource(self.name.clone(), &self.app_data);
    }

    /// Serve one inbound request and return its live session to the caller.
    /// Keep the session available until peer closure or a host-selected deadline;
    /// dropping it at the Resource proof can interrupt the peer's response callback.
    pub async fn serve_once(
        &self,
        endpoint: &Endpoint,
    ) -> io::Result<(ResourceSession, ServedPage)> {
        let mut accepted = endpoint.accept_resource().await?;
        let page = self.serve_accepted(endpoint, &mut accepted).await?;
        Ok((accepted.session, page))
    }

    /// Serve one already accepted resource session. The caller owns acceptance
    /// and can therefore apply its own concurrency and lifetime policy.
    pub async fn serve_accepted(
        &self,
        endpoint: &Endpoint,
        accepted: &mut AcceptedResource,
    ) -> io::Result<ServedPage> {
        if accepted.destination != self.destination(endpoint.identity()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unexpected Nomad Network destination",
            ));
        }
        self.serve_request(&mut accepted.session).await
    }

    /// Respond to one request while leaving the link lifetime with its caller.
    /// A page host should keep this session available for subsequent requests or
    /// peer closure; a Resource proof acknowledges bytes, not completion of the
    /// remote application's response callback.
    pub async fn serve_request(&self, session: &mut ResourceSession) -> io::Result<ServedPage> {
        session.set_config(self.config.transfer);
        let received = session.receive_request().await?;
        self.respond_to_request(session, received).await
    }

    /// Answer an already-received request from the caller's current saved snapshot.
    pub async fn respond_to_request(
        &self,
        session: &mut ResourceSession,
        received: ReceivedRequest,
    ) -> io::Result<ServedPage> {
        session.set_config(self.config.transfer);
        let request = received.request;
        if !request.data.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "static page request carries unexpected data",
            ));
        }
        let Some(body) = self.pages.get(&request.path_hash) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "static page not registered",
            ));
        };
        let body_len = body.len();
        let mode = session
            .respond_auto(received.request_id, body.clone())
            .await?;
        Ok(ServedPage {
            path_hash: request.path_hash,
            body_bytes: body_len,
            mode,
        })
    }
}

/// Fetch one static page through the existing request/resource API.
pub async fn fetch_page(
    endpoint: &Endpoint,
    destination: AddressHash,
    peer: Identity,
    path: &[u8],
) -> io::Result<Vec<u8>> {
    fetch_page_with_config(
        endpoint,
        destination,
        peer,
        path,
        FetchPageConfig::default(),
    )
    .await
}

/// Fetch one page with an explicit transfer policy and post-reassembly body cap.
pub async fn fetch_page_with_config(
    endpoint: &Endpoint,
    destination: AddressHash,
    peer: Identity,
    path: &[u8],
    config: FetchPageConfig,
) -> io::Result<Vec<u8>> {
    validate_path(path).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let request = Request::new(path, Vec::new(), 0.0);
    let mut session = endpoint.open_resource(destination, peer).await?;
    session.set_config(config.transfer);
    let data = session.request(&request).await?.data;
    if data.len() > config.max_page_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("page response exceeds {} byte cap", config.max_page_bytes),
        ));
    }
    Ok(data)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageError {
    InvalidPath,
    BodyTooLarge { actual: usize, maximum: usize },
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath => f.write_str("page path must be absolute, non-empty, and NUL-free"),
            Self::BodyTooLarge { actual, maximum } => {
                write!(f, "page body is {actual} bytes, maximum is {maximum}")
            }
        }
    }
}

impl std::error::Error for PageError {}

fn validate_path(path: &[u8]) -> Result<(), PageError> {
    if path.is_empty() || path[0] != b'/' || path.contains(&0) {
        return Err(PageError::InvalidPath);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::PrivateIdentity;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn stock_index_path_hash_is_stable() {
        assert_eq!(
            page_path_hash(INDEX_PATH).to_string(),
            "fb40abf359b3f25fa0086107c5eee516"
        );
    }

    #[test]
    fn registry_keeps_opaque_page_bytes_by_path_hash() {
        let mut node = StaticNode::new();
        node.insert_page(INDEX_PATH, b"Native Retinue page\n".to_vec())
            .unwrap();
        assert_eq!(node.page_count(), 1);
        assert_eq!(
            node.pages.get(&page_path_hash(INDEX_PATH)).unwrap(),
            b"Native Retinue page\n"
        );
    }

    #[test]
    fn registry_rejects_unsafe_or_oversize_pages() {
        let mut node = StaticNode::new();
        assert_eq!(
            node.insert_page(b"page/index.mu", Vec::new()),
            Err(PageError::InvalidPath)
        );
        assert_eq!(
            node.insert_page(b"/page/\0.mu", Vec::new()),
            Err(PageError::InvalidPath)
        );
        node.set_config(StaticPageConfig {
            max_page_bytes: 3,
            ..node.config()
        });
        assert_eq!(
            node.insert_page(b"/page/x.mu", b"four".to_vec()),
            Err(PageError::BodyTooLarge {
                actual: 4,
                maximum: 3
            })
        );
    }

    #[tokio::test]
    async fn loopback_adapter_serves_pages_and_refuses_unregistered_requests() {
        let mut node = StaticNode::new();
        node.insert_page(INDEX_PATH, b"small page\n".to_vec())
            .unwrap();
        node.insert_page(b"/page/large.mu", vec![b'x'; 128 * 1024])
            .unwrap();

        let server = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x66; 64]));
        let destination = node.destination(server.identity());
        let peer = *server.identity();
        let addr = server
            .listen_tcp("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        node.register(&server);
        let server_task = tokio::spawn(async move {
            let mut results = Vec::new();
            for _ in 0..4 {
                results.push(node.serve_once(&server).await);
            }
            results
        });

        let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x67; 64]));
        client.attach_tcp_client(addr).await.unwrap();
        let fetch_config = FetchPageConfig {
            max_page_bytes: 4 * 1024 * 1024,
            transfer: ResourceTransferConfig {
                timeout: Duration::from_secs(5),
                ..ResourceTransferConfig::default()
            },
        };
        let small = fetch_page_with_config(&client, destination, peer, INDEX_PATH, fetch_config)
            .await
            .unwrap();
        assert_eq!(small, b"small page\n");
        let large =
            fetch_page_with_config(&client, destination, peer, b"/page/large.mu", fetch_config)
                .await
                .unwrap();
        assert_eq!(large.len(), 128 * 1024);
        assert!(
            fetch_page_with_config(
                &client,
                destination,
                peer,
                b"/page/missing.mu",
                fetch_config,
            )
            .await
            .is_err()
        );
        let nonempty = Request::new(INDEX_PATH, b"unexpected".to_vec(), 0.0);
        let nonempty_result = tokio::time::timeout(
            Duration::from_secs(5),
            client.request(destination, peer, &nonempty),
        )
        .await;
        assert!(nonempty_result.is_err() || nonempty_result.unwrap().is_err());

        let results = server_task.await.unwrap();
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[2].is_err());
        assert!(results[3].is_err());
        client.close();
    }
}
