use crate::errors::OfflineRetrievalError;
use anyhow::{anyhow, Result};
use chrono::prelude::*;
use oxigraph::io::RdfFormat;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, LINK};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub offline: bool,
    pub timeout: Duration,
    pub accept_order: Vec<&'static str>,
    pub extension_candidates: Vec<&'static str>,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            offline: false,
            timeout: Duration::from_secs(30),
            accept_order: vec![
                "text/turtle",
                "application/rdf+xml",
                "application/n-triples",
            ],
            extension_candidates: vec![".ttl", ".rdf", ".owl", "index.ttl", "index.rdf"],
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchResult {
    pub bytes: Vec<u8>,
    pub format: Option<RdfFormat>,
    pub final_url: String,
    pub content_type: Option<String>,
}

fn detect_format(ct: &str) -> Option<RdfFormat> {
    let ct = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match ct.as_str() {
        "text/turtle" | "application/x-turtle" => Some(RdfFormat::Turtle),
        "application/rdf+xml" => Some(RdfFormat::RdfXml),
        "application/n-triples" | "application/ntriples" | "text/plain" => Some(RdfFormat::NTriples),
        _ => None,
    }
}

fn build_accept(accept_order: &[&'static str]) -> String {
    if accept_order.is_empty() {
        return "*/*".to_string();
    }
    let mut parts = Vec::new();
    let mut q = 1.0f32;
    for (i, t) in accept_order.iter().enumerate() {
        parts.push(format!("{t}; q={:.1}", q));
        let next = 1.0f32 - 0.1f32 * (i as f32 + 1.0f32);
        q = if next < 0.1 { 0.1 } else { next };
    }
    parts.push("*/*; q=0.1".to_string());
    parts.join(", ")
}

fn build_extension_candidates(orig: &str, exts: &[&str]) -> Vec<String> {
    let mut cands = Vec::new();
    if orig.ends_with('/') {
        for e in exts {
            cands.push(format!("{orig}{e}"));
        }
        return cands;
    }
    // split path
    let slash_pos = orig.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (prefix, filename) = orig.split_at(slash_pos);
    if let Some(dot) = filename.rfind('.') {
        let stem = &filename[..dot];
        let base = format!("{prefix}{stem}");
        for rep in [".ttl", ".rdf", ".owl"] {
            cands.push(format!("{base}{rep}"));
        }
    } else {
        for rep in [".ttl", ".rdf", ".owl"] {
            cands.push(format!("{orig}{rep}"));
        }
    }
    cands
}

fn parse_link_alternates(headers: &HeaderMap, accept_order: &[&'static str]) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(link_val) = headers.get(LINK) {
        if let Ok(link_str) = link_val.to_str() {
            for part in link_str.split(',') {
                let part = part.trim();
                if !part.contains("rel=\"alternate\"") {
                    continue;
                }
                // Try to extract type and URL
                let has_rdf_type = accept_order
                    .iter()
                    .any(|typ| part.contains(&format!("type=\"{}\"", typ)));
                if !has_rdf_type {
                    continue;
                }
                if let Some(start) = part.find('<') {
                    if let Some(end) = part[start + 1..].find('>') {
                        let url = &part[start + 1..start + 1 + end];
                        out.push(url.to_string());
                    }
                }
            }
        }
    }
    out
}

fn try_get(
    url: &str,
    client: &Client,
    accept: &str,
) -> Result<(Vec<u8>, Option<String>, Option<String>, String, reqwest::StatusCode)> {
    let resp = client.get(url).header(ACCEPT, accept).send()?;
    let status = resp.status();
    let final_url = resp.url().to_string();
    let ct = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());
    let link = resp
        .headers()
        .get(LINK)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());
    let bytes = resp.bytes()?.to_vec();
    Ok((bytes, ct, link, final_url, status))
}

pub fn fetch_rdf(url: &str, opts: &FetchOptions) -> Result<FetchResult> {
    if opts.offline {
        return Err(anyhow!(OfflineRetrievalError {
            file: url.to_string()
        }));
    }
    let client = Client::builder().timeout(opts.timeout).build()?;
    let accept = build_accept(&opts.accept_order);

    // First attempt
    let (bytes, ct, link, final_url, status) = try_get(url, &client, &accept)?;

    // If success and looks RDF by Content-Type, return
    if status.is_success() {
        if let Some(ref cts) = ct {
            if let Some(fmt) = detect_format(cts) {
                return Ok(FetchResult {
                    bytes,
                    format: Some(fmt),
                    final_url,
                    content_type: ct,
                });
            }
        }
        // Unknown or HTML content-type: fall through to alternates with Link header hints
    }

    // Try Link: rel="alternate" first if present
    if let Some(link_header) = link {
        let mut headers = HeaderMap::new();
        headers.insert(
            LINK,
            HeaderValue::from_str(&link_header).unwrap_or(HeaderValue::from_static("")),
        );
        for alt in parse_link_alternates(&headers, &opts.accept_order) {
            let (b2, ct2, _link2, fu2, st2) = try_get(&alt, &client, &accept)?;
            if st2.is_success() {
                let fmt = ct2.as_deref().and_then(detect_format);
                return Ok(FetchResult {
                    bytes: b2,
                    format: fmt,
                    final_url: fu2,
                    content_type: ct2,
                });
            }
        }
    }

    // Status-based or type-based fallbacks
    if !status.is_success() || ct.as_deref().map(|s| s.contains("html")).unwrap_or(true) {
        for candidate in build_extension_candidates(url, &opts.extension_candidates) {
            let (b2, ct2, _link2, fu2, st2) = try_get(&candidate, &client, &accept)?;
            if st2.is_success() {
                let fmt = ct2.as_deref().and_then(detect_format);
                return Ok(FetchResult {
                    bytes: b2,
                    format: fmt,
                    final_url: fu2,
                    content_type: ct2,
                });
            }
        }
    }

    // As a last resort, if the original was successful but with unknown CT, return it.
    if status.is_success() {
        let fmt = ct.as_deref().and_then(detect_format);
        return Ok(FetchResult {
            bytes,
            format: fmt,
            final_url,
            content_type: ct,
        });
    }

    Err(anyhow!(
        "Failed to retrieve RDF from {} (HTTP {}) and fallbacks",
        url,
        status
    ))
}

pub fn head_last_modified(url: &str, opts: &FetchOptions) -> Result<Option<DateTime<Utc>>> {
    if opts.offline {
        return Err(anyhow!(OfflineRetrievalError {
            file: url.to_string()
        }));
    }
    let client = Client::builder().timeout(opts.timeout).build()?;
    let accept = build_accept(&opts.accept_order);
    let resp = client.head(url).header(ACCEPT, accept).send()?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    if let Some(h) = resp.headers().get("Last-Modified") {
        if let Ok(s) = h.to_str() {
            if let Ok(dt) = DateTime::parse_from_rfc2822(s) {
                return Ok(Some(dt.with_timezone(&Utc)));
            }
        }
    }
    Ok(None)
}

pub fn head_exists(url: &str, opts: &FetchOptions) -> Result<bool> {
    if opts.offline {
        return Err(anyhow!(OfflineRetrievalError {
            file: url.to_string()
        }));
    }
    let client = Client::builder().timeout(opts.timeout).build()?;
    let accept = build_accept(&opts.accept_order);
    let resp = client.head(url).header(ACCEPT, accept).send()?;
    Ok(resp.status().is_success())
}
