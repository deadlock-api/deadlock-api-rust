use core::time::Duration;

use axum::http::HeaderMap;
use chrono::{DateTime, Utc};
use strum::EnumIs;
use tracing::error;

use crate::error::{APIError, APIResult};

#[derive(Debug, Clone, Copy, Eq, PartialEq, EnumIs)]
pub(super) enum QuotaType {
    IP,
    Key,
    Global,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Quota {
    pub(crate) limit: usize,
    pub(crate) period: Duration,
    pub(super) r#type: QuotaType,
}

impl Quota {
    pub(crate) fn ip_limit(limit: usize, period: Duration) -> Self {
        Self {
            limit,
            period,
            r#type: QuotaType::IP,
        }
    }

    pub(crate) fn key_limit(limit: usize, period: Duration) -> Self {
        Self {
            limit,
            period,
            r#type: QuotaType::Key,
        }
    }

    pub(crate) fn global_limit(limit: usize, period: Duration) -> Self {
        Self {
            limit,
            period,
            r#type: QuotaType::Global,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Status {
    pub(crate) quota: Quota,
    pub(crate) requests: usize,
    pub(crate) oldest_request: DateTime<Utc>,
}

impl Status {
    pub(crate) fn remaining(&self) -> usize {
        self.quota.limit.saturating_sub(self.requests)
    }

    fn is_exceeded(&self) -> bool {
        self.remaining() == 0
    }

    fn next_request_in(&self) -> Duration {
        // If the quota is not exceeded, then there is no need to wait
        if !self.is_exceeded() {
            return Duration::from_millis(0);
        }

        // How long it takes until the oldest request is outside the quota period?
        (self.oldest_request + self.quota.period - Utc::now())
            .to_std()
            .unwrap_or_default()
    }

    pub(crate) fn response_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.append("RateLimit-Limit", self.quota.limit.into());
        headers.append("RateLimit-Period", self.quota.period.as_secs().into());
        headers.append("RateLimit-Remaining", self.remaining().into());
        headers.append("RateLimit-Reset", self.next_request_in().as_secs().into());
        headers.append("Retry-After", self.next_request_in().as_secs().into());
        headers
    }

    pub(super) fn raise_if_exceeded(&self) -> APIResult<()> {
        if self.is_exceeded() {
            error!("Rate limit exceeded: {:?}", self);
            return Err(APIError::RateLimitExceeded {
                status: self.clone(),
            });
        }
        Ok(())
    }
}
