//! SSRF (Server-Side Request Forgery) protection for user-supplied URLs.
//!
//! Two layers of defense:
//! 1. `validate_url_scheme()` — sync check for scheme + internal blocklist
//! 2. `SsrfSafeResolver` — custom DNS resolver that filters private IPs at
//!    resolution time, eliminating the TOCTOU gap between validation and fetch.
//!
//! Internal service-to-service calls (YAML tools, toolgate) bypass all checks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use anyhow::Result;

/// Internal services blocked from user-supplied URLs (reachable only via service-to-service calls).
const INTERNAL_BLOCKLIST: &[&str] = &[
    "localhost:9011",
    "toolgate:9011",
    "localhost:9020",
    "browser-renderer:9020",
    "localhost:8080",
    "searxng:8080",
    "localhost:18789",
    "localhost:5432",   // PostgreSQL
    "postgres:5432",
    "localhost:2375",   // Docker API (prevent SSRF to Docker socket)
];

/// Check if an IP address is private/internal (RFC 1918, loopback, link-local, etc.).
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()       // 127.0.0.0/8
            || v4.is_private()     // 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
            || v4.is_link_local()  // 169.254.0.0/16
            || v4.is_broadcast()
            || v4 == Ipv4Addr::UNSPECIFIED
            // 100.64.0.0/10 (Carrier-grade NAT)
            || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()   // ::1
            || v6 == Ipv6Addr::UNSPECIFIED
            // IPv4-mapped IPv6 (::ffff:x.x.x.x) — check the embedded IPv4
            || v6.to_ipv4_mapped().is_some_and(|v4| is_private_ip(IpAddr::V4(v4)))
            // RFC 4193 unique local (fc00::/7)
            || (v6.segments()[0] & 0xfe00) == 0xfc00
            // Link-local (fe80::/10)
            || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

// ── SSRF-safe DNS resolver ──────────────────────────────────────────────────

/// Custom DNS resolver that filters out private/internal IP addresses.
///
/// When used with `reqwest::ClientBuilder::dns_resolver()`, this ensures that
/// the actual TCP connection only uses public IPs — eliminating the DNS rebinding
/// TOCTOU gap where a hostname could resolve to a public IP during validation
/// but a private IP during the actual connection.
pub struct SsrfSafeResolver;

impl reqwest::dns::Resolve for SsrfSafeResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = format!("{}:0", name.as_str());
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&host)
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .filter(|a| !is_private_ip(a.ip()))
                .collect();

            if addrs.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("SSRF blocked: '{}' resolves only to private/internal IPs", name.as_str()),
                )) as Box<dyn std::error::Error + Send + Sync>);
            }

            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Check if a URL targets an internal/trusted service (localhost toolgate, searxng, etc.).
/// These endpoints are safe for YAML tool execution and should bypass SSRF filtering.
pub fn is_internal_endpoint(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    let host = parsed.host_str().unwrap_or("");
    let port = parsed.port_or_known_default().unwrap_or(80);
    let authority = format!("{}:{}", host, port);
    INTERNAL_BLOCKLIST.iter().any(|a| *a == authority)
}

/// Build a reqwest client with SSRF-safe DNS resolution.
/// Private IPs are filtered at the resolver level — no TOCTOU gap.
pub fn ssrf_safe_client(timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(std::time::Duration::from_secs(10))
        .dns_resolver(Arc::new(SsrfSafeResolver))
        .build()
        .expect("failed to build SSRF-safe HTTP client")
}

// ── URL validation (sync, no DNS) ───────────────────────────────────────────

