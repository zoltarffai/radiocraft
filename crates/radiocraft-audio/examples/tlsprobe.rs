// Diagnostic: why do some stream hosts reject rustls ClientHellos, and does
// the native-tls (OpenSSL) fallback fix them? Findings (2026-11-02, s3.voscast
// .com:10647, a Shoutcast DNAS host): every rustls variant fails with
// `received fatal alert: HandshakeFailure` because the host offers only
// static-RSA TLS 1.2 suites (openssl negotiates AES256-GCM-SHA384), which
// rustls deliberately does not implement. The OpenSSL connector succeeds, so
// StreamSource falls back to it automatically (feature `openssl-fallback`).
// Run: cargo run --example tlsprobe -p radiocraft-audio -- <stream-url>
use std::sync::Arc;
use std::time::Instant;

use ureq::rustls::{ClientConfig, RootCertStore};

fn probe(label: &str, cfg: Option<ClientConfig>, url: &str) {
    let t0 = Instant::now();
    let mut b = ureq::AgentBuilder::new();
    if let Some(cfg) = cfg {
        b = b.tls_config(Arc::new(cfg));
    }
    let agent = b.build();
    let r = agent.get(url).set("Icy-MetaData", "1").call();
    match r {
        Ok(resp) => println!(
            "{label}: OK {} ct={:?} in {:?}",
            resp.status(),
            resp.header("Content-Type"),
            t0.elapsed()
        ),
        Err(e) => println!("{label}: ERR {e} in {:?}", t0.elapsed()),
    }
}

fn roots() -> RootCertStore {
    let mut r = RootCertStore::empty();
    r.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    r
}

fn main() {
    let url = std::env::args().nth(1).expect("url");

    probe("default", None, &url);

    // 1) default provider but classic key-exchange groups only.
    let mut p = ureq::rustls::crypto::ring::default_provider();
    p.kx_groups = vec![
        ureq::rustls::crypto::ring::kx_group::X25519,
        ureq::rustls::crypto::ring::kx_group::SECP256R1,
    ];
    let cfg = ClientConfig::builder_with_provider(Arc::new(p))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots())
        .with_no_client_auth();
    probe("classic-kx", Some(cfg), &url);

    // 2) TLS 1.2 only (default provider).
    let cfg = ClientConfig::builder_with_provider(Arc::new(
        ureq::rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&ureq::rustls::version::TLS12])
    .unwrap()
    .with_root_certificates(roots())
    .with_no_client_auth();
    probe("tls12-only", Some(cfg), &url);

    // 3) TLS 1.2 only + classic kx groups.
    let mut p = ureq::rustls::crypto::ring::default_provider();
    p.kx_groups = vec![
        ureq::rustls::crypto::ring::kx_group::X25519,
        ureq::rustls::crypto::ring::kx_group::SECP256R1,
    ];
    let cfg = ClientConfig::builder_with_provider(Arc::new(p))
        .with_protocol_versions(&[&ureq::rustls::version::TLS12])
        .unwrap()
        .with_root_certificates(roots())
        .with_no_client_auth();
    probe("tls12+classic-kx", Some(cfg), &url);

    // 4) native-tls / OpenSSL stack.
    let t0 = Instant::now();
    match ureq::native_tls::TlsConnector::builder().build() {
        Ok(c) => {
            let agent = ureq::AgentBuilder::new()
                .tls_connector(std::sync::Arc::new(c))
                .build();
            let r = agent.get(&url).set("Icy-MetaData", "1").call();
            match r {
                Ok(resp) => println!(
                    "native-tls: OK {} ct={:?} in {:?}",
                    resp.status(),
                    resp.header("Content-Type"),
                    t0.elapsed()
                ),
                Err(e) => println!("native-tls: ERR {e} in {:?}", t0.elapsed()),
            }
        }
        Err(e) => println!("native-tls: connector build failed: {e}"),
    }
}
