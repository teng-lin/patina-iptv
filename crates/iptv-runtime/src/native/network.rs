//! The native network port: `iptv-net`'s reqwest client.
use std::time::Duration;

use iptv_import::{BodyStream, Fetch, FetchError, StreamingResponse};
use iptv_net::{NetBody, NetClient, NetConfig};
use iptv_providers::{HttpRequest, HttpResponse};

use crate::network::{Body, Chunks, Client, NetFuture, Network, NetworkConfig, Transport};

/// Makes `iptv-net` clients. Needs a Tokio runtime where the requests run
/// (the owner's network executor or the refresh workers' `block_on`).
pub(crate) struct NativeNetwork;

impl Network for NativeNetwork {
    fn client(&self, config: &NetworkConfig) -> Result<Client, FetchError> {
        Ok(Client::new(
            Native(NetClient::new(adapter_config(config))?),
            config.clone(),
        ))
    }
}

/// The adapter configuration of `config`; every field it does not name
/// keeps `iptv-net`'s default.
pub fn adapter_config(config: &NetworkConfig) -> NetConfig {
    NetConfig {
        user_agent: config.user_agent.clone(),
        request_timeout: config.request_timeout,
        read_timeout: config.read_timeout,
        max_redirects: config.max_redirects,
        max_streamed_bytes: config.max_streamed_bytes,
        max_buffered_bytes: config.max_buffered_bytes,
        allow_files: config.allow_files,
        #[cfg(debug_assertions)]
        allow_private_fixture: config.allow_private_fixture,
        ..NetConfig::default()
    }
}

struct Native(NetClient);

impl Transport for Native {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&self, duration: Duration) -> NetFuture<'_, ()> {
        Box::pin(self.0.sleep(duration))
    }

    fn fetch(&self, request: HttpRequest) -> NetFuture<'_, Result<HttpResponse, FetchError>> {
        Box::pin(self.0.fetch(request))
    }

    fn fetch_stream(
        &self,
        request: HttpRequest,
    ) -> NetFuture<'_, Result<StreamingResponse<Body>, FetchError>> {
        Box::pin(async move {
            let response = self.0.fetch_stream(request).await?;
            Ok(StreamingResponse {
                status: response.status,
                same_request_origin: response.same_request_origin,
                content_length: response.content_length,
                body: Body::new(NativeBody(response.body)),
            })
        })
    }
}

struct NativeBody(NetBody);

impl Chunks for NativeBody {
    fn next_chunk(&mut self) -> NetFuture<'_, Result<Option<Vec<u8>>, FetchError>> {
        Box::pin(self.0.next_chunk())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_portable_defaults_are_the_adapter_defaults() {
        assert_eq!(
            super::adapter_config(&super::NetworkConfig::default()),
            super::NetConfig::default()
        );
    }
}