/// Validate URL scheme and block internal service hostnames.
///
/// This is a synchronous pre-check — DNS-based private IP blocking is handled
/// by `SsrfSafeResolver` at connection time.
pub fn validate_url_scheme(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| anyhow::anyhow!("invalid URL: {}", e))?;

    // Block non-HTTP schemes
    match parsed.scheme() {
        "http" | "https" => {}
        scheme => anyhow::bail!("blocked scheme: {}", scheme),
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host"))?;

    let port = parsed.port_or_known_default().unwrap_or(80);
    let authority = format!("{}:{}", host, port);

    // Block internal services (these should only be reached via service-to-service calls)
    if INTERNAL_BLOCKLIST.iter().any(|a| *a == authority) {
        anyhow::bail!(
            "blocked: URL targets internal service ({})",
            authority
        );
    }

    // Block numeric private IPs in URL (bypasses DNS resolver since no DNS lookup needed).
    // Use parsed.host() to properly handle bracketed IPv6 addresses (e.g., [::ffff:127.0.0.1]).
    let ip: Option<IpAddr> = match parsed.host() {
        Some(url::Host::Ipv4(v4)) => Some(IpAddr::V4(v4)),
        Some(url::Host::Ipv6(v6)) => Some(IpAddr::V6(v6)),
        _ => None,
    };
    if let Some(ip) = ip
        && is_private_ip(ip)
    {
        anyhow::bail!("blocked: URL targets private IP address ({})", host);
    }

    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_private_ips() {
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("172.16.0.1".parse().unwrap()));
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
        assert!(is_private_ip("169.254.1.1".parse().unwrap()));
        assert!(is_private_ip("0.0.0.0".parse().unwrap()));
        assert!(is_private_ip("100.64.0.1".parse().unwrap())); // CGNAT
        assert!(is_private_ip("::1".parse().unwrap()));

        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip("93.184.216.34".parse().unwrap()));
    }

    #[test]
    fn test_blocked_schemes() {
        assert!(validate_url_scheme("file:///etc/passwd").is_err());
        assert!(validate_url_scheme("ftp://evil.com/file").is_err());
        assert!(validate_url_scheme("gopher://evil.com").is_err());
    }

    #[test]
    fn test_internal_services_blocked() {
        assert!(validate_url_scheme("http://localhost:9011/api").is_err());
        assert!(validate_url_scheme("http://toolgate:9011/describe-url").is_err());
        assert!(validate_url_scheme("http://localhost:9020/extract").is_err());
        assert!(validate_url_scheme("http://localhost:18789/api/secrets").is_err());
    }

    #[test]
    fn test_public_urls_allowed() {
        assert!(validate_url_scheme("https://example.com").is_ok());
        assert!(validate_url_scheme("http://api.github.com/repos").is_ok());
    }

    #[test]
    fn test_cgnat_range() {
        // 100.64.0.0/10 — CGNAT range (100.64.0.0 – 100.127.255.255)
        assert!(is_private_ip("100.64.0.1".parse().unwrap()));
        assert!(is_private_ip("100.127.255.255".parse().unwrap()));
        // 100.128.0.1 is outside CGNAT — should be public
        assert!(!is_private_ip("100.128.0.1".parse().unwrap()));
    }

    #[test]
    fn test_ipv6_private() {
        // fc00::/7 — unique local addresses
        assert!(is_private_ip("fc00::1".parse().unwrap()));
        assert!(is_private_ip("fd00::1".parse().unwrap()));
        // fe80::/10 — link-local
        assert!(is_private_ip("fe80::1".parse().unwrap()));
    }

    #[test]
    fn test_url_without_host() {
        // Completely invalid URLs should produce an error from the parser.
        assert!(validate_url_scheme("not-a-url").is_err());
        assert!(validate_url_scheme("://missing-scheme").is_err());
        assert!(validate_url_scheme("").is_err());
    }

    #[test]
    fn test_default_port_mapping() {
        // http://localhost:80/api — port 80 is the default for HTTP, not in the blocklist → allowed
        assert!(validate_url_scheme("http://localhost:80/api").is_ok());
        // http://localhost:9011 — matches blocklist entry "localhost:9011" → blocked
        assert!(validate_url_scheme("http://localhost:9011").is_err());
    }

    #[test]
    fn test_numeric_private_ip_blocked() {
        // Numeric private IPs should be blocked at URL validation level
        assert!(validate_url_scheme("http://127.0.0.1:2375").is_err());
        assert!(validate_url_scheme("http://127.0.0.1:5432").is_err());
        assert!(validate_url_scheme("http://10.0.0.1:80").is_err());
        assert!(validate_url_scheme("http://192.168.1.1:8080").is_err());
        // Public IPs should be allowed
        assert!(validate_url_scheme("http://8.8.8.8:80").is_ok());
    }

    #[test]
    fn test_is_internal_endpoint() {
        // Internal services should be detected
        assert!(is_internal_endpoint("http://localhost:9011/generate-image"));
        assert!(is_internal_endpoint("http://localhost:9011/describe-url"));
        assert!(is_internal_endpoint("http://localhost:8080/search"));
        assert!(is_internal_endpoint("http://browser-renderer:9020/automation"));
        assert!(is_internal_endpoint("http://searxng:8080/search"));

        // External services should NOT match
        assert!(!is_internal_endpoint("https://api.fal.ai/generate"));
        assert!(!is_internal_endpoint("https://api.openai.com/v1/chat"));
        assert!(!is_internal_endpoint("http://example.com:9011/test"));
    }

    #[test]
    fn test_docker_and_postgres_blocked() {
        assert!(validate_url_scheme("http://localhost:5432").is_err());
        assert!(validate_url_scheme("http://postgres:5432").is_err());
        assert!(validate_url_scheme("http://localhost:2375").is_err());
    }

    #[test]
    fn test_ipv4_mapped_ipv6() {
        // IPv4-mapped IPv6 addresses must be treated as their IPv4 equivalents
        assert!(is_private_ip("::ffff:127.0.0.1".parse().unwrap()));  // loopback
        assert!(is_private_ip("::ffff:10.0.0.1".parse().unwrap()));   // private
        assert!(is_private_ip("::ffff:192.168.1.1".parse().unwrap())); // private
        assert!(is_private_ip("::ffff:172.16.0.1".parse().unwrap()));  // private
        assert!(is_private_ip("::ffff:169.254.1.1".parse().unwrap())); // link-local
        assert!(is_private_ip("::ffff:100.64.0.1".parse().unwrap()));  // CGNAT
        assert!(is_private_ip("::ffff:0.0.0.0".parse().unwrap()));     // unspecified
        // Public IPv4-mapped should be allowed
        assert!(!is_private_ip("::ffff:8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("::ffff:93.184.216.34".parse().unwrap()));
    }

    #[test]
    fn test_numeric_ipv6_private_blocked() {
        // IPv4-mapped IPv6 in URL should be blocked at validation level
        assert!(validate_url_scheme("http://[::ffff:127.0.0.1]:8080").is_err());
        assert!(validate_url_scheme("http://[::ffff:10.0.0.1]").is_err());
        assert!(validate_url_scheme("http://[::1]:8080").is_err());
        // Public IPv6 should be allowed
        assert!(validate_url_scheme("http://[2606:4700:4700::1111]").is_ok());
    }

    // SEC-04 audit (2026-03-30): Tool name validation verified at all entry points:
    // - yaml_tools.rs: api_upsert_yaml_tool validates chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    // - mcp_workspace.rs: save_mcp_entry uses same validation
    // - providers.rs: uses regex ^[a-zA-Z0-9_-]+$
    // - engine.rs: LLM tool names matched by string equality against registry, never used as filesystem paths
    // No gaps found. Path separators (/, \, ..) rejected by the alphanumeric + dash + underscore allowlist.

}
