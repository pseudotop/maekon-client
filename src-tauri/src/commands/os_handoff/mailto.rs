//! Validation for the `mailto:` side of the OS handoff boundary.

use url::Url;

use super::{Rejection, REFUSED_ENCODED_OCTETS};

/// Second-level domains reserved for documentation and examples (RFC 2606 §3).
const RESERVED_DOMAINS: &[&str] = &["example.com", "example.org", "example.net"];

/// Reserved top-level domains (RFC 2606 §2, RFC 6761).
///
/// `localhost` is deliberately absent because it can deliver to a local MTA.
const RESERVED_TLDS: &[&str] = &["example", "invalid", "test"];

pub(super) fn validate(url: &Url) -> Result<(), Rejection> {
    validate_query(url)?;
    let recipients = recipients(url);
    if recipients.is_empty() {
        return Err(Rejection::MailtoRecipientMissing);
    }
    for recipient in recipients {
        let mut parts = recipient.rsplitn(2, '@');
        let domain = parts.next().unwrap_or_default();
        let local = parts.next().unwrap_or_default();
        if domain.contains('@') || !is_conservative_mailbox(local, domain) {
            return Err(Rejection::MailtoRecipientMalformed);
        }
        if !is_reserved_domain(domain) {
            return Err(Rejection::MailtoDomainNotReserved {
                domain: domain.to_ascii_lowercase(),
            });
        }
    }
    Ok(())
}

/// Split the path-level recipient list before the query header block.
fn recipients(url: &Url) -> Vec<&str> {
    url.path()
        .split(',')
        .map(str::trim)
        .filter(|recipient| !recipient.is_empty())
        .collect()
}

fn is_reserved_domain(domain: &str) -> bool {
    let domain = domain.to_ascii_lowercase();
    if RESERVED_DOMAINS.contains(&domain.as_str()) {
        return true;
    }
    domain
        .rsplit('.')
        .next()
        .is_some_and(|tld| RESERVED_TLDS.contains(&tld))
}

fn is_conservative_mailbox(local: &str, domain: &str) -> bool {
    let valid_local = !local.is_empty()
        && local.len() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains("..")
        && local.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'.' | b'!'
                        | b'#'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'/'
                        | b'='
                        | b'?'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'{'
                        | b'|'
                        | b'}'
                        | b'~'
                )
        });
    let valid_domain = !domain.is_empty()
        && domain.len() <= 253
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && label
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && label
                    .as_bytes()
                    .last()
                    .is_some_and(u8::is_ascii_alphanumeric)
        });
    valid_local && valid_domain
}

/// Accept only one `subject` and one `body`; recipient headers remain forbidden.
fn validate_query(url: &Url) -> Result<(), Rejection> {
    let path = url.path().to_ascii_lowercase();
    if REFUSED_ENCODED_OCTETS
        .iter()
        .any(|octet| path.contains(octet))
    {
        return Err(Rejection::EncodedControlOctet);
    }

    let Some(query) = url.query() else {
        return Ok(());
    };
    let mut subject_seen = false;
    let mut body_seen = false;
    for field in query.split('&') {
        let (key, value) = field.split_once('=').unwrap_or((field, ""));
        if key.eq_ignore_ascii_case("subject") && !subject_seen {
            subject_seen = true;
            let lowered = value.to_ascii_lowercase();
            if REFUSED_ENCODED_OCTETS
                .iter()
                .any(|octet| lowered.contains(octet))
            {
                return Err(Rejection::EncodedControlOctet);
            }
        } else if key.eq_ignore_ascii_case("body") && !body_seen {
            body_seen = true;
            if value.to_ascii_lowercase().contains("%00") {
                return Err(Rejection::EncodedControlOctet);
            }
        } else {
            return Err(Rejection::MailtoHeaderNotAllowed);
        }
    }
    Ok(())
}
