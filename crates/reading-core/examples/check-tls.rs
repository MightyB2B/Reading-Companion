//! Does the client trust the server's certificate, and refuse without it?
//!
//!     cargo run -p reading-core --example check-tls -- https://host:7878 cert.pem
//!
//! Uses exactly what the desktop application uses — reqwest with rustls and an
//! added root — so a pass here means the application will connect. Windows
//! `curl` cannot answer this: it uses schannel, which ignores `--cacert`, so
//! it reports a failure whether or not the certificate is good.
//!
//! Checks both directions. A client that accepts the server is only half the
//! property; one that accepts it *without* the certificate would mean
//! verification is off and anything on the network could impersonate it.

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| {
        eprintln!("usage: check-tls <https://host:port> [certificate.pem]");
        std::process::exit(2);
    });
    let cert_path = args.next();

    println!("Server: {url}\n");

    // --- Without the certificate. Should fail. ---
    let bare = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("client");

    match bare.get(format!("{url}/api/health")).send().await {
        Ok(r) => {
            if url.starts_with("https") {
                println!("[--] connected WITHOUT the certificate: {}", r.status());
                println!("     Either it is signed by a public authority, or this");
                println!("     machine already trusts it. If neither, verification");
                println!("     is not happening and that is serious.");
            } else {
                println!("[ok] plain HTTP answered: {}", r.status());
            }
        }
        Err(e) => println!("[ok] refused without the certificate: {}", short(&e)),
    }

    // --- With it. Should succeed. ---
    let Some(path) = cert_path else {
        println!("\nNo certificate given, so the trusting half was not checked.");
        return;
    };

    let pem = match std::fs::read(&path) {
        Ok(p) => p,
        Err(e) => {
            println!("\n[--] could not read {path}: {e}");
            std::process::exit(1);
        }
    };

    let cert = match reqwest::Certificate::from_pem(&pem) {
        Ok(c) => c,
        Err(e) => {
            println!("\n[--] {path} is not a readable certificate: {e}");
            std::process::exit(1);
        }
    };

    let trusting = reqwest::Client::builder()
        .add_root_certificate(cert)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("client");

    match trusting.get(format!("{url}/api/health")).send().await {
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            println!("\n[ok] connected WITH the certificate: {status}");
            println!("     {}", body.trim());
            println!("\nThe application will connect to this server.");
        }
        Err(e) => {
            println!("\n[--] still refused with the certificate: {}", short(&e));
            println!("     A self-signed certificate has to be usable as a root,");
            println!("     which means basicConstraints CA:true. Windows'");
            println!("     New-SelfSignedCertificate omits that unless asked.");
            std::process::exit(1);
        }
    }
}

/// reqwest errors nest several layers deep; the innermost is the useful one.
fn short(e: &reqwest::Error) -> String {
    let mut source: &dyn std::error::Error = e;
    let mut message = e.to_string();
    while let Some(inner) = source.source() {
        message = inner.to_string();
        source = inner;
    }
    message
}
