use async_trait::async_trait;

use crate::SourceResult;

#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn send(
        &self,
        request: ::http::Request<Vec<u8>>,
    ) -> SourceResult<::http::Response<Vec<u8>>>;
}
