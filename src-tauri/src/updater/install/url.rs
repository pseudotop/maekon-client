//! Download URL validation and host allowlist enforcement.

use super::super::{UpdateError, Updater};

impl Updater {
    pub(crate) fn validate_metadata_base_url(
        &self,
        base_url: &str,
    ) -> Result<reqwest::Url, UpdateError> {
        let parsed = reqwest::Url::parse(base_url).map_err(|e| {
            UpdateError::Download(format!("Failed to parse update metadata base URL: {}", e))
        })?;

        let Some(host) = parsed.host_str() else {
            return Err(UpdateError::Download(
                "Update metadata base URL host is missing".to_string(),
            ));
        };

        if parsed.scheme() != "https" {
            #[cfg(test)]
            if parsed.scheme() == "http" && matches!(host, "localhost" | "127.0.0.1") {
                // Local test server is allowed for deterministic updater tests.
            } else {
                return Err(UpdateError::Download(format!(
                    "Only HTTPS update metadata base URLs are allowed: {}",
                    parsed
                )));
            }

            #[cfg(not(test))]
            return Err(UpdateError::Download(format!(
                "Only HTTPS update metadata base URLs are allowed: {}",
                parsed
            )));
        }

        if !Self::is_allowed_metadata_host(host) {
            return Err(UpdateError::Download(format!(
                "Disallowed update metadata host: {}",
                host
            )));
        }

        Ok(parsed)
    }

    pub(crate) fn validate_download_url(&self, url: &str) -> Result<reqwest::Url, UpdateError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| UpdateError::Download(format!("Failed to parse download URL: {}", e)))?;

        let Some(host) = parsed.host_str() else {
            return Err(UpdateError::Download(
                "Download URL host is missing".to_string(),
            ));
        };

        if parsed.scheme() != "https" {
            #[cfg(test)]
            if parsed.scheme() == "http" && matches!(host, "localhost" | "127.0.0.1") {
                // Local test server is allowed for deterministic updater tests.
            } else {
                return Err(UpdateError::Download(format!(
                    "Only HTTPS download URLs are allowed: {}",
                    parsed
                )));
            }

            #[cfg(not(test))]
            return Err(UpdateError::Download(format!(
                "Only HTTPS download URLs are allowed: {}",
                parsed
            )));
        }

        if !Self::is_allowed_download_host(host) {
            return Err(UpdateError::Download(format!(
                "Disallowed download host: {}",
                host
            )));
        }

        Ok(parsed)
    }

    pub(crate) fn is_allowed_metadata_host(host: &str) -> bool {
        if host == "api.github.com" {
            return true;
        }

        #[cfg(test)]
        {
            matches!(host, "localhost" | "127.0.0.1")
        }

        #[cfg(not(test))]
        {
            false
        }
    }

    pub(crate) fn is_allowed_download_host(host: &str) -> bool {
        let allowlisted = Self::ALLOWED_DOWNLOAD_HOSTS.iter().any(|allowed_host| {
            host == *allowed_host || host.ends_with(&format!(".{}", allowed_host))
        });
        if allowlisted {
            return true;
        }

        #[cfg(test)]
        {
            matches!(host, "localhost" | "127.0.0.1")
        }

        #[cfg(not(test))]
        {
            false
        }
    }
}
