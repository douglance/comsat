use comsat_engine::EngineLimits;

pub const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const CLOUD_MAX_BYTES_PER_SOURCE: usize = 240 * 1024;

#[must_use]
pub fn cloud_limits() -> EngineLimits {
    EngineLimits {
        max_bytes_per_source: CLOUD_MAX_BYTES_PER_SOURCE,
        ..EngineLimits::default()
    }
}
