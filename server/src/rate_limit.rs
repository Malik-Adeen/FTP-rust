use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

pub type AuthRateLimiter = DefaultKeyedRateLimiter<IpAddr>;

pub fn new_auth_limiter() -> Arc<AuthRateLimiter> {
    let quota = Quota::per_minute(NonZeroU32::new(5).unwrap());
    Arc::new(RateLimiter::keyed(quota))
}
